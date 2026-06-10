import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { rowToMessage, makeReplayer } from "../src/channel-replay.mjs";

let dir;
beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "air-msg-replay-"));
  process.env.AGENT_BRIDGE_HOME = dir;
});
afterEach(() => {
  rmSync(dir, { recursive: true, force: true });
});

const row = (over = {}) => ({
  envelope_id: "e1", direction: "received", thread_id: "t1",
  peer_did: "did:wba:p", from: "did:wba:p", to: "did:wba:me", timestamp: "2026-06-10T00:00:00.000Z",
  body: { type: "text", text: "hi" }, encrypted: true, verified: true, key_changed: false,
  spam: false, relay_seq: 41, room_id: undefined, archived_at: "2026-06-10T00:00:01.000Z", ...over,
});

test("rowToMessage: maps an archive row to the wire/live message shape, re-deriving contact from CURRENT pins", () => {
  const m = rowToMessage(row(), { contactLookup: (did) => (did === "did:wba:p" ? { alias: "pat" } : undefined) });
  assert.equal(m.envelope_id, "e1");
  assert.equal(m.from, "did:wba:p");
  assert.equal(m.contact, "pat");                  // current pin state, not a stored alias
  assert.equal(m.seq, 41);
  assert.equal(m.relay_seq, 41);
  assert.equal(m.verified, true);
  assert.equal(m.key_changed, undefined);          // false in the row → field omitted (live shape)
  assert.equal(m.received_at, "2026-06-10T00:00:00.000Z");
  assert.deepEqual(m.body, { type: "text", text: "hi" });
});

test("rowToMessage: an unpinned sender yields no contact (the channel gate will withhold it)", () => {
  const m = rowToMessage(row(), { contactLookup: () => undefined });
  assert.equal(m.contact, undefined);
});

test("rowToMessage: a key-changed row carries key_changed:true (gate withholds it, as live did)", () => {
  const m = rowToMessage(row({ key_changed: true }), { contactLookup: () => ({ alias: "pat" }) });
  assert.equal(m.key_changed, true);
});

test("makeReplayer: replays rows after the gap through push, dedupes envelope_ids across replay+live", async () => {
  const pushed = [];
  const rows = [row({ envelope_id: "eA", relay_seq: 42 }), row({ envelope_id: "eB", relay_seq: 43 })];
  const replayer = makeReplayer({
    push: (m) => pushed.push(m.envelope_id),
    replaySinceFn: (s) => rows.filter((r) => r.relay_seq > s),
    contactLookup: () => ({ alias: "pat" }),
    isBlockedFn: () => false,
  });
  replayer.live({ envelope_id: "eA", seq: 42 });     // live frame seen BEFORE the gap fires
  await replayer.gap(41);
  assert.deepEqual(pushed, ["eA", "eB"]);            // eA pushed once (live); replay added only eB
});

test("makeReplayer: live() pushes and records; replay never double-pushes what live already pushed", async () => {
  const pushed = [];
  const replayer = makeReplayer({
    push: (m) => pushed.push(m.envelope_id),
    replaySinceFn: () => [row({ envelope_id: "eDup", relay_seq: 50 })],
    contactLookup: () => undefined,
    isBlockedFn: () => false,
  });
  replayer.live({ envelope_id: "eDup", seq: 50 });
  await replayer.gap(49);
  assert.deepEqual(pushed, ["eDup"]);                // exactly once
});

test("makeReplayer: a sender blocked AFTER archival is not replayed (live drops blocked at receive)", async () => {
  const pushed = [];
  const replayer = makeReplayer({
    push: (m) => pushed.push(m.envelope_id),
    replaySinceFn: () => [row({ envelope_id: "eBlocked", relay_seq: 60, from: "did:wba:evil" }),
                          row({ envelope_id: "eFine", relay_seq: 61 })],
    contactLookup: () => ({ alias: "pat" }),
    isBlockedFn: (did) => did === "did:wba:evil",
  });
  await replayer.gap(59);
  assert.deepEqual(pushed, ["eFine"]);     // critic H1: the blocklist holds on replay too
});

test("makeReplayer: bounded memory — the seen-set keeps only the most recent maxSeen ids", async () => {
  const replayer = makeReplayer({ push: () => {}, replaySinceFn: () => [], contactLookup: () => undefined, isBlockedFn: () => false, maxSeen: 3 });
  for (let i = 0; i < 10; i++) replayer.live({ envelope_id: `e${i}`, seq: i });
  assert.equal(replayer.seenSize(), 3);
});
