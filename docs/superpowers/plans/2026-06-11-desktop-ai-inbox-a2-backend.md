# AI Inbox — Implementation Plan (Phase A2: the Rust backend stack — daemon-client + archive-reader + identity-adopter + policy-store + Tauri surface)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** v2 — critic pass returned **APPROVE-WITH-CHANGES** (no Critical findings; the critic compiled+ran the serde `#[serde(other)]` enum and the rusqlite 0.32 API and round-tripped every normative fixture — all green). The headline WAL risk was **empirically retired**: two independent cross-PROCESS probes (a `mode=ro` Python reader against a live WAL writer; a separate-process rusqlite reader/writer pair, `busy_errors=0`) confirm a read-only second process reads the daemon's live WAL. All findings applied: **M1** `inbox_threads`→`inbox_conversations` grouped by `peer_did`/`room_id` per design §6 (the ported JS `threads()` grouped by `thread_id`, which §6 explicitly forbids — it would fragment 1:1 conversations) + a collapse test; **M2** the read-during-write soak is now a genuine SEPARATE-PROCESS writer (was two same-process connections) + a fast same-process sanity check retained; **M3** a daemon-offline signal — `InboxEvent::Offline` on first/continued attach failure, forwarded as `inbox_offline`, with a no-listener test (JS `connectDaemonPersistent` rejects `DAEMON_DOWN`; design §5/§8 require the "daemon offline" banner); plus minors — stray `tests/..` git-add token removed, dead+unbounded `pending_sends` dropped, async archive reads moved to `spawn_blocking` (the bounded-retry `std::thread::sleep` must not block a tokio worker), the dead viewer `inbox_gap` arm removed (viewers never receive `gap`, PROTOCOL §5), and the `parse_mute_set` import smell deleted (not aliased). Executor disambiguation note added (`src/inbox/` manager vs `src/commands/inbox.rs`).

**Status:** v1 — initial plan against the verified contract (the daemon-phase rhythm).

**Goal:** Build the Rust client half of the desktop AI inbox: a reconnecting two-role daemon-socket client, a read-only WAL archive reader, an identity-adopter, and a policy store — all in `crates/air-rs` behind an `inbox` feature, plus the thin Tauri command/event surface in `apps/desktop/src-tauri` that the (later) A3 React inbox UI will call. Green on its own against the normative cross-language fixtures.

**Architecture:** `crates/air-rs` gains an `inbox` module (Tauri-agnostic library): wire-frame serde types asserted byte-for-byte against `agent-bridge-mcp/test/fixtures/socket-frames.json`; a newline-delimited line parser (1 MiB ceiling); read-only stores for `contacts.json` / `blocklist.json` / `identity.json` metadata; the `channelGate`; a read-only WAL `archive_reader` (rusqlite, short-lived statements, bounded busy-retry); a `replayer` that ports `makeReplayer` (paginate → blocklist-skip → dedupe → current-pin `rowToMessage` → gate); an `identity_adopter` (collision-is-the-norm); and a `policy_store`. `apps/desktop/src-tauri` adds an `InboxManager` to `AppState`, a long-lived spawned task that drives the **viewer** connection and emits Tauri events, and commands for send / history / identity / policy. **The channel connection + replayer are built and fully tested in the library (the high-risk parity surface, de-risked first) but their Tauri wiring + AI consumption are Phase B** — A2's Tauri surface exposes only what the A3 inbox UI needs (live viewer feed, send+ack, history, adopted identity, the dial store).

**Tech Stack:** Rust 2021. New deps: `rusqlite` (bundled SQLite, read-only WAL); existing `tokio` extended with `net` + `io-util` for `UnixStream`. `serde`/`serde_json`/`uuid`/`thiserror`/`chrono` already present. No regex crate (hand-scan `AIR-…`). POSIX-only v1 (HOME-based home resolution).

**Repo rules that bind every task:**
- Work from `~/air-note`. Branch `feat/ai-inbox-a2-backend` from current `main` (`d270e25`).
- The Rust suite is **hermetic** like the JS one: every test that touches a home points `AGENT_BRIDGE_HOME` at a `tempfile::TempDir`; **never** read the real `~/.air-msg`. Add `tempfile` to `[dev-dependencies]`.
- Parsers MUST tolerate unknown fields (no `#[serde(deny_unknown_fields)]`) and unknown frame **types** (`#[serde(other)] Unknown`) — PROTOCOL.md §8.
- Conditional `message` fields (`contact`, `key_changed`, `thread_id`, `body`) are `Option<…>` with `#[serde(default, skip_serializing_if = "Option::is_none")]` — the encoder reproduces the daemon's omit-when-falsy, the decoder tolerates absence — PROTOCOL.md §5.
- Run a single Rust test file with `cargo test -p air-rs --test <name>`; the whole crate with `cargo test -p air-rs`. The desktop backend compiles with `cargo check -p bossclaw_desktop`.
- DRY/YAGNI/TDD/frequent commits. When prose and the fixture disagree, **the fixture wins** (PROTOCOL.md preamble).

**Wire facts this plan relies on (verified 2026-06-11 against `air-note` main `d270e25`):**

*Normative contract (the source of truth A2 builds against):*
- `agent-bridge-mcp/docs/PROTOCOL.md` (frame catalog, the 5 replay invariants, camelCase-on-the-wire `status.clients[]`, optional-when-falsy, unknown-tolerance, send taxonomy, 1 MiB ceiling, 3 s client handshake, 5 s pre-hello reaper).
- `agent-bridge-mcp/test/fixtures/socket-frames.json` — `version: 1`; keys `client_to_daemon.{hello, hello_channel_resume, ping, status_request, send, send_plaintext, send_reply}` and `daemon_to_client.{hello_ok, message, gap, pong, status, send_ok, send_err, error}`. The `message` fixture is a pinned/verified/key-unchanged sender (carries `contact`, `thread_id`, `body`; carries NO `key_changed`).

*JS reference impls to port byte-faithfully (paths + line ranges):*
- `connectDaemonPersistent` (daemon-ipc.mjs:373-438): backoff `500ms → ×2 → 5000ms` cap; `maxSeen` = max `relay_seq` seen; `baseline = cursorFn()` captured BEFORE the first connect; resume `since_seq = maxSeen ?? baseline ?? undefined`; **first attach sends NO `since_seq`**; sleep BEFORE the first retry with an **unref'd** timer; stop-flag re-checked after EVERY await; `onAttach`/`onDetach` invoked OUTSIDE the connect try (a throwing callback must not look like a failed connect); a deliberate `close()` flips `stopped` first so the socket-close handler is silent (no spurious detach).
- `connectDaemon` (daemon-ipc.mjs:317-351): on `connect`, write `{type:"hello", role, ...(finite(sinceSeq)?{since_seq}:{})}`; `handshakeMs = 3000` → `fail("daemon handshake timed out")`; first frame must be `hello-ok` else `fail`; post-hello dispatch `message`→onMessage, `gap`→onGap(after_seq), `status`→onStatus; `pong` + unknown ignored.
- `makeLineParser` (daemon-ipc.mjs:50-71): accumulate; if `buf.length > MAX_FRAME (1<<20)` AND no `\n` → error + reset buf; split on `\n`; skip blank (`!line.trim()`) lines; `JSON.parse` each; parse error → `onError`, continue.
- `openArchive` (archive.mjs:50-86): `PRAGMA busy_timeout=5000` FIRST then `PRAGMA journal_mode=WAL` (both via `.get()`); schema below. **The daemon (writer) sets WAL; the A2 reader only reads.**
- Archive schema (archive.mjs:26-47 + migrations): table `messages(envelope_id TEXT, direction TEXT, thread_id TEXT, peer_did TEXT, from_did TEXT, to_did TEXT, timestamp TEXT, body_json TEXT, encrypted INT, verified INT, key_changed INT DEFAULT 0, relay_seq INT, archived_at TEXT, PRIMARY KEY(envelope_id,direction))` + later `spam INT DEFAULT 0`, `room_id TEXT`. Table `meta(key TEXT PRIMARY KEY, value TEXT)`; cursor row `key='pull_cursor'`.
- `parseRow` (archive.mjs:104-113): `{envelope_id, direction, thread_id, peer_did, from: from_did, to: to_did, timestamp, body: JSON.parse(body_json), encrypted:!!, verified:!!, key_changed:!!, spam:!!, relay_seq: relay_seq ?? undefined, room_id: room_id ?? undefined, archived_at}`.
- `replaySince` (archive.mjs:217-225): `SELECT * FROM messages WHERE direction='received' AND spam=0 AND relay_seq IS NOT NULL AND relay_seq > ? AND envelope_id NOT LIKE '%:joined' ORDER BY relay_seq ASC LIMIT ?` (default limit 500). **This SQL enforces only invariants #1–#3.**
- `getCursor` (archive.mjs:150-154): `SELECT value FROM meta WHERE key='pull_cursor'`, default 0. `archiveExists` (archive.mjs:21-23): file-exists probe, never materialize.
- `history` (archive.mjs:116-130): WHERE peer/thread/room/before, `spam=0` unless includeSpam, `ORDER BY timestamp DESC, archived_at DESC LIMIT ?`. `threads` (archive.mjs:138-147): `SELECT thread_id, peer_did, MAX(timestamp) last_timestamp, COUNT(*) count FROM messages WHERE spam=0 GROUP BY thread_id ORDER BY last_timestamp DESC`.
- `rowToMessage` (channel-replay.mjs:11-27): `{seq: relay_seq, relay_seq, from, ...(contact?.alias?{contact}:{}) , envelope_id, received_at: timestamp, verified, encrypted, ...(key_changed?{key_changed:true}:{}), ...(room_id?{room_id}:{}), body, thread_id}` where `contact = getContactByDid(row.from)`.
- `makeReplayer.gap` (channel-replay.mjs:49-67): paginate `replaySince(since, {limit: pageSize=500})`; per row `if (isBlocked(row.from)) continue` (#4); dedupe by `envelope_id` against a bounded (`maxSeen=1000`, FIFO) seen-set; `push(rowToMessage(row))` where `push` re-applies `channelGate` (#5); advance `since = rows[last].relay_seq`; stop when a short page (`< pageSize`) signals the hole's end. `live(m)`: dedupe then push.
- `channelGate` (channel.mjs:12-17): `m.verified && m.contact && !m.key_changed && !(mute.has(contact) || mute.has(from) || mute.has(shortPeer(from)))`.
- `shortPeer` (peers.mjs:4-7): `/AIR-[A-Za-z0-9-]+/` match else passthrough. `parseMuteSet` (peers.mjs:11-13): `AIRMSG_MUTE` comma-split, trim, drop empty.
- `getContactByDid` (contacts.mjs:136): `loadContacts().contacts[did] || null`; `contacts.json` = `{version, contacts: {<did>: {alias, air_id, did, name, public_key_multibase, fingerprint, …}}}`; the gate uses `contact.alias` (truthy = pinned).
- `isBlocked` (moderation.mjs:48-54): `!!loadBlocklist().blocked[did]`, **fail-open (false) on ANY error**; `blocklist.json` = `{version, blocked: {<did>: {air_id, alias, blocked_at, drop_count, last_drop_at}}}` at `{home}/blocklist.json`.
- Daemon identity file (identity.mjs:69-143): `{home}/identity.json` = `{version, name, air_id, did, seed_hex (SENSITIVE), public_key_base64url, public_key_multibase, agent_secret (SENSITIVE), relay_url, air_url, service_endpoint_published, created_at}`. **The adopter reads `did` + `name` ONLY; never `seed_hex`/`agent_secret`.** `bridgeHome` (identity.mjs:32-43): `AGENT_BRIDGE_HOME` || `~/.air-msg`.
- Send op (daemon side, already shipped in A1; the A2 client only BUILDS the `send` frame and PARSES the ack): success `{type:"send-ok", id, envelope_id, encrypted}`; failure `{type:"send-err", id, retryable, reason}` (reason ≤512, control-stripped). A `send` with no `id` gets no ack (no-id-no-ack).

*Rust crate facts (verified against `crates/air-rs`):*
- Workspace `Cargo.toml`: `members = ["crates/air-rs", "apps/desktop/src-tauri"]`, resolver 2.
- `crates/air-rs/Cargo.toml`: edition 2021, `version 0.0.1`; deps incl. `serde{derive}`, `serde_json`, `uuid{v4,serde}`, `thiserror`, `chrono{serde}`; `tokio` (currently `["rt-multi-thread","macros","time","sync"]`, **optional**, pulled by the default `transport` feature); `[features] default=["transport"]`, `transport=["reqwest","tokio","futures","async-stream","eventsource-stream","url"]`, `conformance=[]`. `[dev-dependencies]` has `hex`, `serde`, `tokio{rt-multi-thread,macros,time,sync}`.
- Tests are **integration tests in `crates/air-rs/tests/`** (no inline `#[cfg(test)]`); fixtures load via `include_str!(...)` relative to the test file (seal_tests.rs:58). 16 tests currently green.
- `apps/desktop/src-tauri`: Tauri **2**, `tokio = { features = ["full"] }`, `air-rs = { path = "../../../crates/air-rs" }` (default features). `AppState { air_client: Arc<dyn AirClient>, identity_store: IdentityStore }` built in `main.rs` `.setup()` and `.manage(...)`. Commands are `#[tauri::command] pub async fn …(state: State<'_, AppState>, …) -> Result<T, String>`; events via `app.emit("name", json!({…}))` (`app: &AppHandle`); long-lived work via `tauri::async_runtime::spawn(async move {…})` with `Arc<AtomicBool>` cancel flags in a `static LazyLock<RwLock<HashMap<…>>>` (llm_stream.rs precedent). The desktop's OWN identity is `<app_data_dir>/identity.json` = `{did, name, created_at}` (no `air_id`) — DIFFERENT directory + schema from the daemon's `~/.air-msg/identity.json`.

---

## MILESTONE 1 — the `air-rs::inbox` library (green via `cargo test -p air-rs`)

### Task 1: Cargo wiring + module scaffold

**Files:**
- Modify: `crates/air-rs/Cargo.toml`
- Modify: `crates/air-rs/src/lib.rs`
- Create: `crates/air-rs/src/inbox/mod.rs`
- Test: `crates/air-rs/tests/inbox_scaffold.rs`

- [ ] **Step 1: Add deps + the `inbox` feature.** In `crates/air-rs/Cargo.toml`, add to `[dependencies]` (after the existing block):

```toml
# Inbox (Phase A2) — read-only WAL reader for the daemon's archive.db. `bundled` vendors
# SQLite so there is no system-lib dependency across macOS/Linux. Gated by `inbox`.
rusqlite = { version = "0.32", features = ["bundled"], optional = true }
```

Extend the existing `tokio` line to add `net` + `io-util` (UnixStream + AsyncBufRead/Write):

```toml
tokio = { version = "1", features = ["rt-multi-thread", "macros", "time", "sync", "net", "io-util"], optional = true }
```

Replace `[features]` with (inbox is ON by default so `cargo test -p air-rs` and the desktop both get it without extra flags):

```toml
[features]
default = ["transport", "inbox"]
conformance = []  # Enables tests/conformance/jcs_vectors.rs against /specs/air/draft-1/test-vectors.json
transport = ["reqwest", "tokio", "futures", "async-stream", "eventsource-stream", "url"]
inbox = ["rusqlite", "tokio"]
```

Add to `[dev-dependencies]`:

```toml
tempfile = "3"
```

- [ ] **Step 2: Declare the module (feature-gated).** In `crates/air-rs/src/lib.rs`, after the `#[cfg(feature = "transport")] pub mod transport;` line add:

```rust
#[cfg(feature = "inbox")]
pub mod inbox;
```

- [ ] **Step 3: Scaffold the module tree.** Create `crates/air-rs/src/inbox/mod.rs`:

```rust
//! Phase A2 — the desktop inbox client stack (Tauri-agnostic).
//!
//! A reconnecting two-role daemon-socket client, a read-only WAL archive reader, an
//! identity-adopter, and a policy store. Built against the normative contract in
//! `agent-bridge-mcp/docs/PROTOCOL.md` + `test/fixtures/socket-frames.json`.

pub mod frames;
pub mod line_parser;
pub mod stores;
pub mod gate;
pub mod archive_reader;
pub mod replay;
pub mod identity_adopter;
pub mod policy_store;
pub mod client;

/// Resolve the daemon home: `AGENT_BRIDGE_HOME` or `~/.air-msg` (POSIX v1).
/// Mirrors `agent-bridge-mcp/src/identity.mjs` `bridgeHome()`.
pub fn bridge_home() -> std::path::PathBuf {
    if let Ok(h) = std::env::var("AGENT_BRIDGE_HOME") {
        return std::path::PathBuf::from(h);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    std::path::Path::new(&home).join(".air-msg")
}
```

Create each child module as an empty file for now so the tree compiles (later tasks fill them):

```bash
cd ~/air-note/crates/air-rs/src/inbox
for f in frames line_parser stores gate archive_reader replay identity_adopter policy_store client; do
  printf '//! Phase A2 inbox module (filled by a later task).\n' > "$f.rs"
done
```

- [ ] **Step 4: Write the failing scaffold test.** Create `crates/air-rs/tests/inbox_scaffold.rs`:

```rust
//! Proves the inbox feature compiles and home-resolution honours AGENT_BRIDGE_HOME.
use air_rs::inbox::bridge_home;

#[test]
fn bridge_home_honours_env() {
    // Hermetic: this process-global env is fine because the test asserts the override path.
    std::env::set_var("AGENT_BRIDGE_HOME", "/tmp/air-a2-scaffold");
    assert_eq!(bridge_home(), std::path::PathBuf::from("/tmp/air-a2-scaffold"));
    std::env::remove_var("AGENT_BRIDGE_HOME");
}
```

- [ ] **Step 5: Run — verify it builds + passes.**

Run: `cd ~/air-note && cargo test -p air-rs --test inbox_scaffold`
Expected: compiles (rusqlite bundles SQLite on first build — slow once), 1 passed.

- [ ] **Step 6: Commit.**

```bash
cd ~/air-note
git add crates/air-rs/Cargo.toml crates/air-rs/src/lib.rs crates/air-rs/src/inbox crates/air-rs/tests/inbox_scaffold.rs
git commit -m "feat(air-rs): inbox feature scaffold + home resolution (Phase A2)"
```

---

### Task 2: Wire frames + fixture round-trip

**Files:**
- Modify: `crates/air-rs/src/inbox/frames.rs`
- Test: `crates/air-rs/tests/inbox_frames.rs`

- [ ] **Step 1: Define the frame types.** Replace `crates/air-rs/src/inbox/frames.rs` with:

```rust
//! Wire frames for the daemon socket (PROTOCOL.md §2–§6). The fixture is the source of truth.
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A delivered message record (PROTOCOL §5 `message.message`). Conditional fields are omitted
/// when falsy on the wire (optional-when-falsy rule) — `Option` + skip_serializing_if reproduces
/// that, `#[serde(default)]` tolerates their absence on decode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub seq: i64,
    pub relay_seq: i64,
    pub envelope_id: String,
    pub from: String,
    pub verified: bool,
    pub encrypted: bool,
    pub received_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_changed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
}

