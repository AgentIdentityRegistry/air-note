use bossclaw_core::embed::{Embedder, MockEmbedder};
use bossclaw_core::index::{HnswIndex, VectorIndex};
use bossclaw_core::model2vec::Model2Vec;
use bossclaw_core::event::Event;
use bossclaw_core::log::EventLog;
use bossclaw_core::{BossclawError, SCHEMA_VERSION};
use ed25519_dalek::SigningKey;
use serde_json::json;

const DEK: [u8; 32] = [42u8; 32];
const KEY_BYTES: [u8; 32] = [7u8; 32];

/// Small dimension used for batch and basic-shape tests.
const SMALL_DIM: usize = 16;
/// Mid-range dimension used for norm and distinctness tests.
const MID_DIM: usize = 64;

fn mk_config_event(model_id: &str, dim: u32, schema_version: u32) -> Event {
    Event {
        id: String::new(),
        ts: String::new(),
        valid_time: None,
        event_type: "config".to_string(),
        content: json!({
            "active_model_id": model_id,
            "dim": dim,
            "schema_version": schema_version
        }),
        model_meta: None,
        prev_hash: String::new(),
        hash: None,
        signed_by_did: "did:wba:AIR-TEST".to_string(),
        signature: None,
    }
}

// --- active_model() tests ---

#[test]
fn active_model_returns_none_on_empty_log() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let log = EventLog::open(&dir.path().join("m.db"), &DEK, key).unwrap();
    assert!(log.active_model().unwrap().is_none());
}

#[test]
fn active_model_returns_latest_config_event() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let log = EventLog::open(&dir.path().join("m.db"), &DEK, key).unwrap();

    // Append two config events: first "a", then "b".
    log.append(mk_config_event("a", 128, 1)).unwrap();
    log.append(mk_config_event("b", 256, 1)).unwrap();

    let model = log.active_model().unwrap().expect("should have active model");
    assert_eq!(model.active_model_id, "b");
    assert_eq!(model.dim, 256);
    assert_eq!(model.schema_version, 1);
}

#[test]
fn active_model_ignores_non_config_events() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let log = EventLog::open(&dir.path().join("m.db"), &DEK, key).unwrap();

    // Append a config, then a non-config memory event.
    log.append(mk_config_event("a", MID_DIM as u32, 1)).unwrap();
    log.append(Event {
        id: String::new(),
        ts: String::new(),
        valid_time: None,
        event_type: "memory".to_string(),
        content: json!({ "text": "hello" }),
        model_meta: None,
        prev_hash: String::new(),
        hash: None,
        signed_by_did: "did:wba:AIR-TEST".to_string(),
        signature: None,
    })
    .unwrap();

    // active_model() must still return "a" (the latest config), not None.
    let model = log
        .active_model()
        .unwrap()
        .expect("should still have active model");
    assert_eq!(model.active_model_id, "a");
    assert_eq!(model.dim, MID_DIM as u32);
}

// --- MockEmbedder tests ---

#[test]
fn mock_embedder_returns_vectors_of_correct_dim() {
    let embedder = MockEmbedder::new(MID_DIM);
    let vecs = embedder.embed(&["hello world".to_string()]).unwrap();
    assert_eq!(vecs.len(), 1);
    assert_eq!(vecs[0].len(), MID_DIM);
}

#[test]
fn mock_embedder_dim_matches_dim_method() {
    let embedder = MockEmbedder::new(128);
    assert_eq!(embedder.dim(), 128);
    let vecs = embedder.embed(&["test".to_string()]).unwrap();
    assert_eq!(vecs[0].len(), embedder.dim());
}

#[test]
fn mock_embedder_is_deterministic() {
    let embedder = MockEmbedder::new(32);
    let text = "the quick brown fox".to_string();
    let v1 = embedder.embed(std::slice::from_ref(&text)).unwrap();
    let v2 = embedder.embed(std::slice::from_ref(&text)).unwrap();
    assert_eq!(v1, v2, "same input must produce same output");
}

#[test]
fn mock_embedder_output_is_unit_norm_for_nonempty_text() {
    let embedder = MockEmbedder::new(MID_DIM);
    let vecs = embedder
        .embed(&["hello world foo bar".to_string()])
        .unwrap();
    let v = &vecs[0];
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    assert!(
        (norm - 1.0).abs() < 1e-5,
        "L2 norm must be ≈1.0, got {norm}"
    );
}

#[test]
fn mock_embedder_model_id_is_stable() {
    let e1 = MockEmbedder::new(32);
    let e2 = MockEmbedder::new(MID_DIM);
    // model_id is "mock-v1" regardless of dim
    assert_eq!(e1.model_id(), "mock-v1");
    assert_eq!(e2.model_id(), "mock-v1");
}

#[test]
fn mock_embedder_batch_embed() {
    let embedder = MockEmbedder::new(SMALL_DIM);
    let texts = vec!["foo".to_string(), "bar".to_string(), "baz".to_string()];
    let vecs = embedder.embed(&texts).unwrap();
    assert_eq!(vecs.len(), 3);
    for v in &vecs {
        assert_eq!(v.len(), SMALL_DIM);
    }
}

#[test]
fn mock_embedder_different_texts_produce_different_vectors() {
    let embedder = MockEmbedder::new(MID_DIM);
    let v1 = embedder.embed(&["alpha beta".to_string()]).unwrap();
    let v2 = embedder.embed(&["gamma delta".to_string()]).unwrap();
    // Highly unlikely to collide with distinct tokens under FNV-1a + 64 buckets.
    assert_ne!(v1[0], v2[0], "distinct input should produce distinct output");
}

#[test]
fn mock_embedder_dim_zero_returns_invalid_input_error() {
    let embedder = MockEmbedder::new(0);
    let result = embedder.embed(&["x".to_string()]);
    assert!(
        matches!(result, Err(BossclawError::InvalidInput(_))),
        "dim=0 must return InvalidInput, got {result:?}"
    );
}

#[test]
fn mock_embedder_empty_text_returns_zero_vector() {
    let embedder = MockEmbedder::new(MID_DIM);
    let vecs = embedder.embed(&[String::new()]).unwrap();
    assert_eq!(vecs.len(), 1);
    assert_eq!(vecs[0].len(), MID_DIM);
    assert!(
        vecs[0].iter().all(|&x| x == 0.0),
        "empty text must produce an all-zero vector"
    );
}

// --- Tier-A vectors: derive / backfill / filtered read (Task 4) ---

/// Model id produced by `MockEmbedder`, asserted directly so the active-model
/// filter test reads naturally.
const MOCK_MODEL_ID: &str = "mock-v1";

fn mk_memory_event(text: &str) -> Event {
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

/// Test-only embedder that always fails, to exercise the best-effort skip path.
struct FailingEmbedder;

impl Embedder for FailingEmbedder {
    fn embed(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>, BossclawError> {
        Err(BossclawError::Embed("intentional test failure".to_string()))
    }
    fn dim(&self) -> usize {
        MID_DIM
    }
    fn model_id(&self) -> &str {
        MOCK_MODEL_ID
    }
}

#[test]
fn rederive_pending_derives_all_memory_events_in_event_id_order() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let log = EventLog::open(&dir.path().join("m.db"), &DEK, key).unwrap();

    for t in ["alpha one", "beta two", "gamma three"] {
        log.append(mk_memory_event(t)).unwrap();
    }

    let embedder = MockEmbedder::new(MID_DIM);
    assert_eq!(log.rederive_pending(&embedder).unwrap(), 3);

    let vectors = log.vectors_for_model(MOCK_MODEL_ID).unwrap();
    assert_eq!(vectors.len(), 3, "one vector per memory event");

    // Mandatory: rows must come back in event_id ASC order (T5 depends on it).
    let mut sorted = vectors.clone();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let ids: Vec<&String> = vectors.iter().map(|(id, _)| id).collect();
    let sorted_ids: Vec<&String> = sorted.iter().map(|(id, _)| id).collect();
    assert_eq!(ids, sorted_ids, "vectors_for_model must be event_id ASC");

    for (_, v) in &vectors {
        assert_eq!(v.len(), MID_DIM, "decoded vector length == embedder dim");
    }
}

#[test]
fn rederive_pending_is_idempotent_no_duplicate_vectors() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let log = EventLog::open(&dir.path().join("m.db"), &DEK, key).unwrap();

    for t in ["one", "two"] {
        log.append(mk_memory_event(t)).unwrap();
    }
    let embedder = MockEmbedder::new(MID_DIM);
    assert_eq!(log.rederive_pending(&embedder).unwrap(), 2);
    // Second pass: everything already has a vector → nothing pending.
    assert_eq!(log.rederive_pending(&embedder).unwrap(), 0);
    assert_eq!(log.vectors_for_model(MOCK_MODEL_ID).unwrap().len(), 2);
}

#[test]
fn vectors_for_model_filters_by_model_id_no_cross_model_bleed() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let log = EventLog::open(&dir.path().join("m.db"), &DEK, key).unwrap();

    log.append(mk_memory_event("hello")).unwrap();
    let embedder = MockEmbedder::new(MID_DIM);
    log.rederive_pending(&embedder).unwrap();

    // Same store, different model id → must see nothing (C4 active-model filter).
    assert!(
        log.vectors_for_model("other-model").unwrap().is_empty(),
        "no cross-model bleed"
    );
}

#[test]
fn rederive_pending_skips_failing_embeds_best_effort() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let log = EventLog::open(&dir.path().join("m.db"), &DEK, key).unwrap();

    for t in ["a", "b", "c"] {
        log.append(mk_memory_event(t)).unwrap();
    }

    // Failing embedder: best-effort skip → Ok(0), no panic, no rows.
    let failing = FailingEmbedder;
    assert_eq!(log.rederive_pending(&failing).unwrap(), 0);
    assert!(log.vectors_for_model(MOCK_MODEL_ID).unwrap().is_empty());

    // A working embedder for the same model id backfills everything.
    let working = MockEmbedder::new(MID_DIM);
    assert_eq!(log.rederive_pending(&working).unwrap(), 3);
    assert_eq!(log.vectors_for_model(MOCK_MODEL_ID).unwrap().len(), 3);
}

#[test]
fn rederive_pending_skips_non_embeddable_events() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let log = EventLog::open(&dir.path().join("m.db"), &DEK, key).unwrap();

    log.append(mk_memory_event("real text")).unwrap();
    // A config event is not embeddable and must not count toward the backfill.
    log.append(mk_config_event("cfg", MID_DIM as u32, 1)).unwrap();

    let embedder = MockEmbedder::new(MID_DIM);
    assert_eq!(
        log.rederive_pending(&embedder).unwrap(),
        1,
        "only the memory event is embeddable; config is skipped"
    );
    assert_eq!(log.vectors_for_model(MOCK_MODEL_ID).unwrap().len(), 1);
}

