//! Desktop-side cloud reasoner (Anthropic + OpenAI-compat). The brain's first
//! deliberate network egress: off-by-default, fail-closed, host-pinned, signed
//! consent. Lives here (not bossclaw-core) because the engine crate's CI jail
//! forbids `reqwest`. See docs/superpowers/specs/2026-06-30-milestone-d2-cloud-reasoner-design.md §8.

// Implemented task-by-task in the Phase 2a plan.

use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use bossclaw_core::{BossclawError, Reasoner};
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use serde_json::{json, Value};

use crate::web_access::is_blocked_ip;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Pure SSRF screen: returns the addresses only if EVERY one is a safe public
/// destination; errors if any is internal/loopback/link-local/private/CGNAT/
/// metadata, or if the set is empty. Used at connect time to close the
/// DNS-rebind race that a pre-flight host check cannot (spec §8 R2).
// Exercised by `tests::screen_addrs_rejects_any_blocked` and called from
// `PinnedResolver::resolve` → `blocking_client` → the `CloudReasoner` arms. No
// per-helper `dead_code` allow: the allow on `impl CloudReasoner` transitively
// keeps this whole egress subgraph live until Task 9 wires a non-test caller.
fn screen_addrs(addrs: Vec<SocketAddr>) -> Result<Vec<SocketAddr>, BoxError> {
    if addrs.is_empty() {
        return Err("cloud reasoner DNS resolved to no addresses".into());
    }
    if let Some(bad) = addrs.iter().find(|a| is_blocked_ip(a.ip())) {
        return Err(format!(
            "cloud reasoner refusing connection: host resolves to a blocked address ({})",
            bad.ip()
        )
        .into());
    }
    Ok(addrs)
}

/// Map an HTTP status to a fixed error class + status string. The raw provider
/// body is consumed by the caller for classification and DROPPED here — it can
/// echo prompt content (= memory/file bytes), so it never enters the error
/// string surfaced to the webview (spec §8 R3).
pub(crate) fn classify_cloud_error(status: u16) -> BossclawError {
    let class = match status {
        401 | 403 => "auth_rejected",
        429 => "rate_limited",
        400 | 422 => "bad_request",
        404 => "model_or_endpoint_not_found",
        500..=599 => "provider_5xx",
        _ => "provider_error",
    };
    BossclawError::Reasoner(format!("cloud reasoner {class} (HTTP {status})"))
}

/// Output budget for `/v1/messages`. REQUIRED field; sized for a ~16-memory
/// extraction batch (spec §8 R7). Small values truncate the JSON tail.
pub(crate) const ANTHROPIC_MAX_TOKENS: u32 = 4096;
pub(crate) const ANTHROPIC_TOOL_NAME: &str = "emit_result";

/// Build the Anthropic `/v1/messages` body that FORCES one structured tool call
/// whose input_schema is the engine schema. No `temperature`/`thinking`
/// (rejected with 400 on current Opus models) and no `strict` (engine schemas
/// lack `additionalProperties:false`). See spec §8 R7 + plan decision #2.
pub(crate) fn build_anthropic_request(model: &str, system: &str, prompt: &str, schema: &Value) -> Value {
    json!({
        "model": model,
        "max_tokens": ANTHROPIC_MAX_TOKENS,
        "system": system,
        "messages": [ { "role": "user", "content": prompt } ],
        "tools": [ {
            "name": ANTHROPIC_TOOL_NAME,
            "description": "Return the structured result for this request.",
            "input_schema": schema
        } ],
        "tool_choice": { "type": "tool", "name": ANTHROPIC_TOOL_NAME }
    })
}

