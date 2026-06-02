// crypto.mjs — Ed25519 identity + A2A envelope signing for agent-bridge-mcp.
//
// MUST produce byte-identical canonical bytes + signatures to the Rust
// reference implementation (crates/a2a-rs/src/signing.rs) so that a
// JS-signed envelope verifies under Rust `verify_envelope` and vice-versa.
// Proven against the spec conformance vectors in test/conformance.test.mjs.
//
// Procedure (spec §5, mirrors signing.rs):
//   1. drop the `signature` field
//   2. NFC-normalize every string VALUE (not keys — matches Rust nfc_normalize)
//   3. RFC 8785 JCS canonicalization
//   4. Ed25519 sign the canonical UTF-8 bytes
//   5. signature = multibase 'z' + base58btc(raw 64 bytes)
//
// Encodings:
//   * Public key for AIR /register:  base64url(raw 32 bytes)
//   * Public key multibase (DID):    'z' + base58btc(0xed 0x01 || raw 32 bytes)
//   * Signature:                     'z' + base58btc(raw 64 bytes)
//
// KNOWN LIMITATION (spec §5.6): JS numbers are float64, so integers > 2^53
// canonicalize incorrectly — same hazard as Python's stock `jcs`. Our message
// bodies are text, so this is not hit in practice today. A future bigint-aware
// canonicalizer is needed before signing envelopes with cash amounts > 2^53.

import canonicalize from "canonicalize";
import bs58 from "bs58";
import {
  createPrivateKey,
  createPublicKey,
  generateKeyPairSync,
  sign as nodeSign,
  verify as nodeVerify,
  createCipheriv,
  createDecipheriv,
  hkdfSync,
  randomBytes,
} from "node:crypto";
import {
  edwardsToMontgomeryPub,
  edwardsToMontgomeryPriv,
  x25519,
} from "@noble/curves/ed25519.js";

// Ed25519 PKCS8 DER prefix (RFC 8410). Followed by the 32-byte seed.
const ED25519_PKCS8_PREFIX = Buffer.from("302e020100300506032b657004220420", "hex");
// Multicodec prefix for an Ed25519 public key (0xed 0x01).
const MULTICODEC_ED25519_PUB = Buffer.from([0xed, 0x01]);
// Multicodec prefix for an X25519 public key (0xec 0x01). did:key keys start "z6LS".
const MULTICODEC_X25519_PUB = Buffer.from([0xec, 0x01]);

// ---------------------------------------------------------------------------
// Canonicalization (must match signing.rs::canonical_bytes)
// ---------------------------------------------------------------------------

/** Recursively NFC-normalize string VALUES only (keys untouched — matches Rust). */
function nfcNormalize(value) {
  if (typeof value === "string") return value.normalize("NFC");
  if (Array.isArray(value)) return value.map(nfcNormalize);
  if (value && typeof value === "object") {
    const out = {};
    for (const [k, v] of Object.entries(value)) out[k] = nfcNormalize(v);
    return out;
  }
  return value;
}

/**
 * Canonical UTF-8 bytes of an envelope per spec §5: drop `signature`,
 * NFC-normalize string values, RFC 8785 JCS. Returns a Buffer.
 */
export function canonicalBytes(envelope) {
  const { signature, ...rest } = envelope; // drop signature
  void signature;
  const normalized = nfcNormalize(rest);
  const jcs = canonicalize(normalized); // RFC 8785 → string
  return Buffer.from(jcs, "utf8");
}

// ---------------------------------------------------------------------------
// Key handling
// ---------------------------------------------------------------------------

/** Build a node KeyObject private key from a 32-byte Ed25519 seed. */
function privateKeyFromSeed(seedBytes) {
  if (seedBytes.length !== 32) {
    throw new Error(`Ed25519 seed must be 32 bytes, got ${seedBytes.length}`);
  }
  const der = Buffer.concat([ED25519_PKCS8_PREFIX, Buffer.from(seedBytes)]);
  return createPrivateKey({ key: der, format: "der", type: "pkcs8" });
}

/** Raw 32-byte public key from a private KeyObject (via JWK `x`). */
function rawPublicKey(privateKey) {
  const pub = createPublicKey(privateKey);
  const jwk = pub.export({ format: "jwk" });
  return Buffer.from(jwk.x, "base64url"); // 32 bytes
}

/**
 * Generate a fresh Ed25519 identity (or rebuild one from a hex seed).
 * Returns all the encodings the rest of the stack needs.
 */
export function generateIdentity(seedHex = null) {
  let privateKey;
  let seed;
  if (seedHex) {
    seed = Buffer.from(seedHex, "hex");
    privateKey = privateKeyFromSeed(seed);
  } else {
    const kp = generateKeyPairSync("ed25519");
    privateKey = kp.privateKey;
    // recover the 32-byte seed from JWK `d` so we can persist + rebuild it
    const jwk = privateKey.export({ format: "jwk" });
    seed = Buffer.from(jwk.d, "base64url");
  }
  const rawPub = rawPublicKey(privateKey);
  return {
    seedHex: seed.toString("hex"),
    publicKeyBase64url: rawPub.toString("base64url"), // for AIR /register
    publicKeyMultibase: pubKeyMultibase(rawPub), // z6Mk… for DID docs
    privateKey, // node KeyObject (not serialized; rebuilt from seedHex)
    rawPublicKey: rawPub,
  };
}

