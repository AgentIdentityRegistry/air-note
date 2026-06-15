//! The append-only event log. The single source of truth.
//!
//! Appends are strictly serialized: one process-wide `Mutex` guards the
//! read-tip → hash → sign → insert critical section, so the hash chain can
//! never fork (spec §4 single-writer invariant). The evolve loop (M4) is NOT a
//! privileged writer — it calls `append` like everyone else.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;

use chrono::{DateTime, Utc};
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use ulid::Ulid;

use rusqlite::OptionalExtension;

use crate::embed::Embedder;
use crate::error::BossclawError;
use crate::event::{compute_hash, Event, ModelMeta};
use crate::graph::MANUAL_LINK_PRODUCER;
use crate::highwater::{HighWaterStore, Mark};
use crate::index::{HnswIndex, VectorIndex};
use crate::keyword;
use crate::recall::{
    fuse_scored_arms, Hit, NoopReranker, RecallOptions, RecallSource, Reranker, FUSION_FETCH,
    HALF_LIFE_SECS, PIN_MULTIPLIER, RECENCY_WEIGHT,
};
use crate::sign::{sign_hash, verify_hash};
use crate::store::Store;

/// Reserved store-format version recorded in every `config` event.
///
/// Format-gating logic (refusing to open a store written by a future version)
/// is deferred to a later milestone. This constant is the single authoritative
/// source for what value gets written today.
pub const SCHEMA_VERSION: u32 = 1;

/// Statistics returned by [`EventLog::reembed_migration`].
///
/// Provides the §15 time-budget observability signal: callers (and handoff
/// records) use `elapsed_ms` to gauge migration cost before scheduling re-index
/// in production.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReembedStats {
    /// Number of events that had no vector for the new model and were
    /// successfully re-embedded during this migration.
    pub reembedded: usize,
    /// Number of stale `vectors` rows (under the old model) that were
    /// garbage-collected.
    pub gc_removed: usize,
    /// Wall-clock duration of the entire migration in milliseconds.
    pub elapsed_ms: u128,
}

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

/// DID stamped on engine-authored events (`link`/`invalidate`) in v1. Named so
/// the literal is single-sourced (like [`MANUAL_LINK_PRODUCER`]); M4/M7 will
/// replace this with the user's real DID threaded through [`EventLog::signer_did`].
const ENGINE_SIGNER_DID: &str = "did:wba:bossclaw-engine";

/// `(event_id, arm_score)` pair returned by each retrieval arm. Used as the
/// common type for both the vector arm (cosine distance, lower=better) and the
/// keyword arm (BM25 score, lower=better) before fusion.
type ArmHit = (String, f32);

