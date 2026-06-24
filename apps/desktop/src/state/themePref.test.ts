import { describe, it, expect } from "vitest";
import { parseStoredTheme, resolveInitialTheme, THEME_STORAGE_KEY } from "./themePref";

describe("parseStoredTheme", () => {
  it("accepts the two valid values", () => {
    expect(parseStoredTheme("light")).toBe("light");
    expect(parseStoredTheme("dark")).toBe("dark");
  });
  it("returns null for missing or garbage values", () => {
    expect(parseStoredTheme(null)).toBeNull();
    expect(parseStoredTheme("")).toBeNull();
    expect(parseStoredTheme("blue")).toBeNull();
  });
});

describe("resolveInitialTheme", () => {
  it("prefers a stored value over the system preference", () => {
    expect(resolveInitialTheme("light", true)).toBe("light");
    expect(resolveInitialTheme("dark", false)).toBe("dark");
  });
  it("falls back to the system preference when nothing is stored", () => {
    expect(resolveInitialTheme(null, true)).toBe("dark");
    expect(resolveInitialTheme(null, false)).toBe("light");
  });
});

describe("THEME_STORAGE_KEY", () => {
  it("is the namespaced key", () => {
    expect(THEME_STORAGE_KEY).toBe("air.theme");
  });
});