impl Message {
    /// `key_changed` is truthy only when present-and-true (omitted means "no change").
    pub fn key_changed(&self) -> bool {
        matches!(self.key_changed, Some(true))
    }
}

/// One entry in `status.clients[]`. Field names are camelCase ON THE WIRE (PROTOCOL §5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientSnapshot {
    pub role: String,
    #[serde(rename = "lastSeq")]
    pub last_seq: Option<i64>,
    pub dropped: i64,
}

/// Frames the daemon sends to the client. **Decode-only in practice** — the client never serializes
/// a `ServerFrame`. Unknown `type`s decode to `Unknown` (PROTOCOL §8); note `Unknown` would serialize
/// to `{"type":"Unknown"}`, so never round-trip a `ServerFrame` back onto the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerFrame {
    #[serde(rename = "hello-ok")]
    HelloOk { pid: i64, start_time: String, did: String },
    #[serde(rename = "message")]
    Message { message: Message },
    #[serde(rename = "gap")]
    Gap { after_seq: i64 },
    #[serde(rename = "pong")]
    Pong,
    #[serde(rename = "status")]
    Status {
        socket: String,
        last_seq: Option<i64>,
        clients: Vec<ClientSnapshot>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sinks: Option<Vec<String>>,
    },
    #[serde(rename = "send-ok")]
    SendOk { id: String, envelope_id: String, encrypted: bool },
    #[serde(rename = "send-err")]
    SendErr { id: String, retryable: bool, reason: String },
    #[serde(rename = "error")]
    Error { reason: String },
    #[serde(other)]
    Unknown,
}

/// Frames the client sends to the daemon.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientFrame {
    #[serde(rename = "hello")]
    Hello {
        role: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        since_seq: Option<i64>,
    },
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "status")]
    Status,
    #[serde(rename = "send")]
    Send {
        id: String,
        to: String,
        body: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plaintext: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        in_reply_to: Option<String>,
    },
}
```

- [ ] **Step 2: Write the failing fixture tests.** Create `crates/air-rs/tests/inbox_frames.rs`. The fixture lives in the messaging-stack repo; load it with `include_str!` (path relative to this test file: `tests/` → up 3 → repo root → `agent-bridge-mcp/...`). Assert on `serde_json::Value` equality (key order is irrelevant):

```rust
//! Every fixture frame round-trips through the Rust types byte-for-byte (value-equal).
use air_rs::inbox::frames::{ClientFrame, Message, ServerFrame};
use serde_json::{json, Value};

const FIXTURES: &str =
    include_str!("../../../agent-bridge-mcp/test/fixtures/socket-frames.json");

fn fixtures() -> Value {
    serde_json::from_str(FIXTURES).expect("fixtures parse")
}

#[test]
fn fixture_version_is_1() {
    assert_eq!(fixtures()["version"], json!(1));
}

#[test]
fn client_frames_encode_to_fixtures() {
    let f = fixtures();
    let c = &f["client_to_daemon"];

    let hello = ClientFrame::Hello { role: "viewer".into(), since_seq: None };
    assert_eq!(serde_json::to_value(&hello).unwrap(), c["hello"]);

    let resume = ClientFrame::Hello { role: "channel".into(), since_seq: Some(41) };
    assert_eq!(serde_json::to_value(&resume).unwrap(), c["hello_channel_resume"]);

    assert_eq!(serde_json::to_value(ClientFrame::Ping).unwrap(), c["ping"]);
    assert_eq!(serde_json::to_value(ClientFrame::Status).unwrap(), c["status_request"]);

    // `send` omits plaintext/thread_id/in_reply_to (skip_serializing_if) — matches the fixture.
    let send: ClientFrame = serde_json::from_value(c["send"].clone()).unwrap();
    assert_eq!(serde_json::to_value(&send).unwrap(), c["send"]);
    assert!(matches!(send, ClientFrame::Send { ref plaintext, .. } if plaintext.is_none()));

    let send_pt: ClientFrame = serde_json::from_value(c["send_plaintext"].clone()).unwrap();
    assert_eq!(serde_json::to_value(&send_pt).unwrap(), c["send_plaintext"]);

    let send_reply: ClientFrame = serde_json::from_value(c["send_reply"].clone()).unwrap();
    assert_eq!(serde_json::to_value(&send_reply).unwrap(), c["send_reply"]);
}

#[test]
fn server_frames_decode_and_reencode_to_fixtures() {
    let f = fixtures();
    let d = &f["daemon_to_client"];
    for key in ["hello_ok", "message", "gap", "pong", "status", "send_ok", "send_err", "error"] {
        let frame: ServerFrame = serde_json::from_value(d[key].clone())
            .unwrap_or_else(|e| panic!("decode {key}: {e}"));
        assert!(!matches!(frame, ServerFrame::Unknown), "{key} must be a known frame");
        assert_eq!(serde_json::to_value(&frame).unwrap(), d[key], "{key} re-encode");
    }
}

#[test]
fn message_omits_key_changed_when_unchanged() {
    // The fixture message is a key-UNCHANGED sender → no key_changed key on the wire.
    let d = &fixtures()["daemon_to_client"];
    let ServerFrame::Message { message } =
        serde_json::from_value(d["message"].clone()).unwrap()
    else { panic!("not a message") };
    assert_eq!(message.key_changed, None);
    assert!(!message.key_changed());
    assert_eq!(message.contact.as_deref(), Some("pat"));
    // Re-encode must NOT introduce a key_changed key.
    let v = serde_json::to_value(&message).unwrap();
    assert!(v.get("key_changed").is_none());
}

#[test]
fn unknown_frame_type_decodes_to_unknown_not_error() {
    // PROTOCOL §8: additive evolution — unknown daemon frame types are tolerated.
    let v = json!({ "type": "future-thing", "whatever": 1 });
    let frame: ServerFrame = serde_json::from_value(v).expect("must not error");
    assert_eq!(frame, ServerFrame::Unknown);
}

#[test]
fn unknown_fields_within_known_frame_are_ignored() {
    let v = json!({ "type": "gap", "after_seq": 9, "added_later": true });
    let frame: ServerFrame = serde_json::from_value(v).expect("must ignore extra field");
    assert_eq!(frame, ServerFrame::Gap { after_seq: 9 });
}

#[test]
fn status_clients_use_camelcase_lastseq() {
    let d = &fixtures()["daemon_to_client"];
    let ServerFrame::Status { clients, sinks, last_seq, .. } =
        serde_json::from_value(d["status"].clone()).unwrap()
    else { panic!("not status") };
    assert_eq!(last_seq, Some(7));
    assert_eq!(sinks, Some(vec!["banner".into(), "socket".into()]));
    assert_eq!(clients[0].role, "viewer");
    assert_eq!(clients[0].last_seq, Some(7));
    assert_eq!(clients[1].last_seq, None); // channel seeded null
}

// keep `Message` import used even if a future refactor drops a direct reference
#[allow(unused)]
fn _types_ref(_: Message) {}
```

- [ ] **Step 3: Run — confirm RED then GREEN.**

Run: `cd ~/air-note && cargo test -p air-rs --test inbox_frames`
Expected after Step 1's types exist: all pass. If any frame fails value-equality, the struct disagrees with the fixture — **fix the struct, never the fixture.**

- [ ] **Step 4: Commit.**

```bash
cd ~/air-note
git add crates/air-rs/src/inbox/frames.rs crates/air-rs/tests/inbox_frames.rs
git commit -m "feat(air-rs): inbox wire frames asserted against the normative fixtures"
```

---

### Task 3: Line parser (newline framing + 1 MiB ceiling)

**Files:**
- Modify: `crates/air-rs/src/inbox/line_parser.rs`
- Test: `crates/air-rs/tests/inbox_line_parser.rs`

- [ ] **Step 1: Implement (port `makeLineParser`).** Replace `crates/air-rs/src/inbox/line_parser.rs`:

```rust
//! Newline-delimited JSON framing (PROTOCOL §1; ports daemon-ipc.mjs `makeLineParser`).
use serde_json::Value;

/// 1 MiB line ceiling (PROTOCOL §1). A line exceeding this with no newline is a protocol error.
pub const MAX_FRAME: usize = 1 << 20;

#[derive(Debug)]
pub enum FrameEvent {
    /// A parsed JSON object frame.
    Frame(Value),
    /// A line failed to parse (bad JSON) or the ceiling was exceeded — non-fatal to the parser;
    /// the CLIENT decides what to do (the daemon closes the socket on its side).
    ParseError(String),
}

