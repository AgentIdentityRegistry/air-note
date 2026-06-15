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

/// An in-memory ANN index over the vectors of a single (active) embedding model.
///
/// NOT persisted in v1 — rebuilt from the encrypted log on open (zero plaintext
/// index on disk). Implementations operate purely on `(event_id, vector)` pairs
/// and never touch storage.
pub trait VectorIndex: Send + Sync {
    /// Add a vector under `event_id`. Re-adding an existing `event_id` is a
    /// documented no-op (the first vector wins); the authoritative vector for an
    /// id never changes within one model, and a fresh rebuild is the way to pick
    /// up corrections.
    fn add(&mut self, event_id: &str, vec: &[f32]);

    /// Return up to `k` nearest `(event_id, distance)` pairs, ascending by
    /// distance. Tombstoned ids (see [`VectorIndex::remove`]) are excluded.
    fn search(&self, vec: &[f32], k: usize) -> Vec<(String, f32)>;

    /// Tombstone `event_id` so it is filtered out of future searches.
    ///
    /// hnsw_rs has no cheap delete, so removal is a logical tombstone here; the
    /// node is physically dropped on the next rebuild-from-log (which simply does
    /// not re-add it).
    fn remove(&mut self, event_id: &str);

    /// The `event_id` of the most recently [`add`](VectorIndex::add)ed vector, or
    /// `None` if nothing has been added.
    fn last_indexed(&self) -> Option<String>;
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
}
