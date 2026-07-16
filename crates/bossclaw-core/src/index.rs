//! Pure, in-memory approximate-nearest-neighbour (ANN) index over the ACTIVE
//! model's vectors.
//!
//! The index is **never persisted** in v1: keeping a plaintext ANN graph on disk
//! would leak the embedding geometry (and thus the content) that the encrypted
//! log is built to protect. Instead it is rebuilt from the encrypted log on open
//! — see [`crate::log::EventLog::rebuild_indexes`]. This is the proven
//! "zero plaintext index on disk" mechanism.
//!
//! The [`VectorIndex`] trait is deliberately PURE: it knows nothing about
//! `Store`, `EventLog`, or SQL. All persistence lives on `EventLog`; this module
//! only does vector math. That separation is what lets the index be thrown away
//! and rebuilt at will.

use std::collections::{HashMap, HashSet};

use hnsw_rs::prelude::*;

/// Maximum neighbour connections stored per layer in the HNSW graph (the `M`
/// parameter). 16 is the library's typical default and a good recall/memory
/// trade-off for the small-to-medium corpora BossClaw holds. Must be ≤ 256
/// (hnsw_rs hard limit).
const MAX_NB_CONNECTION: usize = 16;

/// Maximum number of hierarchy layers in the HNSW graph. hnsw_rs clamps this to
/// its internal `NB_LAYER_MAX`, so 16 simply requests "as many as allowed".
const MAX_LAYER: usize = 16;

/// Breadth of the candidate list explored while *building* the graph (`efConstruction`).
/// Higher means better-connected graphs (better recall) at higher build cost;
/// 200 is the common default and rebuild happens off the hot path.
const EF_CONSTRUCTION: usize = 200;

/// Lower bound on the search beam width (`ef`). The effective `ef` is
/// `k.max(EF_SEARCH_MIN)` so that small-`k` queries still explore a wide enough
/// candidate set to surface the true nearest neighbours, while large-`k` queries
/// scale their beam with `k`. `ef` must be ≥ `k` for hnsw_rs.
const EF_SEARCH_MIN: usize = 64;

/// Unit-separator that joins an event id and a chunk index into one `VectorIndex`
/// key. `0x1f` (US) cannot appear in a Crockford-base32 ULID, so `event_id_of`
/// below can always recover the bare id — the property the fold-back relies on.
pub const CHUNK_KEY_SEP: char = '\u{1f}';

/// Encode `(event_id, chunk_ix)` as the composite `VectorIndex` key.
pub fn encode_chunk_key(event_id: &str, chunk_ix: usize) -> String {
    format!("{event_id}{CHUNK_KEY_SEP}{chunk_ix}")
}

/// Decode a composite key back to `(event_id, chunk_ix)`, or `None` if it is not
/// a well-formed chunk key (no separator, or a non-numeric index). Uses the FIRST
/// separator (`split_once`) so it agrees byte-for-byte with `event_id_of` on the
/// event-id field — a stray extra separator makes the index non-numeric ⇒ None
/// (never a silently-wrong parse). Event ULIDs never contain 0x1f, so real keys
/// always have exactly one separator.
pub fn decode_chunk_key(key: &str) -> Option<(&str, usize)> {
    let (id, ix) = key.split_once(CHUNK_KEY_SEP)?;
    Some((id, ix.parse().ok()?))
}

/// The bare event id for a composite (or already-bare) key — the fold-back reduce
/// key. A key without the separator is returned unchanged (defensive). Uses the
/// FIRST separator, matching `decode_chunk_key`.
pub fn event_id_of(key: &str) -> &str {
    key.split_once(CHUNK_KEY_SEP).map_or(key, |(id, _)| id)
}

/// Sentinel prefixing a NOTE key in the unified conflict index. `0x1e` (RS) — like
/// `CHUNK_KEY_SEP` (`0x1f`) — cannot appear in a Crockford-base32 ULID or an A5-validated
/// session id (`[A-Za-z0-9_-]`), so a note key and a `(session_id, passage_ix)` chunk key can
/// never collide. Distinct from `CHUNK_KEY_SEP` so `decode_chunk_key` returns `None` on a note key.
pub const NOTE_KEY_SENTINEL: char = '\u{1e}';

