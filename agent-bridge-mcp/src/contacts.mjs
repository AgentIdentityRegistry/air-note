// contacts.mjs — contact book + fingerprint pinning (anti-phishing defense).
//
// When you add a contact, we PIN their current public key. Every later
// message from that contact is checked against the pin: if their key has
// changed (key rotation OR account compromise OR impersonation via a forged
// AIR record), agent_receive flags it loudly so you re-verify out-of-band.
// This is the "security code changed" guarantee from Signal/WhatsApp.
//
// Store: ~/.air-msg/contacts.json (mode 0600, same dir as identity.json).

import {
  existsSync,
  mkdirSync,
  readFileSync,
  writeFileSync,
  chmodSync,
} from "node:fs";
import { join } from "node:path";
import { createHash } from "node:crypto";
import { bridgeHome } from "./identity.mjs";
import { pubKeyMultibase, pubKeyFromMultibase } from "./crypto.mjs";

const CONTACTS_VERSION = 1;
const contactsPath = () => join(bridgeHome(), "contacts.json");

/** Human-comparable fingerprint of a raw 32-byte Ed25519 public key. */
export function fingerprintOf(rawPub) {
  const hash = createHash("sha256").update(rawPub).digest("hex");
  // SSH-style compact form + a short colon-grouped prefix for eyeball compares.
  const sha256b64 = "SHA256:" + Buffer.from(hash, "hex").toString("base64").replace(/=+$/, "");
  const shortHex = hash.slice(0, 16).match(/.{2}/g).join(":"); // first 8 bytes
  return { sha256_hex: hash, sha256_b64: sha256b64, short: shortHex };
}

export function loadContacts() {
  const p = contactsPath();
  if (!existsSync(p)) return { version: CONTACTS_VERSION, contacts: {} };
  return JSON.parse(readFileSync(p, "utf8"));
}

function saveContacts(store) {
  const dir = bridgeHome();
  mkdirSync(dir, { recursive: true, mode: 0o700 });
  const p = contactsPath();
  writeFileSync(p, JSON.stringify(store, null, 2), { mode: 0o600 });
  chmodSync(p, 0o600);
}

/** Extract an AIR ID from a DID or bare AIR ID. */
function airIdFromDid(didOrId) {
  const m = String(didOrId).match(/AIR-[A-Za-z0-9-]+/);
  return m ? m[0] : null;
}

/**
 * Resolve an agent from AIR: public key (via W3C DID document) + display
 * metadata (name, verified status, trust). Returns null if unresolvable.
 */
export async function resolveAgent(airUrl, didOrId) {
  const airId = airIdFromDid(didOrId);
  if (!airId) return null;

  // Key via DID document (spec-correct, works for any did:wba).
  const docResp = await fetch(`${airUrl}/api/v1/agents/${airId}/did-document`);
  if (!docResp.ok) return null;
  const doc = await docResp.json();
  const vm = (doc.verificationMethod || []).find((m) => m.publicKeyMultibase);
  if (!vm) return null;
  let rawPublicKey;
  try {
    rawPublicKey = pubKeyFromMultibase(vm.publicKeyMultibase);
  } catch {
    return null;
  }
  const did = doc.id || didOrId;

  // Metadata via agent record (best-effort — key resolution already succeeded).
  let name = null;
  let verified = false;
  let trust_score = null;
  let username = null; // the peer's published @handle (Milestone G), if claimed
  try {
    const recResp = await fetch(`${airUrl}/api/v1/agents/${airId}`);
    if (recResp.ok) {
      const rec = await recResp.json();
      name = rec.name ?? null;
      verified = rec.verification_status?.verified ?? rec.verified ?? false;
      trust_score = rec.trust_score ?? null;
      username = rec.username ?? null;
    }
  } catch {
    /* metadata optional */
  }

  return {
    air_id: airId,
    did,
    rawPublicKey,
    publicKeyMultibase: vm.publicKeyMultibase,
    fingerprint: fingerprintOf(rawPublicKey),
    name,
    verified,
    trust_score,
    username,
  };
}

/**
 * Add (or re-pin) a contact: resolve from AIR, pin the current key.
 * If the contact already exists with a DIFFERENT pinned key, this is a
 * key-change event — we surface it rather than silently overwriting.
 */
export async function addContact(airUrl, { to, alias }) {
  const resolved = await resolveAgent(airUrl, to);
  if (!resolved) throw new Error(`could not resolve ${to} from AIR`);

  const store = loadContacts();
  const prior = store.contacts[resolved.did];
  const keyChanged =
    prior && prior.public_key_multibase !== resolved.publicKeyMultibase;

  store.contacts[resolved.did] = {
    alias: alias || prior?.alias || resolved.name || resolved.air_id,
    air_id: resolved.air_id,
    did: resolved.did,
    name: resolved.name,
    // preserve a known handle if the registry momentarily returns null; a real release self-heals on the next clean re-pin
    username: resolved.username ?? prior?.username ?? null,
    public_key_multibase: resolved.publicKeyMultibase, // PINNED
    fingerprint: resolved.fingerprint.short,
    fingerprint_full: resolved.fingerprint.sha256_hex,
    verified_at_pin: resolved.verified,
    pinned_at: prior?.pinned_at && !keyChanged ? prior.pinned_at : new Date().toISOString(),
    last_seen: prior?.last_seen ?? null,
    ...(keyChanged ? { previous_key_multibase: prior.public_key_multibase } : {}),
  };
  saveContacts(store);
  return { contact: store.contacts[resolved.did], key_changed: !!keyChanged };
}

export function getContactByDid(did) {
  return loadContacts().contacts[did] || null;
}

export function listContacts() {
  return Object.values(loadContacts().contacts);
}

export function removeContact(aliasOrDid) {
  const store = loadContacts();
  for (const [did, c] of Object.entries(store.contacts)) {
    if (did === aliasOrDid || c.alias === aliasOrDid || c.air_id === aliasOrDid) {
      delete store.contacts[did];
      saveContacts(store);
      return true;
    }
  }
  return false;
}

/** Note that we just saw a message from this contact (updates last_seen). */
export function touchContact(did) {
  const store = loadContacts();
  if (store.contacts[did]) {
    store.contacts[did].last_seen = new Date().toISOString();
    saveContacts(store);
  }
}

/**
 * Check a freshly-resolved key against a pinned contact.
 * Returns {known, pinned_matches, contact} for the receive path.
 */
export function checkPin(did, currentMultibase) {
  const c = getContactByDid(did);
  if (!c) return { known: false, pinned_matches: null, contact: null };
  return {
    known: true,
    pinned_matches: c.public_key_multibase === currentMultibase,
    contact: c,
  };
}

export { pubKeyMultibase };
