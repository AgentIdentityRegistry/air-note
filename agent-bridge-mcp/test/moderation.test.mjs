import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  isBlocked, block, unblock, listBlocked, recordBlockedDrops, loadBlocklist, reportAbuse,
} from "../src/moderation.mjs";
import { generateIdentity } from "../src/crypto.mjs";

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

const SUBJECT_DID = "did:wba:agentidentityregistry.org:agents:AIR-BAD0";
function fakeIdentity() {
  const k = generateIdentity(); // fresh Ed25519
  return {
    air_id: "AIR-ME00", air_url: "http://air.test",
    agent_secret: "s3cret", privateKey: k.privateKey,
  };
}

test("reportAbuse posts a signed, replay-keyed report and returns reported:true on 2xx", async () => {
  const real = global.fetch;
  let captured;
  global.fetch = async (url, opts) => {
    captured = { url: String(url), opts };
    return { ok: true, status: 200, json: async () => ({ status: "received" }) };
  };
  try {
    const r = await reportAbuse({ identity: fakeIdentity(), subjectDid: SUBJECT_DID });
    assert.equal(r.reported, true);
    assert.match(captured.url, /\/api\/v1\/agents\/AIR-BAD0\/abuse-reports$/);
    assert.equal(captured.opts.headers["X-Agent-Secret"], "s3cret");
    const body = JSON.parse(captured.opts.body);
    assert.equal(body.version, 1);
    assert.equal(body.report_type, "spam");
    assert.equal(body.reporter_air_id, "AIR-ME00");
    assert.equal(body.subject_air_id, "AIR-BAD0");
    assert.ok(body.report_id && body.reported_at && body.signature_multibase);
  } finally {
    global.fetch = real;
  }
});

test("reportAbuse degrades to reported:false on HTTP error and on network throw (never throws)", async () => {
  const real = global.fetch;
  try {
    global.fetch = async () => ({ ok: false, status: 404, text: async () => "no route" });
    const r404 = await reportAbuse({ identity: fakeIdentity(), subjectDid: SUBJECT_DID });
    assert.equal(r404.reported, false);

    global.fetch = async () => { throw new Error("ECONNREFUSED"); };
    const rNet = await reportAbuse({ identity: fakeIdentity(), subjectDid: SUBJECT_DID });
    assert.equal(rNet.reported, false);
  } finally {
    global.fetch = real;
  }
});

test("reportAbuse refuses to report yourself (no fetch)", async () => {
  const real = global.fetch;
  let called = false;
  global.fetch = async () => { called = true; return { ok: true, status: 200, json: async () => ({}) }; };
  try {
    const id = fakeIdentity();
    const selfDid = "did:wba:agentidentityregistry.org:agents:" + id.air_id;
    const r = await reportAbuse({ identity: id, subjectDid: selfDid });
    assert.equal(r.reported, false);
    assert.equal(called, false);
  } finally {
    global.fetch = real;
  }
});

test("reportAbuse never throws even if signing fails (bad key) — degrades to reported:false", async () => {
  const real = global.fetch;
  let called = false;
  global.fetch = async () => { called = true; return { ok: true, status: 200, json: async () => ({}) }; };
  try {
    const badId = { air_id: "AIR-ME00", air_url: "http://air.test", agent_secret: "s", privateKey: null };
    const r = await reportAbuse({ identity: badId, subjectDid: SUBJECT_DID });
    assert.equal(r.reported, false); // degraded, did NOT throw
    assert.equal(called, false);     // failed at signing, before fetch
  } finally {
    global.fetch = real;
  }
});
