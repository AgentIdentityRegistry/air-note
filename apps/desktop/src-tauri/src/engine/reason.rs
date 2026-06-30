//! The reasoner seam (mirrors engine/embed.rs): lazy, cached construction of the
//! local Ollama reasoner the evolve loop drives. A cloud reasoner can later drop in
//! behind this same trait with zero rework.
//! See docs/superpowers/specs/2026-06-23-sp3-recall-evolve-design.md.

use crate::engine::EngineOpError;
use bossclaw_core::Reasoner;
use std::sync::{Arc, Mutex};

/// Single source of truth for the evolve reasoner's Ollama model tag (mirrors
/// embed::MODEL_ID). Unpinned for SP3 (the user pulls it via `ollama pull`). Consumed by the
/// Ollama detection probe + scheduler (SP3 Tasks 9–10) and by `reasoner()` below.
pub const REASONER_MODEL_ID: &str = "qwen2.5:7b-instruct";

/// Builds (and caches) the reasoner. Called on first evolve, never at startup.
pub trait ReasonerProvider: Send + Sync {
    /// Called by `EngineHandle::evolve_once` to drive the loop.
    fn reasoner(&self) -> Result<Arc<dyn Reasoner>, EngineOpError>;
}

/// Production provider: yields `bossclaw_core::OllamaReasoner` (loopback-fail-closed)
/// on first use and caches it for the process lifetime.
pub struct OllamaReasonerProvider {
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

// Canonical `CloudProvider` lives in `cloud_reasoner`; re-exported here so the
// config plumbing (Tasks 9/10/12) names a single type. Consumed by Task 9/10/12.
#[allow(unused_imports)]
pub use crate::engine::cloud_reasoner::CloudProvider;
use crate::engine::cloud_reasoner::CloudReasoner;

/// A stable string identity for a config; the provider rebuilds when it changes.
/// Consumed by `ConfigReasonerProvider` (and reachable from main.rs at Task 12).
#[allow(dead_code)]
pub fn config_fingerprint(c: &ReasonerConfig) -> String {
    let mode = match c.mode {
        ReasonerMode::Local => "local",
        ReasonerMode::Cloud => "cloud",
    };
    let provider = match c.provider {
        CloudProvider::Anthropic => "anthropic",
        CloudProvider::OpenAiCompat => "openai-compat",
    };
    format!("{mode}|{provider}|{}|{}", c.model, c.base_url.as_deref().unwrap_or(""))
}

#[allow(dead_code)]
type ConfigReader = Box<dyn Fn() -> ReasonerConfig + Send + Sync>;

/// Config-driven reasoner provider: builds Ollama (Local) or `CloudReasoner`
/// (Cloud) and memoizes keyed on the config fingerprint, rebuilding on change.
/// Wired into `main.rs` by Task 12; reached by `provider_tests` until then.
#[allow(dead_code)]
pub struct ConfigReasonerProvider {
    read_config: ConfigReader,
    cell: Mutex<Option<(String, Arc<dyn Reasoner>)>>,
}

#[allow(dead_code)] // new + build are reached only via provider_tests until main.rs wires this at Task 12.
impl ConfigReasonerProvider {
    pub fn new(read_config: impl Fn() -> ReasonerConfig + Send + Sync + 'static) -> Self {
        Self { read_config: Box::new(read_config), cell: Mutex::new(None) }
    }

