// Copied from apps/desktop/src-tauri/src/engine/scheduler.rs (M1a Task 4); the in-app original is removed in Task 6.
// TWO adaptations (per the plan): (1) the `spawn` loop runs on plain `tokio::spawn` (the daemon runs a
// real tokio runtime, so the `tauri::async_runtime` caveat disappears); (2) the `use crate::air::identity::IdentityStore`
// import is dropped — onboarding is computed from the daemon's data_dir via `crate::identity::is_onboarded`,
// the SAME "<data_dir>/identity.json exists and parses" test the app uses. The PURE fns + their unit
// tests are copied AS-IS.

//! The background evolve driver. The engine has no running loop — its
//! `evolve_once` is a single tick — so the daemon owns the cadence. OFF by default: every
//! gate (onboarded, the sticky `evolve_enabled` off-switch, a present local Ollama, a
//! non-empty queue) must pass before a tick runs, so a fresh install never evolves.
//!
//! ALL gating logic lives in the pure `decide_tick` (unit-tested). The `spawn` loop is a thin
//! shell over it — exercised by manual launch, not unit tests.

use crate::engine::{ollama_probe, EngineHandle};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// How often the loop wakes to consider a tick (~5 min). A tick processes ≤16 events, so the
/// queue drains over several wakes; the interval throttles reasoner load.
pub const EVOLVE_INTERVAL: Duration = Duration::from_secs(300);

/// Hard cap on mandate proposals auto-applied per sweep (mirrors the engine's per-tick proposal
/// cap). Excess clean proposals spill to the next tick (~5-min-per-excess latency, accepted).
pub const MANDATE_AUTOAPPLY_PER_SWEEP: usize = 8;

/// PURE candidate selection for the auto-apply sweep (the unit-tested core). Given `(id, producer)`
/// pairs in oldest-first order, return up to `cap` ids whose producer is EXACTLY the M6c mandate
/// proposer — fail-closed: any other value, including empty/unknown, is excluded (the producer
/// filter is a contract/UX boundary; the taint/loud gate at apply is the security gate). Keeps
/// oldest-first so the sweep is fair.
pub fn sweep_candidates(pending: &[(String, String)], cap: usize) -> Vec<String> {
    pending
        .iter()
        .filter(|(_id, producer)| producer == bossclaw_core::graph::M6C_PROPOSER_PRODUCER)
        .take(cap)
        .map(|(id, _producer)| id.clone())
        .collect()
}

/// Whether one scheduler wake should run an evolve tick or skip it.
#[derive(Debug, PartialEq, Eq)]
pub enum TickGate {
    Run,
    Skip,
}

/// The PURE tick decision (the unit-tested core of the scheduler). A tick runs ONLY when all
/// four conditions hold; any one false → `Skip`. `ollama_ready` is the caller's AND of
/// `reachable && model_present`, so this gate stays independent of the probe shape.
pub fn decide_tick(
    onboarded: bool,
    evolve_enabled: bool,
    ollama_ready: bool,
    queue_depth: usize,
) -> TickGate {
    if onboarded && evolve_enabled && ollama_ready && queue_depth > 0 {
        TickGate::Run
    } else {
        TickGate::Skip
    }
}

/// Choose the readiness signal fed into `decide_tick`'s `ollama_ready` slot:
/// local mode trusts the Ollama probe; cloud mode trusts `reasoner_ready`
/// (itself fail-closed on signed consent). Cloud never silently falls back to
/// local (spec §3.4).
pub fn select_ready(cloud_mode: bool, ollama_probe_ready: bool, cloud_ready: bool) -> bool {
    if cloud_mode {
        cloud_ready
    } else {
        ollama_probe_ready
    }
}

