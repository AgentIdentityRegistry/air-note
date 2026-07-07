//! Rung-2 Phase A integration gate: the language-pack SWAP MACHINERY, driven end-to-end through a
//! real in-process `EngineHandle` (the `test_engine_with_embedder` seam the memharness uses), with
//! MOCK embedders only — no real 500 MB weights are downloaded or required. The frozen quality A/B
//! (+21 ko / −5 en, `air/rung2-multilingual-measurement-2026-07-06`) is a property of the MODEL and
//! is REUSED, not re-run here; these tests prove only that the machinery produces the new-model
//! vectors, GCs the old ones (`vectors` AND `entity_vectors`), keeps search working in both
//! languages, resumes an interrupted migration, and fails loud when the signed model is missing.
//!
//! ## Why a custom provider instead of the plan's `ResourceModel2Vec::with_loader_for_test`
//!
//! The plan sketch wired the production `ResourceModel2Vec` with `.with_loader_for_test(...)` and
//! reached into `handle.embedder_provider`. Neither is reachable from an INTEGRATION test:
//! `with_loader_for_test` and `MockEmbedderProvider` are `#[cfg(test)]`-only (compiled solely for
//! the crate's own unit tests, not for the `test-helpers`-featured lib an integration test links),
//! and `embedder_provider` is a private field. So this file supplies its own resolution-aware mock
//! that implements the PUBLIC [`EmbedderProvider`] trait (the same seam `roundtrip.rs`'s
//! `WideMockEmbedderProvider` uses). The production resolver's fail-loud/sha paths are already
//! covered by the crate's inline unit tests (`embed.rs::complete_record_missing_folder_fails_loud_i3`,
//! `mod.rs::recall_refuses_loudly_when_signed_model_missing`); this gate exercises the daemon-level
//! enable → migrate → atomic-flip → recall flow those unit tests do not drive at once.
//!
//! Unix-only (the engine + daemon are Unix-only).
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use bossclaw_core::{
    Embedder, Event, EventLog, LanguagePackRecord, MigrationState, MockEmbedder,
};
use bossclawd::engine::embed::{EmbedderProvider, ModelState, MODEL_ID};
use bossclawd::engine::{EngineHandle, EngineOpError};
use bossclawd::server::test_engine_with_embedder;
use sha2::{Digest, Sha256};

/// Embedder dimension the mocks use. Small (fast), matches the crate's other daemon migration tests.
const MOCK_DIM: usize = 8;
/// The multilingual language-pack id these tests enable (the real rung-2 target).
const ML_ID: &str = "minishlab/potion-multilingual-128M";

/// A `MockEmbedder` that reports a caller-chosen `model_id`, so a migration can re-embed rows under
/// the resolved id without real weights (mirrors the crate's inline `IdReportingEmbedder`).
struct IdMock {
    inner: MockEmbedder,
    id: String,
}

impl Embedder for IdMock {
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, bossclaw_core::BossclawError> {
        self.inner.embed(texts)
    }
    fn dim(&self) -> usize {
        self.inner.dim()
    }
    fn model_id(&self) -> &str {
        &self.id
    }
}

/// Build a dim-`MOCK_DIM` embedder reporting `id`.
fn mk_embedder(id: &str) -> Arc<dyn Embedder> {
    Arc::new(IdMock { inner: MockEmbedder::new(MOCK_DIM), id: id.to_string() })
}

/// A resolution-aware mock provider: serves the bundled English model ([`MODEL_ID`]) until a
/// migration `publish`es a candidate, which atomically swaps the live cache (exactly as the
/// production `ResourceModel2Vec` does). `build_candidate` verifies the staged folder + sha256 so a
/// missing/mismatched language pack fails loud through `set_active_model` (I3/I4).
struct MockLangProvider {
    /// Root under which downloadable model folders are staged (`<root>/<id>/model.safetensors`).
    models_root: PathBuf,
    /// The live embedder cache — first resolve populates it; `publish` swaps it (resolve-once).
    cell: Mutex<Option<Arc<dyn Embedder>>>,
    state: Mutex<ModelState>,
    reindex: Mutex<Option<(u64, u64)>>,
}

