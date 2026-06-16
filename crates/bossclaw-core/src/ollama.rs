//! The real local reasoner backend (spec §5): `OllamaReasoner` POSTs to a
//! loopback Ollama server, schema-constrained, `temperature 0`, digest-pinned,
//! refusing any non-loopback host (parent §8.5, no egress). Behind the `ollama`
//! cargo feature so the default build is pure (no network dependency); the app
//! may still inject its own [`Reasoner`].
//!
//! # Loopback enforcement (Rev 2 F8)
//!
//! The bare hostname `"localhost"` is NOT accepted. It is resolvable via the
//! hosts file and vulnerable to DNS-rebind attacks. Only numeric loopback
//! addresses are allowed: `127.0.0.0/8` and `::1`. This is checked before any
//! socket is opened — fail-closed.
//!
//! # Model id / digest pinning
//!
//! `model_id()` returns the tag passed to the constructor. For production, use a
//! digest-pinned tag, e.g. `qwen2.5:7b-instruct@sha256:<hex>`, so the provenance
//! record in each emitted event names the exact blob that produced it. A 7b→14b
//! upgrade is non-destructive: old events keep the old tag; new events carry the
//! new one.

use crate::error::BossclawError;
use crate::reason::Reasoner;

/// Default Ollama endpoint — numeric loopback only (parent §8.5: no egress).
const OLLAMA_LOOPBACK_URL: &str = "http://127.0.0.1:11434/api/chat";

/// HTTP request timeout. Generous: a cold 7b on CPU can take tens of seconds for
/// the first token. A timeout (rather than an unbounded wait) keeps a wedged
/// server from hanging the evolve tick forever — the tick degrades to a no-op
/// and retries (spec §10).
const OLLAMA_TIMEOUT_SECS: u64 = 120;

/// The real reasoner: a schema-constrained, deterministic (`temperature 0`),
/// digest-pinned local model over loopback HTTP. Holds the resolved model tag
/// (e.g. `qwen2.5:7b-instruct@sha256:<hex>`) stamped into every emitted event.
///
/// Any non-loopback URL is refused before a socket is opened (Rev 2 F8).
pub struct OllamaReasoner {
    model_tag: String,
    url: String,
    agent: ureq::Agent,
}

impl OllamaReasoner {
    /// Create a reasoner for `model_tag` (e.g. `"qwen2.5:7b-instruct"` or a
    /// digest-pinned form `"qwen2.5:7b-instruct@sha256:<hex>"`) against the
    /// default loopback Ollama server (`http://127.0.0.1:11434/api/chat`).
    /// Use a digest-pinned tag in production so the model cannot silently change
    /// under you (spec §2.1).
    pub fn new(model_tag: &str) -> Self {
        Self::with_url(model_tag, OLLAMA_LOOPBACK_URL)
    }

    /// Create a reasoner pointed at an explicit `url`. **Refuses any non-loopback
    /// host at request time** (see [`is_loopback_url`]) so a misconfiguration can
    /// never cause egress. Primarily for tests that run a loopback stub on a
    /// chosen port; production should use [`OllamaReasoner::new`].
    pub fn with_url(model_tag: &str, url: &str) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(OLLAMA_TIMEOUT_SECS))
            .build();
        Self { model_tag: model_tag.to_string(), url: url.to_string(), agent }
    }
}

