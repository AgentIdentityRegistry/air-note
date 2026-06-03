import { test } from "node:test";
import assert from "node:assert/strict";
import { generateIdentity, pubKeyMultibase } from "../src/crypto.mjs";
import { signOp, verifyOp, opId, buildCreate, buildAdd } from "../src/room-ops.mjs";

test("signOp/verifyOp round-trips and detects tamper", () => {
  const id = generateIdentity();
  const body = buildCreate({ room_id: "r1", name: "Lab", thread_id: "t1",
    founder_did: id.did ?? "did:wba:x", founder_pubkey: id.publicKeyMultibase, founder_seq: "0" });
  const signed = signOp(body, id.privateKey);
  assert.equal(typeof signed.op_sig, "string");
  assert.equal(verifyOp(signed, id.rawPublicKey), true);

  const tampered = { ...signed, name: "Evil" };
  assert.equal(verifyOp(tampered, id.rawPublicKey), false);
});

test("opId is stable and includes op_sig", () => {
  const id = generateIdentity();
  const a = signOp(buildAdd({ room_id: "r1", issuer_did: "did:wba:f", member_did: "did:wba:m",
    member_pubkey: "z6MkM", kind: "agent" }), id.privateKey);
  assert.equal(opId(a), opId({ ...a }));
  assert.notEqual(opId(a), opId({ ...a, op_sig: "zDIFFERENT" }));
  assert.notEqual(opId(a), opId({ ...a, room_id: "DIFFERENT" }));
});

test("verifyOp returns false for a wrong-length rawPub (guard)", () => {
  const id = generateIdentity();
  const signed = signOp(buildAdd({ room_id: "r1", issuer_did: "did:wba:f", member_did: "did:wba:m",
    member_pubkey: "z6MkM", kind: "agent" }), id.privateKey);
  assert.equal(verifyOp(signed, Buffer.alloc(16)), false);
});
