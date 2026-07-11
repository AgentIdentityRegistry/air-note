//! A9 — the daemon's capture SWEEPER: durability + backfill. These tests pin the contract
//! from the OUTSIDE (integration crate) over a REAL hermetic `EngineHandle`
//! (`server::test_engine`: in-memory vault + mock embedder — no keychain, no network), an
//! onboarded temp data dir, and a FAKE Claude projects root pointed at via
//! `AIR_CLAUDE_PROJECTS_ROOT` so nothing touches the real `~/.claude/projects`.
//!
//! The pure decision core (`decide_sweep`) is unit-tested inside the module; here we exercise
//! the effectful `run_sweep_once` end-to-end: the gate (I10), the quiet-mtime floor, backfill,
//! idempotency, and the 0600/0700 on-disk discipline.
#![cfg(unix)]

use std::fs::File;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bossclawd::capture::sweeper::run_sweep_once;
use bossclawd::engine::EngineHandle;

/// A hermetic, onboarded engine + its data dir (mirrors `capture_store.rs::hermetic_engine`).
/// The engine's data dir and the sweeper's data dir are the SAME temp dir — the production
/// invariant (`resolve_data_dir()` feeds both).
fn hermetic_engine() -> (EngineHandle, tempfile::TempDir) {
    bossclawd::vault::seed_secret_cache_for_test(Default::default());
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("identity.json"),
        serde_json::json!({
            "did": "did:wba:example.com:tester",
            "name": "Tester",
            "created_at": "2026-07-11T00:00:00+00:00"
        })
        .to_string(),
    )
    .unwrap();
    (bossclawd::server::test_engine(dir.path().to_path_buf()), dir)
}

fn now_epoch() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
}

/// Write a minimal real-shape Claude Code transcript (ends with a newline → no torn tail) and
/// stamp its mtime to `SystemTime::now() - age`, so a single sweep can distinguish a quiet file
/// from a still-fresh one deterministically (no sleeps).
fn write_transcript(path: &Path, prompt: &str, age: Duration) {
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
    let when = SystemTime::now().checked_sub(age).unwrap();
    File::options().write(true).open(path).unwrap().set_modified(when).unwrap();
}

/// I10: a non-connected brain (capture disabled) sweeps to a no-op — nothing scanned, NO
/// `sessions/` directory, no events. Never reads the projects root, so it needs no env.
#[tokio::test]
async fn sweep_gated_off_creates_nothing() {
    let (engine, data_dir) = hermetic_engine();
    // Fresh brain: capture_enabled defaults OFF.
    let report = run_sweep_once(&engine, data_dir.path(), now_epoch()).await;
    assert!(report.gated_off, "a capture-disabled brain gates the whole sweep off");
    assert_eq!(report.captured, 0);
    assert!(!data_dir.path().join("sessions").exists(), "I10: no directories created when gated off");
    assert!(engine.current_sessions().await.unwrap().is_empty(), "no events recorded");
}

/// The core loop: one sweep captures the QUIET transcript (0600 .md under a 0700 dir, a signed
/// event, project = the slug dir name), SKIPS the still-fresh one (quiet floor), and a second
/// sweep is an idempotent no-op (.md exists + same sha → cheap skip).
#[tokio::test]
async fn sweep_captures_quiet_skips_fresh_and_is_idempotent() {
    let (engine, data_dir) = hermetic_engine();

    // A fake projects root: <root>/<slug>/<session-id>.jsonl (Claude Code's layout).
    let projects = tempfile::tempdir().unwrap();
    let slug = "-Users-tester-repo";
    let proj_dir = projects.path().join(slug);
    std::fs::create_dir_all(&proj_dir).unwrap();
    write_transcript(&proj_dir.join("sess-quiet.jsonl"), "how do I capture sessions?", Duration::from_secs(3600));
    write_transcript(&proj_dir.join("sess-fresh.jsonl"), "still typing right now", Duration::from_secs(0));

    // Enable capture WITH backfill so the quiet (pre-enable-instant) transcript is imported.
    let now = now_epoch();
    engine
        .set_capture_enabled(/*onboarded=*/ true, /*enabled=*/ true, /*backfill=*/ true, /*at=*/ now)
        .await
        .unwrap();

    // Point the sweeper at the fake root ONLY for this test (unique env var; removed at the end).
    std::env::set_var("AIR_CLAUDE_PROJECTS_ROOT", projects.path());

    let report = run_sweep_once(&engine, data_dir.path(), now).await;
    assert!(!report.gated_off);
    assert_eq!(report.scanned, 2, "both .jsonl files are valid candidates");
    assert_eq!(report.captured, 1, "only the quiet transcript is captured; the fresh one is skipped");
    assert_eq!(report.render_failures, 0);
    assert_eq!(report.store_failures, 0);

    // The quiet session is now a signed current capture, tagged with the slug as its project.
    let cur = engine.current_sessions().await.unwrap();
    assert_eq!(cur.len(), 1, "exactly the quiet session");
    assert_eq!(cur[0].session_id, "sess-quiet");
    assert_eq!(cur[0].project, slug, "project = the Claude Code slug dir name (A9 contract)");
    assert_eq!(cur[0].tool, "claude-code");

    // Its .md is on disk, owner-only, under an owner-only sessions dir; the fresh one is absent.
    let quiet_md = data_dir.path().join("sessions/sess-quiet.md");
    assert!(quiet_md.exists(), "the quiet capture's .md was written");
    assert_eq!(quiet_md.metadata().unwrap().permissions().mode() & 0o777, 0o600, "the .md is 0600");
    assert_eq!(
        quiet_md.parent().unwrap().metadata().unwrap().permissions().mode() & 0o777,
        0o700,
        "the sessions dir is 0700"
    );
    assert!(!data_dir.path().join("sessions/sess-fresh.md").exists(), "the fresh transcript was not captured");
    let body = std::fs::read_to_string(&quiet_md).unwrap();
    assert!(body.contains("how do I capture sessions?"), "the rendered body is present");

    // ── Second sweep (same clock): idempotent no-op — quiet skipped (.md + same sha), fresh
    // still too fresh. No duplicate .md, same single event.
    let event_id = cur[0].event_id.clone();
    let report2 = run_sweep_once(&engine, data_dir.path(), now).await;
    assert_eq!(report2.captured, 0, "nothing re-captured on the second sweep");
    let cur2 = engine.current_sessions().await.unwrap();
    assert_eq!(cur2.len(), 1);
    assert_eq!(cur2[0].event_id, event_id, "same event — dedup no-op, not a new capture");
    assert_eq!(
        std::fs::read_dir(data_dir.path().join("sessions")).unwrap().count(),
        1,
        "still exactly one .md"
    );

    std::env::remove_var("AIR_CLAUDE_PROJECTS_ROOT");
}
