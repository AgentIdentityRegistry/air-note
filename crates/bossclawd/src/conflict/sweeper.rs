//! The conflict-detection sweep loop. Mirrors `crate::capture::sweeper`: a pure gate + a thin
//! tokio loop reading the wall clock at the boundary. All heavy work (find → judge → emit) is one
//! `EngineHandle::detect_conflicts_once` call (itself gated + serialized + `spawn_blocking`).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use crate::capture::sweeper::SWEEP_INTERVAL; // piggyback the capture cadence (300s)
use crate::engine::EngineHandle;

/// What one [`run_conflict_sweep_once`] did. All-zero + `gated_off` on a disabled/non-connected
/// brain (I3). `reasoner_unavailable` marks a cloud-not-ready / reasoner-down no-op (I6).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ConflictSweepReport {
    /// Not onboarded OR conflict-detect disabled — nothing scanned, no model call (I3).
    pub gated_off: bool,
    /// The cycle could not complete this pass for any transient reason — cloud not consented,
    /// reasoner down, a cycle already running (`Busy`), or a transient open/join failure. A safe
    /// no-op; retried next cycle (I6).
    pub reasoner_unavailable: bool,
    /// Judge calls made this cycle.
    pub judged: usize,
    /// Proposals emitted.
    pub proposed: usize,
    /// Pairs the judge declined.
    pub dropped: usize,
    /// The per-cycle judge budget was hit.
    pub budget_hit: bool,
    /// The open-proposal ceiling was hit.
    pub ceiling_hit: bool,
    /// Pairs abandoned this run after `CONFLICT_PAIR_ERROR_BUDGET` consecutive reasoner errors (§3.3) —
    /// surfaced so a poison-skip is never a silent drop.
    pub poison_skipped: usize,
}

/// Run ONE conflict-detection sweep: gate → delegate → map the core report. `now` is the
/// wall-clock epoch second (read by [`spawn`] at the boundary). Never panics; a reasoner/engine
/// error becomes a quiet `reasoner_unavailable` no-op (I6 — retry next cycle).
pub async fn run_conflict_sweep_once(
    engine: &EngineHandle,
    data_dir: &Path,
    now: i64,
) -> ConflictSweepReport {
    let onboarded = crate::identity::is_onboarded(data_dir);
    if !onboarded || !engine.conflict_detect_enabled_or_false(onboarded).await {
        return ConflictSweepReport { gated_off: true, ..Default::default() };
    }
    match engine.detect_conflicts_once(onboarded, now).await {
        Ok(r) => ConflictSweepReport {
            judged: r.judged,
            proposed: r.proposed,
            dropped: r.dropped,
            budget_hit: r.budget_hit,
            ceiling_hit: r.ceiling_hit,
            poison_skipped: r.poison_skipped,
            ..Default::default()
        },
        // Busy / reasoner-not-ready / transient open failure → a safe no-op this cycle (I6).
        Err(_) => ConflictSweepReport { reasoner_unavailable: true, ..Default::default() },
    }
}

/// Spawn the background conflict-sweep loop (mirrors `capture::sweeper::spawn`). The first tick
/// fires immediately; `MissedTickBehavior::Skip` prevents catch-up bursts. Detection stays OFF
/// until the owner enables it — the gate lives inside `run_conflict_sweep_once`, so a disabled
/// brain does zero work here. A panic in this task cannot take down the daemon.
pub fn spawn(engine: Arc<EngineHandle>, data_dir: PathBuf) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(SWEEP_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            // Read the wall clock HERE — the boundary — via the shared capture-sweeper helper, so
            // the core stays clock-free and both sweepers epoch-convert identically.
            let now = crate::capture::sweeper::system_time_to_epoch(Some(SystemTime::now()));
            let report = run_conflict_sweep_once(&engine, &data_dir, now).await;
            // Surface only real work (mirrors the capture sweeper's quiet-on-noop discipline).
            if report.proposed > 0
                || report.dropped > 0
                || report.budget_hit
                || report.ceiling_hit
                || report.poison_skipped > 0
            {
                eprintln!(
                    "conflict-sweep: proposed {} / judged {} (dropped {}, budget-hit {}, ceiling-hit {}, poison-skipped {})",
                    report.proposed,
                    report.judged,
                    report.dropped,
                    report.budget_hit,
                    report.ceiling_hit,
                    report.poison_skipped
                );
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{embed, reason};

    /// The ENABLED/proposing projection: a real cycle that emits one proposal must map 1:1 onto the
    /// sweep report (`Ok(r) => { judged, proposed, … }`). The integration crate can't reach this —
    /// its `test_engine` uses a dim-8 embedder + response-less reasoner, so the marquee pair never
    /// forms a proposal. Built INLINE like the engine e2e `engine_detect_conflicts_once_emits_a_
    /// proposal_when_enabled`: dim-64 mock embedder + a scripted judge + two contradicting notes,
    /// with `identity.json` on disk so the sweeper's `is_onboarded` gate passes. Guards against a
    /// silent field transposition in the projection.
    #[tokio::test]
    async fn enabled_cycle_projects_a_proposal() {
        // Keychain-free: in-memory vault + empty provider-key cache (Local mode never reads a key).
        crate::vault::seed_secret_cache_for_test(Default::default());
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

        // Two contradicting notes; the OLDER (`a`, remembered first) is presented first, so a single
        // scripted ordering suffices — mirrors the engine e2e fixture exactly.
        let a = "the default deploy target is vercel";
        let b = "the default deploy target is fly";
        let reasoner: Arc<dyn bossclaw_core::Reasoner> =
            Arc::new(bossclaw_core::ScriptedReasoner::new("test").with_response(
                bossclaw_core::conflict::CONFLICT_SYSTEM,
                &bossclaw_core::conflict::build_conflict_prompt(a, b),
                serde_json::json!({ "contradicts": true, "winner": "newer", "confidence": 92, "why": "renamed" }),
            ));
        let engine = EngineHandle::new(
            crate::server::shared_test_vault(),
            dir.path().to_path_buf(),
            Arc::new(embed::MockEmbedderProvider::new(64)),
            Arc::new(reason::MockReasonerProvider::from_reasoner(reasoner)),
        );

        let onboarded = true;
        engine.remember(onboarded, a.to_string()).await.unwrap();
        engine.remember(onboarded, b.to_string()).await.unwrap();
        engine.set_conflict_detect_enabled(onboarded, true).await.unwrap();

        let report = run_conflict_sweep_once(&engine, dir.path(), 100).await;
        assert!(!report.gated_off, "detection is ENABLED and onboarded — not gated_off");
        assert!(!report.reasoner_unavailable, "the Local-mode cycle completed — not a no-op");
        assert_eq!(report.proposed, 1, "one contradiction → one proposal projected");
        assert!(report.judged >= 1, "the judge was called at least once");
    }
}
