// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { LanguagePackCard } from "./LanguagePackCard";
import * as engine from "../api/engine";
import type { ModelStatusDto } from "../api/engine";

vi.mock("../api/engine");

/** Build a status payload with the Ok defaults, overriding only the fields a case cares about. */
function status(overrides: Partial<ModelStatusDto> = {}): ModelStatusDto {
  return {
    state: "ok",
    expected: null,
    loaded: null,
    reason: null,
    reindex_done: null,
    reindex_total: null,
    ...overrides,
  };
}

describe("LanguagePackCard", () => {
  beforeEach(() => vi.resetAllMocks());

  it("shows the Enable action when no pack is installed (state ok, no re-index)", async () => {
    vi.mocked(engine.modelStatus).mockResolvedValue(status());
    render(<LanguagePackCard installed={false} />);
    expect(await screen.findByRole("button", { name: /enable multilingual/i })).toBeInTheDocument();
  });

  it("shows re-index progress while migrating", async () => {
    vi.mocked(engine.modelStatus).mockResolvedValue(status({ reindex_done: 220, reindex_total: 1043 }));
    render(<LanguagePackCard installed={true} />);
    expect(await screen.findByText(/220\s*\/\s*1,?043/)).toBeInTheDocument();
    // Copy must reassure the user English search keeps working during the re-index. Assert the
    // reindex-unique tail (the card description reuses "English search stays available").
    expect(screen.getByText(/multilingual turns on when this finishes/i)).toBeInTheDocument();
  });

  it("reports multilingual active when installed, ok, and not re-indexing", async () => {
    vi.mocked(engine.modelStatus).mockResolvedValue(status());
    render(<LanguagePackCard installed={true} />);
    expect(await screen.findByText(/multilingual active/i)).toBeInTheDocument();
  });

  it("shows a loud re-download prompt when the model is missing (I3)", async () => {
    vi.mocked(engine.modelStatus).mockResolvedValue(
      status({ state: "missing", expected: "minishlab/potion-multilingual-128M" }),
    );
    render(<LanguagePackCard installed={true} />);
    expect(await screen.findByText(/missing/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /re-download/i })).toBeInTheDocument();
  });

  it("shows an integrity re-download prompt on a sha mismatch", async () => {
    vi.mocked(engine.modelStatus).mockResolvedValue(
      status({ state: "mismatch", expected: "minishlab/potion-multilingual-128M", loaded: "potion-base-8M" }),
    );
    render(<LanguagePackCard installed={true} />);
    expect(await screen.findByText(/integrity/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /re-download/i })).toBeInTheDocument();
  });

  it("surfaces the reason and a retry when a migration failed", async () => {
    vi.mocked(engine.modelStatus).mockResolvedValue(
      status({ state: "failed", reason: "re-embed migration errored" }),
    );
    render(<LanguagePackCard installed={true} />);
    expect(await screen.findByText(/re-embed migration errored/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /retry/i })).toBeInTheDocument();
  });

  it("calls downloadLanguagePack when Enable is clicked", async () => {
    vi.mocked(engine.modelStatus).mockResolvedValue(status());
    vi.mocked(engine.downloadLanguagePack).mockResolvedValue();
    render(<LanguagePackCard installed={false} />);
    fireEvent.click(await screen.findByRole("button", { name: /enable multilingual/i }));
    await waitFor(() => expect(engine.downloadLanguagePack).toHaveBeenCalledOnce());
  });

  it("fails soft when the status poll throws (daemon restarting)", async () => {
    vi.mocked(engine.modelStatus).mockRejectedValue(new Error("transport down"));
    render(<LanguagePackCard installed={false} />);
    // The panel still renders its Enable affordance instead of crashing.
    expect(await screen.findByRole("button", { name: /enable multilingual/i })).toBeInTheDocument();
  });
});
