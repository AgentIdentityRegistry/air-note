//! Rung-4 R4-A reflection: the ONE documented tuning block + pure report/outcome data types.
//! Reflection re-composes topic dossiers on a sleep-time cadence through the SAME citation-floored
//! composer that evolve's summarize batch uses (I9 single-source); this module holds the portable,
//! ungated vocabulary both callers map to their own reports.
//!
//! The tuning consts are PROVISIONAL / harness-tunable (spec §7; the `CONFLICT_PAIR_ERROR_BUDGET`
//! precedent). Everything here is PORTABLE — reflection is built on portable seams
//! (gather/summarize/entity_search/recall), never the `#[cfg(unix)]` conflict-proposal subsystem, so
//! nothing here is gated.

/// Idle window: reflection runs only when no memory-class append (memory + session-capture) landed
/// within this many seconds (spec §2.1). Provisional 600 — the capture sweeper's `QUIET_SECS`
/// precedent. Read fresh each tick against the newest relevant event's ts (no latch).
pub const REFLECT_QUIET_SECS: i64 = 600;

/// Starvation floor: if unrepaired/unparked misses exist AND this many seconds passed since the last
/// COMPLETED reflect run, run ONE budgeted tick even when not quiet and even when evolve is backlogged
/// (spec §2.1 precedence). Provisional 6h. Also the floor re-fire cadence (fires ≤ once per this window).
pub const REFLECT_STALENESS_FLOOR_SECS: i64 = 21_600;

/// Per-miss attempt budget → `parked` at/above this (spec §2.2; the Rung-3 poison-budget lesson).
pub const REFLECT_MISS_ATTEMPT_BUDGET: u32 = 3;

/// Open misses attempted per tick (spec §2.1 budget). Small + fixed; misses first, refresh with the rest.
pub const REFLECT_MISSES_PER_TICK: usize = 4;

/// Stale dossiers refreshed per tick (spec §2.1 budget).
pub const REFLECT_REFRESH_PER_TICK: usize = 4;

/// Top-N known topics a missed query may resolve to (spec §2.2 step 2). Conservative-first (spec §7.2).
pub const REFLECT_TOPIC_N: usize = 2;

/// `k` for the repair-check recalls in `attempt_miss` (spec §2.2 steps 1 & 4). The SP3 miss ring stores
/// only the query, not its original `k`, so reflection re-checks "does this query find anything now?" at a
/// stable modest `k`; any `k >= 1` answers hit-vs-miss, and 5 keeps the check cheap.
pub const REFLECT_RECALL_K: usize = 5;

/// Minimum cosine SIMILARITY (= `1.0 - entity_search` cosine distance, which lies in `[0,2]`; the
/// conversion mirrors `resolve_mention`, log.rs:6987) for a missed query to resolve to a KNOWN topic
/// (spec §2.2 step 2 / §7.2; plan-review OQ1 LOCK). Anchored to `extract::RESOLVE_HIGH` (0.92,
/// extract.rs:19/:146) — evolve's OWN autonomous-action bar: in the `[RESOLVE_LOW, RESOLVE_HIGH)` band
/// evolve refuses to self-act and asks a model to adjudicate, so an AUTONOMOUS reflection refresh must
/// not be bolder than the awake loop's auto-merge. Conservative-first per §7.2 (a too-high floor merely
/// yields more honest `no_material`); §5.3(d) tunes DOWNWARD toward `RESOLVE_LOW` (0.75) only if the
/// held-out probe shows precision allows it. Compared as `1.0 - dist >= REFLECT_TOPIC_FLOOR`.
pub const REFLECT_TOPIC_FLOOR: f32 = 0.92;

/// Age-out horizon for TERMINAL-repaired backlog rows (spec §7.3): `repaired_by_time` /
/// `candidate_repaired` rows older than this are pruned (they carry no forward information);
/// `parked` / `no_material` PERSIST (they do — "we tried and could not" / "we never knew this").
pub const REFLECT_BACKLOG_TERMINAL_TTL_SECS: i64 = 30 * 86_400;

use sha2::{Digest, Sha256};

