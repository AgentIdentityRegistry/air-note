import { test } from "node:test";
import assert from "node:assert/strict";
import { roomReceiveCheck, createFounderBindingOk } from "../src/core.mjs";

const state = { members: [{ did: "did:wba:s" }, { did: "did:wba:me" }], admins: [], halted: false };

test("drops a room/msg from a non-member (roster gate)", () => {
  const r = roomReceiveCheck({ senderDid: "did:wba:stranger", selfDid: "did:wba:me",
    body: { type: "room/msg", recipients: ["did:wba:s", "did:wba:me"], roster_digest: "x" }, state, localDigest: "x" });
  assert.equal(r.accept, false);
  assert.equal(r.reason, "sender-not-in-roster");
});
test("flags drift when recipients/digest mismatch local derivation", () => {
  const r = roomReceiveCheck({ senderDid: "did:wba:s", selfDid: "did:wba:me",
    body: { type: "room/msg", recipients: ["did:wba:s"], roster_digest: "WRONG" }, state, localDigest: "x" });
  assert.equal(r.accept, true);
  assert.equal(r.drift, true);
});
test("accepts a clean room/msg from a member addressed to me", () => {
  const r = roomReceiveCheck({ senderDid: "did:wba:s", selfDid: "did:wba:me",
    body: { type: "room/msg", recipients: ["did:wba:s", "did:wba:me"], roster_digest: "x" }, state, localDigest: "x" });
  assert.equal(r.accept, true);
  assert.equal(r.drift, false);
});

test("createFounderBindingOk: self-asserted founder key matching the real AIR key binds OK", () => {
  const op = { type: "room/create", founder_did: "did:wba:f", founder_pubkey: "zRealFounderKey" };
  assert.equal(createFounderBindingOk(op, "zRealFounderKey"), true);
});
test("createFounderBindingOk: a substituted attacker key does NOT bind (key-substitution attack)", () => {
  const op = { type: "room/create", founder_did: "did:wba:f", founder_pubkey: "zAttackerKey" };
  assert.equal(createFounderBindingOk(op, "zRealFounderKey"), false); // op claims founder_did but pins attacker's key
  assert.equal(createFounderBindingOk(op, null), false);              // founder DID unresolvable from AIR ⇒ drop
});