/// Encode a note body's conflict-index key: the sentinel followed by the note event id.
pub fn encode_note_key(event_id: &str) -> String {
    format!("{NOTE_KEY_SENTINEL}{event_id}")
}

/// Decode a note key back to its event id, or `None` if `key` is not a note key.
pub fn decode_note_key(key: &str) -> Option<&str> {
    key.strip_prefix(NOTE_KEY_SENTINEL)
}

/// A typed reference to a conflict-index member: a current memory note, or a live session
/// passage. The stable-identity scheme Phase 1 already uses (event id for notes; the
/// fold-resolved `session_id` + passage ordinal for passages). Hashable/comparable so it can
/// key exclusion + open-pair sets.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ConflictRef {
    /// A current memory note, identified by its `memory` event id.
    Note {
        /// The `memory` event id of the note body.
        event_id: String,
    },
    /// A live captured-session passage, identified by the session's stable id + passage ordinal.
    Passage {
        /// The fold-resolved stable id of the session the passage belongs to.
        session_id: String,
        /// The passage's ordinal within that session.
        passage_id: usize,
    },
}

impl ConflictRef {
    /// Decode a conflict-index key to its typed ref. Note keys (sentinel-prefixed) are checked
    /// first; everything else falls through to the passage chunk codec. `None` for a malformed key.
    pub fn decode_key(key: &str) -> Option<ConflictRef> {
        if let Some(id) = decode_note_key(key) {
            return Some(ConflictRef::Note { event_id: id.to_string() });
        }
        decode_chunk_key(key).map(|(sid, pid)| ConflictRef::Passage {
            session_id: sid.to_string(),
            passage_id: pid,
        })
    }

    /// A stable, kind-tagged string identity for this ref. Used to build unordered pair keys
    /// (sort two `pair_key`s) and exclusion sets. The `0x1f` field separator can appear in
    /// neither a ULID nor an A5 session id, so distinct refs never collide by concatenation.
    pub fn pair_key(&self) -> String {
        match self {
            ConflictRef::Note { event_id } => format!("N\u{1f}{event_id}"),
            ConflictRef::Passage { session_id, passage_id } => {
                format!("P\u{1f}{session_id}\u{1f}{passage_id}")
            }
        }
    }

    /// Unordered pair identity: the two `pair_key`s, sorted, joined by `\u{1e}` (record separator).
    /// Both orders of (a, b) map to the SAME key — the idempotency identity (spec §3.5). Portable:
    /// the single source of truth for the pair key used by the finder AND log.rs `conflict_pair_key`.
    pub fn unordered_pair_key(a: &ConflictRef, b: &ConflictRef) -> String {
        let (ka, kb) = (a.pair_key(), b.pair_key());
        if ka <= kb { format!("{ka}\u{1e}{kb}") } else { format!("{kb}\u{1e}{ka}") }
    }

    /// The persisted (signed-proposal) JSON shape for this ref.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            ConflictRef::Note { event_id } => {
                serde_json::json!({ "kind": "note", "event_id": event_id })
            }
            ConflictRef::Passage { session_id, passage_id } => serde_json::json!({
                "kind": "passage", "session_id": session_id, "passage_id": passage_id
            }),
        }
    }

    /// Parse a ref back from its persisted JSON, or `None` if malformed.
    pub fn from_json(v: &serde_json::Value) -> Option<ConflictRef> {
        match v.get("kind").and_then(|k| k.as_str())? {
            "note" => Some(ConflictRef::Note {
                event_id: v.get("event_id")?.as_str()?.to_string(),
            }),
            "passage" => Some(ConflictRef::Passage {
                session_id: v.get("session_id")?.as_str()?.to_string(),
                passage_id: usize::try_from(v.get("passage_id")?.as_u64()?).ok()?,
            }),
            _ => None,
        }
    }
}

/// An in-memory ANN index over the vectors of a single (active) embedding model.
///
/// NOT persisted in v1 — rebuilt from the encrypted log on open (zero plaintext
/// index on disk). Implementations operate purely on `(event_id, vector)` pairs
/// and never touch storage.
// `len` here is a row-count accessor for the recall-untouched assertion, not a
// collection-emptiness API; an `is_empty` companion would be dead surface.
#[allow(clippy::len_without_is_empty)]
pub trait VectorIndex: Send + Sync {
    /// Add a vector under `event_id`. Re-adding an existing `event_id` is a
    /// documented no-op (the first vector wins); the authoritative vector for an
    /// id never changes within one model, and a fresh rebuild is the way to pick
    /// up corrections.
    fn add(&mut self, event_id: &str, vec: &[f32]);

