// Copied from apps/desktop/src-tauri/src/engine/cloud_reasoner.rs (M1a Task 4); the in-app original is removed in Task 6.
// SECURITY: the fail-closed, host-pinned, body-scrubbing, SSRF-screened egress path is copied VERBATIM.
// The ONLY change is the SSRF-screen import: `crate::web_access::is_blocked_ip` → `crate::net_guard::is_blocked_ip`
// (the daemon copied that helper into `net_guard.rs`). Do NOT weaken any of the R1–R8 controls.

//! Desktop-side cloud reasoner (Anthropic + OpenAI-compat + Gemini). The brain's first
//! deliberate network egress: off-by-default, fail-closed, host-pinned, signed
//! consent. Lives here (not bossclaw-core) because the engine crate's CI jail
//! forbids `reqwest`.

use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use bossclaw_core::{BossclawError, Reasoner};
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use serde_json::{json, Value};

use crate::net_guard::is_blocked_ip;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Pure SSRF screen: returns the addresses only if EVERY one is a safe public
/// destination; errors if any is internal/loopback/link-local/private/CGNAT/
/// metadata, or if the set is empty. Used at connect time to close the
/// DNS-rebind race that a pre-flight host check cannot (spec §8 R2).
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
/// Forced-function name for the OpenAI-compat `tool_calls` fallback (§3.1 rung 3).
pub(crate) const OPENAI_TOOL_NAME: &str = "emit_result";

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

/// Build the OpenAI-compat `/v1/chat/completions` body for the `tool_calls`
/// fallback (rung 3): FORCE one function call whose `parameters` are the engine
/// schema, mirroring the Anthropic forced-tool arm. Some self-hosted compat
/// servers (llama.cpp, vLLM, LocalAI) reject both `response_format` modes but
/// support function-calling. No `response_format`/`temperature` (parity). §3.1.
pub(crate) fn build_openai_tool_request(model: &str, system: &str, prompt: &str, schema: &Value) -> Value {
    json!({
        "model": model,
        "stream": false,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": prompt }
        ],
        "tools": [ {
            "type": "function",
            "function": {
                "name": OPENAI_TOOL_NAME,
                "description": "Return the structured result for this request.",
                "parameters": schema
            }
        } ],
        "tool_choice": { "type": "function", "function": { "name": OPENAI_TOOL_NAME } }
    })
}

/// Pull the forced tool call's stringified `arguments` out of an OpenAI-compat
/// response and parse it as JSON, tolerating a ```json fence. Missing tool_call
/// or parse failure -> `BossclawError::Reasoner` (retryable no-op tick).
pub(crate) fn extract_openai_tool_result(resp: &Value) -> Result<Value, BossclawError> {
    let arguments = resp
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("tool_calls"))
        .and_then(Value::as_array)
        .and_then(|t| t.first())
        .and_then(|t| t.get("function"))
        .and_then(|f| f.get("arguments"))
        .and_then(Value::as_str)
        .ok_or_else(|| BossclawError::Reasoner("openai response missing tool_call arguments".into()))?;
    let trimmed = strip_json_fence(arguments);
    serde_json::from_str(trimmed)
        .map_err(|e| BossclawError::Reasoner(format!("openai tool_call arguments not valid JSON: {e}")))
}

/// The OpenAI-compat structured-output fallback ladder (spec §3.1), factored out
/// so it is unit-testable with no network. `send` posts a request body and returns
/// `(http_status, payload)` — `payload` is `Value::Null` on a non-2xx (body dropped,
/// R3). Rungs, tried in order: (1) native `json_schema`, (2) `json_object` + schema
/// in the system text, (3) a FORCED function/tool call. Each rung falls through to
/// the next ONLY on 400/404/422 (a schema/endpoint reject); any other non-2xx is
/// classified immediately (no point re-sending an auth/rate/5xx failure). The
/// winning payload is parsed by the extractor matching the rung that produced it.
pub(crate) fn openai_fallback_ladder<F>(
    model: &str,
    system: &str,
    prompt: &str,
    schema: &Value,
    send: F,
) -> Result<Value, BossclawError>
where
    F: Fn(&Value) -> Result<(u16, Value), BossclawError>,
{
    let is_success = |s: u16| (200..300).contains(&s);
    let is_retryable = |s: u16| matches!(s, 400 | 404 | 422);

    // Rungs 1 + 2: response_format json_schema, then json_object. Both parse the
    // assistant message content. Fall through only on a retryable schema reject.
    for fallback in [false, true] {
        let (status, payload) = send(&build_openai_request(model, system, prompt, schema, fallback))?;
        if is_success(status) {
            return extract_openai_result(&payload);
        }
        if !is_retryable(status) {
            return Err(classify_cloud_error(status));
        }
    }

    // Rung 3: forced tool call (compat servers that reject both response_format modes).
    let (status, payload) = send(&build_openai_tool_request(model, system, prompt, schema))?;
    if is_success(status) {
        extract_openai_tool_result(&payload)
    } else {
        Err(classify_cloud_error(status))
    }
}

