# Moderation (block / spam / delete) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add user-facing moderation to AIR Note — hard-**block** a sender (mail dropped at the single `receive()` chokepoint before decrypt/archive), **spam**-report a received message (local hide + a signed, replay-safe private abuse report behind a graceful seam), and **delete** from the local diary (one message or a whole conversation).

**Architecture:** One new enforcement insert in `core.receive()` backed by a new DID-keyed `moderation.mjs` store (`~/.air-msg/blocklist.json`, mirroring `contacts.mjs`); a `spam` column + delete helpers on the existing `node:sqlite` archive; thin `core.*Op` wrappers exposed through the CLI and MCP. Block is a **convenience filter, not a security boundary** (the relay doesn't authenticate senders — see the spec's D12). No new crypto/relay/transport logic; the abuse report reuses the proven `attest()` signing pattern.

**Tech Stack:** Node ≥ 22 ESM, `node:sqlite`, `node:test` + `node:assert/strict`. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-06-02-moderation-design.md`

**Repo / cwd for all commands:** `~/air-note/agent-bridge-mcp`

**Test runner note:** `node --test test/` is BROKEN on Node 25 — run a **single file** (`node --test test/<file>.mjs`) per task, and the **bare** `node --test` (auto-discover) for the full suite. Baseline before starting: full suite is **88 passing**.

**Avoiding a circular import:** `core.mjs` imports from `moderation.mjs`. Therefore `moderation.mjs` must **not** import from `core.mjs`. All alias/AIR-id→DID resolution stays in `core.mjs`'s op wrappers (they call `resolveRecipient` then pass a canonical **DID** to `moderation.mjs`). `moderation.mjs` only imports `bridgeHome` from `identity.mjs` (no core dep) and `signRaw`/`jcsCanonical` from `crypto.mjs` (a leaf).

---

## File Structure

| File | Responsibility | Action |
|------|----------------|--------|
| `src/moderation.mjs` | DID-keyed blocklist store + `isBlocked`/`recordBlockedDrops`(batched)/`block`/`unblock`/`listBlocked`; `reportAbuse` signed-report seam | **create** |
| `src/archive.mjs` | + guarded `spam` column migration, `markSpam`, `getReceived`, `deleteMessage`, `deleteConversation`, `includeSpam` read-filter | modify |
| `src/core.mjs` | + block insert & batched tally in `receive()`; `blockOp`/`unblockOp`/`listBlockedOp`/`reportSpamOp`/`deleteOp`; thread `includeSpam` through `recentInbox`/`historyOp` | modify |
| `src/cli.mjs` | + print `envelope_id` in `inbox`/`history`; `block`/`unblock`/`blocked`/`spam`/`delete` subcommands; `--include-spam`; HELP | modify |
| `src/index.mjs` | + `agent_block`/`agent_unblock`/`agent_list_blocked`/`agent_report_spam`/`agent_delete` tools; `include_spam` on `agent_history` | modify |
| `test/moderation.test.mjs` | blocklist store + drop-tally batching + abuse-report seam | **create** |
| `test/archive.test.mjs` | + spam migration/flag/hide + delete tests | modify |
| `test/moderation-integration.test.mjs` | block enforcement in `receive()` + cursor-advance | **create** |
| `README.md` | + moderation section | modify |

---

## Task 0: Surface `envelope_id` in `inbox` / `history` (BLOCKER prerequisite)

`spam <id>` and `delete --message <id>` need an `envelope_id`, but `inbox`/`history` never print one today (only `send` does, `cli.mjs:194`). Show the **full** id (dimmed) — unambiguous to copy-paste, no prefix-resolution code. MCP already returns `envelope_id` in `agent_receive`/`agent_history` JSON, so this task is CLI-only. (No CLI-stdout unit harness exists in this repo — verify by running.)

**Files:**
- Modify: `src/cli.mjs` (the `inbox` and `history` cases)

- [ ] **Step 1: Add the id line to the `inbox` case**

In `src/cli.mjs`, replace the body-print line in the `inbox` case (currently `cli.mjs:208`) so each message also prints its id:

```js
        console.log(`  ${arrow} ${encBadge} ${vrf} ${who}  ${c.dim(m.timestamp)}`);
        console.log(`    ${bodyText(m.body)}`);
        console.log(`    ${c.dim("id " + m.envelope_id)}`);
```

- [ ] **Step 2: Add the id line to the `history` case**

Same addition in the `history` case loop (currently `cli.mjs:224-225`):

```js
        console.log(`  ${arrow} ${encBadge} ${who}  ${c.dim(m.timestamp)}`);
        console.log(`    ${bodyText(m.body)}`);
        console.log(`    ${c.dim("id " + m.envelope_id)}`);
```

- [ ] **Step 3: Verify the full suite still passes (no regression)**

Run: `node --test`
Expected: 88 passing, 0 failing.

- [ ] **Step 4: Manual verify the id is shown**

Run: `node src/cli.mjs history --limit 3`
Expected: each message block ends with a dimmed `id <uuid>` line. (If you have no archived mail, this prints `0 message(s)` — that's fine; the code path is exercised by Step 3's suite + Task 6 op tests.)

- [ ] **Step 5: Commit**

```bash
git add src/cli.mjs
git commit -m "feat(moderation): show envelope_id in inbox/history (drives spam/delete)" \
  -m "Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 1: `moderation.mjs` — blocklist store

DID-keyed JSON store at `~/.air-msg/blocklist.json`, mirroring `contacts.mjs` (0600 file, 0700 dir). Fail-open `isBlocked` (D6). Batched `recordBlockedDrops` (one write per `receive()` call, D3).

**Files:**
- Create: `src/moderation.mjs`
- Test: `test/moderation.test.mjs`

- [ ] **Step 1: Write the failing tests**

Create `test/moderation.test.mjs`:

