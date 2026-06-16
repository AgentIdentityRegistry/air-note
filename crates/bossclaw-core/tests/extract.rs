//! Pure unit tests for `extract.rs`: the resolution decision, the relation
//! vocabulary/cardinality tables, and the Pass-A reflexion parse.
//! No DB, no model — only the pure functions, driven by fixed inputs.

use bossclaw_core::extract::{
    build_pass_a_prompt, parse_proposals, propose, resolve_decision, Proposals, ResolveDecision,
    PASS_A_SYSTEM, RELATION_VOCAB, RESOLVE_HIGH, RESOLVE_LOW,
};
use bossclaw_core::reason::{extraction_schema, ScriptedReasoner};
use serde_json::json;

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

// ── Pass A tests (Task 4) ─────────────────────────────────────────────────

#[test]
fn pass_a_prompt_carries_source_recalled_vocabulary_cardinality_and_exemplars() {
    let prompt = build_pass_a_prompt(
        "Kenny started at Acme last week.",
        &["Kenny used to be at Globex.".to_string()],
    );
    // Source memory is present.
    assert!(prompt.contains("Kenny started at Acme"), "source memory present");
    // Recalled neighbor is present.
    assert!(prompt.contains("Globex"), "recalled neighbor present");
    // Seed relation vocabulary — the model must reuse these labels.
    assert!(prompt.contains("works_at"), "relation vocabulary present");
    assert!(RELATION_VOCAB.contains(&"works_at"), "vocab seed includes works_at");
    // F9(b): cardinality guidance distinguishes works_at vs works_at_primary.
    assert!(
        prompt.contains("works_at_primary"),
        "prompt must teach cardinality: works_at_primary is single-valued"
    );
    // F9(a): at least one hand-written few-shot exemplar is embedded.
    assert!(
        prompt.contains("EXAMPLE"),
        "prompt must contain few-shot exemplars (F9a)"
    );
}

#[test]
fn parse_proposals_reads_entities_relations_retractions_with_fields() {
    let raw = json!({
        "entities": [{ "mention": "Kenny", "entity_type": "person", "confidence": 0.9 }],
        "relations": [{
            "src": "Kenny", "relation": "works_at", "dst": "Acme",
            "confidence": 0.8, "supported_by": "Kenny started at Acme last week."
        }],
        "retractions": [{
            "src": "Kenny", "relation": "works_at", "dst": "Globex",
            "reason": "moved to Acme", "confidence": 0.7
        }]
    });
    let p: Proposals = parse_proposals(&raw).unwrap();
    assert_eq!(p.entities.len(), 1);
    assert_eq!(p.entities[0].mention, "Kenny");
    assert_eq!(p.relations.len(), 1);
    assert_eq!(p.relations[0].relation, "works_at");
    assert_eq!(p.relations[0].supported_by, "Kenny started at Acme last week.");
    assert!((p.relations[0].confidence - 0.8).abs() < 1e-6);
    assert_eq!(p.retractions.len(), 1);
    assert_eq!(p.retractions[0].dst, "Globex");
}

#[test]
fn parse_proposals_rejects_a_relation_missing_supported_by() {
    // supported_by is mandatory — a relation without a source span is unverifiable
    // and must be dropped (Pass B would drop it anyway; the parser is the first gate).
    let raw = json!({
        "entities": [],
        "relations": [{ "src": "A", "relation": "knows", "dst": "B", "confidence": 0.9 }],
        "retractions": []
    });
    let p = parse_proposals(&raw).unwrap();
    assert!(p.relations.is_empty(), "a relation with no supported_by span is dropped");
}

#[test]
fn propose_runs_pass_a_through_the_reasoner() {
    let source = "Kenny started at Acme last week.";
    let recalled = vec!["Kenny used to be at Globex.".to_string()];
    let prompt = build_pass_a_prompt(source, &recalled);
    let canned = json!({
        "entities": [{ "mention": "Kenny", "entity_type": "person", "confidence": 0.95 }],
        "relations": [{
            "src": "Kenny", "relation": "works_at", "dst": "Acme",
            "confidence": 0.9, "supported_by": "Kenny started at Acme last week."
        }],
        "retractions": []
    });
    // Key the scripted reasoner on propose()'s actual (system, prompt).
    let reasoner = ScriptedReasoner::new("m4-reasoner")
        .with_response(PASS_A_SYSTEM, &prompt, canned);
    let p = propose(&reasoner, source, &recalled).unwrap();
    assert_eq!(p.entities[0].mention, "Kenny");
    assert_eq!(p.relations[0].dst, "Acme");
    // Sanity: the schema builder is the one propose() passes.
    assert_eq!(extraction_schema()["type"], json!("object"));
}
