// moderation.mjs — block / report-spam moderation state.
//
// Block is a DID-keyed JSON store at ~/.air-msg/blocklist.json (mode 0600),
// mirroring contacts.mjs. It is enforced at the single core.receive() chokepoint.
//
// IMPORTANT: block is a CONVENIENCE filter, not a security boundary — the relay
// does not authenticate the sender (sender_did = the sender-controlled
// envelope.from). See docs/superpowers/specs/2026-06-02-moderation-design.md (D12).
//
// This module must NOT import core.mjs (core imports this — circular). Callers
// resolve alias/AIR-id → canonical DID and pass a DID in.

import { existsSync, mkdirSync, readFileSync, writeFileSync, chmodSync } from "node:fs";
import { join } from "node:path";
import { randomUUID } from "node:crypto";
import { signRaw, jcsCanonical } from "./crypto.mjs";
import { bridgeHome } from "./identity.mjs";

const BLOCKLIST_VERSION = 1;
const ABUSE_REPORT_VERSION = 1;
const blocklistPath = () => join(bridgeHome(), "blocklist.json");

/** Extract an AIR id from a DID (local copy — must not depend on core.mjs). */
function airIdFromDid(didOrId) {
  const m = String(didOrId).match(/AIR-[A-Za-z0-9-]+/);
  return m ? m[0] : null;
}

export function loadBlocklist() {
  const p = blocklistPath();
  if (!existsSync(p)) return { version: BLOCKLIST_VERSION, blocked: {} };
  const raw = JSON.parse(readFileSync(p, "utf8"));
  if (!raw || typeof raw.blocked !== "object" || Array.isArray(raw.blocked)) {
    throw new SyntaxError("blocklist: unexpected shape");
  }
  return raw;
}

function saveBlocklist(store) {
  mkdirSync(bridgeHome(), { recursive: true, mode: 0o700 });
  const p = blocklistPath();
  writeFileSync(p, JSON.stringify(store, null, 2), { mode: 0o600 });
  chmodSync(p, 0o600);
}

/** Is this DID blocked? Fails OPEN (returns false) on any read error — a corrupt
 *  blocklist must never silently black-hole all mail (D6). */
export function isBlocked(did) {
  try {
    return !!loadBlocklist().blocked[did];
  } catch {
    return false;
  }
}

/** Block a canonical DID. Idempotent; preserves the original blocked_at. */
export function block(did, { alias = null } = {}) {
  const store = loadBlocklist();
  const prior = store.blocked[did];
  store.blocked[did] = {
    air_id: airIdFromDid(did),
    alias: alias ?? prior?.alias ?? null,
    blocked_at: prior?.blocked_at ?? new Date().toISOString(),
    drop_count: prior?.drop_count ?? 0,
    last_drop_at: prior?.last_drop_at ?? null,
  };
  saveBlocklist(store);
  const e = store.blocked[did];
  return { did, air_id: e.air_id, alias: e.alias, already: !!prior };
}

export function unblock(did) {
  const store = loadBlocklist();
  if (!store.blocked[did]) return { removed: false };
  delete store.blocked[did];
  saveBlocklist(store);
  return { removed: true };
}

export function listBlocked() {
  return Object.entries(loadBlocklist().blocked).map(([did, e]) => ({ did, ...e }));
}

/** Bump per-DID drop tallies in ONE write. countsByDid: Map<did, count>.
 *  Best-effort: a failed tally must never break receive(). Advisory only (D3). */
export function recordBlockedDrops(countsByDid) {
  if (!countsByDid || countsByDid.size === 0) return;
  try {
    const store = loadBlocklist();
    const now = new Date().toISOString();
    for (const [did, n] of countsByDid) {
      const e = store.blocked[did];
      if (!e) continue; // unblocked between the receive-loop check and here
      e.drop_count = (e.drop_count ?? 0) + n;
      e.last_drop_at = now;
    }
    saveBlocklist(store);
  } catch (err) {
    process.stderr.write(`[blocklist] drop-tally write failed: ${err.message ?? err}\n`);
  }
}

/** Spec §7 seam: build + sign a private abuse report and POST it. Always best-effort —
 *  any failure returns {reported:false} and never throws (the local spam-hide already
 *  applied). report_id + version make the signed report replay-safe + versionable. */
export async function reportAbuse({ identity, subjectDid, report_type = "spam",
  log = (s) => process.stderr.write(s + "\n") }) {
  if (!identity?.air_id) return { reported: false, reason: "identity has no air_id" };
  const subject_air_id = airIdFromDid(subjectDid);
  if (!subject_air_id) return { reported: false, reason: "no AIR id in subject" };
  if (subject_air_id === identity.air_id) return { reported: false, reason: "cannot report yourself" };

  const payload = {
    report_id: randomUUID(),
    version: ABUSE_REPORT_VERSION,
    reporter_air_id: identity.air_id,
    subject_air_id,
    report_type,
    reported_at: new Date().toISOString(),
  };

  try {
    const signature_multibase = signRaw(Buffer.from(jcsCanonical(payload), "utf8"), identity.privateKey);
    const resp = await fetch(`${identity.air_url}/api/v1/agents/${subject_air_id}/abuse-reports`, {
      method: "POST",
      headers: { "content-type": "application/json", "X-Agent-Secret": identity.agent_secret },
      body: JSON.stringify({ ...payload, signature_multibase }),
    });
    if (!resp.ok) {
      log(`[abuse-report] ${subject_air_id} → HTTP ${resp.status} (kept local hide)`);
      return { reported: false, reason: `HTTP ${resp.status}` };
    }
    return { reported: true };
  } catch (e) {
    log(`[abuse-report] ${subject_air_id} → ${e.message ?? e} (kept local hide)`);
    return { reported: false, reason: String(e.message ?? e) };
  }
}