/// Pull the forced tool_use block's `.input` out of an Anthropic response.
pub(crate) fn extract_anthropic_result(resp: &Value) -> Result<Value, BossclawError> {
    let content = resp
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| BossclawError::Reasoner("anthropic response missing content array".into()))?;
    for block in content {
        if block.get("type").and_then(Value::as_str) == Some("tool_use")
            && block.get("name").and_then(Value::as_str) == Some(ANTHROPIC_TOOL_NAME)
        {
            return block
                .get("input")
                .cloned()
                .ok_or_else(|| BossclawError::Reasoner("anthropic tool_use missing input".into()));
        }
    }
    Err(BossclawError::Reasoner("anthropic response had no forced tool_use block".into()))
}

/// Build the OpenAI-compat `/v1/chat/completions` body. `fallback=false` uses
/// native `json_schema`; `fallback=true` switches to `json_object` and folds
/// the schema into the system text (some compat servers reject json_schema).
/// No `temperature` (parity with the Anthropic arm). Spec §8 R7 + §3.1.
pub(crate) fn build_openai_request(
    model: &str,
    system: &str,
    prompt: &str,
    schema: &Value,
    fallback: bool,
) -> Value {
    let (system_text, response_format) = if fallback {
        (
            format!(
                "{system}\n\nRespond with a single JSON object that conforms to this JSON schema:\n{schema}"
            ),
            json!({ "type": "json_object" }),
        )
    } else {
        (
            system.to_string(),
            json!({
                "type": "json_schema",
                "json_schema": { "name": "result", "schema": schema, "strict": false }
            }),
        )
    };
    json!({
        "model": model,
        "stream": false,
        "response_format": response_format,
        "messages": [
            { "role": "system", "content": system_text },
            { "role": "user", "content": prompt }
        ]
    })
}

/// Extract `choices[0].message.content` and parse it as JSON, tolerating a
/// ```json fence. Parse failure -> `BossclawError::Reasoner` (retryable no-op).
pub(crate) fn extract_openai_result(resp: &Value) -> Result<Value, BossclawError> {
    let content = resp
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(Value::as_str)
        .ok_or_else(|| BossclawError::Reasoner("openai response missing message content".into()))?;
    let trimmed = strip_json_fence(content);
    serde_json::from_str(trimmed)
        .map_err(|e| BossclawError::Reasoner(format!("openai content not valid JSON: {e}")))
}

/// Strip a leading ```json / ``` fence and trailing ``` if present.
fn strip_json_fence(s: &str) -> &str {
    let t = s.trim();
    let t = t
        .strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .unwrap_or(t);
    t.trim().strip_suffix("```").unwrap_or(t).trim()
}

/// A `reqwest` DNS resolver that screens every resolved address through
/// `is_blocked_ip` before any socket is opened. This is the connect-time pin
/// that closes the rebind race (`web_access.rs:171` documents the residual gap
/// this fills); installed on the blocking client built in Task 5.
struct PinnedResolver;

impl Resolve for PinnedResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let screened = tokio::task::spawn_blocking(move || {
                // Port is irrelevant for screening (reqwest applies the URL's
                // port when connecting); 443 mirrors web_access::validate_host.
                let resolved: Vec<SocketAddr> = (host.as_str(), 443u16)
                    .to_socket_addrs()
                    .map_err(|e| -> BoxError { Box::new(e) })?
                    .collect();
                screen_addrs(resolved)
            })
            .await
            .map_err(|e| -> BoxError { Box::new(e) })??;
            let iter: Addrs = Box::new(screened.into_iter());
            Ok(iter)
        })
    }
}

/// Request timeout: parity with OLLAMA_TIMEOUT_SECS (120s). The reasoner holds
/// the evolve_lock during a tick, so a hung call self-DoSes the tick (spec R6).
const CLOUD_TIMEOUT_SECS: u64 = 120;

