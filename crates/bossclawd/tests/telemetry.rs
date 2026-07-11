//! A12 — recall-miss telemetry + the App-only `RecallStats` op.
//!
//! Unit tests exercise [`Telemetry`] directly (hits/misses, rotation-proof counters, the capped
//! newest-first recent-miss ring, the best-effort/never-panic contract, and 0600/0700 perms). The
//! integration test drives a REAL daemon over its socket: an App records a miss then a hit and reads
//! them back via `RecallStats`, while a guest (`MemoryClient`) is denied the op. Unix-only.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use bossclawd::server;
use bossclawd::telemetry::Telemetry;
use bossclawd_proto::{
    read_frame, write_frame, Hello, HelloOk, OpErrorKindWire, Request, Response, Role, PROTO_VERSION,
};
use tokio::net::UnixStream;

fn mode_of(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// Hits are recorded, misses are recorded, misses store the QUERY (never result text), and rotating
/// the raw line log does NOT reset the durable counters or drop the recent-miss ring (critic m1).
#[test]
fn telemetry_records_hits_and_misses_and_counters_survive_rotation() {
    let dir = tempfile::tempdir().unwrap();
    let t = Telemetry::open(dir.path()).unwrap();
    t.record("query one", 0, None); // a miss (0 hits)
    t.record("query two", 3, Some(0.81)); // a hit
    t.record("café query", 0, None); // another miss (unicode)

    let s = t.stats().unwrap();
    assert_eq!(s.total, 3);
    assert_eq!(s.misses, 2);
    // recent_misses holds the miss QUERIES (never result text), newest-first, capped:
    assert!(s.recent_misses.iter().any(|m| m.query == "query one"));
    assert!(s.recent_misses.iter().any(|m| m.query == "café query"));
    assert!(s.recent_misses.iter().all(|m| m.query != "query two")); // hits are not misses

    // Rotation must NOT reset the durable counters (critic m1) or drop the recent-miss ring — both
    // live in their own files, independent of the rolled `recall.jsonl`.
    t.force_rotate().unwrap();
    let s2 = t.stats().unwrap();
    assert_eq!(s2.total, 3);
    assert_eq!(s2.misses, 2);
    assert!(s2.recent_misses.iter().any(|m| m.query == "café query"));
    // The raw line log was rolled to `.1`; the durable state is untouched.
    assert!(dir.path().join("telemetry").join("recall.jsonl.1").exists());
}

/// `record()` is best-effort: pointed at a store whose files can never be written, it swallows the
/// I/O errors and returns normally — it neither panics nor propagates an error that could fail a
/// recall (critic m2). The failure is constructed uid-independently (works even under root) by
/// making each target path a DIRECTORY, so opening it for writing can never succeed.
#[test]
fn telemetry_record_is_best_effort_never_panics() {
    let dir = tempfile::tempdir().unwrap();
    let t = Telemetry::open(dir.path()).unwrap();
    let tel_dir = dir.path().join("telemetry");
    std::fs::create_dir(tel_dir.join("recall.jsonl")).unwrap();
    std::fs::create_dir(tel_dir.join("counters.json")).unwrap();
    std::fs::create_dir(tel_dir.join("recent_misses.json")).unwrap();

    // Neither call may panic or propagate — `record` returns `()` regardless.
    t.record("unwritable miss", 0, None);
    t.record("unwritable hit", 3, Some(0.9));

    // stats() also survives the sabotaged store: unreadable files degrade to the 0/0/[] default.
    let s = t.stats().unwrap();
    assert_eq!(s.total, 0, "nothing could be written, so counters read back as the default");
    assert!(s.recent_misses.is_empty());
}

/// The recent-miss ring is capped at 20 and newest-first: after 30 misses, the newest survives, the
/// oldest is evicted, and the head is the most recent.
#[test]
fn recent_misses_capped_and_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    let t = Telemetry::open(dir.path()).unwrap();
    for i in 0..30 {
        t.record(&format!("m{i}"), 0, None);
    }
    let s = t.stats().unwrap();
    assert_eq!(s.total, 30);
    assert_eq!(s.misses, 30);
    assert_eq!(s.recent_misses.len(), 20);
    assert_eq!(s.recent_misses.first().unwrap().query, "m29", "newest-first");
    assert!(s.recent_misses.iter().any(|m| m.query == "m29"), "newest present");
    assert!(s.recent_misses.iter().all(|m| m.query != "m0"), "oldest evicted");
}

