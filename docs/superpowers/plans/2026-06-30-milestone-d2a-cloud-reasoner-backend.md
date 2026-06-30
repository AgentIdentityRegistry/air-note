# Milestone D Phase 2a — Cloud Reasoner Backend — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a desktop-side `CloudReasoner` (Anthropic + OpenAI-compatible) behind the existing `Reasoner` seam so the brain's evolve loop *can* run against a cloud LLM — built off-by-default, fail-closed, host-pinned, with a signed-engine-log consent gate, so that **merging this PR sends zero bytes off-box** (no enable UI ships until Phase 2b).

**Architecture:** `CloudReasoner` lives in the desktop crate (`air_agent_desktop`), which already links `reqwest` — `bossclaw-core` is forbidden `reqwest` by the CI jail, so the cloud client *must* be desktop-side. It implements the engine's sync `bossclaw_core::Reasoner` trait (`complete_json` + `model_id`), exactly like `OllamaReasoner`. A new `ConfigReasonerProvider` replaces the hardcoded Ollama provider at one `main.rs` line and rebuilds the reasoner when a config fingerprint changes. **Both** the non-security reasoner config *and* the egress consent record are signed `config` events in the engine event log (the same tamper-evident store the evolve switch uses) — there is no `settings.json` writer in the repo, so nothing webview-writable gates egress. Cloud readiness is fail-closed: a signed consent record must EXIST and MATCH the current `(provider, base_url_host, vault-key-fingerprint)` or evolve is gated off. SSRF is closed at connect time by a custom `reqwest::dns::Resolve` that runs the existing `is_blocked_ip` classifier on every resolved address (closing the DNS-rebind race), plus `redirect::Policy::none()`.

**Tech Stack:** Rust, `reqwest::blocking` (new `blocking` feature), `serde_json`, `std::sync::OnceLock`, ed25519 signing (existing engine log), Tauri commands, `tauri::test` IPC harness, `cargo test`/`clippy`.

**Authoritative source:** `docs/superpowers/specs/2026-06-30-milestone-d2-cloud-reasoner-design.md` §8 (R1–R9). This plan implements R1–R9; where R1–R9 conflict with Rev 1 text, R1–R9 win.

---

## Decisions locked before build (from spec §8 + code-surface mapping 2026-06-30)

1. **Config home (Peter, 2026-06-30):** non-security config (`mode`, `provider`, `model`, `base_url`) lives in the **signed engine log** as a `config` event, *not* `settings.json` (which has no Rust writer — `store_write` is unimplemented and `tauri_plugin_store` is not loaded). The R1 consent record is a separate signed `config` event. One mechanism, fully signed. The chat-provider `SettingsRecord` (`llm_stream.rs:41`) is a separate concern and is **not** touched.
2. **No `temperature` on the Anthropic arm (R7 build-time pin).** Spec §3.1 said `temperature: 0`, but Opus 4.8/4.7/Fable 5 **reject** `temperature` with HTTP 400. Determinism of *values* is not required (the engine clamps `confidence` → `confidence_milli` before signing); determinism of *shape* comes from forced tool-use. So omit `temperature` (and `thinking`) on the Anthropic arm. The OpenAI-compat arm also omits `temperature` for parity/robustness.
3. **CloudReasoner is desktop-side** (`apps/desktop/src-tauri/src/engine/cloud_reasoner.rs`) using `reqwest::blocking`. The `blocking` feature is **not currently enabled** — Task 0 adds it.
4. **Retractions keep the manual-producer tag (pre-existing).** `invalidate()` stamps `MANUAL_LINK_PRODUCER`, not `reasoner.model_id()`. 2a does **not** change this; do not "fix" it.

---

## File structure

**New files**
- `apps/desktop/src-tauri/src/engine/cloud_reasoner.rs` — `CloudReasoner` (impl `Reasoner`), the hardened global `reqwest::blocking::Client`, the `PinnedResolver` (SSRF), per-provider request build + response extract, `classify_cloud_error`. One responsibility: *the egress boundary*.

**Modified files**
- `apps/desktop/src-tauri/Cargo.toml` — add `"blocking"` to `reqwest` features.
- `apps/desktop/src-tauri/src/web_access.rs` — make `is_blocked_ip` `pub(crate)` so the resolver can reuse it.
- `apps/desktop/src-tauri/src/engine/reason.rs` — add `ConfigReasonerProvider` (fingerprint-keyed cache) alongside `OllamaReasonerProvider`; add the `ReasonerConfig`/`ReasonerMode`/`CloudProvider` DTOs + `config_fingerprint`.
- `apps/desktop/src-tauri/src/engine/mod.rs` — engine async wrappers: `reasoner_config`, `set_reasoner_config`, `cloud_reasoner_consent`, `enable_cloud_reasoner` (R5), `reasoner_ready`; extend `record_tick` for the R4 0-item-cloud-tick backstop.
- `apps/desktop/src-tauri/src/engine/scheduler.rs` — cloud-vs-local readiness branch feeding `decide_tick`.
- `apps/desktop/src-tauri/src/engine/mod.rs` (module list) + `apps/desktop/src-tauri/src/engine.rs` — declare `cloud_reasoner` module.
- `apps/desktop/src-tauri/src/commands/engine.rs` — `engine_get_reasoner_config`, `engine_set_reasoner_config`, `engine_enable_cloud_reasoner` (+ IPC tests).
- `apps/desktop/src-tauri/src/main.rs` — register the 3 commands; swap `OllamaReasonerProvider` → `ConfigReasonerProvider`.
- `crates/bossclaw-core/src/log.rs` — `REASONER_CONFIG_KEY`/`CLOUD_REASONER_CONSENT_KEY` consts; `ConfigFlag::{ReasonerConfig,CloudReasonerConsent}` + `key()` arms; `set_reasoner_config`/`reasoner_config_json`, `set_cloud_reasoner_consent`/`cloud_reasoner_consent_json` (default **closed**).
- `crates/bossclaw-core/src/evolve.rs` — add `tainted_recall_snippets: usize` to `EvolveReport`.
- `crates/bossclaw-core/src/log.rs` (`evolve_once`) — count `is_external` snippets in the recall set (R4 hook).

---

## Conventions for every task

- Engine crate is `bossclaw-core`; desktop crate is `air_agent_desktop` at `apps/desktop/src-tauri/`.
- Desktop tests run under `cargo test -p air_agent_desktop`; engine tests under `cargo test -p bossclaw-core`. Desktop engine code is `#[cfg(unix)]`-gated — run desktop tests on macOS/Linux.
- After each task: `cargo clippy --all-targets -- -D warnings` for the touched crate, then commit.
- The repo-wide `cargo audit --deny warnings` gate runs in CI; no new advisories are expected from this plan (no new crates beyond a reqwest feature flag).

---

### Task 0: Enable `reqwest::blocking`, expose `is_blocked_ip`, declare the module

**Files:**
- Modify: `apps/desktop/src-tauri/Cargo.toml` (the `reqwest` line, currently `Cargo.toml:20`)
- Modify: `apps/desktop/src-tauri/src/web_access.rs:113` (visibility of `is_blocked_ip`)
- Modify: `apps/desktop/src-tauri/src/engine.rs` (or the `mod` list in `engine/mod.rs`) — declare `pub mod cloud_reasoner;`
- Create: `apps/desktop/src-tauri/src/engine/cloud_reasoner.rs` (stub)

- [ ] **Step 1: Add the `blocking` feature to reqwest**

In `apps/desktop/src-tauri/Cargo.toml`, change the reqwest dependency line (currently):
```toml
reqwest = { version = "0.12", features = ["json", "rustls-tls"] }
```
to:
```toml
reqwest = { version = "0.12", features = ["json", "rustls-tls", "blocking"] }
```

- [ ] **Step 2: Make `is_blocked_ip` crate-visible**

