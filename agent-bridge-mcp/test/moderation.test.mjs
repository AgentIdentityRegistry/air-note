import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  isBlocked, block, unblock, listBlocked, recordBlockedDrops, loadBlocklist,
} from "../src/moderation.mjs";

const DID_A = "did:wba:agentidentityregistry.org:agents:AIR-AAAA";
const DID_B = "did:wba:agentidentityregistry.org:agents:AIR-BBBB";

let dir;
beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "air-msg-mod-"));
  process.env.AGENT_BRIDGE_HOME = dir;
});
afterEach(() => {
  rmSync(dir, { recursive: true, force: true });
  delete process.env.AGENT_BRIDGE_HOME;
});

test("block then isBlocked is true; unblock removes it", () => {
  assert.equal(isBlocked(DID_A), false);
  const r = block(DID_A, { alias: "bob" });
  assert.equal(r.already, false);
  assert.equal(r.air_id, "AIR-AAAA");
  assert.equal(isBlocked(DID_A), true);
  assert.equal(unblock(DID_A).removed, true);
  assert.equal(isBlocked(DID_A), false);
  assert.equal(unblock(DID_A).removed, false); // idempotent
});

test("block is idempotent and preserves blocked_at", () => {
  const first = block(DID_A);
  const at = loadBlocklist().blocked[DID_A].blocked_at;
  const second = block(DID_A);
  assert.equal(second.already, true);
  assert.equal(loadBlocklist().blocked[DID_A].blocked_at, at); // unchanged
});

test("isBlocked fails OPEN on a corrupt store (D6)", () => {
  writeFileSync(join(dir, "blocklist.json"), "{ this is not json");
  assert.equal(isBlocked(DID_A), false); // never throws → mail is delivered
});

test("recordBlockedDrops batches per-DID counts in a single write", () => {
  block(DID_A);
  block(DID_B);
  recordBlockedDrops(new Map([[DID_A, 3], [DID_B, 1]]));
  const s = loadBlocklist();
  assert.equal(s.blocked[DID_A].drop_count, 3);
  assert.equal(s.blocked[DID_B].drop_count, 1);
  assert.ok(s.blocked[DID_A].last_drop_at);
  recordBlockedDrops(new Map([[DID_A, 2]])); // accumulates
  assert.equal(loadBlocklist().blocked[DID_A].drop_count, 5);
});

test("recordBlockedDrops skips a DID unblocked between check and record", () => {
  recordBlockedDrops(new Map([[DID_A, 9]])); // not blocked → no-op, no throw
  assert.equal(loadBlocklist().blocked[DID_A], undefined);
});

test("listBlocked returns entries with the DID included", () => {
  block(DID_A, { alias: "bob" });
  const list = listBlocked();
  assert.equal(list.length, 1);
  assert.equal(list[0].did, DID_A);
  assert.equal(list[0].alias, "bob");
});

test("listBlocked returns [] on a fresh store", () => {
  assert.deepEqual(listBlocked(), []);
});
