// core.mjs — shared messaging operations for ALL front-ends.
//
// The "one store, many front-ends" design: both the MCP server (index.mjs)
// and the CLI (cli.mjs) call these functions. They read/write the same
// ~/.air-msg store (identity.json, contacts.json), sign with the same key,
// and talk to the same relay. Each function returns plain data; the caller
// formats it (MCP content block, or terminal text).

import { randomUUID } from "node:crypto";
import {
  signEnvelope,
  verifyEnvelope,
  signRaw,
  jcsCanonical,
  pubKeyFromMultibase,
  pubKeyMultibase,
  sealBody,
  openBody,
  buildAad,
} from "./crypto.mjs";
import { ensureIdentity, loadIdentity, registerNewIdentity } from "./identity.mjs";
import {
  archiveMessage, getCursor, setCursor, history as archiveHistory, recentForInbox,
  markSpam, getReceived, deleteMessage, deleteConversation, isArchived,
} from "./archive.mjs";
import {
  addContact,
  listContacts,
  checkPin,
  touchContact,
  fingerprintOf,
  getContactByDid,
  loadContacts,
} from "./contacts.mjs";
import {
  isBlocked, recordBlockedDrops,
  block as blockDid, unblock as unblockDid, listBlocked, reportAbuse,
} from "./moderation.mjs";
import { getRoom, deriveState, deriveRoom, rosterDigest, nextSendSeq, nextFounderSeq, appendOp, loadRooms, saveRooms } from "./rooms.mjs";
import * as roomOps from "./room-ops.mjs";
import { verifyOp } from "./room-ops.mjs";
import { resolveAgent } from "./contacts.mjs";

export const CORE_VERSION = "0.3.0";

const ROOM_OP_MAX_AGE_MS = 48 * 60 * 60 * 1000; // 48h replay/skew horizon (spec §9.1)

export const VALID_ATTESTATION_TYPES = [
  "identity_verification",
  "operator_confirmation",
  "dependency",
  "safety_review",
];

// Resolved sender public keys, keyed by DID (TTL per spec §4.5 = 60s).
const pubKeyCache = new Map();
const PUBKEY_TTL_MS = 60_000;

// --------------------------------------------------------------------------
// Helpers
// --------------------------------------------------------------------------

/** Extract an AIR ID (AIR-XXXX-...) from a DID or accept a bare AIR ID. */
export function airIdFromDid(didOrId) {
  const m = String(didOrId).match(/AIR-[A-Za-z0-9-]+/);
  return m ? m[0] : null;
}

/** Canonical AIR did:wba host — matches the DIDs AIR mints + every DID we construct. */
const AIR_DID_HOST = "agentidentityregistry.org";

/** Build the canonical did:wba DID for a bare AIR id. */
export function didFromAirId(airId) {
  return `did:wba:${AIR_DID_HOST}:agents:${airId}`;
}

/** Resolve "to" which may be a DID, AIR ID, or a saved contact alias. */
export function resolveRecipient(to) {
  if (/^did:/.test(to)) return to;
  // A bare AIR id MUST be normalized to its canonical DID: the relay queues strictly by
  // the exact recipient string, and every recipient pulls from /inbox/<full-did>. Sending
  // to a bare AIR id would POST to a different queue and silently never be delivered.
  if (/^AIR-/.test(to)) return didFromAirId(to);
  // otherwise treat as a saved contact alias
  const c = listContacts().find((x) => x.alias === to);
  return c ? c.did : to;
}

/** Resolve an agent DID's raw Ed25519 public key via the AIR DID document. */
async function resolveAgentPublicKey(airUrl, did) {
  const cached = pubKeyCache.get(did);
  if (cached && Date.now() - cached.at < PUBKEY_TTL_MS) return cached.key;
  const airId = airIdFromDid(did);
  if (!airId) return null;
  const resp = await fetch(`${airUrl}/api/v1/agents/${airId}/did-document`);
  if (!resp.ok) return null;
  const doc = await resp.json();
  const vm = (doc.verificationMethod || []).find((m) => m.publicKeyMultibase);
  if (!vm) return null;
  let key;
  try {
    key = pubKeyFromMultibase(vm.publicKeyMultibase);
  } catch {
    return null;
  }
  pubKeyCache.set(did, { key, at: Date.now() });
  return key;
}

/** Normalize a body argument: a string becomes a {type:"text"} body; objects pass through. */
export function wrapBody(body) {
  return typeof body === "string" ? { type: "text", text: body } : body;
}

/** Build an outbound envelope; encrypts the body unless plaintext is requested. */
export function buildOutboundEnvelope({
  identity, recipientDid, recipientEd25519Pub, body, thread_id, in_reply_to, plaintext = false,
}) {
  const wrapped = wrapBody(body);
  const envelope = {
    id: randomUUID(),
    from: identity.did,
    to: recipientDid,
    timestamp: new Date().toISOString(),
    in_reply_to: in_reply_to ?? null,
    thread_id: thread_id ?? randomUUID(),
    nonce: randomUUID(),
    body: wrapped,
  };
  if (!plaintext) {
    if (!recipientEd25519Pub) {
      throw new Error(
        "cannot resolve recipient's key from AIR — refusing to send unencrypted. " +
        "Pass plaintext:true to send in the clear on purpose.",
      );
    }
    envelope.body = sealBody(wrapped, recipientEd25519Pub, buildAad(envelope));
  }
  return signEnvelope(envelope, identity.privateKey);
}

/** Given a received envelope, my seed, and whether its signature verified, surface
 *  body + encryption status. Never decrypts an unverified message, and never surfaces
 *  a raw ciphertext blob as the body (verify-before-decrypt). */
export function decodeReceivedMessage(envelope, myEd25519SeedHex, verified = true) {
  const isEncrypted = envelope?.body?.type === "encrypted";
  if (!verified) {
    return { encrypted: isEncrypted, body: isEncrypted ? undefined : envelope?.body };
  }
  if (isEncrypted) {
    try {
      const body = openBody(envelope.body, myEd25519SeedHex, buildAad(envelope));
      return { encrypted: true, body };
    } catch (e) {
      return { encrypted: true, decrypt_error: String(e.message ?? e) };
    }
  }
  return { encrypted: false, body: envelope.body };
}