    /// Return up to `k` nearest `(event_id, distance)` pairs, ascending by
    /// distance. Tombstoned ids (see [`VectorIndex::remove`]) are excluded.
    ///
    /// The implementation over-fetches `k + tombstones.len()` candidates from
    /// the underlying HNSW graph so that live hits can refill positions vacated
    /// by tombstone filtering. If more than `tombstones.len()` tombstoned ids
    /// happen to rank among the nearest HNSW candidates — which occurs when
    /// tombstoned vectors cluster around the query — the result MAY contain
    /// fewer than `k` pairs. Callers must tolerate a short result. Full
    /// compaction (physically dropping tombstoned nodes) happens on the next
    /// `rebuild_indexes` call.
    fn search(&self, vec: &[f32], k: usize) -> Vec<(String, f32)>;

    /// Tombstone `event_id` so it is filtered out of future searches.
    ///
    /// hnsw_rs has no cheap delete, so removal is a logical tombstone here; the
    /// node is physically dropped on the next rebuild-from-log (which simply does
    /// not re-add it).
    fn remove(&mut self, event_id: &str);

    /// The `event_id` of the most recently [`add`](VectorIndex::add)ed vector, or
    /// `None` if nothing has been added. A duplicate `add` (same `event_id`, a
    /// documented no-op) does NOT advance `last_indexed`.
    fn last_indexed(&self) -> Option<String>;

    /// The number of distinct vectors currently held (each [`add`](VectorIndex::add)
    /// of a new `event_id` grows it by one; a duplicate `add` does not). Tombstoned
    /// ids still count until the next rebuild physically drops them — `len` is the
    /// element count, not the live-hit count.
    fn len(&self) -> usize;
}

/// HNSW-backed [`VectorIndex`].
///
/// Holds an owned [`Hnsw`] plus the bookkeeping needed to translate between the
/// caller's string `event_id`s and the `usize` slots hnsw_rs requires:
/// - `id_to_slot`: `event_id` → slot. Also the de-dup guard (an id already
///   present is not inserted twice).
/// - `slot_to_id`: slot → `event_id`, to translate `Neighbour.d_id` back.
/// - `tombstones`: ids logically removed; filtered out of every search.
/// - `last_indexed`: the most recently added id.
/// - `next_slot`: monotonically increasing slot allocator.
///
/// Vectors are expected to be L2-normalised (the embedders normalise), so the
/// [`DistCosine`] metric is meaningful.
///
/// # Cross-session rank determinism
///
/// hnsw_rs 0.3.4 seeds its level-assignment RNG from OS randomness at each
/// [`Hnsw::new`] construction — there is no public API to supply a seed.
/// Top-1 recall is stable across rebuilds (the nearest neighbour by cosine
/// distance is always found), but the relative ranking of deeper neighbours
/// varies between independent `HnswIndex` instances built from the same data.
/// Cross-session rank determinism would require persisting the HNSW graph; that
/// is deferred to R2 (the encrypted sidecar milestone) once the privacy model
/// for a persisted ANN graph is established.
pub struct HnswIndex {
    hnsw: Hnsw<'static, f32, DistCosine>,
    id_to_slot: HashMap<String, usize>,
    slot_to_id: HashMap<usize, String>,
    tombstones: HashSet<String>,
    last_indexed: Option<String>,
    next_slot: usize,
}

impl HnswIndex {
    /// Create an empty index sized for roughly `max_elements` vectors.
    ///
    /// `max_elements` is only an allocation HINT to hnsw_rs (it pre-sizes its
    /// per-layer tables); inserting beyond it is safe and simply reallocates.
    /// We clamp to `max(1)` because a 0-capacity HNSW is degenerate. Because the
    /// production model is rebuild-on-open, the rebuild always passes the exact
    /// row count, so the hint is normally precise.
    pub fn with_capacity(max_elements: usize) -> Self {
        let capacity = max_elements.max(1);
        let hnsw = Hnsw::<f32, DistCosine>::new(
            MAX_NB_CONNECTION,
            capacity,
            MAX_LAYER,
            EF_CONSTRUCTION,
            DistCosine {},
        );
        Self {
            hnsw,
            id_to_slot: HashMap::new(),
            slot_to_id: HashMap::new(),
            tombstones: HashSet::new(),
            last_indexed: None,
            next_slot: 0,
        }
    }
}

