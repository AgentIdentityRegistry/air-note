# Chat-App Bridge (Telegram v1) — Design

**Date:** 2026-06-02
**Status:** Approved (brainstorm) → ready for implementation plan
**Repo:** `~/air-note` (canonical home for new AIR Note work)
**Feature:** the universal, client-agnostic doorbell — forward incoming AIR Note mail out to an external chat app, and let the user reply from inside that chat app. Reaches humans behind Codex/Gemini/OpenClaw/etc., not just Claude.

---

## 1. Goal

A **two-way intercom** between AIR Note and an external chat app:

- **Outbound:** new mail arrives → a ping appears in the user's chat app showing who it's from, the message text, and a trust badge.
- **Inbound:** the user replies *inside* the chat app → the bridge signs + encrypts that reply and sends it back as a real AIR Note to the correct peer, in the correct thread.

v1 targets **Telegram** only, behind a thin adapter seam so Slack/Discord/WhatsApp/SMS can be added later without a rewrite.

---

## 2. Existing seams this builds on (no changes to messaging logic)

The bridge is a new *consumer* of the existing pipeline, exactly like the #29 channel-server. It adds **no** crypto/relay/messaging logic of its own.

- `core.receive({ since, limit })` — pull/verify/decrypt/archive/advance-cursor. Returns `{ messages, count, verified_count, cursor, has_more, my_did }`. Each message: `{ seq, from, contact?, envelope_id, received_at, verified, encrypted, key_changed?, verify_note?, body?, thread_id? }`.
  - `from` = `m.sender_did`, which the relay sets straight from the sender-controlled `envelope.from` — the relay does **NOT** authenticate it (`air-site/relay/src/index.js:145`; its own comment, line 156: *"The relay can't verify the `from` field is real"*). It is the pipeline's routing key, but a **CLAIMED** identity, not relay-verified. Cryptographic trust comes ONLY from recipient-side `verifyEnvelope` + `checkPin` (`core.mjs:265-281`): a forged `from` lands `verified:false` (the forger can't sign as the DID they claim). The `core.mjs:262` `envelope.from !== m.sender_did` guard is a *consistency* check (the relay echoed the same value), NOT authentication.
  - `contact` = the pinned alias; present **only** for a known/pinned contact.
  - The verified badge predicate is `verified && !key_changed` (`core.mjs:337`).
