//! The append-only event log. The single source of truth.
//!
//! Appends are strictly serialized: one process-wide `Mutex` guards the
//! read-tip → hash → sign → insert critical section, so the hash chain can
//! never fork (spec §4 single-writer invariant). The evolve loop (M4) is NOT a
//! privileged writer — it calls `append` like everyone else.

use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;

use chrono::Utc;
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use ulid::Ulid;

use crate::embed::Embedder;
use crate::error::BossclawError;
use crate::event::{compute_hash, Event};
use crate::highwater::{HighWaterStore, Mark};
use crate::index::{HnswIndex, VectorIndex};
use crate::sign::{sign_hash, verify_hash};
use crate::store::Store;

/// The parsed content of the latest `config` event.
///
/// A `config` event uses `event_type = "config"` and carries a `content`
/// object with the following fields:
/// - `active_model_id`: identifier of the active embedding model.
/// - `dim`: vector dimensionality produced by that model.
/// - `schema_version`: reserved for format-gating in later milestones.
///
/// Only the LATEST config event is authoritative. Appending a new config event
/// is how the active model is rotated.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActiveModel {
    /// Identifier of the active embedding model (e.g. `"mock-v1"`).
    pub active_model_id: String,
    /// Dimensionality of the vectors produced by the active model.
    /// Callers feeding this into an `Embedder` should convert with
    /// `usize::try_from(model.dim).expect("dim fits usize")`.
    pub dim: u32,
    /// Reserved: format-gating logic is deferred to a later milestone.
    pub schema_version: u32,
}

const GENESIS: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";
const POISON: &str = "event log mutex poisoned";

/// Number of bytes in a little-endian `f32`. Used to size and validate the
/// `embedding` BLOB encoding in the `vectors` table.
const F32_BYTES: usize = std::mem::size_of::<f32>();

/// Event types whose `content["text"]` is fed to the embedder. `page` does not
/// exist until M4 but is listed here so the seam is forward-compatible.
const EMBEDDABLE_EVENT_TYPES: &[&str] = &["memory", "page"];

