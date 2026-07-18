//! Rung-3 Phase-2 — the conflict SWEEPER's outside contract. Pins the fail-safe boundary from the
//! integration crate over a REAL hermetic `EngineHandle` (`server::test_engine`: in-memory vault +
//! mock embedder — no keychain, no network) on an onboarded temp data dir. The pure gate lives in
//! `run_conflict_sweep_once`; here we assert the load-bearing default: a fresh (detection-disabled)
//! brain gates off to a quiet no-op, never a panic.
//!
//! Unix-only: the whole `conflict` module is `#[cfg(unix)]` (it calls the cfg(unix) engine), so the
//! test compiles/runs on unix only (the CI matrix runs macOS + Linux), matching `tests/sweeper.rs`.
#![cfg(unix)]

use bossclawd::engine::EngineHandle;

/// Onboarded, hermetic engine + its data dir — copied from `tests/sweeper.rs::hermetic_engine`.
/// Seeds the in-memory vault, writes `identity.json` so the brain reads as onboarded, and returns
/// a bare `(EngineHandle, TempDir)` via the public `bossclawd::server::test_engine`.
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

#[tokio::test]
async fn gated_off_is_a_quiet_noop() {
    let (engine, dir) = hermetic_engine();
    // Detection is default-CLOSED → the sweep is a quiet no-op (gated_off), never a panic.
    let report =
        bossclawd::conflict::sweeper::run_conflict_sweep_once(&engine, dir.path(), 100).await;
    assert!(report.gated_off, "a fresh (disabled) brain gates off");
    assert_eq!(report.proposed, 0);
}

#[tokio::test]
async fn reasoner_unavailable_when_cloud_is_unconsented() {
    // The fail-safe contract this wrapper exists for: with detection ENABLED but a CLOUD reasoner
    // configured and NO signed consent, the engine's `cloud_consent_ok` pre-gate is false →
    // `detect_conflicts_once` returns `Err(Reasoner)` → the sweeper maps it to a quiet
    // `reasoner_unavailable` no-op, never a panic. Hermetic: no network, no signed consent exists.
    let (engine, dir) = hermetic_engine();
    let onboarded = true;
    engine.set_conflict_detect_enabled(onboarded, true).await.unwrap();
    // The exact cloud config the Task 9 `cloud_consent_ok_is_true_for_local_and_gates_unready_cloud`
    // test writes — cloud mode, no signed consent → the egress path stays closed.
    engine
        .set_reasoner_config(
            onboarded,
            serde_json::json!({
                "mode": "cloud",
                "provider": "anthropic",
                "model": "claude-sonnet-4-6",
                "base_url": null
            }),
        )
        .await
        .unwrap();

    let report =
        bossclawd::conflict::sweeper::run_conflict_sweep_once(&engine, dir.path(), 100).await;
    assert!(report.reasoner_unavailable, "an engine Err becomes a quiet no-op, not a panic");
    assert!(!report.gated_off, "detection is ENABLED — must NOT read as gated_off");
    assert_eq!(report.proposed, 0);
}

/// Whole-branch review (converged concurrency finding): the engine's `resolve_conflict` wrapper holds a
/// dedicated `resolve_lock` across its whole body, so two concurrent `ResolveConflict` calls for the SAME
/// proposal (realistic when a reconnecting MCP client double-submits) serialize into a deterministic
/// first-wins outcome — exactly one `Applied`, the other a clean `NoOp` — never a raced fail-loud
/// `Rejected("already retired"/"already resolved")` to one caller. We seed the proposal directly (the way
/// the core resolve tests do) because this exercises resolve serialization, not detection: detection stays
/// off, no reasoner is built, nothing egresses. Core stays unserialized BY DESIGN (its retired-set
/// roll-forward gate makes an interleaving benign); the mutex is what removes the spurious-Err flakiness.
#[tokio::test]
async fn concurrent_resolve_same_proposal_is_first_wins_applied_then_noop() {
    use bossclaw_core::{ConflictRef, ResolveAction, ResolveOutcome};
    let (engine, _dir) = hermetic_engine();
    let onboarded = true;

    // Seed two conflicting notes (real, current memories) + a `conflict_proposal` over that exact pair.
    let older = engine.remember(onboarded, "branch is main".to_string()).await.unwrap();
    let newer = engine.remember(onboarded, "branch is master".to_string()).await.unwrap();
    let log = engine.get_or_open(onboarded).await.unwrap();
    let a = ConflictRef::Note { event_id: older.clone() };
    let b = ConflictRef::Note { event_id: newer.clone() };
    let prop = log
        .append_conflict_proposal(&a, &b, "newer", "high", "why", 0, &[older, newer])
        .unwrap();
    drop(log); // do not hold the Arc across the concurrent resolves

    // Two concurrent wrapper calls, SAME proposal + SAME action. `resolve_lock` serializes them: the
    // first does the real work, the second WAITS then observes the terminal marker → clean NoOp.
    let (r1, r2) = tokio::join!(
        engine.resolve_conflict(onboarded, prop.clone(), ResolveAction::RetireOlder),
        engine.resolve_conflict(onboarded, prop.clone(), ResolveAction::RetireOlder),
    );
    let o1 = r1.expect("first concurrent resolve is Ok — never a raced fail-loud Err");
    let o2 = r2.expect("second concurrent resolve is Ok — never a raced fail-loud Err");

    // Multiset assertion (which task wins the acquire is not fixed): exactly one Applied + one NoOp.
    let applied = [&o1, &o2].iter().filter(|o| matches!(o, ResolveOutcome::Applied(_))).count();
    let noop = [&o1, &o2].iter().filter(|o| matches!(o, ResolveOutcome::NoOp)).count();
    assert_eq!(applied, 1, "exactly one call applies the resolution (first-wins)");
    assert_eq!(noop, 1, "the serialized second call is a clean idempotent no-op");

    // State corroboration: exactly ONE conflict-scoped retire marker (never a double-retire), and the
    // resolved pair has left the pending set (the frozen loser is retired → non-current).
    let log = engine.get_or_open(onboarded).await.unwrap();
    let digest = log.conflict_digest_counts(0).unwrap();
    assert_eq!(digest.retired, 1, "exactly one conflict-driven retire marker");
    assert!(
        log.pending_conflict_proposals().unwrap().is_empty(),
        "the resolved pair is no longer pending",
    );
}