#[test]
fn derive_vector_returns_false_for_non_embeddable_event() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let log = EventLog::open(&dir.path().join("m.db"), &DEK, key).unwrap();

    let embedder = MockEmbedder::new(MID_DIM);
    let mut config = mk_config_event("cfg", MID_DIM as u32, 1);
    config.id = "01J0000000000000000000000C".to_string();
    assert!(
        !log.derive_vector(&embedder, &config).unwrap(),
        "config events are not embeddable → Ok(false)"
    );
    assert!(log.vectors_for_model(MOCK_MODEL_ID).unwrap().is_empty());
}

#[test]
fn derive_vector_upsert_is_idempotent_and_blob_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let log = EventLog::open(&dir.path().join("m.db"), &DEK, key).unwrap();

    let id = log.append(mk_memory_event("round trip me")).unwrap();
    let stored = log.stream_all().unwrap();
    let event = stored
        .into_iter()
        .find(|e| e.id == id)
        .expect("appended event present");

    let embedder = MockEmbedder::new(MID_DIM);
    assert!(log.derive_vector(&embedder, &event).unwrap());
    // Calling twice must replace this event's chunk set atomically (DELETE +
    // INSERT-all), not duplicate rows.
    assert!(log.derive_vector(&embedder, &event).unwrap());

    let vectors = log.vectors_for_model(MOCK_MODEL_ID).unwrap();
    assert_eq!(vectors.len(), 1, "short text is one chunk; re-derive replaces, not duplicates");
    // vectors_for_model now returns composite chunk keys; a short event is a
    // single chunk 0, so the key decodes to (id, 0).
    assert_eq!(
        bossclaw_core::index::decode_chunk_key(&vectors[0].0),
        Some((id.as_str(), 0)),
        "single-chunk key must decode to (event_id, 0)"
    );

    // The decoded vector must equal a fresh embed of the same text (blob round-trip).
    let expected = embedder
        .embed(&["round trip me".to_string()])
        .unwrap()
        .remove(0);
    assert_eq!(vectors[0].1, expected, "blob decode must match the embedding");
}

/// Test-only embedder that fails on one specific input text and succeeds on all
/// others. Used to verify that `rederive_pending` correctly counts partial
/// successes (the all-fail case is already covered by `FailingEmbedder`).
struct MixedEmbedder {
    fail_on: String,
    inner: MockEmbedder,
}

impl MixedEmbedder {
    fn new(fail_on: impl Into<String>) -> Self {
        Self { fail_on: fail_on.into(), inner: MockEmbedder::new(MID_DIM) }
    }
}

impl Embedder for MixedEmbedder {
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, BossclawError> {
        // Only called as a one-item batch from embed_one; fail if that item
        // matches the trigger text.
        if texts.len() == 1 && texts[0] == self.fail_on {
            return Err(BossclawError::Embed(format!(
                "MixedEmbedder: intentional failure for {:?}",
                self.fail_on
            )));
        }
        self.inner.embed(texts)
    }
    fn dim(&self) -> usize {
        MID_DIM
    }
    fn model_id(&self) -> &str {
        MOCK_MODEL_ID
    }
}

#[test]
fn rederive_pending_partial_failure_skips_one_derives_rest() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let log = EventLog::open(&dir.path().join("m.db"), &DEK, key).unwrap();

    for t in ["good one", "bad seed", "good two"] {
        log.append(mk_memory_event(t)).unwrap();
    }

    // "bad seed" will fail; the other two must succeed.
    let embedder = MixedEmbedder::new("bad seed");
    let derived = log.rederive_pending(&embedder).unwrap();
    assert_eq!(derived, 2, "two successful, one skipped");

    let vectors = log.vectors_for_model(MOCK_MODEL_ID).unwrap();
    assert_eq!(vectors.len(), 2, "only the two good events have vectors");

    // The failed event must still show up as pending on a subsequent pass
    // with a working embedder, so the retry hook can recover it.
    let working = MockEmbedder::new(MID_DIM);
    assert_eq!(log.rederive_pending(&working).unwrap(), 1, "bad seed recovered on retry");
    assert_eq!(log.vectors_for_model(MOCK_MODEL_ID).unwrap().len(), 3);
}

// ---------------------------------------------------------------------------
// Task 5: pure VectorIndex / HnswIndex + rebuild-on-open + vector_search
// ---------------------------------------------------------------------------

/// `k` used by the index recall tests. Small, but > 1 so top-k SET assertions
/// are meaningful.
const RECALL_K: usize = 3;
/// Number of memory events seeded for the rebuild-reproducibility test. Larger
/// than `RECALL_K` so a top-k slice is a strict subset of the corpus.
const REBUILD_CORPUS: usize = 8;
/// Cosine distance is in `[0, 2]`; an exact (or near-exact) match sits at ≈0.
/// The MockEmbedder is L2-normalised, so a query equal to an inserted vector
/// must come back well under this bound.
const EXACT_MATCH_MAX_DISTANCE: f32 = 1e-3;

/// (a) Direct `HnswIndex` add/search: querying with a vector equal to one that
/// was inserted returns that id as top-1 at ≈0 distance.
#[test]
fn hnsw_index_add_then_search_returns_inserted_vector_as_top1() {
    let embedder = MockEmbedder::new(MID_DIM);
    let phrases = ["alpha apple", "beta banana", "gamma grape", "delta date"];
    let vecs = embedder
        .embed(&phrases.iter().map(|s| s.to_string()).collect::<Vec<_>>())
        .unwrap();

    let mut index = HnswIndex::with_capacity(phrases.len());
    for (i, v) in vecs.iter().enumerate() {
        index.add(&format!("id-{i}"), v);
    }

    // Query with a vector identical to the one inserted under "id-2".
    let hits = index.search(&vecs[2], RECALL_K);
    assert!(!hits.is_empty(), "search must return at least one hit");
    assert_eq!(hits[0].0, "id-2", "self-query must rank its own id first");
    assert!(
        hits[0].1 <= EXACT_MATCH_MAX_DISTANCE,
        "self-query distance must be ≈0, got {}",
        hits[0].1
    );
    assert_eq!(index.last_indexed().as_deref(), Some("id-3"));
}

/// Re-add / dedup invariant: the rebuild path calls `add` for every persisted
/// vector, which may include the same `event_id` twice on a stray double-call.
/// The contract is: first vector wins, no slot is double-consumed, and
/// `last_indexed` is NOT advanced by the no-op second call.
#[test]
fn hnsw_index_readd_same_id_is_a_noop_first_vector_wins() {
    let embedder = MockEmbedder::new(MID_DIM);
    // Two distinct vectors for the same id, plus a bystander.
    let v_first = embedder.embed(&["first unique tokens".to_string()]).unwrap().remove(0);
    let v_second = embedder.embed(&["second different words".to_string()]).unwrap().remove(0);
    let v_other = embedder.embed(&["other bystander phrase".to_string()]).unwrap().remove(0);

    let mut index = HnswIndex::with_capacity(3);
    index.add("anchor", &v_other); // inserted first so last_indexed = "anchor"
    index.add("id-x", &v_first);  // first add: v_first wins; last_indexed = "id-x"
    index.add("id-x", &v_second); // duplicate: must be a no-op

    // (a) Only one slot consumed: searching returns "id-x" exactly once.
    let hits = index.search(&v_first, 3);
    let count = hits.iter().filter(|(id, _)| id == "id-x").count();
    assert_eq!(count, 1, "id-x must appear exactly once, got {hits:?}");

    // (b) First vector wins: querying with v_first ranks id-x as top-1.
    assert_eq!(hits[0].0, "id-x", "first-added vector must be top-1 on self-query");
    assert!(
        hits[0].1 <= EXACT_MATCH_MAX_DISTANCE,
        "self-query on first vector must be ≈0 distance, got {}",
        hits[0].1
    );

    // (c) last_indexed not advanced by the no-op second add.
    assert_eq!(
        index.last_indexed().as_deref(),
        Some("id-x"),
        "last_indexed must be id-x (set by the first add, not changed by the duplicate)"
    );
}

/// (b) F2 — rebuild is stable across re-opens: two tests in one function.
///
/// **Top-1 identity:** a query vector *equal* to one of the inserted vectors
/// must return that exact event_id as top-1 on every rebuild. Because cosine
/// distance to the query is ≈0 and all other distances are strictly positive,
/// the winner is unambiguous regardless of the RNG state in the HNSW graph.
///
/// **Recall stability across 3 rebuilds:** a query clearly closest to a known
/// event must include that event_id in the top-k on every rebuild. This proves
/// "the relevant memory is reliably recalled across re-opens" without asserting
/// brittle deep-rank order. hnsw_rs 0.3.4 seeds its level-assignment RNG from
/// OS randomness at each `Hnsw::new` construction (no seed API is exposed), so
/// the absolute rank of the 2nd/3rd neighbours varies; asserting top-k SET
/// equality across instances is therefore not meaningful.
#[test]
fn rebuild_reproduces_top1_and_recall_stability_across_reopens() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("m.db");

    // Seed a corpus of distinct memory events and derive their vectors.
    let phrases: Vec<String> = (0..REBUILD_CORPUS)
        .map(|i| format!("memory entry number {i} about topic {i}"))
        .collect();
    let known_idx: usize = 3; // the event whose text the queries target
    {
        let key = SigningKey::from_bytes(&KEY_BYTES);
        let log = EventLog::open(&db, &DEK, key).unwrap();
        for p in &phrases {
            log.append(mk_memory_event(p)).unwrap();
        }
        let embedder = MockEmbedder::new(MID_DIM);
        assert_eq!(log.rederive_pending(&embedder).unwrap(), REBUILD_CORPUS);
    }

    let embedder = MockEmbedder::new(MID_DIM);

    // A query vector *equal* to the corpus entry at `known_idx` (self-query).
    // Distance to itself is ≈0 so top-1 is unambiguous regardless of graph layout.
    let self_query = embedder
        .embed(&[phrases[known_idx].clone()])
        .unwrap()
        .remove(0);

    // A near-but-not-identical query for the recall-stability check: different
    // tokens from `self_query` but still closest to `known_idx` in the corpus.
    // MockEmbedder is a bag-of-words model, so different token sets produce
    // genuinely different vectors — this exercises the ANN index rather than
    // merely repeating the self-query.
    let recall_query = embedder
        .embed(&[format!("topic {known_idx} entry")])
        .unwrap()
        .remove(0);

    // --- Rebuild 1: establish top-1 and capture the target event_id. ---
    let known_event_id: String = {
        let key = SigningKey::from_bytes(&KEY_BYTES);
        let log = EventLog::open(&db, &DEK, key).unwrap();
        log.rebuild_indexes(&embedder).unwrap();
        let hits = log.vector_search(&self_query, RECALL_K).unwrap();
        assert_eq!(hits.len(), RECALL_K, "rebuild 1: expected k hits");
        assert!(
            hits[0].1 <= EXACT_MATCH_MAX_DISTANCE,
            "rebuild 1: self-query top-1 distance must be ≈0, got {}",
            hits[0].1
        );
        hits[0].0.clone()
    };

    // --- Rebuild 2: re-open (fresh Hnsw, fresh OS-seeded RNG). ---
    {
        let key = SigningKey::from_bytes(&KEY_BYTES);
        let log = EventLog::open(&db, &DEK, key).unwrap();
        assert_eq!(
            log.rederive_pending(&embedder).unwrap(),
            0,
            "vectors persist; rederive after reopen is a no-op"
        );
        log.rebuild_indexes(&embedder).unwrap();

        // Top-1 identity: self-query must still return the same event_id.
        let top1 = &log.vector_search(&self_query, RECALL_K).unwrap()[0].0;
        assert_eq!(top1, &known_event_id, "rebuild 2: top-1 must be reproducible");

        // Recall stability: the known event must appear in the top-k.
        let recall_ids: std::collections::HashSet<String> = log
            .vector_search(&recall_query, RECALL_K)
            .unwrap()
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert!(
            recall_ids.contains(&known_event_id),
            "rebuild 2: known event_id must be in top-{RECALL_K}, got {recall_ids:?}"
        );
    }

    // --- Rebuild 3: one more fresh instance to strengthen the stability claim. ---
    {
        let key = SigningKey::from_bytes(&KEY_BYTES);
        let log = EventLog::open(&db, &DEK, key).unwrap();
        log.rebuild_indexes(&embedder).unwrap();

        let top1 = &log.vector_search(&self_query, RECALL_K).unwrap()[0].0;
        assert_eq!(top1, &known_event_id, "rebuild 3: top-1 must be reproducible");

        let recall_ids: std::collections::HashSet<String> = log
            .vector_search(&recall_query, RECALL_K)
            .unwrap()
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert!(
            recall_ids.contains(&known_event_id),
            "rebuild 3: known event_id must be in top-{RECALL_K}, got {recall_ids:?}"
        );
    }
}

