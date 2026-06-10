# AIR Note receiver daemon (`air-msgd`) — design

**Date:** 2026-06-05
**Status:** Draft (brainstormed + independently reviewed by an architect pass and an adversarial critic pass; revised to address 1 Critical + 4 High findings before this spec was written)
**Track:** BossClaw messaging (`~/air-note`)
**Supersedes scope of:** the manual launchd snippet in `air-msg help` (which stays as the v1 fallback)

## 1. Problem

A relay identity may have only **one live consumer** of its pull cursor at a time
(`consumer-lock.mjs` PID lock; one `pull_cursor` in `archive.mjs`). Today three features
each acquire that lock and run their own pull loop, so only one can run at once:

- `watch` — `watch.mjs` SSE/poll → `core.receive()` → OS banner.
- `channel-server.mjs` — `watch()` with `onMessage = makeChannelPush` (pushes incoming mail
  into a live Claude Code session via the experimental `claude/channel` capability;
  gate = verified + pinned).
- `bridge` — `watch()` with `onMessage = makeBridgeOutbound` + an inbound Telegram listener.

Two consequences: (a) a user must remember to start a consumer (the "forgot to run `watch`"
gap a real two-party dry-run exposed), and (b) the human banner, AI-push, and Telegram are
**mutually exclusive** — you cannot have your AI auto-handle messages *and* get a banner *and*
forward to Telegram, because each wants the single lock.

**Key insight the design rests on:** all three are the *same* `watch()` loop with a different
single `onMessage` sink. `watch()` is already a one-subscriber pub/sub engine. The fix is not a
rewrite — it is "run `watch()` **once** in a daemon and fan its output out to **N** sinks."

## 2. Goals / non-goals

**Goals (v1):**
- One always-on process owns the single relay connection + lock + cursor.
- Human banner + AI-push (channel) + Telegram all run **simultaneously** off that one pull.
- Make always-on the default via a one-command auto-start on login (POSIX).
- No security regression versus today's in-process gating.

**Non-goals (explicitly deferred to v2):**
- **Windows** — the daemon code may run, but the local socket and any auto-start installer are
  out of v1. `node:net` exposes no way to set a Windows named-pipe ACL from stdlib, so the
  "only the owner can connect" guarantee is unverifiable there, and we have no Windows host to
  test on. POSIX-only in v1.
- Outbound `send` — no consumer conflict (stateless POST); unchanged.
- Remote / network access to the daemon — local-only by design.
- Multi-identity orchestration beyond "one daemon per `AGENT_BRIDGE_HOME`."

**Prerequisite (tracked separately):** `core.receive()` currently fetches only the first relay
page and relies on the caller to loop while `has_more` is true (`core.mjs` ~L534). The daemon's
single pull loop MUST drain `has_more` before it ships, or a burst larger than one page lags.
This is an existing bug independent of the daemon; fix it first.

## 3. Architecture

| Module | Role |
|---|---|
| `daemon.mjs` *(new)* | Long-lived process. Owns identity, holds the single `consumer.lock`, runs **one** `watch()` loop, and fans each received message out to all sinks. |
| `daemon-ipc.mjs` *(new)* | Unix-domain-socket server + line-delimited-JSON protocol + a `connectDaemon()` client helper. **Enforces the per-subscriber gate** (see §5). |
| Built-in sinks *(reuse)* | `notifier.mjs` (banner) and Telegram (`makeBridgeOutbound` + inbound listener) run **in-process** in the daemon when enabled in `daemon.json`. |
| Dynamic sinks | Each socket connection is a subscriber that declared a **role** at connect time. |
| `channel-server.mjs` *(refactor)* | Becomes a **thin client**: validate + connect the daemon socket FIRST, then `server.connect(transport)`, then receive role-gated mail and push into the session. Holds **no** lock. |
| `cli.mjs` *(extend)* | `air-msg daemon <start\|stop\|status\|install\|uninstall>`. (One name: `air-msg daemon`, not a separate `air-msgd` binary.) |
| `service/{launchd,systemd}.mjs` *(new)* | Generate + load the per-OS auto-start unit (macOS launchd, Linux systemd-user). |

