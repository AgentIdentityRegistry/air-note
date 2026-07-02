// Copied from apps/desktop/src-tauri/src/engine/embed.rs (M1a Task 4); the in-app original is removed in Task 6.
// The model dir is supplied to `ResourceModel2Vec::new` by the caller — in the daemon it comes
// from `BOSSCLAWD_MODEL_DIR` / the install path (NOT a Tauri `resource_dir`); this module is unchanged.

//! The embedder seam: a provider that yields the real `Model2Vec` (loaded
//! from the bundled model resource, lazily + cached) in production and a
//! `MockEmbedder` in tests.

use crate::engine::EngineOpError;
use bossclaw_core::{Embedder, Model2Vec};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// The single source of truth for the active embedding model id. Ingest and recall-open
/// construct `Model2Vec` with THIS id, so the vectors written and the index rebuilt match.
pub const MODEL_ID: &str = "minishlab/potion-base-8M";

/// Builds (and caches) the embedder. Called on first ingest, never at startup.
pub trait EmbedderProvider: Send + Sync {
    fn embedder(&self) -> Result<Arc<dyn Embedder>, EngineOpError>;
}

/// Production provider: loads `Model2Vec` from the bundled model directory on
/// first use and caches it for the process lifetime.
pub struct ResourceModel2Vec {
    model_dir: PathBuf,
    cell: Mutex<Option<Arc<dyn Embedder>>>,
}

impl ResourceModel2Vec {
    pub fn new(model_dir: PathBuf) -> Self {
        Self { model_dir, cell: Mutex::new(None) }
    }
}

impl EmbedderProvider for ResourceModel2Vec {
    fn embedder(&self) -> Result<Arc<dyn Embedder>, EngineOpError> {
        let mut guard = self.cell.lock().expect("embedder cell poisoned");
        if let Some(e) = guard.as_ref() {
            return Ok(e.clone());
        }
        let model = Model2Vec::from_pretrained(&self.model_dir, MODEL_ID)
            .map_err(|e| EngineOpError::Embedder(e.to_string()))?;
        let arc: Arc<dyn Embedder> = Arc::new(model);
        *guard = Some(arc.clone());
        Ok(arc)
    }
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
