//! Pure unit tests for `extract.rs`: the resolution decision, the relation
//! vocabulary/cardinality tables, and the Pass-A reflexion parse.
//! No DB, no model — only the pure functions, driven by fixed inputs.

use bossclaw_core::extract::{
    build_pass_a_prompt, build_pass_b_prompt, confirm_retractions, critique_with_reasoner,
    is_single_valued, parse_proposals, propose, resolve_decision, verify_floor, Proposals,
    ProposedRelation, ProposedRetraction, ResolveDecision, PASS_A_SYSTEM, PASS_B_SYSTEM,
    RELATION_VOCAB, RESOLVE_HIGH, RESOLVE_LOW,
};
use bossclaw_core::reason::ScriptedReasoner;
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
    // F9(a): the exemplar BODY (not just the section header) is rendered — this
    // substring only exists if PASS_A_EXEMPLARS was actually pushed.
    assert!(
        prompt.contains("Alice joined Umbrella Corp"),
        "prompt must embed exemplar BODY, not just the header (F9a)"
    );
    // I-3: the untrusted source text is wrapped in explicit fence markers.
    assert!(
        prompt.contains("<<<SOURCE_BEGIN>>>") && prompt.contains("<<<SOURCE_END>>>"),
        "source memory must be fenced (untrusted-content fence, design §8.4)"
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
}

#[test]
fn parse_proposals_clamps_out_of_range_confidence() {
    // I-2: a model can emit confidences outside [0, 1]; the parser clamps them so
    // Proposals honors the documented invariant before Task 6's trust gate reads it.
    let raw = json!({
        "entities": [{ "mention": "X", "entity_type": "person", "confidence": 1.5 }],
        "relations": [{
            "src": "X", "relation": "knows", "dst": "Y",
            "confidence": -0.3, "supported_by": "X knows Y."
        }],
        "retractions": []
    });
    let p = parse_proposals(&raw).unwrap();
    assert!((p.entities[0].confidence - 1.0).abs() < 1e-6, "1.5 clamps to 1.0");
    assert!((p.relations[0].confidence - 0.0).abs() < 1e-6, "-0.3 clamps to 0.0");
}

// ── Pass B tests (Task 5 / Rev 2 F1) ─────────────────────────────────────

#[test]
fn single_valued_relations_are_recognised() {
    assert!(is_single_valued("works_at_primary"));
    assert!(is_single_valued("located_in"));
    assert!(!is_single_valued("knows"), "knows is many-valued");
}

#[test]
fn confirm_retractions_only_fires_on_an_active_edge() {
    // A retraction is confirmed ONLY when its (src, relation, dst) is a still-
    // active edge in the current graph. A retraction of a non-existent edge is
    // dropped (it cannot contradict what was never asserted).
    let retractions = vec![
        ProposedRetraction {
            src: "Kenny".into(),
            relation: "works_at_primary".into(),
            dst: "Globex".into(),
            reason: "moved".into(),
            confidence: 0.9,
        },
        ProposedRetraction {
            src: "Kenny".into(),
            relation: "works_at_primary".into(),
            dst: "NeverCorp".into(),
            reason: "x".into(),
            confidence: 0.9,
        },
    ];
    // Current active edge-keys (src, relation, dst) the loop passes in.
    let active = vec![(
        "Kenny".to_string(),
        "works_at_primary".to_string(),
        "Globex".to_string(),
    )];
    let confirmed = confirm_retractions(&retractions, &active);
    assert_eq!(confirmed.len(), 1, "only the retraction matching an active edge survives");
    assert_eq!(confirmed[0].dst, "Globex");
}

#[test]
fn verify_floor_drops_relations_whose_span_is_absent_from_source() {
    // The pure floor drops any relation whose supported_by span is NOT a verbatim
    // substring of the source — the model hallucinated support.
    let source = "Kenny started at Acme last week.";
    let proposals = Proposals {
        entities: vec![],
        relations: vec![
            ProposedRelation {
                src: "Kenny".into(),
                relation: "works_at".into(),
                dst: "Acme".into(),
                confidence: 0.9,
                supported_by: "Kenny started at Acme last week.".into(),
            },
            ProposedRelation {
                src: "Kenny".into(),
                relation: "knows".into(),
                dst: "Zoe".into(),
                confidence: 0.9,
                // this span does NOT appear in source
                supported_by: "Kenny and Zoe are close friends.".into(),
            },
        ],
        retractions: vec![],
    };
    let floor = verify_floor(&proposals, source);
    assert_eq!(floor.relations.len(), 1, "the unsupported relation is dropped by the floor");
    assert_eq!(floor.relations[0].dst, "Acme");
}

/// Build a `Proposals` with a single relation for scripted-reasoner tests.
fn one_relation(src: &str, rel: &str, dst: &str, conf: f32, span: &str) -> Proposals {
    Proposals {
        entities: vec![],
        relations: vec![ProposedRelation {
            src: src.into(),
            relation: rel.into(),
            dst: dst.into(),
            confidence: conf,
            supported_by: span.into(),
        }],
        retractions: vec![],
    }
}