impl VectorIndex for HnswIndex {
    fn add(&mut self, event_id: &str, vec: &[f32]) {
        // De-dup: an id already in the index keeps its original vector. This also
        // makes a stray double-add during rebuild harmless.
        if self.id_to_slot.contains_key(event_id) {
            return;
        }
        let slot = self.next_slot;
        self.next_slot += 1;
        // Serial insert is required (not parallel_insert): slot assignment must
        // match insertion order so that `slot_to_id` is correct, and serial
        // order is a necessary (though not sufficient — see struct doc) condition
        // for stable top-1 recall across rebuilds.
        self.hnsw.insert((vec, slot));
        self.id_to_slot.insert(event_id.to_string(), slot);
        self.slot_to_id.insert(slot, event_id.to_string());
        self.last_indexed = Some(event_id.to_string());
    }

    fn search(&self, vec: &[f32], k: usize) -> Vec<(String, f32)> {
        if k == 0 {
            return Vec::new();
        }
        // Over-fetch so post-hoc tombstone filtering can still return up to `k`
        // live hits: ask for `k + tombstone_count`, clamped to what the index
        // actually holds. `ef` must be ≥ the number requested.
        let requested = k
            .saturating_add(self.tombstones.len())
            .min(self.id_to_slot.len())
            .max(1);
        let ef = requested.max(EF_SEARCH_MIN);
        let neighbours = self.hnsw.search(vec, requested, ef);

        let mut out = Vec::with_capacity(k);
        for n in neighbours {
            // Translate the internal slot back to the caller's event_id. A slot
            // with no mapping should be impossible, but skip defensively rather
            // than panic in library code.
            let id = match self.slot_to_id.get(&n.d_id) {
                Some(id) => id,
                None => continue,
            };
            if self.tombstones.contains(id) {
                continue;
            }
            out.push((id.clone(), n.distance));
            if out.len() == k {
                break;
            }
        }
        out
    }

    fn remove(&mut self, event_id: &str) {
        // Logical tombstone only; the node is physically dropped on next rebuild.
        if self.id_to_slot.contains_key(event_id) {
            self.tombstones.insert(event_id.to_string());
        }
    }

    fn last_indexed(&self) -> Option<String> {
        self.last_indexed.clone()
    }

    fn len(&self) -> usize {
        // `id_to_slot` is the de-dup guard `add` grows once per distinct id, so its
        // size is the authoritative physical element count (`remove` only tombstones
        // — it never shrinks this map; a rebuild is what drops dropped ids).
        self.id_to_slot.len()
    }
}

