// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import { MemoryPanel } from "./MemoryPanel";
import { recall, evolveStatus, ollamaStatus, getReasonerConfig } from "../api/engine";

// MemoryPanel drives the engine + vault through these modules; stub them so the test
// owns the (mode, ollama-ready, cloud-ready) triple the Evolve gate depends on. The child
// panels (ReasonerConfigPanel/CloudEgressBanner) render for real off pure view helpers.
vi.mock("../api/engine", () => ({
  recall: vi.fn(async () => []),
  evolveStatus: vi.fn(),
  setEvolveEnabled: vi.fn(async () => {}),
  evolveNow: vi.fn(async () => ({})),
  ollamaStatus: vi.fn(),
  getReasonerConfig: vi.fn(),
  setReasonerConfig: vi.fn(async () => {}),
  enableCloudReasoner: vi.fn(async () => {}),
}));
vi.mock("../vault", () => ({
  vaultSet: vi.fn(async () => {}),
  vaultHas: vi.fn(async () => false),
}));

type Scenario = { mode: "local" | "cloud"; ready: boolean; ollamaReady: boolean };

/** Prime the three status reads MemoryPanel polls on mount. Evolve is `enabled` in every
 *  scenario so both Evolve buttons' `disabled` reduces to `!reasonerReady` — isolating the gate. */
function primeState({ mode, ready, ollamaReady }: Scenario) {
  vi.mocked(recall).mockResolvedValue([]);
  vi.mocked(getReasonerConfig).mockResolvedValue({
    mode, provider: "anthropic", model: "", base_url: null, ready,
  });
  vi.mocked(ollamaStatus).mockResolvedValue({
    reachable: ollamaReady, model_present: ollamaReady, model_tag: "qwen2.5:7b-instruct",
  });
  vi.mocked(evolveStatus).mockResolvedValue({
    enabled: true, queue_depth: 0, last_tick_ms: null, error_count: 0, last_error: null,
  });
}

// "Turn Evolve Off" only renders once evolveStatus resolves with enabled:true, and
// refreshStatus sets status/ollama/reasonerCfg together — so awaiting it proves all three loaded.
async function evolveButtons() {
  const toggle = await screen.findByRole("button", { name: "Turn Evolve Off" });
  const evolveNow = screen.getByRole("button", { name: "Evolve Now" });
  return { toggle, evolveNow };
}

describe("MemoryPanel Evolve gate is mode-aware", () => {
  beforeEach(() => vi.clearAllMocks());

  it("enables the Evolve controls in Local mode when the local model is ready", async () => {
    // Local readiness is the Ollama probe; cfg.ready is false for Local by design, so gating
    // the buttons on cfg.ready alone leaves the DEFAULT mode's Evolve controls stuck disabled.
    primeState({ mode: "local", ready: false, ollamaReady: true });
    render(<MemoryPanel />);
    const { toggle, evolveNow } = await evolveButtons();
    expect(toggle).toBeEnabled();
    expect(evolveNow).toBeEnabled();
  });

  it("keeps the Evolve controls disabled in Local mode when the local model is not ready", async () => {
    primeState({ mode: "local", ready: false, ollamaReady: false });
    render(<MemoryPanel />);
    const { toggle, evolveNow } = await evolveButtons();
    expect(toggle).toBeDisabled();
    expect(evolveNow).toBeDisabled();
  });

  it("gates Cloud mode on cloud readiness, ignoring the Ollama probe", async () => {
    primeState({ mode: "cloud", ready: true, ollamaReady: false });
    render(<MemoryPanel />);
    const { toggle, evolveNow } = await evolveButtons();
    expect(toggle).toBeEnabled();
    expect(evolveNow).toBeEnabled();
  });

  it("keeps Cloud mode disabled when not consent-ready, even with Ollama up (no silent fallback)", async () => {
    primeState({ mode: "cloud", ready: false, ollamaReady: true });
    render(<MemoryPanel />);
    const { toggle, evolveNow } = await evolveButtons();
    expect(toggle).toBeDisabled();
    expect(evolveNow).toBeDisabled();
  });
});