/** Encode a raw 32-byte Ed25519 public key as multibase z-base58btc with multicodec. */
export function pubKeyMultibase(rawPub) {
  const prefixed = Buffer.concat([MULTICODEC_ED25519_PUB, Buffer.from(rawPub)]);
  return "z" + bs58.encode(prefixed);
}

/** Decode a multibase z-base58btc public key (z6Mk…) back to raw 32 bytes. */
export function pubKeyFromMultibase(mb) {
  if (!mb.startsWith("z")) throw new Error("expected multibase 'z' prefix");
  const bytes = bs58.decode(mb.slice(1));
  if (bytes[0] !== 0xed || bytes[1] !== 0x01) {
    throw new Error("not an Ed25519 multicodec key");
  }
  return Buffer.from(bytes.slice(2)); // 32 bytes
}

// ---------------------------------------------------------------------------
// Sign / verify (must match signing.rs)
// ---------------------------------------------------------------------------

/** Sign an envelope; returns a new envelope with `signature` set (multibase z-base58btc). */
export function signEnvelope(envelope, privateKey) {
  if (envelope.signature) {
    throw new Error("envelope already signed; clear `signature` before re-signing");
  }
  const canonical = canonicalBytes(envelope);
  const sig = nodeSign(null, canonical, privateKey); // Ed25519 → 64 bytes
  return { ...envelope, signature: "z" + bs58.encode(sig) };
}

/** Verify an envelope's signature against a raw 32-byte public key (Buffer). */
export function verifyEnvelope(envelope, rawPub) {
  if (!envelope.signature) throw new Error("envelope has no signature");
  const sig = bs58.decode(envelope.signature.slice(1)); // strip 'z'
  if (sig.length !== 64) throw new Error(`signature must be 64 bytes, got ${sig.length}`);
  const canonical = canonicalBytes(envelope);
  // Rebuild a public KeyObject from raw bytes via SPKI DER.
  const spkiPrefix = Buffer.from("302a300506032b6570032100", "hex");
  const pubKey = createPublicKey({
    key: Buffer.concat([spkiPrefix, Buffer.from(rawPub)]),
    format: "der",
    type: "spki",
  });
  return nodeVerify(null, canonical, pubKey, sig);
}

/** Multibase z-base58btc encode raw bytes (used for signatures elsewhere). */
export function multibaseEncode(bytes) {
  return "z" + bs58.encode(Buffer.from(bytes));
}

/**
 * Sign arbitrary bytes (e.g. a JCS-canonical attestation payload) with an
 * Ed25519 private KeyObject; returns the signature as multibase z-base58btc.
 * Used by agent_attest, which signs the AIR attestation claim rather than an
 * envelope.
 */
export function signRaw(bytes, privateKey) {
  const sig = nodeSign(null, Buffer.from(bytes), privateKey);
  return "z" + bs58.encode(sig);
}

/** RFC 8785 JCS canonical string of an arbitrary object (NFC-normalized values). */
export function jcsCanonical(obj) {
  return canonicalize(nfcNormalize(obj));
}

// ---------------------------------------------------------------------------
// X25519 key-agreement keys, DERIVED from the Ed25519 identity (spec §14.2)
// ---------------------------------------------------------------------------

/** Recipient padlock: Ed25519 public (raw 32) → X25519 public (raw 32). */
export function ed25519PubToX25519(rawEd25519Pub) {
  const buf = Buffer.from(rawEd25519Pub);
  if (buf.length !== 32) throw new Error(`Ed25519 public key must be 32 bytes, got ${buf.length}`);
  return Buffer.from(edwardsToMontgomeryPub(buf));
}

/** Own key-agreement scalar: Ed25519 seed (raw 32) → X25519 private (raw 32). */
export function ed25519SeedToX25519(rawSeed) {
  const buf = Buffer.from(rawSeed);
  if (buf.length !== 32) throw new Error(`Ed25519 seed must be 32 bytes, got ${buf.length}`);
  return Buffer.from(edwardsToMontgomeryPriv(buf));
}

/** Encode a raw 32-byte X25519 public key as multibase z-base58btc (multicodec 0xec01). */
export function x25519PubMultibase(rawPub) {
  const prefixed = Buffer.concat([MULTICODEC_X25519_PUB, Buffer.from(rawPub)]);
  return "z" + bs58.encode(prefixed);
}

