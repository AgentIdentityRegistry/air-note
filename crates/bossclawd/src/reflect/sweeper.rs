use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use bossclaw_core::reflect::{REFLECT_QUIET_SECS, REFLECT_STALENESS_FLOOR_SECS};

use crate::capture::sweeper::SWEEP_INTERVAL; // piggyback the 300s cadence (I2, conflict-sweeper precedent)
use crate::engine::EngineHandle;

/// The plain-data inputs to the PURE reflect gate (spec §2.1). Clock-free: `now` + the newest activity ts
/// are passed in. No fs / engine / lock.
#[derive(Debug, Clone)]
pub struct ReflectDecisionInput {
    pub onboarded: bool,
    pub reflect_enabled: bool,
    pub reasoner_ready: bool,
    pub now: i64,
    /// Newest memory-class event epoch, or `None` (no activity ever → quiet).
    pub newest_activity_at: Option<i64>,
    pub evolve_enabled: bool,
    pub evolve_queue_depth: usize,
    pub open_unparked_misses: usize,
    pub last_completed_run_at: i64,
    pub last_floor_fire_at: i64,
}

/// The gate verdict (spec §2.1). `Run.floor_fired` distinguishes a starvation-floor tick (bounded wake-time
/// work) from an ordinary quiet tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReflectDecision {
    Run { floor_fired: bool },
    GatedOff,
    ReasonerUnavailable,
    NotQuiet,
    DeferredEvolveBacklog,
}

/// The PURE tick decision (spec §2.1). Gate order: hard gates (onboarded ∧ enabled ∧ reasoner-ready) →
/// the starvation FLOOR (which overrides BOTH the quiet gate AND the evolve-backlog defer, §2.1 precedence:
/// a wedged evolve queue can never starve reflection) → quiet → evolve-backlog defer → run. The floor fires
/// at most once per `REFLECT_STALENESS_FLOOR_SECS` (the last-floor-fire guard).
pub fn decide_reflect(i: &ReflectDecisionInput) -> ReflectDecision {
    if !i.onboarded || !i.reflect_enabled {
        return ReflectDecision::GatedOff;
    }
    if !i.reasoner_ready {
        return ReflectDecision::ReasonerUnavailable; // cloud never silently falls back to local (§2.1)
    }
    // Starvation floor: unparked misses exist AND long since the last COMPLETED run AND not fired recently.
    let floor = i.open_unparked_misses > 0
        && i.now - i.last_completed_run_at > REFLECT_STALENESS_FLOOR_SECS
        && i.now - i.last_floor_fire_at > REFLECT_STALENESS_FLOOR_SECS;
    if floor {
        return ReflectDecision::Run { floor_fired: true }; // overrides quiet AND evolve-backlog defer
    }
    // Quiet: newest memory-class append older than the window (no activity ever = quiet).
    let quiet = i.newest_activity_at.is_none_or(|t| i.now - t >= REFLECT_QUIET_SECS);
    if !quiet {
        return ReflectDecision::NotQuiet;
    }
    // Evolve-backlog defer: the daytime helper goes first (its extraction feeds the entity graph).
    if i.evolve_enabled && i.evolve_queue_depth > 0 {
        return ReflectDecision::DeferredEvolveBacklog;
    }
    ReflectDecision::Run { floor_fired: false }
}

/// What one [`run_reflect_sweep_once`] did (mirrors `ConflictSweepReport`). All-zero + `gated_off` on a
/// disabled/non-onboarded brain (I3). No silent caps — `unhealable_thin` et al. surface in `report`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReflectSweepReport {
    pub gated_off: bool,
    pub not_quiet: bool,
    pub deferred_evolve_backlog: bool,
    pub floor_fired: bool,
    pub reasoner_unavailable: bool,
    pub report: bossclaw_core::ReflectReport,
}

