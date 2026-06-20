//! Extraction-from-files milestone tests (D2): eager external-taint chokepoint,
//! fail-closed lineage, and (in later tasks) full pipeline integration.
//!
//! Harness preamble is copied from `tests/evolve.rs` so later tasks can reuse
//! helpers without cross-binary imports.

#![allow(dead_code)] // harness helpers are used by later tasks; silence until then

use bossclaw_core::embed::MockEmbedder;
use bossclaw_core::event::Event;
use bossclaw_core::extract::{
    build_pass_a_prompt, build_pass_b_prompt, parse_proposals, verify_floor, PASS_A_SYSTEM,
    PASS_B_SYSTEM,
};
use bossclaw_core::log::EventLog;
use bossclaw_core::reason::ScriptedReasoner;
use bossclaw_core::summarize::{build_compose_prompt, SUMMARIZE_SYSTEM};
use ed25519_dalek::SigningKey;
use serde_json::{json, Value};

// ── Constants (copied from evolve.rs) ────────────────────────────────────────

const DEK: [u8; 32] = [42u8; 32];
const KEY_BYTES: [u8; 32] = [7u8; 32];
const MID_DIM: usize = 64;

// ── Log factory ──────────────────────────────────────────────────────────────

fn open_log(dir: &std::path::Path) -> EventLog {
    let key = SigningKey::from_bytes(&KEY_BYTES);
    EventLog::open(&dir.join("m.db"), &DEK, key).unwrap()
}

// ── Event constructors (copied from evolve.rs) ────────────────────────────────

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
        signed_by_did: "did:wba:AIR-TEST".to_string(),
        signature: None,
    }
}

/// Append a memory and bring every derived structure up to date.
fn seed_memory(log: &EventLog, embedder: &MockEmbedder, text: &str) -> String {
    let id = log.append(mk_memory(text)).unwrap();
    log.rederive_pending(embedder).unwrap();
    log.rebuild_indexes(embedder).unwrap();
    log.rebuild_graph().unwrap();
    log.rebuild_entity_index(embedder).unwrap();
    id
}

// ── Extraction-specific helpers ───────────────────────────────────────────────

/// Write `text` to <dir>/g/<name>, grant it, ingest, rebuild all derived structures.
fn ingest_file(log: &EventLog, emb: &MockEmbedder, dir: &std::path::Path, name: &str, text: &[u8]) {
    let folder = dir.join("g");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join(name), text).unwrap();
    log.add_grant(&folder).unwrap();
    log.ingest_all(&bossclaw_core::ingest::ParserRouter::native_only(), emb).unwrap();
    log.rederive_pending(emb).unwrap();
    log.rebuild_indexes(emb).unwrap();
    log.rebuild_graph().unwrap();
    log.rebuild_entity_index(emb).unwrap();
}

/// The first `file_ingested` event's (id, stored content.text).
fn file_event(log: &EventLog) -> (String, String) {
    let ev = log
        .stream_all()
        .unwrap()
        .into_iter()
        .find(|e| e.event_type == bossclaw_core::graph::FILE_INGESTED_EVENT_TYPE)
        .unwrap();
    let text = ev.content.get("text").and_then(|t| t.as_str()).unwrap().to_string();
    (ev.id, text)
}

fn first_event_of_type(log: &EventLog, ty: &str) -> bossclaw_core::event::Event {
    log.stream_all()
        .unwrap()
        .into_iter()
        .find(|e| e.event_type == ty)
        .unwrap()
}

// ── Shared helpers (reused by Task 3 and later tasks) ────────────────────────

