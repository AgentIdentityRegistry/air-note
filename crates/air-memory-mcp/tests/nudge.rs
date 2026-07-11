//! Tests for the `nudge` SessionStart hook path (B3): a LIVE, project-aware orientation snapshot with
//! a static fallback. Four properties are pinned: (1) when the daemon answers `Response::Snapshot`,
//! its text is printed verbatim AND the request carries `project == the transcript's parent-dir slug`
//! (the load-bearing match key — NOT `cwd`); (2) ANY failure — dead daemon, wedged daemon, not
//! onboarded — degrades to the static `NUDGE_TEXT`, and the wedged case falls back on the SHORT
//! snapshot bound (NOT the 30s tool `CALL_TIMEOUT`) so it lands under Claude Code's 5s hook kill;
//! (3) the pure project-key derivation is the parent-dir slug (`None` when the path is absent);
//! (4) an SP2-era hook that pipes no stdin still prints the static text (backwards compat). A
//! hand-rolled fake daemon (same shape as `tests/capture_notify.rs`) exercises the wire behavior
//! without linking the engine. Unix-only (the transport is a Unix socket).
#![cfg(unix)]

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use air_memory_mcp::hook::{snapshot_project, HookInput};
use air_memory_mcp::run_nudge;
use bossclawd_proto::{
    read_frame, write_frame, Hello, HelloOk, Request, Response, Role, PROTO_VERSION,
};
use tokio::net::UnixListener;

/// The fallback MUST land under Claude Code's 5s SessionStart hook kill; the wedged-daemon case must
/// return well inside this (the 2s `SNAPSHOT_TIMEOUT` + slack), never the 30s tool `CALL_TIMEOUT`.
const SNAPSHOT_TIMEOUT_MAX_SECS: u64 = 3;

/// A fake daemon serving ONE connection: asserts the client handshook as `MemoryClient`, then answers
/// each request via `answer(req) -> Response`. Mirrors `tests/capture_notify.rs::spawn_fake_daemon`
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
        assert_eq!(hello.role, Role::MemoryClient, "nudge snapshot MUST handshake as MemoryClient");
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

/// A SessionStart hook payload whose transcript lives under a project-slug dir.
fn compact_hook() -> HookInput {
    HookInput {
        session_id: Some("abc-123".to_string()),
        transcript_path: Some("/home/x/.claude/projects/-Users-x-repo/abc-123.jsonl".to_string()),
        source: Some("startup".to_string()),
        ..Default::default()
    }
}

#[tokio::test]
async fn nudge_prints_live_snapshot_when_daemon_answers() {
    let snapshot = "```air-orientation\nLast session you shipped the capture sweeper.\n```";
    let (tx, rx) = std::sync::mpsc::channel();
    let (_dir, sock) = spawn_fake_daemon(move |req| {
        tx.send(req).expect("record request");
        Response::Snapshot(snapshot.to_string())
    })
    .await;

    let out = run_nudge(&sock, compact_hook()).await;
    assert_eq!(out, snapshot, "a live snapshot must be printed verbatim (not the static nudge)");

    // The round-trip already completed (the daemon sent the request before its Response), so recv is
    // non-blocking.
    let req = rx.recv().expect("daemon received a request");
    match req {
        Request::Snapshot { onboarded, project, source, session_id, transcript_path } => {
            assert!(onboarded, "snapshot is a MemoryClient op — onboarded is always true");
            assert_eq!(
                project, "-Users-x-repo",
                "project key MUST be the transcript's parent-dir slug (what capture stored), not cwd"
            );
            assert_eq!(source, "startup");
            assert_eq!(session_id.as_deref(), Some("abc-123"), "compact flavor passes the session id");
            assert_eq!(
                transcript_path.as_deref(),
                Some("/home/x/.claude/projects/-Users-x-repo/abc-123.jsonl"),
                "compact flavor passes the transcript path for the live digest"
            );
        }
        other => panic!("unexpected request: {other:?}"),
    }
}

