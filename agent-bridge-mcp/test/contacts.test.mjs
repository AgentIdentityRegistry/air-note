import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { addContact, getContactByDid } from "../src/contacts.mjs";
import { generateIdentity, pubKeyMultibase } from "../src/crypto.mjs";

const PEER_SEED = "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6f9"; // RFC 8032
const PEER_DID = "did:wba:agentidentityregistry.org:agents:AIR-PEER";
const AIR_URL = "http://air.test";

let dir, realFetch, peer;
beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "air-msg-contacts-"));
  process.env.AGENT_BRIDGE_HOME = dir;
  realFetch = global.fetch;
  peer = generateIdentity(PEER_SEED);
});
afterEach(() => {
  global.fetch = realFetch;
  rmSync(dir, { recursive: true, force: true });
  delete process.env.AGENT_BRIDGE_HOME;
});

/** Stub the two GETs resolveAgent makes: the DID document (key) + the agent record (metadata). */
function stubFetch({ username }) {
  global.fetch = async (url) => {
    const u = String(url);
    if (u.includes("/did-document")) {
      return { ok: true, json: async () => ({ id: PEER_DID, verificationMethod: [{ publicKeyMultibase: pubKeyMultibase(peer.rawPublicKey) }] }) };
    }
    if (u.endsWith("/agents/AIR-PEER")) {
      return { ok: true, json: async () => ({ name: "Kenny", username, verification_status: { verified: false } }) };
    }
    throw new Error(`unexpected fetch: ${u}`);
  };
}

test("addContact captures the peer's published @handle (username)", async () => {
  stubFetch({ username: "kenny" });
  await addContact(AIR_URL, { to: PEER_DID });
  const c = getContactByDid(PEER_DID);
  assert.equal(c.username, "kenny");
  assert.equal(c.name, "Kenny");
});

test("a missing handle is stored as null", async () => {
  stubFetch({ username: undefined });
  await addContact(AIR_URL, { to: PEER_DID });
  assert.equal(getContactByDid(PEER_DID).username, null);
});

test("re-pin preserves a known handle when the registry returns null", async () => {
  stubFetch({ username: "kenny" });
  await addContact(AIR_URL, { to: PEER_DID });
  stubFetch({ username: null });            // metadata hiccup on re-pin
  await addContact(AIR_URL, { to: PEER_DID });
  assert.equal(getContactByDid(PEER_DID).username, "kenny"); // preserved, not wiped
});
