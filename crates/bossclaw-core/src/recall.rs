//! Hybrid recall: pure fusion math, provenance types, and the reranker seam.
//!
//! This module is deliberately PURE — no SQL, no I/O, no `Store`. It mirrors the
//! split used by [`crate::keyword`]: the database work (running the two retrieval
//! arms, fetching candidate timestamps, applying boosts) lives on
//! [`crate::log::EventLog::recall`]; everything here is data types and the
//! [`rrf_fuse`] reciprocal-rank-fusion helper, which is unit-testable on its own.
//!
//! The pipeline shape (spec §5.7) is: embed query → hybrid (vector + keyword) →
//! optional rerank → boosts (recency-decay, pinned) → top-N with evidence
//! labels. Graph-proximity boost is M3 and is intentionally absent here.

use std::collections::HashMap;

/// Which retrieval arm(s) surfaced a hit (provenance / evidence).
///
/// A hit can carry both variants when the same `event_id` was returned by the
/// vector arm AND the keyword arm — that overlap is exactly the "hybrid" signal
/// the recall pipeline exists to capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecallSource {
    /// The semantic (ANN vector) arm returned this id.
    Vector,
    /// The lexical (FTS5 / BM25 keyword) arm returned this id.
    Keyword,
}

/// A recall result with its fused score and provenance.
///
/// `score` is the post-fusion, post-boost score (higher = more relevant); it is
/// NOT comparable across different `recall` calls (RRF magnitudes depend on the
/// candidate set). `sources` lists every arm that surfaced this id, in a stable
/// order (vector before keyword) so callers can render evidence deterministically.
#[derive(Debug, Clone)]
pub struct Hit {
    /// The event id this hit refers to.
    pub event_id: String,
    /// Fused + boosted relevance score; higher is better.
    pub score: f32,
    /// Every arm that surfaced this id (the hit's provenance / evidence).
    pub sources: Vec<RecallSource>,
}

/// Optional re-ranking stage. v1 default is a no-op (spec §5.7).
///
/// A real implementation (e.g. a `bge-reranker` cross-encoder) lands in a later
/// milestone behind this trait, so v1 can ship hybrid-without-rerank and add the
/// model later without changing [`crate::log::EventLog::recall`]'s signature.
pub trait Reranker: Send + Sync {
    /// Re-order `hits` for `query`, returning the new ordering. The no-op default
    /// [`NoopReranker`] returns `hits` unchanged.
    fn rerank(&self, query: &str, hits: Vec<Hit>) -> Vec<Hit>;
}

/// The v1 default reranker — identity (returns hits unchanged).
///
/// Wiring `recall` through this exercises the [`Reranker`] seam end-to-end, so
/// dropping in a real reranker later is a pure substitution.
pub struct NoopReranker;

impl Reranker for NoopReranker {
    fn rerank(&self, _query: &str, hits: Vec<Hit>) -> Vec<Hit> {
        hits
    }
}

/// Caller knobs for recall.
///
/// `Default` yields empty pin and seed lists (no pinned ids, auto-seeded graph
/// proximity), which is the common case.
#[derive(Default)]
pub struct RecallOptions {
    /// Event ids to boost by [`PIN_MULTIPLIER`] regardless of their organic rank
    /// (e.g. a memory the user explicitly pinned). Ids not present in the fused
    /// candidate set are simply ignored.
    pub pinned: Vec<String>,
    /// Explicit graph-proximity seed node ids. When non-empty, recall boosts
    /// candidates within [`GRAPH_MAX_HOPS`] of these (current edges only). When
    /// empty, recall auto-seeds from the top [`GRAPH_AUTO_SEED_TOPK`] fused hits.
    pub graph_seeds: Vec<String>,
}

/// Reciprocal-rank-fusion constant `k` (Cormack et al., "Reciprocal Rank Fusion
/// outperforms Condorcet and individual Rank Learning Methods", SIGIR 2009). The
/// paper's recommended default is 60; it damps the influence of very high ranks
/// so no single arm's #1 can dominate the fused score.
pub const RRF_K: f32 = 60.0;

