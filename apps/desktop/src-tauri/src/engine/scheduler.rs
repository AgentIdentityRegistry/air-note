//! The background evolve driver (SP3 Task 10). The engine has no running loop — its
//! `evolve_once` is a single tick — so the desktop owns the cadence. OFF by default: every
//! gate (onboarded, the sticky `evolve_enabled` off-switch, a present local Ollama, a
//! non-empty queue) must pass before a tick runs, so a fresh install never evolves.
//! See docs/superpowers/specs/2026-06-23-sp3-recall-evolve-design.md §E.
//!
//! ALL gating logic lives in the pure `decide_tick` (unit-tested). The `spawn` loop is a thin
//! shell over it — exercised by manual launch (Task 12), not unit tests.

use crate::air::identity::IdentityStore;
use crate::engine::{ollama_probe, EngineHandle};
use std::sync::Arc;
use std::time::Duration;

/// How often the loop wakes to consider a tick (~5 min). A tick processes ≤16 events, so the
/// queue drains over several wakes; the interval throttles reasoner load. Idle/charging/
/// thermal throttling is M7 (named limitation).
pub const EVOLVE_INTERVAL: Duration = Duration::from_secs(300);

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

/// Spawn the background evolve loop. Uses `tauri::async_runtime::spawn` (NOT `tokio::spawn` —
/// `.setup()` runs outside a tokio reactor, so a bare `tokio::spawn` panics). The ticker uses
/// `MissedTickBehavior::Skip` so a slow (cold-7B, minutes-long) tick can't burst a backlog of
/// catch-up wakes when it finally returns. The loop owns an `Arc<EngineHandle>` + a cloned
/// `IdentityStore` (cheap `Clone`) for its per-wake onboarding read.
///
/// Each wake re-reads ALL gate inputs fresh (the user may toggle evolve, start/stop Ollama, or
/// drain the queue between wakes) and delegates the decision to `decide_tick`. A `Run` calls
/// `evolve_once`, which records its own telemetry; its result is intentionally discarded — a
/// `Busy` (a manual "Evolve now" overlapping this wake) is a harmless skip, and a tick error
/// is already captured in the engine's evolve telemetry surfaced by the status card.
pub fn spawn(engine: Arc<EngineHandle>, identity: IdentityStore) {
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(EVOLVE_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let onboarded = identity.is_onboarded();
            let evolve_enabled = engine.evolve_enabled_or_false(onboarded).await;
            let oll = ollama_probe::probe(crate::engine::reason::REASONER_MODEL_ID).await;
            let queue_depth = engine.queue_depth_or_zero(onboarded).await;
            if decide_tick(onboarded, evolve_enabled, oll.reachable && oll.model_present, queue_depth)
                == TickGate::Run
            {
                // Records telemetry inside; a `Busy` (manual tick overlap) is a harmless skip.
                let _ = engine.evolve_once(onboarded).await;
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
}