/** True iff a room/create's self-asserted founder key matches the founder's real AIR key. */
export function createFounderBindingOk(op, resolvedFounderMultibase) {
  return !!resolvedFounderMultibase && op.founder_pubkey === resolvedFounderMultibase;
}

/** Roster gate + eclipse cross-check for a decrypted room/msg (spec §9.3/§9.5). Pure. */
export function roomReceiveCheck({ senderDid, selfDid, body, state, localDigest }) {
  const isMember = state.members.some((m) => m.did === senderDid);
  if (!isMember) return { accept: false, reason: "sender-not-in-roster" };
  const recipients = Array.isArray(body.recipients) ? body.recipients : [];
  const selfListed = recipients.includes(selfDid);
  const digestOk = body.roster_digest === localDigest;
  const drift = !selfListed || !digestOk;
  return { accept: true, drift };
}

/** If this control op adds ME to a room, build a synthetic "you were added" inbox notice
 *  (else null). Pure — the receive() consumer persists + surfaces it. Without this, a
 *  bootstrapped member gets ZERO inbox signal that they joined a room (they'd have to know
 *  to run `room list`); the create/add control ops are ingested silently as state. */
export function roomJoinNotice({ op, selfDid, roomName }) {
  if (op?.type !== "room/add" || op.member_did !== selfDid) return null;
  return { type: "room/joined", room_id: op.room_id, room_name: roomName ?? op.room_id };
}

/**
 * Ingest one received control op (spec §9, §6.4). The envelope signature is already
 * verified by the caller; here we verify the op's OWN `op_sig` against the authoritative
 * key for its type, then merge it into rooms.json. NEVER appends an unverified op.
 *  - room/create: bind `op.founder_pubkey` to the founder's REAL AIR key (anti key-substitution),
 *    then create the room locally + auto-pin the founder.
 *  - other founder ops (issuer === founder): key from the stored room/create's founder_pubkey.
 *  - admin room/add (issuer !== founder): key from the matching room/admin-grant's holder_pubkey.
 *    If that grant isn't present yet (prerequisite missing), skip — request/heal is Task 7.
 */
async function ingestControlOp({ op, identity }) {
  const roomId = op.room_id;
  if (!roomId) { console.error("[rooms] control op missing room_id — dropping"); return; }

  if (op.type === "room/create") {
    // Idempotency FIRST: a replayed create for a room we already have must not pay a network
    // round-trip. Surface a key-rotation/confused-state anomaly if the founder key disagrees.
    const store = loadRooms();
    const existing = store.rooms[roomId];
    if (existing) {
      const storedCreate = (existing.ops || []).find((o) => o.type === "room/create");
      if (storedCreate && storedCreate.founder_pubkey !== op.founder_pubkey) {
        console.error(`[rooms] existing room ${roomId} received a room/create with a DIFFERENT founder_pubkey — ignoring (possible confused state / key rotation)`);
      }
      return; // idempotent — no network call, no state change
    }
    if (!verifyOp(op, pubKeyFromMultibase(op.founder_pubkey))) {
      console.error(`[rooms] room/create op_sig did NOT verify for ${roomId} — dropping`);
      return;
    }
    // Root-of-trust binding: `op.founder_pubkey` is self-asserted inside the op. Bind it to the
    // founder's REAL AIR key, or a malicious inviter could plant the ATTACKER's key as the room's
    // permanent root of trust under the victim's DID (then forge remove/halt/admin-grant as founder).
    const resolvedRaw = await resolveAgentPublicKey(identity.air_url, op.founder_did);
    if (!createFounderBindingOk(op, resolvedRaw ? pubKeyMultibase(resolvedRaw) : null)) {
      console.error(`[rooms] room/create founder_pubkey does NOT match ${op.founder_did}'s AIR key — dropping (possible key-substitution attack)`);
      return;
    }
    store.rooms[roomId] = {
      name: op.name, thread_id: op.thread_id, founder_did: op.founder_did, ops: [op],
      founder_seq_next: "1", send_seq_next: "0", muted: false, joined_via: "invite",
    };
    saveRooms(store);
    try {
      await addContact(identity.air_url, { to: op.founder_did });
    } catch (err) {
      console.error(`[rooms] auto-pin founder ${op.founder_did} failed (best-effort): ${err.message ?? err}`);
    }
    return;
  }

  const room = getRoom(roomId);
  if (!room) { console.error(`[rooms] ${op.type} for unknown room ${roomId} — dropping (no local room/create)`); return; }

  // Founder op: authoritative key is the founder_pubkey pinned in the stored room/create.
  if (op.issuer_did === room.founder_did) {
    const createOp = (room.ops || []).find((o) => o.type === "room/create");
    if (!createOp) { console.error(`[rooms] ${roomId} has no stored room/create — cannot verify founder op`); return; }
    if (!verifyOp(op, pubKeyFromMultibase(createOp.founder_pubkey))) {
      console.error(`[rooms] founder op ${op.type} op_sig did NOT verify for ${roomId} — dropping`);
      return;
    }
    appendOp(roomId, op);
    // Auto-pin a newly-added member so member-to-member room messages pass the pinned gate.
    // Self-adds are skipped (no value in pinning yourself).
    if (op.type === "room/add" && op.member_did !== identity.did) {
      try {
        await addContact(identity.air_url, { to: op.member_did });
      } catch (err) {
        console.error(`[rooms] auto-pin member ${op.member_did} failed (best-effort): ${err.message ?? err}`);
      }
    }
    return;
  }

  // Admin op (delegated room/add): authoritative key is the grant holder's pubkey.
  if (op.type === "room/add") {
    const grant = (room.ops || []).find((o) => o.type === "room/admin-grant" && o.mandate_id === op.mandate_id);
    if (!grant) {
      // Prerequisite grant not yet replicated locally; request/heal is Task 7 — skip, don't crash.
      console.error(`[rooms] admin room/add for ${roomId} references grant ${op.mandate_id} not yet present — skipping (Task 7)`);
      return;
    }
    if (!verifyOp(op, pubKeyFromMultibase(grant.holder_pubkey))) {
      console.error(`[rooms] admin room/add op_sig did NOT verify for ${roomId} — dropping`);
      return;
    }
    appendOp(roomId, op);
    // Auto-pin the newly-added member (admin path).
    if (op.member_did !== identity.did) {
      try {
        await addContact(identity.air_url, { to: op.member_did });
      } catch (err) {
        console.error(`[rooms] auto-pin member ${op.member_did} failed (best-effort): ${err.message ?? err}`);
      }
    }
    return;
  }

  // Any other non-founder control op type is not authorized in v1 — drop.
  console.error(`[rooms] unauthorized non-founder op ${op.type} for ${roomId} (issuer ${op.issuer_did}) — dropping`);
}

