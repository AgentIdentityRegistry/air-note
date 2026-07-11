//! A13 — the real-daemon FORGET suite (the ship-gate for SP3's forget feature). Drives the 7
//! remaining App-only ops (ListSessions / GetSession / DeleteSession / ListNotes / SupersedeNote /
//! SetCaptureEnabled / CaptureEnabled) over the REAL `bossclawd` socket and proves:
//!   * delete is HONEST + DURABLE — the `.md` is removed, the session leaves the fold, and a recall
//!     of its title goes empty (both fusion arms), IN-SESSION and across a FULL daemon RESTART
//!     (I7 — the crown-jewel resurrection test at the wire level);
//!   * supersede replaces a note in recall (A3 exclusion);
//!   * a `MemoryClient` guest is denied EVERY one of the 7 ops at the role gate (I3 — forget is
//!     App-only);
//!   * `GetSession` on an unknown id is a clean `Rejected` (not a Core fault), and the capture-flag
//!     ops round-trip over the wire.
//!
//! Hermetic engine (in-memory vault + mock embedder/reasoner — NEVER the OS keychain), onboarded
//! fixture, a fake `AIR_CLAUDE_PROJECTS_ROOT` for the capture paths. Unix-only.
#![cfg(unix)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
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
/// roots never race under cargo's parallel test threads (same convention as `memory_client_loop`).
static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
fn env_lock() -> &'static Mutex<()> {
    ENV_LOCK.get_or_init(|| Mutex::new(()))
}

/// A per-process counter so two daemons that share ONE `home` (the restart test) bind distinct
/// socket paths — re-binding a lingering socket path would fail `EADDRINUSE`.
static SOCK_SEQ: AtomicU64 = AtomicU64::new(0);

fn now_epoch() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
}

/// The onboarded-brain fixture identity, written once into a daemon `home`.
fn write_identity(home: &std::path::Path) {
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
}

/// A connected App client (`Role::App` — full access), speaking the framed protocol.
struct App {
    stream: UnixStream,
}

impl App {
    async fn connect(sock: &std::path::Path) -> Self {
        let mut stream = UnixStream::connect(sock).await.expect("connect");
        let hello = Hello { proto_version: PROTO_VERSION, role: Role::App };
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

/// A connected guest client (`Role::MemoryClient` — the scoped role the `air-memory-mcp` adapter
/// uses). Used to prove the forget/listing ops are refused at the role gate BEFORE dispatch.
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

/// A minimal real-shape Claude Code transcript (ends with a newline → no torn tail), mirroring the
/// other capture tests so both capture paths render the same content shape. The `prompt` becomes the
/// derived title (first user prompt), so a distinctive first word is directly recallable.
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

/// Spawn a plain onboarded daemon (no engine handle needed) on a fresh temp home + socket.
async fn spawn_onboarded_daemon() -> (tempfile::TempDir, PathBuf) {
    bossclawd::vault::seed_secret_cache_for_test(Default::default());
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().to_path_buf();
    write_identity(&home);
    let sock = home.join("bossclawd.sock");
    server::spawn_for_test(sock.clone(), home).await;
    (dir, sock)
}

/// Spawn an onboarded daemon AND return the shared engine handle (for app-only setup the wire can't
/// do, e.g. `set_capture_enabled`, and to read the signed fold directly). Mirrors
/// `memory_client_loop::spawn_daemon_with_engine`.
async fn spawn_daemon_with_engine() -> (tempfile::TempDir, PathBuf, Arc<EngineHandle>) {
    use std::os::unix::fs::PermissionsExt;

    use tokio::net::UnixListener;

    bossclawd::vault::seed_secret_cache_for_test(Default::default());
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().to_path_buf();
    write_identity(&home);
    let engine = Arc::new(server::test_engine(home.clone()));
    let sock = home.join("bossclawd.sock");
    let listener = UnixListener::bind(&sock).unwrap();
    std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600)).unwrap();
    tokio::spawn(server::run_accept_loop(engine.clone(), listener));
    (dir, sock, engine)
}

/// Spawn a daemon on a CALLER-OWNED `home` sharing a CALLER-OWNED vault, returning the socket, the
/// engine handle, and the accept-loop `JoinHandle` (so the caller can abort it to model a restart).
/// The shared vault is what lets a second daemon reopen the SAME brain.db (the keystore persists the
/// keys in the vault; a fresh vault would re-mint and fail to decrypt) — the daemon-level companion
/// to core's engine-level reopen tests.
async fn spawn_on_home(
    home: PathBuf,
    vault: Arc<dyn bossclawd::secrets::SecretsVault>,
) -> (PathBuf, Arc<EngineHandle>, tokio::task::JoinHandle<()>) {
    use std::os::unix::fs::PermissionsExt;

    use tokio::net::UnixListener;

    bossclawd::vault::seed_secret_cache_for_test(Default::default());
    let engine = Arc::new(server::test_engine_with_vault(home.clone(), vault));
    let n = SOCK_SEQ.fetch_add(1, Ordering::Relaxed);
    let sock = home.join(format!("bossclawd-{n}.sock"));
    let listener = UnixListener::bind(&sock).unwrap();
    std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600)).unwrap();
    let handle = tokio::spawn(server::run_accept_loop(engine.clone(), listener));
    (sock, engine, handle)
}

