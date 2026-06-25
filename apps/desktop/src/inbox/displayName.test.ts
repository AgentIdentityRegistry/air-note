import { describe, it, expect } from "vitest";
import { shortDid, displayName, handleOf, contactsByDid, conversationLabel } from "./displayName";
import type { ContactView } from "../api/inbox";

const DID = "did:wba:agentidentityregistry.org:agents:AIR-3C33-M64E-KQKJ";
const c = (over: Partial<ContactView> = {}): ContactView => ({
  did: DID, alias: null, name: null, username: null, verified_at_pin: false, ...over,
});

describe("shortDid", () => {
  it("strips the did:wba prefix down to the AIR id", () => {
    expect(shortDid(DID)).toBe("AIR-3C33-M64E-KQKJ");
  });
  it("passes through a value with no prefix", () => {
    expect(shortDid("room-123")).toBe("room-123");
  });
});

describe("displayName precedence: alias → name → short(did)", () => {
  it("prefers the alias", () => {
    expect(displayName(DID, c({ alias: "kenny", name: "Kenny" }))).toBe("kenny");
  });
  it("falls back to the registry name", () => {
    expect(displayName(DID, c({ name: "Kenny" }))).toBe("Kenny");
  });
  it("falls back to short(did) with no contact", () => {
    expect(displayName(DID, undefined)).toBe("AIR-3C33-M64E-KQKJ");
  });
  it("ignores whitespace-only alias/name", () => {
    expect(displayName(DID, c({ alias: "   ", name: "  " }))).toBe("AIR-3C33-M64E-KQKJ");
  });
});

describe("handleOf", () => {
  it("prefixes a claimed handle with @", () => {
    expect(handleOf(c({ username: "kenny" }))).toBe("@kenny");
  });
  it("returns null when unclaimed", () => {
    expect(handleOf(c())).toBeNull();
    expect(handleOf(undefined)).toBeNull();
  });
});

describe("contactsByDid", () => {
  it("indexes by did", () => {
    const m = contactsByDid([c({ alias: "kenny" })]);
    expect(m.get(DID)?.alias).toBe("kenny");
  });
});

describe("conversationLabel", () => {
  it("resolves a peer to name + handle", () => {
    expect(conversationLabel(DID, "peer", c({ alias: "kenny", username: "kenny" })))
      .toEqual({ label: "kenny", handle: "@kenny" });
  });
  it("passes a room id through unchanged with no handle", () => {
    expect(conversationLabel("room-abc", "room", undefined))
      .toEqual({ label: "room-abc", handle: null });
  });
});
