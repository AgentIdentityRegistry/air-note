import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  archiveMessage, history, threads, getCursor, setCursor, closeArchive,
} from "../src/archive.mjs";

let dir;
beforeEach(() => {
  closeArchive();
  dir = mkdtempSync(join(tmpdir(), "air-msg-arch-"));
  process.env.AGENT_BRIDGE_HOME = dir;
});
afterEach(() => {
  closeArchive();
  rmSync(dir, { recursive: true, force: true });
});

const rec = (over = {}) => ({
  envelope_id: "e1", direction: "received", thread_id: "t1",
  peer_did: "did:wba:x:agents:AIR-PEER", from_did: "did:wba:x:agents:AIR-PEER",
  to_did: "did:wba:x:agents:AIR-ME", timestamp: "2026-06-01T00:00:00Z",
  body: { type: "text", text: "hi" }, encrypted: true, verified: true, relay_seq: 5,
  ...over,
});

test("archiveMessage stores a message and history reads it back", () => {
  assert.deepEqual(archiveMessage(rec()), { inserted: true });
  const rows = history();
  assert.equal(rows.length, 1);
  assert.deepEqual(rows[0].body, { type: "text", text: "hi" });
  assert.equal(rows[0].encrypted, true);
  assert.equal(rows[0].verified, true);
  assert.equal(rows[0].direction, "received");
});

test("dedup: same (envelope_id, direction) inserts once", () => {
  assert.equal(archiveMessage(rec()).inserted, true);
  assert.equal(archiveMessage(rec()).inserted, false);
  assert.equal(history().length, 1);
});

test("self-message: same envelope_id, different direction → two rows", () => {
  const base = rec({ envelope_id: "self1", peer_did: "did:me", from_did: "did:me", to_did: "did:me" });
  assert.equal(archiveMessage({ ...base, direction: "sent" }).inserted, true);
  assert.equal(archiveMessage({ ...base, direction: "received", relay_seq: 9 }).inserted, true);
  assert.equal(history().length, 2);
});

test("history filters by peer and thread and respects limit, newest-first", () => {
  archiveMessage(rec({ envelope_id: "a", peer_did: "did:A", thread_id: "tA", timestamp: "2026-06-01T00:00:01Z" }));
  archiveMessage(rec({ envelope_id: "b", peer_did: "did:A", thread_id: "tA", timestamp: "2026-06-01T00:00:03Z" }));
  archiveMessage(rec({ envelope_id: "c", peer_did: "did:B", thread_id: "tB", timestamp: "2026-06-01T00:00:02Z" }));
  assert.equal(history({ peer: "did:A" }).length, 2);
  assert.equal(history({ thread: "tB" }).length, 1);
  assert.equal(history({ limit: 1 }).length, 1);
  assert.equal(history({ peer: "did:A" })[0].envelope_id, "b"); // newest-first
});

test("cursor get/set is monotonic", () => {
  assert.equal(getCursor(), 0);
  setCursor(10); assert.equal(getCursor(), 10);
  setCursor(5);  assert.equal(getCursor(), 10);
  setCursor(20); assert.equal(getCursor(), 20);
});

test("threads lists conversations with counts, newest activity first", () => {
  archiveMessage(rec({ envelope_id: "a", thread_id: "tA", peer_did: "did:A", timestamp: "2026-06-01T00:00:01Z" }));
  archiveMessage(rec({ envelope_id: "b", thread_id: "tB", peer_did: "did:B", timestamp: "2026-06-01T00:00:05Z" }));
  archiveMessage(rec({ envelope_id: "c", thread_id: "tA", peer_did: "did:A", timestamp: "2026-06-01T00:00:02Z" }));
  const t = threads();
  assert.equal(t.length, 2);
  assert.equal(t[0].thread_id, "tB");
  assert.equal(t.find((x) => x.thread_id === "tA").count, 2);
});
