//! B6 — the end-to-end integration proof that **Plan A** (capture / snapshot / forget inside the
//! `bossclawd` daemon) and **Plan B** (this `air-memory-mcp` adapter + the Claude Code hooks) actually
//! COMPOSE. Every prior task tested its own slice against a mock or a fake; this one drives the REAL
//! pieces against one another over a live Unix socket and proves the three seams that only an e2e test
//! can prove:
//!
//!   1. **B3 ↔ A9/A10 project-key match, LIVE.** The `nudge`'s project key is the transcript's
//!      parent-dir slug ([`air_memory_mcp::hook::snapshot_project`]); capture stores a session under the
//!      SAME slug (`server::transcript_project_slug` / the sweeper). We capture a session, then nudge,
//!      and SEE the captured title surface in the snapshot — proving the two derivations agree at
//!      runtime, not merely by inspection.
//!   2. **B4 ↔ B2 command contract, LIVE.** The desktop config-writer (B4) writes a shell-executed hook
//!      command of the form `'<binary>' capture-notify` / `'<binary>' nudge`. We reproduce that exact
//!      command FORM and run the REAL adapter binary (`CARGO_BIN_EXE_air-memory-mcp`, which is exactly
//!      the binary the config-writer's command names) through `sh -c` — so a capture that actually
//!      lands is proof the subcommand token B4 writes is the token B2 accepts, with the shell quoting
//!      intact.
//!   3. **A11 fence surfaces a captured session end-to-end.** The nudge's stdout is the fenced,
//!      project-scoped snapshot NAMING the captured title (security preamble present) — not the static
//!      fallback — so the whole capture→snapshot path is exercised through the real daemon.
//!
//! Then it proves FORGET closes the loop: an App `DeleteSession` removes the `.md`, empties
//! `ListSessions`, empties a recall of the title, and a SUBSEQUENT nudge no longer names it. Finally
//! the guest boundary is re-asserted at the integration level: a `MemoryClient` `DeleteSession` is
//! `NotPermitted`.
//!
//! ## Library fns vs. the real binary (documented choice)
//! We drive BOTH, because each proves something the other cannot:
//!   * the **capture** and **first nudge** go through the REAL BINARY via `sh -c "'<bin>' <sub>"` — the
//!     only way to prove seam #2 (B4's command form actually invokes B2's subcommand). The binary is
//!     just a thin arg/stdin/stdout shell over `daemon::tool_capture_notify` / `run_nudge`, so running
//!     it exercises the same library code path AND the routing/plumbing the library fns skip.
//!   * the **post-delete nudge** goes through the LIBRARY fn [`air_memory_mcp::run_nudge`] directly — a
//!     faster, subprocess-free check that delete is reflected, and a direct exercise of the fn the task
//!     names. Same code path the binary calls.
//!
//! `CARGO_BIN_EXE_air-memory-mcp` is a Cargo guarantee for an integration test of a crate that has a
//! `[[bin]]` — so the binary path is not fragile here; from any OTHER crate it would be, and we would
//! fall back to the library fns per the task's guidance.
//!
//! Hermetic daemon (in-memory vault + mock embedder/reasoner — NEVER the OS keychain, which would hang
//! CI on an ACL prompt), onboarded fixture, a fake `AIR_CLAUDE_PROJECTS_ROOT` for the capture paths.
//! Unix-only (the transport is a Unix socket).
#![cfg(unix)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use bossclawd::engine::EngineHandle;
use bossclawd::server;
use bossclawd_proto::{
    read_frame, write_frame, Hello, HelloOk, OpErrorKindWire, Request, Response, Role, PROTO_VERSION,
};
use tokio::net::UnixStream;
use tokio::sync::Mutex;

/// Serializes the tests that mutate the process-global `AIR_CLAUDE_PROJECTS_ROOT` so their fake roots
/// never race under cargo's parallel test threads (same convention as the `bossclawd` capture suites).
static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
fn env_lock() -> &'static Mutex<()> {
    ENV_LOCK.get_or_init(|| Mutex::new(()))
}

fn now_epoch() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
}

