import { describe, it, expect } from "vitest";
import { bodyText } from "./bodyText";

describe("bodyText", () => {
  it("renders a text body", () => { expect(bodyText({ type: "text", text: "hi" })).toBe("hi"); });
  it("renders a room message", () => { expect(bodyText({ type: "room/msg", text: "yo" })).toBe("yo"); });
  it("matches the CLI room-join wording exactly", () => {
    expect(bodyText({ type: "room/joined", room_name: "ops" })).toBe('📥 You were added to room "ops"');
  });
  it("marks an undecryptable/absent body as locked", () => {
    expect(bodyText(undefined)).toBe("🔒 (encrypted)");
    expect(bodyText({ type: "encrypted" })).toBe("🔒 (encrypted)");
  });
  it("falls back to JSON for unknown shapes", () => {
    expect(bodyText({ type: "offer", item_id: "x" })).toBe('{"type":"offer","item_id":"x"}');
  });
});
