// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach } from "vitest";
import { type ComponentProps } from "react";
import { render, screen, fireEvent } from "@testing-library/react";
import { Sidebar } from "./Sidebar";
import { ThemeProvider } from "../state/theme";

function renderSidebar(props: Partial<ComponentProps<typeof Sidebar>> = {}) {
  const onNavigate = vi.fn();
  render(
    <ThemeProvider>
      <Sidebar view="identity" onNavigate={onNavigate} inboxUnread={0} reviewCount={0} {...props} />
    </ThemeProvider>,
  );
  return { onNavigate };
}

describe("Sidebar", () => {
  beforeEach(() => {
    localStorage.clear();
    delete document.documentElement.dataset.theme;
  });

  it("renders the five primary nav items plus Settings", () => {
    renderSidebar();
    for (const label of ["Identity", "Inbox", "Memory", "Review", "Mandates", "Settings"]) {
      expect(screen.getByRole("button", { name: new RegExp(label) })).toBeInTheDocument();
    }
  });

  it("shows the inbox unread + review badges", () => {
    renderSidebar({ inboxUnread: 7, reviewCount: 2 });
    expect(screen.getByText("7")).toBeInTheDocument();
    expect(screen.getByText("2")).toBeInTheDocument();
  });

  it("calls onNavigate when a nav item is clicked", () => {
    const { onNavigate } = renderSidebar();
    fireEvent.click(screen.getByRole("button", { name: /Memory/ }));
    expect(onNavigate).toHaveBeenCalledWith("memory");
  });

  it("toggles the theme from the footer button", () => {
    renderSidebar();
    expect(document.documentElement.dataset.theme).toBe("light");
    fireEvent.click(screen.getByRole("button", { name: /theme/i }));
    expect(document.documentElement.dataset.theme).toBe("dark");
  });
});
