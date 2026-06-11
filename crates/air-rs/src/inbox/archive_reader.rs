//! Read-only view of the daemon's `archive.db` (WAL). SHORT-LIVED statements only — never hold a
//! read txn open under WAL (it would unbound the writer's WAL file). Ports the read paths of
//! archive.mjs: parseRow / replaySince / getCursor / history / conversations.
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;

const BUSY_TIMEOUT_MS: u64 = 5000;
const OPEN_RETRIES: u32 = 12;
const OPEN_RETRY_SLEEP: Duration = Duration::from_millis(50);

/// One archived message row (ports `parseRow`).
#[derive(Debug, Clone, Serialize)]
pub struct ArchiveRow {
    /// Globally unique envelope id.
    pub envelope_id: String,
    /// `"received"` | `"sent"`.
    pub direction: String,
    /// Conversation thread id.
    pub thread_id: String,
    /// The peer (other party) DID for this row.
    pub peer_did: String,
    /// Sender DID.
    pub from: String,
    /// Recipient DID.
    pub to: String,
    /// ISO 8601 timestamp.
    pub timestamp: String,
    /// Decoded body JSON.
    pub body: Value,
    /// Whether the envelope was encrypted.
    pub encrypted: bool,
    /// Whether the signature verified.
    pub verified: bool,
    /// Whether the sender's key changed since last contact.
    pub key_changed: bool,
    /// Whether this row is flagged spam.
    pub spam: bool,
    /// Relay sequence number (null until stamped).
    pub relay_seq: Option<i64>,
    /// Room id for group messages (null for 1:1).
    pub room_id: Option<String>,
    /// When the row was archived.
    pub archived_at: String,
}

fn archive_path(home: &Path) -> PathBuf {
    home.join("archive.db")
}

/// Does the archive file exist (ports `archiveExists`)? Never materializes a DB.
pub fn archive_exists(home: &Path) -> bool {
    archive_path(home).exists()
}

/// Read-only handle to the archive DB.
pub struct ArchiveReader {
    conn: Connection,
}

impl ArchiveReader {
    /// Open read-only with a busy_timeout, retrying transient open failures (a concurrent
    /// checkpoint or a momentary `-shm`/`-wal` race). Never panics; bubbles a typed error after
    /// the bounded retry budget.
    pub fn open(home: &Path) -> Result<Self, rusqlite::Error> {
        let path = archive_path(home);
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let mut last: Option<rusqlite::Error> = None;
        for _ in 0..OPEN_RETRIES {
            match Connection::open_with_flags(&path, flags) {
                Ok(conn) => {
                    conn.busy_timeout(Duration::from_millis(BUSY_TIMEOUT_MS))?;
                    return Ok(Self { conn });
                }
                Err(e) => {
                    last = Some(e);
                    std::thread::sleep(OPEN_RETRY_SLEEP);
                }
            }
        }
        Err(last.unwrap())
    }