/// The one hardened blocking client (connection pool + SSRF resolver + no
/// redirects + timeout), built lazily on first use. First use is always on a
/// `spawn_blocking` thread, so no async runtime is captured (spec R6).
/// Returns `None` if the client cannot be built — fail-closed: no hardened
/// client means no egress (Task 6 maps `None` to a retryable no-op tick),
/// never a silent fall-back to an un-pinned, redirect-following default client.
pub(crate) fn blocking_client() -> Option<&'static reqwest::blocking::Client> {
    static CLIENT: OnceLock<Option<reqwest::blocking::Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::blocking::Client::builder()
                .dns_resolver(Arc::new(PinnedResolver))
                // LLM APIs never legitimately redirect; never forward auth headers across hops.
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(CLOUD_TIMEOUT_SECS))
                .build()
                .ok() // build failure -> None -> no client -> no egress (fail-closed)
        })
        .as_ref()
}

/// Which cloud provider arm to use. Canonical definition; `engine::reason`
/// re-exports it for config plumbing (`parse_reasoner_config` constructs the variants).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudProvider {
    Anthropic,
    OpenAiCompat,
}

/// Vault key names — MUST match the chat-provider names in `llm_stream.rs`
/// (the reasoner SHARES the chat key; the R1 consent binds its fingerprint so a
/// rotation/provider-change re-consents). Keep these strings in sync.
pub(crate) const ANTHROPIC_KEY_NAME: &str = "anthropic_api_key";
pub(crate) const OPENAI_COMPAT_KEY_NAME: &str = "openai_compat_api_key";
pub(crate) const ANTHROPIC_HOST: &str = "api.anthropic.com";

/// The desktop-side cloud reasoner (spec §8). Reads the provider key from the
/// vault AT CALL TIME (header-only, never stored on the struct, never logged),
/// connects via the hardened blocking client, and returns parsed structured
/// JSON — or a body-scrubbed `BossclawError::Reasoner` (a retryable no-op tick).
// `new` is reached via `build_reasoner` (the `ConfigReasonerProvider` + the R5 enable probe),
// so the whole graph — this struct, its methods, and the request/extract/client helpers they
// transitively reach — is live in the bin target.
pub struct CloudReasoner {
    provider: CloudProvider,
    model: String,
    /// OpenAI-compat only; already HTTPS-normalized + host-screened at config-set (Task 12).
    base_url: Option<String>,
    model_id: String,
    /// TEST-ONLY key seam (compiled OUT of production via `#[cfg(test)]`, so it can
    /// never be a vault bypass that ships): `None` = read the real vault; `Some(None)`
    /// = force "no key" (deterministic fail-closed, no keychain touch, no network);
    /// `Some(Some(k))` = force this key. Exercised by the R5 missing-key test so it is
    /// network-free even on a dev machine that DOES have a configured provider key.
    #[cfg(test)]
    test_key_override: Option<Option<String>>,
}

impl CloudReasoner {
    pub fn new(provider: CloudProvider, model: String, base_url: Option<String>) -> Self {
        let prefix = match provider {
            CloudProvider::Anthropic => "anthropic",
            CloudProvider::OpenAiCompat => "openai-compat",
        };
        let model_id = format!("{prefix}:{model}");
        Self {
            provider,
            model,
            base_url,
            model_id,
            // Production default: read the REAL vault. (Field exists only under cfg(test).)
            #[cfg(test)]
            test_key_override: None,
        }
    }

    /// TEST-ONLY constructor: mirrors [`Self::new`] but forces `read_key` to use
    /// `key` instead of the real vault (`None` = force missing key). Lets the R5
    /// missing-key test assert fail-closed deterministically with NO keychain
    /// access and NO network call, even where a provider key IS configured.
    #[cfg(test)]
    pub(crate) fn new_with_test_key(
        provider: CloudProvider,
        model: String,
        base_url: Option<String>,
        key: Option<String>,
    ) -> Self {
        Self { test_key_override: Some(key), ..Self::new(provider, model, base_url) }
    }

