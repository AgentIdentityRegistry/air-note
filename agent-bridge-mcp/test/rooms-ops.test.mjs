import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { roomCreateLocal, roomGrantAdminLocal, roomInviteLocal } from "../src/core.mjs";
import { deriveRoom } from "../src/rooms.mjs";

let dir;
beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "air-msg-rooms-ops-"));
  process.env.AGENT_BRIDGE_HOME = dir;
});
afterEach(() => {
  rmSync(dir, { recursive: true, force: true });
});

const stubSigner = (b) => ({ ...b, op_sig: "zSIG" });

test("create -> grant admin -> admin invite derives the expected room", () => {
  const founder = { did: "did:wba:f", public_key_multibase: "zF", privateKey: null };
  const { room_id } = roomCreateLocal({ identity: founder, name: "Lab", signer: stubSigner });
  const { mandate_id } = roomGrantAdminLocal({ identity: founder, room_id, holder_did: "did:wba:a", holder_pubkey: "zA", signer: stubSigner });
  roomInviteLocal({ identity: { did: "did:wba:a", public_key_multibase: "zA" }, room_id,
    member_did: "did:wba:m", member_pubkey: "zM", mandate_id, signer: stubSigner });
  const s = deriveRoom(room_id);
  assert.equal(s.members.some((x) => x.did === "did:wba:m"), true);
  assert.equal(s.members.find((x) => x.did === "did:wba:m").kind, "agent"); // admin add ⇒ agent
});

test("founder invite can confer kind:human", () => {
  const founder = { did: "did:wba:f2", public_key_multibase: "zF2", privateKey: null };
  const { room_id } = roomCreateLocal({ identity: founder, name: "Circle", signer: stubSigner });
  roomInviteLocal({ identity: founder, room_id, member_did: "did:wba:p", member_pubkey: "zP", kind: "human", signer: stubSigner });
  assert.equal(deriveRoom(room_id).members.find((x) => x.did === "did:wba:p").kind, "human");
});

test("founder is a first-class member after create (so they receive room mail)", () => {
  const founder = { did: "did:wba:f3", public_key_multibase: "zF3", privateKey: null };
  const { room_id } = roomCreateLocal({ identity: founder, name: "Solo", signer: stubSigner });
  const s = deriveRoom(room_id);
  assert.equal(s.members.some((m) => m.did === founder.did), true);
  assert.equal(s.members.find((m) => m.did === founder.did).kind, "human");
});