```js
import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  isBlocked, block, unblock, listBlocked, recordBlockedDrops, loadBlocklist,
} from "../src/moderation.mjs";

const DID_A = "did:wba:agentidentityregistry.org:agents:AIR-AAAA";
const DID_B = "did:wba:agentidentityregistry.org:agents:AIR-BBBB";

let dir;
beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "air-msg-mod-"));
  process.env.AGENT_BRIDGE_HOME = dir;
});
afterEach(() => {
  rmSync(dir, { recursive: true, force: true });
  delete process.env.AGENT_BRIDGE_HOME;
});

test("block then isBlocked is true; unblock removes it", () => {
  assert.equal(isBlocked(DID_A), false);
  const r = block(DID_A, { alias: "bob" });
  assert.equal(r.already, false);
  assert.equal(r.air_id, "AIR-AAAA");
  assert.equal(isBlocked(DID_A), true);
  assert.equal(unblock(DID_A).removed, true);
  assert.equal(isBlocked(DID_A), false);
  assert.equal(unblock(DID_A).removed, false); // idempotent
});

test("block is idempotent and preserves blocked_at", () => {
  const first = block(DID_A);
  const at = loadBlocklist().blocked[DID_A].blocked_at;
  const second = block(DID_A);
  assert.equal(second.already, true);
  assert.equal(loadBlocklist().blocked[DID_A].blocked_at, at); // unchanged
});

test("isBlocked fails OPEN on a corrupt store (D6)", () => {
  writeFileSync(join(dir, "blocklist.json"), "{ this is not json");
  assert.equal(isBlocked(DID_A), false); // never throws → mail is delivered
});

test("recordBlockedDrops batches per-DID counts in a single write", () => {
  block(DID_A);
  block(DID_B);
  recordBlockedDrops(new Map([[DID_A, 3], [DID_B, 1]]));
  const s = loadBlocklist();
  assert.equal(s.blocked[DID_A].drop_count, 3);
  assert.equal(s.blocked[DID_B].drop_count, 1);
  assert.ok(s.blocked[DID_A].last_drop_at);
  recordBlockedDrops(new Map([[DID_A, 2]])); // accumulates
  assert.equal(loadBlocklist().blocked[DID_A].drop_count, 5);
});

test("recordBlockedDrops skips a DID unblocked between check and record", () => {
  recordBlockedDrops(new Map([[DID_A, 9]])); // not blocked → no-op, no throw
  assert.equal(loadBlocklist().blocked[DID_A], undefined);
});

test("listBlocked returns entries with the DID included", () => {
  block(DID_A, { alias: "bob" });
  const list = listBlocked();
  assert.equal(list.length, 1);
  assert.equal(list[0].did, DID_A);
  assert.equal(list[0].alias, "bob");
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `node --test test/moderation.test.mjs`
Expected: FAIL — `Cannot find module '../src/moderation.mjs'`.

- [ ] **Step 3: Implement the store**

Create `src/moderation.mjs` (the `reportAbuse` seam is added in Task 2):

```js
// moderation.mjs — block / report-spam moderation state.
//
// Block is a DID-keyed JSON store at ~/.air-msg/blocklist.json (mode 0600),
// mirroring contacts.mjs. It is enforced at the single core.receive() chokepoint.
//
// IMPORTANT: block is a CONVENIENCE filter, not a security boundary — the relay
// does not authenticate the sender (sender_did = the sender-controlled
// envelope.from). See docs/superpowers/specs/2026-06-02-moderation-design.md (D12).
//
// This module must NOT import core.mjs (core imports this — circular). Callers
// resolve alias/AIR-id → canonical DID and pass a DID in.

import { existsSync, mkdirSync, readFileSync, writeFileSync, chmodSync } from "node:fs";
import { join } from "node:path";
import { bridgeHome } from "./identity.mjs";

const BLOCKLIST_VERSION = 1;
const blocklistPath = () => join(bridgeHome(), "blocklist.json");

/** Extract an AIR id from a DID (local copy — must not depend on core.mjs). */
function airIdFromDid(didOrId) {
  const m = String(didOrId).match(/AIR-[A-Za-z0-9-]+/);
  return m ? m[0] : null;
}

export function loadBlocklist() {
  const p = blocklistPath();
  if (!existsSync(p)) return { version: BLOCKLIST_VERSION, blocked: {} };
  return JSON.parse(readFileSync(p, "utf8"));
}

function saveBlocklist(store) {
  mkdirSync(bridgeHome(), { recursive: true, mode: 0o700 });
  const p = blocklistPath();
  writeFileSync(p, JSON.stringify(store, null, 2), { mode: 0o600 });
  chmodSync(p, 0o600);
}

/** Is this DID blocked? Fails OPEN (returns false) on any read error — a corrupt
 *  blocklist must never silently black-hole all mail (D6). */
export function isBlocked(did) {
  try {
    return !!loadBlocklist().blocked[did];
  } catch {
    return false;
  }
}

/** Block a canonical DID. Idempotent; preserves the original blocked_at. */
export function block(did, { alias = null } = {}) {
  const store = loadBlocklist();
  const prior = store.blocked[did];
  store.blocked[did] = {
    air_id: airIdFromDid(did),
    alias: alias ?? prior?.alias ?? null,
    blocked_at: prior?.blocked_at ?? new Date().toISOString(),
    drop_count: prior?.drop_count ?? 0,
    last_drop_at: prior?.last_drop_at ?? null,
  };
  saveBlocklist(store);
  const e = store.blocked[did];
  return { did, air_id: e.air_id, alias: e.alias, already: !!prior };
}

export function unblock(did) {
  const store = loadBlocklist();
  if (!store.blocked[did]) return { removed: false };
  delete store.blocked[did];
  saveBlocklist(store);
  return { removed: true };
}

export function listBlocked() {
  return Object.entries(loadBlocklist().blocked).map(([did, e]) => ({ did, ...e }));
}

/** Bump per-DID drop tallies in ONE write. countsByDid: Map<did, count>.
 *  Best-effort: a failed tally must never break receive(). Advisory only (D3). */
