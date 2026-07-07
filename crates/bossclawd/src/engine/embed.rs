//! The embedder seam: the production provider resolves WHICH model to load itself (invariant I1) —
//! env override (dev/harness) → the signed `language_pack` record → the bundled English default —
//! sha-verifies the signed multilingual model at load (I4), fails loud on missing/mismatch (I3),
//! and holds a SWAPPABLE cache so a consent-gated migration can flip the served model without a
//! daemon restart. The simple `ResourceModel2Vec::new(dir)` constructor is preserved for memharness
//! + tests (a fixed dir, no resolution). A `MockEmbedder` provider backs hermetic tests.

use crate::engine::EngineOpError;
use bossclaw_core::{Embedder, EventLog, MigrationState, Model2Vec};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The bundled English default model id (invariant I7: the default path reports THIS id and writes
/// identical vectors). Was the sole hardcoded seam pre-rung-2; now the default of a resolution order.
pub const MODEL_ID: &str = "minishlab/potion-base-8M";

/// The local id-binding file the downloader writes and the resolver reads (invariant I4).
const ID_BINDING_FILE: &str = "air-model.json";
/// The weights filename whose sha256 is the identity guard.
const SAFETENSORS_FILE: &str = "model.safetensors";

/// The loaded-vs-intended state surfaced to the UI (mirrors `ModelStateWire`; invariants I3/U5/U6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelState {
    /// The resolved model loaded and its sha matches the signed record (or the default is serving).
    Ok,
    /// The signed record names a model whose files are absent — recall refuses (I3).
    Missing {
        /// The model id the signed record expected to find on disk.
        expected: String,
    },
    /// The resolved model's weights failed their sha256 integrity check — recall refuses (I4).
    Mismatch {
        /// The model id the signed record expected.
        expected: String,
        /// The sha256 actually found on disk.
        loaded: String,
    },
}

/// A pluggable model loader (production = `Model2Vec::from_pretrained`; tests inject a mock). Takes
/// the physical dir + the id to report, returns a built embedder.
pub type LoaderFn =
    Arc<dyn Fn(&Path, &str) -> Result<Arc<dyn Embedder>, EngineOpError> + Send + Sync>;

/// Builds (and caches) the embedder. Called on first ingest/recall, never at startup.
pub trait EmbedderProvider: Send + Sync {
    /// Legacy build with no resolution context (mocks + the fixed-dir constructor).
    fn embedder(&self) -> Result<Arc<dyn Embedder>, EngineOpError>;

    /// Resolution-aware build (production): reads the signed `language_pack` record from `log` to
    /// decide which model to load. Default: ignore `log`, delegate to [`Self::embedder`].
    fn embedder_for(&self, _log: &EventLog) -> Result<Arc<dyn Embedder>, EngineOpError> {
        self.embedder()
    }

    /// Build the migration TARGET model (load `<root>/<model_id>` + verify `sha`) WITHOUT publishing
    /// it to the live cache. Default: unsupported (mocks don't migrate).
    fn build_candidate(&self, _model_id: &str, _sha: &str) -> Result<Arc<dyn Embedder>, EngineOpError> {
        Err(EngineOpError::Embedder("this provider does not support model swap".into()))
    }

    /// Atomically replace the live cache with a candidate built by [`Self::build_candidate`]. Default: no-op.
    fn publish(&self, _embedder: Arc<dyn Embedder>) {}

    /// The current loaded-vs-intended state (default `Ok`; the production provider tracks the real one).
    fn model_state(&self) -> ModelState {
        ModelState::Ok
    }

    /// Set the live re-index progress (`(done, total)`), for `ModelStatus`. Default: no-op.
    fn set_reindex(&self, _progress: Option<(u64, u64)>) {}

    /// Read the live re-index progress (`(done, total)`), for `ModelStatus`. Default: `None`.
    fn reindex(&self) -> Option<(u64, u64)> {
        None
    }
}

