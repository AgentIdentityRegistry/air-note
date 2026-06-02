# Moderation (block / spam / delete) — Design

**Date:** 2026-06-02
**Status:** Revised after independent critic review → ready for implementation plan
**Repo:** `~/air-note` (canonical home for new AIR Note work — source-of-truth decision 2026-06-02, "a-lite")
**Feature:** give the user control over the *open* messaging surface. Anyone can ping you (the open doorbell — #27/#29/bridge); moderation lets you **lock the door on a sender (block)**, **report junk to the network (spam)**, and **clean your own diary (delete)**. The cryptographic verify+pin gate already protects the AI-context push (#29); this governs the *convenience* layer of everything else.

Continuation of the chat-app-bridge design's **D7** ("Spam/blocking is out of scope — that's the next feature (moderation)").

> **Revision note (post-review):** an independent review corrected a load-bearing error — the relay does **not** authenticate the sender. `sender_did` is the sender-controlled `envelope.from`, echoed by the relay (`air-site/relay/src/index.js:145`; its comment line 156: *"The relay can't verify the `from` field is real"*). So **block is a convenience filter keyed on a *claimed* identity, not a cryptographic boundary** (decision D12). All trust-model wording below reflects this. Other review fixes folded in: surface `envelope_id` to the user (was undriveable), batched drop-tally, replay-safe abuse-report contract, direction-scoped spam/delete, self-report refusal.

---

## 1. Goal

Three user actions over the messaging stack, each at a different point in the pipeline:

| Action | When | What it does |
|--------|------|--------------|
| **block** | proactive (before *or* after they message you) | A message whose **claimed sender** (`sender_did`) is blocked is **hard-dropped at the front door** of `receive()` — never decrypted, never archived, never surfaced on any channel (inbox, OS banner #27, AI-push #29, Telegram bridge). A tiny per-sender drop-tally (count + last-seen, **no content**) is kept for audit. **Caveat:** block matches the *claimed* sender; a forger who sets a different `from` is **not** suppressed (their mail lands `verified:false`). Block is a convenience filter, not a cryptographic wall — see D12. |
| **spam** | reactive (on a received message already in your diary) | **Hide** that message from inbox/history locally **and** fire a **cryptographically-signed, replay-safe abuse report** to AIR (private), behind a graceful seam. |
| **delete** | cleanup | Remove a single message, or a whole two-way conversation, from your **local diary only**. The relay is a transient byte-pipe with no "unsend" — deletion cannot reach the recipient's copy. |

---

## 2. Existing seams this builds on

Moderation adds **one** new enforcement point (block, in `receive()`) and otherwise extends existing stores. It adds **no** crypto/relay/transport logic.

- **`core.receive()`** (`core.mjs:238-342`) — the single pull→verify→decrypt→archive→advance-cursor chokepoint that **every** inbound surface (inbox, `watch` #27, `channel-server` #29, `bridge`) consumes (`watch.mjs:139`, `channel-server.mjs:41`, bridge, `cli.mjs:198`). The block check lives here, so **one insert covers all four inbound surfaces**.
  - Loop is `for (const m of batch.messages)` (`core.mjs:251`). Each `m` carries `m.sender_did`, `m.seq`, `m.envelope_id`, `m.envelope_b64`, `m.queued_at`.
  - **Trust reality:** `m.sender_did` is set by the relay from the **sender-controlled, unauthenticated** `envelope.from` (`air-site/relay/src/index.js:145`). It is the pipeline's routing key, but it is a **claimed** identity. The `core.mjs:262` guard `envelope.from !== m.sender_did` is a *consistency* check (relay echoed the same value), **not** authentication. Cryptographic trust comes only from the recipient's `verifyEnvelope` (`core.mjs:265`) + `checkPin` (`core.mjs:269`). Block therefore operates on the claimed sender (D12).
  - Cursor advance (`core.mjs:330-333`) uses `Math.max(...batch.messages.map(m => m.seq))` over **all** batch messages — so a `continue`-skipped blocked message still advances the cursor past itself. No re-delivery, no extra work. (Verified: the advance reads the raw `batch.messages`, not the filtered output array, and is unconditional on a non-empty batch.)
- **`archive.mjs`** — `node:sqlite` diary at `~/.air-msg/archive.db` (mode 0600); `messages` table PK `(envelope_id, direction)`; reads via `history({peer,thread,before,limit})` / `recentForInbox(limit)`; DDL run **one statement at a time** (`db.exec` is forbidden by a repo hook, `archive.mjs:19`). A self-sent message produces **two rows** (`sent` + `received`) — direction matters for spam/delete (see §6).
- **`contacts.mjs`** — JSON store keyed by **canonical DID** at `~/.air-msg/contacts.json`. The blocklist mirrors its load/save/0600 style but is a **separate store** (you block non-contacts too).
- **`attest()`** (`core.mjs:344-369`) — the proven pattern for the abuse report: build a **JCS-canonical** object, `signRaw(...)` with the identity key, POST to AIR with an `X-Agent-Secret` header. Note `attest()` refuses self-attestation (`core.mjs:351`) — the abuse report copies that self-guard.
- **`resolveRecipient()`** (`core.mjs:65`) + `didFromAirId()` — turn an alias / bare AIR-id / DID into a canonical DID. `block`/`unblock` reuse this so they accept the same peer inputs as `send`.

**Relationship to the existing `mute`:** `mute` (`AIRMSG_MUTE`) is **transient, env-var, notification-only** — a muted peer's mail is still received, decrypted, archived; you just don't get a banner/AI-push (`channel.mjs:14`, `watch.mjs:28`). **block** is **persistent, file-backed, hard-drop** — the mail never enters your machine. Complementary, not redundant. Block does not touch `mute`, and vice-versa.

---

## 3. Locked decisions

| # | Decision | Rationale |
|---|----------|-----------|
| D1 | **Scope = block + spam + delete** (all three this cut) | User choice. |
| D2 | **Block is a hard drop** — discard in `receive()` *before* decrypt/archive; cursor still advances | User choice. Protects the diary from a blocked sender flooding it. |
| D3 | **Drop-tally is advisory & spoofable** — per-sender count + `last_drop_at`, **never content** | Counts *claimed* attempts on a blocked DID. A third party can inflate it (forge `from`); the blocked party can dodge it (change `from`). Useful signal, not a guarantee — labelled as such in output. |
| D4 | **Block keys on the claimed `sender_did`**, checked before verify/decode | The pipeline's existing routing key; cheapest enforcement point. It is a *claimed* identity (D12), so block is convenience-grade. |
| D5 | **Block is its own store** `~/.air-msg/blocklist.json` (not an overload of contacts) | Block applies to non-contacts; keeps the contact book clean. |
| D6 | **Block is fail-open** — corrupt/missing blocklist → deliver mail, log error | Block is a *convenience* filter (D12), so the catastrophic failure ("a corrupt file silently black-holes ALL mail") is worse than "a blocked sender briefly gets through." The cryptographic controls (verify/pin) fail *safe* independently. Matches the repo's best-effort posture (`core.mjs:223`, `channel.mjs:56`). |
| D7 | **Spam = local hide + signed private abuse report** (not auto-block) | User choice. Block stays a separate explicit action. |
| D8 | **Abuse report → a distinct *private* endpoint** `POST /agents/{id}/abuse-reports`, aggregated server-side, never a public accusation | User choice ("whisper to the teacher"). Public abuse-attestations invite defamation/retaliation/brigading. |
| D9 | **Abuse report is behind a graceful seam** — build+sign always; any POST failure degrades to local-hide-only, never throws | Mirrors the #14 `backupArchive` no-op seam (`archive.mjs:133`). Server side deferred to the air-site trust-feedback-loop session; moderation ships independently. |
| D10 | **Delete = local diary only**; one message (both directions of its id) **or** a whole two-way conversation; confirm-gated | The relay can't unsend; guard against accidental data loss. |
| D11 | **Spam-flagged messages are hidden by default** from inbox/history; a `--include-spam` view reveals them | "Hide" must be reversible/auditable; spam is a flag, not a delete. |
| D12 | **Block is a convenience filter, NOT a security boundary** | The relay does not authenticate senders (AIR Principle 3 — recipient verifies). Block reliably stops an *honest* unwanted sender; a determined forger mints/forges identities and lands `verified:false`. The cryptographic shield is verify+pin (already gating #29). A "verified+pinned-only lockdown mode" is **deferred** (§11). |

---

## 4. Architecture overview

One chokepoint feeds every inbound surface, so block is enforced **once**:

```
                         ┌──────────────────────────────────────────────┐
   relay (pull/SSE) ──▶  │ core.receive()  ── THE single chokepoint       │
                         │   drops = new Map()                            │
                         │   for (m of batch.messages):                   │
                         │     ┌─ if isBlocked(m.sender_did):  ◀── BLOCK   │
                         │     │     drops[m.sender_did]++                 │
                         │     │     continue   (no decode/verify/archive) │
                         │     └─ else: verify → decode → archiveMessage   │
                         │   recordBlockedDrops(drops)   ── ONE write       │
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

- **block** = one insert at `core.mjs:251` + one batched tally write, backed by `moderation.mjs`.
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
- `recordBlockedDrops(countsByDid) → void` — **batched**: takes a `Map<did, count>` (one per `receive()` call), loads the store once, increments each `drop_count` + sets `last_drop_at`, writes **once**. Best-effort (swallow errors, log stderr; a failed tally must never break receive). *(Batching avoids rewriting the whole JSON per dropped message — important under a flood; D3.)*
- `block(peerInput) → {did, air_id, alias, already}` — resolve via `resolveRecipient`/`didFromAirId`, upsert (`blocked_at` set once).
- `unblock(peerInput) → {removed: boolean}`.
- `listBlocked() → [{air_id, alias, did, blocked_at, drop_count, last_drop_at}]`.
- `reportAbuse({subject_did, report_type}) → {reported: boolean, reason?}` — the signed-report seam (§7).

### 5.2 Archive spam flag — guarded migration (`archive.mjs`)

Add one column via an **idempotent** migration in `openArchive()` after `SCHEMA`:

```js
const cols = db.prepare(`PRAGMA table_info(messages)`).all().map(c => c.name);
if (!cols.includes("spam")) {
  db.prepare(`ALTER TABLE messages ADD COLUMN spam INTEGER NOT NULL DEFAULT 0`).run();
}
```

Single statement via `prepare().run()` (honors the no-`db.exec` rule); `ADD COLUMN ... NOT NULL DEFAULT 0` is legal in `node:sqlite` (non-null default supplied); re-runs are safe. `archiveMessage`'s explicit-column INSERT (`archive.mjs:57-61`) never names `spam`, so every insert takes the `DEFAULT 0` — no INSERT change needed. *(No new index: the diary is small; a `WHERE spam = 0` scan-filter on top of the existing time/peer indexes is negligible. If more migrations land later, switch to a `meta` schema-version sentinel instead of accreting PRAGMA checks.)*

---

## 6. The three flows

### 6.1 block (proactive)

Insert at the top of the receive loop (`core.mjs:251`), with a batched tally:

```js
const drops = new Map();
for (const m of batch.messages) {
  if (isBlocked(m.sender_did)) {
    drops.set(m.sender_did, (drops.get(m.sender_did) ?? 0) + 1);
    continue;                       // no verify/decode/push/archive
  }
  // ... unchanged verify/decode/push/archive ...
}
if (drops.size) recordBlockedDrops(drops);   // ONE write, after the loop
```

Skipped messages are **not** pushed to `messages[]` and **not** archived; the cursor advance (`core.mjs:330`) still passes them. Net effect across inbox/watch/channel/bridge: a blocked *claimed* sender vanishes (subject to D12).

**Ops** (`core.mjs`): `blockOp({peer})`, `unblockOp({peer})`, `listBlockedOp()` — thin wrappers over `moderation.mjs`. `blockOp` returns a note that unblocking cannot recover already-dropped mail (surfaced by the CLI at block time — NIT).

### 6.2 spam (reactive)

`reportSpamOp({ envelope_id })`:
1. Look up the **`received`** row by `(envelope_id, direction='received')` → get `peer_did` (the subject). Not found → error "no such *received* message in your diary". (You can't spam mail you sent.)
2. `reportAbuse({ subject_did: peer_did })` — refuses if `subject_air_id === my air_id` (self-report guard, mirroring `attest`); otherwise build+sign+POST behind the seam (§7).
3. `markSpam(envelope_id)` → `UPDATE messages SET spam = 1 WHERE envelope_id = ? AND direction = 'received'` (idempotent; received row only). The message immediately drops out of default inbox/history reads (§6.4).
- Return: `{ hidden: true, reported: <bool>, subject, reason? }`.

### 6.3 delete (cleanup)

- `deleteMessage(envelope_id)` → `DELETE FROM messages WHERE envelope_id = ?` — removes the message entirely from your diary (**both** `sent` and `received` rows if a self-message; normally just the one row). Returns `{ deleted: <n> }`.
- `deleteConversation({ peer })` → resolve peer→DID → `DELETE FROM messages WHERE peer_did = ?` — removes the **whole two-way history** with that peer (both your received mail *and* your sent replies, since both rows share `peer_did`; `core.mjs:213` & `:312`). Returns `{ deleted: <n> }`.
- `deleteOp({ envelope_id?, peer?, confirm })` (`core.mjs`): exactly one of `envelope_id`/`peer`; refuses without `confirm === true` (CLI maps `--yes`, MCP requires `confirm: true`). Deletion touches only local archive rows — it does not move the relay cursor, and the cursor's monotonic forward-only advance means deleted messages are not re-pulled.

### 6.4 spam-hidden reads (`archive.mjs`)

`history()` and `recentForInbox()` gain `includeSpam = false`; default adds `spam = 0` to the WHERE clause:

```js
export function history({ peer, thread, before, limit = 50, includeSpam = false } = {}) {
  // ...
  if (!includeSpam) where.push("spam = 0");
  // ...
}
```

`markSpam(envelope_id)`, `deleteMessage`, `deleteConversation` are new exports alongside the existing query helpers.

---

## 7. Abuse-report wire contract (FROZEN here; server impl deferred)

The **client** builds, signs, and POSTs this now. The **server** (AIR registry in `~/air-site`) implements receipt + private aggregation + negative trust weighting **later**, via the trust-feedback-loop session. Until then the seam degrades to local-hide-only (D9).

**Endpoint:** `POST {air_url}/api/v1/agents/{subject_air_id}/abuse-reports`
**Headers:** `content-type: application/json`, `X-Agent-Secret: <my agent_secret>`

**Body** (signature over the JCS-canonical form of all fields *except* `signature_multibase`, exactly like `attest()`):
```json
{
  "report_id":           "uuid-v4",            // client-generated; server DEDUP/replay key
  "version":             1,                    // contract version (so report_type can grow without a breaking change)
  "reporter_air_id":     "AIR-MINE",
  "subject_air_id":      "AIR-BAD",
  "report_type":         "spam",               // enum; future values gated by `version`: phishing, impersonation, abuse
  "reported_at":         "2026-06-02T14:30:00.000Z",
  "signature_multibase": "<sig over JCS(rest)>"
}
```

*(Dropped from an earlier draft: an `envelope_id` "evidence pointer" — the server cannot fetch the relay-GC'd message, so it carried zero evidentiary value and leaked which message was reported. Spam targets the **sender**, not a specific letter.)*

**Client behavior (`reportAbuse` in `moderation.mjs`):**
- Refuse self-report (`subject_air_id === reporter_air_id`) up front.
- Always build + sign (pure, testable). Generate a fresh `report_id` per call.
- POST best-effort. On **any** failure — network error, `404` (endpoint not built yet), `4xx`, `5xx` — log stderr and return `{ reported: false, reason }`. Never throw. The local spam-hide already applied (§6.2), so junk is gone from view regardless.
- On `2xx` → `{ reported: true }`.

**Server behavior (DEFERRED — air-site contract notes):** authenticate `reporter_air_id` via secret/signature; **reject a duplicate `report_id` from the same reporter** (replay defense — a signed report with only a timestamp is otherwise capturable + replayable to fake volume); store reports **privately** (not in the public attestation graph); aggregate per `subject_air_id`; require **multiple distinct reporters** before any score impact (anti-brigade, echoing the attestation model's "≥3 distinct roots"); rate-limit reports per reporter; feed a **negative** term into the trust score (the mirror of `peerAttestationsSubscore`).

---

## 8. Surfaces

### Prerequisite (BLOCKER fix): surface `envelope_id`
Spam/delete key on `envelope_id`, but today `inbox`/`history` don't print it (only `send` does, `cli.mjs:194`). **Task 0 of the plan:** show a short `envelope_id` prefix in `inbox`/`history` CLI output, and include the full `envelope_id` in the MCP `agent_receive`/`agent_history` JSON (it's already in the `receive()` result `core.mjs:299` and archive rows `archive.mjs:71` — just unprinted). Without this, the commands below are undriveable.

### CLI (`cli.mjs`) — peer = alias | DID | AIR-id
- `air-msg block <peer>`   (prints: unblocking cannot recover already-dropped mail)
- `air-msg unblock <peer>`
- `air-msg blocked` — list blocked senders + advisory drop-tallies
- `air-msg spam <envelope-id>` — hide + report
- `air-msg delete --message <envelope-id>` | `air-msg delete --with <peer>`  (refuses without `--yes`; `--with` reuses the existing `history --with` flag, `cli.mjs:214`)
- `inbox`/`history` gain `--include-spam`; help text updated.

### MCP tools (`index.mjs`)
- `agent_block { peer }`
- `agent_unblock { peer }`
- `agent_list_blocked {}`
- `agent_report_spam { envelope_id }`
- `agent_delete { envelope_id?, peer?, confirm }`  (exactly one of `envelope_id`/`peer`; `confirm` required)

---

## 9. Error handling & edge cases

- **Fail-open block (D6):** `isBlocked` returns `false` on any store-read error; `recordBlockedDrops` swallows write errors (logs stderr). Receive never breaks because of the blocklist.
- **Convenience-grade block (D12):** the §1 guarantee ("never surfaced on any channel") holds for messages whose *claimed* sender is blocked; a forger using a different `from` is not suppressed and lands `verified:false`. Do not describe block as a security control in user-facing text.
- **Idempotent migration:** the `spam` column add is PRAGMA-guarded; safe on every open.
- **Seam degradation (D9):** abuse-report POST failures never surface as errors; spam-hide still applies. Self-report is refused before any POST.
- **Delete guard (D10):** `deleteOp` refuses without explicit confirm; reports the deleted-row count.
- **Block ≠ contact removal:** blocking a known contact leaves them in `contacts.json` (pin intact); their mail is dropped. Unblocking restores delivery but **cannot recover already-dropped mail** (hard drop, by design; surfaced at block time — NIT).
- **Direction discipline:** spam targets the `received` row (you can't spam your own sent mail); `deleteMessage` removes all rows for an id; `deleteConversation` removes the two-way thread.
- **Outbound to a blocked peer:** *not* prevented in v1 (block is inbound-only). Out of scope (§11).

---

## 10. Testing (`node:test`, matching the 88-test suite)

- **moderation.mjs:** `block`/`unblock` round-trip; `isBlocked` hit/miss; `isBlocked` fail-open on a corrupt/missing store; `recordBlockedDrops` increments per-DID with a **single** write for a multi-message Map + sets `last_drop_at`; input resolution (alias/DID/AIR-id → same canonical DID); `listBlocked` shape; `reportAbuse` refuses self-report.
- **core.receive():** a blocked `sender_did` is dropped (not in `messages[]`, not archived); cursor **still advances** past it; tally batched once per call; a non-blocked message in the same batch is delivered normally. (Acceptance note, not a code test: a forged-`from` message whose claimed sender is *not* on the blocklist is delivered as `verified:false` — block is convenience-grade, D12.)
- **archive.mjs:** spam-column migration idempotent (run twice, column present, no error); `markSpam` flips only the `received` row; default `history`/`recentForInbox` exclude `spam = 1`; `includeSpam: true` reveals; `deleteMessage` removes by id; `deleteConversation` removes all rows for a peer and returns the count.
- **abuse report:** signed body is JCS-canonical incl. `report_id` + `version`, verifies against the identity key; seam returns `{reported:false}` (not a throw) on 404/network failure and `{reported:true}` on 2xx (fetch mocked); self-report rejected.
- **surfacing:** `inbox`/`history` output includes the (short) `envelope_id`; MCP returns the full id.
- **CLI/MCP wiring:** each command/tool calls the right op; `delete` refuses without `--yes`/`confirm`.

---

## 11. Out of scope / deferred

- **Verified+pinned-only "lockdown mode"** — the real cryptographic answer to "keep a hostile actor out" (drop all non-verified+pinned mail). Deferred; D12 documents why block alone can't do this.
- **Relay-side sender authentication** — the root cause of D12. Protocol/cross-repo change; not a v1 moderation task.
- **Server-side abuse-report endpoint + negative trust weighting** — air-site, via the trust-feedback-loop session (contract frozen in §7).
- **Auto spam detection (ML/heuristics)** — spam is a manual user action only.
- **Encrypt-at-rest for the diary** — gated on hardware keys (#19).
- **Remote/relay delete ("unsend")** — impossible with a transient byte-pipe.
- **Recovering hard-dropped mail on unblock** — by design, blocked mail is never stored.
- **Public abuse attestations** — explicitly rejected (D8).
- **Blocking outbound sends to a blocked peer** — inbound-only for v1.

---

## 12. File-by-file change summary

| File | Action | What |
|------|--------|------|
| `agent-bridge-mcp/src/moderation.mjs` | **create** | blocklist store (`~/.air-msg/blocklist.json`) + `isBlocked`/`recordBlockedDrops`(batched)/`block`/`unblock`/`listBlocked`; `reportAbuse` signed-report seam (self-report guard, `report_id`+`version`) |
| `agent-bridge-mcp/src/archive.mjs` | modify | guarded `spam` column migration; `markSpam` (received row); `deleteMessage`/`deleteConversation`; `includeSpam` filter on `history`/`recentForInbox` |
| `agent-bridge-mcp/src/core.mjs` | modify | block insert + batched tally in `receive()`; `blockOp`/`unblockOp`/`listBlockedOp`/`reportSpamOp`/`deleteOp` |
| `agent-bridge-mcp/src/cli.mjs` | modify | **surface `envelope_id` in `inbox`/`history`** (Task 0); `block`/`unblock`/`blocked`/`spam`/`delete` subcommands + `--include-spam` + help |
| `agent-bridge-mcp/src/index.mjs` | modify | return `envelope_id` in `agent_receive`/`agent_history`; `agent_block`/`agent_unblock`/`agent_list_blocked`/`agent_report_spam`/`agent_delete` tools |
| `agent-bridge-mcp/test/moderation.test.mjs` | **create** | block + drop-tally-batching + abuse-report-seam + self-report tests |
| `agent-bridge-mcp/test/archive.test.mjs` | modify | spam migration/flag/hide + delete tests |
| `agent-bridge-mcp/test/*` (receive/integration) | modify | block-drop + cursor-advance + `envelope_id`-surfaced test |
