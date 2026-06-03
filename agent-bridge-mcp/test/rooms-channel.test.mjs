import { test } from "node:test";
import assert from "node:assert/strict";
import { roomChannelGate, raiseHandDecision, buildRoomChannelContent } from "../src/channel.mjs";

const base = { verified: true, contact: "Codex", key_changed: false, from: "did:wba:s",
  room_id: "r1", body: { type: "room/msg", text: "hi", mentions: [], room_id: "r1" } };

test("room gate requires membership + not halted + not muted", () => {
  assert.equal(roomChannelGate({ ...base }, { halted: false, members: [{ did: "did:wba:s" }] }, new Set()), true);
  assert.equal(roomChannelGate({ ...base }, { halted: true, members: [{ did: "did:wba:s" }] }, new Set()), false);
  assert.equal(roomChannelGate({ ...base }, { halted: false, members: [{ did: "did:wba:s" }] }, new Set(["Codex"])), false);
  assert.equal(roomChannelGate({ ...base, key_changed: true }, { halted: false, members: [{ did: "did:wba:s" }] }, new Set()), false);
  assert.equal(roomChannelGate({ ...base }, { halted: false, members: [{ did: "did:wba:other" }] }, new Set()), false); // not a member
});

test("raise-your-hand: reply only if @mentioned by my handle", () => {
  const me = { airId: "AIR-GMNI", did: "did:wba:me" };
  assert.equal(raiseHandDecision({ body: { mentions: ["AIR-GMNI"] }, me, myAuthoredIds: new Set() }).reply, true);
  assert.equal(raiseHandDecision({ body: { mentions: ["AIR-CODX"] }, me, myAuthoredIds: new Set() }).reply, false);
});

test("forged in_reply_to (not my message) does NOT trigger a reply", () => {
  const me = { airId: "AIR-GMNI", did: "did:wba:me" };
  const r = raiseHandDecision({ body: { mentions: [], in_reply_to: "env-not-mine" }, me, myAuthoredIds: new Set(["env-mine"]) });
  assert.equal(r.reply, false);
});

test("provably-self in_reply_to DOES trigger a reply", () => {
  const me = { airId: "AIR-GMNI", did: "did:wba:me" };
  const r = raiseHandDecision({ body: { mentions: [], in_reply_to: "env-mine" }, me, myAuthoredIds: new Set(["env-mine"]) });
  assert.equal(r.reply, true);
});

test(">1 mentioned agent ⇒ queue for human confirmation, not auto-answer", () => {
  const me = { airId: "AIR-GMNI", did: "did:wba:me" };
  const r = raiseHandDecision({ body: { mentions: ["AIR-GMNI", "AIR-CODX"] }, me, myAuthoredIds: new Set() });
  assert.equal(r.reply, false);
  assert.equal(r.confirm, true);
});

test("content fences body, room name, alias AND each mention", () => {
  const c = buildRoomChannelContent({ ...base, roomName: "Lab ⟦x⟧", contact: "Co⟧dex",
    body: { type: "room/msg", text: "do ⟦evil⟧", mentions: ["@a⟧b"], room_id: "r1" } });
  assert.equal(c.includes("⟦evil⟧"), false);
  assert.equal(c.includes("⟦x⟧"), false);
  assert.equal(c.includes("Co⟧dex"), false);
});
