# AIR Agent Memory Hub — M1a: `bossclawd` daemon extraction + app migration

**Date:** 2026-07-02
**Status:** Design approved (brainstorm); pending spec review → implementation plan
**Crate under change:** `air_agent_desktop` (`apps/desktop/src-tauri`), `bossclaw-core` (unchanged API), new `bossclawd` binary + shared protocol crate

## Program context (why this spec exists)

The goal is to make AIR Agent the universal, GBrain-replacing memory layer for the user's AI tools — auto-save on write, auto-recall on read, wired into Claude Code (then Cowork/Chat), backed by the local-first, signed `bossclaw-core` engine. A 2026-07-02 competitive benchmark ([[air/competitive-intel-agent-memory-2026-07]]) established that local-first + MCP-cross-tool memory is **table stakes** (Mem0, Supermemory, Zep/Graphiti, Cognee, Letta all have it); the differentiators that make AIR world-class are (a) reaching parity on a true bi-temporal graph + autonomous reflection, and (b) the empty niche of **cryptographically signed, verifiable, provably-yours memory**.

The program is four milestones + a benchmark harness, build order **M1 → M2 → M3 → M4**:

| Milestone | What | Catches / surpasses |
|---|---|---|
| **M1 — Exposure** | daemon + Claude Code auto-loop + history import | makes AIR *usable*; start replacing GBrain |
| M2 — Bi-temporal graph | true 4-timestamp edges + LLM contradiction detection | catches Graphiti |
| M3 — Autonomous reflection | sleep-time background reflector driving `evolve` | catches Letta |
| M4 — Verifiable memory | export a signed, independently-verifiable memory bundle | surpasses everyone (empty niche) |
| (ongoing) Benchmark harness | neutral LongMemEval + MemConflict | the measuring stick |

M1 splits into **M1a** (this spec — the daemon + app migration, behavior-preserving) and **M1b** (the Claude Code loop + history import). Architecture decision, already made: **Approach B** — a standalone daemon owns the store and the desktop app becomes one client among many (the eventual shape in [[air/forever-companion-architecture]]), because the whole point is many surfaces sharing one memory. M1a is the foundation; doing it first, in isolation and behavior-preserving, de-risks everything downstream.

## Current state (what M1a changes)

Today the desktop app owns the engine **in-process**: `AppState.engine` holds a `bossclaw-core` `EventLog` (encrypted SQLCipher-style DB) behind a `tokio::sync::Mutex`, plus an `EngineKeystore` that reads the data-encryption key (DEK) from the OS keychain. Every engine Tauri command (`engine_run_ingest`, `engine_recall`, `engine_evolve`, `engine_list_grants`, `engine_add_grant`, …) calls `state.engine.*` directly. The engine already serializes mutating ops (`EngineOpError::Busy("ingest"|"evolve")`) — i.e. single-op-at-a-time is already enforced in-process.

The problem this creates for the program: an encrypted store must have **exactly one writer/key-holder**. Claude Code (M1b) spawns MCP servers and runs hooks as separate short-lived processes; they cannot open the DB themselves. So the owner must move OUT of the app into a shared daemon that all surfaces (the app, then Code) route through.

## Goal / non-goals

**Goal:** Extract a `bossclawd` daemon that becomes the single owner of the encrypted store (the only process holding the DEK and opening the DB), and migrate the desktop app to reach the engine *through* it. **No new user-facing behavior.**

**Success criterion:** the app behaves exactly as today — proven by its existing engine command tests passing **unchanged** against the daemon-backed client.

**Non-goals (deferred):**
- Claude Code MCP server + hooks + history import → **M1b**.
- Any engine-quality change (bi-temporal, reflection, verifiable export) → M2–M4.
- Windows named-pipe transport (Unix socket first; Windows deferred, tracked as an open question).
- Exposing the daemon to any non-app client (that starts in M1b).

## Architecture — three units with clean boundaries

### Unit 1 — `bossclawd` (new binary)
- Wraps the existing `bossclaw-core` engine. **The only process that holds the DEK and opens the encrypted DB.**
- Serves a **local Unix domain socket**, mode `0600` (user-only), path under the app's data dir (e.g. `~/Library/Application Support/ai.air-agent.desktop/bossclawd.sock`).
- Preserves the engine's existing single-op serialization (one writer). Rejects a second concurrent mutating op with the existing `Busy` semantics.
- Runs as an **always-on background service** managed by launchd (macOS) / systemd (Linux), reusing the installer pattern the `air-msg` receiver daemon already shipped (its Phase-4 launchd/systemd installers are the template). Always-on + app-independent is deliberate: M1b's Claude Code needs the librarian even when the desktop app is closed.
- Holds no plaintext secrets on disk; reads the DEK from the same keychain slot the app uses today.

