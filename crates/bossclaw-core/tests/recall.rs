use bossclaw_core::embed::{Embedder, MockEmbedder};
use bossclaw_core::model2vec::Model2Vec;
use bossclaw_core::event::Event;
use bossclaw_core::log::EventLog;
use bossclaw_core::BossclawError;
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
    let v1 = embedder.embed(&[text.clone()]).unwrap();
    let v2 = embedder.embed(&[text]).unwrap();
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
