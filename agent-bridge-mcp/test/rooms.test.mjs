import { test } from "node:test";
import assert from "node:assert/strict";
import { deriveState, rosterDigest } from "../src/rooms.mjs";

const F = "did:wba:founder", A = "did:wba:admin", M = "did:wba:m", N = "did:wba:n";
const create = { type:"room/create", room_id:"r", issuer_did:F, founder_did:F, founder_pubkey:"zF", thread_id:"t", founder_seq:"0" };
const grant  = { type:"room/admin-grant", room_id:"r", issuer_did:F, mandate_id:"g1", holder_did:A, holder_pubkey:"zA", scope:"member:add", founder_seq:"1" };
const adminAddM = { type:"room/add", room_id:"r", issuer_did:A, member_did:M, member_pubkey:"zM", kind:"human", mandate_id:"g1" }; // claims human!
const founderRemoveM = { type:"room/remove", room_id:"r", issuer_did:F, member_did:M, founder_seq:"2" };

test("convergence: any op order yields the same derived state", () => {
  const ops = [create, grant, adminAddM];
  const a = deriveState([...ops]);
  const b = deriveState([...ops].reverse());
  assert.deepEqual(a.members, b.members);
  assert.deepEqual(a.admins, b.admins);
  assert.equal(rosterDigest(a), rosterDigest(b));
});

test("admin-added member is forced kind:agent", () => {
  const s = deriveState([create, grant, adminAddM]);
  const m = s.members.find((x) => x.did === M);
  assert.equal(m.kind, "agent");
});

test("founder remove is sticky and overrides admin add", () => {
  const s = deriveState([create, grant, adminAddM, founderRemoveM]);
  assert.equal(s.members.some((x) => x.did === M), false);
});

test("revoking the admin voids that admin's adds", () => {
  const revoke = { type:"room/admin-revoke", room_id:"r", issuer_did:F, mandate_id:"g1", founder_seq:"2" };
  const s = deriveState([create, grant, adminAddM, revoke]);
  assert.equal(s.members.some((x) => x.did === M), false);
});

test("founder add rescues a member after revoke", () => {
  const revoke = { type:"room/admin-revoke", room_id:"r", issuer_did:F, mandate_id:"g1", founder_seq:"2" };
  const founderAddM = { type:"room/add", room_id:"r", issuer_did:F, member_did:M, member_pubkey:"zM", kind:"human", founder_seq:"3" };
  const s = deriveState([create, grant, adminAddM, revoke, founderAddM]);
  const m = s.members.find((x) => x.did === M);
  assert.ok(m && m.kind === "human");
});

test("founder_seq orders the founder branch deterministically, not wall-clock", () => {
  const addN = { type:"room/add", room_id:"r", issuer_did:F, member_did:N, member_pubkey:"zN", kind:"agent", founder_seq:"5" };
  const remN = { type:"room/remove", room_id:"r", issuer_did:F, member_did:N, founder_seq:"6" };
  assert.equal(deriveState([create, addN, remN]).members.some((x)=>x.did===N), false);
  assert.equal(deriveState([create, remN, addN]).members.some((x)=>x.did===N), false);
});

test("halt derives from latest founder halt/resume by seq", () => {
  const halt = { type:"room/halt", room_id:"r", issuer_did:F, founder_seq:"4" };
  const resume = { type:"room/resume", room_id:"r", issuer_did:F, founder_seq:"5" };
  assert.equal(deriveState([create, halt]).halted, true);
  // resume (seq 5) is the latest founder halt/resume op, so the room is NOT halted.
  // Array order must not change the result — it is decided purely by founder_seq.
  assert.equal(deriveState([create, halt, resume]).halted, false);
  assert.equal(deriveState([create, resume, halt]).halted, false);
  // Latest-by-seq, not by array position: here halt (seq 7) beats resume (seq 6),
  // so both array orderings yield a halted room.
  const resume6 = { type:"room/resume", room_id:"r", issuer_did:F, founder_seq:"6" };
  const halt7 = { type:"room/halt", room_id:"r", issuer_did:F, founder_seq:"7" };
  assert.equal(deriveState([create, resume6, halt7]).halted, true);
  assert.equal(deriveState([create, halt7, resume6]).halted, true);
});