// ── Raw-frame clients (App = full access; Guest = the scoped MemoryClient role the adapter uses). ──
// Integration test files can't share code, so these minimal helpers mirror `bossclawd/tests/forget.rs`.

/// A connected `Role::App` client — the desktop app's full-access role (the "App" actor in spec §11).
struct App {
    stream: UnixStream,
}

impl App {
    async fn connect(sock: &Path) -> Self {
        Self { stream: handshake(sock, Role::App).await }
    }
    async fn call(&mut self, req: Request) -> Response {
        write_frame(&mut self.stream, &serde_json::to_vec(&req).unwrap()).await.unwrap();
        serde_json::from_slice(&read_frame(&mut self.stream).await.unwrap()).unwrap()
    }
}

/// A connected `Role::MemoryClient` guest — the exact scoped role the `air-memory-mcp` adapter uses.
/// Here it exists only to prove the forget op is refused at the role gate (belt-and-suspenders).
struct Guest {
    stream: UnixStream,
}

impl Guest {
    async fn connect(sock: &Path) -> Self {
        Self { stream: handshake(sock, Role::MemoryClient).await }
    }
    async fn call(&mut self, req: Request) -> Response {
        write_frame(&mut self.stream, &serde_json::to_vec(&req).unwrap()).await.unwrap();
        serde_json::from_slice(&read_frame(&mut self.stream).await.unwrap()).unwrap()
    }
}

/// Open a connection and complete the `Hello`/`HelloOk` handshake as `role`.
async fn handshake(sock: &Path, role: Role) -> UnixStream {
    let mut stream = UnixStream::connect(sock).await.expect("connect to daemon socket");
    let hello = Hello { proto_version: PROTO_VERSION, role };
    write_frame(&mut stream, &serde_json::to_vec(&hello).unwrap()).await.unwrap();
    let hello_ok: HelloOk =
        serde_json::from_slice(&read_frame(&mut stream).await.unwrap()).unwrap();
    assert_eq!(hello_ok.proto_version, PROTO_VERSION);
    stream
}