// --------------------------------------------------------------------------
// Operations
// --------------------------------------------------------------------------

export async function register({ name } = {}) {
  const existing = loadIdentity();
  if (existing) {
    return { status: "already_registered", air_id: existing.air_id, did: existing.did, name: existing.name };
  }
  const id = await registerNewIdentity({ name });
  return {
    status: "registered",
    air_id: id.air_id,
    did: id.did,
    name: id.name,
    public_key_multibase: id.public_key_multibase,
    service_endpoint_published: id.service_endpoint_published,
  };
}

export async function myStatus() {
  const identity = loadIdentity();
  if (!identity) return { registered: false };
  let air = null;
  try {
    const resp = await fetch(`${identity.air_url}/api/v1/agents/${identity.air_id}`);
    if (resp.ok) air = await resp.json();
  } catch {
    /* offline */
  }
  return {
    registered: true,
    air_id: identity.air_id,
    did: identity.did,
    name: identity.name,
    fingerprint: fingerprintOf(identity.rawPublicKey).short,
    public_key_multibase: identity.public_key_multibase,
    trust_score: air?.trust_score ?? null,
    verification_status: air?.verification_status ?? null,
    verified: air?.verification_status?.verified ?? air?.verified ?? false,
  };
}

/** POST a signed envelope to a single recipient's relay inbox. Returns the relay receipt. */
async function postEnvelope(identity, recipient, envelope) {
  const resp = await fetch(`${identity.relay_url}/inbox/${encodeURIComponent(recipient)}`, {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(envelope),
  });
  if (!resp.ok) throw new Error(`relay ${resp.status}: ${await resp.text()}`);
  return resp.json();
}

export async function send({ to, body, thread_id, in_reply_to, plaintext = false }) {
  if (!to) throw new Error("recipient (DID, AIR ID, or contact alias) is required");
  if (body === undefined || body === null) throw new Error("body is required");
  const identity = await ensureIdentity();
  const recipient = resolveRecipient(to);
  const recipientEd25519Pub = plaintext
    ? null
    : await resolveAgentPublicKey(identity.air_url, recipient);
  const envelope = buildOutboundEnvelope({
    identity, recipientDid: recipient, recipientEd25519Pub, body, thread_id, in_reply_to, plaintext,
  });
  const receipt = await postEnvelope(identity, recipient, envelope);
  try {
    archiveMessage({
      envelope_id: envelope.id,
      direction: "sent",
      thread_id: envelope.thread_id,
      peer_did: recipient,
      from_did: identity.did,
      to_did: recipient,
      timestamp: envelope.timestamp,
      body: wrapBody(body),
      encrypted: envelope.body.type === "encrypted",
      verified: true,
    });
  } catch (err) {
    // A local archive write must never shadow a successful send.
    console.error(`[archive] failed to persist sent message: ${err.message ?? err}`);
  }
  return {
    status: "sent",
    signed: true,
    encrypted: envelope.body.type === "encrypted",
    envelope_id: receipt.envelope_id,
    relay_seq: receipt.seq,
    from: identity.did,
    to: recipient,
    thread_id: envelope.thread_id,
  };
}