    /// Replay source (ports `replaySince` — invariants #1–#3 only; the replayer adds #4 + #5).
    pub fn replay_since(&self, since_seq: i64, limit: i64) -> Result<Vec<ArchiveRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT envelope_id, direction, thread_id, peer_did, from_did, to_did, timestamp, \
                    body_json, encrypted, verified, key_changed, spam, relay_seq, room_id, archived_at \
             FROM messages \
             WHERE direction = 'received' AND spam = 0 AND relay_seq IS NOT NULL AND relay_seq > ?1 \
               AND envelope_id NOT LIKE '%:joined' \
             ORDER BY relay_seq ASC LIMIT ?2",
        )?;
        let rows = stmt.query_map([since_seq, limit], map_row)?;
        rows.collect()
    }

    /// Pull cursor (ports `getCursor`): highest relay_seq pulled, 0 if unset.
    pub fn get_cursor(&self) -> Result<i64, rusqlite::Error> {
        let v: Option<String> = self
            .conn
            .query_row("SELECT value FROM meta WHERE key = 'pull_cursor'", [], |r| r.get(0))
            .ok();
        Ok(v.and_then(|s| s.parse::<i64>().ok()).unwrap_or(0))
    }

    /// Conversation history, newest-first (ports `history`). `before` is an ISO timestamp.
    pub fn history(
        &self,
        peer: Option<&str>,
        thread: Option<&str>,
        room: Option<&str>,
        before: Option<&str>,
        limit: i64,
        include_spam: bool,
    ) -> Result<Vec<ArchiveRow>, rusqlite::Error> {
        let mut where_sql = Vec::new();
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(p) = peer { where_sql.push("peer_did = ?"); params.push(Box::new(p.to_string())); }
        if let Some(t) = thread { where_sql.push("thread_id = ?"); params.push(Box::new(t.to_string())); }
        if let Some(r) = room { where_sql.push("room_id = ?"); params.push(Box::new(r.to_string())); }
        if let Some(b) = before { where_sql.push("timestamp < ?"); params.push(Box::new(b.to_string())); }
        if !include_spam { where_sql.push("spam = 0"); }
        let clause = if where_sql.is_empty() { String::new() } else { format!("WHERE {}", where_sql.join(" AND ")) };
        params.push(Box::new(limit));
        let sql = format!(
            "SELECT envelope_id, direction, thread_id, peer_did, from_did, to_did, timestamp, \
                    body_json, encrypted, verified, key_changed, spam, relay_seq, room_id, archived_at \
             FROM messages {clause} ORDER BY timestamp DESC, archived_at DESC LIMIT ?"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();
        let rows = stmt.query_map(refs.as_slice(), map_row)?;
        rows.collect()
    }
}

/// One conversation summary for the §6 sidebar. **Grouping per design §6 (critic M1):** 1:1
/// conversations key on `peer_did`, rooms on `room_id` — NOT `thread_id` (outbound 1:1 thread_ids
/// default to a fresh uuid per message and would fragment the list).
#[derive(Debug, Clone, Serialize)]
pub struct ConversationSummary {
    /// The grouping key: `room_id` for rooms, else `peer_did`.
    pub conv_key: String,
    /// `"room"` | `"peer"`.
    pub kind: String,
    /// Newest message timestamp in this conversation.
    pub last_timestamp: String,
    /// Number of (non-spam) rows in this conversation.
    pub count: i64,
}

impl ArchiveReader {
    /// Conversation list for the §6 sidebar — 1:1 keyed by peer_did, rooms by room_id.
    pub fn conversations(&self) -> Result<Vec<ConversationSummary>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT CASE WHEN room_id IS NOT NULL THEN room_id ELSE peer_did END AS conv_key, \
                    CASE WHEN room_id IS NOT NULL THEN 'room' ELSE 'peer' END AS kind, \
                    MAX(timestamp) AS last_timestamp, COUNT(*) AS count \
             FROM messages WHERE spam = 0 \
             GROUP BY conv_key ORDER BY last_timestamp DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(ConversationSummary {
                conv_key: r.get(0)?,
                kind: r.get(1)?,
                last_timestamp: r.get(2)?,
                count: r.get(3)?,
            })
        })?;
        rows.collect()
    }
}

fn map_row(r: &rusqlite::Row) -> Result<ArchiveRow, rusqlite::Error> {
    let body_json: String = r.get(7)?;
    Ok(ArchiveRow {
        envelope_id: r.get(0)?,
        direction: r.get(1)?,
        thread_id: r.get(2)?,
        peer_did: r.get(3)?,
        from: r.get(4)?,
        to: r.get(5)?,
        timestamp: r.get(6)?,
        body: serde_json::from_str(&body_json).unwrap_or(Value::Null),
        encrypted: r.get::<_, i64>(8)? != 0,
        verified: r.get::<_, i64>(9)? != 0,
        key_changed: r.get::<_, i64>(10)? != 0,
        spam: r.get::<_, i64>(11)? != 0,
        relay_seq: r.get(12)?,
        room_id: r.get(13)?,
        archived_at: r.get(14)?,
    })
}
