# Milestone D — Phase 2: Cloud Reasoner (Local or Cloud brain model) — Design Spec

**Status:** DRAFT — for critic + security review, then Peter sign-off.
**Track:** AIR Note desktop app. **Builds on:** Phase 1 `vault_set` (MERGED, main `ca47925`) — the webview can now save a provider API key.
**Source:** review-fixes design §3-D (`docs/superpowers/specs/2026-06-25-air-agent-review-fixes-design.md`) + the Milestone D code-surface map (2026-06-30). Decisions locked by Peter (2026-06-30).

---

## 1. Summary

Let the brain's **evolve** reasoner run against a **cloud LLM** instead of only local Ollama. This is the brain's **first deliberate network egress**: the evolve `prompt` carries recall/neighborhood context derived from the user's ingested files + memories, so cloud mode sends genuinely sensitive on-box content to a third party. Therefore: **desktop-side** (outside the engine's `ureq`-only network jail), **off by default**, **explicit one-time consent + persistent banner**, **fail-closed**, **provider-host allow-listed**.

**Locked decisions (Peter, 2026-06-30):**
- **Providers v1:** Anthropic + OpenAI-compat (Gemini deferred behind the same seam).
- **Consent:** explicit one-time consent before cloud can be enabled + a persistent "cloud — your memory leaves this device" indicator while active + replace the "Everything stays on your machine" copy.
- **Key entry:** via Phase 1's `vaultSet`/`vaultHas` (already shipped).

**Scope split (2 sub-PRs, this spec covers both):**
- **2a — backend** (this PR first): `CloudReasoner` + `ConfigReasonerProvider` + reasoner config + commands + scheduler/UI-status branch. Cloud config defaults to **local**, and there is **no UI to enable cloud yet**, so merging 2a sends nothing off-box.
- **2b — UI**: the Brain-tab Local/Cloud selector, key entry, consent gate, persistent banner, copy changes — the surface that can actually turn cloud on.

This separation keeps the egress backend reviewable on its own and means cloud is unreachable until 2b's consent gate ships.

---

## 2. The seam (recap, from the map)