/// (c) `remove`: a tombstoned id is excluded from subsequent searches.
#[test]
fn remove_tombstones_id_and_excludes_it_from_search() {
    let embedder = MockEmbedder::new(MID_DIM);
    let phrases = ["one uno", "two dos", "three tres"];
    let vecs = embedder
        .embed(&phrases.iter().map(|s| s.to_string()).collect::<Vec<_>>())
        .unwrap();

    let mut index = HnswIndex::with_capacity(phrases.len());
    for (i, v) in vecs.iter().enumerate() {
        index.add(&format!("id-{i}"), v);
    }

    // Before removal, self-query returns "id-1" first.
    assert_eq!(index.search(&vecs[1], RECALL_K)[0].0, "id-1");

    index.remove("id-1");

    // After removal, "id-1" must never appear — even when its own vector is the
    // query (which would otherwise be the exact top-1 match).
    let hits = index.search(&vecs[1], RECALL_K);
    assert!(
        hits.iter().all(|(id, _)| id != "id-1"),
        "tombstoned id must be excluded, got {hits:?}"
    );
}

/// Test-only embedder identical to [`MockEmbedder`] except it reports a
/// *different* `model_id`. Used to persist genuine foreign-model `vectors` rows
/// so the C4 active-model filter can be exercised against real cross-model data.
struct ForeignModelEmbedder {
    inner: MockEmbedder,
}

/// `model_id` of the foreign (non-active) embedder; distinct from
/// [`MOCK_MODEL_ID`] so its rows must be filtered out of the active index.
const FOREIGN_MODEL_ID: &str = "foreign-v1";

impl ForeignModelEmbedder {
    fn new() -> Self {
        Self { inner: MockEmbedder::new(MID_DIM) }
    }
}

impl Embedder for ForeignModelEmbedder {
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, BossclawError> {
        self.inner.embed(texts)
    }
    fn dim(&self) -> usize {
        MID_DIM
    }
    fn model_id(&self) -> &str {
        FOREIGN_MODEL_ID
    }
}

/// (d) C4 at the index layer — the rebuilt index contains ONLY active-model
/// vectors. We derive the same events under BOTH the active model (`mock-v1`)
/// and a foreign model (`foreign-v1`), so the `vectors` table holds genuine
/// cross-model rows, then prove `rebuild_indexes(&MockEmbedder)` indexes only
/// the active rows: the active self-query finds its own id, and no foreign id
/// can ever surface.
///
/// # Why this does not assert `hits.len() == active_count`
///
/// `HnswIndex` is an *approximate* nearest-neighbour index, and hnsw_rs 0.3.4
/// reseeds its level-assignment RNG from OS randomness at each `Hnsw::new`
/// (see [`bossclaw_core::index::HnswIndex`] docs). For a tiny corpus a fraction
/// of seeds yield a graph whose layer-0 connectivity returns fewer than the
/// full set on a `k`-NN query — so demanding the result length equal the corpus
/// size asserts *perfect recall*, which an ANN does not guarantee per-seed and
/// which made this test flaky (~4–5% of runs, single-threaded or parallel). The
/// real C4 invariant is "no cross-model bleed", asserted directly here: every
/// returned id is an active id and the foreign id never appears, regardless of
/// how many active rows the seed happens to surface. Top-1 self-query stability
/// (which holds for every seed) anchors that the active vector is found at all.
#[test]
fn rebuild_indexes_only_includes_active_model_vectors() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("m.db");
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let log = EventLog::open(&db, &DEK, key).unwrap();

    // Two memory events under the active model.
    let id_a = log.append(mk_memory_event("active alpha")).unwrap();
    let id_b = log.append(mk_memory_event("active beta")).unwrap();
    let embedder = MockEmbedder::new(MID_DIM);
    assert_eq!(log.rederive_pending(&embedder).unwrap(), 2);

    // Persist genuine foreign-model rows for the SAME events. `derive_vector`
    // keys on `(event_id, embedder.model_id())`, so these are distinct rows the
    // active rebuild must ignore — this is the real cross-model bleed test.
    let foreign = ForeignModelEmbedder::new();
    for event in log.stream_all().unwrap() {
        if event.event_type == "memory" {
            assert!(
                log.derive_vector(&foreign, &event).unwrap(),
                "memory events are embeddable under the foreign model too"
            );
        }
    }

    // The active model and the foreign model each hold exactly two vectors; the
    // two sets are independent (no bleed in either direction).
    assert_eq!(log.vectors_for_model(MOCK_MODEL_ID).unwrap().len(), 2);
    assert_eq!(log.vectors_for_model(FOREIGN_MODEL_ID).unwrap().len(), 2);

    // Build the index for the ACTIVE model only.
    log.rebuild_indexes(&embedder).unwrap();
    let av = embedder.embed(&["active alpha".to_string()]).unwrap().remove(0);
    let hits = log.vector_search(&av, REBUILD_CORPUS).unwrap();

    // (1) The active vector is found (seed-stable top-1 identity).
    assert_eq!(hits[0].0, id_a, "active-model self-query finds its own id");

    // (2) C4 — no cross-model bleed: every returned id is an active id. The
    //     active set is exactly {id_a, id_b}; the foreign rows must never appear.
    let active_ids: std::collections::HashSet<&String> =
        [&id_a, &id_b].into_iter().collect();
    for (hit_id, _) in &hits {
        assert!(
            active_ids.contains(hit_id),
            "rebuilt index returned a non-active id {hit_id:?}; hits={hits:?}"
        );
    }
}

/// (e) `vector_search` before any `rebuild_indexes` returns the "not built"
/// `InvalidInput` error rather than panicking or returning empty.
#[test]
fn vector_search_before_build_returns_not_built_error() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let log = EventLog::open(&dir.path().join("m.db"), &DEK, key).unwrap();

    let embedder = MockEmbedder::new(MID_DIM);
    let query = embedder.embed(&["anything".to_string()]).unwrap().remove(0);
    let result = log.vector_search(&query, RECALL_K);
    assert!(
        matches!(result, Err(BossclawError::InvalidInput(_))),
        "search before rebuild must be InvalidInput, got {result:?}"
    );
}

// ---------------------------------------------------------------------------
// Real-model integration test (ignored in the hermetic suite)
// ---------------------------------------------------------------------------
//
// To run this test, first fetch the model:
//
//   pip install huggingface_hub
//   python - <<'EOF'
//   from huggingface_hub import snapshot_download
//   path = snapshot_download("minishlab/potion-base-8M")
//   print(path)
//   EOF
//
// Then point the env var at the downloaded directory and run:
//
//   BOSSCLAW_TEST_MODEL_DIR=<path> \
//     cargo test -p bossclaw-core -- --include-ignored model2vec_real_model
//
/// Integration test for [`Model2Vec`] against a real `minishlab/potion-base-8M`
/// model directory.
///
/// Skipped automatically unless `BOSSCLAW_TEST_MODEL_DIR` is set in the
/// environment. See module-level comment for download instructions.
#[test]
#[ignore = "requires BOSSCLAW_TEST_MODEL_DIR pointing at a local model dir"]
fn model2vec_real_model_embedding_shape_and_recall() {
    let dir = match std::env::var("BOSSCLAW_TEST_MODEL_DIR") {
        Ok(v) if !v.is_empty() => std::path::PathBuf::from(v),
        _ => {
            eprintln!(
                "BOSSCLAW_TEST_MODEL_DIR not set — skipping real-model test. \
                 See test doc-comment for download instructions."
            );
            return;
        }
    };

    let model = Model2Vec::from_pretrained(&dir, "minishlab/potion-base-8M")
        .expect("Model2Vec::from_pretrained failed");

    assert!(model.dim() > 0, "dim must be positive after load");
    assert_eq!(model.model_id(), "minishlab/potion-base-8M");

    // Near-paraphrase pair and an unrelated sentence.
    let paraphrase_a = "A cat sat on the mat.".to_string();
    let paraphrase_b = "A feline rested on a rug.".to_string();
    let unrelated = "The stock market fell sharply today.".to_string();

    let vecs = model
        .embed(&[paraphrase_a, paraphrase_b, unrelated])
        .expect("embed failed");

    assert_eq!(vecs.len(), 3);
    assert_eq!(vecs[0].len(), model.dim());
    assert_eq!(vecs[1].len(), model.dim());
    assert_eq!(vecs[2].len(), model.dim());

    let cos = |a: &[f32], b: &[f32]| -> f32 {
        a.iter().zip(b.iter()).map(|(x, y)| x * y).sum::<f32>()
    };

    let sim_paraphrase = cos(&vecs[0], &vecs[1]);
    let sim_unrelated_a = cos(&vecs[0], &vecs[2]);
    let sim_unrelated_b = cos(&vecs[1], &vecs[2]);

    eprintln!("cosine(paraphrase pair)  = {sim_paraphrase:.4}");
    eprintln!("cosine(a, unrelated)     = {sim_unrelated_a:.4}");
    eprintln!("cosine(b, unrelated)     = {sim_unrelated_b:.4}");

    assert!(
        sim_paraphrase > sim_unrelated_a,
        "paraphrase pair ({sim_paraphrase:.4}) should be closer than \
         sentence-a vs unrelated ({sim_unrelated_a:.4})"
    );
    assert!(
        sim_paraphrase > sim_unrelated_b,
        "paraphrase pair ({sim_paraphrase:.4}) should be closer than \
         sentence-b vs unrelated ({sim_unrelated_b:.4})"
    );
}

