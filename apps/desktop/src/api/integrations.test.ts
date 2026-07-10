import { describe, it, expect, vi, beforeEach } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { integrationsStatus, connectClaudeCode, disconnectClaudeCode } from "./integrations";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

describe("api/integrations", () => {
  beforeEach(() => vi.resetAllMocks());

  it("integrationsStatus invokes the status command", async () => {
    vi.mocked(invoke).mockResolvedValue({ claude_code: "not_connected" });
    expect(await integrationsStatus()).toEqual({ claude_code: "not_connected" });
    expect(invoke).toHaveBeenCalledWith("integrations_status");
  });

  it("connect/disconnect invoke their commands", async () => {
    vi.mocked(invoke).mockResolvedValue({ claude_code: "connected" });
    await connectClaudeCode();
    expect(invoke).toHaveBeenCalledWith("integrations_connect_claude_code");
    vi.mocked(invoke).mockResolvedValue({ claude_code: "not_connected" });
    await disconnectClaudeCode();
    expect(invoke).toHaveBeenCalledWith("integrations_disconnect_claude_code");
  });
});
