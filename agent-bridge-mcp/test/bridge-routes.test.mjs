import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { closeArchive } from "../src/archive.mjs";
import {
  putRoute, getRoute, pruneRoutes, getUpdateOffset, setUpdateOffset,
} from "../src/bridge-routes.mjs";

let dir;
beforeEach(() => {
  closeArchive();
  dir = mkdtempSync(join(tmpdir(), "air-msg-routes-"));
  process.env.AGENT_BRIDGE_HOME = dir;
});
afterEach(() => { closeArchive(); rmSync(dir, { recursive: true, force: true }); });

const route = (over = {}) => ({
  platform: "telegram", external_id: "123", peer_did: "did:wba:x:agents:AIR-ALICE",
  contact: "alice", thread_id: "t1", envelope_id: "e1", verified: true, created_at: 1000, ...over,
});

test("put then get round-trips, with verified coerced to boolean", () => {
  putRoute(route());
  const r = getRoute({ external_id: "123" });
  assert.equal(r.peer_did, "did:wba:x:agents:AIR-ALICE");
  assert.equal(r.contact, "alice");
  assert.equal(r.thread_id, "t1");
  assert.equal(r.envelope_id, "e1");
  assert.equal(r.verified, true);
});

test("get miss returns null", () => {
  assert.equal(getRoute({ external_id: "nope" }), null);
});

test("a numeric external_id is keyed as a string", () => {
  putRoute(route({ external_id: 456 }));
  assert.ok(getRoute({ external_id: "456" }));
  assert.ok(getRoute({ external_id: 456 }));
});

test("two different external_ids for the same peer keep two routes", () => {
  putRoute(route({ external_id: "1", thread_id: "tA" }));
  putRoute(route({ external_id: "2", thread_id: "tB" }));
  assert.equal(getRoute({ external_id: "1" }).thread_id, "tA");
  assert.equal(getRoute({ external_id: "2" }).thread_id, "tB");
});

test("pruneRoutes drops routes older than maxAge", () => {
  putRoute(route({ external_id: "old", created_at: 1000 }));
  putRoute(route({ external_id: "new", created_at: 9_000_000 }));
  const removed = pruneRoutes({ now: 9_000_000, maxAgeMs: 1000 });
  assert.equal(removed >= 1, true);
  assert.equal(getRoute({ external_id: "old" }), null);
  assert.ok(getRoute({ external_id: "new" }));
});

test("pruneRoutes drops routes beyond maxRows (count cap), keeping the newest", () => {
  for (let i = 1; i <= 5; i++) putRoute(route({ external_id: String(i), created_at: 1000 + i }));
  const removed = pruneRoutes({ now: 1_000_000, maxAgeMs: 10_000_000, maxRows: 3 });
  assert.equal(removed, 2);                          // 5 routes, keep newest 3 → drop 2 oldest
  assert.equal(getRoute({ external_id: "1" }), null);
  assert.equal(getRoute({ external_id: "2" }), null);
  assert.ok(getRoute({ external_id: "5" }));          // newest kept
});

test("update offset get/set (0 when unset)", () => {
  assert.equal(getUpdateOffset(), 0);
  setUpdateOffset({ offset: 42 });
  assert.equal(getUpdateOffset(), 42);
  setUpdateOffset({ offset: 99 });
  assert.equal(getUpdateOffset(), 99);
});