impl MockLangProvider {
    fn new(models_root: PathBuf) -> Self {
        Self {
            models_root,
            cell: Mutex::new(None),
            state: Mutex::new(ModelState::Ok),
            reindex: Mutex::new(None),
        }
    }
}

impl EmbedderProvider for MockLangProvider {
    fn embedder(&self) -> Result<Arc<dyn Embedder>, EngineOpError> {
        let mut guard = self.cell.lock().expect("cell poisoned");
        if let Some(e) = guard.as_ref() {
            return Ok(e.clone());
        }
        let e = mk_embedder(MODEL_ID); // no record yet → bundled English default (I7)
        *guard = Some(e.clone());
        Ok(e)
    }

    fn embedder_for(&self, _log: &EventLog) -> Result<Arc<dyn Embedder>, EngineOpError> {
        // Resolve-once, publish-swappable: before any migration the English model serves; after the
        // migration's `publish`, the cell holds the multilingual candidate and is returned here.
        self.embedder()
    }

    fn build_candidate(&self, model_id: &str, sha: &str) -> Result<Arc<dyn Embedder>, EngineOpError> {
        let weights = self.models_root.join(model_id).join("model.safetensors");
        if !weights.is_file() {
            *self.state.lock().expect("state poisoned") =
                ModelState::Missing { expected: model_id.to_string() };
            return Err(EngineOpError::Embedder(format!(
                "language pack '{model_id}' files are missing — re-download"
            )));
        }
        let bytes = std::fs::read(&weights).map_err(|e| EngineOpError::Embedder(e.to_string()))?;
        let actual = hex::encode(Sha256::digest(&bytes));
        if actual != sha {
            *self.state.lock().expect("state poisoned") =
                ModelState::Mismatch { expected: model_id.to_string(), loaded: actual };
            return Err(EngineOpError::Embedder(format!(
                "language pack '{model_id}' failed its integrity check — re-download"
            )));
        }
        Ok(mk_embedder(model_id))
    }

    fn publish(&self, embedder: Arc<dyn Embedder>) {
        *self.cell.lock().expect("cell poisoned") = Some(embedder);
        *self.state.lock().expect("state poisoned") = ModelState::Ok;
    }

    fn model_state(&self) -> ModelState {
        self.state.lock().expect("state poisoned").clone()
    }

    fn set_failed(&self, reason: String) {
        *self.state.lock().expect("state poisoned") = ModelState::Failed { reason };
    }

    fn set_reindex(&self, progress: Option<(u64, u64)>) {
        *self.reindex.lock().expect("reindex poisoned") = progress;
    }

    fn reindex(&self) -> Option<(u64, u64)> {
        *self.reindex.lock().expect("reindex poisoned")
    }
}

/// Stage a downloadable model folder under `root/<id>` (fake weights) and return `(id, sha256)` so a
/// signed record / `build_candidate` can bind + verify it.
fn stage_mock_model(root: &Path, id: &str) -> (String, String) {
    let dir = root.join(id);
    std::fs::create_dir_all(&dir).unwrap();
    let bytes = b"mock-weights";
    std::fs::write(dir.join("model.safetensors"), bytes).unwrap();
    let sha = hex::encode(Sha256::digest(bytes));
    (id.to_string(), sha)
}

/// One `memory` event (what the embed/recall pipeline consumes).
fn mk_test_memory(text: &str) -> Event {
    Event {
        id: String::new(),
        ts: String::new(),
        valid_time: None,
        event_type: "memory".to_string(),
        content: serde_json::json!({ "text": text }),
        model_meta: None,
        prev_hash: String::new(),
        hash: None,
        signed_by_did: "did:wba:AIR-TEST".to_string(),
        signature: None,
    }
}

