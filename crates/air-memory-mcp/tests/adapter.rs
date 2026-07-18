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
    // recall + remember lead the surface (the Rung-3 resolution tools are appended after them;
    // the four-tool surface is pinned by `mcp`'s own unit test).
    assert_eq!(names[0], "recall");
    assert_eq!(names[1], "remember");
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
async fn tools_call_notification_gets_no_reply_and_no_side_effect() {
    // A LIVE fake daemon whose answer PANICS on any request: this proves the write is never
    // dispatched, not merely that no reply is returned. If the `id.as_ref()?;` short-circuit were
    // removed, the `remember` write would reach this daemon and blow up (regression caught by an
    // actual write attempt). A correct short-circuit never connects, so `answer` is never called and
    // the daemon's lone `accept` simply idles until the test runtime shuts down.
    let (_dir, sock) = spawn_fake_daemon(|req| match req {
        Request::Remember { .. } => panic!("notification must not dispatch a write"),
        other => panic!("unexpected: {other:?}"),
    })
    .await;
    // `tools/call` with NO `id` is a notification: no reply, and the write must not execute.
    let note = r#"{"jsonrpc":"2.0","method":"tools/call","params":{"name":"remember","arguments":{"text":"hi"}}}"#;
    assert!(
        handle_message(&sock, note).await.is_none(),
        "a tools/call notification gets no reply and no side effect"
    );
}

#[tokio::test]
async fn tools_call_missing_required_arg_is_iserror_not_protocol_error() {
    let sock = Path::new("/unused.sock");
    // Missing `query`: `run_recall` returns `InvalidArgs` BEFORE any socket call, so `/unused.sock`
    // is never touched. Bad args to a known tool are model-correctable → an `isError` tool result
    // (I4), NOT a JSON-RPC `-32602` (which is reserved for an unknown tool name).
    let req = r#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"recall","arguments":{}}}"#;
    let resp = parse(&handle_message(sock, req).await.expect("replies"));
    assert!(resp["error"].is_null(), "invalid args must NOT be a JSON-RPC error");
    assert_eq!(resp["result"]["isError"], true);
    assert!(resp["result"]["content"][0]["text"].as_str().unwrap().contains("Invalid tool arguments"));
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