/// Normalized backlog key (spec §2.2 / §7.2): SHA-256 hex of the TRIMMED, casefolded query. v1 casefold =
/// `to_lowercase()` (semantic dedup stays OUT, §6). Fixed-length hex keeps the PK bounded; the raw
/// `query_text` is stored in its own column for the digest/UI. Bloat is bounded because pages are
/// ENTITY-keyed — near-duplicate queries about one topic converge on the same dossier. PURE.
pub fn normalized_query_key(query: &str) -> String {
    let mut h = Sha256::new();
    h.update(query.trim().to_lowercase().as_bytes());
    hex::encode(h.finalize())
}

/// Backlog state for one missed query (spec §2.2). `Open` is re-attempted; `RepairedByTime`/
/// `CandidateRepaired` are terminal-success (age out later, §7.3); `NoMaterial`/`Parked` are terminal and
/// PERSIST (they carry information — "we never knew this" / "we tried and could not"). PORTABLE.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissState {
    /// Live in the backlog — re-attempted next tick until a terminal state is reached.
    Open,
    /// Terminal success: recall began hitting on its own before any refresh (time healed it). Ages out.
    RepairedByTime,
    /// Terminal success: a refresh made the replay recall hit. Ages out (§7.3).
    CandidateRepaired,
    /// Terminal, PERSISTS: the query resolved to no known topic above the floor ("we never knew this").
    NoMaterial,
    /// Terminal, PERSISTS: the attempt budget was reached without repair ("we tried and could not").
    Parked,
}

impl MissState {
    /// The stable lowercase token persisted in the `reflect_miss_backlog.state` column.
    pub fn as_str(self) -> &'static str {
        match self {
            MissState::Open => "open",
            MissState::RepairedByTime => "repaired_by_time",
            MissState::CandidateRepaired => "candidate_repaired",
            MissState::NoMaterial => "no_material",
            MissState::Parked => "parked",
        }
    }
}

/// One OPEN row of the miss backlog as served to the tick loop (spec §2.2). Named fields on purpose:
/// `key` and `query_text` are both `String`s, so a positional tuple would be a silently-swappable
/// footgun for the T7 consumer. PORTABLE.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenMiss {
    /// The normalized backlog PK — [`normalized_query_key`] of the query.
    pub key: String,
    /// The raw query text as first seen (kept verbatim for the digest/UI and the replay recall).
    pub query_text: String,
    /// Attempts consumed so far (the caller parks at [`REFLECT_MISS_ATTEMPT_BUDGET`]).
    pub attempts: u32,
}

/// The result of ONE `attempt_miss` (spec §2.2): the classification PLUS the number of REAL dossier
/// revisions the attempt emitted (critic M2/OQ4: the digest's "refreshed" unit counts only true
/// `TopicRefreshOutcome::Emitted`s — `candidate_repaired` alone never feeds it). PORTABLE.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MissAttempt {
    /// The classification this attempt reached.
    pub outcome: MissOutcome,
    /// How many topics' dossiers this attempt ACTUALLY re-emitted (step 3 `Emitted` count).
    pub dossiers_emitted: usize,
}

/// The classification of ONE `attempt_miss` (spec §2.2). Carried inside [`MissAttempt`]. PORTABLE.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissOutcome {
    /// Recall now hits without any refresh (step 1).
    RepairedByTime,
    /// The query resolves to no known topic above the floor (step 2) — an honest "we never knew this".
    NoMaterial,
    /// A refresh made the replay recall hit (step 4). OPERATIONAL, not evidence (§5.1).
    CandidateRepaired,
    /// A refresh did not help; the attempt was bumped but the budget is not yet reached.
    StillMissing,
    /// The attempt reached `REFLECT_MISS_ATTEMPT_BUDGET` → parked (bounded loss).
    Parked,
    /// A per-item compose REASONER error (model transport/quality) occurred during the refresh —
    /// isolated + counted (§2.4). Distinct from [`MissOutcome::TransientError`] (engine I/O).
    ReasonerError,
    /// A per-item TRANSIENT engine-I/O error (gather/emit) occurred during the refresh — retry-fixable,
    /// isolated + counted APART from [`MissOutcome::ReasonerError`] so a write hiccup is never blamed on
    /// the model (§2.4; mirrors [`TopicRefreshOutcome::TransientError`]).
    TransientError,
}

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

