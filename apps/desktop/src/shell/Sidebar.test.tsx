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

  it("renders the three primary nav tabs plus Settings (Review/Mandates live in Brain)", () => {
    renderSidebar();
    // Exact names (not regex): "AIR" must not match "AIR Note".
    for (const label of ["AIR", "AIR Note", "Brain", "Settings"]) {
      expect(screen.getByRole("button", { name: label })).toBeInTheDocument();
    }
  });

  it("shows the inbox unread badge on AIR Note and the review badge on Brain", () => {
    renderSidebar({ inboxUnread: 7, reviewCount: 2 });
    expect(screen.getByText("7")).toBeInTheDocument();
    expect(screen.getByText("2")).toBeInTheDocument();
  });

  it("marks Brain active for any Brain view (memory/review/mandates)", () => {
    renderSidebar({ view: "review" });
    expect(screen.getByRole("button", { name: /Brain/ })).toHaveClass("active");
  });

  it("calls onNavigate when a nav item is clicked (Brain opens at its Library landing)", () => {
    const { onNavigate } = renderSidebar();
    fireEvent.click(screen.getByRole("button", { name: /Brain/ }));
    expect(onNavigate).toHaveBeenCalledWith("library");
  });

  it("toggles the theme from the footer icon button", () => {
    renderSidebar();
    expect(document.documentElement.dataset.theme).toBe("light");
    fireEvent.click(screen.getByRole("button", { name: /theme/i }));
    expect(document.documentElement.dataset.theme).toBe("dark");
  });

  it("navigates to Settings from the footer gear icon and marks it active", () => {
    const { onNavigate } = renderSidebar({ view: "settings" });
    const settings = screen.getByRole("button", { name: "Settings" });
    expect(settings).toHaveClass("active");
    fireEvent.click(settings);
    expect(onNavigate).toHaveBeenCalledWith("settings");
  });
});