export async function receive({ since, limit } = {}) {
  const identity = await ensureIdentity();
  let from = since;
  if (from == null) {
    try { from = getCursor(); } catch { from = 0; } // archive unavailable → pull from the start
  }
  const params = new URLSearchParams({ since: String(from) });
  if (limit) params.set("limit", String(limit));
  const resp = await fetch(`${identity.relay_url}/pull/${encodeURIComponent(identity.did)}?${params}`);
  if (!resp.ok) throw new Error(`relay ${resp.status}: ${await resp.text()}`);
  const batch = await resp.json();

  const messages = [];
  const drops = new Map(); // batched block drop-tally (one write after the loop)
  for (const m of batch.messages) {
    if (isBlocked(m.sender_did)) {        // hard drop BEFORE decode/verify/archive (D2/D12)
      drops.set(m.sender_did, (drops.get(m.sender_did) ?? 0) + 1);
      continue;
    }
    let envelope = null;
    let verified = false;
    let verify_note = null;
    let key_changed = false;
    let contact_alias = null;
    try {
      envelope = JSON.parse(Buffer.from(m.envelope_b64, "base64").toString("utf8"));
      const pub = await resolveAgentPublicKey(identity.air_url, m.sender_did);
      if (!pub) {
        verify_note = "sender public key not resolvable from AIR";
      } else if (envelope.from !== m.sender_did) {
        verify_note = "envelope `from` does not match relay sender_did";
      } else {
        verified = verifyEnvelope(envelope, pub);
        if (!verified) {
          verify_note = "signature did NOT verify — possible forgery or tampering";
        } else {
          const pin = checkPin(m.sender_did, pubKeyMultibase(pub));
          if (pin.known) {
            contact_alias = pin.contact.alias;
            if (!pin.pinned_matches) {
              key_changed = true;
              verify_note =
                "⚠️ sender's key CHANGED since you pinned this contact — " +
                "could be key rotation OR compromise/impersonation. " +
                "Re-verify out-of-band before trusting (agent_add_contact to re-pin).";
            } else {
              touchContact(m.sender_did);
            }
          }
        }
      }
    } catch (e) {
      verify_note = `decode/verify error: ${String(e.message ?? e)}`;
    }
    let decoded = { encrypted: false, body: undefined };
    if (envelope) {
      decoded = decodeReceivedMessage(envelope, identity.seed_hex, verified);
      if (decoded.decrypt_error) {
        const note = `decrypt failed: ${decoded.decrypt_error}`;
        verify_note = verify_note ? `${verify_note}; ${note}` : note;
      }
    }

    // --- Room handling (spec §9). Branches BEFORE the 1:1 push/archive below, then
    //     `continue`s, so non-room messages stay on the existing path byte-for-byte. ---
    if (decoded.body?.type?.startsWith("room/")) {
      // The envelope signature is already verified (`verified`); an unverified room
      // body is never trusted — drop it (and never surface room state as 1:1 chat).
      if (!verified) continue;
      // Persistent replay guard (spec §9.1): a redelivered envelope is a no-op.
      if (isArchived(m.envelope_id, "received")) continue;
      // Skew guard (spec §9.1): refuse envelopes older than 48h.
      const ts = Date.parse(envelope.timestamp);
      // absent/malformed timestamp intentionally bypasses (safe failure mode)
      if (!Number.isNaN(ts) && Date.now() - ts > ROOM_OP_MAX_AGE_MS) continue;

      const body = decoded.body;
      if (body.type === "room/msg") {
        const roomId = body.room_id;
        const state = deriveRoom(roomId);
        if (!state) continue; // unknown room → treat as non-member drop
        const chk = roomReceiveCheck({
          senderDid: m.sender_did, selfDid: identity.did, body, state, localDigest: rosterDigest(state),
        });
        if (!chk.accept) continue; // roster gate: drop a room/msg from a non-member
        messages.push({
          seq: m.seq,
          from: m.sender_did,
          ...(contact_alias ? { contact: contact_alias } : {}),
          envelope_id: m.envelope_id,
          received_at: new Date(m.queued_at * 1000).toISOString(),
          verified,
          encrypted: decoded.encrypted,
          room_id: roomId,
          in_reply_to: body.in_reply_to ?? null, // sealed-body value is authoritative for rooms (Task 6)
          to: envelope.to,
          ...(chk.drift ? { drift: true } : {}),
          ...(key_changed ? { key_changed: true } : {}),
          ...(verify_note ? { verify_note } : {}),
          body, thread_id: envelope.thread_id,
        });
        try {
          // replay-guard (isArchived) already passed above
          archiveMessage({
            envelope_id: m.envelope_id, direction: "received",
            thread_id: envelope.thread_id ?? m.envelope_id,
            peer_did: m.sender_did, from_did: envelope.from ?? m.sender_did, to_did: identity.did,
            timestamp: envelope.timestamp ?? new Date(m.queued_at * 1000).toISOString(),
            body, encrypted: decoded.encrypted, verified, relay_seq: m.seq, room_id: roomId,
          });
        } catch (err) {
          console.error(`[archive] failed to persist room message ${m.envelope_id}: ${err.message ?? err}`);
        }
        continue;
      }

      // Control op (room/create|add|remove|admin-grant|admin-revoke|halt|resume|snapshot).
      // Ingest state changes; never push to the channel as chat (state, not a message).
      try {
        await ingestControlOp({ op: body, identity });
      } catch (err) {
        console.error(`[rooms] failed to ingest control op ${body.type} for ${body.room_id}: ${err.message ?? err}`);
      }
      // If THIS op added me, surface a one-time "you were added to room X" inbox notice so a
      // bootstrapped member isn't left with a silent inbox. Stable id ⇒ idempotent across
      // re-syncs (INSERT OR IGNORE + isArchived guard); archived so the inbox list shows it.
      // SECURITY: gate on ACTUAL membership after ingest — a forged/dropped room/add (bad
      // op_sig, missing admin grant, unknown room) is rejected by ingestControlOp and so
      // never lands in the derived roster, so it must not spawn a spurious join notice.
      const joinNotice = roomJoinNotice({ op: body, selfDid: identity.did, roomName: getRoom(body.room_id)?.name });
      const joinedState = joinNotice ? deriveRoom(body.room_id) : null;
      const nowMember = !!joinedState && joinedState.members.some((mm) => mm.did === identity.did);
      // NOTE: replaySince() (archive.mjs) excludes these synthetic ids via NOT LIKE '%:joined' —
      // keep this suffix in sync with that query or replayed join notices re-enter as room chat.
      const joinId = `${body.room_id}:joined`;
      if (joinNotice && nowMember && !isArchived(joinId, "received")) {
        const joinedRoom = getRoom(body.room_id);
        messages.push({
          seq: m.seq, from: m.sender_did, envelope_id: joinId,
          received_at: new Date(m.queued_at * 1000).toISOString(),
          verified, encrypted: false, room_id: body.room_id, system: "room_join", body: joinNotice,
        });
        try {
          archiveMessage({
            envelope_id: joinId, direction: "received",
            thread_id: joinedRoom?.thread_id ?? body.room_id,
            peer_did: body.room_id, from_did: body.issuer_did ?? m.sender_did, to_did: identity.did,
            timestamp: envelope.timestamp ?? new Date(m.queued_at * 1000).toISOString(),
            body: joinNotice, encrypted: false, verified, relay_seq: m.seq, room_id: body.room_id,
          });
        } catch (err) {
          console.error(`[rooms] failed to persist room-join notice for ${body.room_id}: ${err.message ?? err}`);
        }
      }
      continue;
    }

    messages.push({
      seq: m.seq,
      from: m.sender_did,
      ...(contact_alias ? { contact: contact_alias } : {}),
      envelope_id: m.envelope_id,
      received_at: new Date(m.queued_at * 1000).toISOString(),
      verified,
      encrypted: decoded.encrypted,
      ...(key_changed ? { key_changed: true } : {}),
      ...(verify_note ? { verify_note } : {}),
      ...(decoded.body && envelope ? { body: decoded.body, thread_id: envelope.thread_id } : {}),
    });
    try {
      archiveMessage({
        envelope_id: m.envelope_id,
        direction: "received",
        thread_id: envelope?.thread_id ?? m.envelope_id,
        peer_did: m.sender_did,
        from_did: envelope?.from ?? m.sender_did,
        to_did: identity.did,
        timestamp: envelope?.timestamp ?? new Date(m.queued_at * 1000).toISOString(),
        body: decoded.body ?? { type: "unavailable" },
        encrypted: decoded.encrypted,
        verified,
        relay_seq: m.seq,
        key_changed: !!key_changed,
      });
    } catch (err) {
      // A local archive write must never drop a live-delivered message.
      console.error(`[archive] failed to persist received message ${m.envelope_id}: ${err.message ?? err}`);
    }
  }
  if (drops.size) recordBlockedDrops(drops);
  // Advance the cursor past all delivered messages even if some archive writes failed:
  // the live batch was already returned to the caller; the diary is best-effort (see Task 2).
  // NOTE: receive() fetches ONE relay page per call + reports `has_more`; receiveAll() loops
  // this to completion so a backlog larger than one page is fully drained per wake.
  if (batch.messages.length) {
    try { setCursor(Math.max(...batch.messages.map((m) => m.seq))); }
    catch (err) { console.error(`[archive] failed to advance cursor: ${err.message ?? err}`); }
  }
  return {
    messages,
    count: messages.length,
    verified_count: messages.filter((x) => x.verified && !x.key_changed).length,
    cursor: batch.cursor,
    has_more: batch.has_more,
    my_did: identity.did,
  };
}

