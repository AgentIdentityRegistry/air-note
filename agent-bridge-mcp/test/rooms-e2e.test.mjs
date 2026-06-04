// test/rooms-e2e.test.mjs — the §1 scene: founder + 2 agents; admin adds a member;
// founder kicks one; @ask agent A ⇒ A eligible, B not; halt freezes the gate.
import { test } from "node:test";
import assert from "node:assert/strict";
import { deriveState, rosterDigest } from "../src/rooms.mjs";
import { roomReceiveCheck } from "../src/core.mjs";
import { raiseHandDecision, roomChannelGate } from "../src/channel.mjs";

test("the §1 scene: add/kick/address/halt all behave", () => {
  const F = "did:wba:f", A = "did:wba:a", B = "did:wba:b", M = "did:wba:m";
  const ops = [
    { type:"room/create", room_id:"r", issuer_did:F, founder_did:F, founder_pubkey:"zF", thread_id:"t", founder_seq:"0" },
    { type:"room/add", room_id:"r", issuer_did:F, member_did:F, member_pubkey:"zF", kind:"human", founder_seq:"1" }, // founder self-add
    { type:"room/add", room_id:"r", issuer_did:F, member_did:A, member_pubkey:"zA", kind:"agent", founder_seq:"2" },
    { type:"room/add", room_id:"r", issuer_did:F, member_did:B, member_pubkey:"zB", kind:"agent", founder_seq:"3" },
    { type:"room/admin-grant", room_id:"r", issuer_did:F, mandate_id:"g", holder_did:A, holder_pubkey:"zA", scope:"member:add", founder_seq:"4" },
    { type:"room/add", room_id:"r", issuer_did:A, member_did:M, member_pubkey:"zM", kind:"human", mandate_id:"g" }, // admin adds (claims human → forced agent)
    { type:"room/remove", room_id:"r", issuer_did:F, member_did:B, founder_seq:"5" },                               // founder kicks B
  ];
  const s = deriveState(ops);
  assert.deepEqual(s.members.map((m) => m.did).sort(), [A, F, M].sort()); // members = {F, A, M}
  assert.equal(s.members.some((m) => m.did === B), false);                // B kicked
  assert.equal(s.members.find((m) => m.did === M).kind, "agent");          // admin-added ⇒ agent
  assert.equal(s.members.find((m) => m.did === F).kind, "human");          // founder is a human member

  // roster gate: a member is accepted, a stranger is dropped
  const digest = rosterDigest(s);
  const body = { type:"room/msg", room_id:"r", mentions:["AIR-A"], recipients:[A, F, M].sort(), roster_digest: digest, text:"status?" };
  assert.equal(roomReceiveCheck({ senderDid: F, selfDid: A, body, state: s, localDigest: digest }).accept, true);
  assert.equal(roomReceiveCheck({ senderDid: "did:wba:stranger", selfDid: A, body, state: s, localDigest: digest }).accept, false);

  // founder @asks agent A; A eligible to reply, B is not addressed
  const meA = { airId: "AIR-A", did: A }, meB = { airId: "AIR-B", did: B };
  assert.equal(raiseHandDecision({ body, me: meA, myAuthoredIds: new Set() }).reply, true);
  assert.equal(raiseHandDecision({ body, me: meB, myAuthoredIds: new Set() }).reply, false);

  // halt freezes the push gate; before halt a verified member message would push
  const haltState = deriveState([...ops, { type:"room/halt", room_id:"r", issuer_did:F, founder_seq:"6" }]);
  const m = { verified:true, contact:"Founder", key_changed:false, from:F, body };
  assert.equal(roomChannelGate(m, haltState, new Set()), false); // halted ⇒ no push
  assert.equal(roomChannelGate(m, s, new Set()), true);          // not halted + F is a member ⇒ push ok
});