/// Production embedder provider.
pub struct ResourceModel2Vec {
    /// Resolution inputs (`None` env in the fixed-dir constructor; the fixed dir goes in `bundled_dir`).
    env_override: Option<PathBuf>,
    bundled_dir: PathBuf,
    bundled_id: String,
    data_models_root: PathBuf,
    /// When `true`, `embedder_for` ignores `log` and always loads `bundled_dir` as `bundled_id`
    /// (the `new(dir)` fixed-dir mode memharness uses). When `false`, full resolution runs.
    fixed: bool,
    loader: LoaderFn,
    cell: Mutex<Option<Arc<dyn Embedder>>>,
    state: Mutex<ModelState>,
    reindex: Mutex<Option<(u64, u64)>>,
}

impl ResourceModel2Vec {
    /// FIXED-DIR constructor (memharness + simple callers): always loads `dir` as [`MODEL_ID`], no
    /// signed-record resolution, no swap. Byte-for-byte the pre-rung-2 behaviour.
    pub fn new(model_dir: PathBuf) -> Self {
        Self {
            env_override: None,
            bundled_dir: model_dir.clone(),
            bundled_id: MODEL_ID.to_string(),
            data_models_root: model_dir,
            fixed: true,
            loader: default_loader(),
            cell: Mutex::new(None),
            state: Mutex::new(ModelState::Ok),
            reindex: Mutex::new(None),
        }
    }

    /// RESOLUTION-AWARE constructor (the daemon `main.rs`): env override → signed record → bundled.
    pub fn with_resolution(
        env_override: Option<PathBuf>,
        bundled_dir: PathBuf,
        data_models_root: PathBuf,
        bundled_id: String,
    ) -> Self {
        Self {
            env_override,
            bundled_dir,
            bundled_id,
            data_models_root,
            fixed: false,
            loader: default_loader(),
            cell: Mutex::new(None),
            state: Mutex::new(ModelState::Ok),
            reindex: Mutex::new(None),
        }
    }

    /// TEST-ONLY: inject a loader so resolution can be tested without real weights.
    #[cfg(test)]
    pub fn with_loader_for_test(mut self, loader: LoaderFn) -> Self {
        self.loader = loader;
        self
    }

    /// Resolve `(dir, id)` to load, or a fail-loud `ModelState`. Env highest (I1); else the signed
    /// record when `Complete` (verify sha, I3/I4); else the bundled default (I7). An `InProgress`
    /// record keeps serving the bundled/old model until the migration flips it to `Complete`.
    fn resolve(&self, log: &EventLog) -> Result<(PathBuf, String), EngineOpError> {
        if let Some(env) = &self.env_override {
            let id = read_binding_id(env).unwrap_or_else(|| self.bundled_id.clone());
            return Ok((env.clone(), id));
        }
        let rec = log
            .language_pack_record()
            .map_err(|e| EngineOpError::Embedder(e.to_string()))?;
        match rec {
            Some(r) if r.migration == MigrationState::Complete => {
                let dir = self.data_models_root.join(&r.model_id);
                let weights = dir.join(SAFETENSORS_FILE);
                if !weights.is_file() {
                    *self.state.lock().expect("model state poisoned") =
                        ModelState::Missing { expected: r.model_id.clone() };
                    return Err(EngineOpError::Embedder(format!(
                        "language pack '{}' is enabled but its files are missing — re-download",
                        r.model_id
                    )));
                }
                let actual = sha256_file(&weights).map_err(EngineOpError::Embedder)?;
                if actual != r.safetensors_sha {
                    *self.state.lock().expect("model state poisoned") = ModelState::Mismatch {
                        expected: r.model_id.clone(),
                        loaded: actual,
                    };
                    return Err(EngineOpError::Embedder(format!(
                        "language pack '{}' failed its integrity check — re-download",
                        r.model_id
                    )));
                }
                *self.state.lock().expect("model state poisoned") = ModelState::Ok;
                Ok((dir, r.model_id))
            }
            // InProgress (old model still serving) OR no record → bundled English default.
            _ => {
                *self.state.lock().expect("model state poisoned") = ModelState::Ok;
                Ok((self.bundled_dir.clone(), self.bundled_id.clone()))
            }
        }
    }
}