/// How much a maximally-recent event can boost its fused score, as a fraction of
/// that score. The recency multiplier is `1.0 + RECENCY_WEIGHT * decay` where
/// `decay ∈ (0, 1]`, so a brand-new event is boosted by at most +50% and an
/// ancient one by ~0%. Kept below 1.0 on purpose: recency is a *tilt* on the
/// RRF ordering that can reorder candidates with equal or near-equal fused base
/// scores. It does not guarantee preservation of rank gaps between clearly
/// distinct candidates (e.g. a single-arm hit may be re-ranked by a very recent
/// two-arm hit if their RRF gap is smaller than the recency boost). For
/// candidates with identical text (and therefore identical RRF base), the final
/// ordering is determined by the explicit `ts`-DESC comparator in `recall`,
/// not by the float recency multiplier, which may underflow to `0.0` in f32 for
/// sub-millisecond age differences.
pub const RECENCY_WEIGHT: f32 = 0.5;

/// Recency-decay half-life, in seconds. At this age the recency `decay` term is
/// `0.5`, i.e. an event contributes half the recency boost of a brand-new one.
/// Default: 7 days (`7 * 24 * 60 * 60`), reflecting that a memory's "freshness"
/// relevance roughly halves over a week — recent enough to matter for follow-ups,
/// slow enough that week-old context is not abruptly discarded.
pub const HALF_LIFE_SECS: f64 = 7.0 * 24.0 * 60.0 * 60.0;

/// Multiplicative boost applied to the fused score of a pinned id (see
/// [`RecallOptions::pinned`]). Must be `> 1.0` to raise a pinned id above an
/// otherwise-equal non-pinned one; `2.0` doubles the score, which clears the
/// ≤1.5× recency multiplier so a pin reliably wins ties even against the newest
/// non-pinned candidate.
pub const PIN_MULTIPLIER: f32 = 2.0;

/// Max multiplicative graph-proximity boost at 1 hop, as a fraction of the fused
/// score: a direct neighbour of a seed is boosted by `1 + GRAPH_WEIGHT`. Kept
/// just below [`RECENCY_WEIGHT`] (0.5) and far below [`PIN_MULTIPLIER`] (2.0) so
/// graph-relatedness is a *tilt* on ranking, not an override.
pub const GRAPH_WEIGHT: f32 = 0.4;

/// Per-hop decay of the graph boost: each extra hop multiplies the boost factor
/// by this. With [`GRAPH_MAX_HOPS`] = 1 only direct neighbours are boosted, but
/// the term is applied as `GRAPH_HOP_DECAY^(hops-1)` so raising the hop cap later
/// needs no formula change.
pub const GRAPH_HOP_DECAY: f32 = 0.5;

/// How many hops out from a seed the proximity boost reaches. v1 = 1 (direct
/// neighbours only); the BFS + decay term already support a larger cap.
pub const GRAPH_MAX_HOPS: u32 = 1;

/// When no explicit `graph_seeds` are supplied, auto-seed proximity from the top
/// N fused candidates (their own node ids). 1 = "boost what's linked to the
/// single strongest hit", which fires with zero caller input.
pub const GRAPH_AUTO_SEED_TOPK: usize = 1;

/// Intra-result reinforcement seed count (spec §7): auto-seed proximity from the
/// top N fused hits, not just the single top-1 (the M3 [`GRAPH_AUTO_SEED_TOPK`]).
/// A memory linked to several of the result set's strong hits gets the tilt. 3 is
/// conservative — enough to catch a cluster, small enough that the boost stays a
/// tilt (a deep hit is unlikely to seed). Tunable in dogfooding.
pub const GRAPH_REINFORCE_TOPK: usize = 3;

/// How many candidates each arm fetches before fusion. Over-fetching well beyond
/// the caller's final `k` lets RRF see enough of each arm's tail to reorder
/// correctly (an id ranked, say, #20 by keyword but #1 by vector should still be
/// fusible). 50 comfortably exceeds any realistic interactive `k` while staying
/// cheap for the small-to-medium corpora BossClaw holds.
pub const FUSION_FETCH: usize = 50;

/// Fuse several ranked id-lists into a per-id reciprocal-rank-fusion score.
///
/// Each inner `Vec<String>` is one arm's results, ordered best-first. For every
/// arm, the id at 0-based position `i` contributes `1.0 / (RRF_K + rank)` where
/// `rank = i + 1` (1-based, per the RRF paper). An id present in multiple arms
/// accumulates the sum of its per-arm contributions, so overlap across arms
/// raises the fused score — the core hybrid-relevance signal.
///
/// Pure and deterministic: the same input always yields the same map. Ordering
/// of the returned [`HashMap`] is unspecified (callers sort by value).
///
/// # Examples
/// ```
/// use bossclaw_core::recall::{rrf_fuse, RRF_K};
///
/// // "a" is #1 in both arms; "b" is #1 in only one arm.
/// let arms = vec![
///     vec!["a".to_string(), "b".to_string()],
///     vec!["a".to_string(), "c".to_string()],
/// ];
/// let scores = rrf_fuse(&arms);
/// assert!(scores["a"] > scores["b"]);
/// ```
pub fn rrf_fuse(ranked_arms: &[Vec<String>]) -> HashMap<String, f32> {
    let mut scores: HashMap<String, f32> = HashMap::new();
    for arm in ranked_arms {
        for (i, id) in arm.iter().enumerate() {
            // 1-based rank per the RRF definition (Σ 1/(k + rank)).
            *scores.entry(id.clone()).or_insert(0.0) += rrf_contribution(i + 1);
        }
    }
    scores
}