/// Build a `ScriptedReasoner` that answers BOTH passes for a single subject
/// whose recall context is `recalled` (empty on a single-subject store):
/// - Pass A returns `pass_a` for `build_pass_a_prompt(source, recalled)`.
/// - Pass B echoes the floor-verified relations + retractions so the intersect
///   keeps them (the model "agrees").
fn scripted_both_passes(
    model_id: &str,
    source: &str,
    recalled: &[&str],
    neighborhood: &[&str],
    pass_a: Value,
) -> ScriptedReasoner {
    let recalled_owned: Vec<String> = recalled.iter().map(|s| s.to_string()).collect();
    let neighborhood_owned: Vec<String> = neighborhood.iter().map(|s| s.to_string()).collect();
    let a_prompt = build_pass_a_prompt(source, &recalled_owned);
    let proposals = parse_proposals(&pass_a).unwrap();
    let floor = verify_floor(&proposals, source);
    let b_response = json!({
        "entities": [],
        "relations": floor.relations.iter().map(|r| json!({
            "src": r.src, "relation": r.relation, "dst": r.dst,
            "confidence": r.confidence, "supported_by": r.supported_by,
        })).collect::<Vec<_>>(),
        "retractions": floor.retractions.iter().map(|r| json!({
            "src": r.src, "relation": r.relation, "dst": r.dst,
            "reason": r.reason, "confidence": r.confidence,
        })).collect::<Vec<_>>(),
    });
    let b_prompt = build_pass_b_prompt(source, &floor, &neighborhood_owned);
    ScriptedReasoner::new(model_id)
        .with_response(PASS_A_SYSTEM, &a_prompt, pass_a)
        .with_response(PASS_B_SYSTEM, &b_prompt, b_response)
}

// ── Task 1 tests ──────────────────────────────────────────────────────────────

/// A Tier-B event whose source is an external file is stamped external; a Tier-B
/// event with only a clean (memory) source is NOT. Proves the chokepoint.
#[test]
fn tier_b_inherits_external_taint_from_its_sources() {
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(16);
    let log = EventLog::open_with_recall(
        &dir.path().join("m.db"),
        &DEK,
        SigningKey::from_bytes(&KEY_BYTES),
        &emb,
    )
    .unwrap();
    ingest_file(&log, &emb, dir.path(), "f.md", b"secret leaked text");
    let (file_id, _) = file_event(&log);
    let mem_id = log.append(mk_memory("a normal note")).unwrap(); // clean source

    let tainted = log
        .link_machine("entity:a", "knows", "entity:b", 0.9, "scripted", &[file_id])
        .unwrap();
    let clean = log
        .link_machine("entity:c", "knows", "entity:d", 0.9, "scripted", &[mem_id])
        .unwrap();

    assert!(
        bossclaw_core::is_external(&log.event_by_id(&tainted).unwrap().unwrap()),
        "fact derived from a file must be external"
    );
    assert!(
        !bossclaw_core::is_external(&log.event_by_id(&clean).unwrap().unwrap()),
        "fact derived only from a memory must NOT be external"
    );
}

/// Fail-closed (§6.10 / §7): a Tier-B event whose source id cannot be loaded is
/// treated as external (unverifiable lineage is tainted).
#[test]
fn unverifiable_source_is_fail_closed_external() {
    let dir = tempfile::tempdir().unwrap();
    let log = EventLog::open(&dir.path().join("m.db"), &DEK, SigningKey::from_bytes(&KEY_BYTES))
        .unwrap();
    let bogus = "01BOGUSNONEXISTENTSOURCEID00".to_string();
    let ev = log
        .link_machine("entity:a", "rel", "entity:b", 0.9, "scripted", &[bogus])
        .unwrap();
    assert!(
        bossclaw_core::is_external(&log.event_by_id(&ev).unwrap().unwrap()),
        "a Tier-B fact whose source can't be read is fail-closed external"
    );
}

// ── Task 3 tests — Door A end-to-end ─────────────────────────────────────────

/// Pass-A payload for "Alice knows Bob." — two person entities + one knows link.
/// Re-used by all three Task 3 tests; kept as a free fn so the span string can
/// vary per call (the dedup test uses a DIFFERENT memory text for tick 2).
fn knows_pass_a(a: &str, b: &str, span: &str) -> serde_json::Value {
    serde_json::json!({
        "entities": [
            { "mention": a, "entity_type": "person", "confidence": 0.95 },
            { "mention": b, "entity_type": "person", "confidence": 0.95 }],
        "relations": [{ "src": a, "relation": "knows", "dst": b, "confidence": 0.9, "supported_by": span }],
        "retractions": []
    })
}