- `core.send({ to, body, thread_id, in_reply_to, plaintext })` — sign + encrypt + POST. Returns `{ ..., envelope_id, thread_id }`. Accepts `thread_id`/`in_reply_to` so a reply continues the **same** AIR thread.
- `watch({ signal, identity, notifier, openResolver, onMessage, ... })` — the doorbell loop (SSE-first + poll). For each fresh message it (a) rings the injected `notifier` and (b) calls `onMessage?.(m)` (fire-and-forget, `watch.mjs:65`). Coalesces bursts per-peer (`watch.mjs:27-45`).
- `notifier.mjs` — pluggable local "how to ring" seam (osascript/node-notifier/bell/none).
- `archive.mjs` — `node:sqlite` store at `~/.air-msg/archive.db` (mode 0600); `getCursor()`/`setCursor()`; best-effort writes.
- `channel.mjs` / `channel-server.mjs` (#29) — the sibling pattern: a standalone process runs `watch()` with a no-op notifier + a gated `onMessage` that pushes mail elsewhere. The bridge copies this shape and the **detached-promise** discipline (`channel.mjs:53-61`) for async work inside the sync `onMessage`.

**Shared-cursor constraint:** `receive()` advances a single global `pull_cursor` (`core.mjs:330-332`, `archive.mjs:getCursor/setCursor`). Therefore `watch`, `channel-server`, and `bridge` are **mutually exclusive** — only one live consumer per identity. See §11.

---

## 3. Locked decisions

| # | Decision | Rationale |
|---|----------|-----------|
| D1 | **Two-way** intercom (not one-way doorbell) | The headline value: reply without leaving the chat app. |
| D2 | **Telegram first**, adapter seam for the rest | `getUpdates` long-polling needs **no public server** — the daemon stays local. |
| D3 | **Full message text** in the ping **by default**; opt-**down** via `AIRMSG_BRIDGE_BODY=meta` | User's explicit choice for max convenience. See §10 for the privacy tradeoff + required disclosure. |
| D4 | **Reply-threading** is the routing UX (Telegram "reply to a message"), not `/reply <alias>` commands | Natural, and impossible to mis-send (a bare message never guesses a recipient). |
| D5 | **Reply-safety tiering:** forward ALL mail (see everything); **verified+pinned → one-tap reply**; **unverified → reply requires an explicit confirm** | Keeps the open doorbell while closing the reply-hijack hole (a private reply must not fire at an imposter on autopilot). |
| D6 | `air-msg bridge` is a **superset of `air-msg watch`** — it also fires the local OS banner | Nearly free (reuse the real notifier); one daemon does local banner + chat-app push. |
| D7 | Spam/blocking is **out of scope** — that's the next feature (moderation) | Clear boundary; v1 forwards-and-badges, doesn't filter. |

---

## 4. Architecture overview

A new `air-msg bridge` CLI subcommand runs a single long-lived daemon that mirrors `air-msg watch`. One process, **two concurrent loops** sharing one `AbortController`:

```
                        ┌─────────────────────────────────────────────┐
  relay (SSE/poll) ──▶  │ OUTBOUND loop = watch()                      │
                        │   notifier = real OS banner (D6)             │
                        │   onMessage(m) ─▶ bridge outbound hook:      │
                        │     gate/format ▶ adapter.send(ping)         │
                        │     store route: tgMsgId → {peer,thread,...} │
                        └─────────────────────────────────────────────┘
                        ┌─────────────────────────────────────────────┐
  Telegram getUpdates ─▶│ INBOUND loop = adapter.listen({onReply})     │
   (long-poll)          │   onReply({replyToExternalId, text, chatId}) │
                        │     filter chatId === savedChatId            │
                        │     look up route by replyToExternalId       │
                        │     tier-check (D5) ▶ core.send(...)          │
                        │     advance processed-update watermark       │
                        │     ack back into Telegram                   │
                        └─────────────────────────────────────────────┘
```

Both loops are started by `air-msg bridge`, share the abort signal, and run until SIGINT/SIGTERM (same lifecycle as `channel-server.mjs:33-45`).

---

## 5. Components & files (all under `~/air-note/agent-bridge-mcp`)

| File | Responsibility |
|------|----------------|
| `src/bridge.mjs` | Orchestration. Pure-ish: builds the outbound `onMessage` hook (gate → format → send → store route) and the inbound `onReply` handler (filter → route-lookup → tier-check → `core.send` → ack). Pure helpers (`bridgeFormat`, `badgeFor`, route-lookup) are exported for unit tests. No HTTP of its own — it drives the adapter + `core`. |
| `src/adapters/telegram.mjs` | The **only** Telegram-specific file. Implements the adapter interface (§6): `send(ping)` (HTTP `sendMessage`) and `listen({signal, onReply})` (HTTP `getUpdates` long-poll loop with persisted offset). All HTTP injected (`fetchImpl`) for tests. |
| `src/bridge-routes.mjs` | Routing-table data access on top of `archive.db`: `putRoute`, `getRoute`, `pruneRoutes`, plus the processed-update watermark `getUpdateOffset`/`setUpdateOffset`. Separate module so it doesn't pollute `archive.mjs` (the message diary). |
| `src/consumer-lock.mjs` | `~/.air-msg/consumer.lock` acquire/release (PID + 0600). Acquired by `bridge`, and **also wired into `watch` and `channel-server`** so a second consumer exits loudly instead of silently stealing mail. |
| `src/cli.mjs` (edit) | New `bridge` + `bridge setup` subcommands; HELP text. |
| `test/bridge.test.mjs`, `test/adapters.telegram.test.mjs` | New tests (see §15). |

---

## 6. Adapter interface (designed for push *and* poll platforms)

The seam is deliberately minimal — modeled on `notifier.mjs`'s single-method discipline. It does **not** leak Telegram's polling model, so Slack (Socket Mode / websocket) and Discord (Gateway / websocket) implement `listen()` however they like:

```js
// An adapter:
{
  name: "telegram",
  // Send one ping. Returns the platform's server-assigned message id (string).
  async send({ title, body, badge, meta }) -> externalMessageId,
  // Receive replies until `signal` aborts. Calls onReply per inbound reply.
  // Telegram implements this as a getUpdates long-poll; Slack/Discord as a socket.
  async listen({ signal, onReply }) -> void,
}
// onReply({ replyToExternalId, text, chatId }) — orchestrator-supplied callback.
```

Only `externalMessageId` (server-assigned, sender cannot forge it) is ever used as a routing key.

---

## 7. Data flow — OUTBOUND (mail → Telegram)

For each fresh `m` from `watch()`'s `onMessage(m)`:

1. **Body policy (D3):** `text = (AIRMSG_BRIDGE_BODY==="meta") ? "(open AIR Note to read)" : bodyText(m.body)`.
2. **Badge (anti-spoof):** `badge = (m.verified && !m.key_changed) ? "✓ verified" : "⚠️ UNVERIFIED"`, computed **only** from crypto fields — never from sender-controlled strings.
3. **Title:** `m.contact` (pinned alias) if present, else the short AIR-id of `m.from`. The badge is rendered in a **sender-unreachable prefix** so a display name like `"Alice ✓ verified"` cannot forge a check.
4. **Send as plain text — NO `parse_mode`** so body/name content cannot inject Markdown/HTML links or break the send. (Same lesson as the osascript escaping in `notifier.mjs:13-20`.) Untrusted-data framing (à la `channel.mjs:31-41`) is applied so the human also sees "external message."
5. `externalId = await adapter.send({ title, body: text, badge, meta })`.
6. **Store the route BEFORE any reply is possible** (it already is — send returns first): `putRoute(externalId, { peer_did: m.from, contact: m.contact ?? null, thread_id: m.thread_id ?? null, envelope_id: m.envelope_id, verified: m.verified && !m.key_changed, created_at })`.
7. **Burst control:** reuse `watch`'s per-peer coalescing so N messages from one peer become **one** ping (avoids Telegram rate-limit 429 + silent drops). The route stores the *latest* message's thread for that coalesced ping.

The async `adapter.send` runs via the detached-promise pattern so a Telegram failure logs and degrades but never rejects into the sync `onMessage` and crashes the watch loop (`channel.mjs:53-61`).

---

## 8. Data flow — INBOUND (Telegram reply → AIR Note)

`adapter.listen` long-polls `getUpdates`; for each update the orchestrator's `onReply` runs:

1. **Authn filter:** drop the update unless `chatId === savedChatId` (the single authenticated principal). Silently ignore strangers (§9).
2. **Reply-only routing:** require `replyToExternalId` (the Telegram `reply_to_message.message_id`). A bare (non-reply) message → ack `"↩️ reply to a specific message so I know who to send it to"` and stop. **Never guess a recipient.**
3. **Route lookup:** `route = getRoute(replyToExternalId)`. Miss (e.g. aged-out/post-restart) → ack `"That conversation is too old to reply to here — open AIR Note to reply."` and stop.
4. **Reply-safety tier (D5):**
   - `route.verified === true` → proceed (one-tap).
   - else (unverified) → require an explicit confirm: first reply attempt acks `"⚠️ This sender is UNVERIFIED. Reply anyway? Send /yes within 2 min."`; only a following `/yes` (still as a reply to the same ping) proceeds. Confirmation state is per-route, short-TTL, in-memory.
5. **Send:** `await core.send({ to: route.peer_did, body: text, thread_id: route.thread_id, in_reply_to: route.envelope_id })` — continues the same AIR thread.
6. **Order to avoid double-send (crash-safe):** `core.send` must **succeed**, *then* advance the persisted update offset/watermark, *then* ack into Telegram. Biases toward at-least-once (a possible dup on crash) over message loss — the safer failure for a human chat. A redelivered update at/below `getUpdateOffset()` is skipped (`core.send` is **not** idempotent — new id/nonce per call, `core.mjs:108-115`).
7. **Ack** shows the actual recipient: `"✓ sent to <alias or AIR-id>"` so the user can verify after the fact.

---

## 9. Trust & abuse model

- **Reply-target integrity (NOT from the relay):** routes are keyed by Telegram's server-assigned `message_id` → `m.sender_did` (= the sender-controlled `envelope.from`; the relay does NOT authenticate it). A sender CAN therefore steer where a reply goes — including forging `from` to an innocent third party's DID. The defense is **not** relay routing-key integrity; it is the **verify+pin gate**: a forged/unverifiable sender arrives `verified:false` → its route is stored non-one-tap → any reply is gated behind the explicit `/yes` confirm (D5) with an UNVERIFIED warning that names the misroute risk. Only a **verified + pinned** sender (one-tap) is cryptographically who they claim, so only a one-tap reply is safe to auto-route. (Body, display name, and Markdown still can't influence routing or forge the badge — those remain closed.)
- **Badge integrity:** badge derived solely from `verified && !key_changed`, placed where a sender's name cannot reach (closes badge spoof).
- **Injection:** plain-text sends (no `parse_mode`) neutralize Markdown/HTML/link injection from body or name.
- **Bot exposure:** Telegram bots are world-discoverable. Hard-filter every update to `savedChatId`; in BotFather disable group joins (`/setjoingroups` off) and enable privacy mode. The saved `chat_id` is the one authenticated principal.
- **Reply to unverified:** allowed only behind the explicit `/yes` confirm (D5).
- **Secrets:** bot token lives **only** in `~/.air-msg/bridge.json` (mode 0600, same discipline as `identity.json`/`contacts.json`). **Not** in sqlite, not in env-by-default.

---

## 10. Privacy posture (full-text default)

D3 means decrypted message bodies **routinely transit and are stored on Telegram's servers**, outside AIR Note's end-to-end encryption. This is the user's explicit, informed choice. To keep it conscious and reversible:

- `air-msg bridge setup` prints a **one-time disclosure**: *"Full message text will be sent to Telegram's servers, outside AIR Note's end-to-end encryption. Set `AIRMSG_BRIDGE_BODY=meta` for metadata-only pings."*
- `AIRMSG_BRIDGE_BODY=meta` switches to metadata-only (no body text leaves the machine); reply routing still works (you read in AIR Note, reply in Telegram).
- Documented in `air-msg help` and the README bridge section.

---

## 11. Single-consumer lock

`~/.air-msg/consumer.lock` holds the live consumer's PID (file mode 0600). `watch`, `bridge`, and `channel-server` all attempt to acquire on start; a second acquirer prints `"another live consumer (PID NNNN) holds the pull cursor — stop it first"` and exits non-zero. Stale-lock handling: if the recorded PID is not alive, reclaim. This converts today's silent, intermittent message loss into a loud, correct error. (Cross-cutting: touches `watch.mjs` + `channel-server.mjs`, not just the new bridge — flag to the planner.)

---

## 12. Routing table & watermark (`bridge_routes` in `archive.db`)

A **separate** table (not mixed into the human-readable `messages` diary):

```sql
CREATE TABLE IF NOT EXISTS bridge_routes (
  platform     TEXT NOT NULL,   -- "telegram"
  external_id  TEXT NOT NULL,   -- Telegram message_id (server-assigned)
  peer_did     TEXT NOT NULL,   -- relay-verified sender_did (reply destination)
  contact      TEXT,            -- pinned alias, if any
  thread_id    TEXT,            -- AIR thread to continue
  envelope_id  TEXT,            -- for in_reply_to
  verified     INTEGER NOT NULL,-- 1 = verified+pinned (one-tap), 0 = needs /yes
  created_at   INTEGER NOT NULL,
  PRIMARY KEY (platform, external_id)
);
-- meta key/value for the per-platform getUpdates offset watermark
```

- **Per-ping keying** (by `external_id`, not per-peer "latest") so two separate pings from the same peer keep two routes; replying to the older ping still routes correctly. (A burst coalesced into one ping = one route; §7.7.)
- **Growth bound:** `pruneRoutes` drops routes older than 30 days (mirrors the relay's 30-day window) or beyond a max count. Run on daemon start + periodically.
- **Best-effort writes**, like the rest of `archive.mjs`; a route write failure logs and degrades (that reply just can't route → graceful ack).

---

## 13. Config & setup flow

`air-msg bridge setup`:
1. Prompt for / accept the **BotFather bot token**.
2. Print the **privacy disclosure** (§10).
3. Wait for the user to send the bot `/start` (Telegram won't let a bot DM first); capture `chat_id` from the first `getUpdates`.
4. Save `{ telegram: { bot_token, chat_id } }` to `~/.air-msg/bridge.json` (mode 0600).
5. Confirm: *"Bridge ready. Run `air-msg bridge` to start the doorbell."*

`air-msg bridge`: load `bridge.json` → acquire consumer lock → start both loops. Missing config → friendly *"run `air-msg bridge setup` first."*

---

## 14. Error handling

Best-effort everywhere; a degraded external is **never** allowed to crash the daemon or drop AIR messaging:
- Telegram send failure / bad token / network → log to stderr, degrade (local banner still fires via D6), keep running.
- `getUpdates` errors → backoff + retry (mirror `watch`'s SSE backoff discipline).
- Archive/route write failure → log, degrade (reply may not route → graceful ack).
- Async work inside the sync `onMessage` uses the detached-promise + `.catch(log)` pattern (`channel.mjs:53-61`).
- Clean shutdown on SIGINT/SIGTERM via the shared `AbortController`; release the consumer lock.

---

## 15. Testing strategy (mirrors #27/#29; whole suite stays green via `node --test`)

- **Pure unit tests** (no network): `badgeFor` (verified/unverified/key-changed; spoof-name cannot forge ✓), `bridgeFormat` (meta vs full body; plain-text/no-markup), route put/get/lookup, prune by age/count, the reply-safety tier decision, the bare-message/route-miss/stranger-chat branches.
- **Adapter tests with injected `fetchImpl`:** `send` posts the right `sendMessage` payload and returns the message id; `listen` parses `getUpdates`, advances offset correctly, dedups a redelivered update, filters foreign `chatId`, and surfaces `reply_to_message` as `replyToExternalId`.
- **Order/crash test:** simulate "send succeeded, advance failed" → redelivered update is skipped (no double-send); "send failed" → offset NOT advanced (no loss).
- **Consumer-lock test:** second acquirer detects the live PID and exits; stale PID is reclaimed.
- **No live Telegram calls in CI.** A manual spot-check script documents the real end-to-end check.

---

## 16. Out of scope (v1) / future

- **Slack, Discord, WhatsApp, SMS** — later adapters. Slack/Discord = websocket `listen()`; WhatsApp/SMS = public webhook + paid Twilio/Meta (a separate, heavier project).
- **Spam / block / delete (moderation)** — the next roadmap feature; v1 forwards-and-badges only (D7).
- **Group/multi-recipient threads** — single-peer reply routing in v1.
- **Per-contact body policy** — only the global `AIRMSG_BRIDGE_BODY` switch in v1 (YAGNI).
- **Reply-to-a-stranger without confirm** — never in v1 (D5).

---

## 17. Open questions (resolve during planning)

1. `core.send` `body` shape — confirm whether it wants a raw string or a `{type:"text",text}` (the plan will check `wrapBody`/`buildOutboundEnvelope`).
2. Exact long-poll timeout + backoff numbers for `getUpdates` (start ~25s long-poll, short backoff on error).
3. Whether the consumer lock lands as part of this feature or as a tiny standalone precursor PR (it touches `watch`/`channel-server`).
