// identity.mjs — first-run identity bootstrap + AIR registration.
//
// On first use, agent-bridge-mcp:
//   1. generates an Ed25519 keypair
//   2. registers with AIR (POST /api/v1/agents/register) → AIR mints a did:wba
//   3. publishes its relay inbox as an A2AInbox service endpoint (PUT)
//   4. persists everything to ~/.air-msg/identity.json (mode 0600)
//
// On every subsequent run it loads + rebuilds the key from the stored seed.
// One install → registered + addressable, no manual DID wrangling.
//
// SECURITY (MVP): the Ed25519 seed and AIR agent_secret are stored in a
// plaintext 0600 file. Task #19 upgrades this to OS-keychain / Secure Enclave.
// Registration is "self-verified" tier — AIR Verified status is EARNED later
// via cross-org peer attestations (agent_attest), not granted at signup.

import {
  existsSync,
  mkdirSync,
  readFileSync,
  writeFileSync,
  chmodSync,
} from "node:fs";
import { homedir, userInfo, hostname } from "node:os";
import { join } from "node:path";
import { generateIdentity } from "./crypto.mjs";

const IDENTITY_VERSION = 1;
const DEFAULT_AIR_URL = "https://agentidentityregistry.org";
const DEFAULT_RELAY_URL = "https://relay.agentidentityregistry.org";

export const bridgeHome = () => {
  if (process.env.AGENT_BRIDGE_HOME) return process.env.AGENT_BRIDGE_HOME;
  // Under the node test runner, falling through to the real home would write
  // fixture data into the user's live ~/.air-msg store — force tests to opt
  // into a home explicitly (temp-dir idiom: test/archive.test.mjs).
  if (process.env.NODE_TEST_CONTEXT) {
    throw new Error(
      "Refusing the real ~/.air-msg under the test runner — set AGENT_BRIDGE_HOME to a temp dir.",
    );
  }
  return join(homedir(), ".air-msg");
};
const dir = bridgeHome;
const identityPath = () => join(dir(), "identity.json");

/** Best-effort human-friendly default name for auto-registration. */
function defaultName() {
  if (process.env.AGENT_BRIDGE_NAME) return process.env.AGENT_BRIDGE_NAME;
  let who = "agent";
  try {
    who = userInfo().username || "agent";
  } catch {
    /* sandboxes without passwd entry */
  }
  return `${who}@${hostname()} (agent-bridge)`;
}

/** Load the persisted identity, or null if none exists yet. */
export function loadIdentity() {
  const p = identityPath();
  if (!existsSync(p)) return null;
  const stored = JSON.parse(readFileSync(p, "utf8"));
  // Rebuild the private KeyObject from the stored seed — never persisted live.
  const keys = generateIdentity(stored.seed_hex);
  return { ...stored, privateKey: keys.privateKey, rawPublicKey: keys.rawPublicKey };
}

/** Persist identity to ~/.air-msg/identity.json with locked-down perms. */
function persist(record) {
  const d = dir();
  mkdirSync(d, { recursive: true, mode: 0o700 });
  const p = identityPath();
  writeFileSync(p, JSON.stringify(record, null, 2), { mode: 0o600 });
  chmodSync(p, 0o600); // enforce even if file pre-existed with looser perms
}

/**
 * Register a fresh identity with AIR and publish the relay inbox endpoint.
 * Returns the full identity (including live privateKey).
 */
export async function registerNewIdentity({ name, airUrl, relayUrl } = {}) {
  const air = (airUrl || process.env.AGENT_BRIDGE_AIR_URL || DEFAULT_AIR_URL).replace(/\/$/, "");
  const relay = (relayUrl || process.env.AGENT_BRIDGE_RELAY_URL || DEFAULT_RELAY_URL).replace(/\/$/, "");
  const agentName = name || defaultName();

  const keys = generateIdentity(); // fresh Ed25519

  // 1. Register — omit creator_did so AIR mints a did:wba anchored on our key.
  const regResp = await fetch(`${air}/api/v1/agents/register`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      name: agentName,
      public_key: keys.publicKeyBase64url, // base64url raw 32 bytes (AIR validator)
      creator_type: "individual",
      capabilities: ["a2a-messaging"],
      description: "Personal AI agent reachable via agent-bridge messaging.",
    }),
  });
  if (!regResp.ok) {
    throw new Error(`AIR register failed ${regResp.status}: ${await regResp.text()}`);
  }
  const reg = await regResp.json();
  const airId = reg.air_id;
  const did = reg.creator_did; // AIR-minted did:wba
  const agentSecret = reg.agent_secret;

  // 2. Publish our A2AInbox service endpoint so others can resolve where to
  //    send us. Best-effort: a failure here doesn't lose the identity (the
  //    record exists; the endpoint can be re-published later).
  let serviceEndpointPublished = false;
  try {
    const putResp = await fetch(`${air}/api/v1/agents/${airId}`, {
      method: "PUT",
      headers: { "content-type": "application/json", "X-Agent-Secret": agentSecret },
      body: JSON.stringify({
        service_endpoints: [
          { type: "A2AInbox", serviceEndpoint: `${relay}/inbox/${did}` },
        ],
      }),
    });
    serviceEndpointPublished = putResp.ok;
  } catch {
    serviceEndpointPublished = false;
  }

  const record = {
    version: IDENTITY_VERSION,
    name: agentName,
    air_id: airId,
    did,
    seed_hex: keys.seedHex, // SENSITIVE — Ed25519 private seed
    public_key_base64url: keys.publicKeyBase64url,
    public_key_multibase: keys.publicKeyMultibase,
    agent_secret: agentSecret, // SENSITIVE — AIR write/attest auth
    relay_url: relay,
    air_url: air,
    service_endpoint_published: serviceEndpointPublished,
    created_at: new Date().toISOString(),
  };
  persist(record);
  return { ...record, privateKey: keys.privateKey, rawPublicKey: keys.rawPublicKey };
}

/**
 * Lazy accessor: return the existing identity, or bootstrap (generate +
 * register) a new one. This is what tools call before sending/attesting.
 */
export async function ensureIdentity(opts = {}) {
  const existing = loadIdentity();
  if (existing) return existing;
  return registerNewIdentity(opts);
}

export { identityPath };
