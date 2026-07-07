// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { LanguagePackCard } from "./LanguagePackCard";
import * as engine from "../api/engine";
import { MULTILINGUAL_MODEL_ID, MULTILINGUAL_SAFETENSORS_SHA, type ModelStatusDto } from "../api/engine";

vi.mock("../api/engine", async (importOriginal) => {
  // Keep the real string constants (the card compares active_model_id against them); mock only the IPC.
  const actual = await importOriginal<typeof import("../api/engine")>();
  return { ...actual, downloadLanguagePack: vi.fn(), setActiveModel: vi.fn(), modelStatus: vi.fn() };
});

/** Build a status payload with the Ok defaults, overriding only the fields a case cares about. */
function status(overrides: Partial<ModelStatusDto> = {}): ModelStatusDto {
  return {
    state: "ok",
    expected: null,
    loaded: null,
    reason: null,
    reindex_done: null,
    reindex_total: null,
    active_model_id: "minishlab/potion-base-8M",
    ...overrides,
  };
}

describe("LanguagePackCard", () => {
  beforeEach(() => vi.resetAllMocks());

  it("shows the Enable action when no pack is installed (state ok, no re-index)", async () => {
    vi.mocked(engine.modelStatus).mockResolvedValue(status());
    render(<LanguagePackCard />);
    expect(await screen.findByRole("button", { name: /enable multilingual/i })).toBeInTheDocument();
  });

  it("shows re-index progress while migrating", async () => {
    vi.mocked(engine.modelStatus).mockResolvedValue(status({ reindex_done: 220, reindex_total: 1043 }));
    render(<LanguagePackCard />);
    expect(await screen.findByText(/220\s*\/\s*1,?043/)).toBeInTheDocument();
    // Copy must reassure the user English search keeps working during the re-index. Assert the
    // reindex-unique tail (the card description reuses "English search stays available").
    expect(screen.getByText(/multilingual turns on when this finishes/i)).toBeInTheDocument();
  });

  it("reports multilingual active and hides Enable when the served model is the pack (ok, idle)", async () => {
    vi.mocked(engine.modelStatus).mockResolvedValue(status({ active_model_id: MULTILINGUAL_MODEL_ID }));
    render(<LanguagePackCard />);
    expect(await screen.findByText(/multilingual active/i)).toBeInTheDocument();
    // The redundant ~500 MB Enable button must be gone once the pack is live (review MAJOR).
    expect(screen.queryByRole("button", { name: /enable multilingual/i })).not.toBeInTheDocument();
  });

  it("keeps offering Enable while the served model is still the English base (ok, idle)", async () => {
    vi.mocked(engine.modelStatus).mockResolvedValue(status());
    render(<LanguagePackCard />);
    expect(await screen.findByRole("button", { name: /enable multilingual/i })).toBeInTheDocument();
    expect(screen.queryByText(/multilingual active/i)).not.toBeInTheDocument();
  });

  it("shows a loud re-download prompt when the model is missing (I3)", async () => {
    vi.mocked(engine.modelStatus).mockResolvedValue(
      status({ state: "missing", expected: "minishlab/potion-multilingual-128M" }),
    );
    render(<LanguagePackCard />);
    expect(await screen.findByText(/missing/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /re-download/i })).toBeInTheDocument();
  });

  it("shows an integrity re-download prompt on a sha mismatch", async () => {
    vi.mocked(engine.modelStatus).mockResolvedValue(
      status({ state: "mismatch", expected: "minishlab/potion-multilingual-128M", loaded: "potion-base-8M" }),
    );
    render(<LanguagePackCard />);
    expect(await screen.findByText(/integrity/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /re-download/i })).toBeInTheDocument();
  });

  it("surfaces the reason and a retry when a migration failed", async () => {
    vi.mocked(engine.modelStatus).mockResolvedValue(
      status({ state: "failed", reason: "re-embed migration errored" }),
    );
    render(<LanguagePackCard />);
    expect(await screen.findByText(/re-embed migration errored/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /retry/i })).toBeInTheDocument();
  });

  it("re-enables via setActiveModel (not a re-download) when a failed migration is retried", async () => {
    // On a migration failure the model FILES are intact — only the re-embed errored — so retry must
    // re-enable them, never re-download ~500 MB (review MINOR).
    vi.mocked(engine.modelStatus).mockResolvedValue(status({ state: "failed", reason: "boom" }));
    vi.mocked(engine.setActiveModel).mockResolvedValue();
    render(<LanguagePackCard />);
    fireEvent.click(await screen.findByRole("button", { name: /retry/i }));
    await waitFor(() =>
      expect(engine.setActiveModel).toHaveBeenCalledWith(MULTILINGUAL_MODEL_ID, MULTILINGUAL_SAFETENSORS_SHA),
    );
    expect(engine.downloadLanguagePack).not.toHaveBeenCalled();
  });

  it("calls downloadLanguagePack when Enable is clicked", async () => {
    vi.mocked(engine.modelStatus).mockResolvedValue(status());
    vi.mocked(engine.downloadLanguagePack).mockResolvedValue();
    render(<LanguagePackCard />);
    fireEvent.click(await screen.findByRole("button", { name: /enable multilingual/i }));
    await waitFor(() => expect(engine.downloadLanguagePack).toHaveBeenCalledOnce());
  });

  it("fails soft when the status poll throws (daemon restarting)", async () => {
    vi.mocked(engine.modelStatus).mockRejectedValue(new Error("transport down"));
    render(<LanguagePackCard />);
    // The panel still renders its Enable affordance instead of crashing.
    expect(await screen.findByRole("button", { name: /enable multilingual/i })).toBeInTheDocument();
  });
});
