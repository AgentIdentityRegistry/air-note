# AIR Agent Memory Hub — M1a: `bossclawd` daemon extraction + app migration

**Date:** 2026-07-02
**Status:** Rev 2 — revised after independent architect + critic review (design verdict: SOUND; plan verdict: reworked to address 2 critical + 4 major findings). Pending final review → implementation.
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

**Success criterion:** the app behaves exactly as today — proven by its existing engine command tests passing against the daemon-backed client **via an in-process transport double** (the tests' assertions are unchanged; only the transport is swapped), plus integration tests over a real socket. (See the corrected acceptance bar in Testing — the naive "unchanged tests over a real socket" was impossible.)

**Non-goals (deferred):**
- Claude Code MCP server + hooks + history import → **M1b**.
- Any engine-quality change (bi-temporal, reflection, verifiable export) → M2–M4.
- Windows named-pipe transport (Unix socket first; Windows deferred, tracked as an open question).
- Exposing the daemon to any non-app client (that starts in M1b).

## Architecture — three units with clean boundaries

### Unit 1 — `bossclawd` (new **sibling crate** `crates/bossclawd/`, not a `src/bin` of the Tauri crate)
Rationale for a sibling crate (review-driven): a `bin` inside `air_agent_desktop` drags the whole Tauri/GTK dependency tree into the daemon and gives it no Tauri `resource_dir` for the embedder model. A standalone crate depends only on `bossclaw-core` + the protocol crate + tokio.
- Wraps the existing `bossclaw-core` engine. **The only process that holds the DEK and opens the encrypted DB.**
- **Hosts the state that lives in the app today** (this is the "not a half-daemon" fix): the single `EngineHandle` (shared across all connections; the existing `ingest_lock`/`evolve_lock` `try_lock`→`Busy` guards stay daemon-side), the **embedder** (`ResourceModel2Vec` — the daemon must locate the bundled model via an install path / env var, since it has no Tauri `resource_dir`), the **reasoner config cell + `ConfigReasonerProvider`**, and the **evolve `scheduler`** + Ollama probe (they need the reasoner, so they move here — the app stops spawning the scheduler).
- Serves a **local Unix domain socket**, mode `0600` (user-only), under the app's data dir (e.g. `~/Library/Application Support/ai.air-agent.desktop/bossclawd.sock`).
- **Single-owner arbitration (the actual single-writer guarantee — NOT just an in-process Mutex):** port the repo's proven two-part mechanism — a PID lockfile (`agent-bridge-mcp/src/consumer-lock.mjs`: acquire, `isPidAlive` signal-0 probe, reclaim-stale / refuse-live) + a socket liveness probe (`daemon-ipc.mjs`: *"a PID file can outlive a crashed daemon; ECONNREFUSED cannot lie"*). On startup: `connect()` the socket → if it answers, exit (a live owner exists); else acquire the PID lock, unlink any stale socket, bind. The app's "start if absent" path **probes-then-starts, never spawns unconditionally.** This closes the launchd-vs-app double-start race that would corrupt the SQLCipher DB.
- **Runs as an always-on background service** (launchd/systemd), **authored fresh in Rust-land** — the in-repo `air-msg` installer is Node.js and is *pattern-only reference*, not literally reusable. Always-on + app-independent is deliberate: M1b's Claude Code needs the librarian even when the desktop app is closed.
- Holds no plaintext secrets on disk. **Keychain/code-signing (go/no-go, see safety section):** the daemon ships **inside the app bundle, co-signed with the same Developer ID + a shared `keychain-access-groups` entitlement** so it shares the DEK's per-signature ACL. A separately-signed binary may be *denied* the DEK the signed app wrote (the documented `dev-build-signed.sh` / PR #44 hazard). A first spike verifies the daemon reads `air-agent.engine.dek` with no prompt before anything else is built.

### Unit 2 — shared protocol crate `crates/bossclawd-proto/`
- Typed `Request`/`Response` enums for the engine surface currently exposed by Tauri commands: status, add/revoke grant, set-writable, list writable/grants/files, run-ingest, list-files, recall, evolve, plus confirm/preview + mandate ops the app already has. Plus a `Response::Err(String)` (scrubbed engine-error string) and the onboarding/DEK-absent signal (`NotOnboarded`) must cross the wire.
- **The payload types must be hand-written MIRROR structs, not re-exports** (review-critical correction): the `bossclaw-core` boundary types (`Grant`, `Mandate`, `FileRecord`, `Hit`, `EvolveReport`, `EvolveStatus`, `IngestReport`, `PendingProposal`, `WriteOp`, …) derive only `Debug/Clone/PartialEq/Eq` — **NOT `Serialize`/`Deserialize`**. So the protocol crate defines its own serde-derived mirror of each, plus `From`/`Into` conversions on both sides. The existing DTOs in `commands/engine.rs` (`GrantDto`, `FileRecordDto`, `IngestReportDto`, the recall-hit DTO) already encode these shapes and are the source of truth for the mirrors.
- **Version handshake:** a `Hello { proto_version }` / `HelloOk { pid, proto_version }` first frame (mirroring `air-rs` inbox `HelloOk`). Mismatch → the client surfaces "engine unavailable" rather than mis-deserializing. Guards the two-now-separate binaries against version skew after a partial update.
- One contract both sides compile against, so wire formats cannot drift.
- Framing: length-prefixed JSON, `MAX_FRAME` = 32 MiB (a ceiling — note: whole-file preview payloads (old+new text) are the largest; they sit below the cap but justify the size).
- `#![forbid(unsafe_code)]` in this crate + the daemon crate (parity with `bossclaw-core`).

### Unit 3 — the app's `Engine` becomes a thin socket client
- Same public methods the app calls today (`run_ingest(onboarded)`, `recall(...)`, `evolve(...)`, grant ops, …). **Internals change from direct engine calls to socket requests.**
- **Tauri command signatures do not change at all** → the frontend and DTOs are untouched. (The command *tests* need a test transport — see the corrected acceptance bar in Safety/Testing.)
- **Persistent, reconnecting connection — NOT connect-per-call** (review correction): one `UnixStream` held in the client with reconnect-on-error (reuse the `air-rs` inbox `connect_persistent` + backoff pattern). Concurrent recalls multiplex on it, so use per-request correlation IDs on the framed messages (or a small pool) — pinned down in the plan.
- Connect failure / daemon down / connection dropped **mid-request** → the app's existing "engine unavailable" `EngineState` (fail-closed; never a second opener, never a silent local fallback, never a hang).
- The app **probes** the socket on launch and starts the installed service only if the probe shows no live owner (see arbitration above); it does not own the daemon's lifecycle — the service is launchd/systemd-managed.

## Data flow

UI → Tauri command (unchanged signature) → `Engine` client → length-prefixed JSON over the Unix socket → `bossclawd` → the real `bossclaw-core` engine method → typed response back → DTO → UI. Identical results; one socket hop added.

## Safety (the reason M1a goes first)

- **One writer, always — enforced, not asserted.** The in-process `Mutex` protects nothing across processes; the guarantee comes from the PID-lock + socket-liveness arbitration (Unit 1): the daemon refuses to start if a live owner answers the socket, reclaims a stale lock/socket otherwise, and the app probes-before-spawn. This closes the launchd-vs-app double-start race that would corrupt the SQLCipher DB.
- **Fail-closed.** Daemon unreachable, or a connection dropped mid-request → the app's existing "engine unavailable" state. No silent fallback to opening the DB in-process (that would reintroduce two openers); no hang.
- **Keychain / code-signing (GO/NO-GO, corrected from the naive "same slot" claim).** macOS Keychain items are ACL'd per accessing-binary code-signature. A separately-signed `bossclawd` may be *denied* the DEK the signed `.app` wrote (the documented `dev-build-signed.sh` / PR #44 hazard). Mitigation: ship `bossclawd` **inside the app bundle, co-signed with the same Developer ID + a shared `keychain-access-groups` entitlement** so both share the DEK ACL. **A first spike verifies the daemon reads `air-agent.engine.dek` with no prompt — before any other work.** If that fails, the migration can't preserve behavior and we revisit.
- **Socket confidentiality.** `0600` user-only socket under the app data dir; no network listener (Unix socket, not TCP). Consistent with the local-first, no-surprise-egress posture.

## Testing strategy

- **Acceptance bar (corrected — the old "existing tests pass unchanged" was self-contradictory).** The existing command tests build a *real in-process* `EngineHandle` with mock providers over Tauri IPC; they cannot pass against a socket client untouched. So: the `EngineClient` is written over a **transport trait** with two impls — (a) an **in-process transport double** (`tokio::io::duplex` to an in-memory daemon handler) that lets the existing command tests run **behavior-identically with no real socket**, and (b) the real Unix-socket transport used in production + integration tests. "Behavior-preserving" is proven by the command tests passing against transport (a); the socket itself is proven by integration tests against a real `bossclawd` on a temp socket.
- **New tests:**
  - Protocol round-trip: each `Request` → mirror-type conversion → daemon handler → `Response` → back to core type (no network; duplex transport).
  - **Version handshake:** mismatched `proto_version` → client surfaces "unavailable", no mis-deserialize.
  - **Single-owner:** a second `bossclawd` start with a live owner refuses (arbitration); a stale lock/socket is reclaimed; concurrent mutating ops still return `Busy`.
  - **Fail-closed:** daemon down at connect AND connection dropped mid-request → "engine unavailable", never opens the DB, never hangs.
  - **Onboarding/DEK-absent** crosses the wire: `NotOnboarded` round-trips.
  - Installer smoke (launchd/systemd): daemon starts, serves, survives restart, uninstalls cleanly (fresh Rust installer; `air-msg` smoke as pattern reference).
- Gates: `cargo test -p air_agent_desktop`, `cargo test -p bossclawd-proto`, `cargo test -p bossclawd`, `cargo clippy --all-targets -D warnings`, `cargo build -p bossclawd`, `forbid(unsafe)` in the new crates.

## Resolved by the Rev-2 review (were open questions)
- **App-starts-daemon vs launchd race** → RESOLVED: probe-before-spawn + PID-lock + socket-liveness arbitration (Unit 1). Not left to plan time — it's a named prerequisite task.
- **Keychain ACL for a separate binary** → RESOLVED as a design constraint: co-signed in-bundle + shared `keychain-access-groups`; gated by a first go/no-go spike.
- **Serde on boundary types** → RESOLVED: mirror types + conversions in `bossclawd-proto` (core types are not `Serialize`).
- **Scheduler/embedder/reasoner home** → RESOLVED: they move into `bossclawd` (Unit 1); the app stops spawning the scheduler.
- **Installer reuse depth** → RESOLVED: authored fresh in Rust; the Node `air-msg` installer is pattern-only.
- **Daemon crate shape** → RESOLVED: sibling `crates/bossclawd/`, not a `src/bin` of the Tauri crate.

## Still open / deferred
1. **Windows transport** — named pipe (ACL differs). Deferred; Unix-first (macOS is the daily driver).
2. **Concurrency model on the persistent connection** — per-request correlation IDs vs a small connection pool for concurrent recalls. Pin the exact choice in the plan/implementation; both are viable.
3. **Migration granularity** — incremental per op-family (recommended: read ops → ingest → recall → evolve → grant mutations, suite green each step).

## Cross-links
[[air/forever-companion-architecture]] · [[air/product-roadmap-2026-06]] · [[air/competitive-intel-agent-memory-2026-07]] · [[air/session-handoff-2026-07-02-live-roundtrip-and-tool-calls]] · prior desktop-engine specs: `docs/superpowers/specs/2026-06-22-desktop-engine-spine-design.md`
