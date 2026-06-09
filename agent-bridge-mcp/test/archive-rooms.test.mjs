import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { archiveMessage, isArchived, history, closeArchive } from "../src/archive.mjs";

let dir;
beforeEach(() => {
  closeArchive();
  dir = mkdtempSync(join(tmpdir(), "air-msg-arch-rooms-"));
  process.env.AGENT_BRIDGE_HOME = dir;
});
afterEach(() => {
  closeArchive();
  rmSync(dir, { recursive: true, force: true });
});

test("room_id is stored and history filters by it; isArchived guards replay", () => {
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