    fn key_name(&self) -> &'static str {
        match self.provider {
            CloudProvider::Anthropic => ANTHROPIC_KEY_NAME,
            CloudProvider::OpenAiCompat => OPENAI_COMPAT_KEY_NAME,
        }
    }

    /// Read the provider key from the vault at CALL time (never stored on self,
    /// never logged). Empty/missing/error -> Err (fail-closed).
    fn read_key(&self) -> Result<String, BossclawError> {
        // TEST-ONLY seam (compiled out of production): when set, bypass the real
        // vault read so a test forces "no key" (or a fixed key) without touching the
        // keychain — keeping the fail-closed test network-free. Never a production
        // bypass: the whole `if` is gated by `#[cfg(test)]`.
        #[cfg(test)]
        if let Some(override_key) = &self.test_key_override {
            return match override_key {
                Some(k) if !k.trim().is_empty() => Ok(k.clone()),
                _ => Err(BossclawError::Reasoner("cloud reasoner key missing in vault".into())),
            };
        }
        match crate::vault::secret_get_cached(self.key_name()) {
            Ok(Some(k)) if !k.trim().is_empty() => Ok(k),
            Ok(_) => Err(BossclawError::Reasoner("cloud reasoner key missing in vault".into())),
            Err(_) => Err(BossclawError::Reasoner("cloud reasoner key read failed".into())),
        }
    }

    fn anthropic_complete(
        &self,
        key: &str,
        system: &str,
        prompt: &str,
        schema: &Value,
    ) -> Result<Value, BossclawError> {
        let client = blocking_client()
            .ok_or_else(|| BossclawError::Reasoner("cloud reasoner client unavailable".into()))?;
        // Anthropic is pinned to the literal host; base_url is IGNORED (spec R2).
        let endpoint = format!("https://{ANTHROPIC_HOST}/v1/messages");
        let body = build_anthropic_request(&self.model, system, prompt, schema);
        let resp = client
            .post(&endpoint)
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01")
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .json(&body)
            .send()
            .map_err(|e| BossclawError::Reasoner(format!("anthropic transport: {e}")))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(classify_cloud_error(status.as_u16())); // body dropped (R3)
        }
        let payload: Value = resp
            .json()
            .map_err(|e| BossclawError::Reasoner(format!("anthropic response not JSON: {e}")))?;
        extract_anthropic_result(&payload)
    }

    fn openai_complete(
        &self,
        key: &str,
        system: &str,
        prompt: &str,
        schema: &Value,
    ) -> Result<Value, BossclawError> {
        let client = blocking_client()
            .ok_or_else(|| BossclawError::Reasoner("cloud reasoner client unavailable".into()))?;
        let base = self
            .base_url
            .as_deref()
            .ok_or_else(|| BossclawError::Reasoner("openai-compat base_url missing".into()))?;
        let endpoint = format!("{}/v1/chat/completions", base.trim_end_matches('/'));

        let send = |fallback: bool| -> Result<(reqwest::StatusCode, Value), BossclawError> {
            let body = build_openai_request(&self.model, system, prompt, schema, fallback);
            let resp = client
                .post(&endpoint)
                .header(reqwest::header::AUTHORIZATION, format!("Bearer {key}"))
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .json(&body)
                .send()
                .map_err(|e| BossclawError::Reasoner(format!("openai transport: {e}")))?;
            let status = resp.status();
            if status.is_success() {
                let v: Value = resp
                    .json()
                    .map_err(|e| BossclawError::Reasoner(format!("openai response not JSON: {e}")))?;
                Ok((status, v))
            } else {
                // Drain body for status only; classified later. Body dropped (R3).
                Ok((status, Value::Null))
            }
        };

        // Primary json_schema; on 400/404/422 retry once with json_object (R7).
        let (status, payload) = send(false)?;
        let (status, payload) = if matches!(status.as_u16(), 400 | 404 | 422) {
            send(true)?
        } else {
            (status, payload)
        };
        if !status.is_success() {
            return Err(classify_cloud_error(status.as_u16()));
        }
        extract_openai_result(&payload)
    }
}

