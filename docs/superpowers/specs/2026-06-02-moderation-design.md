# Moderation (block / spam / delete) — Design

**Date:** 2026-06-02
**Status:** Approved (brainstorm) → ready for implementation plan
**Repo:** `~/air-note` (canonical home for new AIR Note work — source-of-truth decision 2026-06-02, "a-lite")
**Feature:** give the user control over the *open* messaging surface. Anyone can ping you (the open doorbell — #27/#29/bridge); moderation lets you **lock the door on a sender (block)**, **report junk to the network (spam)**, and **clean your own diary (delete)**. The verified+pinned gate already protects the AI-context push (#29); this governs everything else.

Continuation of the chat-app-bridge design's **D7** ("Spam/blocking is out of scope — that's the next feature (moderation)").

---

## 1. Goal

Three user actions over the messaging stack, each at a different point in the pipeline:

| Action | When | What it does |
|--------|------|--------------|
| **block** | proactive (before *or* after they message you) | A blocked sender's mail is **hard-dropped at the front door** — never decrypted, never archived, never surfaced on *any* channel (inbox, OS banner #27, AI-push #29, Telegram bridge). A tiny per-sender drop-tally (count + last-seen, **no content**) is kept for audit. |
| **spam** | reactive (on a message already in your diary) | **Hide** that message from inbox/history locally **and** fire a **cryptographically-signed abuse report** to AIR (private), behind a graceful seam. |
| **delete** | cleanup | Remove a single message, or a whole conversation, from your **local diary only**. The relay is a transient byte-pipe with no "unsend" — deletion cannot reach the recipient's copy. |

---

## 2. Existing seams this builds on

Moderation adds **one** new enforcement point (block, in `receive()`) and otherwise extends existing stores. It adds **no** crypto/relay/transport logic.

- **`core.receive()`** (`core.mjs:238-342`) — the single pull→verify→decrypt→archive→advance-cursor chokepoint that **every** surface (inbox, `watch` #27, `channel-server` #29, `bridge`) consumes. The block check lives here, so **one insert covers all four surfaces**.
  - Loop is `for (const m of batch.messages)` (`core.mjs:251`). Each `m` carries `m.sender_did` (the **relay-attested, trusted** sender identity — the code explicitly distrusts `envelope.from`, `core.mjs:262`), `m.seq`, `m.envelope_id`, `m.envelope_b64`, `m.queued_at`.
  - Cursor advance (`core.mjs:330-333`) uses `Math.max(...batch.messages.map(m => m.seq))` over **all** batch messages — so a `continue`-skipped blocked message still advances the cursor past itself. No re-delivery, no extra work.
- **`archive.mjs`** — `node:sqlite` diary at `~/.air-msg/archive.db` (mode 0600); `messages` table PK `(envelope_id, direction)`; reads via `history({peer,thread,before,limit})` / `recentForInbox(limit)`; DDL run **one statement at a time** (`db.exec` is forbidden by a repo hook, `archive.mjs:19`). Spam-flag, spam-hidden reads, and delete extend this.
- **`contacts.mjs`** — JSON store keyed by **canonical DID** at `~/.air-msg/contacts.json`. The blocklist mirrors its load/save/0600 style but is a **separate store** (you can block a non-contact).
- **`attest()`** (`core.mjs:344-369`) — the proven pattern for the abuse report: build a **JCS-canonical** object, `signRaw(...)` it with the identity key, POST to AIR with an `X-Agent-Secret` header. The abuse report reuses this exact crypto with a different (private) endpoint.
- **`resolveRecipient()`** (`core.mjs:65`) + `didFromAirId()` — turn an alias / bare AIR-id / DID into a canonical DID. `block`/`unblock` reuse this so they accept the same peer inputs as `send`.

**Relationship to the existing `mute`:** `mute` (`AIRMSG_MUTE`) is **transient, env-var, notification-only** — a muted peer's mail is still received, decrypted, and archived; you just don't get a banner/AI-push. **block** is **persistent, file-backed, hard-drop** — the mail never enters your machine. They are complementary, not redundant. Block does not touch `mute`, and vice-versa.

---

## 3. Locked decisions

| # | Decision | Rationale |
|---|----------|-----------|
| D1 | **Scope = block + spam + delete** (all three this cut) | User choice. |
| D2 | **Block is a hard drop** — discard in `receive()` *before* decrypt/archive; cursor still advances | User choice. Matches the iMessage mental model and protects the diary from a blocked flooder. |
| D3 | **Block keeps a tiny per-sender drop-tally** (count + `last_drop_at`), **never content** | Audit ("did my blocked stalker keep trying?") without storage cost or privacy leak. |
| D4 | **Block keys on the relay-attested `sender_did`**, checked before verify/decode | The only trusted routing key in the pipeline (`core.mjs:262`); cheapest correct point. |
| D5 | **Block is its own store** `~/.air-msg/blocklist.json` (not an overload of contacts) | Block applies to non-contacts too; keeps the contact book clean. |
| D6 | **Block is fail-open** — if the blocklist can't be read, mail is delivered (error logged) | A corrupt blocklist must never silently black-hole all of your mail. |
| D7 | **Spam = local hide + signed network report** (not auto-block) | User choice (the network-signal option). Block stays a separate explicit action. |
| D8 | **Abuse report → a distinct *private* endpoint** `POST /agents/{id}/abuse-reports`, aggregated server-side, never a public accusation | User choice ("whisper to the teacher"). Public abuse-attestations invite defamation/retaliation/brigading. |
| D9 | **Abuse report is behind a graceful seam** — build+sign always; any POST failure degrades to local-hide-only, never throws | Mirrors the #14 `backupArchive` no-op seam. The server side is deferred to the air-site trust-feedback-loop session; moderation ships independently. |
| D10 | **Delete = local diary only**, single message **or** whole conversation; destructive → requires confirm (or `--yes`) | The relay can't unsend; guard against accidental data loss. |
| D11 | **Spam-flagged messages are hidden by default** from inbox/history; a `--include-spam` view reveals them | "Hide" must be reversible/auditable; spam is a flag, not a delete. |

---

## 4. Architecture overview

One chokepoint feeds every surface, so block is enforced **once**:

```
                         ┌──────────────────────────────────────────────┐
   relay (pull/SSE) ──▶  │ core.receive()  ── THE single chokepoint       │
                         │   for (m of batch.messages):                   │
                         │     ┌─ if isBlocked(m.sender_did):  ◀── BLOCK   │
                         │     │     recordBlockedDrop(m.sender_did)       │
                         │     │     continue   (no decode/verify/archive) │
                         │     └─ else: verify → decode → archiveMessage   │
                         │   cursor = max(seq over ALL batch msgs)         │
                         └──────────────────────────────────────────────┘
                                          │ returns only non-blocked mail
              ┌───────────────┬───────────┴───────────┬───────────────────┐
              ▼               ▼                       ▼                   ▼
          inbox / history   watch (#27 banner)   channel (#29 AI-push)   bridge (Telegram)
              │
              ├─ history()/recentForInbox()  ── default WHERE spam = 0   ◀── SPAM (hide)
              └─ markSpam(envelope_id) ─▶ reportAbuse(subject) [seam]     ◀── SPAM (report)

          deleteMessage(id) / deleteConversation(peer)  ── archive rows only  ◀── DELETE
```

- **block** = one insert at `core.mjs:251`, backed by `moderation.mjs`.
- **spam** = an archive column + a read-filter + a signed-report seam.
- **delete** = archive `DELETE` helpers + a confirm guard.

---

## 5. Data model & stores

### 5.1 Blocklist — `~/.air-msg/blocklist.json` (new, `moderation.mjs`)

```json
{
  "version": 1,
  "blocked": {
    "did:wba:agentidentityregistry.org:agents:AIR-XXXX": {
      "air_id":      "AIR-XXXX",
      "alias":       "spammer-bob",          // best-effort: contact alias at block time, else null
      "blocked_at":  "2026-06-02T13:00:00.000Z",
      "drop_count":  3,
      "last_drop_at":"2026-06-02T14:22:10.000Z"
    }
  }
}
```

Keyed by **canonical DID** (same key space as `contacts.json` and `m.sender_did`), so `isBlocked(m.sender_did)` is a direct lookup. Saved 0600, `mkdir 0700`, identical to `contacts.mjs`.

`moderation.mjs` exports:
- `isBlocked(did) → boolean` — direct key lookup; **returns `false` on any read error** (D6 fail-open).
- `recordBlockedDrop(did) → void` — bump `drop_count` + set `last_drop_at`; **best-effort** (swallow errors, log to stderr; a failed tally must not break receive).
- `block(peerInput) → {did, air_id, alias, already}` — resolve via `resolveRecipient`/`didFromAirId`, upsert entry (`blocked_at` set once).
- `unblock(peerInput) → {removed: boolean}`.
- `listBlocked() → [{air_id, alias, did, blocked_at, drop_count, last_drop_at}]`.

### 5.2 Archive spam flag — guarded migration (`archive.mjs`)

Add to the diary a single column, via an **idempotent** migration in `openArchive()` after `SCHEMA`:

```js
// after the SCHEMA loop, before returning _db:
const cols = db.prepare(`PRAGMA table_info(messages)`).all().map(c => c.name);
if (!cols.includes("spam")) {
  db.prepare(`ALTER TABLE messages ADD COLUMN spam INTEGER NOT NULL DEFAULT 0`).run();
}
```

(Single statement via `prepare().run()`, honoring the no-`db.exec` rule. Re-runs are safe.)

---

## 6. The three flows

### 6.1 block (proactive)

- **Enforcement** — insert at the top of the receive loop (`core.mjs:251`):
  ```js
  for (const m of batch.messages) {
    if (isBlocked(m.sender_did)) { recordBlockedDrop(m.sender_did); continue; }
    // ... unchanged verify/decode/push/archive ...
  }
  ```
  Skipped messages are **not** pushed to `messages[]` and **not** archived; the cursor advance (`core.mjs:330`) still passes them. Net effect across inbox/watch/channel/bridge: the blocked sender vanishes.
- **Ops** (`core.mjs`): `blockOp({peer})`, `unblockOp({peer})`, `listBlockedOp()` — thin wrappers over `moderation.mjs`.

### 6.2 spam (reactive)

`reportSpamOp({ envelope_id })`:
1. Look up the archived row by `envelope_id` (direction `received`) → get `peer_did` (the subject). If not found → error "no such message in your diary".
2. `markSpam(envelope_id)` → `UPDATE messages SET spam = 1 WHERE envelope_id = ?` (idempotent). The message immediately drops out of default inbox/history reads (§6.4).
3. `reportAbuse({ subject_did: peer_did, envelope_id })` — build+sign+POST behind the seam (§7). Return whether the network report landed.
- Return: `{ hidden: true, reported: <bool>, subject, reason? }`.

### 6.3 delete (cleanup)

- `deleteMessage(envelope_id)` → `DELETE FROM messages WHERE envelope_id = ?` (all directions of that id). Returns `{ deleted: <n> }`.
- `deleteConversation({ peer })` → resolve peer→DID → `DELETE FROM messages WHERE peer_did = ?`. Returns `{ deleted: <n> }`.
- `deleteOp({ envelope_id?, peer?, confirm })` (`core.mjs`): exactly one of `envelope_id`/`peer`; refuses without `confirm === true` (CLI maps `--yes`, MCP requires `confirm: true`).

### 6.4 spam-hidden reads (`archive.mjs`)

`history()` and `recentForInbox()` gain an `includeSpam = false` option; default adds `spam = 0` to the WHERE clause:

```js
export function history({ peer, thread, before, limit = 50, includeSpam = false } = {}) {
  // ...
  if (!includeSpam) { where.push("spam = 0"); }
  // ...
}
```

`markSpam(envelope_id)` and `deleteMessage`/`deleteConversation` are new exports alongside the existing query helpers.

---

## 7. Abuse-report wire contract (FROZEN here; server impl deferred)

The **client** builds, signs, and POSTs this now. The **server** (AIR registry in `~/air-site`) implements receipt + private aggregation + negative trust weighting **later**, via the trust-feedback-loop session. Until then the seam degrades to local-hide-only (D9).

**Endpoint:** `POST {air_url}/api/v1/agents/{subject_air_id}/abuse-reports`

**Headers:** `content-type: application/json`, `X-Agent-Secret: <my agent_secret>`

**Body** (signature over the JCS-canonical form of all fields *except* `signature_multibase`, exactly like `attest()`):
```json
{
  "reporter_air_id":     "AIR-MINE",
  "subject_air_id":      "AIR-BAD",
  "report_type":         "spam",                       // enum; future: phishing, abuse, impersonation
  "envelope_id":         "<offending message id>",     // evidence pointer (optional but recommended)
  "reported_at":         "2026-06-02T14:30:00.000Z",
  "signature_multibase": "<sig over JCS(rest)>"
}
```

**Client behavior (`reportAbuse` in `moderation.mjs`):**
- Always build + sign (pure, testable).
- POST best-effort. On **any** failure — network error, `404` (endpoint not built yet), `4xx`, `5xx` — log to stderr and return `{ reported: false, reason }`. Never throw. The local spam-hide already happened (§6.2), so junk is gone from view regardless.
- On `2xx` → `{ reported: true }`.

**Server behavior (DEFERRED — air-site contract notes):** authenticate `reporter_air_id` via secret/signature; store reports **privately** (not in the public attestation graph); aggregate per `subject_air_id`; feed a **negative** term into the trust score (the mirror of `peerAttestationsSubscore`); rate-limit reports per reporter to prevent weaponizing the report channel. A subject's score impact requires multiple distinct reporters (anti-brigading), echoing the attestation model's "≥3 distinct roots".

---

## 8. Surfaces

### CLI (`cli.mjs`) — peer = alias | DID | AIR-id
- `air-msg block <peer>`
- `air-msg unblock <peer>`
- `air-msg blocked` — list blocked senders + drop-tallies
- `air-msg spam <envelope-id>` — hide + report
- `air-msg delete --message <envelope-id>` | `air-msg delete --with <peer>`  (refuses without `--yes`)
- Help text updated; `inbox`/`history` gain `--include-spam`.

### MCP tools (`index.mjs`)
- `agent_block { peer }`
- `agent_unblock { peer }`
- `agent_list_blocked {}`
- `agent_report_spam { envelope_id }`
- `agent_delete { envelope_id?, peer?, confirm }`  (exactly one of `envelope_id`/`peer`; `confirm` required)

---

## 9. Error handling & edge cases

- **Fail-open block (D6):** `isBlocked` returns `false` on any store-read error; `recordBlockedDrop` swallows write errors (logs stderr). Receive never breaks because of the blocklist.
- **Idempotent migration:** the `spam` column add is guarded by `PRAGMA table_info`; safe on every open.
- **Seam degradation (D9):** abuse-report POST failures never surface as errors; spam-hide still applies.
- **Delete guard (D10):** `deleteOp` refuses without explicit confirm; reports the deleted-row count.
- **Block ≠ contact removal:** blocking a known contact leaves them in `contacts.json` (pin intact); their mail is simply dropped. Unblocking restores delivery (but cannot recover already-dropped mail — hard drop, by design).
- **Spam idempotency:** re-marking an already-spam message is a no-op locally; the report may fire again (acceptable; server dedups/rate-limits).
- **Outbound to a blocked peer:** *not* prevented in v1 (block is inbound-only). Out of scope.

---

## 10. Testing (`node:test`, matching the 88-test suite)

- **moderation.mjs:** `block`/`unblock` round-trip; `isBlocked` hit/miss; `isBlocked` fail-open on a corrupt/missing store; `recordBlockedDrop` increments + sets `last_drop_at`; input resolution (alias/DID/AIR-id all map to the same canonical DID); `listBlocked` shape.
- **core.receive():** a blocked `sender_did` is dropped (not in `messages[]`, not archived); cursor **still advances** past it; drop-tally bumped; a non-blocked message in the same batch is delivered normally.
- **archive.mjs:** spam-column migration is idempotent (run twice, no error, column present); `markSpam` flips the flag; default `history`/`recentForInbox` exclude `spam = 1`; `includeSpam: true` reveals; `deleteMessage` removes one envelope; `deleteConversation` removes all rows for a peer and returns the count.
- **abuse report:** the signed body is JCS-canonical + verifies against the identity key; the seam returns `{reported:false}` (not a throw) on 404/network failure and `{reported:true}` on 2xx (fetch mocked).
- **CLI/MCP wiring:** each new command/tool calls the right op; `delete` refuses without `--yes`/`confirm`.

---

## 11. Out of scope / deferred

- **Server-side abuse-report endpoint + negative trust weighting** — air-site, via the trust-feedback-loop session (contract frozen in §7).
- **Auto spam detection (ML/heuristics)** — spam is a manual user action only.
- **Encrypt-at-rest for the diary** — still gated on hardware keys (#19).
- **Remote/relay delete ("unsend")** — impossible with a transient byte-pipe.
- **Recovering hard-dropped mail on unblock** — by design, blocked mail is never stored.
- **Public abuse attestations** — explicitly rejected (D8).
- **Blocking outbound sends to a blocked peer** — inbound-only for v1.

---

## 12. File-by-file change summary

| File | Action | What |
|------|--------|------|
| `agent-bridge-mcp/src/moderation.mjs` | **create** | blocklist store (`~/.air-msg/blocklist.json`) + `isBlocked`/`recordBlockedDrop`/`block`/`unblock`/`listBlocked`; `reportAbuse` signed-report seam |
| `agent-bridge-mcp/src/archive.mjs` | modify | guarded `spam` column migration; `markSpam`; `deleteMessage`/`deleteConversation`; `includeSpam` filter on `history`/`recentForInbox` |
| `agent-bridge-mcp/src/core.mjs` | modify | block insert in `receive()` loop; `blockOp`/`unblockOp`/`listBlockedOp`/`reportSpamOp`/`deleteOp` |
| `agent-bridge-mcp/src/cli.mjs` | modify | `block`/`unblock`/`blocked`/`spam`/`delete` subcommands + `--include-spam` + help |
| `agent-bridge-mcp/src/index.mjs` | modify | `agent_block`/`agent_unblock`/`agent_list_blocked`/`agent_report_spam`/`agent_delete` MCP tools |
| `agent-bridge-mcp/test/moderation.test.mjs` | **create** | block + abuse-report-seam tests |
| `agent-bridge-mcp/test/archive.test.mjs` | modify | spam migration/flag/hide + delete tests |
| `agent-bridge-mcp/test/*` (receive/integration) | modify | block-drop + cursor-advance test |