impl EmbedderProvider for ResourceModel2Vec {
    fn embedder(&self) -> Result<Arc<dyn Embedder>, EngineOpError> {
        // Fixed-dir mode: no log needed.
        let mut guard = self.cell.lock().expect("embedder cell poisoned");
        if let Some(e) = guard.as_ref() {
            return Ok(e.clone());
        }
        let e = (self.loader)(&self.bundled_dir, &self.bundled_id)?;
        *guard = Some(e.clone());
        Ok(e)
    }

    fn embedder_for(&self, log: &EventLog) -> Result<Arc<dyn Embedder>, EngineOpError> {
        if self.fixed {
            return self.embedder();
        }
        {
            let guard = self.cell.lock().expect("embedder cell poisoned");
            if let Some(e) = guard.as_ref() {
                return Ok(e.clone());
            }
        }
        let (dir, id) = self.resolve(log)?; // sets ModelState + returns Err on missing/mismatch (I3)
        let e = (self.loader)(&dir, &id)?;
        *self.cell.lock().expect("embedder cell poisoned") = Some(e.clone());
        Ok(e)
    }

    fn build_candidate(&self, model_id: &str, sha: &str) -> Result<Arc<dyn Embedder>, EngineOpError> {
        let dir = self.data_models_root.join(model_id);
        let weights = dir.join(SAFETENSORS_FILE);
        if !weights.is_file() {
            return Err(EngineOpError::Embedder(format!("model '{model_id}' files are missing")));
        }
        let actual = sha256_file(&weights).map_err(EngineOpError::Embedder)?;
        if actual != sha {
            return Err(EngineOpError::Embedder(format!("model '{model_id}' failed its integrity check")));
        }
        (self.loader)(&dir, model_id)
    }

    fn publish(&self, embedder: Arc<dyn Embedder>) {
        *self.cell.lock().expect("embedder cell poisoned") = Some(embedder);
        *self.state.lock().expect("model state poisoned") = ModelState::Ok;
    }

    fn model_state(&self) -> ModelState {
        self.state.lock().expect("model state poisoned").clone()
    }

    fn set_reindex(&self, progress: Option<(u64, u64)>) {
        *self.reindex.lock().expect("reindex poisoned") = progress;
    }

    fn reindex(&self) -> Option<(u64, u64)> {
        *self.reindex.lock().expect("reindex poisoned")
    }
}

/// The production loader: `Model2Vec::from_pretrained` (unchanged behaviour for the fixed path).
fn default_loader() -> LoaderFn {
    Arc::new(|dir: &Path, id: &str| {
        Model2Vec::from_pretrained(dir, id)
            .map(|m| Arc::new(m) as Arc<dyn Embedder>)
            .map_err(|e| EngineOpError::Embedder(e.to_string()))
    })
}

/// sha256 a file, hex-encoded (matches the downloader's `air-model.json` binding).
fn sha256_file(path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    Ok(hex::encode(Sha256::digest(&bytes)))
}

/// Read the `model_id` from a folder's `air-model.json`, if present (used for the env-override id).
fn read_binding_id(dir: &Path) -> Option<String> {
    let raw = std::fs::read(dir.join(ID_BINDING_FILE)).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    v.get("model_id").and_then(|s| s.as_str()).map(str::to_string)
}

#[cfg(test)]
pub struct MockEmbedderProvider {
    dim: usize,
}

#[cfg(test)]
impl MockEmbedderProvider {
    pub fn new(dim: usize) -> Self {
        Self { dim }
    }
}