/// A fresh onboarded handle wired with the resolution-aware mock provider over a temp home whose
/// `models/` root is where language packs are staged. Returns `(handle, provider, home_guard,
/// models_root)`. The `provider` clone lets a test read the English embedder (to seed an entity
/// vector under the OLD id); the `home_guard` tempdir must stay in scope for the test's lifetime.
async fn onboarded_handle() -> (Arc<EngineHandle>, Arc<MockLangProvider>, tempfile::TempDir, PathBuf) {
    // Seed the process-global provider-key cache EMPTY so nothing can reach the OS keychain (the
    // documented CI-hang hazard); the mirror of every `roundtrip.rs` daemon test.
    bossclawd::vault::seed_secret_cache_for_test(Default::default());
    let home = tempfile::tempdir().unwrap();
    let models_root = home.path().join("models");
    std::fs::create_dir_all(&models_root).unwrap();
    let provider = Arc::new(MockLangProvider::new(models_root.clone()));
    let handle =
        Arc::new(test_engine_with_embedder(home.path().to_path_buf(), provider.clone()));
    (handle, provider, home, models_root)
}

/// Poll until the WHOLE migration has finished for `expected_id` (bounded): the signed record flips
/// to `Complete` at the commit point, but GC + rebuild run AFTER it and the provider clears its
/// re-index progress to `None` only once the whole migration finishes — gate on both so a half-done
/// state (record Complete, old rows not yet GC'd) is never observed. Mirrors the crate's inline
/// `wait_until_active`.
async fn wait_until_active(handle: &Arc<EngineHandle>, expected_id: &str) {
    for _ in 0..200 {
        let rec = handle.get_or_open(true).await.unwrap().language_pack_record().unwrap();
        let complete = matches!(&rec, Some(r)
            if r.migration == MigrationState::Complete && r.model_id == expected_id);
        if complete && handle.model_state().1.is_none() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("migration did not complete within the bound");
}

/// The enable path end-to-end (U4/U8/I3/I5): `set_active_model` drives the all-or-nothing migration
/// to completion — every embeddable event AND the entity vector are re-embedded under the new id,
/// the old model's rows (`vectors` + `entity_vectors`) are GC'd, the signed record ends `Complete`,
/// and recall still returns hits in BOTH languages against the swapped model.
#[tokio::test]
async fn language_pack_enable_migrates_and_search_survives() {
    let (handle, provider, _home, models_root) = onboarded_handle().await;
    let (id, sha) = stage_mock_model(&models_root, ML_ID);

    let log = handle.get_or_open(true).await.unwrap();
    // Seed one English + one Korean memory so "search survives in both languages" is a real claim.
    // The query tokens appear verbatim in each body (MockEmbedder is a whitespace bag-of-words), so
    // both arms surface the doc deterministically regardless of ANN seed.
    let mut ids = Vec::new();
    for t in ["the ocean waves crash on the shore", "바다 파도 소리"] {
        ids.push(log.append(mk_test_memory(t)).unwrap());
    }
    handle.run_ingest(true).await.unwrap();

    // Seed an entity vector under the OLD (English) model so the entity-migration path (U8) is
    // exercised. The entity is backed by a real `entity` event (+ rebuild_graph) so it lands in the
    // `entities` projection the migration re-derives entity vectors from — a bare entity_vectors row
    // with no backing entity is a state production never produces (mirrors the core migration test).
    let english = provider.embedder_for(&log).unwrap();
    let aria = log
        .entity("Aria Novak", &[], "person", "m4-reasoner", std::slice::from_ref(&ids[0]))
        .unwrap();
    log.rebuild_graph().unwrap();
    log.derive_entity_vector(&*english, &aria, "Aria Novak").unwrap();
    assert_eq!(log.entity_vectors_for_model(MODEL_ID).unwrap().len(), 1, "entity vector seeded under the old id");

    handle.set_active_model(true, id.clone(), sha).await.unwrap();
    wait_until_active(&handle, &id).await;

    // One vector per embeddable event under the new id; old id + entity vectors GC'd (U8, I5).
    assert_eq!(log.vectors_for_model(&id).unwrap().len(), 2, "new-id vectors cover every event");
    assert!(log.vectors_for_model(MODEL_ID).unwrap().is_empty(), "old-id vectors GC'd");
    assert_eq!(log.entity_vectors_for_model(&id).unwrap().len(), 1, "entity vector migrated (U8)");
    assert!(log.entity_vectors_for_model(MODEL_ID).unwrap().is_empty(), "old entity vectors GC'd (U8)");
    assert_eq!(log.language_pack_record().unwrap().unwrap().migration, MigrationState::Complete);

    // Search still works in both languages (machinery, not quality — the mock embeds both). Assert
    // each query surfaces ITS OWN doc, not merely "some hit": both docs are indexed, so a bare
    // non-empty check could pass even if one language's re-embed were broken (the ANN would still
    // return the other doc). Matching the language-unique body token makes each arm load-bearing.
    let en = handle.recall(true, "ocean waves".into(), 5).await.unwrap();
    assert!(
        en.iter().any(|h| h.text.contains("ocean")),
        "English recall must surface the English memory after the swap, got {en:?}"
    );
    let ko = handle.recall(true, "바다 파도".into(), 5).await.unwrap();
    assert!(
        ko.iter().any(|h| h.text.contains("바다")),
        "Korean recall must surface the Korean memory after the swap, got {ko:?}"
    );
}

/// Boot resume (I6): a consented-but-interrupted migration (an `InProgress` record + old-model
/// vectors, NO new-model vectors) finishes on the next boot via the SAME all-or-nothing flow — the
/// signed record is the sole authority that resumes it.
///
/// Deviation from the crate's inline `interrupted_migration_resumes_on_boot`: that test uses two
/// handles sharing one vault to model a real process restart. An integration test cannot build a
/// second handle over the same in-memory vault (`TestVault` is crate-private), so this drives the
/// resume on the SAME handle after seeding the interrupted state directly — the resume LOGIC
/// (record read → migration re-run → `Complete`) is identical.
#[tokio::test]
async fn interrupted_migration_resumes_on_boot() {
    let (handle, _provider, _home, models_root) = onboarded_handle().await;
    let (id, sha) = stage_mock_model(&models_root, ML_ID);

    let log = handle.get_or_open(true).await.unwrap();
    log.append(mk_test_memory("resume me")).unwrap();
    handle.run_ingest(true).await.unwrap();
    // Simulate a crash MID-migration: consent recorded (InProgress), the re-embed never ran.
    log.set_language_pack_record(&LanguagePackRecord {
        model_id: id.clone(),
        safetensors_sha: sha,
        migration: MigrationState::InProgress,
        consented_at: "t".into(),
    })
    .unwrap();
    assert!(log.vectors_for_model(&id).unwrap().is_empty(), "no new-id vectors before resume");

    handle.resume_migration_if_pending(true).await;
    wait_until_active(&handle, &id).await;

    assert_eq!(log.language_pack_record().unwrap().unwrap().migration, MigrationState::Complete);
    assert_eq!(log.vectors_for_model(&id).unwrap().len(), 1, "resume finished the re-embed");
    assert!(log.vectors_for_model(MODEL_ID).unwrap().is_empty(), "old-id vectors GC'd on resume");
}

/// Fail-loud (I3): enabling a language pack whose files are absent must refuse LOUDLY and
/// synchronously — `set_active_model`'s pre-flight `build_candidate` fails before any record is
/// written, so nothing is half-committed (no `InProgress` marker a boot would then try to resume).
#[tokio::test]
async fn set_active_model_fails_loud_when_the_model_is_missing() {
    // The model folder is deliberately NOT staged.
    let (handle, _provider, _home, _models_root) = onboarded_handle().await;
    let log = handle.get_or_open(true).await.unwrap();

    let err = handle
        .set_active_model(true, ML_ID.into(), "deadbeef".into())
        .await
        .unwrap_err();
    assert!(
        matches!(err, EngineOpError::Embedder(_)),
        "a missing language pack → typed Embedder error, got {err:?}"
    );
    assert!(
        log.language_pack_record().unwrap().is_none(),
        "a failed enable writes NO signed record (nothing half-committed)"
    );
    assert!(
        matches!(handle.model_state().0, ModelState::Missing { .. }),
        "the provider surfaces Missing so the UI can prompt a re-download"
    );
}
