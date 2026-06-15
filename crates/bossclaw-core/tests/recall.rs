use bossclaw_core::embed::{Embedder, MockEmbedder};
use bossclaw_core::event::Event;
use bossclaw_core::log::EventLog;
use ed25519_dalek::SigningKey;
use serde_json::json;

const DEK: [u8; 32] = [42u8; 32];
const KEY_BYTES: [u8; 32] = [7u8; 32];

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
    log.append(mk_config_event("a", 64, 1)).unwrap();
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
    let model = log.active_model().unwrap().expect("should still have active model");
    assert_eq!(model.active_model_id, "a");
    assert_eq!(model.dim, 64);
}

// --- MockEmbedder tests ---

#[test]
fn mock_embedder_returns_vectors_of_correct_dim() {
    let embedder = MockEmbedder::new(64);
    let vecs = embedder.embed(&["hello world".to_string()]).unwrap();
    assert_eq!(vecs.len(), 1);
    assert_eq!(vecs[0].len(), 64);
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
    let embedder = MockEmbedder::new(64);
    let vecs = embedder.embed(&["hello world foo bar".to_string()]).unwrap();
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
    let e2 = MockEmbedder::new(64);
    // model_id is "mock-v1" regardless of dim
    assert_eq!(e1.model_id(), "mock-v1");
    assert_eq!(e2.model_id(), "mock-v1");
}

#[test]
fn mock_embedder_batch_embed() {
    let embedder = MockEmbedder::new(16);
    let texts = vec!["foo".to_string(), "bar".to_string(), "baz".to_string()];
    let vecs = embedder.embed(&texts).unwrap();
    assert_eq!(vecs.len(), 3);
    for v in &vecs {
        assert_eq!(v.len(), 16);
    }
}

#[test]
fn mock_embedder_different_texts_produce_different_vectors() {
    let embedder = MockEmbedder::new(64);
    let v1 = embedder.embed(&["alpha beta".to_string()]).unwrap();
    let v2 = embedder.embed(&["gamma delta".to_string()]).unwrap();
    // Highly unlikely to collide with distinct tokens
    assert_ne!(v1[0], v2[0], "distinct input should produce distinct output");
}
