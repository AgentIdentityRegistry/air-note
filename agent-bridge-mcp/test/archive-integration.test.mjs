import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { send, receive, buildOutboundEnvelope, historyOp, recentInbox } from "../src/core.mjs";
import { history, closeArchive, getCursor, archiveMessage } from "../src/archive.mjs";
import { generateIdentity, pubKeyMultibase } from "../src/crypto.mjs";

const ME_SEED = "9d61b19deffd5a60ba844af492ec2cc44449c5697b3269197" + "03bac031cae7f60";
const ME_DID = "did:wba:agentidentityregistry.org:agents:AIR-ME";
const PEER_DID = "did:wba:agentidentityregistry.org:agents:AIR-PEER";

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
  dir = mkdtempSync(join(tmpdir(), "air-msg-int-"));
  process.env.AGENT_BRIDGE_HOME = dir;
  seedIdentity(dir);
  realFetch = global.fetch;
});
afterEach(() => {
  global.fetch = realFetch;
  closeArchive();
  rmSync(dir, { recursive: true, force: true });
});

test("send archives the outgoing message as a 'sent' row", async () => {
  global.fetch = async () => ({ ok: true, json: async () => ({ envelope_id: "X", seq: 1 }) });
  const r = await send({ to: PEER_DID, body: "hello peer", plaintext: true });
  assert.equal(r.status, "sent");
  const rows = history({ peer: PEER_DID });
  assert.equal(rows.length, 1);
  assert.equal(rows[0].direction, "sent");
  assert.deepEqual(rows[0].body, { type: "text", text: "hello peer" });
  assert.equal(rows[0].encrypted, false); // plaintext:true → not sealed
  assert.equal(rows[0].verified, true);
});

test("send still succeeds when the archive write fails (diary failure is non-fatal)", async () => {
  global.fetch = async () => ({ ok: true, json: async () => ({ envelope_id: "X", seq: 1 }) });
  mkdirSync(join(dir, "archive.db")); // make the DB path un-openable as a file → archive throws
  const r = await send({ to: PEER_DID, body: "hi", plaintext: true });
  assert.equal(r.status, "sent"); // the archive failure did not shadow the successful send
});

const PEER_SEED = "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6f9"; // RFC 8032

test("receive archives incoming messages, advances the cursor, and dedupes on re-pull", async () => {
  const me = generateIdentity(ME_SEED);
  const peer = generateIdentity(PEER_SEED);
  // Build a real signed+encrypted envelope FROM the peer TO me.
  const env = buildOutboundEnvelope({
    identity: { did: PEER_DID, privateKey: peer.privateKey },
    recipientDid: ME_DID, recipientEd25519Pub: me.rawPublicKey, body: "from peer",
  });
  const b64 = Buffer.from(JSON.stringify(env)).toString("base64");

  global.fetch = async (url) => {
    const u = String(url);
    if (u.includes("/pull/")) {
      const since = Number(new URL(u).searchParams.get("since"));
      const messages = since < 1
        ? [{ envelope_b64: b64, sender_did: PEER_DID, envelope_id: env.id, seq: 1, queued_at: 1717200000 }]
        : [];
      return { ok: true, json: async () => ({ messages, cursor: Math.max(since, 1), has_more: false }) };
    }
    if (u.includes("/did-document")) {
      return { ok: true, json: async () => ({ verificationMethod: [{ publicKeyMultibase: pubKeyMultibase(peer.rawPublicKey) }] }) };
    }
    throw new Error(`unexpected fetch: ${u}`);
  };

  const r1 = await receive();
  assert.equal(r1.count, 1);
  const rows = history({ peer: PEER_DID });
  assert.equal(rows.length, 1);
  assert.equal(rows[0].direction, "received");
  assert.deepEqual(rows[0].body, { type: "text", text: "from peer" });
  assert.equal(rows[0].verified, true);
  assert.equal(rows[0].encrypted, true);
  assert.equal(getCursor(), 1);

  // Second pull starts at the cursor → empty → no new rows.
  const r2 = await receive();
  assert.equal(r2.count, 0);
  assert.equal(history({ peer: PEER_DID }).length, 1);
});

