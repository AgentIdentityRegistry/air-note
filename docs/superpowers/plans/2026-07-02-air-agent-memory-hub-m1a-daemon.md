# AIR Agent Memory Hub — M1a (`bossclawd` daemon + app migration) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract a single-owner `bossclawd` daemon over `bossclaw-core` and migrate the desktop app to reach the engine through it over a local Unix socket — with zero user-visible behavior change.

**Architecture:** One process (`bossclawd`) holds the DEK and opens the encrypted store; a shared typed protocol crate defines the wire contract; the app's `Engine` keeps its public method signatures but swaps its internals from direct engine calls to socket requests. Tauri command signatures are unchanged, so the frontend and existing command tests are untouched.

**Tech Stack:** Rust, Tokio, Unix domain sockets (`tokio::net::UnixListener`/`UnixStream`), serde/serde_json (length-prefixed JSON framing), `bossclaw-core` (unchanged), launchd/systemd (installer, reusing the `air-msg` daemon pattern). Crate under change: `air_agent_desktop` (`apps/desktop/src-tauri`).

**Spec:** `docs/superpowers/specs/2026-07-02-air-agent-memory-hub-m1a-daemon.md`

---

## File structure (decomposition locked here)

- **Create** `crates/bossclawd-proto/` — new crate: typed `Request`/`Response` enums + framing helpers (`read_frame`/`write_frame`). One responsibility: the wire contract. Depends only on serde/serde_json + `bossclaw-core` DTO-compatible primitives (or its own mirror types to avoid a heavy dep).
- **Create** `apps/desktop/src-tauri/src/bin/bossclawd.rs` (or a sibling crate `crates/bossclawd/`) — the daemon binary: owns `bossclaw-core` `EventLog` + `EngineKeystore`, `UnixListener` accept loop, dispatch each `Request` to the engine, single-op serialization, return `Response`.
- **Create** `apps/desktop/src-tauri/src/engine/client.rs` — the app-side socket client: one method per engine op the app calls today, same signatures as the current in-process `Engine`.
- **Modify** `apps/desktop/src-tauri/src/engine/mod.rs` — `Engine` delegates to `client.rs` instead of opening the store in-process; keep `EngineError`/`EngineOpError`/`EngineState` mapping so status stays identical.
- **Modify** `apps/desktop/src-tauri/src/main.rs` (or `lib.rs` setup) — on startup, ensure `bossclawd` is running (connect; if absent, start the installed service), then build the `Engine` client into `AppState`.
- **Create** `apps/desktop/src-tauri/resources/bossclawd.plist` + a `scripts/install-bossclawd.sh` (macOS launchd) and the systemd unit — modeled on the existing `air-msg` daemon installer.
- **Unchanged:** `apps/desktop/src-tauri/src/commands/engine.rs` (command signatures), all frontend, all DTOs.

**Recommended migration granularity:** incremental — one engine op family at a time (read-only ops first: status/list; then ingest; then recall; then evolve; then grant mutations), with the full suite green at each step.

---

## Task 0: Inventory the engine surface (read-only; the map everything else references)

**Files:**
- Read: `apps/desktop/src-tauri/src/engine/mod.rs`, `apps/desktop/src-tauri/src/commands/engine.rs`, `crates/bossclaw-core/src/lib.rs`
- Create: `docs/superpowers/plans/m1a-engine-surface.md` (the inventory artifact)

- [ ] **Step 1:** List every method the app currently calls on `state.engine` (grep `state.engine.` across `src/commands/` and `src/engine/`). For each: exact signature (params + return type), whether it mutates, and which Tauri command wraps it.
- [ ] **Step 2:** For each method, note the `bossclaw-core` types crossing the boundary (e.g. `Grant`, `FileRecord`, `IngestReport`, `HitWithText`, `EvolveReport`, `EvolveStatus`) — these become the protocol's payload types (mirror or re-export).
- [ ] **Step 3:** Record the DEK/keystore open path (`EngineKeystore`, how `EventLog` is opened) — this code MOVES to the daemon verbatim.
- [ ] **Step 4:** Commit the inventory doc. `git add docs/superpowers/plans/m1a-engine-surface.md && git commit -m "docs(m1a): engine-surface inventory for daemon extraction"`

