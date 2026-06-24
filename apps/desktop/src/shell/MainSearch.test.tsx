// @vitest-environment jsdom
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { MainSearch } from "./MainSearch";

describe("MainSearch", () => {
  it("renders the search placeholder and the ⌘K hint", () => {
    render(<MainSearch onOpen={() => {}} />);
    expect(screen.getByText("Search memory, conversations, files…")).toBeInTheDocument();
    expect(screen.getByText("⌘K")).toBeInTheDocument();
  });

  it("calls onOpen when clicked", () => {
    const onOpen = vi.fn();
    render(<MainSearch onOpen={onOpen} />);
    fireEvent.click(screen.getByRole("button", { name: /search memory, conversations, and files/i }));
    expect(onOpen).toHaveBeenCalledOnce();
  });
});
