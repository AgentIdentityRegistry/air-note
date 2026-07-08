# Memory Hub SP1 — The Safe Read+Write Code Loop Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a coding agent, through a small Rust MCP adapter, `recall` AIR memories and `remember` new ones — safely (a scoped `MemoryClient` role the daemon enforces fail-closed), backed by the existing `bossclawd` daemon — proven end-to-end with no UI.

**Architecture:** Three units land in order behind a single daemon socket. (U1) A `remember` write op: a new `EventLog::remember` core fn appends a signed `memory`-type event stamped `origin=external` (reusing the taint model) and derives its vector, exposed as `Request::Remember` → `EngineHandle::remember`. (U2) Per-op authorization: a connection `Role` established at the `Hello` handshake (default `App` = all ops, so the app is byte-for-byte unchanged), enforced fail-closed in the daemon's `dispatch` via a per-role op-allowlist; `MemoryClient` may invoke only `Recall`+`Remember`, and (a security bonus) cannot assert its own onboarding. (U3) A new workspace bin crate `air-memory-mcp`: a hand-rolled MCP-over-stdio JSON-RPC 2.0 server exposing exactly `recall` + `remember`, backed by the daemon socket as a `MemoryClient`, reusing `bossclawd-proto`'s frame codec + handshake verbatim (never reimplementing the wire protocol). (U4) End-to-end proof over the real socket + a documented `.mcp.json` wiring snippet.

**Tech Stack:** Rust (workspace), `tokio` (Unix sockets + async stdio), `serde`/`serde_json` (JSON-RPC + wire frames), `bossclawd-proto` (the length-prefixed frame codec + `Request`/`Response`/`Hello`), `bossclaw-core` (the signed `EventLog`). No new third-party dependency is introduced (the MCP loop is hand-rolled — see "Open questions resolved").

---

## Verified current anchors (re-read 2026-07-09, base `main` `54dfefa`, branch `feat-memory-hub-sp1-code-loop`)

Every line:file below was opened and confirmed on this tree. Trust these, not the spec's approximate numbers.