#[cfg(test)]
impl EmbedderProvider for MockEmbedderProvider {
    fn embedder(&self) -> Result<Arc<dyn Embedder>, EngineOpError> {
        Ok(Arc::new(bossclaw_core::MockEmbedder::new(self.dim)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bossclaw_core::{EventLog, LanguagePackRecord, MigrationState, MockEmbedder};
    use ed25519_dalek::SigningKey;
    use std::sync::Arc;

    const DEK: [u8; 32] = [42u8; 32];
    const KEY: [u8; 32] = [7u8; 32];

    fn open_log(dir: &std::path::Path) -> EventLog {
        EventLog::open(&dir.join("m.db"), &DEK, SigningKey::from_bytes(&KEY)).unwrap()
    }

    /// A loader that yields a MockEmbedder reporting the requested id, so resolution can be tested
    /// without real weights. It also records which (dir, id) it was asked to load.
    fn mock_loader() -> LoaderFn {
        Arc::new(|_dir: &std::path::Path, id: &str| {
            Ok(Arc::new(MockEmbedder::new(8)) as Arc<dyn bossclaw_core::Embedder>).map(|e| {
                // Wrap so model_id() reports the RESOLVED id, not mock-v1.
                Arc::new(IdOverride { inner: e, id: id.to_string() }) as Arc<dyn bossclaw_core::Embedder>
            })
        })
    }

    struct IdOverride { inner: Arc<dyn bossclaw_core::Embedder>, id: String }
    impl bossclaw_core::Embedder for IdOverride {
        fn embed(&self, t: &[String]) -> Result<Vec<Vec<f32>>, bossclaw_core::BossclawError> { self.inner.embed(t) }
        fn dim(&self) -> usize { self.inner.dim() }
        fn model_id(&self) -> &str { &self.id }
    }

    /// Write a valid model folder (fake safetensors + air-model.json) under `root/<id>` and return
    /// its sha256 so the signed record can bind to it.
    fn stage_model(root: &std::path::Path, id: &str, bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("model.safetensors"), bytes).unwrap();
        let sha = hex::encode(Sha256::digest(bytes));
        std::fs::write(
            dir.join("air-model.json"),
            serde_json::to_vec(&serde_json::json!({ "model_id": id, "safetensors_sha": sha })).unwrap(),
        )
        .unwrap();
        sha
    }

    #[test]
    fn no_record_resolves_bundled_english_default_i7() {
        let tmp = tempfile::tempdir().unwrap();
        let log = open_log(tmp.path());
        let bundled = tmp.path().join("models/potion-base-8M");
        std::fs::create_dir_all(&bundled).unwrap();
        let p = ResourceModel2Vec::with_resolution(
            None, bundled.clone(), tmp.path().join("models"), MODEL_ID.to_string(),
        )
        .with_loader_for_test(mock_loader());
        let e = p.embedder_for(&log).unwrap();
        assert_eq!(e.model_id(), MODEL_ID, "no record → bundled English id (I7)");
        assert_eq!(p.model_state(), bossclaw_core_model_state_ok());
    }

    #[test]
    fn env_override_is_highest_priority() {
        let tmp = tempfile::tempdir().unwrap();
        let log = open_log(tmp.path());
        let envdir = tmp.path().join("envmodel");
        std::fs::create_dir_all(&envdir).unwrap();
        let p = ResourceModel2Vec::with_resolution(
            Some(envdir.clone()), tmp.path().join("models/potion-base-8M"),
            tmp.path().join("models"), MODEL_ID.to_string(),
        )
        .with_loader_for_test(mock_loader());
        // Even with a Complete multilingual record present, env wins (dev/harness override, I1).
        let sha = stage_model(&tmp.path().join("models"), "ml/v1", b"weights");
        log.set_language_pack_record(&LanguagePackRecord {
            model_id: "ml/v1".into(), safetensors_sha: sha, migration: MigrationState::Complete,
            consented_at: "t".into(),
        }).unwrap();
        let e = p.embedder_for(&log).unwrap();
        assert_eq!(e.model_id(), MODEL_ID, "env override wins over the signed record (I1)");
    }

    #[test]
    fn complete_record_loads_multilingual_when_sha_matches() {
        let tmp = tempfile::tempdir().unwrap();
        let log = open_log(tmp.path());
        let root = tmp.path().join("models");
        let sha = stage_model(&root, "ml/v1", b"weights");
        log.set_language_pack_record(&LanguagePackRecord {
            model_id: "ml/v1".into(), safetensors_sha: sha, migration: MigrationState::Complete,
            consented_at: "t".into(),
        }).unwrap();
        let p = ResourceModel2Vec::with_resolution(None, root.join("potion-base-8M"), root, MODEL_ID.to_string())
            .with_loader_for_test(mock_loader());
        assert_eq!(p.embedder_for(&log).unwrap().model_id(), "ml/v1");
    }

    #[test]
    fn complete_record_missing_folder_fails_loud_i3() {
        let tmp = tempfile::tempdir().unwrap();
        let log = open_log(tmp.path());
        let root = tmp.path().join("models");
        std::fs::create_dir_all(&root).unwrap();
        log.set_language_pack_record(&LanguagePackRecord {
            model_id: "ml/gone".into(), safetensors_sha: "abc".into(),
            migration: MigrationState::Complete, consented_at: "t".into(),
        }).unwrap();
        let p = ResourceModel2Vec::with_resolution(None, root.join("potion-base-8M"), root, MODEL_ID.to_string())
            .with_loader_for_test(mock_loader());
        // `Arc<dyn Embedder>` is not `Debug`, so `unwrap_err()` won't compile — match explicitly.
        let err = match p.embedder_for(&log) {
            Err(e) => e,
            Ok(_) => panic!("missing folder must refuse (I3)"),
        };
        assert!(matches!(err, EngineOpError::Embedder(_)), "missing folder must refuse (I3): {err:?}");
        assert!(matches!(p.model_state(), ModelState::Missing { .. }), "state reflects Missing");
    }

    #[test]
    fn complete_record_sha_mismatch_fails_loud_i4() {
        let tmp = tempfile::tempdir().unwrap();
        let log = open_log(tmp.path());
        let root = tmp.path().join("models");
        stage_model(&root, "ml/v1", b"real-weights"); // real sha
        log.set_language_pack_record(&LanguagePackRecord {
            model_id: "ml/v1".into(), safetensors_sha: "WRONGSHA".into(),
            migration: MigrationState::Complete, consented_at: "t".into(),
        }).unwrap();
        let p = ResourceModel2Vec::with_resolution(None, root.join("potion-base-8M"), root, MODEL_ID.to_string())
            .with_loader_for_test(mock_loader());
        // `Arc<dyn Embedder>` is not `Debug`, so `unwrap_err()` won't compile — match explicitly.
        let err = match p.embedder_for(&log) {
            Err(e) => e,
            Ok(_) => panic!("sha mismatch must refuse (I4)"),
        };
        assert!(matches!(err, EngineOpError::Embedder(_)), "sha mismatch must refuse (I4): {err:?}");
        assert!(matches!(p.model_state(), ModelState::Mismatch { .. }));
    }

    #[test]
    fn in_progress_record_serves_bundled_old_model() {
        let tmp = tempfile::tempdir().unwrap();
        let log = open_log(tmp.path());
        let root = tmp.path().join("models");
        let bundled = root.join("potion-base-8M");
        std::fs::create_dir_all(&bundled).unwrap();
        stage_model(&root, "ml/v1", b"weights");
        log.set_language_pack_record(&LanguagePackRecord {
            model_id: "ml/v1".into(), safetensors_sha: "x".into(),
            migration: MigrationState::InProgress, consented_at: "t".into(),
        }).unwrap();
        let p = ResourceModel2Vec::with_resolution(None, bundled, root, MODEL_ID.to_string())
            .with_loader_for_test(mock_loader());
        assert_eq!(p.embedder_for(&log).unwrap().model_id(), MODEL_ID,
            "during migration the OLD English model still serves");
    }

    fn bossclaw_core_model_state_ok() -> ModelState { ModelState::Ok }
}
