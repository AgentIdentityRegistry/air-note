import { describe, it, expect } from "vitest";
import { validateDisplayName, MAX_DISPLAY_NAME_LEN } from "./displayName";

describe("validateDisplayName", () => {
  it("accepts a name and returns it trimmed", () => {
    expect(validateDisplayName("  Aria Novak  ")).toEqual({ ok: true, name: "Aria Novak" });
  });

  it("rejects an empty name", () => {
    expect(validateDisplayName("")).toEqual({ ok: false, error: "Name can’t be empty." });
  });

  it("rejects a whitespace-only name", () => {
    expect(validateDisplayName("   ")).toEqual({ ok: false, error: "Name can’t be empty." });
  });

  it("rejects a name over the cap", () => {
    const huge = "x".repeat(MAX_DISPLAY_NAME_LEN + 1);
    expect(validateDisplayName(huge)).toEqual({
      ok: false,
      error: `Name is too long (max ${MAX_DISPLAY_NAME_LEN} characters).`,
    });
  });

  it("accepts a name exactly at the cap", () => {
    const atCap = "x".repeat(MAX_DISPLAY_NAME_LEN);
    expect(validateDisplayName(atCap)).toEqual({ ok: true, name: atCap });
  });
});
