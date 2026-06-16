//! Integration tests for the M3 bi-temporal graph: `link`/`invalidate` events,
//! `edges`/`nodes` schema, and the F2 producer gate.

use bossclaw_core::event::Event;
use bossclaw_core::log::EventLog;
use ed25519_dalek::SigningKey;
use serde_json::json;

const DEK: [u8; 32] = [42u8; 32];
const KEY_BYTES: [u8; 32] = [7u8; 32];
const DID: &str = "did:wba:AIR-TEST";

fn open_log(dir: &std::path::Path) -> EventLog {
    let key = SigningKey::from_bytes(&KEY_BYTES);
    EventLog::open(&dir.join("m.db"), &DEK, key).unwrap()
}

fn mk_memory(text: &str) -> Event {
    Event {
        id: String::new(),
        ts: String::new(),
        valid_time: None,
        event_type: "memory".to_string(),
        content: json!({ "text": text }),
        model_meta: None,
        prev_hash: String::new(),
        hash: None,
        signed_by_did: DID.to_string(),
        signature: None,
    }
}

#[test]
fn link_appends_tier_b_event_with_source_ids() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let a = log.append(mk_memory("kenny")).unwrap();
    let b = log.append(mk_memory("acme")).unwrap();

    // Empty source_event_ids → helper defaults to [src, dst] (non-empty, so append accepts it).
    let edge_event_id = log.link(&a, "works_at", &b, None, &[]).unwrap();

    let ev = log.stream_all().unwrap().into_iter().find(|e| e.id == edge_event_id).unwrap();
    assert_eq!(ev.event_type, "link");
    assert_eq!(ev.content["src"], json!(a));
    assert_eq!(ev.content["relation"], json!("works_at"));
    assert_eq!(ev.content["dst"], json!(b));
    let meta = ev.model_meta.expect("link is Tier-B");
    assert_eq!(meta.model_id, "manual");
    assert_eq!(meta.source_event_ids, vec![a.clone(), b.clone()]);
}

#[test]
fn invalidate_appends_event_with_edge_key() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let a = log.append(mk_memory("kenny")).unwrap();
    let b = log.append(mk_memory("acme")).unwrap();
    log.link(&a, "works_at", &b, None, &[]).unwrap();

    let inv_id = log.invalidate(&a, "works_at", &b, None, std::slice::from_ref(&a)).unwrap();
    let ev = log.stream_all().unwrap().into_iter().find(|e| e.id == inv_id).unwrap();
    assert_eq!(ev.event_type, "invalidate");
    assert_eq!(ev.content["src"], json!(a));
    assert_eq!(ev.model_meta.unwrap().source_event_ids, vec![a]);
}

// ── Task 2: rebuild_graph + all_edges + all_nodes ────────────────────────────

use bossclaw_core::graph::Edge;

#[test]
fn rebuild_graph_is_byte_identical_across_rebuilds() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let a = log.append(mk_memory("kenny")).unwrap();
    let b = log.append(mk_memory("acme")).unwrap();
    let c = log.append(mk_memory("beta corp")).unwrap();
    log.link(&a, "works_at", &b, None, &[]).unwrap();
    log.link(&a, "knows", &c, None, &[]).unwrap();
    log.invalidate(&a, "works_at", &b, None, std::slice::from_ref(&a)).unwrap();

    log.rebuild_graph().unwrap();
    let edges1 = log.all_edges().unwrap();
    let nodes1 = log.all_nodes().unwrap();
    log.rebuild_graph().unwrap();
    let edges2 = log.all_edges().unwrap();
    let nodes2 = log.all_nodes().unwrap();

    assert_eq!(edges1, edges2, "edges fold must be byte-identical across rebuilds");
    assert_eq!(nodes1, nodes2, "nodes fold must be byte-identical across rebuilds");
    assert_eq!(edges1.len(), 2, "two link events → two edge rows");
    assert_eq!(nodes1.len(), 3, "three distinct endpoints → three nodes");
}