/// Stateful accumulator: feed raw bytes, get back zero or more events. Mirrors the JS parser's
/// semantics exactly: skip blank lines, surface parse errors, and on an over-ceiling line with no
/// newline yet, emit one ParseError and RESET the buffer (drop the garbage).
#[derive(Default)]
pub struct LineParser {
    buf: String,
}

impl LineParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, chunk: &[u8]) -> Vec<FrameEvent> {
        let mut out = Vec::new();
        self.buf.push_str(&String::from_utf8_lossy(chunk));
        if self.buf.len() > MAX_FRAME && !self.buf.contains('\n') {
            out.push(FrameEvent::ParseError(format!("line exceeds {MAX_FRAME} bytes")));
            self.buf.clear();
            return out;
        }
        while let Some(nl) = self.buf.find('\n') {
            let line: String = self.buf.drain(..=nl).collect();
            let line = line.trim_end_matches('\n');
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Value>(line) {
                Ok(v) => out.push(FrameEvent::Frame(v)),
                Err(e) => out.push(FrameEvent::ParseError(e.to_string())),
            }
        }
        out
    }
}
```

- [ ] **Step 2: Failing tests.** Create `crates/air-rs/tests/inbox_line_parser.rs`:

```rust
use air_rs::inbox::line_parser::{FrameEvent, LineParser, MAX_FRAME};
use serde_json::json;

fn frames(evs: Vec<FrameEvent>) -> Vec<serde_json::Value> {
    evs.into_iter().filter_map(|e| match e { FrameEvent::Frame(v) => Some(v), _ => None }).collect()
}

#[test]
fn splits_two_frames_in_one_chunk() {
    let mut p = LineParser::new();
    let out = p.feed(b"{\"type\":\"pong\"}\n{\"type\":\"gap\",\"after_seq\":3}\n");
    let fs = frames(out);
    assert_eq!(fs, vec![json!({"type":"pong"}), json!({"type":"gap","after_seq":3})]);
}

#[test]
fn reassembles_a_frame_split_across_chunks() {
    let mut p = LineParser::new();
    assert!(frames(p.feed(b"{\"type\":\"po")).is_empty());
    let fs = frames(p.feed(b"ng\"}\n"));
    assert_eq!(fs, vec![json!({"type":"pong"})]);
}

#[test]
fn skips_blank_lines() {
    let mut p = LineParser::new();
    let fs = frames(p.feed(b"\n   \n{\"type\":\"pong\"}\n"));
    assert_eq!(fs, vec![json!({"type":"pong"})]);
}

#[test]
fn bad_json_surfaces_a_parse_error_and_continues() {
    let mut p = LineParser::new();
    let out = p.feed(b"not json\n{\"type\":\"pong\"}\n");
    assert!(matches!(out[0], FrameEvent::ParseError(_)));
    assert_eq!(frames(out), vec![json!({"type":"pong"})]);
}

#[test]
fn over_ceiling_line_without_newline_errors_and_resets() {
    let mut p = LineParser::new();
    let big = vec![b'x'; MAX_FRAME + 1];
    let out = p.feed(&big);
    assert_eq!(out.len(), 1);
    assert!(matches!(out[0], FrameEvent::ParseError(_)));
    // Buffer reset: a subsequent clean frame parses.
    assert_eq!(frames(p.feed(b"{\"type\":\"pong\"}\n")), vec![json!({"type":"pong"})]);
}
```

- [ ] **Step 3: Run.** `cd ~/air-note && cargo test -p air-rs --test inbox_line_parser` → 5 passed.
- [ ] **Step 4: Commit.** `git add crates/air-rs/src/inbox/line_parser.rs crates/air-rs/tests/inbox_line_parser.rs && git commit -m "feat(air-rs): newline frame parser with 1 MiB ceiling"`

---

### Task 4: Stores (home-scoped readers + mute + short_peer)

**Files:**
- Modify: `crates/air-rs/src/inbox/stores.rs`
- Test: `crates/air-rs/tests/inbox_stores.rs`

- [ ] **Step 1: Implement.** Replace `crates/air-rs/src/inbox/stores.rs`:

```rust
//! Read-only, home-scoped views of the CLI's JSON stores (contacts/blocklist/identity) plus the
//! mute set and the DID→AIR-id helper. Ports peers.mjs / contacts.mjs / moderation.mjs / identity.mjs.
use serde::Deserialize;
use std::collections::HashSet;
use std::path::Path;

/// Short AIR-id from a DID (ports `shortPeer`): first `AIR-<alnum/->` run, else the input.
pub fn short_peer(did: &str) -> String {
    let bytes = did.as_bytes();
    if let Some(start) = did.find("AIR-") {
        let mut end = start + 4;
        while end < bytes.len() {
            let c = bytes[end];
            if c.is_ascii_alphanumeric() || c == b'-' { end += 1 } else { break }
        }
        if end > start + 4 {
            return did[start..end].to_string();
        }
    }
    did.to_string()
}

/// Parse `AIRMSG_MUTE` (comma-separated alias/DID/AIR-id) into a set (ports `parseMuteSet`).
pub fn parse_mute_set() -> HashSet<String> {
    std::env::var("AIRMSG_MUTE")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// A pinned contact (only the fields A2 consumes; serde ignores the rest).
#[derive(Debug, Clone, Deserialize)]
pub struct Contact {
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default)]
    pub air_id: Option<String>,
    #[serde(default)]
    pub public_key_multibase: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ContactsFile {
    #[serde(default)]
    contacts: std::collections::HashMap<String, Contact>,
}

/// Current pinned contact for a DID (ports `getContactByDid`). None on any read/parse error.
pub fn get_contact_by_did(home: &Path, did: &str) -> Option<Contact> {
    let raw = std::fs::read_to_string(home.join("contacts.json")).ok()?;
    let file: ContactsFile = serde_json::from_str(&raw).ok()?;
    file.contacts.get(did).cloned()
}

/// Is this DID blocked (ports `isBlocked`)? **Fail-OPEN (false) on ANY error** — a corrupt
/// blocklist must never black-hole all mail (moderation.mjs D6).
pub fn is_blocked(home: &Path, did: &str) -> bool {
    (|| -> Option<bool> {
        let raw = std::fs::read_to_string(home.join("blocklist.json")).ok()?;
        let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
        Some(v.get("blocked")?.get(did).is_some())
    })()
    .unwrap_or(false)
}

/// The daemon identity METADATA the desktop is allowed to read (PUBLIC fields only).
#[derive(Debug, Clone, Deserialize)]
pub struct DaemonIdentityMeta {
    pub did: String,
    #[serde(default)]
    pub name: Option<String>,
}

/// Read `{home}/identity.json` and return ONLY did + name. SENSITIVE fields (`seed_hex`,
/// `agent_secret`) are never deserialized into the desktop process (design §4).
pub fn read_daemon_identity_meta(home: &Path) -> Option<DaemonIdentityMeta> {
    let raw = std::fs::read_to_string(home.join("identity.json")).ok()?;
    serde_json::from_str::<DaemonIdentityMeta>(&raw).ok()
}
```

- [ ] **Step 2: Failing tests.** Create `crates/air-rs/tests/inbox_stores.rs`:

```rust
use air_rs::inbox::stores::*;
use std::fs;
use tempfile::TempDir;

fn home() -> TempDir {
    TempDir::new().unwrap()
}

#[test]
fn short_peer_extracts_air_id_or_passes_through() {
    assert_eq!(short_peer("did:wba:agentidentityregistry.org:agents:AIR-2JE0-EM7W-JNBK"), "AIR-2JE0-EM7W-JNBK");
    assert_eq!(short_peer("plain-alias"), "plain-alias");
}

#[test]
fn mute_set_parses_and_trims() {
    std::env::set_var("AIRMSG_MUTE", " pat, AIR-XXXX ,,bob ");
    let m = parse_mute_set();
    assert!(m.contains("pat") && m.contains("AIR-XXXX") && m.contains("bob"));
    assert!(!m.contains(""));
    std::env::remove_var("AIRMSG_MUTE");
}

#[test]
fn contact_lookup_returns_alias_when_pinned() {
    let h = home();
    fs::write(h.path().join("contacts.json"),
        r#"{"version":1,"contacts":{"did:x":{"alias":"pat","air_id":"AIR-1","public_key_multibase":"zABC"}}}"#).unwrap();
    let c = get_contact_by_did(h.path(), "did:x").unwrap();
    assert_eq!(c.alias.as_deref(), Some("pat"));
    assert!(get_contact_by_did(h.path(), "did:unknown").is_none());
}

#[test]
fn blocked_check_fails_open_on_missing_or_corrupt() {
    let h = home();
    assert!(!is_blocked(h.path(), "did:x")); // no file → not blocked
    fs::write(h.path().join("blocklist.json"), "{ this is not json").unwrap();
    assert!(!is_blocked(h.path(), "did:x")); // corrupt → fail OPEN (false)
    fs::write(h.path().join("blocklist.json"),
        r#"{"version":1,"blocked":{"did:x":{"air_id":"AIR-1"}}}"#).unwrap();
    assert!(is_blocked(h.path(), "did:x"));
    assert!(!is_blocked(h.path(), "did:y"));
}

#[test]
fn identity_meta_reads_only_public_fields() {
    let h = home();
    fs::write(h.path().join("identity.json"),
        r#"{"version":1,"name":"peters-agent","air_id":"AIR-2JE0","did":"did:wba:x:agents:AIR-2JE0","seed_hex":"DEADBEEF","agent_secret":"TOPSECRET"}"#).unwrap();
    let meta = read_daemon_identity_meta(h.path()).unwrap();
    assert_eq!(meta.did, "did:wba:x:agents:AIR-2JE0");
    assert_eq!(meta.name.as_deref(), Some("peters-agent"));
    // Compile-time guarantee: DaemonIdentityMeta has no seed_hex/agent_secret field to read into.
}
```

- [ ] **Step 3: Run.** `cargo test -p air-rs --test inbox_stores` → 5 passed.
- [ ] **Step 4: Commit.** `git add crates/air-rs/src/inbox/stores.rs crates/air-rs/tests/inbox_stores.rs && git commit -m "feat(air-rs): home-scoped contact/blocklist/identity readers + mute"`

---

### Task 5: The channel gate

**Files:**
- Modify: `crates/air-rs/src/inbox/gate.rs`
- Test: `crates/air-rs/tests/inbox_gate.rs`

- [ ] **Step 1: Implement (port `channelGate`).** Replace `crates/air-rs/src/inbox/gate.rs`:

```rust
//! The channel admission gate (ports channel.mjs `channelGate`): verified + pinned (non-empty
//! contact alias) + key-unchanged + not-muted. Pure.
use crate::inbox::frames::Message;
use crate::inbox::stores::short_peer;
use std::collections::HashSet;

pub fn channel_gate(m: &Message, mute: &HashSet<String>) -> bool {
    let contact = match m.contact.as_deref() {
        Some(c) if !c.is_empty() => c,
        _ => return false,
    };
    if !m.verified || m.key_changed() {
        return false;
    }
    if mute.contains(contact) || mute.contains(&m.from) || mute.contains(&short_peer(&m.from)) {
        return false;
    }
    true
}
```

- [ ] **Step 2: Failing tests.** Create `crates/air-rs/tests/inbox_gate.rs`:

```rust
use air_rs::inbox::frames::Message;
use air_rs::inbox::gate::channel_gate;
use std::collections::HashSet;

fn base() -> Message {
    Message {
        seq: 1, relay_seq: 1, envelope_id: "e1".into(),
        from: "did:wba:x:agents:AIR-PEER-PEER-PEER".into(),
        verified: true, encrypted: true, received_at: "t".into(),
        contact: Some("pat".into()), key_changed: None, thread_id: None, room_id: None, body: None,
    }
}

#[test]
fn admits_verified_pinned_unchanged_unmuted() {
    assert!(channel_gate(&base(), &HashSet::new()));
}
#[test]
fn rejects_unverified() {
    let mut m = base(); m.verified = false;
    assert!(!channel_gate(&m, &HashSet::new()));
}
#[test]
fn rejects_unpinned_or_empty_contact() {
    let mut m = base(); m.contact = None;
    assert!(!channel_gate(&m, &HashSet::new()));
    m.contact = Some(String::new());
    assert!(!channel_gate(&m, &HashSet::new()));
}
#[test]
fn rejects_key_changed() {
    let mut m = base(); m.key_changed = Some(true);
    assert!(!channel_gate(&m, &HashSet::new()));
}
#[test]
fn rejects_muted_by_alias_did_or_airid() {
    let m = base();
    assert!(!channel_gate(&m, &HashSet::from(["pat".to_string()])));
    assert!(!channel_gate(&m, &HashSet::from([m.from.clone()])));
    assert!(!channel_gate(&m, &HashSet::from(["AIR-PEER-PEER-PEER".to_string()])));
}
```

- [ ] **Step 3: Run.** `cargo test -p air-rs --test inbox_gate` → 5 passed.
- [ ] **Step 4: Commit.** `git add crates/air-rs/src/inbox/gate.rs crates/air-rs/tests/inbox_gate.rs && git commit -m "feat(air-rs): channel admission gate (verified+pinned+unchanged+unmuted)"`

---

### Task 6: Read-only WAL archive reader

**Files:**
- Modify: `crates/air-rs/src/inbox/archive_reader.rs`
- Test: `crates/air-rs/tests/inbox_archive_reader.rs`

> **Risk this task de-risks (call out to the critic):** opening a WAL database `SQLITE_OPEN_READ_ONLY` from a *second process* has sharp edges — the reader still needs the `-wal`/`-shm` files, which exist because the desktop runs as the SAME user (0600 files in the user's own `~/.air-msg`). The mitigation the spec mandates is **bounded busy-retry, never a hard error**; the read-during-write soak test below is its proof.

- [ ] **Step 1: Implement.** Replace `crates/air-rs/src/inbox/archive_reader.rs`:

```rust
//! Read-only view of the daemon's `archive.db` (WAL). SHORT-LIVED statements only — never hold a
//! read txn open under WAL (it would unbound the writer's WAL file). Ports the read paths of
//! archive.mjs: parseRow / replaySince / getCursor / history / threads.
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;

const BUSY_TIMEOUT_MS: u64 = 5000;
const OPEN_RETRIES: u32 = 12;
const OPEN_RETRY_SLEEP: Duration = Duration::from_millis(50);

/// One archived message row (ports `parseRow`).
#[derive(Debug, Clone, Serialize)]
pub struct ArchiveRow {
    pub envelope_id: String,
    pub direction: String,
    pub thread_id: String,
    pub peer_did: String,
    pub from: String,
    pub to: String,
    pub timestamp: String,
    pub body: Value,
    pub encrypted: bool,
    pub verified: bool,
    pub key_changed: bool,
    pub spam: bool,
    pub relay_seq: Option<i64>,
    pub room_id: Option<String>,
    pub archived_at: String,
}

fn archive_path(home: &Path) -> PathBuf {
    home.join("archive.db")
}

/// Does the archive file exist (ports `archiveExists`)? Never materializes a DB.
pub fn archive_exists(home: &Path) -> bool {
    archive_path(home).exists()
}

pub struct ArchiveReader {
    conn: Connection,
}

impl ArchiveReader {
    /// Open read-only with a busy_timeout, retrying transient open failures (a concurrent
    /// checkpoint or a momentary `-shm`/`-wal` race). Never panics; bubbles a typed error after
    /// the bounded retry budget.
    pub fn open(home: &Path) -> Result<Self, rusqlite::Error> {
        let path = archive_path(home);
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let mut last: Option<rusqlite::Error> = None;
        for _ in 0..OPEN_RETRIES {
            match Connection::open_with_flags(&path, flags) {
                Ok(conn) => {
                    conn.busy_timeout(Duration::from_millis(BUSY_TIMEOUT_MS))?;
                    return Ok(Self { conn });
                }
                Err(e) => {
                    last = Some(e);
                    std::thread::sleep(OPEN_RETRY_SLEEP);
                }
            }
        }
        Err(last.unwrap())
    }

    /// Replay source (ports `replaySince` — invariants #1–#3 only; the replayer adds #4 + #5).
    pub fn replay_since(&self, since_seq: i64, limit: i64) -> Result<Vec<ArchiveRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT envelope_id, direction, thread_id, peer_did, from_did, to_did, timestamp, \
                    body_json, encrypted, verified, key_changed, spam, relay_seq, room_id, archived_at \
             FROM messages \
             WHERE direction = 'received' AND spam = 0 AND relay_seq IS NOT NULL AND relay_seq > ?1 \
               AND envelope_id NOT LIKE '%:joined' \
             ORDER BY relay_seq ASC LIMIT ?2",
        )?;
        let rows = stmt.query_map([since_seq, limit], map_row)?;
        rows.collect()
    }

    /// Pull cursor (ports `getCursor`): highest relay_seq pulled, 0 if unset.
    pub fn get_cursor(&self) -> Result<i64, rusqlite::Error> {
        let v: Option<String> = self
            .conn
            .query_row("SELECT value FROM meta WHERE key = 'pull_cursor'", [], |r| r.get(0))
            .ok();
        Ok(v.and_then(|s| s.parse::<i64>().ok()).unwrap_or(0))
    }

    /// Conversation history, newest-first (ports `history`). `before` is an ISO timestamp.
    pub fn history(
        &self,
        peer: Option<&str>,
        thread: Option<&str>,
        room: Option<&str>,
        before: Option<&str>,
        limit: i64,
        include_spam: bool,
    ) -> Result<Vec<ArchiveRow>, rusqlite::Error> {
        let mut where_sql = Vec::new();
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(p) = peer { where_sql.push("peer_did = ?"); params.push(Box::new(p.to_string())); }
        if let Some(t) = thread { where_sql.push("thread_id = ?"); params.push(Box::new(t.to_string())); }
        if let Some(r) = room { where_sql.push("room_id = ?"); params.push(Box::new(r.to_string())); }
        if let Some(b) = before { where_sql.push("timestamp < ?"); params.push(Box::new(b.to_string())); }
        if !include_spam { where_sql.push("spam = 0"); }
        let clause = if where_sql.is_empty() { String::new() } else { format!("WHERE {}", where_sql.join(" AND ")) };
        params.push(Box::new(limit));
        let sql = format!(
            "SELECT envelope_id, direction, thread_id, peer_did, from_did, to_did, timestamp, \
                    body_json, encrypted, verified, key_changed, spam, relay_seq, room_id, archived_at \
             FROM messages {clause} ORDER BY timestamp DESC, archived_at DESC LIMIT ?"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();
        let rows = stmt.query_map(refs.as_slice(), map_row)?;
        rows.collect()
    }
}

/// One conversation summary for the §6 sidebar. **Grouping per design §6 (critic M1):** 1:1
/// conversations key on `peer_did`, rooms on `room_id` — NOT `thread_id` (outbound 1:1 thread_ids
/// default to a fresh uuid per message and would fragment the list).
#[derive(Debug, Clone, Serialize)]
pub struct ConversationSummary {
    /// The grouping key: `room_id` for rooms, else `peer_did`.
    pub conv_key: String,
    /// `"room"` | `"peer"`.
    pub kind: String,
    pub last_timestamp: String,
    pub count: i64,
}

impl ArchiveReader {
    pub fn conversations(&self) -> Result<Vec<ConversationSummary>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT CASE WHEN room_id IS NOT NULL THEN room_id ELSE peer_did END AS conv_key, \
                    CASE WHEN room_id IS NOT NULL THEN 'room' ELSE 'peer' END AS kind, \
                    MAX(timestamp) AS last_timestamp, COUNT(*) AS count \
             FROM messages WHERE spam = 0 \
             GROUP BY conv_key ORDER BY last_timestamp DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(ConversationSummary {
                conv_key: r.get(0)?,
                kind: r.get(1)?,
                last_timestamp: r.get(2)?,
                count: r.get(3)?,
            })
        })?;
        rows.collect()
    }
}

