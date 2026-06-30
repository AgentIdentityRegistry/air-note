# Milestone D Phase 2b — Cloud Reasoner UI (Brain-tab enable path) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the Brain-tab UI that lets a user turn the evolve reasoner from Local (Ollama) to Cloud — a Local|Cloud selector, provider/model/key entry, an explicit blunt consent gate, and a persistent "your memory leaves this device" banner — plus the boot reseed that makes a Cloud choice survive restart.

**Architecture:** The Phase 2a backend is complete and merged (HEAD `1eb4068`): the Rust commands `engine_get_reasoner_config`, `engine_set_reasoner_config`, and `engine_enable_cloud_reasoner` exist and are registered (`main.rs:213-218`); `reasoner_ready` / signed-log consent / SSRF pin all live behind them. 2b is **frontend + two thin wiring tasks**. We add TypeScript bindings, a pure view-model module (unit-tested in isolation, matching `evolveStatus.ts`), three small React components wired by dependency injection (matching the repo's `MainSearch`/`ReviewPanel` test idiom — **no `mockIPC`**), the `MemoryPanel` integration, and the `main.rs` boot reseed of the reasoner-config cell.

**Tech Stack:** React 18 + TypeScript (Vite), Tauri 2 (`@tauri-apps/api/core` `invoke`), vitest + @testing-library/react (jsdom), Rust (tauri commands), `cargo`.

---

## Context the implementer MUST know

**The two-command distinction (get this right — it is the security model):**
- `engine_set_reasoner_config(config)` — persists `{mode,provider,model,base_url}` to the signed log. Does **NOT** grant consent and does **NOT** make cloud `ready`. Used to switch **back to Local** (`mode:"local"`), which is always safe.
- `engine_enable_cloud_reasoner(config)` — the R5 flow: validates, runs **one** trivial test-key probe against the provider, and **only on success** signs the consent record binding `{provider, host, key-fingerprint}`. This is the **only** way cloud becomes `ready`. The consent modal calls this. A bad/expired key makes this reject (and nothing is enabled).

**`ReasonerConfigDto.ready` is authoritative readiness.** The backend's `reasoner_ready_or_false` already returns: Local → Ollama reachable+model present; Cloud → consent record exists AND matches current provider/host/key-fingerprint. The frontend gate for the Evolve buttons becomes `cfg.ready` (replacing `ollamaReady`). `ollamaStatus` stays only for the Local *detail* text.

**snake_case footgun.** Tauri renames the camelCase *argument name* (`config`) to the snake_case Rust param, but it does **NOT** touch the *keys inside* the object. So the config object's keys MUST be snake_case: `base_url`, never `baseUrl`. (An existing Rust IPC test, `engine_set_reasoner_config_binds_camelcase_and_persists` at `commands/engine.rs:926-956`, pins this.)

**Provider string literals.** The DTO documents `provider` as `"anthropic" | "openai-compat"`. Task 2 includes a verification step against the Rust `CloudProvider` (de)serialization (`engine/cloud_reasoner.rs:220-223`, `provider_str` at `engine/reason.rs:126-131`) — use whatever literal those actually emit; this plan assumes `"anthropic"` / `"openai-compat"`.

**Color tokens.** The repo enforces zero hardcoded colors (eslint + grep gate). Use only CSS variables that already exist in `styles.css`. This plan uses the tokens observed in `MemoryPanel.tsx`/`ReviewPanel.tsx`: `--error`, `--text-secondary`, `--text-tertiary`, `--surface-soft`, `--border-soft`. Task 4/5 include a step to confirm any token name against `styles.css` before committing.

**Build/test commands:**
- Frontend tests: `npm run test --workspace @bossclaw/desktop` (or `cd apps/desktop && npx vitest run`)
- Typecheck: `npm run typecheck --workspace @bossclaw/desktop`
- Lint: `npm run lint`
- Rust: `cargo test -p bossclaw-core` and `cargo build -p air_agent_desktop` and `cargo clippy --all-targets -- -D warnings`

**Deferred (NOT in this plan — explicit, per spec §198-199 + 2a carryovers):** surfacing `EvolveReport.tainted_recall_snippets` to the webview and a live "N file-derived snippets" count in the banner; the Info-1 per-read consent signature re-verify; the `extract_openai_result` `tool_calls` fallback. The blunt consent copy already discloses the worst case, so the live count is a follow-up.

---

## File Structure

- **Modify** `apps/desktop/src-tauri/src/main.rs:65-96` — boot reseed of `reasoner_cfg` from the signed log (the documented TODO).
- **Modify** `apps/desktop/src-tauri/src/engine/mod.rs` (or `engine/reason.rs`) — add a tested round-trip regression for restart persistence (Task 1).
- **Modify** `apps/desktop/src/api/engine.ts` — add `ReasonerConfigDto`, `ReasonerConfigInput`, and the three bindings.
- **Create** `apps/desktop/src/api/engine.reasoner.test.ts` — binding payload-shape test (snake_case guard).
- **Create** `apps/desktop/src/memory/reasonerView.ts` — pure view-model (labels, defaults, copy, payload builder, vault-key map).
- **Create** `apps/desktop/src/memory/reasonerView.test.ts` — pure unit tests.
- **Create** `apps/desktop/src/memory/CloudEgressBanner.tsx` + `CloudEgressBanner.test.tsx`.
- **Create** `apps/desktop/src/memory/CloudConsentModal.tsx` + `CloudConsentModal.test.tsx`.
- **Create** `apps/desktop/src/memory/ReasonerConfigPanel.tsx` + `ReasonerConfigPanel.test.tsx`.
- **Modify** `apps/desktop/src/memory/MemoryPanel.tsx` — fetch reasoner config, render the panel + banner, mode-aware copy, `cfg.ready` gate.

Each component takes its `api`/data dependencies as **props** so it is testable without IPC; `MemoryPanel` injects the real `api/engine.ts` + `vault.ts` functions.

---

### Task 1: Boot reseed — a Cloud config survives restart (Rust)

**Files:**
- Modify: `apps/desktop/src-tauri/src/main.rs:65-96`
- Test: `apps/desktop/src-tauri/src/engine/mod.rs` (engine unit tests module)

- [ ] **Step 1: Write the failing test** (in the `#[cfg(test)] mod tests` of `engine/mod.rs`; mirror the existing 2a reasoner tests — find one with `rg "fn reasoner_" apps/desktop/src-tauri/src/engine/mod.rs` and copy its harness for building a temp `EngineHandle`)

```rust
#[test]
fn signed_cloud_config_is_what_boot_reseed_reads_back() {
    // Restart persistence contract (Phase 2b): a Cloud config signed in a prior
    // session must be exactly what main.rs reseeds the reasoner cell with on boot.
    let h = test_engine(); // existing helper: builds an EngineHandle on a temp dir
    let onboarded = true;

    // Default before any signed config = Local.
    assert_eq!(h.reasoner_config_or_default(onboarded).mode, ReasonerMode::Local);

    // Sign a cloud config (no consent needed for set_reasoner_config).
    let cfg = serde_json::json!({
        "mode": "cloud", "provider": "anthropic",
        "model": "claude-sonnet-4-6", "base_url": null
    });
    h.set_reasoner_config(onboarded, cfg).expect("set");

    // The boot reseed reads this back verbatim.
    let seeded = h.reasoner_config_or_default(onboarded);
    assert_eq!(seeded.mode, ReasonerMode::Cloud);
    assert_eq!(seeded.model, "claude-sonnet-4-6");
}
```

- [ ] **Step 2: Run test to verify it passes-or-fails honestly**

Run: `cargo test -p air_agent_desktop signed_cloud_config_is_what_boot_reseed_reads_back -- --nocapture`
Expected: PASS if `set_reasoner_config`/`reasoner_config_or_default` signatures match; if it FAILS to compile, fix the test's call shape against the real signatures (`engine/mod.rs:978-987`) — do **not** change production code to fit a wrong test. (This test guards the contract the reseed depends on; the wiring itself is Step 3.)

- [ ] **Step 3: Wire the reseed in `main.rs`** — insert immediately after the engine block closes (after the `};` at `main.rs:96`, before the scheduler spawn at `:101`):

```rust
            // Phase 2b: re-seed the reasoner-config cell from the signed log so a
            // Cloud config chosen in a previous session survives restart (the cell
            // otherwise starts at the Local default each boot — see TODO above).
            #[cfg(unix)]
            {
                let onboarded = identity_store.is_onboarded();
                let seeded = engine.reasoner_config_or_default(onboarded);
                if let Ok(mut cell) = reasoner_cfg.lock() {
                    *cell = seeded;
                }
            }
```

- [ ] **Step 4: Update the now-stale TODO comment** at `main.rs:65-69` — replace "is deferred to Phase 2b" with "is done just below (Phase 2b)". Keep the rest of the comment.

- [ ] **Step 5: Build + test**

Run: `cargo build -p air_agent_desktop 2>&1 | tail -5` then `cargo test -p air_agent_desktop reasoner 2>&1 | grep "test result:"`
Expected: build OK; reasoner tests PASS.

- [ ] **Step 6: Commit**

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

- [ ] **Step 1: Verify the provider literals** — `rg "openai" apps/desktop/src-tauri/src/engine/cloud_reasoner.rs apps/desktop/src-tauri/src/engine/reason.rs`. Confirm the serialized provider strings (`"anthropic"`, `"openai-compat"`) and mode strings (`"local"`, `"cloud"`). Use the real literals in the types below if they differ.

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
Expected: FAIL — `getReasonerConfig is not a function` (bindings not added yet).

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
/** The write payload — snake_case value keys (Tauri does not rename inner object keys). No `ready` (output-only). */
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

- [ ] **Step 3: Write the component** `apps/desktop/src/memory/CloudEgressBanner.tsx`:

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

- [ ] **Step 4: Confirm color tokens exist** — `rg -- "--error|--surface-soft" apps/desktop/src/styles.css | head`. If a token name differs, use the real one. Then run the test:

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
 * failure it surfaces the already-classified error and does NOT enable.
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
    onChanged: vi.fn(),
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

  it("Enable opens consent; confirming sends the snake_case payload to enableCloud", async () => {
    const d = deps({ onVaultHas: vi.fn().mockResolvedValue(true) });
    render(<ReasonerConfigPanel {...d} />);
    fireEvent.click(screen.getByRole("button", { name: /^cloud$/i }));
    // key already saved (vaultHas=true) → Enable is available
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
import {
  buildConfigInput, defaultModelFor, vaultKeyFor, providerLabel,
} from "./reasonerView";
import type { ReasonerConfigDto, ReasonerConfigInput, CloudProvider } from "../api/engine";
import type { ProviderVaultKey } from "../vault";

type Props = {
  cfg: ReasonerConfigDto;
  onSetConfig: (input: ReasonerConfigInput) => Promise<void>;
  onEnableCloud: (input: ReasonerConfigInput) => Promise<void>;
  onVaultSet: (key: ProviderVaultKey, value: string) => Promise<void>;
  onVaultHas: (key: ProviderVaultKey) => Promise<boolean>;
  onChanged: () => void;
};

export function ReasonerConfigPanel(props: Props) {
  const { cfg, onSetConfig, onEnableCloud, onVaultSet, onVaultHas, onChanged } = props;

  // UI mode (which panel is shown). Initialized from the saved config.
  const [selectedMode, setSelectedMode] = useState<"local" | "cloud">(cfg.mode);
  const [provider, setProvider] = useState<CloudProvider>(cfg.provider);
  const [model, setModel] = useState(cfg.model || defaultModelFor(cfg.provider));
  const [baseUrl, setBaseUrl] = useState(cfg.base_url ?? "");
  const [keyInput, setKeyInput] = useState("");
  const [keySaved, setKeySaved] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showConsent, setShowConsent] = useState(false);

  // Reflect whether a key is stored for the current provider.
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
        onChanged();
      } catch (e) { setError(String(e)); } finally { setBusy(false); }
    }
  };

  const onChangeProvider = (p: CloudProvider) => {
    setProvider(p);
    setModel(defaultModelFor(p));
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

  const canEnable = keySaved && model.trim() !== "" &&
    (provider !== "openai-compat" || baseUrl.trim().startsWith("https://"));

  const onConfirmConsent = async () => {
    await onEnableCloud(formInput());
    setShowConsent(false);
    onChanged();
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
            <select
              value={provider}
              onChange={(e) => onChangeProvider(e.target.value as CloudProvider)}
              style={{ marginLeft: 8 }}
            >
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
Expected: PASS (4 tests). If a query fails on label association, ensure inputs are wrapped by their `<label>` (they are) — testing-library matches nested label text.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src/memory/ReasonerConfigPanel.tsx apps/desktop/src/memory/ReasonerConfigPanel.test.tsx
git commit -m "feat(reasoner-ui): Local|Cloud selector + provider/model/key form + enable flow (spec 2b §4)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 7: Wire into MemoryPanel (fetch config, render panel + banner, mode-aware copy, cfg.ready gate)

**Files:**
- Modify: `apps/desktop/src/memory/MemoryPanel.tsx`

- [ ] **Step 1: Add imports** — extend the `api/engine` import (`:4-7`) with the reasoner bindings + DTO, add `vault` + the new components + view helpers:

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

and extend `refreshStatus` (`:32-41`) to fetch all three together:

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

Update the two Evolve buttons (`:187`, `:191`) to gate on `reasonerReady` instead of `ollamaReady`:

```tsx
          <Button variant="secondary" onClick={onToggleEvolve} disabled={!status || toggling || !reasonerReady}>
            {status?.enabled ? "Turn Evolve Off" : "Turn Evolve On"}
          </Button>
          <Button variant="primary" onClick={onEvolveNow} disabled={evolving || !reasonerReady || !status?.enabled}>
            {evolving ? "Evolving…" : "Evolve Now"}
          </Button>
```

- [ ] **Step 4: Make the two copy strings mode-aware + render the panel + banner.**

Header blurb `:108-110` → mode-aware (`reasonerCfg` may be null on first paint → default to Local copy):

```tsx
      <p style={{ color: "var(--text-secondary)", fontSize: 13 }}>
        {searchBlurb(reasonerCfg?.mode ?? "local", reasonerCfg?.provider ?? "anthropic")}
      </p>
```

Add the banner right under the header `<p>`:

```tsx
      <CloudEgressBanner cfg={reasonerCfg} />
```

Evolve blurb `:158-160` → mode-aware:

```tsx
        <p style={{ color: "var(--text-secondary)", fontSize: 13 }}>
          {modeBlurb(reasonerCfg?.mode ?? "local", reasonerCfg?.provider ?? "anthropic")}
        </p>
```

Render the config panel after the Evolve blurb (before the status line), injecting the real api/vault functions:

```tsx
        {reasonerCfg ? (
          <ReasonerConfigPanel
            cfg={reasonerCfg}
            onSetConfig={setReasonerConfig}
            onEnableCloud={enableCloudReasoner}
            onVaultSet={vaultSet}
            onVaultHas={vaultHas}
            onChanged={() => void refreshStatus()}
          />
        ) : null}
```

- [ ] **Step 5: Gate the Local-only detail text by mode** — the "Local model: …" line (`:166-175`) and the `ollama pull` install hint (`:177-181`) should render only when `!isCloud` (in Cloud mode they're irrelevant). Wrap both in `{!isCloud && ( … )}`.

- [ ] **Step 6: Typecheck + run the full frontend suite**

Run: `npm run typecheck --workspace @bossclaw/desktop && cd apps/desktop && npx vitest run`
Expected: typecheck clean; all tests PASS (existing + the new reasoner tests).

- [ ] **Step 7: Commit**

```bash
git add apps/desktop/src/memory/MemoryPanel.tsx
git commit -m "feat(reasoner-ui): wire Local|Cloud panel + banner + mode-aware copy + cfg.ready gate into Brain tab (spec 2b §4)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 8: Full-suite gate + manual QA checklist

**Files:** none (verification only)

- [ ] **Step 1: All automated gates**

Run each; all must pass:
```bash
npm run typecheck --workspace @bossclaw/desktop
npm run lint
cd apps/desktop && npx vitest run
cd ~/air-note && cargo build -p air_agent_desktop 2>&1 | tail -5
cargo clippy --all-targets -- -D warnings 2>&1 | tail -5
cargo test -p air_agent_desktop reasoner 2>&1 | grep "test result:"
```

- [ ] **Step 2: Token-purity grep** (the repo gate)

Run: `rg -n "#[0-9a-fA-F]{3,8}|rgb\(|rgba\(" apps/desktop/src/memory/*.tsx`
Expected: no hardcoded colors in the new components.

- [ ] **Step 3: Manual GUI QA** (`npm run dev`, signed dev build per `scripts/dev-build-signed.sh` if keychain prompts) — record results:
  - [ ] Default state: Brain tab shows **Local** selected, no egress banner, copy reads "Everything stays on your machine".
  - [ ] Select Cloud → form appears; provider dropdown + model prefilled; Enable disabled until a key is saved.
  - [ ] Save an Anthropic key → "key saved ✓"; Enable becomes available.
  - [ ] Click Enable → consent modal with the blunt copy; Enable button disabled until the box is checked.
  - [ ] Confirm with a **bad** key → classified error shown, modal stays, **no banner** (fail-closed).
  - [ ] Confirm with a **good** key → modal closes, **persistent banner** appears, Evolve buttons enabled.
  - [ ] Quit + relaunch → still Cloud + banner (the Task-1 reseed).
  - [ ] Switch back to Local → banner disappears, no re-consent needed.
  - [ ] Light + dark mode both legible.

- [ ] **Step 4: Live round-trip (the deferred 2a QA, now actionable)** — run the `#[ignore]`d `cloud_reasoner_live_roundtrip` with a real key:

Run: `cargo test -p air_agent_desktop cloud_reasoner_live_roundtrip -- --ignored --nocapture`
Expected: a real extraction round-trips (confirms the socket/header path end-to-end).

---

## Review / CI / PR sequencing (after Task 8)

Per the locked milestone process (this is an egress-**enabling** UI — both reviews are mandatory):
1. **Whole-impl review** (`superpowers:code-reviewer` over the full 2b diff) — focus: the two-command distinction is honored (consent only via `enableCloudReasoner`), the `cfg.ready` gate, no key ever rendered, mode-aware copy is honest.
2. **Dedicated security review** (`oh-my-claudecode:security-reviewer`) — focus: can the webview reach a cloud-egressing state WITHOUT the consent modal / a successful test-key? Is the banner truly tied to `cloudActive` (mode+ready)? Does switching provider/key force re-consent (key-fp binding)?
3. Fix-loop → push branch `milestone-d2b-cloud-reasoner-ui` → PR → CI green (all platforms + `cargo-audit`) → Peter merge.
4. GBrain: write the 2b handoff, re-point `air/session-start-protocol` (fresh fetch), append any lessons, re-run the Step 0 audit.

---

## Deferred (explicitly out of scope for 2b)

- Surface `EvolveReport.tainted_recall_snippets` through `EvolveReportDto` (Rust + TS) and show a live "N file-derived snippets sent" count in/near the banner (spec §198-199; the hook is wired write-only at `evolve.rs:65` / `log.rs:5913`). Blunt consent copy already discloses the worst case.
- Info-1 per-read consent signature re-verify on the readiness path.
- `extract_openai_result` `tool_calls` fallback.
- Gemini provider (later fast-follow behind the same seam).

---

## Self-Review (against spec §4 + §8 carryovers)

- **§4 Local|Cloud selector (default Local):** Task 6 (`ReasonerConfigPanel`, selector initialized from `cfg.mode`). ✅
- **§4 Cloud sub-panel (provider dropdown / model / base_url / key via vaultSet + vaultHas "saved ✓"):** Task 6. ✅
- **§4 explicit consent gate → sets consent (R1/R5 via `engine_enable_cloud_reasoner`):** Task 5 + Task 6 wiring. ✅
- **§4 persistent banner while active:** Task 4 (`CloudEgressBanner`, gated on `cloudActive`). ✅
- **§4 copy changes (`:109`, `:159`) → mode-aware:** Task 7 (`searchBlurb`/`modeBlurb`). ✅
- **§4 `ollamaReady` gate → `reasonerReady`:** Task 7 (`cfg.ready`). ✅
- **§4 pure logic extracted + vitest-tested; panel = DI tests:** Tasks 3–6. ✅ (DI instead of `mockIPC` — matches repo convention and spec line 98's "pure logic" intent.)
- **R1 signed consent only via enable:** consent modal calls `enableCloudReasoner`, never `setReasonerConfig`. ✅
- **R4 honest consent copy:** `consentBody` includes "passwords, keys, or personal data" + "leaves this device". ✅
- **R4 silent-bad-key:** caught by the enable-time test-key probe (backend) surfaced as the modal error (Task 5). ✅
- **2a carryover — cell→log restart-reseed:** Task 1. ✅
- **2a carryover — snake_case config keys:** `buildConfigInput` + the Task 2 binding test. ✅
- **R8 default-local egresses nothing:** unchanged — default `cfg.mode` is local; cloud only via the consent gate. ✅

**Placeholder scan:** none — every step has runnable code/commands. **Type consistency:** `ReasonerConfigInput`/`ReasonerConfigDto`/`CloudProvider`/`ReasonerMode` are defined once in Task 2 and used unchanged in Tasks 3–7; `vaultKeyFor` returns `ProviderVaultKey`; the config payload keys are snake_case everywhere.
