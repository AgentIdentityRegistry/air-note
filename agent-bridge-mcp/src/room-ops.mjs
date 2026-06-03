// src/room-ops.mjs — pure builders + sign/verify for room/* control ops.
// Mirrors crypto.signEnvelope but signs into `op_sig` (so an op survives being
// forwarded inside another envelope — see spec §7). No state, no I/O.
import { createHash, sign as nodeSign, verify as nodeVerify, createPublicKey } from "node:crypto";
import bs58 from "bs58";
import { jcsCanonical } from "./crypto.mjs";

/** Canonical bytes of an op with `op_sig` removed (NFC+JCS via jcsCanonical). */
function canonicalOpBytes(op) {
  const { op_sig, ...rest } = op; void op_sig;
  return Buffer.from(jcsCanonical(rest), "utf8");
}

/** Sign an op body; returns a new op with `op_sig` (multibase z-base58btc). */
export function signOp(op, privateKey) {
  if (op.op_sig) throw new Error("op already signed");
  const sig = nodeSign(null, canonicalOpBytes(op), privateKey);
  return { ...op, op_sig: "z" + bs58.encode(sig) };
}

/** Verify an op's `op_sig` against a raw 32-byte Ed25519 public key (Buffer). */
export function verifyOp(op, rawPub) {
  if (!op.op_sig || !op.op_sig.startsWith("z")) return false;
  let sig;
  try { sig = bs58.decode(op.op_sig.slice(1)); } catch { return false; }
  if (sig.length !== 64) return false;
  const spkiPrefix = Buffer.from("302a300506032b6570032100", "hex");
  const pubKey = createPublicKey({
    key: Buffer.concat([spkiPrefix, Buffer.from(rawPub)]), format: "der", type: "spki",
  });
  try { return nodeVerify(null, canonicalOpBytes(op), pubKey, sig); } catch { return false; }
}

/** Stable op identity: sha256 over the canonical op INCLUDING op_sig (spec §7). */
export function opId(op) {
  return createHash("sha256").update(jcsCanonical(op)).digest("hex");
}

// ---- typed builders (field lists per spec §6.1). issuer_did = the signer. ----
export const buildCreate = (f) => ({ type: "room/create", room_id: f.room_id, issuer_did: f.founder_did,
  name: f.name, thread_id: f.thread_id, founder_did: f.founder_did, founder_pubkey: f.founder_pubkey, founder_seq: f.founder_seq });
export const buildAdminGrant = (f) => ({ type: "room/admin-grant", room_id: f.room_id, issuer_did: f.founder_did,
  mandate_id: f.mandate_id, holder_did: f.holder_did, holder_pubkey: f.holder_pubkey, scope: "member:add", founder_seq: f.founder_seq, ...(f.expires_at ? { expires_at: f.expires_at } : {}) });
export const buildAdminRevoke = (f) => ({ type: "room/admin-revoke", room_id: f.room_id, issuer_did: f.founder_did, mandate_id: f.mandate_id, founder_seq: f.founder_seq });
export const buildAdd = (f) => ({ type: "room/add", room_id: f.room_id, issuer_did: f.issuer_did,
  member_did: f.member_did, member_pubkey: f.member_pubkey, kind: f.kind, ...(f.mandate_id ? { mandate_id: f.mandate_id } : {}), ...(f.founder_seq != null ? { founder_seq: f.founder_seq } : {}) });
export const buildRemove = (f) => ({ type: "room/remove", room_id: f.room_id, issuer_did: f.founder_did, member_did: f.member_did, founder_seq: f.founder_seq });
export const buildHalt = (f) => ({ type: "room/halt", room_id: f.room_id, issuer_did: f.founder_did, founder_seq: f.founder_seq });
export const buildResume = (f) => ({ type: "room/resume", room_id: f.room_id, issuer_did: f.founder_did, founder_seq: f.founder_seq });
