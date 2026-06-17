//! Live behavioral gate for the M4a clever linker (spec §2.2, §12). Every test is
//! `#[ignore]`: it requires a running local Ollama with `qwen2.5:7b-instruct` and
//! is NOT part of the hermetic CI suite. It asserts behavioral PROPERTIES, never
//! byte-identity — a live LLM is non-deterministic, so the gate proves *what the
//! loop does with a real model's output* (counts/existence), not the exact bytes.
//! This is the M4a analogue of M2's `recall@3` real-model gate.
//!
//! Run locally (Ollama up, model pulled):
//!   `cargo test -p bossclaw-core --features ollama --test live_ollama -- --ignored --nocapture`
//!
//! A 7b doing Pass A + Pass B + entity adjudications over a few memories can take
//! tens of seconds to a couple of minutes; the `OllamaReasoner` request timeout
//! (120s/call) bounds a wedged server.
//!
//! # Why `MockEmbedder` and not a real embedder
//!
//! The gate proves the **reasoner** (the F2/F4 extraction → resolution →
//! contradiction-retirement loop driven by the live model), NOT the embedder.
//! `MockEmbedder`'s deterministic bag-of-words is sufficient for entity
//! resolution and recall here (shared surface tokens → cosine ~1.0 ≥
//! `RESOLVE_HIGH`, so the same mention across two ticks resolves to the same
//! `entity:<ulid>`), and it keeps the embed side hermetic so any failure is
//! unambiguously attributable to the real model, not embedding noise.
//!
//! # Model provenance (Task 1 F8b)
//!
//! These tests construct the reasoner with the bare tag `qwen2.5:7b-instruct`. In
//! production, pin the digest so the provenance record in each emitted event names
//! the exact blob that produced it, e.g. (captured 2026-06-17 via `ollama show` /
//! `/api/tags`):
//!   `qwen2.5:7b-instruct@sha256:845dbda0ea48ed749caafd9e6037047aa19acfcfd82e704d7ca97d631a0b697e`
//! (7.6B params, Q4_K_M). A 7b→14b upgrade is non-destructive: old events keep the
//! old tag, new events carry the new one.
#![cfg(feature = "ollama")]

use bossclaw_core::embed::MockEmbedder;
use bossclaw_core::event::Event;
use bossclaw_core::graph::ENTITY_NODE_PREFIX;
use bossclaw_core::log::EventLog;
use bossclaw_core::ollama::OllamaReasoner;
use ed25519_dalek::SigningKey;
use serde_json::json;

/// Data-encryption key for the temp store (fixed bytes — hermetic, throwaway).
const DEK: [u8; 32] = [42u8; 32];
/// Signing key bytes for the temp store (fixed — throwaway, never leaves the test).
const KEY_BYTES: [u8; 32] = [7u8; 32];
/// `MockEmbedder` dimensionality (matches the hermetic suite's `MID_DIM`).
const MID_DIM: usize = 64;
/// The live model tag. Production should digest-pin (see the module provenance
/// note); the bare tag is fine for a local dogfood gate.
const MODEL: &str = "qwen2.5:7b-instruct";

/// Open a fresh encrypted [`EventLog`] in `dir` (mirrors `tests/evolve.rs`).
fn open_log(dir: &std::path::Path) -> EventLog {
    let key = SigningKey::from_bytes(&KEY_BYTES);
    EventLog::open(&dir.join("m.db"), &DEK, key).unwrap()
}

/// Build an unsigned `memory` event carrying `text` (the `append` writer signs +
/// chains it). Identical shape to the hermetic suite's `mk_memory`.
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

/// Append `text`, bring every derived structure up to date (the production
/// open→derive→rebuild lifecycle), then run ONE evolve tick against the real
/// model. Mirrors `tests/evolve.rs::seed_memory` + `evolve_once`, but with a live
/// [`OllamaReasoner`] instead of a `ScriptedReasoner`. Returns the tick's report.
fn ingest_and_evolve(
    log: &EventLog,
    embedder: &MockEmbedder,
    reasoner: &OllamaReasoner,
    text: &str,
) -> bossclaw_core::EvolveReport {
    log.append(mk_memory(text)).unwrap();
    log.rederive_pending(embedder).unwrap();
    log.rebuild_indexes(embedder).unwrap();
    log.rebuild_graph().unwrap();
    log.rebuild_entity_index(embedder).unwrap();
    log.evolve_once(embedder, reasoner).unwrap()
}

// ── Property 1: a memory naming a person mints ≥1 entity ───────────────────────