fn map_row(r: &rusqlite::Row) -> Result<ArchiveRow, rusqlite::Error> {
    let body_json: String = r.get(7)?;
    Ok(ArchiveRow {
        envelope_id: r.get(0)?,
        direction: r.get(1)?,
        thread_id: r.get(2)?,
        peer_did: r.get(3)?,
        from: r.get(4)?,
        to: r.get(5)?,
        timestamp: r.get(6)?,
        body: serde_json::from_str(&body_json).unwrap_or(Value::Null),
        encrypted: r.get::<_, i64>(8)? != 0,
        verified: r.get::<_, i64>(9)? != 0,
        key_changed: r.get::<_, i64>(10)? != 0,
        spam: r.get::<_, i64>(11)? != 0,
        relay_seq: r.get(12)?,
        room_id: r.get(13)?,
        archived_at: r.get(14)?,
    })
}
```

- [ ] **Step 2: A test helper that writes a REAL daemon-shape archive.** The test must build an archive byte-identical to what the daemon writes so the reader is exercised against the true schema. Create `crates/air-rs/tests/inbox_archive_reader.rs`:

```rust
use air_rs::inbox::archive_reader::{archive_exists, ArchiveReader};
use rusqlite::Connection;
use tempfile::TempDir;

/// Build an archive.db exactly as the daemon would (schema from archive.mjs:26-47 + migrations,
/// WAL set writer-side). Returns the temp home holding it.
fn seed_archive() -> TempDir {
    let home = TempDir::new().unwrap();
    let path = home.path().join("archive.db");
    let conn = Connection::open(&path).unwrap();
    conn.pragma_update(None, "busy_timeout", 5000).unwrap();
    conn.pragma_update(None, "journal_mode", "WAL").unwrap();
    conn.execute_batch(
        "CREATE TABLE messages (
            envelope_id TEXT NOT NULL, direction TEXT NOT NULL, thread_id TEXT NOT NULL,
            peer_did TEXT NOT NULL, from_did TEXT NOT NULL, to_did TEXT NOT NULL,
            timestamp TEXT NOT NULL, body_json TEXT NOT NULL, encrypted INTEGER NOT NULL,
            verified INTEGER NOT NULL, key_changed INTEGER NOT NULL DEFAULT 0, relay_seq INTEGER,
            spam INTEGER NOT NULL DEFAULT 0, room_id TEXT, archived_at TEXT NOT NULL,
            PRIMARY KEY (envelope_id, direction));
         CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    ).unwrap();
    let mut ins = |env: &str, dir: &str, from: &str, relay_seq: Option<i64>, verified: i64, key_changed: i64, spam: i64, ts: &str| {
        conn.execute(
            "INSERT INTO messages (envelope_id,direction,thread_id,peer_did,from_did,to_did,timestamp,body_json,encrypted,verified,key_changed,relay_seq,spam,room_id,archived_at) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,1,?9,?10,?11,?12,NULL,?7)",
            rusqlite::params![env, dir, "th", from, from, "me", ts, r#"{"type":"text","text":"hi"}"#, verified, key_changed, relay_seq, spam],
        ).unwrap();
    };
    ins("e1", "received", "did:peer", Some(1), 1, 0, 0, "2026-06-11T00:00:01Z");
    ins("e2", "received", "did:peer", Some(2), 1, 0, 0, "2026-06-11T00:00:02Z");
    ins("e3", "sent",     "did:peer", Some(3), 1, 0, 0, "2026-06-11T00:00:03Z"); // excluded (#1)
    ins("e4", "received", "did:peer", Some(4), 1, 0, 1, "2026-06-11T00:00:04Z"); // excluded (#2 spam)
    ins("room1:joined", "received", "did:peer", Some(5), 1, 0, 0, "2026-06-11T00:00:05Z"); // excluded (#3)
    conn.execute("INSERT INTO meta (key,value) VALUES ('pull_cursor','2')", []).unwrap();
    home
}

#[test]
fn archive_exists_probe() {
    let h = TempDir::new().unwrap();
    assert!(!archive_exists(h.path()));
    let s = seed_archive();
    assert!(archive_exists(s.path()));
}

#[test]
fn replay_since_applies_sql_invariants_1_to_3() {
    let h = seed_archive();
    let r = ArchiveReader::open(h.path()).unwrap();
    let rows = r.replay_since(0, 500).unwrap();
    // e1,e2 only: sent (e3), spam (e4), and the :joined notice (room1:joined) are all excluded.
    let ids: Vec<_> = rows.iter().map(|x| x.envelope_id.as_str()).collect();
    assert_eq!(ids, vec!["e1", "e2"]);
    // strictly-greater-than:
    assert_eq!(r.replay_since(1, 500).unwrap().iter().map(|x| x.envelope_id.clone()).collect::<Vec<_>>(), vec!["e2"]);
}

#[test]
fn get_cursor_reads_meta() {
    let h = seed_archive();
    let r = ArchiveReader::open(h.path()).unwrap();
    assert_eq!(r.get_cursor().unwrap(), 2);
}

#[test]
fn history_reads_back_and_conversations_group_by_peer_not_thread() {
    // §6 (critic M1): two received rows, SAME peer, DIFFERENT thread_id → exactly ONE conversation.
    let h = seed_archive();
    {
        let w = Connection::open(h.path().join("archive.db")).unwrap();
        for (env, thread, sec) in [("c1", "thread-A", 10), ("c2", "thread-B", 11)] {
            w.execute(
                "INSERT INTO messages (envelope_id,direction,thread_id,peer_did,from_did,to_did,timestamp,body_json,encrypted,verified,key_changed,relay_seq,spam,room_id,archived_at) \
                 VALUES (?1,'received',?2,'did:peer','did:peer','me',?3,'{\"type\":\"text\",\"text\":\"x\"}',1,1,0,?4,0,NULL,?3)",
                rusqlite::params![env, thread, format!("2026-06-11T00:02:{sec}Z"), sec],
            ).unwrap();
        }
    }
    let r = ArchiveReader::open(h.path()).unwrap();
    let hist = r.history(Some("did:peer"), None, None, None, 50, false).unwrap();
    assert!(hist.iter().all(|x| !x.spam));            // spam excluded by default
    assert!(hist.iter().any(|x| x.envelope_id == "e3")); // history includes sent rows
    let convs = r.conversations().unwrap();
    let peer_convs: Vec<_> = convs.iter().filter(|c| c.conv_key == "did:peer").collect();
    assert_eq!(peer_convs.len(), 1, "different thread_ids for one peer must NOT fragment the sidebar");
    assert_eq!(peer_convs[0].kind, "peer");
}

#[test]
fn reads_during_same_process_write_fast_check() {
    // Fast always-on sanity: a SECOND same-process connection writes while the RO reader queries.
    // WAL must let both proceed without a hard SQLITE_BUSY. (The faithful cross-process proof is the
    // next test; this one needs no external runtime so it always runs.)
    let h = seed_archive();
    let writer = Connection::open(h.path().join("archive.db")).unwrap();
    writer.busy_timeout(std::time::Duration::from_millis(5000)).unwrap();
    let reader = ArchiveReader::open(h.path()).unwrap();
    for i in 6..40 {
        writer.execute(
            "INSERT INTO messages (envelope_id,direction,thread_id,peer_did,from_did,to_did,timestamp,body_json,encrypted,verified,key_changed,relay_seq,spam,room_id,archived_at) \
             VALUES (?1,'received','th','did:peer','did:peer','me',?2,'{\"type\":\"text\",\"text\":\"x\"}',1,1,0,?3,0,NULL,?2)",
            rusqlite::params![format!("e{i}"), format!("2026-06-11T00:01:{:02}Z", i), i],
        ).unwrap();
        let rows = reader.replay_since(0, 500).unwrap(); // short-lived statement each loop
        assert!(rows.len() >= 2, "reader must keep seeing rows during writes");
    }
}

