// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, within, waitFor } from "@testing-library/react";
import { LibraryPanel } from "./LibraryPanel";
import {
  listSessions, listNotes, recall, getSession, deleteSession, supersedeNote, recallStats,
} from "../api/engine";
import type { SessionSummaryDto, NoteDto, HitDto } from "../api/engine";

// LibraryPanel drives the engine through the api module directly (the MemoryPanel/LanguagePackCard
// convention); mock only the IPC wrappers it calls and keep everything else real.
vi.mock("../api/engine", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/engine")>();
  return {
    ...actual,
    listSessions: vi.fn(), listNotes: vi.fn(), recall: vi.fn(),
    getSession: vi.fn(), deleteSession: vi.fn(),
    supersedeNote: vi.fn(), recallStats: vi.fn(),
  };
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
  // No recalls yet by default → the stats strip stays hidden and out of these tests' way.
  vi.mocked(recallStats).mockResolvedValue({ total: 0, misses: 0, recent_misses: [] });
  vi.mocked(supersedeNote).mockResolvedValue("n-new");
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
    const input = await screen.findByLabelText(/Filter your library/);

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
    const input = await screen.findByLabelText(/Filter your library/);

    // "daemon" matches no loaded session/note, so any match came from recall, not the client filter.
    fireEvent.change(input, { target: { value: "daemon" } });
    fireEvent.click(screen.getByRole("button", { name: "Search memory" }));

    await screen.findByText("Recalled: the daemon excludes superseded notes");
    expect(recall).toHaveBeenCalledWith("daemon", 10);
    // Hits live in a distinct Memory group, labeled with the searched term so it can't be
    // mistaken for the live client-side filter as the box keeps re-filtering.
    expect(screen.getByRole("heading", { name: /Memory · .daemon./ })).toBeInTheDocument();
  });

  it("pressing Enter in the search box runs recall (not just the button)", async () => {
    primeArchive([S_OLD, S_NEW], [NOTE]);
    const hit: HitDto = {
      event_id: "h2", score: 0.5, kind: "memory", sources: ["keyword"], text: "Recalled via Enter",
    };
    vi.mocked(recall).mockResolvedValue([hit]);
    render(<LibraryPanel />);
    const input = await screen.findByLabelText(/Filter your library/);

    fireEvent.change(input, { target: { value: "auth" } });
    fireEvent.keyDown(input, { key: "Enter" });

    await screen.findByText("Recalled via Enter");
    expect(recall).toHaveBeenCalledWith("auth", 10);
  });

  it("a search that finds nothing shows the 'Nothing found in memory' notice", async () => {
    primeArchive([S_OLD, S_NEW], [NOTE]);
    vi.mocked(recall).mockResolvedValue([]);
    render(<LibraryPanel />);
    const input = await screen.findByLabelText(/Filter your library/);

    fireEvent.change(input, { target: { value: "nonexistent-term" } });
    fireEvent.click(screen.getByRole("button", { name: "Search memory" }));

    expect(await screen.findByText(/Nothing found in memory/i)).toBeInTheDocument();
  });

  it("a recall failure surfaces the error (caught as a bare string, not e.message)", async () => {
    primeArchive([S_OLD, S_NEW], [NOTE]);
    vi.mocked(recall).mockRejectedValue("recall exploded");
    render(<LibraryPanel />);
    const input = await screen.findByLabelText(/Filter your library/);

    fireEvent.change(input, { target: { value: "boom" } });
    fireEvent.click(screen.getByRole("button", { name: "Search memory" }));

    expect(await screen.findByText(/recall exploded/)).toBeInTheDocument();
  });

  it("sessions-but-no-notes shows a neutral 'No notes yet.', not empty-quote filter copy", async () => {
    // The landing/common early state: some sessions, zero notes, no active filter. The Notes
    // section must NOT render `No notes match "".` (empty quotes) — it must read "No notes yet.".
    primeArchive([S_NEW], []);
    render(<LibraryPanel />);
    await screen.findByText("Design memory hub");

    expect(screen.getByText("No notes yet.")).toBeInTheDocument();
    expect(screen.queryByText(/No notes match/)).toBeNull();
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

  // ---- C5: session reader + honest Delete flow ----

  it("clicking a session opens the reader with its rendered markdown", async () => {
    primeArchive([S_NEW], []);
    vi.mocked(getSession).mockResolvedValue({ summary: S_NEW, markdown: "## You\nfix the parser" });
    render(<LibraryPanel />);
    await screen.findByText("Design memory hub");

    fireEvent.click(screen.getByRole("button", { name: "View" }));

    // The reader shows the session's markdown as text; getSession was called for that row's id.
    expect(await screen.findByText(/fix the parser/)).toBeInTheDocument();
    expect(getSession).toHaveBeenCalledWith("s-new");
  });

  it("renders session markdown as literal text — HTML/script content never becomes markup", async () => {
    // Locks in the #1 security guarantee: captured content is external-tainted and must render as
    // TEXT (a <pre>), never via dangerouslySetInnerHTML / an HTML renderer. If someone swaps the <pre>
    // for a markdown-to-HTML renderer, this fails.
    const evil = '<img src=x onerror=alert(1)> <b>bold</b>';
    primeArchive([S_NEW], []);
    vi.mocked(getSession).mockResolvedValue({ summary: S_NEW, markdown: evil });
    const { container } = render(<LibraryPanel />);
    await screen.findByText("Design memory hub");

    fireEvent.click(screen.getByRole("button", { name: "View" }));

    // The raw tag string is present in the DOM as visible text…
    expect(await screen.findByText(/<img src=x onerror=alert\(1\)> <b>bold<\/b>/)).toBeInTheDocument();
    // …and NOT parsed into real elements.
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector("b")).toBeNull();
  });

  it("Delete asks for confirmation quoting the honesty note, then removes the row on confirm", async () => {
    primeArchive([S_OLD, S_NEW], []);
    vi.mocked(deleteSession).mockResolvedValue(undefined);
    render(<LibraryPanel />);
    await screen.findByText("Design memory hub");

    // Two rows → scope to the newest session's row so we delete the right one.
    const row = screen.getByText("Design memory hub").closest("li") as HTMLElement;
    fireEvent.click(within(row).getByRole("button", { name: "Delete" }));

    // The confirm dialog quotes §7b honestly: the body is destroyed, the title stays in the chain.
    expect(await screen.findByText(/permanently deleted/i)).toBeInTheDocument();
    expect(screen.getByText(/title remains in the encrypted log/i)).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Delete session" }));

    await waitFor(() => expect(deleteSession).toHaveBeenCalledWith("s-new"));
    await waitFor(() => expect(screen.queryByText("Design memory hub")).toBeNull());
    // The other session is untouched.
    expect(screen.getByText("Refactor auth")).toBeInTheDocument();
  });

  it("Delete can be cancelled (no deleteSession call)", async () => {
    primeArchive([S_NEW], []);
    render(<LibraryPanel />);
    await screen.findByText("Design memory hub");

    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    await screen.findByText(/title remains in the encrypted log/i);
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));

    expect(deleteSession).not.toHaveBeenCalled();
    expect(screen.getByText("Design memory hub")).toBeInTheDocument();
    // The confirm dialog is dismissed.
    expect(screen.queryByText(/title remains in the encrypted log/i)).toBeNull();
  });

  it("deleting an already-deleted session shows 'already deleted', not a crash", async () => {
    // Race: someone deleted it elsewhere. Tauri rejects with the daemon's bare string.
    primeArchive([S_NEW], []);
    vi.mocked(deleteSession).mockRejectedValue("session not found or deleted");
    render(<LibraryPanel />);
    await screen.findByText("Design memory hub");

    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    fireEvent.click(await screen.findByRole("button", { name: "Delete session" }));

    // A friendly notice (not a raw error), and the list is refreshed (listSessions re-called).
    expect(await screen.findByText(/already deleted/i)).toBeInTheDocument();
    await waitFor(() => expect(vi.mocked(listSessions).mock.calls.length).toBeGreaterThan(1));
  });

  it("deleting the currently-open session closes the reader", async () => {
    primeArchive([S_NEW], []);
    vi.mocked(getSession).mockResolvedValue({ summary: S_NEW, markdown: "body text here" });
    vi.mocked(deleteSession).mockResolvedValue(undefined);
    render(<LibraryPanel />);
    await screen.findByText("Design memory hub");

    fireEvent.click(screen.getByRole("button", { name: "View" }));
    await screen.findByText(/body text here/);

    // Delete from inside the reader (only one dialog is open at this point).
    const reader = screen.getByRole("dialog");
    fireEvent.click(within(reader).getByRole("button", { name: "Delete" }));
    fireEvent.click(await screen.findByRole("button", { name: "Delete session" }));

    await waitFor(() => expect(screen.queryByText(/body text here/)).toBeNull());
    expect(deleteSession).toHaveBeenCalledWith("s-new");
  });

  it("opening a session that was already deleted shows 'already deleted' in the reader, not a crash", async () => {
    // The other race arm: getSession itself rejects with the daemon's bare not-found string.
    primeArchive([S_NEW], []);
    vi.mocked(getSession).mockRejectedValue("session not found or deleted");
    render(<LibraryPanel />);
    await screen.findByText("Design memory hub");

    fireEvent.click(screen.getByRole("button", { name: "View" }));

    expect(await screen.findByText(/already deleted/i)).toBeInTheDocument();
    await waitFor(() => expect(vi.mocked(listSessions).mock.calls.length).toBeGreaterThan(1));
  });

  // ---- C6: note Supersede (edit-in-place) + recall-miss stats strip ----

  it("Supersede opens an edit-in-place prefilled with the note, and Save writes the corrected text", async () => {
    primeArchive([], [NOTE]);
    render(<LibraryPanel />);
    await screen.findByText("Prefer tokens over hardcoded colors");

    // Turn the row into an editor prefilled with the note's current text.
    fireEvent.click(screen.getByRole("button", { name: "Supersede" }));
    const textarea = screen.getByRole("textbox", { name: /edit note/i }) as HTMLTextAreaElement;
    expect(textarea.value).toBe("Prefer tokens over hardcoded colors");

    // The daemon returns the new current note on re-list (old excluded, replacement present).
    const CORRECTED: NoteDto = {
      event_id: "n2", text: "Prefer design tokens; never hardcode colors",
      created_at: 1_750_000_500, superseded_by: null,
    };
    vi.mocked(listNotes).mockResolvedValue([CORRECTED]);

    fireEvent.change(textarea, { target: { value: "Prefer design tokens; never hardcode colors" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() =>
      expect(supersedeNote).toHaveBeenCalledWith("n1", "Prefer design tokens; never hardcode colors"),
    );
    // Refresh reflects the replacement; the old text is gone.
    expect(await screen.findByText("Prefer design tokens; never hardcode colors")).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByText("Prefer tokens over hardcoded colors")).toBeNull());
  });

  it("Supersede can be cancelled (no supersedeNote call, note unchanged)", async () => {
    primeArchive([], [NOTE]);
    render(<LibraryPanel />);
    await screen.findByText("Prefer tokens over hardcoded colors");

    fireEvent.click(screen.getByRole("button", { name: "Supersede" }));
    const textarea = screen.getByRole("textbox", { name: /edit note/i });
    fireEvent.change(textarea, { target: { value: "something else entirely" } });
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));

    expect(supersedeNote).not.toHaveBeenCalled();
    // Back to view mode with the original text; the editor is gone.
    expect(screen.getByText("Prefer tokens over hardcoded colors")).toBeInTheDocument();
    expect(screen.queryByRole("textbox", { name: /edit note/i })).toBeNull();
  });

  it("Save is disabled/blocked for empty or unchanged text", async () => {
    primeArchive([], [NOTE]);
    render(<LibraryPanel />);
    await screen.findByText("Prefer tokens over hardcoded colors");

    fireEvent.click(screen.getByRole("button", { name: "Supersede" }));
    const textarea = screen.getByRole("textbox", { name: /edit note/i });
    const save = () => screen.getByRole("button", { name: "Save" });

    // Prefilled = unchanged → Save disabled.
    expect(save()).toBeDisabled();

    // Whitespace-only (blank after trim) → still disabled; clicking does nothing.
    fireEvent.change(textarea, { target: { value: "   " } });
    expect(save()).toBeDisabled();
    fireEvent.click(save());
    expect(supersedeNote).not.toHaveBeenCalled();

    // A genuine change enables it.
    fireEvent.change(textarea, { target: { value: "A genuinely different note" } });
    expect(save()).toBeEnabled();
  });

  it("the stats strip shows totals and recent miss queries", async () => {
    primeArchive([S_NEW], [NOTE]);
    vi.mocked(recallStats).mockResolvedValue({
      total: 10, misses: 3,
      recent_misses: [
        { query: "quantum tunnelling", at: 1_750_000_000 },
        { query: "where did I park", at: 1_750_100_000 },
      ],
    });
    render(<LibraryPanel />);
    await screen.findByText("Design memory hub");

    // Totals — lifetime recalls and how many found nothing.
    expect(await screen.findByText(/10 recalls/)).toBeInTheDocument();
    expect(screen.getByText(/3 found nothing/)).toBeInTheDocument();
    // The recent-miss QUERIES are listed (queries only — never result text, spec §8).
    expect(screen.getByText(/quantum tunnelling/)).toBeInTheDocument();
    expect(screen.getByText(/where did I park/)).toBeInTheDocument();
  });

  it("a supersede error surfaces (caught as a bare string), no crash", async () => {
    // Tauri rejects Result<_, String> with a BARE STRING; the row must show it, not read .message.
    primeArchive([], [NOTE]);
    vi.mocked(supersedeNote).mockRejectedValue("supersede exploded");
    render(<LibraryPanel />);
    await screen.findByText("Prefer tokens over hardcoded colors");

    fireEvent.click(screen.getByRole("button", { name: "Supersede" }));
    fireEvent.change(screen.getByRole("textbox", { name: /edit note/i }), {
      target: { value: "corrected but will fail" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    expect(await screen.findByText(/supersede exploded/)).toBeInTheDocument();
    // No crash — the editor stays open with the error inline.
    expect(screen.getByRole("textbox", { name: /edit note/i })).toBeInTheDocument();
  });

  it("a failed re-list after a successful save keeps the panel up (localized notice, not the fatal card)", async () => {
    // The supersede SAVED; only the follow-up listNotes() failed. The Library must stay fully
    // rendered with a localized non-fatal notice — NOT the full-screen "Couldn't load" error card.
    primeArchive([S_NEW], [NOTE]);
    vi.mocked(supersedeNote).mockResolvedValue("n2");
    render(<LibraryPanel />);
    await screen.findByText("Prefer tokens over hardcoded colors");

    fireEvent.click(screen.getByRole("button", { name: "Supersede" }));
    fireEvent.change(screen.getByRole("textbox", { name: /edit note/i }), {
      target: { value: "corrected text that saves fine" },
    });
    // The save resolves, but the very next listNotes() (the refresh) rejects once.
    vi.mocked(listNotes).mockRejectedValueOnce("listNotes hiccup");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => expect(supersedeNote).toHaveBeenCalled());
    // Localized, non-fatal notice — and the rest of the panel (a session) is still on screen.
    expect(await screen.findByText(/reload to see the update/i)).toBeInTheDocument();
    expect(screen.getByText("Design memory hub")).toBeInTheDocument();
    // NOT the full-screen error card.
    expect(screen.queryByRole("button", { name: "Try again" })).toBeNull();
    // The raw refresh error string never leaks to the UI.
    expect(screen.queryByText(/listNotes hiccup/)).toBeNull();
  });

  it("a recall-stats failure degrades quietly (no strip, Library still works)", async () => {
    // A stats hiccup must never break the Library — the strip simply doesn't render.
    primeArchive([S_NEW], [NOTE]);
    vi.mocked(recallStats).mockRejectedValue("stats unavailable");
    render(<LibraryPanel />);

    // The archive still renders fine.
    await screen.findByText("Design memory hub");
    expect(screen.getByText("Prefer tokens over hardcoded colors")).toBeInTheDocument();
    // No stats error leaks into the UI, and no strip.
    expect(screen.queryByText(/stats unavailable/)).toBeNull();
    expect(screen.queryByText(/found nothing/)).toBeNull();
  });
});
