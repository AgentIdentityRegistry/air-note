import { describe, it, expect, vi, beforeEach } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invoke(...a) }));

import { downloadLanguagePack, setActiveModel, modelStatus } from "./engine";

describe("language-pack bindings", () => {
  beforeEach(() => invoke.mockReset());

  it("downloadLanguagePack invokes the download command with no args", async () => {
    invoke.mockResolvedValue(undefined);
    await downloadLanguagePack();
    expect(invoke).toHaveBeenCalledWith("engine_download_language_pack");
  });

  it("setActiveModel passes camelCase args the Tauri layer maps to snake_case params", async () => {
    invoke.mockResolvedValue(undefined);
    await setActiveModel("minishlab/potion-multilingual-128M", "deadbeef");
    expect(invoke).toHaveBeenCalledWith("engine_set_active_model", {
      modelId: "minishlab/potion-multilingual-128M",
      safetensorsSha: "deadbeef",
    });
  });

  it("modelStatus returns the payload-encoded status verbatim", async () => {
    const dto = {
      state: "failed",
      expected: null,
      loaded: null,
      reason: "re-embed migration errored",
      reindex_done: null,
      reindex_total: null,
    };
    invoke.mockResolvedValue(dto);
    await expect(modelStatus()).resolves.toEqual(dto);
    expect(invoke).toHaveBeenCalledWith("engine_model_status");
  });
});
