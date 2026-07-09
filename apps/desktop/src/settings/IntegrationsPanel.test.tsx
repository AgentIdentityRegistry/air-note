// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { IntegrationsPanel } from "./IntegrationsPanel";
import * as api from "../api/integrations";

vi.mock("../api/integrations", () => ({
  integrationsStatus: vi.fn(),
  connectClaudeCode: vi.fn(),
  disconnectClaudeCode: vi.fn(),
}));

describe("IntegrationsPanel", () => {
  beforeEach(() => vi.resetAllMocks());

  it("shows Connect when detected but not connected", async () => {
    vi.mocked(api.integrationsStatus).mockResolvedValue({ claude_code: "not_connected" });
    render(<IntegrationsPanel />);
    expect(await screen.findByRole("button", { name: /connect claude code/i })).toBeEnabled();
  });

  it("shows Disconnect when connected", async () => {
    vi.mocked(api.integrationsStatus).mockResolvedValue({ claude_code: "connected" });
    render(<IntegrationsPanel />);
    expect(await screen.findByRole("button", { name: /disconnect/i })).toBeInTheDocument();
  });

  it("disables the action and hints when Claude Code is not found", async () => {
    vi.mocked(api.integrationsStatus).mockResolvedValue({ claude_code: "not_found" });
    render(<IntegrationsPanel />);
    expect(await screen.findByText(/install claude code/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /connect claude code/i })).toBeDisabled();
  });

  it("clicking Connect calls the command and refreshes to Connected", async () => {
    vi.mocked(api.integrationsStatus).mockResolvedValue({ claude_code: "not_connected" });
    vi.mocked(api.connectClaudeCode).mockResolvedValue({ claude_code: "connected" });
    render(<IntegrationsPanel />);
    fireEvent.click(await screen.findByRole("button", { name: /connect claude code/i }));
    await waitFor(() => expect(api.connectClaudeCode).toHaveBeenCalledOnce());
    expect(await screen.findByRole("button", { name: /disconnect/i })).toBeInTheDocument();
  });
});