/// Build the Gemini `:generateContent` body. `fallback=false` uses `responseSchema` (strict);
/// `fallback=true` drops it and folds the schema into the system instruction (some engine schemas
/// use features Gemini's `responseSchema` subset rejects). `responseMimeType: application/json`
/// both ways. No `temperature` (parity with the other arms). The model rides in the URL path, not
/// the body. Spec §8 R7 + §3.1.
pub(crate) fn build_gemini_request(system: &str, prompt: &str, schema: &Value, fallback: bool) -> Value {
    let (system_text, generation_config) = if fallback {
        (
            format!(
                "{system}\n\nRespond with a single JSON object that conforms to this JSON schema:\n{schema}"
            ),
            json!({ "responseMimeType": "application/json" }),
        )
    } else {
        (
            system.to_string(),
            json!({ "responseMimeType": "application/json", "responseSchema": schema }),
        )
    };
    json!({
        "systemInstruction": { "parts": [ { "text": system_text } ] },
        "contents": [ { "role": "user", "parts": [ { "text": prompt } ] } ],
        "generationConfig": generation_config
    })
}

/// Pull `candidates[0].content.parts[0].text` and parse it as JSON, tolerating a ```json fence.
/// Parse failure -> `BossclawError::Reasoner` (retryable no-op).
pub(crate) fn extract_gemini_result(resp: &Value) -> Result<Value, BossclawError> {
    let text = resp
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
        .and_then(|c| c.get("content"))
        .and_then(|m| m.get("parts"))
        .and_then(Value::as_array)
        .and_then(|p| p.first())
        .and_then(|p| p.get("text"))
        .and_then(Value::as_str)
        .ok_or_else(|| BossclawError::Reasoner("gemini response missing candidate text".into()))?;
    let trimmed = strip_json_fence(text);
    serde_json::from_str(trimmed)
        .map_err(|e| BossclawError::Reasoner(format!("gemini content not valid JSON: {e}")))
}

/// A `reqwest` DNS resolver that screens every resolved address through
/// `is_blocked_ip` before any socket is opened. This is the connect-time pin
/// that closes the rebind race; installed on the blocking client.
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
/// client means no egress, never a silent fall-back to an un-pinned, redirect-
/// following default client.
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
    Gemini,
}

/// Vault key names — MUST match the chat-provider names in `llm_stream.rs`
/// (the reasoner SHARES the chat key; the R1 consent binds its fingerprint so a
/// rotation/provider-change re-consents). Keep these strings in sync.
pub(crate) const ANTHROPIC_KEY_NAME: &str = "anthropic_api_key";
pub(crate) const OPENAI_COMPAT_KEY_NAME: &str = "openai_compat_api_key";
pub(crate) const ANTHROPIC_HOST: &str = "api.anthropic.com";
/// Gemini shares the chat-side Google key (`llm_stream.rs` `GOOGLE_KEY_NAME`) and, like Anthropic,
/// a PINNED host (base_url is ignored). The key rides in the `x-goog-api-key` HEADER — NEVER the
/// URL query (spec R2 + no secrets in URLs).
pub(crate) const GEMINI_HOST: &str = "generativelanguage.googleapis.com";
pub(crate) const GEMINI_KEY_NAME: &str = "google_api_key";

/// The desktop-side cloud reasoner (spec §8). Reads the provider key from the
/// vault AT CALL TIME (header-only, never stored on the struct, never logged),
/// connects via the hardened blocking client, and returns parsed structured
/// JSON — or a body-scrubbed `BossclawError::Reasoner` (a retryable no-op tick).
pub struct CloudReasoner {
    provider: CloudProvider,
    model: String,
    /// OpenAI-compat only; already HTTPS-normalized + host-screened at config-set.
    base_url: Option<String>,
    model_id: String,
    /// TEST-ONLY key seam (compiled OUT of production via `#[cfg(test)]`, so it can
    /// never be a vault bypass that ships): `None` = read the real vault; `Some(None)`
    /// = force "no key" (deterministic fail-closed, no keychain touch, no network);
    /// `Some(Some(k))` = force this key.
    #[cfg(test)]
    test_key_override: Option<Option<String>>,
}