#[test]
fn invalidate_closes_not_deletes_and_relink_opens_new_interval() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let a = log.append(mk_memory("kenny")).unwrap();
    let b = log.append(mk_memory("acme")).unwrap();

    log.link(&a, "works_at", &b, None, &[]).unwrap();
    log.invalidate(&a, "works_at", &b, None, std::slice::from_ref(&a)).unwrap();
    log.link(&a, "works_at", &b, None, &[]).unwrap(); // re-link → new interval
    log.rebuild_graph().unwrap();

    let edges = log.all_edges().unwrap();
    assert_eq!(edges.len(), 2, "invalidate closes (not deletes); re-link adds a 2nd row");
    let closed: Vec<&Edge> = edges.iter().filter(|e| e.invalidated_at.is_some()).collect();
    let open: Vec<&Edge> = edges.iter().filter(|e| e.is_current()).collect();
    assert_eq!(closed.len(), 1, "exactly one closed assertion");
    assert_eq!(open.len(), 1, "exactly one re-opened current assertion");
    assert!(closed[0].valid_to.is_some(), "closed edge carries a world-clock end");
    assert!(closed[0].invalidated_by.is_some(), "closed edge records its invalidator");
}

#[test]
fn nodes_kind_is_memory_for_memory_endpoints() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let a = log.append(mk_memory("kenny")).unwrap();
    // dst is an id that does NOT exist as a memory event → "external".
    log.link(&a, "mentions", "entity:acme", None, std::slice::from_ref(&a)).unwrap();
    log.rebuild_graph().unwrap();

    let nodes = log.all_nodes().unwrap();
    let kind_of = |id: &str| nodes.iter().find(|n| n.node_id == id).map(|n| n.kind.clone());
    assert_eq!(kind_of(&a).as_deref(), Some("memory"));
    assert_eq!(kind_of("entity:acme").as_deref(), Some("external"));
}

// ── Rev 2 T-A: invalidate closes ALL active assertions for a key ─────────────

#[test]
fn invalidate_closes_all_active_assertions_for_a_key() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let a = log.append(mk_memory("kenny")).unwrap();
    let b = log.append(mk_memory("acme")).unwrap();
    log.link(&a, "works_at", &b, None, &[]).unwrap();
    log.link(&a, "works_at", &b, None, &[]).unwrap(); // two concurrent assertions, same key
    log.invalidate(&a, "works_at", &b, None, std::slice::from_ref(&a)).unwrap();
    log.rebuild_graph().unwrap();
    let edges = log.all_edges().unwrap();
    assert_eq!(edges.len(), 2, "both assertions persist (close-not-delete)");
    assert!(
        edges.iter().all(|e| e.invalidated_at.is_some()),
        "one invalidate closes ALL active assertions for the key"
    );
}

// ── Rev 2 T-C: invalidate with no active assertion is a no-op ────────────────

#[test]
fn invalidate_with_no_active_assertion_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let a = log.append(mk_memory("kenny")).unwrap();
    let b = log.append(mk_memory("acme")).unwrap();
    log.invalidate(&a, "works_at", &b, None, std::slice::from_ref(&a)).unwrap(); // before any link
    log.rebuild_graph().unwrap();
    assert!(log.all_edges().unwrap().is_empty(), "invalidate with no matching link adds no edge");
}

// ── Task 3: neighbors + backlinks ────────────────────────────────────────────

#[test]
fn neighbors_returns_current_edges_both_directions_backlinks_filterable() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let a = log.append(mk_memory("kenny")).unwrap();
    let b = log.append(mk_memory("acme")).unwrap();
    let c = log.append(mk_memory("carol")).unwrap();
    log.link(&a, "works_at", &b, None, &[]).unwrap(); // a → b (outgoing from a)
    log.link(&c, "manages", &a, None, &[]).unwrap();   // c → a (incoming to a = backlink)
    let stale = log.link(&a, "old", &b, None, &[]).unwrap();
    log.invalidate(&a, "old", &b, None, std::slice::from_ref(&a)).unwrap(); // closed → excluded
    log.rebuild_graph().unwrap();

    let n = log.neighbors(&a).unwrap();
    assert_eq!(n.len(), 2, "two current edges touch a (the invalidated 'old' edge is excluded)");
    assert!(n.iter().all(|e| e.edge_id != stale), "invalidated edge must not appear");

    // Backlinks = the subset whose dst == a (incoming).
    let backlinks: Vec<&_> = n.iter().filter(|e| e.dst == a).collect();
    assert_eq!(backlinks.len(), 1);
    assert_eq!(backlinks[0].src, c);
    assert_eq!(backlinks[0].relation, "manages");
}

