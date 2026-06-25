import { describe, it, expect, vi } from "vitest";
import { globalSearch, type GlobalSearchDeps } from "./globalSearch";
import type { Conversation } from "../inbox/model";
import type { HitDto, FileRecordDto } from "../api/engine";

const conv = (k: string, t: string): Conversation => ({
  convKey: k, kind: "peer", lastTimestamp: "2026-06-24T00:00:00Z", lastText: t, unread: 0,
});
const hit = (id: string, t: string): HitDto => ({ event_id: id, kind: "memory", text: t, score: 1, sources: ["vector"] });
const file = (p: string, id: string): FileRecordDto => ({
  canonical_path: p, file_event_id: id, content_hash: "h", grant_root: "/r", writable: false,
});

const deps = (over: Partial<GlobalSearchDeps> = {}): GlobalSearchDeps => ({
  recall: vi.fn(async () => [hit("e1", "alpha memory")]),
  listFiles: vi.fn(async () => [file("/d/alpha.md", "f1")]),
  conversations: [conv("did:key:alpha", "alpha chat")],
  contacts: new Map(),
  ...over,
});

describe("globalSearch", () => {
  it("returns all-empty groups for an empty query and never calls the engine", async () => {
    const d = deps();
    const out = await globalSearch("   ", d);
    expect(out.memory).toEqual([]);
    expect(out.conversations).toEqual([]);
    expect(out.files).toEqual([]);
    expect(d.recall).not.toHaveBeenCalled();
    expect(d.listFiles).not.toHaveBeenCalled();
  });

  it("fans out to all three sources and groups the hits", async () => {
    const out = await globalSearch("alpha", deps());
    expect(out.memory.map((r) => r.id)).toEqual(["mem:e1"]);
    expect(out.conversations.map((r) => r.id)).toEqual(["conv:did:key:alpha"]);
    expect(out.files.map((r) => r.id)).toEqual(["file:f1"]);
    expect(out.errors).toEqual({ memory: false, conversations: false, files: false });
  });

  it("isolates a failing source: empty group + error flag, others still return", async () => {
    const out = await globalSearch("alpha", deps({ recall: vi.fn(async () => { throw new Error("engine down"); }) }));
    expect(out.memory).toEqual([]);
    expect(out.errors.memory).toBe(true);
    expect(out.conversations.map((r) => r.id)).toEqual(["conv:did:key:alpha"]);
    expect(out.files.map((r) => r.id)).toEqual(["file:f1"]);
  });

  it("isolates a failing listFiles: empty files group + error flag, others still return", async () => {
    const out = await globalSearch("alpha", deps({ listFiles: vi.fn(async () => { throw new Error("fs down"); }) }));
    expect(out.files).toEqual([]);
    expect(out.errors.files).toBe(true);
    expect(out.memory.map((r) => r.id)).toEqual(["mem:e1"]);
    expect(out.conversations.map((r) => r.id)).toEqual(["conv:did:key:alpha"]);
  });
});