impl CloudReasoner {
    pub fn new(provider: CloudProvider, model: String, base_url: Option<String>) -> Self {
        let prefix = match provider {
            CloudProvider::Anthropic => "anthropic",
            CloudProvider::OpenAiCompat => "openai-compat",
            CloudProvider::Gemini => "gemini",
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
            CloudProvider::Gemini => GEMINI_KEY_NAME,
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

        // Post one body; return (status, payload) with the body dropped on non-2xx
        // (R3). The fallback ladder decides which rung to try and how to extract.
        let send = |body: &Value| -> Result<(u16, Value), BossclawError> {
            let resp = client
                .post(&endpoint)
                .header(reqwest::header::AUTHORIZATION, format!("Bearer {key}"))
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .json(body)
                .send()
                .map_err(|e| BossclawError::Reasoner(format!("openai transport: {e}")))?;
            let status = resp.status();
            if status.is_success() {
                let v: Value = resp
                    .json()
                    .map_err(|e| BossclawError::Reasoner(format!("openai response not JSON: {e}")))?;
                Ok((status.as_u16(), v))
            } else {
                Ok((status.as_u16(), Value::Null)) // body dropped (R3)
            }
        };

        openai_fallback_ladder(&self.model, system, prompt, schema, send)
    }

    fn gemini_complete(
        &self,
        key: &str,
        system: &str,
        prompt: &str,
        schema: &Value,
    ) -> Result<Value, BossclawError> {
        let client = blocking_client()
            .ok_or_else(|| BossclawError::Reasoner("cloud reasoner client unavailable".into()))?;
        // Gemini is pinned to the literal host; the model rides in the URL PATH and the key in the
        // `x-goog-api-key` HEADER — never the URL query (spec R2 + no secrets in URLs). base_url ignored.
        let endpoint =
            format!("https://{GEMINI_HOST}/v1beta/models/{}:generateContent", self.model);

        let send = |fallback: bool| -> Result<(reqwest::StatusCode, Value), BossclawError> {
            let body = build_gemini_request(system, prompt, schema, fallback);
            let resp = client
                .post(&endpoint)
                .header("x-goog-api-key", key)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .json(&body)
                .send()
                .map_err(|e| BossclawError::Reasoner(format!("gemini transport: {e}")))?;
            let status = resp.status();
            if status.is_success() {
                let v: Value = resp
                    .json()
                    .map_err(|e| BossclawError::Reasoner(format!("gemini response not JSON: {e}")))?;
                Ok((status, v))
            } else {
                Ok((status, Value::Null)) // body dropped (R3)
            }
        };

        // Primary responseSchema; on 400/404/422 retry once without it (schema-subset reject).
        let (status, payload) = send(false)?;
        let (status, payload) = if matches!(status.as_u16(), 400 | 404 | 422) {
            send(true)?
        } else {
            (status, payload)
        };
        if !status.is_success() {
            return Err(classify_cloud_error(status.as_u16()));
        }
        extract_gemini_result(&payload)
    }
}

impl Reasoner for CloudReasoner {
    fn complete_json(&self, system: &str, prompt: &str, schema: &Value) -> Result<Value, BossclawError> {
        let key = self.read_key()?;
        match self.provider {
            CloudProvider::Anthropic => self.anthropic_complete(&key, system, prompt, schema),
            CloudProvider::OpenAiCompat => self.openai_complete(&key, system, prompt, schema),
            CloudProvider::Gemini => self.gemini_complete(&key, system, prompt, schema),
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
    fn openai_tool_request_and_extract_roundtrip() {
        let schema = bossclaw_core::reason::extraction_schema();

        // Tool-call variant (fallback rung 3): forces ONE function call whose
        // parameters ARE the engine schema; no response_format, no temperature
        // (parity with the Anthropic forced-tool arm).
        let body = build_openai_tool_request("local-model", "SYS", "PROMPT", &schema);
        assert_eq!(body["model"], "local-model");
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][0]["content"], "SYS");
        assert_eq!(body["messages"][1]["role"], "user");
        assert_eq!(body["messages"][1]["content"], "PROMPT");
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["function"]["name"], OPENAI_TOOL_NAME);
        assert_eq!(body["tools"][0]["function"]["parameters"], schema);
        assert_eq!(body["tool_choice"]["type"], "function");
        assert_eq!(body["tool_choice"]["function"]["name"], OPENAI_TOOL_NAME);
        assert!(body.get("response_format").is_none());
        assert!(body.get("temperature").is_none());

