//! Rung-4 R4-A (reflection sleep loop) shared types. Reflection re-composes topic dossiers on a
//! sleep-time cadence through the SAME citation-floored composer that evolve's summarize batch uses
//! (I9 single-source); this module holds the portable, ungated vocabulary both callers map to their
//! own reports. [`TopicRefreshOutcome`] is the result of refreshing ONE topic (spec §2.2 step 3);
//! T5 formalizes the reflection report + re-exports on top of it — T4 lands this enum first.

/// The result of refreshing ONE topic's dossier (spec §2.2 step 3). Reused by evolve's summarize batch
/// AND reflection's refresh, so each caller maps it to its own report. PORTABLE.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopicRefreshOutcome {
    /// A page was emitted; `superseded` iff a prior page for this topic was replaced (F5).
    Emitted {
        /// `true` iff this emit atomically replaced a prior page for the topic (F5).
        superseded: bool,
    },
    /// The gathered cited set matched the current page's → no emit (F6 idempotency).
    SkippedUnchanged,
    /// The fact-set is below `PAGE_MIN_FACTS`, or a gather/parse/assemble error occurred → no emit (F4).
    /// Distinct from `SkippedUnchanged` so `refresh_stale_pages` can count `unhealable_thin` (§2.3).
    SkippedThin,
    /// The compose reasoner call failed (transport/decoding). Distinct from `SkippedThin` so reflection can
    /// count `reasoner_errors` per-item (§2.4, the Rung-3 poison lesson); evolve's summarize treats it as a
    /// no-op `continue` exactly like the two Skipped variants (behavior-preserving).
    ReasonerError,
}