/// I7 — delete is HONEST + DURABLE in one session: the App sees a captured session (ListSessions +
/// GetSession + recall), deletes it, and afterward it is GONE everywhere — off the list, GetSession
/// is `Rejected`, the `.md` file is removed, a recall of the title is empty (keyword arm), and the
/// signed fold is empty.
#[tokio::test]
async fn forget_suite_delete_is_durable_and_removes_the_md() {
    let _env = env_lock().lock().await;
    let (dir, sock, engine) = spawn_daemon_with_engine().await;

    let projects = tempfile::tempdir().unwrap();
    let slug = "-Users-tester-repo";
    let proj_dir = projects.path().join(slug);
    std::fs::create_dir_all(&proj_dir).unwrap();
    let sid = "forget-durable-1";
    let transcript = proj_dir.join(format!("{sid}.jsonl"));
    let title_word = "zephyrantha"; // distinctive single FTS token
    write_transcript(&transcript, &format!("{title_word} how does delete work"));

    // App-only setup: consent to capture (direct engine handle) + point capture at our fake root.
    engine.set_capture_enabled(true, true, false, now_epoch()).await.unwrap();
    std::env::set_var("AIR_CLAUDE_PROJECTS_ROOT", projects.path());

    let mut app = App::connect(&sock).await;

    // Capture the session (over the wire).
    assert!(matches!(
        app.call(Request::CaptureNotify {
            onboarded: true,
            session_id: sid.into(),
            transcript_path: transcript.to_string_lossy().into_owned(),
        })
        .await,
        Response::Ok
    ));

    // ListSessions sees exactly it.
    match app.call(Request::ListSessions { onboarded: true }).await {
        Response::ListSessions(s) => {
            assert_eq!(s.len(), 1, "the captured session is listed");
            assert_eq!(s[0].session_id, sid);
            assert_eq!(s[0].project, slug);
            assert_eq!(s[0].tool, "claude-code");
        }
        other => panic!("expected ListSessions, got {other:?}"),
    }

    // GetSession returns the detail with the title in the markdown body.
    match app.call(Request::GetSession { onboarded: true, session_id: sid.into() }).await {
        Response::Session(d) => {
            assert_eq!(d.summary.session_id, sid);
            assert!(d.markdown.contains(title_word), "the detail markdown carries the title");
        }
        other => panic!("expected Session, got {other:?}"),
    }

    // Recall finds it pre-delete (sanity).
    match app.call(Request::Recall { onboarded: true, query: title_word.into(), k: 5 }).await {
        Response::Recall(hits) => assert!(!hits.is_empty(), "recall finds the captured title pre-delete"),
        other => panic!("expected Recall, got {other:?}"),
    }

    // Delete it.
    assert!(matches!(
        app.call(Request::DeleteSession { onboarded: true, session_id: sid.into() }).await,
        Response::Ok
    ));

    // Post-delete: off the list.
    match app.call(Request::ListSessions { onboarded: true }).await {
        Response::ListSessions(s) => assert!(s.is_empty(), "deleted session is gone from ListSessions"),
        other => panic!("expected ListSessions, got {other:?}"),
    }
    // GetSession → clean Rejected (not found / deleted).
    match app.call(Request::GetSession { onboarded: true, session_id: sid.into() }).await {
        Response::Err { kind: OpErrorKindWire::Rejected, .. } => {}
        other => panic!("expected Rejected for a deleted session, got {other:?}"),
    }
    // The `.md` file is removed.
    assert!(
        !dir.path().join(format!("sessions/{sid}.md")).exists(),
        "delete removes the .md (content destroyed, not just tombstoned)"
    );
    // Recall of the title is empty (the keyword arm no longer surfaces it).
    match app.call(Request::Recall { onboarded: true, query: title_word.into(), k: 5 }).await {
        Response::Recall(hits) => assert!(hits.is_empty(), "recall of the title is empty after delete"),
        other => panic!("expected Recall, got {other:?}"),
    }
    // The signed fold is empty.
    assert!(engine.current_sessions().await.unwrap().is_empty(), "current_sessions is empty after delete");

    std::env::remove_var("AIR_CLAUDE_PROJECTS_ROOT");
}

