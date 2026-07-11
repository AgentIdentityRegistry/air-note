//! A7 — the daemon's capture store: rendered Markdown lands on disk (0600 under a
//! 0700 dir), then a signed `session_captured` event ties it to the log. These tests
//! pin the contract from the OUTSIDE (integration crate), over a REAL hermetic
//! `EngineHandle` (`server::test_engine`: in-memory vault + mock embedder, so no
//! keychain and no network), an onboarded temp home, and a temp data dir.
//!
//! Crash-consistency (spec §4b): the store writes the file THEN the event, and
//! `heal_orphans` reconciles both windows on boot / before each sweep.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use bossclawd::capture::render::Rendered;
use bossclawd::capture::store::{delete_capture, heal_orphans, store_capture, CaptureIdentity};
use bossclawd::engine::EngineHandle;

/// A hermetic, onboarded engine + a temp data dir (the two the store needs). Mirrors
/// `memory_client_loop.rs`'s `spawn_onboarded_daemon`, but keeps the `EngineHandle`
/// in-process (no socket) since the store calls it directly. The engine's data dir and
/// the store's data dir are the SAME temp dir — exactly the production invariant
/// (`resolve_data_dir()` feeds both).
fn hermetic_engine() -> (EngineHandle, tempfile::TempDir) {
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
    (bossclawd::server::test_engine(home), dir)
}

/// A `Rendered` with a known body + sha, standing in for A6's output. The store never
/// reads `markdown` (it composes its own front-matter from the raw parts + the session
/// fields and appends `body`), so `markdown` here is a throwaway.
fn sample_rendered() -> Rendered {
    Rendered {
        title: "deterministic transcript rendering".into(),
        markdown: "IGNORED BY THE STORE".into(),
        body: "## You\nHow do I render deterministically?\n\n## Assistant\nLike so.\n\n".into(),
        sha256: "aa".repeat(32),
        started_at: 1_783_764_000,
        ended_at: 1_783_764_009,
        approx_bytes: 42,
        dropped_torn_tail: false,
        oversized_lines: 0,
        skipped_unknown: 0,
    }
}

fn id(session_id: &str) -> CaptureIdentity {
    CaptureIdentity {
        session_id: session_id.into(),
        project: "/repo".into(),
        tool: "claude-code".into(),
    }
}

#[tokio::test]
async fn store_writes_0600_md_under_0700_dir_then_event() {
    let (engine, data_dir) = hermetic_engine();
    let rendered = sample_rendered();
    store_capture(&engine, data_dir.path(), &id("abc-123"), &rendered)
        .await
        .unwrap();

    let md = data_dir.path().join("sessions/abc-123.md");
    assert!(md.exists(), "the .md must be written");
    assert_eq!(
        md.metadata().unwrap().permissions().mode() & 0o777,
        0o600,
        "the .md is owner-only (0600)"
    );
    assert_eq!(
        md.parent().unwrap().metadata().unwrap().permissions().mode() & 0o777,
        0o700,
        "the sessions dir is owner-only (0700)"
    );

    // The signed event exists and the fold sees it.
    let cur = engine.current_sessions().await.unwrap();
    assert_eq!(cur.len(), 1);
    assert_eq!(cur[0].session_id, "abc-123");
    assert_eq!(cur[0].sha256, rendered.sha256, "event sha mirrors the render");
    assert_eq!(
        cur[0].path,
        md.to_string_lossy(),
        "the event records the .md path as metadata"
    );

    // The stored .md carries the session fields the renderer never knew (merged front-matter).
    let body = std::fs::read_to_string(&md).unwrap();
    assert!(body.starts_with("---\n"), "one front-matter block");
    assert!(body.contains("session_id: abc-123"));
    assert!(body.contains("project: /repo"));
    assert!(body.contains("tool: claude-code"));
    assert!(body.contains(&rendered.sha256));
    assert!(body.contains("How do I render deterministically?"), "the body is preserved");
}

#[tokio::test]
async fn store_rejects_invalid_session_id_before_touching_fs() {
    let (engine, data_dir) = hermetic_engine();
    let r = store_capture(&engine, data_dir.path(), &id("../evil"), &sample_rendered()).await;
    assert!(r.is_err(), "a traversal id must be refused");

    // Nothing was written — the reject is BEFORE any fs touch (A5 D1, defense in depth).
    let sessions = data_dir.path().join("sessions");
    assert!(
        !sessions.exists() || std::fs::read_dir(&sessions).unwrap().count() == 0,
        "no file created for a rejected id"
    );
    // And no event either.
    assert!(engine.current_sessions().await.unwrap().is_empty());
}