/// Run ONE reflect sweep: gather the pure gate's inputs → `decide_reflect` → on `Run`, delegate to
/// `EngineHandle::reflect_once` and (on a floor tick) stamp the last-floor-fire marker. `now` is the
/// wall-clock epoch second (read by [`spawn`] at the boundary). Never panics; a reasoner/engine error
/// becomes a quiet `reasoner_unavailable` no-op (I6 — retry next cycle).
pub async fn run_reflect_sweep_once(engine: &EngineHandle, data_dir: &Path, now: i64) -> ReflectSweepReport {
    let onboarded = crate::identity::is_onboarded(data_dir);
    let reflect_enabled = onboarded && engine.reflect_enabled_or_false(onboarded).await;
    if !reflect_enabled {
        return ReflectSweepReport { gated_off: true, ..Default::default() };
    }
    let reasoner_ready = engine.reflect_reasoner_ready(onboarded).await;
    let evolve_enabled = engine.evolve_enabled_or_false(onboarded).await;
    let evolve_queue_depth = engine.queue_depth_or_zero(onboarded).await;
    let Some(g) = engine.reflect_gate_inputs(onboarded).await else {
        return ReflectSweepReport { reasoner_unavailable: true, ..Default::default() }; // open failure → no-op
    };
    let decision = decide_reflect(&ReflectDecisionInput {
        onboarded,
        reflect_enabled,
        reasoner_ready,
        now,
        newest_activity_at: g.newest_activity_at,
        evolve_enabled,
        evolve_queue_depth,
        open_unparked_misses: g.open_unparked_misses,
        last_completed_run_at: g.last_completed_run_at,
        last_floor_fire_at: g.last_floor_fire_at,
    });
    match decision {
        ReflectDecision::GatedOff => ReflectSweepReport { gated_off: true, ..Default::default() },
        ReflectDecision::ReasonerUnavailable => {
            ReflectSweepReport { reasoner_unavailable: true, ..Default::default() }
        }
        ReflectDecision::NotQuiet => ReflectSweepReport { not_quiet: true, ..Default::default() },
        ReflectDecision::DeferredEvolveBacklog => {
            ReflectSweepReport { deferred_evolve_backlog: true, ..Default::default() }
        }
        ReflectDecision::Run { floor_fired } => {
            if floor_fired {
                engine.stamp_reflect_floor_fire(onboarded, now).await; // best-effort; guards re-fires
            }
            match engine.reflect_once(onboarded, now).await {
                Ok(report) => ReflectSweepReport { floor_fired, report, ..Default::default() },
                Err(_) => ReflectSweepReport { reasoner_unavailable: true, floor_fired, ..Default::default() },
            }
        }
    }
}

