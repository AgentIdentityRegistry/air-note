//! The evolve-loop runtime (spec §8): the always-on curator that turns new
//! memories into signed `entity`/`link`/`invalidate` events. It is NOT a
//! privileged writer — every emit goes through [`crate::log::EventLog::append`]
//! (the single serialized writer). The loop holds an [`crate::embed::Embedder`] +
//! a [`crate::reason::Reasoner`] + a read handle for recall/graph; it never opens
//! a second writer.
//!
//! `evolve_once` (one tick) lives as a method on [`crate::log::EventLog`] (it is
//! SQL/IO-heavy); this module holds the report/status/scheduler types and the
//! PURE debounce decision so they are unit-testable without a DB.
//!
//! # What M4a ships vs. M7 (DoD honesty)
//!
//! M4a ships: the per-tick batch cap ([`crate::extract::EVOLVE_BATCH`]), the pure
//! [`debounce_due`] decision, the sticky fail-closed off-switch
//! ([`crate::log::EventLog::evolve_enabled`] /
//! [`crate::log::EventLog::set_evolve_enabled`]), the in-crate resource
//! fail-safes (Rev 2 F6), and a live [`EvolveStatus`] `queue_depth` + `enabled`.
//! The RUNNING scheduler/thread and the idle/charging/thermal throttle are M7;
//! that is why [`EvolveStatus::last_tick_ms`] / [`EvolveStatus::error_count`] /
//! [`EvolveStatus::last_error`] are honest stubs (`None`/`0`) here — they are
//! wired by M7's long-lived loop driver, not persisted in M4a.

/// Max memories processed per evolve tick (re-exported from the pure-consts
/// module so the "evolve consts" grouping reads naturally without a second
/// source of truth — single-sourced in [`crate::extract`], no cycle).
pub use crate::extract::{EVOLVE_BATCH, EVOLVE_DEBOUNCE_MS};

/// What one [`crate::log::EventLog::evolve_once`] tick did (spec §8 observability
/// + the test oracle). Counts are per-tick, not cumulative.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EvolveReport {
    /// New `entity` events minted this tick (resolution chose Mint).
    pub entities_minted: usize,
    /// Machine `link` events emitted this tick (new active edge-keys only).
    pub links_emitted: usize,
    /// `invalidate` events emitted this tick (confirmed contradictions).
    pub invalidates_emitted: usize,
    /// Memories processed this tick (≤ [`EVOLVE_BATCH`]).
    pub memories_processed: usize,
    /// True iff the tick short-circuited because the off-switch is engaged.
    pub skipped_disabled: bool,
}

/// A snapshot of loop health for the desktop (spec §8 observability surface).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvolveStatus {
    /// Events behind the cursor (unprocessed `memory` events) right now. LIVE in
    /// M4a (computed from the log + the cursor).
    pub queue_depth: usize,
    /// Wall-clock duration of the most recent tick in ms, or `None` if no tick
    /// has run since open. Honest stub in M4a (`None`); wired by M7's loop driver.
    pub last_tick_ms: Option<u128>,
    /// Total reasoner/tick errors observed since open (each made the tick a
    /// retryable no-op — spec §10). Honest stub in M4a (`0`); wired by M7.
    pub error_count: usize,
    /// The most recent error message, if any. Honest stub in M4a (`None`); M7.
    pub last_error: Option<String>,
    /// Whether the loop is currently enabled. LIVE in M4a — the sticky
    /// fail-closed [`crate::log::EventLog::evolve_enabled`] verdict.
    pub enabled: bool,
}

/// Decide whether a debounced tick is due (spec §8 scheduler): a tick fires once
/// at least `debounce_ms` have elapsed since the last append that scheduled it.
/// PURE — the caller supplies the clock readings, so this is unit-testable.
/// `now_ms` and `last_append_ms` are monotonic millisecond readings;
/// `saturating_sub` keeps a clock that went backwards from firing spuriously.
pub fn debounce_due(now_ms: u128, last_append_ms: u128, debounce_ms: u128) -> bool {
    now_ms.saturating_sub(last_append_ms) >= debounce_ms
}

#[cfg(test)]
mod tests {
    use super::debounce_due;
    use crate::extract::{truncate_for_reasoner, MAX_INPUT_TEXT_BYTES};

    #[test]
    fn debounce_fires_only_after_the_window() {
        assert!(!debounce_due(1500, 1000, 2000), "0.5s < 2s window → not due");
        assert!(debounce_due(3000, 1000, 2000), "2s elapsed → due");
        assert!(debounce_due(3001, 1000, 2000), "past the window → due");
    }

    #[test]
    fn debounce_does_not_fire_on_a_backwards_clock() {
        // A monotonic clock should not go backwards, but if a reading does, the
        // saturating subtraction yields 0 → never spuriously fires.
        assert!(!debounce_due(900, 1000, 2000), "now < last → 0 elapsed → not due");
    }

    #[test]
    fn truncate_leaves_short_text_untouched() {
        let s = "Kenny works at Acme.";
        assert_eq!(truncate_for_reasoner(s), s);
    }

    #[test]
    fn truncate_caps_oversized_text_on_a_char_boundary() {
        // A string of multi-byte chars longer than the cap: the prefix must be
        // ≤ the cap AND a valid &str (never a split multi-byte sequence).
        let big = "é".repeat(MAX_INPUT_TEXT_BYTES); // 2 bytes each → 2× over the cap
        let out = truncate_for_reasoner(&big);
        assert!(out.len() <= MAX_INPUT_TEXT_BYTES, "truncated to the byte cap");
        assert!(big.starts_with(out), "prefix of the original");
        // Re-borrowing as &str proves it is on a char boundary (no panic).
        assert!(out.chars().all(|c| c == 'é'));
    }
}
