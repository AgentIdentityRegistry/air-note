//! The append-only event log. The single source of truth.
//!
//! Appends are strictly serialized: one process-wide `Mutex` guards the
//! read-tip → hash → sign → insert critical section, so the hash chain can
//! never fork (spec §4 single-writer invariant). The evolve loop (M4) is NOT a
//! privileged writer — it calls `append` like everyone else.

use std::path::Path;
use std::sync::Mutex;

use chrono::Utc;
use ed25519_dalek::SigningKey;
use ulid::Ulid;

use crate::error::BossclawError;
use crate::event::{compute_hash, Event};
use crate::highwater::{HighWaterStore, Mark};
use crate::sign::{sign_hash, verify_hash};
use crate::store::Store;

const GENESIS: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

/// The serialized, signed event log.
pub struct EventLog {
    inner: Mutex<Store>,
    key: SigningKey,
    highwater: Option<Box<dyn HighWaterStore>>,
}

impl EventLog {
    /// Open (creating if needed) an event log at `path`, encrypted with `dek`,
    /// signing with `key`.
    pub fn open(path: &Path, dek: &[u8; 32], key: SigningKey) -> Result<Self, BossclawError> {
        let store = Store::open(path, dek)?;
        store.exec(
            "CREATE TABLE IF NOT EXISTS events (
                seq        INTEGER PRIMARY KEY AUTOINCREMENT,
                id         TEXT NOT NULL UNIQUE,
                ts         TEXT NOT NULL,
                event_type TEXT NOT NULL,
                payload    TEXT NOT NULL,
                prev_hash  TEXT NOT NULL,
                hash       TEXT NOT NULL UNIQUE
            )",
        )?;
        Ok(Self { inner: Mutex::new(store), key, highwater: None })
    }

    /// Append an event. `id`, `ts`, `prev_hash`, `hash`, `signature` are
    /// assigned here; the caller supplies `event_type`, `content`, `model_meta`,
    /// `signed_by_did`, optional `valid_time`.
    pub fn append(&self, mut event: Event) -> Result<String, BossclawError> {
        if let Some(meta) = &event.model_meta {
            if meta.source_event_ids.is_empty() {
                return Err(BossclawError::Chain(
                    "Tier-B event requires non-empty source_event_ids".into(),
                ));
            }
        }

        let store = self.inner.lock().expect("event log mutex poisoned");
        let conn = store.conn();
        let tx = conn.unchecked_transaction()?;

        let prev_hash: String = tx
            .query_row("SELECT hash FROM events ORDER BY seq DESC LIMIT 1", [], |r| r.get(0))
            .unwrap_or_else(|_| GENESIS.to_string());

        event.id = Ulid::new().to_string();
        event.ts = Utc::now().to_rfc3339();
        event.prev_hash = prev_hash;
        event.hash = None;
        event.signature = None;

        let hash = compute_hash(&event)?;
        let hash_hex = hex::encode(hash);
        let sig = sign_hash(&hash, &self.key);
        event.hash = Some(hash_hex.clone());
        event.signature = Some(sig);

        let payload = serde_json::to_string(&event)?;
        tx.execute(
            "INSERT INTO events (id, ts, event_type, payload, prev_hash, hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![event.id, event.ts, event.event_type, payload, event.prev_hash, hash_hex],
        )?;
        tx.commit()?;
        Ok(event.id)
    }

    /// Number of events in the log.
    pub fn count(&self) -> Result<i64, BossclawError> {
        let store = self.inner.lock().expect("poisoned");
        let n = store.conn().query_row("SELECT count(*) FROM events", [], |r| r.get(0))?;
        Ok(n)
    }

    /// Re-verify the whole chain: every row's hash recomputes from its canonical
    /// bytes + prev_hash, links to the prior row, and its signature verifies.
    pub fn verify_chain(&self) -> Result<(), BossclawError> {
        let store = self.inner.lock().expect("poisoned");
        let conn = store.conn();
        let mut stmt = conn.prepare("SELECT payload, prev_hash, hash FROM events ORDER BY seq ASC")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        })?;

        let mut expected_prev = GENESIS.to_string();
        for row in rows {
            let (payload, prev_hash, hash_hex) = row?;
            if prev_hash != expected_prev {
                return Err(BossclawError::Chain(format!(
                    "broken link: expected prev {expected_prev}, got {prev_hash}"
                )));
            }
            let event: Event = serde_json::from_str(&payload)?;
            let recomputed = hex::encode(compute_hash(&event)?);
            if recomputed != hash_hex {
                return Err(BossclawError::Chain(format!(
                    "hash mismatch at {}: stored {hash_hex}, recomputed {recomputed}",
                    event.id
                )));
            }
            let sig = event
                .signature
                .as_deref()
                .ok_or_else(|| BossclawError::Chain("missing signature".into()))?;
            let hash_bytes = compute_hash(&event)?;
            verify_hash(&hash_bytes, sig, &self.key.verifying_key())?;
            expected_prev = hash_hex;
        }
        Ok(())
    }

    /// Open with a high-water store; checks truncation immediately.
    pub fn open_with_highwater(
        path: &Path,
        dek: &[u8; 32],
        key: SigningKey,
        highwater: Box<dyn HighWaterStore>,
    ) -> Result<Self, BossclawError> {
        let mut log = Self::open(path, dek, key)?;
        if let Some(mark) = highwater.load()? {
            let live = log.count()?;
            if live < mark.count {
                return Err(BossclawError::Truncation(format!(
                    "live count {live} < high-water {} (tail deleted)",
                    mark.count
                )));
            }
        }
        log.highwater = Some(highwater);
        Ok(log)
    }

    /// Persist the current tip as the signed high-water mark (debounced by the
    /// caller — every K events / on idle / on clean shutdown, NOT per append).
    pub fn checkpoint_highwater(&self) -> Result<(), BossclawError> {
        let hw = match &self.highwater {
            Some(h) => h,
            None => return Ok(()),
        };
        let store = self.inner.lock().expect("event log mutex poisoned");
        let conn = store.conn();
        let count: i64 = conn.query_row("SELECT count(*) FROM events", [], |r| r.get(0))?;
        let tip_hash: String = conn
            .query_row("SELECT hash FROM events ORDER BY seq DESC LIMIT 1", [], |r| r.get(0))
            .unwrap_or_else(|_| GENESIS.to_string());
        hw.save(&Mark { count, tip_hash })
    }
}
