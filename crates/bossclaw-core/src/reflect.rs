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
    /// STRUCTURALLY too thin to page — exactly two producers: the gathered fact-set is below
    /// `PAGE_MIN_FACTS`, or the citation floor left nothing to assemble (`assemble` → `None`, F4).
    /// ONLY structural thinness lands here because T8's `refresh_stale_pages` counts this variant as
    /// `unhealable_thin` (§2.3) — a transient failure reported as "unhealable" would corrupt that
    /// honesty metric (see [`TopicRefreshOutcome::TransientError`]).
    SkippedThin,
    /// MODEL-OUTPUT failure — exactly two producers: the compose `complete_json` call failed
    /// (transport/decoding), or `parse_draft` rejected the returned draft (malformed model JSON — a
    /// reasoner-QUALITY failure, retry-fixable). Distinct from `SkippedThin` so reflection can count
    /// `reasoner_errors` per-item (§2.4, the Rung-3 poison lesson); evolve's summarize treats it as a
    /// no-op `continue` exactly like the Skipped variants (behavior-preserving).
    ReasonerError,
    /// ENGINE I/O failure, retry-fixable — exactly two producers: `gather_fact_set` returned an error
    /// (the read side), or `emit_page` failed (the write side). Distinct from `SkippedThin` so a write
    /// hiccup is never reported "unhealable"; distinct from `ReasonerError` so model quality and engine
    /// I/O don't blur. Future consumers (T7/T8) count it as a transient/retry bucket; evolve's summarize
    /// maps it to a no-op `continue` (behavior-preserving — the old inline body swallowed the same
    /// errors).
    TransientError,
}