This inventory is the authoritative list the protocol enum (Task 2) and the client (Task 4) must cover 1:1. No engine op may be dropped or renamed.

---

## Task 1: Scaffold the protocol crate with framing (TDD)

**Files:**
- Create: `crates/bossclawd-proto/Cargo.toml`, `crates/bossclawd-proto/src/lib.rs`
- Modify: root `Cargo.toml` workspace members
- Test: in `crates/bossclawd-proto/src/lib.rs` (`#[cfg(test)]`)

- [ ] **Step 1: Write the failing test** for length-prefixed framing round-trip:

```rust
#[tokio::test]
async fn frame_roundtrip() {
    use tokio::io::duplex;
    let (mut a, mut b) = duplex(1024);
    let msg = b"hello frame";
    write_frame(&mut a, msg).await.unwrap();
    let got = read_frame(&mut b).await.unwrap();
    assert_eq!(got, msg);
}
```

- [ ] **Step 2: Run it, verify it fails** — `cargo test -p bossclawd-proto frame_roundtrip` → FAIL (`write_frame`/`read_frame` not found).
- [ ] **Step 3: Implement** `write_frame`/`read_frame` (u32 big-endian length prefix + body) over `AsyncRead`/`AsyncWrite`, plus a `MAX_FRAME` guard (reject oversize → error, never allocate unbounded):

```rust
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
pub const MAX_FRAME: usize = 32 * 1024 * 1024; // 32 MiB ceiling
pub async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, body: &[u8]) -> std::io::Result<()> {
    let len = u32::try_from(body.len()).map_err(|_| std::io::Error::other("frame too large"))?;
    w.write_all(&len.to_be_bytes()).await?;
    w.write_all(body).await?;
    w.flush().await
}
pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> std::io::Result<Vec<u8>> {
    let mut lenb = [0u8; 4];
    r.read_exact(&mut lenb).await?;
    let len = u32::from_be_bytes(lenb) as usize;
    if len > MAX_FRAME { return Err(std::io::Error::other("frame exceeds MAX_FRAME")); }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await?;
    Ok(body)
}
```

- [ ] **Step 4: Run it, verify it passes** — `cargo test -p bossclawd-proto frame_roundtrip` → PASS.
- [ ] **Step 5: Commit** — `git add crates/bossclawd-proto Cargo.toml && git commit -m "feat(bossclawd-proto): length-prefixed socket framing"`

---

## Task 2: Define the Request/Response protocol (TDD, driven by the Task 0 inventory)

**Files:**
- Modify: `crates/bossclawd-proto/src/lib.rs`
- Test: same file

- [ ] **Step 1: Write the failing test** asserting serde round-trip for a representative variant of each op family (status, a read op, ingest, recall, evolve, a grant mutation):

```rust
#[test]
fn request_response_serde_roundtrip() {
    for req in [Request::Status, Request::ListGrants, Request::RunIngest, Request::Recall { query: "q".into(), k: 5 }] {
        let s = serde_json::to_vec(&req).unwrap();
        let back: Request = serde_json::from_slice(&s).unwrap();
        assert_eq!(req, back);
    }
    let resp = Response::Recall(vec![]); // shape per inventory
    let s = serde_json::to_vec(&resp).unwrap();
    let _back: Response = serde_json::from_slice(&s).unwrap();
}
```

- [ ] **Step 2: Run, verify fail** — `cargo test -p bossclawd-proto request_response_serde_roundtrip` → FAIL (types not defined).
- [ ] **Step 3: Implement** `#[derive(Serialize, Deserialize, PartialEq, Debug)] enum Request { ... }` and `enum Response { ... }` with **one variant per engine op from the Task 0 inventory** (payload types mirrored from `bossclaw-core`). Include a `Response::Err(String)` variant for engine errors (carrying the already-scrubbed engine error string). Add `#[serde(tag = "op")]` for readable frames.
- [ ] **Step 4: Run, verify pass.**
- [ ] **Step 5: Commit** — `git commit -am "feat(bossclawd-proto): Request/Response covering the full engine op surface"`

