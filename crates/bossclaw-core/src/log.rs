//! The append-only event log. The single source of truth.
//!
//! Appends are strictly serialized: one process-wide `Mutex` guards the
//! read-tip → hash → sign → insert critical section, so the hash chain can
//! never fork (spec §4 single-writer invariant). The evolve loop (M4) is NOT a
//! privileged writer — it calls `append` like everyone else.

use std::collections::{BTreeMap, HashMap, HashSet};
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
use crate::evolve::{EvolveReport, EvolveStatus};
use crate::extract::{ResolveDecision, EVOLVE_BATCH, MAX_ENTITIES_PER_MEMORY, MAX_REFLECT};
use crate::graph::{
    entity_node_id, CONFIG_EVENT_TYPE, ENTITY_EVENT_TYPE, ENTITY_NODE_KIND, EXTERNAL_NODE_KIND,
    MANUAL_LINK_PRODUCER, MEMORY_EVENT_TYPE, MEMORY_NODE_KIND, UNRESOLVED_ENTITY_TYPE,
};
use crate::highwater::{HighWaterStore, Mark};
use crate::index::{HnswIndex, VectorIndex};
use crate::keyword;
use crate::recall::{
    fuse_scored_arms, Hit, NoopReranker, RecallOptions, RecallSource, Reranker, FUSION_FETCH,
    GRAPH_HOP_DECAY, GRAPH_MAX_HOPS, GRAPH_REINFORCE_TOPK, GRAPH_WEIGHT, HALF_LIFE_SECS,
    PIN_MULTIPLIER, RECENCY_WEIGHT,
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

/// The `content` key carrying the evolve on/off switch in a control `config`
/// event (spec §8 / Rev 2 F2-sec). Single-sourced so the ONE writer
/// ([`EventLog::set_evolve_enabled`]) and the reader ([`EventLog::evolve_enabled`])
/// can never drift the key apart — a typo in one would silently disarm the
/// fail-closed off-switch.
const EVOLVE_ENABLED_KEY: &str = "evolve_enabled";

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
    /// In-memory ANN index over `entity`-event vectors ONLY, for entity
    /// resolution (spec §6). Physically separate from `vector_index` so recall
    /// can never surface an entity node and resolution can never match a memory.
    /// `None` until [`EventLog::rebuild_entity_index`]; rebuilt from the encrypted
    /// log on open (zero plaintext index on disk, like the recall index).
    entity_index: Mutex<Option<Box<dyn VectorIndex>>>,
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
                edge_id          TEXT PRIMARY KEY,
                src              TEXT NOT NULL,
                relation         TEXT NOT NULL,
                dst              TEXT NOT NULL,
                valid_from       TEXT NOT NULL,
                valid_to         TEXT,
                ingested_at      TEXT NOT NULL,
                invalidated_at   TEXT,
                invalidated_by   TEXT,
                origin           TEXT NOT NULL DEFAULT 'manual',
                confidence_milli INTEGER
            )",
        )?;
        store.exec(
            "CREATE TABLE IF NOT EXISTS nodes (
                node_id TEXT PRIMARY KEY,
                kind    TEXT NOT NULL
            )",
        )?;
        // Entity projection (Tier-A; spec §4). One row per `entity` event,
        // id = "entity:<event ulid>". A deterministic fold over entity events,
        // rebuilt by `rebuild_graph`. The label is a property, never the id.
        store.exec(
            "CREATE TABLE IF NOT EXISTS entities (
                entity_id   TEXT PRIMARY KEY,
                label       TEXT NOT NULL,
                aliases     TEXT NOT NULL,
                entity_type TEXT NOT NULL
            )",
        )?;
        // Entity-resolution vectors (Tier-A derived; spec §6). Separate from
        // `vectors` so the resolution index NEVER mixes with the recall index —
        // recall must exclude entity-kind, resolution searches only entity-kind.
        store.exec(
            "CREATE TABLE IF NOT EXISTS entity_vectors (
                entity_id TEXT NOT NULL,
                model_id  TEXT NOT NULL,
                dim       INTEGER NOT NULL,
                embedding BLOB NOT NULL,
                PRIMARY KEY(entity_id, model_id)
            )",
        )?;
        // Evolve-loop progress (re-derivable progress state — NOT a Tier-A fold,
        // spec §4). Single row (id pinned to 0), advanced after each committed
        // batch. Losing it only re-processes events (idempotent: an active
        // edge-key is skipped and a resolved entity is reused), never corrupts.
        store.exec(
            "CREATE TABLE IF NOT EXISTS evolve_cursor (
                id       INTEGER PRIMARY KEY CHECK (id = 0),
                last_seq INTEGER NOT NULL
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
            entity_index: Mutex::new(None),
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
        log.rebuild_graph()?; // graph (+ its recall boost) live on open; persisted edges survive reopen
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
    /// `config` event that CARRIES the model fields.
    ///
    /// A model-config event has `event_type = "config"` and a `content` object
    /// with `active_model_id`, `dim`, and `schema_version`. Config events are
    /// scanned newest-first and the first one that successfully parses as an
    /// [`ActiveModel`] wins; configs that carry only other control keys (e.g. a
    /// control `config` setting just `evolve_enabled`, Rev 2 F2-sec(c)) are
    /// SKIPPED rather than erroring — the on/off switch and the active model are
    /// independent control keys that may be set in separate config events.
    ///
    /// Returns `Ok(None)` if no `config` event carries the model fields.
    pub fn active_model(&self) -> Result<Option<ActiveModel>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT payload FROM events WHERE event_type = ?1 ORDER BY seq DESC",
        )?;
        let rows = stmt.query_map([CONFIG_EVENT_TYPE], |r| r.get::<_, String>(0))?;
        for row in rows {
            let event: Event = serde_json::from_str(&row?)?;
            // Tolerant: a config lacking the model fields is a different control
            // config (e.g. evolve_enabled-only). Skip it, do not error.
            if let Ok(model) = serde_json::from_value::<ActiveModel>(event.content) {
                return Ok(Some(model));
            }
        }
        Ok(None)
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

        // ── Graph-proximity seeds: explicit, else auto-seed from the top fused
        //    base score(s). Then BFS current-edge neighbors (best-effort: a graph
        //    error degrades to no boost, never failing recall — spec §6/§10). ──
        let seeds: Vec<String> = if !opts.graph_seeds.is_empty() {
            opts.graph_seeds.clone()
        } else {
            // Intra-result reinforcement (spec §7 / Rev 2): auto-seed expands from
            // the single top-1 hit (M3's GRAPH_AUTO_SEED_TOPK) to the top
            // GRAPH_REINFORCE_TOPK fused hits — a memory linked to ANY of the
            // result set's strong hits gets the proximity tilt, not only neighbors
            // of the single strongest hit.
            let mut by_score: Vec<(&String, &f32)> = fused.iter().collect();
            by_score.sort_by(|a, b| {
                // id desc = deterministic tie-break only (not semantically meaningful).
                b.1.total_cmp(a.1).then_with(|| b.0.cmp(a.0))
            });
            by_score.into_iter().take(GRAPH_REINFORCE_TOPK).map(|(id, _)| id.clone()).collect()
        };
        let graph_hops = self
            .current_neighbors_with_hops(&seeds, GRAPH_MAX_HOPS)
            .unwrap_or_else(|e| {
                log::warn!("recall: graph-proximity boost skipped: {e}");
                HashMap::new()
            });

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

                // Graph-proximity tilt: a current-edge neighbour of a seed is
                // boosted by 1 + GRAPH_WEIGHT * GRAPH_HOP_DECAY^(hops-1).
                if let Some(&hop) = graph_hops.get(&id) {
                    let decay = (GRAPH_HOP_DECAY as f64).powi(hop as i32 - 1);
                    score_f64 *= 1.0 + GRAPH_WEIGHT as f64 * decay;
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
            event_type: CONFIG_EVENT_TYPE.to_string(),
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

    /// Append a signed Tier-B MACHINE `link` carrying its `confidence` as an
    /// INTEGER `confidence_milli` (0..=1000) in the signed CONTENT (spec §4/§7;
    /// Rev 2 F3 — never a raw `f32`, never in `ModelMeta`). For the M4a reasoner:
    /// a NON-MANUAL producer, so `source_event_ids` MUST be non-empty (the F2
    /// taint guard rejects an empty set — an empty default would launder taint
    /// past the §5.11 lineage walk).
    ///
    /// `confidence` is clamped to `[0.0, 1.0]` then quantized to integer milli
    /// (`(c.clamp(0.0,1.0) * 1000.0).round() as i64`) so the JCS-canonical signed
    /// bytes have ONE deterministic form — a float would risk
    /// [`EventLog::verify_chain`] breaking across `serde_jcs` versions on this
    /// append-only signed store. The value projects to `edges.confidence_milli`
    /// and gates the recall boost (spec §7): a machine edge below
    /// [`crate::extract::TRUST_MIN`] is recorded + queryable but does NOT tilt
    /// recall. The `producer` MUST NOT be [`MANUAL_LINK_PRODUCER`] (a machine link
    /// is, by definition, non-manual — that is what makes `origin = "machine"`).
    /// Returns the new edge event's id.
    ///
    /// The `edges` table is NOT updated here — call [`EventLog::rebuild_graph`].
    pub fn link_machine(
        &self,
        src: &str,
        relation: &str,
        dst: &str,
        confidence: f32,
        producer: &str,
        source_event_ids: &[String],
    ) -> Result<String, BossclawError> {
        if source_event_ids.is_empty() {
            return Err(BossclawError::InvalidInput(
                "machine link requires explicit non-empty source_event_ids (the cheat-sheet \
                 read-set) — an empty default would launder taint past the §5.11 lineage walk"
                    .into(),
            ));
        }
        // A machine link is, by definition, NON-manual — that is what makes
        // origin = "machine" and keeps its confidence. A manual producer would
        // silently fold as a manual edge with confidence discarded. The producer
        // is engine-internal (never user input), so a debug_assert is the right
        // guard: it catches a wiring mistake in tests/dev without a release cost.
        debug_assert!(
            producer != MANUAL_LINK_PRODUCER,
            "link_machine producer must be non-manual"
        );
        // Integer milli (Rev 2 F3): clamp to [0,1] then quantize — single-sourced
        // in extract so the encode side and the trust-gate threshold can never
        // diverge. ONE canonical JCS form, no f32/f64 ambiguity in SIGNED content.
        let confidence_milli = crate::extract::to_confidence_milli(confidence);
        self.append(Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: "link".to_string(),
            content: serde_json::json!({
                "src": src,
                "relation": relation,
                "dst": dst,
                "confidence_milli": confidence_milli,
            }),
            model_meta: Some(ModelMeta {
                model_id: producer.to_string(),
                prompt_hash: String::new(),
                source_event_ids: source_event_ids.to_vec(),
            }),
            prev_hash: String::new(),
            hash: None,
            signed_by_did: self.signer_did(),
            signature: None,
        })
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

    /// Append a signed Tier-B `entity` event minting a stable `entity:<ulid>`
    /// node carrying `{label, aliases, entity_type}` (spec §4). Returns the
    /// namespaced node id `entity:<event id>` (NOT the bare event id) — the form
    /// links reference.
    ///
    /// `entity` is a NON-MANUAL producer: `source_event_ids` MUST be non-empty
    /// (the memory/-ies that introduced the entity). An empty source set is
    /// rejected (the M3 F2 taint guard, parent §5.11) — defaulting here would
    /// erase the inducing memory from the lineage the actuator walks fail-closed.
    ///
    /// The `entities` table is NOT updated here — call [`EventLog::rebuild_graph`]
    /// to refresh it (same append→rebuild lifecycle as [`EventLog::link`]).
    pub fn entity(
        &self,
        label: &str,
        aliases: &[String],
        entity_type: &str,
        producer: &str,
        source_event_ids: &[String],
    ) -> Result<String, BossclawError> {
        if source_event_ids.is_empty() {
            // entity is never the manual producer; an empty source set is always
            // a taint-laundering reject (mirrors `append_graph_event`'s F2 arm).
            return Err(BossclawError::InvalidInput(
                "entity event requires explicit non-empty source_event_ids (the inducing \
                 memory) — an empty default would erase it from the §5.11 lineage walk"
                    .into(),
            ));
        }
        let event_id = self.append(Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: ENTITY_EVENT_TYPE.to_string(),
            content: serde_json::json!({
                "label": label,
                "aliases": aliases,
                "entity_type": entity_type,
            }),
            model_meta: Some(ModelMeta {
                model_id: producer.to_string(),
                prompt_hash: String::new(),
                source_event_ids: source_event_ids.to_vec(),
            }),
            prev_hash: String::new(),
            hash: None,
            signed_by_did: self.signer_did(),
            signature: None,
        })?;
        Ok(entity_node_id(&event_id))
    }

    /// Every entity, `ORDER BY entity_id ASC` (deterministic). Tier-A read.
    pub fn all_entities(&self) -> Result<Vec<crate::graph::Entity>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT entity_id, label, aliases, entity_type \
             FROM entities ORDER BY entity_id ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            let aliases_json: String = r.get(2)?;
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                aliases_json,
                r.get::<_, String>(3)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (entity_id, label, aliases_json, entity_type) = row?;
            // aliases is stored as a JSON array string; a malformed value degrades
            // to empty rather than failing the read (best-effort, matches the fold).
            let aliases: Vec<String> =
                serde_json::from_str(&aliases_json).unwrap_or_default();
            out.push(crate::graph::Entity { entity_id, label, aliases, entity_type });
        }
        Ok(out)
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

    /// Rebuild the persisted `edges`/`nodes` tables as a deterministic fold over
    /// every `link`/`invalidate` event (`ORDER BY seq ASC`). Tier-A: byte-
    /// identical across rebuilds (spec §4/§9). Wipes both tables and re-inserts
    /// under one transaction. Cheap (graph events are few). Call after appending
    /// `link`/`invalidate` events to refresh `neighbors`/`as_of`/the recall boost.
    ///
    /// **Lifecycle:** graph queries and the recall boost reflect the `edges`
    /// table as of the last `rebuild_graph` / [`EventLog::open_with_recall`].
    /// After appending `link`/`invalidate` events WITHIN a session, call
    /// `rebuild_graph` again — the same append→rebuild lifecycle as
    /// [`EventLog::rebuild_indexes`].
    pub fn rebuild_graph(&self) -> Result<(), BossclawError> {
        let started = Instant::now();
        let events = self.graph_events_ordered()?;
        let edges = crate::graph::fold_edges(&events);
        // F4: a signed link/invalidate with malformed content is silently
        // dropped by the fold (it never becomes an edge). Surface the count so
        // malformed-but-signed events are not invisible.
        let malformed = events
            .iter()
            .filter(|e| crate::graph::parse_link_content(&e.content).is_none())
            .count();
        if malformed > 0 {
            log::warn!(
                "rebuild_graph: {malformed} link/invalidate event(s) had malformed content \
                 and were skipped"
            );
        }

        // Fold entity events → entities projection + the set of entity node ids
        // (used to label node kind "entity" rather than "external").
        let entity_events = self.entity_events_ordered()?;
        let entities = crate::graph::fold_entities(&entity_events);
        // Set of entity node ids → used to mark node kind "entity" (overrides
        // the "external" default for ids the edges reference).
        let entity_ids: HashSet<String> =
            entities.iter().map(|e| e.entity_id.clone()).collect();

        let memory_ids = self.memory_page_ids()?;

        // Distinct endpoints → nodes (BTreeMap = deterministic node order).
        let mut node_kinds: BTreeMap<String, String> = BTreeMap::new();
        for e in &edges {
            for endpoint in [&e.src, &e.dst] {
                node_kinds.entry(endpoint.clone()).or_insert_with(|| {
                    if entity_ids.contains(endpoint) {
                        ENTITY_NODE_KIND.to_string()
                    } else if memory_ids.contains(endpoint) {
                        MEMORY_NODE_KIND.to_string()
                    } else {
                        EXTERNAL_NODE_KIND.to_string()
                    }
                });
            }
        }

        let edge_count = edges.len();
        let node_count = node_kinds.len();
        let entity_count = entities.len();
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let tx = conn.unchecked_transaction()?;
        tx.execute("DELETE FROM edges", [])?;
        tx.execute("DELETE FROM nodes", [])?;
        tx.execute("DELETE FROM entities", [])?;
        for e in &edges {
            tx.execute(
                "INSERT INTO edges
                   (edge_id, src, relation, dst, valid_from, valid_to,
                    ingested_at, invalidated_at, invalidated_by, origin, confidence_milli)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                rusqlite::params![
                    e.edge_id, e.src, e.relation, e.dst, e.valid_from, e.valid_to,
                    e.ingested_at, e.invalidated_at, e.invalidated_by, e.origin, e.confidence_milli
                ],
            )?;
        }
        for (node_id, kind) in &node_kinds {
            tx.execute(
                "INSERT INTO nodes (node_id, kind) VALUES (?1, ?2)",
                rusqlite::params![node_id, kind],
            )?;
        }
        for e in &entities {
            tx.execute(
                "INSERT INTO entities (entity_id, label, aliases, entity_type)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    e.entity_id,
                    e.label,
                    // JSON array string — serde_json Vec<String> serialization is
                    // deterministic (array order preserved), so the stored string
                    // is byte-stable across rebuilds (byte-identical-rebuild holds).
                    serde_json::to_string(&e.aliases)?,
                    e.entity_type
                ],
            )?;
        }
        tx.commit()?;
        log::info!(
            "rebuilt graph: {edge_count} edges, {node_count} nodes, \
             {entity_count} entities in {}ms",
            started.elapsed().as_millis()
        );
        Ok(())
    }

    /// All `link`/`invalidate` events, payload-parsed, in chain (`seq ASC`) order.
    fn graph_events_ordered(&self) -> Result<Vec<Event>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT payload FROM events
             WHERE event_type IN ('link', 'invalidate') ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(serde_json::from_str(&row?)?);
        }
        Ok(out)
    }

    /// All `entity` events, payload-parsed, in chain (`seq ASC`) order.
    ///
    /// Used by [`EventLog::rebuild_graph`] to fold entity events into the
    /// `entities` projection. Parameterised query only — no string interpolation.
    fn entity_events_ordered(&self) -> Result<Vec<Event>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT payload FROM events WHERE event_type = 'entity' ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(serde_json::from_str(&row?)?);
        }
        Ok(out)
    }

    /// Set of event ids whose type is `memory`/`page` — used to label node kinds.
    fn memory_page_ids(&self) -> Result<HashSet<String>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt =
            conn.prepare("SELECT id FROM events WHERE event_type IN ('memory', 'page')")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = HashSet::new();
        for row in rows {
            out.insert(row?);
        }
        Ok(out)
    }

    /// Every edge, `ORDER BY edge_id ASC` (deterministic). Tier-A read.
    pub fn all_edges(&self) -> Result<Vec<crate::graph::Edge>, BossclawError> {
        self.query_edges(
            "SELECT edge_id, src, relation, dst, valid_from, valid_to, \
                ingested_at, invalidated_at, invalidated_by, origin, confidence_milli \
             FROM edges ORDER BY edge_id ASC",
            &[],
        )
    }

    /// Every node, `ORDER BY node_id ASC`.
    pub fn all_nodes(&self) -> Result<Vec<crate::graph::Node>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare("SELECT node_id, kind FROM nodes ORDER BY node_id ASC")?;
        let rows = stmt.query_map([], |r| {
            Ok(crate::graph::Node { node_id: r.get(0)?, kind: r.get(1)? })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Current edges touching `node` in either direction (`invalidated_at IS
    /// NULL`). The result includes:
    ///
    /// - **Outgoing** edges where `src == node`.
    /// - **Incoming** (backlink) edges where `dst == node`.
    /// - **Self-loops** where `src == dst == node` (appear exactly once, not
    ///   twice — `OR` on a single row is still one row).
    ///
    /// Caller can filter for backlinks with `.iter().filter(|e| e.dst == node)`.
    /// `ORDER BY edge_id ASC` for deterministic output.
    pub fn neighbors(&self, node: &str) -> Result<Vec<crate::graph::Edge>, BossclawError> {
        self.query_edges(
            "SELECT edge_id, src, relation, dst, valid_from, valid_to, \
                ingested_at, invalidated_at, invalidated_by, origin, confidence_milli \
             FROM edges \
             WHERE (src = ?1 OR dst = ?1) AND invalidated_at IS NULL \
             ORDER BY edge_id ASC",
            &[&node as &dyn rusqlite::ToSql],
        )
    }

    /// Bi-temporal edge query for `node` (spec §5). Both `AsOf` axes are optional
    /// `WHERE` filters layered on the persisted edges:
    /// - `valid_time` t → `valid_from <= t AND (valid_to IS NULL OR t < valid_to)`
    ///   ("true in the world at t").
    /// - `known_as_of` t → `ingested_at <= t AND (invalidated_at IS NULL OR
    ///   t < invalidated_at)` ("known at t").
    ///
    /// When BOTH axes are `None`, returns the current graph (`invalidated_at IS
    /// NULL`), identical to [`EventLog::neighbors`]. Query timestamps are
    /// normalized with [`crate::graph::normalize_ts`] so TEXT comparison is
    /// chronological. `ORDER BY edge_id ASC`.
    pub fn as_of(
        &self,
        node: &str,
        as_of: &crate::graph::AsOf,
    ) -> Result<Vec<crate::graph::Edge>, BossclawError> {
        let mut sql = String::from(
            "SELECT edge_id, src, relation, dst, valid_from, valid_to, \
                ingested_at, invalidated_at, invalidated_by, origin, confidence_milli \
             FROM edges WHERE (src = ?1 OR dst = ?1)",
        );

        // F1 (clippy `redundant_closure` trap): normalize_ts takes `&str` but
        // the closure arg is `&String`; `.as_str()` makes the deref explicit so
        // clippy does NOT suggest `.map(normalize_ts)` (which would compile-fail).
        let valid = as_of.valid_time.as_ref().map(|t| crate::graph::normalize_ts(t.as_str()));
        let known = as_of.known_as_of.as_ref().map(|t| crate::graph::normalize_ts(t.as_str()));

        // Owned, normalized param strings kept alive for the bind slice below.
        let mut owned: Vec<String> = Vec::new();

        // SQL params are 1-indexed; ?1 is `node`, so the k-th owned timestamp
        // binds to ?{owned.len()+2}. Both `+2` sites below share this invariant.
        match (&valid, &known) {
            (None, None) => sql.push_str(" AND invalidated_at IS NULL"),
            _ => {
                if let Some(t) = &valid {
                    let i = owned.len() + 2; // ?1 is node
                    sql.push_str(&format!(
                        " AND valid_from <= ?{i} AND (valid_to IS NULL OR ?{i} < valid_to)"
                    ));
                    owned.push(t.clone());
                }
                if let Some(t) = &known {
                    let i = owned.len() + 2;
                    sql.push_str(&format!(
                        " AND ingested_at <= ?{i} AND (invalidated_at IS NULL OR ?{i} < invalidated_at)"
                    ));
                    owned.push(t.clone());
                }
            }
        }
        sql.push_str(" ORDER BY edge_id ASC");

        let mut params: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(1 + owned.len());
        params.push(&node as &dyn rusqlite::ToSql);
        for t in &owned {
            params.push(t as &dyn rusqlite::ToSql);
        }
        self.query_edges(&sql, &params)
    }

    /// Run a SELECT that returns the full edge column list (in the fixed order
    /// used by [`EventLog::all_edges`]) and map rows to [`crate::graph::Edge`].
    /// Shared by `all_edges`, `neighbors`, and `as_of` so the column→field
    /// mapping is single-sourced.
    fn query_edges(
        &self,
        sql: &str,
        params: &[&dyn rusqlite::ToSql],
    ) -> Result<Vec<crate::graph::Edge>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map(params, |r| {
            Ok(crate::graph::Edge {
                edge_id: r.get(0)?,
                src: r.get(1)?,
                relation: r.get(2)?,
                dst: r.get(3)?,
                valid_from: r.get(4)?,
                valid_to: r.get(5)?,
                ingested_at: r.get(6)?,
                invalidated_at: r.get(7)?,
                invalidated_by: r.get(8)?,
                origin: r.get(9)?,
                confidence_milli: r.get(10)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Map every node within `max_hops` of any `seed` (over CURRENT edges,
    /// treated as undirected for relatedness) to its shortest hop distance
    /// (1..=max_hops). Seeds themselves are excluded. Used by the recall
    /// graph-proximity boost. A seed with no current edges contributes nothing.
    fn current_neighbors_with_hops(
        &self,
        seeds: &[String],
        max_hops: u32,
    ) -> Result<HashMap<String, u32>, BossclawError> {
        let mut hops: HashMap<String, u32> = HashMap::new();
        let mut frontier: HashSet<String> = seeds.iter().cloned().collect();
        let mut visited: HashSet<String> = seeds.iter().cloned().collect();
        for hop in 1..=max_hops {
            if frontier.is_empty() {
                break;
            }
            let next = self.current_adjacent(&frontier)?;
            let mut new_frontier: HashSet<String> = HashSet::new();
            for id in next {
                if visited.insert(id.clone()) {
                    hops.insert(id.clone(), hop);
                    new_frontier.insert(id);
                }
            }
            frontier = new_frontier;
        }
        Ok(hops)
    }

    /// Distinct opposite endpoints of CURRENT edges incident to any id in
    /// `frontier` (undirected: returns both `dst` where `src ∈ frontier` and
    /// `src` where `dst ∈ frontier`). Empty `frontier` → empty set.
    ///
    /// **Trust gate (spec §7 / Rev 2 M4+F3):** only edges that pass
    /// `origin = 'manual' OR (origin = 'machine' AND confidence_milli >= ?)`
    /// contribute the proximity boost — manual edges always, machine edges only
    /// when their integer `confidence_milli` clears the threshold derived from
    /// [`crate::extract::TRUST_MIN`] (= 600). Low-confidence machine edges are
    /// still recorded + queryable (never-forget), but do NOT tilt recall. The
    /// threshold is an INTEGER **bound as a SQL parameter** (never `format!`-ed
    /// into the SQL) — both the F3 signing-integrity contract and SQLi hygiene.
    /// `confidence_milli` is NULL for manual edges, so the `origin = 'manual'` arm
    /// matches them regardless (NULL never satisfies `>= ?`, which is why the OR
    /// is structured this way).
    ///
    /// Both `IN` clauses share the same `?1..?n` placeholders (the id list bound
    /// ONCE — n params, not 2n, which would exceed the statement's parameter
    /// count); the trust threshold is bound ONCE at `?{n+1}` and referenced in
    /// both halves of the `UNION`.
    fn current_adjacent(
        &self,
        frontier: &HashSet<String>,
    ) -> Result<HashSet<String>, BossclawError> {
        if frontier.is_empty() {
            return Ok(HashSet::new());
        }
        let ids: Vec<&String> = frontier.iter().collect();
        let placeholders: String =
            (0..ids.len()).map(|i| format!("?{}", i + 1)).collect::<Vec<_>>().join(",");
        // Trust threshold bound as the parameter AFTER the id placeholders.
        let trust_param = format!("?{}", ids.len() + 1);
        let trust = format!(
            "(origin = 'manual' OR (origin = 'machine' AND confidence_milli >= {trust_param}))"
        );
        // dst where src ∈ frontier  UNION  src where dst ∈ frontier (current +
        // trust-gated only). Both IN clauses reference the SAME ?1..?n
        // placeholders; the trust threshold is the SAME ?{n+1} in both halves.
        let sql = format!(
            "SELECT dst AS other FROM edges \
               WHERE invalidated_at IS NULL AND {trust} AND src IN ({placeholders}) \
             UNION \
             SELECT src AS other FROM edges \
               WHERE invalidated_at IS NULL AND {trust} AND dst IN ({placeholders})"
        );
        // Integer trust threshold derived from the documented f32 TRUST_MIN via the
        // SAME single-sourced quantizer as the encode side (Rev 2 F3 / review I1):
        // TRUST_MIN stays an f32 used ONLY to derive this integer = 600, and encode
        // ⇄ threshold can never diverge.
        let trust_min_milli = crate::extract::to_confidence_milli(crate::extract::TRUST_MIN);
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(&sql)?;
        let mut params: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|id| *id as &dyn rusqlite::ToSql).collect();
        params.push(&trust_min_milli as &dyn rusqlite::ToSql);
        let rows = stmt.query_map(params.as_slice(), |r| r.get::<_, String>(0))?;
        let mut out = HashSet::new();
        for row in rows {
            out.insert(row?);
        }
        Ok(out)
    }

    /// Derive + store the resolution vector for an `entity` node under
    /// `(entity_id, model_id)` in a dedicated `entity_vectors` table. Separate
    /// from `vectors` (which feeds recall) so the two indexes never bleed. The
    /// `text` is the entity's label (+ optionally aliases) — what future mentions
    /// are matched against. Idempotent upsert.
    pub fn derive_entity_vector(
        &self,
        embedder: &dyn Embedder,
        entity_id: &str,
        text: &str,
    ) -> Result<(), BossclawError> {
        let embedding = embed_one(embedder, text)?;
        let blob = vec_to_blob(&embedding);
        let store = self.inner.lock().expect(POISON);
        store.conn().execute(
            "INSERT OR REPLACE INTO entity_vectors (entity_id, model_id, dim, embedding)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![entity_id, embedder.model_id(), embedder.dim() as i64, blob],
        )?;
        Ok(())
    }

    /// Rebuild the in-memory entity-resolution index from `entity_vectors` for
    /// the active model (zero plaintext index on disk; rebuilt on open — same
    /// mechanism as [`EventLog::rebuild_indexes`]). Serial insertion over
    /// `entity_id ASC` for reproducibility.
    pub fn rebuild_entity_index(&self, embedder: &dyn Embedder) -> Result<(), BossclawError> {
        let rows = self.entity_vectors_for_model(embedder.model_id())?;
        let mut index = HnswIndex::with_capacity(rows.len());
        for (entity_id, vec) in rows {
            index.add(&entity_id, &vec);
        }
        let boxed: Box<dyn VectorIndex> = Box::new(index);
        *self.entity_index.lock().expect(POISON) = Some(boxed);
        Ok(())
    }

    /// All entity vectors for `model_id` as `(entity_id, vector)` pairs, ordered
    /// `entity_id ASC` (deterministic rebuild order). Mirrors
    /// [`EventLog::vectors_for_model`] but over the `entity_vectors` table.
    fn entity_vectors_for_model(
        &self,
        model_id: &str,
    ) -> Result<Vec<(String, Vec<f32>)>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT entity_id, embedding FROM entity_vectors WHERE model_id = ?1 \
             ORDER BY entity_id ASC",
        )?;
        let rows = stmt.query_map([model_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, blob) = row?;
            out.push((id, blob_to_vec(&blob)?));
        }
        Ok(out)
    }

    /// Search the entity-resolution index for the `k` nearest `(entity_id,
    /// distance)` pairs to `mention`'s embedding. ONLY entity nodes are searched
    /// (the index holds only `entity_vectors`). Returns [`BossclawError::InvalidInput`]
    /// if the entity index was never built.
    pub fn entity_search(
        &self,
        embedder: &dyn Embedder,
        mention: &str,
        k: usize,
    ) -> Result<Vec<(String, f32)>, BossclawError> {
        let query = embed_one(embedder, mention)?;
        let guard = self.entity_index.lock().expect(POISON);
        match guard.as_ref() {
            Some(index) => Ok(index.search(&query, k)),
            None => Err(BossclawError::InvalidInput(
                "entity index not built — call rebuild_entity_index".into(),
            )),
        }
    }

    /// Resolve one entity `mention` against the existing entity nodes (spec §6):
    /// embed → search the entity index → convert distance to cosine similarity →
    /// [`crate::extract::resolve_decision`]; for the mid-band, ask `reasoner` to
    /// adjudicate and collapse its answer to a final [`crate::extract::ResolveDecision::Merge`]
    /// (a chosen candidate) or [`crate::extract::ResolveDecision::Mint`] (`"none"` / unknown id).
    ///
    /// The adjudication call is the ONLY model use here; merge/mint short-circuit
    /// without a model call (cheap + deterministic at the thresholds).
    pub fn resolve_mention(
        &self,
        embedder: &dyn Embedder,
        reasoner: &dyn crate::reason::Reasoner,
        mention: &str,
    ) -> Result<crate::extract::ResolveDecision, BossclawError> {
        use crate::extract::ResolveDecision;
        // DistCosine returns distance in [0, 2]; similarity = 1 - distance.
        let candidates: Vec<(String, f32)> = self
            .entity_search(embedder, mention, crate::extract::GRAPH_CONTEXT_K)?
            .into_iter()
            .map(|(id, dist)| (id, 1.0 - dist))
            .collect();
        match crate::extract::resolve_decision(&candidates) {
            ResolveDecision::Adjudicate(ids) => {
                let decided = self.adjudicate_entity(reasoner, mention, &ids)?;
                match decided {
                    Some(id) => Ok(ResolveDecision::Merge(id)),
                    None => Ok(ResolveDecision::Mint),
                }
            }
            other => Ok(other),
        }
    }

    /// Ask `reasoner` which of `candidate_ids` (if any) the `mention` refers to.
    /// Returns `Some(id)` for a chosen candidate that is actually in the list,
    /// `None` for `"none"` OR any id the model invented (defensive: a hallucinated
    /// id must not become a merge target). Uses the adjudication schema.
    fn adjudicate_entity(
        &self,
        reasoner: &dyn crate::reason::Reasoner,
        mention: &str,
        candidate_ids: &[String],
    ) -> Result<Option<String>, BossclawError> {
        let system = "You resolve entity coreference. Answer ONLY with the JSON the schema \
                      describes: the id of the candidate the mention refers to, or \"none\".";
        let prompt = crate::extract::build_adjudication_prompt(mention, candidate_ids);
        let answer = reasoner.complete_json(system, &prompt, &crate::reason::adjudication_schema())?;
        let chosen = answer.get("match").and_then(|m| m.as_str()).unwrap_or("none");
        if chosen == "none" {
            return Ok(None);
        }
        // Fail-closed: only accept an id the model was actually offered.
        Ok(candidate_ids.iter().find(|id| id.as_str() == chosen).cloned())
    }

    // ── Evolve loop (spec §8, Task 7) ────────────────────────────────────────

    /// Read the evolve cursor (the last processed `seq`); `0` if never set (the
    /// table is empty on a fresh store → no memory has been processed). The
    /// cursor is persistent progress state, NOT a fold (spec §4).
    pub fn evolve_cursor(&self) -> Result<i64, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let seq = conn
            .query_row("SELECT last_seq FROM evolve_cursor WHERE id = 0", [], |r| r.get(0))
            .optional()?
            .unwrap_or(0);
        Ok(seq)
    }

    /// Set the evolve cursor to `last_seq` (idempotent upsert of the single row).
    /// Persistent progress state — NOT rebuilt from events (spec §4). Losing it
    /// only re-processes events idempotently; it never corrupts the log.
    pub fn set_evolve_cursor(&self, last_seq: i64) -> Result<(), BossclawError> {
        let store = self.inner.lock().expect(POISON);
        store.conn().execute(
            "INSERT INTO evolve_cursor (id, last_seq) VALUES (0, ?1)
             ON CONFLICT(id) DO UPDATE SET last_seq = ?1",
            rusqlite::params![last_seq],
        )?;
        Ok(())
    }

    /// Set the evolve on/off switch by appending a control `config` event whose
    /// content is `{ "evolve_enabled": <enabled> }` (Rev 2 F2-sec(b)).
    ///
    /// This is the ONLY writer of the [`EVOLVE_ENABLED_KEY`] key — the off-switch
    /// is a PRIVILEGE, not arbitrary data, so it has a typed setter (the precedent
    /// is the active-model config written by [`EventLog::reembed_migration`]).
    /// Control config must not be written through a generic `append` in v1.
    /// The change is Ed25519-signed + hash-chained like every event, so a forged
    /// or replayed flip is tamper-evident via `verify_chain`. (M7 additionally
    /// verifies the signer DID == the resolved user owner before honoring it;
    /// `signed_by_did` is unverified today — spec §16 / M3 §12.1.)
    ///
    /// Carries NO model fields, so it never disturbs [`EventLog::active_model`]
    /// (which skips configs lacking `active_model_id`/`dim`/`schema_version`).
    pub fn set_evolve_enabled(&self, enabled: bool) -> Result<(), BossclawError> {
        self.append(Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: CONFIG_EVENT_TYPE.to_string(),
            // Explicit map so the key is the named const (json!{} cannot take a
            // const identifier as an object key).
            content: serde_json::Value::Object({
                let mut m = serde_json::Map::new();
                m.insert(EVOLVE_ENABLED_KEY.to_string(), serde_json::Value::Bool(enabled));
                m
            }),
            model_meta: None,
            prev_hash: String::new(),
            hash: None,
            signed_by_did: self.signer_did(),
            signature: None,
        })?;
        Ok(())
    }

    /// Whether the evolve loop is enabled (spec §8 off-switch / Rev 2 F2-sec(a)).
    ///
    /// STICKY / fail-closed semantics: config events are scanned newest-first and
    /// the FIRST one that carries an explicit `evolve_enabled` bool wins. Because
    /// [`EventLog::set_evolve_enabled`] is the only writer of the key, this is
    /// exactly "the latest EXPLICIT value": once an explicit `false` exists with
    /// no LATER explicit `true`, the loop stays disabled — a flag-LESS newer
    /// config (e.g. an active-model switch) does NOT silently re-arm the loop.
    /// Default-open (`true`) ONLY when the flag was never set at all.
    ///
    /// Honored BEFORE any model call in [`EventLog::evolve_once`].
    pub fn evolve_enabled(&self) -> Result<bool, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT payload FROM events WHERE event_type = ?1 ORDER BY seq DESC",
        )?;
        let rows = stmt.query_map([CONFIG_EVENT_TYPE], |r| r.get::<_, String>(0))?;
        for row in rows {
            let ev: Event = serde_json::from_str(&row?)?;
            if let Some(flag) = ev.content.get(EVOLVE_ENABLED_KEY).and_then(|v| v.as_bool()) {
                return Ok(flag); // newest explicit flag wins → sticky
            }
        }
        Ok(true) // flag never set → default open
    }

    /// The `(seq, id, text)` of each unprocessed `memory` event strictly after the
    /// cursor, in `seq ASC` order, capped at `limit` (the per-tick batch). Only
    /// `memory` events are processed (the evolve unit of work; `file_ingested`
    /// extraction is deferred — M4a scope). Returns owned data so the store lock
    /// is released before any model/embedder call (lock discipline).
    fn unprocessed_memories_since(
        &self,
        cursor: i64,
        limit: usize,
    ) -> Result<Vec<(i64, String, String)>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT seq, id, payload FROM events
             WHERE event_type = ?1 AND seq > ?2 ORDER BY seq ASC LIMIT ?3",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![MEMORY_EVENT_TYPE, cursor, limit as i64],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)),
        )?;
        let mut out = Vec::new();
        for row in rows {
            let (seq, id, payload) = row?;
            let ev: Event = serde_json::from_str(&payload)?;
            if let Some(text) = ev.content.get("text").and_then(|t| t.as_str()) {
                out.push((seq, id, text.to_string()));
            }
        }
        Ok(out)
    }

    /// The CURRENT active edge-keys `(src, relation, dst)` from the folded `edges`
    /// table (`invalidated_at IS NULL`). These endpoints are already RESOLVED
    /// `entity:<ulid>` ids (the fold stores whatever a `link` carried, and the
    /// evolve loop only ever emits links on resolved ids — Rev 2 F4), so a
    /// retraction must be remapped to resolved ids BEFORE it is confirmed against
    /// this set. Used both to confirm retractions and to seed within-tick dedup.
    fn active_edge_keys(&self) -> Result<Vec<(String, String, String)>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT src, relation, dst FROM edges WHERE invalidated_at IS NULL",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Fetch the `content["text"]` of each id in `ids` (memory/page events), in
    /// the caller's order (recall rank), skipping ids with no text. Turns recalled
    /// EVENT ids into the Pass-A cheat-sheet text. Parameterized `IN (...)` — no
    /// string interpolation of values.
    fn texts_for_ids(&self, ids: &[String]) -> Result<Vec<String>, BossclawError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders: String =
            (0..ids.len()).map(|i| format!("?{}", i + 1)).collect::<Vec<_>>().join(",");
        let sql = format!("SELECT id, payload FROM events WHERE id IN ({placeholders})");
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(params.as_slice(), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        // Preserve the caller's id order (recall rank), not SQL row order.
        let mut by_id: HashMap<String, String> = HashMap::new();
        for row in rows {
            let (id, payload) = row?;
            let ev: Event = serde_json::from_str(&payload)?;
            if let Some(t) = ev.content.get("text").and_then(|t| t.as_str()) {
                by_id.insert(id, t.to_string());
            }
        }
        Ok(ids.iter().filter_map(|id| by_id.get(id).cloned()).collect())
    }

    /// Map a proposed mention to its resolved `entity:<ulid>` if known, else pass
    /// the raw string through (a relation endpoint the model named but resolution
    /// did not cover — kept as an opaque node id, never silently dropped). Pure
    /// helper over the per-memory `mention_to_id` map (Rev 2 F4).
    fn map_mention(
        mention_to_id: &HashMap<String, String>,
        mention: &str,
    ) -> String {
        mention_to_id.get(mention).cloned().unwrap_or_else(|| mention.to_string())
    }

    /// Fold a [`EventLog::resolve_or_mint`] outcome `(entity_id, minted)` into the
    /// tick counters, returning the id. Single-sourced so the resolve loop counts a
    /// mint in exactly one place (no per-call-site duplication).
    fn count_mint(
        report: &mut EvolveReport,
        minted_this_tick: &mut bool,
        outcome: (String, bool),
    ) -> String {
        let (id, minted) = outcome;
        if minted {
            report.entities_minted += 1;
            *minted_this_tick = true;
        }
        id
    }

    /// Run ONE evolve tick (spec §3, §8 / Rev 2 F1/F4/F5/F6): for each unprocessed
    /// `memory` (≤ [`EVOLVE_BATCH`]): recall context → Pass A propose → resolve
    /// EVERY distinct mention (entities ∪ relations ∪ retractions) to a stable
    /// `entity:<ulid>` → augment with the resolved-entity neighborhood → Pass B
    /// (pure fail-closed span floor + ONE model critique that can only subtract,
    /// then cardinality-gated retraction confirmation against the CURRENT graph
    /// on RESOLVED ids) → emit `entity`/`invalidate`/`link` events through
    /// [`EventLog::append`] (the single serialized writer — the loop is NOT
    /// privileged) → advance the cursor after the batch commits.
    ///
    /// Idempotency: an active edge-key is skipped (seeded from the graph and
    /// updated WITHIN the tick, Rev 2 F5, so two memories asserting the same edge
    /// in one tick emit only once); a resolved/just-minted entity is reused.
    ///
    /// Resource fail-safes (Rev 2 F6): at most [`MAX_ENTITIES_PER_MEMORY`]
    /// entities are accepted per memory, the source text is truncated to
    /// [`crate::extract::MAX_INPUT_TEXT_BYTES`] before the model sees it, and the
    /// entity index is rebuilt ONCE after the batch (not per memory).
    ///
    /// Degrade-never-break (spec §10): the off-switch short-circuits to a no-op
    /// BEFORE any model call; a reasoner/graph error on a memory logs + STOPS the
    /// batch (the cursor does not advance past an unprocessed memory) so the
    /// memory retries next tick — recall + storage are untouched.
    pub fn evolve_once(
        &self,
        embedder: &dyn Embedder,
        reasoner: &dyn crate::reason::Reasoner,
    ) -> Result<EvolveReport, BossclawError> {
        let mut report = EvolveReport::default();
        // Off-switch is checked BEFORE any model call (Rev 2 F2-sec).
        if !self.evolve_enabled()? {
            report.skipped_disabled = true;
            return Ok(report);
        }
        let cursor = self.evolve_cursor()?;
        let batch = self.unprocessed_memories_since(cursor, EVOLVE_BATCH)?;
        // Within-tick active-key set (Rev 2 F5): seed from the current graph, then
        // grow as this tick emits — so a duplicate edge across two memories in the
        // SAME tick is skipped, not double-emitted.
        let mut active_keys: HashSet<(String, String, String)> =
            self.active_edge_keys()?.into_iter().collect();
        let mut last_committed_seq = cursor;
        // Whether any mint happened → rebuild the entity index ONCE after the
        // batch (Rev 2 F6), instead of O(memories) rebuilds inside the loop.
        let mut minted_this_tick = false;
        // Tick-scoped mention→id cache (mention surface form → resolved id). Since
        // the entity index is rebuilt only AFTER the batch (F6), a mention minted
        // by an EARLIER memory in this tick is not yet in the index; this cache
        // lets a LATER memory in the same tick reuse that mint instead of minting a
        // duplicate — which is also what lets within-tick edge dedup (F5) land
        // (two memories asserting the same edge resolve to the same key).
        let mut tick_mint_cache: HashMap<String, String> = HashMap::new();

        for (seq, mem_id, full_text) in batch {
            // F6: bound the text handed to the reasoner (the on-disk memory is
            // untouched; only the extraction copy is truncated).
            let text = crate::extract::truncate_for_reasoner(&full_text).to_string();

            // ── 1. recall context (M2). entity-kind is excluded from recall by
            //    construction (separate index), so neighbors are memories/pages.
            //    The read-set is EVENT ids only (never entity:<ulid>), spec §16. ──
            let recalled: Vec<String> = self
                .recall(embedder, &text, crate::extract::GRAPH_CONTEXT_K, &RecallOptions::default())
                .map(|hits| {
                    hits.into_iter()
                        .filter(|h| h.event_id != mem_id) // never feed the source back as context
                        .map(|h| h.event_id)
                        .collect()
                })
                .unwrap_or_default();
            let recalled_texts = self.texts_for_ids(&recalled)?;
            let read_set: Vec<String> = {
                let mut v = vec![mem_id.clone()];
                v.extend(recalled.iter().cloned());
                v
            };

            // ── 2. Pass A — propose. A reasoner error makes THIS memory a no-op
            //    (stop the batch; the cursor stays at last_committed_seq so the
            //    memory retries next tick) — spec §10. ──
            let proposals = match crate::extract::propose(reasoner, &text, &recalled_texts) {
                Ok(p) => p,
                Err(e) => {
                    log::warn!("evolve: Pass A failed for memory {mem_id}, stopping batch: {e}");
                    break;
                }
            };

            // ── 3. resolve EVERY distinct mention across entities ∪ relations ∪
            //    retractions to a stable entity:<ulid> (Rev 2 F4). An entity
            //    mention that resolves Mint becomes a signed `entity` event;
            //    relation/retraction endpoints the model named but did not list as
            //    entities are still resolved so they remap to graph-key ids. ──
            let mut mention_to_id: HashMap<String, String> = HashMap::new();
            // Resolve EVERY distinct mention to a stable entity id in ONE pass.
            // The work list is, in order: entity proposals (capped at
            // MAX_ENTITIES_PER_MEMORY, F6) with their declared type, then every
            // relation/retraction endpoint with the neutral UNRESOLVED_ENTITY_TYPE
            // (a bare endpoint the model named but did not list in entities[]).
            // First-seen wins, so an endpoint that is also a declared entity keeps
            // its real type. Folding both into one loop means the mint-count + the
            // resolve call appear exactly once (no duplication).
            let resolve_work = proposals
                .entities
                .iter()
                .take(MAX_ENTITIES_PER_MEMORY)
                .map(|e| (e.mention.clone(), e.entity_type.clone()))
                .chain(
                    proposals
                        .relations
                        .iter()
                        .flat_map(|r| [r.src.clone(), r.dst.clone()])
                        .chain(
                            proposals
                                .retractions
                                .iter()
                                .flat_map(|r| [r.src.clone(), r.dst.clone()]),
                        )
                        .map(|m| (m, UNRESOLVED_ENTITY_TYPE.to_string())),
                );
            for (mention, entity_type) in resolve_work {
                if mention_to_id.contains_key(&mention) {
                    continue; // first-seen wins (declared type beats the endpoint default)
                }
                let outcome = self.resolve_or_mint(
                    embedder,
                    reasoner,
                    &mention,
                    &entity_type,
                    &read_set,
                    &mut tick_mint_cache,
                )?;
                let id = Self::count_mint(&mut report, &mut minted_this_tick, outcome);
                mention_to_id.insert(mention, id);
            }

            // ── 4. augment: the neighborhood of the resolved entity ids (the
            //    second half of the cheat sheet) as `src -relation-> dst` lines. ──
            let neighborhood = self.neighborhood_lines(&mention_to_id)?;

            // ── 5. Pass B — model-driven critique over a pure fail-closed floor
            //    (Rev 2 F1): the floor keeps only span-verified relations; the
            //    model may DROP or down-confidence but NEVER add an edge the floor
            //    didn't support. Bounded by MAX_REFLECT total passes (Pass A +
            //    this critique = 2). A reasoner error → no-op this memory. ──
            // The MAX_REFLECT bound (Pass A propose + one Pass B critique = 2) is
            // enforced at COMPILE time: this tick runs exactly those two model
            // passes, so a future tightening of MAX_REFLECT below 2 must fail the
            // build rather than silently under-run the reflexion contract.
            const _: () = assert!(MAX_REFLECT >= 2, "evolve runs Pass A + one critique");
            let refined = match crate::extract::critique_with_reasoner(
                reasoner, &text, &proposals, &neighborhood,
            ) {
                Ok(r) => r,
                Err(e) => {
                    log::warn!("evolve: Pass B failed for memory {mem_id}, stopping batch: {e}");
                    break;
                }
            };

            // ── 6. remap refined relations'/retractions' endpoints to resolved
            //    ids (Rev 2 F4) BEFORE confirming retractions / emitting links. ──
            let remapped_retractions: Vec<crate::extract::ProposedRetraction> = refined
                .retractions
                .iter()
                .map(|r| crate::extract::ProposedRetraction {
                    src: Self::map_mention(&mention_to_id, &r.src),
                    relation: r.relation.clone(),
                    dst: Self::map_mention(&mention_to_id, &r.dst),
                    reason: r.reason.clone(),
                    confidence: r.confidence,
                })
                .collect();
            // Confirm against the CURRENT active edges (resolved ids). active_keys
            // already holds resolved-id keys; only materialize the slice when there
            // is actually a retraction to confirm (retractions are rare — avoid the
            // per-memory clone of the whole active set otherwise).
            let confirmed = if remapped_retractions.is_empty() {
                Vec::new()
            } else {
                let active_now: Vec<(String, String, String)> =
                    active_keys.iter().cloned().collect();
                crate::extract::confirm_retractions(&remapped_retractions, &active_now)
            };

            // ── 6a. invalidate confirmed contradictions FIRST (so the fold closes
            //    the old interval before any replacement opens). Drop the retired
            //    key from the within-tick active set. ──
            for r in &confirmed {
                self.invalidate(&r.src, &r.relation, &r.dst, None, &read_set)?;
                active_keys.remove(&(r.src.clone(), r.relation.clone(), r.dst.clone()));
                report.invalidates_emitted += 1;
            }

            // ── 6b. emit confirmed relations as machine links on RESOLVED ids,
            //    skipping any (src, relation, dst) ALREADY active — including ones
            //    emitted earlier in THIS tick (Rev 2 F5). ──
            for rel in &refined.relations {
                let s = Self::map_mention(&mention_to_id, &rel.src);
                let d = Self::map_mention(&mention_to_id, &rel.dst);
                let key = (s.clone(), rel.relation.clone(), d.clone());
                if active_keys.contains(&key) {
                    continue; // already asserted → emit nothing (idempotent)
                }
                self.link_machine(
                    &s, &rel.relation, &d, rel.confidence, reasoner.model_id(), &read_set,
                )?;
                active_keys.insert(key);
                report.links_emitted += 1;
            }

            report.memories_processed += 1;
            last_committed_seq = seq;
        }

        // ── 7. rebuild the entity index ONCE after the batch (Rev 2 F6) so the
        //    next tick can resolve this tick's mints, and refresh the graph so
        //    the folded `edges`/`entities` reflect the just-emitted events. ──
        if minted_this_tick {
            self.rebuild_entity_index(embedder)?;
        }
        if report.links_emitted > 0
            || report.invalidates_emitted > 0
            || report.entities_minted > 0
        {
            self.rebuild_graph()?;
        }

        // ── 8. advance the cursor to the last fully-processed memory's seq, only
        //    after the batch committed (a stopped batch leaves it where it was). ──
        if last_committed_seq > cursor {
            self.set_evolve_cursor(last_committed_seq)?;
        }
        Ok(report)
    }

    /// Resolve one `mention` to an entity id, minting a signed `entity` event +
    /// its resolution vector when resolution says Mint. Returns `(entity_id,
    /// minted)` where `minted` is `true` iff a fresh entity was created.
    ///
    /// `entity_type` labels a freshly minted entity; `read_set` is the provenance
    /// (EVENT ids only) stamped as the mint's `source_event_ids`. The
    /// `Adjudicate` arm is already collapsed to Merge/Mint inside
    /// [`EventLog::resolve_mention`]; the match below is exhaustive defensively.
    ///
    /// `tick_cache` carries mints WITHIN the current tick: because the entity
    /// index is rebuilt only after the batch (Rev 2 F6), a mention this tick
    /// already minted is not yet searchable, so the cache is consulted FIRST to
    /// reuse that id (returning `minted = false` — the mint was already counted).
    /// This keeps one surface mention = one entity per tick and is what lets the
    /// within-tick edge dedup (F5) compare equal keys.
    fn resolve_or_mint(
        &self,
        embedder: &dyn Embedder,
        reasoner: &dyn crate::reason::Reasoner,
        mention: &str,
        entity_type: &str,
        read_set: &[String],
        tick_cache: &mut HashMap<String, String>,
    ) -> Result<(String, bool), BossclawError> {
        if let Some(id) = tick_cache.get(mention) {
            return Ok((id.clone(), false)); // already minted/resolved this tick
        }
        let resolved = match self.resolve_mention(embedder, reasoner, mention)? {
            ResolveDecision::Merge(id) => (id, false),
            // resolve_mention collapses Adjudicate→Merge/Mint; Mint (and the
            // unreachable Adjudicate) mint a fresh signed entity.
            ResolveDecision::Mint | ResolveDecision::Adjudicate(_) => {
                let new_id = self.entity(mention, &[], entity_type, reasoner.model_id(), read_set)?;
                self.derive_entity_vector(embedder, &new_id, mention)?;
                (new_id, true)
            }
        };
        tick_cache.insert(mention.to_string(), resolved.0.clone());
        Ok(resolved)
    }

    /// The current 1-hop neighborhood of the resolved entity ids as human-readable
    /// `src -relation-> dst` lines (spec §6 cheat-sheet, second half), de-duped and
    /// deterministically ordered. Fed to Pass B so the model can confirm
    /// contradictions against KNOWN edges. Best-effort per id: a graph read error
    /// on one id is skipped (degrade, never break — spec §10).
    fn neighborhood_lines(
        &self,
        mention_to_id: &HashMap<String, String>,
    ) -> Result<Vec<String>, BossclawError> {
        let mut seen: BTreeMap<String, ()> = BTreeMap::new();
        for id in mention_to_id.values() {
            let edges = match self.neighbors(id) {
                Ok(e) => e,
                Err(e) => {
                    log::warn!("evolve: neighborhood lookup failed for {id}: {e}");
                    continue;
                }
            };
            for edge in edges {
                seen.insert(format!("{} -{}-> {}", edge.src, edge.relation, edge.dst), ());
            }
        }
        Ok(seen.into_keys().collect())
    }

    /// A snapshot of evolve-loop health (spec §8). `queue_depth` = unprocessed
    /// `memory` events behind the cursor (LIVE); `enabled` reflects the sticky
    /// off-switch (LIVE). `last_tick_ms`/`error_count`/`last_error` are honest
    /// M4a stubs (`None`/`0`/`None`) — the running tick/error counters are owned
    /// by M7's long-lived loop driver, not persisted here, so this method stays a
    /// pure read and is unit-testable.
    pub fn evolve_status(&self) -> Result<EvolveStatus, BossclawError> {
        let cursor = self.evolve_cursor()?;
        let queue_depth = {
            let store = self.inner.lock().expect(POISON);
            let conn = store.conn();
            conn.query_row(
                "SELECT count(*) FROM events WHERE event_type = ?1 AND seq > ?2",
                rusqlite::params![MEMORY_EVENT_TYPE, cursor],
                |r| r.get::<_, i64>(0),
            )? as usize
        };
        Ok(EvolveStatus {
            queue_depth,
            last_tick_ms: None,
            error_count: 0,
            last_error: None,
            enabled: self.evolve_enabled()?,
        })
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

    /// Minimal `memory` event for the graph-BFS unit test (mirrors the helper in
    /// `tests/graph.rs`; kept local so this unit module stays self-contained).
    fn mk_memory(text: &str) -> Event {
        Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: "memory".to_string(),
            content: serde_json::json!({ "text": text }),
            model_meta: None,
            prev_hash: String::new(),
            hash: None,
            signed_by_did: "did:wba:AIR-TEST".to_string(),
            signature: None,
        }
    }

    /// The multi-hop BFS expands to `max_hops` and records the SHORTEST hop
    /// distance per node. Exercises the hop≥2 branch that the shipped
    /// `GRAPH_MAX_HOPS = 1` never reaches, so the `GRAPH_HOP_DECAY^(hop-1)` decay
    /// term and the frontier expansion are proven rather than merely asserted.
    /// Chain a→b→c: from seed `a`, `b` is a direct neighbor (hop 1) and `c` is
    /// reachable only through `b` (hop 2); the seed itself is excluded.
    #[test]
    fn current_neighbors_with_hops_expands_to_max_hops_shortest_distance() {
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        let a = log.append(mk_memory("a")).unwrap();
        let b = log.append(mk_memory("b")).unwrap();
        let c = log.append(mk_memory("c")).unwrap();
        log.link(&a, "x", &b, None, &[]).unwrap();
        log.link(&b, "x", &c, None, &[]).unwrap();
        log.rebuild_graph().unwrap();

        let hops = log.current_neighbors_with_hops(std::slice::from_ref(&a), 2).unwrap();
        let expected: HashMap<String, u32> = [(b, 1), (c, 2)].into_iter().collect();
        assert_eq!(
            hops, expected,
            "BFS must reach b at hop 1 and c at hop 2, excluding the seed a"
        );
    }
}
