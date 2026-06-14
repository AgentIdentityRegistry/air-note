import { describe, it, expect } from "vitest";
import { shouldSkip, isUnreadableBody } from "./aiSkip";
import type { InboxMessage } from "../api/inbox";

/** Minimal valid InboxMessage with overridable fields. */
function msg(over: Partial<InboxMessage> = {}): InboxMessage {
  return {
    seq: 1,
    relay_seq: 1,
    envelope_id: "env-1",
    from: "did:peer:alice",
    verified: true,
    encrypted: false,
    received_at: "2026-06-14T00:00:00Z",
    body: { type: "text", text: "hi" },
    ...over,
  };
}

describe("isUnreadableBody — gates on RAW shape, not rendered text (D12/M-B)", () => {
  it("absent body (undefined/null) is unreadable", () => {
    expect(isUnreadableBody(undefined)).toBe(true);
    expect(isUnreadableBody(null)).toBe(true);
  });

  it("explicit {type:'encrypted'} sentinel is unreadable", () => {
    expect(isUnreadableBody({ type: "encrypted" })).toBe(true);
  });

  it("a normal text body is readable", () => {
    expect(isUnreadableBody({ type: "text", text: "hello" })).toBe(false);
  });

  it("a benign body whose TEXT contains the lock glyph is still readable (raw shape, not string)", () => {
    // bodyText() would render this verbatim, but the raw shape is a real text body → readable.
    expect(isUnreadableBody({ type: "text", text: "🔒 (encrypted)" })).toBe(false);
  });

  it("a room/msg body is readable (room exclusion is handled separately, not here)", () => {
    expect(isUnreadableBody({ type: "room/msg", text: "yo" })).toBe(false);
  });
});

describe("shouldSkip — pre-reservation gating", () => {
  const empty = new Set<string>();

  it("returns null (proceed) for a normal direct, readable, unlocked message", () => {
    expect(shouldSkip(msg(), empty)).toBeNull();
  });

  it("skips rooms (D6) — even if otherwise perfectly replyable", () => {
    expect(shouldSkip(msg({ room_id: "room-42" }), empty)).toBe("room");
  });

  it("room takes precedence over an unreadable body", () => {
    expect(shouldSkip(msg({ room_id: "room-42", body: null }), empty)).toBe("room");
  });

  it("skips an unreadable (absent) body — undefined (D12)", () => {
    expect(shouldSkip(msg({ body: undefined }), empty)).toBe("unreadable");
  });

  it("skips an unreadable (absent) body — null (D12 spec: m.body == null) (M3)", () => {
    expect(shouldSkip(msg({ body: null }), empty)).toBe("unreadable");
  });

  it("skips an explicit encrypted body", () => {
    expect(shouldSkip(msg({ body: { type: "encrypted" } }), empty)).toBe("unreadable");
  });

  it("skips a DID already in the in-flight lock set", () => {
    const locked = new Set(["did:peer:alice"]);
    expect(shouldSkip(msg({ from: "did:peer:alice" }), locked)).toBe("in_flight");
  });

  it("does NOT skip a different DID when another is locked", () => {
    const locked = new Set(["did:peer:bob"]);
    expect(shouldSkip(msg({ from: "did:peer:alice" }), locked)).toBeNull();
  });

  it("unreadable is checked before the in-flight lock (order: room → unreadable → lock)", () => {
    const locked = new Set(["did:peer:alice"]);
    expect(shouldSkip(msg({ from: "did:peer:alice", body: null }), locked)).toBe("unreadable");
  });
});