/// I7 CROWN JEWEL — the resurrection test at the DAEMON level: capture → delete → tear the daemon
/// down (abort the accept loop + drop the engine handle) → RE-OPEN a fresh daemon on the SAME
/// data_dir (a shared vault lets it reopen the same brain.db; the recall index rebuilds on first
/// recall) → the session STAYS gone (ListSessions empty AND a recall of its title empty). This is
/// the wire-level companion to A3's engine-level reopen test.
#[tokio::test]
async fn forget_suite_delete_survives_daemon_restart() {
    let _env = env_lock().lock().await;
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().to_path_buf();
    write_identity(&home);
    let vault = server::shared_test_vault();

    let projects = tempfile::tempdir().unwrap();
    let slug = "-Users-tester-repo";
    let proj_dir = projects.path().join(slug);
    std::fs::create_dir_all(&proj_dir).unwrap();
    let sid = "forget-restart-1";
    let transcript = proj_dir.join(format!("{sid}.jsonl"));
    let title_word = "quokkaxenon";
    write_transcript(&transcript, &format!("{title_word} must not resurrect"));
    std::env::set_var("AIR_CLAUDE_PROJECTS_ROOT", projects.path());

    // ── Daemon #1: capture, sanity-check, delete. ──
    let (sock1, engine1, h1) = spawn_on_home(home.clone(), vault.clone()).await;
    engine1.set_capture_enabled(true, true, false, now_epoch()).await.unwrap();
    {
        let mut app = App::connect(&sock1).await;
        assert!(matches!(
            app.call(Request::CaptureNotify {
                onboarded: true,
                session_id: sid.into(),
                transcript_path: transcript.to_string_lossy().into_owned(),
            })
            .await,
            Response::Ok
        ));
        match app.call(Request::ListSessions { onboarded: true }).await {
            Response::ListSessions(s) => assert_eq!(s.len(), 1, "captured before restart"),
            other => panic!("expected ListSessions, got {other:?}"),
        }
        assert!(matches!(
            app.call(Request::DeleteSession { onboarded: true, session_id: sid.into() }).await,
            Response::Ok
        ));
    } // the app client drops here → its server-side connection task ends.

    // ── Tear the daemon DOWN: stop the accept loop, then drop the engine handle so daemon #1's
    // brain.db connection is released before daemon #2 reopens it. ──
    h1.abort();
    let _ = h1.await; // await the cancellation so the loop is truly stopped.
    drop(engine1);

    // ── Daemon #2: a FRESH engine on the SAME home + vault → reopens the SAME brain.db. ──
    let (sock2, engine2, _h2) = spawn_on_home(home.clone(), vault.clone()).await;
    let mut app2 = App::connect(&sock2).await;

    // The deleted session STAYS gone across the restart (the tombstone is durable in the signed log).
    match app2.call(Request::ListSessions { onboarded: true }).await {
        Response::ListSessions(s) => assert!(s.is_empty(), "deleted session stays gone across daemon restart"),
        other => panic!("expected ListSessions, got {other:?}"),
    }
    // And a recall of its title is empty after a full index rebuild (the exclusion is rebuild-proof).
    match app2.call(Request::Recall { onboarded: true, query: title_word.into(), k: 5 }).await {
        Response::Recall(hits) => assert!(hits.is_empty(), "no resurrection: recall of the title empty after restart"),
        other => panic!("expected Recall, got {other:?}"),
    }
    assert!(engine2.current_sessions().await.unwrap().is_empty(), "fold empty after restart");

    std::env::remove_var("AIR_CLAUDE_PROJECTS_ROOT");
}

