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

  it("offers Gemini as a provider and hides base URL for it (pinned host)", async () => {
    render(<ReasonerConfigPanel {...deps()} />);
    fireEvent.click(screen.getByRole("button", { name: /^cloud$/i }));
    const select = await screen.findByLabelText(/provider/i);
    expect(screen.getByRole("option", { name: "Gemini" })).toBeInTheDocument();
    fireEvent.change(select, { target: { value: "gemini" } });
    expect(screen.queryByLabelText(/base url/i)).not.toBeInTheDocument();
  });
});
