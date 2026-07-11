// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { CommandPalette } from "./CommandPalette";
import type { GroupedResults } from "./types";

// Stub the inbox hook (the palette reads conversations + contacts from it).
vi.mock("../state/inbox", () => ({ useInbox: () => ({ conversations: [], contacts: new Map() }) }));

const grouped: GroupedResults = {
  memory: [{ id: "mem:1", kind: "memory", title: "Memory", snippet: "alpha memory", target: { view: "memory" } }],
  conversations: [{ id: "conv:1", kind: "conversation", title: "did:key:bob", snippet: "alpha chat", target: { view: "inbox", convKey: "did:key:bob" } }],
  files: [],
  sessions: [],
  errors: { memory: false, conversations: false, files: false, sessions: false },
};
const search = vi.fn(async () => grouped);

beforeEach(() => search.mockClear());

function setup(open = true) {
  const onClose = vi.fn();
  const onNavigate = vi.fn();
  render(<CommandPalette open={open} onClose={onClose} onNavigate={onNavigate} search={search} />);
  return { onClose, onNavigate };
}

describe("CommandPalette", () => {
  it("renders nothing when closed", () => {
    setup(false);
    expect(screen.queryByPlaceholderText(/search/i)).not.toBeInTheDocument();
  });

  it("focuses the input, debounces a query, and renders grouped results", async () => {
    setup();
    const input = screen.getByPlaceholderText(/search/i);
    expect(input).toHaveFocus();
    fireEvent.change(input, { target: { value: "alpha" } });
    // Real timers: findByText polls up to 1000ms, easily covering the 180ms debounce.
    expect(await screen.findByText("alpha memory")).toBeInTheDocument();
    expect(screen.getByText("did:key:bob")).toBeInTheDocument();
    expect(search).toHaveBeenCalledWith("alpha", expect.anything());
  });

  it("Enter navigates to the selected result's target and closes", async () => {
    const { onNavigate, onClose } = setup();
    fireEvent.change(screen.getByPlaceholderText(/search/i), { target: { value: "alpha" } });
    await screen.findByText("alpha memory");
    fireEvent.keyDown(window, { key: "ArrowDown" }); // move from memory[0] to conversation[1]
    fireEvent.keyDown(window, { key: "Enter" });
    expect(onNavigate).toHaveBeenCalledWith({ view: "inbox", convKey: "did:key:bob" });
    expect(onClose).toHaveBeenCalled();
  });

  it("Esc closes", () => {
    const { onClose } = setup();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).toHaveBeenCalled();
  });

  it("renders the Library group and navigates to the library view when a session hit is chosen", async () => {
    const withSession: GroupedResults = {
      ...grouped,
      sessions: [{ id: "session:s1", kind: "session", title: "Redesign spec", snippet: "work", target: { view: "library" } }],
    };
    search.mockResolvedValueOnce(withSession);
    const { onNavigate, onClose } = setup();
    fireEvent.change(screen.getByPlaceholderText(/search/i), { target: { value: "redesign" } });
    expect(await screen.findByText("Library")).toBeInTheDocument();
    fireEvent.mouseDown(screen.getByText("Redesign spec"));
    expect(onNavigate).toHaveBeenCalledWith({ view: "library" });
    expect(onClose).toHaveBeenCalled();
  });

  // Locks the one hard invariant: the palette's `groups` render order must match
  // rankResults.flattenResults' order. Memory + Library + Conversations are all populated,
  // so the flat list is [memory[0], session[0], conversation[0]]. One ArrowDown must land on
  // the session row (flat index 1); Enter then navigates to the library view. If someone
  // reorders only one of the two lists, flat[1] stops being the Library hit and this fails.
  it("keyboard-selects the Library row across populated groups (flatten order === render order)", async () => {
    const allPopulated: GroupedResults = {
      ...grouped,
      sessions: [{ id: "session:s1", kind: "session", title: "Redesign spec", snippet: "work", target: { view: "library" } }],
    };
    search.mockResolvedValueOnce(allPopulated);
    const { onNavigate, onClose } = setup();
    fireEvent.change(screen.getByPlaceholderText(/search/i), { target: { value: "redesign" } });
    await screen.findByText("alpha memory"); // memory[0] rendered → results are in
    fireEvent.keyDown(window, { key: "ArrowDown" }); // flat index 0 (memory) → 1 (session)
    fireEvent.keyDown(window, { key: "Enter" });
    expect(onNavigate).toHaveBeenCalledWith({ view: "library" });
    expect(onClose).toHaveBeenCalled();
  });
});