One daemon per identity (per `AGENT_BRIDGE_HOME`); its socket, lock, and PID file all live in
that home dir.

## 4. Data flow

```
relay → daemon watch() (SSE/poll, drains has_more)
      → core.receive() (verify + decrypt + archive + advance the ONE cursor)
      → for each message m:
          • call each in-process sink (banner / telegram) — best-effort
          • for each connected socket subscriber: IF the daemon's role-gate for that
            subscriber admits m, write {type:"message", ...m} to it
```

Late subscribers get **live-from-attach**; history/backlog comes from the archive (see §6), the
daemon never replays its own live stream.

## 5. Security — the per-subscriber gate (the heart of the revision)

The first draft made the daemon a "dumb fan-out" that wrote every decrypted message to every
connection, with each *client* choosing whether to apply its gate. **That is a confidentiality
breach:** any local process (an `npm` postinstall, an editor extension, a second agent) could
connect and read the full decrypted plaintext of every message — including unverified or
key-changed (possible-impersonation) senders that the channel gate exists specifically to
withhold. The gate would be advisory, which is strictly worse than today's in-process gating.

**Resolution — the daemon enforces, the client does not choose.** The connect handshake declares
a **role**, and the daemon applies that role's filter *before* writing to that socket:

| Role | Daemon-enforced filter before write |
|---|---|
| `channel` | `verified && pinned (has contact alias) && !key_changed` — the existing `channelGate` policy, applied IN the daemon. |
| `bridge` | bridge's own tiering (verified+pinned → one-tap; else confirm) — policy moves into the daemon's bridge sink, which is in-process anyway. |
| `viewer` | banner-equivalent: all non-blocked, non-muted (a passive `watch` UI). Plaintext, but only mail the in-process banner would already surface. |

Decrypt-once is preserved; the plaintext that crosses the socket to a `channel` client is exactly
the subset that crosses into a Claude session today — no more. The socket file lives in the
`0700` home dir at `0600`; binding refuses if `AGENT_BRIDGE_HOME` is group/other-writable (a
shared-path home is unsupported), and the daemon re-stats the bound socket's owner+mode after
`listen` (guards a TOCTOU bind-hijack). The untrusted-body fence (`channel.mjs`) is unchanged.

## 6. Delivery semantics per sink (not one-size-fits-all)

Banner and Telegram are *best-effort* — a missed ding is a missed ding; drop-on-overflow is fine.
The **channel** sink exists so the AI *acts* on mail ("stop the deploy") — best-effort there is a
silent-failure factory. So delivery guarantees differ by role:

- **`viewer` / `bridge`:** best-effort. Per-client write buffer with a concrete cap
  (default 256 KiB); on overflow, **drop and log** (`[daemon] dropped N msgs to <role> client,
  slow consumer`). Never blocks the fan-out loop.
- **`channel`:** **at-least-once.** Each message carries its `relay_seq`. On overflow OR
  reconnect, the daemon sends `{type:"gap", after_seq}` and the client **replays from the
  archive** (`history({ since_seq })` — the `relay_seq` column already exists) deduped by
  `envelope_id`. To make the archive a complete replay source, **daemon mode makes the archive
  write a precondition for advancing the cursor** (a deliberate change from `receive()`'s current
  "advance even if the archive write failed" behavior, which is correct for an in-process sink
  but loses messages for a late-attaching cross-process client).

Fan-out is non-blocking per client: a slow channel client triggers gap+replay, it never stalls
the daemon's `receive()` loop.

## 7. Daemon ↔ legacy resolution (no split-brain)

The lock makes "just become a client" impossible without an explicit, ordered decision. Every
entrypoint resolves state in ONE ordered step — try the socket first, fall back under the lock:

| State (probe socket, then lock) | `air-msg watch` | `air-msg channel` | `air-msg bridge` |
|---|---|---|---|
| **Socket live** (connect ok) | attach as `viewer`, print `attached to air-msgd (PID N)` | attach as `channel` client | run as a daemon sink (configured); standalone refuses with a pointer to the daemon |
| **Stale socket, daemon dead** (ECONNREFUSED) | take lock, `unlink` stale socket, go standalone (legacy) | same | same |
| **No socket / no daemon** | legacy standalone (takes lock) | legacy standalone | legacy standalone |

