// src/rooms.mjs — room state store + the deterministic membership merge.
// Membership is NOT a versioned list; it is a pure function of the signed op-set
// (spec §6.2). deriveState() is the heart and is intentionally I/O-free + total.
import { existsSync, mkdirSync, readFileSync, writeFileSync, chmodSync } from "node:fs";
import { join } from "node:path";
import { createHash } from "node:crypto";
import { bridgeHome } from "./identity.mjs";
import { jcsCanonical } from "./crypto.mjs";
import { opId } from "./room-ops.mjs";

const ROOMS_VERSION = 1;
const roomsPath = () => join(bridgeHome(), "rooms.json");

/** Order founder ops by their decimal-string founder_seq (numeric), tiebreak op_sig. */
function founderSeqCmp(a, b) {
  const d = Number(a.founder_seq) - Number(b.founder_seq);
  return d !== 0 ? d : String(a.op_sig ?? "").localeCompare(String(b.op_sig ?? ""));
}

/**
 * Derive {members, admins, halted} from a vetted op-set (ops already had their
 * op_sig verified at ingest — Task 5). founder_did is taken from room/create.
 * Pure + order-independent (spec §6.2). Members/admins returned sorted by did.
 */
export function deriveState(ops) {
  const createOp = ops.find((o) => o.type === "room/create");
  if (!createOp) return { members: [], admins: [], halted: false, founder_did: null };
  const founderDid = createOp.founder_did;

  const founderOps = ops.filter((o) => o.issuer_did === founderDid).sort(founderSeqCmp);

  // --- admins: latest founder op per mandate_id wins (grant vs revoke); honor expiry ---
  // Relies on founderOps being sorted ASCENDING by founder_seq: Map.set overwrites,
  // so the final assignment per mandate_id is the latest grant-vs-revoke decision.
  const mandateLatest = new Map();
  for (const o of founderOps) {
    if (o.type === "room/admin-grant" || o.type === "room/admin-revoke") mandateLatest.set(o.mandate_id, o);
  }
  const now = Date.now();
  const activeMandates = new Map();
  for (const [mid, o] of mandateLatest) {
    if (o.type !== "room/admin-grant") continue;
    if (o.expires_at && Date.parse(o.expires_at) <= now) continue;
    activeMandates.set(mid, { holder_did: o.holder_did, holder_pubkey: o.holder_pubkey });
  }
  const admins = [...activeMandates.entries()]
    .map(([mandate_id, v]) => ({ mandate_id, holder_did: v.holder_did, holder_pubkey: v.holder_pubkey }))
    .sort((a, b) => a.holder_did.localeCompare(b.holder_did));

  // --- per-member resolution ---
  const candidates = new Set(ops.filter((o) => o.type === "room/add").map((o) => o.member_did));
  const members = [];
  for (const did of candidates) {
    const founderAboutMember = founderOps
      .filter((o) => (o.type === "room/add" || o.type === "room/remove") && o.member_did === did)
      // Intentional defensive redundancy: founderOps is already sorted, but this is the
      // security heart, so re-sorting is a cheap correctness guard. Do NOT remove.
      .sort(founderSeqCmp);
    const latestF = founderAboutMember[founderAboutMember.length - 1];
    if (latestF) {
      if (latestF.type === "room/remove") continue;
      members.push({ did, kind: latestF.kind, member_pubkey: latestF.member_pubkey });
      continue;
    }
    // O(n) scan over the raw op-set per candidate. Fine for small rooms (≤15 members,
    // spec §3); callers MUST cache deriveState() and never run it inside a hot receive loop.
    const validAdminAdd = ops.find((o) =>
      o.type === "room/add" && o.member_did === did && o.issuer_did !== founderDid &&
      o.mandate_id && activeMandates.has(o.mandate_id) &&
      activeMandates.get(o.mandate_id).holder_did === o.issuer_did);
    if (validAdminAdd) members.push({ did, kind: "agent", member_pubkey: validAdminAdd.member_pubkey });
  }
  members.sort((a, b) => a.did.localeCompare(b.did));

  const haltOps = founderOps.filter((o) => o.type === "room/halt" || o.type === "room/resume");
  const halted = haltOps.length ? haltOps[haltOps.length - 1].type === "room/halt" : false;

  return { members, admins, halted, founder_did: founderDid };
}

/** Versioned digest over members + admins + halted (spec §6.3). */
export function rosterDigest(state) {
  return createHash("sha256").update(jcsCanonical({
    digest_v: 1,
    members: state.members.map((m) => m.did).sort(),
    admins: state.admins.map((a) => a.holder_did).sort(),
    halted: !!state.halted,
  })).digest("hex");
}

// --- Persistence (~/.air-msg/rooms.json, mode 0600). Mirrors contacts.mjs. ---
export function loadRooms() {
  const p = roomsPath();
  if (!existsSync(p)) return { version: ROOMS_VERSION, rooms: {} };
  return JSON.parse(readFileSync(p, "utf8"));
}
export function saveRooms(store) {
  mkdirSync(bridgeHome(), { recursive: true, mode: 0o700 });
  const p = roomsPath();
  writeFileSync(p, JSON.stringify(store, null, 2), { mode: 0o600 });
  try { chmodSync(p, 0o600); } catch { /* non-POSIX */ }
}
export function getRoom(room_id) { return loadRooms().rooms[room_id] || null; }
export function listRooms() { return Object.values(loadRooms().rooms); }

/** Next per-room outbound sequence (decimal string), persisted; never lowered. */
export function nextSendSeq(room_id) {
  const store = loadRooms();
  const room = store.rooms[room_id];
  if (!room) throw new Error(`unknown room ${room_id}`);
  const cur = Number(room.send_seq_next ?? "0");
  room.send_seq_next = String(cur + 1);
  saveRooms(store);
  return String(cur);
}

/** Derive the live state for a stored room. */
export function deriveRoom(room_id) {
  const room = getRoom(room_id);
  return room ? deriveState(room.ops || []) : null;
}

/** Append a vetted op (op_sig already verified by the caller) to a room; dedup by opId; persist. */
export function appendOp(room_id, op) {
  const store = loadRooms();
  const room = store.rooms[room_id];
  if (!room) throw new Error(`unknown room ${room_id}`);
  room.ops = room.ops || [];
  if (!room.ops.some((o) => opId(o) === opId(op))) room.ops.push(op);
  store.rooms[room_id] = room;
  saveRooms(store);
  return deriveState(room.ops);
}