#[tokio::test]
async fn nudge_falls_back_to_static_on_dead_daemon_and_on_timeout_and_not_onboarded() {
    // (a) dead daemon — a socket path nobody ever bound.
    let dir = tempfile::tempdir().unwrap();
    let dead_sock = dir.path().join("bossclawd.sock");
    assert_eq!(
        run_nudge(&dead_sock, compact_hook()).await,
        air_memory_mcp::NUDGE_TEXT,
        "a dead daemon must degrade to the static nudge, never panic"
    );

    // (b) wedged daemon — accepts the connection but NEVER replies. The fallback must land on the
    // SHORT snapshot bound (NOT the 30s tool CALL_TIMEOUT) so the hook is never held past its budget.
    let dir2 = tempfile::tempdir().unwrap();
    let wedged_sock = dir2.path().join("bossclawd.sock");
    let listener = UnixListener::bind(&wedged_sock).expect("bind wedged daemon");
    tokio::spawn(async move {
        // Keep the accepted stream alive (dropping it would EOF the client into an instant error,
        // defeating the timeout test) and never write a byte back.
        let (_stream, _addr) = listener.accept().await.expect("accept");
        std::future::pending::<()>().await;
    });
    let start = Instant::now();
    let out = run_nudge(&wedged_sock, compact_hook()).await;
    let elapsed = start.elapsed();
    assert_eq!(out, air_memory_mcp::NUDGE_TEXT, "a wedged daemon must degrade to the static nudge");
    assert!(
        elapsed < Duration::from_secs(SNAPSHOT_TIMEOUT_MAX_SECS),
        "must fall back on the SHORT snapshot bound (not the 30s CALL_TIMEOUT), took {elapsed:?}"
    );

    // (c) a reachable, onboarded-failing daemon answering NotOnboarded → static nudge.
    let (_dir3, sock3) = spawn_fake_daemon(|_req| Response::NotOnboarded).await;
    assert_eq!(
        run_nudge(&sock3, compact_hook()).await,
        air_memory_mcp::NUDGE_TEXT,
        "a NotOnboarded response must degrade to the static nudge"
    );
}

#[test]
fn nudge_project_key_is_the_transcript_slug_not_cwd() {
    let with_path = |p: Option<&str>| HookInput {
        transcript_path: p.map(str::to_string),
        ..Default::default()
    };

    // The load-bearing match key: the transcript's PARENT-DIRECTORY name (the Claude Code cwd-slug),
    // byte-identical to what the daemon stored — NOT the file, NOT cwd.
    assert_eq!(
        snapshot_project(&with_path(Some(
            "/home/x/.claude/projects/-Users-x-repo/abc-123.jsonl"
        )))
        .as_deref(),
        Some("-Users-x-repo"),
        "project key = the transcript's parent-dir slug"
    );
    // Absent path → None → the caller uses the static nudge (no matchable key).
    assert_eq!(snapshot_project(&with_path(None)), None, "no transcript_path → None");
    // The B1 contract surfaces a present-but-empty field as Some("") → treated as absent.
    assert_eq!(snapshot_project(&with_path(Some(""))), None, "empty transcript_path → None");
    // A bare filename with no parent dir is degenerate (a real transcript always lives under a project
    // dir) → None, so the caller uses the static nudge rather than an empty, unmatchable key.
    assert_eq!(snapshot_project(&with_path(Some("abc-123.jsonl"))), None, "no parent dir → None");
}

#[tokio::test]
async fn nudge_without_stdin_prints_static_text() {
    // Backwards compat: an SP2-era hook that pipes NOTHING → read_from_stdin yields HookInput::default
    // → no transcript_path → no matchable project → the static nudge, WITHOUT contacting the daemon
    // (the None branch short-circuits before any socket call — proven here with a never-bound path).
    let dir = tempfile::tempdir().unwrap();
    let never_bound = dir.path().join("bossclawd.sock");
    assert_eq!(
        run_nudge(&never_bound, HookInput::default()).await,
        air_memory_mcp::NUDGE_TEXT,
        "no stdin → default HookInput → static nudge"
    );
}

#[test]
fn nudge_subcommand_prints_nudge_text_and_exits_zero() {
    // The real binary with closed stdin (`output()` gives the child an immediately-closed stdin) — the
    // end-to-end backwards-compat proof AND the byte-for-byte NUDGE_TEXT assertion. No daemon is
    // running, so the empty stdin → default HookInput → static text, and no socket banner is emitted.
    let out = Command::new(env!("CARGO_BIN_EXE_air-memory-mcp"))
        .arg("nudge")
        .output()
        .expect("run air-memory-mcp nudge");

    assert!(out.status.success(), "nudge must exit 0; got {:?}", out.status);
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        air_memory_mcp::NUDGE_TEXT,
        "stdout must be exactly NUDGE_TEXT (no trailing newline added)"
    );
    // It must NOT emit the server's socket banner (that only prints on the server path).
    let err = String::from_utf8(out.stderr).unwrap();
    assert!(!err.contains("using daemon socket"), "nudge must not start the server: {err}");
}