// ---------------------------------------------------------------------------
// Task 6: FTS5 keyword index — keyword_add / keyword_search / rebuild FTS
// ---------------------------------------------------------------------------

use bossclaw_core::keyword::escape_fts_query;

/// Basic add + search: appending a memory event and calling keyword_add lets
/// keyword_search return the event's id for a matching query.
#[test]
fn keyword_add_and_search_returns_matching_event_id() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let log = EventLog::open(&dir.path().join("m.db"), &DEK, key).unwrap();

    let id = log.append(mk_memory_event("the quick brown fox")).unwrap();
    log.keyword_add(&id, "the quick brown fox").unwrap();

    let hits = log.keyword_search("quick brown", 5).unwrap();
    assert!(!hits.is_empty(), "should find at least one hit");
    assert_eq!(hits[0].0, id, "top hit must be the indexed event");
}

/// A query that does not match any indexed body returns an empty vec.
#[test]
fn keyword_search_non_matching_query_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let log = EventLog::open(&dir.path().join("m.db"), &DEK, key).unwrap();

    let id = log.append(mk_memory_event("cats and dogs")).unwrap();
    log.keyword_add(&id, "cats and dogs").unwrap();

    let hits = log.keyword_search("rhinoceros", 5).unwrap();
    assert!(hits.is_empty(), "no match expected, got {hits:?}");
}

/// An empty or whitespace-only query returns an empty vec immediately (FTS5
/// would error on an empty MATCH string; we guard it in keyword_search).
#[test]
fn keyword_search_empty_query_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let log = EventLog::open(&dir.path().join("m.db"), &DEK, key).unwrap();

    let id = log.append(mk_memory_event("something")).unwrap();
    log.keyword_add(&id, "something").unwrap();

    assert!(log.keyword_search("", 5).unwrap().is_empty());
    assert!(log.keyword_search("   ", 5).unwrap().is_empty());
}

/// keyword_add is idempotent by event_id: adding the same event_id twice
/// must produce exactly one row in fts_map and one searchable entry.
#[test]
fn keyword_add_dedup_same_event_id_produces_one_fts_map_row() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let log = EventLog::open(&dir.path().join("m.db"), &DEK, key).unwrap();

    let id = log.append(mk_memory_event("unique content here")).unwrap();

    // Add the same event_id twice.
    log.keyword_add(&id, "unique content here").unwrap();
    log.keyword_add(&id, "unique content here").unwrap();

    // Should still find exactly one result.
    let hits = log.keyword_search("unique content", 10).unwrap();
    let count = hits.iter().filter(|(hit_id, _)| hit_id == &id).count();
    assert_eq!(count, 1, "event_id must appear exactly once, got {hits:?}");
}

/// rebuild_indexes populates the FTS index from event log content; after
/// rebuild, keyword_search finds the event without a prior keyword_add call.
#[test]
fn rebuild_indexes_populates_fts_from_event_log() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let log = EventLog::open(&dir.path().join("m.db"), &DEK, key).unwrap();

    let id = log.append(mk_memory_event("rustacean memory store")).unwrap();
    let embedder = MockEmbedder::new(MID_DIM);
    log.rederive_pending(&embedder).unwrap();

    // rebuild_indexes must populate FTS (no manual keyword_add).
    log.rebuild_indexes(&embedder).unwrap();

    let hits = log.keyword_search("rustacean", 5).unwrap();
    assert!(!hits.is_empty(), "rebuild must populate FTS");
    assert_eq!(hits[0].0, id);
}

/// rebuild_indexes is idempotent: calling it twice produces the same search
/// results and does NOT duplicate entries in fts_map.
#[test]
fn rebuild_indexes_fts_is_idempotent_no_duplicate_rows() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let log = EventLog::open(&dir.path().join("m.db"), &DEK, key).unwrap();

    for phrase in ["first memory", "second memory", "third memory"] {
        log.append(mk_memory_event(phrase)).unwrap();
    }
    let embedder = MockEmbedder::new(MID_DIM);
    log.rederive_pending(&embedder).unwrap();

    log.rebuild_indexes(&embedder).unwrap();
    let hits_first = log.keyword_search("memory", 10).unwrap();

    // Second rebuild — results must be identical, no duplicates.
    log.rebuild_indexes(&embedder).unwrap();
    let hits_second = log.keyword_search("memory", 10).unwrap();

    assert_eq!(
        hits_first.len(),
        hits_second.len(),
        "duplicate entries after second rebuild: first={hits_first:?}, second={hits_second:?}"
    );
    assert_eq!(hits_second.len(), 3, "exactly 3 events indexed");
}

/// escape_fts_query: a query containing `"` and FTS5 operators does not panic
/// and the escaped form is a valid quoted-phrase literal.
#[test]
fn escape_fts_query_sanitises_operators_and_quotes() {
    // FTS5 operators that would break an unescaped MATCH.
    let dangerous = r#"foo OR "bar"#;
    let escaped = escape_fts_query(dangerous);

    // Must be a quoted phrase (starts and ends with ").
    assert!(escaped.starts_with('"'), "must start with quote");
    assert!(escaped.ends_with('"'), "must end with quote");

    // Must not panic when used in an actual query against a real index.
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let log = EventLog::open(&dir.path().join("m.db"), &DEK, key).unwrap();
    let id = log.append(mk_memory_event("foo bar baz")).unwrap();
    log.keyword_add(&id, "foo bar baz").unwrap();

    // The escaped query is treated as a literal phrase — no parse error.
    let result = log.keyword_search(dangerous, 5);
    assert!(result.is_ok(), "escaped query must not error: {result:?}");
}

// ---------------------------------------------------------------------------
// Task 7: hybrid recall — RRF fusion + recency/pin boosts + no-op reranker
// ---------------------------------------------------------------------------

use bossclaw_core::recall::{rrf_fuse, RecallOptions, RecallSource};

/// Final top-k requested by the recall tests. Small (so assertions read
/// naturally) but > 1 so "ranks above" comparisons are meaningful.
const RECALL_TOP_K: usize = 5;

/// Seed a fresh, fully-indexed `EventLog`: append each `(text)` as a memory
/// event (in order), derive every vector, then build BOTH indexes. Returns the
/// log, a held-open `tempdir` (kept alive by the caller), and the appended ids in
/// append order so tests can refer to a specific event.
fn seeded_log(texts: &[&str]) -> (EventLog, tempfile::TempDir, Vec<String>) {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let log = EventLog::open(&dir.path().join("m.db"), &DEK, key).unwrap();
    let mut ids = Vec::with_capacity(texts.len());
    for t in texts {
        ids.push(log.append(mk_memory_event(t)).unwrap());
    }
    let embedder = MockEmbedder::new(MID_DIM);
    log.rederive_pending(&embedder).unwrap();
    log.rebuild_indexes(&embedder).unwrap();
    (log, dir, ids)
}

/// `Hit` for a given event id, if present in the result set.
fn find_hit<'a>(
    hits: &'a [bossclaw_core::Hit],
    id: &str,
) -> Option<&'a bossclaw_core::Hit> {
    hits.iter().find(|h| h.event_id == id)
}

/// Rank (0-based position) of an event id in the result set, if present.
fn rank_of(hits: &[bossclaw_core::Hit], id: &str) -> Option<usize> {
    hits.iter().position(|h| h.event_id == id)
}

/// Hybrid relevance: an event that matches the query BOTH semantically (shared
/// bag-of-words tokens → near-zero cosine distance) and lexically (FTS5 match)
/// must rank #1. The other corpus entries share no tokens with the query, so the
/// target is the only id surfaced by both arms.
#[test]
fn recall_hybrid_match_ranks_first() {
    // Disjoint vocabularies: only the "rustacean" entry overlaps the query.
    let (log, _dir, ids) = seeded_log(&[
        "quantum entanglement physics lecture",
        "rustacean memory engine ferris crab",
        "sourdough baking hydration levels",
    ]);
    let target = &ids[1];

    let embedder = MockEmbedder::new(MID_DIM);
    let hits = log
        .recall(&embedder, "rustacean memory engine", RECALL_TOP_K, &RecallOptions::default())
        .unwrap();

    assert!(!hits.is_empty(), "recall must return hits");
    assert_eq!(
        &hits[0].event_id, target,
        "the doubly-matched event must rank first, got {hits:?}"
    );
    // It was surfaced by BOTH arms (the hybrid signal).
    let target_hit = find_hit(&hits, target).expect("target present");
    assert!(
        target_hit.sources.contains(&RecallSource::Vector)
            && target_hit.sources.contains(&RecallSource::Keyword),
        "top hit must carry both Vector and Keyword provenance, got {:?}",
        target_hit.sources
    );
}

/// The keyword arm contributes: an event the query matches lexically appears in
/// results with `RecallSource::Keyword` among its sources.
#[test]
fn recall_keyword_arm_contributes_keyword_source() {
    let (log, _dir, ids) = seeded_log(&[
        "alpha beta gamma delta",
        "epsilon zeta eta theta",
    ]);
    let target = &ids[0];

    let embedder = MockEmbedder::new(MID_DIM);
    let hits = log
        .recall(&embedder, "alpha beta", RECALL_TOP_K, &RecallOptions::default())
        .unwrap();

    let target_hit = find_hit(&hits, target).expect("lexically-matched event must be present");
    assert!(
        target_hit.sources.contains(&RecallSource::Keyword),
        "matched event must carry Keyword provenance, got {:?}",
        target_hit.sources
    );
}

/// The vector arm contributes: `RecallSource::Vector` appears for a
/// semantically-matched event. The query vector equals the target's embedding
/// (same tokens), so the ANN arm returns it; provenance must record that.
#[test]
fn recall_vector_arm_contributes_vector_source() {
    let (log, _dir, ids) = seeded_log(&[
        "neptune saturn jupiter mars",
        "violin cello viola contrabass",
    ]);
    let target = &ids[0];

    let embedder = MockEmbedder::new(MID_DIM);
    let hits = log
        .recall(&embedder, "neptune saturn jupiter mars", RECALL_TOP_K, &RecallOptions::default())
        .unwrap();

    let target_hit = find_hit(&hits, target).expect("semantically-matched event must be present");
    assert!(
        target_hit.sources.contains(&RecallSource::Vector),
        "matched event must carry Vector provenance, got {:?}",
        target_hit.sources
    );
}

