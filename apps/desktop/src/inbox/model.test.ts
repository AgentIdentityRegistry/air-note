import { describe, it, expect } from "vitest";
import {
  fromArchiveRow, fromLiveMessage, makeOptimistic, convKey, dedupeById, groupConversations,
  type ThreadItem,
} from "./model";
import type { ArchiveRow, InboxMessage } from "../api/inbox";

const row = (over: Partial<ArchiveRow>): ArchiveRow => ({
  envelope_id: "e1", direction: "received", thread_id: "t1", peer_did: "did:wba:p1",
  from: "did:wba:p1", to: "did:wba:me", timestamp: "2026-06-14T00:00:00Z",
  body: { type: "text", text: "hi" }, encrypted: true, verified: true,
  key_changed: false, spam: false, relay_seq: 1, room_id: null, archived_at: "x", ...over,
});

describe("convKey", () => {
  it("room_id for rooms, peer_did for 1:1", () => {
    expect(convKey({ room_id: "r1", peer_did: "p" })).toBe("r1");
    expect(convKey({ room_id: null, peer_did: "p" })).toBe("p");
  });
});

describe("normalizers", () => {
  it("live received message: peer = from, direction received", () => {
    const m: InboxMessage = { seq: 1, relay_seq: 1, envelope_id: "e9", from: "did:wba:p2",
      verified: true, encrypted: true, received_at: "2026-06-14T01:00:00Z", body: { type: "text", text: "yo" } };
    const t = fromLiveMessage(m);
    expect(t).toMatchObject({ peer_did: "did:wba:p2", direction: "received", timestamp: "2026-06-14T01:00:00Z" });
  });
  it("optimistic sent row is pending with a correlation id", () => {
    const t = makeOptimistic("corr1", "did:wba:p3", { type: "text", text: "draft" }, "2026-06-14T02:00:00Z");
    expect(t).toMatchObject({ direction: "sent", peer_did: "did:wba:p3", status: "pending", correlationId: "corr1", room_id: null });
  });
});

describe("dedupeById", () => {
  it("keeps the first occurrence (confirmed rows passed first win)", () => {
    const confirmed = fromArchiveRow(row({ envelope_id: "dup" }));
    const optimistic = { ...makeOptimistic("c", "did:wba:p1", {}, "z"), envelope_id: "dup" } as ThreadItem;
    const out = dedupeById([confirmed, optimistic]);
    expect(out).toHaveLength(1);
    expect(out[0].status).toBe("ok");
  });
});

describe("groupConversations", () => {
  it("groups by conv key, newest-first, with preview + unread", () => {
    const items: ThreadItem[] = [
      fromArchiveRow(row({ envelope_id: "a", peer_did: "did:wba:p1", timestamp: "2026-06-14T00:00:00Z" })),
      fromArchiveRow(row({ envelope_id: "b", peer_did: "did:wba:p2", timestamp: "2026-06-14T03:00:00Z" })),
      fromArchiveRow(row({ envelope_id: "c", peer_did: "did:wba:p1", timestamp: "2026-06-14T02:00:00Z" })),
    ];
    const convs = groupConversations(items, new Set(["b"]));
    expect(convs.map((c) => c.convKey)).toEqual(["did:wba:p2", "did:wba:p1"]);
    expect(convs[0].unread).toBe(1);
    expect(convs[1].lastText).toBe("hi");
  });
});
