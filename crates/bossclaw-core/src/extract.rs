//! Pure extraction + resolution logic (spec §6): the retrieval-augmented prompt
//! construction, response parsing, the reflexion state machine, and the
//! entity-resolution decision. PURE — no SQL, no I/O, no `Store`. Takes recall +
//! graph results as inputs and calls the [`crate::reason::Reasoner`] trait, so it
//! is unit-testable with [`crate::reason::ScriptedReasoner`]. Mirrors the pure
//! split in [`crate::recall`] / [`crate::graph`].

/// Cosine auto-merge floor (spec §11). At or above this similarity a mention is
/// merged into the best candidate entity WITHOUT asking the model. High on
/// purpose — a wrong entity merge is expensive to undo. Tunable in dogfooding.
pub const RESOLVE_HIGH: f32 = 0.92;

/// Mint-new ceiling (spec §11). At or below this similarity a mention mints a
/// fresh entity. Between [`RESOLVE_LOW`] and [`RESOLVE_HIGH`] the model
/// adjudicates ("same as one of these, or none"). Tunable in dogfooding.
pub const RESOLVE_LOW: f32 = 0.75;

/// Total reflexion passes per memory (spec §11): propose (Pass A) + one critique
/// (Pass B). Bounds the model calls per memory so a tick's latency is bounded.
pub const MAX_REFLECT: u32 = 2;

/// Minimum machine-edge confidence to contribute the recall boost / be actuator-
/// eligible (spec §7, §11). Below this a machine edge is still recorded
/// (never-forget) and queryable, but does not tilt recall. Tunable.
pub const TRUST_MIN: f32 = 0.6;

/// How many recalled neighbors are fed as the Pass-A cheat sheet (spec §11). The
/// retrieval-augmented context that lets a 7b reconcile against KNOWN facts
/// rather than recall everything. Tunable.
pub const GRAPH_CONTEXT_K: usize = 8;

/// Maximum entities accepted from one memory (spec §11 / Rev 2 F6). A
/// booby-trapped memory cannot flood the entity index.
pub const MAX_ENTITIES_PER_MEMORY: usize = 32;

/// The outcome of resolving one entity mention against the existing entity nodes
/// (spec §6). The embedding-first, model-adjudicated mid-band decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveDecision {
    /// Cosine ≥ [`RESOLVE_HIGH`]: auto-merge into this existing entity id.
    Merge(String),
    /// Cosine ≤ [`RESOLVE_LOW`] (or no candidates): mint a fresh `entity` event.
    Mint,
    /// Mid-band: ask the reasoner to pick among these candidate ids (best-first)
    /// or answer "none" (→ mint). The caller runs the adjudication step.
    Adjudicate(Vec<String>),
}

/// Decide how to resolve a mention given its `candidates` as `(entity_id,
/// cosine_similarity)` pairs (NOT necessarily sorted). Pure: compares the BEST
/// candidate's similarity to the thresholds. `Merge` above HIGH, `Mint` at/below
/// LOW or when empty, else `Adjudicate` with all candidates sorted best-first.
///
/// Cosine similarity here is `1 - distance` (the vector index returns distance);
/// the caller converts before calling so this function speaks one scale.
///
/// Similarity is expected in `[-1, 1]` (the cosine distance `d` from `DistCosine`
/// on unit-norm vectors, converted by `1.0 - d`); a value below 0 (an obtuse
/// angle) is safely `<= RESOLVE_LOW`, so it mints — no special-casing needed.
pub fn resolve_decision(candidates: &[(String, f32)]) -> ResolveDecision {
    let mut sorted: Vec<&(String, f32)> = candidates.iter().collect();
    // Best (highest similarity) first; id as a deterministic tie-break.
    sorted.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let best = match sorted.first() {
        Some(c) => c.1,
        None => return ResolveDecision::Mint,
    };
    if best >= RESOLVE_HIGH {
        ResolveDecision::Merge(sorted[0].0.clone())
    } else if best <= RESOLVE_LOW {
        ResolveDecision::Mint
    } else {
        ResolveDecision::Adjudicate(sorted.into_iter().map(|(id, _)| id.clone()).collect())
    }
}

/// Build the entity-resolution adjudication prompt (spec §6): the mention plus a
/// short candidate list (best-first). The model answers with one candidate id or
/// `"none"`. Pure string construction so it is unit-testable.
pub fn build_adjudication_prompt(mention: &str, candidate_ids: &[String]) -> String {
    let mut s = String::new();
    s.push_str("Mention to resolve:\n");
    s.push_str(mention);
    s.push_str("\n\nCandidate entities (choose the one the mention refers to, or \"none\"):\n");
    for id in candidate_ids {
        s.push_str("- ");
        s.push_str(id);
        s.push('\n');
    }
    s
}