/// The serialized, signed event log.
pub struct EventLog {
    inner: Mutex<Store>,
    key: SigningKey,
    highwater: Option<Box<dyn HighWaterStore>>,
    /// In-memory ANN index over the active model's vectors. `None` until
    /// [`EventLog::rebuild_indexes`] builds it. Never persisted — rebuilt from
    /// the encrypted log on open (zero plaintext index on disk). Guarded by its
    /// own `Mutex` so a rebuild never blocks log appends. The boxed trait is
    /// `Send + Sync` (the [`VectorIndex`] bound guarantees it), so `EventLog`
    /// stays `Send + Sync` and shareable as `Arc<EventLog>`.
    vector_index: Mutex<Option<Box<dyn VectorIndex>>>,
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
        // Tier-A derived vectors. One row per (event, model); the embedding is
        // little-endian f32 bytes. Keyed on (event_id, model_id) so re-deriving
        // under the same model is an idempotent upsert and different models can
        // coexist for the same event without colliding.
        store.exec(
            "CREATE TABLE IF NOT EXISTS vectors (
                event_id  TEXT NOT NULL,
                model_id  TEXT NOT NULL,
                dim       INTEGER NOT NULL,
                embedding BLOB NOT NULL,
                PRIMARY KEY(event_id, model_id)
            )",
        )?;
        Ok(Self {
            inner: Mutex::new(store),
            key,
            highwater: None,
            vector_index: Mutex::new(None),
        })
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

        let store = self.inner.lock().expect(POISON);
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
        let store = self.inner.lock().expect(POISON);
        let n = store.conn().query_row("SELECT count(*) FROM events", [], |r| r.get(0))?;
        Ok(n)
    }

    /// Re-verify the whole chain: every row's hash recomputes from its canonical
    /// bytes + prev_hash, links to the prior row, and its signature verifies.
    pub fn verify_chain(&self) -> Result<(), BossclawError> {
        let store = self.inner.lock().expect(POISON);
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
            let hash_bytes = compute_hash(&event)?;
            let recomputed = hex::encode(hash_bytes);
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

    /// Return the active embedding model configuration, parsed from the latest
    /// `config` event in the log.
    ///
    /// A `config` event has `event_type = "config"` and a `content` object
    /// with `active_model_id`, `dim`, and `schema_version`. Only the row with
    /// the highest `seq` is used; earlier config events are superseded.
    ///
    /// Returns `Ok(None)` if no `config` event has ever been appended.
    pub fn active_model(&self) -> Result<Option<ActiveModel>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let result = conn.query_row(
            "SELECT payload FROM events WHERE event_type='config' ORDER BY seq DESC LIMIT 1",
            [],
            |r| r.get::<_, String>(0),
        );
        match result {
            Ok(payload) => {
                let event: Event = serde_json::from_str(&payload)?;
                let model: ActiveModel = serde_json::from_value(event.content)?;
                Ok(Some(model))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(BossclawError::Store(e.to_string())),
        }
    }

    /// Return every event in chain order (M1: full scan; M2 adds `since`).
    pub fn stream_all(&self) -> Result<Vec<Event>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare("SELECT payload FROM events ORDER BY seq ASC")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(serde_json::from_str(&row?)?);
        }
        Ok(out)
    }

    /// Derive and store the Tier-A vector for a single event, if embeddable.
    ///
    /// If [`embeddable_text`] yields `Some(text)`, the text is embedded as a
    /// one-item batch and upserted into the `vectors` table under
    /// `(event.id, embedder.model_id())` (INSERT OR REPLACE), returning
    /// `Ok(true)`. Non-embeddable events store nothing and return `Ok(false)`.
    /// Embedder failures propagate as `Err`.
    ///
    /// Production calls this AFTER [`EventLog::append`] has committed and MAY
    /// ignore the returned `Err`: vector derivation is best-effort (spec §10),
    /// and a missing vector is repaired later by
    /// [`EventLog::rederive_pending`]. The append itself is never blocked on
    /// embedding success.
    pub fn derive_vector(
        &self,
        embedder: &dyn Embedder,
        event: &Event,
    ) -> Result<bool, BossclawError> {
        let text = match embeddable_text(event) {
            Some(t) => t,
            None => return Ok(false),
        };
        let embedding = embed_one(embedder, &text)?;
        let blob = vec_to_blob(&embedding);
        let store = self.inner.lock().expect(POISON);
        store.conn().execute(
            "INSERT OR REPLACE INTO vectors (event_id, model_id, dim, embedding)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![event.id, embedder.model_id(), embedder.dim() as i64, blob],
        )?;
        Ok(true)
    }

    /// Backfill every embeddable event that has no vector for this model.
    ///
    /// This is both the initial backfill and the spec §10 retry hook: it finds
    /// events of an embeddable type that lack a `vectors` row for
    /// `embedder.model_id()` (in `seq` order) and derives them. The store
    /// `Mutex` is held only to collect the pending rows and (separately) to
    /// upsert each result — never across [`Embedder::embed`], so the single
    /// store mutex cannot deadlock against the embedder.
    ///
    /// BEST-EFFORT: an individual embed failure is logged via [`log::warn!`]
    /// and skipped; the backfill continues. Returns the number of vectors
    /// successfully derived.
    pub fn rederive_pending(&self, embedder: &dyn Embedder) -> Result<usize, BossclawError> {
        let pending = self.collect_pending(embedder.model_id())?;
        let mut derived = 0usize;
        for event in pending {
            // `collect_pending` already filters to embeddable event types, but
            // the individual event's `content["text"]` may still be absent or
            // non-string (malformed data). Warn so the bad event is visible;
            // do NOT insert a tombstone — a zero-length vector would corrupt
            // T5 index reads.
            let text = match embeddable_text(&event) {
                Some(t) => t,
                None => {
                    log::warn!(
                        "rederive_pending: event {} (type={}) has no embeddable text; \
                         skipping (malformed content)",
                        event.id,
                        event.event_type,
                    );
                    continue;
                }
            };
            let embedding = match embed_one(embedder, &text) {
                Ok(v) => v,
                Err(e) => {
                    log::warn!(
                        "rederive_pending: skipping event {} (embed failed): {e}",
                        event.id
                    );
                    continue;
                }
            };
            let blob = vec_to_blob(&embedding);
            let store = self.inner.lock().expect(POISON);
            store.conn().execute(
                "INSERT OR REPLACE INTO vectors (event_id, model_id, dim, embedding)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![event.id, embedder.model_id(), embedder.dim() as i64, blob],
            )?;
            derived += 1;
        }
        Ok(derived)
    }

    /// All stored vectors for `model_id`, as `(event_id, vector)` pairs ordered
    /// by `event_id ASC`.
    ///
    /// This is the active-model-filtered read: only vectors derived under the
    /// given `model_id` are returned, so cross-model comparison is impossible by
    /// construction. The `event_id ASC` ordering is mandatory — the T5
    /// deterministic index rebuild depends on a stable row order.
    pub fn vectors_for_model(
        &self,
        model_id: &str,
    ) -> Result<Vec<(String, Vec<f32>)>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT event_id, embedding FROM vectors WHERE model_id = ?1 ORDER BY event_id ASC",
        )?;
        let rows = stmt.query_map([model_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (event_id, blob) = row?;
            out.push((event_id, blob_to_vec(&blob)?));
        }
        Ok(out)
    }

    /// Rebuild the in-memory vector index from the encrypted log for the active
    /// embedding model.
    ///
    /// Reads every persisted vector for `embedder.model_id()` (via
    /// [`EventLog::vectors_for_model`], which returns rows `ORDER BY event_id
    /// ASC`), builds a fresh [`HnswIndex`] sized to the exact row count, and
    /// **serially** adds each `(event_id, vector)`. Serial insertion over a
    /// deterministic row order is what makes the index reproducible across
    /// re-opens (spec F2). The finished index replaces any previous one.
    ///
    /// Because only `model_id`-matching rows are read, the index can only ever
    /// contain active-model vectors — cross-model bleed is impossible by
    /// construction (spec C4).
    ///
    /// Emits a [`log::info!`] timing line so rebuild cost is visible before the
    /// recall benchmark (T9). For now this rebuilds only the vector index; T6
    /// will extend it to also rebuild the FTS index.
    pub fn rebuild_indexes(&self, embedder: &dyn Embedder) -> Result<(), BossclawError> {
        let started = Instant::now();
        let rows = self.vectors_for_model(embedder.model_id())?;
        let count = rows.len();
        let mut index = HnswIndex::with_capacity(count);
        for (event_id, vec) in rows {
            index.add(&event_id, &vec);
        }
        let boxed: Box<dyn VectorIndex> = Box::new(index);
        *self.vector_index.lock().expect(POISON) = Some(boxed);
        log::info!(
            "rebuilt vector index: {count} vectors in {}ms",
            started.elapsed().as_millis()
        );
        Ok(())
    }

    /// Search the in-memory vector index for the `k` nearest `(event_id,
    /// distance)` pairs to `query_vec`, ascending by distance.
    ///
    /// Returns [`BossclawError::InvalidInput`] if the index has not been built
    /// yet (no [`EventLog::rebuild_indexes`] call since open) — recall cannot run
    /// against a missing index. Tombstoned ids are excluded by the index itself.
    ///
    /// T7's `recall()` will embed the query text and then call this.
    pub fn vector_search(
        &self,
        query_vec: &[f32],
        k: usize,
    ) -> Result<Vec<(String, f32)>, BossclawError> {
        let guard = self.vector_index.lock().expect(POISON);
        match guard.as_ref() {
            Some(index) => Ok(index.search(query_vec, k)),
            None => Err(BossclawError::InvalidInput(
                "vector index not built — call rebuild_indexes".into(),
            )),
        }
    }

    /// Collect, under a single short-lived lock, the events of an embeddable
    /// type that have no `vectors` row for `model_id`, in `seq` order. Returns
    /// owned `Event`s so the lock is released before any embedding happens.
    ///
    /// The SQL `IN (...)` filter is built from [`EMBEDDABLE_EVENT_TYPES`] so
    /// there is a single authoritative list — the Rust const and the SQL clause
    /// cannot drift independently.
    fn collect_pending(&self, model_id: &str) -> Result<Vec<Event>, BossclawError> {
        // Build `?2,?3,...` placeholders (one per embeddable type; ?1 = model_id).
        let placeholders: String = EMBEDDABLE_EVENT_TYPES
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", i + 2))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT e.payload FROM events e
             LEFT JOIN vectors v ON v.event_id = e.id AND v.model_id = ?1
             WHERE v.event_id IS NULL AND e.event_type IN ({placeholders})
             ORDER BY e.seq ASC"
        );
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(&sql)?;
        // Bind model_id first (?1), then each embeddable type (?2, ?3, …).
        // `&&str` coerces to `&dyn ToSql`; `&model_id` produces `&&str` from
        // `&str`, and `t` from `EMBEDDABLE_EVENT_TYPES` is already `&&str`.
        let params: Vec<&dyn rusqlite::ToSql> =
            std::iter::once(&model_id as &dyn rusqlite::ToSql)
                .chain(
                    EMBEDDABLE_EVENT_TYPES
                        .iter()
                        .map(|t| t as &dyn rusqlite::ToSql),
                )
                .collect();
        let rows = stmt.query_map(params.as_slice(), |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(serde_json::from_str(&row?)?);
        }
        Ok(out)
    }

    /// Persist the current tip as the signed high-water mark (debounced by the
    /// caller — every K events / on idle / on clean shutdown, NOT per append).
    pub fn checkpoint_highwater(&self) -> Result<(), BossclawError> {
        let hw = match &self.highwater {
            Some(h) => h,
            None => return Ok(()),
        };
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let count: i64 = conn.query_row("SELECT count(*) FROM events", [], |r| r.get(0))?;
        let tip_hash: String = conn
            .query_row("SELECT hash FROM events ORDER BY seq DESC LIMIT 1", [], |r| r.get(0))
            .unwrap_or_else(|_| GENESIS.to_string());
        hw.save(&Mark { count, tip_hash })
    }
}

