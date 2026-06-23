import { describe, it, expect } from "vitest";
import { inlineDiff } from "./diffView";

describe("inlineDiff", () => {
  it("marks a changed line as a removal then an addition", () => {
    const lines = inlineDiff("Alice works at Acme.\n", "Alice works at Globex.\n");
    expect(lines).toEqual([
      { kind: "del", text: "Alice works at Acme." },
      { kind: "add", text: "Alice works at Globex." },
    ]);
  });

  it("keeps unchanged lines as context", () => {
    const lines = inlineDiff("a\nb\nc\n", "a\nB\nc\n");
    expect(lines).toEqual([
      { kind: "ctx", text: "a" },
      { kind: "del", text: "b" },
      { kind: "add", text: "B" },
      { kind: "ctx", text: "c" },
    ]);
  });

  it("handles a pure addition (empty old)", () => {
    expect(inlineDiff("", "new line\n")).toEqual([{ kind: "add", text: "new line" }]);
  });

  it("returns nothing for identical input", () => {
    expect(inlineDiff("same\n", "same\n")).toEqual([{ kind: "ctx", text: "same" }]);
  });
});
