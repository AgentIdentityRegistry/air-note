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

#[test]
fn non_manual_producer_with_empty_source_ids_is_rejected() {
    // F2 gate: a non-manual producer must supply explicit source_event_ids;
    // the [src, dst] convenience default is ONLY for the manual producer.
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let a = log.append(mk_memory("kenny")).unwrap();
    let b = log.append(mk_memory("acme")).unwrap();

    // The public helpers always pass MANUAL_LINK_PRODUCER so they always succeed.
    // We cannot call append_graph_event directly (it's private), so we verify
    // the gate indirectly: link() with empty sources succeeds (manual path),
    // but a Tier-B append with empty source_event_ids via append() is rejected.
    log.link(&a, "works_at", &b, None, &[]).unwrap(); // empty → defaults to [a, b], OK

    // Direct Tier-B append with empty source_event_ids must fail (this is the
    // same check that F2 relies on for non-manual producers).
    let result = log.append(bossclaw_core::event::Event {
        id: String::new(),
        ts: String::new(),
        valid_time: None,
        event_type: "link".to_string(),
        content: json!({ "src": a, "relation": "works_at", "dst": b }),
        model_meta: Some(bossclaw_core::event::ModelMeta {
            model_id: "some-llm".to_string(),
            prompt_hash: String::new(),
            source_event_ids: vec![], // empty — must be rejected
        }),
        prev_hash: String::new(),
        hash: None,
        signed_by_did: DID.to_string(),
        signature: None,
    });
    assert!(result.is_err(), "non-manual empty source_event_ids must be rejected by append");
}