/// A3 — supersede replaces a note in recall: an App remembers a note, ListNotes shows it (current,
/// `superseded_by: None`), SupersedeNote yields a new id, and afterward the OLD note is EXCLUDED from
/// ListNotes while a recall of the shared prefix returns the NEW text — never the old.
#[tokio::test]
async fn forget_suite_supersede_note_replaces_in_recall() {
    let (_dir, sock) = spawn_onboarded_daemon().await;
    let mut app = App::connect(&sock).await;

    // Remember a note (App may remember).
    let old_id = match app.call(Request::Remember { onboarded: true, text: "vault slot seven".into() }).await {
        Response::Remember(id) => id,
        other => panic!("expected Remember, got {other:?}"),
    };

    // ListNotes shows it, current (superseded_by: None).
    match app.call(Request::ListNotes { onboarded: true }).await {
        Response::ListNotes(notes) => {
            let n = notes.iter().find(|n| n.event_id == old_id).expect("the note is listed");
            assert_eq!(n.text, "vault slot seven");
            assert!(n.superseded_by.is_none(), "a live note has no successor pointer");
        }
        other => panic!("expected ListNotes, got {other:?}"),
    }

    // Supersede it with new text → a NEW event id.
    let new_id = match app
        .call(Request::SupersedeNote { onboarded: true, event_id: old_id.clone(), text: "vault slot nine".into() })
        .await
    {
        Response::Superseded(id) => id,
        other => panic!("expected Superseded, got {other:?}"),
    };
    assert_ne!(new_id, old_id, "supersede mints a distinct event id");

    // ListNotes now EXCLUDES the old note and INCLUDES the new one (current-only fold).
    match app.call(Request::ListNotes { onboarded: true }).await {
        Response::ListNotes(notes) => {
            assert!(!notes.iter().any(|n| n.event_id == old_id), "the superseded note is excluded");
            let n = notes.iter().find(|n| n.event_id == new_id).expect("the new note is listed");
            assert_eq!(n.text, "vault slot nine");
            assert!(n.superseded_by.is_none());
        }
        other => panic!("expected ListNotes, got {other:?}"),
    }

    // Recall of the shared prefix returns the NEW text, never the old (A3 exclusion, both arms).
    match app.call(Request::Recall { onboarded: true, query: "vault slot".into(), k: 5 }).await {
        Response::Recall(hits) => {
            assert!(
                hits.iter().any(|h| h.text.contains("vault slot nine")),
                "recall surfaces the superseding note"
            );
            assert!(
                !hits.iter().any(|h| h.text.contains("vault slot seven")),
                "recall never surfaces the superseded note"
            );
        }
        other => panic!("expected Recall, got {other:?}"),
    }
}

/// I3 — forget is App-only: a `MemoryClient` guest is refused EVERY one of the 7 A13 ops with
/// `NotPermitted`, denied by `Role::allows` BEFORE dispatch (no engine work happens). Locks the
/// forget-is-App-only guarantee at the wire.
#[tokio::test]
async fn forget_suite_guest_cannot_forget_or_list() {
    let (_dir, sock) = spawn_onboarded_daemon().await;
    let mut guest = Guest::connect(&sock).await;

    let app_only = [
        Request::DeleteSession { onboarded: true, session_id: "s".into() },
        Request::SupersedeNote { onboarded: true, event_id: "e".into(), text: "x".into() },
        Request::ListSessions { onboarded: true },
        Request::GetSession { onboarded: true, session_id: "s".into() },
        Request::ListNotes { onboarded: true },
        Request::SetCaptureEnabled { onboarded: true, enabled: true, backfill: false },
        Request::CaptureEnabled { onboarded: true },
    ];
    for req in app_only {
        let resp = guest.call(req.clone()).await;
        assert!(
            matches!(resp, Response::Err { kind: OpErrorKindWire::NotPermitted, .. }),
            "{req:?} must be NotPermitted for MemoryClient, got {resp:?}"
        );
    }
}

/// GetSession on an unknown id is a clean `Rejected` ("session not found or deleted") — NOT a
/// generic Core fault — so the UI can tell an already-deleted session from a real error (spec §3).
#[tokio::test]
async fn get_session_not_found_is_clean_rejected() {
    let (_dir, sock) = spawn_onboarded_daemon().await;
    let mut app = App::connect(&sock).await;

    match app.call(Request::GetSession { onboarded: true, session_id: "no-such-session".into() }).await {
        Response::Err { kind: OpErrorKindWire::Rejected, message } => {
            assert!(message.contains("not found"), "the reject names not-found, got {message:?}");
            assert!(!message.contains("no-such-session"), "the reject never echoes the id");
        }
        other => panic!("expected a Rejected not-found, got {other:?}"),
    }
}

/// The capture-flag ops round-trip over the wire: CaptureEnabled defaults false, SetCaptureEnabled
/// flips it, and CaptureEnabled then reads true (the backfill flag's effect is A8/A9's concern —
/// here we only prove the wire op reaches the engine and moves the sticky flag).
#[tokio::test]
async fn set_capture_enabled_roundtrips_over_the_wire() {
    let (_dir, sock) = spawn_onboarded_daemon().await;
    let mut app = App::connect(&sock).await;

    match app.call(Request::CaptureEnabled { onboarded: true }).await {
        Response::CaptureEnabled(on) => assert!(!on, "capture is default-OFF"),
        other => panic!("expected CaptureEnabled, got {other:?}"),
    }
    assert!(matches!(
        app.call(Request::SetCaptureEnabled { onboarded: true, enabled: true, backfill: true }).await,
        Response::Ok
    ));
    match app.call(Request::CaptureEnabled { onboarded: true }).await {
        Response::CaptureEnabled(on) => assert!(on, "SetCaptureEnabled flipped the flag over the wire"),
        other => panic!("expected CaptureEnabled, got {other:?}"),
    }
}
