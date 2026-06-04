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
test("drift when self IS in recipients but roster_digest mismatches localDigest", () => {
  const r = roomReceiveCheck({ senderDid: "did:wba:s", selfDid: "did:wba:me",
    body: { type: "room/msg", recipients: ["did:wba:s", "did:wba:me"], roster_digest: "WRONG" }, state, localDigest: "x" });
  assert.equal(r.accept, true);
  assert.equal(r.drift, true); // digest mismatch alone flags drift
});
test("drift when self NOT in recipients even though the digest matches", () => {
  const r = roomReceiveCheck({ senderDid: "did:wba:s", selfDid: "did:wba:me",
    body: { type: "room/msg", recipients: ["did:wba:s"], roster_digest: "x" }, state, localDigest: "x" });
  assert.equal(r.accept, true);
  assert.equal(r.drift, true); // not addressed to me alone flags drift
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

// Visible gaps: the receive()-level guards are exercised by the in-repo end-to-end probe today;
// these mark the still-missing standalone unit coverage (need network/Date stubs).
test.todo("replay-guard drops a duplicate room/msg (needs relay/isArchived stub)");
test.todo("48h skew guard drops a stale room/msg (needs Date stub)");