/// One arm's reciprocal-rank-fusion contribution for a given 1-based `rank`:
/// `1 / (RRF_K + rank)`. The single source of the RRF formula — both
/// [`rrf_fuse`] and the tie-aware [`fuse_scored_arms`] call it, so the two paths
/// can never compute RRF differently.
fn rrf_contribution(rank: usize) -> f32 {
    1.0 / (RRF_K + rank as f32)
}

/// Tie-aware reciprocal-rank fusion over arms of `(id, arm_score)` pairs.
///
/// This is the production fusion used by [`crate::log::EventLog::recall`]. Unlike
/// the pure position-based [`rrf_fuse`] (which takes already-ranked id-lists and
/// is the unit-tested reference for the RRF formula), this variant is handed each
/// arm's raw `(id, score)` output and assigns **competition ranks**: candidates
/// with the *same* arm score share the *same* rank, and the next distinct score
/// resumes at `group_start + group_len` (standard "1224" ranking).
///
/// Why tie-awareness matters: two memories with identical text produce an
/// identical embedding (identical cosine distance) AND an identical BM25 score,
/// so they genuinely tie in both arms. Position-based ranking would hand them
/// adjacent ranks (1 and 2), creating an RRF gap (~`1/RRF_K`) far larger than the
/// deliberately-weak recency tilt could ever overcome — the older event would
/// win purely by arbitrary arm ordering. Collapsing tied scores to a shared rank
/// makes their fused base score *exactly equal*, so a downstream recency/pin
/// boost is the deterministic tie-break (spec §5.7), independent of the arbitrary
/// order in which the underlying ANN/BM25 engine emitted the tied ids.
///
/// `lower_is_better` selects the score ordering: `true` for distance/BM25 arms
/// (both of which rank lower scores first), `false` for similarity scores.
pub(crate) fn fuse_scored_arms(
    arms: &[(&[(String, f32)], bool)],
) -> HashMap<String, f32> {
    let mut scores: HashMap<String, f32> = HashMap::new();
    for (arm, lower_is_better) in arms {
        for (id, rank) in rank_with_ties(arm, *lower_is_better) {
            *scores.entry(id).or_insert(0.0) += rrf_contribution(rank);
        }
    }
    scores
}

/// Assign 1-based **competition ranks** to `scored`, where equal scores share a
/// rank and the next distinct score resumes at `group_start + group_size`.
///
/// `lower_is_better = true` orders ascending (smallest score → rank 1), matching
/// the distance/BM25 convention used by both recall arms. The input is not
/// mutated; a sorted copy is ranked and returned as `(id, rank)`. Pure and
/// deterministic for a given multiset of scores (the relative order of two ids
/// with *equal* score does not affect either id's rank, which is the whole point).
fn rank_with_ties(scored: &[(String, f32)], lower_is_better: bool) -> Vec<(String, usize)> {
    let mut sorted: Vec<&(String, f32)> = scored.iter().collect();
    sorted.sort_by(|a, b| {
        if lower_is_better {
            a.1.total_cmp(&b.1)
        } else {
            b.1.total_cmp(&a.1)
        }
    });
    let mut out = Vec::with_capacity(sorted.len());
    let mut group_start_rank = 1usize;
    let mut i = 0usize;
    while i < sorted.len() {
        // Extend the current tie-group while the score is bit-identical.
        let mut j = i + 1;
        while j < sorted.len() && sorted[j].1.to_bits() == sorted[i].1.to_bits() {
            j += 1;
        }
        // Every id in [i, j) shares `group_start_rank`.
        for entry in &sorted[i..j] {
            out.push((entry.0.clone(), group_start_rank));
        }
        group_start_rank += j - i;
        i = j;
    }
    out
}