/// Door A end-to-end: evolving over a file produces a `link` event stamped
/// external; a fact derived from that tainted link is ALSO external (transitive).
#[test]
fn evolving_a_file_yields_external_facts_and_propagates() {
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(64);
    let log = EventLog::open_with_recall(
        &dir.path().join("m.db"),
        &DEK,
        SigningKey::from_bytes(&KEY_BYTES),
        &emb,
    )
    .unwrap();
    ingest_file(&log, &emb, dir.path(), "f.md", b"Alice knows Bob.");
    let (_file_id, source) = file_event(&log); // STORED text, byte-exact
    let reasoner =
        scripted_both_passes("scripted", &source, &[], &[], knows_pass_a("Alice", "Bob", &source));
    log.set_evolve_enabled(true).unwrap();
    log.evolve_once(&emb, &reasoner).unwrap();

    let link = first_event_of_type(&log, "link");
    assert!(
        bossclaw_core::is_external(&link),
        "a link extracted from file text must be external"
    );

    // Transitive: a NEW Tier-B fact sourced from that tainted link is also external.
    let derived = log
        .link_machine("entity:bob", "employer", "entity:acme", 0.9, "scripted", &[link.id])
        .unwrap();
    assert!(
        bossclaw_core::is_external(&log.event_by_id(&derived).unwrap().unwrap()),
        "a fact derived from a tainted fact is transitively external"
    );
}

/// §6.6 no-loop: derived entity/link events are NEVER re-extracted as subjects.
#[test]
fn derived_events_are_not_evolve_subjects() {
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(64);
    let log = EventLog::open_with_recall(
        &dir.path().join("m.db"),
        &DEK,
        SigningKey::from_bytes(&KEY_BYTES),
        &emb,
    )
    .unwrap();
    ingest_file(&log, &emb, dir.path(), "f.md", b"Alice knows Bob.");
    let (_id, source) = file_event(&log);
    let reasoner =
        scripted_both_passes("scripted", &source, &[], &[], knows_pass_a("Alice", "Bob", &source));
    log.set_evolve_enabled(true).unwrap();
    log.evolve_once(&emb, &reasoner).unwrap();
    assert_eq!(
        log.evolve_status().unwrap().queue_depth,
        0,
        "only memory+file are subjects; derived events never re-enter the cursor"
    );
}

/// §6.9 dedup: a second subject re-asserting the SAME edge across ticks does not
/// duplicate it (M4 within-tick active_keys seeded from the current graph).
#[test]
fn re_asserting_an_edge_does_not_duplicate_it() {
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(64);
    let log = EventLog::open_with_recall(
        &dir.path().join("m.db"),
        &DEK,
        SigningKey::from_bytes(&KEY_BYTES),
        &emb,
    )
    .unwrap();
    ingest_file(&log, &emb, dir.path(), "f.md", b"Alice knows Bob.");
    let (_id, source) = file_event(&log);
    log.set_evolve_enabled(true).unwrap();
    log.evolve_once(
        &emb,
        &scripted_both_passes(
            "scripted",
            &source,
            &[],
            &[],
            knows_pass_a("Alice", "Bob", &source),
        ),
    )
    .unwrap();
    let alice = log
        .all_entities()
        .unwrap()
        .into_iter()
        .find(|e| e.label == "Alice")
        .unwrap();
    let n1 = log
        .neighbors(&alice.entity_id)
        .unwrap()
        .iter()
        .filter(|e| e.relation == "knows")
        .count();

    // Tick 2: a clean memory re-asserts the same edge → deduped (no second edge).
    let m2 = "Alice knows Bob, again.";
    let mid = seed_memory(&log, &emb, m2);
    let _ = mid;
    log.evolve_once(
        &emb,
        &scripted_both_passes("scripted", m2, &[], &[], knows_pass_a("Alice", "Bob", m2)),
    )
    .unwrap();
    let n2 = log
        .neighbors(&alice.entity_id)
        .unwrap()
        .iter()
        .filter(|e| e.relation == "knows")
        .count();
    assert_eq!(n1, n2, "re-asserting an edge must not duplicate it (M4 dedup)");
}

// ── Task 5 helpers ────────────────────────────────────────────────────────────

