//! Desktop-side Ollama detection (SP3 Task 9). A plain `GET` of the LOCAL Ollama's
//! `/api/tags` over the desktop's existing `reqwest` — it is desktop-side, so it does NOT
//! enter `bossclaw-core`'s dependency graph (the engine network guard is engine-scoped).
//! See docs/superpowers/specs/2026-06-23-sp3-recall-evolve-design.md §D.
//!
//! Security: the host is a HARDCODED loopback literal (no user input → no SSRF surface), and
//! ANY error (connect refused, timeout, non-200, bad JSON) maps to "not reachable" — the
//! probe never panics and never returns an `Err`.

use serde_json::Value;
use std::time::Duration;

/// The fixed loopback URL Ollama serves its tag list on. Hardcoded (no user input) so the
/// probe can never be pointed off-box — mirrors the engine's loopback-only `OllamaReasoner`.
const OLLAMA_TAGS_URL: &str = "http://127.0.0.1:11434/api/tags";

/// Short connect/read budget: a missing Ollama should fail fast (the Memory tab polls this),
/// not hang the status card for the default multi-second client timeout.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// The result of a single probe. Both fields are `false` on any failure (see module docs).
pub struct OllamaStatus {
    /// The probe got a 2xx from the local Ollama.
    pub reachable: bool,
    /// `model_tag` appears in the returned `models[].name` list.
    pub model_present: bool,
}

/// Whether `tag` appears as a `models[].name` in an `/api/tags` response body. PURE — the
/// caller supplies the parsed JSON, so this is the unit-tested core. A malformed body
/// (missing/!array `models`) yields `false` rather than erroring.
pub fn model_present_in_tags(body: &Value, tag: &str) -> bool {
    body.get("models")
        .and_then(|m| m.as_array())
        .map(|arr| arr.iter().any(|m| m.get("name").and_then(|n| n.as_str()) == Some(tag)))
        .unwrap_or(false)
}

/// Probe the LOCAL Ollama (hardcoded loopback host) for reachability + whether `model_tag`
/// is pulled. NEVER errors / NEVER panics: any connect/timeout/status/parse failure →
/// `{ reachable: false, model_present: false }` (the spec'd fail-closed behavior, not a shim).
pub async fn probe(model_tag: &str) -> OllamaStatus {
    let request = reqwest::Client::new().get(OLLAMA_TAGS_URL).timeout(PROBE_TIMEOUT).send().await;
    match request {
        Ok(resp) if resp.status().is_success() => match resp.json::<Value>().await {
            Ok(body) => OllamaStatus {
                reachable: true,
                model_present: model_present_in_tags(&body, model_tag),
            },
            // Reached Ollama but the body wasn't parseable JSON — reachable, model unknown.
            Err(_) => OllamaStatus { reachable: true, model_present: false },
        },
        // Connect refused / timeout / non-2xx → treat as not running.
        _ => OllamaStatus { reachable: false, model_present: false },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_model_present_from_tags() {
        let body = serde_json::json!({
            "models": [ { "name": "qwen2.5:7b-instruct" }, { "name": "llama3" } ]
        });
        assert!(model_present_in_tags(&body, "qwen2.5:7b-instruct"));
        assert!(!model_present_in_tags(&body, "absent:1b"));
    }

    #[test]
    fn malformed_tags_body_is_not_present() {
        // No `models` key at all, and a non-array `models` — both yield `false`, never panic.
        assert!(!model_present_in_tags(&serde_json::json!({}), "qwen2.5:7b-instruct"));
        assert!(!model_present_in_tags(&serde_json::json!({ "models": "nope" }), "qwen2.5:7b-instruct"));
    }
}