In `apps/desktop/src-tauri/src/web_access.rs`, change the signature at line 113 from:
```rust
fn is_blocked_ip(ip: IpAddr) -> bool {
```
to:
```rust
pub(crate) fn is_blocked_ip(ip: IpAddr) -> bool {
```
(Body unchanged — it already blocks loopback/private/link-local/`169.254.169.254`/CGNAT/ULA/NAT64/IPv4-mapped-v6.)

- [ ] **Step 3: Declare the new module**

Find where `engine` submodules are declared (the desktop crate uses `apps/desktop/src-tauri/src/engine.rs` or a `mod` block in `engine/mod.rs`; grep `mod reason;` to locate it) and add next to `mod reason;`:
```rust
#[cfg(unix)]
pub mod cloud_reasoner;
```

- [ ] **Step 4: Create the stub module so the crate compiles**

Create `apps/desktop/src-tauri/src/engine/cloud_reasoner.rs` with:
```rust
//! Desktop-side cloud reasoner (Anthropic + OpenAI-compat). The brain's first
//! deliberate network egress: off-by-default, fail-closed, host-pinned, signed
//! consent. Lives here (not bossclaw-core) because the engine crate's CI jail
//! forbids `reqwest`. See docs/superpowers/specs/2026-06-30-milestone-d2-cloud-reasoner-design.md §8.

// Implemented task-by-task in the Phase 2a plan.
```

- [ ] **Step 5: Verify the crate builds**

Run: `cd ~/air-note && cargo build -p air_agent_desktop`
Expected: builds clean (the new feature compiles `reqwest::blocking` in; the stub module is empty).

- [ ] **Step 6: Commit**

```bash
git add apps/desktop/src-tauri/Cargo.toml apps/desktop/src-tauri/Cargo.lock apps/desktop/src-tauri/src/web_access.rs apps/desktop/src-tauri/src/engine.rs apps/desktop/src-tauri/src/engine/cloud_reasoner.rs
git commit -m "chore(reasoner): enable reqwest blocking, expose is_blocked_ip, scaffold cloud_reasoner module"
```

---

### Task 1: SSRF — `screen_addrs` (pure) + `PinnedResolver` (R2)

**Files:**
- Modify: `apps/desktop/src-tauri/src/engine/cloud_reasoner.rs`
- Test: same file, `#[cfg(test)] mod tests`

**Design:** factor the *decision* into a pure `screen_addrs(addrs) -> Result<Vec<SocketAddr>, BlockedAddr>` (unit-tested), then wire it into a `reqwest::dns::Resolve` that does the OS lookup off-thread and screens the result. "Refuse if ANY resolved address is blocked" (R2).

- [ ] **Step 1: Write the failing test**

Add to `cloud_reasoner.rs`:
```rust
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
}
```

- [ ] **Step 2: Run it (fails to compile — `screen_addrs` undefined)**

Run: `cargo test -p air_agent_desktop screen_addrs_rejects_any_blocked`
Expected: FAIL — `cannot find function screen_addrs`.

- [ ] **Step 3: Implement `screen_addrs` + `PinnedResolver`**

Add to `cloud_reasoner.rs` (top of file, after the doc comment):
```rust
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

use crate::web_access::is_blocked_ip;

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
```

- [ ] **Step 4: Run the test (passes)**

Run: `cargo test -p air_agent_desktop screen_addrs_rejects_any_blocked`
Expected: PASS.

- [ ] **Step 5: Lint + commit**

```bash
cargo clippy -p air_agent_desktop --all-targets -- -D warnings
git add apps/desktop/src-tauri/src/engine/cloud_reasoner.rs
git commit -m "feat(reasoner): connect-time SSRF DNS resolver reusing is_blocked_ip (spec R2)"
```

---

### Task 2: `classify_cloud_error` — drop raw bodies (R3)

**Files:**
- Modify: `apps/desktop/src-tauri/src/engine/cloud_reasoner.rs`
- Test: same file

**Why:** a provider 400 body can echo the prompt (= memory/file bytes). The surfaced error must be a fixed taxonomy + status only; the raw body is consumed for classification then dropped — never embedded.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module:
```rust
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
```

- [ ] **Step 2: Run it (fails — `classify_cloud_error` undefined)**

Run: `cargo test -p air_agent_desktop classify_cloud_error_never_leaks_body`
Expected: FAIL.

- [ ] **Step 3: Implement**

Add to `cloud_reasoner.rs`:
```rust
use bossclaw_core::BossclawError;

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
```

- [ ] **Step 4: Run the test (passes)**

