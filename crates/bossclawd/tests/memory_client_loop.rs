//! U4 — the safe read+write loop over the REAL daemon socket, driven as a `MemoryClient` (the same
//! role the `air-memory-mcp` adapter uses). Proves: (1) recall works, (2) remember→recall
//! round-trips, (3) a destructive op is refused. Hermetic engine, onboarded fixture. Unix-only.
#![cfg(unix)]

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use bossclawd::engine::EngineHandle;
use bossclawd::server;
use bossclawd_proto::{
    read_frame, write_frame, Hello, HelloOk, OpErrorKindWire, Request, Response, Role, PROTO_VERSION,
};
use tokio::net::UnixStream;
use tokio::sync::Mutex;

/// Serializes the tests that mutate the process-global `AIR_CLAUDE_PROJECTS_ROOT` so their fake
/// roots never race under cargo's parallel test threads (same convention as `tests/sweeper.rs`).
/// A tokio `Mutex` is await-safe, so it can be held across the guest round-trips.
static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
fn env_lock() -> &'static Mutex<()> {
    ENV_LOCK.get_or_init(|| Mutex::new(()))
}

fn now_epoch() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
}

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

/// Like [`spawn_onboarded_daemon`] but RETURNS the shared engine handle. The SP3 capture tests need
/// it for setup the guest role cannot do over the wire (`set_capture_enabled` is app-only) and to
/// read the signed fold (`current_sessions`) directly — proving an IMMEDIATE `CaptureNotify` capture
/// with NO sweeper running (this spawns only `run_accept_loop`, not `capture::sweeper::spawn`).
/// Mirrors `server::spawn_for_test`, but hands back the `Arc<EngineHandle>` it serves on.
async fn spawn_daemon_with_engine() -> (tempfile::TempDir, PathBuf, Arc<EngineHandle>) {
    use std::os::unix::fs::PermissionsExt;

    use tokio::net::UnixListener;

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
    let engine = Arc::new(server::test_engine(home.clone()));
    let sock = home.join("bossclawd.sock");
    let listener = UnixListener::bind(&sock).unwrap();
    std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600)).unwrap();
    tokio::spawn(server::run_accept_loop(engine.clone(), listener));
    (dir, sock, engine)
}

/// Write a minimal real-shape Claude Code transcript (ends with a newline → no torn tail), mirroring
/// `tests/sweeper.rs::write_transcript` so both capture paths render the same content shape.
fn write_transcript(path: &std::path::Path, prompt: &str) {
    let body = format!(
        "{}\n{}\n",
        serde_json::json!({
            "type": "user",
            "message": {"role": "user", "content": prompt},
            "timestamp": "2026-07-11T10:00:00.000Z"
        }),
        serde_json::json!({
            "type": "assistant",
            "message": {"role": "assistant", "content": [{"type": "text", "text": "Sure."}]},
            "timestamp": "2026-07-11T10:00:01.000Z"
        }),
    );
    std::fs::write(path, body).unwrap();
}

