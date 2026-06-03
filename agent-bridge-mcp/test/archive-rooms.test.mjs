import { test } from "node:test";
import assert from "node:assert/strict";
import { archiveMessage, isArchived, history, closeArchive } from "../src/archive.mjs";

test("room_id is stored and history filters by it; isArchived guards replay", () => {
  closeArchive();
  const eid = `e-room-${Date.now()}`;
  const rid = `r-${Date.now()}`;
  const rec = { envelope_id: eid, direction: "received", thread_id: "t1", peer_did: "did:wba:s",
    from_did: "did:wba:s", to_did: "did:wba:me", timestamp: new Date().toISOString(),
    body: { type: "room/msg", text: "hi" }, encrypted: true, verified: true, room_id: rid };
  assert.equal(isArchived(eid, "received"), false);
  archiveMessage(rec);
  assert.equal(isArchived(eid, "received"), true);
  const rows = history({ room: rid });
  assert.equal(rows.some((r) => r.envelope_id === eid), true);
});
