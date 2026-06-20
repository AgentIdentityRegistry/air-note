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
use bossclaw_core::recall::RecallOptions;
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

/// Write `text` to `<dir>/g/<name>`, grant the folder, ingest it read-only, and
/// bring every derived structure up to date (mirrors `tests/extraction.rs`'s
/// `ingest_file` — the production grant→ingest→derive lifecycle for a FILE
/// subject). The file row becomes an evolve subject under Door C.
fn ingest_file(
    log: &EventLog,
    embedder: &MockEmbedder,
    dir: &std::path::Path,
    name: &str,
    text: &[u8],
) {
    let folder = dir.join("g");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join(name), text).unwrap();
    log.add_grant(&folder).unwrap();
    log.ingest_all(&bossclaw_core::ingest::ParserRouter::native_only(), embedder)
        .unwrap();
    log.rederive_pending(embedder).unwrap();
    log.rebuild_indexes(embedder).unwrap();
    log.rebuild_graph().unwrap();
    log.rebuild_entity_index(embedder).unwrap();
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

// ── Property 5 (M4b headline): a grounded dossier, surfaced by recall, superseded
//    on contradiction, idempotent with no new facts ─────────────────────────────

/// THE M4b summarize loop, proven LIVE. The summarizer is the half of the evolve
/// loop that turns the M4a graph into a per-entity **dossier** (`page`) kept
/// current via `supersede`, with every claim leashed to a signed source by the
/// deterministic citation floor. This gate asserts the five end-to-end properties
/// against the REAL `qwen2.5:7b-instruct`:
///   1. after extraction + summarize ticks, a `page` exists for the entity;
///   2. EVERY claim in that page cites ONLY ids in the gathered fact-set (grounded
///      — the floor's subtract-only contract holding over a real model's draft);
///   3. `recall("Kenny …")` surfaces the page (`Hit.kind == "page"`);
///   4. a CONTRADICTING memory + re-tick → the page is SUPERSEDED (a fresh current
///      page whose event id differs, the prior dropped from the projection);
///   5. a re-tick with NO new facts emits NO new page (F6 idempotency, LIVE).
///
/// ROBUSTNESS (honest note, mirroring Property 3): a 7b at temperature 0 is
/// *near*-deterministic, but whether extraction mints enough facts to clear
/// `PAGE_MIN_FACTS` and whether the model cites grounded ids is model-output
/// variance. We therefore drive the GROUNDED-DOSSIER phase under bounded retries
/// (fresh stores each attempt) and pass if ANY attempt produces a grounded,
/// recall-surfaced page; the contradiction + idempotency phases then run once on
/// that store. This is NOT weakening the property — the property (the live loop
/// *must be capable of* producing a grounded dossier and superseding it on a
/// contradiction) is unchanged; the retry only absorbs the model's run-to-run
/// variance, exactly as Property 3 and M2's recall gate do. If every attempt
/// fails, the test fails loudly.
#[test]
#[ignore = "requires a local Ollama running qwen2.5:7b-instruct"]
fn live_dossier_is_grounded_surfaces_and_supersedes_on_contradiction() {
    /// Bounded attempts to absorb 7b output variance in the grounding phase.
    /// Worst-case runtime: ~3 × ~50s ≈ 2.5 minutes (extraction + compose per attempt).
    const MAX_ATTEMPTS: usize = 3;

    let embedder = MockEmbedder::new(MID_DIM);
    let reasoner = OllamaReasoner::new(MODEL);

    let mut last_failure = String::new();
    'attempts: for attempt in 1..=MAX_ATTEMPTS {
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());

        // ── Seed a few facts about Kenny so extraction has an entity + edges to
        //    summarize. Each call runs one full tick (extraction THEN summarize).
        //    A second bare tick guarantees the summarize phase sees the fully
        //    folded graph even if the first tick summarized before all edges
        //    folded. ──
        ingest_and_evolve(&log, &embedder, &reasoner, "Kenny Ferris is a software engineer at Acme.");
        ingest_and_evolve(&log, &embedder, &reasoner, "Kenny Ferris leads the Acme search team.");
        log.evolve_once(&embedder, &reasoner).unwrap();

        // Embed any freshly-emitted page so recall can surface it (the summarize
        // phase refreshes the pages projection but not the vector index).
        log.rederive_pending(&embedder).unwrap();
        log.rebuild_indexes(&embedder).unwrap();
        log.rebuild_graph().unwrap();

        // ── Property 1: a page exists for the Kenny entity. ──
        let pages = log.current_pages().unwrap();
        let kenny_entity = log
            .all_entities()
            .unwrap()
            .into_iter()
            .find(|e| e.label.to_lowercase().contains("kenny"));
        let kenny_entity = match kenny_entity {
            Some(e) => e,
            None => {
                last_failure = format!("attempt {attempt}: no Kenny entity minted");
                eprintln!("{last_failure}; retrying");
                continue 'attempts;
            }
        };
        let page = match pages.iter().find(|p| p.topic_id == kenny_entity.entity_id) {
            Some(p) => p.clone(),
            None => {
                last_failure = format!(
                    "attempt {attempt}: no page for entity {} (pages={})",
                    kenny_entity.entity_id,
                    pages.len()
                );
                eprintln!("{last_failure}; retrying");
                continue 'attempts;
            }
        };

        // ── Property 2: every claim's cites ⊆ the fact-set (grounded). Read the
        //    page event's signed claims and check each cite is a real fact id (the
        //    deterministic floor guarantees this; the live assertion proves the
        //    real model's draft actually went THROUGH the floor). ──
        let facts = log.gather_fact_set(&kenny_entity).unwrap();
        let fact_ids = facts.fact_ids();
        assert!(
            !fact_ids.is_empty(),
            "the fact-set is non-empty (the page was grounded in real memories)"
        );
        let page_ev = log
            .stream_all()
            .unwrap()
            .into_iter()
            .find(|e| e.id == page.page_event_id)
            .expect("the current page resolves to a real event");
        let claims = page_ev.content["claims"].as_array().cloned().unwrap_or_default();
        assert!(!claims.is_empty(), "the grounded page has ≥1 surviving claim");
        for claim in &claims {
            let cites = claim["cites"].as_array().cloned().unwrap_or_default();
            assert!(!cites.is_empty(), "every surviving claim cites ≥1 source (floor: no empty cites)");
            for cite in &cites {
                let cid = cite.as_str().expect("a cite is a string event id");
                assert!(
                    fact_ids.contains(cid),
                    "claim cite {cid} is in the fact-set (grounded; the floor dropped any that were not)"
                );
                assert!(
                    !cid.starts_with(ENTITY_NODE_PREFIX),
                    "a cite is an EVENT id, never an entity:<ulid> node id"
                );
            }
        }
        // The page's lineage: non-empty, all event ids (no entity: prefix), the
        // inducing memories are present. We check this by asserting each surviving
        // cite is an event id (done above in the claim loop) and that model_meta is
        // machine-origin and non-empty.
        let page_meta = page_ev.model_meta.clone().expect("page carries model_meta lineage");
        assert!(!page_meta.source_event_ids.is_empty(), "page lineage is non-empty");
        assert_ne!(page_meta.model_id, "manual", "the page is machine-origin");
        for sid in &page_meta.source_event_ids {
            assert!(
                !sid.starts_with(ENTITY_NODE_PREFIX),
                "page lineage id {sid} must be an EVENT id, never an entity:<ulid> node id"
            );
        }

        // ── Property 3: recall surfaces the page with kind == "page". ──
        let hits = log
            .recall(&embedder, "Kenny Ferris Acme engineer", 10, &RecallOptions::default())
            .unwrap();
        let surfaced = hits.iter().any(|h| h.event_id == page.page_event_id && h.kind == "page");
        assert!(
            surfaced,
            "recall surfaces the current dossier as a page hit (hits: {:?})",
            hits.iter().map(|h| (&h.event_id, &h.kind)).collect::<Vec<_>>()
        );

        // ── Property 4: a contradicting memory → the page is SUPERSEDED. ──
        let prior_page_id = page.page_event_id.clone();
        // Snapshot the fact-set BEFORE ingesting the contradiction so we can tell
        // whether the fact-set actually changed (needed to distinguish variance from
        // a deterministic supersede-machinery break below).
        let facts_before_contradiction = log.gather_fact_set(&kenny_entity).unwrap();
        let fact_ids_before = facts_before_contradiction.fact_ids();

        // A memory that changes Kenny's employer; re-tick (extraction updates the
        // graph → the topic is re-dirtied → summarize regenerates the dossier).
        ingest_and_evolve(
            &log,
            &embedder,
            &reasoner,
            "Update: Kenny Ferris has left Acme and now works at Globex.",
        );
        log.evolve_once(&embedder, &reasoner).unwrap();
        log.rederive_pending(&embedder).unwrap();
        log.rebuild_indexes(&embedder).unwrap();
        log.rebuild_graph().unwrap();

        let pages_after = log.current_pages().unwrap();
        let current = pages_after.iter().find(|p| p.topic_id == kenny_entity.entity_id);
        let current = match current {
            Some(p) => p.clone(),
            None => {
                last_failure = format!("attempt {attempt}: topic lost its page after the contradiction");
                eprintln!("{last_failure}; retrying");
                continue 'attempts;
            }
        };
        if current.page_event_id == prior_page_id {
            // Check whether the fact-set actually changed. If it did, the supersede
            // machinery had every reason to fire but didn't — that is a deterministic
            // failure, not model variance.
            let facts_after = log.gather_fact_set(&kenny_entity).unwrap();
            let fact_ids_after = facts_after.fact_ids();
            let fact_set_changed = fact_ids_after != fact_ids_before;
            if fact_set_changed {
                // The contradiction memory was ingested and the fact-set grew (the new
                // memory id is now in fact_ids_after), but the summarize loop did NOT
                // emit a new page. This is NOT model-output variance (the model already
                // ran); it means the supersede path in the summarize loop is broken.
                panic!(
                    "attempt {attempt}: the fact-set changed after the contradiction \
                     (new ids: {:?}) but the page was NOT superseded (still {prior_page_id}). \
                     This is a deterministic supersede-machinery failure, not model variance.",
                    fact_ids_after.difference(&fact_ids_before).collect::<Vec<_>>()
                );
            }
            // The fact-set did not change: the model did not update the graph (it may
            // have labelled the change with a multi-valued relation that carries no
            // retraction, or extraction produced nothing). That IS model variance →
            // retry so a different extraction attempt has a chance to fire.
            last_failure = format!(
                "attempt {attempt}: the contradiction did not supersede the page \
                 (still {prior_page_id}); fact-set unchanged (model variance — retrying)"
            );
            eprintln!("{last_failure}");
            continue 'attempts;
        }
        // A supersede event referencing the prior page exists, and the prior page is
        // no longer current (dropped from the projection).
        let superseded = log.stream_all().unwrap().into_iter().any(|e| {
            e.event_type == "supersede"
                && e.content.get("supersedes").and_then(|v| v.as_str()) == Some(prior_page_id.as_str())
        });
        assert!(superseded, "a supersede event retired the prior page id");
        assert!(
            pages_after.iter().all(|p| p.page_event_id != prior_page_id),
            "the superseded page is no longer current (left the projection)"
        );
        // The new body is still grounded (cites ⊆ the new fact-set).
        let new_facts = log.gather_fact_set(&kenny_entity).unwrap();
        let new_fact_ids = new_facts.fact_ids();
        let new_page_ev = log
            .stream_all()
            .unwrap()
            .into_iter()
            .find(|e| e.id == current.page_event_id)
            .unwrap();
        for claim in new_page_ev.content["claims"].as_array().cloned().unwrap_or_default() {
            for cite in claim["cites"].as_array().cloned().unwrap_or_default() {
                let cid = cite.as_str().unwrap();
                assert!(new_fact_ids.contains(cid), "the regenerated page is still grounded ({cid})");
            }
        }

        // ── Property 5: a re-tick with NO new facts emits NO new page (F6). ──
        let current_page_id = current.page_event_id.clone();
        let report_idem = log.evolve_once(&embedder, &reasoner).unwrap();
        assert_eq!(
            report_idem.pages_emitted, 0,
            "no new facts → the summarize phase emits no new page (F6 idempotency)"
        );
        assert_eq!(report_idem.pages_superseded, 0, "no supersede churn on an unchanged grounding set");
        let pages_final = log.current_pages().unwrap();
        let final_page = pages_final
            .iter()
            .find(|p| p.topic_id == kenny_entity.entity_id)
            .expect("the topic still has exactly one current page");
        assert_eq!(
            final_page.page_event_id, current_page_id,
            "the current page is unchanged after an idempotent re-tick"
        );

        log.verify_chain().unwrap();
        eprintln!("live M4b dossier gate passed on attempt {attempt}/{MAX_ATTEMPTS}");
        return;
    }
    panic!(
        "the live model never produced a grounded, recall-surfaced, supersede-on-\
         contradiction dossier across {MAX_ATTEMPTS} attempts — last: {last_failure}"
    );
}