/// The report of one `refresh_stale_pages` pass (spec §2.3). PORTABLE.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct StaleRefreshReport {
    /// Stale pages whose refresh emitted a healed revision.
    pub healed: usize,
    /// Stale pages whose refresh changed nothing (F6) this pass.
    pub unchanged: usize,
    /// Stale pages that cannot legally re-emit — their lineage is ENTIRELY retired/superseded so the
    /// fact-set fell below `PAGE_MIN_FACTS` (§2.3 thin-set residual). Counted, not retried forever.
    pub unhealable_thin: usize,
    /// Per-item compose REASONER errors this pass (model transport/quality; isolated + counted).
    pub reasoner_errors: usize,
    /// Per-item TRANSIENT engine-I/O errors (gather/emit) this pass — retry-fixable, counted APART from
    /// `reasoner_errors` so a write hiccup is never reported "unhealable" and never blamed on the model
    /// (§2.3 taxonomy; feeds the T8 transient cap).
    pub transient_errors: usize,
}

/// The tick report `reflect_once` returns (spec §2.4). Mirrors `EvolveReport`'s derive/style.
/// `merge_proposed` is always 0 in R4-A (R4-B fills it). PORTABLE.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReflectReport {
    /// Open misses attempted this tick.
    pub attempted: usize,
    /// Misses whose refresh made the replay recall hit (operational, not evidence — §5.1).
    pub candidate_repaired: usize,
    /// Misses that recall began hitting on their own before any refresh (step 1).
    pub repaired_by_time: usize,
    /// Misses that resolved to no known topic above the floor — an honest "we never knew this" (step 2).
    pub no_material: usize,
    /// Misses that reached the attempt budget this tick and were parked (bounded loss).
    pub parked: usize,
    /// REAL dossier revisions emitted this tick = miss-pipeline `Emitted`s + stale-refresh heals — the
    /// digest's honest "refreshed" unit (critic M2/OQ4). NEVER counts `candidate_repaired` by itself.
    pub dossiers_refreshed: usize,
    /// Stale pages that cannot legally re-emit (fact-set below `PAGE_MIN_FACTS`; §2.3 thin-set residual).
    pub unhealable_thin: usize,
    /// Merges proposed this tick — always 0 in R4-A (R4-B fills it).
    pub merge_proposed: usize,
    /// `true` iff the tick was a no-op because reflection is disabled (flag off).
    pub skipped_disabled: bool,
    /// Per-item compose REASONER errors this tick (model transport/quality; isolated + counted, §2.4).
    /// GRANULARITY (this field and `transient_errors`): error EVENTS at mixed grain — one per erroring
    /// MISS from the pipeline (N erroring topics within one miss collapse to one) plus one per erroring
    /// stale PAGE from the tidy pass. Honest activity signals, not per-topic counts.
    pub reasoner_errors: usize,
    /// Per-item TRANSIENT engine-I/O errors this tick (gather/emit; retry-fixable, counted APART from
    /// `reasoner_errors` so a write hiccup is never blamed on the model — §2.4; feeds the T8 cap).
    /// Granularity: mixed, exactly as `reasoner_errors` documents.
    pub transient_errors: usize,
}

/// The reflect progress cursor (re-derivable single row). `last_served_*` back the digest delta (T13);
/// `last_completed_run_at`/`last_floor_fire_at` are daemon-supplied epoch markers (clock-free core, the
/// `capture_enabled_at` precedent) backing the §2.1 starvation floor. PORTABLE.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ReflectCursor {
    /// `refreshed_total` at the last digest serve — backs the digest "refreshed" delta (T13).
    pub last_served_refreshed: i64,
    /// `no_material_total` at the last digest serve — backs the digest "no_material" delta (T13).
    pub last_served_no_material: i64,
    /// Epoch secs of the last COMPLETED reflect run (daemon-supplied) — the §2.1 starvation-floor input.
    pub last_completed_run_at: i64,
    /// Epoch secs of the last floor fire (daemon-supplied) — the §2.1 floor re-fire guard.
    pub last_floor_fire_at: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_query_key_is_trim_casefold_stable() {
        // Trimmed + casefolded → the same key; distinct queries → distinct keys; fixed-length hex.
        assert_eq!(
            normalized_query_key("  Where is Kenny?  "),
            normalized_query_key("where is kenny?")
        );
        assert_ne!(
            normalized_query_key("where is kenny?"),
            normalized_query_key("where is acme?")
        );
        assert_eq!(normalized_query_key("x").len(), 64, "sha256 hex is fixed-length (bounded PK)");
    }
}
