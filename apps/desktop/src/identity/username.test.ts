import { describe, it, expect } from "vitest";
import { validateUsername } from "./username";

describe("validateUsername", () => {
  it("accepts a plain handle", () => {
    expect(validateUsername("alice")).toEqual({ ok: true, username: "alice" });
  });

  it("accepts digits and underscores", () => {
    expect(validateUsername("alice_99")).toEqual({ ok: true, username: "alice_99" });
  });

  it("lowercases before validating", () => {
    expect(validateUsername("Alice")).toEqual({ ok: true, username: "alice" });
    expect(validateUsername("  ALICE_99  ")).toEqual({ ok: true, username: "alice_99" });
  });

  it("rejects an empty handle", () => {
    expect(validateUsername("")).toEqual({
      ok: false,
      error: "Username must be 3–30 characters.",
    });
  });

  it("rejects a too-short handle", () => {
    expect(validateUsername("ab")).toEqual({
      ok: false,
      error: "Username must be 3–30 characters.",
    });
  });

  it("rejects a too-long handle", () => {
    expect(validateUsername("a".repeat(31))).toEqual({
      ok: false,
      error: "Username must be 3–30 characters.",
    });
  });

  it("accepts boundary lengths", () => {
    expect(validateUsername("abc")).toEqual({ ok: true, username: "abc" });
    expect(validateUsername("a".repeat(30))).toEqual({ ok: true, username: "a".repeat(30) });
  });

  it("rejects hyphens and spaces", () => {
    expect(validateUsername("al-ice")).toEqual({
      ok: false,
      error: "Username can only use letters, numbers, and underscores.",
    });
    expect(validateUsername("al ice")).toEqual({
      ok: false,
      error: "Username can only use letters, numbers, and underscores.",
    });
  });

  it("rejects other punctuation", () => {
    expect(validateUsername("alice!")).toEqual({
      ok: false,
      error: "Username can only use letters, numbers, and underscores.",
    });
  });
});