/**
 * Drain the relay pull to completion (bounded). `receive()` returns ONE page + a `has_more`
 * flag; on its own no caller loops on it, so a backlog larger than one page would sit unpulled
 * until the next unrelated wake — and the local archive (inbox/agent_receive/history) would
 * silently lag the relay. receiveAll() repeats receive() while `has_more`, returning the union
 * of every page. Bounded by `maxPages` AND by an empty page, so a misbehaving relay that keeps
 * returning `has_more: true` cannot spin forever. Each page's archive write + cursor advance
 * already happened inside receive(), so even a partial drain persists what it pulled.
 * `receiveFn` is injectable for tests.
 */
export async function receiveAll({ since, limit, maxPages = 100 } = {}, receiveFn = receive) {
  const all = [];
  let last = { messages: [], has_more: false, cursor: undefined, my_did: undefined };
  let cursorArg = since;   // first pull may use an explicit `since`; later pulls follow the cursor
  let pages = 0;
  do {
    last = await receiveFn({ since: cursorArg, limit });
    all.push(...(last.messages ?? []));
    cursorArg = undefined;  // subsequent pulls read the now-advanced persisted cursor
    pages += 1;
  } while (last.has_more && (last.messages?.length ?? 0) > 0 && pages < maxPages);
  return {
    ...last,
    messages: all,
    count: all.length,
    verified_count: all.filter((x) => x.verified && !x.key_changed).length,
    pages,
    // has_more stays true ONLY if we stopped at the page bound with a backlog still remaining.
    has_more: !!last.has_more && pages >= maxPages,
  };
}

export async function attest({ subject, attestation_type, statement = "" }) {
  if (!VALID_ATTESTATION_TYPES.includes(attestation_type)) {
    throw new Error(`attestation_type must be one of: ${VALID_ATTESTATION_TYPES.join(", ")}`);
  }
  const identity = await ensureIdentity();
  const subjectAirId = airIdFromDid(subject);
  if (!subjectAirId) throw new Error("subject must be an AIR ID or DID containing one");
  if (subjectAirId === identity.air_id) throw new Error("cannot attest yourself");

  const signed_at = new Date().toISOString();
  const canonical = jcsCanonical({
    attester_air_id: identity.air_id,
    attestation_type,
    signed_at,
    statement,
    subject_air_id: subjectAirId,
  });
  const signature_multibase = signRaw(Buffer.from(canonical, "utf8"), identity.privateKey);
  const resp = await fetch(`${identity.air_url}/api/v1/agents/${subjectAirId}/attestations`, {
    method: "POST",
    headers: { "content-type": "application/json", "X-Agent-Secret": identity.agent_secret },
    body: JSON.stringify({ attester_air_id: identity.air_id, attestation_type, signed_at, statement, signature_multibase }),
  });
  if (!resp.ok) throw new Error(`AIR attestation ${resp.status}: ${await resp.text()}`);
  return { status: "attested", subject: subjectAirId, attestation_type, result: await resp.json() };
}

export async function addContactOp({ to, alias }) {
  if (!to) throw new Error("contact DID or AIR ID is required");
  const identity = await ensureIdentity();
  const { contact, key_changed } = await addContact(identity.air_url, { to, alias });
  return {
    status: key_changed ? "re-pinned (KEY CHANGED)" : "added",
    key_changed,
    contact: {
      alias: contact.alias,
      air_id: contact.air_id,
      did: contact.did,
      name: contact.name,
      fingerprint: contact.fingerprint,
      air_verified: contact.verified_at_pin,
    },
  };
}

export async function search({ query, verified_only = false, limit = 20 }) {
  if (!query) throw new Error("query is required");
  const identity = loadIdentity();
  const airUrl = identity?.air_url || "https://agentidentityregistry.org";
  const q = query.toLowerCase();
  const found = [];
  for (let offset = 0; offset < 200 && found.length < limit; offset += 100) {
    const resp = await fetch(`${airUrl}/api/v1/agents?limit=100&offset=${offset}`);
    if (!resp.ok) break;
    const page = await resp.json();
    for (const a of page.agents || []) {
      if (!a.name || !a.name.toLowerCase().includes(q)) continue;
      if (verified_only && !a.verified) continue;
      found.push({
        air_id: a.air_id,
        name: a.name,
        air_verified: !!a.verified,
        trust_score: a.trust_score,
        did: didFromAirId(a.air_id),
      });
      if (found.length >= limit) break;
    }
    if ((page.agents || []).length < 100) break;
  }
  return { query, verified_only, count: found.length, results: found };
}