/** Decode a multibase z-base58btc X25519 public key back to raw 32 bytes. */
export function x25519PubFromMultibase(mb) {
  if (!mb.startsWith("z")) throw new Error("expected multibase 'z' prefix");
  const bytes = bs58.decode(mb.slice(1));
  if (bytes[0] !== 0xec || bytes[1] !== 0x01) {
    throw new Error("not an X25519 multicodec key");
  }
  if (bytes.length !== 34) {
    throw new Error(`X25519 multibase key must decode to 34 bytes, got ${bytes.length}`);
  }
  return Buffer.from(bytes.slice(2));
}

// ---------------------------------------------------------------------------
// Sealed-box body encryption (spec §14): ephemeral-static X25519 → HKDF-SHA256
// → ChaCha20-Poly1305. encrypt-then-sign. NOT forward-secret (see design §9).
// ---------------------------------------------------------------------------

const E2E_ALG = "x25519-hkdf-sha256-chacha20poly1305";
const E2E_INFO = Buffer.from("air-msg/e2e/v1", "utf8");

/** AAD binding the sealed blob to one envelope: id\0from\0to\0thread_id (UTF-8). */
export function buildAad({ id, from, to, thread_id }) {
  const fields = { id, from, to, thread_id };
  for (const [name, val] of Object.entries(fields)) {
    if (typeof val !== "string" || val.length === 0) {
      throw new Error(`AAD field "${name}" must be a non-empty string`);
    }
    if ([...val].some((ch) => ch.charCodeAt(0) === 0)) {
      throw new Error(`AAD field "${name}" must not contain a NUL byte`);
    }
  }
  const z = Buffer.from([0]);
  return Buffer.concat([
    Buffer.from(id, "utf8"), z,
    Buffer.from(from, "utf8"), z,
    Buffer.from(to, "utf8"), z,
    Buffer.from(thread_id, "utf8"),
  ]);
}

/** Derive the per-message AEAD key from a shared secret and the ephemeral pub. */
function deriveKey(shared, ephPub) {
  const info = Buffer.concat([E2E_INFO, Buffer.from(ephPub)]);
  // Empty salt == HashLen-zeros salt (HMAC zero-pads the key to block size),
  // so this matches Rust hkdf's `Hkdf::new(None, ikm)`.
  return Buffer.from(hkdfSync("sha256", shared, Buffer.alloc(0), info, 32));
}

/** Internal: seal with a caller-supplied ephemeral key + nonce (deterministic; for vectors). */
export function sealBodyWithEphemeral(bodyObj, recipientEd25519PubRaw, aad, ephPriv, nonce) {
  const recipientX = ed25519PubToX25519(recipientEd25519PubRaw);
  const ephPub = x25519.getPublicKey(ephPriv);
  const shared = x25519.getSharedSecret(ephPriv, recipientX);
  const key = deriveKey(shared, ephPub);
  const plaintext = Buffer.from(jcsCanonical(bodyObj), "utf8");
  const cipher = createCipheriv("chacha20-poly1305", key, nonce, { authTagLength: 16 });
  cipher.setAAD(aad);
  const ct = Buffer.concat([cipher.update(plaintext), cipher.final()]);
  const tag = cipher.getAuthTag();
  return {
    type: "encrypted",
    alg: E2E_ALG,
    v: 1,
    epk: x25519PubMultibase(ephPub),
    nonce: Buffer.from(nonce).toString("base64url"),
    ct: Buffer.concat([ct, tag]).toString("base64url"),
  };
}

/** Seal a body for a recipient (fresh ephemeral key + random nonce). */
export function sealBody(bodyObj, recipientEd25519PubRaw, aad) {
  const ephPriv = x25519.utils.randomPrivateKey();
  const nonce = randomBytes(12);
  return sealBodyWithEphemeral(bodyObj, recipientEd25519PubRaw, aad, ephPriv, nonce);
}

/** Open an encrypted body using my Ed25519 seed (hex). Throws on any failure. */
export function openBody(encBody, myEd25519SeedHex, aad) {
  if (encBody.alg !== E2E_ALG) throw new Error(`unsupported alg: ${encBody.alg}`);
  const myX = ed25519SeedToX25519(Buffer.from(myEd25519SeedHex, "hex"));
  const ephPub = x25519PubFromMultibase(encBody.epk);
  const shared = x25519.getSharedSecret(myX, ephPub);
  const key = deriveKey(shared, ephPub);
  const nonce = Buffer.from(encBody.nonce, "base64url");
  if (nonce.length !== 12) throw new Error(`nonce must be 12 bytes, got ${nonce.length}`);
  const ctTag = Buffer.from(encBody.ct, "base64url");
  if (ctTag.length < 16) throw new Error("ciphertext too short");
  const tag = ctTag.subarray(ctTag.length - 16);
  const ct = ctTag.subarray(0, ctTag.length - 16);
  const decipher = createDecipheriv("chacha20-poly1305", key, nonce, { authTagLength: 16 });
  decipher.setAAD(aad);
  decipher.setAuthTag(tag);
  const plaintext = Buffer.concat([decipher.update(ct), decipher.final()]); // throws on mismatch
  return JSON.parse(plaintext.toString("utf8"));
}
