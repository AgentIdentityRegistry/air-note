# Milestone D — Phase 2: Cloud Reasoner (Local or Cloud brain model) — Design Spec

**Status:** Rev 2 — critic + security reviewed (both SHIP-WITH-FIXES); §8 resolutions are AUTHORITATIVE and supersede conflicting Rev 1 text. Ready to plan/build Phase 2a.
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

---

## 8. Rev 2 — Review Resolutions (AUTHORITATIVE; supersedes conflicting Rev 1 text)

Both the critic and the security review returned SHIP-WITH-FIXES. The design shape (desktop-side `reqwest` outside the engine's `ureq` jail, off-by-default, fail-closed, host-guarded, header-only key, output-as-data) is confirmed sound and **verified** (the `ReasonerProvider` seam + reserved `EngineOpError::Reasoner` exist; the CI jail forbids `reqwest` in `bossclaw-core`; output-as-authority holds — the cloud round-trip introduces no new authority path; the schema-translation primitives are correct). The following resolutions are REQUIRED before/within 2a code. **All of C1, C2, H3, M8 are 2a-backend concerns — they land in 2a, NOT deferred to 2b.**

### R1 (Security Critical #2) — Cloud-enable + consent live in the SIGNED engine log, not `settings.json`
The webview-writable, unsigned `settings.json` MUST NOT be the sole gate for egress (the evolve on/off switch — *less* sensitive — already uses the signed log via `set_evolve_enabled`; the *more* sensitive egress switch may not get a weaker boundary). Design:
- **Non-security config** (`mode` hint, `provider`, `model`, `base_url`) MAY live in `settings.json`.
- **The egress gate is a SIGNED consent record in the engine event log** capturing `{provider, base_url_host, key_fingerprint (e.g. sha256(key)[..8]), consented_at}`, written ONLY by the `engine_enable_cloud_reasoner` command after the R5 test-key call succeeds.
- `reasoner_ready` for cloud requires the signed consent record to EXIST and **MATCH** the current `(provider, base_url_host, vault key fingerprint)`. Any mismatch — settings.json edited to a different provider/host, key rotated/changed, or no record — → **not ready → fail-closed** (re-consent required). A bare `mode=cloud` in settings.json with no matching signed record is **inert**.
- This simultaneously closes M6 (a stale shared chat key can't silently become the egress credential — its fingerprint won't match the consented one) and M7 (torn/foreign settings.json writes break the match → fail-closed).
- Build-time: NAME and audit the exact `settings.json` writer + its file mode (the reviewer couldn't find `store_read`/`store_write` registered — resolve this in the plan; the security argument holds regardless because settings.json is no longer the egress boundary).

### R2 (Security Critical #1 + High #5) — SSRF: connect-time pinning, no redirects, this is the SOLE network boundary
Cloud mode removes `OllamaReasoner`'s loopback fail-closed (`ollama.rs:80`), so the cloud client's guard is the **only** thing preventing arbitrary egress. "SHOULD reuse web_access" → **MUST**, to the connect-time bar:
- The `CloudReasoner` `reqwest::blocking::Client` MUST install a **custom `reqwest::dns::Resolve`** that runs `is_blocked_ip()` (reuse the pure classifier from `web_access.rs:118-162`) on EVERY resolved address and refuses the connection if any is internal/loopback/link-local/private/CGNAT/metadata (`169.254.169.254`). This closes the DNS-rebind race that `web_access.rs:166-173` documents as residual (a per-tick reasoner makes that race a repeating exfil primitive). `validate_host()` pre-flight stays as defense-in-depth.
- `redirect::Policy::none()` (LLM APIs never legitimately redirect) AND never forward the `Authorization`/`x-api-key` header across any hop.
- **Anthropic arm:** connect ONLY to the literal host `api.anthropic.com`; `base_url` is IGNORED for Anthropic.
- **OpenAI-compat `base_url`:** HTTPS-required; reject literal-IP base_urls outright; `is_blocked_ip` host check at config-set time AND connect time.
- Test: a `base_url` (or DNS) resolving to a blocked IP is refused before any bytes leave.

### R3 (Security High #3) — Provider error bodies are FORBIDDEN from reaching the webview
The 512-byte `last_error` truncate (`mod.rs:550`) is a length bound, not a content scrub; a provider 400 echoes prompt content (= memory/file bytes). Do NOT copy `extract_provider_error_message` (`llm_stream.rs:421-445`, which returns the provider message/body verbatim). Instead a fixed taxonomy + status only:
```rust
fn classify_cloud_error(status: u16) -> BossclawError {
    let class = match status {
        401 | 403 => "auth_rejected", 429 => "rate_limited",
        400 | 422 => "bad_request",   404 => "model_or_endpoint_not_found",
        500..=599 => "provider_5xx",  _   => "provider_error",
    };
    BossclawError::Reasoner(format!("cloud reasoner {class} (HTTP {status})"))
}
```
The raw body is consumed for classification and DROPPED — never embedded in the error string. Test: a canned error body containing a fake "prompt" substring never appears in the resulting `last_error`.

### R4 (Critic Major 1 + Security High #4) — Catch the silent-bad-key failure; honest consent
- **Silent failure:** the engine's `evolve_once` treats a Pass-A reasoner error as `break → Ok(empty report)`, so `last_error` stays `None` (`log.rs:5795-5801`, `mod.rs:544-552`) — a bad/expired key yields an evolve that silently no-ops every tick. FIX via R5 (catch at enable time). Also (nice-to-have) the desktop wrapper MAY record a `last_error` when a CLOUD tick returns `Ok` but processed 0 items, as a backstop.
- **Honest consent copy (2b, but text fixed now):** the consent modal MUST state the worst case plainly (no euphemism): *"This can include the full text of files you've ingested — including any passwords, keys, or personal data inside them — sent to <provider>. Your memory leaves this device."*
- **Tainted-content hook (2a):** the engine already tags file-derived/`is_external` recall context; surface a COUNT of tainted snippets included in a cloud tick's payload (telemetry/`last_*` field) so 2b can show "this tick sent N file-derived snippets." Specify the hook in 2a even though the UI lands in 2b.

### R5 — Test-key-on-enable (the consent-gated first contact)
The `engine_enable_cloud_reasoner` flow: user enters key + confirms the R4 consent → run ONE `complete_json` against a TRIVIAL fixed prompt (no memory/file content) against the chosen provider → on success, write the R1 signed consent record (binding provider/host/key-fp) and enable; on failure, surface the classified (R3) error AT THIS POINT and do NOT enable. This catches the bad key immediately (R4), and is the only network call in the consent flow (it sends no user memory, so it's covered by the act of consenting to enable cloud).

### R6 (Security Medium #8 + Open-Q4) — `reqwest::blocking` construction + timeout
- Build the blocking `Client` **once** per `CloudReasoner` via a runtime-free `OnceCell` constructed lazily ON the `spawn_blocking` thread (never capturing an outer runtime) — a per-tick panic ("runtime within a runtime") would be a silent permanent evolve outage caught only by `record_tick` poison-recovery. Only the KEY is read per call (from the vault); the client (with its connection pool + the R2 resolver/redirect policy) is reused.
- **Mandatory request timeout** (parity with `OLLAMA_TIMEOUT_SECS=120`, `ollama.rs:32`; 60s defensible) — the reasoner holds the `evolve_lock`, so a hung call self-DoSes the tick.
- Test: drive `complete_json` from within a `tokio::runtime` + `spawn_blocking` to prove no panic.

### R7 (Critic gaps) — required operational details
- **Anthropic `max_tokens`** is a REQUIRED field on `/v1/messages` (the existing `claude_generate` hardcodes 512, too small for extraction). Size for a 16-memory batch (≥ ~4096; pin against current Anthropic docs at build). A too-small value truncates JSON → `parse_proposals` silently drops the tail.
- **`model_id()`** returns a provider-qualified string, e.g. `"anthropic:claude-sonnet-4-6"` / `"openai-compat:gpt-5-mini"` (lands in the signed event log as provenance; must not collide with or masquerade as the local Ollama tag).
- **Header-only key, NEVER URL query** (don't copy the Gemini key-in-URL pattern `llm_stream.rs:1063-1068`; Anthropic `x-api-key`, OpenAI-compat `Authorization: Bearer`).
- **429/5xx → `BossclawError::Reasoner` → next-tick retry**, no in-tick backoff (decision, not omission).
- **Shared provider key:** the reasoner reuses the existing chat key names (`anthropic_api_key` / `openai_compat_api_key`) — one key per provider; the R1 consent binds its fingerprint so a rotation/provider-change re-consents.
- **Strict mode OFF:** Anthropic tool-use without `strict` and OpenAI `json_schema` with `strict:false` — the engine schemas (`reason.rs:126-190`) lack `additionalProperties:false`, which strict requires; non-strict + the json_object/fenced-parse fallback is correct. (Native structured-output first, prompt-and-parse as backstop.)

### R8 — 2a-sends-nothing-off-box (the load-bearing safety invariant)
Absent ANY reasoner config in `settings.json` (fresh install / no reasoner block), `ConfigReasonerProvider` MUST build the Ollama reasoner EXACTLY as today (default `mode=Local`), and `reasoner_ready` must take the local probe path. No parse default may flip to a cloud branch. Plus: 2a ships no enable UI and no signed consent can exist without R5 → **merging 2a egresses nothing**. State + test this default.

### R9 (Critic/Security minors) — test additions
Add to §6: garbage-adjudication-id → resolver mints, no crash (cite `log.rs` resolver); confidence-determinism is absorbed by `to_confidence_milli` clamp (`extract.rs:50`); the R2 blocked-IP refusal test; the R3 error-scrub test; the R6 under-runtime no-panic test; the R5 test-key success/failure paths; the R8 default-local test; the R1 signed-consent match/mismatch matrix.

### Open question deferred to 2b (non-blocking for 2a)
Whether the persistent banner should show the live tainted-snippet count (R4 hook) — UI decision at 2b.