// ── Rev 2 T-E: SQL-injection regression ──────────────────────────────────────

#[test]
fn malicious_relation_label_is_inert_data_not_sql() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let a = log.append(mk_memory("kenny")).unwrap();
    let b = log.append(mk_memory("acme")).unwrap();
    let evil = "x\") OR 1=1 --";
    log.link(&a, evil, &b, None, &[]).unwrap();
    log.rebuild_graph().unwrap();
    let n = log.neighbors(&a).unwrap();
    assert_eq!(n.len(), 1, "one edge; the label did not alter query semantics");
    assert_eq!(n[0].relation, evil, "relation round-trips as literal data");
}

// ── Task 2 (M4a): entity event + entities projection + entity-node fold ──────

#[test]
fn entity_appends_tier_b_event_with_explicit_sources() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let m = log.append(mk_memory("kenny works at acme")).unwrap();

    // entity() is a NON-manual producer → MUST pass explicit non-empty sources.
    let entity_id = log
        .entity("Kenny", &["Ken".to_string()], "person", "m4-reasoner", std::slice::from_ref(&m))
        .unwrap();

    assert!(
        entity_id.starts_with(bossclaw_core::graph::ENTITY_NODE_PREFIX),
        "node id is namespaced: {entity_id}"
    );
    let ev = log.stream_all().unwrap().into_iter()
        .find(|e| format!("entity:{}", e.id) == entity_id).unwrap();
    assert_eq!(ev.event_type, "entity");
    assert_eq!(ev.content["label"], json!("Kenny"));
    assert_eq!(ev.content["aliases"], json!(["Ken"]));
    assert_eq!(ev.content["entity_type"], json!("person"));
    let meta = ev.model_meta.expect("entity is Tier-B");
    assert_eq!(meta.model_id, "m4-reasoner");
    assert_eq!(meta.source_event_ids, vec![m]);
}

#[test]
fn entity_rejects_non_manual_producer_with_empty_sources() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    // F2 guard extended to entity: a non-manual producer with empty sources is
    // rejected (an empty default would erase the inducing memory from lineage).
    let err = log
        .entity("Kenny", &[], "person", "m4-reasoner", &[])
        .expect_err("non-manual entity with empty sources must be rejected");
    assert!(matches!(err, bossclaw_core::BossclawError::InvalidInput(_)));
}

#[test]
fn rebuild_graph_folds_entities_and_marks_entity_node_kind() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let m = log.append(mk_memory("kenny")).unwrap();
    let kenny = log
        .entity("Kenny", &["Ken".to_string()], "person", "m4-reasoner", std::slice::from_ref(&m))
        .unwrap();
    // A link from the memory to the entity node.
    log.link(&m, "mentions", &kenny, None, std::slice::from_ref(&m)).unwrap();
    log.rebuild_graph().unwrap();

    // entities projection populated from the entity event.
    let entities = log.all_entities().unwrap();
    assert_eq!(entities.len(), 1);
    assert_eq!(entities[0].entity_id, kenny);
    assert_eq!(entities[0].label, "Kenny");
    assert_eq!(entities[0].aliases, vec!["Ken".to_string()]);
    assert_eq!(entities[0].entity_type, "person");

    // The entity endpoint is marked kind="entity" (NOT "external"), the memory
    // endpoint stays "memory".
    let nodes = log.all_nodes().unwrap();
    let kind_of = |id: &str| nodes.iter().find(|n| n.node_id == id).map(|n| n.kind.clone());
    assert_eq!(kind_of(&kenny).as_deref(), Some("entity"));
    assert_eq!(kind_of(&m).as_deref(), Some("memory"));
}

