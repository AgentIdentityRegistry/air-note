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
fn extraction_schema_constrains_the_three_proposal_arrays() {
    let schema = extraction_schema();
    // Object schema with the three top-level proposal arrays the prompt asks for.
    assert_eq!(schema["type"], json!("object"));
    let props = &schema["properties"];
    for key in ["entities", "relations", "retractions"] {
        assert_eq!(props[key]["type"], json!("array"), "{key} must be an array");
    }
    // A relation item carries the supported_by span + confidence the parser reads.
    let rel_item = &props["relations"]["items"]["properties"];
    assert!(rel_item.get("src").is_some());
    assert!(rel_item.get("relation").is_some());
    assert!(rel_item.get("dst").is_some());
    assert!(rel_item.get("confidence").is_some());
    assert!(rel_item.get("supported_by").is_some());
}

#[test]
fn adjudication_schema_constrains_a_single_choice() {
    let schema = adjudication_schema();
    assert_eq!(schema["type"], json!("object"));
    // The adjudicator answers "which candidate (or none)" → a string match id.
    assert!(schema["properties"].get("match").is_some());
}