    fn build(config: &ReasonerConfig) -> Arc<dyn Reasoner> {
        build_reasoner(config)
    }
}

/// The ONE reasoner builder (Local→Ollama with the pinned local tag; Cloud→`CloudReasoner`).
/// `ConfigReasonerProvider::build` and the R5 enable-with-test-key flow (`engine/mod.rs`'s
/// `enable_cloud_reasoner`) both go through this, so a one-shot probe reasoner is byte-for-byte
/// what the scheduler would later build. Consumed by Task 9 (provider) + Task 12 (enable flow).
pub(crate) fn build_reasoner(config: &ReasonerConfig) -> Arc<dyn Reasoner> {
    match config.mode {
        ReasonerMode::Local => {
            // Exactly as today: model id stays the hardcoded local tag (R8).
            Arc::new(bossclaw_core::OllamaReasoner::new(REASONER_MODEL_ID))
        }
        ReasonerMode::Cloud => Arc::new(CloudReasoner::new(
            config.provider,
            config.model.clone(),
            config.base_url.clone(),
        )),
    }
}

impl ReasonerProvider for ConfigReasonerProvider {
    fn reasoner(&self) -> Result<Arc<dyn Reasoner>, EngineOpError> {
        let config = (self.read_config)();
        let fp = config_fingerprint(&config);
        let mut guard = self.cell.lock().expect("reasoner cell poisoned");
        if let Some((cached_fp, r)) = guard.as_ref() {
            if *cached_fp == fp {
                return Ok(r.clone());
            }
        }
        let arc = Self::build(&config);
        *guard = Some((fp, arc.clone()));
        Ok(arc)
    }
}

/// Which reasoner the evolve loop drives: the local Ollama probe, or a signed,
/// consented cloud provider. Consumed by Task 9/10/12.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasonerMode {
    Local,
    Cloud,
}

/// The reasoner config the scheduler reads to decide mode + readiness. Consumed
/// by Task 9/10/12.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ReasonerConfig {
    pub mode: ReasonerMode,
    pub provider: CloudProvider,
    pub model: String,
    pub base_url: Option<String>,
}

impl Default for ReasonerConfig {
    fn default() -> Self {
        Self { mode: ReasonerMode::Local, provider: CloudProvider::Anthropic, model: String::new(), base_url: None }
    }
}

/// The wire string for a provider, matching the signed consent record's
/// `provider` field (same strings as the `CloudReasoner::model_id` prefixes).
fn provider_str(p: CloudProvider) -> &'static str {
    match p {
        CloudProvider::Anthropic => "anthropic",
        CloudProvider::OpenAiCompat => "openai-compat",
    }
}

/// The host the config WOULD connect to (Anthropic is pinned; OpenAI-compat uses
/// the base_url host). Returns None if an OpenAI-compat base_url is missing/invalid.
/// Consumed by Task 9/10/12.
#[allow(dead_code)]
pub fn config_host(config: &ReasonerConfig) -> Option<String> {
    match config.provider {
        CloudProvider::Anthropic => Some("api.anthropic.com".to_string()),
        CloudProvider::OpenAiCompat => reqwest::Url::parse(config.base_url.as_deref()?)
            .ok()
            .and_then(|u| u.host_str().map(|h| h.to_string())),
    }
}

