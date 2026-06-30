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
