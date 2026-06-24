import { describe, it, expect } from "vitest";
import { filterConversations } from "./filterConversations";
import type { Conversation } from "../inbox/model";

const conv = (convKey: string, lastText: string): Conversation => ({
  convKey, kind: "peer", lastTimestamp: "2026-06-24T00:00:00Z", lastText, unread: 0,
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

  it("maps to a SearchResult targeting the inbox conversation", () => {
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
    expect(filterConversations(many, "hello", 3)).toHaveLength(3);
  });
});