#[test]
fn reads_while_a_separate_process_writes() {
    // The faithful soak (critic M2): a genuinely SEPARATE OS process writes to the live WAL db while
    // the rusqlite read-only reader reads — the real daemon-writer / desktop-reader topology. Uses
    // python3 (universally present on macOS + Linux CI; stdlib sqlite3; stable WAL). Skips (does not
    // fail) if python3 is absent so a python-less dev box stays green.
    use std::process::Command;
    if Command::new("python3").arg("--version").output().is_err() {
        eprintln!("skipping cross-process soak: python3 not found");
        return;
    }
    let h = seed_archive();
    let db = h.path().join("archive.db");
    let script = h.path().join("writer.py");
    std::fs::write(&script, r#"
import sqlite3, sys, time
c = sqlite3.connect(sys.argv[1])
c.execute("PRAGMA busy_timeout=5000")
c.execute("PRAGMA journal_mode=WAL")
body = '{"type":"text","text":"x"}'
for i in range(100, 160):
    c.execute(
        "INSERT INTO messages (envelope_id,direction,thread_id,peer_did,from_did,to_did,timestamp,body_json,encrypted,verified,key_changed,relay_seq,spam,room_id,archived_at) "
        "VALUES (?,'received','th','did:peer','did:peer','me',?,?,1,1,0,?,0,NULL,?)",
        ("e" + str(i), "2026-06-11T00:03:" + str(i), body, i, "2026-06-11T00:03:" + str(i)),
    )
    c.commit()
    time.sleep(0.04)
"#).unwrap();
    let mut child = Command::new("python3").arg(&script).arg(&db).spawn().unwrap();
    let reader = ArchiveReader::open(h.path()).unwrap();
    let mut peak = 0usize;
    for _ in 0..60 {
        peak = peak.max(reader.replay_since(0, 1000).unwrap().len()); // short-lived stmt each loop
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let _ = child.wait();
    // After the writer exits, every committed row is visible: seed e1,e2 (received) + e100..e159.
    let final_n = reader.replay_since(0, 1000).unwrap().len();
    assert!(final_n >= 60, "RO reader must read rows a SEPARATE process wrote (final {final_n}, peak {peak})");
}
```

- [ ] **Step 3: Run.** `cargo test -p air-rs --test inbox_archive_reader` → 6 passed (incl. the separate-process soak; it self-skips with a printed notice if `python3` is unavailable). If a soak flakes on `SQLITE_BUSY`, that is the exact failure the bounded retry + busy_timeout exist to absorb — widen `OPEN_RETRIES`/wrap queries before changing the design. The cross-process probe was already confirmed green out-of-band (Python `mode=ro` reader + rusqlite reader/writer, `busy_errors=0`).
- [ ] **Step 4: Commit.** `git add crates/air-rs/src/inbox/archive_reader.rs crates/air-rs/tests/inbox_archive_reader.rs && git commit -m "feat(air-rs): read-only WAL archive reader (replay/cursor/history/threads)"`

---

### Task 7: The replayer (the 5-invariant pipeline)

**Files:**
- Modify: `crates/air-rs/src/inbox/replay.rs`
- Test: `crates/air-rs/tests/inbox_replay.rs`

- [ ] **Step 1: Implement (port `rowToMessage` + `makeReplayer`).** Replace `crates/air-rs/src/inbox/replay.rs`:

```rust
//! At-least-once replay (ports channel-replay.mjs). On a `gap`, replay the hole from the archive
//! and re-apply ALL FIVE invariants identically to live delivery — replay never delivers more than
//! live did. The SQL gives #1–#3; THIS module adds #4 (blocklist) + #5 (current-pin channel gate),
//! plus dedupe across the live/replay overlap.
use crate::inbox::archive_reader::{ArchiveReader, ArchiveRow};
use crate::inbox::frames::Message;
use crate::inbox::gate::channel_gate;
use crate::inbox::stores::{get_contact_by_did, is_blocked};
use std::collections::{HashSet, VecDeque};
use std::path::Path;

const MAX_SEEN: usize = 1000;
const PAGE_SIZE: i64 = 500;

/// Map an archive row to the wire `Message` shape, deriving `contact` from CURRENT pin state
/// (invariant #5's "currently-pinned"). Ports `rowToMessage`.
pub fn row_to_message(row: &ArchiveRow, home: &Path) -> Message {
    let contact = get_contact_by_did(home, &row.from).and_then(|c| c.alias);
    Message {
        seq: row.relay_seq.unwrap_or(0),
        relay_seq: row.relay_seq.unwrap_or(0),
        from: row.from.clone(),
        contact: contact.filter(|a| !a.is_empty()),
        envelope_id: row.envelope_id.clone(),
        received_at: row.timestamp.clone(),
        verified: row.verified,
        encrypted: row.encrypted,
        key_changed: if row.key_changed { Some(true) } else { None },
        room_id: row.room_id.clone(),
        body: Some(row.body.clone()),
        thread_id: Some(row.thread_id.clone()),
    }
}

/// Deduping replay coordinator. `live` feeds streamed frames; `gap` fills a hole from the archive.
/// Both paths emit only gate-admitted messages (the daemon already gated live, but re-gating is the
/// JS pipeline's behaviour and is the security backstop for the replay path).
pub struct Replayer {
    seen: HashSet<String>,
    order: VecDeque<String>,
    mute: HashSet<String>,
}

impl Replayer {
    pub fn new(mute: HashSet<String>) -> Self {
        Self { seen: HashSet::new(), order: VecDeque::new(), mute }
    }

    fn remember(&mut self, id: &str) {
        if self.seen.insert(id.to_string()) {
            self.order.push_back(id.to_string());
            if self.order.len() > MAX_SEEN {
                if let Some(old) = self.order.pop_front() {
                    self.seen.remove(&old);
                }
            }
        }
    }

    /// A streamed frame: dedupe, then admit iff the gate passes. Returns Some to emit.
    pub fn live(&mut self, m: Message) -> Option<Message> {
        if self.seen.contains(&m.envelope_id) {
            return None;
        }
        self.remember(&m.envelope_id.clone());
        if channel_gate(&m, &self.mute) { Some(m) } else { None }
    }

    /// Replay the hole after `after_seq`. Paginates (a long outage exceeds one page — a silent
    /// truncation would be invisible mail loss), skips blocked senders (#4), dedupes, maps with
    /// current-pin `contact`, and gates (#5). Returns the messages to emit, oldest-first.
    pub fn gap(&mut self, reader: &ArchiveReader, home: &Path, after_seq: i64)
        -> Result<Vec<Message>, rusqlite::Error>
    {
        let mut out = Vec::new();
        let mut since = after_seq;
        loop {
            let rows = reader.replay_since(since, PAGE_SIZE)?;
            let n = rows.len() as i64;
            for row in &rows {
                if is_blocked(home, &row.from) { continue; }          // #4
                if self.seen.contains(&row.envelope_id) { continue; }
                self.remember(&row.envelope_id.clone());
                let m = row_to_message(row, home);
                if channel_gate(&m, &self.mute) {                      // #5
                    out.push(m);
                }
            }
            if n < PAGE_SIZE { break; }                               // short page = end of hole
            since = rows.last().and_then(|r| r.relay_seq).unwrap_or(since);
        }
        Ok(out)
    }

    pub fn seen_size(&self) -> usize {
        self.seen.len()
    }
}
```

- [ ] **Step 2: Failing tests** — reuse the archive seeder by exercising replay invariants #4 + #5 on top of #1–#3. Create `crates/air-rs/tests/inbox_replay.rs`:

```rust
use air_rs::inbox::archive_reader::ArchiveReader;
use air_rs::inbox::frames::Message;
use air_rs::inbox::replay::Replayer;
use rusqlite::Connection;
use std::collections::HashSet;
use std::fs;
use tempfile::TempDir;

/// Archive with: e1 received from a PINNED+verified peer (admitted), e2 from an UNVERIFIED peer
/// (gate #5 rejects), e3 from a peer about to be BLOCKED (#4), e5 :joined (SQL #3), e6 spam (#2).
fn seed(home: &TempDir) {
    let conn = Connection::open(home.path().join("archive.db")).unwrap();
    conn.pragma_update(None, "journal_mode", "WAL").unwrap();
    conn.execute_batch(
        "CREATE TABLE messages (envelope_id TEXT,direction TEXT,thread_id TEXT,peer_did TEXT,from_did TEXT,to_did TEXT,timestamp TEXT,body_json TEXT,encrypted INT,verified INT,key_changed INT DEFAULT 0,relay_seq INT,spam INT DEFAULT 0,room_id TEXT,archived_at TEXT,PRIMARY KEY(envelope_id,direction));
         CREATE TABLE meta (key TEXT PRIMARY KEY,value TEXT NOT NULL);",
    ).unwrap();
    let mut ins = |env: &str, from: &str, verified: i64, seq: i64, spam: i64| {
        conn.execute(
            "INSERT INTO messages VALUES (?1,'received','th',?2,?2,'me',?3,'{\"type\":\"text\",\"text\":\"hi\"}',1,?4,0,?5,?6,NULL,?3)",
            rusqlite::params![env, from, format!("2026-06-11T00:00:0{seq}Z"), verified, seq, spam],
        ).unwrap();
    };
    ins("e1", "did:pinned", 1, 1, 0);
    ins("e2", "did:unverified", 0, 2, 0);
    ins("e3", "did:blocked", 1, 3, 0);
    ins("room1:joined", "did:pinned", 1, 4, 0);
    ins("e6", "did:pinned", 1, 5, 1);
    // contacts: pin did:pinned and did:blocked (so the gate would pass them on alias) ...
    fs::write(home.path().join("contacts.json"),
        r#"{"version":1,"contacts":{"did:pinned":{"alias":"pat"},"did:blocked":{"alias":"mal"}}}"#).unwrap();
    // ... but did:blocked is on the blocklist (invariant #4 must drop it even though it is pinned).
    fs::write(home.path().join("blocklist.json"),
        r#"{"version":1,"blocked":{"did:blocked":{"air_id":"AIR-MAL"}}}"#).unwrap();
}

#[test]
fn gap_replay_applies_all_five_invariants() {
    let home = TempDir::new().unwrap();
    seed(&home);
    let reader = ArchiveReader::open(home.path()).unwrap();
    let mut r = Replayer::new(HashSet::new());
    let out = r.gap(&reader, home.path(), 0).unwrap();
    let ids: Vec<_> = out.iter().map(|m| m.envelope_id.as_str()).collect();
    // Only e1 survives: e2 unverified (#5), e3 blocked (#4), room1:joined (#3), e6 spam (#2);
    // sent rows (#1) don't exist here. e1's contact is the CURRENT pin alias "pat".
    assert_eq!(ids, vec!["e1"]);
    assert_eq!(out[0].contact.as_deref(), Some("pat"));
}

#[test]
fn dedup_prevents_double_push_across_live_and_replay() {
    let home = TempDir::new().unwrap();
    seed(&home);
    let reader = ArchiveReader::open(home.path()).unwrap();
    let mut r = Replayer::new(HashSet::new());
    // Live delivers e1 first ...
    let live = Message {
        seq: 1, relay_seq: 1, envelope_id: "e1".into(), from: "did:pinned".into(),
        verified: true, encrypted: true, received_at: "t".into(), contact: Some("pat".into()),
        key_changed: None, thread_id: None, room_id: None, body: None,
    };
    assert!(r.live(live).is_some());
    // ... then a gap replays the same window — e1 must NOT be re-emitted.
    let out = r.gap(&reader, home.path(), 0).unwrap();
    assert!(out.iter().all(|m| m.envelope_id != "e1"));
}

#[test]
fn unpinned_after_receipt_is_withheld_on_replay() {
    // Invariant #5 "currently-pinned": delete the pin → the row loses its contact → gate rejects.
    let home = TempDir::new().unwrap();
    seed(&home);
    fs::write(home.path().join("contacts.json"), r#"{"version":1,"contacts":{}}"#).unwrap();
    let reader = ArchiveReader::open(home.path()).unwrap();
    let mut r = Replayer::new(HashSet::new());
    let out = r.gap(&reader, home.path(), 0).unwrap();
    assert!(out.is_empty(), "an unpinned-after-receipt sender must be withheld on replay");
}
```

- [ ] **Step 3: Run.** `cargo test -p air-rs --test inbox_replay` → 3 passed.
- [ ] **Step 4: Commit.** `git add crates/air-rs/src/inbox/replay.rs crates/air-rs/tests/inbox_replay.rs && git commit -m "feat(air-rs): replayer enforces all five replay invariants (incl. blocklist + current-pin gate)"`

---

### Task 8: Identity adopter (collision is the norm)

**Files:**
- Modify: `crates/air-rs/src/inbox/identity_adopter.rs`
- Test: `crates/air-rs/tests/inbox_identity_adopter.rs`

- [ ] **Step 1: Implement.** Replace `crates/air-rs/src/inbox/identity_adopter.rs`:

```rust
//! Identity adoption (design §4): one agent, many surfaces. If the daemon home has an identity, the
//! desktop ADOPTS it (did + name + DERIVED air_id) and reports any prior desktop-created identity as
//! dormant. `create_identity` MUST be disabled whenever a daemon home exists.
use crate::inbox::stores::{read_daemon_identity_meta, short_peer};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Adoption {
    /// The daemon identity is adopted. `dormant_did` is the desktop's prior self-created identity,
    /// if any (shown once as "now dormant").
    Adopted { did: String, air_id: String, name: Option<String>, dormant_did: Option<String> },
    /// No daemon identity anywhere → the desktop shows the install-the-CLI screen. Identity
    /// creation on a fresh machine is OUT OF SCOPE v1.
    NeedsDaemon,
}

/// Decide adoption from the daemon home + the desktop's own prior identity DID (if it created one).
pub fn adopt(home: &Path, desktop_prior_did: Option<&str>) -> Adoption {
    match read_daemon_identity_meta(home) {
        Some(meta) => {
            let air_id = short_peer(&meta.did);
            let dormant = match desktop_prior_did {
                Some(d) if d != meta.did => Some(d.to_string()),
                _ => None,
            };
            Adoption::Adopted { did: meta.did, air_id, name: meta.name, dormant_did: dormant }
        }
        None => Adoption::NeedsDaemon,
    }
}

/// `create_identity` gate: forbidden whenever a daemon home identity exists (design §4 — a "reset"
/// must not re-fork the split-brain).
pub fn creation_allowed(home: &Path) -> bool {
    read_daemon_identity_meta(home).is_none()
}
```

- [ ] **Step 2: Failing tests.** Create `crates/air-rs/tests/inbox_identity_adopter.rs`:

```rust
use air_rs::inbox::identity_adopter::{adopt, creation_allowed, Adoption};
use std::fs;
use tempfile::TempDir;

fn seed_identity(home: &TempDir, did: &str, name: &str) {
    fs::write(home.path().join("identity.json"),
        format!(r#"{{"version":1,"name":"{name}","air_id":"ignored","did":"{did}","seed_hex":"SECRET","agent_secret":"SECRET"}}"#)).unwrap();
}

#[test]
fn adopts_daemon_identity_and_derives_air_id() {
    let h = TempDir::new().unwrap();
    seed_identity(&h, "did:wba:x:agents:AIR-2JE0-EM7W-JNBK", "peters-agent");
    match adopt(h.path(), None) {
        Adoption::Adopted { did, air_id, name, dormant_did } => {
            assert_eq!(did, "did:wba:x:agents:AIR-2JE0-EM7W-JNBK");
            assert_eq!(air_id, "AIR-2JE0-EM7W-JNBK"); // DERIVED, not read from the file's air_id
            assert_eq!(name.as_deref(), Some("peters-agent"));
            assert!(dormant_did.is_none());
        }
        _ => panic!("expected adoption"),
    }
}

#[test]
fn reports_prior_desktop_identity_as_dormant() {
    let h = TempDir::new().unwrap();
    seed_identity(&h, "did:daemon:AIR-NEW", "agent");
    match adopt(h.path(), Some("did:desktop:AIR-OLD")) {
        Adoption::Adopted { dormant_did, .. } => assert_eq!(dormant_did.as_deref(), Some("did:desktop:AIR-OLD")),
        _ => panic!(),
    }
    // Same DID → not dormant (the desktop already points at the daemon identity).
    match adopt(h.path(), Some("did:daemon:AIR-NEW")) {
        Adoption::Adopted { dormant_did, .. } => assert!(dormant_did.is_none()),
        _ => panic!(),
    }
}

#[test]
fn no_daemon_identity_needs_daemon_and_allows_creation() {
    let h = TempDir::new().unwrap();
    assert_eq!(adopt(h.path(), None), Adoption::NeedsDaemon);
    assert!(creation_allowed(h.path()));
}

#[test]
fn creation_forbidden_when_daemon_exists() {
    let h = TempDir::new().unwrap();
    seed_identity(&h, "did:daemon:AIR-X", "agent");
    assert!(!creation_allowed(h.path()));
}
```

- [ ] **Step 3: Run.** `cargo test -p air-rs --test inbox_identity_adopter` → 4 passed.
- [ ] **Step 4: Commit.** `git add crates/air-rs/src/inbox/identity_adopter.rs crates/air-rs/tests/inbox_identity_adopter.rs && git commit -m "feat(air-rs): identity adopter (collision-as-norm, derived air_id, creation gate)"`

---

### Task 9: Policy store (the per-contact dial)

**Files:**
- Modify: `crates/air-rs/src/inbox/policy_store.rs`
- Test: `crates/air-rs/tests/inbox_policy_store.rs`

> A2 builds the STORE; Phase B consumes the dial. The store is the desktop's ONLY writer (`agent-policy.json`, 0600); a corrupt/missing file reads as all-draft (safe).

- [ ] **Step 1: Implement.** Replace `crates/air-rs/src/inbox/policy_store.rs`:

```rust
//! `{home}/agent-policy.json` — the per-contact autonomy dial (design §7). Written ONLY by the
//! desktop (one writer per file; `contacts.json` is the CLI's). Corrupt/missing → all draft.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Autonomy {
    Off,
    #[default]
    Draft,
    Auto,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContactPolicy {
    #[serde(default)]
    pub ai_autonomy: Autonomy,
    /// Auto-sent envelope_ids (Phase B loop guard); A2 just round-trips it.
    #[serde(default)]
    pub auto_ledger: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Policy {
    pub version: u32,
    #[serde(default)]
    pub contacts: HashMap<String, ContactPolicy>,
}

impl Default for Policy {
    fn default() -> Self {
        Self { version: 1, contacts: HashMap::new() }
    }
}

fn policy_path(home: &Path) -> std::path::PathBuf {
    home.join("agent-policy.json")
}

/// Read the policy; a missing OR corrupt file yields the safe default (everything = draft).
pub fn load(home: &Path) -> Policy {
    std::fs::read_to_string(policy_path(home))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// The dial for one contact — default `Draft` when absent (design §7).
pub fn autonomy_for(home: &Path, did: &str) -> Autonomy {
    load(home).contacts.get(did).map(|c| c.ai_autonomy).unwrap_or_default()
}

/// Set a contact's dial and persist 0600. Returns the written Policy.
pub fn set_autonomy(home: &Path, did: &str, value: Autonomy) -> std::io::Result<Policy> {
    let mut p = load(home);
    p.contacts.entry(did.to_string()).or_default().ai_autonomy = value;
    write_atomic(home, &p)?;
    Ok(p)
}

fn write_atomic(home: &Path, p: &Policy) -> std::io::Result<()> {
    std::fs::create_dir_all(home)?;
    let path = policy_path(home);
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(p).expect("policy serializes");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(json.as_bytes())?;
        f.flush()?;
    }
    set_0600(&tmp);
    std::fs::rename(&tmp, &path)?;
    set_0600(&path);
    Ok(())
}

#[cfg(unix)]
fn set_0600(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}
#[cfg(not(unix))]
fn set_0600(_path: &Path) {}
```

- [ ] **Step 2: Failing tests.** Create `crates/air-rs/tests/inbox_policy_store.rs`:

```rust
use air_rs::inbox::policy_store::{autonomy_for, load, set_autonomy, Autonomy};
use std::fs;
use tempfile::TempDir;

#[test]
fn missing_file_is_all_draft() {
    let h = TempDir::new().unwrap();
    assert_eq!(autonomy_for(h.path(), "did:x"), Autonomy::Draft);
}

#[test]
fn corrupt_file_is_all_draft() {
    let h = TempDir::new().unwrap();
    fs::write(h.path().join("agent-policy.json"), "{ not json").unwrap();
    assert_eq!(autonomy_for(h.path(), "did:x"), Autonomy::Draft);
}

#[test]
fn set_then_read_round_trips_and_persists_0600() {
    let h = TempDir::new().unwrap();
    set_autonomy(h.path(), "did:x", Autonomy::Auto).unwrap();
    assert_eq!(autonomy_for(h.path(), "did:x"), Autonomy::Auto);
    assert_eq!(autonomy_for(h.path(), "did:other"), Autonomy::Draft); // untouched = default
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(h.path().join("agent-policy.json")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    // The stored JSON uses lowercase enum values (stable wire form for future surfaces).
    let raw = fs::read_to_string(h.path().join("agent-policy.json")).unwrap();
    assert!(raw.contains("\"auto\""));
    let _ = load(h.path());
}
```

- [ ] **Step 3: Run.** `cargo test -p air-rs --test inbox_policy_store` → 3 passed.
- [ ] **Step 4: Commit.** `git add crates/air-rs/src/inbox/policy_store.rs crates/air-rs/tests/inbox_policy_store.rs && git commit -m "feat(air-rs): per-contact autonomy dial store (safe-default draft, 0600)"`

---

### Task 10: The reconnecting daemon client

**Files:**
- Modify: `crates/air-rs/src/inbox/client.rs`
- Test: `crates/air-rs/tests/inbox_client.rs`

> Ports `connectDaemon` + `connectDaemonPersistent`. The Rust shape uses a `tokio::mpsc` event channel + an `Arc<AtomicBool>` stop flag + a `Notify` to break the backoff sleep promptly — preserving every semantic: backoff 500→×2→5000 reset-on-attach, `maxSeen`/`baseline` resume, first-attach-no-`since_seq`, stop re-checked after every await, attach/detach surfaced as events, 3 s handshake timeout.

- [ ] **Step 1: Implement.** Replace `crates/air-rs/src/inbox/client.rs`:

```rust
//! Reconnecting daemon-socket client (ports daemon-ipc.mjs connectDaemon[Persistent]).
use crate::inbox::frames::{ClientFrame, Message, ServerFrame};
use crate::inbox::line_parser::{FrameEvent, LineParser};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, Notify};

pub const INITIAL_BACKOFF: Duration = Duration::from_millis(500);
pub const BACKOFF_CAP: Duration = Duration::from_millis(5000);
pub const HANDSHAKE: Duration = Duration::from_millis(3000);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Viewer,
    Channel,
}

impl Role {
    fn wire(self) -> &'static str {
        match self { Role::Viewer => "viewer", Role::Channel => "channel" }
    }
}

/// What the client surfaces to the caller. The caller (Tauri layer) forwards these as events and,
/// for the Channel role, drives the replayer on `Gap`.
#[derive(Debug, Clone)]
pub enum InboxEvent {
    Attached { pid: i64, did: String },
    Detached,
    /// The daemon is unreachable — emitted ONCE per outage streak when an attach attempt fails and
    /// we were not already signalled offline (JS `connectDaemonPersistent` rejects `DAEMON_DOWN` on
    /// first attach; design §5/§8 "daemon offline — reconnecting"). Cleared on the next `Attached`.
    Offline,
    Message(Message),
    Gap { after_seq: i64 },
    SendOk { id: String, envelope_id: String, encrypted: bool },
    SendErr { id: String, retryable: bool, reason: String },
    /// Raw status frame as JSON (rarely needed; kept simple).
    Status(Value),
}

/// Control handle: send frames to the daemon, or stop the client.
pub struct ClientHandle {
    stop: Arc<AtomicBool>,
    wake: Arc<Notify>,
    tx_out: mpsc::UnboundedSender<ClientFrame>,
}

impl ClientHandle {
    pub fn send_frame(&self, f: ClientFrame) {
        let _ = self.tx_out.send(f);
    }
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.wake.notify_waiters();
    }
}

pub struct ClientConfig {
    pub socket_path: PathBuf,
    pub role: Role,
    /// Baseline resume cursor captured BEFORE the first connect (the archive cursor, or None).
    /// Mirrors `cursorFn()` snapshotted pre-first-attach.
    pub baseline: Option<i64>,
}

/// Spawn the persistent client on the caller's tokio runtime. Returns immediately with a handle.
pub fn connect_persistent(cfg: ClientConfig, events: mpsc::UnboundedSender<InboxEvent>) -> ClientHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let wake = Arc::new(Notify::new());
    let (tx_out, rx_out) = mpsc::unbounded_channel::<ClientFrame>();
    let handle = ClientHandle { stop: stop.clone(), wake: wake.clone(), tx_out };
    tokio::spawn(reconnect_loop(cfg, events, stop, wake, rx_out));
    handle
}

async fn reconnect_loop(
    cfg: ClientConfig,
    events: mpsc::UnboundedSender<InboxEvent>,
    stop: Arc<AtomicBool>,
    wake: Arc<Notify>,
    mut rx_out: mpsc::UnboundedReceiver<ClientFrame>,
) {
    let mut max_seen: Option<i64> = None;
    let mut backoff = INITIAL_BACKOFF;
    let mut first = true;
    let mut offline_signaled = false;

    while !stop.load(Ordering::SeqCst) {
        if !first {
            // Sleep BEFORE retrying (onClose means the daemon is gone now). Break early on stop.
            tokio::select! {
                _ = tokio::time::sleep(backoff) => {}
                _ = wake.notified() => {}
            }
            if stop.load(Ordering::SeqCst) { return; }
        }
        // First attach sends NO since_seq; resume uses max_seen ?? baseline.
        let since = if first { None } else { max_seen.or(cfg.baseline) };
        first = false;

        match connect_once(&cfg, since, &events, &stop, &wake, &mut rx_out, &mut max_seen).await {
            ConnOutcome::Stopped => return,
            ConnOutcome::Attached => {
                backoff = INITIAL_BACKOFF;     // reset-on-attach
                offline_signaled = false;      // a future outage may re-signal offline
                let _ = events.send(InboxEvent::Detached);
            }
            ConnOutcome::FailedToConnect => {
                if !offline_signaled {
                    let _ = events.send(InboxEvent::Offline); // surface once per outage streak (§5/§8)
                    offline_signaled = true;
                }
                backoff = (backoff * 2).min(BACKOFF_CAP);
            }
        }
    }
}

enum ConnOutcome {
    Stopped,
    Attached,        // we attached then the connection closed (reconnect)
    FailedToConnect, // never attached (daemon down) — back off harder
}

async fn connect_once(
    cfg: &ClientConfig,
    since: Option<i64>,
    events: &mpsc::UnboundedSender<InboxEvent>,
    stop: &Arc<AtomicBool>,
    wake: &Arc<Notify>,
    rx_out: &mut mpsc::UnboundedReceiver<ClientFrame>,
    max_seen: &mut Option<i64>,
) -> ConnOutcome {
    let stream = match UnixStream::connect(&cfg.socket_path).await {
        Ok(s) => s,
        Err(_) => return ConnOutcome::FailedToConnect,
    };
    let (rd, mut wr) = stream.into_split();
    let mut reader = BufReader::new(rd);

    // Handshake: send hello, await hello-ok within HANDSHAKE.
    let hello = ClientFrame::Hello { role: cfg.role.wire().to_string(), since_seq: since };
    if write_frame(&mut wr, &hello).await.is_err() {
        return ConnOutcome::FailedToConnect;
    }
    let mut parser = LineParser::new();
    let mut buf = Vec::new();
    let attached = tokio::time::timeout(HANDSHAKE, async {
        loop {
            buf.clear();
            let n = reader.read_until(b'\n', &mut buf).await.ok()?;
            if n == 0 { return None; } // EOF before hello-ok
            for ev in parser.feed(&buf) {
                if let FrameEvent::Frame(v) = ev {
                    match serde_json::from_value::<ServerFrame>(v) {
                        Ok(ServerFrame::HelloOk { pid, did, .. }) => return Some((pid, did)),
                        Ok(_) | Err(_) => return None, // any non-hello-ok first frame = refusal
                    }
                }
            }
        }
    })
    .await
    .ok()
    .flatten();

    let (pid, did) = match attached {
        Some(v) => v,
        None => return ConnOutcome::FailedToConnect,
    };
    if stop.load(Ordering::SeqCst) { return ConnOutcome::Stopped; }
    let _ = events.send(InboxEvent::Attached { pid, did });

    // Post-hello: pump inbound frames + outbound send requests until close or stop.
    loop {
        buf.clear();
        tokio::select! {
            biased;
            _ = wake.notified() => {
                if stop.load(Ordering::SeqCst) { return ConnOutcome::Stopped; }
            }
            out = rx_out.recv() => {
                match out {
                    Some(frame) => { if write_frame(&mut wr, &frame).await.is_err() { return ConnOutcome::Attached; } }
                    None => { /* handle dropped — keep reading */ }
                }
            }
            read = reader.read_until(b'\n', &mut buf) => {
                let n = match read { Ok(n) => n, Err(_) => return ConnOutcome::Attached };
                if n == 0 { return ConnOutcome::Attached; } // daemon closed → reconnect
                for ev in parser.feed(&buf) {
                    if let FrameEvent::Frame(v) = ev {
                        dispatch(v, events, max_seen);
                    }
                }
                if stop.load(Ordering::SeqCst) { return ConnOutcome::Stopped; }
            }
        }
    }
}

fn dispatch(v: Value, events: &mpsc::UnboundedSender<InboxEvent>, max_seen: &mut Option<i64>) {
    let frame: ServerFrame = match serde_json::from_value(v) { Ok(f) => f, Err(_) => return };
    match frame {
        ServerFrame::Message { message } => {
            if max_seen.map_or(true, |m| message.relay_seq > m) {
                *max_seen = Some(message.relay_seq);
            }
            let _ = events.send(InboxEvent::Message(message));
        }
        ServerFrame::Gap { after_seq } => { let _ = events.send(InboxEvent::Gap { after_seq }); }
        ServerFrame::SendOk { id, envelope_id, encrypted } =>
            { let _ = events.send(InboxEvent::SendOk { id, envelope_id, encrypted }); }
        ServerFrame::SendErr { id, retryable, reason } =>
            { let _ = events.send(InboxEvent::SendErr { id, retryable, reason }); }
        ServerFrame::Status { .. } => { /* surfaced rarely; omitted for brevity */ }
        ServerFrame::Pong | ServerFrame::HelloOk { .. } | ServerFrame::Error { .. } | ServerFrame::Unknown => {}
    }
}

async fn write_frame<W: AsyncWriteExt + Unpin>(w: &mut W, f: &ClientFrame) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(f).expect("frame serializes");
    line.push(b'\n');
    w.write_all(&line).await?;
    w.flush().await
}
```

- [ ] **Step 2: A fake daemon for tests.** The integration test binds a `UnixListener`, completes the handshake, and scripts frames — so reconnect/resume is exercised hermetically. Create `crates/air-rs/tests/inbox_client.rs`:

```rust
use air_rs::inbox::client::{connect_persistent, ClientConfig, InboxEvent, Role};
use air_rs::inbox::frames::ClientFrame;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::mpsc;

async fn read_line(reader: &mut (impl AsyncBufReadExt + Unpin)) -> Value {
    let mut buf = Vec::new();
    reader.read_until(b'\n', &mut buf).await.unwrap();
    serde_json::from_slice(&buf).unwrap()
}

#[tokio::test]
async fn attaches_sends_and_receives() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("daemon.sock");
    let listener = UnixListener::bind(&sock).unwrap();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (rd, mut wr) = stream.into_split();
        let mut reader = BufReader::new(rd);
        let hello = read_line(&mut reader).await;
        assert_eq!(hello["type"], "hello");
        assert_eq!(hello["role"], "viewer");
        assert!(hello.get("since_seq").is_none(), "first attach sends no since_seq");
        wr.write_all(b"{\"type\":\"hello-ok\",\"pid\":4242,\"start_time\":\"t\",\"did\":\"did:me\"}\n").await.unwrap();
        // Push a message frame.
        wr.write_all(b"{\"type\":\"message\",\"message\":{\"seq\":7,\"relay_seq\":7,\"envelope_id\":\"e7\",\"from\":\"did:peer\",\"verified\":true,\"encrypted\":true,\"received_at\":\"t\"}}\n").await.unwrap();
        // Expect a send frame from the client, then ack it.
        let send = read_line(&mut reader).await;
        assert_eq!(send["type"], "send");
        let id = send["id"].as_str().unwrap();
        wr.write_all(format!("{{\"type\":\"send-ok\",\"id\":\"{id}\",\"envelope_id\":\"relay-1\",\"encrypted\":true}}\n").as_bytes()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await; // hold the socket open
    });

    let (tx, mut rx) = mpsc::unbounded_channel::<InboxEvent>();
    let handle = connect_persistent(ClientConfig { socket_path: sock, role: Role::Viewer, baseline: None }, tx);

    // Attached.
    assert!(matches!(rx.recv().await.unwrap(), InboxEvent::Attached { pid: 4242, .. }));
    // Message.
    match rx.recv().await.unwrap() { InboxEvent::Message(m) => assert_eq!(m.envelope_id, "e7"), e => panic!("{e:?}") }
    // Send + ack.
    handle.send_frame(ClientFrame::Send { id: "corr-1".into(), to: "did:peer".into(), body: json!({"type":"text","text":"hi"}), plaintext: None, thread_id: None, in_reply_to: None });
    loop {
        match rx.recv().await.unwrap() {
            InboxEvent::SendOk { id, envelope_id, .. } => { assert_eq!(id, "corr-1"); assert_eq!(envelope_id, "relay-1"); break; }
            _ => continue,
        }
    }
    handle.stop();
    let _ = server.await;
}

#[tokio::test]
async fn resumes_with_since_seq_after_a_reconnect() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("daemon.sock");
    let listener = UnixListener::bind(&sock).unwrap();

    let server = tokio::spawn(async move {
        // First attach: no since_seq; deliver relay_seq=7; then DROP the connection.
        let (s1, _) = listener.accept().await.unwrap();
        let (rd, mut wr) = s1.into_split();
        let mut reader = BufReader::new(rd);
        let hello1 = read_line(&mut reader).await;
        assert!(hello1.get("since_seq").is_none());
        wr.write_all(b"{\"type\":\"hello-ok\",\"pid\":1,\"start_time\":\"t\",\"did\":\"did:me\"}\n").await.unwrap();
        wr.write_all(b"{\"type\":\"message\",\"message\":{\"seq\":7,\"relay_seq\":7,\"envelope_id\":\"e7\",\"from\":\"did:peer\",\"verified\":true,\"encrypted\":true,\"received_at\":\"t\"}}\n").await.unwrap();
        drop(wr); drop(reader); // force reconnect

        // Second attach: MUST carry since_seq = 7 (max relay_seq seen).
        let (s2, _) = listener.accept().await.unwrap();
        let (rd2, mut wr2) = s2.into_split();
        let mut reader2 = BufReader::new(rd2);
        let hello2 = read_line(&mut reader2).await;
        assert_eq!(hello2["since_seq"], json!(7), "resume must send max-seen since_seq");
        wr2.write_all(b"{\"type\":\"hello-ok\",\"pid\":1,\"start_time\":\"t\",\"did\":\"did:me\"}\n").await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
    });

    let (tx, mut rx) = mpsc::unbounded_channel::<InboxEvent>();
    let _h = connect_persistent(ClientConfig { socket_path: sock, role: Role::Channel, baseline: Some(3) }, tx);
    // Drain until the second Attached arrives (proves the resume handshake the server asserted).
    let mut attaches = 0;
    while let Some(ev) = rx.recv().await {
        if let InboxEvent::Attached { .. } = ev { attaches += 1; if attaches == 2 { break; } }
    }
    assert_eq!(attaches, 2);
    let _ = server.await; // server's since_seq assertion is the real check
}