Run: `cargo test -p air_agent_desktop classify_cloud_error_never_leaks_body`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src-tauri/src/engine/cloud_reasoner.rs
git commit -m "feat(reasoner): classify_cloud_error drops raw provider bodies (spec R3)"
```

---

### Task 3: Anthropic arm — request build + response extract

**Files:**
- Modify: `apps/desktop/src-tauri/src/engine/cloud_reasoner.rs`
- Test: same file

**Wire shape (pinned vs current docs):** forced tool-use. ONE tool whose `input_schema` is the engine schema; `tool_choice` forces it; `max_tokens` required; **no** `temperature`/`thinking`; **no** `strict` (engine schemas lack `additionalProperties:false`). Read the forced `tool_use` block's `.input`.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module:
```rust
#[test]
fn anthropic_request_and_extract_roundtrip() {
    let schema = bossclaw_core::adjudication_schema();
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
```

- [ ] **Step 2: Run it (fails)**

Run: `cargo test -p air_agent_desktop anthropic_request_and_extract_roundtrip`
Expected: FAIL — undefined items.

- [ ] **Step 3: Implement**

Add to `cloud_reasoner.rs`:
```rust
use serde_json::{json, Value};

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
```

- [ ] **Step 4: Run the test (passes)**

Run: `cargo test -p air_agent_desktop anthropic_request_and_extract_roundtrip`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src-tauri/src/engine/cloud_reasoner.rs
git commit -m "feat(reasoner): anthropic forced tool-use request + extract (no temperature, spec R7)"
```

---

### Task 4: OpenAI-compat arm — request build + extract + fenced-parse fallback

**Files:**
- Modify: `apps/desktop/src-tauri/src/engine/cloud_reasoner.rs`
- Test: same file

**Wire shape:** primary `response_format: json_schema` (strict:false); the network-level fallback to `json_object` on 400/404/422 happens in `complete_json` (Task 6) — here we build both bodies and a tolerant extractor that strips a ```json fence.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module:
```rust
#[test]
fn openai_request_variants_and_tolerant_extract() {
    let schema = bossclaw_core::extraction_schema();

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
```

- [ ] **Step 2: Run it (fails)**

Run: `cargo test -p air_agent_desktop openai_request_variants_and_tolerant_extract`
Expected: FAIL.

- [ ] **Step 3: Implement**

Add to `cloud_reasoner.rs`:
```rust
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
                "{system}\n\nRespond with a single JSON object that conforms to this JSON Schema:\n{schema}"
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
```

- [ ] **Step 4: Run the test (passes)**

Run: `cargo test -p air_agent_desktop openai_request_variants_and_tolerant_extract`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src-tauri/src/engine/cloud_reasoner.rs
git commit -m "feat(reasoner): openai-compat json_schema request + json_object fallback + fenced extract (spec R7)"
```

---

### Task 5: Hardened global blocking client (R6 + R2) + no-runtime-panic test

**Files:**
- Modify: `apps/desktop/src-tauri/src/engine/cloud_reasoner.rs`
- Test: same file

**Why a process-global `OnceLock`:** build the `reqwest::blocking::Client` exactly once, lazily, ON the `spawn_blocking` thread (never capturing the async runtime) — a per-tick "runtime within a runtime" panic would be a silent permanent evolve outage (R6). The client carries the `PinnedResolver`, `redirect::Policy::none()`, and the timeout; only the key/URL/body vary per call.

- [ ] **Step 1: Write the failing test (R6: construct under a runtime + spawn_blocking, no panic)**

Add to the `tests` module:
```rust
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
```

- [ ] **Step 2: Run it (fails — `blocking_client` undefined)**

Run: `cargo test -p air_agent_desktop blocking_client_builds_under_runtime_without_panic`
Expected: FAIL.

- [ ] **Step 3: Implement**

Add to `cloud_reasoner.rs`:
```rust
use std::sync::OnceLock;
use std::time::Duration;

/// Request timeout: parity with OLLAMA_TIMEOUT_SECS (120s). The reasoner holds
/// the evolve_lock during a tick, so a hung call self-DoSes the tick (spec R6).
const CLOUD_TIMEOUT_SECS: u64 = 120;

static CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();

/// The one hardened blocking client (connection pool + SSRF resolver + no
/// redirects + timeout), built lazily on first use. First use is always on a
/// `spawn_blocking` thread, so no async runtime is captured (spec R6).
pub(crate) fn blocking_client() -> &'static reqwest::blocking::Client {
    CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .dns_resolver(Arc::new(PinnedResolver))
            // LLM APIs never legitimately redirect; never forward auth headers across hops.
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(CLOUD_TIMEOUT_SECS))
            .build()
            // A client we cannot build means we cannot egress: that is the safe
            // direction. Fall back to a default client only for the (impossible
            // in practice) builder failure, so the type stays infallible here.
            .unwrap_or_else(|_| reqwest::blocking::Client::new())
    })
}
```
> Note for the implementer: if `clippy` flags the `unwrap_or_else` default as a silent SSRF bypass risk, replace `blocking_client()` with a `Result`-returning `try_blocking_client()` and have `complete_json` map a build failure to `BossclawError::Reasoner("cloud client unavailable")`. Builder failure here is effectively unreachable (no proxy/TLS config that can fail), but prefer the `Result` form if the reviewer asks.

- [ ] **Step 4: Run the test (passes)**

Run: `cargo test -p air_agent_desktop blocking_client_builds_under_runtime_without_panic`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src-tauri/src/engine/cloud_reasoner.rs
git commit -m "feat(reasoner): hardened global blocking client (DNS pin, no redirects, timeout, OnceLock) (spec R2/R6)"
```

---

### Task 6: `CloudReasoner` — `complete_json` + `model_id` (impl `Reasoner`)

**Files:**
- Modify: `apps/desktop/src-tauri/src/engine/cloud_reasoner.rs`
- Test: same file (pure pieces already covered; add `model_id` + a vault-missing-key test; live round-trip is `#[ignore]`)

**Behavior:** read the key from the vault at call time (header-only); build per provider; send via `blocking_client()`; on non-2xx → `classify_cloud_error`; on OpenAI 400/404/422 → retry once with the `json_object` fallback body; extract.

- [ ] **Step 1: Write the failing tests (model_id + missing-key fail-closed)**

Add to the `tests` module:
```rust
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
    // No vault key present -> complete_json returns Err (never a panic, never egress).
    let r = CloudReasoner::new(CloudProvider::Anthropic, "claude-sonnet-4-6".into(), None);
    let schema = bossclaw_core::adjudication_schema();
    let out = r.complete_json("sys", "prompt", &schema);
    assert!(out.is_err());
}

#[ignore = "live network; needs a real provider key in the vault"]
#[test]
fn cloud_reasoner_live_roundtrip() {
    let r = CloudReasoner::new(CloudProvider::Anthropic, "claude-sonnet-4-6".into(), None);
    let schema = bossclaw_core::adjudication_schema();
    let out = r.complete_json("Return the chosen id.", "candidates: [a]. text: a", &schema).unwrap();
    assert!(out.get("match").is_some());
}
```
> `CloudProvider` is defined in Task 9 (`engine/reason.rs`) and re-exported. For this task, temporarily define `CloudProvider` here and move it in Task 9, OR sequence Task 9's enum before this — implementer's choice; the plan defines the enum canonically in Task 9. Simplest: add the enum now in `cloud_reasoner.rs` and have Task 9 `pub use` it.

- [ ] **Step 2: Run it (fails)**

Run: `cargo test -p air_agent_desktop cloud_reasoner_`
Expected: FAIL — `CloudReasoner`/`CloudProvider` undefined.

- [ ] **Step 3: Implement the enum, struct, and `Reasoner` impl**

Add to `cloud_reasoner.rs`:
```rust
use bossclaw_core::Reasoner;

/// Which cloud provider arm to use. Canonical definition; `engine::reason`
/// re-exports it for config plumbing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudProvider {
    Anthropic,
    OpenAiCompat,
}

/// Vault key names (shared with the chat providers; the R1 consent binds the
/// fingerprint so a rotation/provider-change re-consents). Matches llm_stream.rs.
const ANTHROPIC_KEY_NAME: &str = "anthropic_api_key";
const OPENAI_COMPAT_KEY_NAME: &str = "openai_compat_api_key";
const ANTHROPIC_HOST: &str = "api.anthropic.com";

pub struct CloudReasoner {
    provider: CloudProvider,
    model: String,
    /// OpenAI-compat only; already HTTPS-normalized + host-screened at config-set.
    base_url: Option<String>,
    model_id: String,
}

impl CloudReasoner {
    pub fn new(provider: CloudProvider, model: String, base_url: Option<String>) -> Self {
        let prefix = match provider {
            CloudProvider::Anthropic => "anthropic",
            CloudProvider::OpenAiCompat => "openai-compat",
        };
        let model_id = format!("{prefix}:{model}");
        Self { provider, model, base_url, model_id }
    }

    fn key_name(&self) -> &'static str {
        match self.provider {
            CloudProvider::Anthropic => ANTHROPIC_KEY_NAME,
            CloudProvider::OpenAiCompat => OPENAI_COMPAT_KEY_NAME,
        }
    }

    /// Read the provider key from the vault at CALL time (never stored on self,
    /// never logged). Empty/missing -> Err (fail-closed).
    fn read_key(&self) -> Result<String, BossclawError> {
        match crate::vault::secret_get_cached(self.key_name()) {
            Ok(Some(k)) if !k.trim().is_empty() => Ok(k),
            Ok(_) => Err(BossclawError::Reasoner("cloud reasoner key missing in vault".into())),
            Err(_) => Err(BossclawError::Reasoner("cloud reasoner key read failed".into())),
        }
    }

    fn anthropic_complete(&self, key: &str, system: &str, prompt: &str, schema: &Value) -> Result<Value, BossclawError> {
        // Anthropic is pinned to the literal host; base_url is IGNORED (spec R2).
        let endpoint = format!("https://{ANTHROPIC_HOST}/v1/messages");
        let body = build_anthropic_request(&self.model, system, prompt, schema);
        let resp = blocking_client()
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

    fn openai_complete(&self, key: &str, system: &str, prompt: &str, schema: &Value) -> Result<Value, BossclawError> {
        let base = self
            .base_url
            .as_deref()
            .ok_or_else(|| BossclawError::Reasoner("openai-compat base_url missing".into()))?;
        let endpoint = format!("{}/v1/chat/completions", base.trim_end_matches('/'));

        let send = |fallback: bool| -> Result<(reqwest::StatusCode, Value), BossclawError> {
            let body = build_openai_request(&self.model, system, prompt, schema, fallback);
            let resp = blocking_client()
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
                // Drain body for status only; classify later. Body dropped (R3).
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
```

- [ ] **Step 4: Run the tests (the two non-ignored pass)**

Run: `cargo test -p air_agent_desktop cloud_reasoner_`
Expected: `cloud_reasoner_model_id_is_provider_qualified` PASS, `cloud_reasoner_missing_key_fails_closed` PASS, `cloud_reasoner_live_roundtrip` IGNORED.

- [ ] **Step 5: Lint + commit**

```bash
cargo clippy -p air_agent_desktop --all-targets -- -D warnings
git add apps/desktop/src-tauri/src/engine/cloud_reasoner.rs
git commit -m "feat(reasoner): CloudReasoner complete_json + model_id (header-only key, fail-closed, R3/R7)"
```

---

### Task 7: Signed config + consent in the engine log (R1)

**Files:**
- Modify: `crates/bossclaw-core/src/log.rs` (consts near `:196`; `ConfigFlag` at `:202`/`:214`; clone setter/getter after `set_evolve_enabled` at `:5025` / `evolve_enabled` at `:5058`)
- Test: `crates/bossclaw-core/src/log.rs` `#[cfg(test)]` (or the existing log test module)

**Design:** two new `config`-event keys. `content` is arbitrary signed JSON, so we store the full record. Defaults are **closed** (absent → no config / no consent).

- [ ] **Step 1: Write the failing test**

Add to the bossclaw-core log tests (find the existing `mod tests` in `log.rs`; mirror an existing `set_evolve_enabled` round-trip test if present):
```rust
#[test]
fn reasoner_config_and_consent_roundtrip_signed() {
    let log = open_test_log(); // existing helper that builds an EventLog with a signer key
    // Defaults: absent -> None (fail-closed).
    assert!(log.reasoner_config_json().unwrap().is_none());
    assert!(log.cloud_reasoner_consent_json().unwrap().is_none());

    // Write non-security config.
    let cfg = serde_json::json!({
        "mode": "cloud", "provider": "anthropic",
        "model": "claude-sonnet-4-6", "base_url": null
    });
    log.set_reasoner_config(cfg.clone()).unwrap();
    assert_eq!(log.reasoner_config_json().unwrap().unwrap(), cfg);

    // Write the signed consent record.
    let consent = serde_json::json!({
        "provider": "anthropic", "base_url_host": "api.anthropic.com",
        "key_fingerprint": "deadbeef", "consented_at": "2026-06-30T00:00:00Z"
    });
    log.set_cloud_reasoner_consent(consent.clone()).unwrap();
    assert_eq!(log.cloud_reasoner_consent_json().unwrap().unwrap(), consent);

    // Newest write wins (sticky) and the whole chain still verifies (signed).
    log.verify_chain().unwrap();
}
```
> If `open_test_log`/`verify_chain` helper names differ, match the existing test conventions in `log.rs` (the evolve-switch tests already construct a signed `EventLog`).

- [ ] **Step 2: Run it (fails)**

Run: `cargo test -p bossclaw-core reasoner_config_and_consent_roundtrip_signed`
Expected: FAIL — undefined methods.

- [ ] **Step 3: Add the consts + `ConfigFlag` arms**

Near the other `*_KEY` consts (~`log.rs:196`):
```rust
const REASONER_CONFIG_KEY: &str = "reasoner_config";
const CLOUD_REASONER_CONSENT_KEY: &str = "cloud_reasoner_consent";
```
Extend the `ConfigFlag` enum (`log.rs:202`):
```rust
pub enum ConfigFlag {
    Evolve,
    Proposals,
    Mandates,
    ReasonerConfig,
    CloudReasonerConsent,
}
```
And its `key()` (`log.rs:214`) — add arms:
```rust
            ConfigFlag::ReasonerConfig => REASONER_CONFIG_KEY,
            ConfigFlag::CloudReasonerConsent => CLOUD_REASONER_CONSENT_KEY,
```

- [ ] **Step 4: Add the setters + getters (clone `set_evolve_enabled`/`evolve_enabled`)**

After `set_evolve_enabled` (`log.rs:5025`) add (the `content` map stores the whole JSON value under the key):
```rust
    /// Persist the non-security reasoner config (mode/provider/model/base_url)
    /// as a signed `config` event. Webview writes route through a command, not
    /// a file — keeps egress-adjacent config tamper-evident (spec R1, plan #1).
    pub fn set_reasoner_config(&self, config: serde_json::Value) -> Result<(), BossclawError> {
        self.append(Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: CONFIG_EVENT_TYPE.to_string(),
            content: serde_json::Value::Object({
                let mut m = serde_json::Map::new();
                m.insert(REASONER_CONFIG_KEY.to_string(), config);
                m
            }),
            model_meta: None,
            prev_hash: String::new(),
            hash: None,
            signed_by_did: self.signer_did(),
            signature: None,
        })?;
        Ok(())
    }

    /// The signed cloud-enable consent record, binding
    /// {provider, base_url_host, key_fingerprint, consented_at}. Written ONLY by
    /// the enable flow after the R5 test-key call succeeds.
    pub fn set_cloud_reasoner_consent(&self, record: serde_json::Value) -> Result<(), BossclawError> {
        self.append(Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: CONFIG_EVENT_TYPE.to_string(),
            content: serde_json::Value::Object({
                let mut m = serde_json::Map::new();
                m.insert(CLOUD_REASONER_CONSENT_KEY.to_string(), record);
                m
            }),
            model_meta: None,
            prev_hash: String::new(),
            hash: None,
            signed_by_did: self.signer_did(),
            signature: None,
        })?;
        Ok(())
    }
```
After `evolve_enabled` (`log.rs:5058`) add the newest-wins readers (default **closed** = `None`):
```rust
    /// Newest `reasoner_config` value, or `None` if never set (default Local).
    pub fn reasoner_config_json(&self) -> Result<Option<serde_json::Value>, BossclawError> {
        self.latest_config_value(REASONER_CONFIG_KEY)
    }

    /// Newest signed consent record, or `None` if never set (default: no egress).
    pub fn cloud_reasoner_consent_json(&self) -> Result<Option<serde_json::Value>, BossclawError> {
        self.latest_config_value(CLOUD_REASONER_CONSENT_KEY)
    }

    /// Shared scan: newest `config` event carrying `key`, returning its value.
    fn latest_config_value(&self, key: &str) -> Result<Option<serde_json::Value>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT payload FROM events WHERE event_type = ?1 ORDER BY seq DESC",
        )?;
        let rows = stmt.query_map([CONFIG_EVENT_TYPE], |r| r.get::<_, String>(0))?;
        for row in rows {
            let ev: Event = serde_json::from_str(&row?)?;
            if let Some(v) = ev.content.get(key) {
                return Ok(Some(v.clone()));
            }
        }
        Ok(None)
    }
```
> Match `evolve_enabled`'s exact locking idiom (`self.inner.lock().expect(POISON)`, `store.conn()`); if the SQL column/table names differ in the current file, copy them verbatim from `evolve_enabled`.

- [ ] **Step 5: Run the test (passes)**

Run: `cargo test -p bossclaw-core reasoner_config_and_consent_roundtrip_signed`
Expected: PASS.

- [ ] **Step 6: Lint + commit**

```bash
cargo clippy -p bossclaw-core --all-targets -- -D warnings
git add crates/bossclaw-core/src/log.rs
git commit -m "feat(engine): signed reasoner-config + cloud-consent config events, default-closed (spec R1)"
```

---

### Task 8: `reasoner_ready` + consent match matrix (R1)

**Files:**
- Modify: `apps/desktop/src-tauri/src/engine/reason.rs` (pure helpers + DTOs)
- Test: same file

**Design:** the DTOs (`ReasonerMode`, `CloudProvider` re-export, `ReasonerConfig`) live here; `reasoner_ready` is a pure function over `(config, consent, vault_key_fingerprint)`. Cloud is ready ONLY when a consent record exists and matches `(provider, base_url_host, key_fingerprint)`; any mismatch → not ready (re-consent). Local is ready when the caller's probe says so.

- [ ] **Step 1: Write the failing test (the R1 match/mismatch matrix)**

Add to `engine/reason.rs`:
```rust
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
```

- [ ] **Step 2: Run it (fails)**

Run: `cargo test -p air_agent_desktop ready_tests`
Expected: FAIL — undefined types/fn.

- [ ] **Step 3: Implement the DTOs + `reasoner_ready` + host derivation**

Add to `engine/reason.rs`:
```rust
pub use crate::engine::cloud_reasoner::CloudProvider;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasonerMode {
    Local,
    Cloud,
}

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

/// The wire string for a provider, matching the consent record.
fn provider_str(p: CloudProvider) -> &'static str {
    match p {
        CloudProvider::Anthropic => "anthropic",
        CloudProvider::OpenAiCompat => "openai-compat",
    }
}

/// The host the config WOULD connect to (Anthropic is pinned; OpenAI-compat uses
/// the base_url host). Returns None if an OpenAI-compat base_url is missing/invalid.
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
/// any mismatch -> not ready (spec R1).
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
```

- [ ] **Step 4: Run the test (passes)**

Run: `cargo test -p air_agent_desktop ready_tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src-tauri/src/engine/reason.rs
git commit -m "feat(reasoner): fail-closed reasoner_ready + signed-consent match matrix (spec R1)"
```

---

### Task 9: `ConfigReasonerProvider` — fingerprint cache + Local/Cloud build (R8)

**Files:**
- Modify: `apps/desktop/src-tauri/src/engine/reason.rs`
- Test: same file

**Design:** replaces the unconditional memo cell with a `Mutex<Option<(String, Arc<dyn Reasoner>)>>` keyed on a config fingerprint. Reads `ReasonerConfig` (from a closure/handle the engine supplies). **R8:** absent config → `ReasonerConfig::default()` (Local) → builds `OllamaReasoner` exactly as today.

- [ ] **Step 1: Write the failing test**

Add to `engine/reason.rs`:
```rust
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
```

- [ ] **Step 2: Run it (fails)**

Run: `cargo test -p air_agent_desktop provider_tests`
Expected: FAIL.

- [ ] **Step 3: Implement `config_fingerprint` + `ConfigReasonerProvider`**

Add to `engine/reason.rs`:
```rust
use std::sync::Mutex;
use crate::engine::cloud_reasoner::CloudReasoner;

/// A stable string identity for a config; the provider rebuilds when it changes.
pub fn config_fingerprint(c: &ReasonerConfig) -> String {
    let mode = match c.mode { ReasonerMode::Local => "local", ReasonerMode::Cloud => "cloud" };
    let provider = provider_str(c.provider);
    format!("{mode}|{provider}|{}|{}", c.model, c.base_url.as_deref().unwrap_or(""))
}

type ConfigReader = Box<dyn Fn() -> ReasonerConfig + Send + Sync>;

/// Config-driven reasoner provider: builds Ollama (Local) or CloudReasoner
/// (Cloud) and memoizes keyed on the config fingerprint, rebuilding on change.
pub struct ConfigReasonerProvider {
    read_config: ConfigReader,
    cell: Mutex<Option<(String, Arc<dyn Reasoner>)>>,
}

impl ConfigReasonerProvider {
    pub fn new(read_config: impl Fn() -> ReasonerConfig + Send + Sync + 'static) -> Self {
        Self { read_config: Box::new(read_config), cell: Mutex::new(None) }
    }

    fn build(config: &ReasonerConfig) -> Arc<dyn Reasoner> {
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
```
> Keep `OllamaReasonerProvider` in the file (unchanged) — tests elsewhere may construct it. `ConfigReasonerProvider` is additive.

- [ ] **Step 4: Run the test (passes)**

Run: `cargo test -p air_agent_desktop provider_tests`
Expected: PASS.

- [ ] **Step 5: Lint + commit**

```bash
cargo clippy -p air_agent_desktop --all-targets -- -D warnings
git add apps/desktop/src-tauri/src/engine/reason.rs
git commit -m "feat(reasoner): ConfigReasonerProvider with fingerprint cache; default-local build (spec R8)"
```

---

### Task 10: Scheduler — cloud-vs-local readiness branch

**Files:**
- Modify: `apps/desktop/src-tauri/src/engine/scheduler.rs` (the live loop at `:79-87`; `decide_tick` at `:49-60` is UNCHANGED)
- Test: `engine/scheduler.rs` (the existing `decide_tick` test at `:109-117` stays green; add a readiness-selection unit test)

**Design:** `decide_tick`'s `ollama_ready: bool` slot is mode-agnostic. In cloud mode, skip `ollama_probe::probe(...)` and pass `engine.reasoner_ready(...)` (Task 12 adds the async wrapper) into that slot; in local mode keep `oll.reachable && oll.model_present`. Factor the selection into a tiny pure helper so it's unit-testable without a runtime.

- [ ] **Step 1: Write the failing test**

Add to `scheduler.rs` tests:
```rust
#[test]
fn readiness_source_follows_mode() {
    // Local: use the ollama probe result; ignore cloud readiness.
    assert!(select_ready(false, true, false));   // local probe true
    assert!(!select_ready(false, false, true));  // local probe false (cloud true ignored)
    // Cloud: use cloud readiness; ignore the probe.
    assert!(select_ready(true, false, true));     // cloud ready true
    assert!(!select_ready(true, true, false));    // cloud not ready (probe true ignored)
}
```

- [ ] **Step 2: Run it (fails)**

Run: `cargo test -p air_agent_desktop readiness_source_follows_mode`
Expected: FAIL.

- [ ] **Step 3: Implement the selector + wire it into the loop**

Add the pure helper near `decide_tick`:
```rust
/// Choose the readiness signal fed into `decide_tick`'s `ollama_ready` slot:
/// local mode trusts the Ollama probe; cloud mode trusts `reasoner_ready`
/// (which is itself fail-closed on signed consent). Cloud never silently falls
/// back to local (spec §3.4).
pub fn select_ready(cloud_mode: bool, ollama_probe_ready: bool, cloud_ready: bool) -> bool {
    if cloud_mode { cloud_ready } else { ollama_probe_ready }
}
```
Then in `spawn`'s loop (`scheduler.rs:79-87`), replace the single probe+gate with a mode-aware version:
```rust
            let config = engine.reasoner_config_or_default().await; // Task 12
            let cloud_mode = matches!(config.mode, crate::engine::reason::ReasonerMode::Cloud);
            let ready = if cloud_mode {
                engine.reasoner_ready_or_false().await // Task 12
            } else {
                let oll = ollama_probe::probe(crate::engine::reason::REASONER_MODEL_ID).await;
                oll.reachable && oll.model_present
            };
            let queue_depth = engine.queue_depth_or_zero(onboarded).await;
            if decide_tick(onboarded, evolve_enabled, select_ready(cloud_mode, ready, ready), queue_depth)
                == TickGate::Run
            {
```
> The `select_ready(cloud_mode, ready, ready)` call passes the already-mode-selected `ready` into both slots; the helper exists primarily for the unit test and to document intent. Alternatively, compute `oll`/`cloud_ready` separately and call `select_ready(cloud_mode, oll_ready, cloud_ready)` — pick the form the implementer finds clearest; both keep `decide_tick` unchanged.

- [ ] **Step 4: Run the tests (selector passes; the existing `decide_tick` test still passes)**

Run: `cargo test -p air_agent_desktop -- scheduler`
Expected: `readiness_source_follows_mode` PASS, existing `decide_tick` test PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src-tauri/src/engine/scheduler.rs
git commit -m "feat(reasoner): scheduler gates cloud mode on reasoner_ready, no silent local fallback (spec §3.4)"
```

---

### Task 11: R4 — tainted-snippet count + silent-bad-key backstop

**Files:**
- Modify: `crates/bossclaw-core/src/evolve.rs` (`EvolveReport` at `:32`/`:46`)
- Modify: `crates/bossclaw-core/src/log.rs` (`evolve_once` recall block at `:5771-5784`)
- Modify: `apps/desktop/src-tauri/src/engine/mod.rs` (`record_tick` at `:544`; call site at `:535`)
- Test: `bossclaw-core` evolve test + a desktop `record_tick` unit test

**Two hooks:** (a) count `is_external` snippets in each tick's recall payload and surface it on `EvolveReport` (so 2b can show "N file-derived snippets sent"); (b) when a **cloud** tick returns `Ok` but processed 0 items with a non-empty queue, record a `last_error` backstop so a bad/expired key doesn't silently no-op forever.

- [ ] **Step 1: Write the failing test (report field + backstop)**

In `bossclaw-core` evolve tests:
```rust
#[test]
fn evolve_report_carries_tainted_recall_count_field() {
    // Compile-level guarantee the field exists and defaults to 0 when no
    // external snippets are recalled. (Full taint-count behavior is covered by
    // the existing evolve integration tests once external content is present.)
    let report = EvolveReport::default();
    assert_eq!(report.tainted_recall_snippets, 0);
}
```
In `air_agent_desktop` (`engine/mod.rs` tests):
```rust
#[test]
fn cloud_zero_item_tick_records_backstop_error() {
    use bossclaw_core::EvolveReport;
    let tel = std::sync::Mutex::new(EvolveTelemetry::default());
    // Ok report, 0 processed, cloud tick, queue had work -> last_error set.
    record_tick_into(&tel, 5, &Ok(EvolveReport { memories_processed: 0, ..Default::default() }), true, 3);
    assert!(tel.lock().unwrap().last_error.is_some());
    // Local 0-item tick (or empty queue) -> no synthetic error.
    let tel2 = std::sync::Mutex::new(EvolveTelemetry::default());
    record_tick_into(&tel2, 5, &Ok(EvolveReport { memories_processed: 0, ..Default::default() }), false, 3);
    assert!(tel2.lock().unwrap().last_error.is_none());
}
```
> Refactor `record_tick` to delegate to a pure `record_tick_into(tel, ms, result, cloud_mode, queue_depth)` so it is unit-testable. If `EvolveTelemetry`/field names differ, match the real ones at `engine/mod.rs:238`.

- [ ] **Step 2: Run them (fail)**

Run: `cargo test -p bossclaw-core evolve_report_carries_tainted_recall_count_field` and `cargo test -p air_agent_desktop cloud_zero_item_tick_records_backstop_error`
Expected: FAIL.

- [ ] **Step 3a: Add the report field**

In `crates/bossclaw-core/src/evolve.rs`, add to `EvolveReport` (keep `#[derive(Default)]`; if absent, add it):
```rust
    /// Count of file-derived / `is_external` snippets included in this tick's
    /// recall payload — surfaced so the UI (2b) can disclose "N file-derived
    /// snippets sent to the cloud" (spec R4). Local ticks set it too (harmless).
    pub tainted_recall_snippets: usize,
```

- [ ] **Step 3b: Count taint in `evolve_once`**

In `crates/bossclaw-core/src/log.rs`, in the recall block (`:5771-5784`), after `recalled` is built and before/with `recalled_texts`, count externals over the recalled ids and accumulate into the report. Insert:
```rust
            // R4 hook: how many recalled snippets are file-derived / external?
            let tainted_this_memory = recalled
                .iter()
                .filter(|id| {
                    self.event_by_id(id)
                        .ok()
                        .flatten()
                        .map(|ev| crate::ingest::is_external(&ev))
                        .unwrap_or(false)
                })
                .count();
            report.tainted_recall_snippets += tainted_this_memory;
```
> `report` is the `EvolveReport` built at `log.rs:5729`; `event_by_id` is at `log.rs:835`. If the report binding is immutable, make it `let mut report` at its construction site.

- [ ] **Step 3c: Backstop in `record_tick`**

In `apps/desktop/src-tauri/src/engine/mod.rs`, refactor `record_tick` (`:544`) to thread mode+queue and delegate:
```rust
    fn record_tick(
        &self,
        ms: u128,
        result: &Result<bossclaw_core::EvolveReport, EngineOpError>,
        cloud_mode: bool,
        queue_depth: usize,
    ) {
        record_tick_into(&self.evolve_tel, ms, result, cloud_mode, queue_depth);
    }

/// Pure tick recorder. Records `last_error` on `Err`, AND synthesizes one when a
/// CLOUD tick returns Ok-but-processed-zero while the queue had work — a bad or
/// expired key otherwise no-ops silently every tick (spec R4).
fn record_tick_into(
    tel: &std::sync::Mutex<EvolveTelemetry>,
    ms: u128,
    result: &Result<bossclaw_core::EvolveReport, EngineOpError>,
    cloud_mode: bool,
    queue_depth: usize,
) {
    let mut tel = tel.lock().unwrap_or_else(|p| p.into_inner());
    tel.last_tick_ms = Some(ms);
    match result {
        Err(e) => {
            tel.error_count += 1;
            let mut s = e.to_string();
            truncate_on_char_boundary(&mut s, 512);
            tel.last_error = Some(s);
        }
        Ok(report) if cloud_mode && report.memories_processed == 0 && queue_depth > 0 => {
            tel.error_count += 1;
            tel.last_error = Some(
                "cloud reasoner processed 0 of a non-empty queue (check the provider key/endpoint)".to_string(),
            );
        }
        Ok(_) => {}
    }
}
```
Update the single call site (`engine/mod.rs:535`) to pass the mode + queue depth it already computes (or fetch them there). If the wrapper doesn't currently know the mode, read it via `self.reasoner_config_or_default()` (Task 12) before the call.

- [ ] **Step 4: Run the tests (pass)**

Run: `cargo test -p bossclaw-core evolve_report_carries_tainted_recall_count_field` and `cargo test -p air_agent_desktop cloud_zero_item_tick_records_backstop_error`
Expected: PASS.

- [ ] **Step 5: Lint + commit**

```bash
cargo clippy -p bossclaw-core --all-targets -- -D warnings
cargo clippy -p air_agent_desktop --all-targets -- -D warnings
git add crates/bossclaw-core/src/evolve.rs crates/bossclaw-core/src/log.rs apps/desktop/src-tauri/src/engine/mod.rs
git commit -m "feat(reasoner): tainted-recall count + cloud 0-item backstop error (spec R4)"
```

---

### Task 12: Engine wrappers + Tauri commands (R5) + wire the provider

**Files:**
- Modify: `apps/desktop/src-tauri/src/engine/mod.rs` (async wrappers + `enable_cloud_reasoner` R5 + `key_fingerprint`)
- Modify: `apps/desktop/src-tauri/src/commands/engine.rs` (3 commands + IPC tests)
- Modify: `apps/desktop/src-tauri/src/main.rs` (register commands; swap provider)
- Test: `commands/engine.rs` IPC tests (mirror `:567-640`)

**Commands (spec §3.5 + R5):**
- `engine_get_reasoner_config -> ReasonerConfigDto` (mirror `EvolveStatusDto`).
- `engine_set_reasoner_config(config)` — validates: cloud requires base_url HTTPS + host not blocked; writes the **non-security** signed config event. Does NOT grant consent.
- `engine_enable_cloud_reasoner(config)` — the R5 flow: validate → run ONE trivial-prompt `complete_json` (no memory) → on success write the **signed consent** (binding provider/host/key-fp) → enable; on failure surface the classified error, do not enable.

- [ ] **Step 1: Write the failing IPC test (set-config binds camelCase; enable rejects bad key)**

In `commands/engine.rs` tests, clone the `:567-640` pattern. Two tests:
```rust
// (a) engine_set_reasoner_config binds the camelCase IPC arg and persists.
//     Assert via engine_get_reasoner_config returning the mode we set.
// (b) engine_enable_cloud_reasoner with no vault key -> Err (no consent written),
//     and engine_get_reasoner_config still reports cloud NOT ready.
```
Write both using `tauri::test::mock_builder().invoke_handler(tauri::generate_handler![engine_set_reasoner_config, engine_get_reasoner_config, engine_enable_cloud_reasoner])`, `__allow_command(...)` for each, and `get_ipc_response` with a JSON body like `{ "config": { "mode": "cloud", "provider": "anthropic", "model": "claude-sonnet-4-6", "baseUrl": null } }`. Use `MockReasonerProvider` in the `AppState` as the existing tests do (`:583-595`). Assert: (a) get returns `mode == "cloud"`; (b) enable returns an error string and get reports `ready == false`.
> Keep the assertions deterministic — the enable test must NOT hit the network (no key in the mock vault → `read_key` fails before any send).

- [ ] **Step 2: Run them (fail)**

Run: `cargo test -p air_agent_desktop -- engine_reasoner_config`
Expected: FAIL — commands undefined.

- [ ] **Step 3a: Engine async wrappers + R5 enable**

In `apps/desktop/src-tauri/src/engine/mod.rs`, add (mirroring `set_evolve_enabled` at `:593` for gating + error mapping):
```rust
    /// Current reasoner config, or the Local default when unset (R8).
    pub async fn reasoner_config_or_default(&self) -> crate::engine::reason::ReasonerConfig {
        self.with_log_read(|log| {
            Ok(parse_reasoner_config(log.reasoner_config_json()?))
        })
        .await
        .unwrap_or_default()
    }

    /// Fail-closed cloud readiness for the scheduler (false on any read error).
    pub async fn reasoner_ready_or_false(&self) -> bool {
        let config = self.reasoner_config_or_default().await;
        self.with_log_read(|log| {
            let consent = log.cloud_reasoner_consent_json()?;
            let fp = current_key_fingerprint(&config);
            Ok(crate::engine::reason::reasoner_ready(&config, consent.as_ref(), fp.as_deref(), false))
        })
        .await
        .unwrap_or(false)
    }

    pub async fn set_reasoner_config(&self, onboarded: bool, config: serde_json::Value) -> Result<(), EngineOpError> {
        // Persist non-security config only; consent is NOT granted here.
        self.with_log_write(onboarded, |log| {
            log.set_reasoner_config(config).map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
    }

    /// R5: test-key-on-enable. Validate -> one trivial complete_json (no memory)
    /// -> on success write signed consent + persist config; on failure surface
    /// the classified error and DO NOT enable.
    pub async fn enable_cloud_reasoner(&self, onboarded: bool, config: serde_json::Value) -> Result<(), EngineOpError> {
        let parsed = parse_reasoner_config(Some(config.clone()));
        // Build a one-shot reasoner from the proposed config and probe it.
        let probe_reasoner = crate::engine::reason::build_reasoner_for(&parsed);
        let schema = bossclaw_core::adjudication_schema();
        let probe = tokio::task::spawn_blocking(move || {
            probe_reasoner.complete_json("Reply with the JSON {\"match\":\"ok\"}.", "candidates: [ok]. text: ok", &schema)
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?;
        probe.map_err(|e| EngineOpError::Core(e.to_string()))?;
        // Success -> write config + signed consent binding the current key fp.
        let host = crate::engine::reason::config_host(&parsed).unwrap_or_default();
        let fp = current_key_fingerprint(&parsed).unwrap_or_default();
        let consent = serde_json::json!({
            "provider": provider_wire(&parsed),
            "base_url_host": host,
            "key_fingerprint": fp,
            "consented_at": now_rfc3339(),
        });
        self.with_log_write(onboarded, |log| {
            log.set_reasoner_config(config.clone()).map_err(|e| EngineOpError::Core(e.to_string()))?;
            log.set_cloud_reasoner_consent(consent.clone()).map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
    }
```
Plus small helpers in `engine/mod.rs` (or `reason.rs`):
```rust
/// sha256(key)[..8] hex; the consent binds this so a rotation re-consents.
pub fn key_fingerprint(key: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(key.as_bytes());
    hex::encode(&digest[..4]) // 8 hex chars
}
```
> `sha2`/`hex` are already engine deps (used by the signing path). `with_log_read`/`with_log_write`, `now_rfc3339`, `current_key_fingerprint` (reads the vault key for the configured provider and hashes it, `None` if absent), `parse_reasoner_config` (Value → `ReasonerConfig`, default Local), `provider_wire`, and `build_reasoner_for` (config → `Arc<dyn Reasoner>`, reuse `ConfigReasonerProvider::build`) are thin glue — implement them next to the wrappers, matching existing engine idioms for log access. If the engine has no `with_log_read/write` helper, inline the `self.log`-lock pattern the sibling switches use.

- [ ] **Step 3b: The 3 Tauri commands**

In `apps/desktop/src-tauri/src/commands/engine.rs` (mirror `engine_set_evolve_enabled` / `engine_mandates_enabled`):
```rust
#[derive(serde::Serialize)]
pub struct ReasonerConfigDto {
    pub mode: String,        // "local" | "cloud"
    pub provider: String,    // "anthropic" | "openai-compat"
    pub model: String,
    pub base_url: Option<String>,
    pub ready: bool,         // fail-closed readiness (signed-consent match)
}

#[tauri::command]
pub async fn engine_get_reasoner_config(state: State<'_, AppState>) -> Result<ReasonerConfigDto, String> {
    let config = state.engine.reasoner_config_or_default().await;
    let ready = state.engine.reasoner_ready_or_false().await;
    Ok(ReasonerConfigDto {
        mode: match config.mode { crate::engine::reason::ReasonerMode::Local => "local", _ => "cloud" }.into(),
        provider: crate::engine::reason::provider_wire(&config),
        model: config.model,
        base_url: config.base_url,
        ready,
    })
}

#[tauri::command]
pub async fn engine_set_reasoner_config(config: serde_json::Value, state: State<'_, AppState>) -> Result<(), String> {
    let onboarded = state.identity_store.is_onboarded();
    validate_reasoner_config(&config)?; // HTTPS + host-not-blocked for cloud
    state.engine.set_reasoner_config(onboarded, config).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn engine_enable_cloud_reasoner(config: serde_json::Value, state: State<'_, AppState>) -> Result<(), String> {
    let onboarded = state.identity_store.is_onboarded();
    validate_reasoner_config(&config)?;
    state.engine.enable_cloud_reasoner(onboarded, config).await.map_err(|e| e.to_string())
}
```
`validate_reasoner_config` (same file): for cloud + openai-compat, require `base_url` present, `normalize_https_base` success (reuse `crate::llm_stream::normalize_https_base` — make it `pub(crate)` if needed), reject literal-IP base_urls, and `is_blocked_ip` on the resolved host at set time (defense-in-depth on top of the connect-time resolver).

- [ ] **Step 3c: Register commands + swap the provider in `main.rs`**

In `apps/desktop/src-tauri/src/main.rs` handler list (near `:190-195`), add:
```rust
            #[cfg(unix)]
            commands::engine::engine_get_reasoner_config,
            #[cfg(unix)]
            commands::engine::engine_set_reasoner_config,
            #[cfg(unix)]
            commands::engine::engine_enable_cloud_reasoner,
```
At the provider injection (`main.rs:73-80`), replace:
```rust
                let reasoner_provider =
                    std::sync::Arc::new(crate::engine::reason::OllamaReasonerProvider::new());
```
with a `ConfigReasonerProvider` whose closure reads the engine log. Because the provider needs to read config but is constructed *before* the `EngineHandle`, supply it a cloned handle to the log/vault, or construct the provider with a closure that reads a shared `EngineHandle` set up via `Arc`/`OnceCell`. Simplest pattern that matches the codebase: build the `EngineHandle` first with a placeholder, then... (avoid the cycle) — instead, give `ConfigReasonerProvider::new` a closure capturing the same `vault` + `data_dir` used to open the log, opening a short-lived read of the config event. Implement `read_reasoner_config_from_disk(vault, data_dir) -> ReasonerConfig` reusing `EventLog::open` read-only. Wire:
```rust
                let cfg_vault = vault.clone();
                let cfg_dir = data_dir.clone();
                let reasoner_provider = std::sync::Arc::new(
                    crate::engine::reason::ConfigReasonerProvider::new(move || {
                        crate::engine::reason::read_reasoner_config_from_disk(&cfg_vault, &cfg_dir)
                    }),
                );
```
> `read_reasoner_config_from_disk` opens the same event log the `EngineHandle` uses and calls `reasoner_config_json()` → `parse_reasoner_config`. If opening a second read-only handle to the same DB is awkward, instead have `EngineHandle` own the `ConfigReasonerProvider` and expose a setter the scheduler/commands already reach — choose whichever matches how `OllamaReasonerProvider` is currently threaded; the contract is "provider reads current signed config each `reasoner()` call."

- [ ] **Step 4: Run the IPC tests (pass)**

Run: `cargo test -p air_agent_desktop -- engine_reasoner_config`
Expected: PASS (set/get round-trips; enable-without-key errors and stays not-ready).

- [ ] **Step 5: Build the whole app + lint + commit**

```bash
cargo build -p air_agent_desktop
cargo clippy -p air_agent_desktop --all-targets -- -D warnings
git add apps/desktop/src-tauri/src/engine/mod.rs apps/desktop/src-tauri/src/commands/engine.rs apps/desktop/src-tauri/src/main.rs apps/desktop/src-tauri/src/engine/reason.rs apps/desktop/src-tauri/src/llm_stream.rs
git commit -m "feat(reasoner): get/set/enable reasoner-config commands (R5 test-key) + ConfigReasonerProvider wired in main (spec §3.5/R5)"
```

---

### Task 13: R8/R9 — "egresses nothing" end-to-end + remaining matrix tests

**Files:**
- Test: `apps/desktop/src-tauri/src/engine/reason.rs` (or a desktop integration test) + `crates/bossclaw-core` where cited.

**R8 load-bearing invariant + R9 minors.** Prove a fresh install egresses nothing and backfill the test matrix §6/R9.

- [ ] **Step 1: Write the tests**

R8 (desktop): with no config event written, the provider builds Ollama and cloud readiness is false:
```rust
#[test]
fn fresh_install_is_local_and_not_cloud_ready() {
    let cfg = ReasonerConfig::default();
    assert!(matches!(cfg.mode, ReasonerMode::Local));
    // No consent + cloud config -> never ready (can't egress without R5 enable).
    let cloud = ReasonerConfig { mode: ReasonerMode::Cloud, ..ReasonerConfig::default() };
    assert!(!reasoner_ready(&cloud, None, None, false));
}
```
R9 spot checks already covered by earlier tasks (R2 blocked-IP → Task 1; R3 scrub → Task 2; R6 no-panic → Task 5; R5 success/fail → Task 12; R1 matrix → Task 8). Add the two not yet covered:
- **Garbage adjudication-id → resolver mints, no crash** (bossclaw-core): feed `adjudication_schema()` extraction where the model returns `{"match":"not-a-real-id"}` and assert the existing resolver path mints/handles it without panic (cite the resolver at `log.rs`; mirror an existing adjudication test if present).
- **Determinism absorbed** (bossclaw-core): assert `to_confidence_milli` clamps an out-of-range float (cite `extract.rs:50`) — a 1-line guard test if not already present.

- [ ] **Step 2: Run (fail where new)**

Run: `cargo test -p air_agent_desktop fresh_install_is_local_and_not_cloud_ready` and the bossclaw-core additions.
Expected: FAIL for any genuinely new assertion.

- [ ] **Step 3: Implement / fill gaps**

These are tests over already-built behavior — no production code should be needed beyond what Tasks 1–12 added. If a test reveals a gap (e.g. confidence not clamped), fix minimally at the cited site.

- [ ] **Step 4: Run the FULL suites (all green)**

Run:
```bash
cargo test -p bossclaw-core
cargo test -p air_agent_desktop
cargo clippy --all-targets -- -D warnings
```
Expected: all PASS; clippy clean.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "test(reasoner): R8 egresses-nothing default + R9 adjudication/determinism matrix (spec R8/R9)"
```

---

## Self-Review (run against spec §8 before declaring the plan done)

**Spec coverage (R1–R9):**
- R1 (signed consent, default-closed, match matrix) → Tasks 7, 8, 12. Config-in-signed-log decision (plan #1) documented. ✓
- R2 (connect-time DNS pin, redirect none, Anthropic literal host, reject literal-IP base_url) → Tasks 0, 1, 5, 6, 12. ✓
- R3 (classify_cloud_error, raw body dropped) → Task 2 (+ used in Task 6). ✓
- R4 (silent-bad-key backstop + tainted count + honest copy [copy lands in 2b]) → Task 11. Consent copy text is 2b. ✓
- R5 (test-key-on-enable) → Task 12 `enable_cloud_reasoner`. ✓
- R6 (OnceLock blocking client, no captured runtime, timeout) → Task 5. ✓
- R7 (max_tokens, model_id provider-qualified, header-only key, 429/5xx→retry-next-tick, shared key, strict OFF; **plus** temperature omitted) → Tasks 3, 4, 6. ✓
- R8 (default-local, merging egresses nothing) → Tasks 9, 13. ✓
- R9 (test matrix) → spread across Tasks 1, 2, 5, 8, 11, 12, 13. ✓

**Placeholder scan:** the `> Note`/`> If` callouts mark spots where exact local idioms (lock pattern, helper names, module-declaration site) must match the current file — they give the code AND the fallback, not "TODO". No bare TODOs. The provider-wiring step (Task 12 §3c) offers two concrete wiring forms; the implementer picks one — acceptable because both are fully specified and the contract is stated.

**Type consistency:** `CloudProvider` defined once (Task 6 in `cloud_reasoner.rs`), re-exported by `reason.rs` (Task 8). `ReasonerConfig`/`ReasonerMode` defined in Task 8, used in 9/10/12. `EvolveReport.tainted_recall_snippets` defined in Task 11, asserted in 11/—. `classify_cloud_error`/`build_*`/`extract_*` defined in 2/3/4, consumed in 6. `key_fingerprint`/`config_host`/`provider_wire` defined in 8/12, consumed in 8/12. Consent JSON keys (`provider`,`base_url_host`,`key_fingerprint`,`consented_at`) identical in 7 (test), 8 (match), 12 (write). ✓

**Known pre-existing behaviors deliberately NOT changed:** retraction events keep `MANUAL_LINK_PRODUCER` (not the cloud model_id); the chat `SettingsRecord` stays read-only in Rust. Both noted so review doesn't flag them as gaps.

---

## Post-build gates (before PR)

1. `cargo test -p bossclaw-core && cargo test -p air_agent_desktop` — green.
2. `cargo clippy --all-targets -- -D warnings` — clean (note: `blocking` feature may surface new lints).
3. `cargo build` (full app) — green.
4. `cargo audit` — the repo-wide `--deny warnings` gate; no new advisories expected (only a reqwest feature flag added). If one appears, bump-or-allowlist per the locked policy.
5. **Dedicated security review of the egress** (`oh-my-claudecode:security-reviewer`, Opus) BEFORE the PR — this is an egress milestone; the locked process requires it.
6. Manual smoke (optional, post-merge): `npm run dev` — confirm cloud is OFF by default and there is no enable UI (2b ships that).
