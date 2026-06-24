// @vitest-environment jsdom
import { describe, it, expect, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { ThemeProvider, useTheme } from "./theme";
import { THEME_STORAGE_KEY } from "./themePref";

function Probe() {
  const { theme, toggleTheme } = useTheme();
  return <button onClick={toggleTheme}>theme:{theme}</button>;
}

describe("ThemeProvider", () => {
  beforeEach(() => {
    localStorage.clear();
    delete document.documentElement.dataset.theme;
  });

  it("sets data-theme on the root and persists when toggled", () => {
    render(<ThemeProvider><Probe /></ThemeProvider>);
    // matchMedia is undefined in jsdom → guard yields light as the default.
    expect(document.documentElement.dataset.theme).toBe("light");

    fireEvent.click(screen.getByRole("button"));

    expect(screen.getByRole("button").textContent).toBe("theme:dark");
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(localStorage.getItem(THEME_STORAGE_KEY)).toBe("dark");
  });

  it("reads the persisted theme on mount", () => {
    localStorage.setItem(THEME_STORAGE_KEY, "dark");
    render(<ThemeProvider><Probe /></ThemeProvider>);
    expect(document.documentElement.dataset.theme).toBe("dark");
  });
});