/// The text fed to the embedder for an event, or `None` if the event is not
/// embeddable.
///
/// Only `memory` and `page` events carry embeddable prose; both expose it at
/// `content["text"]`. `config`, `grant`, and other control events return
/// `None`. (`page` is reserved for M4 and produces nothing today, since no such
/// events exist yet — listing it here keeps the derive seam forward-compatible.)
fn embeddable_text(event: &Event) -> Option<String> {
    if !EMBEDDABLE_EVENT_TYPES.contains(&event.event_type.as_str()) {
        return None;
    }
    event.content["text"].as_str().map(String::from)
}

/// Embed a single text as a one-item batch and return its vector.
///
/// Centralises the batch-of-one call + the "exactly one vector back" invariant
/// so both [`EventLog::derive_vector`] and [`EventLog::rederive_pending`] agree
/// on the shape contract. A batch that returns the wrong count is surfaced as
/// [`BossclawError::Embed`] rather than panicking.
fn embed_one(embedder: &dyn Embedder, text: &str) -> Result<Vec<f32>, BossclawError> {
    let mut batch = embedder.embed(&[text.to_string()])?;
    if batch.len() != 1 {
        return Err(BossclawError::Embed(format!(
            "embedder returned {} vectors for a 1-item batch",
            batch.len()
        )));
    }
    Ok(batch.remove(0))
}

/// Encode a vector as little-endian `f32` bytes for the `embedding` BLOB.
fn vec_to_blob(vec: &[f32]) -> Vec<u8> {
    let mut blob = Vec::with_capacity(vec.len() * F32_BYTES);
    for &x in vec {
        blob.extend_from_slice(&x.to_le_bytes());
    }
    blob
}

/// Decode little-endian `f32` bytes from an `embedding` BLOB.
///
/// Returns [`BossclawError::Store`] if the byte length is not a multiple of
/// [`F32_BYTES`] (a corrupt or truncated blob).
fn blob_to_vec(blob: &[u8]) -> Result<Vec<f32>, BossclawError> {
    if !blob.len().is_multiple_of(F32_BYTES) {
        return Err(BossclawError::Store(format!(
            "embedding blob length {} is not a multiple of {F32_BYTES}",
            blob.len()
        )));
    }
    let mut out = Vec::with_capacity(blob.len() / F32_BYTES);
    for chunk in blob.chunks_exact(F32_BYTES) {
        // `chunks_exact(4)` guarantees exactly 4 bytes; index directly rather
        // than `try_into` (which is unreachable and confuses the reader).
        out.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }
    Ok(out)
}