fn empty_pass_a() -> Value {
    json!({ "entities": [], "relations": [], "retractions": [] })
}

/// Seed an entity + machine link citing `lineage`, rebuild, return the topic id.
fn seed_topic_citing(log: &EventLog, src_label: &str, dst_label: &str, lineage: &[String]) -> String {
    let topic = log.entity(src_label, &[], "org", "scripted", lineage).unwrap();
    let dst   = log.entity(dst_label, &[], "thing", "scripted", lineage).unwrap();
    log.link_machine(&topic, "shipped", &dst, 0.9, "scripted", lineage).unwrap();
    log.rebuild_graph().unwrap();
    topic
}

// ── Task 5 tests ──────────────────────────────────────────────────────────────

// Door C + D8: a dossier whose gather lineage cites a file is external, AND the
// file TEXT reaches the (fenced) compose prompt.
#[test]
fn dossier_from_file_includes_text_and_is_external() {
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(64);
    let log = EventLog::open_with_recall(&dir.path().join("m.db"), &DEK, SigningKey::from_bytes(&KEY_BYTES), &emb).unwrap();
    ingest_file(&log, &emb, dir.path(), "f.md", b"Acme shipped widget X.");
    let (file_id, _) = file_event(&log);
    let topic = seed_topic_citing(&log, "Acme", "widgetX", std::slice::from_ref(&file_id));
    let entity = log.all_entities().unwrap().into_iter().find(|e| e.entity_id == topic).unwrap();
    let facts = log.gather_fact_set(&entity).unwrap();
    assert!(facts.memories.iter().any(|(id, _)| id == &file_id), "Door C: file text is in the fact-set");
    let compose = build_compose_prompt(&facts);
    assert!(compose.contains("<<<SOURCE_BEGIN>>>"), "file text is fenced in the compose prompt (§6.5)");

    let reasoner = scripted_both_passes("scripted", "x", &[], &[], empty_pass_a())
        .with_response(SUMMARIZE_SYSTEM, &compose,
            serde_json::json!({ "title": "Acme", "claims": [{ "text": "Acme shipped widget X.", "cites": [file_id] }] }));
    log.set_evolve_enabled(true).unwrap();
    log.evolve_once(&emb, &reasoner).unwrap();

    let page = first_event_of_type(&log, "page");
    assert!(bossclaw_core::is_external(&page), "a dossier synthesized from file content is external");
}

// D8 anti-laundering (§6.4): the composing model cites ONLY a clean memory, but a
// file is in the gather lineage → the page is STILL external (taint anchored to
// the engine lineage, NOT the model's cites). This FAILS before the D8 change.
#[test]
fn dossier_stays_external_even_when_model_cites_around_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(64);
    let log = EventLog::open_with_recall(&dir.path().join("m.db"), &DEK, SigningKey::from_bytes(&KEY_BYTES), &emb).unwrap();
    ingest_file(&log, &emb, dir.path(), "f.md", b"Acme shipped widget X.");
    let (file_id, _) = file_event(&log);
    let clean = seed_memory(&log, &emb, "Acme is a company."); // clean source the model WILL cite
    let topic = seed_topic_citing(&log, "Acme", "widgetX", &[file_id.clone(), clean.clone()]);
    let entity = log.all_entities().unwrap().into_iter().find(|e| e.entity_id == topic).unwrap();
    let facts = log.gather_fact_set(&entity).unwrap();
    let compose = build_compose_prompt(&facts);

    // ADVERSARIAL: cite ONLY the clean memory, never the file.
    let reasoner = scripted_both_passes("scripted", "x", &[], &[], empty_pass_a())
        .with_response(SUMMARIZE_SYSTEM, &compose,
            serde_json::json!({ "title": "Acme", "claims": [{ "text": "Acme is a company.", "cites": [clean] }] }));
    log.set_evolve_enabled(true).unwrap();
    log.evolve_once(&emb, &reasoner).unwrap();

    let page = first_event_of_type(&log, "page");
    assert!(bossclaw_core::is_external(&page),
        "D8: page is external because the gather lineage has the file, even though the model cited only the clean memory");
}