/// Write a minimal real-shape Claude Code transcript (ends with a newline → no torn tail). The
/// `prompt`'s first word becomes the derived session title, so a distinctive first word is directly
/// recallable and directly visible in the project snapshot. Mirrors the `bossclawd` capture suites.
fn write_transcript(path: &Path, prompt: &str) {
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

/// Spawn an onboarded, hermetic daemon and hand back the temp home (also the data_dir — the `.md`
/// lands under `<home>/sessions/`), the socket, and the shared engine handle (for `set_capture_enabled`,
/// which the wire can't do from a guest). Mirrors `bossclawd/tests/forget.rs::spawn_daemon_with_engine`.
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
            "created_at": "2026-07-11T00:00:00+00:00"
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

// ── The B4 command FORM, reproduced so we can run the real binary exactly as Claude Code would. ──

/// POSIX single-quote a string into one inert shell literal — byte-identical to the desktop
/// config-writer's `sh_single_quote` (`integrations/claude_code.rs`). The hook command is
/// shell-executed by Claude Code, so the binary path must be single-quoted (`"`/`$`/backtick/`\`
/// neutralized). Here the path is Cargo's own integration-test binary, so quoting is belt-and-suspenders,
/// but reproducing the exact FORM is the point of seam #2.
fn sh_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The exact shell command the config-writer (B4) emits for a hook: `'<binary>' <subcommand>`
/// (mirrors `capture_hook_group` / `nudge_hook_group` in `integrations/claude_code.rs`). `env!` resolves
/// to THE binary the config-writer's command would name — the same crate's `[[bin]]`.
fn hook_command(subcommand: &str) -> String {
    format!("{} {}", sh_single_quote(env!("CARGO_BIN_EXE_air-memory-mcp")), subcommand)
}

/// Run the real `air-memory-mcp` binary through `sh -c "<b4-command-form>"`, feeding `stdin_json` on
/// stdin and pointing it at our hermetic daemon via `BOSSCLAWD_SOCKET` (exactly the env var the
/// config-writer's `mcpServers` entry sets, honored by `bossclawd_paths`). Returns the child's stdout.
///
/// Run on the blocking pool (`spawn_blocking`) so the current-thread test runtime keeps driving the
/// in-process daemon while the child blocks on the socket round-trip — otherwise the daemon's accept
/// loop could never serve the child and it would hang. `sh`, stdin, and env are all Unix-portable.
async fn run_hook_via_shell(command: String, sock: PathBuf, stdin_json: String) -> String {
    tokio::task::spawn_blocking(move || {
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(&command)
            .env(bossclawd_paths::ENV_SOCKET, &sock)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn air-memory-mcp via sh -c");
        // Write the hook JSON, then drop stdin → EOF so the adapter's byte-bounded read returns.
        child
            .stdin
            .take()
            .expect("child stdin is piped")
            .write_all(stdin_json.as_bytes())
            .expect("write hook JSON to child stdin");
        let out = child.wait_with_output().expect("wait for adapter child");
        // The hooks always exit 0 (I1); a non-zero exit would be a real regression worth catching.
        assert!(out.status.success(), "the adapter hook must always exit 0, got {:?}", out.status);
        String::from_utf8_lossy(&out.stdout).into_owned()
    })
    .await
    .expect("join the adapter child task")
}

/// The full Plan-A + Plan-B loop over the REAL daemon + REAL adapter: capture → snapshot → forget.
#[tokio::test]
async fn e2e_capture_snapshot_forget_through_daemon_and_adapter() {
    let _env = env_lock().lock().await;
    let (dir, sock, engine) = spawn_daemon_with_engine().await;
    let home = dir.path();

    // ── GIVEN: capture consented (App-only setup, direct engine handle) + a fake projects root with a
    //    real transcript under `<root>/<slug>/<sid>.jsonl` carrying a distinctive title word. ──
    engine.set_capture_enabled(true, true, false, now_epoch()).await.unwrap();
    let projects = tempfile::tempdir().unwrap();
    std::env::set_var("AIR_CLAUDE_PROJECTS_ROOT", projects.path());

    let slug = "-Users-tester-e2e-repo";
    let proj_dir = projects.path().join(slug);
    std::fs::create_dir_all(&proj_dir).unwrap();
    let sid = "b6-e2e-session-1";
    let transcript = proj_dir.join(format!("{sid}.jsonl"));
    let title_word = "xylophraxis"; // a distinctive single FTS token, unique to this session's title
    write_transcript(&transcript, &format!("{title_word} how does the whole loop compose"));
    let transcript_str = transcript.to_string_lossy().into_owned();

    // ── WHEN: the SessionEnd hook fires — run the REAL binary via B4's command form `'<bin>' capture-notify`. ──
    let capture_command = hook_command("capture-notify");
    assert!(
        capture_command.contains("capture-notify"),
        "B4↔B2: the SessionEnd command must carry B2's `capture-notify` subcommand token: {capture_command}"
    );
    let session_end_json = serde_json::json!({
        "hook_event_name": "SessionEnd",
        "session_id": sid,
        "transcript_path": transcript_str,
        "reason": "clear",
    })
    .to_string();
    let capture_stdout = run_hook_via_shell(capture_command, sock.clone(), session_end_json).await;
    assert!(capture_stdout.is_empty(), "capture-notify is fire-and-forget: it prints nothing, got {capture_stdout:?}");

    // ── THEN: the session is captured (verified over the real socket, not by the fire-and-forget exit code). ──
    let mut app = App::connect(&sock).await;
    match app.call(Request::ListSessions { onboarded: true }).await {
        Response::ListSessions(s) => {
            assert_eq!(s.len(), 1, "the adapter's capture poke landed exactly one session");
            assert_eq!(s[0].session_id, sid, "under the id the hook sent");
            assert_eq!(s[0].project, slug, "under the transcript's parent-dir slug (A9/A10 key)");
            assert_eq!(s[0].tool, "claude-code");
        }
        other => panic!("expected ListSessions, got {other:?}"),
    }
    // The `.md` is on disk, owner-only (0600) — the born-private capture store (A7).
    let md = home.join(format!("sessions/{sid}.md"));
    assert!(md.exists(), "the captured session's .md exists at <data_dir>/sessions/<sid>.md");
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            md.metadata().unwrap().permissions().mode() & 0o777,
            0o600,
            "the captured .md is owner-only (0600)"
        );
    }
    // A recall of the title finds it (the capture is indexed + recallable).
    match app.call(Request::Recall { onboarded: true, query: title_word.into(), k: 5 }).await {
        Response::Recall(hits) => {
            assert!(hits.iter().any(|h| h.text.contains(title_word)), "recall surfaces the captured title");
        }
        other => panic!("expected Recall, got {other:?}"),
    }

    // ── WHEN: the SessionStart hook fires — run the REAL binary via B4's command form `'<bin>' nudge`. ──
    let nudge_command = hook_command("nudge");
    assert!(
        nudge_command.contains("nudge"),
        "B4↔B2: the SessionStart command must carry B2's `nudge` subcommand token: {nudge_command}"
    );
    let session_start_json = serde_json::json!({
        "hook_event_name": "SessionStart",
        "source": "startup",
        "session_id": sid,
        "transcript_path": transcript_str,
        "cwd": "/Users/tester/e2e-repo",
    })
    .to_string();
    let snapshot = run_hook_via_shell(nudge_command, sock.clone(), session_start_json).await;

    // ── THEN: stdout is the LIVE fenced snapshot NAMING the captured title — proving the project-key
    //    match (seam #1) and the fence surfacing a captured session (seam #3), end-to-end. ──
    assert!(
        snapshot.contains(title_word),
        "the live snapshot names the captured session's title (project-key match, capture→snapshot): {snapshot}"
    );
    assert!(
        snapshot.contains("not instructions") || snapshot.contains("DATA, not"),
        "the snapshot carries the fence security preamble (A11): {snapshot}"
    );
    assert!(
        snapshot != air_memory_mcp::NUDGE_TEXT,
        "the nudge served the LIVE snapshot, NOT the static fallback"
    );
    assert!(snapshot.len() <= 4096, "the snapshot respects the I8 byte budget: {} bytes", snapshot.len());

    // ── WHEN: the App forgets the session (spec §11 delete). ──
    assert!(
        matches!(app.call(Request::DeleteSession { onboarded: true, session_id: sid.into() }).await, Response::Ok),
        "App DeleteSession → Ok"
    );

    // ── THEN: delete is honest — off the list, `.md` gone, recall empty. ──
    match app.call(Request::ListSessions { onboarded: true }).await {
        Response::ListSessions(s) => assert!(s.is_empty(), "the deleted session is gone from ListSessions"),
        other => panic!("expected ListSessions, got {other:?}"),
    }
    assert!(!md.exists(), "delete removes the .md (content destroyed, not just tombstoned)");
    match app.call(Request::Recall { onboarded: true, query: title_word.into(), k: 5 }).await {
        Response::Recall(hits) => assert!(hits.is_empty(), "recall of the title is empty after delete"),
        other => panic!("expected Recall, got {other:?}"),
    }

    // ── THEN: a SUBSEQUENT nudge no longer names it — the snapshot reflects the delete. Driven through
    //    the LIBRARY fn `run_nudge` (the same code path the binary calls) against the real daemon. ──
    let post_delete_nudge = air_memory_mcp::run_nudge(
        &sock,
        air_memory_mcp::hook::HookInput {
            session_id: Some(sid.into()),
            transcript_path: Some(transcript_str.clone()),
            source: Some("startup".into()),
            ..Default::default()
        },
    )
    .await;
    assert!(
        !post_delete_nudge.contains(title_word),
        "after delete the snapshot no longer names the forgotten session: {post_delete_nudge}"
    );

    // ── Guest boundary, end-to-end: a MemoryClient DeleteSession is refused at the role gate. ──
    let mut guest = Guest::connect(&sock).await;
    match guest.call(Request::DeleteSession { onboarded: true, session_id: sid.into() }).await {
        Response::Err { kind: OpErrorKindWire::NotPermitted, .. } => {}
        other => panic!("a MemoryClient DeleteSession must be NotPermitted, got {other:?}"),
    }

    std::env::remove_var("AIR_CLAUDE_PROJECTS_ROOT");
}
