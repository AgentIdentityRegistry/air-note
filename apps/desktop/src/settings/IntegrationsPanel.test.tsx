// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { IntegrationsPanel } from "./IntegrationsPanel";
import * as api from "../api/integrations";

vi.mock("../api/integrations", async (importOriginal) => {
  // Keep the real `isConnected` type guard (the panel discriminates state with it); mock only the
  // command wrappers so no Tauri `invoke` is touched.
  const actual = await importOriginal<typeof import("../api/integrations")>();
  return {
    isConnected: actual.isConnected,
    integrationsStatus: vi.fn(),
    connectClaudeCode: vi.fn(),
    disconnectClaudeCode: vi.fn(),
    backfillCount: vi.fn(),
    setCaptureEnabled: vi.fn(),
    captureEnabled: vi.fn(),
  };
});

describe("IntegrationsPanel", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    // Secondary fetches degrade quietly — give them safe defaults so tests that don't care never
    // trip the fail-closed paths. Individual tests override as needed.
    vi.mocked(api.backfillCount).mockResolvedValue(0);
    vi.mocked(api.captureEnabled).mockResolvedValue(false);
  });

  it("shows Connect when detected but not connected", async () => {
    vi.mocked(api.integrationsStatus).mockResolvedValue({ claude_code: "not_connected" });
    render(<IntegrationsPanel />);
    expect(await screen.findByRole("button", { name: /connect claude code/i })).toBeEnabled();
  });

  it("connect flow shows a pre-checked consent checkbox, the plaintext disclosure, and the N count", async () => {
    vi.mocked(api.integrationsStatus).mockResolvedValue({ claude_code: "not_connected" });
    vi.mocked(api.backfillCount).mockResolvedValue(42);
    render(<IntegrationsPanel />);

    const consent = await screen.findByRole("checkbox", { name: /keep a local copy of my claude code sessions/i });
    expect(consent).toBeChecked();
    // Both plaintext-honesty clauses (spec §6a) must be present, verbatim.
    expect(screen.getByText(/processed on this Mac/i)).toBeInTheDocument();
    expect(screen.getByText(/stored unencrypted on this Mac/i)).toBeInTheDocument();
    // The backfill count is disclosed in the "~30 days, N found" copy.
    expect(await screen.findByText(/~30 days, 42 found/i)).toBeInTheDocument();
  });

  it("unchecking consent still connects, but passes captureConsent=false", async () => {
    vi.mocked(api.integrationsStatus).mockResolvedValue({ claude_code: "not_connected" });
    vi.mocked(api.connectClaudeCode).mockResolvedValue({ claude_code: { connected: { capture: true } } });
    render(<IntegrationsPanel />);

    const consent = await screen.findByRole("checkbox", { name: /keep a local copy of my claude code sessions/i });
    fireEvent.click(consent); // uncheck the pre-checked box
    expect(consent).not.toBeChecked();
    fireEvent.click(screen.getByRole("button", { name: /connect claude code/i }));
    await waitFor(() => expect(api.connectClaudeCode).toHaveBeenCalledWith(false));
  });

  it("shows Disconnect and the capture toggle when fully connected", async () => {
    vi.mocked(api.integrationsStatus).mockResolvedValue({ claude_code: { connected: { capture: true } } });
    render(<IntegrationsPanel />);
    expect(await screen.findByRole("button", { name: /disconnect/i })).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: /capture new claude code sessions/i })).toBeInTheDocument();
  });

  it("the toggle reflects the ENGINE flag, not the hook bool (the real divergent state)", async () => {
    // connect() ALWAYS writes the SessionEnd hook regardless of consent, so declining capture yields
    // Connected{capture:true} (hook present) while the engine flag stays false. The toggle MUST render
    // UNCHECKED here — reading the hook bool instead would be a false consent signal. captureEnabled()
    // defaults to false (beforeEach), diverging from status.connected.capture=true.
    vi.mocked(api.integrationsStatus).mockResolvedValue({ claude_code: { connected: { capture: true } } });
    render(<IntegrationsPanel />);
    const toggle = await screen.findByRole("checkbox", { name: /capture new claude code sessions/i });
    expect(toggle).not.toBeChecked();
  });

  it("the capture toggle reflects the engine flag, is labeled forward-only, and toggling calls setCaptureEnabled", async () => {
    vi.mocked(api.integrationsStatus).mockResolvedValue({ claude_code: { connected: { capture: true } } });
    vi.mocked(api.captureEnabled).mockResolvedValue(true); // engine flag ON → toggle checked
    vi.mocked(api.setCaptureEnabled).mockResolvedValue({ claude_code: { connected: { capture: true } } });
    render(<IntegrationsPanel />);

    const toggle = await screen.findByRole("checkbox", { name: /capture new claude code sessions/i });
    expect(toggle).toBeChecked();
    expect(screen.getByText(/applies going forward — history import is only offered at Connect/i)).toBeInTheDocument();

    fireEvent.click(toggle); // turn capture OFF
    await waitFor(() => expect(api.setCaptureEnabled).toHaveBeenCalledWith(false));
  });

  it("Connected with capture:false shows the Re-connect prompt (SP2-era install)", async () => {
    vi.mocked(api.integrationsStatus).mockResolvedValue({ claude_code: { connected: { capture: false } } });
    render(<IntegrationsPanel />);
    expect(await screen.findByText(/Re-connect to enable session capture/i)).toBeInTheDocument();
    // The Connect flow is offered again (re-connecting wires the hook), plus Disconnect.
    expect(screen.getByRole("checkbox", { name: /keep a local copy of my claude code sessions/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /disconnect/i })).toBeInTheDocument();
  });

  it("disables the action and hints when Claude Code is not found", async () => {
    vi.mocked(api.integrationsStatus).mockResolvedValue({ claude_code: "not_found" });
    render(<IntegrationsPanel />);
    expect(await screen.findByText(/install claude code/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /connect claude code/i })).toBeDisabled();
  });

  it("clicking Connect calls the command with consent and refreshes to Connected", async () => {
    vi.mocked(api.integrationsStatus).mockResolvedValue({ claude_code: "not_connected" });
    vi.mocked(api.connectClaudeCode).mockResolvedValue({ claude_code: { connected: { capture: true } } });
    render(<IntegrationsPanel />);
    fireEvent.click(await screen.findByRole("button", { name: /connect claude code/i }));
    await waitFor(() => expect(api.connectClaudeCode).toHaveBeenCalledWith(true));
    expect(await screen.findByRole("button", { name: /disconnect/i })).toBeInTheDocument();
  });

  it("a secondary-fetch failure never blanks the panel (backfill + capture degrade quietly)", async () => {
    vi.mocked(api.integrationsStatus).mockResolvedValue({ claude_code: "not_connected" });
    vi.mocked(api.backfillCount).mockRejectedValue("boom");
    vi.mocked(api.captureEnabled).mockRejectedValue("boom");
    render(<IntegrationsPanel />);
    // Still renders the connect flow; the disclosure just shows "0 found".
    expect(await screen.findByText(/~30 days, 0 found/i)).toBeInTheDocument();
  });
});