/// Recency tie-break: two events with IDENTICAL text tie on both arms (identical
/// embedding → identical cosine distance; identical body → identical BM25), so
/// their fused RRF base score is exactly equal. The later-appended event (newer
/// `ts`) must therefore rank above the older one — recency is the sole, and
/// deterministic, decider regardless of the absolute `Utc::now()`.
#[test]
fn recall_recency_breaks_tie_newer_ranks_first() {
    let text = "identical twins tie breaker phrase";
    // Append the SAME text twice; ids[0] is older, ids[1] is newer.
    let (log, _dir, ids) = seeded_log(&[text, text]);
    let older = &ids[0];
    let newer = &ids[1];

    let embedder = MockEmbedder::new(MID_DIM);
    let hits = log
        .recall(&embedder, text, RECALL_TOP_K, &RecallOptions::default())
        .unwrap();

    let newer_rank = rank_of(&hits, newer).expect("newer event present");
    let older_rank = rank_of(&hits, older).expect("older event present");
    assert!(
        newer_rank < older_rank,
        "newer event (rank {newer_rank}) must rank above older (rank {older_rank}); hits={hits:?}"
    );
}

/// Pin boost: pinning an id raises it above an otherwise-equal non-pinned id.
/// Two identical-text events tie on RRF; pinning the OLDER one (which recency
/// would otherwise rank second) must flip it above the newer, unpinned event —
/// proving the pin multiplier dominates the recency tilt.
#[test]
fn recall_pin_boost_raises_pinned_above_equal() {
    let text = "pinned versus unpinned equal candidates";
    let (log, _dir, ids) = seeded_log(&[text, text]);
    let older = &ids[0];
    let newer = &ids[1];

    // Sanity: without a pin, the newer event wins the recency tie-break.
    let embedder = MockEmbedder::new(MID_DIM);
    let baseline = log
        .recall(&embedder, text, RECALL_TOP_K, &RecallOptions::default())
        .unwrap();
    assert!(
        rank_of(&baseline, newer) < rank_of(&baseline, older),
        "baseline: newer should lead before pinning; got {baseline:?}"
    );

    // Pin the OLDER event → it must now outrank the newer, unpinned one.
    let opts = RecallOptions { pinned: vec![older.clone()], ..Default::default() };
    let hits = log.recall(&embedder, text, RECALL_TOP_K, &opts).unwrap();
    let older_rank = rank_of(&hits, older).expect("older present");
    let newer_rank = rank_of(&hits, newer).expect("newer present");
    assert!(
        older_rank < newer_rank,
        "pinned older event (rank {older_rank}) must outrank unpinned newer (rank {newer_rank}); hits={hits:?}"
    );
}

/// Downstream-untouched (Rung 3 A10): a MULTI-CHUNK event still flows through the
/// full recall pipeline by its BARE event id — recency/pin boost applies to it,
/// and it passes the pages/files + superseded/revoked filters. This is the
/// end-to-end proof that vector_search's chunk→event fold-back preserves the
/// bare-event_id contract every downstream stage depends on: if a composite
/// chunk key leaked, the pin set (keyed by bare id) would never match and the
/// boost would not land.
#[test]
fn recall_multi_chunk_event_keeps_bare_id_downstream_contract() {
    // A long event (multiple chunks) sharing the query vocabulary, plus a
    // single-chunk competitor with the SAME query tokens so they tie on
    // relevance before any boost.
    let shared = "quantum lattice resonance cascade";
    let mut long_text = String::new();
    for block in 0..8 {
        long_text.push_str(&format!("# Part {block}\n\n"));
        for line in 0..6 {
            long_text.push_str(&format!(
                "{shared} — part {block} line {line} elaborates with filler prose \
                 and token{block}{line} to keep blocks distinct.\n"
            ));
        }
        long_text.push('\n');
    }
    assert!(
        long_text.chars().count() > 1_500,
        "long event must exceed one chunk budget"
    );

    let (log, _dir, ids) = seeded_log(&[long_text.as_str(), shared]);
    let long_id = &ids[0];
    let short_id = &ids[1];

    let embedder = MockEmbedder::new(MID_DIM);

    // (1) Baseline: both surface for the shared query, each by its BARE id.
    let baseline = log
        .recall(&embedder, shared, RECALL_TOP_K, &RecallOptions::default())
        .unwrap();
    let long_hit = find_hit(&baseline, long_id)
        .expect("multi-chunk event must surface in recall by its bare event id");
    assert_eq!(
        &long_hit.event_id, long_id,
        "the multi-chunk event is identified by its BARE id, not a chunk key"
    );
    assert!(
        find_hit(&baseline, short_id).is_some(),
        "the single-chunk competitor must also surface"
    );

    // (2) Pin the multi-chunk event BY ITS BARE ID → the pin boost must land on
    //     it (proving the folded id matches the pin set) and lift it above the
    //     unpinned competitor.
    let opts = RecallOptions { pinned: vec![long_id.clone()], ..Default::default() };
    let pinned = log.recall(&embedder, shared, RECALL_TOP_K, &opts).unwrap();
    let long_rank = rank_of(&pinned, long_id).expect("multi-chunk event present when pinned");
    let short_rank = rank_of(&pinned, short_id).expect("competitor present");
    assert!(
        long_rank < short_rank,
        "pinning the multi-chunk event by its bare id must boost it above the \
         unpinned competitor (fold-back preserved the bare-id contract); \
         pinned={pinned:?}"
    );
}

/// Degrade-to-keyword (spec §10): WITHOUT building the vector index, recall must
/// still return keyword results and must NOT error. The vector arm fails with
/// `InvalidInput` (index not built); recall logs a warning and falls back to the
/// keyword arm.
#[test]
fn recall_degrades_to_keyword_when_vector_index_missing() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let log = EventLog::open(&dir.path().join("m.db"), &DEK, key).unwrap();

    let id = log.append(mk_memory_event("ferris the crab mascot")).unwrap();
    // Build ONLY the keyword index (no vector index): add the FTS entry directly.
    log.keyword_add(&id, "ferris the crab mascot").unwrap();

    // The query is escaped to a literal FTS5 phrase, so its tokens must be
    // adjacent in the body — "ferris the" matches "ferris the crab mascot".
    let embedder = MockEmbedder::new(MID_DIM);
    let result = log.recall(&embedder, "ferris the", RECALL_TOP_K, &RecallOptions::default());

    let hits = result.expect("recall must not error when only the vector arm is unavailable");
    let hit = find_hit(&hits, &id).expect("keyword-only result must surface the event");
    assert!(
        hit.sources.contains(&RecallSource::Keyword) && !hit.sources.contains(&RecallSource::Vector),
        "degraded result must carry Keyword provenance only, got {:?}",
        hit.sources
    );
}

// ---------------------------------------------------------------------------
// §10 arm-resolution pure unit tests (resolve_arms helper)
// ---------------------------------------------------------------------------

/// vector-fail → keyword-only: when the vector arm errors, recall degrades to
/// keyword-only without propagating the error.
#[test]
fn resolve_arms_vector_fail_degrades_to_keyword_only() {
    let kw: Vec<(String, f32)> = vec![("ev1".to_string(), -1.0)];
    let (vec_arm, kw_arm) =
        bossclaw_core::log::resolve_arms(
            Err(BossclawError::InvalidInput("no index".into())),
            Ok(kw.clone()),
        )
        .expect("should not error when keyword arm is ok");
    assert!(vec_arm.is_empty(), "vector arm must be empty on failure");
    assert_eq!(kw_arm, kw, "keyword arm must be returned unchanged");
}

/// keyword-fail → vector-only: when the keyword arm errors, recall degrades to
/// vector-only without propagating the error.
#[test]
fn resolve_arms_keyword_fail_degrades_to_vector_only() {
    let vec_hits: Vec<(String, f32)> = vec![("ev2".to_string(), 0.1)];
    let (vec_arm, kw_arm) =
        bossclaw_core::log::resolve_arms(
            Ok(vec_hits.clone()),
            Err(BossclawError::Store("fts gone".into())),
        )
        .expect("should not error when vector arm is ok");
    assert_eq!(vec_arm, vec_hits, "vector arm must be returned unchanged");
    assert!(kw_arm.is_empty(), "keyword arm must be empty on failure");
}

/// both-fail → Err: when both arms error, resolve_arms must propagate Err.
#[test]
fn resolve_arms_both_fail_returns_err() {
    let result = bossclaw_core::log::resolve_arms(
        Err(BossclawError::InvalidInput("no index".into())),
        Err(BossclawError::Store("fts gone".into())),
    );
    assert!(
        result.is_err(),
        "both arms failed → must return Err, got {result:?}"
    );
    assert!(
        matches!(result, Err(BossclawError::InvalidInput(_))),
        "error variant must be InvalidInput (vector arm surfaced), got {result:?}"
    );
}

/// `rrf_fuse` unit test (pure): an id ranked #1 in BOTH arms must score strictly
/// higher than one ranked #1 in only a single arm. This pins the RRF formula
/// (Σ 1/(k + rank), 1-based) independently of any database wiring.
#[test]
fn rrf_fuse_id_in_both_arms_scores_higher_than_single_arm() {
    let arms = vec![
        // Arm 1: "both" #1, "vec_only" #2.
        vec!["both".to_string(), "vec_only".to_string()],
        // Arm 2: "both" #1, "kw_only" #2.
        vec!["both".to_string(), "kw_only".to_string()],
    ];
    let scores = rrf_fuse(&arms);

    let both = scores["both"];
    let vec_only = scores["vec_only"];
    let kw_only = scores["kw_only"];

    assert!(
        both > vec_only,
        "id in both arms ({both}) must beat single-arm id ({vec_only})"
    );
    assert!(
        both > kw_only,
        "id in both arms ({both}) must beat single-arm id ({kw_only})"
    );
    // The two single-arm ids (each #1 in exactly one arm) tie.
    assert!(
        (vec_only - kw_only).abs() < 1e-9,
        "two ids each ranked #1 in one arm must tie: {vec_only} vs {kw_only}"
    );
}

// ---------------------------------------------------------------------------
// Task 9: re-embed migration + timing budget + integrity note
// ---------------------------------------------------------------------------

/// A second test embedder with a distinct `model_id` ("mock-v2") and the same
/// FNV-1a bag-of-words logic as `MockEmbedder` (via delegation). Used to
/// exercise the model-switch path without any shared `model_id` constant.
struct MockEmbedderV2 {
    inner: MockEmbedder,
}

/// Stable `model_id` for the second test model.
const MOCK_V2_MODEL_ID: &str = "mock-v2";

impl MockEmbedderV2 {
    fn new(dim: usize) -> Self {
        Self { inner: MockEmbedder::new(dim) }
    }
}

impl Embedder for MockEmbedderV2 {
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, BossclawError> {
        self.inner.embed(texts)
    }
    fn dim(&self) -> usize {
        self.inner.dim()
    }
    fn model_id(&self) -> &str {
        MOCK_V2_MODEL_ID
    }
}

