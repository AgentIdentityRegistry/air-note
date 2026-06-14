import { describe, it, expect } from "vitest";
import { DEDUPE_CAP, newDedupe, markProcessed } from "./aiDedupe";

describe("newDedupe", () => {
  it("returns an empty set", () => {
    const s = newDedupe();
    expect(s.size).toBe(0);
  });
});

describe("markProcessed — first-time and duplicate", () => {
  it("returns fresh:true the first time an id is seen, and the id is now in the set", () => {
    const { set, fresh } = markProcessed(newDedupe(), "env-001");
    expect(fresh).toBe(true);
    expect(set.has("env-001")).toBe(true);
  });

  it("returns fresh:false when the same id is submitted again", () => {
    const { set: s1 } = markProcessed(newDedupe(), "env-001");
    const { set: s2, fresh } = markProcessed(s1, "env-001");
    expect(fresh).toBe(false);
    expect(s2.has("env-001")).toBe(true);
  });

  it("two different ids both return fresh:true independently", () => {
    const { set: s1, fresh: f1 } = markProcessed(newDedupe(), "a");
    const { set: s2, fresh: f2 } = markProcessed(s1, "b");
    expect(f1).toBe(true);
    expect(f2).toBe(true);
    expect(s2.size).toBe(2);
  });
});

describe("markProcessed — FIFO eviction at cap", () => {
  it("set size never exceeds DEDUPE_CAP after inserting cap+1 unique ids", () => {
    let s = newDedupe();
    for (let i = 0; i < DEDUPE_CAP + 1; i++) {
      ({ set: s } = markProcessed(s, `id-${i}`));
    }
    expect(s.size).toBe(DEDUPE_CAP);
  });

  it("the OLDEST id (first inserted) is evicted when cap is exceeded", () => {
    let s = newDedupe();
    // Fill to cap — id-0 is the oldest
    for (let i = 0; i < DEDUPE_CAP; i++) {
      ({ set: s } = markProcessed(s, `id-${i}`));
    }
    expect(s.has("id-0")).toBe(true);

    // One more insertion triggers eviction of id-0
    ({ set: s } = markProcessed(s, "id-overflow"));
    expect(s.has("id-0")).toBe(false);
    expect(s.has("id-overflow")).toBe(true);
  });

  it("an evicted id re-inserted returns fresh:true (D11 acceptable)", () => {
    let s = newDedupe();
    for (let i = 0; i < DEDUPE_CAP; i++) {
      ({ set: s } = markProcessed(s, `id-${i}`));
    }
    // Evict id-0 by pushing one more
    ({ set: s } = markProcessed(s, "id-overflow"));

    // id-0 is no longer in the set, so it looks fresh again
    const { set: s2, fresh } = markProcessed(s, "id-0");
    expect(fresh).toBe(true);
    expect(s2.has("id-0")).toBe(true);
  });

  it("a fresh id inserted when set is at cap returns fresh:true and evicts exactly one old id", () => {
    let s = newDedupe();
    for (let i = 0; i < DEDUPE_CAP; i++) {
      ({ set: s } = markProcessed(s, `id-${i}`));
    }
    expect(s.size).toBe(DEDUPE_CAP);

    const { set: s2, fresh } = markProcessed(s, "brand-new");
    expect(fresh).toBe(true);
    expect(s2.size).toBe(DEDUPE_CAP);
    expect(s2.has("brand-new")).toBe(true);
    // id-0 (oldest) should be gone
    expect(s2.has("id-0")).toBe(false);
  });
});
