import { describe, it, expect } from "vitest";
import { MAIN_NAV, BRAIN_VIEWS, navBadge, isBrainView, landingView } from "./nav";

describe("MAIN_NAV", () => {
  it("lists the three primary tabs in order (Settings is pinned separately; Review/Mandates live in Brain)", () => {
    expect(MAIN_NAV.map((n) => n.label)).toEqual(["AIR", "AIR Note", "Brain"]);
    expect(MAIN_NAV.map((n) => n.view)).toEqual(["identity", "inbox", "library"]);
  });
  it("routes the Brain item at the Library (its landing sub-tab), not raw memory search", () => {
    const brain = MAIN_NAV.find((n) => n.label === "Brain");
    expect(brain?.view).toBe("library");
  });
});

describe("BRAIN_VIEWS", () => {
  it("includes library as a Brain-hosted view", () => {
    expect(BRAIN_VIEWS).toContain("library");
  });
});

describe("isBrainView", () => {
  it("is true for the views hosted inside the Brain hub", () => {
    expect(isBrainView("library")).toBe(true);
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

describe("landingView", () => {
  it("lands an onboarded user on the Library", () => {
    expect(landingView(true)).toBe("library");
  });
  it("keeps a not-onboarded user on identity (the onboarding gate is never bypassed)", () => {
    expect(landingView(false)).toBe("identity");
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