/// A memory like "Kenny Ferris is a software engineer at Acme." → the live model
/// extracts the person (and the org), and the loop mints ≥1 `entity` event. We
/// assert BOTH the report count and a folded entity row (the projection holds).
#[test]
#[ignore = "requires a local Ollama running qwen2.5:7b-instruct"]
fn live_person_memory_mints_at_least_one_entity() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let embedder = MockEmbedder::new(MID_DIM);
    let reasoner = OllamaReasoner::new(MODEL);

    let report = ingest_and_evolve(
        &log,
        &embedder,
        &reasoner,
        "Kenny Ferris is a software engineer at Acme.",
    );

    assert!(
        report.entities_minted >= 1,
        "a memory naming a person must mint ≥1 entity (live report: {report:?})"
    );
    log.rebuild_graph().unwrap();
    let entities = log.all_entities().unwrap();
    assert!(
        !entities.is_empty(),
        "≥1 entity row exists after the tick (got {})",
        entities.len()
    );
    // Provenance smoke-check: every minted entity is a stable `entity:<ulid>` node.
    assert!(
        entities.iter().all(|e| e.entity_id.starts_with(ENTITY_NODE_PREFIX)),
        "minted entities carry the namespaced entity:<ulid> id"
    );
    log.verify_chain().unwrap();
}

// ── Property 2: a stated relationship yields a machine link with confidence ────

/// The same person/org memory yields ≥1 `link` edge of `origin = "machine"` whose
/// `confidence_milli` is set. The confidence being present is the observable proof
/// that the relation carried a `supported_by` span through Pass A's parse (a
/// span-less relation is dropped before it can become a link), survived the pure
/// floor + Pass B critique, and was emitted with the model's confidence.
#[test]
#[ignore = "requires a local Ollama running qwen2.5:7b-instruct"]
fn live_stated_relationship_yields_a_machine_link_with_confidence() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let embedder = MockEmbedder::new(MID_DIM);
    let reasoner = OllamaReasoner::new(MODEL);

    let report = ingest_and_evolve(
        &log,
        &embedder,
        &reasoner,
        "Kenny Ferris is a software engineer at Acme.",
    );
    assert!(
        report.links_emitted >= 1,
        "a stated relationship must yield ≥1 machine link (live report: {report:?})"
    );

    log.rebuild_graph().unwrap();
    let edges = log.all_edges().unwrap();
    assert!(
        edges.iter().any(|e| e.origin == "machine"),
        "≥1 machine-origin edge exists (edges: {})",
        edges.len()
    );
    // The supported_by span drove extraction ⇒ the emitted link carried the
    // model's confidence ⇒ the fold records confidence_milli (never NULL, which
    // is reserved for manual links). This is the span-provenance property.
    assert!(
        edges
            .iter()
            .any(|e| e.origin == "machine" && e.confidence_milli.is_some()),
        "a machine link carries confidence_milli (the supported_by span drove it)"
    );
    // F2/§16 lineage: the machine link's source_event_ids reach a real events row,
    // never an entity:<ulid> node id (no taint-laundering).
    let all = log.stream_all().unwrap();
    let real_ids: std::collections::HashSet<String> =
        all.iter().map(|e| e.id.clone()).collect();
    let link_ev = all
        .iter()
        .find(|e| e.event_type == "link")
        .expect("a link event was emitted");
    let meta = link_ev.model_meta.as_ref().expect("link carries lineage");
    assert!(!meta.source_event_ids.is_empty(), "lineage is non-empty");
    for sid in &meta.source_event_ids {
        assert!(
            !sid.starts_with(ENTITY_NODE_PREFIX),
            "lineage id {sid} is an EVENT id, never an entity node id"
        );
        assert!(real_ids.contains(sid), "lineage id {sid} resolves to a real row");
    }
    log.verify_chain().unwrap();
}

// ── Property 3 (the headline): a contradiction across two ticks → an invalidate ─