export function listContactsOp() {
  const contacts = listContacts().map((c) => ({
    alias: c.alias,
    air_id: c.air_id,
    did: c.did,
    name: c.name,
    fingerprint: c.fingerprint,
    air_verified: c.verified_at_pin,
    pinned_at: c.pinned_at,
    last_seen: c.last_seen,
    ...(c.previous_key_multibase ? { key_changed_since_first_pin: true } : {}),
  }));
  return { count: contacts.length, contacts };
}

export async function showInvite() {
  const identity = await ensureIdentity();
  const fp = fingerprintOf(identity.rawPublicKey);
  return {
    name: identity.name,
    air_id: identity.air_id,
    did: identity.did,
    fingerprint: fp.short,
    fingerprint_full: fp.sha256_b64,
    share_line: `Add me on agent-bridge: ${identity.did} (fingerprint ${fp.short})`,
  };
}

export async function health() {
  const identity = loadIdentity();
  const relayUrl = identity?.relay_url || process.env.AGENT_BRIDGE_RELAY_URL || "https://relay.agentidentityregistry.org";
  let relay = null;
  try {
    const resp = await fetch(`${relayUrl}/health`);
    relay = resp.ok ? await resp.json() : { error: `HTTP ${resp.status}` };
  } catch (e) {
    relay = { error: String(e.message ?? e) };
  }
  return {
    bridge_version: CORE_VERSION,
    registered: !!identity,
    my_did: identity?.did ?? "(not registered)",
    relay_url: relayUrl,
    relay,
  };
}

/** Read message history from the local archive. `peer` may be a DID, AIR id, or contact alias. */
export function historyOp({ peer, thread, limit = 50, before, includeSpam = false } = {}) {
  const resolvedPeer = peer ? resolveRecipient(peer) : undefined;
  const messages = archiveHistory({ peer: resolvedPeer, thread, limit, before, includeSpam });
  return { count: messages.length, messages, resolvedPeer };
}

/** Recent messages across all conversations (the inbox view), from the local archive. */
export function recentInbox({ limit = 20, includeSpam = false } = {}) {
  const messages = recentForInbox(limit, { includeSpam });
  return { count: messages.length, messages };
}

export function blockOp({ peer }) {
  if (!peer) throw new Error("peer (DID, AIR id, or alias) is required");
  const did = resolveRecipient(peer);
  const contact = getContactByDid(did);
  const r = blockDid(did, { alias: contact?.alias ?? null });
  return { status: r.already ? "already blocked" : "blocked", did: r.did, air_id: r.air_id, alias: r.alias };
}

export function unblockOp({ peer }) {
  if (!peer) throw new Error("peer (DID, AIR id, or alias) is required");
  const did = resolveRecipient(peer);
  const { removed } = unblockDid(did);
  return { status: removed ? "unblocked" : "not blocked", did };
}

export function listBlockedOp() {
  const blocked = listBlocked();
  return { count: blocked.length, blocked };
}

export async function reportSpamOp({ envelope_id }) {
  if (!envelope_id) throw new Error("envelope_id is required (copy it from inbox/history)");
  const row = getReceived(envelope_id);
  if (!row) throw new Error(`no received message with envelope_id ${envelope_id} in your diary`);
  const identity = await ensureIdentity();
  markSpam(envelope_id);                 // guaranteed local hide FIRST
  const report = await reportAbuse({ identity, subjectDid: row.peer_did });
  return {
    hidden: true,
    reported: report.reported,
    subject: airIdFromDid(row.peer_did) ?? row.peer_did,
    ...(report.reason ? { reason: report.reason } : {}),
  };
}

export async function deleteOp({ envelope_id, peer, confirm = false } = {}) {
  const hasId = !!envelope_id, hasPeer = !!peer;
  if (!hasId && !hasPeer) throw new Error("pass exactly one of envelope_id or peer — got neither");
  if (hasId && hasPeer)   throw new Error("pass exactly one of envelope_id or peer — got both");
  if (confirm !== true)   throw new Error("refusing to delete without confirm:true (CLI: pass --yes)");
  if (envelope_id) {
    const { deleted } = deleteMessage(envelope_id);
    return { deleted, scope: "message", envelope_id };
  }
  const did = resolveRecipient(peer);
  const { deleted } = deleteConversation(did);
  return { deleted, scope: "conversation", peer: did };
}

// --------------------------------------------------------------------------
// Room op local builders (network-free, injectable signer)
// --------------------------------------------------------------------------

export function roomCreateLocal({ identity, name, signer = (b) => roomOps.signOp(b, identity.privateKey) }) {
  const room_id = randomUUID(), thread_id = randomUUID();
  const createOp = signer(roomOps.buildCreate({ room_id, name, thread_id,
    founder_did: identity.did, founder_pubkey: identity.public_key_multibase, founder_seq: "0" }));
  // Founder self-add so the founder is a first-class member and RECEIVES room mail:
  // sendRoom/fanOutOp deliver to members∖{self}, so without this a reply to the whole
  // room never reaches the founder (spec §1 scene).
  const selfAdd = signer(roomOps.buildAdd({ room_id, issuer_did: identity.did, member_did: identity.did,
    member_pubkey: identity.public_key_multibase, kind: "human", founder_seq: "1" }));
  const store = loadRooms();
  store.rooms[room_id] = { name, thread_id, founder_did: identity.did, ops: [createOp, selfAdd],
    founder_seq_next: "2", send_seq_next: "0", muted: false, joined_via: "create" };
  saveRooms(store);
  return { room_id, thread_id, op: createOp };
}