#[tokio::test]
async fn signals_offline_when_no_daemon_is_listening() {
    // Critic M3: a first-attach failure must surface as an Offline EVENT, not silence — the design
    // §8 "daemon offline — reconnecting" banner binds to it.
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("daemon.sock"); // nothing binds this path
    let (tx, mut rx) = mpsc::unbounded_channel::<InboxEvent>();
    let handle = connect_persistent(ClientConfig { socket_path: sock, role: Role::Viewer, baseline: None }, tx);
    let ev = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await.expect("offline within 2s").unwrap();
    assert!(matches!(ev, InboxEvent::Offline), "expected Offline, got {ev:?}");
    handle.stop();
}
```

- [ ] **Step 3: Run.** `cargo test -p air-rs --test inbox_client` → 3 passed. (If `resumes_with_since_seq` is flaky, the backoff sleep before the 2nd attach is the cause — the test already tolerates it by draining; do not shorten BACKOFF in the library.)
- [ ] **Step 4: Run the WHOLE crate.** `cargo test -p air-rs` → all green (16 prior + the new inbox tests). **Milestone 1 done: the air-rs inbox library is green.**
- [ ] **Step 5: Commit.** `git add crates/air-rs/src/inbox/client.rs crates/air-rs/tests/inbox_client.rs && git commit -m "feat(air-rs): reconnecting two-role daemon client (resume, send, gap surfacing)"`

---

## MILESTONE 2 — the Tauri surface (green via `cargo check -p bossclaw_desktop`)

> A2's Tauri layer wires the **viewer** connection (the inbox live feed), send+ack, archive history, adopted identity, and the dial store — everything the A3 React inbox needs. The channel connection + replayer (Milestone 1) are library-tested and await their Phase B consumer.

### Task 11: InboxManager state

**Files:**
- Create: `apps/desktop/src-tauri/src/inbox/mod.rs`
- Create: `apps/desktop/src-tauri/src/inbox/manager.rs`
- Modify: `apps/desktop/src-tauri/src/main.rs` (module decl + AppState field + setup)
- Modify: `apps/desktop/src-tauri/src/commands/identity.rs` (AppState struct — add the field)

- [ ] **Step 1: Manager skeleton.** Create `apps/desktop/src-tauri/src/inbox/mod.rs`:

```rust
pub mod manager;
```

Create `apps/desktop/src-tauri/src/inbox/manager.rs`:

```rust
//! Owns the desktop's live viewer connection. One per app.
use air_rs::inbox::client::ClientHandle;
use std::sync::Mutex;