/// Zero-dep compile-time guard: if a future field breaks `Send + Sync` on
/// `HnswIndex`, this fails at the definition site rather than at a distant
/// call site that boxes it as `Box<dyn VectorIndex>`.
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<HnswIndex>();
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_key_round_trips_and_separator_is_ulid_safe() {
        let key = encode_chunk_key("01J8Z3ABCDXYZ", 7);
        assert_eq!(key, "01J8Z3ABCDXYZ\u{1f}7");
        let (id, ix) = decode_chunk_key(&key).unwrap();
        assert_eq!(id, "01J8Z3ABCDXYZ");
        assert_eq!(ix, 7);
        // 0x1f never appears in a Crockford-base32 ULID, so decoding is unambiguous.
        assert_eq!(decode_chunk_key("no-separator-here"), None);
        // Malformed index is rejected (not silently 0).
        assert_eq!(decode_chunk_key("id\u{1f}notanumber"), None);
    }

    #[test]
    fn event_id_of_and_decode_agree_on_split_direction() {
        // Both use the FIRST separator (split_once) — so a key with a stray extra
        // separator decodes/reduces identically. (event ULIDs never contain 0x1f,
        // so this only guards defensive/mixed-data robustness, not a real ULID.)
        let weird = "01J8Z3ABCDXYZ\u{1f}2\u{1f}5"; // two separators
        assert_eq!(event_id_of(weird), "01J8Z3ABCDXYZ", "reduce key = first field");
        // decode_chunk_key rejects it: the index field "2\u{1f}5" isn't a number.
        assert_eq!(decode_chunk_key(weird), None, "ambiguous multi-sep key is not a valid chunk key");
    }

    #[test]
    fn event_id_of_extracts_the_bare_id_for_foldback() {
        assert_eq!(event_id_of("01J8Z3ABCDXYZ\u{1f}3"), "01J8Z3ABCDXYZ");
        // A bare (non-chunk) key is returned unchanged — defensive for mixed data.
        assert_eq!(event_id_of("01J8Z3ABCDXYZ"), "01J8Z3ABCDXYZ");
    }

    #[test]
    fn len_counts_distinct_added_vectors() {
        // A fresh index holds nothing; each distinct `add` grows the count by one;
        // a duplicate `add` (documented no-op) does NOT — `len` == the number of
        // physical vectors the index would search, the invariant Task 6's
        // "recall index untouched" assertion depends on.
        let mut ix = HnswIndex::with_capacity(4);
        assert_eq!(ix.len(), 0, "a fresh index is empty");
        ix.add("a", &[1.0, 0.0, 0.0]);
        ix.add("b", &[0.0, 1.0, 0.0]);
        assert_eq!(ix.len(), 2, "two distinct vectors ⇒ len 2");
        ix.add("a", &[0.0, 0.0, 1.0]); // duplicate id: dedup no-op
        assert_eq!(ix.len(), 2, "re-adding an existing id does not grow len");
    }

    #[test]
    fn note_key_and_passage_key_are_disjoint_and_typed() {
        // A note key round-trips and is NOT mistaken for a passage key.
        let nk = encode_note_key("01J8Z3ABCDXYZ");
        assert_eq!(decode_note_key(&nk), Some("01J8Z3ABCDXYZ"));
        assert_eq!(decode_chunk_key(&nk), None, "a note key is never a valid chunk key");
        assert_eq!(
            ConflictRef::decode_key(&nk),
            Some(ConflictRef::Note { event_id: "01J8Z3ABCDXYZ".into() })
        );

        // A passage key still decodes as a Passage (existing chunk codec is untouched).
        let pk = encode_chunk_key("s1", 3);
        assert_eq!(decode_note_key(&pk), None, "a passage key has no note sentinel");
        assert_eq!(
            ConflictRef::decode_key(&pk),
            Some(ConflictRef::Passage { session_id: "s1".into(), passage_id: 3 })
        );

        // The sentinel is distinct from the chunk separator, and neither appears in a ULID.
        assert_ne!(NOTE_KEY_SENTINEL, CHUNK_KEY_SEP);

        // pair_key is a stable, kind-tagged identity used to build unordered pair keys.
        assert_eq!(
            ConflictRef::Note { event_id: "a".into() }.pair_key(),
            ConflictRef::Note { event_id: "a".into() }.pair_key()
        );
        assert_ne!(
            ConflictRef::Note { event_id: "a".into() }.pair_key(),
            ConflictRef::Passage { session_id: "a".into(), passage_id: 0 }.pair_key()
        );

        // JSON round-trips both variants (the persisted proposal shape).
        for r in [
            ConflictRef::Note { event_id: "n1".into() },
            ConflictRef::Passage { session_id: "s1".into(), passage_id: 2 },
        ] {
            assert_eq!(ConflictRef::from_json(&r.to_json()), Some(r));
        }

        // from_json is the deserialization boundary for the persisted, signed proposal
        // shape — it MUST fail closed (None) on every malformed input, never a wrong ref.
        use serde_json::json;
        assert_eq!(ConflictRef::from_json(&json!({"kind": "bogus"})), None);
        assert_eq!(ConflictRef::from_json(&json!({"kind": "note"})), None); // missing event_id
        assert_eq!(
            ConflictRef::from_json(&json!({"kind": "passage", "session_id": "s"})),
            None // missing passage_id
        );
        assert_eq!(
            ConflictRef::from_json(&json!({"kind": "passage", "session_id": "s", "passage_id": -1})),
            None // negative → not a u64
        );
    }
}