/// True iff `resp` is the per-connection capture rate-limit rejection (a `Rejected` whose message
/// names the rate limit) — distinct from a path/id `Rejected`, which carries a different message.
fn is_rate_limited(resp: &Response) -> bool {
    matches!(resp, Response::Err { kind: OpErrorKindWire::Rejected, message } if message.contains("rate limit"))
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

/// A10 §4c: a guest (`MemoryClient`) `CaptureNotify` renders the just-ended transcript to a signed
/// capture IMMEDIATELY — no sweeper runs here, so success proves the inline dispatch path. Confirms
/// the guest surface reaches CaptureNotify (the override + allowlist wiring) and that the store lands
/// on disk + in the signed fold the instant the poke returns.
#[tokio::test]
async fn capture_notify_is_guest_reachable_and_renders_immediately() {
    let _env = env_lock().lock().await; // serialize AIR_CLAUDE_PROJECTS_ROOT use.
    let (dir, sock, engine) = spawn_daemon_with_engine().await;

    // A fake projects root: <root>/<slug>/<session-id>.jsonl (Claude Code's layout).
    let projects = tempfile::tempdir().unwrap();
    let slug = "-Users-tester-repo";
    let proj_dir = projects.path().join(slug);
    std::fs::create_dir_all(&proj_dir).unwrap();
    let sid = "cap-immediate-1";
    let transcript = proj_dir.join(format!("{sid}.jsonl"));
    let prompt = "how does immediate capture work?";
    write_transcript(&transcript, prompt);

    // Consent to capture (app-only — done via the direct engine handle, not over the guest wire).
    engine.set_capture_enabled(true, true, false, now_epoch()).await.unwrap();
    std::env::set_var("AIR_CLAUDE_PROJECTS_ROOT", projects.path());

    let mut guest = Guest::connect(&sock).await;
    let resp = guest
        .call(Request::CaptureNotify {
            onboarded: true,
            session_id: sid.into(),
            transcript_path: transcript.to_string_lossy().into_owned(),
        })
        .await;
    assert!(matches!(resp, Response::Ok), "guest CaptureNotify renders immediately → Ok, got {resp:?}");

    // IMMEDIATELY (no sweep): the .md is on disk AND the signed fold holds the current capture.
    let md = dir.path().join(format!("sessions/{sid}.md"));
    assert!(md.exists(), "the capture .md exists the instant CaptureNotify returns (no sweeper ran)");
    let body = std::fs::read_to_string(&md).unwrap();
    assert!(body.contains(prompt), "the rendered body carries the prompt");
    let cur = engine.current_sessions().await.unwrap();
    assert_eq!(cur.len(), 1, "exactly the notified session is captured");
    assert_eq!(cur[0].session_id, sid);
    assert_eq!(cur[0].project, slug, "project = the transcript's slug parent dir (A9-identical)");
    assert_eq!(cur[0].tool, "claude-code");

    std::env::remove_var("AIR_CLAUDE_PROJECTS_ROOT");
}

/// A10 I4: every attacker-influenceable input is rejected BEFORE it can write — a bad session id
/// (fails the A5 allowlist), a path outside the projects root, and a non-`.jsonl` path. In EVERY
/// case the response is a clean `Rejected` (no hostile bytes) and NO session file is created.
#[tokio::test]
async fn capture_notify_rejects_traversal_and_bad_session_id() {
    let _env = env_lock().lock().await; // serialize AIR_CLAUDE_PROJECTS_ROOT use.
    let (dir, sock, engine) = spawn_daemon_with_engine().await;
    // Capture ENABLED so we pass the I10 no-op gate and actually reach validation.
    engine.set_capture_enabled(true, true, false, now_epoch()).await.unwrap();

    let projects = tempfile::tempdir().unwrap();
    let proj_dir = projects.path().join("-Users-tester-repo");
    std::fs::create_dir_all(&proj_dir).unwrap();
    std::fs::write(proj_dir.join("real.txt"), b"not a transcript").unwrap();
    std::env::set_var("AIR_CLAUDE_PROJECTS_ROOT", projects.path());

    let mut guest = Guest::connect(&sock).await;

    // (a) bad session id: `../evil` fails the A5 allowlist before any path is built.
    let bad_id = guest
        .call(Request::CaptureNotify {
            onboarded: true,
            session_id: "../evil".into(),
            transcript_path: proj_dir.join("whatever.jsonl").to_string_lossy().into_owned(),
        })
        .await;
    assert!(
        matches!(bad_id, Response::Err { kind: OpErrorKindWire::Rejected, .. }),
        "a bad session id → Rejected, got {bad_id:?}"
    );

    // (b) absolute path outside the projects root (/etc/passwd) → refused.
    let outside = guest
        .call(Request::CaptureNotify {
            onboarded: true,
            session_id: "good-sid".into(),
            transcript_path: "/etc/passwd".into(),
        })
        .await;
    assert!(
        matches!(outside, Response::Err { kind: OpErrorKindWire::Rejected, .. }),
        "a path outside the projects root → Rejected, got {outside:?}"
    );

    // (b2) a `.jsonl` OUTSIDE the root exercises the containment `strip_prefix` reject (not just the
    // extension gate) — a real file so the rejection is provably containment, never ENOENT.
    let escape = tempfile::tempdir().unwrap();
    let escape_jsonl = escape.path().join("escape.jsonl");
    std::fs::write(&escape_jsonl, b"{}\n").unwrap();
    let outside2 = guest
        .call(Request::CaptureNotify {
            onboarded: true,
            session_id: "good-sid".into(),
            transcript_path: escape_jsonl.to_string_lossy().into_owned(),
        })
        .await;
    assert!(
        matches!(outside2, Response::Err { kind: OpErrorKindWire::Rejected, .. }),
        "a .jsonl outside the root → containment Rejected, got {outside2:?}"
    );

    // (c) not a `.jsonl`: the lexical extension gate refuses it before any syscall.
    let not_jsonl = guest
        .call(Request::CaptureNotify {
            onboarded: true,
            session_id: "good-sid".into(),
            transcript_path: proj_dir.join("real.txt").to_string_lossy().into_owned(),
        })
        .await;
    assert!(
        matches!(not_jsonl, Response::Err { kind: OpErrorKindWire::Rejected, .. }),
        "a non-.jsonl path → Rejected, got {not_jsonl:?}"
    );

    // In EVERY rejection: NO capture file was created under sessions/.
    let sessions = dir.path().join("sessions");
    let files = if sessions.exists() {
        std::fs::read_dir(&sessions).unwrap().count()
    } else {
        0
    };
    assert_eq!(files, 0, "a rejected CaptureNotify writes no session file");

    std::env::remove_var("AIR_CLAUDE_PROJECTS_ROOT");
}

/// A10 I10: a `CaptureNotify` on a brain that has NOT consented to capture is a SILENT no-op success
/// — even with a perfectly VALID, confined transcript, NOTHING is written (no `sessions/` dir, no
/// signed event). Isolates the capture-disabled gate: the transcript is real and in-root, so absence
/// of a capture can only be the gate (not a path reject).
#[tokio::test]
async fn capture_notify_disabled_is_a_silent_no_op() {
    let _env = env_lock().lock().await; // serialize AIR_CLAUDE_PROJECTS_ROOT use.
    let (dir, sock, engine) = spawn_daemon_with_engine().await;
    // Capture is LEFT DISABLED (fresh brain default OFF) — no set_capture_enabled.

    let projects = tempfile::tempdir().unwrap();
    let proj_dir = projects.path().join("-Users-tester-repo");
    std::fs::create_dir_all(&proj_dir).unwrap();
    let sid = "cap-disabled-1";
    let transcript = proj_dir.join(format!("{sid}.jsonl"));
    write_transcript(&transcript, "this must not be captured while disabled");
    std::env::set_var("AIR_CLAUDE_PROJECTS_ROOT", projects.path());

    let mut guest = Guest::connect(&sock).await;
    let resp = guest
        .call(Request::CaptureNotify {
            onboarded: true,
            session_id: sid.into(),
            transcript_path: transcript.to_string_lossy().into_owned(),
        })
        .await;
    assert!(matches!(resp, Response::Ok), "a disabled-capture poke is a clean no-op Ok, got {resp:?}");
    assert!(
        !dir.path().join("sessions").exists(),
        "I10: capture disabled → no sessions dir, no .md written"
    );
    assert!(
        engine.current_sessions().await.unwrap().is_empty(),
        "I10: capture disabled → no signed session_captured event"
    );

    std::env::remove_var("AIR_CLAUDE_PROJECTS_ROOT");
}

/// A10: a single connection may fire at most `GUEST_OP_RATE` capture pokes per window; the next is
/// refused with the rate-limit rejection, and a FRESH connection resets the bucket. Capture is left
/// DISABLED — the limiter runs BEFORE dispatch, so it fires regardless of the I10 capture gate (each
/// allowed poke is a clean OK no-op). DOCUMENTED LIMITATION: production MCP clients reconnect per
/// call, so this bounds a single misbehaving CONNECTION, not reconnect-spam (which the same-uid
/// trust boundary + socket 0600 scope instead).
#[tokio::test]
async fn capture_notify_rate_limited_per_connection() {
    let (_dir, sock, _engine) = spawn_daemon_with_engine().await;
    // GUEST_OP_RATE = 10 (the daemon's const). Capture disabled → each allowed poke is an OK no-op.
    let cap = 10;

    let mut guest = Guest::connect(&sock).await;
    for i in 0..cap {
        let resp = guest
            .call(Request::CaptureNotify {
                onboarded: true,
                session_id: "s".into(),
                transcript_path: "/nope.jsonl".into(),
            })
            .await;
        assert!(!is_rate_limited(&resp), "call {i} is within the window cap, got {resp:?}");
    }
    // The (cap+1)th within the window is refused with the rate-limit rejection.
    let over = guest
        .call(Request::CaptureNotify {
            onboarded: true,
            session_id: "s".into(),
            transcript_path: "/nope.jsonl".into(),
        })
        .await;
    assert!(is_rate_limited(&over), "the (cap+1)th capture poke is rate-limited, got {over:?}");

    // A FRESH connection resets the bucket — the limit is per-connection, not global/reconnect-proof.
    let mut fresh = Guest::connect(&sock).await;
    let after_reset = fresh
        .call(Request::CaptureNotify {
            onboarded: true,
            session_id: "s".into(),
            transcript_path: "/nope.jsonl".into(),
        })
        .await;
    assert!(!is_rate_limited(&after_reset), "a fresh connection resets the bucket, got {after_reset:?}");
}

/// A10 regression: the guest surface widened by EXACTLY `CaptureNotify` + `Snapshot` and nothing
/// else. The destructive/listing/config app-only ops stay refused with `NotPermitted` — denied by
/// `Role::allows` BEFORE dispatch, so A10's override widening never leaked them.
#[tokio::test]
async fn guest_still_cannot_reach_app_only_ops() {
    let (_dir, sock) = spawn_onboarded_daemon().await;
    let mut guest = Guest::connect(&sock).await;

    for req in [
        Request::DeleteSession { onboarded: true, session_id: "s".into() },
        Request::ListSessions { onboarded: true },
        Request::SetCaptureEnabled { onboarded: true, enabled: true, backfill: false },
    ] {
        let resp = guest.call(req.clone()).await;
        assert!(
            matches!(resp, Response::Err { kind: OpErrorKindWire::NotPermitted, .. }),
            "{req:?} must stay NotPermitted for MemoryClient, got {resp:?}"
        );
    }
}
