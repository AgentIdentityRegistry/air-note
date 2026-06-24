import { describe, it, expect } from "vitest";
import { memoryResults, flattenResults } from "./rankResults";
import type { HitDto } from "../api/engine";
import type { GroupedResults } from "./types";

const hit = (event_id: string, kind: string, text: string): HitDto => ({
  event_id, kind, text, score: 1, sources: ["vector"],
});

describe("memoryResults", () => {
  it("labels kinds (memory/page/file_ingested) and targets the memory panel", () => {
    const out = memoryResults([hit("e1", "page", "dossier text"), hit("e2", "memory", "a memory")]);
    expect(out[0]).toMatchObject({ id: "mem:e1", kind: "memory", title: "Dossier", snippet: "dossier text", target: { view: "memory" } });
    expect(out[1].title).toBe("Memory");
  });
  it("falls back to the raw kind and caps", () => {
    expect(memoryResults([hit("e", "weird", "t")])[0].title).toBe("weird");
    expect(memoryResults(Array.from({ length: 9 }, (_, i) => hit(`e${i}`, "memory", "t")), 5)).toHaveLength(5);
  });
});

describe("flattenResults", () => {
  it("concatenates memory, then conversations, then files in order", () => {
    const g: GroupedResults = {
      memory: [{ id: "mem:1", kind: "memory", title: "M", snippet: "", target: { view: "memory" } }],
      conversations: [{ id: "conv:1", kind: "conversation", title: "C", snippet: "", target: { view: "inbox", convKey: "c" } }],
      files: [{ id: "file:1", kind: "file", title: "F", snippet: "", target: { view: "settings" } }],
      errors: { memory: false, conversations: false, files: false },
    };
    expect(flattenResults(g).map((r) => r.id)).toEqual(["mem:1", "conv:1", "file:1"]);
  });
});