#[derive(Default)]
pub struct InboxManager {
    /// The live viewer client (None until inbox_start). Stopped on app exit / inbox_stop. Send acks
    /// round-trip to the UI as Tauri events, so no correlation map is kept here (critic m2).
    pub viewer: Mutex<Option<ClientHandle>>,
}

impl InboxManager {
    pub fn new() -> Self {
        Self::default()
    }
}
```

- [ ] **Step 2: Add the field to AppState.** In `apps/desktop/src-tauri/src/commands/identity.rs`, extend the struct (keep the existing two fields; do NOT keep the stray `#[tauri::command]` attribute if present — it is wrong on a struct):

```rust
pub struct AppState {
    pub air_client: std::sync::Arc<dyn crate::air::AirClient>,
    pub identity_store: crate::air::identity::IdentityStore,
    pub inbox: std::sync::Arc<crate::inbox::manager::InboxManager>,
}
```

- [ ] **Step 3: Declare the module + initialize state.** In `apps/desktop/src-tauri/src/main.rs`, add `mod inbox;` with the other module decls, and in `.setup(...)` add the field to the `app.manage(AppState { ... })` literal:

```rust
            app.manage(AppState {
                air_client,
                identity_store,
                inbox: std::sync::Arc::new(crate::inbox::manager::InboxManager::new()),
            });
```

- [ ] **Step 4: Compile.** `cd ~/air-note && cargo check -p bossclaw_desktop`
Expected: builds (no commands wired yet). If `AppState` is constructed elsewhere (tests), add the field there too — grep `AppState {`.
- [ ] **Step 5: Commit.** `git add apps/desktop/src-tauri/src/inbox apps/desktop/src-tauri/src/main.rs apps/desktop/src-tauri/src/commands/identity.rs && git commit -m "feat(desktop): InboxManager app state"`

---

### Task 12: Tauri commands + events

**Files:**
- Create: `apps/desktop/src-tauri/src/commands/inbox.rs`
- Modify: `apps/desktop/src-tauri/src/commands/mod.rs` (or wherever command modules are declared)
- Modify: `apps/desktop/src-tauri/src/main.rs` (`generate_handler!`)

- [ ] **Step 1: Commands.** Create `apps/desktop/src-tauri/src/commands/inbox.rs`:

