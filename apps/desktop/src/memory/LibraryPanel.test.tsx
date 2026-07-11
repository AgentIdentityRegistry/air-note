// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { LibraryPanel } from "./LibraryPanel";
import { listSessions, listNotes, recall } from "../api/engine";
import type { SessionSummaryDto, NoteDto, HitDto } from "../api/engine";

// LibraryPanel drives the engine through the api module directly (the MemoryPanel/LanguagePackCard
// convention); mock only the three IPC wrappers it calls and keep everything else real.
vi.mock("../api/engine", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/engine")>();
  return { ...actual, listSessions: vi.fn(), listNotes: vi.fn(), recall: vi.fn() };
});

// Two sessions, deliberately given oldest-first so the newest-first sort is actually exercised.
const S_OLD: SessionSummaryDto = {
  session_id: "s-old", title: "Refactor auth", project: "air-note", tool: "claude-code",
  started_at: 1_700_000_000, ended_at: 1_700_003_600, approx_bytes: 1024,
};
const S_NEW: SessionSummaryDto = {
  session_id: "s-new", title: "Design memory hub", project: "air-note", tool: "claude-code",
  started_at: 1_800_000_000, ended_at: 1_800_003_600, approx_bytes: 2048,
};
const NOTE: NoteDto = {
  event_id: "n1", text: "Prefer tokens over hardcoded colors", created_at: 1_750_000_000, superseded_by: null,
};

/** Format an epoch-second timestamp the same way the panel does — locale-agnostic within a run. */
const day = (epochSeconds: number) => new Date(epochSeconds * 1000).toLocaleDateString();

function primeArchive(sessions: SessionSummaryDto[], notes: NoteDto[]) {
  vi.mocked(listSessions).mockResolvedValue(sessions);
  vi.mocked(listNotes).mockResolvedValue(notes);
  vi.mocked(recall).mockResolvedValue([]);
}

describe("LibraryPanel", () => {
  beforeEach(() => vi.clearAllMocks());

  it("renders captured sessions (title/project/date) and notes, newest first", async () => {
    primeArchive([S_OLD, S_NEW], [NOTE]);
    render(<LibraryPanel />);

    // Titles + note text present.
    await screen.findByText("Design memory hub");
    expect(screen.getByText("Refactor auth")).toBeInTheDocument();
    expect(screen.getByText("Prefer tokens over hardcoded colors")).toBeInTheDocument();
    // Project + human date rendered for the newest session.
    expect(screen.getAllByText("air-note").length).toBeGreaterThan(0);
    expect(screen.getByText(day(S_NEW.started_at))).toBeInTheDocument();

    // Newest session (larger started_at) is ordered before the older one.
    const titles = screen.getAllByText(/Design memory hub|Refactor auth/);
    expect(titles[0]).toHaveTextContent("Design memory hub");
    expect(titles[1]).toHaveTextContent("Refactor auth");
  });

  it("the search box filters sessions and notes client-side (case-insensitive)", async () => {
    primeArchive([S_OLD, S_NEW], [NOTE]);
    render(<LibraryPanel />);
    const input = await screen.findByLabelText("Filter your library");

    // A title match (lowercase query, mixed-case title) keeps only that session; hides the other + the note.
    fireEvent.change(input, { target: { value: "design" } });
    expect(screen.getByText("Design memory hub")).toBeInTheDocument();
    expect(screen.queryByText("Refactor auth")).toBeNull();
    expect(screen.queryByText("Prefer tokens over hardcoded colors")).toBeNull();

    // A note-text match keeps the note and hides both sessions — proving the filter spans both lists.
    fireEvent.change(input, { target: { value: "TOKENS" } });
    expect(screen.getByText("Prefer tokens over hardcoded colors")).toBeInTheDocument();
    expect(screen.queryByText("Design memory hub")).toBeNull();
    expect(screen.queryByText("Refactor auth")).toBeNull();
  });

  it("the 'search memory' action runs recall and shows hits in a Memory group", async () => {
    primeArchive([S_OLD, S_NEW], [NOTE]);
    const hit: HitDto = {
      event_id: "h1", score: 0.87, kind: "memory", sources: ["vector"],
      text: "Recalled: the daemon excludes superseded notes",
    };
    vi.mocked(recall).mockResolvedValue([hit]);
    render(<LibraryPanel />);
    const input = await screen.findByLabelText("Filter your library");

    // "daemon" matches no loaded session/note, so any match came from recall, not the client filter.
    fireEvent.change(input, { target: { value: "daemon" } });
    fireEvent.click(screen.getByRole("button", { name: "Search memory" }));

    await screen.findByText("Recalled: the daemon excludes superseded notes");
    expect(recall).toHaveBeenCalledWith("daemon", 10);
    // Hits live in a distinct "Memory" group, separate from the Sessions/Notes lists.
    expect(screen.getByRole("heading", { name: "Memory" })).toBeInTheDocument();
  });

  it("empty archive shows a neutral empty state", async () => {
    primeArchive([], []);
    render(<LibraryPanel />);
    expect(await screen.findByText(/captured sessions and notes will appear here/i)).toBeInTheDocument();
  });

  it("a load error surfaces (not a blank panel)", async () => {
    // Tauri rejects Result<_,String> with a BARE STRING; the panel must catch it as a string.
    vi.mocked(listSessions).mockRejectedValue("boom: cannot reach daemon");
    vi.mocked(listNotes).mockResolvedValue([]);
    vi.mocked(recall).mockResolvedValue([]);
    render(<LibraryPanel />);

    expect(await screen.findByText(/boom: cannot reach daemon/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Try again" })).toBeInTheDocument();
  });
});
