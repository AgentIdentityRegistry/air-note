//! Tests for the `capture-notify` hook path (B2): the SessionEnd fire-and-forget poke.
//!
//! Three things are proven here: (1) the poke maps stdin fields to a `Request::CaptureNotify` over a
//! `MemoryClient` handshake, (2) a dead daemon is *tolerated* (an `Err` the subcommand discards →
//! exit 0, never a panic — the sweeper is the durability guarantee, I1/§6), and (3) a wedged daemon
//! is bounded by a SHORT timeout (NOT the 30s tool `CALL_TIMEOUT`) so it can never hang the
//! SessionEnd hook. A hand-rolled fake daemon (the same shape as `tests/adapter.rs`) lets us assert
//! the wire behavior without linking the engine. Unix-only (the transport is a Unix socket).
#![cfg(unix)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use air_memory_mcp::daemon::{tool_capture_notify, DaemonError};
use air_memory_mcp::hook::{capture_target, HookInput};
use bossclawd_proto::{
    read_frame, write_frame, Hello, HelloOk, Request, Response, Role, PROTO_VERSION,
};
use tokio::net::UnixListener;

/// A fake daemon serving ONE connection: it asserts the client handshook as `MemoryClient`, then
/// answers each request via `answer(req) -> Response`. Mirrors `tests/adapter.rs::spawn_fake_daemon`
/// (integration test files don't share code, so the pattern is replicated).
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
        assert_eq!(hello.role, Role::MemoryClient, "capture-notify MUST handshake as MemoryClient");
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
async fn capture_notify_sends_the_request_with_stdin_fields() {
    // The fake daemon records the Request it received (via a std channel — tokio's `sync` feature is
    // not enabled and deps stay unchanged) and answers Response::Ok.
    let (tx, rx) = std::sync::mpsc::channel();
    let (_dir, sock) = spawn_fake_daemon(move |req| {
        tx.send(req).expect("record request");
        Response::Ok
    })
    .await;

    tool_capture_notify(&sock, "abc-123", "/t.jsonl").await.expect("capture-notify ok");

    // The round-trip already completed (the daemon sent before writing Response::Ok), so this recv
    // is non-blocking.
    let req = rx.recv().expect("daemon received a request");
    match req {
        Request::CaptureNotify { onboarded, session_id, transcript_path } => {
            assert!(onboarded, "capture poke is a MemoryClient op — onboarded is always true");
            assert_eq!(session_id, "abc-123");
            assert_eq!(transcript_path, "/t.jsonl");
        }
        other => panic!("unexpected request: {other:?}"),
    }
}

#[tokio::test]
async fn capture_notify_tolerates_a_dead_daemon() {
    // A socket path that was never bound — nobody is listening (I1/§6).
    let dir = tempfile::tempdir().unwrap();
    let sock: PathBuf = dir.path().join("bossclawd.sock");
    // A dead daemon must never panic and never hang. The fn surfaces an `Unavailable` Err that the
    // SUBCOMMAND discards (`let _ = …`) → the process still `return Ok(())` → exits 0. The sweeper
    // (A9) backfills the missed session, so a failed poke is only a lost optimization, never lost data.
    let res = tool_capture_notify(&sock, "abc-123", "/t.jsonl").await;
    assert!(
        matches!(res, Err(DaemonError::Unavailable(_))),
        "a dead daemon yields a discardable Unavailable Err, got {res:?}"
    );
    let _ = res; // exactly what the subcommand does before `return Ok(())` → exit 0
}

#[tokio::test]
async fn capture_notify_short_timeout() {
    // A daemon that accepts the connection but NEVER replies (it never even completes the handshake).
    // tool_capture_notify must give up on its SHORT bound — NOT the 30s tool CALL_TIMEOUT — so a
    // wedged daemon can never hang the SessionEnd hook.
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("bossclawd.sock");
    let listener = UnixListener::bind(&sock).expect("bind fake daemon");
    tokio::spawn(async move {
        // Keep the accepted stream alive (drop would EOF the client into an instant error, defeating
        // the point) and never write a byte back.
        let (_stream, _addr) = listener.accept().await.expect("accept");
        std::future::pending::<()>().await;
    });

    let start = Instant::now();
    let res = tool_capture_notify(&sock, "abc-123", "/t.jsonl").await;
    let elapsed = start.elapsed();
    assert!(res.is_err(), "a never-replying daemon must time out to an Err, got {res:?}");
    assert!(
        elapsed < Duration::from_secs(3),
        "must return on the SHORT capture bound (not the 30s CALL_TIMEOUT), took {elapsed:?}"
    );
}

#[test]
fn capture_notify_skips_when_fields_absent_or_empty() {
    // The poke fires ONLY when BOTH session_id and transcript_path are present AND non-empty. The B1
    // contract surfaces a present-but-empty field as Some("") (control-only strips to ""), so an
    // empty value MUST be treated as ABSENT → skip the poke.
    let present = |sid: Option<&str>, path: Option<&str>| HookInput {
        session_id: sid.map(str::to_string),
        transcript_path: path.map(str::to_string),
        ..Default::default()
    };

    // BOTH present + non-empty → Some(pair)
    assert_eq!(
        capture_target(&present(Some("abc-123"), Some("/t.jsonl"))),
        Some(("abc-123".to_string(), "/t.jsonl".to_string()))
    );

    // Any missing OR empty field → None (no poke)
    assert_eq!(capture_target(&present(None, Some("/t.jsonl"))), None, "session_id absent");
    assert_eq!(capture_target(&present(Some(""), Some("/t.jsonl"))), None, "session_id empty");
    assert_eq!(capture_target(&present(Some("abc-123"), None)), None, "transcript_path absent");
    assert_eq!(capture_target(&present(Some("abc-123"), Some(""))), None, "transcript_path empty");
    assert_eq!(capture_target(&present(None, None)), None, "both absent");
}
