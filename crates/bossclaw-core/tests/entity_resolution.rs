//! Entity resolution against the live `entities` projection: embed the mention,
//! search existing entity vectors (kind-filtered), apply RESOLVE_HIGH/LOW, route
//! the mid-band to the scripted adjudicator. Hermetic: MockEmbedder + ScriptedReasoner.

use bossclaw_core::embed::MockEmbedder;
use bossclaw_core::event::Event;
use bossclaw_core::extract::ResolveDecision;
use bossclaw_core::log::EventLog;
use bossclaw_core::reason::ScriptedReasoner;
use bossclaw_core::recall::RecallOptions;
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
fn entity_is_reachable_via_entity_index_but_never_via_recall() {
    // T-G (locked constraint #8): entity events are embedded for resolution but
    // recall must EXCLUDE entity-kind — BY CONSTRUCTION, because entity events are
    // not in EMBEDDABLE_EVENT_TYPES (so they never enter the recall `vectors` table
    // or the recall index). The entity's label is a VERBATIM copy of the query, so
    // any leak would surface here. We prove BOTH halves: the entity is reachable
    // via the dedicated entity index, and recall(query) returns the memory but
    // never the entity:<ulid> node id.
    let dir = tempfile::tempdir().unwrap();
    let embedder = MockEmbedder::new(MID_DIM);
    // open_with_recall builds the recall (vector + FTS) index on open; we also
    // build the separate entity index. The memory's text IS the query, so recall
    // has a real memory to return.
    let query = "kenny ferris";
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let log = EventLog::open_with_recall(&dir.path().join("m.db"), &DEK, key, &embedder).unwrap();
    let m = log.append(mk_memory(query)).unwrap();
    // The entity label is a verbatim copy of the query string.
    let kenny = log
        .entity(query, &[], "person", "m4-reasoner", std::slice::from_ref(&m))
        .unwrap();
    log.derive_entity_vector(&embedder, &kenny, query).unwrap();
    // Rebuild BOTH indexes after the appends so the memory is recallable and the
    // entity is resolvable.
    log.rebuild_indexes(&embedder).unwrap();
    log.rebuild_entity_index(&embedder).unwrap();

    // Half 1: the entity IS reachable via the dedicated entity index.
    let entity_hits = log.entity_search(&embedder, query, 5).unwrap();
    assert!(
        entity_hits.iter().any(|(id, _)| id == &kenny),
        "entity index finds the entity node"
    );

    // Half 2: recall returns the memory but NEVER the entity:<ulid> node id.
    let recall_hits = log.recall(&embedder, query, 5, &RecallOptions::default()).unwrap();
    assert!(
        recall_hits.iter().any(|h| h.event_id == m),
        "recall surfaces the matching memory"
    );
    assert!(
        recall_hits.iter().all(|h| h.event_id != kenny),
        "recall must NEVER return the entity node id {kenny} (by-construction exclusion)"
    );
}
