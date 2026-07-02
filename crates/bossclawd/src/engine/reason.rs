// Copied from apps/desktop/src-tauri/src/engine/reason.rs (M1a Task 4); the in-app original is removed in Task 6.

//! The reasoner seam (mirrors engine/embed.rs): lazy, cached construction of the
//! local Ollama reasoner the evolve loop drives. A cloud reasoner drops in behind
//! this same trait with zero rework.

use crate::engine::EngineOpError;
use bossclaw_core::Reasoner;
use std::sync::{Arc, Mutex};

/// Single source of truth for the evolve reasoner's Ollama model tag (mirrors
/// embed::MODEL_ID). Consumed by the Ollama detection probe + scheduler and by `reasoner()`.
pub const REASONER_MODEL_ID: &str = "qwen2.5:7b-instruct";

/// Builds (and caches) the reasoner. Called on first evolve, never at startup.
pub trait ReasonerProvider: Send + Sync {
    /// Called by `EngineHandle::evolve_once` to drive the loop.
    fn reasoner(&self) -> Result<Arc<dyn Reasoner>, EngineOpError>;
}

// The production provider is `ConfigReasonerProvider` (below), wired in `main.rs`. It reads a
// shared config cell and builds Local (Ollama) or Cloud (`CloudReasoner`) via `build_reasoner`,
// rebuilding when the config fingerprint changes.

// Canonical `CloudProvider` lives in `cloud_reasoner`; re-exported here so the
// config plumbing (provider + main.rs) names a single type.
pub use crate::engine::cloud_reasoner::CloudProvider;
use crate::engine::cloud_reasoner::CloudReasoner;

/// A stable string identity for a config; the provider rebuilds when it changes.
/// Consumed by `ConfigReasonerProvider::reasoner`.
pub fn config_fingerprint(c: &ReasonerConfig) -> String {
    let mode = match c.mode {
        ReasonerMode::Local => "local",
        ReasonerMode::Cloud => "cloud",
    };
    let provider = match c.provider {
        CloudProvider::Anthropic => "anthropic",
        CloudProvider::OpenAiCompat => "openai-compat",
        CloudProvider::Gemini => "gemini",
    };
    format!("{mode}|{provider}|{}|{}", c.model, c.base_url.as_deref().unwrap_or(""))
}

type ConfigReader = Box<dyn Fn() -> ReasonerConfig + Send + Sync>;

/// Config-driven reasoner provider: builds Ollama (Local) or `CloudReasoner`
/// (Cloud) and memoizes keyed on the config fingerprint, rebuilding on change.
/// Wired into `main.rs` (reads the shared `reasoner_cfg` cell).
pub struct ConfigReasonerProvider {
    read_config: ConfigReader,
    cell: Mutex<Option<(String, Arc<dyn Reasoner>)>>,
}

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
/// what the scheduler would later build.
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
/// consented cloud provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasonerMode {
    Local,
    Cloud,
}

/// The reasoner config the scheduler reads to decide mode + readiness.
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
/// SHARED by the consent READER (`reasoner_ready`) and the consent WRITER
/// (`engine/mod.rs`'s `enable_cloud_reasoner`) so the two can never drift —
/// a drift would silently produce `ready==false` after a successful enable.
pub(crate) fn provider_str(p: CloudProvider) -> &'static str {
    match p {
        CloudProvider::Anthropic => "anthropic",
        CloudProvider::OpenAiCompat => "openai-compat",
        CloudProvider::Gemini => "gemini",
    }
}

/// The host the config WOULD connect to (Anthropic + Gemini are pinned; OpenAI-compat uses
/// the base_url host). Returns None if an OpenAI-compat base_url is missing/invalid.
/// Consumed by `enable_cloud_reasoner` (consent host) + `reasoner_ready`.
pub fn config_host(config: &ReasonerConfig) -> Option<String> {
    match config.provider {
        CloudProvider::Anthropic => Some(crate::engine::cloud_reasoner::ANTHROPIC_HOST.to_string()),
        CloudProvider::Gemini => Some(crate::engine::cloud_reasoner::GEMINI_HOST.to_string()),
        CloudProvider::OpenAiCompat => reqwest::Url::parse(config.base_url.as_deref()?)
            .ok()
            .and_then(|u| u.host_str().map(|h| h.to_string())),
    }
}

/// Fail-closed readiness. Local: follows the caller's probe. Cloud: a signed
/// consent record must EXIST and MATCH (provider, host, vault key fingerprint);
/// any mismatch -> not ready (spec R1). Consumed by `reasoner_ready_or_false`.
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
    /// A reasoner with NO canned responses (only `model_id` is exercised).
    pub fn new(model_id: &str) -> Self {
        Self { reasoner: Arc::new(bossclaw_core::ScriptedReasoner::new(model_id)) }
    }

    /// Wrap ANY `Reasoner` — the evolve tests inject a small prompt-agnostic stub.
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
    fn fresh_install_is_local_and_not_cloud_ready() {
        // R8: absent any config -> default Local -> egresses nothing.
        let cfg = ReasonerConfig::default();
        assert!(matches!(cfg.mode, ReasonerMode::Local));
        // A cloud config with NO signed consent + no key -> never ready (can't egress).
        let cloud = ReasonerConfig { mode: ReasonerMode::Cloud, ..ReasonerConfig::default() };
        assert!(!reasoner_ready(&cloud, None, None, false));
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