impl Reasoner for CloudReasoner {
    fn complete_json(&self, system: &str, prompt: &str, schema: &Value) -> Result<Value, BossclawError> {
        let key = self.read_key()?;
        match self.provider {
            CloudProvider::Anthropic => self.anthropic_complete(&key, system, prompt, schema),
            CloudProvider::OpenAiCompat => self.openai_complete(&key, system, prompt, schema),
        }
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    fn sa(ip: [u8; 4]) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(ip[0], ip[1], ip[2], ip[3])), 443)
    }

    #[test]
    fn screen_addrs_rejects_any_blocked() {
        // All public -> Ok, preserved in order.
        let public = vec![sa([93, 184, 216, 34]), sa([1, 1, 1, 1])];
        assert!(screen_addrs(public.clone()).is_ok());

        // Loopback present -> Err (DNS-rebind primitive refused before connect).
        let with_loopback = vec![sa([93, 184, 216, 34]), sa([127, 0, 0, 1])];
        assert!(screen_addrs(with_loopback).is_err());

        // Cloud metadata -> Err.
        assert!(screen_addrs(vec![sa([169, 254, 169, 254])]).is_err());

        // Empty -> Err (nothing to connect to).
        assert!(screen_addrs(Vec::new()).is_err());
    }

    #[test]
    fn classify_cloud_error_never_leaks_body() {
        // A 400 body containing a fake prompt substring must not survive into the error string.
        let err = classify_cloud_error(400);
        let msg = err.to_string();
        assert!(msg.contains("bad_request"));
        assert!(msg.contains("400"));
        assert!(!msg.to_lowercase().contains("prompt"));
        assert!(!msg.to_lowercase().contains("memory"));

        assert!(classify_cloud_error(401).to_string().contains("auth_rejected"));
        assert!(classify_cloud_error(403).to_string().contains("auth_rejected"));
        assert!(classify_cloud_error(429).to_string().contains("rate_limited"));
        assert!(classify_cloud_error(404).to_string().contains("model_or_endpoint_not_found"));
        assert!(classify_cloud_error(503).to_string().contains("provider_5xx"));
        assert!(classify_cloud_error(418).to_string().contains("provider_error"));
    }

    #[test]
    fn anthropic_request_and_extract_roundtrip() {
        let schema = bossclaw_core::reason::adjudication_schema();
        let body = build_anthropic_request("claude-sonnet-4-6", "SYS", "PROMPT", &schema);

        assert_eq!(body["model"], "claude-sonnet-4-6");
        assert_eq!(body["max_tokens"], ANTHROPIC_MAX_TOKENS);
        assert_eq!(body["system"], "SYS");
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"], "PROMPT");
        // Forced single tool, schema verbatim, no strict/temperature/thinking.
        assert_eq!(body["tools"][0]["name"], ANTHROPIC_TOOL_NAME);
        assert_eq!(body["tools"][0]["input_schema"], schema);
        assert_eq!(body["tool_choice"]["type"], "tool");
        assert_eq!(body["tool_choice"]["name"], ANTHROPIC_TOOL_NAME);
        assert!(body.get("temperature").is_none());
        assert!(body.get("thinking").is_none());
        assert!(body["tools"][0].get("strict").is_none());

        // Extract the forced tool_use .input.
        let resp = serde_json::json!({
            "content": [
                { "type": "text", "text": "ignored" },
                { "type": "tool_use", "name": ANTHROPIC_TOOL_NAME, "input": { "match": "abc" } }
            ]
        });
        let out = extract_anthropic_result(&resp).expect("extract");
        assert_eq!(out["match"], "abc");

        // No tool_use block -> Err, not panic.
        let bad = serde_json::json!({ "content": [ { "type": "text", "text": "no tool" } ] });
        assert!(extract_anthropic_result(&bad).is_err());
    }

    #[test]
    fn openai_request_variants_and_tolerant_extract() {
        let schema = bossclaw_core::reason::extraction_schema();

        let primary = build_openai_request("gpt-5-mini", "SYS", "PROMPT", &schema, false);
        assert_eq!(primary["model"], "gpt-5-mini");
        assert_eq!(primary["messages"][0]["role"], "system");
        assert_eq!(primary["messages"][1]["role"], "user");
        assert_eq!(primary["response_format"]["type"], "json_schema");
        assert_eq!(primary["response_format"]["json_schema"]["schema"], schema);
        assert_eq!(primary["response_format"]["json_schema"]["strict"], false);
        assert!(primary.get("temperature").is_none());

        // Fallback body uses json_object and folds the schema into the system text.
        let fallback = build_openai_request("gpt-5-mini", "SYS", "PROMPT", &schema, true);
        assert_eq!(fallback["response_format"]["type"], "json_object");
        assert!(fallback["messages"][0]["content"].as_str().unwrap().contains("schema"));

        // Clean JSON content.
        let clean = serde_json::json!({
            "choices": [ { "message": { "content": "{\"match\":\"x\"}" } } ]
        });
        assert_eq!(extract_openai_result(&clean).unwrap()["match"], "x");

        // Fenced content -> stripped + parsed.
        let fenced = serde_json::json!({
            "choices": [ { "message": { "content": "```json\n{\"match\":\"y\"}\n```" } } ]
        });
        assert_eq!(extract_openai_result(&fenced).unwrap()["match"], "y");

        // Non-JSON content -> Err, not panic.
        let junk = serde_json::json!({
            "choices": [ { "message": { "content": "sorry, I cannot" } } ]
        });
        assert!(extract_openai_result(&junk).is_err());
    }

    #[test]
    fn blocking_client_builds_under_runtime_without_panic() {
        // Reproduce the engine's call context: a tokio runtime + spawn_blocking.
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let built = tokio::task::spawn_blocking(|| {
                // get_or_init must not panic with "runtime within a runtime".
                let _client = blocking_client();
                true
            })
            .await
            .unwrap();
            assert!(built);
        });
    }

    #[test]
    fn cloud_reasoner_model_id_is_provider_qualified() {
        let a = CloudReasoner::new(CloudProvider::Anthropic, "claude-sonnet-4-6".into(), None);
        assert_eq!(a.model_id(), "anthropic:claude-sonnet-4-6");
        let o = CloudReasoner::new(
            CloudProvider::OpenAiCompat,
            "gpt-5-mini".into(),
            Some("https://api.example.com".into()),
        );
        assert_eq!(o.model_id(), "openai-compat:gpt-5-mini");
    }

    #[test]
    fn cloud_reasoner_missing_key_fails_closed() {
        // R5 fail path: missing key -> complete_json returns Err (never a panic, never
        // egress). The test-key seam forces "no key" WITHOUT reading the real vault, so
        // this is deterministic and network-free even on a machine that HAS a configured
        // provider key (e.g. set via vault_set) — read_key returns Err before any send().
        let r = CloudReasoner::new_with_test_key(
            CloudProvider::Anthropic,
            "claude-sonnet-4-6".into(),
            None,
            None, // forced missing key
        );
        let schema = bossclaw_core::reason::adjudication_schema();
        let out = r.complete_json("sys", "prompt", &schema);
        assert!(out.is_err());
    }

    #[ignore = "live network; needs a real provider key in the vault"]
    #[test]
    fn cloud_reasoner_live_roundtrip() {
        let r = CloudReasoner::new(CloudProvider::Anthropic, "claude-sonnet-4-6".into(), None);
        let schema = bossclaw_core::reason::adjudication_schema();
        let out = r
            .complete_json("Return the chosen id.", "candidates: [a]. text: a", &schema)
            .unwrap();
        assert!(out.get("match").is_some());
    }
}
