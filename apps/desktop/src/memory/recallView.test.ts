import { describe, it, expect } from "vitest";
import { toRow } from "./recallView";
import type { HitDto } from "../api/engine";

describe("toRow", () => {
  it("labels a page hit as Dossier and joins sources", () => {
    const h: HitDto = {
      event_id: "e",
      score: 0.42,
      kind: "page",
      sources: ["vector", "keyword"],
      text: "hi",
    };
    const r = toRow(h);
    expect(r.id).toBe("e");
    expect(r.kindLabel).toBe("Dossier");
    expect(r.sourcesLabel).toBe("vector + keyword");
    expect(r.score).toBe("0.42");
    expect(r.text).toBe("hi");
  });

  it("labels a memory hit as Memory", () => {
    const h: HitDto = { event_id: "m", score: 1, kind: "memory", sources: ["vector"], text: "t" };
    expect(toRow(h).kindLabel).toBe("Memory");
    expect(toRow(h).sourcesLabel).toBe("vector");
    expect(toRow(h).score).toBe("1.00");
  });

  it("labels a file_ingested hit as File", () => {
    const h: HitDto = { event_id: "f", score: 0, kind: "file_ingested", sources: ["keyword"], text: "t" };
    expect(toRow(h).kindLabel).toBe("File");
  });

  it("falls back to the raw kind for an unknown kind", () => {
    const h: HitDto = { event_id: "u", score: 0.5, kind: "something_else", sources: [], text: "t" };
    expect(toRow(h).kindLabel).toBe("something_else");
    expect(toRow(h).sourcesLabel).toBe("");
  });
});