/// Migration happy path:
/// 1. Seed events and derive vectors under mock-v1.
/// 2. `reembed_migration(&v2)` switches to mock-v2.
/// 3. Assert: active model is mock-v2; mock-v1 vectors are GC'd; mock-v2
///    vectors count == event_count; recall works; stats are correct.
#[test]
fn reembed_migration_happy_path() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let log = EventLog::open(&dir.path().join("m.db"), &DEK, key).unwrap();

    // Seed 3 memory events and derive under mock-v1.
    let event_count = 3usize;
    for t in ["ocean waves crashing", "forest trees rustling", "mountain peaks snowy"] {
        log.append(mk_memory_event(t)).unwrap();
    }
    let v1 = MockEmbedder::new(MID_DIM);
    let backfilled = log.rederive_pending(&v1).unwrap();
    assert_eq!(backfilled, event_count, "all events backfilled under v1");

    let old_count = log.vectors_for_model(MOCK_MODEL_ID).unwrap().len();
    assert_eq!(old_count, event_count);

    // Run migration to mock-v2.
    let v2 = MockEmbedderV2::new(MID_DIM);
    let stats = log.reembed_migration(&v2).unwrap();

    // Active model must now be mock-v2.
    let active = log.active_model().unwrap().expect("active model present after migration");
    assert_eq!(active.active_model_id, MOCK_V2_MODEL_ID);
    assert_eq!(active.dim, MID_DIM as u32);
    assert_eq!(active.schema_version, SCHEMA_VERSION);

    // mock-v1 vectors must be GC'd.
    assert!(
        log.vectors_for_model(MOCK_MODEL_ID).unwrap().is_empty(),
        "v1 vectors must be GC'd after migration"
    );

    // mock-v2 vectors must cover all events.
    assert_eq!(
        log.vectors_for_model(MOCK_V2_MODEL_ID).unwrap().len(),
        event_count,
        "v2 vectors must cover all events"
    );

    // Stats.
    assert_eq!(stats.reembedded, event_count, "stats.reembedded must equal event_count");
    assert_eq!(stats.gc_removed, old_count, "stats.gc_removed must equal old v1 count");

    // Recall works under the new model (rebuild_indexes was called by migration).
    let hits = log
        .recall(&v2, "ocean waves", MID_DIM, &RecallOptions::default())
        .unwrap();
    assert!(!hits.is_empty(), "recall must return results after migration");
}

/// Resumability / idempotency:
/// Running `reembed_migration` TWICE leaves a single consistent active model,
/// re-embeds 0 events the second time, GCs 0 rows, and verify_chain passes.
#[test]
fn reembed_migration_idempotent_second_run_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let log = EventLog::open(&dir.path().join("m.db"), &DEK, key).unwrap();

    for t in ["apple pie recipe", "cherry tart filling"] {
        log.append(mk_memory_event(t)).unwrap();
    }
    let v1 = MockEmbedder::new(MID_DIM);
    log.rederive_pending(&v1).unwrap();

    let v2 = MockEmbedderV2::new(MID_DIM);

    // First run.
    let stats1 = log.reembed_migration(&v2).unwrap();
    assert_eq!(stats1.reembedded, 2);
    assert_eq!(stats1.gc_removed, 2); // 2 v1 rows removed

    // Second run: nothing to embed, nothing to GC.
    let stats2 = log.reembed_migration(&v2).unwrap();
    assert_eq!(stats2.reembedded, 0, "second run must re-embed nothing");
    assert_eq!(stats2.gc_removed, 0, "second run must GC nothing");

    // Active model is still mock-v2.
    let active = log.active_model().unwrap().unwrap();
    assert_eq!(active.active_model_id, MOCK_V2_MODEL_ID);

    // The chain remains valid after two config events.
    log.verify_chain().expect("verify_chain must pass after two migrations");
}

/// Integrity: after a migration, `verify_chain()` returns Ok (the config event
/// appended by reembed_migration is in the signed chain and validates).
#[test]
fn reembed_migration_config_event_is_in_signed_chain() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let log = EventLog::open(&dir.path().join("m.db"), &DEK, key).unwrap();

    log.append(mk_memory_event("signed integrity test")).unwrap();
    let v1 = MockEmbedder::new(MID_DIM);
    log.rederive_pending(&v1).unwrap();

    let v2 = MockEmbedderV2::new(MID_DIM);
    log.reembed_migration(&v2).unwrap();

    // The config event written by migration must be in the signed chain.
    log.verify_chain().expect("chain must verify after migration");

    // Latest config reflects the new model.
    let active = log.active_model().unwrap().unwrap();
    assert_eq!(active.active_model_id, MOCK_V2_MODEL_ID);
}

/// Initial setup: on a fresh store with NO prior config event,
/// `reembed_migration` correctly sets the active model, embeds all events,
/// and GCs nothing (there are no stale vectors).
#[test]
fn reembed_migration_initial_setup_no_prior_config() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let log = EventLog::open(&dir.path().join("m.db"), &DEK, key).unwrap();

    // No config event yet.
    assert!(log.active_model().unwrap().is_none(), "fresh store must have no active model");

    for t in ["sunrise morning light", "sunset evening glow"] {
        log.append(mk_memory_event(t)).unwrap();
    }

    let v1 = MockEmbedder::new(MID_DIM);
    // Do NOT call rederive_pending — migration must embed everything itself.
    let stats = log.reembed_migration(&v1).unwrap();

    assert_eq!(stats.reembedded, 2, "initial migration must embed all events");
    assert_eq!(stats.gc_removed, 0, "no stale vectors on fresh store");

    let active = log.active_model().unwrap().expect("config must exist after migration");
    assert_eq!(active.active_model_id, MOCK_MODEL_ID);
    assert_eq!(active.schema_version, SCHEMA_VERSION);

    // Chain verifies (config event is properly signed + chained).
    log.verify_chain().expect("chain must verify after initial migration");
}

// ---------------------------------------------------------------------------
// open_with_recall lifecycle (whole-impl review fix)
// ---------------------------------------------------------------------------

#[test]
fn open_with_recall_builds_index_so_recall_is_semantic_without_manual_rebuild() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("m.db");
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let embedder = MockEmbedder::new(MID_DIM);

    // Seed a store with derived vectors, then close it (the in-memory index dies).
    {
        let log = EventLog::open(&db, &DEK, key.clone()).unwrap();
        for t in ["alpha topic one", "beta topic two", "gamma topic three"] {
            log.append(mk_memory_event(t)).unwrap();
        }
        assert_eq!(log.rederive_pending(&embedder).unwrap(), 3);
    }

    // open_with_recall must rebuild the in-memory index during open — no manual
    // rebuild_indexes call needed for recall to use the semantic arm.
    let log = EventLog::open_with_recall(&db, &DEK, key, &embedder).unwrap();

    // vector_search succeeds (it would be Err(InvalidInput) if the index were
    // not built), proving open_with_recall built it.
    let qv = &embedder.embed(&["beta topic two".to_string()]).unwrap()[0];
    let hits = log
        .vector_search(qv, 3)
        .expect("open_with_recall must have built the vector index");
    assert!(!hits.is_empty(), "vector index returns results after open_with_recall");

    // recall surfaces the Vector arm — not degraded to keyword-only.
    let results = log
        .recall(&embedder, "beta topic two", 3, &bossclaw_core::recall::RecallOptions::default())
        .unwrap();
    assert!(
        results
            .iter()
            .any(|h| h.sources.contains(&bossclaw_core::recall::RecallSource::Vector)),
        "recall after open_with_recall must include a Vector-sourced hit"
    );
}

#[test]
fn open_with_recall_on_empty_store_does_not_panic() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let embedder = MockEmbedder::new(MID_DIM);

    // Fresh store, no events: open_with_recall builds an empty index, no panic.
    let log =
        EventLog::open_with_recall(&dir.path().join("empty.db"), &DEK, key, &embedder).unwrap();

    let results = log
        .recall(&embedder, "anything", 3, &bossclaw_core::recall::RecallOptions::default())
        .unwrap();
    assert!(results.is_empty(), "empty store yields no recall hits");
}

// ---------------------------------------------------------------------------
// Task 5: graph-proximity recall boost (M3 §6)
// ---------------------------------------------------------------------------

/// Auto-seed boost fires on a CURRENT edge and disappears when that edge is
/// invalidated. Score-based (robust to the arbitrary base order of
/// query-irrelevant candidates): the linked neighbour's score carries the ~1.4×
/// graph multiplier while the edge is current, and reverts to its unboosted base
/// once the edge is retired (current-edges-only gating, spec §6).
///
/// Corpus sized ≥6 (Rev 2 F7): intra-result reinforcement widens auto-seed from
/// top-1 to the top [`GRAPH_REINFORCE_TOPK`] (= 3) fused hits, and a seed is
/// EXCLUDED from being its own boost target, so the neighbour must sit OUTSIDE the
/// top-3 to be boosted purely as the top hit's neighbour.
///
/// DETERMINISM (HNSW unseeded-RNG, index.rs:94): the keyword arm escapes the whole
/// query into one FTS5 quoted *phrase* (keyword.rs), so it only surfaces events
/// that contain that phrase verbatim — a purely-disjoint or prefix-only event is
/// then reachable ONLY via the approximate ANN arm and can intermittently drop
/// out. So EVERY event embeds the full query phrase (deterministic keyword
/// presence for all, including seed membership) and is ranked by the count of
/// trailing distinct noise tokens (more noise → lower bag-of-words cosine → lower
/// rank). Event 0 (phrase only) is the top seed; the neighbour (most noise) ranks
/// last, outside the top-3, present in every result set.
#[test]
fn recall_graph_proximity_auto_seed_boosts_only_current_edges() {
    let (log, _dir, ids) = seeded_log(&[
        "rustacean memory engine ferris crab",                       // 0: phrase only → top seed
        "rustacean memory engine ferris crab nz1",                   // 1: +1 noise
        "rustacean memory engine ferris crab nz1 nz2",               // 2: +2 noise
        "rustacean memory engine ferris crab nz1 nz2 nz3",           // 3: +3 noise
        "rustacean memory engine ferris crab nz1 nz2 nz3 nz4",       // 4: +4 noise
        // 5: NEIGHBOUR — phrase + most noise → ranks LAST (outside the top-3), but
        // present in every result set because it contains the phrase verbatim.
        "rustacean memory engine ferris crab nz1 nz2 nz3 nz4 nz5 nz6",
    ]);
    // Link the top hit (0) to the out-of-top-3 neighbour (5).
    log.link(&ids[0], "relates_to", &ids[5], None, &[]).unwrap();
    log.rebuild_graph().unwrap();

    let embedder = MockEmbedder::new(MID_DIM);
    let query = "rustacean memory engine ferris crab";
    // Request the whole corpus so the last-ranked neighbour is always present in
    // BOTH result sets (its boost would otherwise lift it in/out of a small top-k,
    // confounding the score comparison with a presence change).
    let k = ids.len();

    let boosted = log.recall(&embedder, query, k, &RecallOptions::default()).unwrap();
    let s_boosted = find_hit(&boosted, &ids[5]).expect("neighbor present").score;

    // Retire the edge → the neighbour must lose the boost.
    log.invalidate(&ids[0], "relates_to", &ids[5], None, &[ids[0].clone()]).unwrap();
    log.rebuild_graph().unwrap();
    let retired = log.recall(&embedder, query, k, &RecallOptions::default()).unwrap();
    let s_retired = find_hit(&retired, &ids[5]).expect("neighbor present").score;

    assert!(
        s_boosted > s_retired * 1.2,
        "current-edge neighbor must be boosted (~1.4x) vs the retired-edge baseline: \
         boosted={s_boosted}, retired={s_retired}"
    );
}