#[tokio::test]
async fn recapture_same_content_is_idempotent_noop() {
    let (engine, data_dir) = hermetic_engine();
    let rendered = sample_rendered();

    store_capture(&engine, data_dir.path(), &id("abc-123"), &rendered).await.unwrap();
    let cur1 = engine.current_sessions().await.unwrap();
    assert_eq!(cur1.len(), 1);
    let event_id_1 = cur1[0].event_id.clone();

    // Store the SAME rendered again (same sha256) → A2's dedup makes it a no-op.
    store_capture(&engine, data_dir.path(), &id("abc-123"), &rendered).await.unwrap();
    let cur2 = engine.current_sessions().await.unwrap();
    assert_eq!(cur2.len(), 1, "no duplicate session");
    assert_eq!(cur2[0].event_id, event_id_1, "same event id — dedup no-op, not a new capture");

    // Exactly one .md on disk.
    let count = std::fs::read_dir(data_dir.path().join("sessions")).unwrap().count();
    assert_eq!(count, 1, "no duplicate .md");
}

#[tokio::test]
async fn delete_capture_removes_md_and_tombstones() {
    let (engine, data_dir) = hermetic_engine();
    store_capture(&engine, data_dir.path(), &id("abc-123"), &sample_rendered()).await.unwrap();
    let md = data_dir.path().join("sessions/abc-123.md");
    assert!(md.exists());
    assert_eq!(engine.current_sessions().await.unwrap().len(), 1);

    delete_capture(&engine, data_dir.path(), "abc-123").await.unwrap();
    assert!(!md.exists(), "the .md is gone");
    assert!(engine.current_sessions().await.unwrap().is_empty(), "the session is tombstoned");

    // Deleting again (missing .md, already tombstoned) is a benign no-op — idempotent.
    delete_capture(&engine, data_dir.path(), "abc-123").await.unwrap();
    assert!(engine.current_sessions().await.unwrap().is_empty());
}

/// heal_orphans reconciles the two crash windows around the file-then-event write.
///
/// Direction chosen for window (b) — a current event whose .md is missing (an
/// out-of-band deletion; NOT a store crash window, since the file is written first):
/// **regenerate a minimal .md from the signed event's metadata** (front-matter rebuilt
/// from the `CurrentSession`, body a clear "not recovered" note). The append-only signed
/// event is the durable source of truth; the .md is a derived view, so we rebuild the
/// view rather than tombstone a memory the owner never asked to delete (I7 — deletion is
/// owner-commanded). It self-heals to the real body if the source transcript is re-swept
/// (store_capture always writes the file first). This preserves the event⇔file invariant
/// without inventing transcript content.
#[tokio::test]
async fn heal_orphans_covers_both_crash_windows() {
    let (engine, data_dir) = hermetic_engine();
    let sessions = data_dir.path().join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();

    // (a) A .md with valid front-matter but NO engine event (crash after file, before append).
    // Hand-write it in the exact schema store_capture emits (sorted keys, one block).
    let orphan_md = "---\n\
        approx_bytes: 10\n\
        ended_at: 1783764009\n\
        lines_oversized: 0\n\
        lines_skipped: 0\n\
        project: /repo\n\
        session_id: orphan1\n\
        sha256: bb\n\
        started_at: 1783764000\n\
        title: an orphaned capture\n\
        tool: claude-code\n\
        torn_tail: false\n\
        ---\n\n## You\nhi\n\n";
    write_file(&sessions.join("orphan1.md"), orphan_md);

    // (b) A normally-captured session whose .md we then delete out of band (event survives).
    store_capture(&engine, data_dir.path(), &id("live1"), &sample_rendered()).await.unwrap();
    std::fs::remove_file(sessions.join("live1.md")).unwrap();

    let report = heal_orphans(&engine, data_dir.path()).await.unwrap();
    assert_eq!(report.orphan_files_captured, 1, "window (a): the orphan .md gets an event");
    assert_eq!(report.dangling_events_regenerated, 1, "window (b): the dangling event gets a file");

    // (a) the orphan file now has a current event.
    let cur = engine.current_sessions().await.unwrap();
    assert!(cur.iter().any(|c| c.session_id == "orphan1"), "orphan1 healed into an event");
    // (b) the dangling event's .md is regenerated (event⇔file invariant restored).
    assert!(sessions.join("live1.md").exists(), "live1.md regenerated from its event");
    assert!(cur.iter().any(|c| c.session_id == "live1"), "live1 still a current session");

    // Idempotent: a second heal on a now-consistent store does nothing.
    let again = heal_orphans(&engine, data_dir.path()).await.unwrap();
    assert_eq!(again.orphan_files_captured, 0);
    assert_eq!(again.dangling_events_regenerated, 0);
}

fn write_file(path: &Path, contents: &str) {
    std::fs::write(path, contents).unwrap();
}