Ordering at bind: connect-probe → on ECONNREFUSED, acquire lock → `unlink` stale socket → `listen`
→ re-stat. Two daemons racing at login: bind loser gets `EADDRINUSE`, attempts a client-connect
(success → exit 0 "another daemon already running"; failure → unlink + retry once → give up).
Reconnect contract: clients auto-reconnect with backoff; a daemon restart (e.g. upgrade) drops
all clients, who reconnect and (for `channel`) receive a `gap` + replay.

## 8. Lifecycle / install (POSIX)

- `air-msg daemon start` — foreground or `--detach`; writes a PID file `{home}/daemon.pid` with
  `{pid, start_time}` (start-time guards PID reuse; the lock JSON gains the same field so a
  recycled PID is not mistaken for a live holder).
- `air-msg daemon stop` — signal via PID file; releases lock + unlinks socket on clean exit.
- `air-msg daemon status` — **defined output**: holder PID + start-time, socket path, connected
  clients (count + roles), current cursor, last-received `relay_seq`, enabled in-process sinks.
  (Without this, split-brain is undebuggable.)
- `air-msg daemon install` — detect OS → write + load a launchd LaunchAgent (macOS) or a
  systemd-user service (Linux) that runs `air-msg daemon start --detach` at login + keeps it
  alive. `uninstall` removes it. Opt-out = never install (or `uninstall`); the manual snippet in
  `air-msg help` remains for users who want to hand-roll it.

## 9. Testing

- **Unit (no I/O):** the fan-out hub (one message → every registered sink called, role-gate
  admits/denies correctly — an explicit, tested invariant that "the daemon, not the client,
  enforces the gate"); the line-protocol framing/parse; the per-OS service-file *generators*
  (assert the plist / unit content); the `status` serializer.
- **Socket lifecycle:** an in-memory `net` socket pair — connect → `hello` → role handshake →
  a streamed message respecting the role gate → a `gap` + archive-replay path.
- **Integration:** spawn the daemon against a **stubbed relay**, attach a fake `channel`
  subscriber, post a message → it arrives gated; kill the daemon mid-stream → subscriber
  reconnects and replays the gap from the archive. The decision-table transitions (§7) each get a
  case.
- `watch()` itself is already covered; the installers' *load* step is manual on a real box.

## 10. Out of scope / deferred

- Windows (daemon may run; **no socket, no installer** in v1 — pipe ACL unverifiable).
- The `has_more` drain is a **prerequisite bug fix**, not part of this feature.
- Per-subscriber historical replay beyond the channel at-least-once path.
- Any network-reachable daemon surface.

## 11. Open questions

- ~~Does the relay's `/pull` SSE `since` guarantee no gap if two consumers briefly overlap during a
  daemon↔legacy handoff, or does the relay dedup by cursor?~~
  **RESOLVED 2026-06-10:** `/pull` (poll AND SSE) filters `acked_at IS NULL` and is cursor-driven
  (`since=N`) — see `~/air-site/relay/src/index.js` L193–237. Daemon and legacy share one
  `AGENT_BRIDGE_HOME` → one cursor + the archive's `(envelope_id, direction)` PK dedup, so a brief
  handoff overlap is at-least-once, never lossy.
- Should `bridge` be *only* a daemon sink in v1 (simpler), or retain a standalone mode? (Current
  spec keeps standalone as the no-daemon fallback.)
- Phase 2 note: the wire protocol stamps `relay_seq` from the pushed object's `seq` at the socket
  boundary (`core.receive()`'s onMessage objects carry `seq`; `relay_seq` exists only on archive
  rows) — Phase 3's gap/replay keys on `message.relay_seq` with no frame change.
- Open (Phase 4): does the MCP host auto-relaunch a channel server that exits 0 when the daemon
  goes away? Phase 2 ships clean-exit-on-disconnect as a stopgap; reconnect/backoff supersedes it.
