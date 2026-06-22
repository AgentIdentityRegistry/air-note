//! The reasoner seam (mirrors engine/embed.rs): lazy, cached construction of the
//! local Ollama reasoner the evolve loop drives. A cloud reasoner can later drop in
//! behind this same trait with zero rework.
//! See docs/superpowers/specs/2026-06-23-sp3-recall-evolve-design.md.

use crate::engine::EngineOpError;
use bossclaw_core::Reasoner;
use std::sync::{Arc, Mutex};

/// Single source of truth for the evolve reasoner's Ollama model tag (mirrors
/// embed::MODEL_ID). Unpinned for SP3 (the user pulls it via `ollama pull`).
// Used by the Ollama detection probe + scheduler (SP3 Tasks 9–10) and by `reasoner()` below.
#[allow(dead_code)] // consumed at Task 9 (ollama_probe) / Task 10 (scheduler)
pub const REASONER_MODEL_ID: &str = "qwen2.5:7b-instruct";

/// Builds (and caches) the reasoner. Called on first evolve, never at startup.
pub trait ReasonerProvider: Send + Sync {
    // Called by `EngineHandle::evolve_once` (SP3 Task 7); the seam + impls land first.
    #[allow(dead_code)]
    fn reasoner(&self) -> Result<Arc<dyn Reasoner>, EngineOpError>;
}

/// Production provider: yields `bossclaw_core::OllamaReasoner` (loopback-fail-closed)
/// on first use and caches it for the process lifetime.
pub struct OllamaReasonerProvider {
    // Read by `reasoner()`, which `evolve_once` first calls at SP3 Task 7.
    #[allow(dead_code)]
    cell: Mutex<Option<Arc<dyn Reasoner>>>,
}

impl OllamaReasonerProvider {
    pub fn new() -> Self {
        Self { cell: Mutex::new(None) }
    }
}

impl Default for OllamaReasonerProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl ReasonerProvider for OllamaReasonerProvider {
    fn reasoner(&self) -> Result<Arc<dyn Reasoner>, EngineOpError> {
        let mut guard = self.cell.lock().expect("reasoner cell poisoned");
        if let Some(r) = guard.as_ref() {
            return Ok(r.clone());
        }
        let arc: Arc<dyn Reasoner> = Arc::new(bossclaw_core::OllamaReasoner::new(REASONER_MODEL_ID));
        *guard = Some(arc.clone());
        Ok(arc)
    }
}

#[cfg(test)]
pub struct MockReasonerProvider {
    reasoner: Arc<dyn Reasoner>,
}

#[cfg(test)]
impl MockReasonerProvider {
    /// A reasoner with NO canned responses (only `model_id` is exercised). Tests that
    /// drive `evolve_once` build a `ScriptedReasoner` with `.with_response(...)` turns
    /// and wrap it via `from_scripted`.
    pub fn new(model_id: &str) -> Self {
        Self { reasoner: Arc::new(bossclaw_core::ScriptedReasoner::new(model_id)) }
    }

    // Used by the scripted-evolve test (SP3 Task 7) to wrap a reasoner with canned turns.
    #[allow(dead_code)]
    pub fn from_scripted(s: bossclaw_core::ScriptedReasoner) -> Self {
        Self { reasoner: Arc::new(s) }
    }
}

#[cfg(test)]
impl ReasonerProvider for MockReasonerProvider {
    fn reasoner(&self) -> Result<Arc<dyn Reasoner>, EngineOpError> {
        Ok(self.reasoner.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mock_provider_yields_a_scripted_reasoner() {
        let p = MockReasonerProvider::new("test-model");
        let r = p.reasoner().expect("reasoner builds");
        assert_eq!(r.model_id(), "test-model");
    }
    #[test]
    fn ollama_provider_caches_one_instance() {
        let p = OllamaReasonerProvider::new();
        let a = p.reasoner().expect("a");
        let b = p.reasoner().expect("b");
        assert!(std::sync::Arc::ptr_eq(&a, &b), "second call returns the cached Arc");
    }
}
