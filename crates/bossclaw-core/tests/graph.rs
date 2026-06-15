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

// ── Self-loop test (rider from T2 code review) ────────────────────────────────

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
