//! At-least-once replay (ports channel-replay.mjs). On a `gap`, replay the hole from the archive
//! and re-apply ALL FIVE invariants identically to live delivery — replay never delivers more than
//! live did. The SQL gives #1–#3; THIS module adds #4 (blocklist) + #5 (current-pin channel gate),
//! plus dedupe across the live/replay overlap.
use crate::inbox::archive_reader::{ArchiveReader, ArchiveRow};
use crate::inbox::frames::Message;
use crate::inbox::gate::channel_gate;
use crate::inbox::stores::{get_contact_by_did, is_blocked};
use std::collections::{HashSet, VecDeque};
use std::path::Path;

const MAX_SEEN: usize = 1000;
const PAGE_SIZE: i64 = 500;

/// Map an archive row to the wire `Message` shape, deriving `contact` from CURRENT pin state
/// (invariant #5's "currently-pinned"). Ports `rowToMessage`.
pub fn row_to_message(row: &ArchiveRow, home: &Path) -> Message {
    let contact = get_contact_by_did(home, &row.from).and_then(|c| c.alias);
    Message {
        seq: row.relay_seq.unwrap_or(0),
        relay_seq: row.relay_seq.unwrap_or(0),
        from: row.from.clone(),
        contact: contact.filter(|a| !a.is_empty()),
        envelope_id: row.envelope_id.clone(),
        received_at: row.timestamp.clone(),
        verified: row.verified,
        encrypted: row.encrypted,
        key_changed: if row.key_changed { Some(true) } else { None },
        room_id: row.room_id.clone(),
        body: Some(row.body.clone()),
        thread_id: Some(row.thread_id.clone()),
    }
}

/// Deduping replay coordinator. `live` feeds streamed frames; `gap` fills a hole from the archive.
/// Both paths emit only gate-admitted messages (the daemon already gated live, but re-gating is the
/// JS pipeline's behaviour and is the security backstop for the replay path).
pub struct Replayer {
    seen: HashSet<String>,
    order: VecDeque<String>,
    mute: HashSet<String>,
}

impl Replayer {
    /// Construct a replayer with the given mute set (alias/DID/short-AIR-id).
    pub fn new(mute: HashSet<String>) -> Self {
        Self { seen: HashSet::new(), order: VecDeque::new(), mute }
    }

    fn remember(&mut self, id: &str) {
        if self.seen.insert(id.to_string()) {
            self.order.push_back(id.to_string());
            if self.order.len() > MAX_SEEN {
                if let Some(old) = self.order.pop_front() {
                    self.seen.remove(&old);
                }
            }
        }
    }

    /// A streamed frame: dedupe, then admit iff the gate passes. Returns Some to emit.
    pub fn live(&mut self, m: Message) -> Option<Message> {
        if self.seen.contains(&m.envelope_id) {
            return None;
        }
        self.remember(&m.envelope_id.clone());
        if channel_gate(&m, &self.mute) { Some(m) } else { None }
    }

    /// Replay the hole after `after_seq`. Paginates (a long outage exceeds one page — a silent
    /// truncation would be invisible mail loss), skips blocked senders (#4), dedupes, maps with
    /// current-pin `contact`, and gates (#5). Returns the messages to emit, oldest-first.
    pub fn gap(&mut self, reader: &ArchiveReader, home: &Path, after_seq: i64)
        -> Result<Vec<Message>, rusqlite::Error>
    {
        let mut out = Vec::new();
        let mut since = after_seq;
        loop {
            let rows = reader.replay_since(since, PAGE_SIZE)?;
            let n = rows.len() as i64;
            for row in &rows {
                if is_blocked(home, &row.from) { continue; }          // #4
                if self.seen.contains(&row.envelope_id) { continue; }
                self.remember(&row.envelope_id.clone());
                let m = row_to_message(row, home);
                if channel_gate(&m, &self.mute) {                      // #5
                    out.push(m);
                }
            }
            if n < PAGE_SIZE { break; }                               // short page = end of hole
            since = rows.last().and_then(|r| r.relay_seq).unwrap_or(since);
        }
        Ok(out)
    }

    /// Current size of the dedupe set (test/observability hook).
    pub fn seen_size(&self) -> usize {
        self.seen.len()
    }
}