        // Extract the forced tool call's stringified `arguments`.
        let clean = serde_json::json!({
            "choices": [ { "message": { "tool_calls": [
                { "type": "function", "function": { "name": OPENAI_TOOL_NAME, "arguments": "{\"match\":\"z\"}" } }
            ] } } ]
        });
        assert_eq!(extract_openai_tool_result(&clean).unwrap()["match"], "z");

        // Fenced arguments -> stripped + parsed (reuses strip_json_fence).
        let fenced = serde_json::json!({
            "choices": [ { "message": { "tool_calls": [
                { "function": { "name": OPENAI_TOOL_NAME, "arguments": "```json\n{\"match\":\"q\"}\n```" } }
            ] } } ]
        });
        assert_eq!(extract_openai_tool_result(&fenced).unwrap()["match"], "q");

        // No tool_calls at all -> Err, not panic.
        let none = serde_json::json!({ "choices": [ { "message": { "content": "no tools" } } ] });
        assert!(extract_openai_tool_result(&none).is_err());

        // tool_calls present but empty array -> Err, not panic (.first() is None).
        let empty = serde_json::json!({ "choices": [ { "message": { "tool_calls": [] } } ] });
        assert!(extract_openai_tool_result(&empty).is_err());

        // tool_calls present but arguments not valid JSON -> Err, not panic.
        let junk = serde_json::json!({
            "choices": [ { "message": { "tool_calls": [ { "function": { "arguments": "nope" } } ] } } ]
        });
        assert!(extract_openai_tool_result(&junk).is_err());
    }

    #[test]
    fn openai_fallback_ladder_walks_schema_object_then_tool() {
        use std::cell::RefCell;
        use std::collections::VecDeque;
        let schema = bossclaw_core::reason::adjudication_schema();

        // Drive the ladder with a scripted sequence of (status, payload) responses,
        // recording which rung's body was sent each time — no network.
        let run = |responses: Vec<(u16, Value)>| -> (Result<Value, BossclawError>, Vec<&'static str>) {
            let queue = RefCell::new(VecDeque::from(responses));
            let modes: RefCell<Vec<&'static str>> = RefCell::new(Vec::new());
            let out = openai_fallback_ladder("m", "SYS", "PROMPT", &schema, |body| {
                let mode = if body.get("tools").is_some() {
                    "tool"
                } else if body["response_format"]["type"] == "json_object" {
                    "object"
                } else {
                    "schema"
                };
                modes.borrow_mut().push(mode);
                Ok(queue.borrow_mut().pop_front().expect("sender called more times than scripted"))
            });
            (out, modes.into_inner())
        };
        let content = |s: &str| serde_json::json!({ "choices": [ { "message": { "content": s } } ] });
        let tool = |s: &str| serde_json::json!({
            "choices": [ { "message": { "tool_calls": [
                { "function": { "name": OPENAI_TOOL_NAME, "arguments": s } }
            ] } } ]
        });

        // Rung 1 (json_schema) succeeds -> one call, parsed from message.content.
        let (out, modes) = run(vec![(200, content("{\"match\":\"a\"}"))]);
        assert_eq!(out.unwrap()["match"], "a");
        assert_eq!(modes, vec!["schema"]);

        // Rung 1 retryable (400) -> rung 2 (json_object) succeeds.
        let (out, modes) = run(vec![(400, Value::Null), (200, content("{\"match\":\"b\"}"))]);
        assert_eq!(out.unwrap()["match"], "b");
        assert_eq!(modes, vec!["schema", "object"]);

        // Rungs 1+2 retryable -> rung 3 (tool_calls) succeeds, parsed via the tool extractor.
        let (out, modes) = run(vec![(400, Value::Null), (422, Value::Null), (200, tool("{\"match\":\"c\"}"))]);
        assert_eq!(out.unwrap()["match"], "c");
        assert_eq!(modes, vec!["schema", "object", "tool"]);

        // Non-retryable status on rung 1 -> classified immediately, NO fallback.
        let (out, modes) = run(vec![(401, Value::Null)]);
        assert!(out.unwrap_err().to_string().contains("auth_rejected"));
        assert_eq!(modes, vec!["schema"]);

        // Every rung returns a retryable error -> the final rung is classified as Err.
        let (out, modes) = run(vec![(400, Value::Null), (404, Value::Null), (422, Value::Null)]);
        assert!(out.is_err());
        assert_eq!(modes, vec!["schema", "object", "tool"]);
    }

