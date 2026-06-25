import { describe, it, expect } from "vitest";
import { filterConversations } from "./filterConversations";
import type { Conversation } from "../inbox/model";
import type { ContactView } from "../api/inbox";

const conv = (convKey: string, lastText: string, kind: "peer" | "room" = "peer"): Conversation => ({
  convKey, kind, lastTimestamp: "2026-06-24T00:00:00Z", lastText, unread: 0,
});
const cv = (did: string, over: Partial<ContactView> = {}): ContactView => ({
  did, alias: null, name: null, username: null, verified_at_pin: false, ...over,
});

describe("filterConversations", () => {
  const convs = [
    conv("did:key:alice", "lunch tomorrow?"),
    conv("did:key:bob", "shipping the redesign"),
  ];

  it("returns [] for an empty query", () => {
    expect(filterConversations(convs, "  ")).toEqual([]);
  });

  it("matches case-insensitively on convKey and lastText", () => {
    expect(filterConversations(convs, "ALICE").map((r) => r.id)).toEqual(["conv:did:key:alice"]);
    expect(filterConversations(convs, "redesign").map((r) => r.id)).toEqual(["conv:did:key:bob"]);
  });

  it("matches on the resolved contact name and titles the result with it", () => {
    const contacts = new Map([["did:key:bob", cv("did:key:bob", { alias: "Bob Loblaw" })]]);
    const [r] = filterConversations(convs, "loblaw", contacts);
    expect(r).toMatchObject({ id: "conv:did:key:bob", title: "Bob Loblaw" });
  });

  it("matches on the @handle", () => {
    const contacts = new Map([["did:key:alice", cv("did:key:alice", { username: "alice_a" })]]);
    expect(filterConversations(convs, "@alice_a", contacts).map((r) => r.id)).toEqual(["conv:did:key:alice"]);
  });

  it("titles a result with short(did) when no contact is known", () => {
    const [r] = filterConversations(convs, "alice");
    expect(r).toMatchObject({
      kind: "conversation",
      title: "did:key:alice",
      snippet: "lunch tomorrow?",
      target: { view: "inbox", convKey: "did:key:alice" },
    });
  });

  it("caps the number of results", () => {
    const many = Array.from({ length: 10 }, (_, i) => conv(`did:key:p${i}`, "hello"));
    expect(filterConversations(many, "hello", undefined, 3)).toHaveLength(3);
  });
});