/// Spawn the background evolve loop on the daemon's tokio runtime (plain `tokio::spawn` — the
/// daemon runs a real multi-thread runtime, unlike the app's `.setup()` which ran outside a
/// reactor). The ticker uses `MissedTickBehavior::Skip` so a slow (cold-7B, minutes-long) tick
/// can't burst a backlog of catch-up wakes when it finally returns. The loop owns an
/// `Arc<EngineHandle>` + the `data_dir` for its per-wake onboarding read (the daemon's
/// `identity::is_onboarded`, mirroring the app's per-wake `IdentityStore::is_onboarded`).
///
/// Each wake re-reads ALL gate inputs fresh (the user may toggle evolve, start/stop Ollama, or
/// drain the queue between wakes) and delegates the decision to `decide_tick`. A `Run` calls
/// `evolve_once`, which records its own telemetry; its result is intentionally discarded — a
/// `Busy` (a manual "Evolve now" overlapping this wake) is a harmless skip, and a tick error
/// is already captured in the engine's evolve telemetry surfaced by the status card.
pub fn spawn(engine: Arc<EngineHandle>, data_dir: PathBuf) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(EVOLVE_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let onboarded = crate::identity::is_onboarded(&data_dir);
            let evolve_enabled = engine.evolve_enabled_or_false(onboarded).await;
            let config = engine.reasoner_config_or_default(onboarded).await;
            let cloud_mode = matches!(config.mode, crate::engine::reason::ReasonerMode::Cloud);
            // Probe ONLY the active source; the inactive one is `false`. In cloud mode the
            // Ollama probe is SKIPPED entirely and there is NO fallback to local (spec §3.4):
            // readiness is `reasoner_ready_or_false`, itself fail-closed on signed consent.
            let ollama_ready = if cloud_mode {
                false
            } else {
                let oll = ollama_probe::probe(crate::engine::reason::REASONER_MODEL_ID).await;
                oll.reachable && oll.model_present
            };
            let cloud_ready = if cloud_mode {
                engine.reasoner_ready_or_false(onboarded).await
            } else {
                false
            };
            let ready = select_ready(cloud_mode, ollama_ready, cloud_ready);
            let queue_depth = engine.queue_depth_or_zero(onboarded).await;
            if decide_tick(onboarded, evolve_enabled, ready, queue_depth) == TickGate::Run {
                // Records telemetry inside; a `Busy` (manual tick overlap) is a harmless skip.
                let _ = engine.evolve_once(onboarded).await;
                // SP5: right after the tick, auto-apply the CLEAN mandate proposals it produced and
                // leave risky ones queued. INTENTIONALLY coupled to the Run tick — on Ollama-down no
                // tick runs, so an already-queued clean proposal waits until Ollama returns (accepted
                // coupling). No-ops when mandates are off. Per-item errors are swallowed+logged inside
                // the sweep; here we surface a one-line summary so the autonomous loop is observable.
                match engine.mandate_autoapply_sweep(onboarded).await {
                    Ok(0) => {} // nothing applied this tick — stay quiet.
                    Ok(n) => eprintln!("mandate sweep: auto-applied {n} clean mandate write(s)"),
                    Err(e) => eprintln!("mandate sweep: aborted before applying: {e}"),
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_decision_gates_correctly() {
        use TickGate::*;
        assert_eq!(decide_tick(false, true, true, 5), Skip); // not onboarded
        assert_eq!(decide_tick(true, false, true, 5), Skip); // evolve disabled
        assert_eq!(decide_tick(true, true, false, 5), Skip); // ollama unavailable
        assert_eq!(decide_tick(true, true, true, 0), Skip); // empty queue
        assert_eq!(decide_tick(true, true, true, 5), Run); // all conditions met
    }

    #[test]
    fn readiness_source_follows_mode() {
        // Local: use the ollama probe result; ignore cloud readiness.
        assert!(select_ready(false, true, false)); // local probe true
        assert!(!select_ready(false, false, true)); // local probe false (cloud true ignored)
        // Cloud: use cloud readiness; ignore the probe.
        assert!(select_ready(true, false, true)); // cloud ready true
        assert!(!select_ready(true, true, false)); // cloud not ready (probe true ignored)
    }

    #[test]
    fn sweep_candidates_filters_to_m6c_oldest_first_and_caps() {
        // (id, producer) pairs in oldest-first order; only m6c survive, capped at the limit.
        let pending = vec![
            ("p1".to_string(), "m6b-reconciler".to_string()),
            ("p2".to_string(), "m6c-mandate-proposer".to_string()),
            ("p3".to_string(), "m6c-mandate-proposer".to_string()),
            ("p4".to_string(), "".to_string()), // empty/unknown → never
            ("p5".to_string(), "m6c-mandate-proposer".to_string()),
        ];
        let picked = sweep_candidates(&pending, 2);
        assert_eq!(picked, vec!["p2".to_string(), "p3".to_string()],
            "only m6c, oldest-first, capped at 2 (p5 spills to the next tick)");
        // An empty/unknown producer is NEVER auto-appliable.
        let none = sweep_candidates(&[("x".to_string(), "".to_string())], 8);
        assert!(none.is_empty(), "empty producer is never swept");
    }
}
