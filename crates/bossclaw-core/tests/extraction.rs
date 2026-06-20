//! Extraction-from-files milestone tests (D2): eager external-taint chokepoint,
//! fail-closed lineage, and (in later tasks) full pipeline integration.
//!
//! Harness preamble is copied from `tests/evolve.rs` so later tasks can reuse
//! helpers without cross-binary imports.

#![allow(dead_code)] // harness helpers are used by later tasks; silence until then

use bossclaw_core::embed::MockEmbedder;
use bossclaw_core::event::Event;
use bossclaw_core::log::EventLog;
use ed25519_dalek::SigningKey;
use serde_json::json;

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

/// Write `text` to <dir>/g/<name>, grant it, ingest, rebuild.
fn ingest_file(log: &EventLog, emb: &MockEmbedder, dir: &std::path::Path, name: &str, text: &[u8]) {
    let folder = dir.join("g");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join(name), text).unwrap();
    log.add_grant(&folder).unwrap();
    log.ingest_all(&bossclaw_core::ingest::ParserRouter::native_only(), emb).unwrap();
    log.rebuild_indexes(emb).unwrap();
    log.rebuild_graph().unwrap();
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