export function recordBlockedDrops(countsByDid) {
  if (!countsByDid || countsByDid.size === 0) return;
  try {
    const store = loadBlocklist();
    const now = new Date().toISOString();
    for (const [did, n] of countsByDid) {
      const e = store.blocked[did];
      if (!e) continue; // unblocked between the receive-loop check and here
      e.drop_count = (e.drop_count ?? 0) + n;
      e.last_drop_at = now;
    }
    saveBlocklist(store);
  } catch (err) {
    process.stderr.write(`[blocklist] drop-tally write failed: ${err.message ?? err}\n`);
  }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `node --test test/moderation.test.mjs`
Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add src/moderation.mjs test/moderation.test.mjs
git commit -m "feat(moderation): blocklist store (block/unblock/isBlocked + batched drop-tally)" \
  -m "Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: `moderation.mjs` — `reportAbuse` seam

A signed, replay-safe private abuse report (spec §7), behind a graceful seam (D9): always build+sign; any POST failure degrades to `{reported:false}` and never throws. Refuses self-report. Reuses the `attest()` signing pattern (`core.mjs:353-361`).

**Files:**
- Modify: `src/moderation.mjs`
- Test: `test/moderation.test.mjs`

- [ ] **Step 1: Write the failing tests**

Append to `test/moderation.test.mjs` (and extend the import line at the top to add `reportAbuse`):

```js
// add `reportAbuse` to the existing import from ../src/moderation.mjs
import { generateIdentity } from "../src/crypto.mjs";

const SUBJECT_DID = "did:wba:agentidentityregistry.org:agents:AIR-BAD0";
function fakeIdentity() {
  const k = generateIdentity(); // fresh Ed25519
  return {
    air_id: "AIR-ME00", air_url: "http://air.test",
    agent_secret: "s3cret", privateKey: k.privateKey,
  };
}

test("reportAbuse posts a signed, replay-keyed report and returns reported:true on 2xx", async () => {
  const real = global.fetch;
  let captured;
  global.fetch = async (url, opts) => {
    captured = { url: String(url), opts };
    return { ok: true, status: 200, json: async () => ({ status: "received" }) };
  };
  try {
    const r = await reportAbuse({ identity: fakeIdentity(), subjectDid: SUBJECT_DID });
    assert.equal(r.reported, true);
    assert.match(captured.url, /\/api\/v1\/agents\/AIR-BAD0\/abuse-reports$/);
    assert.equal(captured.opts.headers["X-Agent-Secret"], "s3cret");
    const body = JSON.parse(captured.opts.body);
    assert.equal(body.version, 1);
    assert.equal(body.report_type, "spam");
    assert.equal(body.reporter_air_id, "AIR-ME00");
    assert.equal(body.subject_air_id, "AIR-BAD0");
    assert.ok(body.report_id && body.reported_at && body.signature_multibase);
  } finally {
    global.fetch = real;
  }
});

test("reportAbuse degrades to reported:false on HTTP error and on network throw (never throws)", async () => {
  const real = global.fetch;
  try {
    global.fetch = async () => ({ ok: false, status: 404, text: async () => "no route" });
    const r404 = await reportAbuse({ identity: fakeIdentity(), subjectDid: SUBJECT_DID });
    assert.equal(r404.reported, false);

    global.fetch = async () => { throw new Error("ECONNREFUSED"); };
    const rNet = await reportAbuse({ identity: fakeIdentity(), subjectDid: SUBJECT_DID });
    assert.equal(rNet.reported, false);
  } finally {
    global.fetch = real;
  }
});

test("reportAbuse refuses to report yourself (no fetch)", async () => {
  const real = global.fetch;
  let called = false;
  global.fetch = async () => { called = true; return { ok: true, status: 200, json: async () => ({}) }; };
  try {
    const id = fakeIdentity();
    const selfDid = "did:wba:agentidentityregistry.org:agents:" + id.air_id;
    const r = await reportAbuse({ identity: id, subjectDid: selfDid });
    assert.equal(r.reported, false);
    assert.equal(called, false);
  } finally {
    global.fetch = real;
  }
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `node --test test/moderation.test.mjs`
Expected: FAIL — `reportAbuse` is not exported.

- [ ] **Step 3: Implement `reportAbuse`**

Add to the top imports of `src/moderation.mjs`:

```js
import { randomUUID } from "node:crypto";
import { signRaw, jcsCanonical } from "./crypto.mjs";
```

Add the constant near `BLOCKLIST_VERSION`:

```js
const ABUSE_REPORT_VERSION = 1;
```

Append the function:

```js
/** Spec §7 seam: build + sign a private abuse report and POST it. Always best-effort —
 *  any failure returns {reported:false} and never throws (the local spam-hide already
 *  applied). report_id + version make the signed report replay-safe + versionable. */
export async function reportAbuse({ identity, subjectDid, report_type = "spam",
  log = (s) => process.stderr.write(s + "\n") }) {
  const subject_air_id = airIdFromDid(subjectDid);
  if (!subject_air_id) return { reported: false, reason: "no AIR id in subject" };
  if (subject_air_id === identity.air_id) return { reported: false, reason: "cannot report yourself" };

  const payload = {
    report_id: randomUUID(),
    version: ABUSE_REPORT_VERSION,
    reporter_air_id: identity.air_id,
    subject_air_id,
    report_type,
    reported_at: new Date().toISOString(),
  };
  const signature_multibase = signRaw(Buffer.from(jcsCanonical(payload), "utf8"), identity.privateKey);

  try {
    const resp = await fetch(`${identity.air_url}/api/v1/agents/${subject_air_id}/abuse-reports`, {
      method: "POST",
      headers: { "content-type": "application/json", "X-Agent-Secret": identity.agent_secret },
      body: JSON.stringify({ ...payload, signature_multibase }),
    });
    if (!resp.ok) {
      log(`[abuse-report] ${subject_air_id} → HTTP ${resp.status} (kept local hide)`);
      return { reported: false, reason: `HTTP ${resp.status}` };
    }
    return { reported: true };
  } catch (e) {
    log(`[abuse-report] ${subject_air_id} → ${e.message ?? e} (kept local hide)`);
    return { reported: false, reason: String(e.message ?? e) };
  }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `node --test test/moderation.test.mjs`
Expected: PASS (9 tests).

- [ ] **Step 5: Commit**

```bash
git add src/moderation.mjs test/moderation.test.mjs
git commit -m "feat(moderation): reportAbuse seam (signed replay-safe private report, degrades on failure)" \
  -m "Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: `archive.mjs` — spam column, `markSpam`, `getReceived`, spam-hidden reads

Idempotent `spam` column migration; `markSpam` flips the **received** row only; `history`/`recentForInbox` exclude spam by default (`includeSpam` reveals); `getReceived` fetches a received row for `reportSpamOp`; `parseRow` surfaces `spam` so a reveal view can badge it.

**Files:**
- Modify: `src/archive.mjs`
- Test: `test/archive.test.mjs`

- [ ] **Step 1: Write the failing tests**

Append to `test/archive.test.mjs` (extend the import to add `markSpam, getReceived, recentForInbox, openArchive`):

```js
// extend the import from ../src/archive.mjs with: markSpam, getReceived, recentForInbox, openArchive

test("spam column migration is idempotent", () => {
  openArchive(); // first open runs the migration
  closeArchive();
  openArchive(); // second open must not throw
  const cols = openArchive().prepare(`PRAGMA table_info(messages)`).all().map((c) => c.name);
  assert.ok(cols.includes("spam"));
});

test("markSpam flips only the received row; default reads hide it, includeSpam reveals", () => {
  archiveMessage(rec({ envelope_id: "spam1", direction: "received" }));
  assert.equal(history().length, 1);
  assert.equal(markSpam("spam1").updated, 1);
  assert.equal(history().length, 0);                       // hidden by default
  assert.equal(history({ includeSpam: true }).length, 1);  // revealed
  assert.equal(history({ includeSpam: true })[0].spam, true);
  assert.equal(recentForInbox(20).length, 0);              // inbox hides spam
});

test("markSpam does not touch a 'sent' row of the same envelope_id", () => {
  const base = rec({ envelope_id: "self9", peer_did: "did:me", from_did: "did:me", to_did: "did:me" });
  archiveMessage({ ...base, direction: "sent" });
  archiveMessage({ ...base, direction: "received", relay_seq: 9 });
  markSpam("self9");
  assert.equal(history({ includeSpam: true }).find((m) => m.direction === "sent").spam, false);
  assert.equal(history({ includeSpam: true }).find((m) => m.direction === "received").spam, true);
});

test("getReceived returns the received row or null", () => {
  archiveMessage(rec({ envelope_id: "g1", peer_did: "did:P" }));
  assert.equal(getReceived("g1").peer_did, "did:P");
  assert.equal(getReceived("nope"), null);
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `node --test test/archive.test.mjs`
Expected: FAIL — `markSpam`/`getReceived` not exported (and `includeSpam`/`spam` not yet handled).

- [ ] **Step 3: Add the migration in `openArchive()`**

In `src/archive.mjs`, inside `openArchive()`, after the `for (const stmt of SCHEMA) db.prepare(stmt).run();` line and before `chmodSync`, add:

```js
  // Migration: add the moderation `spam` flag if an older DB predates it. Guarded by
  // PRAGMA so it runs at most once; ADD COLUMN ... NOT NULL DEFAULT 0 is legal in node:sqlite.
  const cols = db.prepare(`PRAGMA table_info(messages)`).all().map((col) => col.name);
  if (!cols.includes("spam")) {
    db.prepare(`ALTER TABLE messages ADD COLUMN spam INTEGER NOT NULL DEFAULT 0`).run();
  }
```

- [ ] **Step 4: Surface `spam` in `parseRow`**

In `parseRow`, add `spam: !!r.spam,` to the returned object (alongside `verified`):

```js
    body: JSON.parse(r.body_json), encrypted: !!r.encrypted, verified: !!r.verified,
    spam: !!r.spam,
    relay_seq: r.relay_seq ?? undefined, archived_at: r.archived_at,
```

- [ ] **Step 5: Add the `includeSpam` filter to `history` + `recentForInbox`**

Change the `history` signature + add the filter:

```js
export function history({ peer, thread, before, limit = 50, includeSpam = false } = {}) {
  const db = openArchive();
  const where = [];
  const params = [];
  if (peer)   { where.push("peer_did = ?"); params.push(peer); }
  if (thread) { where.push("thread_id = ?"); params.push(thread); }
  if (before) { where.push("timestamp < ?"); params.push(before); }
  if (!includeSpam) { where.push("spam = 0"); }
  const clause = where.length ? `WHERE ${where.join(" AND ")}` : "";
  params.push(limit);
  return db.prepare(
    `SELECT * FROM messages ${clause} ORDER BY timestamp DESC, archived_at DESC LIMIT ?`
  ).all(...params).map(parseRow);
}
```

Change `recentForInbox` to thread the flag:

```js
export function recentForInbox(limit = 20, { includeSpam = false } = {}) {
  return history({ limit, includeSpam });
}
```

- [ ] **Step 6: Add `markSpam` + `getReceived`**

Add near the other query helpers:

```js
/** Flag a RECEIVED message as spam (hidden from default reads). Idempotent. */
export function markSpam(envelope_id) {
  const db = openArchive();
  const res = db.prepare(
    `UPDATE messages SET spam = 1 WHERE envelope_id = ? AND direction = 'received'`
  ).run(envelope_id);
  return { updated: res.changes };
}

/** Fetch the received row for an envelope_id (used to find the spam subject), or null. */
export function getReceived(envelope_id) {
  const db = openArchive();
  const r = db.prepare(
    `SELECT * FROM messages WHERE envelope_id = ? AND direction = 'received'`
  ).get(envelope_id);
  return r ? parseRow(r) : null;
}
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `node --test test/archive.test.mjs`
Expected: PASS (existing 7 + 4 new = 11).

- [ ] **Step 8: Commit**

```bash
git add src/archive.mjs test/archive.test.mjs
git commit -m "feat(moderation): archive spam flag (guarded migration, markSpam, getReceived, hidden-by-default reads)" \
  -m "Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: `archive.mjs` — `deleteMessage` + `deleteConversation`

Local-only deletes. `deleteMessage` removes all rows for an id (both directions of a self-message); `deleteConversation` removes the whole two-way history with a peer.

**Files:**
- Modify: `src/archive.mjs`
- Test: `test/archive.test.mjs`

- [ ] **Step 1: Write the failing tests**

Append to `test/archive.test.mjs` (extend the import with `deleteMessage, deleteConversation`):

```js
test("deleteMessage removes all rows for an envelope_id and reports the count", () => {
  const base = rec({ envelope_id: "d1", peer_did: "did:me", from_did: "did:me", to_did: "did:me" });
  archiveMessage({ ...base, direction: "sent" });
  archiveMessage({ ...base, direction: "received", relay_seq: 2 });
  assert.equal(deleteMessage("d1").deleted, 2);
  assert.equal(history({ includeSpam: true }).length, 0);
  assert.equal(deleteMessage("d1").deleted, 0); // already gone
});

test("deleteConversation removes the whole two-way thread for a peer", () => {
  archiveMessage(rec({ envelope_id: "r1", direction: "received", peer_did: "did:P" }));
  archiveMessage(rec({ envelope_id: "s1", direction: "sent", peer_did: "did:P" }));
  archiveMessage(rec({ envelope_id: "x1", direction: "received", peer_did: "did:OTHER" }));
  assert.equal(deleteConversation("did:P").deleted, 2);
  const left = history({ includeSpam: true });
  assert.equal(left.length, 1);
  assert.equal(left[0].peer_did, "did:OTHER");
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `node --test test/archive.test.mjs`
Expected: FAIL — `deleteMessage`/`deleteConversation` not exported.

- [ ] **Step 3: Implement the delete helpers**

Add to `src/archive.mjs`:

```js
/** Delete a message entirely from the local diary (all directions of its id). */
export function deleteMessage(envelope_id) {
  const db = openArchive();
  const res = db.prepare(`DELETE FROM messages WHERE envelope_id = ?`).run(envelope_id);
  return { deleted: res.changes };
}

/** Delete the whole two-way conversation with a peer (received + sent rows). */
export function deleteConversation(peer_did) {
  const db = openArchive();
  const res = db.prepare(`DELETE FROM messages WHERE peer_did = ?`).run(peer_did);
  return { deleted: res.changes };
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `node --test test/archive.test.mjs`
Expected: PASS (13 total).

- [ ] **Step 5: Commit**

```bash
git add src/archive.mjs test/archive.test.mjs
git commit -m "feat(moderation): archive deleteMessage + deleteConversation (local diary only)" \
  -m "Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: `core.receive()` — block enforcement + batched tally

Drop a blocked sender at the top of the receive loop, before any parse/verify/decode/archive. Accumulate per-DID drop counts and write the tally **once** after the loop. The cursor advance is unchanged, so blocked messages still advance it (verified at `core.mjs:331`).

**Files:**
- Modify: `src/core.mjs`
- Test: `test/moderation-integration.test.mjs` (create)

- [ ] **Step 1: Write the failing test**

Create `test/moderation-integration.test.mjs`:

```js
import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { receive, buildOutboundEnvelope } from "../src/core.mjs";
import { history, closeArchive, getCursor } from "../src/archive.mjs";
import { block, loadBlocklist } from "../src/moderation.mjs";
import { generateIdentity, pubKeyMultibase } from "../src/crypto.mjs";

const ME_SEED = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";
const PEER_SEED = "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6f9";
const ME_DID = "did:wba:agentidentityregistry.org:agents:AIR-ME";
const BLOCKED_DID = "did:wba:agentidentityregistry.org:agents:AIR-BLOCKED";
const OK_DID = "did:wba:agentidentityregistry.org:agents:AIR-OK";

function seedIdentity(dir) {
  writeFileSync(join(dir, "identity.json"), JSON.stringify({
    version: 1, name: "test", air_id: "AIR-ME", did: ME_DID, seed_hex: ME_SEED,
    public_key_base64url: "", public_key_multibase: "", agent_secret: "secret",
    relay_url: "http://relay.test", air_url: "http://air.test",
    service_endpoint_published: true, created_at: "2026-06-01T00:00:00Z",
  }), { mode: 0o600 });
}

let dir, realFetch;
beforeEach(() => {
  closeArchive();
  dir = mkdtempSync(join(tmpdir(), "air-msg-modint-"));
  process.env.AGENT_BRIDGE_HOME = dir;
  seedIdentity(dir);
  realFetch = global.fetch;
});
afterEach(() => {
  global.fetch = realFetch;
  closeArchive();
  rmSync(dir, { recursive: true, force: true });
  delete process.env.AGENT_BRIDGE_HOME;
});

test("receive() hard-drops a blocked sender, archives the rest, advances cursor past both, tallies", async () => {
  const me = generateIdentity(ME_SEED);
  const peer = generateIdentity(PEER_SEED);
  // Two envelopes whose claimed sender (the relay's sender_did) differs.
  const env = (did) => buildOutboundEnvelope({
    identity: { did, privateKey: peer.privateKey }, recipientDid: ME_DID,
    recipientEd25519Pub: me.rawPublicKey, body: "hi",
  });
  const blockedEnv = env(BLOCKED_DID);
  const okEnv = env(OK_DID);
  const b64 = (e) => Buffer.from(JSON.stringify(e)).toString("base64");

  block(BLOCKED_DID);

  global.fetch = async (url) => {
    const u = String(url);
    if (u.includes("/pull/")) {
      const since = Number(new URL(u).searchParams.get("since"));
      const messages = since < 7 ? [
        { envelope_b64: b64(blockedEnv), sender_did: BLOCKED_DID, envelope_id: blockedEnv.id, seq: 3, queued_at: 1717200000 },
        { envelope_b64: b64(okEnv), sender_did: OK_DID, envelope_id: okEnv.id, seq: 7, queued_at: 1717200001 },
      ] : [];
      return { ok: true, json: async () => ({ messages, cursor: 0, has_more: false }) };
    }
    if (u.includes("/did-document")) {
      // pub key resolves for OK (verify true); irrelevant for the blocked one (never reached).
      return { ok: true, json: async () => ({ verificationMethod: [{ publicKeyMultibase: pubKeyMultibase(peer.rawPublicKey) }] }) };
    }
    throw new Error(`unexpected fetch: ${u}`);
  };

  const r = await receive();
  assert.equal(r.count, 1);                       // only the non-blocked message returned
  assert.equal(r.messages[0].from, OK_DID);
  const rows = history({ includeSpam: true });
  assert.equal(rows.length, 1);                   // blocked one NOT archived
  assert.equal(rows[0].peer_did, OK_DID);
  assert.equal(getCursor(), 7);                   // advanced past BOTH (max seq incl. blocked)
  assert.equal(loadBlocklist().blocked[BLOCKED_DID].drop_count, 1); // tally bumped
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `node --test test/moderation-integration.test.mjs`
Expected: FAIL — the blocked message is currently archived (no enforcement yet): `r.count` is 2 and `getCursor` assertion or `rows.length` fails.

- [ ] **Step 3: Add the import to `core.mjs`**

In `src/core.mjs`, after the `contacts.mjs` import block (around line 31), add:

```js
import { isBlocked, recordBlockedDrops } from "./moderation.mjs";
```

- [ ] **Step 4: Add the block insert + batched tally in `receive()`**

In `receive()`, change the message loop. Add the `drops` map before the loop and the block check + tally:

```js
  const messages = [];
  const drops = new Map(); // batched block drop-tally (one write after the loop)
  for (const m of batch.messages) {
    if (isBlocked(m.sender_did)) {        // hard drop BEFORE decode/verify/archive (D2/D12)
      drops.set(m.sender_did, (drops.get(m.sender_did) ?? 0) + 1);
      continue;
    }
    let envelope = null;
    // ... rest of the existing loop body unchanged ...
```

Then, after the loop closes and before the cursor-advance block (`core.mjs:326`), add:

```js
  if (drops.size) recordBlockedDrops(drops);
  // Advance the cursor past all delivered messages even if some archive writes failed:
  // ... existing cursor-advance block unchanged ...
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `node --test test/moderation-integration.test.mjs`
Expected: PASS.

- [ ] **Step 6: Run the full suite (no regression in receive/watch/channel/bridge consumers)**

Run: `node --test`
Expected: all passing (88 baseline + the new moderation tests).

- [ ] **Step 7: Commit**

```bash
git add src/core.mjs test/moderation-integration.test.mjs
git commit -m "feat(moderation): enforce block at the receive() chokepoint (hard drop + batched tally)" \
  -m "Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: `core.mjs` — op wrappers

Thin wrappers that resolve `peer`→DID (`resolveRecipient`) and delegate to `moderation.mjs` / `archive.mjs`. `reportSpamOp` looks up the received row's subject; `deleteOp` is confirm-gated.

**Files:**
- Modify: `src/core.mjs`
- Test: `test/moderation-integration.test.mjs`

- [ ] **Step 1: Write the failing tests**

Append to `test/moderation-integration.test.mjs` (extend the `core.mjs` import with `blockOp, unblockOp, listBlockedOp, reportSpamOp, deleteOp` and the `archive.mjs` import with `archiveMessage`):

```js
test("blockOp resolves an alias-less DID and lists it; unblockOp removes it", async () => {
  const r = await blockOp({ peer: BLOCKED_DID });
  assert.equal(r.status, "blocked");
  const list = listBlockedOp();
  assert.equal(list.count, 1);
  assert.equal(list.blocked[0].did, BLOCKED_DID);
  assert.equal((await unblockOp({ peer: BLOCKED_DID })).status, "unblocked");
  assert.equal(listBlockedOp().count, 0);
});

test("reportSpamOp errors on an unknown id, hides + reports on a known received row", async () => {
  await assert.rejects(() => reportSpamOp({ envelope_id: "missing" }), /no received message/);

  archiveMessage({
    envelope_id: "junk1", direction: "received", thread_id: "t", peer_did: OK_DID,
    from_did: OK_DID, to_did: ME_DID, timestamp: "2026-06-01T00:00:00Z",
    body: { type: "text", text: "spam" }, encrypted: false, verified: true, relay_seq: 1,
  });
  global.fetch = async () => ({ ok: true, status: 200, json: async () => ({}) }); // abuse endpoint up
  const r = await reportSpamOp({ envelope_id: "junk1" });
  assert.equal(r.hidden, true);
  assert.equal(r.reported, true);
  assert.equal(r.subject, "AIR-OK");
  assert.equal(history().length, 0);                      // hidden from default inbox
  assert.equal(history({ includeSpam: true }).length, 1); // still there, flagged
});

test("deleteOp refuses without confirm and requires exactly one selector", async () => {
  await assert.rejects(() => deleteOp({ envelope_id: "x" }), /confirm/);
  await assert.rejects(() => deleteOp({ envelope_id: "x", peer: "y", confirm: true }), /exactly one/);
});

test("deleteOp deletes a conversation when confirmed", async () => {
  archiveMessage({
    envelope_id: "c1", direction: "received", thread_id: "t", peer_did: OK_DID,
    from_did: OK_DID, to_did: ME_DID, timestamp: "2026-06-01T00:00:00Z",
    body: { type: "text", text: "hi" }, encrypted: false, verified: true, relay_seq: 1,
  });
  const r = await deleteOp({ peer: OK_DID, confirm: true });
  assert.equal(r.deleted, 1);
  assert.equal(r.scope, "conversation");
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `node --test test/moderation-integration.test.mjs`
Expected: FAIL — the `*Op` functions are not exported.

- [ ] **Step 3: Implement the op wrappers**

Extend the `moderation.mjs` import in `src/core.mjs` to include the store + seam functions:

```js
import {
  isBlocked, recordBlockedDrops,
  block as blockDid, unblock as unblockDid, listBlocked, reportAbuse,
} from "./moderation.mjs";
```

Extend the `archive.mjs` import to add the new helpers:

```js
import {
  archiveMessage, getCursor, setCursor, history as archiveHistory, recentForInbox,
  markSpam, getReceived, deleteMessage, deleteConversation,
} from "./archive.mjs";
```

Add the ops (near `attest`/`addContactOp`):

```js
export async function blockOp({ peer }) {
  if (!peer) throw new Error("peer (DID, AIR id, or alias) is required");
  const did = resolveRecipient(peer);
  const contact = getContactByDid(did);
  const r = blockDid(did, { alias: contact?.alias ?? null });
  return { status: r.already ? "already blocked" : "blocked", did: r.did, air_id: r.air_id, alias: r.alias };
}

export async function unblockOp({ peer }) {
  if (!peer) throw new Error("peer (DID, AIR id, or alias) is required");
  const did = resolveRecipient(peer);
  const { removed } = unblockDid(did);
  return { status: removed ? "unblocked" : "not blocked", did };
}

export function listBlockedOp() {
  const blocked = listBlocked();
  return { count: blocked.length, blocked };
}

export async function reportSpamOp({ envelope_id }) {
  if (!envelope_id) throw new Error("envelope_id is required (copy it from inbox/history)");
  const row = getReceived(envelope_id);
  if (!row) throw new Error(`no received message with envelope_id ${envelope_id} in your diary`);
  const identity = await ensureIdentity();
  const report = await reportAbuse({ identity, subjectDid: row.peer_did });
  markSpam(envelope_id);
  return {
    hidden: true,
    reported: report.reported,
    subject: airIdFromDid(row.peer_did),
    ...(report.reason ? { reason: report.reason } : {}),
  };
}

export async function deleteOp({ envelope_id, peer, confirm = false } = {}) {
  if (!!envelope_id === !!peer) throw new Error("pass exactly one of envelope_id or peer");
  if (confirm !== true) throw new Error("refusing to delete without confirm:true (CLI: pass --yes)");
  if (envelope_id) {
    const { deleted } = deleteMessage(envelope_id);
    return { deleted, scope: "message", envelope_id };
  }
  const did = resolveRecipient(peer);
  const { deleted } = deleteConversation(did);
  return { deleted, scope: "conversation", peer: did };
}
```

Thread `includeSpam` through the read ops — change `recentInbox` and `historyOp`:

```js
export function historyOp({ peer, thread, limit = 50, before, includeSpam = false } = {}) {
  const resolvedPeer = peer ? resolveRecipient(peer) : undefined;
  const messages = archiveHistory({ peer: resolvedPeer, thread, limit, before, includeSpam });
  return { count: messages.length, messages, resolvedPeer };
}

export function recentInbox({ limit = 20, includeSpam = false } = {}) {
  const messages = recentForInbox(limit, { includeSpam });
  return { count: messages.length, messages };
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `node --test test/moderation-integration.test.mjs`
Expected: PASS.

- [ ] **Step 5: Run the full suite**

Run: `node --test`
Expected: all passing.

- [ ] **Step 6: Commit**

```bash
git add src/core.mjs test/moderation-integration.test.mjs
git commit -m "feat(moderation): core ops (block/unblock/listBlocked/reportSpam/delete) + includeSpam reads" \
  -m "Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: Surfaces — CLI subcommands + MCP tools

Wire the ops to both front-ends. (No CLI/MCP unit harness exists in this repo — verify with the full suite + a manual smoke.)

**Files:**
- Modify: `src/cli.mjs`, `src/index.mjs`

- [ ] **Step 1: Add `--include-spam` to the `inbox` + `history` CLI reads**

In `src/cli.mjs`, the `inbox` case — change the `recentInbox` call:

```js
      const { messages } = core.recentInbox({
        limit: flags.limit ? Number(flags.limit) : 20,
        includeSpam: !!flags["include-spam"],
      });
```

The `history` case — add `includeSpam` to the `historyOp` call:

```js
      const result = core.historyOp({
        peer: flags.with, thread: flags.thread,
        limit: flags.limit ? Number(flags.limit) : 50,
        includeSpam: !!flags["include-spam"],
      });
```

- [ ] **Step 2: Add the five new CLI subcommands**

In the `switch (cmd)` in `src/cli.mjs`, add these cases (e.g. after the `attest` case):

```js
    case "block": {
      const [peer] = positionals;
      if (!peer) { console.error("usage: air-msg block <did|air-id|alias>"); process.exit(1); }
      const r = await core.blockOp({ peer });
      console.log(`${c.green("✓ " + r.status)} ${c.bold(r.alias || r.air_id || r.did)}`);
      console.log(c.dim("  their mail is now dropped on arrival. NOTE: unblocking later cannot recover messages dropped while blocked, and a sender who forges a different identity can still get through (block is a convenience filter, not a security wall)."));
      break;
    }
    case "unblock": {
      const [peer] = positionals;
      if (!peer) { console.error("usage: air-msg unblock <did|air-id|alias>"); process.exit(1); }
      const r = await core.unblockOp({ peer });
      console.log(`${c.green("✓ " + r.status)} ${c.dim(r.did)}`);
      break;
    }
    case "blocked": {
      const r = core.listBlockedOp();
      if (r.count === 0) { console.log(c.dim("(no blocked senders)")); break; }
      for (const b of r.blocked) {
        const tally = b.drop_count
          ? c.dim(`  ${b.drop_count} dropped${b.last_drop_at ? ", last " + b.last_drop_at : ""}`)
          : "";
        console.log(`${c.red("⊘")} ${c.bold(b.alias || b.air_id)}  ${c.dim(b.did)}${tally}`);
      }
      console.log(c.dim("  (tallies are advisory — a forged sender can evade or inflate them)"));
      break;
    }
    case "spam": {
      const [envelope_id] = positionals;
      if (!envelope_id) { console.error("usage: air-msg spam <envelope-id>   (copy the id from inbox/history)"); process.exit(1); }
      const r = await core.reportSpamOp({ envelope_id });
      const report = r.reported
        ? c.green("reported")
        : c.yellow("local-only" + (r.reason ? ` (${r.reason})` : ""));
      console.log(`${c.green("✓ hidden")} ${c.dim("from " + r.subject)} · ${report}`);
      break;
    }
    case "delete": {
      if (!flags.yes) { console.error("refusing to delete without --yes (this permanently erases local diary rows)"); process.exit(1); }
      let r;
      if (flags.message) r = await core.deleteOp({ envelope_id: flags.message, confirm: true });
      else if (flags.with) r = await core.deleteOp({ peer: flags.with, confirm: true });
      else { console.error("usage: air-msg delete --message <envelope-id> | --with <peer>  --yes"); process.exit(1); }
      const tgt = r.scope === "message" ? r.envelope_id : "conversation with " + r.peer;
      console.log(`${c.green("✓ deleted")} ${r.deleted} row(s)  ${c.dim(tgt)}`);
      break;
    }
```

- [ ] **Step 3: Update the HELP text**

In the `HELP` template literal in `src/cli.mjs`, add these lines under the command list (after the `attest` line):

```
  air-msg block <to>                     Drop all mail from a sender (convenience filter)
  air-msg unblock <to>                   Remove a sender from the blocklist
  air-msg blocked                        List blocked senders + drop tallies
  air-msg spam <envelope-id>             Hide a junk message + report it to AIR (private)
  air-msg delete --message <id> --yes    Delete one message from your local diary
  air-msg delete --with <to> --yes       Delete a whole conversation from your local diary
  inbox/history also accept --include-spam to reveal hidden spam.
```

- [ ] **Step 4: Register the five MCP tools**

In `src/index.mjs`, add to the `TOOLS` array:

```js
  {
    name: "agent_block",
    description: "Block a sender (by DID, AIR id, or alias): their mail is dropped on arrival, never surfaced. Convenience filter — a sender who forges a different identity still arrives unverified.",
    inputSchema: { type: "object", properties: { peer: { type: "string", description: "DID, AIR id, or contact alias to block." } }, required: ["peer"] },
  },
  {
    name: "agent_unblock",
    description: "Remove a sender from your blocklist so their mail is delivered again. Cannot recover mail dropped while blocked.",
    inputSchema: { type: "object", properties: { peer: { type: "string", description: "DID, AIR id, or contact alias to unblock." } }, required: ["peer"] },
  },
  {
    name: "agent_list_blocked",
    description: "List blocked senders with an advisory drop tally (count of dropped attempts; spoofable).",
    inputSchema: { type: "object", properties: {} },
  },
  {
    name: "agent_report_spam",
    description: "Mark a received message as spam: hide it from your inbox AND send a signed private abuse report to AIR (best-effort; hides locally even if the report can't be sent). Needs the message's envelope_id (from agent_receive/agent_history).",
    inputSchema: { type: "object", properties: { envelope_id: { type: "string", description: "envelope_id of the received message to report." } }, required: ["envelope_id"] },
  },
  {
    name: "agent_delete",
    description: "Delete from your LOCAL diary only (the relay cannot unsend). Pass exactly one of envelope_id (one message) or peer (a whole conversation). confirm must be true.",
    inputSchema: {
      type: "object",
      properties: {
        envelope_id: { type: "string", description: "Delete a single message by id." },
        peer: { type: "string", description: "Delete a whole conversation (DID, AIR id, or alias)." },
        confirm: { type: "boolean", description: "Must be true — guards against accidental deletion." },
      },
      required: ["confirm"],
    },
  },
```

Add to the `HANDLERS` map:

```js
  agent_block: (a) => core.blockOp(a),
  agent_unblock: (a) => core.unblockOp(a),
  agent_list_blocked: () => core.listBlockedOp(),
  agent_report_spam: (a) => core.reportSpamOp(a),
  agent_delete: (a) => core.deleteOp(a),
```

Add `include_spam` passthrough to the existing `agent_history` — change its handler and add the property. Handler:

```js
  agent_history: (a) => core.historyOp({ ...a, includeSpam: a.include_spam }),
```

And add to `agent_history`'s `inputSchema.properties`:

```js
        include_spam: { type: "boolean", description: "Include messages you marked as spam (default false)." },
```

- [ ] **Step 5: Verify the MCP server lists all 16 tools and boots**

Run: `printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | node src/index.mjs 2>/dev/null | node -e "let s='';process.stdin.on('data',d=>s+=d).on('end',()=>{const t=JSON.parse(s.split('\n').filter(Boolean).pop()).result.tools.map(x=>x.name);console.log(t.length, t.join(','));})"`
Expected: `16` and the list includes `agent_block,agent_unblock,agent_list_blocked,agent_report_spam,agent_delete`.

- [ ] **Step 6: Manual CLI smoke**

Run:
```bash
node src/cli.mjs block AIR-TESTBLOCK1
node src/cli.mjs blocked
node src/cli.mjs unblock AIR-TESTBLOCK1
node src/cli.mjs blocked
```
Expected: block prints `✓ blocked AIR-TESTBLOCK1` + the caveat; `blocked` lists it; after unblock, `(no blocked senders)`. (Uses your real `~/.air-msg` — `AIR-TESTBLOCK1` is a throwaway id; the final state is clean.)

- [ ] **Step 7: Run the full suite**

Run: `node --test`
Expected: all passing.

- [ ] **Step 8: Commit**

```bash
git add src/cli.mjs src/index.mjs
git commit -m "feat(moderation): CLI subcommands + MCP tools for block/spam/delete" \
  -m "Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: README + final verification

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Add a moderation section to `README.md`**

Find the section listing commands/features (near the bridge/watch docs) and add:

```markdown
### Moderation (block / spam / delete)

Control the open doorbell — anyone can message you; these let you push back.

| Command | What it does |
|---------|--------------|
| `air-msg block <to>` | Drop all incoming mail from a sender at the door (never decrypted, saved, or surfaced). A small drop-tally is kept. |
| `air-msg unblock <to>` / `air-msg blocked` | Remove a block / list blocked senders + tallies. |
| `air-msg spam <envelope-id>` | Hide a junk message locally **and** send AIR a signed private abuse report (best-effort). |
| `air-msg delete --message <id> --yes` / `--with <to> --yes` | Erase one message or a whole conversation from your local diary (local only — the relay can't unsend). |

`inbox`/`history` show each message's `id` (use it with `spam`/`delete`), and accept `--include-spam` to reveal hidden junk.

**Block is a convenience filter, not a security wall:** the relay does not authenticate senders, so a determined forger can still reach you as an *unverified* sender. The cryptographic guarantees (signature verification + fingerprint pinning) are what actually protect you. The same five actions are available as MCP tools (`agent_block`, `agent_unblock`, `agent_list_blocked`, `agent_report_spam`, `agent_delete`).
```

- [ ] **Step 2: Final full-suite run**

Run: `node --test`
Expected: all passing — baseline 88 + the new moderation tests (moderation store/seam, archive spam/delete, receive block-enforcement, core ops).

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "docs(moderation): README section for block/spam/delete" \
  -m "Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

- [ ] **Step 4: Push the branch**

```bash
git push   # branch feat/moderation already tracks origin
```

---

## Self-Review (completed by plan author)

**Spec coverage:** block hard-drop (Task 5) · drop-tally advisory+batched (Tasks 1, 5) · separate blocklist store (Task 1) · fail-open (Task 1) · spam local-hide + signed private report (Tasks 2, 3, 6) · report_id+version+self-report-guard (Task 2) · delete message/conversation, confirm-gated (Tasks 4, 6) · spam hidden-by-default + `--include-spam` (Tasks 3, 6, 7) · `envelope_id` surfaced (Task 0) · CLI + MCP surfaces (Task 7) · README (Task 8). All §12 files covered.

**Placeholder scan:** none — every code step shows the actual code; every run step shows the exact command + expected output.

**Type/name consistency:** `moderation.mjs` exports (`isBlocked`, `recordBlockedDrops`, `block`, `unblock`, `listBlocked`, `loadBlocklist`, `reportAbuse`) are imported under those names (with `block as blockDid`/`unblock as unblockDid` aliases in `core.mjs` to avoid shadowing). `archive.mjs` new exports (`markSpam`, `getReceived`, `deleteMessage`, `deleteConversation`) match their imports in `core.mjs`. `recentForInbox(limit, {includeSpam})` and `history({...,includeSpam})` signatures match all call sites (`recentInbox`, `historyOp`, CLI). Op return shapes (`{status,...}`, `{hidden,reported,subject}`, `{deleted,scope,...}`) match the CLI/MCP consumers.

**Circular-import check:** `moderation.mjs` imports only `identity.mjs` (`bridgeHome`) + `crypto.mjs` (`signRaw`,`jcsCanonical`) + `node:crypto` — never `core.mjs`. Resolution lives in `core.mjs` ops. No cycle.
