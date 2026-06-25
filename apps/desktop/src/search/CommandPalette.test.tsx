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
  errors: { memory: false, conversations: false, files: false },
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
});