export function roomGrantAdminLocal({ identity, room_id, holder_did, holder_pubkey, signer = (b) => roomOps.signOp(b, identity.privateKey) }) {
  const mandate_id = randomUUID();
  const op = signer(roomOps.buildAdminGrant({ room_id, founder_did: identity.did, mandate_id, holder_did, holder_pubkey, founder_seq: nextFounderSeq(room_id) }));
  appendOp(room_id, op);
  return { mandate_id, op };
}

export function roomInviteLocal({ identity, room_id, member_did, member_pubkey, kind, mandate_id, signer = (b) => roomOps.signOp(b, identity.privateKey) }) {
  const room = getRoom(room_id);
  const isFounder = room && room.founder_did === identity.did;
  const fields = { room_id, issuer_did: identity.did, member_did, member_pubkey };
  if (isFounder) { fields.kind = kind || "agent"; fields.founder_seq = nextFounderSeq(room_id); }
  else { fields.kind = "agent"; fields.mandate_id = mandate_id; } // admin add (deriveState forces agent regardless)
  const op = signer(roomOps.buildAdd(fields));
  appendOp(room_id, op);
  return { op };
}

export function roomKickLocal({ identity, room_id, member_did, signer = (b) => roomOps.signOp(b, identity.privateKey) }) {
  const op = signer(roomOps.buildRemove({ room_id, founder_did: identity.did, member_did, founder_seq: nextFounderSeq(room_id) }));
  appendOp(room_id, op); return { op };
}

export function roomRevokeAdminLocal({ identity, room_id, mandate_id, signer = (b) => roomOps.signOp(b, identity.privateKey) }) {
  const op = signer(roomOps.buildAdminRevoke({ room_id, founder_did: identity.did, mandate_id, founder_seq: nextFounderSeq(room_id) }));
  appendOp(room_id, op); return { op };
}

export function roomHaltLocal({ identity, room_id, resume = false, signer = (b) => roomOps.signOp(b, identity.privateKey) }) {
  const build = resume ? roomOps.buildResume : roomOps.buildHalt;
  const op = signer(build({ room_id, founder_did: identity.did, founder_seq: nextFounderSeq(room_id) }));
  appendOp(room_id, op); return { op };
}

// --------------------------------------------------------------------------
// Room op network wrappers (fan-out)
// --------------------------------------------------------------------------

/** POST a control op (as a sealed envelope body) to a set of recipient DIDs (best-effort). */
async function fanOutOp(identity, room_id, op, extraRecipients = []) {
  const state = deriveRoom(room_id);
  const recipients = new Set([...((state?.members ?? []).map((m) => m.did)), ...extraRecipients]);
  recipients.delete(identity.did);
  const thread_id = getRoom(room_id).thread_id;
  const report = [];
  for (const did of recipients) {
    try {
      const pub = await resolveAgentPublicKey(identity.air_url, did);
      const env = buildOutboundEnvelope({ identity, recipientDid: did, recipientEd25519Pub: pub, body: op, thread_id });
      await postEnvelope(identity, did, env);
      report.push({ did, ok: true });
    } catch (e) { report.push({ did, ok: false, error: String(e.message ?? e) }); }
  }
  return report;
}

/** Send a NEW member the full current op-set so they can bootstrap the room locally. */
async function bootstrapMember(identity, room_id, member_did) {
  const room = getRoom(room_id);
  const pub = await resolveAgentPublicKey(identity.air_url, member_did);
  for (const op of room.ops) {
    const env = buildOutboundEnvelope({ identity, recipientDid: member_did, recipientEd25519Pub: pub, body: op, thread_id: room.thread_id });
    await postEnvelope(identity, member_did, env);
  }
}

export async function roomCreateOp({ name }) {
  const identity = await ensureIdentity();
  const { room_id, thread_id } = roomCreateLocal({ identity, name });
  return { status: "created", room_id, thread_id };
}

export async function roomGrantAdminOp({ room_id, to }) {
  const identity = await ensureIdentity();
  const resolved = await resolveAgent(identity.air_url, to);
  if (!resolved) throw new Error(`could not resolve ${to} from AIR`);
  const { mandate_id, op } = roomGrantAdminLocal({ identity, room_id, holder_did: resolved.did, holder_pubkey: resolved.publicKeyMultibase });
  const fanout = await fanOutOp(identity, room_id, op);
  return { status: "admin-granted", mandate_id, fanout };
}

export async function roomInviteOp({ room_id, to, mandate_id, kind }) {
  const identity = await ensureIdentity();
  const resolved = await resolveAgent(identity.air_url, to);
  if (!resolved) throw new Error(`could not resolve ${to} from AIR`);

  // If admin inviter, resolve their active mandate_id from the derived state.
  let resolvedMandateId = mandate_id;
  const state = deriveRoom(room_id);
  const room = getRoom(room_id);
  const isFounder = room && room.founder_did === identity.did;
  if (!isFounder && !resolvedMandateId) {
    const adminEntry = state?.admins?.find((a) => a.holder_did === identity.did);
    if (!adminEntry) throw new Error("you are not an admin of this room");
    resolvedMandateId = adminEntry.mandate_id;
  }

  // `kind` is honored only on the founder path (roomInviteLocal forces "agent" for admin adds).
  const { op } = roomInviteLocal({ identity, room_id, member_did: resolved.did, member_pubkey: resolved.publicKeyMultibase,
    kind: kind || "agent", mandate_id: resolvedMandateId });
  // Bootstrap the new member with the full op-set. A bootstrap failure must NOT abort the
  // invite (the add op is already appended + signed) — report it instead of throwing.
  let bootstrap_ok = true, bootstrap_error;
  try {
    await bootstrapMember(identity, room_id, resolved.did);
  } catch (e) {
    bootstrap_ok = false;
    bootstrap_error = String(e.message ?? e);
    console.error(`[rooms] bootstrap of new member ${resolved.did} for ${room_id} failed: ${bootstrap_error}`);
  }
  // Auto-pin the invitee so the inviter can receive their room messages (pinned gate).
  try {
    await addContact(identity.air_url, { to: resolved.did });
  } catch (err) {
    console.error(`[rooms] auto-pin invitee ${resolved.did} failed (best-effort): ${err.message ?? err}`);
  }
  // Fan-out the add op to existing members.
  const fanout = await fanOutOp(identity, room_id, op, []);
  return { status: "invited", member_did: resolved.did, bootstrap_ok, ...(bootstrap_error ? { bootstrap_error } : {}), fanout };
}