// ── Property 6 (extraction-from-files headline): a file evolved by the real model
//    yields an EXTERNAL fact ─────────────────────────────────────────────────────

/// THE Door A taint property, proven LIVE. A real small file ("Alice works at
/// Acme.") is granted + ingested read-only, then evolved by the real
/// `qwen2.5:7b-instruct`. The model extracts the stated relationship; the loop
/// emits a `link`. Because that link's engine lineage reaches the file row, the
/// taint chokepoint stamps it EXTERNAL — the live analogue of the hermetic
/// `evolving_a_file_yields_external_facts_and_propagates` gate. We assert the link
/// exists AND is external (the headline) and, defensively, that it is machine-
/// origin (the model cannot forge a manual/trusted fact from file content).
#[test]
#[ignore = "requires a local Ollama running qwen2.5:7b-instruct"]
fn live_extraction_from_file_is_external() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let embedder = MockEmbedder::new(MID_DIM);
    let reasoner = OllamaReasoner::new(MODEL);

    ingest_file(&log, &embedder, dir.path(), "alice.md", b"Alice works at Acme.");
    let report = log.evolve_once(&embedder, &reasoner).unwrap();
    assert!(
        report.links_emitted >= 1,
        "the real model must extract ≥1 link from the file (live report: {report:?})"
    );

    let link = log
        .stream_all()
        .unwrap()
        .into_iter()
        .find(|e| e.event_type == "link")
        .expect("a link event was emitted from the file");
    assert!(
        bossclaw_core::is_external(&link),
        "Door A (LIVE): a fact extracted from a file must be external"
    );
    // The model cannot launder taint into a manual fact (Task 6 property, live).
    let meta = link.model_meta.as_ref().expect("the link carries lineage");
    assert_ne!(meta.model_id, "manual", "a file-derived link is machine-origin, never manual");
    log.verify_chain().unwrap();
}