/// The telemetry dir is born 0700 and every file it writes is born 0600 (no world-readable window).
#[test]
fn telemetry_files_are_0600_under_0700_dir() {
    let dir = tempfile::tempdir().unwrap();
    let t = Telemetry::open(dir.path()).unwrap();
    t.record("q", 0, None); // a miss → writes recall.jsonl, counters.json AND recent_misses.json
    let _ = t.stats().unwrap();

    let tel_dir = dir.path().join("telemetry");
    assert_eq!(mode_of(&tel_dir), 0o700);
    assert_eq!(mode_of(&tel_dir.join("recall.jsonl")), 0o600);
    assert_eq!(mode_of(&tel_dir.join("counters.json")), 0o600);
    assert_eq!(mode_of(&tel_dir.join("recent_misses.json")), 0o600);
}

// ── Integration: a real daemon over its socket. ──────────────────────────────────────────────────

/// A framed client that can handshake in either role, mirroring `tests/memory_client_loop.rs`.
struct Client {
    stream: UnixStream,
}

impl Client {
    async fn connect(sock: &Path, role: Role) -> Self {
        let mut stream = UnixStream::connect(sock).await.expect("connect");
        let hello = Hello { proto_version: PROTO_VERSION, role };
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

/// Spawn a hermetic, onboarded daemon (in-memory vault + mock providers), returning its socket path.
/// Mirrors `tests/memory_client_loop.rs::spawn_onboarded_daemon`.
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

/// The Recall dispatch arm records telemetry, and the App-only `RecallStats` op reads it back — a
/// miss counts and surfaces its QUERY (never the hit query, never any note text), a hit counts but
/// is not a miss, and a guest is denied `RecallStats`.
///
/// The guaranteed miss is a recall on the EMPTY brain: the vector (ANN) arm returns the k nearest
/// neighbours whenever the index is non-empty, so once notes exist a recall is never truly empty —
/// the reliable 0-hit case is the pre-`remember` empty brain.
#[tokio::test]
async fn recall_dispatch_records_telemetry_and_recall_stats_returns_it() {
    let (dir, sock) = spawn_onboarded_daemon().await;
    let mut app = Client::connect(&sock, Role::App).await;

    // Recall #1 on the EMPTY brain → a guaranteed miss (0 hits).
    let miss_query = "nonexistent needle that matches nothing";
    match app.call(Request::Recall { onboarded: true, query: miss_query.into(), k: 8 }).await {
        Response::Recall(hits) => assert!(hits.is_empty(), "empty brain recalls nothing"),
        other => panic!("expected Recall(empty), got {other:?}"),
    }

    // A couple of remembered notes (these do NOT count as recalls).
    for text in ["kwang ships air", "aria novak dossier"] {
        match app.call(Request::Remember { onboarded: true, text: text.into() }).await {
            Response::Remember(_) => {}
            other => panic!("expected Remember(id), got {other:?}"),
        }
    }

    // Recall #2 → a hit (shares a token with the first note).
    let hit_query = "kwang air";
    match app.call(Request::Recall { onboarded: true, query: hit_query.into(), k: 8 }).await {
        Response::Recall(hits) => assert!(!hits.is_empty(), "matching query returns hits"),
        other => panic!("expected Recall(hits), got {other:?}"),
    }

    // RecallStats (App): total == 2 (both recalls), misses == 1, recent_misses has the miss query
    // and NOT the hit query and NOT any note text.
    match app.call(Request::RecallStats { onboarded: true }).await {
        Response::RecallStats(w) => {
            assert_eq!(w.total, 2, "both recalls counted; remember does not");
            assert_eq!(w.misses, 1, "only the empty-brain recall was a miss");
            assert!(w.recent_misses.iter().any(|m| m.query == miss_query), "miss query surfaces");
            assert!(
                w.recent_misses.iter().all(|m| m.query != hit_query),
                "the hit query is not a miss"
            );
            // Spec §7b honesty: no recall RESULT / note text ever resurfaces through telemetry.
            for note in ["kwang ships air", "aria novak dossier"] {
                assert!(
                    w.recent_misses.iter().all(|m| m.query != note),
                    "note text must never appear in recent_misses"
                );
            }
        }
        other => panic!("expected RecallStats, got {other:?}"),
    }

    // A guest (`MemoryClient`) is denied `RecallStats` (role.allows fail-closed).
    let mut guest = Client::connect(&sock, Role::MemoryClient).await;
    match guest.call(Request::RecallStats { onboarded: true }).await {
        Response::Err { kind, .. } => assert_eq!(kind, OpErrorKindWire::NotPermitted),
        other => panic!("expected Err(NotPermitted) for a guest RecallStats, got {other:?}"),
    }

    drop(dir);
}
