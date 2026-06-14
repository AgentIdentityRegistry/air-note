import { describe, it, expect } from "vitest";
import { addUnread, clearConv } from "./unread";
import { fromArchiveRow, type ThreadItem } from "./model";
import type { ArchiveRow } from "../api/inbox";

const item = (id: string, peer: string): ThreadItem =>
  fromArchiveRow({ envelope_id: id, direction: "received", thread_id: "t", peer_did: peer, from: peer,
    to: "me", timestamp: "z", body: {}, encrypted: false, verified: true, key_changed: false,
    spam: false, relay_seq: 1, room_id: null, archived_at: "z" } as ArchiveRow);

describe("unread ops", () => {
  it("addUnread returns a NEW set with the id", () => {
    const a = new Set<string>(); const b = addUnread(a, "e1");
    expect(b.has("e1")).toBe(true); expect(a.has("e1")).toBe(false);
  });
  it("clearConv removes only the conv's loaded ids", () => {
    const set = new Set(["e1", "e2", "e3"]);
    const loaded = [item("e1", "p1"), item("e2", "p2")];
    const out = clearConv(set, loaded, "p1");
    expect(out.has("e1")).toBe(false); expect(out.has("e2")).toBe(true); expect(out.has("e3")).toBe(true);
  });
});