**Note:** the reviewer/implementer must verify the enum covers the Task 0 inventory 1:1 (a missing op = a command that can't be migrated).

---

## Task 3: `bossclawd` binary — socket server owning the engine (TDD)

**Files:**
- Create: `apps/desktop/src-tauri/src/bin/bossclawd.rs`
- (Reuse) the DEK/keystore + `EventLog` open code identified in Task 0, moved here
- Test: an integration test `apps/desktop/src-tauri/tests/bossclawd_roundtrip.rs`

- [ ] **Step 1: Write the failing test** — spin the server on a temp socket with a temp `AGENT_*`/engine home + a test DEK, connect a client, send `Request::Status`, assert a `Response::Status(..)` comes back:

```rust
// uses a temp dir for the engine home + a deterministic test key (mirror existing engine tests' setup)
#[tokio::test]
async fn status_roundtrip_over_socket() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("d.sock");
    let handle = spawn_bossclawd_for_test(&sock, dir.path()).await; // helper in the bin, cfg(test)-exposed
    let mut c = tokio::net::UnixStream::connect(&sock).await.unwrap();
    bossclawd_proto::write_frame(&mut c, &serde_json::to_vec(&Request::Status).unwrap()).await.unwrap();
    let resp: Response = serde_json::from_slice(&bossclawd_proto::read_frame(&mut c).await.unwrap()).unwrap();
    assert!(matches!(resp, Response::Status(_)));
    handle.shutdown().await;
}
```

- [ ] **Step 2: Run, verify fail** — server + helper don't exist.
- [ ] **Step 3: Implement** the daemon: bind `UnixListener` at the socket path (create parent dir; set mode `0600`), open the engine once (the moved keystore/`EventLog` code) behind the existing `Mutex`, accept loop → per-connection task → `read_frame` → deserialize `Request` → dispatch to the engine method → serialize `Response` → `write_frame`. Preserve the `Busy` serialization for mutating ops. Map engine errors to `Response::Err(scrubbed_string)`. Factor a `cfg(test)` `spawn_bossclawd_for_test` helper.
- [ ] **Step 4: Run, verify pass.**
- [ ] **Step 5: Commit** — `git commit -am "feat(bossclawd): unix-socket server owning the engine (status op)"`

- [ ] **Step 6–N (repeat per op family):** add a dispatch arm + a round-trip test for each remaining op from the inventory (list ops, run-ingest, recall, evolve, grant mutations). One family per RED→GREEN→commit cycle. Reuse the same test harness.

---

## Task 4: App-side `Engine` client (TDD, one worked op then repeat)

**Files:**
- Create: `apps/desktop/src-tauri/src/engine/client.rs`
- Test: same file / `tests/`

- [ ] **Step 1: Write the failing test** — with a test `bossclawd` running on a temp socket, an `EngineClient` pointed at it returns the same shape the old in-process method did for `status()` and `recall()`.
- [ ] **Step 2: Run, verify fail.**
- [ ] **Step 3: Implement** `EngineClient { sock_path }` with one async method per op: connect, `write_frame(serde(Request::X))`, `read_frame` → `Response::X(..)` or `Response::Err → Err(EngineOpError::Core(..))`. Signatures MATCH the current `Engine` methods (Task 0 inventory) exactly.
- [ ] **Step 4: Run, verify pass.**
- [ ] **Step 5: Commit.**
- [ ] **Step 6–N:** repeat per op family, tests green each step.

---

## Task 5: Wire the client into `Engine`/`AppState` (migration; existing tests are the bar)

**Files:**
- Modify: `apps/desktop/src-tauri/src/engine/mod.rs`, `apps/desktop/src-tauri/src/main.rs`

- [ ] **Step 1:** Change `Engine` to hold an `EngineClient` instead of the in-process `EventLog`/keystore. Keep every public method + its signature; each now delegates to the client. Keep `EngineError`/`EngineOpError`/`EngineState` mapping (daemon-down → the existing "unavailable" state).
- [ ] **Step 2:** In `main.rs` setup: ensure `bossclawd` is reachable (connect; if not, start the installed service — see open question #2 in the spec, resolved here to "connect-or-start-installed-service"), then construct the `Engine` client into `AppState`.
- [ ] **Step 3: Run the FULL existing suite** — `cargo test -p air_agent_desktop` → all previously-passing engine command tests still PASS (this is the behavior-preserving acceptance bar). Fix until green.
- [ ] **Step 4: Commit** — `git commit -am "refactor(desktop): Engine delegates to bossclawd over the socket (behavior-preserving)"`

---

## Task 6: Single-writer + fail-closed tests

**Files:**
- Test: `apps/desktop/src-tauri/tests/bossclawd_invariants.rs`

- [ ] **Step 1: Write failing tests:** (a) a second attempt to open the store directly is refused / not attempted by the app (assert the app path never opens the DB — no second opener); (b) client with no daemon running → `EngineOpError`/"unavailable" state, never a panic, never a DB open.
- [ ] **Step 2: Run, verify fail.**
- [ ] **Step 3: Implement** any guard needed so both hold (e.g. the app has no code path that opens the store anymore; the client maps connect-refused to the unavailable state).
- [ ] **Step 4: Run, verify pass.**
- [ ] **Step 5: Commit.**

---

## Task 7: Lifecycle / installer (launchd + systemd), reusing the `air-msg` pattern

**Files:**
- Create: `apps/desktop/src-tauri/resources/bossclawd.plist`, `scripts/install-bossclawd.sh`, systemd unit
- Read first: the existing `air-msg` daemon installer (Phase-4 launchd/systemd) as the template

- [ ] **Step 1:** Author the launchd plist (KeepAlive, socket path env, log paths) + install script, modeled on `air-msg`'s. `RunAtLoad` + `KeepAlive` so it survives kills/restart.
- [ ] **Step 2:** Author the systemd unit (`Restart=always`) mirroring the `air-msg` systemd smoke.
- [ ] **Step 3: Smoke** (manual, documented): install → daemon starts → app connects → kill → relaunches → uninstall clean. Document in the plan's verification notes.
- [ ] **Step 4: Commit.**

---

## Task 8: Final gates

- [ ] `cargo test -p air_agent_desktop` (full suite green, incl. the pre-existing engine command tests unchanged)
- [ ] `cargo test -p bossclawd-proto`
- [ ] `cargo clippy --all-targets -- -D warnings` (workspace) clean
- [ ] `cargo build --bin bossclawd` succeeds
- [ ] `forbid(unsafe)` preserved in touched crates; zero new `unsafe`
- [ ] Confirm `apps/desktop/src-tauri/src/commands/engine.rs` command signatures are byte-unchanged (the frontend seam)
- [ ] Commit + open PR into `main`; PR body links this plan + the spec; note manual GUI QA (app still works end-to-end) as a Peter-gated post-merge step.

---

## Self-review notes (author)

- **Spec coverage:** daemon owner (Tasks 3,7) ✓ · shared protocol (Tasks 1,2) ✓ · app-as-thin-client, signatures unchanged (Tasks 4,5) ✓ · single-writer + fail-closed (Task 6) ✓ · behavior-preserving proof = existing tests pass (Task 5 Step 3, Task 8) ✓ · always-on installer (Task 7) ✓ · Unix-socket 0600 (Task 3) ✓. Deferred items (M1b Code loop/import, M2–M4, Windows) correctly absent.
- **Placeholder scan:** the only intentionally-deferred detail is "enumerate the full op list," which is made an explicit first task (Task 0) whose artifact later tasks reference 1:1 — not a hand-wave. Code blocks are concrete for framing/protocol/server/client patterns; per-op arms are a documented repeat of a worked pattern.
- **Type consistency:** `Request`/`Response` (Task 2) are the single source both server (Task 3) and client (Task 4) use; `bossclawd_proto::{read_frame,write_frame}` names consistent across Tasks 1/3/4; `EngineClient` (Task 4) consumed by `Engine` (Task 5).
- **Open question resolved here:** spec open-Q #2 → app "connects, and starts the installed service if absent" (Task 5 Step 2). Spec open-Q #3 (reuse depth of `air-msg` infra) → Task 7 reads it as a template; literal-reuse depth decided during Task 7. Spec open-Q #1 (Windows) → out of scope.