- Trait `bossclaw_core::Reasoner` (`crates/bossclaw-core/src/reason.rs:29-47`): `fn complete_json(&self, system:&str, prompt:&str, schema:&serde_json::Value) -> Result<serde_json::Value, BossclawError>` + `fn model_id(&self)->&str`. `system` = trusted instruction channel, `prompt` = untrusted/fenced data channel, `schema` = JSON-schema the impl must honor; output is **data, never authority**.
- Two schemas the engine passes verbatim: `extraction_schema()` (`reason.rs:126-173`: `{entities[], relations[], retractions[]}`, floats for `confidence`) and `adjudication_schema()` (`reason.rs:179-190`: `{match: string}`).
- Local impl `OllamaReasoner` (`crates/bossclaw-core/src/ollama.rs`) POSTs `127.0.0.1:11434` with `format: schema` (Ollama's native structured output), loopback-fail-closed.
- Provider seam `ReasonerProvider` (`apps/desktop/src-tauri/src/engine/reason.rs:16-49`): `OllamaReasonerProvider` caches one `Arc<dyn Reasoner>` for process life; injected into `EngineHandle` at `main.rs:73-80`. A reserved `EngineOpError::Reasoner(String)` (`engine/mod.rs:55-62`) already routes a provider build-failure.
- **CI guard** (`build.yml:122-133`): `bossclaw-core` may only link `ureq` (never `reqwest`). So `CloudReasoner` MUST be desktop-side (the `air_agent_desktop` crate already uses `reqwest`).

---

## 3. Phase 2a — backend

### 3.1 `CloudReasoner` (desktop-side, `reqwest`)
New module `apps/desktop/src-tauri/src/engine/cloud_reasoner.rs`. Implements `bossclaw_core::Reasoner`. Holds: provider kind, model id, optional base_url, and reads the API key from the vault (`secret_get_cached`) **at call time** (never stored on the struct). `complete_json` is synchronous (the trait is sync; the engine calls it inside `spawn_blocking`) — use a blocking `reqwest::blocking` client OR block-on a small async call; **decision: `reqwest::blocking`** (the trait is sync and we're already on a blocking thread; avoids a nested runtime).

**Per-provider structured-output (the net-new core):**
- **Anthropic** (`https://api.anthropic.com/v1/messages`, `x-api-key` + `anthropic-version: 2023-06-01`): force structured JSON via **tool-use** — define ONE tool whose `input_schema` is the engine's JSON-schema, set `tool_choice: {type:"tool", name:<tool>}`, send `system` as the top-level `system` field and `prompt` as the user message, then read the forced `tool_use` block's `.input` as the result `Value`. (Anthropic's `input_schema` is JSON-Schema-shaped, so `extraction_schema()`/`adjudication_schema()` map almost directly.)
- **OpenAI-compat** (`{base}/v1/chat/completions`, `Authorization: Bearer`): `response_format: {type:"json_schema", json_schema:{name, schema:<engine schema>, strict:false}}`. `strict:true` is NOT used in v1 — it requires `additionalProperties:false` + every field `required`, which the engine schemas don't guarantee; non-strict json_schema + a **defensive parse** is more robust. Fallback chain (reuse the planner precedent at `llm_stream.rs:630-658`): on HTTP 400/404/422, retry with `response_format:{type:"json_object"}` and the schema described in the `system` text; on a non-JSON body, strip a ```json fence and parse; on parse failure return `BossclawError::Reasoner` (→ retryable no-op tick, never a crash).

**Exact request/response shapes are pinned at build against current Anthropic + OpenAI docs** (via the `claude-api` skill / context7) — this spec fixes the MECHANISM, not the bytes. Response extraction reuses the existing parsers' spirit (`extract_claude_text` etc. at `llm_stream.rs:490-540`) but targets the tool-use `input` / `message.content` JSON.

**Determinism:** `temperature: 0`. The engine already converts the float `confidence` → integer `confidence_milli` before signing (`reason.rs:13-17`), so cloud float-formatting variance is absorbed downstream.

### 3.2 Reasoner config + persistence
```rust
enum ReasonerMode { Local, Cloud }
enum CloudProvider { Anthropic, OpenAiCompat }
struct ReasonerConfig {
    mode: ReasonerMode,              // default Local
    provider: CloudProvider,         // only meaningful when Cloud
    model: String,                   // e.g. "claude-sonnet-4-6" / "gpt-5-mini"
    base_url: Option<String>,        // OpenAI-compat only; HTTPS-enforced
    cloud_consent_at: Option<String>,// RFC3339; set when the user consents (2b)
}
```
Persisted in **`settings.json`** (alongside the existing chat `SettingsRecord`, `llm_stream.rs:43-70`) — NOT the signed event log (it's config, not an autonomy switch; the *enable* safety comes from consent + fail-closed, below). Read/written by new helpers.

### 3.3 `ConfigReasonerProvider` (config-driven + cache invalidation)
Replaces the hardcoded `OllamaReasonerProvider` wiring at `main.rs:73-80`. Reads `ReasonerConfig` and builds either `OllamaReasoner(REASONER_MODEL_ID→config.model)` or `CloudReasoner(...)`. **Cache invalidation:** the current provider memoizes one reasoner forever (`reason.rs:40-48`); the config provider must rebuild when the config (mode/provider/model/base_url) changes — key the cache on a config fingerprint, rebuild on mismatch.

### 3.4 Fail-closed readiness (the safety core)
`reasoner_ready(config) -> bool`:
- **Local:** Ollama reachable + model present (today's probe, `scheduler.rs:81`).
- **Cloud:** `cloud_consent_at.is_some()` AND `vault_has(provider key)` AND `base_url` (if set) is HTTPS AND host is allow-listed. **No network probe** (don't ping the provider each tick).
- **Any config read error → `false`** (fail-closed: a transient error never enables egress).

The scheduler (`scheduler.rs:81`) branches: cloud mode skips the Ollama probe and gates on `reasoner_ready`. If cloud is selected but NOT ready, evolve is **gated off** and the reason surfaced — it does **not** silently fall back to local (which would be a surprising model swap) and **never** sends off-box without consent+key.

### 3.5 Commands
- `engine_get_reasoner_config() -> ReasonerConfigDto`
- `engine_set_reasoner_config(config)` — validates: cloud requires consent + key present + (base_url HTTPS + allow-listed host); rejects otherwise. Mirrors `engine_set_evolve_enabled` (`commands/engine.rs:352`). Registered in `main.rs`; TS binding in `api/engine.ts`.

### 3.6 Security (egress) — Phase 2a
1. **Provider-host allow-list.** `CloudReasoner` may only connect to `api.anthropic.com` (Anthropic) or the configured OpenAI-compat `base_url` — which MUST be HTTPS and SHOULD be validated against an SSRF-style guard (no loopback/private/link-local hosts; reuse `web_access.rs`'s SSRF host check + the existing HTTPS-enforce at `llm_stream.rs:136-155`). Prevents "point the reasoner at an internal host."
2. **Fail-closed** (3.4): no consent/key → no egress, evolve gated off.
3. **Error scrubbing.** A cloud error body can echo request/prompt content; the surfaced `last_error` is already truncated to 512 bytes (`mod.rs:539-553`) — additionally **scrub** (don't pass the raw provider body through; map to a short class string + status code).
4. **Key handling.** Read from the vault at call time; only ever placed in the auth header/body; never logged (debug logs print key NAMES only, never values — Phase 1 verified).
5. **Untrusted input leaving the box.** The `prompt` (fenced untrusted file content) leaves to the provider — this is inherent to cloud reasoning; consent (2b) makes it informed. Output remains data-not-authority (engine fence unchanged).
6. **Control-char-clean keys** (Phase 1 already rejects them at write) — so no header injection from the key.

---

## 4. Phase 2b — UI (Brain tab)

In `MemoryPanel.tsx` Evolve section (`:156-197`), next to the `ollamaReady` gate:
- **Local | Cloud** segmented selector (default Local).
- **Cloud sub-panel:** provider dropdown (Anthropic / OpenAI-compat), model field (prefilled defaults), OpenAI-compat base_url field (HTTPS), API-key entry → `vaultSet` with a `vaultHas` "key saved ✓" status (never shows the key), and a **Save** that calls `engine_set_reasoner_config`.
- **Explicit consent gate:** before cloud can be enabled, a one-time modal — *"Cloud mode sends your brain's working context (built from your memories and ingested files) to <provider>. Your memory leaves this device. Continue?"* — sets `cloud_consent_at`. Cloud can't be saved without it.
- **Persistent banner** while cloud is active: a non-dismissible indicator in the Brain tab ("Brain model: Cloud · <provider> — context leaves this device").
- **Copy changes:** replace "Everything stays on your machine" (`:109`) and "Off by default; runs only on your machine" (`:159`) with mode-aware copy (on-machine for Local; egress disclosure for Cloud).
- The Evolve buttons' `ollamaReady` gate (`:103`) becomes `reasonerReady` (local probe OR cloud-ready).

UI logic that's pure (the mode/ready/label derivation) is extracted + vitest-tested; the rendered panel is manual GUI QA (web-preview mockIPC, per the lessons).

---

## 5. Security analysis — the egress threat model (for the security review)

| Threat | Mitigation |
|---|---|
| Sensitive memory/file content silently leaving the box | Off-by-default + explicit one-time consent + persistent banner + fail-closed (no consent → no egress) |
| Reasoner pointed at an attacker/internal host (SSRF) | Provider-host allow-list: Anthropic fixed host; OpenAI-compat base_url HTTPS-enforced + SSRF host guard (no loopback/private/link-local) |
| API key exfil / leak | Read from keychain at call time, header-only, never logged; webview can't read it back (Phase 1 no-`vault_get`); control-chars rejected at write (Phase 1) |
| Cloud error body leaking prompt content to UI/logs | Scrub → short class + status; existing 512-byte truncate as backstop |
| Untrusted file content → prompt-injecting the cloud model | Output is data-not-authority (engine fence unchanged); injection can't escalate to authority; input-leaving is consented |
| Transient error silently enabling/regressing egress | Fail-closed: any config/key read error → reasoner_ready=false → evolve gated off |
| Concurrent vault writes (Phase 1 deferred Low-2) | If 2b adds concurrent key writes, serialize blob mutation with a write mutex (carry the Phase-1-deferred fix here) |

**Open questions for the reviewers:**
1. Is `settings.json` the right home for `cloud_consent_at`, or should consent live in the signed event log (tamper-evident)? (Leaning settings.json — it's a UX gate, not a security boundary; the boundary is fail-closed + the host allow-list.)
2. Is "gated off when cloud-selected-but-not-ready" the right fail mode, vs. silently running local? (Leaning gated-off + surfaced.)
3. Should we cap/redact what goes into the cloud `prompt` (e.g., warn when ingested-file content is in the recall set), or is consent sufficient disclosure? (Leaning consent-sufficient for v1; note as a future hardening.)
4. `reqwest::blocking` vs async-block-on inside the sync trait on a `spawn_blocking` thread — any runtime hazard?

---

## 6. Test strategy
- **CloudReasoner schema-translate + response-extract:** pure unit tests — feed canned Anthropic tool-use JSON / OpenAI json_schema responses, assert the engine-schema `Value` comes out; assert the fallback chain (400→json_object→fenced-parse→Err). No network.
- **Reasoner config + readiness:** pure unit tests for `reasoner_ready` (local/cloud/fail-closed matrix) + `engine_set_reasoner_config` validation (reject cloud w/o consent/key/HTTPS). Engine-command IPC test (the `commands/engine.rs:567+` pattern).
- **ConfigReasonerProvider cache invalidation:** unit test — config change rebuilds the reasoner.
- **Live `#[ignore]`:** real Anthropic/OpenAI key (never CI), asserts a real extraction round-trips.
- **UI:** vitest on the pure mode/ready/label logic; manual GUI QA for the panel + consent + banner.
- Provider hosts are NEVER contacted in CI (the only network is the `#[ignore]` live test).

---

## 7. Sequencing
2a (backend, cloud defaults off, no enable UI) → security review → PR → CI → merge. Then 2b (UI + consent) → security review (consent/banner/copy) → PR → CI → merge. Each via TDD plan + subagent-driven build + whole-impl review. Gemini = a later fast-follow behind the seam.
