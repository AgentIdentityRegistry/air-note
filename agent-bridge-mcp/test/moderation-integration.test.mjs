import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { receive, buildOutboundEnvelope, blockOp, unblockOp, listBlockedOp, reportSpamOp, deleteOp, historyOp } from "../src/core.mjs";
import { history, closeArchive, getCursor, archiveMessage } from "../src/archive.mjs";
import { block, loadBlocklist } from "../src/moderation.mjs";
import { generateIdentity, pubKeyMultibase } from "../src/crypto.mjs";

const ME_SEED = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";
const PEER_SEED = "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6f9";
const ME_DID = "did:wba:agentidentityregistry.org:agents:AIR-ME";
const BLOCKED_DID = "did:wba:agentidentityregistry.org:agents:AIR-BLOCKED";
const OK_DID = "did:wba:agentidentityregistry.org:agents:AIR-OK";

function seedIdentity(dir) {
  writeFileSync(join(dir, "identity.json"), JSON.stringify({
    version: 1, name: "test", air_id: "AIR-ME", did: ME_DID, seed_hex: ME_SEED,
    public_key_base64url: "", public_key_multibase: "", agent_secret: "secret",
    relay_url: "http://relay.test", air_url: "http://air.test",
    service_endpoint_published: true, created_at: "2026-06-01T00:00:00Z",
  }), { mode: 0o600 });
}

let dir, realFetch;
beforeEach(() => {
  closeArchive();
  dir = mkdtempSync(join(tmpdir(), "air-msg-modint-"));
  process.env.AGENT_BRIDGE_HOME = dir;
  seedIdentity(dir);
  realFetch = global.fetch;
});
afterEach(() => {
  global.fetch = realFetch;
  closeArchive();
  rmSync(dir, { recursive: true, force: true });
  delete process.env.AGENT_BRIDGE_HOME;
});

test("receive() hard-drops a blocked sender, archives the rest, advances cursor past both, tallies", async () => {
  const me = generateIdentity(ME_SEED);
  const peer = generateIdentity(PEER_SEED);
  const env = (did) => buildOutboundEnvelope({
    identity: { did, privateKey: peer.privateKey }, recipientDid: ME_DID,
    recipientEd25519Pub: me.rawPublicKey, body: "hi",
  });
  const blockedEnv = env(BLOCKED_DID);
  const okEnv = env(OK_DID);
  const b64 = (e) => Buffer.from(JSON.stringify(e)).toString("base64");

  block(BLOCKED_DID);

  global.fetch = async (url) => {
    const u = String(url);
    if (u.includes("/pull/")) {
      const since = Number(new URL(u).searchParams.get("since"));
      const messages = since < 7 ? [
        { envelope_b64: b64(blockedEnv), sender_did: BLOCKED_DID, envelope_id: blockedEnv.id, seq: 7, queued_at: 1717200000 },
        { envelope_b64: b64(okEnv), sender_did: OK_DID, envelope_id: okEnv.id, seq: 3, queued_at: 1717200001 },
      ] : [];
      return { ok: true, json: async () => ({ messages, cursor: 0, has_more: false }) };
    }
    if (u.includes("/did-document")) {
      return { ok: true, json: async () => ({ verificationMethod: [{ publicKeyMultibase: pubKeyMultibase(peer.rawPublicKey) }] }) };
    }
    throw new Error(`unexpected fetch: ${u}`);
  };

  const r = await receive();
  assert.equal(r.count, 1);                       // only the non-blocked message returned
  assert.equal(r.messages[0].from, OK_DID);
  const rows = history({ includeSpam: true });
  assert.equal(rows.length, 1);                   // blocked one NOT archived
  assert.equal(rows[0].peer_did, OK_DID);
  assert.equal(getCursor(), 7);                   // advanced past BOTH (max seq incl. blocked)
  assert.equal(loadBlocklist().blocked[BLOCKED_DID].drop_count, 1); // tally bumped
});

test("blockOp resolves a DID and lists it; unblockOp removes it", async () => {
  const r = await blockOp({ peer: BLOCKED_DID });
  assert.equal(r.status, "blocked");
  const list = listBlockedOp();
  assert.equal(list.count, 1);
  assert.equal(list.blocked[0].did, BLOCKED_DID);
  assert.equal((await unblockOp({ peer: BLOCKED_DID })).status, "unblocked");
  assert.equal(listBlockedOp().count, 0);
});

test("reportSpamOp errors on an unknown id, hides + reports on a known received row", async () => {
  await assert.rejects(() => reportSpamOp({ envelope_id: "missing" }), /no received message/);

  archiveMessage({
    envelope_id: "junk1", direction: "received", thread_id: "t", peer_did: OK_DID,
    from_did: OK_DID, to_did: ME_DID, timestamp: "2026-06-01T00:00:00Z",
    body: { type: "text", text: "spam" }, encrypted: false, verified: true, relay_seq: 1,
  });
  global.fetch = async () => ({ ok: true, status: 200, json: async () => ({}) }); // abuse endpoint up
  const r = await reportSpamOp({ envelope_id: "junk1" });
  assert.equal(r.hidden, true);
  assert.equal(r.reported, true);
  assert.equal(r.subject, "AIR-OK");
  assert.equal(history().length, 0);                      // hidden from default inbox
  assert.equal(history({ includeSpam: true }).length, 1); // still there, flagged
});

test("deleteOp refuses without confirm and requires exactly one selector", async () => {
  await assert.rejects(() => deleteOp({ envelope_id: "x" }), /confirm/);
  await assert.rejects(() => deleteOp({ envelope_id: "x", peer: "y", confirm: true }), /exactly one/);
});

test("deleteOp deletes a conversation when confirmed", async () => {
  archiveMessage({
    envelope_id: "c1", direction: "received", thread_id: "t", peer_did: OK_DID,
    from_did: OK_DID, to_did: ME_DID, timestamp: "2026-06-01T00:00:00Z",
    body: { type: "text", text: "hi" }, encrypted: false, verified: true, relay_seq: 1,
  });
  const r = await deleteOp({ peer: OK_DID, confirm: true });
  assert.equal(r.deleted, 1);
  assert.equal(r.scope, "conversation");
});

test("deleteOp deletes a single message when envelope_id is given", async () => {
  archiveMessage({
    envelope_id: "m1", direction: "received", thread_id: "t", peer_did: OK_DID,
    from_did: OK_DID, to_did: ME_DID, timestamp: "2026-06-01T00:00:00Z",
    body: { type: "text", text: "x" }, encrypted: false, verified: true, relay_seq: 1,
  });
  const r = await deleteOp({ envelope_id: "m1", confirm: true });
  assert.equal(r.deleted, 1);
  assert.equal(r.scope, "message");
  assert.equal(history({ includeSpam: true }).length, 0);
});

test("reportSpamOp hides locally even when the abuse report fails", async () => {
  archiveMessage({
    envelope_id: "junk2", direction: "received", thread_id: "t", peer_did: OK_DID,
    from_did: OK_DID, to_did: ME_DID, timestamp: "2026-06-01T00:00:00Z",
    body: { type: "text", text: "spam" }, encrypted: false, verified: true, relay_seq: 1,
  });
  global.fetch = async () => { throw new Error("network down"); };
  const r = await reportSpamOp({ envelope_id: "junk2" });
  assert.equal(r.hidden, true);
  assert.equal(r.reported, false);
  assert.ok(r.reason);
  assert.equal(history({ includeSpam: true }).length, 1); // still marked spam
});
