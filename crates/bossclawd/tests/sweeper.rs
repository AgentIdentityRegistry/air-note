//! A9 — the daemon's capture SWEEPER: durability + backfill. These tests pin the contract
//! from the OUTSIDE (integration crate) over a REAL hermetic `EngineHandle`
//! (`server::test_engine`: in-memory vault + mock embedder — no keychain, no network), an
//! onboarded temp data dir, and a FAKE Claude projects root pointed at via
//! `AIR_CLAUDE_PROJECTS_ROOT` so nothing touches the real `~/.claude/projects`.
//!
//! The pure decision core (`decide_sweep`) is unit-tested inside the module; here we exercise
//! the effectful `run_sweep_once` end-to-end: the gate (I10), the quiet-mtime floor, backfill,
//! idempotency, the 0600/0700 on-disk discipline, and the recovery-stub self-heal across sweeps.
#![cfg(unix)]

use std::fs::File;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::Mutex;

use bossclawd::capture::sweeper::run_sweep_once;
use bossclawd::engine::EngineHandle;

/// Serializes the tests that mutate the process-global `AIR_CLAUDE_PROJECTS_ROOT` so their fake
/// roots never race under cargo's parallel test threads. A tokio `Mutex` is await-safe (held
/// across the sweeps without tripping `clippy::await_holding_lock`, unlike `std::sync::Mutex`).
static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
fn env_lock() -> &'static Mutex<()> {
    ENV_LOCK.get_or_init(|| Mutex::new(()))
}

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

/// Stamp `path`'s mtime to `SystemTime::now() - age` (needs a writable handle).
fn set_mtime(path: &Path, age: Duration) {
    let when = SystemTime::now().checked_sub(age).unwrap();
    File::options().write(true).open(path).unwrap().set_modified(when).unwrap();
}

/// Write a minimal real-shape Claude Code transcript (ends with a newline → no torn tail) and
/// stamp its mtime to `now - age`, so a single sweep can distinguish a quiet file from a
/// still-fresh one deterministically (no sleeps).
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
    set_mtime(path, age);
}

/// I10: a non-connected brain (capture disabled) sweeps to a no-op — nothing scanned, NO
/// `sessions/` directory, no events. Never reads the projects root, so it needs no env/lock.
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
    let _env = env_lock().lock().await; // serialize AIR_CLAUDE_PROJECTS_ROOT use.
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

/// The regression the spec review caught (MEDIUM): a heal-(b) recovery STUB carries the event's
/// REAL sha + a fresh mtime, so a naive skip (md_exists && same_sha) would skip it FOREVER and the
/// placeholder body would never become the real transcript. The fix marks the stub `recovered:
/// false`; the sweeper treats a stub as non-adoptable so it re-renders when the transcript is
/// present. This drives the full window over `run_sweep_once`: capture → out-of-band .md loss
/// while the transcript is fresh → heal stubs it → once quiet, the next sweep re-renders the real
/// body over the stub. A transcript-GONE stub would stay put (not a candidate) — no churn.
#[tokio::test]
async fn sweep_re_renders_recovery_stub_across_sweeps() {
    let _env = env_lock().lock().await; // serialize AIR_CLAUDE_PROJECTS_ROOT use.
    let (engine, data_dir) = hermetic_engine();

    let projects = tempfile::tempdir().unwrap();
    let proj_dir = projects.path().join("-Users-tester-repo");
    std::fs::create_dir_all(&proj_dir).unwrap();
    let transcript = proj_dir.join("sess-heal.jsonl");
    let prompt = "explain the heal window in detail";
    write_transcript(&transcript, prompt, Duration::from_secs(3600)); // quiet

    let now = now_epoch();
    engine
        .set_capture_enabled(/*onboarded=*/ true, /*enabled=*/ true, /*backfill=*/ true, /*at=*/ now)
        .await
        .unwrap();
    std::env::set_var("AIR_CLAUDE_PROJECTS_ROOT", projects.path());
    let md = data_dir.path().join("sessions/sess-heal.md");

    // ── Sweep 1: capture the REAL body (a real render omits the `recovered` marker). The
    // rendered conversation carries a `## You` heading — the tell of a real body (the prompt
    // ALSO appears in the `title:` front-matter, so it is NOT a body-vs-stub discriminator).
    let r1 = run_sweep_once(&engine, data_dir.path(), now).await;
    assert_eq!(r1.captured, 1);
    let real = std::fs::read_to_string(&md).unwrap();
    assert!(real.contains("## You"), "sweep 1 rendered the real conversation body");
    assert!(real.contains(prompt), "the real body carries the prompt");
    assert!(!real.contains("recovered: false"), "a real render carries no recovered marker");

    // ── Window-(b) loss WHILE the transcript is fresh (still being written): the .md is lost
    // out of band and the transcript is touched, so the sweep's quiet floor won't re-render it —
    // heal-(b) regenerates it as a STUB instead.
    std::fs::remove_file(&md).unwrap();
    set_mtime(&transcript, Duration::from_secs(0)); // fresh → quiet-floored next sweep.

    let r2 = run_sweep_once(&engine, data_dir.path(), now).await;
    assert_eq!(r2.captured, 0, "the fresh transcript is quiet-floored, not re-captured this sweep");
    assert_eq!(r2.heal.dangling_events_regenerated, 1, "heal regenerated the missing .md as a stub");
    let stub = std::fs::read_to_string(&md).unwrap();
    assert!(stub.contains("recovered: false"), "the regenerated .md is marked an unrecovered stub");
    assert!(!stub.contains("## You"), "the stub carries the placeholder body, not the rendered conversation");

    // ── The session goes quiet again (ended / time passed).
    set_mtime(&transcript, Duration::from_secs(3600));

    // ── Sweep 3: the stub's transcript is present + quiet → `md_is_stub` forces a re-render of
    // the REAL body over the stub. THIS is what the bug broke (it skipped the stub forever).
    let r3 = run_sweep_once(&engine, data_dir.path(), now).await;
    assert_eq!(r3.captured, 1, "the stub re-renders on the next sweep (self-heal)");
    let healed = std::fs::read_to_string(&md).unwrap();
    assert!(healed.contains("## You"), "sweep 3 replaced the stub with the real conversation body");
    assert!(healed.contains(prompt), "the healed body carries the prompt");
    assert!(!healed.contains("recovered: false"), "the recovered marker is gone — a real render again");

    // No churn/duplication: exactly one current session, exactly one .md.
    assert_eq!(engine.current_sessions().await.unwrap().len(), 1);
    assert_eq!(std::fs::read_dir(data_dir.path().join("sessions")).unwrap().count(), 1);

    std::env::remove_var("AIR_CLAUDE_PROJECTS_ROOT");
}
