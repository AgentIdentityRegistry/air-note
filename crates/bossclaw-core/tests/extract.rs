//! Pure unit tests for `extract.rs`: the resolution decision, the relation
//! vocabulary/cardinality tables, and (later tasks) the reflexion parse/critique.
//! No DB, no model — only the pure functions, driven by fixed inputs.

use bossclaw_core::extract::{resolve_decision, ResolveDecision, RESOLVE_HIGH, RESOLVE_LOW};

#[test]
fn resolve_decision_auto_merges_above_high() {
    // Best candidate's cosine similarity ≥ RESOLVE_HIGH → auto-merge to it.
    let d = resolve_decision(&[("entity:ken".to_string(), RESOLVE_HIGH + 0.01)]);
    assert_eq!(d, ResolveDecision::Merge("entity:ken".to_string()));
}

#[test]
fn resolve_decision_mints_below_low() {
    // Best candidate ≤ RESOLVE_LOW (or no candidates) → mint a fresh entity.
    let d = resolve_decision(&[("entity:ken".to_string(), RESOLVE_LOW - 0.01)]);
    assert_eq!(d, ResolveDecision::Mint);
    assert_eq!(resolve_decision(&[]), ResolveDecision::Mint, "no candidates → mint");
}

#[test]
fn resolve_decision_routes_midband_to_adjudication_with_candidate_list() {
    // Strictly between LOW and HIGH → adjudicate among the candidates (sorted
    // best-first), so the model picks "same as one of these, or none".
    let cands = vec![
        ("entity:ken".to_string(), 0.80),
        ("entity:kenji".to_string(), 0.78),
    ];
    match resolve_decision(&cands) {
        ResolveDecision::Adjudicate(ids) => {
            assert_eq!(ids, vec!["entity:ken".to_string(), "entity:kenji".to_string()]);
        }
        other => panic!("mid-band must adjudicate, got {other:?}"),
    }
}