/// Explicit `graph_seeds` boost a chosen node's neighbour that auto-seeding would
/// NOT reach via the boost. The link is explicitSeed(2) ↔ neighbour(1). For the
/// contract to hold under the widened top-[`GRAPH_REINFORCE_TOPK`] auto-seed, the
/// explicit seed (event 2) must NOT itself be an auto-seed — otherwise auto-seeding
/// would already boost the neighbour and there would be nothing left for the
/// explicit seed to add. So the top-3 auto-seeds are filled by three OTHER
/// phrase-matching events (0, 3, 4), none linked to the neighbour, and event 2 is
/// disjoint (never an auto-seed; its edge is still honoured when passed explicitly).
///
/// DETERMINISM (HNSW unseeded RNG, index.rs:94): the neighbour (event 1) is READ
/// from the result set, so it embeds the full query phrase → the keyword arm
/// surfaces it deterministically; its extra noise tokens keep it ranked below the
/// three phrase-only-ish auto-seeds. Event 2 is only ever used as a seed *id*
/// (its edges are queried directly), so it needs no recall presence.
#[test]
fn recall_graph_proximity_explicit_seeds_boost_over_autoseed() {
    let (log, _dir, ids) = seeded_log(&[
        "rustacean memory engine ferris",                       // 0: phrase → top auto-seed
        // 1: NEIGHBOUR (read from hits) — phrase + most noise → present via keyword,
        // ranked below the top-3 auto-seeds, and NOT linked to any of them.
        "rustacean memory engine ferris nz1 nz2 nz3 nz4 nz5",
        // 2: EXPLICIT SEED — disjoint, so it is never an auto-seed; only used as a
        // graph_seeds id (its edges are queried directly, no recall presence needed).
        "another disjoint vocabulary set",
        "rustacean memory engine ferris nz1",                   // 3: phrase → fills a top auto-seed
        "rustacean memory engine ferris nz1 nz2",               // 4: phrase → fills a top auto-seed
    ]);
    // Link the explicit (non-auto) seed (2) to the neighbour (1).
    log.link(&ids[2], "relates_to", &ids[1], None, &[]).unwrap();
    log.rebuild_graph().unwrap();

    let embedder = MockEmbedder::new(MID_DIM);
    let query = "rustacean memory engine ferris";
    let k = ids.len();

    let auto = log.recall(&embedder, query, k, &RecallOptions::default()).unwrap();
    let s_auto = find_hit(&auto, &ids[1]).expect("present").score;

    let opts = RecallOptions { graph_seeds: vec![ids[2].clone()], ..Default::default() };
    let seeded = log.recall(&embedder, query, k, &opts).unwrap();
    let s_seeded = find_hit(&seeded, &ids[1]).expect("present").score;

    assert!(
        s_seeded > s_auto * 1.2,
        "explicit seed must boost its neighbor over the auto-seed baseline: \
         seeded={s_seeded}, auto={s_auto}"
    );
}

/// T-D (security M3) — never-forget: a memory stays recallable after its only
/// edge is invalidated. Retiring a graph edge must NEVER suppress the memory
/// from recall — the edge's absence removes the boost but not the memory itself.
#[test]
fn invalidating_an_edge_does_not_suppress_the_memory_from_recall() {
    let (log, _dir, ids) = seeded_log(&["ferris the rustacean crab", "unrelated tokens here"]);
    log.link(&ids[0], "relates_to", &ids[1], None, &[]).unwrap();
    log.invalidate(&ids[0], "relates_to", &ids[1], None, &[ids[0].clone()]).unwrap();
    log.rebuild_graph().unwrap();

    let embedder = MockEmbedder::new(MID_DIM);
    let hits = log.recall(&embedder, "ferris the rustacean crab", RECALL_TOP_K, &RecallOptions::default()).unwrap();
    assert!(find_hit(&hits, &ids[0]).is_some(), "memory must remain recallable after its edge is retired (never-forget)");
}

// ---------------------------------------------------------------------------
// Task 6: trust-gated boost + intra-result reinforcement (Rev 2 T-H, F3, F7)
// ---------------------------------------------------------------------------

/// T-H (security #4) — trust gate is ZERO contribution, not merely "less than".
/// A LOW-confidence machine edge (`confidence_milli` 100 < 600) must contribute
/// EXACTLY ZERO recall boost: its neighbour scores bit-identically to the no-edge
/// baseline (retire the edge → compare EQUAL). A `≥600` machine edge — and a
/// `manual` edge — DOES boost. Low-confidence machine edges remain recorded +
/// queryable (never-forget); they just do not tilt recall (spec §7).
///
/// Corpus sized ≥6 (Rev 2 F7): with top-[`GRAPH_REINFORCE_TOPK`] auto-seeding and
/// seed-self-exclusion, the boosted neighbour must sit OUTSIDE the top-3 so it is
/// boosted ONLY via the trusted edge to the top hit, never because it is itself a
/// seed. As in the auto-seed test, EVERY event embeds the full query phrase so the
/// keyword arm surfaces all of them deterministically (the HNSW ANN arm is
/// unseeded, index.rs:94); trailing distinct noise tokens set the rank (more noise
/// → lower cosine → lower rank), placing the neighbour last, outside the top-3.
#[test]
fn recall_trust_gate_low_confidence_machine_edge_contributes_exactly_zero() {
    let texts: &[&str] = &[
        "rustacean memory engine ferris crab",                       // 0: phrase only → top seed
        "rustacean memory engine ferris crab nz1",                   // 1: +1 noise
        "rustacean memory engine ferris crab nz1 nz2",               // 2: +2 noise
        "rustacean memory engine ferris crab nz1 nz2 nz3",           // 3: +3 noise
        "rustacean memory engine ferris crab nz1 nz2 nz3 nz4",       // 4: +4 noise
        // 5: NEIGHBOUR — phrase + most noise → ranks LAST, outside the top-3, but
        // present in every result set (contains the phrase verbatim).
        "rustacean memory engine ferris crab nz1 nz2 nz3 nz4 nz5 nz6",
    ];
    let (log, _dir, ids) = seeded_log(texts);
    let embedder = MockEmbedder::new(MID_DIM);
    let query = "rustacean memory engine ferris crab";
    let k = ids.len();

    // ── (1) LOW-confidence machine edge (100 milli < 600): gated OUT. ──
    log.link_machine(&ids[0], "relates_to", &ids[5], 0.10, "m4-reasoner", &[ids[0].clone()])
        .unwrap();
    log.rebuild_graph().unwrap();
    let low = log.recall(&embedder, query, k, &RecallOptions::default()).unwrap();
    let s_low = find_hit(&low, &ids[5]).expect("neighbor present").score;

    // The low-confidence edge still EXISTS + is queryable (never-forget)…
    assert_eq!(log.all_edges().unwrap().len(), 1, "low-confidence edge is still recorded");

    // …and is retired to establish the true no-edge baseline.
    log.invalidate(&ids[0], "relates_to", &ids[5], None, &[ids[0].clone()]).unwrap();
    log.rebuild_graph().unwrap();
    let base = log.recall(&embedder, query, k, &RecallOptions::default()).unwrap();
    let s_base = find_hit(&base, &ids[5]).expect("neighbor present").score;

    // "Exactly zero contribution", asserted within one f32 ULP: the two scores are
    // captured at different Utc::now() instants (a rebuild_graph separates the two
    // recall calls), so the recency-jitter delta is ~one ULP and a bit-exact
    // assert_eq! could tip intermittently on slower hardware (flaky SECURITY test).
    // A real graph contribution would be ~+40%, far above one ULP — this still
    // falsifies any leak.
    let ulp = (s_base.abs() * f32::EPSILON).max(f32::MIN_POSITIVE);
    assert!(
        (s_low - s_base).abs() <= ulp,
        "a <600 machine edge must contribute ZERO boost (equal within 1 f32 ULP): \
         low={s_low}, base={s_base}, ulp={ulp}"
    );

    // ── (2) HIGH-confidence machine edge (950 milli ≥ 600): DOES boost. ──
    log.link_machine(&ids[0], "relates_to", &ids[5], 0.95, "m4-reasoner", &[ids[0].clone()])
        .unwrap();
    log.rebuild_graph().unwrap();
    let high = log.recall(&embedder, query, k, &RecallOptions::default()).unwrap();
    let s_high = find_hit(&high, &ids[5]).expect("neighbor present").score;
    assert!(
        s_high > s_base * 1.2,
        "a ≥600 machine edge must boost the neighbour over the no-edge baseline: \
         high={s_high}, baseline={s_base}"
    );

    // ── (3) A MANUAL edge also boosts (origin='manual' always passes the gate). ──
    log.invalidate(&ids[0], "relates_to", &ids[5], None, &[ids[0].clone()]).unwrap();
    log.link(&ids[0], "relates_to", &ids[5], None, &[ids[0].clone()]).unwrap();
    log.rebuild_graph().unwrap();
    let manual = log.recall(&embedder, query, k, &RecallOptions::default()).unwrap();
    let s_manual = find_hit(&manual, &ids[5]).expect("neighbor present").score;
    assert!(
        s_manual > s_base * 1.2,
        "a manual edge must boost the neighbour over the no-edge baseline: \
         manual={s_manual}, baseline={s_base}"
    );
}

