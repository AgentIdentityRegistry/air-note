import { describe, it, expect } from "vitest";
import { badgesFor } from "./badges";

describe("badgesFor", () => {
  it("lock + verified for a normal encrypted message", () => {
    expect(badgesFor({ encrypted: true, verified: true })).toEqual([
      { label: "🔒", tone: "neutral" }, { label: "✓", tone: "success" },
    ]);
  });
  it("flags unverified", () => {
    expect(badgesFor({ encrypted: false, verified: false })).toEqual([{ label: "unverified", tone: "warning" }]);
  });
  it("flags changed key + spam", () => {
    const out = badgesFor({ encrypted: true, verified: true, key_changed: true, spam: true });
    expect(out).toContainEqual({ label: "⚠ key changed", tone: "error" });
    expect(out).toContainEqual({ label: "spam", tone: "warning" });
  });
});
