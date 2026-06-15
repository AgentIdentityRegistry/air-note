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
    // Calling twice (INSERT OR REPLACE) must not duplicate the row.
    assert!(log.derive_vector(&embedder, &event).unwrap());

    let vectors = log.vectors_for_model(MOCK_MODEL_ID).unwrap();
    assert_eq!(vectors.len(), 1, "upsert, not duplicate insert");
    assert_eq!(vectors[0].0, id);

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
    let opts = RecallOptions { pinned: vec![older.clone()] };
    let hits = log.recall(&embedder, text, RECALL_TOP_K, &opts).unwrap();
    let older_rank = rank_of(&hits, older).expect("older present");
    let newer_rank = rank_of(&hits, newer).expect("newer present");
    assert!(
        older_rank < newer_rank,
        "pinned older event (rank {older_rank}) must outrank unpinned newer (rank {newer_rank}); hits={hits:?}"
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
