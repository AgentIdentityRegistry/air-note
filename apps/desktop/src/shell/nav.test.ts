import { describe, it, expect } from "vitest";
import { MAIN_NAV, navBadge, isBrainView } from "./nav";

describe("MAIN_NAV", () => {
  it("lists the three primary tabs in order (Settings is pinned separately; Review/Mandates live in Brain)", () => {
    expect(MAIN_NAV.map((n) => n.label)).toEqual(["AIR", "AIR Note", "Brain"]);
    expect(MAIN_NAV.map((n) => n.view)).toEqual(["identity", "inbox", "memory"]);
  });
});

describe("isBrainView", () => {
  it("is true for the views hosted inside the Brain hub", () => {
    expect(isBrainView("memory")).toBe(true);
    expect(isBrainView("review")).toBe(true);
    expect(isBrainView("mandates")).toBe(true);
  });
  it("is false for top-level views outside the Brain hub", () => {
    expect(isBrainView("identity")).toBe(false);
    expect(isBrainView("inbox")).toBe(false);
    expect(isBrainView("settings")).toBe(false);
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