/// Fail-closed readiness. Local: follows the caller's probe. Cloud: a signed
/// consent record must EXIST and MATCH (provider, host, vault key fingerprint);
/// any mismatch -> not ready (spec R1). Consumed by Task 9/10/12.
#[allow(dead_code)]
pub fn reasoner_ready(
    config: &ReasonerConfig,
    consent: Option<&serde_json::Value>,
    vault_key_fingerprint: Option<&str>,
    local_probe_ready: bool,
) -> bool {
    match config.mode {
        ReasonerMode::Local => local_probe_ready,
        ReasonerMode::Cloud => {
            let (Some(consent), Some(key_fp), Some(host)) =
                (consent, vault_key_fingerprint, config_host(config))
            else {
                return false;
            };
            let c_provider = consent.get("provider").and_then(|v| v.as_str());
            let c_host = consent.get("base_url_host").and_then(|v| v.as_str());
            let c_fp = consent.get("key_fingerprint").and_then(|v| v.as_str());
            c_provider == Some(provider_str(config.provider))
                && c_host == Some(host.as_str())
                && c_fp == Some(key_fp)
        }
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
    /// and wrap it via `from_reasoner`.
    pub fn new(model_id: &str) -> Self {
        Self { reasoner: Arc::new(bossclaw_core::ScriptedReasoner::new(model_id)) }
    }

    /// Wrap ANY `Reasoner` (SP3 Task 7). The engine's `ScriptedReasoner` is SHA-256-keyed on
    /// the exact `(system, prompt)`, but `evolve_once` computes the recall/neighborhood
    /// context internally, so reproducing those keys at the desktop level is fragile. The
    /// evolve tests instead inject a small prompt-agnostic stub that picks a schema-valid
    /// response by inspecting the `schema` arg (extraction vs adjudication); this wraps it.
    /// A `ScriptedReasoner` can also be passed here as `Arc<dyn Reasoner>` when exact
    /// prompt-keying IS desired.
    pub fn from_reasoner(r: Arc<dyn Reasoner>) -> Self {
        Self { reasoner: r }
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

#[cfg(test)]
mod provider_tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn default_config_builds_local_ollama() {
        // R8: no config -> default Local -> Ollama with the existing model id.
        let cfg = std::sync::Arc::new(Mutex::new(ReasonerConfig::default()));
        let cfg2 = cfg.clone();
        let provider = ConfigReasonerProvider::new(move || cfg2.lock().unwrap().clone());
        let r = provider.reasoner().unwrap();
        assert_eq!(r.model_id(), REASONER_MODEL_ID);
    }

    #[test]
    fn config_change_rebuilds() {
        let cfg = std::sync::Arc::new(Mutex::new(ReasonerConfig::default()));
        let cfg2 = cfg.clone();
        let provider = ConfigReasonerProvider::new(move || cfg2.lock().unwrap().clone());
        let first = provider.reasoner().unwrap();
        // Flip to cloud Anthropic -> different reasoner (different model_id).
        *cfg.lock().unwrap() = ReasonerConfig {
            mode: ReasonerMode::Cloud,
            provider: CloudProvider::Anthropic,
            model: "claude-sonnet-4-6".into(),
            base_url: None,
        };
        let second = provider.reasoner().unwrap();
        assert_ne!(first.model_id(), second.model_id());
        assert_eq!(second.model_id(), "anthropic:claude-sonnet-4-6");
    }
}

#[cfg(test)]
mod ready_tests {
    use super::*;

    fn cloud_cfg() -> ReasonerConfig {
        ReasonerConfig {
            mode: ReasonerMode::Cloud,
            provider: CloudProvider::Anthropic,
            model: "claude-sonnet-4-6".into(),
            base_url: None,
        }
    }

    fn matching_consent() -> serde_json::Value {
        serde_json::json!({
            "provider": "anthropic",
            "base_url_host": "api.anthropic.com",
            "key_fingerprint": "abc123",
            "consented_at": "2026-06-30T00:00:00Z"
        })
    }

    #[test]
    fn local_ready_follows_probe() {
        let cfg = ReasonerConfig { mode: ReasonerMode::Local, ..cloud_cfg() };
        assert!(reasoner_ready(&cfg, None, None, true));
        assert!(!reasoner_ready(&cfg, None, None, false));
    }

    #[test]
    fn cloud_ready_requires_matching_signed_consent() {
        let cfg = cloud_cfg();
        // No consent -> not ready.
        assert!(!reasoner_ready(&cfg, None, Some("abc123"), false));
        // Matching consent + key fp -> ready (local probe irrelevant in cloud mode).
        assert!(reasoner_ready(&cfg, Some(&matching_consent()), Some("abc123"), false));
        // Key rotated (fp differs) -> not ready (re-consent).
        assert!(!reasoner_ready(&cfg, Some(&matching_consent()), Some("DIFFERENT"), false));
        // Provider/host edited in config away from the consented one -> not ready.
        let edited = ReasonerConfig { provider: CloudProvider::OpenAiCompat, base_url: Some("https://x.example.com".into()), ..cloud_cfg() };
        assert!(!reasoner_ready(&edited, Some(&matching_consent()), Some("abc123"), false));
        // Missing vault key fp -> not ready.
        assert!(!reasoner_ready(&cfg, Some(&matching_consent()), None, false));
    }
}