export async function roomKickOp({ room_id, member }) {
  const identity = await ensureIdentity();
  const resolved = await resolveAgent(identity.air_url, member);
  if (!resolved) throw new Error(`could not resolve ${member} from AIR`);
  const { op } = roomKickLocal({ identity, room_id, member_did: resolved.did });
  const fanout = await fanOutOp(identity, room_id, op);
  return { status: "kicked", member_did: resolved.did, fanout };
}

export async function roomRevokeAdminOp({ room_id, mandate_id }) {
  const identity = await ensureIdentity();
  const { op } = roomRevokeAdminLocal({ identity, room_id, mandate_id });
  const fanout = await fanOutOp(identity, room_id, op);
  return { status: "admin-revoked", mandate_id, fanout };
}

export async function roomHaltOp({ room_id }) {
  const identity = await ensureIdentity();
  const { op } = roomHaltLocal({ identity, room_id, resume: false });
  const fanout = await fanOutOp(identity, room_id, op);
  return { status: "halted", room_id, fanout };
}

export async function roomResumeOp({ room_id }) {
  const identity = await ensureIdentity();
  const { op } = roomHaltLocal({ identity, room_id, resume: true });
  const fanout = await fanOutOp(identity, room_id, op);
  return { status: "resumed", room_id, fanout };
}

export function roomListOp() {
  const identity = loadIdentity();
  const myDid = identity?.did;
  const store = loadRooms();
  const entries = Object.entries(store.rooms);
  return {
    count: entries.length,
    rooms: entries.map(([room_id, r]) => {
      const state = deriveState(r.ops || []);
      return {
        room_id,
        name: r.name,
        thread_id: r.thread_id,
        member_count: state.members.length,
        halted: state.halted,
        am_founder: r.founder_did === myDid,
        joined_via: r.joined_via,
        muted: r.muted,
      };
    }),
  };
}

export function roomHistoryOp({ room_id, limit = 50 } = {}) {
  const messages = archiveHistory({ room: room_id, limit });
  return { room_id, count: messages.length, messages };
}

export async function roomRequestOp({ room_id }) {
  const identity = await ensureIdentity();
  const room = getRoom(room_id);
  if (!room) throw new Error(`unknown room ${room_id}`);
  const payload = {
    type: "room/req-ops",
    room_id,
    issuer_did: identity.did,
    have: (room.ops || []).map((o) => roomOps.opId(o)),
  };
  const op = roomOps.signOp(payload, identity.privateKey);
  // TODO(spec §6.4): responder-side req-ops handler + rate-limit
  const founderDid = room.founder_did;
  try {
    const pub = await resolveAgentPublicKey(identity.air_url, founderDid);
    const env = buildOutboundEnvelope({ identity, recipientDid: founderDid, recipientEd25519Pub: pub, body: op, thread_id: room.thread_id });
    await postEnvelope(identity, founderDid, env);
    return { status: "sent", to: founderDid };
  } catch (e) {
    return { status: "failed", to: founderDid, error: String(e.message ?? e) };
  }
}

/** Build the single room/msg body shared (identical bytes) across the whole fan-out (spec §7/§8). */
export function buildRoomMsgBody({ room_id, members, roster_digest, sender_seq, mentions, text, in_reply_to }) {
  return {
    type: "room/msg",
    room_id,
    sender_seq: String(sender_seq),
    recipients: [...members].sort(),
    roster_digest,
    mentions: mentions ?? [],
    text,
    ...(in_reply_to ? { in_reply_to } : {}),
  };
}

/** Fan-out a message to every other member of a room (spec §8). Returns a per-member report. */
export async function sendRoom({ room_id, text, in_reply_to, mentions } = {}) {
  if (!room_id) throw new Error("room_id is required");
  const identity = await ensureIdentity();
  const rawRoom = getRoom(room_id);
  if (!rawRoom) throw new Error(`unknown room ${room_id}`);
  const state = deriveState(rawRoom.ops || []);
  if (state.halted) throw new Error("room is halted — /resume before sending");
  const thread_id = rawRoom.thread_id;
  const memberDids = state.members.map((m) => m.did);
  const recipients = memberDids.filter((d) => d !== identity.did);
  const digest = rosterDigest(state);
  const sender_seq = nextSendSeq(room_id);
  const body = buildRoomMsgBody({ room_id, members: memberDids, roster_digest: digest, sender_seq, mentions, text, in_reply_to });

  const report = [];
  for (const did of recipients) {
    try {
      const pub = await resolveAgentPublicKey(identity.air_url, did);
      // Room reply-threading lives in the sealed body (consumed by raise-your-hand in Task 6),
      // not the envelope layer — so we pass thread_id but NOT in_reply_to to buildOutboundEnvelope.
      const envelope = buildOutboundEnvelope({ identity, recipientDid: did, recipientEd25519Pub: pub, body, thread_id });
      await postEnvelope(identity, did, envelope);
      report.push({ did, ok: true });
    } catch (e) {
      report.push({ did, ok: false, error: String(e.message ?? e) });
    }
  }

  // One representative sent row per fan-out (stable synthetic id), regardless of partial
  // per-member delivery failures — the sender's history({room}) shows the message exactly once.
  try {
    archiveMessage({
      envelope_id: `${room_id}:sent:${sender_seq}`,   // stable, one row per fan-out
      direction: "sent", thread_id, peer_did: room_id, to_did: room_id,
      from_did: identity.did, timestamp: new Date().toISOString(),
      body, encrypted: true, verified: true, room_id,
    });
  } catch (err) { console.error(`[archive] room send copy: ${err.message ?? err}`); }

  return { status: "sent", room_id, sender_seq: String(sender_seq), delivered: report.filter((r) => r.ok).length, report };
}