#[test]
fn critique_with_reasoner_model_drop_is_honored() {
    // When the model drops a relation from its critique output, it must be absent
    // from the refined set — even though the floor would have kept it.
    let source = "Kenny started at Acme last week.";
    let span = "Kenny started at Acme last week.";
    let proposals = one_relation("Kenny", "works_at", "Acme", 0.9, span);

    // Build the exact prompt that critique_with_reasoner will construct.
    let floor = verify_floor(&proposals, source);
    let neighborhood: Vec<String> = vec![];
    let prompt = build_pass_b_prompt(source, &floor, &neighborhood);

    // Model returns an empty relations list — it dropped everything.
    let model_response = json!({
        "entities": [],
        "relations": [],
        "retractions": []
    });
    let reasoner = ScriptedReasoner::new("test-scripted-v1")
        .with_response(PASS_B_SYSTEM, &prompt, model_response);

    let refined = critique_with_reasoner(&reasoner, source, &proposals, &neighborhood).unwrap();
    assert!(
        refined.relations.is_empty(),
        "model DROP must be honored — relation absent from refined output"
    );
}

#[test]
fn critique_with_reasoner_model_invented_relation_is_rejected() {
    // SECURITY-CRITICAL: the model invents a relation NOT in the floor-verified
    // set. The intersect must reject it — it can NEVER appear in the output.
    let source = "Kenny started at Acme last week.";
    let span = "Kenny started at Acme last week.";
    let proposals = one_relation("Kenny", "works_at", "Acme", 0.9, span);

    let floor = verify_floor(&proposals, source);
    let neighborhood: Vec<String> = vec![];
    let prompt = build_pass_b_prompt(source, &floor, &neighborhood);

    // Model INVENTS a new relation (Alice-knows-Bob) that was never proposed.
    // This simulates a hallucination or injection attack via the source text.
    let model_response = json!({
        "entities": [],
        "relations": [
            // The floor-supported relation (kept by model).
            {
                "src": "Kenny", "relation": "works_at", "dst": "Acme",
                "confidence": 0.85, "supported_by": span
            },
            // INVENTED — not in the floor set.
            {
                "src": "Alice", "relation": "knows", "dst": "Bob",
                "confidence": 0.99, "supported_by": "Alice and Bob are friends."
            }
        ],
        "retractions": []
    });
    let reasoner = ScriptedReasoner::new("test-scripted-v1")
        .with_response(PASS_B_SYSTEM, &prompt, model_response);

    let refined = critique_with_reasoner(&reasoner, source, &proposals, &neighborhood).unwrap();
    assert_eq!(
        refined.relations.len(),
        1,
        "SECURITY: model-invented relation must be REJECTED — only floor-supported ones survive"
    );
    assert_eq!(refined.relations[0].src, "Kenny");
    assert_eq!(refined.relations[0].dst, "Acme");
    // Verify Alice-Bob is nowhere.
    assert!(
        refined.relations.iter().all(|r| r.src != "Alice"),
        "invented Alice→Bob must be absent from refined output"
    );
}

#[test]
fn critique_with_reasoner_model_down_confidences_relation() {
    // The model may DOWN-confidence a relation (take the min). It must not raise it.
    let source = "Kenny started at Acme last week.";
    let span = "Kenny started at Acme last week.";
    // Floor proposes confidence = 0.9.
    let proposals = one_relation("Kenny", "works_at", "Acme", 0.9, span);

    let floor = verify_floor(&proposals, source);
    let neighborhood: Vec<String> = vec![];
    let prompt = build_pass_b_prompt(source, &floor, &neighborhood);

    // Model returns a LOWER confidence (0.5) — this should be taken (min).
    let model_response = json!({
        "entities": [],
        "relations": [{
            "src": "Kenny", "relation": "works_at", "dst": "Acme",
            "confidence": 0.5, "supported_by": span
        }],
        "retractions": []
    });
    let reasoner = ScriptedReasoner::new("test-scripted-v1")
        .with_response(PASS_B_SYSTEM, &prompt, model_response);

    let refined = critique_with_reasoner(&reasoner, source, &proposals, &neighborhood).unwrap();
    assert_eq!(refined.relations.len(), 1);
    // min(0.9_floor, 0.5_model) = 0.5
    assert!(
        (refined.relations[0].confidence - 0.5).abs() < 1e-6,
        "model must be able to DOWN-confidence but not raise it; expected 0.5, got {}",
        refined.relations[0].confidence
    );
}

#[test]
fn verify_floor_protects_regardless_of_model_opinion() {
    // A relation whose supported_by is NOT in source is dropped by the floor
    // even if the model would have kept it. The floor is the security boundary.
    let source = "Kenny started at Acme last week.";
    let proposals = Proposals {
        entities: vec![],
        relations: vec![ProposedRelation {
            src: "Kenny".into(),
            relation: "knows".into(),
            dst: "Zoe".into(),
            confidence: 0.95,
            // span NOT in source — hallucinated
            supported_by: "Kenny and Zoe are close friends.".into(),
        }],
        retractions: vec![],
    };

    let floor = verify_floor(&proposals, source);
    let neighborhood: Vec<String> = vec![];
    let prompt = build_pass_b_prompt(source, &floor, &neighborhood);

    // Model tries to keep the hallucinated relation by returning it.
    let model_response = json!({
        "entities": [],
        "relations": [{
            "src": "Kenny", "relation": "knows", "dst": "Zoe",
            "confidence": 0.95,
            "supported_by": "Kenny and Zoe are close friends."
        }],
        "retractions": []
    });
    let reasoner = ScriptedReasoner::new("test-scripted-v1")
        .with_response(PASS_B_SYSTEM, &prompt, model_response);

    let refined = critique_with_reasoner(&reasoner, source, &proposals, &neighborhood).unwrap();
    assert!(
        refined.relations.is_empty(),
        "floor must drop the unsupported relation regardless of model opinion"
    );
}