    #[test]
    fn gemini_request_variants_and_tolerant_extract() {
        let schema = bossclaw_core::reason::extraction_schema();

        // Primary: responseMimeType json + responseSchema verbatim; system + user parts; no temperature.
        let primary = build_gemini_request("SYS", "PROMPT", &schema, false);
        assert_eq!(primary["systemInstruction"]["parts"][0]["text"], "SYS");
        assert_eq!(primary["contents"][0]["role"], "user");
        assert_eq!(primary["contents"][0]["parts"][0]["text"], "PROMPT");
        assert_eq!(primary["generationConfig"]["responseMimeType"], "application/json");
        assert_eq!(primary["generationConfig"]["responseSchema"], schema);
        assert!(primary["generationConfig"].get("temperature").is_none());

        // Fallback: NO responseSchema; schema folded into the system instruction text.
        let fallback = build_gemini_request("SYS", "PROMPT", &schema, true);
        assert_eq!(fallback["generationConfig"]["responseMimeType"], "application/json");
        assert!(fallback["generationConfig"].get("responseSchema").is_none());
        assert!(fallback["systemInstruction"]["parts"][0]["text"].as_str().unwrap().contains("schema"));

        // Clean JSON in the candidate part.
        let clean = serde_json::json!({
            "candidates": [ { "content": { "parts": [ { "text": "{\"match\":\"x\"}" } ] } } ]
        });
        assert_eq!(extract_gemini_result(&clean).unwrap()["match"], "x");

        // Fenced content -> stripped + parsed (reuses strip_json_fence).
        let fenced = serde_json::json!({
            "candidates": [ { "content": { "parts": [ { "text": "```json\n{\"match\":\"y\"}\n```" } ] } } ]
        });
        assert_eq!(extract_gemini_result(&fenced).unwrap()["match"], "y");

        // No candidates / non-JSON -> Err, not panic.
        assert!(extract_gemini_result(&serde_json::json!({ "candidates": [] })).is_err());
        let junk = serde_json::json!({ "candidates": [ { "content": { "parts": [ { "text": "nope" } ] } } ] });
        assert!(extract_gemini_result(&junk).is_err());
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
        let g = CloudReasoner::new(CloudProvider::Gemini, "gemini-2.5-flash".into(), None);
        assert_eq!(g.model_id(), "gemini:gemini-2.5-flash");
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

    // Live end-to-end egress check against the real provider. Env-fed (NOT the
    // vault) via the `#[cfg(test)]` key seam so it runs from the CLI/CI without a
    // keychain-approval hang — the never-otherwise-exercised part is the live HTTP
    // request/response roundtrip through the production request-builder, hardened
    // SSRF client, and extractor.
    // Run: AIR_LIVE_ANTHROPIC_KEY=sk-... \
    //   cargo test -p bossclawd cloud_reasoner_live_roundtrip -- --ignored --nocapture
    #[ignore = "live network; needs AIR_LIVE_ANTHROPIC_KEY env var"]
    #[test]
    fn cloud_reasoner_live_roundtrip() {
        let key = std::env::var("AIR_LIVE_ANTHROPIC_KEY")
            .expect("set AIR_LIVE_ANTHROPIC_KEY to run the live roundtrip");
        let model = std::env::var("AIR_LIVE_ANTHROPIC_MODEL")
            .unwrap_or_else(|_| "claude-haiku-4-5-20251001".to_string());
        let r =
            CloudReasoner::new_with_test_key(CloudProvider::Anthropic, model, None, Some(key));
        let schema = bossclaw_core::reason::adjudication_schema();
        let out = r
            .complete_json("Return the chosen id.", "candidates: [a]. text: a", &schema)
            .unwrap();
        assert!(out.get("match").is_some());
    }
}