/// THE F4 contradiction-retirement, proven LIVE. Tick 1 establishes Kenny's
/// PRIMARY employer (single-valued `works_at_primary` → Globex). Tick 2 is a
/// memory stating he changed primary employers. For the invalidate to fire, the
/// loop logic must:
///   1. surface tick-1's memory via recall (MockEmbedder shares the "Kenny Ferris
///      … primary job … at" tokens, so it does),
///   2. have the live model propose a RETRACTION of `Kenny works_at_primary
///      Globex` (qwen does — single-valued relation semantics are in the prompt),
///   3. resolve tick-2's "Kenny Ferris"/"Globex" mentions to tick-1's entity ids,
///   4. `confirm_retractions` against the active edge keys → emit the invalidate.
///
/// We assert an `invalidate` event exists AND the Globex primary edge is retired.
///
/// ROBUSTNESS (honest note): a 7b at temperature 0 is *near*-deterministic but the
/// extraction is the hard part — occasionally it may label the change `works_at`
/// (multi-valued, no retraction) or omit the retraction. We therefore retry the
/// CONTRADICTING tick a bounded number of times (fresh stores each attempt) and
/// pass if ANY attempt fires the invalidate. This is NOT weakening the property —
/// the property (a contradiction *must be capable of* retiring the prior fact via
/// the live loop) is unchanged; the retry only absorbs the model's output variance
/// across runs, exactly as M2's recall gate tolerates ANN nondeterminism. If every
/// attempt fails, the test fails loudly (no tautology, no fake pass).
#[test]
#[ignore = "requires a local Ollama running qwen2.5:7b-instruct"]
fn live_contradiction_across_two_memories_fires_an_invalidate() {
    /// Bounded attempts to absorb 7b output variance (see the robustness note).
    const MAX_ATTEMPTS: usize = 3;

    let embedder = MockEmbedder::new(MID_DIM);
    let reasoner = OllamaReasoner::new(MODEL);

    let mut last_failure = String::new();
    for attempt in 1..=MAX_ATTEMPTS {
        // Fresh store per attempt so a partial earlier attempt cannot leak state.
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());

        // ── Tick 1: establish the PRIMARY (single-valued) employer = Globex. ──
        let r1 = ingest_and_evolve(
            &log,
            &embedder,
            &reasoner,
            "Kenny Ferris's primary job is at Globex.",
        );

        // ── Tick 2: he changed primary employers → contradicts the prior fact. ──
        let r2 = ingest_and_evolve(
            &log,
            &embedder,
            &reasoner,
            "Kenny Ferris no longer works at Globex — his primary job is now at Acme.",
        );
        log.rebuild_graph().unwrap();

        let invalidated = log
            .stream_all()
            .unwrap()
            .into_iter()
            .any(|e| e.event_type == "invalidate");

        // The folded graph confirms a primary edge to Globex was retired (its
        // learned-clock end is set). This is the visible consequence of the F4
        // retirement — stronger than just "an invalidate event exists".
        let globex_primary_retired = log.all_edges().unwrap().into_iter().any(|e| {
            e.relation == "works_at_primary" && e.invalidated_at.is_some()
        });

        if invalidated && globex_primary_retired {
            log.verify_chain().unwrap();
            // Never-forget: the originating memories are still byte-recallable even
            // though the edge is retired (carried M3 invariant, holding LIVE).
            let texts: Vec<String> = log
                .stream_all()
                .unwrap()
                .into_iter()
                .filter(|e| e.event_type == "memory")
                .filter_map(|e| {
                    e.content.get("text").and_then(|t| t.as_str()).map(String::from)
                })
                .collect();
            assert_eq!(texts.len(), 2, "both source memories are retained after retirement");
            eprintln!(
                "live contradiction fired on attempt {attempt}/{MAX_ATTEMPTS} \
                 (r1={r1:?}, r2={r2:?})"
            );
            return;
        }
        last_failure = format!(
            "attempt {attempt}/{MAX_ATTEMPTS}: invalidate_event={invalidated}, \
             globex_primary_retired={globex_primary_retired} (r1={r1:?}, r2={r2:?})"
        );
        eprintln!("live contradiction NOT fired — {last_failure}; retrying");
    }
    panic!(
        "the live model never fired the F4 contradiction-retirement across \
         {MAX_ATTEMPTS} attempts — last: {last_failure}"
    );
}

// ── Property 4: re-running a tick after a memory is idempotent ─────────────────

/// After a memory is fully processed, re-running `evolve_once` with no new
/// memories emits nothing: the cursor is past the only memory, so the report shows
/// zero new work and the event count is unchanged. This proves the loop is a
/// no-op on already-curated state (Rev 2 F5 cross-tick idempotency, LIVE).
#[test]
#[ignore = "requires a local Ollama running qwen2.5:7b-instruct"]
fn live_re_running_a_tick_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let embedder = MockEmbedder::new(MID_DIM);
    let reasoner = OllamaReasoner::new(MODEL);

    ingest_and_evolve(
        &log,
        &embedder,
        &reasoner,
        "Kenny Ferris is a software engineer at Acme.",
    );
    let count_after_first = log.count().unwrap();

    // A second tick with NO new memories: the cursor is past the only memory, so
    // there is nothing to process and nothing to emit.
    let report = log.evolve_once(&embedder, &reasoner).unwrap();
    assert_eq!(report.memories_processed, 0, "no unprocessed memories on re-run");
    assert_eq!(report.entities_minted, 0, "re-run mints no entity");
    assert_eq!(report.links_emitted, 0, "re-run emits no link");
    assert_eq!(report.invalidates_emitted, 0, "re-run emits no invalidate");
    assert_eq!(
        log.count().unwrap(),
        count_after_first,
        "re-running adds no events (idempotent)"
    );
    log.verify_chain().unwrap();
}
