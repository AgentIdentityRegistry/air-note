import { describe, it, expect } from "vitest";
import { MAIN_NAV, navBadge } from "./nav";

describe("MAIN_NAV", () => {
  it("lists the five primary views in order (Settings is pinned separately)", () => {
    expect(MAIN_NAV.map((n) => n.view)).toEqual([
      "identity", "inbox", "memory", "review", "mandates",
    ]);
  });
});

describe("navBadge", () => {
  it("returns the count as a string when positive", () => {
    expect(navBadge(3)).toBe("3");
  });
  it("returns null for zero or undefined (no badge)", () => {
    expect(navBadge(0)).toBeNull();
    expect(navBadge(undefined)).toBeNull();
  });
});
