//! Tests for the M4a reasoner seam: the deterministic `ScriptedReasoner` double
//! and the JSON-schema builders. The live backend is proven separately by the
//! `#[ignore]` gate in `tests/live_ollama.rs`.

use bossclaw_core::reason::{
    adjudication_schema, extraction_schema, Reasoner, ScriptedReasoner,
};
use serde_json::json;

#[test]
fn scripted_reasoner_returns_canned_json_keyed_by_system_and_prompt() {
    let canned = json!({ "entities": [], "relations": [], "retractions": [] });
    let reasoner = ScriptedReasoner::new("test-scripted-v1")
        .with_response("SYS", "PROMPT-A", canned.clone());

    // Exact (system, prompt) match → the canned value, byte-for-byte.
    let got = reasoner
        .complete_json("SYS", "PROMPT-A", &extraction_schema())
        .unwrap();
    assert_eq!(got, canned);

    // Deterministic: a second identical call yields the identical value.
    let again = reasoner
        .complete_json("SYS", "PROMPT-A", &extraction_schema())
        .unwrap();
    assert_eq!(again, canned);

    // model_id is the configured stamp.
    assert_eq!(reasoner.model_id(), "test-scripted-v1");
}

#[test]
fn scripted_reasoner_errors_on_unknown_prompt() {
    let reasoner = ScriptedReasoner::new("test-scripted-v1");
    let err = reasoner
        .complete_json("SYS", "UNSEEN", &extraction_schema())
        .expect_err("an unscripted (system,prompt) must error, not hang or panic");
    assert!(
        matches!(err, bossclaw_core::BossclawError::Reasoner(_)),
        "unknown prompt must surface as BossclawError::Reasoner, got {err:?}"
    );
}

#[test]
fn scripted_reasoner_does_not_collide_on_equal_concatenated_length() {
    // ("ab","c") and ("a","bc") concatenate to the same 3-char string. The
    // `0x1F` unit separator between system and prompt keeps their keys distinct,
    // so a response registered for one pair must NOT answer the other.
    let canned = json!({ "marker": "ab|c" });
    let reasoner = ScriptedReasoner::new("test-scripted-v1")
        .with_response("ab", "c", canned.clone());

    // The exact pair hits.
    assert_eq!(
        reasoner.complete_json("ab", "c", &extraction_schema()).unwrap(),
        canned
    );

    // The length-equal sibling must MISS (proves no concatenation collision).
    let err = reasoner
        .complete_json("a", "bc", &extraction_schema())
        .expect_err("('a','bc') must not collide with ('ab','c')");
    assert!(
        matches!(err, bossclaw_core::BossclawError::Reasoner(_)),
        "a non-colliding miss must surface as BossclawError::Reasoner, got {err:?}"
    );
}

#[test]
fn extraction_schema_constrains_the_three_proposal_arrays() {
    let schema = extraction_schema();
    // Object schema with the three top-level proposal arrays the prompt asks for.
    assert_eq!(schema["type"], json!("object"));
    let props = &schema["properties"];
    for key in ["entities", "relations", "retractions"] {
        assert_eq!(props[key]["type"], json!("array"), "{key} must be an array");
    }
    // An entity item carries the mention + type + confidence the parser reads.
    let entity_item = &props["entities"]["items"]["properties"];
    assert!(entity_item.get("mention").is_some());
    assert!(entity_item.get("entity_type").is_some());
    assert!(entity_item.get("confidence").is_some());

    // A relation item carries the supported_by span + confidence the parser reads.
    let rel_item = &props["relations"]["items"]["properties"];
    assert!(rel_item.get("src").is_some());
    assert!(rel_item.get("relation").is_some());
    assert!(rel_item.get("dst").is_some());
    assert!(rel_item.get("confidence").is_some());
    assert!(rel_item.get("supported_by").is_some());

    // A retraction item carries the (src,relation,dst) it retires plus a reason.
    let retraction_item = &props["retractions"]["items"]["properties"];
    assert!(retraction_item.get("src").is_some());
    assert!(retraction_item.get("relation").is_some());
    assert!(retraction_item.get("dst").is_some());
    assert!(retraction_item.get("reason").is_some());
    assert!(retraction_item.get("confidence").is_some());
}

#[test]
fn adjudication_schema_constrains_a_single_choice() {
    let schema = adjudication_schema();
    assert_eq!(schema["type"], json!("object"));
    // The adjudicator answers "which candidate (or none)" → a string match id.
    assert!(schema["properties"].get("match").is_some());
}
