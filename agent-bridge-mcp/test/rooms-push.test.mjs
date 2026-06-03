import { test } from "node:test";
import assert from "node:assert/strict";
import { roomPushDecision } from "../src/channel.mjs";

const member = { did: "did:wba:s" }, me = { airId: "AIR-ME", did: "did:wba:me" };
const state = { halted: false, members: [member, { did: "did:wba:me" }] };
const baseMsg = { verified: true, contact: "Peer", key_changed: false, from: "did:wba:s", room_id: "r1" };

test("addressed member message ⇒ push + addressed", () => {
  const m = { ...baseMsg, body: { type: "room/msg", mentions: ["AIR-ME"], room_id: "r1" } };
  const d = roomPushDecision(m, state, me, new Set());
  assert.equal(d.push, true); assert.equal(d.addressed, true);
});
test("un-addressed member message ⇒ push but not addressed", () => {
  const m = { ...baseMsg, body: { type: "room/msg", mentions: ["AIR-OTHER"], room_id: "r1" } };
  const d = roomPushDecision(m, state, me, new Set());
  assert.equal(d.push, true); assert.equal(d.addressed, false);
});
test("halted room ⇒ no push", () => {
  const m = { ...baseMsg, body: { type: "room/msg", mentions: ["AIR-ME"], room_id: "r1" } };
  assert.equal(roomPushDecision(m, { ...state, halted: true }, me, new Set()).push, false);
});
test("non-member sender ⇒ no push", () => {
  const m = { ...baseMsg, from: "did:wba:stranger", body: { type: "room/msg", mentions: ["AIR-ME"], room_id: "r1" } };
  assert.equal(roomPushDecision(m, state, me, new Set()).push, false);
});
test(">1 mention incl. me ⇒ push + confirm (not auto-addressed)", () => {
  const m = { ...baseMsg, body: { type: "room/msg", mentions: ["AIR-ME", "AIR-X"], room_id: "r1" } };
  const d = roomPushDecision(m, state, me, new Set());
  assert.equal(d.push, true); assert.equal(d.confirm, true); assert.equal(d.addressed, false);
});
