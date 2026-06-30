# Milestone D Phase 2b — Cloud Reasoner UI (Brain-tab enable path) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the Brain-tab UI that lets a user turn the evolve reasoner from Local (Ollama) to Cloud — a Local|Cloud selector, provider/model/key entry, an explicit blunt consent gate, and a persistent "your memory leaves this device" banner — plus the boot reseed that makes a Cloud choice survive restart.

**Architecture:** The Phase 2a backend is complete and merged (main `1eb4068`): the Rust commands `engine_get_reasoner_config`, `engine_set_reasoner_config`, and `engine_enable_cloud_reasoner` exist and are registered (`main.rs:213-218`); `reasoner_ready` / signed-log consent / SSRF pin all live behind them. 2b is **frontend + two thin Rust wiring edits**. We add TypeScript bindings, a pure view-model module (unit-tested in isolation, matching `evolveStatus.ts`), three small React components wired by dependency injection (matching the repo's `MainSearch`/`CommandPalette` test idiom — **no `mockIPC`**), the `MemoryPanel` integration, and the `main.rs` boot reseed of the reasoner-config cell.

**Tech Stack:** React 18 + TypeScript (Vite), Tauri 2 (`@tauri-apps/api/core` `invoke`), vitest + @testing-library/react (jsdom), Rust (tauri commands, `tokio`), `cargo`.

**Plan status: Rev 2 — critic + security reviewed, fixes folded in.** Security: **Risk LOW**, 0 Critical/High/Medium — "can 2b egress without consent? **No**" (the egress gate is the scheduler reading the *signed-log* `reasoner_ready`, which only `enable_cloud_reasoner` can satisfy after a test-key probe; the webview cannot forge it). Critic: **SHIP-WITH-FIXES** — design + all backend claims verified; the two compile-blockers it found (async-called-from-sync in Task 1) and the canEnable double-gate are fixed below. See "Review outcomes".

---

## Context the implementer MUST know

**The two-command distinction (get this right — it is the security model):**
- `engine_set_reasoner_config(config)` — persists `{mode,provider,model,base_url}` to the signed log. Does **NOT** grant consent and does **NOT** make cloud `ready`. Used to switch **back to Local** (`mode:"local"`), which is always safe.
- `engine_enable_cloud_reasoner(config)` — the R5 flow: validates, runs **one** trivial test-key probe against the provider, and **only on success** signs the consent record binding `{provider, host, key-fingerprint}`. This is the **only** way cloud becomes `ready`. The consent modal calls this. A bad/expired key makes this reject (and nothing is enabled). *(Verified: `enable_cloud_reasoner` at `engine/mod.rs:1045` does `probe…?` before any signed write; the command updates the in-memory cell only after the `?` succeeds.)*

**The egress gate is backend, not UI.** The background evolve scheduler computes `reasoner_ready_or_false` from the **signed log** every tick (`scheduler.rs:104-113`) and only runs `evolve_once` when ready. A buggy/malicious webview calling commands directly still cannot egress without a valid signed consent record. The UI's job is to *drive* that backend gate honestly — it is not itself the gate.

**`ReasonerConfigDto.ready` is authoritative readiness.** `reasoner_ready_or_false` returns: Local → Ollama reachable+model present; Cloud → consent record exists AND matches current provider/host/key-fingerprint. The frontend gate for the Evolve buttons becomes `cfg.ready` (replacing `ollamaReady`); never recompute readiness client-side. `ollamaStatus` stays only for the Local *detail* text.

**Re-consent is enforced by the backend binding (not the UI).** The consent record binds `provider + host + key-fingerprint`. Changing provider / base_url-host / rotating the key makes `reasoner_ready` mismatch → `ready=false` → cloud silently (and safely) stops. **Model is deliberately NOT part of the binding** — switching model under the same provider/host/key keeps egress to the same consented destination/credential (acceptable; the consent copy names the provider, not a model). The UI must *surface* a re-consent prompt when the user diverges (Low-2 fix in Task 6), but must not rely on a UI flag for safety.

**snake_case footgun.** Tauri renames the camelCase *argument name* (`config`) to the snake_case Rust param, but does **NOT** touch the *keys inside* the object. The config object's keys MUST be snake_case: `base_url`, never `baseUrl`. (Pinned by the existing Rust IPC test `engine_set_reasoner_config_binds_camelcase_and_persists` at `commands/engine.rs:926-956`.) Note: the Rust `ReasonerConfigDto` has a stale doc comment mentioning "camelCase" but has **no** `#[serde(rename_all)]` — `base_url` is serialized literally. Trust the code, ignore that comment.

**Provider/mode string literals (VERIFIED).** `provider`: `"anthropic"` / `"openai-compat"`; `mode`: `"local"` / `"cloud"`. These are the exact wire strings the signed-consent reader compares (`reason.rs:165`) and the writer emits — a TS/Rust drift would make `reasoner_ready` silently mismatch after enable. Confirmed against `provider_str` (`engine/reason.rs:126-131`) and `CloudProvider` serde (`engine/cloud_reasoner.rs:220-223`).

**Color tokens (VERIFIED to exist in `styles.css`):** use only `--error`, `--text-secondary`, `--text-tertiary`, `--surface-soft`, `--border-soft`. The repo enforces zero hardcoded colors.

**Tests = DI + pure modules, NOT `mockIPC`.** Zero `mockIPC` usage exists in the repo; the established idiom is prop-injection + `vi.fn()` (see `apps/desktop/src/shell/MainSearch.test.tsx` and `apps/desktop/src/search/CommandPalette.test.tsx`) and pure-logic modules tested directly (`apps/desktop/src/memory/evolveStatus.test.ts`). The consent-modal *component* pattern (inline `Card` + checkbox-gated button) is adapted from `apps/desktop/src/review/ReviewPanel.tsx:224-257` (that component has no test file — cite it only as a UI pattern).

**Build/test commands:**
- Frontend tests: `cd apps/desktop && npx vitest run` (or `npm run test --workspace @bossclaw/desktop`)
- Typecheck: `npm run typecheck --workspace @bossclaw/desktop` · Lint: `npm run lint`
- Rust: `cargo test -p air_agent_desktop`, `cargo build -p air_agent_desktop`, `cargo clippy --all-targets -- -D warnings`

**Deferred (NOT in this plan — explicit, per spec §198-199 + 2a carryovers):** surfacing `EvolveReport.tainted_recall_snippets` to the webview + a live "N file-derived snippets" count in the banner (the hook is genuinely write-only: defined `evolve.rs:65`, written `log.rs:5913`, no DTO read path); the Info-1 per-read consent signature re-verify; the `extract_openai_result` `tool_calls` fallback. The blunt consent copy already discloses the worst case.

---

## Review outcomes (Rev 2)

- **Security review — Risk LOW.** 0 Critical/High/Medium; 3 Low (UX/clarity), 4 Info. Verified: egress is backend-enforced (scheduler + signed consent); key never returns to the webview (`vault_has` is bool-only, no `vault_get`); bad-key path is fail-closed (cell updated only after the `?`-guarded enable succeeds). Low-1 (banner timing), Low-2 (re-consent UX), Low-3 (client check is cosmetic) — all fixed below.
- **Critic — SHIP-WITH-FIXES.** Design sound; all 6 backend claims TRUE; all CSS tokens exist; provider literals match; Tasks 2–7 compile. Fixed: **C1** (reseed called an `async fn` from the sync `.setup` → now `block_on` a tested helper), **C2** (Task 1 test was `#[test]` calling async + a nonexistent `test_engine()` → now `#[tokio::test]` using the real `test_vault_and_dir()`/`new_test_handle()` helpers), **M1** (test now exercises the actual cell-mutation wiring, not a pre-existing 2a contract), **M2** (canEnable no longer half-validates HTTPS — backend owns it). Minor file-path/line-number citations corrected.

---

## File Structure

- **Modify** `apps/desktop/src-tauri/src/engine/mod.rs` — add `reseed_reasoner_cell` helper + its `#[tokio::test]` (Task 1).
- **Modify** `apps/desktop/src-tauri/src/main.rs:65-96` — `block_on` the reseed (the documented TODO).
- **Modify** `apps/desktop/src/api/engine.ts` — add `ReasonerConfigDto`, `ReasonerConfigInput`, `ReasonerMode`, `CloudProvider`, and the three bindings.
- **Create** `apps/desktop/src/api/engine.reasoner.test.ts` — binding payload-shape test (snake_case guard).
- **Create** `apps/desktop/src/memory/reasonerView.ts` + `reasonerView.test.ts` — pure view-model.
- **Create** `apps/desktop/src/memory/CloudEgressBanner.tsx` + `CloudEgressBanner.test.tsx`.
- **Create** `apps/desktop/src/memory/CloudConsentModal.tsx` + `CloudConsentModal.test.tsx`.
- **Create** `apps/desktop/src/memory/ReasonerConfigPanel.tsx` + `ReasonerConfigPanel.test.tsx`.
- **Modify** `apps/desktop/src/memory/MemoryPanel.tsx` — fetch reasoner config, render the panel + banner, mode-aware copy, `cfg.ready` gate.

Each component takes its `api`/data dependencies as **props** (testable without IPC); `MemoryPanel` injects the real `api/engine.ts` + `vault.ts` functions.

---

### Task 1: Boot reseed — a Cloud config survives restart (Rust)

The net-new behavior is "on boot, copy the signed-log config into the in-memory `reasoner_cfg` cell that the provider closure reads each tick." We extract that into a tested `async` helper (`reseed_reasoner_cell`) so the *wiring* — not just the pre-existing readback — is covered, and `block_on` it from the synchronous `.setup` closure.

**Files:**
- Modify: `apps/desktop/src-tauri/src/engine/mod.rs` (add the helper near the other reasoner methods; add the test to the existing `#[cfg(test)] mod tests`)
- Modify: `apps/desktop/src-tauri/src/main.rs:65-96`

- [ ] **Step 1: Write the failing test** — append to the `#[cfg(test)] mod tests` in `engine/mod.rs`. Use the existing harness helpers `test_vault_and_dir()` (`mod.rs:1202`) and `new_test_handle()` (`mod.rs:1207`). All `EngineHandle` reasoner methods are `async`, so this is a `#[tokio::test]`:

```rust
#[tokio::test]
async fn reseed_reasoner_cell_loads_signed_cloud_config_into_the_cell() {
    // Restart-persistence WIRING (Phase 2b): on boot, reseed_reasoner_cell must copy
    // the signed-log config into the in-memory cell the provider closure reads.
    let (vault, dir) = test_vault_and_dir();
    let h = new_test_handle(vault, &dir);
    let cell = std::sync::Mutex::new(crate::engine::reason::ReasonerConfig::default());
    assert_eq!(cell.lock().unwrap().mode, crate::engine::reason::ReasonerMode::Local);

    // Sign a cloud config (set_reasoner_config grants NO consent — fine for this test).
    h.set_reasoner_config(true, serde_json::json!({
        "mode": "cloud", "provider": "anthropic",
        "model": "claude-sonnet-4-6", "base_url": null
    })).await.expect("set cloud config");

    // The boot reseed copies it into the cell.
    super::reseed_reasoner_cell(&h, &cell, true).await;

    let got = cell.lock().unwrap().clone();
    assert_eq!(got.mode, crate::engine::reason::ReasonerMode::Cloud);
    assert_eq!(got.model, "claude-sonnet-4-6");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p air_agent_desktop reseed_reasoner_cell_loads_signed_cloud_config_into_the_cell 2>&1 | tail -15`
Expected: FAIL to compile — `cannot find function reseed_reasoner_cell in module super` (helper not added yet). (If the harness helper names differ, align with the real ones at `mod.rs:1200-1213`.)

- [ ] **Step 3: Add the helper** at module scope in `engine/mod.rs` (alongside the other `#[cfg(unix)]` engine items, outside the `impl EngineHandle`):

```rust
/// Phase 2b boot reseed: copy the persisted (signed-log) reasoner config into the
/// in-memory cell the provider closure reads each tick, so a Cloud choice survives
/// restart. `async` because it reads the engine's signed log; `main.rs` `block_on`s it.
/// Fail-safe: a read with no signed config (or `onboarded=false`) yields the Local default.
#[cfg(unix)]
pub(crate) async fn reseed_reasoner_cell(
    engine: &EngineHandle,
    cell: &std::sync::Mutex<reason::ReasonerConfig>,
    onboarded: bool,
) {
    let seeded = engine.reasoner_config_or_default(onboarded).await;
    if let Ok(mut guard) = cell.lock() {
        *guard = seeded;
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p air_agent_desktop reseed_reasoner_cell_loads_signed_cloud_config_into_the_cell 2>&1 | grep "test result:"`
Expected: PASS (1 passed).

- [ ] **Step 5: Wire it into `main.rs`** — insert immediately after the engine block closes (after the `};` at `main.rs:96`, before the scheduler spawn at `:101`). `.setup` is synchronous, so `block_on` the async helper (a single bounded local SQLite read; the webview does not exist yet, so the cell lock is uncontended — no race):

```rust
            // Phase 2b: re-seed the reasoner-config cell from the signed log so a Cloud
            // config chosen in a previous session survives restart. `.setup` is sync, so
            // block on the async read (one local SQLite read, before the webview exists).
            #[cfg(unix)]
            tauri::async_runtime::block_on(crate::engine::reseed_reasoner_cell(
                &engine,
                &reasoner_cfg,
                identity_store.is_onboarded(),
            ));
```

- [ ] **Step 6: Update the now-stale TODO comment** at `main.rs:65-69` — replace the sentence "re-seeding it from the persisted signed log after onboarding (so a Cloud config survives restart) is deferred to Phase 2b" with "re-seeding it from the persisted signed log on boot (so a Cloud config survives restart) is done just below (Phase 2b)". Keep the rest.

- [ ] **Step 7: Build + reasoner tests**

Run: `cargo build -p air_agent_desktop 2>&1 | tail -5` then `cargo test -p air_agent_desktop reasoner 2>&1 | grep "test result:"`
Expected: build OK; reasoner tests PASS.

- [ ] **Step 8: Commit**

```bash
git add apps/desktop/src-tauri/src/main.rs apps/desktop/src-tauri/src/engine/mod.rs
git commit -m "feat(reasoner): reseed reasoner-config cell from signed log on boot (Cloud survives restart) (spec 2b)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 2: TypeScript bindings + DTOs

**Files:**
- Modify: `apps/desktop/src/api/engine.ts` (append after `:46`, near the other evolve bindings)
- Test: `apps/desktop/src/api/engine.reasoner.test.ts`

- [ ] **Step 1: Confirm the literals** — `rg -n "openai|anthropic" apps/desktop/src-tauri/src/engine/cloud_reasoner.rs apps/desktop/src-tauri/src/engine/reason.rs`. Confirm provider `"anthropic"`/`"openai-compat"` and mode `"local"`/`"cloud"`. (The Rust `ReasonerConfigDto` doc comment mentions "camelCase" but has no `#[serde(rename_all)]` — `base_url` is literal; ignore the stale comment.)

- [ ] **Step 2: Write the failing test**

```ts
import { describe, it, expect, vi, beforeEach } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invoke(...a) }));

import { getReasonerConfig, setReasonerConfig, enableCloudReasoner } from "./engine";

describe("reasoner bindings", () => {
  beforeEach(() => invoke.mockReset());

  it("getReasonerConfig invokes the get command", async () => {
    invoke.mockResolvedValue({ mode: "local", provider: "anthropic", model: "x", base_url: null, ready: false });
    await getReasonerConfig();
    expect(invoke).toHaveBeenCalledWith("engine_get_reasoner_config");
  });

  it("setReasonerConfig nests the config under a camelCase arg with snake_case value keys", async () => {
    invoke.mockResolvedValue(undefined);
    await setReasonerConfig({ mode: "cloud", provider: "openai-compat", model: "gpt-5-mini", base_url: "https://api.example.com" });
    expect(invoke).toHaveBeenCalledWith("engine_set_reasoner_config", {
      config: { mode: "cloud", provider: "openai-compat", model: "gpt-5-mini", base_url: "https://api.example.com" },
    });
  });

  it("enableCloudReasoner sends the same shape to the enable command", async () => {
    invoke.mockResolvedValue(undefined);
    await enableCloudReasoner({ mode: "cloud", provider: "anthropic", model: "claude-sonnet-4-6", base_url: null });
    expect(invoke).toHaveBeenCalledWith("engine_enable_cloud_reasoner", {
      config: { mode: "cloud", provider: "anthropic", model: "claude-sonnet-4-6", base_url: null },
    });
  });
});
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cd apps/desktop && npx vitest run src/api/engine.reasoner.test.ts`
Expected: FAIL — `getReasonerConfig is not a function`.

- [ ] **Step 4: Add the types + bindings** to `apps/desktop/src/api/engine.ts`:

```ts
export type ReasonerMode = "local" | "cloud";
export type CloudProvider = "anthropic" | "openai-compat";
export type ReasonerConfigDto = {
  mode: ReasonerMode;
  provider: CloudProvider;
  model: string;
  base_url: string | null;
  ready: boolean;
};
/** Write payload — snake_case value keys (Tauri does not rename inner object keys). No `ready` (output-only). */
export type ReasonerConfigInput = {
  mode: ReasonerMode;
  provider: CloudProvider;
  model: string;
  base_url: string | null;
};

export const getReasonerConfig = (): Promise<ReasonerConfigDto> =>
  invoke<ReasonerConfigDto>("engine_get_reasoner_config");
/** Persists config (no consent, never makes cloud ready). Used to switch back to Local. */
export const setReasonerConfig = (config: ReasonerConfigInput): Promise<void> =>
  invoke<void>("engine_set_reasoner_config", { config });
/** The R5 enable flow: test-key probe → signs consent → activates cloud. The only path that makes cloud ready. */
export const enableCloudReasoner = (config: ReasonerConfigInput): Promise<void> =>
  invoke<void>("engine_enable_cloud_reasoner", { config });
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cd apps/desktop && npx vitest run src/api/engine.reasoner.test.ts`
Expected: PASS (3 tests).

- [ ] **Step 6: Commit**

```bash
git add apps/desktop/src/api/engine.ts apps/desktop/src/api/engine.reasoner.test.ts
git commit -m "feat(reasoner-ui): TS bindings + DTOs for reasoner config/enable (snake_case payload guard) (spec 2b)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 3: Pure view-model module

**Files:**
- Create: `apps/desktop/src/memory/reasonerView.ts`
- Test: `apps/desktop/src/memory/reasonerView.test.ts`

- [ ] **Step 1: Write the failing test**

```ts
import { describe, it, expect } from "vitest";
import {
  providerLabel, defaultModelFor, vaultKeyFor, cloudActive,
  bannerText, modeBlurb, searchBlurb, consentBody, buildConfigInput,
} from "./reasonerView";
import type { ReasonerConfigDto } from "../api/engine";

const cloudReady: ReasonerConfigDto =
  { mode: "cloud", provider: "anthropic", model: "claude-sonnet-4-6", base_url: null, ready: true };

describe("reasonerView", () => {
  it("labels providers", () => {
    expect(providerLabel("anthropic")).toBe("Anthropic");
    expect(providerLabel("openai-compat")).toBe("OpenAI-compatible");
  });
  it("supplies a default model per provider", () => {
    expect(defaultModelFor("anthropic")).toBe("claude-sonnet-4-6");
    expect(defaultModelFor("openai-compat")).toBe("gpt-5-mini");
  });
  it("maps a provider to its vault key", () => {
    expect(vaultKeyFor("anthropic")).toBe("anthropic_api_key");
    expect(vaultKeyFor("openai-compat")).toBe("openai_compat_api_key");
  });
  it("cloudActive only when mode is cloud AND ready", () => {
    expect(cloudActive(cloudReady)).toBe(true);
    expect(cloudActive({ ...cloudReady, ready: false })).toBe(false);
    expect(cloudActive({ ...cloudReady, mode: "local" })).toBe(false);
  });
  it("banner names the provider and warns about egress", () => {
    expect(bannerText(cloudReady)).toBe("Brain model: Cloud · Anthropic — context leaves this device");
  });
  it("mode-aware blurbs", () => {
    expect(modeBlurb("local", "anthropic")).toContain("runs only on your machine");
    expect(modeBlurb("cloud", "anthropic")).toContain("leaves this device");
    expect(searchBlurb("local", "anthropic")).toContain("Everything stays on your machine");
    expect(searchBlurb("cloud", "openai-compat")).toContain("OpenAI-compatible");
  });
  it("consent body is blunt about file contents", () => {
    const body = consentBody("anthropic");
    expect(body).toContain("passwords, keys, or personal data");
    expect(body).toContain("Anthropic");
  });
  it("buildConfigInput emits snake_case base_url, null unless openai-compat with a value", () => {
    expect(buildConfigInput({ mode: "cloud", provider: "anthropic", model: " m ", baseUrl: "x" }))
      .toEqual({ mode: "cloud", provider: "anthropic", model: "m", base_url: null });
    expect(buildConfigInput({ mode: "cloud", provider: "openai-compat", model: "m", baseUrl: " https://h " }))
      .toEqual({ mode: "cloud", provider: "openai-compat", model: "m", base_url: "https://h" });
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd apps/desktop && npx vitest run src/memory/reasonerView.test.ts`
Expected: FAIL — cannot find module `./reasonerView`.

- [ ] **Step 3: Write the module** `apps/desktop/src/memory/reasonerView.ts`:

```ts
import type { ReasonerConfigDto, ReasonerConfigInput, ReasonerMode, CloudProvider } from "../api/engine";
import type { ProviderVaultKey } from "../vault";

export const providerLabel = (p: CloudProvider): string =>
  p === "anthropic" ? "Anthropic" : "OpenAI-compatible";

export const defaultModelFor = (p: CloudProvider): string =>
  p === "anthropic" ? "claude-sonnet-4-6" : "gpt-5-mini";

export const vaultKeyFor = (p: CloudProvider): ProviderVaultKey =>
  p === "anthropic" ? "anthropic_api_key" : "openai_compat_api_key";

/** Cloud is actively egressing only when the saved mode is cloud AND the backend reports ready. */
export const cloudActive = (cfg: ReasonerConfigDto): boolean => cfg.mode === "cloud" && cfg.ready;

export const bannerText = (cfg: ReasonerConfigDto): string =>
  `Brain model: Cloud · ${providerLabel(cfg.provider)} — context leaves this device`;

export const modeBlurb = (mode: ReasonerMode, provider: CloudProvider): string =>
  mode === "cloud"
    ? `Cloud mode sends your brain's working context — built from your memories and ingested files — to ${providerLabel(provider)}. Your memory leaves this device.`
    : "A local model can organize memories into dossiers in the background. Off by default; runs only on your machine.";

/** Search/recall is always local; only evolve egresses. Keep that distinction honest. */
export const searchBlurb = (mode: ReasonerMode, provider: CloudProvider): string =>
  mode === "cloud"
    ? `Search everything the agent has read and learned. Search stays on your machine; in Cloud mode, evolve sends context to ${providerLabel(provider)}.`
    : "Search everything the agent has read and learned. Everything stays on your machine.";

/** Blunt, no-euphemism consent body (spec R4). */
export const consentBody = (provider: CloudProvider): string =>
  `Cloud mode sends your brain's working context to ${providerLabel(provider)} on every evolve tick. ` +
  `This can include the full text of files you've ingested — including any passwords, keys, or personal data inside them. ` +
  `Your memory leaves this device. You can switch back to Local at any time.`;

/** Build the snake_case write payload (base_url only for openai-compat with a non-empty value). */
export const buildConfigInput = (form: {
  mode: ReasonerMode; provider: CloudProvider; model: string; baseUrl: string;
}): ReasonerConfigInput => ({
  mode: form.mode,
  provider: form.provider,
  model: form.model.trim(),
  base_url: form.provider === "openai-compat" && form.baseUrl.trim() !== "" ? form.baseUrl.trim() : null,
});
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd apps/desktop && npx vitest run src/memory/reasonerView.test.ts`
Expected: PASS (8 tests).

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src/memory/reasonerView.ts apps/desktop/src/memory/reasonerView.test.ts
git commit -m "feat(reasoner-ui): pure view-model (labels, copy, snake_case payload, vault-key map) (spec 2b)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 4: CloudEgressBanner component

**Files:**
- Create: `apps/desktop/src/memory/CloudEgressBanner.tsx`
- Test: `apps/desktop/src/memory/CloudEgressBanner.test.tsx`

- [ ] **Step 1: Write the failing test**

```tsx
// @vitest-environment jsdom
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { CloudEgressBanner } from "./CloudEgressBanner";
import type { ReasonerConfigDto } from "../api/engine";

const cfg: ReasonerConfigDto = { mode: "cloud", provider: "anthropic", model: "m", base_url: null, ready: true };

describe("CloudEgressBanner", () => {
  it("shows the egress warning when cloud is active", () => {
    render(<CloudEgressBanner cfg={cfg} />);
    expect(screen.getByText(/context leaves this device/i)).toBeInTheDocument();
  });
  it("renders nothing when local or not ready", () => {
    const { container, rerender } = render(<CloudEgressBanner cfg={{ ...cfg, mode: "local" }} />);
    expect(container).toBeEmptyDOMElement();
    rerender(<CloudEgressBanner cfg={{ ...cfg, ready: false }} />);
    expect(container).toBeEmptyDOMElement();
    rerender(<CloudEgressBanner cfg={null} />);
    expect(container).toBeEmptyDOMElement();
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd apps/desktop && npx vitest run src/memory/CloudEgressBanner.test.tsx`
Expected: FAIL — cannot find module `./CloudEgressBanner`.

- [ ] **Step 3: Write the component** `apps/desktop/src/memory/CloudEgressBanner.tsx` (non-dismissible by construction — no dismiss handler exists):

```tsx
import type { ReasonerConfigDto } from "../api/engine";
import { bannerText, cloudActive } from "./reasonerView";

/** Persistent, non-dismissible indicator shown only while cloud reasoning is active. */
export function CloudEgressBanner({ cfg }: { cfg: ReasonerConfigDto | null }) {
  if (!cfg || !cloudActive(cfg)) return null;
  return (
    <div
      role="status"
      style={{
        marginTop: 12, padding: "8px 12px", borderRadius: 6,
        border: "1px solid var(--error)", color: "var(--error)",
        background: "var(--surface-soft)", fontSize: 13, fontWeight: 600,
      }}
    >
      {bannerText(cfg)}
    </div>
  );
}
```

- [ ] **Step 4: Run test to verify it passes** (tokens `--error`/`--surface-soft` are confirmed present in `styles.css`)

Run: `cd apps/desktop && npx vitest run src/memory/CloudEgressBanner.test.tsx`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src/memory/CloudEgressBanner.tsx apps/desktop/src/memory/CloudEgressBanner.test.tsx
git commit -m "feat(reasoner-ui): persistent cloud egress banner (spec 2b §4)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 5: CloudConsentModal component

**Files:**
- Create: `apps/desktop/src/memory/CloudConsentModal.tsx`
- Test: `apps/desktop/src/memory/CloudConsentModal.test.tsx`

- [ ] **Step 1: Write the failing test**

```tsx
// @vitest-environment jsdom
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { CloudConsentModal } from "./CloudConsentModal";

describe("CloudConsentModal", () => {
  it("blocks Enable until the box is checked, then calls onConfirm", async () => {
    const onConfirm = vi.fn().mockResolvedValue(undefined);
    render(<CloudConsentModal provider="anthropic" onConfirm={onConfirm} onCancel={() => {}} />);
    const enable = screen.getByRole("button", { name: /enable cloud reasoner/i });
    expect(enable).toBeDisabled();
    fireEvent.click(screen.getByRole("checkbox"));
    expect(enable).toBeEnabled();
    fireEvent.click(enable);
    await waitFor(() => expect(onConfirm).toHaveBeenCalledOnce());
  });

  it("shows the classified error and stays open when enable rejects", async () => {
    const onConfirm = vi.fn().mockRejectedValue("cloud reasoner auth_rejected (HTTP 401)");
    render(<CloudConsentModal provider="anthropic" onConfirm={onConfirm} onCancel={() => {}} />);
    fireEvent.click(screen.getByRole("checkbox"));
    fireEvent.click(screen.getByRole("button", { name: /enable cloud reasoner/i }));
    expect(await screen.findByText(/auth_rejected \(HTTP 401\)/)).toBeInTheDocument();
  });

  it("Cancel calls onCancel", () => {
    const onCancel = vi.fn();
    render(<CloudConsentModal provider="anthropic" onConfirm={vi.fn()} onCancel={onCancel} />);
    fireEvent.click(screen.getByRole("button", { name: /cancel/i }));
    expect(onCancel).toHaveBeenCalledOnce();
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd apps/desktop && npx vitest run src/memory/CloudConsentModal.test.tsx`
Expected: FAIL — cannot find module `./CloudConsentModal`.

- [ ] **Step 3: Write the component** `apps/desktop/src/memory/CloudConsentModal.tsx` (adapts the `ReviewPanel.tsx:224-257` checkbox-gate pattern):

```tsx
import { useState } from "react";
import { Card } from "../components/Card";
import { Button } from "../components/Button";
import { consentBody, providerLabel } from "./reasonerView";
import type { CloudProvider } from "../api/engine";

/**
 * One-time, blunt consent gate before cloud egress is enabled. `onConfirm` performs
 * the test-key probe + signs the consent record (engine_enable_cloud_reasoner). On
 * failure it surfaces the already-classified error and does NOT enable / close.
 */
export function CloudConsentModal({
  provider, onConfirm, onCancel,
}: {
  provider: CloudProvider;
  onConfirm: () => Promise<void>;
  onCancel: () => void;
}) {
  const [acknowledged, setAcknowledged] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const onEnable = async () => {
    setBusy(true);
    setError(null);
    try {
      await onConfirm();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Card>
      <div style={{ fontWeight: 600, color: "var(--error)" }}>
        Enable Cloud Reasoner — your memory leaves this device
      </div>
      <p style={{ fontSize: 13, color: "var(--text-secondary)" }}>{consentBody(provider)}</p>
      <label style={{ display: "flex", gap: 6, alignItems: "center", fontSize: 13 }}>
        <input type="checkbox" checked={acknowledged} onChange={(e) => setAcknowledged(e.target.checked)} />
        I understand my memory will be sent to {providerLabel(provider)}
      </label>
      <div style={{ display: "flex", gap: 8, marginTop: 8 }}>
        <Button variant="primary" disabled={!acknowledged || busy} onClick={onEnable}>
          {busy ? "Enabling…" : "Enable Cloud Reasoner"}
        </Button>
        <Button variant="secondary" disabled={busy} onClick={onCancel}>Cancel</Button>
      </div>
      {error ? <p style={{ fontSize: 13, color: "var(--error)" }}>{error}</p> : null}
    </Card>
  );
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd apps/desktop && npx vitest run src/memory/CloudConsentModal.test.tsx`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src/memory/CloudConsentModal.tsx apps/desktop/src/memory/CloudConsentModal.test.tsx
git commit -m "feat(reasoner-ui): blunt one-time cloud consent modal, fail-closed on bad key (spec 2b R4/R5)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 6: ReasonerConfigPanel (selector + cloud form + enable flow)

**Files:**
- Create: `apps/desktop/src/memory/ReasonerConfigPanel.tsx`
- Test: `apps/desktop/src/memory/ReasonerConfigPanel.test.tsx`

Key behaviors (review-shaped): the **only** egress-enabling call is `onEnableCloud` via the consent modal; `onChanged` is awaited so the banner is present the instant the modal closes (Low-1); a re-consent note shows when the form's provider/base_url diverges from the consented config (Low-2); the client base_url check is cosmetic — the backend (`validate_reasoner_config` + pinned resolver) is the real SSRF gate and surfaces precise rejections (Low-3/M2).

- [ ] **Step 1: Write the failing test**

```tsx
// @vitest-environment jsdom
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { ReasonerConfigPanel } from "./ReasonerConfigPanel";
import type { ReasonerConfigDto } from "../api/engine";

const localCfg: ReasonerConfigDto = { mode: "local", provider: "anthropic", model: "claude-sonnet-4-6", base_url: null, ready: true };

function deps(over: Partial<Parameters<typeof ReasonerConfigPanel>[0]> = {}) {
  return {
    cfg: localCfg,
    onSetConfig: vi.fn().mockResolvedValue(undefined),
    onEnableCloud: vi.fn().mockResolvedValue(undefined),
    onVaultSet: vi.fn().mockResolvedValue(undefined),
    onVaultHas: vi.fn().mockResolvedValue(false),
    onChanged: vi.fn().mockResolvedValue(undefined),
    ...over,
  };
}

describe("ReasonerConfigPanel", () => {
  it("reveals the cloud form when Cloud is selected", async () => {
    render(<ReasonerConfigPanel {...deps()} />);
    fireEvent.click(screen.getByRole("button", { name: /^cloud$/i }));
    expect(await screen.findByLabelText(/provider/i)).toBeInTheDocument();
    expect(screen.getByLabelText(/model/i)).toBeInTheDocument();
  });

  it("saving a key calls vaultSet then re-checks vaultHas", async () => {
    const d = deps();
    render(<ReasonerConfigPanel {...d} />);
    fireEvent.click(screen.getByRole("button", { name: /^cloud$/i }));
    fireEvent.change(await screen.findByLabelText(/api key/i), { target: { value: "sk-test" } });
    fireEvent.click(screen.getByRole("button", { name: /save key/i }));
    await waitFor(() => expect(d.onVaultSet).toHaveBeenCalledWith("anthropic_api_key", "sk-test"));
    expect(d.onVaultHas).toHaveBeenCalledWith("anthropic_api_key");
  });

  it("re-checks vaultHas for the new provider when the provider changes", async () => {
    const onVaultHas = vi.fn().mockResolvedValue(false);
    render(<ReasonerConfigPanel {...deps({ onVaultHas })} />);
    fireEvent.click(screen.getByRole("button", { name: /^cloud$/i }));
    await screen.findByLabelText(/provider/i);
    onVaultHas.mockClear();
    fireEvent.change(screen.getByLabelText(/provider/i), { target: { value: "openai-compat" } });
    await waitFor(() => expect(onVaultHas).toHaveBeenCalledWith("openai_compat_api_key"));
  });

  it("Enable opens consent; confirming sends the snake_case payload then awaits refresh", async () => {
    const d = deps({ onVaultHas: vi.fn().mockResolvedValue(true) });
    render(<ReasonerConfigPanel {...d} />);
    fireEvent.click(screen.getByRole("button", { name: /^cloud$/i }));
    fireEvent.click(await screen.findByRole("button", { name: /^enable cloud/i }));
    fireEvent.click(await screen.findByRole("checkbox"));
    fireEvent.click(screen.getByRole("button", { name: /enable cloud reasoner/i }));
    await waitFor(() => expect(d.onEnableCloud).toHaveBeenCalledWith({
      mode: "cloud", provider: "anthropic", model: "claude-sonnet-4-6", base_url: null,
    }));
    expect(d.onChanged).toHaveBeenCalled();
  });

  it("switching to Local persists mode:local via setConfig (no consent needed)", async () => {
    const d = deps({ cfg: { ...localCfg, mode: "cloud" } });
    render(<ReasonerConfigPanel {...d} />);
    fireEvent.click(screen.getByRole("button", { name: /^local$/i }));
    await waitFor(() => expect(d.onSetConfig).toHaveBeenCalledWith(expect.objectContaining({ mode: "local" })));
    expect(d.onChanged).toHaveBeenCalled();
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd apps/desktop && npx vitest run src/memory/ReasonerConfigPanel.test.tsx`
Expected: FAIL — cannot find module `./ReasonerConfigPanel`.

- [ ] **Step 3: Write the component** `apps/desktop/src/memory/ReasonerConfigPanel.tsx`:

```tsx
import { useEffect, useState } from "react";
import { Button } from "../components/Button";
import { CloudConsentModal } from "./CloudConsentModal";
import { buildConfigInput, defaultModelFor, vaultKeyFor, providerLabel } from "./reasonerView";
import type { ReasonerConfigDto, ReasonerConfigInput, CloudProvider } from "../api/engine";
import type { ProviderVaultKey } from "../vault";

type Props = {
  cfg: ReasonerConfigDto;
  onSetConfig: (input: ReasonerConfigInput) => Promise<void>;
  onEnableCloud: (input: ReasonerConfigInput) => Promise<void>;
  onVaultSet: (key: ProviderVaultKey, value: string) => Promise<void>;
  onVaultHas: (key: ProviderVaultKey) => Promise<boolean>;
  onChanged: () => Promise<void>; // awaited so the banner/gate reflect the new cfg immediately
};

export function ReasonerConfigPanel(props: Props) {
  const { cfg, onSetConfig, onEnableCloud, onVaultSet, onVaultHas, onChanged } = props;

  const [selectedMode, setSelectedMode] = useState<"local" | "cloud">(cfg.mode);
  const [provider, setProvider] = useState<CloudProvider>(cfg.provider);
  const [model, setModel] = useState(cfg.model || defaultModelFor(cfg.provider));
  const [baseUrl, setBaseUrl] = useState(cfg.base_url ?? "");
  const [keyInput, setKeyInput] = useState("");
  const [keySaved, setKeySaved] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showConsent, setShowConsent] = useState(false);

  // Reflect whether a key is stored for the CURRENT provider (re-runs on provider change).
  useEffect(() => {
    let alive = true;
    void onVaultHas(vaultKeyFor(provider)).then((has) => { if (alive) setKeySaved(has); });
    return () => { alive = false; };
  }, [provider, onVaultHas]);

  const onSelectLocal = async () => {
    setSelectedMode("local");
    if (cfg.mode === "cloud") {
      setBusy(true); setError(null);
      try {
        await onSetConfig({ mode: "local", provider, model: model.trim(), base_url: null });
        await onChanged();
      } catch (e) { setError(String(e)); } finally { setBusy(false); }
    }
  };

  const onChangeProvider = (p: CloudProvider) => {
    setProvider(p);
    setModel(defaultModelFor(p)); // note: overwrites a hand-typed model on provider switch (acceptable)
  };

  const onSaveKey = async () => {
    setBusy(true); setError(null);
    try {
      await onVaultSet(vaultKeyFor(provider), keyInput);
      setKeyInput("");
      setKeySaved(await onVaultHas(vaultKeyFor(provider)));
    } catch (e) { setError(String(e)); } finally { setBusy(false); }
  };

  const formInput = (): ReasonerConfigInput =>
    buildConfigInput({ mode: "cloud", provider, model, baseUrl });

  // Client gate is COSMETIC. The real SSRF/HTTPS enforcement is the backend
  // validate_reasoner_config + the connect-time pinned resolver, which also surface
  // the precise rejection through the consent modal's error.
  const canEnable = keySaved && model.trim() !== "" &&
    (provider !== "openai-compat" || baseUrl.trim() !== "");

  // Low-2: if cloud is enabled and the form diverges from the CONSENTED provider/host,
  // the backend will fail-close (consent binding mismatch). Tell the user to re-consent.
  const consentedBaseUrl = cfg.base_url ?? null;
  const formBaseUrl = formInput().base_url;
  const needsReconsent = cfg.mode === "cloud" && cfg.ready &&
    (provider !== cfg.provider || formBaseUrl !== consentedBaseUrl);

  const onConfirmConsent = async () => {
    await onEnableCloud(formInput());
    await onChanged();          // settle cfg.ready BEFORE closing so the banner shows immediately
    setShowConsent(false);
  };

  return (
    <div style={{ marginTop: 12 }}>
      <div style={{ display: "flex", gap: 8 }}>
        <Button variant={selectedMode === "local" ? "primary" : "secondary"} disabled={busy} onClick={onSelectLocal}>
          Local
        </Button>
        <Button variant={selectedMode === "cloud" ? "primary" : "secondary"} disabled={busy} onClick={() => setSelectedMode("cloud")}>
          Cloud
        </Button>
      </div>

      {selectedMode === "cloud" ? (
        <div style={{ marginTop: 8, display: "flex", flexDirection: "column", gap: 8 }}>
          <label style={{ fontSize: 13 }}>
            Provider
            <select value={provider} onChange={(e) => onChangeProvider(e.target.value as CloudProvider)} style={{ marginLeft: 8 }}>
              <option value="anthropic">Anthropic</option>
              <option value="openai-compat">OpenAI-compatible</option>
            </select>
          </label>

          <label style={{ fontSize: 13 }}>
            Model
            <input value={model} onChange={(e) => setModel(e.target.value)} style={{ marginLeft: 8 }} />
          </label>

          {provider === "openai-compat" ? (
            <label style={{ fontSize: 13 }}>
              Base URL (HTTPS)
              <input value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)} placeholder="https://…" style={{ marginLeft: 8 }} />
            </label>
          ) : null}

          <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
            <label style={{ fontSize: 13 }}>
              API key
              <input type="password" value={keyInput} onChange={(e) => setKeyInput(e.target.value)} style={{ marginLeft: 8 }} />
            </label>
            <Button variant="secondary" disabled={busy || keyInput.trim() === ""} onClick={onSaveKey}>Save key</Button>
            {keySaved ? <span style={{ fontSize: 13, color: "var(--text-secondary)" }}>key saved ✓</span> : null}
          </div>

          {needsReconsent ? (
            <p style={{ fontSize: 13, color: "var(--error)" }}>
              You changed the provider or base URL — click Enable Cloud to re-consent before cloud resumes.
            </p>
          ) : null}

          <div>
            <Button variant="primary" disabled={busy || !canEnable} onClick={() => setShowConsent(true)}>
              Enable Cloud ({providerLabel(provider)})
            </Button>
          </div>

          {error ? <p style={{ fontSize: 13, color: "var(--error)" }}>{error}</p> : null}
        </div>
      ) : null}

      {showConsent ? (
        <CloudConsentModal provider={provider} onConfirm={onConfirmConsent} onCancel={() => setShowConsent(false)} />
      ) : null}
    </div>
  );
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd apps/desktop && npx vitest run src/memory/ReasonerConfigPanel.test.tsx`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src/memory/ReasonerConfigPanel.tsx apps/desktop/src/memory/ReasonerConfigPanel.test.tsx
git commit -m "feat(reasoner-ui): Local|Cloud selector + form + enable flow (await refresh, re-consent note) (spec 2b §4)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 7: Wire into MemoryPanel (fetch config, render panel + banner, mode-aware copy, cfg.ready gate)

**Files:**
- Modify: `apps/desktop/src/memory/MemoryPanel.tsx`

- [ ] **Step 1: Add imports** — extend the `api/engine` import (`:4-7`) with the reasoner bindings + DTO; add `vault` + the new components + view helpers:

```tsx
import {
  recall, evolveStatus, setEvolveEnabled, evolveNow, ollamaStatus,
  getReasonerConfig, setReasonerConfig, enableCloudReasoner,
  type HitDto, type EvolveStatusDto, type OllamaStatusDto, type ReasonerConfigDto,
} from "../api/engine";
import { vaultSet, vaultHas } from "../vault";
import { ReasonerConfigPanel } from "./ReasonerConfigPanel";
import { CloudEgressBanner } from "./CloudEgressBanner";
import { searchBlurb, modeBlurb } from "./reasonerView";
```

- [ ] **Step 2: Add reasoner config state + fetch it in `refreshStatus`** — add state next to the evolve state (`:25-30`):

```tsx
  const [reasonerCfg, setReasonerCfg] = useState<ReasonerConfigDto | null>(null);
```

and extend `refreshStatus` (`:32-41`) to fetch all three together (it already returns `Promise<void>`, which `ReasonerConfigPanel.onChanged` will await):

```tsx
  const refreshStatus = async () => {
    try {
      const [s, o, r] = await Promise.all([evolveStatus(), ollamaStatus(), getReasonerConfig()]);
      setStatus(s);
      setOllama(o);
      setReasonerCfg(r);
      setUnavailable(false);
    } catch {
      setUnavailable(true);
    }
  };
```

- [ ] **Step 3: Replace the `ollamaReady` gate with the unified `reasonerReady`** — change `:103`:

```tsx
  const reasonerReady = !!reasonerCfg?.ready;
  const isCloud = reasonerCfg?.mode === "cloud";
```

Update the two Evolve buttons (in the `:183-194` button `<div>`) to gate on `reasonerReady` instead of `ollamaReady`:

```tsx
          <Button variant="secondary" onClick={onToggleEvolve} disabled={!status || toggling || !reasonerReady}>
            {status?.enabled ? "Turn Evolve Off" : "Turn Evolve On"}
          </Button>
          <Button variant="primary" onClick={onEvolveNow} disabled={evolving || !reasonerReady || !status?.enabled}>
            {evolving ? "Evolving…" : "Evolve Now"}
          </Button>
```

- [ ] **Step 4: Mode-aware copy + render panel + banner.**

Header blurb `:108-110` → mode-aware (`reasonerCfg` may be null on first paint → default Local copy):

```tsx
      <p style={{ color: "var(--text-secondary)", fontSize: 13 }}>
        {searchBlurb(reasonerCfg?.mode ?? "local", reasonerCfg?.provider ?? "anthropic")}
      </p>
```

Add the banner directly under that header `<p>`:

```tsx
      <CloudEgressBanner cfg={reasonerCfg} />
```

Evolve blurb `:158-160` → mode-aware:

```tsx
        <p style={{ color: "var(--text-secondary)", fontSize: 13 }}>
          {modeBlurb(reasonerCfg?.mode ?? "local", reasonerCfg?.provider ?? "anthropic")}
        </p>
```

Render the config panel after the Evolve blurb (before the status line), injecting the real api/vault functions (`onChanged` returns the `refreshStatus` promise so the panel can await it — Low-1):

```tsx
        {reasonerCfg ? (
          <ReasonerConfigPanel
            cfg={reasonerCfg}
            onSetConfig={setReasonerConfig}
            onEnableCloud={enableCloudReasoner}
            onVaultSet={vaultSet}
            onVaultHas={vaultHas}
            onChanged={refreshStatus}
          />
        ) : null}
```

- [ ] **Step 5: Gate the Local-only detail by mode** — the "Local model: …" line (`:166-175`) and the `ollama pull` install hint (`:177-181`) are irrelevant in Cloud mode. Wrap each block's render in `{!isCloud && ( … )}` (keep the existing inner `{!ollamaReady && ollama != null ? …}` ternary intact inside the install-hint block).

- [ ] **Step 6: Typecheck + run the full frontend suite**

Run: `npm run typecheck --workspace @bossclaw/desktop && cd apps/desktop && npx vitest run`
Expected: typecheck clean; all tests PASS (existing + new reasoner tests).

- [ ] **Step 7: Commit**

```bash
git add apps/desktop/src/memory/MemoryPanel.tsx
git commit -m "feat(reasoner-ui): wire Local|Cloud panel + banner + mode-aware copy + cfg.ready gate into Brain tab (spec 2b §4)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 8: Full-suite gate + manual QA checklist

**Files:** none (verification only)

- [ ] **Step 1: All automated gates** — each must pass:
```bash
npm run typecheck --workspace @bossclaw/desktop
npm run lint
cd apps/desktop && npx vitest run
cd ~/air-note && cargo build -p air_agent_desktop 2>&1 | tail -5
cargo clippy --all-targets -- -D warnings 2>&1 | tail -5
cargo test -p air_agent_desktop reasoner 2>&1 | grep "test result:"
```

- [ ] **Step 2: Token-purity grep** (repo gate)

Run: `rg -n "#[0-9a-fA-F]{3,8}|rgb\(|rgba\(" apps/desktop/src/memory/*.tsx`
Expected: no hardcoded colors in the new components.

- [ ] **Step 3: Manual GUI QA** (`npm run dev`; if keychain prompts, use `scripts/dev-build-signed.sh`) — record results:
  - [ ] Default: Brain tab shows **Local** selected, no egress banner, copy reads "Everything stays on your machine".
  - [ ] Select Cloud → form appears; provider dropdown + model prefilled; Enable disabled until a key is saved.
  - [ ] Save an Anthropic key → "key saved ✓"; Enable available.
  - [ ] Click Enable → consent modal with the blunt copy; button disabled until the box is checked.
  - [ ] Confirm with a **bad** key → classified error shown, modal stays, **no banner** (fail-closed).
  - [ ] Confirm with a **good** key → modal closes, **persistent banner** appears immediately, Evolve buttons enabled.
  - [ ] Change provider while enabled → "re-consent" note appears, banner drops (fail-closed).
  - [ ] Quit + relaunch → still Cloud + banner (the Task-1 reseed).
  - [ ] Switch back to Local → banner disappears, no re-consent needed.
  - [ ] Light + dark mode both legible.

- [ ] **Step 4: Live round-trip** (the deferred 2a QA, now actionable) — run the `#[ignore]`d `cloud_reasoner_live_roundtrip` with a real key:

Run: `cargo test -p air_agent_desktop cloud_reasoner_live_roundtrip -- --ignored --nocapture`
Expected: a real extraction round-trips (confirms the socket/header path end-to-end).

---

## Review / CI / PR sequencing (after Task 8)

Egress-**enabling** UI → both reviews mandatory:
1. **Whole-impl review** (`superpowers:code-reviewer` over the full 2b diff) — the two-command distinction (consent only via `enableCloudReasoner`), the `cfg.ready` gate, no key rendered, honest mode-aware copy, the Low-1/Low-2 fixes landed.
2. **Dedicated security review** (`oh-my-claudecode:security-reviewer`) — re-confirm against the implemented code what the plan-review confirmed against the plan: no UI path reaches cloud-on without consent + a passing test-key; banner bound to `cloudActive`; provider/key change forces re-consent.
3. Fix-loop → push `milestone-d2b-cloud-reasoner-ui` → PR → CI green (all platforms + `cargo-audit`) → Peter merge.
4. GBrain: write the 2b handoff, re-point `air/session-start-protocol` (fresh fetch), append lessons, re-run the Step 0 audit.

---

## Deferred (explicitly out of scope for 2b)

- Surface `EvolveReport.tainted_recall_snippets` (Rust+TS) + a live "N file-derived snippets sent" count near the banner (spec §198-199; hook is write-only at `evolve.rs:65` / `log.rs:5913`). Blunt consent copy meets the R4 disclosure floor.
- Info-1 per-read consent signature re-verify on the readiness path.
- `extract_openai_result` `tool_calls` fallback.
- Gemini provider (later fast-follow behind the same seam).

---

## Self-Review (against spec §4 + §8 carryovers)

- **§4 Local|Cloud selector (default Local):** Task 6. ✅
- **§4 cloud sub-panel (provider/model/base_url/key via vaultSet + vaultHas "saved ✓"):** Task 6. ✅
- **§4 explicit consent gate → R1/R5 via `engine_enable_cloud_reasoner`:** Tasks 5 + 6. ✅
- **§4 persistent banner while active:** Task 4 (`cloudActive` gate). ✅
- **§4 copy changes (`:109`, `:159`) → mode-aware:** Task 7 (`searchBlurb`/`modeBlurb`). ✅
- **§4 `ollamaReady` → `reasonerReady` (`cfg.ready`):** Task 7. ✅
- **§4 pure logic extracted + tested; panel DI-tested (no mockIPC):** Tasks 3–6. ✅
- **R1 signed consent only via enable:** consent modal calls `enableCloudReasoner`, never `setReasonerConfig`. ✅
- **R1 re-consent binding surfaced:** Task 6 `needsReconsent` note (Low-2). ✅
- **R4 honest consent copy:** `consentBody` ("passwords, keys, or personal data" + "leaves this device"). ✅
- **R4 silent-bad-key caught:** enable-time test-key probe surfaced as the modal error (Task 5). ✅
- **2a carryover — cell→log restart-reseed:** Task 1 (tested `reseed_reasoner_cell` + `block_on` wiring). ✅
- **2a carryover — snake_case config keys:** `buildConfigInput` + Task 2 binding test. ✅
- **R8 default-local egresses nothing:** unchanged — default mode local; cloud only via the consent gate; reseed is fail-safe Local when no signed config / not onboarded. ✅

**Placeholder scan:** none — every step has runnable code/commands. **Type consistency:** `ReasonerConfigInput`/`ReasonerConfigDto`/`CloudProvider`/`ReasonerMode` defined once (Task 2), used unchanged; `vaultKeyFor` returns `ProviderVaultKey`; payload keys snake_case throughout; `onChanged: () => Promise<void>` consistent between Task 6 (awaited) and Task 7 (passes `refreshStatus`). **Compile-safety (Rust):** Task 1 uses the real harness (`test_vault_and_dir`/`new_test_handle`), `#[tokio::test] async` + `.await`, and `block_on` for the sync-`.setup` call site.
