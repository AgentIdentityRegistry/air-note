//! Desktop-side cloud reasoner (Anthropic + OpenAI-compat). The brain's first
//! deliberate network egress: off-by-default, fail-closed, host-pinned, signed
//! consent. Lives here (not bossclaw-core) because the engine crate's CI jail
//! forbids `reqwest`. See docs/superpowers/specs/2026-06-30-milestone-d2-cloud-reasoner-design.md §8.

// Implemented task-by-task in the Phase 2a plan.

use std::net::{SocketAddr, ToSocketAddrs};

use bossclaw_core::BossclawError;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use serde_json::{json, Value};

use crate::web_access::is_blocked_ip;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Pure SSRF screen: returns the addresses only if EVERY one is a safe public
/// destination; errors if any is internal/loopback/link-local/private/CGNAT/
/// metadata, or if the set is empty. Used at connect time to close the
/// DNS-rebind race that a pre-flight host check cannot (spec §8 R2).
// Exercised by `tests::screen_addrs_rejects_any_blocked` and called from
// `PinnedResolver::resolve`; the bin target compiles without `cfg(test)`, where
// `PinnedResolver` is itself dead until the Task 5 client wires it in.
#[allow(dead_code)]
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
#[allow(dead_code)] // Consumed by the Task 6 CloudReasoner when a provider call returns non-2xx.
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
#[allow(dead_code)] // Consumed by the Task 6 CloudReasoner when it builds the Anthropic request body.
pub(crate) const ANTHROPIC_MAX_TOKENS: u32 = 4096;
#[allow(dead_code)] // Consumed by the Task 6 CloudReasoner to name + select the forced tool.
pub(crate) const ANTHROPIC_TOOL_NAME: &str = "emit_result";

/// Build the Anthropic `/v1/messages` body that FORCES one structured tool call
/// whose input_schema is the engine schema. No `temperature`/`thinking`
/// (rejected with 400 on current Opus models) and no `strict` (engine schemas
/// lack `additionalProperties:false`). See spec §8 R7 + plan decision #2.
#[allow(dead_code)] // Consumed by the Task 6 CloudReasoner's Anthropic arm.
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
#[allow(dead_code)] // Consumed by the Task 6 CloudReasoner after the Anthropic call returns.
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
#[allow(dead_code)] // Consumed by the Task 6 CloudReasoner's OpenAI-compat arm.
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
#[allow(dead_code)] // Consumed by the Task 6 CloudReasoner after the OpenAI-compat call returns.
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
#[allow(dead_code)] // Private helper for `extract_openai_result`, consumed by Task 6.
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
#[allow(dead_code)] // Constructed by the Task 5 client builder (Arc::new(PinnedResolver)).
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
}
