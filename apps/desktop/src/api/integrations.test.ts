import { describe, it, expect, vi, beforeEach } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import {
  integrationsStatus, connectClaudeCode, disconnectClaudeCode,
  backfillCount, setCaptureEnabled, captureEnabled, isConnected,
  setReflectEnabled, reflectEnabled,
} from "./integrations";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

describe("api/integrations", () => {
  beforeEach(() => vi.resetAllMocks());

  it("integrationsStatus invokes the status command and returns the object-form connected status", async () => {
    vi.mocked(invoke).mockResolvedValue({ claude_code: { connected: { capture: true } } });
    expect(await integrationsStatus()).toEqual({ claude_code: { connected: { capture: true } } });
    expect(invoke).toHaveBeenCalledWith("integrations_status");
  });

  it("connectClaudeCode forwards the consent as a camelCase arg (Tauri renames to snake_case)", async () => {
    vi.mocked(invoke).mockResolvedValue({ claude_code: { connected: { capture: true } } });
    await connectClaudeCode(true);
    expect(invoke).toHaveBeenCalledWith("integrations_connect_claude_code", { captureConsent: true });
  });

  it("disconnectClaudeCode invokes its command", async () => {
    vi.mocked(invoke).mockResolvedValue({ claude_code: "not_connected" });
    await disconnectClaudeCode();
    expect(invoke).toHaveBeenCalledWith("integrations_disconnect_claude_code");
  });

  it("backfillCount invokes its command and returns the number", async () => {
    vi.mocked(invoke).mockResolvedValue(42);
    expect(await backfillCount()).toBe(42);
    expect(invoke).toHaveBeenCalledWith("integrations_backfill_count");
  });

  it("setCaptureEnabled forwards the enabled flag as a camelCase arg", async () => {
    vi.mocked(invoke).mockResolvedValue({ claude_code: { connected: { capture: true } } });
    await setCaptureEnabled(false);
    expect(invoke).toHaveBeenCalledWith("integrations_set_capture_enabled", { enabled: false });
  });

  it("captureEnabled invokes its command and returns the bool", async () => {
    vi.mocked(invoke).mockResolvedValue(true);
    expect(await captureEnabled()).toBe(true);
    expect(invoke).toHaveBeenCalledWith("integrations_capture_enabled");
  });

  it("setReflectEnabled forwards the enabled flag as a camelCase arg", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);
    await setReflectEnabled(true);
    expect(invoke).toHaveBeenCalledWith("integrations_set_reflect_enabled", { enabled: true });
  });
  it("reflectEnabled invokes its command and returns the bool", async () => {
    vi.mocked(invoke).mockResolvedValue(false);
    expect(await reflectEnabled()).toBe(false);
    expect(invoke).toHaveBeenCalledWith("integrations_reflect_enabled");
  });

  it("isConnected narrows only the object-form Connected variant, never the bare strings", () => {
    expect(isConnected({ connected: { capture: false } })).toBe(true);
    expect(isConnected("not_found")).toBe(false);
    expect(isConnected("not_connected")).toBe(false);
  });
});
