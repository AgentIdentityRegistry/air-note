// archive.mjs — local persistent message diary (#14). Sole owner of ~/.air-msg/archive.db.
//
// Stores DECRYPTED, readable messages (sent + received) so history survives the relay's
// 30-day window and `inbox` always has something to show. Dedup is by (envelope_id,
// direction): relay redeliveries are no-ops, and a self-sent message can appear as both a
// 'sent' and a 'received' row. node:sqlite — zero dependencies, needs Node >= 22.
//
// SECURITY (decision #1): plaintext at rest in the 0600 store; encrypt-at-rest awaits #19.

import { DatabaseSync } from "node:sqlite";
import { mkdirSync, chmodSync } from "node:fs";
import { join } from "node:path";
import { bridgeHome } from "./identity.mjs";

let _db = null;

const archivePath = () => join(bridgeHome(), "archive.db");

// DDL run one statement at a time via prepare().run() (the repo hook forbids db.exec).
const SCHEMA = [
  `CREATE TABLE IF NOT EXISTS messages (
      envelope_id  TEXT NOT NULL,
      direction    TEXT NOT NULL,
      thread_id    TEXT NOT NULL,
      peer_did     TEXT NOT NULL,
      from_did     TEXT NOT NULL,
      to_did       TEXT NOT NULL,
      timestamp    TEXT NOT NULL,
      body_json    TEXT NOT NULL,
      encrypted    INTEGER NOT NULL,
      verified     INTEGER NOT NULL,
      relay_seq    INTEGER,
      archived_at  TEXT NOT NULL,
      PRIMARY KEY (envelope_id, direction)
    )`,
  `CREATE INDEX IF NOT EXISTS idx_messages_thread ON messages(thread_id, timestamp)`,
  `CREATE INDEX IF NOT EXISTS idx_messages_peer ON messages(peer_did, timestamp)`,
  `CREATE INDEX IF NOT EXISTS idx_messages_time ON messages(timestamp)`,
  `CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)`,
];

/** Open (creating + migrating if needed) the archive DB. Memoized per process. */
export function openArchive() {
  if (_db) return _db;
  mkdirSync(bridgeHome(), { recursive: true, mode: 0o700 });
  const path = archivePath();
  const db = new DatabaseSync(path);
  for (const stmt of SCHEMA) db.prepare(stmt).run();
  try { chmodSync(path, 0o600); } catch { /* best effort on non-POSIX */ }
  _db = db;
  return db;
}

/** Save one message (INSERT OR IGNORE on the (envelope_id, direction) key). */
export function archiveMessage(rec) {
  const db = openArchive();
  const res = db.prepare(`
    INSERT OR IGNORE INTO messages
      (envelope_id, direction, thread_id, peer_did, from_did, to_did, timestamp,
       body_json, encrypted, verified, relay_seq, archived_at)
    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
  `).run(
    rec.envelope_id, rec.direction, rec.thread_id, rec.peer_did, rec.from_did, rec.to_did,
    rec.timestamp, JSON.stringify(rec.body), rec.encrypted ? 1 : 0, rec.verified ? 1 : 0,
    rec.relay_seq ?? null, new Date().toISOString(),
  );
  return { inserted: res.changes > 0 };
}

function parseRow(r) {
  return {
    envelope_id: r.envelope_id, direction: r.direction, thread_id: r.thread_id,
    peer_did: r.peer_did, from: r.from_did, to: r.to_did, timestamp: r.timestamp,
    body: JSON.parse(r.body_json), encrypted: !!r.encrypted, verified: !!r.verified,
    relay_seq: r.relay_seq ?? undefined, archived_at: r.archived_at,
  };
}

/** Query history newest-first. Filters: peer (DID), thread (id), before (ISO ts), limit. */
export function history({ peer, thread, before, limit = 50 } = {}) {
  const db = openArchive();
  const where = [];
  const params = [];
  if (peer)   { where.push("peer_did = ?"); params.push(peer); }
  if (thread) { where.push("thread_id = ?"); params.push(thread); }
  if (before) { where.push("timestamp < ?"); params.push(before); }
  const clause = where.length ? `WHERE ${where.join(" AND ")}` : "";
  params.push(limit);
  return db.prepare(
    `SELECT * FROM messages ${clause} ORDER BY timestamp DESC, archived_at DESC LIMIT ?`
  ).all(...params).map(parseRow);
}

/** Recent messages across all peers, newest-first (the inbox view). */
export function recentForInbox(limit = 20) {
  return history({ limit });
}

/** Distinct conversations with last activity + message count. */
export function threads() {
  const db = openArchive();
  return db.prepare(`
    SELECT thread_id, peer_did, MAX(timestamp) AS last_timestamp, COUNT(*) AS count
    FROM messages GROUP BY thread_id ORDER BY last_timestamp DESC
  `).all().map((r) => ({
    thread_id: r.thread_id, peer_did: r.peer_did,
    last_timestamp: r.last_timestamp, count: Number(r.count),
  }));
}

/** Relay pull cursor (highest relay_seq pulled). 0 if unset. */
export function getCursor() {
  const db = openArchive();
  const row = db.prepare("SELECT value FROM meta WHERE key = 'pull_cursor'").get();
  return row ? Number(row.value) : 0;
}

/** Raise the cursor (monotonic — never lowers). */
export function setCursor(seq) {
  const db = openArchive();
  const next = Math.max(getCursor(), Number(seq) || 0);
  db.prepare(
    "INSERT INTO meta (key, value) VALUES ('pull_cursor', ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value"
  ).run(String(next));
}

/** Close the DB (used by tests to reset between cases). */
export function closeArchive() {
  if (_db) { _db.close(); _db = null; }
}

/** Placeholder for the future cloud-backup layer (#14 stage 2; see design §6). Deferred. */
export async function backupArchive(/* adapter */) { /* intentionally a no-op seam */ }