/// Spawn the background reflect-sweep loop (mirrors `conflict::sweeper::spawn`). First tick fires
/// immediately; `MissedTickBehavior::Skip` prevents catch-up bursts. Reflection stays OFF until the owner
/// enables it (the gate lives inside `run_reflect_sweep_once`). A panic here cannot take down the daemon.
pub fn spawn(engine: Arc<EngineHandle>, data_dir: PathBuf) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(SWEEP_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let now = crate::capture::sweeper::system_time_to_epoch(Some(SystemTime::now()));
            let r = run_reflect_sweep_once(&engine, &data_dir, now).await;
            // Surface only real work (mirrors the conflict sweeper's quiet-on-noop discipline). No silent
            // caps: unhealable_thin AND transient_errors are in the line (the §2.3/§2.4 taxonomy — a write
            // hiccup is never hidden and never blamed on the model).
            if r.report.dossiers_refreshed > 0
                || r.report.candidate_repaired > 0
                || r.report.repaired_by_time > 0
                || r.report.no_material > 0
                || r.report.parked > 0
                || r.report.unhealable_thin > 0
                || r.report.reasoner_errors > 0
                || r.report.transient_errors > 0
            {
                eprintln!(
                    "reflect-sweep: refreshed {} / candidate {} / repaired-by-time {} / no-material {} / \
                     parked {} / unhealable-thin {} (attempted {}, floor {}, reasoner-err {}, transient-err {})",
                    r.report.dossiers_refreshed,
                    r.report.candidate_repaired,
                    r.report.repaired_by_time,
                    r.report.no_material,
                    r.report.parked,
                    r.report.unhealable_thin,
                    r.report.attempted,
                    r.floor_fired,
                    r.report.reasoner_errors,
                    r.report.transient_errors,
                );
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> ReflectDecisionInput {
        ReflectDecisionInput {
            onboarded: true, reflect_enabled: true, reasoner_ready: true, now: 1_000_000,
            newest_activity_at: Some(1_000_000 - REFLECT_QUIET_SECS), // exactly quiet
            evolve_enabled: false, evolve_queue_depth: 0, open_unparked_misses: 0,
            last_completed_run_at: 1_000_000, last_floor_fire_at: 1_000_000,
        }
    }

    #[test]
    fn gate_off_when_not_onboarded_or_disabled_or_reasoner_down() {
        for i in [
            ReflectDecisionInput { onboarded: false, ..base() },
            ReflectDecisionInput { reflect_enabled: false, ..base() },
        ] { assert_eq!(decide_reflect(&i), ReflectDecision::GatedOff); }
        assert_eq!(decide_reflect(&ReflectDecisionInput { reasoner_ready: false, ..base() }),
            ReflectDecision::ReasonerUnavailable);
    }

    #[test]
    fn runs_only_when_quiet() {
        assert_eq!(decide_reflect(&base()), ReflectDecision::Run { floor_fired: false });
        let noisy = ReflectDecisionInput { newest_activity_at: Some(1_000_000 - 1), ..base() };
        assert_eq!(decide_reflect(&noisy), ReflectDecision::NotQuiet);
        let never = ReflectDecisionInput { newest_activity_at: None, ..base() };
        assert_eq!(decide_reflect(&never), ReflectDecision::Run { floor_fired: false }, "no activity ever = quiet");
    }

    #[test]
    fn defers_to_evolve_backlog_unless_floor_fires() {
        let backlogged = ReflectDecisionInput { evolve_enabled: true, evolve_queue_depth: 3, ..base() };
        assert_eq!(decide_reflect(&backlogged), ReflectDecision::DeferredEvolveBacklog);
    }

    #[test]
    fn floor_overrides_both_quiet_and_evolve_backlog() {
        // Not quiet AND evolve-backlogged, but a long-stale unparked miss → the floor fires anyway (§2.1).
        let wedged = ReflectDecisionInput {
            newest_activity_at: Some(1_000_000 - 1), // NOT quiet
            evolve_enabled: true, evolve_queue_depth: 5, // backlogged
            open_unparked_misses: 2,
            last_completed_run_at: 1_000_000 - REFLECT_STALENESS_FLOOR_SECS - 1,
            last_floor_fire_at: 1_000_000 - REFLECT_STALENESS_FLOOR_SECS - 1,
            ..base()
        };
        assert_eq!(decide_reflect(&wedged), ReflectDecision::Run { floor_fired: true });
        // But the floor fires at most once per interval: a recent floor fire → no re-fire (falls through
        // to the ordinary gate, which here is NotQuiet).
        let recent = ReflectDecisionInput { last_floor_fire_at: 1_000_000 - 10, ..wedged };
        assert_eq!(decide_reflect(&recent), ReflectDecision::NotQuiet);
    }

    #[test]
    fn floor_needs_open_misses_and_evolve_defer_needs_a_nonempty_queue() {
        // Conjunct guard 1 (critic m3): BOTH floor timers stale but ZERO open unparked misses → the floor
        // must NOT fire (`open_unparked_misses > 0` is a required conjunct); the ordinary gate applies
        // (here: NotQuiet). Guards a regression that drops the misses conjunct and turns the floor into a
        // periodic wake-time timer.
        let stale_timers_no_misses = ReflectDecisionInput {
            newest_activity_at: Some(1_000_000 - 1), // not quiet
            evolve_enabled: true,
            evolve_queue_depth: 5,
            open_unparked_misses: 0,
            last_completed_run_at: 1_000_000 - REFLECT_STALENESS_FLOOR_SECS - 1,
            last_floor_fire_at: 1_000_000 - REFLECT_STALENESS_FLOOR_SECS - 1,
            ..base()
        };
        assert_eq!(decide_reflect(&stale_timers_no_misses), ReflectDecision::NotQuiet,
            "no open misses → no floor fire, ever");

        // Conjunct guard 2 (critic m3): evolve ENABLED but its queue EMPTY → NO defer; a quiet tick runs.
        // Guards a regression that defers on `evolve_enabled` alone (which would silence reflection on
        // every evolve-enabled brain regardless of backlog).
        let evolve_on_but_idle = ReflectDecisionInput { evolve_enabled: true, evolve_queue_depth: 0, ..base() };
        assert_eq!(decide_reflect(&evolve_on_but_idle), ReflectDecision::Run { floor_fired: false },
            "an idle evolve queue defers nothing");
    }
}