```rust
//! Tauri command surface for the inbox (design §3/§6/§8). The viewer connection feeds live events;
//! send goes over the socket and its ack returns as an event; history/identity/policy are reads.
use crate::commands::identity::AppState;
use air_rs::inbox::archive_reader::ArchiveReader;
use air_rs::inbox::client::{connect_persistent, ClientConfig, ClientHandle, InboxEvent, Role};
use air_rs::inbox::frames::ClientFrame;
use air_rs::inbox::identity_adopter::{adopt, Adoption};
use air_rs::inbox::policy_store::{autonomy_for, set_autonomy, Autonomy};
use air_rs::inbox::bridge_home;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, State};
use tokio::sync::mpsc;

/// Probe whether a daemon socket + identity exist (drives the "install the CLI" screen).
#[tauri::command]
pub async fn inbox_status() -> Result<Value, String> {
    let home = bridge_home();
    Ok(json!({
        "home": home.to_string_lossy(),
        "socket_exists": home.join("daemon.sock").exists(),
        "identity_exists": home.join("identity.json").exists(),
        "archive_exists": home.join("archive.db").exists(),
    }))
}

/// The adopted identity (collision-as-norm). `desktop_prior_did` is the desktop's own legacy id, if any.
#[tauri::command]
pub async fn inbox_identity(desktop_prior_did: Option<String>) -> Result<Adoption, String> {
    Ok(adopt(&bridge_home(), desktop_prior_did.as_deref()))
}

/// Start the live viewer connection. Idempotent: a second call is a no-op while connected.
#[tauri::command]
pub async fn inbox_start(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    {
        let guard = state.inbox.viewer.lock().map_err(|_| "inbox lock".to_string())?;
        if guard.is_some() {
            return Ok(()); // already running
        }
    }
    let home = bridge_home();
    let (tx, mut rx) = mpsc::unbounded_channel::<InboxEvent>();
    let handle: ClientHandle = connect_persistent(
        ClientConfig { socket_path: home.join("daemon.sock"), role: Role::Viewer, baseline: None },
        tx,
    );
    {
        let mut guard = state.inbox.viewer.lock().map_err(|_| "inbox lock".to_string())?;
        *guard = Some(handle);
    }
    // Forward client events → Tauri events. Runs until the channel closes (handle.stop()).
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(ev) = rx.recv().await {
            match ev {
                InboxEvent::Attached { pid, did } => { let _ = app2.emit("inbox_attached", json!({"pid": pid, "did": did})); }
                InboxEvent::Detached => { let _ = app2.emit("inbox_detached", json!({})); }
                InboxEvent::Offline => { let _ = app2.emit("inbox_offline", json!({})); } // §8 banner (critic M3)
                InboxEvent::Message(m) => { let _ = app2.emit("inbox_message", &m); }
                // Gap is channel-only (PROTOCOL §5); the viewer connection never receives it — no event (critic m, dead-arm).
                InboxEvent::Gap { .. } => {}
                InboxEvent::SendOk { id, envelope_id, encrypted } => { let _ = app2.emit("inbox_send_ok", json!({"id": id, "envelope_id": envelope_id, "encrypted": encrypted})); }
                InboxEvent::SendErr { id, retryable, reason } => { let _ = app2.emit("inbox_send_err", json!({"id": id, "retryable": retryable, "reason": reason})); }
                InboxEvent::Status(_) => {}
            }
        }
    });
    Ok(())
}

#[tauri::command]
pub async fn inbox_stop(state: State<'_, AppState>) -> Result<(), String> {
    let mut guard = state.inbox.viewer.lock().map_err(|_| "inbox lock".to_string())?;
    if let Some(h) = guard.take() {
        h.stop();
    }
    Ok(())
}

/// Send a message over the socket. Returns the correlation id immediately; the ack arrives as an
/// `inbox_send_ok` / `inbox_send_err` event (design §3/§8 — optimistic send, per-row ack).
#[tauri::command]
pub async fn inbox_send(
    state: State<'_, AppState>,
    to: String,
    body: Value,
    thread_id: Option<String>,
    in_reply_to: Option<String>,
) -> Result<String, String> {
    let id = uuid::Uuid::new_v4().to_string();
    let guard = state.inbox.viewer.lock().map_err(|_| "inbox lock".to_string())?;
    let handle = guard.as_ref().ok_or("inbox not connected — call inbox_start first")?;
    handle.send_frame(ClientFrame::Send {
        id: id.clone(), to, body, plaintext: None, thread_id, in_reply_to,
    });
    // The ack returns as an inbox_send_ok / inbox_send_err event keyed by this id — no server-side
    // correlation map (critic m2). The caller awaits the event.
    Ok(id)
}

/// Conversation list (newest-first) for the §6 sidebar — 1:1 keyed by peer_did, rooms by room_id.
#[tauri::command]
pub async fn inbox_conversations() -> Result<Value, String> {
    let home = bridge_home();
    if !home.join("archive.db").exists() {
        return Ok(json!([])); // never materialize a DB just to read an empty list
    }
    // ArchiveReader::open has a bounded blocking busy-retry — run it off the async worker (critic m3).
    tauri::async_runtime::spawn_blocking(move || -> Result<Value, String> {
        let reader = ArchiveReader::open(&home).map_err(|e| e.to_string())?;
        let convs = reader.conversations().map_err(|e| e.to_string())?;
        serde_json::to_value(convs).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// History for one peer (or recent across peers when `peer` is None).
#[tauri::command]
pub async fn inbox_history(peer: Option<String>, limit: Option<i64>, include_spam: Option<bool>) -> Result<Value, String> {
    let home = bridge_home();
    if !home.join("archive.db").exists() {
        return Ok(json!([]));
    }
    tauri::async_runtime::spawn_blocking(move || -> Result<Value, String> {
        let reader = ArchiveReader::open(&home).map_err(|e| e.to_string())?;
        let rows = reader
            .history(peer.as_deref(), None, None, None, limit.unwrap_or(50), include_spam.unwrap_or(false))
            .map_err(|e| e.to_string())?;
        serde_json::to_value(rows).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn inbox_policy_get(did: String) -> Result<String, String> {
    Ok(match autonomy_for(&bridge_home(), &did) {
        Autonomy::Off => "off", Autonomy::Draft => "draft", Autonomy::Auto => "auto",
    }.to_string())
}

#[tauri::command]
pub async fn inbox_policy_set(did: String, value: String) -> Result<(), String> {
    let v = match value.as_str() {
        "off" => Autonomy::Off, "draft" => Autonomy::Draft, "auto" => Autonomy::Auto,
        other => return Err(format!("unknown autonomy '{other}'")),
    };
    set_autonomy(&bridge_home(), &did, v).map_err(|e| e.to_string())?;
    Ok(())
}
```

> **Note for the executor:** there are TWO `inbox` modules — `apps/desktop/src-tauri/src/inbox/` (the `InboxManager` state, Task 11) and `apps/desktop/src-tauri/src/commands/inbox.rs` (these commands). Keep them distinct: `crate::inbox::manager::InboxManager` is the managed state; `crate::commands::inbox::*` are the commands. `parse_mute_set` belongs to the Phase B channel path and is deliberately NOT imported here (critic — delete dead imports, never alias them).

- [ ] **Step 2: Register the module + commands.** Declare `pub mod inbox;` in `apps/desktop/src-tauri/src/commands/mod.rs` (it already declares `pub mod a2a;` + `pub mod identity;` — add alongside). Then add to `tauri::generate_handler![ ... ]` in `main.rs`:

```rust
            commands::inbox::inbox_status,
            commands::inbox::inbox_identity,
            commands::inbox::inbox_start,
            commands::inbox::inbox_stop,
            commands::inbox::inbox_send,
            commands::inbox::inbox_conversations,
            commands::inbox::inbox_history,
            commands::inbox::inbox_policy_get,
            commands::inbox::inbox_policy_set,
```

(Match the existing path style — if commands are imported via `use`, follow that; if referenced as `module::fn`, follow that.)

- [ ] **Step 3: Compile.** `cargo check -p bossclaw_desktop`. Resolve any `Emitter` import (Tauri 2 needs `use tauri::Emitter;` for `app.emit`).
- [ ] **Step 4: Commit.** `git add apps/desktop/src-tauri/src/commands/inbox.rs apps/desktop/src-tauri/src/commands apps/desktop/src-tauri/src/main.rs && git commit -m "feat(desktop): inbox commands + event forwarding (viewer feed, send, history, identity, dial)"`

---

### Task 13: Whole-stack verification + docs

**Files:**
- Modify: `docs/superpowers/specs/2026-06-11-desktop-ai-inbox-design.md` (Status line)
- Modify: `agent-bridge-mcp/docs/PROTOCOL.md` (mark the Rust side of the contract LIVE)
- (No code — this task is the green-bar + paper trail.)

- [ ] **Step 1: Full Rust suite.** `cd ~/air-note && cargo test -p air-rs` → all green. Capture the count.
- [ ] **Step 2: Desktop backend compiles.** `cargo check -p bossclaw_desktop` → clean.
- [ ] **Step 3: Lint.** `cargo clippy -p air-rs --all-targets -- -D warnings` (fix any warning; the `_keep_mute_in_scope` smell from Task 12 must be resolved, not silenced).
- [ ] **Step 4: Manual real-daemon smoke (documented, NOT in the automated suite — hermeticity).** Record the steps in the plan's execution notes; run only with Peter's consent against a TEMP home (never `~/.air-msg`):

```bash
# In one shell: a daemon on a temp home.
export AGENT_BRIDGE_HOME=$(mktemp -d)
cd ~/air-note/agent-bridge-mcp && node src/cli.mjs daemon start --detach   # (after an identity exists in that home)
# Then a tiny Rust example or the desktop dev build connects as viewer, sends, and observes the ack.
```

- [ ] **Step 5: Update the design spec Status** to note Phase A2 (Rust backend) MERGED, listing the modules + green counts.
- [ ] **Step 6: Commit + open PR.**

```bash
cd ~/air-note
git add docs/superpowers/specs/2026-06-11-desktop-ai-inbox-design.md agent-bridge-mcp/docs/PROTOCOL.md
git commit -m "docs: Phase A2 Rust backend landed — contract now has both sides"
git push -u origin feat/ai-inbox-a2-backend
gh pr create --fill --title "Phase A2: Rust inbox backend (daemon-client + archive-reader + adopter + policy + Tauri surface)"
```

---

## Self-review + critic resolution

**Spec coverage** (design §-by-§): §3 architecture → Tasks 6/7/10/11/12; §4 identity adoption → Task 8; §5 connection lifecycle + archive reader (backoff/since_seq/WAL/bounded-retry/offline) → Tasks 6/10; §5 protocol parity (PROTOCOL.md + fixtures asserted by the Rust suite) → Tasks 2/3; §6 inbox grouping (1:1 by `peer_did`, rooms by `room_id`) + history → Task 6 (`conversations`/`history`) + Task 12 commands; §7 dial store → Task 9 (consumption is Phase B); §8 error handling (send-err inline; daemon-offline) → Task 12 events (`inbox_send_err`/`inbox_offline`) + Task 10 reconnect/`Offline`; §9 testing (fixtures, fake-socket reconnect/resume/offline, replay invariants, SAME- + SEPARATE-process archive soak, hermetic temp-home) → Tasks 2/6/7/10; §10 security (RO archive, replay≤live, no key in desktop, 0600 policy) → Tasks 6/7/8/9. **Deferred to Phase B (stated, not gaps):** the channel-connection Tauri wiring + cross-stream dedupe + the AI loop + prompt fencing (§7) — A2 builds & tests the channel client/replayer in the library but does not consume it.

**Placeholder scan:** none — every step carries runnable code or an exact command.

**Type consistency:** `Message`/`ClientFrame`/`ServerFrame` (frames.rs) used identically in gate/replay/client; `ArchiveRow` shared by archive_reader→replay; `Adoption`/`Autonomy`/`ConversationSummary` serialized the same way in lib + commands; `InboxEvent`/`ClientHandle`/`ClientConfig`/`Role` consistent across client.rs and commands/inbox.rs.

**Critic pass — APPROVE-WITH-CHANGES (all applied) + empirical probes:**
- **REFUTED by the critic compiling+running real code (verified-safe, NOT changed):** `#[serde(other)]` on the internally-tagged enum tolerates unknown `type`s; the `include_str!("../../../agent-bridge-mcp/...")` fixture path resolves correctly and ties the air-rs build to the in-repo fixture; the rusqlite 0.32 API (`query_map` with a boxed-param vec + `refs.as_slice()`, `pragma_update`, `OpenFlags`) compiles + runs; the resume/`since_seq` cursor math is faithful to `connectDaemonPersistent`.
- **EMPIRICALLY RETIRED (the headline risk):** WAL read-only from a SECOND PROCESS works — a `mode=ro` Python reader read a live WAL writer's fresh rows (counts 7→16, 0 errors, SQLite 3.51.0), and a separate-process rusqlite reader/writer pair logged `busy_errors=0` incl. reading un-checkpointed WAL frames. The design (`SQLITE_OPEN_READ_ONLY` + busy_timeout + bounded open-retry) stands; the daemon runs as the same user in a 0700 home, so the reader can write `-wal`/`-shm`.
- **FIXED — Major:** **M1** `inbox_threads`→`inbox_conversations` grouped by `peer_did`/`room_id` (the ported `threads()` grouped by `thread_id`, which §6 forbids — it fragments 1:1 conversations) + a collapse test; **M2** the read-during-write soak is now a genuine SEPARATE-process writer (+ a fast same-process check retained); **M3** `InboxEvent::Offline` + `inbox_offline` event + a no-listener test (§8's "daemon offline" banner had no event to bind to).
- **FIXED — minor:** stray `tests/..` git-add token removed; dead+unbounded `pending_sends` dropped; async archive reads moved to `spawn_blocking` (the bounded-retry `std::thread::sleep` must not block a tokio worker); dead viewer `inbox_gap` arm removed (gap is channel-only, PROTOCOL §5); the `parse_mute_set` import smell deleted; executor `src/inbox/`-vs-`src/commands/inbox.rs` disambiguation note added.

**Verify-at-execution (genuine, not yet run):** the full `cargo test -p air-rs` green count, `cargo check -p bossclaw_desktop`, and clippy `-D warnings` (Tasks 10/13). The separate-process soak self-skips without `python3`. One residual at-execution check (Task 11): grep that no other `AppState { ... }` literal (tests/alt entrypoints) needs the new `inbox` field.
