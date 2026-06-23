import { describe, it, expect } from "vitest";
import { allWritable } from "./writableGrants";

describe("allWritable", () => {
  it("true only when every active root is writable", () => {
    const active = ["/a", "/b"];
    expect(allWritable(active, new Set(["/a", "/b"]))).toBe(true);
    expect(allWritable(active, new Set(["/a"]))).toBe(false);
  });
  it("false when there are no active roots", () => {
    expect(allWritable([], new Set())).toBe(false);
  });
});
