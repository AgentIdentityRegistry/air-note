# Chat-App Bridge (Telegram v1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A two-way Telegram intercom for AIR Note — forward incoming mail out to Telegram, and route the user's in-app replies back as real (signed + encrypted) AIR Notes.

**Architecture:** A new `air-msg bridge` daemon (mirrors `air-msg watch`) runs two concurrent loops over one `AbortController`: OUTBOUND = `watch()` with an `onMessage` hook that sends a ping to Telegram and stores a route; INBOUND = the Telegram adapter's `getUpdates` long-poll that maps a reply back to its peer via the stored route and calls `core.send`. It adds **no** messaging/crypto logic — it drives `core` + a pluggable adapter, the #29 sibling-consumer pattern.

**Tech Stack:** Node ≥22 ESM, `node --test`, `node:sqlite` (existing `archive.db`), zero new dependencies (Telegram is plain `fetch`). Spec: `docs/superpowers/specs/2026-06-02-chat-app-bridge-design.md`.

---

## Refinements to the spec (decided at planning time)

1. **Consumer lock lives at the entrypoints, not inside `watch()`.** `watch.mjs` is a reusable library (also used by `channel-server.mjs`); acquiring a process-wide lock inside it would be an untestable side-effect. So the lock is acquired in the CLI `watch`/`bridge` cases and in `channel-server.mjs main()`. `watch.mjs` itself is untouched. (Spec §11 said "touches watch.mjs"; this is the cleaner realization.)
2. **Per-message pings, not burst coalescing** (supersedes spec §7.7). Each received message gets its own Telegram ping and its own route row, so reply-threading maps to the *exact* message (precise `thread_id`/`in_reply_to`). Rate-limit safety instead comes from a **serialized outbound send queue + one 429 retry** inside the adapter — simpler than timed per-peer coalescing and better for a reply intercom.
3. **`core.send` body is a plain string** (confirmed: `wrapBody` at `core.mjs:98` turns a string into `{type:"text",text}`; the CLI's own `send` passes a string).
4. **`getUpdates` long-poll = 25s server timeout**; on error, exponential backoff 1s→5s cap, reset on success (mirrors `watch.mjs`'s SSE backoff).

---

## File map

| File | New/Mod | Responsibility |
|------|---------|----------------|
| `src/consumer-lock.mjs` | New | PID lockfile at `<home>/consumer.lock`; acquire/release/stale-reclaim; `acquireOrExit`. |
| `src/bridge-routes.mjs` | New | `bridge_routes` table + update-offset on the shared `archive.db` handle. |
| `src/bridge-config.mjs` | New | `<home>/bridge.json` (0600) load/save — the bot token + chat id. |
| `src/bridge.mjs` | New | Pure helpers (`badgeFor`/`bridgeFormat`/`replyTier`) + orchestration (`makeBridgeOutbound`/`makeReplyHandler`/`makeConfirmStore`). |
| `src/adapters/telegram.mjs` | New | The only Telegram-specific code: `send`, `listen`, `captureFirstChat`. |
| `src/cli.mjs` | Mod | `bridge` + `bridge setup` cases; lock in the `watch` case; HELP. |
| `src/channel-server.mjs` | Mod | Acquire the consumer lock in `main()`. |
| `test/*.test.mjs` | New | One test file per new module. |

**Task order (linear, dependency-safe):** 1 lock → 2 wire-lock → 3 routes → 4 config → 5 bridge-pure → 6 telegram-adapter → 7 bridge-orchestration → 8 CLI.

---

## Task 1: Consumer lock

**Files:**
- Create: `src/consumer-lock.mjs`
- Test: `test/consumer-lock.test.mjs`

- [ ] **Step 1: Write the failing test** — `test/consumer-lock.test.mjs`

```js
import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { acquireConsumerLock, releaseConsumerLock, isPidAlive, acquireOrExit } from "../src/consumer-lock.mjs";

let dir;
beforeEach(() => { dir = mkdtempSync(join(tmpdir(), "air-msg-lock-")); });
afterEach(() => { rmSync(dir, { recursive: true, force: true }); });

test("acquire on empty dir succeeds and writes the pid file", () => {
  const r = acquireConsumerLock({ name: "watch", home: dir, pid: 100, isAlive: () => true });
  assert.equal(r.acquired, true);
  assert.ok(existsSync(join(dir, "consumer.lock")));
});

test("a second LIVE consumer is refused, with the holder", () => {
  acquireConsumerLock({ name: "watch", home: dir, pid: 100, isAlive: () => true });
  const r = acquireConsumerLock({ name: "bridge", home: dir, pid: 200, isAlive: () => true });
  assert.equal(r.acquired, false);
  assert.equal(r.holder.pid, 100);
  assert.equal(r.holder.name, "watch");
});

test("a stale lock (dead holder) is reclaimed", () => {
  acquireConsumerLock({ name: "watch", home: dir, pid: 100, isAlive: () => true });
  const r = acquireConsumerLock({ name: "bridge", home: dir, pid: 200, isAlive: (p) => p === 200 });
  assert.equal(r.acquired, true);
});

test("re-acquire by the same pid is idempotent", () => {
  acquireConsumerLock({ name: "watch", home: dir, pid: 100, isAlive: () => true });
  const r = acquireConsumerLock({ name: "watch", home: dir, pid: 100, isAlive: () => true });
  assert.equal(r.acquired, true);
});

test("release removes the lock iff we own it", () => {
  acquireConsumerLock({ name: "watch", home: dir, pid: 100, isAlive: () => true });
  releaseConsumerLock({ home: dir, pid: 999 });
  assert.ok(existsSync(join(dir, "consumer.lock")));
  releaseConsumerLock({ home: dir, pid: 100 });
  assert.ok(!existsSync(join(dir, "consumer.lock")));
});

test("isPidAlive: this process is alive; an absurd pid is not", () => {
  assert.equal(isPidAlive(process.pid), true);
  assert.equal(isPidAlive(2 ** 30), false);
});

test("acquireOrExit: prints + exits 1 when held by a live consumer", () => {
  acquireConsumerLock({ name: "watch", home: dir, pid: 100, isAlive: () => true });
  const logs = []; let code = null;
  const ok = acquireOrExit("bridge", { home: dir, pid: 200, isAlive: () => true,
    log: (s) => logs.push(s), exit: (n) => { code = n; } });
  assert.equal(ok, false);
  assert.equal(code, 1);
  assert.ok(logs.some((l) => l.includes("another live consumer") && l.includes("100")));
});
```

- [ ] **Step 2: Run — expect FAIL** (`Cannot find module '../src/consumer-lock.mjs'`)

```
node --test test/consumer-lock.test.mjs
```

- [ ] **Step 3: Implement** — `src/consumer-lock.mjs`

```js
// consumer-lock.mjs — single live-consumer lock for the shared relay pull cursor.
// watch, channel-server, and bridge all advance ONE cursor (archive.mjs pull_cursor),
// so only one may run per identity. This turns "two daemons silently eat each other's
// mail" into a loud, correct error. The lock is a PID file at <home>/consumer.lock (0600).

import { readFileSync, writeFileSync, rmSync, existsSync, chmodSync } from "node:fs";
import { join } from "node:path";
import { bridgeHome } from "./identity.mjs";

const lockPath = (home) => join(home, "consumer.lock");

/** Is a process alive? Signal 0 probes without killing. EPERM = exists but not ours. */
export function isPidAlive(pid, kill = process.kill) {
  if (!pid || pid <= 0) return false;
  try { kill(pid, 0); return true; }
  catch (e) { return e.code === "EPERM"; }
}

/**
 * Acquire the consumer lock. Returns { acquired:true } on success, or
 * { acquired:false, holder } if a LIVE consumer already holds it. A stale lock
 * (dead holder PID) or our own is reclaimed. All deps injectable for tests.
 */
export function acquireConsumerLock({
  name = "consumer", home = bridgeHome(), pid = process.pid, isAlive = isPidAlive,
} = {}) {
  const path = lockPath(home);
  if (existsSync(path)) {
    let holder = null;
    try { holder = JSON.parse(readFileSync(path, "utf8")); } catch { holder = null; }
    if (holder && holder.pid !== pid && isAlive(holder.pid)) {
      return { acquired: false, holder };
    }
  }
  writeFileSync(path, JSON.stringify({ pid, name, since: new Date().toISOString() }), { mode: 0o600 });
  try { chmodSync(path, 0o600); } catch { /* best effort on non-POSIX */ }
  return { acquired: true };
}

/** Release the lock iff we own it. Best-effort, never throws. */
export function releaseConsumerLock({ home = bridgeHome(), pid = process.pid } = {}) {
  const path = lockPath(home);
  try {
    if (!existsSync(path)) return;
    const holder = JSON.parse(readFileSync(path, "utf8"));
    if (holder.pid === pid) rmSync(path, { force: true });
  } catch { /* best effort */ }
}

/** Daemon-entrypoint guard: acquire, or print a clear message + exit(1). Returns acquired? */
export function acquireOrExit(name, {
  home = bridgeHome(), pid = process.pid, isAlive = isPidAlive,
  log = console.error, exit = (n) => process.exit(n),
} = {}) {
  const r = acquireConsumerLock({ name, home, pid, isAlive });
  if (!r.acquired) {
    log(`✗ another live consumer (PID ${r.holder?.pid}, "${r.holder?.name}") holds the relay cursor — stop it first.`);
    exit(1);
    return false;
  }
  return true;
}
```

- [ ] **Step 4: Run — expect PASS**

```
node --test test/consumer-lock.test.mjs
```

- [ ] **Step 5: Commit**

```bash
git add src/consumer-lock.mjs test/consumer-lock.test.mjs
git commit -m "feat(bridge): consumer lock — one live relay-cursor consumer per identity"
```

---

## Task 2: Wire the lock into the `watch` + `channel-server` entrypoints

**Files:**
- Modify: `src/cli.mjs` (the `watch` case, ~line 190) + import
- Modify: `src/channel-server.mjs` (`main()`)

There is no clean unit test for a process entrypoint; verification is (a) the whole suite still passes, (b) a manual two-daemon check.

- [ ] **Step 1: Import the lock in `src/cli.mjs`** (add after the existing `import { watch } ...` near line 26)

```js
import { acquireOrExit, releaseConsumerLock } from "./consumer-lock.mjs";
```

- [ ] **Step 2: Acquire in the `watch` case.** In `src/cli.mjs`, inside `case "watch": {`, immediately after `const identity = await ensureIdentity();` add:

```js
      if (!acquireOrExit("watch")) break;
```

And in the same case, change the `stop` handler to release the lock. Replace:

```js
      const stop = () => { console.log(c.dim("\n…stopping watch")); ac.abort(); };
```

with:

```js
      const stop = () => { console.log(c.dim("\n…stopping watch")); ac.abort(); releaseConsumerLock(); };
```

And after the `await watch({...})` call in the `watch` case completes (just before `break;`), add:

```js
      releaseConsumerLock();
```

- [ ] **Step 3: Acquire in `src/channel-server.mjs`.** Add the import near the top (after the other `./` imports):

```js
import { acquireOrExit, releaseConsumerLock } from "./consumer-lock.mjs";
```

In `main()`, immediately after `const identity = await ensureIdentity();`, add:

```js
  if (!acquireOrExit("channel-server")) return;
```

And register release on shutdown — change:

```js
  process.once("SIGINT", () => ac.abort());
  process.once("SIGTERM", () => ac.abort());
```

to:

```js
  process.once("SIGINT", () => { ac.abort(); releaseConsumerLock(); });
  process.once("SIGTERM", () => { ac.abort(); releaseConsumerLock(); });
```

- [ ] **Step 4: Run the whole suite — expect PASS (no regressions)**

```
node --test
```

- [ ] **Step 5: Manual two-daemon check** (document the result in the commit body)

```
# Terminal A:
node src/cli.mjs watch
# Terminal B (should refuse + exit 1):
node src/cli.mjs watch
#   expect: "✗ another live consumer (PID …, "watch") holds the relay cursor — stop it first."
```

- [ ] **Step 6: Commit**

```bash
git add src/cli.mjs src/channel-server.mjs
git commit -m "feat(bridge): acquire the consumer lock in watch + channel-server entrypoints"
```

---

## Task 3: Routing table (`bridge_routes`) + update offset

**Files:**
- Create: `src/bridge-routes.mjs`
- Test: `test/bridge-routes.test.mjs`

- [ ] **Step 1: Write the failing test** — `test/bridge-routes.test.mjs`

```js
import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { closeArchive } from "../src/archive.mjs";
import {
  putRoute, getRoute, pruneRoutes, getUpdateOffset, setUpdateOffset, resetBridgeRoutesCache,
} from "../src/bridge-routes.mjs";

let dir;
beforeEach(() => {
  closeArchive(); resetBridgeRoutesCache();
  dir = mkdtempSync(join(tmpdir(), "air-msg-routes-"));
  process.env.AGENT_BRIDGE_HOME = dir;
});
afterEach(() => { closeArchive(); rmSync(dir, { recursive: true, force: true }); });

const route = (over = {}) => ({
  platform: "telegram", external_id: "123", peer_did: "did:wba:x:agents:AIR-ALICE",
  contact: "alice", thread_id: "t1", envelope_id: "e1", verified: true, created_at: 1000, ...over,
});

test("put then get round-trips, with verified coerced to boolean", () => {
  putRoute(route());
  const r = getRoute({ external_id: "123" });
  assert.equal(r.peer_did, "did:wba:x:agents:AIR-ALICE");
  assert.equal(r.contact, "alice");
  assert.equal(r.thread_id, "t1");
  assert.equal(r.envelope_id, "e1");
  assert.equal(r.verified, true);
});

test("get miss returns null", () => {
  assert.equal(getRoute({ external_id: "nope" }), null);
});

test("a numeric external_id is keyed as a string", () => {
  putRoute(route({ external_id: 456 }));
  assert.ok(getRoute({ external_id: "456" }));
  assert.ok(getRoute({ external_id: 456 }));
});

test("two different external_ids for the same peer keep two routes", () => {
  putRoute(route({ external_id: "1", thread_id: "tA" }));
  putRoute(route({ external_id: "2", thread_id: "tB" }));
  assert.equal(getRoute({ external_id: "1" }).thread_id, "tA");
  assert.equal(getRoute({ external_id: "2" }).thread_id, "tB");
});

test("pruneRoutes drops routes older than maxAge", () => {
  putRoute(route({ external_id: "old", created_at: 1000 }));
  putRoute(route({ external_id: "new", created_at: 9_000_000 }));
  const removed = pruneRoutes({ now: 9_000_000, maxAgeMs: 1000 });
  assert.equal(removed >= 1, true);
  assert.equal(getRoute({ external_id: "old" }), null);
  assert.ok(getRoute({ external_id: "new" }));
});

test("update offset get/set (0 when unset)", () => {
  assert.equal(getUpdateOffset(), 0);
  setUpdateOffset({ offset: 42 });
  assert.equal(getUpdateOffset(), 42);
  setUpdateOffset({ offset: 99 });
  assert.equal(getUpdateOffset(), 99);
});
```

- [ ] **Step 2: Run — expect FAIL** (module not found)

```
node --test test/bridge-routes.test.mjs
```

- [ ] **Step 3: Implement** — `src/bridge-routes.mjs`

```js
// bridge-routes.mjs — reply-routing table for the chat-app bridge, on the shared
// archive.db handle. archive.mjs owns the FILE; this module owns the bridge_routes
// TABLE + the bridge's update-offset key in the generic `meta` table. Routes are keyed
// by the chat platform's server-assigned message id → the relay-VERIFIED sender DID, so
// nothing the sender controls can influence who a reply goes to. DDL via prepare().run()
// (the repo hook forbids db.exec).

import { openArchive } from "./archive.mjs";

let _ensured = false;
function db() {
  const d = openArchive();
  if (!_ensured) {
    d.prepare(`CREATE TABLE IF NOT EXISTS bridge_routes (
      platform     TEXT NOT NULL,
      external_id  TEXT NOT NULL,
      peer_did     TEXT NOT NULL,
      contact      TEXT,
      thread_id    TEXT,
      envelope_id  TEXT,
      verified     INTEGER NOT NULL,
      created_at   INTEGER NOT NULL,
      PRIMARY KEY (platform, external_id)
    )`).run();
    _ensured = true;
  }
  return d;
}

/** Tests call closeArchive() (drops the handle); reset the guard so the table re-ensures. */
export function resetBridgeRoutesCache() { _ensured = false; }

export function putRoute({
  platform = "telegram", external_id, peer_did, contact = null,
  thread_id = null, envelope_id = null, verified, created_at,
}) {
  db().prepare(`INSERT OR REPLACE INTO bridge_routes
    (platform, external_id, peer_did, contact, thread_id, envelope_id, verified, created_at)
    VALUES (?, ?, ?, ?, ?, ?, ?, ?)`)
    .run(platform, String(external_id), peer_did, contact, thread_id, envelope_id, verified ? 1 : 0, created_at);
}

export function getRoute({ platform = "telegram", external_id }) {
  const r = db().prepare(`SELECT * FROM bridge_routes WHERE platform = ? AND external_id = ?`)
    .get(platform, String(external_id));
  if (!r) return null;
  return {
    platform: r.platform, external_id: r.external_id, peer_did: r.peer_did,
    contact: r.contact ?? null, thread_id: r.thread_id ?? null, envelope_id: r.envelope_id ?? null,
    verified: !!r.verified, created_at: r.created_at,
  };
}

/** Drop routes older than maxAgeMs (default 30d) or beyond maxRows newest. Returns count removed. */
export function pruneRoutes({ platform = "telegram", now, maxAgeMs = 30 * 24 * 3600 * 1000, maxRows = 5000 } = {}) {
  const d = db();
  const byAge = d.prepare(`DELETE FROM bridge_routes WHERE platform = ? AND created_at < ?`)
    .run(platform, now - maxAgeMs);
  const overflow = d.prepare(`DELETE FROM bridge_routes WHERE platform = ? AND external_id IN (
      SELECT external_id FROM bridge_routes WHERE platform = ? ORDER BY created_at DESC LIMIT -1 OFFSET ?)`)
    .run(platform, platform, maxRows);
  return (byAge.changes || 0) + (overflow.changes || 0);
}

const OFFSET_KEY = (platform) => `bridge_update_offset_${platform}`;

export function getUpdateOffset({ platform = "telegram" } = {}) {
  const row = db().prepare(`SELECT value FROM meta WHERE key = ?`).get(OFFSET_KEY(platform));
  return row ? Number(row.value) : 0;
}

export function setUpdateOffset({ platform = "telegram", offset }) {
  db().prepare(`INSERT INTO meta (key, value) VALUES (?, ?)
    ON CONFLICT(key) DO UPDATE SET value = excluded.value`).run(OFFSET_KEY(platform), String(offset));
}
```

- [ ] **Step 4: Run — expect PASS**

```
node --test test/bridge-routes.test.mjs
```

- [ ] **Step 5: Commit**

```bash
git add src/bridge-routes.mjs test/bridge-routes.test.mjs
git commit -m "feat(bridge): bridge_routes table + update-offset watermark on archive.db"
```

---

## Task 4: Bridge config (`bridge.json`, 0600)

**Files:**
- Create: `src/bridge-config.mjs`
- Test: `test/bridge-config.test.mjs`

- [ ] **Step 1: Write the failing test** — `test/bridge-config.test.mjs`

```js
import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { loadBridgeConfig, saveBridgeConfig } from "../src/bridge-config.mjs";

let dir;
beforeEach(() => { dir = mkdtempSync(join(tmpdir(), "air-msg-cfg-")); });
afterEach(() => { rmSync(dir, { recursive: true, force: true }); });

test("load on a fresh dir returns null", () => {
  assert.equal(loadBridgeConfig({ home: dir }), null);
});

test("save then load round-trips", () => {
  saveBridgeConfig({ telegram: { bot_token: "T", chat_id: 555 } }, { home: dir });
  const cfg = loadBridgeConfig({ home: dir });
  assert.equal(cfg.telegram.bot_token, "T");
  assert.equal(cfg.telegram.chat_id, 555);
});

test("the config file is created mode 0600", () => {
  const path = saveBridgeConfig({ telegram: { bot_token: "T", chat_id: 1 } }, { home: dir });
  assert.equal(statSync(path).mode & 0o777, 0o600);
});
```

- [ ] **Step 2: Run — expect FAIL**

```
node --test test/bridge-config.test.mjs
```

- [ ] **Step 3: Implement** — `src/bridge-config.mjs`

```js
// bridge-config.mjs — chat-app bridge config (bot token + chat id) at <home>/bridge.json,
// mode 0600 (same secret discipline as identity.json/contacts.json). The token is a secret:
// it lives ONLY in this file — never in sqlite, never echoed back.

import { readFileSync, writeFileSync, existsSync, chmodSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import { bridgeHome } from "./identity.mjs";

const configPath = (home) => join(home, "bridge.json");

export function loadBridgeConfig({ home = bridgeHome() } = {}) {
  const path = configPath(home);
  if (!existsSync(path)) return null;
  try { return JSON.parse(readFileSync(path, "utf8")); } catch { return null; }
}

export function saveBridgeConfig(cfg, { home = bridgeHome() } = {}) {
  mkdirSync(home, { recursive: true, mode: 0o700 });
  const path = configPath(home);
  writeFileSync(path, JSON.stringify(cfg, null, 2), { mode: 0o600 });
  try { chmodSync(path, 0o600); } catch { /* best effort on non-POSIX */ }
  return path;
}
```

- [ ] **Step 4: Run — expect PASS**

```
node --test test/bridge-config.test.mjs
```

- [ ] **Step 5: Commit**

```bash
git add src/bridge-config.mjs test/bridge-config.test.mjs
git commit -m "feat(bridge): bridge.json config (bot token + chat id) at mode 0600"
```

---

## Task 5: Bridge pure helpers (`badgeFor` / `bridgeFormat` / `replyTier`)

**Files:**
- Create: `src/bridge.mjs` (helpers only; orchestration added in Task 7)
- Test: `test/bridge.test.mjs` (helpers section; orchestration tests added in Task 7)

- [ ] **Step 1: Write the failing test** — `test/bridge.test.mjs`

```js
import { test } from "node:test";
import assert from "node:assert/strict";
import { badgeFor, bridgeFormat, replyTier } from "../src/bridge.mjs";

const msg = (over = {}) => ({
  from: "did:wba:x:agents:AIR-ALICE", contact: "alice", verified: true,
  body: { type: "text", text: "hi there" }, thread_id: "t1", envelope_id: "e1", ...over,
});

test("badgeFor: verified+unchanged → verified; else UNVERIFIED", () => {
  assert.equal(badgeFor(msg()), "✓ verified");
  assert.equal(badgeFor(msg({ verified: false })), "⚠️ UNVERIFIED");
  assert.equal(badgeFor(msg({ key_changed: true })), "⚠️ UNVERIFIED");
});

test("badgeFor: a spoofed display name CANNOT forge the badge (it's crypto-only)", () => {
  // unverified sender names themselves "Alice ✓ verified" — badge must still be UNVERIFIED
  assert.equal(badgeFor(msg({ verified: false, contact: "Alice ✓ verified" })), "⚠️ UNVERIFIED");
});

test("bridgeFormat full: badge is a sender-unreachable PREFIX + the body text", () => {
  const p = bridgeFormat(msg({ body: { type: "text", text: "ping" } }));
  assert.ok(p.title.startsWith("✓ verified"));   // badge first — name can't get in front
  assert.ok(p.title.includes("alice"));
  assert.equal(p.body, "ping");
});

test("bridgeFormat meta: body text is withheld", () => {
  const p = bridgeFormat(msg(), { bodyMode: "meta" });
  assert.equal(p.body, "(open AIR Note to read)");
});

test("bridgeFormat: markup in the body is passed through verbatim (caller sends plain text)", () => {
  const p = bridgeFormat(msg({ body: { type: "text", text: "*bold* [x](http://evil)" } }));
  assert.equal(p.body, "*bold* [x](http://evil)"); // no escaping/parsing here; adapter sends with NO parse_mode
});

test("bridgeFormat: non-text + empty + absent bodies show markers, never 'undefined'", () => {
  assert.equal(bridgeFormat(msg({ body: { type: "image" } })).body, "(non-text message)");
  assert.equal(bridgeFormat(msg({ body: { type: "unavailable" } })).body, "(could not decrypt)");
  assert.equal(bridgeFormat(msg({ body: { type: "text" } })).body, "(empty message)");
  assert.equal(bridgeFormat(msg({ body: undefined })).body, "(no content)");
});

test("bridgeFormat: no pinned alias → short AIR-id as the title name", () => {
  assert.ok(bridgeFormat(msg({ contact: undefined })).title.includes("AIR-ALICE"));
});

test("replyTier: verified route → one-tap; unverified → confirm", () => {
  assert.equal(replyTier({ verified: true }), "one-tap");
  assert.equal(replyTier({ verified: false }), "confirm");
  assert.equal(replyTier(null), "confirm");
});
```

- [ ] **Step 2: Run — expect FAIL**

```
node --test test/bridge.test.mjs
```

- [ ] **Step 3: Implement** — `src/bridge.mjs` (helpers; orchestration appended in Task 7)

```js
// bridge.mjs — chat-app bridge orchestration + pure helpers. Forwards incoming AIR Note
// mail to an external chat adapter and routes the user's in-app replies back as real
// (signed + encrypted) AIR Notes. No messaging/crypto logic of its own — it drives
// core.send + the adapter, the #29 sibling-consumer pattern.

/** Short AIR-id label from a DID (or pass through). */
function shortPeer(did) {
  const m = String(did).match(/AIR-[A-Za-z0-9-]+/);
  return m ? m[0] : String(did);
}

/** Trust badge derived ONLY from crypto fields — never from sender-controlled strings,
 *  so a display name like "Alice ✓ verified" cannot forge a check. */
export function badgeFor(m) {
  return (m.verified && !m.key_changed) ? "✓ verified" : "⚠️ UNVERIFIED";
}

/** One-line body text (text → text; else a marker; never raw structure or 'undefined'). */
function bodyText(body) {
  if (!body) return "(no content)";
  if (body.type === "text") return body.text != null ? String(body.text) : "(empty message)";
  if (body.type === "unavailable") return "(could not decrypt)";
  return "(non-text message)";
}

/**
 * Build the plain-text Telegram ping for a received message. The caller sends with NO
 * parse_mode, so a hostile body/name cannot inject markup or fake links. The badge is a
 * sender-unreachable PREFIX. `bodyMode` is "full" (default) or "meta".
 * @returns {{title:string, body:string, badge:string}}
 */
export function bridgeFormat(m, { bodyMode = "full" } = {}) {
  const who = m.contact || shortPeer(m.from);
  const badge = badgeFor(m);
  const body = bodyMode === "meta" ? "(open AIR Note to read)" : bodyText(m.body);
  return { title: `${badge} · 📬 ${who}`, body, badge };
}

/** Reply tier from a stored route: verified+pinned → one-tap; else an explicit confirm. */
export function replyTier(route) {
  return route && route.verified ? "one-tap" : "confirm";
}
```

- [ ] **Step 4: Run — expect PASS**

```
node --test test/bridge.test.mjs
```

- [ ] **Step 5: Commit**

```bash
git add src/bridge.mjs test/bridge.test.mjs
git commit -m "feat(bridge): pure ping-format + badge + reply-tier helpers"
```

---

## Task 6: Telegram adapter (`send` / `listen` / `captureFirstChat`)

**Files:**
- Create: `src/adapters/telegram.mjs`
- Test: `test/adapters.telegram.test.mjs`

- [ ] **Step 1: Write the failing test** — `test/adapters.telegram.test.mjs`

```js
import { test } from "node:test";
import assert from "node:assert/strict";
import { createTelegramAdapter, captureFirstChat } from "../src/adapters/telegram.mjs";

/** A scripted fetch: each call shifts the next handler off the queue. */
function scriptedFetch(handlers) {
  const calls = [];
  const q = [...handlers];
  const fetchImpl = async (url, opts) => {
    calls.push({ url, body: opts?.body ? JSON.parse(opts.body) : null });
    const h = q.shift() || (() => ({ ok: true, status: 200, json: async () => ({ ok: true, result: [] }) }));
    return h(url, opts);
  };
  return { fetchImpl, calls };
}
const okResult = (result) => () => ({ ok: true, status: 200, json: async () => ({ ok: true, result }) });

test("send: POSTs sendMessage with chat_id + text and NO parse_mode; returns the message id", async () => {
  const { fetchImpl, calls } = scriptedFetch([okResult({ message_id: 777 })]);
  const a = createTelegramAdapter({ token: "T", chatId: 555, fetchImpl });
  const id = await a.send({ title: "✓ verified · 📬 alice", body: "ping" });
  assert.equal(id, "777");
  assert.ok(calls[0].url.endsWith("/sendMessage"));
  assert.equal(calls[0].body.chat_id, 555);
  assert.equal(calls[0].body.text, "✓ verified · 📬 alice\nping");
  assert.equal("parse_mode" in calls[0].body, false);
});

test("send: a 429 is retried once after retry_after, then succeeds", async () => {
  const { fetchImpl } = scriptedFetch([
    () => ({ ok: false, status: 429, json: async () => ({ ok: false, parameters: { retry_after: 0 } }) }),
    okResult({ message_id: 9 }),
  ]);
  const a = createTelegramAdapter({ token: "T", chatId: 1, fetchImpl });
  assert.equal(await a.send({ title: "t", body: "b" }), "9");
});

test("listen: a matching-chat reply calls onReply with the reply_to id + text, then advances the offset", async () => {
  let savedOffset = 0;
  const ac = new AbortController();
  const update = { update_id: 10, message: { message_id: 50, chat: { id: 555 }, text: "yes do it",
    reply_to_message: { message_id: 777 } } };
  const { fetchImpl } = scriptedFetch([
    okResult([update]),
    () => { ac.abort(); return { ok: true, status: 200, json: async () => ({ ok: true, result: [] }) }; },
  ]);
  const seen = [];
  const a = createTelegramAdapter({ token: "T", chatId: 555, fetchImpl,
    getOffset: () => savedOffset, setOffset: (o) => { savedOffset = o; } });
  await a.listen({ signal: ac.signal, onReply: async (r) => { seen.push(r); } });
  assert.equal(seen.length, 1);
  assert.equal(seen[0].replyToExternalId, "777");
  assert.equal(seen[0].text, "yes do it");
  assert.equal(savedOffset, 11); // update_id + 1
});

test("listen: an update from a FOREIGN chat is ignored but still acked (offset advances)", async () => {
  let savedOffset = 0;
  const ac = new AbortController();
  const foreign = { update_id: 20, message: { message_id: 1, chat: { id: 999 }, text: "hi" } };
  const { fetchImpl } = scriptedFetch([
    okResult([foreign]),
    () => { ac.abort(); return { ok: true, status: 200, json: async () => ({ ok: true, result: [] }) }; },
  ]);
  const seen = [];
  const a = createTelegramAdapter({ token: "T", chatId: 555, fetchImpl,
    getOffset: () => savedOffset, setOffset: (o) => { savedOffset = o; } });
  await a.listen({ signal: ac.signal, onReply: async (r) => seen.push(r) });
  assert.equal(seen.length, 0);
  assert.equal(savedOffset, 21);
});

test("listen: when onReply throws, the offset is NOT advanced past it (at-least-once)", async () => {
  let savedOffset = 0;
  const ac = new AbortController();
  const update = { update_id: 30, message: { message_id: 5, chat: { id: 555 }, text: "x",
    reply_to_message: { message_id: 1 } } };
  const { fetchImpl } = scriptedFetch([
    okResult([update]),
    () => { ac.abort(); return { ok: true, status: 200, json: async () => ({ ok: true, result: [] }) }; },
  ]);
  const a = createTelegramAdapter({ token: "T", chatId: 555, fetchImpl,
    getOffset: () => savedOffset, setOffset: (o) => { savedOffset = o; } });
  await a.listen({ signal: ac.signal, onReply: async () => { throw new Error("send failed"); } });
  assert.equal(savedOffset, 0); // never advanced → redelivered next poll
});

test("listen: the reply() callback sends a sendMessage back to the saved chat", async () => {
  let savedOffset = 0;
  const ac = new AbortController();
  const update = { update_id: 40, message: { message_id: 7, chat: { id: 555 }, text: "ok",
    reply_to_message: { message_id: 2 } } };
  const { fetchImpl, calls } = scriptedFetch([
    okResult([update]),
    okResult({ message_id: 8 }), // the reply() send
    () => { ac.abort(); return { ok: true, status: 200, json: async () => ({ ok: true, result: [] }) }; },
  ]);
  const a = createTelegramAdapter({ token: "T", chatId: 555, fetchImpl,
    getOffset: () => savedOffset, setOffset: (o) => { savedOffset = o; } });
  await a.listen({ signal: ac.signal, onReply: async (r) => { await r.reply("✓ sent to alice"); } });
  const sent = calls.find((cl) => cl.url.endsWith("/sendMessage") && cl.body.text === "✓ sent to alice");
  assert.ok(sent);
  assert.equal(sent.body.chat_id, 555);
});

test("captureFirstChat: returns the chat id of the first message from any chat", async () => {
  const { fetchImpl } = scriptedFetch([
    okResult([]),
    okResult([{ update_id: 1, message: { chat: { id: 4242 }, text: "/start" } }]),
  ]);
  const id = await captureFirstChat({ token: "T", fetchImpl, pollDelayMs: 0, maxPolls: 5 });
  assert.equal(id, 4242);
});
```

- [ ] **Step 2: Run — expect FAIL**

```
node --test test/adapters.telegram.test.mjs
```

- [ ] **Step 3: Implement** — `src/adapters/telegram.mjs`

```js
// adapters/telegram.mjs — the ONLY Telegram-specific module. Implements the bridge
// adapter seam: send(ping) and listen({signal,onReply}), plus captureFirstChat for setup.
// Outbound sends are serialized + retried once on 429 (rate-limit safety). Inbound uses
// getUpdates long-polling (no public server). All HTTP injected via fetchImpl for tests;
// the persisted update offset is read/advanced through getOffset/setOffset (bridge-routes)
// so a restart resumes and a redelivered update is never double-processed.

const API = (token, method) => `https://api.telegram.org/bot${token}/${method}`;

const sleep = (ms, signal) => new Promise((res) => {
  const t = setTimeout(res, ms);
  signal?.addEventListener("abort", () => { clearTimeout(t); res(); }, { once: true });
});

export function createTelegramAdapter({
  token, chatId, fetchImpl = fetch, getOffset = () => 0, setOffset = () => {},
  longPollSecs = 25, log = (s) => process.stderr.write(s + "\n"),
}) {
  const chat = Number(chatId);
  let chain = Promise.resolve(); // serialize outbound sends (Telegram ~1 msg/s per chat)

  async function rawSend(text, replyToId) {
    const params = { chat_id: chat, text }; // NO parse_mode — text is untrusted
    if (replyToId) params.reply_to_message_id = Number(replyToId);
    for (let attempt = 0; ; attempt++) {
      const resp = await fetchImpl(API(token, "sendMessage"), {
        method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(params),
      });
      if (resp.status === 429 && attempt < 1) {
        let retry = 1;
        try { retry = (await resp.json())?.parameters?.retry_after ?? 1; } catch { /* ignore */ }
        await sleep(retry * 1000);
        continue;
      }
      const data = await resp.json();
      if (!data.ok) throw new Error(`telegram sendMessage failed: ${data.description ?? resp.status}`);
      return String(data.result.message_id);
    }
  }

  return {
    name: "telegram",

    /** Send one ping. Serialized; returns the message id, or null if it failed (degrade). */
    async send({ title, body }) {
      const text = `${title}\n${body}`;
      const p = chain.then(() => rawSend(text))
        .catch((e) => { log(`[telegram] send: ${e.message ?? e}`); return null; });
      chain = p.then(() => {}, () => {});
      return p;
    },

    /** Long-poll getUpdates until aborted. Filters to the saved chat; per reply, awaits
     *  onReply, then advances the offset only past SUCCESSFUL updates (at-least-once). */
    async listen({ signal, onReply }) {
      let backoff = 1000;
      while (!signal?.aborted) {
        const offset = (() => { try { return getOffset(); } catch { return 0; } })();
        let updates;
        try {
          const url = API(token, "getUpdates") + `?timeout=${longPollSecs}&offset=${offset}`;
          const resp = await fetchImpl(url, { signal });
          const data = await resp.json();
          if (!data.ok) throw new Error(data.description ?? "getUpdates not ok");
          updates = data.result ?? [];
          backoff = 1000;
        } catch (e) {
          if (signal?.aborted) break;
          log(`[telegram] getUpdates: ${e.message ?? e}`);
          await sleep(backoff, signal);
          backoff = Math.min(backoff * 2, 5000);
          continue;
        }

        let lastOk = offset - 1;
        for (const u of updates) {
          const msg = u.message;
          // Authn: only the saved chat may drive the bridge. Drop others, but still advance
          // past them so they aren't redelivered forever.
          if (!msg || Number(msg.chat?.id) !== chat || typeof msg.text !== "string") {
            lastOk = u.update_id; continue;
          }
          const replyToExternalId = msg.reply_to_message ? String(msg.reply_to_message.message_id) : null;
          try {
            await onReply({ replyToExternalId, text: msg.text, reply: (t) => rawSend(t, msg.message_id) });
            lastOk = u.update_id; // success → safe to ack
          } catch (e) {
            log(`[telegram] onReply failed (update ${u.update_id}): ${e.message ?? e}`);
            break; // do NOT advance past a failed reply → redelivered next poll
          }
        }
        if (lastOk >= offset) { try { setOffset(lastOk + 1); } catch (e) { log(`[telegram] setOffset: ${e.message ?? e}`); } }
      }
    },
  };
}

/** Setup helper: poll getUpdates until the first message arrives; return its chat id.
 *  Used by `air-msg bridge setup` to capture the user's chat id after they /start the bot. */
export async function captureFirstChat({ token, fetchImpl = fetch, signal, pollDelayMs = 2000, maxPolls = 150 }) {
  for (let i = 0; i < maxPolls && !signal?.aborted; i++) {
    const resp = await fetchImpl(API(token, "getUpdates") + `?timeout=0&offset=0`, { signal });
    const data = await resp.json();
    if (!data.ok) throw new Error(data.description ?? "getUpdates not ok");
    const withChat = (data.result ?? []).find((u) => u.message?.chat?.id != null);
    if (withChat) return Number(withChat.message.chat.id);
    await sleep(pollDelayMs, signal);
  }
  return null;
}
```

- [ ] **Step 4: Run — expect PASS**

```
node --test test/adapters.telegram.test.mjs
```

- [ ] **Step 5: Commit**

```bash
git add src/adapters/telegram.mjs test/adapters.telegram.test.mjs
git commit -m "feat(bridge): Telegram adapter — serialized send + getUpdates long-poll listen"
```

---

## Task 7: Bridge orchestration (`makeBridgeOutbound` / `makeConfirmStore` / `makeReplyHandler`)

**Files:**
- Modify: `src/bridge.mjs` (append orchestration to the helpers from Task 5)
- Modify: `test/bridge.test.mjs` (append orchestration tests)

- [ ] **Step 1: Append the failing tests** to `test/bridge.test.mjs`

```js
import { makeBridgeOutbound, makeConfirmStore, makeReplyHandler } from "../src/bridge.mjs";

/** A fake adapter that records sends + hands out a reply() that records acks. */
function fakeAdapter() {
  const sent = []; const acks = [];
  return {
    name: "telegram", acks, sent,
    async send(ping) { sent.push(ping); return String(sent.length); }, // externalId = "1","2",...
    reply: (t) => { acks.push(t); return Promise.resolve(); },
  };
}

test("outbound: a message is sent + a route stored keyed by the returned external id", async () => {
  const adapter = fakeAdapter();
  const routes = [];
  const hook = makeBridgeOutbound({ adapter, now: () => 123,
    putRouteFn: (r) => routes.push(r) });
  hook({ from: "did:wba:x:agents:AIR-ALICE", contact: "alice", verified: true,
    thread_id: "t1", envelope_id: "e1", body: { type: "text", text: "hi" } });
  await new Promise((r) => setTimeout(r, 0));
  assert.equal(adapter.sent.length, 1);
  assert.equal(routes.length, 1);
  assert.equal(routes[0].external_id, "1");
  assert.equal(routes[0].peer_did, "did:wba:x:agents:AIR-ALICE");
  assert.equal(routes[0].thread_id, "t1");
  assert.equal(routes[0].verified, true);
  assert.equal(routes[0].created_at, 123);
});

test("outbound: a failed send (null id) stores no route", async () => {
  const adapter = { name: "telegram", async send() { return null; } };
  const routes = [];
  makeBridgeOutbound({ adapter, putRouteFn: (r) => routes.push(r) })(
    { from: "did:x:AIR-A", verified: true, body: { type: "text", text: "x" } });
  await new Promise((r) => setTimeout(r, 0));
  assert.equal(routes.length, 0);
});

test("outbound: a throwing send never throws out of the hook", async () => {
  const adapter = { name: "telegram", async send() { throw new Error("boom"); } };
  const logs = [];
  let threw = false;
  try {
    makeBridgeOutbound({ adapter, log: (s) => logs.push(s) })(
      { from: "did:x:AIR-A", verified: true, body: { type: "text", text: "x" } });
  } catch { threw = true; }
  assert.equal(threw, false);
  await new Promise((r) => setTimeout(r, 0));
  assert.ok(logs.some((l) => l.includes("outbound failed")));
});

const verifiedRoute = { peer_did: "did:wba:x:agents:AIR-ALICE", contact: "alice",
  thread_id: "t1", envelope_id: "e1", verified: true };
const unverifiedRoute = { peer_did: "did:wba:x:agents:AIR-BOB", contact: null,
  thread_id: "t2", envelope_id: "e2", verified: false };

test("reply (verified, one-tap): core.send is called with thread continuity + acked", async () => {
  const sends = []; const acks = [];
  const h = makeReplyHandler({ sendFn: async (a) => sends.push(a),
    getRouteFn: () => verifiedRoute, confirm: makeConfirmStore() });
  await h({ replyToExternalId: "1", text: "on my way", reply: (t) => { acks.push(t); } });
  assert.equal(sends.length, 1);
  assert.deepEqual(sends[0], { to: "did:wba:x:agents:AIR-ALICE", body: "on my way", thread_id: "t1", in_reply_to: "e1" });
  assert.ok(acks[0].includes("sent to alice"));
});

test("reply (no reply-to): asks the user to reply to a specific message; no send", async () => {
  const sends = []; const acks = [];
  const h = makeReplyHandler({ sendFn: async (a) => sends.push(a), getRouteFn: () => verifiedRoute, confirm: makeConfirmStore() });
  await h({ replyToExternalId: null, text: "hello", reply: (t) => acks.push(t) });
  assert.equal(sends.length, 0);
  assert.ok(acks[0].includes("Reply to a specific message"));
});

test("reply (route miss / aged out): graceful ack; no send", async () => {
  const sends = []; const acks = [];
  const h = makeReplyHandler({ sendFn: async (a) => sends.push(a), getRouteFn: () => null, confirm: makeConfirmStore() });
  await h({ replyToExternalId: "999", text: "hi", reply: (t) => acks.push(t) });
  assert.equal(sends.length, 0);
  assert.ok(acks[0].includes("too old to reply"));
});

test("reply (unverified): first reply is HELD pending /yes; nothing sent yet", async () => {
  const sends = []; const acks = [];
  const confirm = makeConfirmStore();
  const h = makeReplyHandler({ sendFn: async (a) => sends.push(a), getRouteFn: () => unverifiedRoute, confirm });
  await h({ replyToExternalId: "5", text: "secret reply", reply: (t) => acks.push(t) });
  assert.equal(sends.length, 0);
  assert.ok(acks[0].includes("UNVERIFIED"));
  assert.ok(acks[0].includes("/yes"));
});

test("reply (unverified): /yes releases the HELD text to core.send", async () => {
  const sends = []; const acks = [];
  const confirm = makeConfirmStore();
  const h = makeReplyHandler({ sendFn: async (a) => sends.push(a), getRouteFn: () => unverifiedRoute, confirm });
  await h({ replyToExternalId: "5", text: "secret reply", reply: (t) => acks.push(t) });
  await h({ replyToExternalId: "5", text: "/yes", reply: (t) => acks.push(t) });
  assert.equal(sends.length, 1);
  assert.equal(sends[0].body, "secret reply");
  assert.equal(sends[0].to, "did:wba:x:agents:AIR-BOB");
  assert.ok(acks[1].includes("sent to AIR-BOB")); // no alias → short AIR-id
});

test("reply (unverified): /yes with nothing pending (expired) asks to resend; no send", async () => {
  const sends = []; const acks = [];
  const h = makeReplyHandler({ sendFn: async (a) => sends.push(a), getRouteFn: () => unverifiedRoute, confirm: makeConfirmStore() });
  await h({ replyToExternalId: "5", text: "/yes", reply: (t) => acks.push(t) });
  assert.equal(sends.length, 0);
  assert.ok(acks[0].toLowerCase().includes("nothing pending"));
});

test("reply: a core.send failure propagates (so the adapter won't ack the update)", async () => {
  const h = makeReplyHandler({ sendFn: async () => { throw new Error("relay down"); },
    getRouteFn: () => verifiedRoute, confirm: makeConfirmStore() });
  await assert.rejects(() => h({ replyToExternalId: "1", text: "hi", reply: () => {} }), /relay down/);
});
```

- [ ] **Step 2: Run — expect FAIL** (orchestration exports missing)

```
node --test test/bridge.test.mjs
```

- [ ] **Step 3: Append the implementation** to `src/bridge.mjs`

```js
import { putRoute, getRoute } from "./bridge-routes.mjs";

/** Build the watch() onMessage(m) hook: format → adapter.send → store the route.
 *  Detached-promise so a slow/failed Telegram send never crashes the watch loop. */
export function makeBridgeOutbound({
  adapter, bodyMode = "full", now = () => Date.now(),
  putRouteFn = putRoute, log = (s) => process.stderr.write(s + "\n"),
}) {
  return (m) => {
    Promise.resolve().then(async () => {
      const externalId = await adapter.send(bridgeFormat(m, { bodyMode }));
      if (!externalId) return; // send degraded → nothing to route a reply back to
      putRouteFn({
        platform: adapter.name, external_id: externalId,
        peer_did: m.from, contact: m.contact ?? null,
        thread_id: m.thread_id ?? null, envelope_id: m.envelope_id ?? null,
        verified: !!(m.verified && !m.key_changed), created_at: now(),
      });
    }).catch((err) => log(`[bridge] outbound failed: ${err.message ?? err}`));
  };
}

/** In-memory pending-reply store for UNVERIFIED senders (held until /yes, short TTL). */
export function makeConfirmStore({ ttlMs = 120_000, now = () => Date.now() } = {}) {
  const pending = new Map(); // externalId → { text, expiresAt }
  return {
    put(externalId, text) { pending.set(externalId, { text, expiresAt: now() + ttlMs }); },
    get(externalId) {
      const e = pending.get(externalId);
      if (!e || e.expiresAt < now()) { pending.delete(externalId); return null; }
      return e.text;
    },
    clear(externalId) { pending.delete(externalId); },
  };
}

/** Short AIR-id (or DID) for an ack line. */
function destLabel(route) {
  return route.contact || (String(route.peer_did).match(/AIR-[A-Za-z0-9-]+/)?.[0] ?? route.peer_did);
}

/** Build the adapter onReply handler: route lookup → reply-safety tier → core.send → ack.
 *  Throws if sendFn throws, so the adapter leaves the update un-acked (at-least-once). */
export function makeReplyHandler({ sendFn, getRouteFn = getRoute, confirm, platform = "telegram" }) {
  return async ({ replyToExternalId, text, reply }) => {
    if (!replyToExternalId) {
      await reply("↩️ Reply to a specific message so I know who to send it to.");
      return;
    }
    const route = getRouteFn({ platform, external_id: replyToExternalId });
    if (!route) {
      await reply("That conversation is too old to reply to here — open AIR Note to reply.");
      return;
    }
    const isYes = text.trim() === "/yes";

    if (replyTier(route) === "confirm") {
      if (!isYes) {
        confirm.put(replyToExternalId, text);
        await reply("⚠️ This sender is UNVERIFIED. Reply /yes (to this message) within 2 min to send anyway.");
        return;
      }
      const held = confirm.get(replyToExternalId);
      if (held == null) { await reply("Nothing pending to confirm (it may have expired). Send your reply again."); return; }
      await sendFn({ to: route.peer_did, body: held, thread_id: route.thread_id, in_reply_to: route.envelope_id });
      confirm.clear(replyToExternalId);
      await reply(`✓ sent to ${destLabel(route)}`);
      return;
    }

    // verified + pinned → one-tap (a literal "/yes" here is just text)
    await sendFn({ to: route.peer_did, body: text, thread_id: route.thread_id, in_reply_to: route.envelope_id });
    await reply(`✓ sent to ${destLabel(route)}`);
  };
}
```

- [ ] **Step 4: Run — expect PASS**

```
node --test test/bridge.test.mjs
```

- [ ] **Step 5: Commit**

```bash
git add src/bridge.mjs test/bridge.test.mjs
git commit -m "feat(bridge): outbound hook + reply-safety handler (verified one-tap / unverified /yes)"
```

---

## Task 8: CLI `bridge` + `bridge setup` + HELP

**Files:**
- Modify: `src/cli.mjs` (imports, two cases, HELP text)

This task wires the pieces into a runnable daemon. The pure pieces are already tested; verification here is the whole suite + a documented manual end-to-end check.

- [ ] **Step 1: Add imports** to `src/cli.mjs` (after the Task-2 `consumer-lock` import). `createNotifier`, `resolveOpenCommand`, `runOpenCommand`, `detectAiCmd`, `watch`, `ensureIdentity`, and `core` are ALREADY imported (cli.mjs:22-26) — do NOT re-import them. Add only these:

```js
import { loadBridgeConfig, saveBridgeConfig } from "./bridge-config.mjs";
import { createTelegramAdapter, captureFirstChat } from "./adapters/telegram.mjs";
import { makeBridgeOutbound, makeReplyHandler, makeConfirmStore } from "./bridge.mjs";
import { getUpdateOffset, setUpdateOffset, pruneRoutes } from "./bridge-routes.mjs";
import { createInterface } from "node:readline/promises";
```

- [ ] **Step 2: Add the `bridge` case** to the `switch (cmd)` in `main()` (place it right after the `watch` case ends, before `case "add":`)

```js
    case "bridge": {
      if (positionals[0] === "setup") { await bridgeSetup(); break; }

      const cfg = loadBridgeConfig();
      if (!cfg?.telegram?.bot_token || cfg?.telegram?.chat_id == null) {
        console.error("Bridge not configured. Run: air-msg bridge setup");
        process.exit(1);
      }
      if (!acquireOrExit("bridge")) break;

      const identity = await ensureIdentity();
      const bodyMode = process.env.AIRMSG_BRIDGE_BODY === "meta" ? "meta" : "full";
      const adapter = createTelegramAdapter({
        token: cfg.telegram.bot_token,
        chatId: Number(cfg.telegram.chat_id),
        getOffset: () => getUpdateOffset({ platform: "telegram" }),
        setOffset: (o) => setUpdateOffset({ platform: "telegram", offset: o }),
      });

      // D6: the bridge is a superset of `watch` — fire the local OS banner too.
      const openMode = process.env.AIRMSG_OPEN || "terminal-history";
      const aiCmd = process.env.AIRMSG_AI_CMD || (openMode === "ai" ? detectAiCmd() : undefined);
      const openResolver = (peer, info) => resolveOpenCommand(peer, { mode: openMode, aiCmd, ...info });
      const notifier = await createNotifier({ onClick: (argv) => runOpenCommand(argv) });

      const confirm = makeConfirmStore();
      const outbound = makeBridgeOutbound({ adapter, bodyMode });

      const ac = new AbortController();
      const stop = () => { console.log(c.dim("\n…stopping bridge")); ac.abort(); releaseConsumerLock(); };
      process.once("SIGINT", stop);
      process.once("SIGTERM", stop);

      pruneRoutes({ platform: "telegram", now: Date.now() });
      console.log(`${c.green("● bridging")} ${c.bold(identity.did)} ${c.dim("→ Telegram")}`);
      console.log(`  ${c.dim(`body: ${bodyMode} · notify: ${notifier.backend} · Ctrl-C to stop`)}`);
      if (bodyMode === "full") {
        console.log(c.yellow("  ⚠ full message text is sent to Telegram (outside E2E). Set AIRMSG_BRIDGE_BODY=meta for metadata-only."));
      }

      // INBOUND loop (replies → AIR Notes) runs alongside the OUTBOUND watch loop.
      const inbound = adapter
        .listen({ signal: ac.signal, onReply: makeReplyHandler({ sendFn: core.send, confirm }) })
        .catch((e) => { if (e?.name !== "AbortError") console.error("bridge inbound:", e.message ?? e); });

      await watch({
        signal: ac.signal, identity, notifier, openResolver,
        onMessage: (m) => {
          outbound(m); // push to Telegram + store the route
          const vrf = (m.verified && !m.key_changed) ? c.green("✓") : c.red("⚠");
          console.log(`  ↓→tg ${vrf} ${m.contact || m.from}`);
        },
      }).catch((e) => { if (e?.name !== "AbortError") throw e; });

      await inbound;
      releaseConsumerLock();
      break;
    }
```

- [ ] **Step 3: Add the `bridgeSetup()` helper** (define it as a top-level `async function` in `cli.mjs`, e.g. just above `async function main()`)

```js
async function bridgeSetup() {
  const rl = createInterface({ input: process.stdin, output: process.stdout });
  try {
    console.log(c.bold("AIR Note → Telegram bridge setup"));
    console.log(c.yellow(
      "⚠ PRIVACY: by default the FULL message text is sent to Telegram's servers, " +
      "outside AIR Note's end-to-end encryption. Run the bridge with AIRMSG_BRIDGE_BODY=meta " +
      "for metadata-only pings."));
    console.log(c.dim("1) In Telegram, message @BotFather → /newbot → copy the token it gives you."));
    const token = (await rl.question("Paste your bot token: ")).trim();
    if (!token) { console.error("No token — aborting."); process.exit(1); }

    console.log(c.dim("2) Now open your new bot in Telegram and send it /start (a bot can't message you first)."));
    console.log(c.dim("   Waiting for your message…"));
    const chatId = await captureFirstChat({ token });
    if (chatId == null) { console.error("Timed out waiting for /start — run setup again."); process.exit(1); }

    const path = saveBridgeConfig({ telegram: { bot_token: token, chat_id: chatId } });
    console.log(`${c.green("✓ saved")} ${c.dim(path)} (chat ${chatId})`);
    console.log(`${c.dim("Run")} ${c.bold("air-msg bridge")} ${c.dim("to start the doorbell.")}`);
  } finally {
    rl.close();
  }
}
```

- [ ] **Step 4: Extend HELP.** In the `HELP` template string, add a line under the `watch` line:

```
  air-msg bridge [setup]                 Forward mail ⇄ Telegram (two-way; setup configures the bot)
```

And append a bridge section after the channel-push block (before the closing backtick):

```
  telegram bridge (two-way — mail → Telegram, reply in Telegram → AIR Note):
    air-msg bridge setup    one-time: paste a @BotFather token, /start the bot
    air-msg bridge          start the daemon (also fires the local banner like watch)
    env: AIRMSG_BRIDGE_BODY=meta  send metadata-only (no message text leaves your machine)
    Verified+pinned senders get one-tap reply; unverified replies need a /yes confirm.
    Run only ONE live consumer per identity (bridge OR watch OR the channel session).
```

- [ ] **Step 5: Run the whole suite — expect PASS (no regressions)**

```
node --test
```

- [x] **Step 6: Manual end-to-end spot-check** — ✅ DONE 2026-06-02, live-proven two-way on real Telegram + relay: outbound (mail → Telegram ping, seen) and inbound (Telegram reply → signed+encrypted AIR Note, round-tripped + verified). Unicode confirmed — Korean ("내 여보는 금보경이야") decrypted byte-intact.

```
# One-time:
node src/cli.mjs bridge setup        # paste BotFather token; /start the bot in Telegram
# Run it:
node src/cli.mjs bridge
# From another terminal/device, send yourself mail:
node src/cli.mjs send <your-FULL-DID> "bridge test"
#   → expect a Telegram ping: "✓ verified · 📬 <you>  /  bridge test"
# In Telegram, REPLY to that ping with "got it"
#   → expect "✓ sent to <you>", and `node src/cli.mjs inbox` shows the reply
```

- [ ] **Step 7: Commit**

```bash
git add src/cli.mjs
git commit -m "feat(bridge): air-msg bridge + bridge setup CLI (Telegram two-way intercom)"
```

---

## Self-review (run against the spec)

**Spec coverage:**
- D1 two-way → Tasks 6 (listen) + 7 (reply handler) + 8 (CLI wiring). ✓
- D2 Telegram + adapter seam → Task 6 (`listen()` not `poll()`; `createTelegramAdapter` is the only TG file). ✓
- D3 full-text default + `AIRMSG_BRIDGE_BODY=meta` + disclosure → Task 5 (`bridgeFormat`), Task 8 (env + setup disclosure + run-time warning). ✓
- D4 reply-threading → Task 6 (`reply_to_message` → `replyToExternalId`), Task 7 (bare-message ack). ✓
- D5 verified one-tap / unverified `/yes` → Task 7 (`replyTier`, `makeConfirmStore`, handler). ✓
- D6 superset of watch (local banner) → Task 8 (real notifier wired). ✓
- D7 moderation out of scope → nothing filters; forward-all-badge. ✓
- §9 trust model: route keyed to `m.from` (Task 7), badge crypto-only (Task 5), plain-text no parse_mode (Task 6), chat-id authn filter (Task 6). ✓
- §11 single-consumer lock → Tasks 1–2. ✓
- §12 separate `bridge_routes` + 30-day prune + per-ping keying → Task 3, pruned in Task 8. ✓
- Crash-safety / no double-send (send-then-advance) → Task 6 listen offset logic + Task 7 handler throwing on send failure. ✓

**Placeholder scan:** No "TBD"/"handle errors"/uncoded-test placeholders. Task-8 Step-1 spells out which imports already exist vs. which to add (prose, not a dummy import). Every code step shows complete code. ✓

**Type/name consistency:** `external_id` (string) everywhere; route shape `{platform, external_id, peer_did, contact, thread_id, envelope_id, verified, created_at}` identical across `bridge-routes.mjs`, `makeBridgeOutbound`, `makeReplyHandler`. Adapter contract `{name, send({title,body})→id, listen({signal,onReply})}`, `onReply({replyToExternalId, text, reply})` — matches producer (Task 6) and consumer (Task 7). `core.send({to, body, thread_id, in_reply_to})` matches `core.mjs:191`. ✓

---

## After the plan (execution-phase, per the #27/#29 rhythm)

- Build subagent-driven: a fresh implementer per task + a spec-review pass + a code-quality pass, then a **final whole-implementation Opus review** before declaring merge-ready.
- Whole suite green via `node --test` (single-file runs per task; `node --test` auto-discover for the full sweep — the `test/` dir form is broken on new Node).
- Rust workspace is untouched (this is JS-only); `cargo test -p air-rs` should remain green.
- Update `README.md` (bridge section) + a short note in the next session handoff (GBrain `air/…`).
