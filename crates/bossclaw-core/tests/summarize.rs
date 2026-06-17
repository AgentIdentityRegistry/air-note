//! Pure tests for the M4b summarizer pipeline (compose prompt + parse). The
//! citation floor + assemble are tested in this file too (Task 3). The live model
//! is proven by the `#[ignore]` gate in `tests/live_ollama.rs`.
use bossclaw_core::graph::Entity;
use bossclaw_core::summarize::{build_compose_prompt, compose_schema, parse_draft, FactSet};
use serde_json::json;

fn facts() -> FactSet {
    FactSet {
        entity: Entity {
            entity_id: "entity:01K".into(),
            label: "Kenny".into(),
            aliases: vec![],
            entity_type: "person".into(),
        },
        edges: vec!["entity:01K -works_at-> entity:01A".into()],
        memories: vec![("01MEM".into(), "Kenny works at Acme.".into())],
    }
}

#[test]
fn compose_prompt_fences_sources_and_tags_ids_and_asks_to_cite() {
    let p = build_compose_prompt(&facts());
    assert!(p.contains("Kenny works at Acme."), "memory text present");
    assert!(p.contains("01MEM"), "each memory tagged with its event id (for cites)");
    assert!(
        p.contains("entity:01K -works_at-> entity:01A"),
        "edges present as lines"
    );
    assert!(
        p.contains("<<<SOURCE_BEGIN") && p.contains("SOURCE_END>>>"),
        "untrusted text fenced"
    );
    assert!(p.to_lowercase().contains("cite"), "instructs the model to cite source ids");
}

#[test]
fn parse_draft_reads_title_and_claims_with_cites() {
    let raw = json!({ "title": "Kenny",
        "claims": [{ "text": "Kenny works at Acme.", "cites": ["01MEM"] }] });
    let d = parse_draft(&raw).unwrap();
    assert_eq!(d.title, "Kenny");
    assert_eq!(d.claims.len(), 1);
    assert_eq!(d.claims[0].cites, vec!["01MEM".to_string()]);
}

#[test]
fn compose_schema_constrains_title_and_claims() {
    let s = compose_schema();
    assert_eq!(s["type"], json!("object"));
    assert_eq!(s["properties"]["claims"]["type"], json!("array"));
    let item = &s["properties"]["claims"]["items"]["properties"];
    assert!(item.get("text").is_some() && item.get("cites").is_some());
}
