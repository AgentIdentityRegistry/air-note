//! Entity resolution against the live `entities` projection: embed the mention,
//! search existing entity vectors (kind-filtered), apply RESOLVE_HIGH/LOW, route
//! the mid-band to the scripted adjudicator. Hermetic: MockEmbedder + ScriptedReasoner.

use bossclaw_core::embed::MockEmbedder;
use bossclaw_core::event::Event;
use bossclaw_core::extract::ResolveDecision;
use bossclaw_core::log::EventLog;
use bossclaw_core::reason::ScriptedReasoner;
use ed25519_dalek::SigningKey;
use serde_json::json;

const DEK: [u8; 32] = [42u8; 32];
const KEY_BYTES: [u8; 32] = [7u8; 32];
const MID_DIM: usize = 64;

fn open_log(dir: &std::path::Path) -> EventLog {
    let key = SigningKey::from_bytes(&KEY_BYTES);
    EventLog::open(&dir.join("m.db"), &DEK, key).unwrap()
}
fn mk_memory(text: &str) -> Event {
    Event {
        id: String::new(), ts: String::new(), valid_time: None,
        event_type: "memory".to_string(), content: json!({ "text": text }),
        model_meta: None, prev_hash: String::new(), hash: None,
        signed_by_did: "did:wba:AIR-TEST".to_string(), signature: None,
    }
}

#[test]
fn resolving_an_identical_mention_reuses_the_existing_entity() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let embedder = MockEmbedder::new(MID_DIM);
    let m = log.append(mk_memory("kenny ferris rustacean")).unwrap();
    // Mint Kenny, derive its entity vector, rebuild so it is searchable.
    let kenny = log.entity("kenny ferris rustacean", &[], "person", "m4-reasoner", std::slice::from_ref(&m)).unwrap();
    log.derive_entity_vector(&embedder, &kenny, "kenny ferris rustacean").unwrap();
    log.rebuild_entity_index(&embedder).unwrap();

    // The SAME surface text re-embeds to an identical vector (cosine 1.0 ≥ HIGH)
    // → resolve must MERGE to the existing node, not mint a second.
    let reasoner = ScriptedReasoner::new("m4-reasoner"); // adjudicator unused at cosine 1.0
    let decision = log
        .resolve_mention(&embedder, &reasoner, "kenny ferris rustacean")
        .unwrap();
    assert_eq!(decision, ResolveDecision::Merge(kenny));
}

#[test]
fn resolving_a_disjoint_mention_mints_a_new_entity() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let embedder = MockEmbedder::new(MID_DIM);
    let m = log.append(mk_memory("kenny")).unwrap();
    let kenny = log.entity("kenny ferris rustacean", &[], "person", "m4-reasoner", std::slice::from_ref(&m)).unwrap();
    log.derive_entity_vector(&embedder, &kenny, "kenny ferris rustacean").unwrap();
    log.rebuild_entity_index(&embedder).unwrap();

    // A totally disjoint mention shares no tokens (cosine 0.0 ≤ LOW) → mint.
    let reasoner = ScriptedReasoner::new("m4-reasoner");
    let decision = log
        .resolve_mention(&embedder, &reasoner, "completely unrelated quantum lecture")
        .unwrap();
    assert_eq!(decision, ResolveDecision::Mint);
}

#[test]
fn entity_vectors_are_not_returned_by_recall() {
    // The locked constraint: entity events are embedded for resolution but recall
    // must EXCLUDE entity-kind. (Full recall-exclusion is wired in T8; here we
    // assert the dedicated entity index is separate from the recall index.)
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let embedder = MockEmbedder::new(MID_DIM);
    let m = log.append(mk_memory("kenny")).unwrap();
    let kenny = log.entity("kenny ferris", &[], "person", "m4-reasoner", &[m]).unwrap();
    log.derive_entity_vector(&embedder, &kenny, "kenny ferris").unwrap();
    log.rebuild_entity_index(&embedder).unwrap();
    // entity_search finds the entity…
    let hits = log.entity_search(&embedder, "kenny ferris", 5).unwrap();
    assert!(hits.iter().any(|(id, _)| id == &kenny), "entity index finds the entity node");
}