**`crates/bossclawd-proto/src/lib.rs`**
- `pub const PROTO_VERSION: u32 = 1;` — L43.
- `pub struct Hello { pub proto_version: u32 }` — L50–54. Constructed at L647, L664 (this file's tests).
- `pub struct HelloOk { pub pid: u32, pub proto_version: u32 }` — L61–67.
- `pub enum Request { … }` — L81–152; last two arms `SetActiveModel` (L148) + `ModelStatus` (L151); `Recall { onboarded, query, k }` (L98); `Teardown` (L128, unit); `EnableCloudReasoner { onboarded, config }` (L143); `AddGrant`/`AddMandate`/`SetActiveModel` are the destructive/egress ops. Externally tagged, `#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]`.
- `pub enum Response { … }` — L172–229; `Ok` (L177), `Recall(Vec<HitWire>)` (L188).
- `pub enum OpErrorKindWire { … }` — L242–267 (11 arms; `Core` … `KeystoreDbMismatch`). `#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]`.
- `pub struct HitWire { pub hit: HitMirror, pub text: String }` — L273–279.
- Tests: `request_response_serde_roundtrip` (L473), `op_error_kind_wire_roundtrips_every_arm` (L558–585, iterates a hard-coded array of every arm), `handshake_serde_roundtrip` (L645), `hello_is_not_a_request` (L662).

**`crates/bossclawd/src/server.rs`**
- `serve_connection` — L49–111. Handshake L58–92 (reads first frame as `Hello`, version-checks, replies `HelloOk`). Dispatch loop L94–110 calls `dispatch(&engine, req)` at L103.
- `async fn dispatch(engine: &Arc<EngineHandle>, req: Request) -> Response` — L134–268. Exhaustive `match req` (no `_` wildcard); last arms `SetActiveModel` (L261) + `ModelStatus` (L264).
- `fn protocol_err(message: String) -> Response` — L116–118 (`OpErrorKindWire::Core`).
- `pub fn op_error_response(e: EngineOpError) -> Response` — L302–316 (exhaustive on `EngineOpError`).
- `pub fn engine_error_response(e: EngineError) -> Response` — L321–333.
- `run_accept_loop` — L478–515; same-uid peer-cred gate L484–499; doc-comment L465–473 defers per-op authz to M1b (**this is the seam U2 fills**).
- `pub async fn spawn_for_test(sock_path, home)` — L530; `pub fn test_engine(home)` — L553; `test_engine_with_embedder` — L563.

**`crates/bossclaw-core/src/log.rs`**
- `const EMBEDDABLE_EVENT_TYPES` — L320–324, = `[MEMORY_EVENT_TYPE, PAGE_EVENT_TYPE, FILE_INGESTED_EVENT_TYPE]` (so a `memory` event is embedded + recallable).
- `pub fn append(&self, event: Event) -> Result<String, BossclawError>` — L788 (assigns id/ts/prev_hash/hash/signature; `reject_empty_tier_b` only bites Tier-B).
- `pub fn event_by_id(&self, id) -> Result<Option<Event>, BossclawError>` — L895.
- `pub fn rebuild_indexes(&self, embedder) -> Result<(), BossclawError>` — L1261 (rebuilds vector index from persisted vectors + repopulates FTS from all embeddable events).
- `pub fn recall(&self, embedder, query, k, opts) -> Result<Vec<Hit>, BossclawError>` — L1471.
- `pub(crate) fn signer_did(&self) -> String` — L4477 (returns `ENGINE_SIGNER_DID = "did:wba:bossclaw-engine"`, L203). Portable (no `#[cfg(unix)]`).
- `pub(crate) fn derive_vector_for(&self, embedder, event_id) -> Result<(), BossclawError>` — L4483 (derives + persists a vector for a just-appended event; no-op for a text-less event). Portable.
- Test helper `fn open_log(dir: &Path) -> EventLog` — L7251 (in the `#[cfg(test)] mod tests { use super::*; }` block, L7246). `mk_memory(text)` reference shape — L7393–7406.

**`crates/bossclaw-core/src/graph.rs`**
- `pub const MEMORY_EVENT_TYPE: &str = "memory";` — L23.
- `pub const EXTERNAL_ORIGIN: &str = "external";` — L64.

**`crates/bossclaw-core/src/ingest.rs`**
- `pub fn is_external(event: &Event) -> bool` — L633–635 (reads `content["origin"] == EXTERNAL_ORIGIN`). `#[cfg(unix)]`-scoped `impl` block above it, but the fn itself is at module scope; the daemon is Unix-only regardless.
- Reference build of a signed external-tainted event (`file_ingested_content` L604–628; content is `{"text": …, "origin": EXTERNAL_ORIGIN, …}`).

**`crates/bossclaw-core/src/event.rs`**
- `pub struct Event { id, ts, valid_time, event_type (serde rename "type"), content, model_meta, prev_hash, hash, signed_by_did, signature }` — L24–50. `hash`/`signature` set by `append`.

**`crates/bossclaw-core/src/error.rs`**
- `pub enum BossclawError { … InvalidInput(String) … }` — `InvalidInput` at L43.

**`crates/bossclaw-core/src/lib.rs`**
- Public re-exports: `Embedder`, `MockEmbedder` (L53), `BossclawError` (L54), `Hit`, `RecallOptions`, `RecallSource` (L85).

**`crates/bossclawd/src/engine/mod.rs`**
- `pub struct EngineHandle` — L256; field `db_path: PathBuf` = `data_dir.join("brain.db")` (L259, set in `new` L301). **No stored `data_dir`; derive it via `db_path.parent()`.**
- `pub fn new(vault, data_dir, embedder_provider, reasoner_provider) -> Self` — L292–311.
- `pub async fn get_or_open(&self, onboarded: bool) -> Result<Arc<EventLog>, EngineError>` — L337 (returns `NotOnboarded` when `!onboarded`; else `keystore.load_or_mint()` — **which MINTS on a fresh machine**, the footgun U2 closes for `MemoryClient`).
- `pub async fn run_ingest` — L481 (resolves embedder via `embedder_provider.embedder_for(&log)?` then sets `*self.indexed.lock().await = true`).
- `async fn ensure_indexed(&self, log) -> Result<Arc<dyn Embedder>, EngineOpError>` — L535 (rebuilds only when `!*self.indexed`, then sets it true).
- `pub async fn recall(&self, onboarded, query, k) -> Result<Vec<HitWithText>, EngineOpError>` — L558 (`get_or_open` → `ensure_indexed` → `spawn_blocking(log.recall)` → hydrate snippet from `content["text"]`).
- `async fn publish_and_invalidate(&self, candidate)` — L1458 (sets `*self.indexed.lock().await = false` to force the next recall to rebuild — **the exact pattern `remember` reuses**).
- `indexed: Mutex<bool>` field — L268.

**`crates/bossclawd/src/engine/embed.rs`**
- `trait EmbedderProvider { fn embedder(&self) -> Result<Arc<dyn Embedder>, EngineOpError>; fn embedder_for(&self, log: &EventLog) -> Result<Arc<dyn Embedder>, EngineOpError> { default → self.embedder() } }` — L57/L61.

**`crates/bossclawd/src/identity.rs`**
- `pub fn is_onboarded(data_dir: &Path) -> bool` — reads `<data_dir>/identity.json`, `true` iff it parses (did/name/created_at present). Fail-safe false. Module `pub mod identity;` in `lib.rs` L32.

**`crates/bossclawd/src/main.rs`** (mirror these constants in the adapter's socket resolution)
- `ENV_SOCKET = "BOSSCLAWD_SOCKET"` (L48), `ENV_DATA_DIR = "BOSSCLAWD_DATA_DIR"` (L46), `SOCKET_FILE = "bossclawd.sock"` (L55), `APP_DIR_NAME = "ai.air-agent.desktop"` (L59). `resolve_socket_path(data_dir) = BOSSCLAWD_SOCKET | data_dir/bossclawd.sock` (L213). `app_data_dir` = macOS `~/Library/Application Support/ai.air-agent.desktop`, Linux `$XDG_DATA_HOME|~/.local/share`/`ai.air-agent.desktop` (L197–210).

**Client + transport (reference patterns; the app must be recompiled after the `Hello` change)**
- `apps/desktop/src-tauri/src/engine/transport.rs` — `Hello { proto_version: PROTO_VERSION }` at L92 (`SocketTransport::connect`) + L256 (`DuplexTransport::handshake`).
- `apps/desktop/src-tauri/src/engine/daemon.rs` — `Hello { proto_version: PROTO_VERSION }` at L85 (`probe`).
- `apps/desktop/src-tauri/src/engine/client.rs` — `op_error_from_wire` exhaustive `match kind` at L475–493 (**adding an `OpErrorKindWire` arm forces a new arm here**); `teardown_error_from_response` at L506–518 has a `_ =>` catch-all (no change needed).
- `crates/memharness/src/client.rs` — `Hello { proto_version: PROTO_VERSION }` at L31.
- `crates/bossclawd/tests/roundtrip.rs` (L30), `crates/bossclawd/tests/invariants.rs` (L84, L158, L222) — test `Hello` construction sites.

**Every in-tree `Hello { … }` construction site that must gain `role:` (compile-forced by U2):** `bossclawd-proto/src/lib.rs:647`, `bossclawd-proto/src/lib.rs:664`, `crates/memharness/src/client.rs:31`, `crates/bossclawd/tests/roundtrip.rs:30`, `crates/bossclawd/tests/invariants.rs:84`, `crates/bossclawd/tests/invariants.rs:158`, `crates/bossclawd/tests/invariants.rs:222`, `apps/desktop/src-tauri/src/engine/daemon.rs:85`, `apps/desktop/src-tauri/src/engine/transport.rs:92`, `apps/desktop/src-tauri/src/engine/transport.rs:256`. (The `air-rs` `ClientFrame::Hello` sites are an UNRELATED protocol — do **not** touch them.)

---

## Naming contract (pin every shared name; do not deviate)

**`bossclawd-proto` (`crates/bossclawd-proto/src/lib.rs`)**
- `pub enum Role { App, MemoryClient }` — `#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]`, with `#[default] App`.
- `Hello.role: Role` — new field, `#[serde(default)]` (wire back-compat: a peer that omits it deserializes as `App`).
- `impl Role { pub fn allows(&self, req: &Request) -> bool }` — the fail-closed per-op allowlist.
- `Request::Remember { onboarded: bool, text: String }` — new variant (append after `ModelStatus`).
- `Response::Remember(String)` — new variant carrying the new event id.
- `OpErrorKindWire::NotPermitted` — new arm (append after `KeystoreDbMismatch`).

**`bossclaw-core` (`crates/bossclaw-core/src/log.rs`)**
- `pub fn remember(&self, embedder: &dyn Embedder, text: &str) -> Result<String, BossclawError>` — new method on `impl EventLog`.

**`bossclawd` engine (`crates/bossclawd/src/engine/mod.rs`)**
- `pub async fn remember(&self, onboarded: bool, text: String) -> Result<String, EngineOpError>` — new `EngineHandle` method.
- `pub fn is_onboarded_local(&self) -> bool` — new `EngineHandle` method (`self.db_path.parent().map(crate::identity::is_onboarded).unwrap_or(false)`).

**`bossclawd` dispatch (`crates/bossclawd/src/server.rs`)**
- `async fn dispatch(engine: &Arc<EngineHandle>, role: Role, req: Request) -> Response` — signature gains `role`.
- `fn not_permitted_response() -> Response` — `Response::Err { kind: NotPermitted, message: "operation not permitted for this connection's role".into() }`.
- `fn override_onboarding_for_guest(req: Request, onboarded: bool) -> Request` — rewrites the `onboarded` flag of `Recall`/`Remember` for the `MemoryClient` role only.

**`air-memory-mcp` (new crate `crates/air-memory-mcp`)**
- Bin name `air-memory-mcp`. Package name `air-memory-mcp`.
- Module `daemon` (`src/daemon.rs`): `pub enum DaemonError { Unavailable(String), NotOnboarded, EmptyText, InvalidArgs(String), Wire(String), Protocol(String) }` with `pub fn user_message(&self) -> String`; `pub fn resolve_socket_path() -> PathBuf`; `pub async fn call_daemon(sock: &Path, req: Request) -> Result<Response, DaemonError>`; `pub async fn tool_recall(sock: &Path, query: &str, k: usize) -> Result<String, DaemonError>`; `pub async fn tool_remember(sock: &Path, text: &str) -> Result<String, DaemonError>`.
- Module `mcp` (`src/mcp.rs`): `pub async fn handle_message(sock: &Path, line: &str) -> Option<String>`; consts `SERVER_NAME = "air-memory-mcp"`, `SERVER_VERSION = env!("CARGO_PKG_VERSION")`, `DEFAULT_PROTOCOL_VERSION = "2025-06-18"`, `TOOL_RECALL = "recall"`, `TOOL_REMEMBER = "remember"`, `DEFAULT_RECALL_K: usize = 8`.
- `src/main.rs`: `#[tokio::main(flavor = "current_thread")]` stdio read→`handle_message`→write loop.

---

## File structure

| File | Responsibility |
|---|---|
| `crates/bossclaw-core/src/log.rs` | **Modify.** Add `EventLog::remember` (core write chokepoint) + its unit test. |
| `crates/bossclawd-proto/src/lib.rs` | **Modify.** Add `Role` enum, `Hello.role`, `Role::allows`, `Request::Remember`, `Response::Remember`, `OpErrorKindWire::NotPermitted`; extend the round-trip tests. |
| `crates/bossclawd/src/engine/mod.rs` | **Modify.** Add `EngineHandle::remember` + `EngineHandle::is_onboarded_local`. |
| `crates/bossclawd/src/server.rs` | **Modify.** Thread `Role` from `serve_connection` into `dispatch`; enforce the allowlist fail-closed; add the `Remember` dispatch arm; recompute onboarding for `MemoryClient`. |
| `crates/bossclawd/tests/roundtrip.rs` | **Modify.** Update `Hello` construction to `role: Role::App`; add the `Remember`→`Recall` round-trip (App role). |
| `crates/bossclawd/tests/invariants.rs` | **Modify.** Update three `Hello` construction sites to `role: Role::App`. |
| `crates/bossclawd/tests/authz.rs` | **Create.** The U2 authorization matrix over a real socket: refusals, allows, App-all, fail-closed default, mint-footgun. |
| `crates/bossclawd/tests/memory_client_loop.rs` | **Create.** The U4 end-to-end loop as a `MemoryClient`: recall, remember→recall, destructive refused. |
| `crates/memharness/src/client.rs` | **Modify.** Update the one `Hello` construction to `role: Role::App`. |
| `apps/desktop/src-tauri/src/engine/transport.rs` | **Modify.** Update two `Hello` constructions to `role: Role::App`. |
| `apps/desktop/src-tauri/src/engine/daemon.rs` | **Modify.** Update the `probe` `Hello` construction to `role: Role::App`. |
| `apps/desktop/src-tauri/src/engine/client.rs` | **Modify.** Add the `OpErrorKindWire::NotPermitted` arm to `op_error_from_wire`. |
| `Cargo.toml` (workspace) | **Modify.** Add `crates/air-memory-mcp` to `members`. |
| `crates/air-memory-mcp/Cargo.toml` | **Create.** The adapter package manifest. |
| `crates/air-memory-mcp/src/main.rs` | **Create.** The stdio JSON-RPC loop entry point. |
| `crates/air-memory-mcp/src/daemon.rs` | **Create.** The thin socket client over `bossclawd-proto` + tool functions. |
| `crates/air-memory-mcp/src/mcp.rs` | **Create.** The hand-rolled MCP JSON-RPC 2.0 message handler. |
| `crates/air-memory-mcp/tests/adapter.rs` | **Create.** Adapter-level tests (MCP shapes + fake-daemon wire assertions + daemon-down). |
| `crates/air-memory-mcp/README.md` | **Create.** The `.mcp.json` wiring snippet + usage. |

---

## Phase A — U1: the `remember` write op

### Task A1: core `EventLog::remember`

**Files:**
- Modify: `crates/bossclaw-core/src/log.rs` (add the method in the main `impl EventLog` block near `derive_vector_for`, ~L4483; add the test in the `#[cfg(test)] mod tests` block, ~L7255).

- [ ] **Step 1: Write the failing test**

Add to `crates/bossclaw-core/src/log.rs` inside `mod tests` (after `open_log`, ~L7255):

```rust
    #[test]
    fn remember_appends_external_tainted_recallable_memory() {
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        let embedder = MockEmbedder::new(8);

        // A remembered note is a signed `memory` event stamped external-taint.
        let id = log.remember(&embedder, "ferris the crab loves rust").unwrap();
        let ev = log.event_by_id(&id).unwrap().expect("event present");
        assert_eq!(ev.event_type, "memory", "remember writes a memory-type event");
        assert_eq!(
            ev.content.get("origin").and_then(|v| v.as_str()),
            Some("external"),
            "remembered memories are external-tainted (I2): recallable, never auto-trusted"
        );
        assert_eq!(
            ev.content.get("text").and_then(|v| v.as_str()),
            Some("ferris the crab loves rust"),
            "the note text is stored top-level so the embedder finds it"
        );

        // Recallable immediately: rebuild the indexes, then a recall surfaces it.
        log.rebuild_indexes(&embedder).unwrap();
        let hits = log
            .recall(&embedder, "ferris rust", 5, &RecallOptions::default())
            .unwrap();
        assert!(hits.iter().any(|h| h.event_id == id), "remembered note is recallable");

        // Empty / blank text is rejected (no empty memory events).
        assert!(matches!(
            log.remember(&embedder, "   "),
            Err(BossclawError::InvalidInput(_))
        ));
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p bossclaw-core remember_appends_external_tainted_recallable_memory`
Expected: FAIL to compile — `error[E0599]: no method named `remember` found for struct `EventLog``.

- [ ] **Step 3: Write the minimal implementation**

Add to `crates/bossclaw-core/src/log.rs` in the main `impl EventLog` block, immediately above `derive_vector_for` (~L4481):

```rust
    /// Append a signed `memory`-type event carrying `text`, stamped `origin = external`
    /// (the taint model, single-sourced via [`crate::graph::EXTERNAL_ORIGIN`]) so a
    /// remembered note is recallable (`memory` ∈ [`EMBEDDABLE_EVENT_TYPES`]) yet never
    /// auto-trusted downstream (`is_external` stays true). Derives + persists the note's
    /// vector so a subsequent [`EventLog::rebuild_indexes`] + `recall` surfaces it.
    /// Rejects empty/blank text with [`BossclawError::InvalidInput`] (no empty events).
    /// Tier-A (`model_meta: None`), signed by the engine DID like every ground-truth write.
    pub fn remember(&self, embedder: &dyn Embedder, text: &str) -> Result<String, BossclawError> {
        if text.trim().is_empty() {
            return Err(BossclawError::InvalidInput("cannot remember empty or blank text".into()));
        }
        let event = Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: crate::graph::MEMORY_EVENT_TYPE.to_string(),
            content: serde_json::json!({
                "text": text,
                "origin": crate::graph::EXTERNAL_ORIGIN,
            }),
            model_meta: None,
            prev_hash: String::new(),
            hash: None,
            signed_by_did: self.signer_did(),
            signature: None,
        };
        let id = self.append(event)?;
        self.derive_vector_for(embedder, &id)?;
        Ok(id)
    }
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p bossclaw-core remember_appends_external_tainted_recallable_memory`
Expected: PASS (`test tests::remember_appends_external_tainted_recallable_memory ... ok`).

- [ ] **Step 5: Guard the whole crate still builds + lints clean**

Run: `cargo clippy -p bossclaw-core --all-targets -- -D warnings`
Expected: finishes with no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/bossclaw-core/src/log.rs
git commit -m "feat(bossclaw-core): remember() — signed external-tainted recallable memory write (U1)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

### Task A2: wire the write op — proto `Remember` + dispatch arm + engine wrapper

**Files:**
- Modify: `crates/bossclawd-proto/src/lib.rs` (add `Request::Remember` after L152's `ModelStatus`; add `Response::Remember(String)` after L217's `ModelStatus(ModelStatusWire)`; extend the round-trip test at L473).
- Modify: `crates/bossclawd/src/engine/mod.rs` (add `EngineHandle::remember` after `recall`, ~L585).
- Modify: `crates/bossclawd/src/server.rs` (add the `Request::Remember` dispatch arm, in the "Ingest / recall" group, ~L167).
- Modify: `crates/bossclawd/tests/roundtrip.rs` (the RED test; first update its `Hello` at L30 — **note:** the `role` field does not exist yet, so this update happens in Phase B; for A2 the round-trip test uses the still-`role`-less `Hello`).

**Important ordering note:** A2 does NOT add `Role` yet. The `Hello` at `roundtrip.rs:30` stays `Hello { proto_version: PROTO_VERSION }` (App semantics are the default once Phase B lands). The `Remember`→`Recall` round-trip is driven as the ordinary (soon-to-be-`App`) client.

- [ ] **Step 1: Write the failing test**

Add to `crates/bossclawd/tests/roundtrip.rs` (a new `#[tokio::test]`, following the existing `Client`/`spawn_daemon` helpers):

```rust
/// U1 over the wire: a `Remember` writes a memory the very next `Recall` surfaces
/// (the memory is embeddable + the engine invalidates the recall index on write).
#[tokio::test]
async fn remember_then_recall_roundtrips_over_socket() {
    let (_dir, sock) = spawn_daemon().await;
    let mut client = Client::connect(&sock).await;

    let resp = client
        .call(Request::Remember { onboarded: true, text: "aria novak ships rust".to_string() })
        .await;
    let event_id = match resp {
        Response::Remember(id) => id,
        other => panic!("expected Response::Remember, got {other:?}"),
    };
    assert!(!event_id.is_empty(), "Remember returns the new event id");

    let hits = match client
        .call(Request::Recall { onboarded: true, query: "aria rust".to_string(), k: 5 })
        .await
    {
        Response::Recall(hits) => hits,
        other => panic!("expected Response::Recall, got {other:?}"),
    };
    assert!(
        hits.iter().any(|h| h.hit.event_id == event_id && h.text.contains("aria novak")),
        "the remembered note is recalled with its hydrated snippet"
    );

    // Empty text is rejected (typed Rejected).
    let err = client
        .call(Request::Remember { onboarded: true, text: "   ".to_string() })
        .await;
    assert!(
        matches!(err, Response::Err { kind: bossclawd_proto::OpErrorKindWire::Rejected, .. }),
        "blank remember → Rejected, got {err:?}"
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p bossclawd --test roundtrip remember_then_recall_roundtrips_over_socket`
Expected: FAIL to compile — `error[E0599]: no variant named `Remember` found for enum `Request``.

- [ ] **Step 3: Add the proto `Request::Remember` variant**

In `crates/bossclawd-proto/src/lib.rs`, append inside `enum Request` immediately after the `ModelStatus { onboarded: bool }` arm (L151):

```rust
    /// `EngineHandle::remember` → the coding-agent write op (SP1 / M1b). Appends a signed
    /// `memory`-type event stamped `origin=external` (recallable, never auto-trusted). No
    /// Tauri command maps to it (it is reached only via the `air-memory-mcp` adapter as a
    /// `MemoryClient`); `onboarded` mirrors every other op.
    Remember { onboarded: bool, text: String },
```

- [ ] **Step 4: Add the proto `Response::Remember` variant**

In `crates/bossclawd-proto/src/lib.rs`, append inside `enum Response` immediately after the `ModelStatus(ModelStatusWire)` arm (L217):

```rust
    /// `Remember` result — the id of the newly appended `memory` event.
    Remember(String),
```

- [ ] **Step 5: Extend the proto serde round-trip test**

In `crates/bossclawd-proto/src/lib.rs`, in `request_response_serde_roundtrip` (L473): add to the `requests` vec (after the `Teardown` entry, L496):

```rust
            Request::Remember { onboarded: true, text: "remember me".to_string() },
```

and to the `responses` vec (after `Response::Busy(...)`, L542):

```rust
            Response::Remember("01J-REMEMBERED".to_string()),
```

- [ ] **Step 6: Add the `EngineHandle::remember` wrapper**

In `crates/bossclawd/src/engine/mod.rs`, add immediately after `recall` (~L585, before `evolve_once`):

```rust
    /// Append a signed external-tainted `memory` (U1) and return its event id. Resolves the
    /// active embedder (env → signed record → bundled), derives the note's vector on a blocking
    /// thread, then invalidates the recall index so the NEXT `recall` rebuilds and surfaces it
    /// (the same index-invalidation contract as `publish_and_invalidate`). Empty/blank text is
    /// the engine's typed `Rejected`; any other core failure folds to `Core`.
    pub async fn remember(&self, onboarded: bool, text: String) -> Result<String, EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        let embedder = self.embedder_provider.embedder_for(&log)?;
        let id = spawn_blocking(move || -> Result<String, EngineOpError> {
            log.remember(&*embedder, &text).map_err(|e| match e {
                bossclaw_core::BossclawError::InvalidInput(m) => EngineOpError::Rejected(m),
                other => EngineOpError::Core(other.to_string()),
            })
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))??;
        // Force the next recall to rebuild the in-memory index so the new memory is searchable
        // (its vector is persisted; the FTS entry is (re)built by `rebuild_indexes`).
        *self.indexed.lock().await = false;
        Ok(id)
    }
```

(`spawn_blocking` is already imported in this module — it is used by `recall`/`ensure_indexed`.)

- [ ] **Step 7: Add the `Request::Remember` dispatch arm**

In `crates/bossclawd/src/server.rs`, inside `dispatch`'s `match req` in the "Ingest / recall" group, after the `Request::Recall { … }` arm (ends L167):

```rust
        Request::Remember { onboarded, text } => {
            op_result(engine.remember(onboarded, text).await, Response::Remember)
        }
```

- [ ] **Step 8: Run the round-trip test to verify it passes**

Run: `cargo test -p bossclawd --test roundtrip remember_then_recall_roundtrips_over_socket`
Expected: PASS.

- [ ] **Step 9: Run the proto tests + workspace clippy**

Run: `cargo test -p bossclawd-proto && cargo clippy -p bossclawd -p bossclawd-proto --all-targets -- -D warnings`
Expected: all green, no warnings.

- [ ] **Step 10: Commit**

```bash
git add crates/bossclawd-proto/src/lib.rs crates/bossclawd/src/engine/mod.rs crates/bossclawd/src/server.rs crates/bossclawd/tests/roundtrip.rs
git commit -m "feat(bossclawd): Request::Remember wire op + engine wrapper + dispatch arm (U1)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Phase B — U2: per-op authorization (the security-critical unit)

This phase lands the `Role` type **and** its `dispatch` enforcement **and** every compile-forced `Hello`/`OpErrorKindWire` update in **one commit**, so there is never a tree state where `Role` exists but is unenforced (per the exhaustive-match/never-red constraint). It is the highest-risk unit: a hole re-exposes destructive/egress ops to an external agent.

### Task B1: `Role` handshake + fail-closed dispatch enforcement + onboarding recompute

**Files:**
- Modify: `crates/bossclawd-proto/src/lib.rs` (add `Role`, `Hello.role`, `Role::allows`, `OpErrorKindWire::NotPermitted`; update the two in-file `Hello` sites + the `op_error_kind_wire_roundtrips_every_arm` array; add `Role` unit tests).
- Modify: `crates/bossclawd/src/server.rs` (capture `role` in `serve_connection`; thread it into `dispatch`; enforce the allowlist; recompute onboarding for `MemoryClient`).
- Modify: `crates/bossclawd/src/engine/mod.rs` (add `is_onboarded_local`).
- Modify: `crates/bossclawd/tests/invariants.rs`, `crates/bossclawd/tests/roundtrip.rs`, `crates/memharness/src/client.rs`, `apps/desktop/src-tauri/src/engine/transport.rs`, `apps/desktop/src-tauri/src/engine/daemon.rs`, `apps/desktop/src-tauri/src/engine/client.rs` (compile-forced updates enumerated in "Verified current anchors").
- Create: `crates/bossclawd/tests/authz.rs` (the authorization matrix).

- [ ] **Step 1: Write the failing authorization-matrix test**

Create `crates/bossclawd/tests/authz.rs`:

```rust
//! U2 — per-op authorization at the daemon boundary. Drives a REAL `bossclawd` accept loop over a
//! temp Unix socket (`server::spawn_for_test`) with a hermetic engine (in-memory vault + mock
//! providers, NEVER the OS keychain). A `MemoryClient`-role connection must be REFUSED every op
//! outside `{Recall, Remember}` (fail-closed) and ALLOWED those two; an `App`-role connection is
//! allowed everything. Unix-only (the daemon + socket are Unix-only).
#![cfg(unix)]

use std::path::PathBuf;

use bossclawd::server;
use bossclawd_proto::{
    read_frame, write_frame, Hello, HelloOk, OpErrorKindWire, Request, Response, Role, PROTO_VERSION,
};
use tokio::net::UnixStream;

/// A connected test client that handshakes with a chosen `Role`, then speaks framed `Request`s.
struct RoleClient {
    stream: UnixStream,
}

impl RoleClient {
    async fn connect(sock_path: &std::path::Path, role: Role) -> Self {
        let mut stream = UnixStream::connect(sock_path).await.expect("connect to daemon socket");
        let hello = Hello { proto_version: PROTO_VERSION, role };
        write_frame(&mut stream, &serde_json::to_vec(&hello).unwrap()).await.expect("send Hello");
        let reply = read_frame(&mut stream).await.expect("read HelloOk");
        let hello_ok: HelloOk = serde_json::from_slice(&reply).expect("parse HelloOk");
        assert_eq!(hello_ok.proto_version, PROTO_VERSION, "daemon speaks our version");
        Self { stream }
    }

    async fn call(&mut self, req: Request) -> Response {
        write_frame(&mut self.stream, &serde_json::to_vec(&req).unwrap()).await.expect("send req");
        let frame = read_frame(&mut self.stream).await.expect("read resp");
        serde_json::from_slice(&frame).expect("parse resp")
    }
}

/// Spawn a daemon on a fresh temp socket + temp home. Writes a valid `identity.json` into the home
/// so the daemon's `MemoryClient` onboarding recompute reports ONBOARDED (the happy fixture).
async fn spawn_onboarded_daemon() -> (tempfile::TempDir, PathBuf) {
    bossclawd::vault::seed_secret_cache_for_test(Default::default());
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().to_path_buf();
    std::fs::write(
        home.join("identity.json"),
        serde_json::json!({
            "did": "did:wba:example.com:tester",
            "name": "Tester",
            "created_at": "2026-07-09T00:00:00+00:00"
        })
        .to_string(),
    )
    .unwrap();
    let sock_path = home.join("bossclawd.sock");
    server::spawn_for_test(sock_path.clone(), home).await;
    (dir, sock_path)
}

fn is_not_permitted(resp: &Response) -> bool {
    matches!(resp, Response::Err { kind: OpErrorKindWire::NotPermitted, .. })
}

/// Every destructive / egress / mutation op is REFUSED for `MemoryClient` (fail-closed).
#[tokio::test]
async fn memory_client_is_refused_destructive_ops() {
    let (_dir, sock) = spawn_onboarded_daemon().await;
    let mut c = RoleClient::connect(&sock, Role::MemoryClient).await;

    // Teardown (identity reset — the most destructive op).
    assert!(is_not_permitted(&c.call(Request::Teardown).await), "Teardown must be refused");
    // EnableCloudReasoner (network egress).
    assert!(
        is_not_permitted(
            &c.call(Request::EnableCloudReasoner {
                onboarded: true,
                config: serde_json::json!({"mode": "cloud"}),
            })
            .await
        ),
        "EnableCloudReasoner must be refused"
    );
    // A grant op (opens a folder to ingestion).
    assert!(
        is_not_permitted(
            &c.call(Request::AddGrant { onboarded: true, path: PathBuf::from("/etc") }).await
        ),
        "AddGrant must be refused"
    );
    // A model op (triggers a 500MB+ download + re-embed migration).
    assert!(
        is_not_permitted(
            &c.call(Request::SetActiveModel {
                onboarded: true,
                model_id: "x".to_string(),
                safetensors_sha: "y".to_string(),
            })
            .await
        ),
        "SetActiveModel must be refused"
    );
    // FAIL-CLOSED: a plain read op that is NOT on the allowlist is still refused.
    assert!(
        is_not_permitted(&c.call(Request::Status { onboarded: true }).await),
        "Status is not on the MemoryClient allowlist → refused (fail-closed default)"
    );
}

/// `MemoryClient` is ALLOWED exactly `Recall` + `Remember`.
#[tokio::test]
async fn memory_client_is_allowed_recall_and_remember() {
    let (_dir, sock) = spawn_onboarded_daemon().await;
    let mut c = RoleClient::connect(&sock, Role::MemoryClient).await;

    let id = match c.call(Request::Remember { onboarded: true, text: "guest note".into() }).await {
        Response::Remember(id) => id,
        other => panic!("Remember must be allowed, got {other:?}"),
    };
    assert!(!id.is_empty());

    let hits = match c.call(Request::Recall { onboarded: true, query: "guest".into(), k: 5 }).await {
        Response::Recall(hits) => hits,
        other => panic!("Recall must be allowed, got {other:?}"),
    };
    assert!(hits.iter().any(|h| h.hit.event_id == id), "recall surfaces the just-remembered note");
}

/// `App` (the default role) is allowed every op — the existing app is unchanged.
#[tokio::test]
async fn app_role_is_allowed_all_ops() {
    let (_dir, sock) = spawn_onboarded_daemon().await;
    let mut c = RoleClient::connect(&sock, Role::App).await;
    // A representative destructive op the MemoryClient is refused: for App it dispatches normally
    // (a fresh brain teardown succeeds — the point is it is NOT NotPermitted).
    let resp = c.call(Request::Status { onboarded: true }).await;
    assert!(matches!(resp, Response::Status(_)), "App may call Status, got {resp:?}");
    let resp = c.call(Request::AddGrant { onboarded: true, path: std::env::temp_dir() }).await;
    assert!(!is_not_permitted(&resp), "App is never NotPermitted, got {resp:?}");
}

/// The mint footgun (security): a `MemoryClient` CANNOT assert onboarding to force a keystore mint.
/// Against a daemon whose home has NO identity.json, recall/remember return NotOnboarded even though
/// the client sends `onboarded: true` — the daemon recomputes onboarding itself for the guest role.
#[tokio::test]
async fn memory_client_cannot_force_onboarding() {
    bossclawd::vault::seed_secret_cache_for_test(Default::default());
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().to_path_buf(); // NO identity.json written.
    let sock = home.join("bossclawd.sock");
    server::spawn_for_test(sock.clone(), home).await;

    let mut c = RoleClient::connect(&sock, Role::MemoryClient).await;
    // Client lies `onboarded: true`; the daemon overrides it with its own (false) check.
    assert!(
        matches!(c.call(Request::Recall { onboarded: true, query: "x".into(), k: 1 }).await, Response::NotOnboarded),
        "guest recall on a not-onboarded brain → NotOnboarded (no mint)"
    );
    assert!(
        matches!(c.call(Request::Remember { onboarded: true, text: "x".into() }).await, Response::NotOnboarded),
        "guest remember on a not-onboarded brain → NotOnboarded (no mint)"
    );
}
```

- [ ] **Step 2: Run the matrix test to verify it fails**

Run: `cargo test -p bossclawd --test authz`
Expected: FAIL to compile — `error[E0432]: unresolved import `bossclawd_proto::Role`` (and `Hello` has no field `role`).

- [ ] **Step 3: Add the `Role` enum + `Role::allows`**

In `crates/bossclawd-proto/src/lib.rs`, add just above `pub struct Hello` (L50):

```rust
/// The privilege a connection requests at the [`Hello`] handshake. The daemon enforces a per-role
/// op-allowlist in `dispatch` (fail-closed: an op not explicitly allowed for the role is refused).
///
/// Defaults to [`Role::App`] (full access) so a peer that omits the field — including any build
/// predating this field — connects exactly as before: the desktop app is unchanged. Only a client
/// that OPTS INTO [`Role::MemoryClient`] is scoped down (the `air-memory-mcp` adapter). This is the
/// "Simple" bar (least-privilege-by-default); cryptographic capability tokens ("Strict") are a
/// deferred future hardening — a same-uid process can already forge `App`, so this does not defend
/// against a malicious peer, only scopes a cooperative one.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Role {
    /// Full access to every wire op (the default; the desktop app).
    #[default]
    App,
    /// A scoped coding-agent client: may invoke ONLY `Recall` + `Remember`.
    MemoryClient,
}

impl Role {
    /// Fail-closed per-op allowlist. `App` may invoke every op. `MemoryClient` may invoke ONLY
    /// [`Request::Recall`] + [`Request::Remember`]; **every other variant — present or future — is
    /// refused by default** (the `matches!` denies anything not explicitly listed). A new `Request`
    /// variant is therefore refused for `MemoryClient` until someone deliberately adds it here.
    pub fn allows(&self, req: &Request) -> bool {
        match self {
            Role::App => true,
            Role::MemoryClient => {
                matches!(req, Request::Recall { .. } | Request::Remember { .. })
            }
        }
    }
}
```

- [ ] **Step 4: Add the `role` field to `Hello`**

In `crates/bossclawd-proto/src/lib.rs`, change `struct Hello` (L50–54) to:

```rust
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct Hello {
    /// The protocol version the client was built against.
    pub proto_version: u32,
    /// The privilege the connection requests. `#[serde(default)]` so a peer that omits it (any
    /// build predating this field) is treated as [`Role::App`] — the app is unchanged (I3).
    #[serde(default)]
    pub role: Role,
}
```

- [ ] **Step 5: Add the `OpErrorKindWire::NotPermitted` arm**

In `crates/bossclawd-proto/src/lib.rs`, append inside `enum OpErrorKindWire` after `KeystoreDbMismatch` (L266):

```rust
    /// A per-op authorization refusal (U2): the connection's [`Role`] does not allow the requested
    /// op. A daemon-protocol fault with no in-process engine analogue (like `Core`); the client
    /// renders it under the generic engine-error prefix. The desktop app never receives it (it is
    /// always `App`), but the arm keeps the wire enum exhaustive.
    NotPermitted,
```

- [ ] **Step 6: Update the two in-file `Hello` construction sites + the round-trip array**

In `crates/bossclawd-proto/src/lib.rs`:
- L647 (`handshake_serde_roundtrip`): `let hello = Hello { proto_version: PROTO_VERSION, role: Role::App };`
- L664 (`hello_is_not_a_request`): `let hello_bytes = serde_json::to_vec(&Hello { proto_version: 1, role: Role::App }).unwrap();`
- In `op_error_kind_wire_roundtrips_every_arm` (L560), append to the `kinds` array after `OpErrorKindWire::KeystoreDbMismatch,`:

```rust
            OpErrorKindWire::NotPermitted,
```

- [ ] **Step 7: Add `Role` unit tests to the proto test module**

In `crates/bossclawd-proto/src/lib.rs`, add inside `mod protocol_tests` (after `hello_is_not_a_request`, ~L668):

```rust
    /// The `MemoryClient` allowlist is exactly `{Recall, Remember}` and fail-closed on everything
    /// else. Mutation-resistant: adding any other op to `Role::allows` fails an assertion here.
    #[test]
    fn memory_client_allowlist_is_exactly_recall_and_remember() {
        let allowed = [
            Request::Recall { onboarded: true, query: "q".into(), k: 1 },
            Request::Remember { onboarded: true, text: "t".into() },
        ];
        for req in &allowed {
            assert!(Role::MemoryClient.allows(req), "MemoryClient must allow {req:?}");
        }
        let refused = [
            Request::Teardown,
            Request::EnableCloudReasoner { onboarded: true, config: serde_json::Value::Null },
            Request::AddGrant { onboarded: true, path: std::path::PathBuf::from("/x") },
            Request::AddMandate {
                onboarded: true,
                target: std::path::PathBuf::from("/t"),
                source_scope: std::path::PathBuf::from("/s"),
                recipe: "r".into(),
            },
            Request::SetActiveModel { onboarded: true, model_id: "m".into(), safetensors_sha: "s".into() },
            Request::Status { onboarded: true },
            Request::RunIngest { onboarded: true },
        ];
        for req in &refused {
            assert!(!Role::MemoryClient.allows(req), "MemoryClient must REFUSE {req:?} (fail-closed)");
        }
        // App is allowed everything.
        for req in allowed.iter().chain(refused.iter()) {
            assert!(Role::App.allows(req), "App must allow {req:?}");
        }
    }

    /// A `Hello` frame that omits `role` (a peer predating the field) deserializes as `App` — the
    /// wire back-compat that keeps the app unchanged (I3).
    #[test]
    fn hello_role_defaults_to_app_on_missing_field() {
        let legacy = serde_json::json!({ "proto_version": PROTO_VERSION }).to_string();
        let hello: Hello = serde_json::from_str(&legacy).unwrap();
        assert_eq!(hello.role, Role::App);
    }
```

- [ ] **Step 8: Add `EngineHandle::is_onboarded_local`**

In `crates/bossclawd/src/engine/mod.rs`, add to `impl EngineHandle` (near `get_or_open`, ~L367):

```rust
    /// The daemon's OWN onboarding check (`<data_dir>/identity.json` parses), used to override a
    /// `MemoryClient`'s self-asserted `onboarded` flag so a guest-pass client can never force a
    /// keystore mint / brain creation. `data_dir` is `db_path`'s parent (`db_path =
    /// data_dir/brain.db`). Fail-safe false if the parent is unresolvable.
    pub fn is_onboarded_local(&self) -> bool {
        self.db_path.parent().map(crate::identity::is_onboarded).unwrap_or(false)
    }
```

- [ ] **Step 9: Thread `Role` into `dispatch` + enforce fail-closed + recompute guest onboarding**

In `crates/bossclawd/src/server.rs`:

(a) Import `Role` — extend the `use bossclawd_proto::{…}` at L31–34 to include `Role`:

```rust
use bossclawd_proto::{
    read_frame, write_frame, Hello, HelloOk, HitWire, OpErrorKindWire, Request, Response, Role,
    PROTO_VERSION,
};
```

(b) In `serve_connection`, capture the role from the accepted `Hello` and pass it to `dispatch`. Change the `else`/binding at L61 from `let Ok(hello) = …` to keep `hello`, then at the dispatch call (L103) pass `hello.role`:

```rust
    let Ok(hello) = serde_json::from_slice::<Hello>(&first) else {
```
(unchanged) … and at L103:
```rust
            Ok(req) => dispatch(&engine, hello.role, req).await,
```
(`hello` is already in scope for the whole fn after the handshake — it is moved out of the `let Ok(hello) = …` binding; add `let role = hello.role;` right after the version check at L84 and use `role` at the dispatch call to avoid borrowing `hello` across the loop):

```rust
    write_frame(&mut write, &hello_ok_bytes).await?;
    let role = hello.role;
```
then L103 becomes `Ok(req) => dispatch(&engine, role, req).await,`.

(c) Change the `dispatch` signature (L134) and add the gate + guest-onboarding recompute at the top of the fn body:

```rust
async fn dispatch(engine: &Arc<EngineHandle>, role: Role, req: Request) -> Response {
    // ── Per-op authorization (U2 / I1), fail-closed: a role may invoke only its allow-listed ops. ──
    if !role.allows(&req) {
        return not_permitted_response();
    }
    // A guest-pass (`MemoryClient`) must not be able to ASSERT onboarding — that would let it force
    // a keystore mint / brain creation. The daemon computes onboarding itself for that role; `App`
    // keeps its self-asserted flag (I3 — the app is unchanged).
    let req = match role {
        Role::App => req,
        Role::MemoryClient => override_onboarding_for_guest(req, engine.is_onboarded_local()),
    };
    match req {
```

(the rest of the `match req` body is unchanged, including the `Request::Remember` arm from Task A2).

(d) Add the two helpers near `protocol_err` (~L118):

```rust
/// A per-op authorization refusal (U2). Rides the [`OpErrorKindWire::NotPermitted`] kind; the
/// message is generic (it does not echo the op) so nothing about the refused request leaks.
fn not_permitted_response() -> Response {
    Response::Err {
        kind: OpErrorKindWire::NotPermitted,
        message: "operation not permitted for this connection's role".to_string(),
    }
}

/// Override the `onboarded` flag of a guest (`MemoryClient`) request with the daemon's own
/// onboarding truth. Only `Recall`/`Remember` are reachable for that role (the allowlist), so only
/// those are rewritten; anything else is returned unchanged (defensive — it is already refused).
fn override_onboarding_for_guest(req: Request, onboarded: bool) -> Request {
    match req {
        Request::Recall { query, k, .. } => Request::Recall { onboarded, query, k },
        Request::Remember { text, .. } => Request::Remember { onboarded, text },
        other => other,
    }
}
```

- [ ] **Step 10: Update the compile-forced desktop `op_error_from_wire` arm**

In `apps/desktop/src-tauri/src/engine/client.rs`, add to the `match kind` in `op_error_from_wire` (after the `KeystoreDbMismatch` arm, L489–491):

```rust
        // The app is always `App`, so it never receives this; the arm keeps the match exhaustive.
        OpErrorKindWire::NotPermitted => EngineOpError::Core(message),
```

- [ ] **Step 11: Update every remaining compile-forced `Hello` construction site**

Set `role: Role::App` (import `Role` where needed) at each:
- `crates/bossclawd/tests/roundtrip.rs:30` → `let hello = Hello { proto_version: PROTO_VERSION, role: Role::App };` (add `Role` to the `use bossclawd_proto::{…}` line at L14).
- `crates/bossclawd/tests/invariants.rs:84`, `:158`, `:222` → add `, role: Role::App` (L158 is the skewed-version `Hello`; still `App`). Add `Role` to that file's proto `use`.
- `crates/memharness/src/client.rs:31` → `let hello = Hello { proto_version: PROTO_VERSION, role: Role::App };` (add `Role` to its proto `use`).
- `apps/desktop/src-tauri/src/engine/transport.rs:92` and `:256` → `let hello = Hello { proto_version: PROTO_VERSION, role: Role::App };` (add `Role` to the `use bossclawd_proto::{…}` on L38 and the duplex module's `use` on L190).
- `apps/desktop/src-tauri/src/engine/daemon.rs:85` → `let hello = Hello { proto_version: PROTO_VERSION, role: Role::App };` (add `Role` to its `use bossclawd_proto::{…}` on L21).

- [ ] **Step 12: Run the authz matrix + proto tests to verify they pass**

Run: `cargo test -p bossclawd --test authz && cargo test -p bossclawd-proto`
Expected: all green (5 authz tests + the proto suite incl. the two new `Role` tests).

- [ ] **Step 13: Prove nothing else regressed (App path unchanged)**

Run: `cargo test -p bossclawd --test roundtrip --test invariants && cargo build -p air_agent_desktop -p memharness`
Expected: green — the existing App-role round-trip + invariants pass, and the app + memharness compile with the new `Hello`.

- [ ] **Step 14: Workspace clippy over the touched crates**

Run: `cargo clippy -p bossclawd-proto -p bossclawd -p memharness -p air_agent_desktop --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 15: Commit**

```bash
git add crates/bossclawd-proto/src/lib.rs crates/bossclawd/src/server.rs crates/bossclawd/src/engine/mod.rs crates/bossclawd/tests/authz.rs crates/bossclawd/tests/roundtrip.rs crates/bossclawd/tests/invariants.rs crates/memharness/src/client.rs apps/desktop/src-tauri/src/engine/transport.rs apps/desktop/src-tauri/src/engine/daemon.rs apps/desktop/src-tauri/src/engine/client.rs
git commit -m "feat(bossclawd): per-op authz — MemoryClient role, fail-closed dispatch allowlist, guest onboarding recompute (U2)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Phase C — U3: the Rust MCP adapter (`air-memory-mcp`)

A hand-rolled MCP-over-stdio JSON-RPC 2.0 server exposing exactly `recall` + `remember`, backed by the daemon socket as a `MemoryClient`, reusing `bossclawd-proto`'s frame codec + handshake (never reimplementing the wire protocol — I5).

### Task C1: crate skeleton + workspace member

**Files:**
- Modify: `Cargo.toml` (workspace `members`).
- Create: `crates/air-memory-mcp/Cargo.toml`, `crates/air-memory-mcp/src/main.rs` (temporary stub).

- [ ] **Step 1: Create the package manifest**

Create `crates/air-memory-mcp/Cargo.toml`:

```toml
[package]
name = "air-memory-mcp"
version = "0.0.1"
edition = "2021"
license = "Apache-2.0"
description = "air-memory-mcp: an MCP stdio adapter exposing recall + remember over the bossclawd socket as a scoped MemoryClient (SP1)."
repository = "https://github.com/AgentIdentityRegistry/air-note"

[[bin]]
name = "air-memory-mcp"
path = "src/main.rs"

[dependencies]
# The wire protocol is SINGLE-SOURCED here (I5): frame codec + Request/Response + Hello + Role.
# The adapter never reimplements the codec or handshake. (Pulls bossclaw-core transitively, as
# proto's mirror types require it — an accepted trade for not duplicating the security-sensitive
# wire format in a second language/crate.)
bossclawd-proto = { path = "../bossclawd-proto" }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
# Unix socket + async stdio. `current_thread` runtime is enough (MCP requests are serial); the
# feature set covers `UnixStream` (net), the frame codec (io-util), stdin/stdout (io-std), the
# `#[tokio::main]`/`#[tokio::test]` macros, and per-call timeouts (time).
tokio = { version = "1", features = ["rt", "macros", "net", "io-util", "io-std", "time"] }

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: Create a temporary stub `main.rs`**

Create `crates/air-memory-mcp/src/main.rs`:

```rust
fn main() {
    eprintln!("air-memory-mcp: stub");
}
```

- [ ] **Step 3: Add the crate to the workspace**

In `Cargo.toml`, add `"crates/air-memory-mcp"` to `members`:

```toml
members = ["crates/air-rs", "crates/bossclaw-core", "crates/bossclawd", "crates/bossclawd-proto", "crates/memharness", "crates/air-memory-mcp", "apps/desktop/src-tauri"]
```

- [ ] **Step 4: Verify it builds**

Run: `cargo build -p air-memory-mcp`
Expected: compiles (`Compiling air-memory-mcp v0.0.1` … `Finished`).

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/air-memory-mcp/Cargo.toml crates/air-memory-mcp/src/main.rs
git commit -m "chore(air-memory-mcp): new workspace bin crate skeleton (U3)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

### Task C2: the daemon client module (`daemon.rs`)

**Files:**
- Create: `crates/air-memory-mcp/src/daemon.rs`.
- Modify: `crates/air-memory-mcp/src/main.rs` (declare `mod daemon;`).
- Create: `crates/air-memory-mcp/tests/adapter.rs` (the fake-daemon wire tests).

- [ ] **Step 1: Write the failing fake-daemon test**

Create `crates/air-memory-mcp/tests/adapter.rs`:

```rust
//! Adapter-level tests. A hand-rolled fake daemon (a `UnixListener` that does the Hello/HelloOk
//! handshake then answers canned `Response`s) lets us assert the adapter (a) handshakes as
//! `MemoryClient`, (b) maps each tool to the right `Request`, and (c) surfaces a clean error when
//! the daemon is down — WITHOUT linking the whole engine. Unix-only.
#![cfg(unix)]

use std::path::{Path, PathBuf};

use air_memory_mcp::daemon::{tool_recall, tool_remember, DaemonError};
use bossclawd_proto::types::{HitMirror, RecallSourceMirror};
use bossclawd_proto::{
    read_frame, write_frame, Hello, HelloOk, HitWire, Request, Response, Role, PROTO_VERSION,
};
use tokio::net::UnixListener;

/// A fake daemon serving ONE connection: it asserts the client handshook as `MemoryClient`, then
/// answers each request via `answer(req) -> Response`.
async fn spawn_fake_daemon(
    answer: impl Fn(Request) -> Response + Send + 'static,
) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("bossclawd.sock");
    let listener = UnixListener::bind(&sock).expect("bind fake daemon");
    tokio::spawn(async move {
        let (mut stream, _addr) = listener.accept().await.expect("accept");
        let hello: Hello =
            serde_json::from_slice(&read_frame(&mut stream).await.expect("read Hello")).unwrap();
        assert_eq!(hello.role, Role::MemoryClient, "adapter MUST handshake as MemoryClient");
        let hello_ok = HelloOk { pid: std::process::id(), proto_version: PROTO_VERSION };
        write_frame(&mut stream, &serde_json::to_vec(&hello_ok).unwrap()).await.unwrap();
        while let Ok(frame) = read_frame(&mut stream).await {
            let req: Request = serde_json::from_slice(&frame).unwrap();
            let resp = answer(req);
            if write_frame(&mut stream, &serde_json::to_vec(&resp).unwrap()).await.is_err() {
                break;
            }
        }
    });
    (dir, sock)
}

#[tokio::test]
async fn recall_tool_maps_to_recall_request_and_renders_hits() {
    let (_dir, sock) = spawn_fake_daemon(|req| match req {
        Request::Recall { onboarded: true, query, k } => {
            assert_eq!(query, "aria");
            assert_eq!(k, 8);
            Response::Recall(vec![HitWire {
                hit: HitMirror {
                    event_id: "e1".to_string(),
                    score: 0.9,
                    sources: vec![RecallSourceMirror::Vector],
                    kind: "memory".to_string(),
                },
                text: "aria novak ships rust".to_string(),
            }])
        }
        other => panic!("unexpected request: {other:?}"),
    })
    .await;

    let out = tool_recall(&sock, "aria", 8).await.expect("recall ok");
    assert!(out.contains("aria novak ships rust"), "renders the hit snippet: {out}");
}

#[tokio::test]
async fn remember_tool_maps_to_remember_request() {
    let (_dir, sock) = spawn_fake_daemon(|req| match req {
        Request::Remember { onboarded: true, text } => {
            assert_eq!(text, "note this");
            Response::Remember("01J-NEW".to_string())
        }
        other => panic!("unexpected request: {other:?}"),
    })
    .await;

    let out = tool_remember(&sock, "note this").await.expect("remember ok");
    assert!(out.contains("01J-NEW"), "confirms with the new event id: {out}");
}

#[tokio::test]
async fn not_onboarded_surfaces_a_clean_error() {
    let (_dir, sock) = spawn_fake_daemon(|_req| Response::NotOnboarded).await;
    let err = tool_recall(&sock, "x", 8).await.expect_err("NotOnboarded → Err");
    assert!(matches!(err, DaemonError::NotOnboarded), "got {err:?}");
}

#[tokio::test]
async fn daemon_down_surfaces_unavailable_never_panics() {
    // A socket path that was never bound — nobody is listening (I4).
    let dir = tempfile::tempdir().unwrap();
    let sock: PathBuf = dir.path().join("bossclawd.sock");
    let err = tool_remember(&sock, "x").await.expect_err("no daemon → Err");
    assert!(matches!(err, DaemonError::Unavailable(_)), "got {err:?}");
}

#[tokio::test]
async fn blank_remember_is_rejected_before_the_daemon() {
    // Defense in depth: the adapter refuses blank text without a daemon round-trip.
    let unbound = Path::new("/nonexistent/bossclawd.sock");
    let err = tool_remember(unbound, "   ").await.expect_err("blank → Err");
    assert!(matches!(err, DaemonError::EmptyText), "got {err:?}");
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p air-memory-mcp --test adapter`
Expected: FAIL to compile — `error[E0433]: … air_memory_mcp::daemon` (the module + a lib target do not exist yet).

- [ ] **Step 3: Give the crate a lib target so tests can reach `daemon`**

Add to `crates/air-memory-mcp/Cargo.toml` above `[[bin]]`:

```toml
[lib]
name = "air_memory_mcp"
path = "src/lib.rs"
```

- [ ] **Step 4: Create `src/lib.rs`**

Create `crates/air-memory-mcp/src/lib.rs`:

```rust
//! `air-memory-mcp` library surface: the daemon client (`daemon`) and the MCP JSON-RPC handler
//! (`mcp`). The bin (`main.rs`) is a thin stdio loop over these. Split into a lib so integration
//! tests can drive `daemon`/`mcp` directly.

pub mod daemon;
pub mod mcp;
```

- [ ] **Step 5: Implement `daemon.rs`**

Create `crates/air-memory-mcp/src/daemon.rs`:

```rust
//! The adapter's thin socket client over `bossclawd-proto`. Reuses proto's frame codec + handshake
//! + `Request`/`Response` verbatim (I5) — it never reimplements the wire protocol. Connects fresh
//! per tool call (MCP calls are infrequent; a fresh connect sidesteps the non-cancellation-safe
//! codec's mid-frame-reuse hazard entirely), handshakes as [`Role::MemoryClient`], sends one
//! request, reads one response. Every failure maps to a [`DaemonError`] the MCP layer renders as a
//! clean tool error (I4) — never a panic.

use std::path::{Path, PathBuf};
use std::time::Duration;

use bossclawd_proto::{
    read_frame, write_frame, Hello, HelloOk, OpErrorKindWire, Request, Response, Role, PROTO_VERSION,
};
use tokio::net::UnixStream;

/// Env override for the socket path (mirrors the daemon's `BOSSCLAWD_SOCKET`).
const ENV_SOCKET: &str = "BOSSCLAWD_SOCKET";
/// Env override for the data dir (mirrors the daemon's `BOSSCLAWD_DATA_DIR`).
const ENV_DATA_DIR: &str = "BOSSCLAWD_DATA_DIR";
/// The socket file under the data dir (mirrors the daemon's `SOCKET_FILE`).
const SOCKET_FILE: &str = "bossclawd.sock";
/// The AIR Agent bundle dir name (mirrors the daemon's `APP_DIR_NAME`).
const APP_DIR_NAME: &str = "ai.air-agent.desktop";
/// Per-call connect + round-trip bound. Generous, but guarantees a wedged/absent daemon can never
/// hang a tool call.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// A daemon-call failure, rendered by the MCP layer as a clean tool error (never a panic).
#[derive(Debug)]
pub enum DaemonError {
    /// The daemon socket is down / unreachable (connect refused, I/O error, timeout, bad handshake).
    Unavailable(String),
    /// The brain is not set up yet (the daemon recomputed onboarding for our guest role).
    NotOnboarded,
    /// Blank `remember` text, rejected before the daemon round-trip (defense in depth).
    EmptyText,
    /// A tool argument was missing or the wrong type.
    InvalidArgs(String),
    /// A typed engine error crossed the wire (`Response::Err`).
    Wire(String),
    /// An unexpected response variant (protocol drift) or a `Busy` signal.
    Protocol(String),
}

impl DaemonError {
    /// A single, user-facing sentence for the coding agent (surfaced as an `isError` tool result).
    pub fn user_message(&self) -> String {
        match self {
            DaemonError::Unavailable(_) => {
                "AIR memory service is unavailable — is AIR Agent (the bossclawd daemon) running?"
                    .to_string()
            }
            DaemonError::NotOnboarded => {
                "AIR Agent isn't set up yet — open the AIR Agent app and complete onboarding first."
                    .to_string()
            }
            DaemonError::EmptyText => "Cannot remember empty or blank text.".to_string(),
            DaemonError::InvalidArgs(m) => format!("Invalid tool arguments: {m}"),
            DaemonError::Wire(m) => format!("AIR memory error: {m}"),
            DaemonError::Protocol(m) => format!("Unexpected AIR memory response: {m}"),
        }
    }
}

/// Resolve the daemon socket path exactly as the daemon does: `BOSSCLAWD_SOCKET` if set, else
/// `<data_dir>/bossclawd.sock`, where `data_dir` is `BOSSCLAWD_DATA_DIR` if set, else the platform
/// app-data dir for the AIR Agent bundle id. A hand-wired `.mcp.json` typically sets
/// `BOSSCLAWD_SOCKET` explicitly (see the crate README).
pub fn resolve_socket_path() -> PathBuf {
    if let Some(p) = std::env::var_os(ENV_SOCKET) {
        return PathBuf::from(p);
    }
    data_dir().join(SOCKET_FILE)
}

/// The platform data dir (mirrors the daemon's `resolve_data_dir`/`app_data_dir`). Falls back to
/// the current dir only if `HOME` is unset (a degraded environment).
fn data_dir() -> PathBuf {
    if let Some(d) = std::env::var_os(ENV_DATA_DIR) {
        return PathBuf::from(d);
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join("Library/Application Support").join(APP_DIR_NAME);
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
            return PathBuf::from(xdg).join(APP_DIR_NAME);
        }
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(".local/share").join(APP_DIR_NAME);
        }
    }
    PathBuf::from(".")
}

/// Open a fresh connection, handshake as [`Role::MemoryClient`], send `req`, read the `Response`.
/// Bounded by [`CALL_TIMEOUT`]; any failure is a [`DaemonError`] (never a panic).
pub async fn call_daemon(sock: &Path, req: Request) -> Result<Response, DaemonError> {
    let exchange = async {
        let mut stream = UnixStream::connect(sock)
            .await
            .map_err(|e| DaemonError::Unavailable(format!("connect failed: {e}")))?;
        let hello = Hello { proto_version: PROTO_VERSION, role: Role::MemoryClient };
        let hello_bytes = serde_json::to_vec(&hello)
            .map_err(|e| DaemonError::Protocol(format!("encode Hello: {e}")))?;
        write_frame(&mut stream, &hello_bytes)
            .await
            .map_err(|e| DaemonError::Unavailable(format!("handshake write: {e}")))?;
        let reply = read_frame(&mut stream)
            .await
            .map_err(|e| DaemonError::Unavailable(format!("handshake read: {e}")))?;
        let hello_ok: HelloOk = serde_json::from_slice(&reply)
            .map_err(|_| DaemonError::Unavailable("bad handshake reply".to_string()))?;
        if hello_ok.proto_version != PROTO_VERSION {
            return Err(DaemonError::Unavailable(format!(
                "protocol version mismatch: adapter {PROTO_VERSION}, daemon {}",
                hello_ok.proto_version
            )));
        }
        let req_bytes = serde_json::to_vec(&req)
            .map_err(|e| DaemonError::Protocol(format!("encode request: {e}")))?;
        write_frame(&mut stream, &req_bytes)
            .await
            .map_err(|e| DaemonError::Unavailable(format!("request write: {e}")))?;
        let frame = read_frame(&mut stream)
            .await
            .map_err(|e| DaemonError::Unavailable(format!("response read: {e}")))?;
        serde_json::from_slice::<Response>(&frame)
            .map_err(|e| DaemonError::Protocol(format!("decode response: {e}")))
    };
    tokio::time::timeout(CALL_TIMEOUT, exchange)
        .await
        .map_err(|_| DaemonError::Unavailable("daemon call timed out".to_string()))?
}

/// Map a non-success `Response` (shared by both tools) to a `DaemonError`.
fn map_error_response(resp: Response) -> DaemonError {
    match resp {
        Response::NotOnboarded => DaemonError::NotOnboarded,
        Response::Busy(op) => DaemonError::Protocol(format!("memory service busy: {op}")),
        Response::Err { kind, message } => match kind {
            OpErrorKindWire::NotPermitted => {
                // Unreachable via the 2-tool surface (defense in depth); surface it plainly.
                DaemonError::Wire("operation not permitted".to_string())
            }
            _ => DaemonError::Wire(message),
        },
        other => DaemonError::Protocol(format!("unexpected response: {other:?}")),
    }
}

/// The `recall` tool: send `Request::Recall`, render the hits as a readable text block.
pub async fn tool_recall(sock: &Path, query: &str, k: usize) -> Result<String, DaemonError> {
    match call_daemon(sock, Request::Recall { onboarded: true, query: query.to_string(), k }).await? {
        Response::Recall(hits) => Ok(render_hits(query, &hits)),
        other => Err(map_error_response(other)),
    }
}

/// The `remember` tool: reject blank text, else send `Request::Remember` and confirm with the id.
pub async fn tool_remember(sock: &Path, text: &str) -> Result<String, DaemonError> {
    if text.trim().is_empty() {
        return Err(DaemonError::EmptyText);
    }
    match call_daemon(sock, Request::Remember { onboarded: true, text: text.to_string() }).await? {
        Response::Remember(id) => Ok(format!("Remembered. (event {id})")),
        other => Err(map_error_response(other)),
    }
}

/// Render recall hits as a compact, agent-readable text block.
fn render_hits(query: &str, hits: &[bossclawd_proto::HitWire]) -> String {
    if hits.is_empty() {
        return format!("No memories found for \"{query}\".");
    }
    let mut out = format!("{} memory result(s) for \"{query}\":\n", hits.len());
    for (i, h) in hits.iter().enumerate() {
        out.push_str(&format!(
            "{}. [{}] (score {:.3}) {}\n",
            i + 1,
            h.hit.kind,
            h.hit.score,
            h.text.trim()
        ));
    }
    out
}
```

- [ ] **Step 6: Wire the module into `main.rs` (still a stub run)**

Replace `crates/air-memory-mcp/src/main.rs` with:

```rust
use air_memory_mcp::daemon;

fn main() {
    // Real stdio loop lands in Task C3; this keeps the bin compiling against the lib.
    let _ = daemon::resolve_socket_path();
    eprintln!("air-memory-mcp: stub (loop lands in C3)");
}
```

- [ ] **Step 7: Run the adapter tests to verify they pass**

Run: `cargo test -p air-memory-mcp --test adapter`
Expected: all five tests PASS.

- [ ] **Step 8: Clippy the crate**

Run: `cargo clippy -p air-memory-mcp --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 9: Commit**

```bash
git add crates/air-memory-mcp/Cargo.toml crates/air-memory-mcp/src/lib.rs crates/air-memory-mcp/src/daemon.rs crates/air-memory-mcp/src/main.rs crates/air-memory-mcp/tests/adapter.rs
git commit -m "feat(air-memory-mcp): MemoryClient socket client + recall/remember tools (U3)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

### Task C3: the MCP JSON-RPC stdio handler (`mcp.rs`)

**Files:**
- Create: `crates/air-memory-mcp/src/mcp.rs`.
- Modify: `crates/air-memory-mcp/src/main.rs` (the real stdio loop).
- Modify: `crates/air-memory-mcp/tests/adapter.rs` (add MCP-layer tests).

- [ ] **Step 1: Write the failing MCP-layer tests**

Append to `crates/air-memory-mcp/tests/adapter.rs`:

```rust
use air_memory_mcp::mcp::handle_message;

/// Parse one JSON-RPC response line back to a `serde_json::Value`.
fn parse(line: &str) -> serde_json::Value {
    serde_json::from_str(line).expect("valid JSON-RPC response")
}

#[tokio::test]
async fn initialize_returns_capabilities_and_server_info() {
    // No daemon needed: initialize is answered locally.
    let sock = Path::new("/unused.sock");
    let req = r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"c","version":"1"}}}"#;
    let resp = parse(&handle_message(sock, req).await.expect("initialize replies"));
    assert_eq!(resp["id"], 0);
    assert_eq!(resp["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(resp["result"]["serverInfo"]["name"], "air-memory-mcp");
    assert!(resp["result"]["capabilities"]["tools"].is_object());
}

#[tokio::test]
async fn initialized_notification_gets_no_reply() {
    let sock = Path::new("/unused.sock");
    let note = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
    assert!(handle_message(sock, note).await.is_none(), "a notification gets no response");
}

#[tokio::test]
async fn tools_list_advertises_recall_and_remember() {
    let sock = Path::new("/unused.sock");
    let req = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
    let resp = parse(&handle_message(sock, req).await.expect("tools/list replies"));
    let names: Vec<String> = resp["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, vec!["recall".to_string(), "remember".to_string()]);
    // Each tool carries an object inputSchema with the required fields.
    let recall = &resp["result"]["tools"][0];
    assert_eq!(recall["inputSchema"]["required"][0], "query");
}

#[tokio::test]
async fn tools_call_recall_routes_to_the_daemon() {
    let (_dir, sock) = spawn_fake_daemon(|req| match req {
        Request::Recall { query, .. } => Response::Recall(vec![HitWire {
            hit: HitMirror {
                event_id: "e".into(),
                score: 0.5,
                sources: vec![RecallSourceMirror::Vector],
                kind: "memory".into(),
            },
            text: format!("hit for {query}"),
        }]),
        other => panic!("unexpected: {other:?}"),
    })
    .await;
    let req = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"recall","arguments":{"query":"ferris"}}}"#;
    let resp = parse(&handle_message(&sock, req).await.expect("tools/call replies"));
    assert_eq!(resp["id"], 2);
    assert_eq!(resp["result"]["isError"], false);
    assert!(resp["result"]["content"][0]["text"].as_str().unwrap().contains("hit for ferris"));
}

#[tokio::test]
async fn tools_call_daemon_down_is_an_iserror_result_not_a_crash() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("bossclawd.sock"); // never bound (I4)
    let req = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"remember","arguments":{"text":"hi"}}}"#;
    let resp = parse(&handle_message(&sock, req).await.expect("tools/call replies"));
    assert_eq!(resp["result"]["isError"], true, "daemon-down is a clean tool error");
    assert!(resp["result"]["content"][0]["text"].as_str().unwrap().contains("unavailable"));
}

#[tokio::test]
async fn unknown_method_is_method_not_found() {
    let sock = Path::new("/unused.sock");
    let req = r#"{"jsonrpc":"2.0","id":4,"method":"resources/list"}"#;
    let resp = parse(&handle_message(sock, req).await.expect("replies"));
    assert_eq!(resp["error"]["code"], -32601);
}

#[tokio::test]
async fn parse_error_is_minus_32700() {
    let sock = Path::new("/unused.sock");
    let resp = parse(&handle_message(sock, "not json at all").await.expect("replies"));
    assert_eq!(resp["error"]["code"], -32700);
}

#[tokio::test]
async fn unknown_tool_is_invalid_params() {
    let sock = Path::new("/unused.sock");
    let req = r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"delete_everything","arguments":{}}}"#;
    let resp = parse(&handle_message(sock, req).await.expect("replies"));
    assert_eq!(resp["error"]["code"], -32602);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p air-memory-mcp --test adapter unknown_method_is_method_not_found`
Expected: FAIL to compile — `error[E0432]: unresolved import `air_memory_mcp::mcp::handle_message``.

- [ ] **Step 3: Implement `mcp.rs`**

Create `crates/air-memory-mcp/src/mcp.rs`:

```rust
//! The hand-rolled MCP-over-stdio JSON-RPC 2.0 message handler. The MCP surface is exactly two
//! tools (`recall`, `remember`) plus the lifecycle methods `initialize` / `notifications/*` /
//! `tools/list`, so a full SDK is unnecessary (see the plan's "Open questions resolved"). Every
//! message is one JSON object on one line; [`handle_message`] returns `Some(response_line)` for a
//! request, or `None` for a notification (no `id`) — the loop writes `Some` to stdout and drops
//! `None`. Tool failures are `isError: true` tool results (never a JSON-RPC error), so the agent
//! sees a clean message instead of a broken session (I4).

use std::path::Path;

use serde_json::{json, Value};

use crate::daemon::{self, DaemonError};

/// Server identity reported in `initialize`.
pub const SERVER_NAME: &str = "air-memory-mcp";
/// Server version reported in `initialize`.
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
/// The MCP protocol revision we default to when the client omits one (we echo the client's if given).
pub const DEFAULT_PROTOCOL_VERSION: &str = "2025-06-18";
/// The two tool names.
pub const TOOL_RECALL: &str = "recall";
pub const TOOL_REMEMBER: &str = "remember";
/// Default `k` for `recall` when the caller omits it.
pub const DEFAULT_RECALL_K: usize = 8;

/// Handle one JSON-RPC line. `Some(line)` is the response to write to stdout; `None` means the
/// message was a notification (no `id`) that takes no reply. Never panics.
pub async fn handle_message(sock: &Path, line: &str) -> Option<String> {
    let msg: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return Some(error_line(Value::Null, -32700, "Parse error")),
    };
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or_default();

    match method {
        "initialize" => {
            let version = msg
                .pointer("/params/protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(DEFAULT_PROTOCOL_VERSION)
                .to_string();
            Some(result_line(
                id.unwrap_or(Value::Null),
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION }
                }),
            ))
        }
        "tools/list" => Some(result_line(id.unwrap_or(Value::Null), tools_list_result())),
        "tools/call" => {
            let id = id.unwrap_or(Value::Null);
            let name = msg.pointer("/params/name").and_then(Value::as_str).unwrap_or_default();
            let args = msg.pointer("/params/arguments").cloned().unwrap_or(Value::Null);
            match name {
                TOOL_RECALL => Some(tool_result_line(id, run_recall(sock, &args).await)),
                TOOL_REMEMBER => Some(tool_result_line(id, run_remember(sock, &args).await)),
                other => Some(error_line(id, -32602, &format!("unknown tool: {other}"))),
            }
        }
        // Any notification (no `id`), including `notifications/initialized`, takes no reply.
        _ if id.is_none() => None,
        // A request with an unrecognized method.
        _ => Some(error_line(id.unwrap_or(Value::Null), -32601, &format!("method not found: {method}"))),
    }
}

/// The `tools/list` result: exactly `recall` + `remember`, each with a JSON-Schema `inputSchema`.
fn tools_list_result() -> Value {
    json!({
        "tools": [
            {
                "name": TOOL_RECALL,
                "description": "Search the user's AIR memory for notes relevant to a query. Returns \
                                ranked snippets. Use this before answering to recall prior context.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "What to search memory for." },
                        "k": {
                            "type": "integer",
                            "description": "Max results (default 8).",
                            "minimum": 1
                        }
                    },
                    "required": ["query"]
                }
            },
            {
                "name": TOOL_REMEMBER,
                "description": "Save a new note to the user's AIR memory so it can be recalled later. \
                                Stored as external (untrusted) content: recallable, never auto-applied.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "text": { "type": "string", "description": "The note to remember." }
                    },
                    "required": ["text"]
                }
            }
        ]
    })
}

/// Parse+run the `recall` tool.
async fn run_recall(sock: &Path, args: &Value) -> Result<String, DaemonError> {
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .ok_or_else(|| DaemonError::InvalidArgs("`query` (string) is required".to_string()))?;
    let k = match args.get("k") {
        None | Some(Value::Null) => DEFAULT_RECALL_K,
        Some(v) => v
            .as_u64()
            .filter(|n| *n >= 1)
            .map(|n| n as usize)
            .ok_or_else(|| DaemonError::InvalidArgs("`k` must be a positive integer".to_string()))?,
    };
    daemon::tool_recall(sock, query, k).await
}

/// Parse+run the `remember` tool.
async fn run_remember(sock: &Path, args: &Value) -> Result<String, DaemonError> {
    let text = args
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(|| DaemonError::InvalidArgs("`text` (string) is required".to_string()))?;
    daemon::tool_remember(sock, text).await
}

/// A JSON-RPC success response line.
fn result_line(id: Value, result: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

/// A JSON-RPC error response line.
fn error_line(id: Value, code: i64, message: &str) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }).to_string()
}

/// A `tools/call` result line: success text or an `isError: true` tool result (never a JSON-RPC
/// error — a failed tool must not break the session, I4).
fn tool_result_line(id: Value, outcome: Result<String, DaemonError>) -> String {
    let (text, is_error) = match outcome {
        Ok(text) => (text, false),
        Err(e) => (e.user_message(), true),
    };
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": { "content": [{ "type": "text", "text": text }], "isError": is_error }
    })
    .to_string()
}
```

- [ ] **Step 4: Run the MCP-layer tests to verify they pass**

Run: `cargo test -p air-memory-mcp --test adapter`
Expected: all tests PASS (C2's fake-daemon tests + C3's MCP tests).

- [ ] **Step 5: Implement the real stdio loop in `main.rs`**

Replace `crates/air-memory-mcp/src/main.rs`:

```rust
//! `air-memory-mcp` — an MCP stdio server exposing `recall` + `remember` over the `bossclawd`
//! socket as a scoped `MemoryClient` (SP1). Reads newline-delimited JSON-RPC messages from stdin,
//! answers each via `mcp::handle_message`, and writes response lines to stdout. Notifications get
//! no reply. Runs on a single-thread runtime — MCP requests are serial.

use air_memory_mcp::{daemon, mcp};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    let sock = daemon::resolve_socket_path();
    eprintln!("air-memory-mcp: using daemon socket {}", sock.display());

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = mcp::handle_message(&sock, &line).await {
            stdout.write_all(response.as_bytes()).await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
        }
    }
    Ok(())
}
```

- [ ] **Step 6: Verify the whole crate builds + lints + tests**

Run: `cargo build -p air-memory-mcp && cargo clippy -p air-memory-mcp --all-targets -- -D warnings && cargo test -p air-memory-mcp`
Expected: all green, no warnings.

- [ ] **Step 7: Commit**

```bash
git add crates/air-memory-mcp/src/mcp.rs crates/air-memory-mcp/src/main.rs crates/air-memory-mcp/tests/adapter.rs
git commit -m "feat(air-memory-mcp): hand-rolled MCP JSON-RPC stdio handler + loop (U3)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Phase D — U4: end-to-end proof + manual wiring

### Task D1: the real-daemon MemoryClient loop test

**Files:**
- Create: `crates/bossclawd/tests/memory_client_loop.rs`.

This proves U1+U2 together over a **real** socket as a **MemoryClient**: recall works, remember→recall round-trips, a destructive op is refused. (The authz matrix in `authz.rs` covers refusals per-op; this file is the spec §9 "the real boundary" loop proof.)

- [ ] **Step 1: Write the failing loop test**

Create `crates/bossclawd/tests/memory_client_loop.rs`:

```rust
//! U4 — the safe read+write loop over the REAL daemon socket, driven as a `MemoryClient` (the same
//! role the `air-memory-mcp` adapter uses). Proves: (1) recall works, (2) remember→recall
//! round-trips, (3) a destructive op is refused. Hermetic engine, onboarded fixture. Unix-only.
#![cfg(unix)]

use std::path::PathBuf;

use bossclawd::server;
use bossclawd_proto::{
    read_frame, write_frame, Hello, HelloOk, OpErrorKindWire, Request, Response, Role, PROTO_VERSION,
};
use tokio::net::UnixStream;

struct Guest {
    stream: UnixStream,
}

impl Guest {
    async fn connect(sock: &std::path::Path) -> Self {
        let mut stream = UnixStream::connect(sock).await.expect("connect");
        let hello = Hello { proto_version: PROTO_VERSION, role: Role::MemoryClient };
        write_frame(&mut stream, &serde_json::to_vec(&hello).unwrap()).await.unwrap();
        let hello_ok: HelloOk =
            serde_json::from_slice(&read_frame(&mut stream).await.unwrap()).unwrap();
        assert_eq!(hello_ok.proto_version, PROTO_VERSION);
        Self { stream }
    }
    async fn call(&mut self, req: Request) -> Response {
        write_frame(&mut self.stream, &serde_json::to_vec(&req).unwrap()).await.unwrap();
        serde_json::from_slice(&read_frame(&mut self.stream).await.unwrap()).unwrap()
    }
}

async fn spawn_onboarded_daemon() -> (tempfile::TempDir, PathBuf) {
    bossclawd::vault::seed_secret_cache_for_test(Default::default());
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().to_path_buf();
    std::fs::write(
        home.join("identity.json"),
        serde_json::json!({
            "did": "did:wba:example.com:tester",
            "name": "Tester",
            "created_at": "2026-07-09T00:00:00+00:00"
        })
        .to_string(),
    )
    .unwrap();
    let sock = home.join("bossclawd.sock");
    server::spawn_for_test(sock.clone(), home).await;
    (dir, sock)
}

#[tokio::test]
async fn memory_client_full_loop() {
    let (_dir, sock) = spawn_onboarded_daemon().await;
    let mut guest = Guest::connect(&sock).await;

    // (1) Recall on an empty brain is a clean empty result (not an error).
    match guest.call(Request::Recall { onboarded: true, query: "anything".into(), k: 5 }).await {
        Response::Recall(hits) => assert!(hits.is_empty(), "empty brain recalls nothing"),
        other => panic!("expected Recall, got {other:?}"),
    }

    // (2) Remember → the next recall surfaces it.
    let id = match guest.call(Request::Remember { onboarded: true, text: "kwang ships air".into() }).await {
        Response::Remember(id) => id,
        other => panic!("expected Remember, got {other:?}"),
    };
    match guest.call(Request::Recall { onboarded: true, query: "kwang air".into(), k: 5 }).await {
        Response::Recall(hits) => {
            assert!(hits.iter().any(|h| h.hit.event_id == id && h.text.contains("kwang ships air")));
        }
        other => panic!("expected Recall, got {other:?}"),
    }

    // (3) A destructive op is refused for the guest role.
    assert!(
        matches!(
            guest.call(Request::Teardown).await,
            Response::Err { kind: OpErrorKindWire::NotPermitted, .. }
        ),
        "Teardown is refused for MemoryClient"
    );
}
```

- [ ] **Step 2: Run it**

Run: `cargo test -p bossclawd --test memory_client_loop`
Expected: PASS (all requirements already implemented in Phases A+B — this is the consolidated proof).

- [ ] **Step 3: Commit**

```bash
git add crates/bossclawd/tests/memory_client_loop.rs
git commit -m "test(bossclawd): end-to-end MemoryClient loop over the socket (U4)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

### Task D2: the `.mcp.json` wiring snippet + adapter README

**Files:**
- Create: `crates/air-memory-mcp/README.md`.

- [ ] **Step 1: Write the README with the manual wiring snippet**

Create `crates/air-memory-mcp/README.md`:

```markdown
# air-memory-mcp

An MCP (Model Context Protocol) stdio server that gives a coding agent two tools backed by your
AIR Agent memory:

- **`recall(query, k?)`** — search your AIR memory for relevant notes.
- **`remember(text)`** — save a new note (stored as external/untrusted: recallable, never
  auto-applied).

It talks to the local `bossclawd` daemon (the AIR Agent memory engine) over its Unix socket as a
scoped **MemoryClient** — the daemon refuses it every other operation (no teardown, no cloud
enable, no folder grants), enforced daemon-side.

## Build

```bash
cargo build --release -p air-memory-mcp
# binary: target/release/air-memory-mcp
```

## Wire it into Claude Code (manual, SP1)

Add to your project's `.mcp.json` (or Claude Code's MCP config). AIR Agent must be installed and
onboarded (its `bossclawd` daemon running).

```json
{
  "mcpServers": {
    "air-memory": {
      "command": "/absolute/path/to/target/release/air-memory-mcp",
      "env": {
        "BOSSCLAWD_SOCKET": "/Users/you/Library/Application Support/ai.air-agent.desktop/bossclawd.sock"
      }
    }
  }
}
```

- If `BOSSCLAWD_SOCKET` is omitted, the adapter resolves the same default path the daemon uses:
  macOS `~/Library/Application Support/ai.air-agent.desktop/bossclawd.sock`, Linux
  `$XDG_DATA_HOME`/`~/.local/share`/`ai.air-agent.desktop/bossclawd.sock`.
- If AIR Agent isn't running, the tools return a clean "memory service unavailable" message (they
  never crash the session).

## Security

The adapter connects as `MemoryClient`; the daemon enforces a fail-closed allowlist (`recall`,
`remember` only) — a compromised or buggy adapter still cannot reach any destructive/egress op.
This is the "Simple" bar (a cooperative client is scoped); it does not defend against a *malicious*
same-uid process (which could already connect today). Cryptographic capability tokens are a future
hardening.
```

- [ ] **Step 2: Commit**

```bash
git add crates/air-memory-mcp/README.md
git commit -m "docs(air-memory-mcp): .mcp.json wiring snippet + usage (U4)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

### Task D3: final whole-workspace verification gate

**Files:** none (verification only).

- [ ] **Step 1: Build the whole workspace**

Run: `cargo build --workspace`
Expected: every crate compiles (`Finished`).

- [ ] **Step 2: Clippy the whole workspace, deny warnings**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 3: Run the full test suite for the touched crates**

Run: `cargo test -p bossclaw-core -p bossclawd-proto -p bossclawd -p air-memory-mcp -p memharness`
Expected: all green (incl. `authz`, `roundtrip`, `invariants`, `memory_client_loop`, `adapter`).

- [ ] **Step 4: Confirm the app still builds (I3 — unchanged behavior)**

Run: `cargo build -p air_agent_desktop`
Expected: compiles (its only change was `role: Role::App` at the two `Hello` sites + the `NotPermitted` arm).

- [ ] **Step 5: Manual smoke of the adapter over stdio (optional but recommended)**

With AIR Agent onboarded + running (or a dev daemon on a known socket), run the adapter and pipe an
`initialize` + `tools/list` by hand:

```bash
printf '%s\n%s\n' \
  '{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"smoke","version":"1"}}}' \
  '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' \
  | BOSSCLAWD_SOCKET="$HOME/Library/Application Support/ai.air-agent.desktop/bossclawd.sock" \
    ./target/debug/air-memory-mcp
```

Expected: two JSON-RPC response lines — the `initialize` result (serverInfo `air-memory-mcp`) and a
`tools/list` result advertising `recall` + `remember`.

- [ ] **Step 6: No commit** (verification only). If any gate failed, fix and re-run before declaring SP1 done.

---

## Spec coverage

| Spec item | Task(s) |
|---|---|
| **U1** — `remember` write op (core fn + `Request::Remember` + engine wrapper) | A1 (core `EventLog::remember`), A2 (proto `Request::Remember`/`Response::Remember` + `EngineHandle::remember` + dispatch arm) |
| **U2** — per-op authorization (`Role` handshake + fail-closed dispatch allowlist) | B1 (`Role`, `Hello.role`, `Role::allows`, `OpErrorKindWire::NotPermitted`, dispatch enforcement, guest onboarding recompute) |
| **U3** — Rust MCP adapter (2 tools, daemon-backed, reuses proto) | C1 (crate skeleton), C2 (`daemon.rs` socket client + tools), C3 (`mcp.rs` JSON-RPC handler + stdio loop) |
| **U4** — end-to-end proof + manual wiring | D1 (real-socket MemoryClient loop), D2 (`.mcp.json` snippet + README), D3 (workspace gate + smoke) |
| **I1** — guest pass enforced at the daemon (fail-closed) | B1 (`Role::allows` deny-by-default + dispatch gate); tested in `authz.rs` (`memory_client_is_refused_destructive_ops` incl. the fail-closed `Status` case) |
| **I2** — remembered memories are external-tainted | A1 (`content["origin"] = EXTERNAL_ORIGIN`); tested in A1's core test + `memory_client_full_loop` |
| **I3** — the app is unchanged (`Role` defaults to `App`) | B1 (`#[serde(default)]` + `#[default] App` + all app-side `Hello` sites set `Role::App`); tested by `hello_role_defaults_to_app_on_missing_field`, `app_role_is_allowed_all_ops`, and D3 Step 4 |
| **I4** — fail-safe on daemon-down (clean MCP error, no crash) | C2 (`DaemonError::Unavailable`), C3 (`isError: true` tool result); tested by `daemon_down_surfaces_unavailable_never_panics` + `tools_call_daemon_down_is_an_iserror_result_not_a_crash` |
| **I5** — single-source the wire protocol (link proto, no re-impl) | C1 (dep on `bossclawd-proto` only), C2 (`call_daemon` reuses `read_frame`/`write_frame`/`Hello`/`Request`/`Response` verbatim) |
| Error table: daemon down | C2/C3 (`Unavailable` → isError) |
| Error table: `MemoryClient` calls non-allowlisted op | B1 (`NotPermitted`); `map_error_response` surfaces it in the adapter |
| Error table: empty/blank `remember` | A1 (core reject) + A2 (`Rejected` wire) + C2 (`EmptyText` pre-check); tested at all three layers |
| Error table: not onboarded | B1 (guest onboarding recompute → `NotOnboarded`); tested by `memory_client_cannot_force_onboarding` + adapter `not_onboarded_surfaces_a_clean_error` |
| Error table: malformed MCP request | C3 (`-32700`/`-32601`/`-32602`); tested by `parse_error_*`, `unknown_method_*`, `unknown_tool_*` |
| Testing strategy: unit (core/daemon) | A1, B1 (proto `Role` unit tests), authz matrix |
| Testing strategy: integration (real boundary) | D1 (`memory_client_loop.rs`) + A2 (`roundtrip.rs`) |
| Testing strategy: adapter-level (tools map + daemon-down) | C2, C3 (`adapter.rs`) |
| Gates (`build`/`clippy -D warnings`/`test`) | D3 |

---

## Open questions resolved

**Q1 — Rust MCP-over-stdio: `rmcp` SDK vs a hand-rolled JSON-RPC loop → DECISION: hand-rolled.**
`cargo search rmcp` confirms the official `rmcp = "2.1.0"` exists and is maintained. But the MCP surface here is tiny — two tools plus `initialize`/`notifications/*`/`tools/list` — and this is a **distributed binary** where minimizing the dependency/attack surface and keeping full control of the exact JSON on the wire matters more than SDK ergonomics. A hand-rolled loop adds **zero** new third-party crates (only `serde_json` + `tokio`, already in the tree), is ~150 lines fully under test, and cannot drift with an SDK's macro/schema-generation behavior. `rmcp` would pull a large transitive tree (schemars, macro crates, its own transport stack) for no proportional benefit at this surface size. Chosen: the minimal hand-rolled JSON-RPC-2.0 stdio loop (`mcp.rs`). Version negotiation is lenient-correct: `initialize` echoes the client's `protocolVersion` when present (spec-compliant when we support it — our handling is version-agnostic), else `DEFAULT_PROTOCOL_VERSION = "2025-06-18"`.

**Q2 — `remember` input shape → DECISION: text-only (YAGNI).** `Request::Remember { onboarded, text }`. No `source`/`tag` metadata: nothing in SP1 needs it, and SP3 (auto-capture) can add fields later behind serde defaults without a wire break. The external-taint stamp is applied unconditionally in core (`content["origin"] = "external"`), not passed by the client.

**Q3 — where the adapter's socket-client code lives → DECISION: a thin adapter-local client over `bossclawd-proto` (no shared-crate extraction).** The desktop `EngineClient`/`Transport` are tightly coupled to desktop-only types (`EngineOpError`, `EngineError`, `HitWithText`, the whole Family-2 wire→desktop conversion surface) the adapter does not need; extracting them into a shared crate is a large refactor touching the entire desktop error surface for a **2-op** client. Instead the adapter's `call_daemon` reuses proto's frame codec (`read_frame`/`write_frame`), handshake (`Hello`/`HelloOk`), and `Request`/`Response` **verbatim** — the security-sensitive wire format is single-sourced in proto (I5 satisfied), only the trivial connect-handshake-one-request glue is adapter-local. Accepted trade-off: linking proto transitively pulls `bossclaw-core` into the adapter binary (proto's mirror `From` impls require it); this is strictly better than reimplementing the wire types in a second crate/language and risking drift.

**Q4 — the `Role` wire representation + how the app opts into `App` → DECISION: an enum field on `Hello` with `#[serde(default)] role: Role` and `#[default] App`.** Wire back-compat: a peer that omits `role` (any build predating the field) deserializes as `App`, so no migration hazard; the app is behaviorally unchanged (I3). In-tree, every `Hello` construction site is updated to an explicit `Role::App` (enumerated in "Verified current anchors") — mechanical and compile-forced, each an explicit full-access request. `MemoryClient` is requested only by the adapter's `call_daemon`.

**Q5 — the `NotPermitted` typed error → DECISION: a new `OpErrorKindWire::NotPermitted` arm (not a new `Response` variant).** It extends the existing typed-error surface (spec §11.5), rides the existing `Response::Err { kind, message }` shape, and needs only: the proto arm + round-trip-array entry, the daemon's `not_permitted_response`, and the compile-forced desktop `op_error_from_wire` arm (`→ EngineOpError::Core`, unreachable for the always-`App` app). No new `Response` variant keeps the blast radius minimal.

**Bonus decision (beyond the spec's open questions) — the daemon recomputes onboarding for `MemoryClient`.** A `MemoryClient` sends `onboarded: true` unconditionally, but the daemon **ignores** that flag for the guest role and substitutes its own `identity::is_onboarded(<data_dir>)` check (`override_onboarding_for_guest` + `EngineHandle::is_onboarded_local`). This closes a real footgun: without it, an adapter connecting to a **not-yet-onboarded** machine would pass `onboarded: true` and trip `get_or_open` → `load_or_mint`, silently **minting a keystore + brain** the user never created. With it, guest recall/remember on a not-onboarded brain cleanly return `NotOnboarded` ("set up AIR Agent first"), matching the spec's error-table intent — and a guest can never force brain creation. The `App` role is unaffected (keeps its self-asserted flag), so I3 holds. Tested by `memory_client_cannot_force_onboarding`.

**Nothing remains open.** All five spec open questions are resolved above; the error table, every invariant (I1–I5), and the testing strategy each map to a task in "Spec coverage".