### Unit 2 — shared protocol crate (`bossclawd-proto` or a module)
- Typed request/response enums for the engine surface currently exposed by Tauri commands: status, add/revoke grant, set-writable, list writable/grants/files, run-ingest, list-files, recall, evolve, plus confirm/preview + mandate ops the app already has.
- One contract both the daemon and the client compile against, so wire formats cannot drift (lesson from prior sessions: stale wire-format comments + TS/Rust key mismatches are dangerous — a shared typed contract removes the class).
- Framing: length-prefixed JSON over the socket (simplest; mirrors the app's existing DTO-over-IPC style). Bodies are engine data, so the socket being user-only `0600` is the confidentiality boundary.

### Unit 3 — the app's `Engine` becomes a thin socket client
- Same public methods the app calls today (`run_ingest(onboarded)`, `recall(...)`, `evolve(...)`, grant ops, …). **Internals change from direct engine calls to socket requests.**
- **Tauri command signatures do not change at all** → the frontend, the DTOs, and the app's existing command tests are untouched.
- On connect failure / daemon down → map to the app's existing "engine unavailable" `EngineState` (fail-closed; never a second opener, never a silent local fallback).
- The app ensures the daemon is running (starts/adopts it on launch if not already up), but does not *own* its lifecycle beyond that — the service is launchd/systemd-managed.

## Data flow

UI → Tauri command (unchanged signature) → `Engine` client → length-prefixed JSON over the Unix socket → `bossclawd` → the real `bossclaw-core` engine method → typed response back → DTO → UI. Identical results; one socket hop added.

## Safety (the reason M1a goes first)

- **One writer, always.** Only `bossclawd` opens the DB and holds the DEK. The app stops opening the file entirely. This closes the multi-process corruption risk *before* any second client (Code) exists.
- **Fail-closed.** Daemon unreachable → the app's existing "engine unavailable" state. No silent fallback to opening the DB in-process (that would reintroduce two openers).
- **Behavior-preserving migration.** The DEK stays in the same keychain slot; `bossclawd` reads it exactly as the app does today. This is "move the DB-opening code from the app into the daemon," not "change how memory works."
- **Socket confidentiality.** `0600` user-only socket under the app data dir; no network listener (a Unix socket, not TCP). Consistent with the local-first, no-surprise-egress posture of the rest of the app.

## Testing strategy

- **Acceptance bar:** the app's existing engine command tests must pass **unchanged** against the daemon-backed `Engine` client (behavior-preserving proof).
- **New tests:**
  - Protocol round-trip: each request type serializes → daemon handles → typed response (no network; in-process socket or a fake transport).
  - **Single-writer:** a second attempt to open the store is refused; concurrent mutating ops return `Busy` as today.
  - **Fail-closed:** daemon down → client surfaces the "engine unavailable" state, never opens the DB itself.
  - Installer smoke (launchd/systemd): daemon starts, serves, survives a restart, uninstalls cleanly — reuse the `air-msg` daemon's smoke approach.
- Gates as usual: `cargo test -p air_agent_desktop`, `cargo clippy -D warnings`, `cargo build` for the new binary, `forbid(unsafe)` preserved.

## Open questions / deferred

1. **Windows transport** — named pipe (ACL model differs from Unix socket). Deferred; Unix-first (macOS is the daily-driver target). Track for a later milestone.
2. **App-starts-daemon vs pure launchd** — for M1a the app can spawn/adopt the daemon if it isn't running; the launchd/systemd service is the durable path. Decide the exact hand-off (does the app ever spawn, or only connect?) at plan time; the safe default is "connect; if absent, start the installed service."
3. **Reuse depth of the `air-msg` daemon infra** — the socket-server + thin-client + installer *pattern* is proven in the repo (Node side for messaging; Rust `air-rs` inbox client). How much Rust infra is literally reusable vs pattern-only is a plan-time investigation.
4. **Migration granularity** — migrate all engine commands at once, or incrementally per-command behind the seam. Recommend incremental (one command family at a time, tests green at each step).

## Cross-links
[[air/forever-companion-architecture]] · [[air/product-roadmap-2026-06]] · [[air/competitive-intel-agent-memory-2026-07]] · [[air/session-handoff-2026-07-02-live-roundtrip-and-tool-calls]] · prior desktop-engine specs: `docs/superpowers/specs/2026-06-22-desktop-engine-spine-design.md`