/// Intra-result reinforcement (spec §7 / Rev 2): a memory that neighbours a
/// NON-top strong fused hit still gets the proximity tilt. M3 auto-seeded only the
/// single top-1 hit, so a neighbour of the #2/#3 hit got nothing; widening to the
/// top-[`GRAPH_REINFORCE_TOPK`] (= 3) fused hits lights it up. The neighbour is
/// linked to event 1 (NOT the #1 hit), so under M3's top-1 auto-seed it would get
/// nothing; the boost it receives here proves a seed BEYOND the top-1 fired.
///
/// DETERMINISM (HNSW unseeded RNG, index.rs:94): RRF gives a two-arm hit a strictly
/// higher fused score than any one-arm hit, so if the ANN arm intermittently drops
/// one of three two-arm seeds (sending it to keyword-only) a two-arm neighbour
/// would leapfrog it into the top-3 and become a seed itself — silently killing the
/// boost. To make the top-3 robust, the three seeds (events 0–2) are indexed in
/// BOTH arms (vector + keyword) while the neighbour (event 3) is indexed in the
/// keyword arm ONLY: a one-arm hit can never displace a two-arm seed, so {0,1,2}
/// are deterministically the top-3 and the neighbour is deterministically rank 4
/// (present, but never a seed). Hand-built (not `seeded_log`) precisely so the
/// neighbour's vector is withheld.
#[test]
fn recall_intra_result_reinforcement_boosts_neighbor_of_a_non_top_hit() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let log = EventLog::open(&dir.path().join("m.db"), &DEK, key).unwrap();
    let embedder = MockEmbedder::new(MID_DIM);
    let query = "rustacean memory engine ferris crab";

    // Seeds (two-arm): full phrase + increasing noise → 0 strongest, 1 the NON-top
    // strong hit, 2 fills the 3rd seed slot. Vector + keyword indexed.
    let seed_texts = [
        "rustacean memory engine ferris crab",         // 0
        "rustacean memory engine ferris crab nz1",     // 1 (non-top seed)
        "rustacean memory engine ferris crab nz1 nz2", // 2
    ];
    let mut ids = Vec::new();
    for t in seed_texts {
        ids.push(log.append(mk_memory_event(t)).unwrap());
    }

    // Build BOTH indexes from the SEEDS ONLY. The neighbour is appended *after* this
    // point precisely so its vector can never enter the HNSW: `rederive_pending`
    // embeds every pending event and `rebuild_indexes` builds the vector index from
    // ALL stored vectors, so appending the neighbour first would silently make it a
    // TWO-arm hit. A two-arm neighbour can be reordered past a seed by the HNSW's
    // OS-seeded deeper-rank jitter (index.rs §"Cross-session rank determinism"),
    // knocking that seed out of the top-`GRAPH_REINFORCE_TOPK` reinforcement set and
    // killing the boost — the macos-latest non-determinism this ordering removes.
    log.rederive_pending(&embedder).unwrap();
    log.rebuild_indexes(&embedder).unwrap();

    // Neighbour (one-arm): appended after the index build and given keyword-only
    // presence. Its VECTOR is withheld, so it is keyword-surfaced (deterministic
    // presence) yet can never out-rank a two-arm seed.
    let neighbor_text = "rustacean memory engine ferris crab nz1 nz2 nz3";
    let neighbor = log.append(mk_memory_event(neighbor_text)).unwrap();
    log.keyword_add(&neighbor, neighbor_text).unwrap();

    let k = ids.len() + 1;

    // Edge from the NON-top hit (1) to the keyword-only neighbour.
    log.link(&ids[1], "relates_to", &neighbor, None, &[ids[1].clone()]).unwrap();
    log.rebuild_graph().unwrap();
    let hits = log.recall(&embedder, query, k, &RecallOptions::default()).unwrap();
    let nb_hit = find_hit(&hits, &neighbor).expect("neighbor present");
    // Determinism precondition: the neighbour is keyword-only (one-arm). If it ever
    // regained a vector it could displace a seed under ANN jitter and reintroduce the
    // flake, so assert the precondition rather than leaving it implicit.
    assert!(
        nb_hit.sources.contains(&RecallSource::Keyword)
            && !nb_hit.sources.contains(&RecallSource::Vector),
        "neighbour must be keyword-only (one-arm), got sources={:?}",
        nb_hit.sources
    );
    let s_nb = nb_hit.score;

    // Retire the edge → the neighbour loses the reinforcement boost (baseline).
    log.invalidate(&ids[1], "relates_to", &neighbor, None, &[ids[1].clone()]).unwrap();
    log.rebuild_graph().unwrap();
    let base = log.recall(&embedder, query, k, &RecallOptions::default()).unwrap();
    let s_nb_base = find_hit(&base, &neighbor).expect("neighbor present").score;

    assert!(
        s_nb > s_nb_base * 1.2,
        "neighbour of a non-top strong hit must be reinforced: boosted={s_nb}, base={s_nb_base}"
    );
}

// ── M4b Task 5: recall surfaces CURRENT pages, excludes superseded ones, and
//    honours the one-way `exclude_pages` rule (spec §7 / F2 / F3). ──

/// The discriminator recall uses to identify a dossier hit — the same string the
/// `page` event carries as its `event_type`. Spelled once here so the tests never
/// drift from the production discriminator (no magic string).
const PAGE_KIND: &str = "page";

/// Producer label for the synthetic pages these tests emit (any non-empty model
/// id; the recall path never inspects it).
const TEST_PRODUCER: &str = "m4-reasoner";

/// Fold the event log into its projections so `current_pages()` (the recall
/// page-filter's source of truth) reflects the `page`/`supersede` events appended
/// so far, AND re-embed + re-index so a freshly-appended page is reachable by
/// recall. Mirrors the seed path in `seeded_log` but for the post-hoc pages.
fn reindex_and_refold(log: &EventLog, embedder: &MockEmbedder) {
    log.rederive_pending(embedder).unwrap();
    log.rebuild_indexes(embedder).unwrap();
    log.rebuild_graph().unwrap();
}

/// (a) a CURRENT page surfaces in recall tagged `kind == "page"`; (b) after it is
/// superseded by a replacement, recall returns the NEW page and NEVER the
/// superseded id — even though the superseded page's vector is still in the index;
/// (c) `RecallOptions{exclude_pages:true,..}` drops ALL page hits while leaving
/// raw memories. Exercises the §7/F2 superseded-exclusion and the F3 one-way rule.
#[test]
fn current_page_surfaces_superseded_excluded_and_exclude_pages_hides_all() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let log = EventLog::open(&dir.path().join("m.db"), &DEK, key).unwrap();
    let embedder = MockEmbedder::new(MID_DIM);

    // The query phrase every asserted event embeds verbatim, so BOTH the keyword
    // arm (FTS5 quotes the whole query into one phrase) and the vector arm surface
    // it deterministically (see the graph-proximity test's note above).
    let query = "acme corp dossier";
    let topic = "entity:01TOPICKENNY";

    // A raw memory the page can cite (contains the query phrase verbatim).
    let mem = log
        .append(mk_memory_event("acme corp dossier kenny works here"))
        .unwrap();

    // First dossier (current). Its body embeds the query phrase so recall finds it.
    let p1 = log
        .page(
            topic,
            "Kenny",
            "acme corp dossier kenny summary v1",
            &[json!({ "text": "Kenny works at Acme.", "cites": [mem.clone()] })],
            &[],
            TEST_PRODUCER,
            std::slice::from_ref(&mem),
        )
        .unwrap();
    reindex_and_refold(&log, &embedder);

    // (a) the current page surfaces, tagged kind == "page".
    let hits = log
        .recall(&embedder, query, RECALL_TOP_K, &RecallOptions::default())
        .unwrap();
    let page_hit = find_hit(&hits, &p1).expect("current page must surface in recall");
    assert_eq!(page_hit.kind, PAGE_KIND, "a page hit must carry kind == \"page\"");

    // (b) supersede p1 with p2 (atomic supersede+page); the superseded id must
    //     never surface again, even though its vector is still indexed.
    let (p2, superseded) = log
        .emit_page(
            topic,
            "Kenny",
            "acme corp dossier kenny summary v2",
            &[json!({ "text": "Kenny now leads Acme.", "cites": [mem.clone()] })],
            &[],
            TEST_PRODUCER,
            std::slice::from_ref(&mem),
            Some(&p1),
        )
        .unwrap();
    assert!(superseded, "emit_page with a prior id must report a supersede");
    reindex_and_refold(&log, &embedder);

    let hits = log
        .recall(&embedder, query, RECALL_TOP_K, &RecallOptions::default())
        .unwrap();
    assert!(
        find_hit(&hits, &p2).is_some(),
        "the new current page must surface; hits={hits:?}"
    );
    assert!(
        find_hit(&hits, &p1).is_none(),
        "the SUPERSEDED page id must never surface, even with its vector indexed; hits={hits:?}"
    );

    // (c) exclude_pages drops every page-kind hit, but raw memories remain.
    let opts = RecallOptions { exclude_pages: true, ..Default::default() };
    let hits = log.recall(&embedder, query, RECALL_TOP_K, &opts).unwrap();
    assert!(
        !hits.iter().any(|h| h.kind == PAGE_KIND),
        "exclude_pages must hide ALL page hits; hits={hits:?}"
    );
    assert!(
        find_hit(&hits, &mem).is_some(),
        "exclude_pages must still surface raw memories; hits={hits:?}"
    );
}

/// Ordering invariant (§7 / F2): the superseded-page filter runs BEFORE
/// `truncate(k)`, so a superseded page that would rank #1 cannot crowd a valid
/// memory out of a `k == 1` result. The page text is the bare query phrase (zero
/// noise → top bag-of-words cosine → rank #1); the memory carries the phrase plus
/// noise (lower cosine → rank #2). Once the page is superseded, recall at k=1 must
/// still return the memory — proving the drop precedes the truncate, not follows.
#[test]
fn superseded_page_at_rank_one_does_not_crowd_out_a_valid_memory() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let log = EventLog::open(&dir.path().join("m.db"), &DEK, key).unwrap();
    let embedder = MockEmbedder::new(MID_DIM);

    // A distinctive phrase so nothing else in the corpus collides on it.
    let query = "zeta zeta zeta";
    let topic = "entity:01TOPICZETA";

    // Memory: query phrase + noise → present in both arms, ranks BELOW a zero-noise
    // match.
    let mem = log
        .append(mk_memory_event("zeta zeta zeta noiseone noisetwo noisethree"))
        .unwrap();

    // Page: EXACTLY the query phrase → zero trailing noise → highest cosine → it
    // would rank #1 if it were allowed to surface.
    let p1 = log
        .page(
            topic,
            "Zeta",
            "zeta zeta zeta",
            &[json!({ "text": "Zeta fact.", "cites": [mem.clone()] })],
            &[],
            TEST_PRODUCER,
            std::slice::from_ref(&mem),
        )
        .unwrap();
    log.rederive_pending(&embedder).unwrap();
    log.rebuild_indexes(&embedder).unwrap();
    log.rebuild_graph().unwrap();

    // Sanity: while current, the page DOES rank #1 at k=2 (above the noisier
    // memory) — establishing it would occupy the single slot at k=1.
    let pre = log
        .recall(&embedder, query, 2, &RecallOptions::default())
        .unwrap();
    assert_eq!(
        rank_of(&pre, &p1),
        Some(0),
        "the zero-noise page must rank #1 while current; pre={pre:?}"
    );

    // Supersede the page (orphan supersede is benign here, F9) → it leaves the
    // pages projection but its vector stays in the index.
    log.supersede(&p1, TEST_PRODUCER, std::slice::from_ref(&mem)).unwrap();
    log.rebuild_graph().unwrap();
    assert!(
        log.current_pages().unwrap().is_empty(),
        "after supersede with no replacement, the topic has no current page"
    );

    // k == 1: the superseded #1 page must be filtered BEFORE truncate, so the
    // rank-2 memory survives as the single returned hit.
    let hits = log.recall(&embedder, query, 1, &RecallOptions::default()).unwrap();
    assert_eq!(hits.len(), 1, "k=1 must yield exactly one hit; hits={hits:?}");
    assert_eq!(
        &hits[0].event_id, &mem,
        "the superseded #1 page must not crowd out the valid memory at k=1; hits={hits:?}"
    );
    assert!(
        find_hit(&hits, &p1).is_none(),
        "the superseded page must never appear; hits={hits:?}"
    );
}
