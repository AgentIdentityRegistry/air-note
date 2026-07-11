import { describe, it, expect } from "vitest";
import { SUBTABS } from "./BrainPanel";
import { isBrainView } from "../shell/nav";

describe("BrainPanel SUBTABS", () => {
  it("leads with the Library sub-tab (the onboarded landing)", () => {
    expect(SUBTABS[0].view).toBe("library");
    expect(SUBTABS[0].label).toBe("Library");
  });
  it("still exposes the original Brain sections after Library", () => {
    expect(SUBTABS.map((t) => t.view)).toEqual(["library", "memory", "review", "mandates"]);
  });
  it("only hosts Brain views", () => {
    for (const t of SUBTABS) expect(isBrainView(t.view)).toBe(true);
  });
});
