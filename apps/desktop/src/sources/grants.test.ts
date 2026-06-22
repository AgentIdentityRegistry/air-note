import { describe, it, expect } from "vitest";
import { activeGrants } from "./grants";
import type { GrantDto } from "../api/engine";

describe("activeGrants", () => {
  it("drops revoked grants", () => {
    const all: GrantDto[] = [
      { canonical_root: "/a", granted_at: "t1", revoked: false },
      { canonical_root: "/b", granted_at: "t2", revoked: true },
    ];
    expect(activeGrants(all).map((g) => g.canonical_root)).toEqual(["/a"]);
  });
});