#[test]
fn rebuild_graph_is_byte_identical_with_entities() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let m = log.append(mk_memory("kenny at acme")).unwrap();
    let kenny = log.entity("Kenny", &[], "person", "m4-reasoner", std::slice::from_ref(&m)).unwrap();
    let acme = log.entity("Acme", &[], "org", "m4-reasoner", std::slice::from_ref(&m)).unwrap();
    log.link(&kenny, "works_at", &acme, None, std::slice::from_ref(&m)).unwrap();

    log.rebuild_graph().unwrap();
    let (e1, n1, ent1) = (log.all_edges().unwrap(), log.all_nodes().unwrap(), log.all_entities().unwrap());
    log.rebuild_graph().unwrap();
    let (e2, n2, ent2) = (log.all_edges().unwrap(), log.all_nodes().unwrap(), log.all_entities().unwrap());

    assert_eq!(e1, e2, "edges fold byte-identical with entities present");
    assert_eq!(n1, n2, "nodes fold byte-identical with entities present");
    assert_eq!(ent1, ent2, "entities fold byte-identical across rebuilds");
    assert_eq!(ent1.len(), 2, "two entity events → two entities rows");
}

// ── Rev 2 T-F: SQLi regression on entity label + alias paths ─────────────────

#[test]
fn malicious_entity_label_and_alias_are_inert_data_not_sql() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let m = log.append(mk_memory("bobby tables")).unwrap();
    let evil_label = r#"Robert"); DROP TABLE entities; --"#;
    let evil_alias = r#"Bobby'); DROP TABLE nodes; --"#;
    let entity_id = log
        .entity(evil_label, &[evil_alias.to_string()], "person", "m4-reasoner", std::slice::from_ref(&m))
        .unwrap();
    log.rebuild_graph().unwrap();

    let entities = log.all_entities().unwrap();
    assert_eq!(entities.len(), 1, "one entity — malicious label did not alter query semantics");
    assert_eq!(entities[0].entity_id, entity_id);
    assert_eq!(entities[0].label, evil_label, "label round-trips as inert literal data");
    assert_eq!(entities[0].aliases, vec![evil_alias.to_string()], "alias round-trips as inert literal data");
    // The tables still exist and query successfully — the DROP was inert.
    assert!(log.all_edges().unwrap().is_empty(), "edges table still exists and is queryable");
}

// ── Self-loop dedup in fold_edges/neighbors (M3 edge fold; gap found in T2 review) ──

#[test]
fn self_loop_is_one_edge_one_node_one_neighbor() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let a = log.append(mk_memory("self-referential")).unwrap();
    log.link(&a, "refers_to", &a, None, std::slice::from_ref(&a)).unwrap();
    log.rebuild_graph().unwrap();

    assert_eq!(log.all_edges().unwrap().len(), 1, "one link event → one edge row");
    assert_eq!(log.all_nodes().unwrap().len(), 1, "self-loop: endpoint deduplicates to one node");
    assert_eq!(
        log.neighbors(&a).unwrap().len(),
        1,
        "self-loop appears exactly once in neighbors, not twice"
    );
}

// ── Task 4: as_of (both clocks) ──────────────────────────────────────────────

use bossclaw_core::graph::AsOf;

#[test]
fn as_of_valid_time_shows_world_truth_then_known_as_of_shows_belief_then() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let a = log.append(mk_memory("kenny")).unwrap();
    let b = log.append(mk_memory("acme")).unwrap();

    // Kenny worked at Acme in the world from 2020 until 2022.
    log.link(&a, "works_at", &b, Some("2020-01-01T00:00:00Z"), &[]).unwrap();
    log.invalidate(&a, "works_at", &b, Some("2022-01-01T00:00:00Z"), std::slice::from_ref(&a)).unwrap();
    log.rebuild_graph().unwrap();

    // all-None == current: the edge is invalidated, so nothing current.
    assert!(log.as_of(&a, &AsOf::default()).unwrap().is_empty(), "edge is retired → not current");

    // valid_time inside [2020, 2022): the fact WAS true in the world.
    let mid = AsOf { valid_time: Some("2021-06-01T00:00:00Z".into()), known_as_of: None };
    assert_eq!(log.as_of(&a, &mid).unwrap().len(), 1, "true in the world in 2021");

    // valid_time after 2022: no longer true.
    let after = AsOf { valid_time: Some("2023-01-01T00:00:00Z".into()), known_as_of: None };
    assert!(log.as_of(&a, &after).unwrap().is_empty(), "not true in the world in 2023");

    // known_as_of in the far future: we learned (and then un-learned) it — the
    // invalidate's ingested time is "now", so a far-future known_as_of sees it as
    // already-retracted; a known_as_of BEFORE the invalidate ingested would see it
    // as still-believed. Both ingestion times are ~now, so assert the retracted side.
    let known_future = AsOf { valid_time: None, known_as_of: Some("2999-01-01T00:00:00Z".into()) };
    assert!(log.as_of(&a, &known_future).unwrap().is_empty(), "by 2999 we had retracted it");
}

