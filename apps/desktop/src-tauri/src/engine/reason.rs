//! Reasoner CONFIG + DATA types the app still names after the M1a daemon split (Task 6).
//!
//! The reasoner PROVIDER machinery — `ReasonerProvider`/`ConfigReasonerProvider`, the Ollama +
//! cloud builders, consent/readiness (`reasoner_ready`, `config_host`) — moved into the `bossclawd`
//! daemon with the engine. What stays here is the small set of config/data types the command layer
//! and the client's return types reference:
//! - [`ReasonerConfig`] / [`ReasonerMode`] / [`CloudProvider`]: the client returns a
//!   `ReasonerConfig` (from `ReasonerConfigWire`), and `engine_get_reasoner_config` reads its fields;
//! - [`provider_str`]: `engine_get_reasoner_config` maps the provider to its webview string;
//! - [`REASONER_MODEL_ID`]: `engine_ollama_status` shows it in the "install Ollama and pull …" hint.

/// Single source of truth for the LOCAL evolve reasoner's Ollama model tag. Shown by the
/// app-side Ollama status probe (`engine_ollama_status`) in its install hint. The daemon owns the
/// same constant for the reasoner it actually builds; this copy is app-side UI text only.
pub const REASONER_MODEL_ID: &str = "qwen2.5:7b-instruct";

/// Which reasoner the evolve loop drives: the local Ollama probe, or a signed, consented cloud
/// provider. Carried across the wire in `ReasonerConfigWire`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasonerMode {
    Local,
    Cloud,
}

/// The cloud provider family for a Cloud-mode config. Was defined in the now-daemon-owned
/// `cloud_reasoner` module; it lives here now because it is a plain config enum the client's
/// `ReasonerConfig` return type carries. The daemon has its own copy (behind the same wire enum).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudProvider {
    Anthropic,
    OpenAiCompat,
    Gemini,
}

/// The reasoner config the Settings UI reads/writes and the client returns.
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

/// The webview string for a provider (matches the persisted config + the TS `ReasonerConfigDto`
/// twin). Consumed by `engine_get_reasoner_config` when it renders the current config.
pub(crate) fn provider_str(p: CloudProvider) -> &'static str {
    match p {
        CloudProvider::Anthropic => "anthropic",
        CloudProvider::OpenAiCompat => "openai-compat",
        CloudProvider::Gemini => "gemini",
    }
}
