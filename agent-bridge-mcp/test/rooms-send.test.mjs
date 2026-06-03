import { test } from "node:test";
import assert from "node:assert/strict";
import { buildRoomMsgBody } from "../src/core.mjs";

test("buildRoomMsgBody stamps identical cross-check fields for the whole fan-out", () => {
  const members = ["did:wba:b", "did:wba:a", "did:wba:c"]; // unsorted input
  const body = buildRoomMsgBody({ room_id: "r1", members, self: "did:wba:me",
    roster_digest: "deadbeef", sender_seq: "7", mentions: ["AIR-CODX"], text: "hello team" });
  assert.equal(body.type, "room/msg");
  assert.deepEqual(body.recipients, ["did:wba:a", "did:wba:b", "did:wba:c"]); // sorted
  assert.equal(body.roster_digest, "deadbeef");
  assert.equal(body.sender_seq, "7");
  assert.deepEqual(body.mentions, ["AIR-CODX"]);
  assert.equal(body.text, "hello team");
});
