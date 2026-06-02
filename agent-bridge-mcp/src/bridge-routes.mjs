// bridge-routes.mjs — reply-routing table for the chat-app bridge, on the shared
// archive.db handle. archive.mjs owns the FILE; this module owns the bridge_routes
// TABLE + the bridge's update-offset key in the generic `meta` table. Routes are keyed
// by the chat platform's server-assigned message id → the relay-VERIFIED sender DID, so
// nothing the sender controls can influence who a reply goes to. DDL via prepare().run()
// (the repo hook forbids db.exec).

import { openArchive } from "./archive.mjs";

// Tie "table ensured" to the actual db handle so it can never desync from archive's _db:
// any new handle (after closeArchive()) is not in the set → DDL re-runs automatically.
const _ensured = new WeakSet();
function db() {
  const d = openArchive();
  if (!_ensured.has(d)) {
    d.prepare(`CREATE TABLE IF NOT EXISTS bridge_routes (
      platform     TEXT NOT NULL,
      external_id  TEXT NOT NULL,
      peer_did     TEXT NOT NULL,
      contact      TEXT,
      thread_id    TEXT,
      envelope_id  TEXT,
      verified     INTEGER NOT NULL,
      created_at   INTEGER NOT NULL,
      PRIMARY KEY (platform, external_id)
    )`).run();
    _ensured.add(d);
  }
  return d;
}

export function putRoute({
  platform = "telegram", external_id, peer_did, contact = null,
  thread_id = null, envelope_id = null, verified, created_at,
}) {
  db().prepare(`INSERT OR REPLACE INTO bridge_routes
    (platform, external_id, peer_did, contact, thread_id, envelope_id, verified, created_at)
    VALUES (?, ?, ?, ?, ?, ?, ?, ?)`)
    .run(platform, String(external_id), peer_did, contact, thread_id, envelope_id, verified ? 1 : 0, created_at);
}

export function getRoute({ platform = "telegram", external_id }) {
  const r = db().prepare(`SELECT * FROM bridge_routes WHERE platform = ? AND external_id = ?`)
    .get(platform, String(external_id));
  if (!r) return null;
  return {
    platform: r.platform, external_id: r.external_id, peer_did: r.peer_did,
    contact: r.contact ?? null, thread_id: r.thread_id ?? null, envelope_id: r.envelope_id ?? null,
    verified: !!r.verified, created_at: r.created_at,
  };
}

/** Drop routes older than maxAgeMs (default 30d) or beyond maxRows newest. Returns count removed. */
export function pruneRoutes({ platform = "telegram", now = Date.now(), maxAgeMs = 30 * 24 * 3600 * 1000, maxRows = 5000 } = {}) {
  const d = db();
  const byAge = d.prepare(`DELETE FROM bridge_routes WHERE platform = ? AND created_at < ?`)
    .run(platform, now - maxAgeMs);
  // SQLite requires LIMIT -1 to enable OFFSET without a row cap
  const overflow = d.prepare(`DELETE FROM bridge_routes WHERE platform = ? AND external_id IN (
      SELECT external_id FROM bridge_routes WHERE platform = ? ORDER BY created_at DESC LIMIT -1 OFFSET ?)`)
    .run(platform, platform, maxRows);
  return (byAge.changes || 0) + (overflow.changes || 0);
}

const offsetKey = (platform) => `bridge_update_offset_${platform}`;

export function getUpdateOffset({ platform = "telegram" } = {}) {
  const row = db().prepare(`SELECT value FROM meta WHERE key = ?`).get(offsetKey(platform));
  return row ? Number(row.value) : 0;
}

export function setUpdateOffset({ platform = "telegram", offset }) {
  db().prepare(`INSERT INTO meta (key, value) VALUES (?, ?)
    ON CONFLICT(key) DO UPDATE SET value = excluded.value`).run(offsetKey(platform), String(offset));
}