// Rev 2 T-B: as_of with BOTH axes set.
#[test]
fn as_of_both_axes_together() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let a = log.append(mk_memory("kenny")).unwrap();
    let b = log.append(mk_memory("acme")).unwrap();
    log.link(&a, "works_at", &b, Some("2020-01-01T00:00:00Z"), &[]).unwrap();
    log.rebuild_graph().unwrap();
    // True in the world in 2021 AND already known by 2999 → 1 row.
    let both = AsOf { valid_time: Some("2021-01-01T00:00:00Z".into()), known_as_of: Some("2999-01-01T00:00:00Z".into()) };
    assert_eq!(log.as_of(&a, &both).unwrap().len(), 1);
    // True in 2021 but NOT yet known in 1990 (ingested ~now) → 0 rows.
    let too_early = AsOf { valid_time: Some("2021-01-01T00:00:00Z".into()), known_as_of: Some("1990-01-01T00:00:00Z".into()) };
    assert!(log.as_of(&a, &too_early).unwrap().is_empty());
}

#[test]
fn append_rejects_empty_source_event_ids_for_tier_b() {
    // This exercises M1's pre-existing append guard (NOT the F2 gate): any Tier-B
    // event — one carrying `model_meta` — must have a non-empty `source_event_ids`,
    // else `append` rejects it. (The F2 producer gate inside `append_graph_event`
    // is proven separately by the unit test in `src/log.rs`, since the public
    // `link`/`invalidate` helpers always pass the manual producer and so can never
    // reach the [src,dst]-default reject arm.)
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let a = log.append(mk_memory("kenny")).unwrap();
    let b = log.append(mk_memory("acme")).unwrap();

    let result = log.append(Event {
        id: String::new(),
        ts: String::new(),
        valid_time: None,
        event_type: "link".to_string(),
        content: json!({ "src": a, "relation": "works_at", "dst": b }),
        model_meta: Some(bossclaw_core::event::ModelMeta {
            model_id: "some-llm".to_string(),
            prompt_hash: String::new(),
            source_event_ids: vec![], // empty — tripped by the append guard
        }),
        prev_hash: String::new(),
        hash: None,
        signed_by_did: DID.to_string(),
        signature: None,
    });
    assert!(
        result.is_err(),
        "append must reject a Tier-B event with empty source_event_ids"
    );
}

// ── Task 6: edges origin + integer-milli confidence (Rev 2 F3) ───────────────

#[test]
fn edges_carry_origin_and_confidence_milli_from_the_producing_link() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let m = log.append(mk_memory("kenny at acme")).unwrap();
    let kenny = log
        .entity("Kenny", &[], "person", "m4-reasoner", std::slice::from_ref(&m))
        .unwrap();
    let acme = log
        .entity("Acme", &[], "org", "m4-reasoner", std::slice::from_ref(&m))
        .unwrap();

    // A MACHINE link WITH confidence (the M4a reasoner producer). 0.83 → 830 milli
    // (Rev 2 F3 — the SIGNED content carries the INTEGER, never a raw f32).
    log.link_machine(&kenny, "works_at", &acme, 0.83, "m4-reasoner", std::slice::from_ref(&m))
        .unwrap();
    // A MANUAL link (no confidence).
    let other = log.append(mk_memory("note")).unwrap();
    log.link(&m, "relates_to", &other, None, std::slice::from_ref(&m)).unwrap();
    log.rebuild_graph().unwrap();

    let edges = log.all_edges().unwrap();
    let machine = edges.iter().find(|e| e.relation == "works_at").unwrap();
    assert_eq!(machine.origin, "machine");
    assert_eq!(
        machine.confidence_milli,
        Some(830),
        "0.83 quantizes to 830 integer-milli in the signed content"
    );
    let manual = edges.iter().find(|e| e.relation == "relates_to").unwrap();
    assert_eq!(manual.origin, "manual");
    assert_eq!(manual.confidence_milli, None, "manual links carry NULL confidence");
}

