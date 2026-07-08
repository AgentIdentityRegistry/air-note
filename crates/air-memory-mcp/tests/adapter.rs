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

#[tokio::test]
async fn version_mismatch_surfaces_protocol_error() {
    // A reachable daemon that speaks a DIFFERENT protocol version: it completes the Hello read but
    // replies with an incompatible `HelloOk`. This is NOT "unavailable" (the daemon is running) —
    // the adapter must classify it as `Protocol` so `user_message` keeps the "adapter X, daemon Y"
    // diagnostic. A dedicated listener keeps the shared `spawn_fake_daemon` (and its 5 tests) intact.
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("bossclawd.sock");
    let listener = UnixListener::bind(&sock).expect("bind fake daemon");
    tokio::spawn(async move {
        let (mut stream, _addr) = listener.accept().await.expect("accept");
        // Consume the client's Hello, then answer with a mismatched version (no request is read:
        // the adapter bails on the mismatch before sending one).
        read_frame(&mut stream).await.expect("read Hello");
        let hello_ok = HelloOk { pid: std::process::id(), proto_version: PROTO_VERSION + 1 };
        write_frame(&mut stream, &serde_json::to_vec(&hello_ok).unwrap()).await.unwrap();
    });

    let err = tool_recall(&sock, "x", 8).await.expect_err("version mismatch → Err");
    assert!(matches!(err, DaemonError::Protocol(_)), "got {err:?}");
}
