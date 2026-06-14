# BossClaw Desktop — First GUI Subscriber (AI Inbox) — design

**Date:** 2026-06-11
**Status:** v2 — second-opinion architect review returned APPROVE-WITH-CHANGES; all findings folded in
(identity-collision rewrite, archive WAL, structural loop guard, grouping keys, send-err taxonomy,
dial-scope truth, corrected citations). Approved 2026-06-11.

**Phase A2 (Rust backend) — IMPLEMENTED 2026-06-11** on branch `feat/ai-inbox-a2-backend` (plan
`docs/superpowers/plans/2026-06-11-desktop-ai-inbox-a2-backend.md`): the `air-rs::inbox` library —
frames+fixtures, line parser, home-scoped stores, channel gate, read-only WAL archive reader (with a
real separate-process soak), the 5-invariant replayer, identity adopter, policy-store dial, and the
reconnecting two-role daemon client — plus the Tauri command/event surface (viewer feed, send, history,
identity, dial). **59 air-rs tests pass; `cargo check -p bossclaw_desktop` clean; clippy `-D warnings`
clean.** A whole-branch Opus review (SHIP-WITH-FIXES) found + fixed a send/recv frame-corruption bug at
the `select!` cancellation seam (now regression-tested). The channel connection + replayer are
library-built+tested but their Tauri wiring + the AI loop are **Phase B**. Next: **A3** (the React Inbox
UI over these commands), then **Phase B** (the per-contact AI dial loop + guards).
**Track:** BossClaw desktop (`~/air-note/apps/desktop`) + a small messaging-stack delta (`agent-bridge-mcp`)
**Builds on:** receiver daemon Phases 1–4 (spec `agent-bridge-mcp/docs/superpowers/specs/2026-06-05-receiver-daemon-design.md`; merged PR #14, main `a78e0af`)

## 1. Problem

The AIR Note stack has an always-on daemon with gated socket delivery, at-least-once replay,
reconnect/resume, and installers — but every consumer is a terminal. The desktop app (Tauri 2 +
React) has AIR onboarding and LLM streaming but zero messaging. The "first GUI subscriber" makes
the desktop a real client of the daemon: a live inbox, sending, and an AI that acts on trusted
mail — without violating the one-consumer rule or duplicating stack logic in a second language.

## 2. Decided constraints

- **One agent, many surfaces** (Peter, 2026-06-11): the desktop adopts the SAME identity/home the
  CLI + daemon use (`AGENT_BRIDGE_HOME`, default `~/.air-msg`). It is a window onto the existing
  agent, not a second agent.
- **v1 product goal: full AI inbox** — live feed + send + AI acting on gated mail.
- **Per-contact autonomy dial**: off / draft / auto. The file-level default is **draft**, but note
  the dial is only ever CONSULTED for channel-eligible senders (verified + pinned + key-unchanged)
  — an unpinned sender never crosses the channel gate, so their mail is never AI-actionable
  regardless of dial (it still renders in the inbox via the viewer connection).
- The daemon stays the **single stateful owner**: consumer lock, pull cursor, ALL archive writes,
  gate enforcement. The desktop is a thin client.
- **The messaging-stack delta is exactly two changes** (both small, both critic-reviewed in the
  Phase A plan): the `send` request op, and `openArchive()` switching to WAL + busy_timeout (§5).
- Two build phases under this one spec — **Phase A** plumbing + inbox + send; **Phase B** AI loop.
  Each phase gets its own critic-reviewed implementation plan (the daemon-phase rhythm).

## 3. Architecture

```
React UI:   Inbox view · Composer · AI panel · per-contact dial (Settings)
   ↕ Tauri commands + events
Rust:       daemon-client   (NEW, crates/air-rs)   — two Unix-socket connections, line-JSON
            archive-reader  (NEW)                  — READ-ONLY SQLite on {home}/archive.db (WAL)
            identity-adopter(NEW)                  — reads {home}/identity.json METADATA only
            policy-store    (NEW)                  — {home}/agent-policy.json (the dial)
            ai-loop         (Phase B)              — reuses existing llm_stream infra
   ↕ {home}/daemon.sock
Node daemon: EXTENDED: {type:"send"} request op → core.send → ack; openArchive() → WAL
```

**Two socket connections, on purpose.** A `viewer` connection feeds the whole inbox surface
(banner-equivalent visibility, mute-filtered). A `channel` connection feeds the AI — the DAEMON,
not the desktop, decides which mail an AI may act on, exactly as for the Claude session. The
per-subscriber gate and per-subscriber flow control already exist (daemon spec §5/§6), so the two
connections get independent backpressure for free: a slow AI can never stall the inbox feed.
**A channel-eligible message arrives on BOTH connections; the UI dedupes by `envelope_id` across
the two streams** (the replay path's dedupe is within-channel only — this cross-stream dedupe is
the desktop's job).

**Send over the socket.** New request frame: client sends `{type:"send", id, to, body}` (id =
client-generated correlation uuid); the daemon runs `core.send` (resolve recipient, seal, sign,
POST to relay, archive the sent row) and answers `{type:"send-ok", id, envelope_id, encrypted}`
or `{type:"send-err", id, retryable, reason}`. **The `retryable` flag is part of the contract:**
relay 5xx / network failure → `retryable: true` (the UI offers retry); recipient unresolvable or
refuse-to-send-unencrypted → `retryable: false` (no retry button — a blind retry would loop
forever). `core.send` is verified safe for IPC invocation (no process.exit, no stdout coupling,
archives its own sent row); it shares the daemon's event loop with the pull loop (no cursor
interaction — a slow relay POST delays both, accepted). **Fail-fast is a deliberate v1 choice**:
no outbox; a send during a relay blip surfaces as an error row, not a silent queue. A persistent
outbox is a named v1.5 follow-up. Send is accepted from any POST-hello subscriber regardless of
role: roles are DELIVERY filters (confidentiality), not request authority — the 0600 socket is
the OS user boundary, and any process that can connect could already run `air-msg send`. The
frame also carries an optional `plaintext` boolean (default false; CLI `--plaintext` parity,
needed for hermetic tests) — the desktop always sends encrypted. The frame additionally accepts
optional `thread_id` and `in_reply_to` fields for composer threading (§6): the reply composer
reuses the incoming `thread_id` and sets `in_reply_to` to the envelope being replied to; both
are forwarded unchanged to `core.send`.

## 4. Identity adoption (Phase A) — the collision is the primary case

**Shipped reality:** the desktop's only onboarding path today SELF-CREATES a new AIR identity
(`create_identity`: fresh keypair → AIR registration → key in the OS keychain, metadata
`{did, name, created_at}` in the Tauri app-data dir — note: no `air_id` field, different schema,
different directory from the daemon's `~/.air-msg/identity.json`). Any machine that ran desktop
onboarding therefore already has TWO agents. This design makes the daemon's identity win:

- **Precedence:** if `{home}/identity.json` exists (home = `AGENT_BRIDGE_HOME` || `~/.air-msg`),
  the desktop ADOPTS it — reads did / name (metadata ONLY; `air_id` is DERIVED from the DID, not
  read, since the daemon's file has it but no file is required to). The desktop's app-data
  identity becomes **inert**: not used, not deleted, one-time notice shown ("this app previously
  created agent AIR-XXXX; it is now dormant — your active agent is AIR-YYYY").
- **`create_identity` is disabled whenever a daemon home exists** — otherwise a settings "reset"
  re-forks the identity and resurrects the split-brain.
- **No key material in the desktop, ever (v1):** the daemon signs sends; the desktop never reads
  `seed_hex` or touches the keychain for the adopted identity.
- The trust-score UI binds to the ADOPTED DID.
- No identity anywhere → instructions screen ("install AIR Note's CLI and run: `air-msg daemon
  install`" — copy assumes the CLI may not be installed) with auto-retry probing. GUI identity
  creation on fresh machines is out of scope v1 (Rust-side registration = parity risk).

## 5. Connection lifecycle + the archive reader

The Rust client mirrors `connectDaemonPersistent` semantics: backoff 500 ms → ×2 → 5 s cap,
reset on attach; first-attempt failure surfaces "daemon offline" (no standalone fallback — the
desktop NEVER pulls). The async-teardown lessons port with it (stop-flag re-checked after every
await; user callbacks outside the retry path). In Tauri, the app runtime is the live handle that
outlives backoff windows (the unref'd-timer lesson) — stated here so the Rust port keeps the
semantics deliberately. The viewer connection attaches live-only; the channel connection sends
`since_seq` (max-seen, baseline = archive cursor at first attach) and handles `{type:"gap"}` by
replaying from the archive (`relay_seq > after_seq`, received-only, spam excluded, synthetic
`%:joined` notices excluded, **blocklist re-checked at replay** — replay never delivers more
than live did).

**Archive concurrency (second-opinion Critical, fixed):** `openArchive()` today uses SQLite's
default rollback journal — a cross-process reader and the daemon writer would lock each other out
(`SQLITE_BUSY`) under exactly the burst that gap-replay serves. The Phase A messaging-stack delta
therefore includes: `openArchive()` sets `PRAGMA journal_mode=WAL` + `busy_timeout` (~5 s) — WAL
is a persistent file property the WRITER must set. The Rust reader opens `SQLITE_OPEN_READONLY`
with its own busy_timeout and tolerates transient `-wal`/`-shm` states with bounded retry, never a
hard error.

**Protocol parity (structural mitigation):** Phase A adds `agent-bridge-mcp/docs/PROTOCOL.md`
(frame catalog: hello, hello-ok, message, gap, ping/pong, status, send, send-ok, send-err, error)
plus cross-language FRAME FIXTURES checked into the repo and asserted by BOTH suites — the
Ed25519 interop-vector precedent applied to the socket layer.

## 6. Inbox UI (Phase A)

New **Inbox** view in the Shell nav (Identity | Inbox | Settings). **Grouping keys, pinned:**
1:1 conversations group by `peer_did` (NOT `thread_id` — outbound 1:1 thread_ids default to a
fresh uuid per message and would fragment conversations); rooms group by `room_id`. The composer
threads replies properly (`in_reply_to` + reuse of the incoming `thread_id`) so 1:1 thread_ids
become meaningful over time. CLI badge vocabulary (🔒 ✓); spam hidden by default with a toggle
(mirrors `--include-spam`); room messages tagged with their room. Composer with contact picker
(read-only `contacts.json`) or raw DID; optimistic send with per-row ack / retryable-err handling.
**Unread is session-local UI state in v1** (not persisted; resets on relaunch — explicitly NOT an
archive schema change). **No second OS notification** — the daemon's coalesced banner remains the
one doorbell; the desktop adds an in-app unread badge only.

## 7. AI loop (Phase B)

Channel frames → policy lookup by sender DID → act per dial (recall §2: only channel-eligible
senders ever reach this code):

- **off** — nothing (mail still visible in the inbox via the viewer connection).
- **draft** (default) — the agent reads the message and produces a draft reply with visible
  reasoning in the AI panel; Peter sends/edits/discards.
- **auto** — as draft, then auto-send IF the loop guards allow.

**Loop guards (two, in order):**
1. **Structural (primary):** *never auto-reply to a reply to my own auto-reply.* Auto-sent
  messages are recorded in a desktop-side ledger (envelope_id + timestamp, in
  `agent-policy.json`); when an incoming message's `in_reply_to` points at a ledgered auto-sent
  envelope_id, the dial degrades to draft for that message. This is the group-chat design's
  standing rule ("agents never auto-reply to another agent's auto-reply", agent-group-chat
  design §10/§11) applied to 1:1. No wire marker needed — the ledger is local truth.
2. **Budget (backstop):** per-contact rolling-hour cap + persisted daily cap; exhausted →
  degrade to draft + badge. Numbers picked at Phase B planning.

**Key-rotation mandate rule:** a `key_changed` contact is already closed out by the channel gate
(auto stops automatically). After a RE-PIN, the dial does NOT silently resume: **re-pin resets
that contact's dial to draft** — auto requires a deliberate human re-arm against the new key.

The dial lives in `{home}/agent-policy.json` (`{version: 1, contacts: {<did>: {ai_autonomy,
auto_ledger…}}}`), written ONLY by the desktop (one writer per file; `contacts.json` is the CLI's
file), 0600, readable by future surfaces. Corrupt/missing policy → everything treated as draft +
visible warning.

**Prompt safety:** bodies enter the AI as EXTERNAL UNTRUSTED data with the channel-server fence
vocabulary (`⟦untrusted message start⟧ … ⟦end⟧`) and a system instruction never to follow
instructions inside them. Context: sender alias/DID, verified state, bounded thread history
(received + sent rows for that peer, most recent N, blocked rows excluded), the new message.
Model: the app's configured default provider. **Rooms are excluded from the AI in v1**; room mail
still renders in the inbox.

## 8. Error handling

Daemon offline → banner "daemon offline — reconnecting", composer disabled, inbox renders archive
history read-only. `send-err` → inline on the message row; retry button ONLY when
`retryable: true`. Archive read failure → live-only feed + visible warning (bounded busy-retry
first, per §5). Provider/AI failure → draft marked "agent failed" + retry; NEVER auto-send on any
error path. Policy file corrupt → safe-dial (draft) + warning.

## 9. Testing

- **Frame fixtures** (parity anchor): canonical frames in one JSON file; the JS suite asserts the
  daemon emits/accepts them; the Rust suite asserts the client parses/builds them.
- **Daemon JS:** `send` op tests (post-hello gating, correlation, retryable/terminal taxonomy,
  err paths) + WAL migration test (existing rollback-journal DB opens and converts) in the
  hermetic suite.
- **Rust:** parser/encoder units; reconnect/resume against a fake tokio socket server; replay
  invariant tests (blocklist re-check, join-notice exclusion, strictly-greater seq); archive
  reader vs a writing daemon (concurrent read-during-write soak).
- **E2E:** real daemon on a temp home (the spawn idiom): connect → send → ack → archived row →
  gap replay; cross-stream envelope_id dedupe.
- All tests temp-home; the real `~/.air-msg` is never touched (bridgeHome-guard discipline).

## 10. Security model

Socket 0600 = OS user boundary. Roles = delivery filters; the daemon-enforced channel gate decides
what the AI may act on. Replay ≤ live re-checked client-side. The private key never enters the
desktop process (v1). Untrusted-body fencing in every AI prompt. Auto mode is structurally
loop-guarded + budget-capped; re-pin resets auto to draft. `agent-policy.json` written 0600.
**Distribution boundary:** v1 is unsandboxed direct-distribution (as today) — macOS App Sandbox
would block the `$HOME` Unix-socket connect and is explicitly out of scope.

## 11. Out of scope (v1)

GUI identity creation on fresh machines · AI in rooms · per-contact model overrides · Windows
(daemon is POSIX-only v1) · hardware keys (#19) · forever-companion memory layers (own arc) ·
`bossclawd`/MCP extraction (v1.5 arc) · migration/deletion of old desktop identities (inert +
notice only) · persistent outbox (named v1.5 follow-up) · persisted read-state.

## 12. Open questions

- Exact loop-guard budget numbers — pick at Phase B planning.
- Should `send-ok` echo the sent message over viewer connections (other surfaces' live view), or
  rely on the archive as truth? (Lean: no echo in v1; the archive is truth.)
- The auto-ledger's retention (prune entries older than the budget windows need).