#[test]
fn edges_origin_confidence_rebuild_is_byte_identical() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let m = log.append(mk_memory("kenny at acme")).unwrap();
    let kenny = log
        .entity("Kenny", &[], "person", "m4-reasoner", std::slice::from_ref(&m))
        .unwrap();
    let acme = log
        .entity("Acme", &[], "org", "m4-reasoner", std::slice::from_ref(&m))
        .unwrap();
    log.link_machine(&kenny, "works_at", &acme, 0.42, "m4-reasoner", std::slice::from_ref(&m))
        .unwrap();
    log.rebuild_graph().unwrap();
    let e1 = log.all_edges().unwrap();
    log.rebuild_graph().unwrap();
    let e2 = log.all_edges().unwrap();
    assert_eq!(
        e1, e2,
        "origin/confidence_milli columns are pure functions of event fields → byte-identical rebuild holds"
    );
}

// ── Rev 2 T-E: machine-link confidence is SIGNED (tamper-evident) ────────────

/// T-E (security #7): the integer `confidence_milli` lives inside the SIGNED
/// `link` content. Directly UPDATEing the stored payload's confidence to a
/// different value must break the hash chain — `verify_chain()` returns `Err`.
/// This freezes the "confidence is signed / tamper-evident" contract.
#[test]
fn machine_link_confidence_milli_is_signed_tamper_breaks_verify_chain() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let path = dir.path().join("m.db");
    {
        let log = EventLog::open(&path, &DEK, key.clone()).unwrap();
        let m = log.append(mk_memory("kenny at acme")).unwrap();
        let kenny = log
            .entity("Kenny", &[], "person", "m4-reasoner", std::slice::from_ref(&m))
            .unwrap();
        let acme = log
            .entity("Acme", &[], "org", "m4-reasoner", std::slice::from_ref(&m))
            .unwrap();
        // confidence 0.9 → confidence_milli 900 in the signed content.
        log.link_machine(&kenny, "works_at", &acme, 0.9, "m4-reasoner", std::slice::from_ref(&m))
            .unwrap();
        log.verify_chain().expect("chain verifies before tamper");
    }
    {
        // Tamper the stored payload: rewrite confidence_milli 900 → 100 directly,
        // bypassing the signed-append path.
        let store = bossclaw_core::store::Store::open(&path, &DEK).unwrap();
        store
            .exec(
                "UPDATE events \
                 SET payload = replace(payload, '\"confidence_milli\":900', '\"confidence_milli\":100') \
                 WHERE event_type='link' AND payload LIKE '%\"confidence_milli\":900%'",
            )
            .unwrap();
    }
    let log = EventLog::open(&path, &DEK, key).unwrap();
    assert!(
        log.verify_chain().is_err(),
        "tampering the signed confidence_milli must break verify_chain (confidence is signed)"
    );
}

// ── Rev 2 T-F (machine path): malicious relation label is inert data, not SQL ─

/// T-F (security #6, machine-link path): a malicious relation label on a MACHINE
/// link round-trips as inert literal data (the M3/Task-2 T-E covered the manual
/// relation + entity-label paths; this covers `link_machine`). Confirms the
/// parameterized INSERT/SELECT treats the label as data even on the new path.
#[test]
fn malicious_machine_relation_label_is_inert_data_not_sql() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let m = log.append(mk_memory("bobby tables")).unwrap();
    let kenny = log
        .entity("Kenny", &[], "person", "m4-reasoner", std::slice::from_ref(&m))
        .unwrap();
    let acme = log
        .entity("Acme", &[], "org", "m4-reasoner", std::slice::from_ref(&m))
        .unwrap();
    let evil_relation = r#"works_at"); DROP TABLE edges; --"#;
    log.link_machine(&kenny, evil_relation, &acme, 0.95, "m4-reasoner", std::slice::from_ref(&m))
        .unwrap();
    log.rebuild_graph().unwrap();

    let edges = log.all_edges().unwrap();
    // The edges table still exists and is queryable — the DROP was inert.
    assert_eq!(edges.len(), 1, "one edge — malicious relation did not alter query semantics");
    assert_eq!(
        edges[0].relation, evil_relation,
        "machine relation label round-trips as inert literal data"
    );
    assert_eq!(edges[0].origin, "machine");
    assert_eq!(edges[0].confidence_milli, Some(950));
}