impl Reasoner for OllamaReasoner {
    fn complete_json(
        &self,
        system: &str,
        prompt: &str,
        schema: &serde_json::Value,
    ) -> Result<serde_json::Value, BossclawError> {
        // Fail-closed egress guard: refuse anything that is not a numeric loopback
        // host BEFORE any socket is opened (parent §8.5, Rev 2 F8).
        if !is_loopback_url(&self.url) {
            return Err(BossclawError::Reasoner(format!(
                "refusing non-loopback reasoner host: {} (numeric loopback only, no egress)",
                self.url
            )));
        }
        // Ollama /api/chat with structured output: `format` = the schema,
        // `options.temperature = 0` for determinism, `stream = false` so we get
        // one complete JSON body.
        let body = serde_json::json!({
            "model": self.model_tag,
            "stream": false,
            "format": schema,
            "options": { "temperature": 0 },
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": prompt }
            ]
        });
        let resp = self
            .agent
            .post(&self.url)
            .send_json(body)
            .map_err(|e| BossclawError::Reasoner(format!("ollama transport: {e}")))?;
        let envelope: serde_json::Value = resp
            .into_json()
            .map_err(|e| BossclawError::Reasoner(format!("ollama response not JSON: {e}")))?;
        // The structured content is a JSON STRING in message.content; parse it to
        // the value the caller's schema described.
        let content = envelope
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .ok_or_else(|| {
                BossclawError::Reasoner("ollama response missing message.content".into())
            })?;
        serde_json::from_str(content)
            .map_err(|e| BossclawError::Reasoner(format!("ollama content not valid JSON: {e}")))
    }

    fn model_id(&self) -> &str {
        &self.model_tag
    }
}

/// True iff `url`'s host is a **numeric** loopback address (`127.0.0.0/8` or
/// `::1`). The bare hostname `"localhost"` is explicitly NOT accepted (Rev 2 F8:
/// hosts-file / DNS-rebind risk). Best-effort parse — an unparseable URL is NOT
/// loopback (fail-closed).
fn is_loopback_url(url: &str) -> bool {
    // Strip scheme, then take the authority up to the first '/'.
    let after_scheme = url.split("://").nth(1).unwrap_or(url);
    let authority = after_scheme.split('/').next().unwrap_or("");
    // Drop an optional port (host:port). IPv6 literals are bracketed: [::1]:11434.
    let host = if let Some(rest) = authority.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        authority.split(':').next().unwrap_or("")
    };
    // Only numeric IPs accepted — parse and assert .is_loopback().
    match host.parse::<std::net::IpAddr>() {
        Ok(ip) => ip.is_loopback(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{is_loopback_url, OllamaReasoner};
    use crate::reason::{extraction_schema, Reasoner};

    /// Pure unit test: the URL classifier accepts numeric loopback and rejects
    /// everything else, including bare `"localhost"` (Rev 2 F8).
    #[test]
    fn loopback_url_classifier_numeric_only() {
        assert!(is_loopback_url("http://127.0.0.1:11434/api/chat"));
        assert!(is_loopback_url("http://[::1]:11434/api/chat"));
        assert!(is_loopback_url("http://127.0.0.2:11434/"));
        // "localhost" is explicitly rejected (Rev 2 F8: DNS-rebind risk).
        assert!(!is_loopback_url("http://localhost:11434/api/chat"));
        assert!(!is_loopback_url("http://10.0.0.5:11434/api/chat"));
        assert!(!is_loopback_url("http://evil.example.com/api/chat"));
        assert!(!is_loopback_url("not a url"));
    }

    /// WIRED refusal test (Rev 2 F8-c): `complete_json` on a non-loopback URL
    /// returns `Err(BossclawError::Reasoner)` from the actual request path —
    /// proving the guard is on the hot path, not just in the classifier.
    #[test]
    fn complete_json_refuses_non_loopback_url_before_any_socket() {
        let reasoner = OllamaReasoner::with_url("qwen2.5:7b-instruct", "http://10.0.0.5:11434");
        let schema = extraction_schema();
        let err = reasoner
            .complete_json("SYS", "PROMPT", &schema)
            .expect_err("non-loopback URL must be refused");
        assert!(
            matches!(err, crate::BossclawError::Reasoner(_)),
            "expected BossclawError::Reasoner, got {err:?}"
        );
        // Confirm the error message names the offending URL.
        let msg = err.to_string();
        assert!(
            msg.contains("10.0.0.5"),
            "error message should name the refused host, got: {msg}"
        );
    }
}
