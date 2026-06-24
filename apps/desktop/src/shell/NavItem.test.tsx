// @vitest-environment jsdom
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { NavItem } from "./NavItem";

describe("NavItem", () => {
  it("renders the label and a badge when count > 0", () => {
    render(<NavItem view="inbox" label="Inbox" count={4} active={false} onNavigate={() => {}} />);
    expect(screen.getByText("Inbox")).toBeInTheDocument();
    expect(screen.getByText("4")).toBeInTheDocument();
  });

  it("renders no badge when count is 0 or undefined", () => {
    render(<NavItem view="memory" label="Memory" active={false} onNavigate={() => {}} />);
    expect(screen.queryByText("0")).not.toBeInTheDocument();
  });

  it("marks the active item and calls onNavigate with its view", () => {
    const onNavigate = vi.fn();
    render(<NavItem view="review" label="Review" active onNavigate={onNavigate} />);
    const btn = screen.getByRole("button", { name: /Review/ });
    expect(btn).toHaveClass("active");
    expect(btn).toHaveAttribute("aria-current", "page");
    fireEvent.click(btn);
    expect(onNavigate).toHaveBeenCalledWith("review");
  });
});
