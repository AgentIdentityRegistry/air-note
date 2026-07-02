# AIR Agent Memory Hub — M1a (`bossclawd` daemon + app migration) Implementation Plan — Rev 2

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract a single-owner `bossclawd` daemon over `bossclaw-core` and migrate the Tauri desktop app to reach the engine through it over a local Unix socket — with zero user-visible behavior change.

**Architecture:** One process (`bossclawd`, a sibling crate) holds the DEK and opens the encrypted store, AND hosts the embedder + reasoner + evolve-scheduler that live in the app today. A shared protocol crate defines serde **mirror types** (core types aren't `Serialize`) + a version handshake. The app's `Engine` keeps its public method signatures but swaps internals to a persistent, reconnecting socket client behind a transport trait. Tauri command signatures are unchanged.

**Tech Stack:** Rust, Tokio, Unix domain sockets, serde/serde_json (length-prefixed JSON), `bossclaw-core` (unchanged), launchd/systemd. Crates: new `crates/bossclawd/` + `crates/bossclawd-proto/`; modified `air_agent_desktop`.

**Spec:** `docs/superpowers/specs/2026-07-02-air-agent-memory-hub-m1a-daemon.md` (Rev 2)

**Rev 2 note:** revised after independent architect (verdict SOUND) + critic (verdict REWORK) review. Fixes baked in: mirror types (core types lack `Serialize`), corrected acceptance bar (transport double), single-owner arbitration, keychain go/no-go spike, scheduler/embedder/reasoner move into the daemon, version handshake, persistent client, fresh Rust installer, sibling crate.

---

## File structure

- **Create** `crates/bossclawd-proto/` — `Request`/`Response` enums, serde **mirror types** for each `bossclaw-core` boundary type + `From`/`Into` conversions, `Hello`/`HelloOk` version frames, `read_frame`/`write_frame`. `#![forbid(unsafe_code)]`.
- **Create** `crates/bossclawd/` — sibling crate (NOT a `src/bin` of the Tauri crate): owns the `EngineHandle`, embedder, reasoner cell + `ConfigReasonerProvider`, evolve scheduler + Ollama probe; PID-lock + socket-liveness arbitration; `UnixListener` accept loop sharing one `EngineHandle`. `#![forbid(unsafe_code)]`.
- **Create** `apps/desktop/src-tauri/src/engine/transport.rs` — a `Transport` trait (`request(&self, Request) -> Result<Response>`) with two impls: `SocketTransport` (persistent reconnecting `UnixStream`) and `#[cfg(test)] DuplexTransport` (in-process `tokio::io::duplex` to an in-memory daemon handler).
- **Create** `apps/desktop/src-tauri/src/engine/client.rs` — `EngineClient<T: Transport>`: one method per engine op, same signatures as today's `Engine`.
- **Modify** `apps/desktop/src-tauri/src/engine/mod.rs` — `Engine` delegates to `EngineClient`; keep `EngineError`/`EngineOpError`/`EngineState`. **Remove** the in-process `EventLog`/embedder/reasoner/scheduler ownership (moved to the daemon).
- **Modify** `apps/desktop/src-tauri/src/main.rs` — stop spawning the scheduler; probe-then-start the daemon; build `Engine` over `SocketTransport`.
- **Create** installer: `apps/desktop/src-tauri/resources/bossclawd.plist`, systemd unit, `scripts/install-bossclawd.sh` — authored fresh (Rust), `air-msg` Node installer as pattern-only reference.
- **Unchanged:** `apps/desktop/src-tauri/src/commands/*.rs` signatures, all frontend, all DTOs.

---

## Task 0: Engine-surface inventory (read-only)

**Files:** Read `engine/mod.rs`, `commands/engine.rs`, `crates/bossclaw-core/src/lib.rs`; Create `docs/superpowers/plans/m1a-engine-surface.md`.

- [x] **Step 1:** grep `state.engine.` across `src/` — list every method (signature, mutates?, wrapping Tauri command).
- [x] **Step 2:** For each boundary type crossing the wire (`Grant`, `Mandate`, `FileRecord`, `Hit`, `EvolveReport`, `EvolveStatus`, `IngestReport`, `PendingProposal`, `WriteOp`, …) record its fields. **Confirm each derives only `Debug/Clone/PartialEq/Eq` (NOT `Serialize`)** → each needs a mirror. The existing `commands/engine.rs` DTOs (`GrantDto`, `FileRecordDto`, `IngestReportDto`, recall-hit DTO) are the field source-of-truth. *(Confirmed; two wrinkles: `IngestReport` has no `Clone`, `Hit` has no `PartialEq` — see inventory.)*
- [x] **Step 3:** Record where these live in the app today and must MOVE to the daemon: the DEK/keystore open (`EngineKeystore`, `get_or_open`), the embedder (`ResourceModel2Vec` + its `resource_dir` model path, `main.rs:76-81`), the reasoner cell (`reasoner_cfg`/`reseed_reasoner_cell`, `main.rs:71,101`), the scheduler (`scheduler::spawn`, `main.rs:111`) + Ollama probe.
- [x] **Step 4:** Commit the inventory doc. → `m1a-engine-surface.md`

---

## Task 0.5: 🚦 Keychain-ACL GO/NO-GO spike (BLOCKING — do before any build work)

**Why:** macOS Keychain items are ACL'd per accessing-binary signature. A separately-signed `bossclawd` may be denied the DEK the signed `.app` wrote (the `dev-build-signed.sh` / PR #44 hazard). If it can't read the DEK, M1a can't preserve behavior — stop and revisit.

**Files:** a throwaway `crates/bossclawd/examples/dek_probe.rs` (or a `#[ignore]` test).

- [x] **Step 1:** Write a minimal program linking `bossclaw-core`/the vault that reads `air-agent.engine.dek` from service `ai.air-agent.desktop` (mirror `keystore.rs` read). *(Standalone get-only probe on the identical `keyring 2.3` call path — the ACL judges the calling binary's signature, not the wrapping crate.)*
- [x] **Step 2:** Build it, co-signed with the app's identity. *(No `keychain-access-groups` — that entitlement doesn't govern login-keychain ACLs; what matters is the item's trusted-app **designated requirement**: identifier + certificate leaf.)*
- [x] **Step 3:** Confirm it reads the DEK **with no interactive prompt**. Record the result in `m1a-engine-surface.md`.
- [x] **GATE: ✅ PASS** — silent 32-byte DEK read when signed `--identifier ai.air-agent.desktop` with the app's cert (same cert with a different identifier PROMPTS — Task 8 must override the identifier when signing the bundled `bossclawd`). Full record: `m1a-engine-surface.md`.

---

## Task 1: Protocol crate — framing (TDD)

**Files:** Create `crates/bossclawd-proto/` (Cargo.toml, `src/lib.rs`); add to workspace members.

- [ ] **Step 1: RED** — `frame_roundtrip` test over `tokio::io::duplex`.

```rust
#[tokio::test]
async fn frame_roundtrip() {
    use tokio::io::duplex;
    let (mut a, mut b) = duplex(1024);
    write_frame(&mut a, b"hello frame").await.unwrap();
    assert_eq!(read_frame(&mut b).await.unwrap(), b"hello frame");
}
```

- [ ] **Step 2:** `cargo test -p bossclawd-proto frame_roundtrip` → FAIL (not found).
- [ ] **Step 3: GREEN** — implement `write_frame`/`read_frame` (u32-BE length prefix, `MAX_FRAME = 32 * 1024 * 1024` guard rejecting oversize) over `AsyncRead`/`AsyncWrite` (same code as Rev 1). Add `#![forbid(unsafe_code)]`.
- [ ] **Step 4:** test PASS.
- [ ] **Step 5:** commit.

---

## Task 2: Protocol — mirror types + conversions + Request/Response + handshake (TDD)

**Files:** `crates/bossclawd-proto/src/lib.rs` (+ `types.rs`).

- [ ] **Step 1: RED** — for EACH boundary type, a `mirror_conversion_roundtrip` test: build a `bossclaw-core` value → `Into` mirror → serde round-trip → `Into` core → assert `==` the original (uses the core types' `PartialEq`).

```rust
#[test]
fn grant_mirror_roundtrip() {
    let g: bossclaw_core::graph::Grant = /* construct a sample */;
    let mirror: GrantMirror = g.clone().into();
    let bytes = serde_json::to_vec(&mirror).unwrap();
    let back: GrantMirror = serde_json::from_slice(&bytes).unwrap();
    let core_again: bossclaw_core::graph::Grant = back.into();
    assert_eq!(g, core_again);
}
// ...one per boundary type from the Task 0 inventory.
```

- [ ] **Step 2:** run → FAIL (mirror types not defined).
- [ ] **Step 3: GREEN** — define each `#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)] struct XMirror { ... }` with fields matching the DTOs, plus `impl From<CoreType> for XMirror` and `impl From<XMirror> for CoreType`. Then define `Request` / `Response` enums (one variant per Task 0 op; payloads use mirror types; add `Response::Err(String)` and a `NotOnboarded` signal). Add `Hello { proto_version: u32 }` / `HelloOk { pid: u32, proto_version: u32 }` frames + a `PROTO_VERSION` const.
- [ ] **Step 4:** all conversion tests + a `request_response_serde_roundtrip` test PASS.
- [ ] **Step 5:** commit. **Reviewer check:** the enum covers the Task 0 inventory 1:1.

---

## Task 3: Single-owner arbitration (TDD) — port `consumer-lock.mjs`

**Files:** `crates/bossclawd/src/lock.rs`; Test: same file.

- [ ] **Step 1: RED** — tests: (a) acquiring the lock in an empty dir succeeds + writes a 0600 PID file; (b) a second acquire while the first is held (live PID) is REFUSED; (c) a lock file with a dead PID is RECLAIMED; (d) a stale socket file (no listener) is detectable as reclaimable.
- [ ] **Step 2:** run → FAIL.
- [ ] **Step 3: GREEN** — implement `acquire_or_refuse(lock_path, sock_path)`: if `connect(sock_path)` answers → `Err(LiveOwner)`; else read the PID file, if `is_pid_alive(pid)` (signal-0 via `libc::kill(pid,0)` or `nix`) → `Err(LiveOwner)`; else unlink stale socket + lock, write our PID (0600), return the guard. Mirror `consumer-lock.mjs` semantics: *reclaim stale, refuse live*.
- [ ] **Step 4:** tests PASS.
- [ ] **Step 5:** commit.

---

## Task 4: `bossclawd` daemon — engine + embedder + reasoner + scheduler + accept loop (TDD)

**Files:** `crates/bossclawd/src/main.rs`, `src/server.rs`; Test: `crates/bossclawd/tests/roundtrip.rs`.

- [ ] **Step 1: RED** — `status_roundtrip_over_socket`: a `cfg(test)` helper spins the server on a temp socket with a temp engine home + test DEK + `MockEmbedder`/`MockReasoner`; client sends `Hello` then `Request::Status`; asserts `HelloOk` then `Response::Status(_)`.
- [ ] **Step 2:** run → FAIL.
- [ ] **Step 3: GREEN** — implement the daemon:
  - Startup: `lock::acquire_or_refuse` (Task 3) → if refused, exit 0 (a live owner exists).
  - Build ONE `EngineHandle` (moved `get_or_open` code), the embedder (model dir from `BOSSCLAWD_MODEL_DIR` env / install path — NOT Tauri `resource_dir`), the reasoner cell + `ConfigReasonerProvider`, and spawn the evolve `scheduler` + Ollama probe here.
  - `UnixListener::bind` (0600); accept loop → per-connection task holding an `Arc` of the shared `EngineHandle`; first frame must be `Hello` (version check → `HelloOk` or close); then `read_frame` → `Request` → dispatch to the engine method (mutating ops keep `try_lock`→`Busy`) → `Response` (mirror conversion) or `Response::Err(scrubbed)` → `write_frame`.
  - A `cfg(test)` `spawn_for_test(sock, home)` helper.
- [ ] **Step 4:** test PASS.
- [ ] **Step 5:** commit.
- [ ] **Steps 6–N (per op family):** add a dispatch arm + round-trip test for each remaining op (list ops → run-ingest → recall → evolve → grant mutations), one RED→GREEN→commit each. Include a `NotOnboarded` round-trip.

---

## Task 5: Transport trait + persistent client (TDD)

**Files:** `apps/desktop/src-tauri/src/engine/transport.rs`, `client.rs`.

- [ ] **Step 1: RED** — with a test daemon on a temp socket, `EngineClient::new(SocketTransport)` returns the same shapes the old in-process `Engine` did for `status()` + `recall()`; and after a simulated daemon drop the next call reconnects (or returns "unavailable", never hangs).
- [ ] **Step 2:** run → FAIL.
- [ ] **Step 3: GREEN** — define `trait Transport { async fn request(&self, Request) -> Result<Response, EngineOpError>; }`. `SocketTransport`: hold a persistent `UnixStream` (do the `Hello` handshake on connect); on I/O error, reconnect once then map to "unavailable" (reuse `air-rs` `connect_persistent` backoff). Concurrent requests: tag frames with a correlation id (or serialize through a `Mutex<Stream>` for M1a simplicity — pick one, document it). `EngineClient<T: Transport>`: one method per op, `Request::X` → `Response::X` / `Response::Err → EngineOpError::Core` / `NotOnboarded → EngineError`.
- [ ] **Step 4:** test PASS.
- [ ] **Step 5:** commit. **Steps 6–N:** repeat per op family.

---

## Task 6: Migrate `Engine`/`AppState` (in-process transport double proves behavior-preservation)

**Files:** `engine/mod.rs`, `main.rs`, `transport.rs` (`DuplexTransport`).

- [ ] **Step 1: GREEN-first is not allowed — RED via the existing tests:** add `#[cfg(test)] DuplexTransport` — an in-process `tokio::io::duplex` pair wired to an in-memory daemon handler (reuse Task 4's dispatch over the duplex, no real socket). Point the existing command tests' `AppState` at `EngineClient<DuplexTransport>`.
- [ ] **Step 2:** Run the FULL existing suite `cargo test -p air_agent_desktop` — the command-test assertions are UNCHANGED; only the transport is swapped. They must PASS (this is the behavior-preserving proof). Fix until green.
- [ ] **Step 3:** Change production `Engine` to hold `EngineClient<SocketTransport>`; keep every public method signature + the `EngineError`/`EngineOpError`/`EngineState` mapping. In `main.rs`: stop `scheduler::spawn`; probe-then-start the daemon; build the client into `AppState`.
- [ ] **Step 4:** `cargo test -p air_agent_desktop` green again.
- [ ] **Step 5:** commit `refactor(desktop): Engine delegates to bossclawd; scheduler+embedder+reasoner move to daemon`.

---

## Task 7: Invariant + failure-path tests

**Files:** `crates/bossclawd/tests/invariants.rs`, `apps/desktop/.../tests/`.

- [ ] **RED→GREEN each:** (a) **single-owner** — a 2nd daemon start with a live owner refuses; stale lock/socket reclaimed. (b) **version skew** — client `Hello` with wrong `proto_version` → closed → app "unavailable". (c) **fail-closed at connect** — no daemon → "unavailable", no DB open. (d) **fail-closed mid-request** — daemon killed mid-call → "unavailable", no hang. (e) **onboarding** — `NotOnboarded` round-trips.
- [ ] commit.

---

## Task 8: Installer (fresh Rust; `air-msg` Node installer = pattern-only)

**Files:** `resources/bossclawd.plist`, systemd unit, `scripts/install-bossclawd.sh`. Read the `air-msg` Node installer as reference only.

- [ ] **Step 1:** launchd plist (`RunAtLoad` + `KeepAlive`, socket + `BOSSCLAWD_MODEL_DIR` env, log paths). Bundle `bossclawd` in the app + Tauri `externalBin`/resources so it ships co-signed (ties to Task 0.5).
- [ ] **Step 2:** systemd unit (`Restart=always`).
- [ ] **Step 3:** documented manual smoke: install → starts → app connects → kill → relaunches → uninstall clean.
- [ ] **Step 4:** commit.

---

## Task 9: Final gates

- [ ] `cargo test -p air_agent_desktop` (full suite green via `DuplexTransport`)
- [ ] `cargo test -p bossclawd-proto` / `-p bossclawd`
- [ ] `cargo clippy --all-targets -- -D warnings` clean
- [ ] `cargo build -p bossclawd` succeeds; bundled + co-signed
- [ ] `#![forbid(unsafe_code)]` present in `bossclawd-proto` + `bossclawd`; the `libc::kill` in Task 3 is the one allowed `unsafe` — isolate it in a small `is_pid_alive` fn with `#[allow(unsafe_code)]` + a safety comment, or use the `nix` crate to avoid `unsafe`
- [ ] `commands/*.rs` signatures byte-unchanged (grep-diff the command fns)
- [ ] Open PR into `main`; body links this plan + spec; flag Peter-gated post-merge GUI QA (signed build: app still works end-to-end, daemon auto-starts).

---

## Self-review (author, Rev 2)

- **Spec coverage:** daemon owner + arbitration (Tasks 3,4) ✓ · keychain go/no-go (Task 0.5) ✓ · mirror types (Task 2) ✓ · scheduler/embedder/reasoner moved (Task 4) ✓ · persistent client + transport trait (Task 5) ✓ · corrected acceptance bar via DuplexTransport (Task 6) ✓ · version handshake (Tasks 2,4,7) ✓ · fail-closed/mid-request/single-owner tests (Task 7) ✓ · fresh Rust installer (Task 8) ✓ · sibling crate (file structure) ✓.
- **Critical review findings closed:** serde-mirror (was "re-export", now Task 2 mirrors+conversions) ✓ · acceptance bar (was self-contradictory, now DuplexTransport in Task 6) ✓ · scheduler/embedder/reasoner half-daemon (now Task 4) ✓ · single-writer race (now Task 3 arbitration) ✓ · keychain ACL (now blocking Task 0.5) ✓ · installer Node≠Rust (Task 8 fresh) ✓ · crate shape (sibling) ✓.
- **Type consistency:** `Request`/`Response` + mirrors (Task 2) are the single contract used by daemon (Task 4) and client (Task 5); `Transport` trait (Task 5) consumed by `Engine` (Task 6) with `DuplexTransport` for tests; `read_frame`/`write_frame`/`Hello`/`PROTO_VERSION` names consistent.
- **Unsafe:** flagged the one `libc::kill` site with a mitigation (isolate or use `nix`) so `forbid(unsafe_code)` holds.