/// Pair of live arm results (vector arm, keyword arm) returned by
/// [`resolve_arms`] after applying §10 graceful degradation.
type ArmPair = (Vec<ArmHit>, Vec<ArmHit>);
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
        // FTS5 full-text index (contentless — the event log is the content of
        // record). The `fts` virtual table stores only the indexed tokens; the
        // `fts_map` side-table maps FTS rowids back to event_ids because a
        // contentless FTS5 table cannot expose a readable payload column.
        //
        // Both tables live INSIDE the SQLCipher DB, so their on-disk bytes are
        // encrypted alongside every other table. `PRAGMA temp_store = MEMORY`
        // below ensures that FTS5 index-merge temporary files are never written to
        // disk as plaintext — they stay in process memory.
        store.exec(
            "CREATE VIRTUAL TABLE IF NOT EXISTS fts USING fts5(body, content='')",
        )?;
        store.exec(
            "CREATE TABLE IF NOT EXISTS fts_map (
                rowid    INTEGER PRIMARY KEY,
                event_id TEXT NOT NULL UNIQUE
            )",
        )?;
        // Bi-temporal graph projection (Tier-A; spec §5.6). One `edges` row per
        // `link` event (PK = the link's ULID); `invalidate` closes rows by
        // setting valid_to/invalidated_at. `nodes` = distinct endpoints. Both are
        // a deterministic fold over link/invalidate events, rebuilt by
        // `rebuild_graph`. Timestamps are stored normalized (fixed-width UTC) so
        // SQL TEXT comparison equals chronological comparison.
        store.exec(
            "CREATE TABLE IF NOT EXISTS edges (
                edge_id        TEXT PRIMARY KEY,
                src            TEXT NOT NULL,
                relation       TEXT NOT NULL,
                dst            TEXT NOT NULL,
                valid_from     TEXT NOT NULL,
                valid_to       TEXT,
                ingested_at    TEXT NOT NULL,
                invalidated_at TEXT,
                invalidated_by TEXT
            )",
        )?;
        store.exec(
            "CREATE TABLE IF NOT EXISTS nodes (
                node_id TEXT PRIMARY KEY,
                kind    TEXT NOT NULL
            )",
        )?;
        // Route FTS5 merge temporaries to memory, preventing any plaintext index
        // spill to the filesystem. This is a connection-level setting; it must be
        // re-applied on every open.
        store.exec("PRAGMA temp_store = MEMORY")?;
        // Verify the pragma actually took effect. SQLCipher builds compiled
        // with certain options can silently ignore temp_store; if that happened
        // FTS5 index-merge files would be written as plaintext to the OS temp
        // directory and the no-plaintext-on-disk guarantee would be void. We
        // surface the failure loudly at open rather than letting it slip past
        // the security test's dir-scan (which only covers the DB directory).
        let temp_store_val: i64 = store
            .conn()
            .query_row("PRAGMA temp_store", [], |r| r.get(0))?;
        if temp_store_val != 2 {
            return Err(BossclawError::Store(format!(
                "PRAGMA temp_store = MEMORY did not take effect (got {temp_store_val}, want 2); \
                 FTS5 index-merge files would spill to the OS temp dir as plaintext"
            )));
        }
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
        let mut stmt =
            conn.prepare("SELECT payload, prev_hash, hash FROM events ORDER BY seq ASC")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        })?;
        Self::verify_rows(rows, GENESIS.to_string(), &self.key)
    }

    /// Verify only the tail of the chain after a trusted cursor event.
    ///
    /// The cursor event and all events before it are trusted without re-checking.
    /// Only the events whose `seq` is greater than the cursor's `seq` are
    /// verified (hash recomputation, chain link, and signature).
    ///
    /// # Arguments
    /// * `from_event_id` — `None` verifies the whole chain (identical to
    ///   [`verify_chain`]). `Some(id)` verifies only the tail after the trusted
    ///   cursor event identified by `id`.
    ///
    /// # Errors
    /// * [`BossclawError::Chain`] if `from_event_id` is `Some` and the cursor
    ///   event is not found in the log.
    /// * [`BossclawError::Chain`] if any post-cursor row fails the link check,
    ///   hash recomputation, or signature verification.
    pub fn verify_chain_since(
        &self,
        from_event_id: Option<&str>,
    ) -> Result<(), BossclawError> {
        let cursor_id = match from_event_id {
            None => return self.verify_chain(),
            Some(id) => id,
        };

        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();

        // Look up the cursor row; its hash is the trusted starting point.
        let result = conn
            .query_row(
                "SELECT seq, hash FROM events WHERE id = ?1",
                rusqlite::params![cursor_id],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?;

        let (cursor_seq, cursor_hash) = result.ok_or_else(|| {
            BossclawError::Chain(format!(
                "verify_chain_since: cursor event {cursor_id} not found"
            ))
        })?;

        // Scan only events strictly after the trusted cursor.
        let mut stmt = conn.prepare(
            "SELECT payload, prev_hash, hash FROM events WHERE seq > ?1 ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map(rusqlite::params![cursor_seq], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        })?;

        Self::verify_rows(rows, cursor_hash, &self.key)
    }

    /// Shared per-row verification loop used by both [`verify_chain`] and
    /// [`verify_chain_since`].
    ///
    /// For each row (in the order produced by `rows`), this function:
    /// 1. Checks that `prev_hash` equals `expected_prev` (chain link).
    /// 2. Deserialises the payload into an [`Event`].
    /// 3. Recomputes the canonical hash and compares it with the stored value.
    /// 4. Verifies the Ed25519 signature over the hash bytes.
    /// 5. Advances `expected_prev` to the current row's hash.
    ///
    /// Returns `Ok(())` when every row passes; propagates the first failure as
    /// [`BossclawError::Chain`].
    fn verify_rows(
        rows: impl Iterator<Item = Result<(String, String, String), rusqlite::Error>>,
        mut expected_prev: String,
        key: &SigningKey,
    ) -> Result<(), BossclawError> {
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
            verify_hash(&hash_bytes, sig, &key.verifying_key())?;
            expected_prev = hash_hex;
        }
        Ok(())
    }

    /// Open an event log and immediately build the in-memory recall indexes.
    ///
    /// Convenience constructor that calls [`EventLog::open`] then
    /// [`EventLog::rebuild_indexes`] in one step, so the returned `EventLog` is
    /// **recall-ready**: [`EventLog::recall`] and [`EventLog::vector_search`]
    /// work without a separate `rebuild_indexes` call.
    ///
    /// # Lifecycle
    ///
    /// The in-memory vector index reflects the state of the `vectors` table at
    /// the moment [`EventLog::rebuild_indexes`] last ran (either here during
    /// open, or in a later explicit call). **After appending new events and
    /// deriving their vectors, call `rebuild_indexes(embedder)` again to make
    /// those events recallable via the semantic (vector) arm.** Until then,
    /// [`EventLog::recall`] degrades gracefully to keyword-only for the new
    /// events — this is the spec §10 intentional behaviour, but it must be
    /// explicit rather than a silent surprise.
    ///
    /// An incremental single-event `index_event` path (so appends don't require
    /// a full rebuild) is deferred to M7: the desktop decides the
    /// rebuild-vs-incremental policy once startup cost is profiled at scale.
    ///
    /// # Errors
    ///
    /// Propagates any error from [`EventLog::open`] (wrong key, I/O) or from
    /// [`EventLog::rebuild_indexes`] (embed failure, SQL error).
    ///
    /// # Note on highwater
    ///
    /// If you need both recall-ready open and truncation detection, open with
    /// [`EventLog::open_with_highwater`] then call `rebuild_indexes(embedder)`
    /// separately. A combined constructor is deferred to M7.
    pub fn open_with_recall(
        path: &Path,
        dek: &[u8; 32],
        key: SigningKey,
        embedder: &dyn Embedder,
    ) -> Result<Self, BossclawError> {
        let log = Self::open(path, dek, key)?;
        log.rebuild_indexes(embedder)?;
        Ok(log)
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

    /// Rebuild the in-memory vector index AND the FTS5 keyword index from the
    /// encrypted log for the active embedding model.
    ///
    /// **Vector rebuild:** reads every persisted vector for `embedder.model_id()`
    /// (via [`EventLog::vectors_for_model`], which returns rows `ORDER BY
    /// event_id ASC`), builds a fresh [`HnswIndex`] sized to the exact row
    /// count, and **serially** adds each `(event_id, vector)`.  Serial
    /// insertion over a deterministic row order is what makes the index
    /// reproducible across re-opens (spec F2).  The finished index replaces any
    /// previous one.  Because only `model_id`-matching rows are read, the index
    /// can only ever contain active-model vectors — cross-model bleed is
    /// impossible by construction (spec C4).
    ///
    /// **FTS rebuild:** wipes `fts` and `fts_map` entirely, then re-populates
    /// from every `memory`/`page` event (the same embeddable types that feed the
    /// vector index), scanned `ORDER BY seq ASC`.  No embedder is needed for
    /// this half — FTS indexes the raw event text.
    ///
    /// Both rebuilds are idempotent: calling this method twice leaves the indexes
    /// in the same state as calling it once.
    ///
    /// Emits [`log::info!`] timing lines so rebuild cost is visible before the
    /// recall benchmark (T9).
    pub fn rebuild_indexes(&self, embedder: &dyn Embedder) -> Result<(), BossclawError> {
        // ── Vector index rebuild ──────────────────────────────────────────────
        let vec_started = Instant::now();
        let rows = self.vectors_for_model(embedder.model_id())?;
        let vec_count = rows.len();
        let mut index = HnswIndex::with_capacity(vec_count);
        for (event_id, vec) in rows {
            index.add(&event_id, &vec);
        }
        let boxed: Box<dyn VectorIndex> = Box::new(index);
        *self.vector_index.lock().expect(POISON) = Some(boxed);
        log::info!(
            "rebuilt vector index: {vec_count} vectors in {}ms",
            vec_started.elapsed().as_millis()
        );

        // ── FTS5 keyword index rebuild ────────────────────────────────────────
        let fts_started = Instant::now();
        // Collect the events to index before taking the store lock (same
        // pattern as collect_pending — never hold the lock across I/O or
        // expensive work).
        let events_to_index = self.collect_embeddable_events_ordered()?;
        let fts_count = events_to_index.len();

        {
            let store = self.inner.lock().expect(POISON);
            let conn = store.conn();
            // Wipe the existing FTS index so this call is fully idempotent.
            // A contentless FTS5 table does not support plain `DELETE FROM
            // fts`; the FTS5 `delete-all` auxiliary command is the correct
            // API for clearing all indexed content.
            conn.execute_batch(
                "INSERT INTO fts(fts) VALUES('delete-all'); DELETE FROM fts_map;",
            )?;
        }

        // Re-populate row by row. Each keyword_add call takes and releases the
        // lock internally, keeping the lock-hold time minimal.
        for (event_id, text) in events_to_index {
            self.keyword_add(&event_id, &text)?;
        }

        log::info!(
            "rebuilt fts index: {fts_count} entries in {}ms",
            fts_started.elapsed().as_millis()
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

    /// Index an event in the FTS5 keyword index.
    ///
    /// The `event_id` / `text` pair is inserted into the `fts` virtual table
    /// (body column) and the corresponding rowid is recorded in `fts_map` so
    /// that keyword searches can return `event_id` values.
    ///
    /// **Idempotent by event_id:** if `event_id` already has a row in
    /// `fts_map` this method returns `Ok(())` immediately — no duplicate FTS
    /// entry is created.  Both the `fts` insert and the `fts_map` insert are
    /// performed in a single transaction to keep them consistent.
    pub fn keyword_add(&self, event_id: &str, text: &str) -> Result<(), BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();

        // Open the transaction first so the dedup check AND both inserts are
        // one atomic unit. The process-wide Mutex serializes all callers, so
        // the rowid captured immediately after the fts insert is unambiguous —
        // no other writer can have interleaved between the two statements.
        let tx = conn.unchecked_transaction()?;

        // Dedup check inside the transaction — eliminates the TOCTOU window
        // that would exist between a pre-tx read and the subsequent writes.
        let exists = tx
            .query_row(
                "SELECT 1 FROM fts_map WHERE event_id = ?1",
                rusqlite::params![event_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if exists {
            // tx drops here, rolling back (nothing was written).
            return Ok(());
        }

        tx.execute("INSERT INTO fts(body) VALUES (?1)", rusqlite::params![text])?;
        // last_insert_rowid is read from the same transaction object immediately
        // after the fts insert; Transaction derefs to Connection so the call is
        // identical in shape to conn.last_insert_rowid().
        let rowid = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO fts_map(rowid, event_id) VALUES (?1, ?2)",
            rusqlite::params![rowid, event_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Search the FTS5 keyword index for events whose body matches `query`.
    ///
    /// The raw query string is escaped via [`keyword::escape_fts_query`] before
    /// being passed to FTS5's `MATCH` operator, so user-supplied strings
    /// containing FTS5 operators or unbalanced quotes cannot alter query
    /// semantics or cause a parse error.
    ///
    /// Returns up to `k` `(event_id, score)` pairs ordered by BM25 rank
    /// (lower BM25 score = more relevant; T7's RRF fusion will normalise by
    /// rank position rather than raw score).
    ///
    /// An empty or whitespace-only `query` returns `Ok(vec![])` immediately —
    /// passing an empty string to FTS5 `MATCH` is a parse error, so we guard
    /// against it here.
    pub fn keyword_search(
        &self,
        query: &str,
        k: usize,
    ) -> Result<Vec<(String, f32)>, BossclawError> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let escaped = keyword::escape_fts_query(query);
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT m.event_id, bm25(fts) AS score
             FROM fts
             JOIN fts_map m ON m.rowid = fts.rowid
             WHERE fts MATCH ?1
             ORDER BY score
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![escaped, k as i64], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)? as f32))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Hybrid recall: embed the query, run BOTH retrieval arms, fuse by
    /// reciprocal rank, apply recency + pin boosts, rerank, and return the top-`k`
    /// [`Hit`]s with provenance. This is the heart of M2 (spec §5.7).
    ///
    /// # Pipeline
    /// 1. **Embed** `query` (one-item batch) → query vector.
    /// 2. **Two arms**, each fetching [`FUSION_FETCH`] candidates (≥ `k`, so
    ///    fusion sees enough tail to reorder): the vector arm
    ///    ([`EventLog::vector_search`]) and the keyword arm
    ///    ([`EventLog::keyword_search`]).
    /// 3. **Fuse** both arms (tie-aware RRF) → base score per id, while recording
    ///    which arm(s) surfaced each id (its [`RecallSource`]s).
    /// 4. **Boost** multiplicatively using **f64** throughout to avoid f32
    ///    precision underflow: recency `*= 1 + RECENCY_WEIGHT * exp(-age/HALF_LIFE)`
    ///    (age = now − the event's `ts`), and pin `*= PIN_MULTIPLIER` for ids in
    ///    `opts.pinned`. The recency tilt narrows but does not necessarily close
    ///    every adjacent-rank gap; it reorders candidates with equal or near-equal
    ///    fused base scores.
    /// 5. **Rerank** through the [`Reranker`] seam (v1: [`NoopReranker`]).
    /// 6. **Sort** by final score DESC, with **`ts` DESC** as the explicit
    ///    recency tie-break and **`event_id` DESC** as the final deterministic
    ///    backstop, then return the top `k`.
    ///
    /// ## Why the explicit `ts`-DESC tie-break is required
    ///
    /// The recency multiplier `1 + RECENCY_WEIGHT * exp(-age/HALF_LIFE_SECS)` is
    /// computed in f64 and then stored in `Hit.score` as f32. For events that are
    /// only milliseconds apart (common in tests), the f64 delta is on the order of
    /// 1e-11, which underflows to exactly `0.0` when cast to f32 — leaving two
    /// candidates with bit-identical f32 scores. A sort that breaks those ties by
    /// HashMap iteration order (random per process via hashbrown's random seed)
    /// would be non-deterministic ~30 % of runs. The `ts`-DESC comparator makes
    /// "newer wins ties" a hard guarantee independent of float precision.
    ///
    /// # Graceful degradation (spec §10)
    /// Recall is robust to a missing or unbuilt index. Arm resolution is handled
    /// by [`resolve_arms`]: a failing vector arm (embed error OR index not built —
    /// [`BossclawError::InvalidInput`]) is logged and recall degrades to
    /// **keyword-only**; a failing keyword arm degrades to **vector-only**; only
    /// when both fail is `Err` returned.
    ///
    /// # Lifecycle note
    /// The semantic (vector) arm reflects the index state at the last
    /// [`EventLog::rebuild_indexes`] / [`EventLog::open_with_recall`] call.
    /// **Events appended after that call are not yet in the vector index** and
    /// will only surface via the keyword arm until `rebuild_indexes(embedder)`
    /// is called again. This is intentional spec §10 graceful degradation, not a
    /// bug — but callers should be aware of the gap.
    pub fn recall(
        &self,
        embedder: &dyn Embedder,
        query: &str,
        k: usize,
        opts: &RecallOptions,
    ) -> Result<Vec<Hit>, BossclawError> {
        // ── Run both arms, applying spec §10 graceful degradation. ──
        let vector_result = embed_one(embedder, query)
            .and_then(|qv| self.vector_search(&qv, FUSION_FETCH));
        let keyword_result = self.keyword_search(query, FUSION_FETCH);
        let (vector_arm, keyword_arm) = resolve_arms(vector_result, keyword_result)?;

        // ── Provenance: which arm(s) surfaced each id (vector before keyword for
        //    a stable evidence order). Membership sets keep this O(1) per id. ──
        let vector_set: std::collections::HashSet<&String> =
            vector_arm.iter().map(|(id, _)| id).collect();
        let keyword_set: std::collections::HashSet<&String> =
            keyword_arm.iter().map(|(id, _)| id).collect();

        // ── Fuse both arms → base RRF score (f32) per id. Tie-aware: candidates
        //    with an identical arm score share a rank, so identical-text events get
        //    an EQUAL base, making the ts-DESC comparator below the deterministic
        //    tie-break (both arms rank lower scores first → lower_is_better=true). ──
        let fused = fuse_scored_arms(&[
            (vector_arm.as_slice(), true),
            (keyword_arm.as_slice(), true),
        ]);

        // ── Recency boost needs each candidate's ts; fetch them in one query. ──
        let candidate_ids: Vec<String> = fused.keys().cloned().collect();
        let timestamps = self.candidate_timestamps(&candidate_ids)?;
        let now = Utc::now();
        let pinned: std::collections::HashSet<&String> = opts.pinned.iter().collect();

        // ── Assemble hits: compute the full-precision (f64) boosted score, store
        //    it alongside the Hit so the sort comparator can use it without
        //    re-computing. Hit.score is set from the f64 value (truncated to f32
        //    for the public field) so callers get a reasonably precise score. ──
        let scored: Vec<(Hit, f64)> = fused
            .into_iter()
            .map(|(id, base_score)| {
                // Carry base score in f64 to avoid sub-millisecond recency deltas
                // underflowing when cast to f32 (see doc comment above).
                let mut score_f64 = base_score as f64;

                // Recency tilt: multiplicative, bounded by (1 + RECENCY_WEIGHT).
                // A candidate with no parseable ts gets factor 1.0 (no boost).
                if let Some(ts) = timestamps.get(&id) {
                    let age_secs = (now - *ts).num_milliseconds() as f64 / 1000.0;
                    let decay = (-age_secs / HALF_LIFE_SECS).exp();
                    score_f64 *= 1.0 + RECENCY_WEIGHT as f64 * decay;
                }

                // Pin: hard multiplicative boost for explicitly-pinned ids.
                if pinned.contains(&id) {
                    score_f64 *= PIN_MULTIPLIER as f64;
                }

                let mut sources = Vec::new();
                if vector_set.contains(&id) {
                    sources.push(RecallSource::Vector);
                }
                if keyword_set.contains(&id) {
                    sources.push(RecallSource::Keyword);
                }
                let hit = Hit { event_id: id, score: score_f64 as f32, sources };
                (hit, score_f64)
            })
            .collect();

        // ── Rerank (v1: identity). Split scored into (Hit, f64) components;
        //    keep the id→f64 map for the sort comparator. ──
        let reranker = NoopReranker;
        let mut id_to_score: std::collections::HashMap<String, f64> =
            HashMap::with_capacity(scored.len());
        let hits_only: Vec<Hit> = scored
            .into_iter()
            .map(|(h, s)| {
                id_to_score.insert(h.event_id.clone(), s);
                h
            })
            .collect();
        let mut hits = reranker.rerank(query, hits_only);

        // ── Sort: score_f64 DESC → ts DESC (newer wins) → event_id DESC (backstop).
        //    The ts-DESC key is the explicit recency tie-break that survives f32
        //    underflow (see doc comment). event_id DESC is the final deterministic
        //    backstop for candidates that genuinely share a ts (e.g. same-millisecond
        //    appends in tests). ──
        hits.sort_by(|a, b| {
            let sa = id_to_score.get(&a.event_id).copied().unwrap_or(0.0);
            let sb = id_to_score.get(&b.event_id).copied().unwrap_or(0.0);
            sb.total_cmp(&sa)
                .then_with(|| {
                    let ta = timestamps.get(&a.event_id);
                    let tb = timestamps.get(&b.event_id);
                    tb.cmp(&ta) // newer (larger DateTime) first
                })
                .then_with(|| b.event_id.cmp(&a.event_id)) // lexicographic DESC backstop
        });
        hits.truncate(k);
        Ok(hits)
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

    /// Collect `(event_id, text)` for every embeddable event, in `seq ASC`
    /// order, under a single short-lived lock.
    ///
    /// Used by [`EventLog::rebuild_indexes`] to populate the FTS keyword index.
    /// Only events whose `content["text"]` is a non-empty string are returned;
    /// events with missing or non-string `text` are silently skipped (their
    /// vectors would also be absent — see `embeddable_text`).
    fn collect_embeddable_events_ordered(&self) -> Result<Vec<(String, String)>, BossclawError> {
        let placeholders: String = EMBEDDABLE_EVENT_TYPES
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT id, payload FROM events WHERE event_type IN ({placeholders}) ORDER BY seq ASC"
        );
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> = EMBEDDABLE_EVENT_TYPES
            .iter()
            .map(|t| t as &dyn rusqlite::ToSql)
            .collect();
        let rows = stmt.query_map(params.as_slice(), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (event_id, payload) = row?;
            let event: Event = serde_json::from_str(&payload)?;
            if let Some(text) = embeddable_text(&event) {
                out.push((event_id, text));
            }
        }
        Ok(out)
    }

    /// Fetch the ingestion timestamp of each id in `ids`, parsed to
    /// [`DateTime<Utc>`], under a single short-lived lock.
    ///
    /// Used by [`EventLog::recall`] for the recency boost. The SQL is a single
    /// `SELECT id, ts FROM events WHERE id IN (...)` with one placeholder per id
    /// (matching the dynamic-`IN` pattern used by the other collectors). Ids not
    /// found in the log, or rows whose `ts` is not valid RFC 3339, are simply
    /// absent from the returned map — recall treats a missing ts as "no recency
    /// boost" rather than failing the whole query.
    ///
    /// An empty `ids` short-circuits to an empty map (an empty `IN ()` clause is
    /// a SQL syntax error).
    fn candidate_timestamps(
        &self,
        ids: &[String],
    ) -> Result<std::collections::HashMap<String, DateTime<Utc>>, BossclawError> {
        let mut out = std::collections::HashMap::new();
        if ids.is_empty() {
            return Ok(out);
        }
        let placeholders: String = (0..ids.len())
            .map(|i| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!("SELECT id, ts FROM events WHERE id IN ({placeholders})");
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(params.as_slice(), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (id, ts) = row?;
            // A malformed ts is non-fatal: skip it so the candidate just misses
            // the recency boost (factor 1.0) instead of failing the whole recall.
            match DateTime::parse_from_rfc3339(&ts) {
                Ok(parsed) => {
                    out.insert(id, parsed.with_timezone(&Utc));
                }
                Err(e) => {
                    log::warn!("recall: event {id} has unparseable ts {ts:?}: {e}");
                }
            }
        }
        Ok(out)
    }

    /// Switch the active embedding model, re-embed all events, GC stale vectors,
    /// and rebuild the in-memory indexes.
    ///
    /// # Steps (order is load-bearing for resumability)
    ///
    /// 1. **Config event** — append a `config` event naming `embedder` as the
    ///    new active model. The `schema_version` is inherited from the most
    ///    recent existing config, or [`SCHEMA_VERSION`] if no config exists yet.
    ///
    /// 2. **Re-embed** — [`EventLog::rederive_pending`] backfills every event
    ///    that lacks a vector for `embedder.model_id()`. Best-effort: individual
    ///    embed failures are logged and skipped.
    ///
    /// 3. **GC** — `DELETE FROM vectors WHERE model_id != embedder.model_id()`.
    ///    All rows for every other model are removed. The count of removed rows is
    ///    recorded in [`ReembedStats::gc_removed`].
    ///
    /// 4. **Rebuild** — [`EventLog::rebuild_indexes`] rebuilds both the ANN
    ///    vector index and the FTS5 keyword index under the new model.
    ///
    /// # Integrity note
    ///
    /// The active-model switch is recorded as a `config` event that is
    /// Ed25519-signed and hash-chained (M1), so a forged or replayed
    /// model-switch is tamper-evident via `verify_chain` / `verify_chain_since`.
    /// Surfacing a model-switch to the user as a recall-integrity alert is
    /// deferred to the desktop (M7).
    ///
    /// # Resumability
    ///
    /// A crash between the config switch (step 1) and the GC (step 3) is
    /// correctness-safe: recall is active-model-filtered, so stale rows for
    /// the old model are simply ignored. Re-running `reembed_migration` (or
    /// the next migration) completes the GC, making this operation
    /// idempotent/resumable. A second run re-embeds 0 (nothing pending) and
    /// GCs 0 (stale rows already removed), leaving one consistent active model.
    ///
    /// A crash after the GC (step 3) but before `rebuild_indexes` completes
    /// (step 4) is also safe: the `vectors` table already contains only the new
    /// model's rows (no data loss), and the in-memory index is simply stale or
    /// absent. Recovery is a single call to `rebuild_indexes(embedder)`, which
    /// is also what a normal reopen + rebuild does — no special handling needed.
    ///
    /// # Lock discipline
    ///
    /// The single-store [`Mutex`] is never held across [`Embedder::embed`]
    /// calls. Re-embedding is delegated to [`EventLog::rederive_pending`] which
    /// already implements that discipline. The GC `DELETE` is a short, bounded
    /// operation and holds the lock only for that one statement.
    ///
    /// # Returns
    ///
    /// [`ReembedStats`] carrying `reembedded`, `gc_removed`, and `elapsed_ms`
    /// — the §15 time-budget observability signal.
    pub fn reembed_migration(
        &self,
        embedder: &dyn Embedder,
    ) -> Result<ReembedStats, BossclawError> {
        let migration_start = Instant::now();

        // Step 1: append a config event selecting the new active model.
        // Reuse the existing schema_version if a config already exists.
        let schema_version = self
            .active_model()?
            .map(|m| m.schema_version)
            .unwrap_or(SCHEMA_VERSION);

        self.append(Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: "config".to_string(),
            content: serde_json::json!({
                "active_model_id": embedder.model_id(),
                "dim": embedder.dim() as u32,
                "schema_version": schema_version,
            }),
            model_meta: None,
            prev_hash: String::new(),
            hash: None,
            signed_by_did: "did:wba:bossclaw-migration".to_string(),
            signature: None,
        })?;

        // Step 2: re-embed every event missing a vector for the new model.
        let reembedded = self.rederive_pending(embedder)?;

        // Step 3: GC — delete all vectors for every model OTHER than the new one.
        // Hold the lock only for this short DELETE statement.
        let gc_removed = {
            let store = self.inner.lock().expect(POISON);
            let conn = store.conn();
            conn.execute(
                "DELETE FROM vectors WHERE model_id != ?1",
                rusqlite::params![embedder.model_id()],
            )?;
            conn.changes() as usize
        };

        // Step 4: rebuild the in-memory ANN + FTS indexes under the new model.
        self.rebuild_indexes(embedder)?;

        // Count total events BEFORE stopping the clock so `elapsed_ms` spans
        // the whole operation including this query.
        let total_events = self.count()?;
        let elapsed_ms = migration_start.elapsed().as_millis();

        // Avoid division by zero; an idempotent re-run (reembedded == 0) or a
        // migration on an empty store are both valid. Report the throughput label
        // as "reembedded/sec" (not "events/sec") so a 0-reembed idempotent run
        // prints "0 reembedded/sec" without ambiguity.
        let reembedded_per_sec = if elapsed_ms > 0 {
            reembedded as f64 / (elapsed_ms as f64 / 1000.0)
        } else {
            f64::INFINITY
        };
        log::info!(
            "re-embed migration: {} vectors re-embedded in {}ms ({:.0} reembedded/sec); \
             gc_removed={} total_events={} model={}",
            reembedded,
            elapsed_ms,
            reembedded_per_sec,
            gc_removed,
            total_events,
            embedder.model_id(),
        );

        Ok(ReembedStats { reembedded, gc_removed, elapsed_ms })
    }

    /// Append a signed Tier-B `link` event connecting `src` —`relation`→ `dst`.
    ///
    /// `valid_time` (optional, RFC 3339) is the world-clock start; absent means
    /// "valid from when we learned it" (the event's ingestion `ts`). If
    /// `source_event_ids` is empty it defaults to `[src, dst]` so the Tier-B
    /// non-empty-provenance rule is satisfied honestly (the two endpoints justify
    /// the link). Returns the new event id (which is also the edge's identity).
    ///
    /// The `edges` table is NOT updated here — call [`EventLog::rebuild_graph`]
    /// to refresh `neighbors`/`as_of`/the recall boost (same "rebuild after
    /// append" lifecycle as [`EventLog::rebuild_indexes`]).
    pub fn link(
        &self,
        src: &str,
        relation: &str,
        dst: &str,
        valid_time: Option<&str>,
        source_event_ids: &[String],
    ) -> Result<String, BossclawError> {
        self.append_graph_event("link", MANUAL_LINK_PRODUCER, src, relation, dst, valid_time, source_event_ids)
    }

    /// Append a signed Tier-B `invalidate` event retiring the edge-key
    /// `(src, relation, dst)`. `valid_time` (optional) is when the fact stopped
    /// being true in the world. Same `source_event_ids` defaulting and lifecycle
    /// as [`EventLog::link`].
    pub fn invalidate(
        &self,
        src: &str,
        relation: &str,
        dst: &str,
        valid_time: Option<&str>,
        source_event_ids: &[String],
    ) -> Result<String, BossclawError> {
        self.append_graph_event("invalidate", MANUAL_LINK_PRODUCER, src, relation, dst, valid_time, source_event_ids)
    }

    /// Shared builder for `link`/`invalidate`. The `[src, dst]` convenience
    /// default for `source_event_ids` is gated to the manual producer only.
    ///
    /// **SECURITY (taint, parent §5.11):** the `[src, dst]` default is for
    /// MANUAL (engine/test) links only — there the two endpoints genuinely ARE
    /// the whole justification. A non-manual producer (the M4 reasoner) MUST
    /// pass its real read-set; defaulting there would erase the inducing event
    /// from the lineage the actuator walks fail-closed.
    ///
    // The `producer` parameter is required by the F2 security gate; the remaining
    // args are the event's intrinsic fields. A params struct would add indirection
    // without safety benefit for this private, two-call-site helper.
    #[allow(clippy::too_many_arguments)]
    fn append_graph_event(
        &self,
        event_type: &str,
        producer: &str,
        src: &str,
        relation: &str,
        dst: &str,
        valid_time: Option<&str>,
        source_event_ids: &[String],
    ) -> Result<String, BossclawError> {
        let sources = match (producer == MANUAL_LINK_PRODUCER, source_event_ids.is_empty()) {
            (true, true) => vec![src.to_string(), dst.to_string()],
            (false, true) => {
                // Caller-argument-policy rejection → InvalidInput (not Chain, which
                // is for hash/chain-integrity failures). NB: M1's analogous empty-
                // source guard in `append` uses Chain (pre-existing; a candidate for
                // a later unify — do NOT change M1 here).
                return Err(BossclawError::InvalidInput(
                    "non-manual graph link requires explicit source_event_ids (no [src,dst] \
                     default — would launder taint past the §5.11 lineage walk)".into(),
                ));
            }
            (_, false) => source_event_ids.to_vec(),
        };
        self.append(Event {
            id: String::new(),
            ts: String::new(),
            valid_time: valid_time.map(String::from),
            event_type: event_type.to_string(),
            content: serde_json::json!({ "src": src, "relation": relation, "dst": dst }),
            model_meta: Some(ModelMeta {
                model_id: producer.to_string(),
                prompt_hash: String::new(),
                source_event_ids: sources,
            }),
            prev_hash: String::new(),
            hash: None,
            signed_by_did: self.signer_did(),
            signature: None,
        })
    }

    /// The DID stamped on engine-authored events (`link`/`invalidate`). v1 uses a
    /// fixed engine identity; M4/M7 will thread the user's real DID through here.
    ///
    /// Note: `signed_by_did` is informational here (not verified against `key` at
    /// append). A fixed engine DID keeps the M3 surface small; threading the user
    /// DID is M4/M7 (carried, security I3).
    ///
    /// Returns an owned `String` (not the `&'static str` const) because M4/M7 will
    /// make this dynamic — the user's real DID, looked up per call.
    fn signer_did(&self) -> String {
        ENGINE_SIGNER_DID.to_string()
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

/// Resolve the raw results of the two recall arms into live arm data, applying
/// spec §10 graceful degradation.
///
/// | vector result | keyword result | outcome |
/// |---|---|---|
/// | `Ok(hits)` | `Ok(hits)` | both arms active |
/// | `Err(_)` | `Ok(hits)` | keyword-only (vector failure logged) |
/// | `Ok(hits)` | `Err(_)` | vector-only (keyword failure logged) |
/// | `Err(ve)` | `Err(_)` | `Err(InvalidInput(…ve…))` |
///
/// This is a **pure** function (no I/O, no `self`) so it can be unit-tested
/// directly without a database. `recall` delegates the arm-failure logic here.
pub fn resolve_arms(
    vector: Result<Vec<ArmHit>, BossclawError>,
    keyword: Result<Vec<ArmHit>, BossclawError>,
) -> Result<ArmPair, BossclawError> {
    match (vector, keyword) {
        (Ok(v), Ok(k)) => Ok((v, k)),
        (Err(ve), Ok(k)) => {
            log::warn!("recall: vector arm unavailable, degrading to keyword-only: {ve}");
            Ok((Vec::new(), k))
        }
        (Ok(v), Err(ke)) => {
            log::warn!("recall: keyword arm unavailable, degrading to vector-only: {ke}");
            Ok((v, Vec::new()))
        }
        (Err(ve), Err(ke)) => {
            log::warn!("recall: both arms unavailable (keyword: {ke})");
            Err(BossclawError::InvalidInput(format!(
                "recall failed: both arms unavailable (vector: {ve})"
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEK: [u8; 32] = [42u8; 32];
    const KEY_BYTES: [u8; 32] = [7u8; 32];

    fn open_log(dir: &Path) -> EventLog {
        let key = SigningKey::from_bytes(&KEY_BYTES);
        EventLog::open(&dir.join("m.db"), &DEK, key).unwrap()
    }

    /// F2 security gate (parent §5.11): the private `append_graph_event` defaults
    /// `source_event_ids` to `[src, dst]` ONLY for the manual producer; a
    /// non-manual producer with an empty source set is REJECTED so taint cannot be
    /// laundered past the lineage walk. This unit test reaches the private helper
    /// directly (the public `link`/`invalidate` always pass `MANUAL_LINK_PRODUCER`,
    /// so they can never trigger the reject arm).
    #[test]
    fn append_graph_event_rejects_non_manual_producer_with_empty_sources() {
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());

        // Non-manual producer + empty sources → the F2 reject arm fires.
        let err = log
            .append_graph_event("link", "m4-reasoner", "a", "works_at", "b", None, &[])
            .expect_err("non-manual producer with empty sources must be rejected");
        match err {
            BossclawError::InvalidInput(msg) => assert!(
                msg.contains("non-manual"),
                "reject message should name the non-manual gate, got: {msg}"
            ),
            other => panic!("expected BossclawError::InvalidInput, got {other:?}"),
        }

        // Manual producer + empty sources → succeeds, defaulting to [src, dst].
        let id = log
            .append_graph_event("link", MANUAL_LINK_PRODUCER, "a", "works_at", "b", None, &[])
            .expect("manual producer with empty sources must succeed");
        let ev = log.stream_all().unwrap().into_iter().find(|e| e.id == id).unwrap();
        let meta = ev.model_meta.expect("link is Tier-B");
        assert_eq!(meta.model_id, MANUAL_LINK_PRODUCER);
        assert_eq!(
            meta.source_event_ids,
            vec!["a".to_string(), "b".to_string()],
            "manual empty-source link defaults to [src, dst]"
        );
    }
}