function peerEnvelopeB64(me, peer) {
  const env = buildOutboundEnvelope({
    identity: { did: PEER_DID, privateKey: peer.privateKey },
    recipientDid: ME_DID, recipientEd25519Pub: me.rawPublicKey, body: "from peer",
  });
  return { env, b64: Buffer.from(JSON.stringify(env)).toString("base64") };
}

test("receive still returns the batch when the archive is unavailable (diary non-fatal)", async () => {
  const me = generateIdentity(ME_SEED);
  const peer = generateIdentity(PEER_SEED);
  const { env, b64 } = peerEnvelopeB64(me, peer);
  global.fetch = async (url) => {
    const u = String(url);
    if (u.includes("/pull/")) {
      const since = Number(new URL(u).searchParams.get("since"));
      const messages = since < 1 ? [{ envelope_b64: b64, sender_did: PEER_DID, envelope_id: env.id, seq: 1, queued_at: 1717200000 }] : [];
      return { ok: true, json: async () => ({ messages, cursor: Math.max(since, 1), has_more: false }) };
    }
    if (u.includes("/did-document")) return { ok: true, json: async () => ({ verificationMethod: [{ publicKeyMultibase: pubKeyMultibase(peer.rawPublicKey) }] }) };
    throw new Error(`unexpected fetch: ${u}`);
  };
  mkdirSync(join(dir, "archive.db")); // un-openable as a file → every archive op throws
  const r = await receive();
  assert.equal(r.count, 1); // message still delivered to the caller despite the broken diary
});

test("historyOp returns archived rows filtered by peer; recentInbox returns recent rows", () => {
  const mk = (over) => ({
    envelope_id: over.id, direction: "received", thread_id: "t", peer_did: over.peer,
    from_did: over.peer, to_did: ME_DID, timestamp: over.ts,
    body: { type: "text", text: over.text }, encrypted: true, verified: true, relay_seq: 1,
  });
  archiveMessage(mk({ id: "1", peer: PEER_DID, ts: "2026-06-01T00:00:01Z", text: "one" }));
  archiveMessage(mk({ id: "2", peer: PEER_DID, ts: "2026-06-01T00:00:02Z", text: "two" }));
  archiveMessage(mk({ id: "3", peer: "did:wba:agentidentityregistry.org:agents:AIR-OTHER", ts: "2026-06-01T00:00:03Z", text: "other" }));

  const h = historyOp({ peer: PEER_DID });
  assert.equal(h.count, 2);
  assert.equal(h.messages[0].body.text, "two"); // newest-first
  assert.equal(h.resolvedPeer, PEER_DID); // a DID passes through resolveRecipient unchanged

  const inbox = recentInbox({ limit: 10 });
  assert.equal(inbox.count, 3);
  assert.equal(inbox.messages[0].body.text, "other");
});

test("receive advances the cursor to max(seq) across a multi-message batch", async () => {
  const me = generateIdentity(ME_SEED);
  const peer = generateIdentity(PEER_SEED);
  const a = peerEnvelopeB64(me, peer);
  const b = peerEnvelopeB64(me, peer);
  global.fetch = async (url) => {
    const u = String(url);
    if (u.includes("/pull/")) {
      const since = Number(new URL(u).searchParams.get("since"));
      const messages = since < 7 ? [
        { envelope_b64: a.b64, sender_did: PEER_DID, envelope_id: a.env.id, seq: 3, queued_at: 1717200000 },
        { envelope_b64: b.b64, sender_did: PEER_DID, envelope_id: b.env.id, seq: 7, queued_at: 1717200001 },
      ] : [];
      return { ok: true, json: async () => ({ messages, cursor: 99, has_more: false }) }; // cursor:99 must be IGNORED
    }
    if (u.includes("/did-document")) return { ok: true, json: async () => ({ verificationMethod: [{ publicKeyMultibase: pubKeyMultibase(peer.rawPublicKey) }] }) };
    throw new Error(`unexpected fetch: ${u}`);
  };
  await receive();
  assert.equal(getCursor(), 7); // max(seq), NOT batch.cursor (99)
});
