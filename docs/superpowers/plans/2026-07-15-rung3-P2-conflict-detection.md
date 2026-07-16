# Rung 3 — Phase 2: Semantic Conflict **Detection** — Implementation Plan

> For agentic workers: REQUIRED SUB-SKILL: superpowers:subagent-driven-development

**Goal.** Teach the daemon to **notice** when two of its own memories contradict each other and record a
signed `conflict_proposal` (both sides, typed refs, coarse confidence, sanitized reason) for a later owner
decision. Detection never picks a winner, never retires, never surfaces UI, and never runs unless the owner
switched it on. A wrong judge call costs a dropped counter, never a memory (invariant I1). This plan implements
the approved design `docs/superpowers/specs/2026-07-15-rung3-phase2-detection-design.md` end to end: the unified
fights index (note bodies + session passages in ONE conflict index), a restart-safe conflict cursor, an
off-by-default owner gate, the `conflict_proposal` event family (append + idempotency + GC + projection), a pure
candidate-finder, and a background sweep that piggybacks the capture sweeper's cadence.

**Architecture.** Three layers, mirroring the shipped Rung-3 Phase-1 split.
- **`bossclaw-core` (the brain).** Owns all vector + fold + event work: the unified conflict index, typed
  `ConflictRef` decode, the conflict cursor, the `ConflictDetect` config flag, the `conflict_proposal` append /
  idempotency / GC / projection, the pure `decide_conflict_sweep` candidate-finder, and the one orchestrating
  method `EventLog::detect_conflicts_once`. Core is **filesystem-free**: it has passage *vectors* (in
  `session_passage_vectors`) but NOT passage *text*, so the judge step takes a caller-supplied
  `passage_text` closure.
- **`bossclawd` (the daemon).** Owns the effectful shell: the async `EngineHandle::detect_conflicts_once`
  wrapper (gate → reasoner → `spawn_blocking` core), the daemon-side passage-text resolver (reads the session
  `.md`, re-chunks), the background `conflict::sweeper` loop, and the boot-time force-off in `prime_switches`.
- **The judge (`bossclaw-core/src/conflict.rs`).** Already shipped in Phase 0 (`judge_pair`, `Verdict`,
  `Winner`, `CONFLICT_CONF_MIN`). Phase 2 adds the candidate-finder + proposal-hygiene helpers alongside it and
  wires the judge into the sweep.

**Detection only (Phase-2 boundary, do not cross).** NO resolution ops, NO UI, NO guest-reachable op, NO
`conflict_resolved`/`coexist_allowed`/`dismissed` event types (those are Phase 3, added with the resolve ops
that emit them). The candidate-finder takes a resolution-exclusion set that is **empty** in Phase 2 so Phase 3
fills it without reshaping the finder.

**Tech Stack.** Rust (workspace crates `bossclaw-core`, `bossclawd`); `rusqlite`/SQLCipher; `hnsw_rs` ANN
(`HnswIndex`); `serde_json`; tokio (daemon loop + `spawn_blocking`). Tests use `MockEmbedder`,
`ScriptedReasoner`, `MockReasonerProvider`, `MockEmbedderProvider`, `tempfile`. **All cargo commands are
SYNCHRONOUS / foreground** (never backgrounded) so each red→green transition is observed before the next step.

**Non-unix build contract (`#[cfg(unix)]` discipline).** In `bossclaw-core` the entire `write_proposal` family
is `#[cfg(unix)]` — `build_proposer_event` (log.rs:3084), `append_write_proposal_with` (:2665),
`is_proposal_suppressed` (:2732), `pending_proposals` (:2779) — the portable-core discipline (a non-unix build
must still compile). Every new core method that calls `build_proposer_event` (proposal append, its open-set
fold, idempotency predicate, projection, and the `detect_conflicts_once` orchestrator) is therefore ALSO
`#[cfg(unix)]`, and their tests carry the same `#[cfg(unix)]`. The PORTABLE additions stay ungated: `ConflictRef`
+ key codecs, `conflict_search_refs`, the conflict cursor + subject enumeration, `ConfigFlag::ConflictDetect` +
its getter/setter, the pure `decide_conflict_sweep`, and the pub data structs (`ConflictSubject`,
`ConflictProposalRow`, `ConflictDetectReport`). On the `bossclawd` side, `engine`/`identity`/`server` are already
`#[cfg(unix)]` modules (lib.rs), so the engine wrapper and the new `conflict` sweeper module inherit unix-gating
(`#[cfg(unix)] pub mod conflict;`) — no per-fn gates needed there.

### Anchor drift — trust these greps, not the design's §10 line numbers

The design's §10 anchor index drifted from `main` `64207b5`. Verified 2026-07-15:

| Symbol | design §10 | actual (verified) |
| --- | --- | --- |
| `rebuild_conflict_index` (log.rs) | `:5838` | **`:5956`** |
| `conflict_search` (log.rs) | `:5874` | **`:6002`** |
| `build_proposer_event` (log.rs) | `:2677` | **`:3085`** (`:2677` is the tail of `append_write_proposal_with`) |
| `set_evolve_cursor` (log.rs) | `:6090` | **`:6093`** (reader `evolve_cursor` `:6080`) |
| `append_write_proposal_with` | `:2667` | `:2667` ✓ |
| `is_proposal_suppressed` | `:2733` | `:2733` ✓ |
| `pending_proposals` | `:2780` | `:2780` ✓ |
| `ConfigFlag` / `capture_enabled` / `set_capture_enabled` | `:273`/`:6445`/`:6493` | ✓ / ✓ / ✓ |
| `current_notes` / `fold_notes` / `fold_sessions` | — | `:5189` / `:8420` / `:8324` |
| `evolve_once` / `prime_switches` / `evolve_enabled_or_false` (mod.rs) | `:914`/`:529`/`:1006` | ✓ / ✓ / ✓ |
| reasoner obtain / cloud pre-gate (mod.rs) | `:936`/`:926-934` | ✓ / ✓ |
| `judge_pair`/`Verdict`/`Winner`/`CONFLICT_CONF_MIN` (conflict.rs) | `:131`/`:30`/`:16`/`:123` | ✓ |
| `encode_chunk_key`/`decode_chunk_key` (index.rs) | `:46`/`:56` | ✓ |
| sweeper `spawn`/`run_sweep_once`/`SWEEP_INTERVAL`/`CAPTURE_PER_SWEEP`/`decide_sweep` | `:284`/`:186`/`:48`/`:59`/`:137` | ✓ |

**Two substantive realities the design's implicit model missed (baked into this plan):**
1. **`conflict_search` cannot be repurposed.** It returns `Vec<(String, usize, f32)>` (session_id, passage_ix,
   distance) and has a live external caller `crates/memharness/src/retrieval_grade.rs:153`. Changing its
   signature breaks the harness. Phase 2 therefore adds a **sibling** `conflict_search_refs` that returns typed
   `Vec<(ConflictRef, f32)>`; `conflict_search` stays byte-identical.
2. **Passage text is not in core.** `store_session_passages(embedder, event_id, chunks)` persists only
   *vectors*; the chunk *text* lives in the on-disk session `.md` (`<data_dir>/sessions/<id>.md`). The judge
   needs real text (design §3.5). So `EventLog::detect_conflicts_once` takes a
   `passage_text: &dyn Fn(&str, usize) -> Option<String>` closure; the daemon supplies it via
   `read_capture_markdown` → `capture_body` → `chunk_text`. Note text stays fully in core (`current_notes`).
   The candidate-finder needs NO text (it queries the passage's stored vector directly), so only the judge step
   crosses this boundary.

---

## File Structure

| File | Create/Modify | Responsibility |
| --- | --- | --- |
| `crates/bossclaw-core/src/index.rs` | Modify | `NOTE_KEY_SENTINEL`, `encode_note_key`/`decode_note_key`, `enum ConflictRef` + `decode_key`/`pair_key`/`to_json`/`from_json` (Task 1). |
| `crates/bossclaw-core/src/graph.rs` | Modify | `CONFLICT_PROPOSAL_EVENT_TYPE`, `CONFLICT_PROPOSER_PRODUCER` consts (Task 5). |
| `crates/bossclaw-core/src/conflict.rs` | Modify | Phase-2 constants; `confidence_band`, `winner_str`, `bound_judge_text`, **`templated_why`** (content-free persisted reason — I7); the pure `decide_conflict_sweep` + `FinderInput` (Tasks 5, 8). |
| `crates/bossclaw-core/src/log.rs` | Modify | Note arm of `rebuild_conflict_index` + `conflict_search_refs` (2); **2-column** `conflict_cursor`/`set_conflict_cursor` `(seq, subject_offset)` + table + `unprocessed_conflict_subjects_since` (3); `ConfigFlag::ConflictDetect` + `conflict_detect_enabled`/`set_conflict_detect_enabled` (4); `#[cfg(unix)]` `append_conflict_proposal` (5); `#[cfg(unix)]` `open_conflict_proposals` + `is_conflict_proposal_suppressed` (6); `#[cfg(unix)]` `pending_conflict_proposals` + GC (7); `#[cfg(unix)]` `detect_conflicts_once` **subject-by-subject** orchestration + `ConflictDetectReport` (10). |
| `crates/bossclaw-core/src/lib.rs` | Modify | Re-export `ConflictRef`, `ConflictSubject`, `ConflictProposalRow`, `ConflictDetectReport` (Tasks 1, 3, 7, 10). |
| `crates/bossclawd/src/engine/mod.rs` | Modify | `cloud_consent_ok` helper + `evolve_once` refactor (9); `conflict_lock` field; `conflict_tel: Mutex<ConflictTelemetry>` + `record_conflict_tick` + `conflict_telemetry` read (11); `conflict_detect_enabled_or_false`; async `detect_conflicts_once` wrapper (4, 11); `prime_switches` force-off (4). (`mod engine` is `#[cfg(unix)]` — no per-fn gate.) |
| `crates/bossclawd/src/capture/store.rs` | Modify | `pub(crate) fn session_passage_text` daemon passage-text resolver (Task 11). |
| `crates/bossclawd/src/conflict/mod.rs` | **Create** | Module root for the conflict sweeper (Task 12). |
| `crates/bossclawd/src/conflict/sweeper.rs` | **Create** | `ConflictSweepReport`, `run_conflict_sweep_once`, `spawn` — the background loop (Task 12). |
| `crates/bossclawd/src/lib.rs` | Modify | `#[cfg(unix)] pub mod conflict;` (Task 12). |
| `crates/bossclawd/src/main.rs` | Modify | `conflict::sweeper::spawn(...)` off-by-default (Task 13). |

**New constants (all in `bossclaw-core/src/conflict.rs`, provisional / harness-tunable — see §8 of the design):**

```rust
/// Cosine-similarity floor a neighbour must clear to become a candidate pair (cost governor +
/// precision). Conservative-high; harness/owner-tunable. `sim = 1.0 - cosine_distance`.
pub const CANDIDATE_SIM_MIN: f32 = 0.82;
/// Per-cycle judge-call budget; backlog drips across cycles (mirrors `CAPTURE_PER_SWEEP = 8`).
pub const CONFLICT_JUDGE_PER_SWEEP: usize = 8;
/// Open-proposal ceiling: on exceed, stop proposing and surface one quiet "many pending" count.
pub const CONFLICT_OPEN_CEILING: usize = 20;
/// Top-k neighbours pulled from the unified index per subject before the sim gate. Pinned EQUAL to
/// the judge budget (= the per-subject cap) so the finder is STRICTLY LOSSLESS: a subject can find at
/// most `budget` above-floor candidates and ALL of them are kept + judged — never found-then-dropped
/// (owner decision: "never skip"). `search_k <= budget` is the only fully-lossless config that also
/// preserves the no-stall guarantee (one subject's pairs always fit one fresh full budget).
pub const CONFLICT_SEARCH_K: usize = CONFLICT_JUDGE_PER_SWEEP;
/// Max candidate pairs kept per subject (top-similarity). Equals the judge budget so a single
/// subject is always fully judgeable within one full budget — no permanent cursor stall.
pub const MAX_CANDIDATE_PAIRS_PER_SUBJECT: usize = CONFLICT_JUDGE_PER_SWEEP;
/// Max subject EVENTS scanned per cycle since the cursor (a capture expands to its passages).
pub const CONFLICT_SCAN_BOUND: usize = 64;
/// Byte cap on each snippet handed to the judge (inherits SP3's snapshot budget intent).
pub const MAX_JUDGE_TEXT_BYTES: usize = 4096;
/// Confidence at/above which a stored proposal's coarse band is "high" (else "med"). All stored
/// verdicts are already >= CONFLICT_CONF_MIN (70), so this only splits the actionable range.
pub const CONFLICT_BAND_HIGH_MIN: u8 = 85;
```

**I7 — the persisted `why` is a CONTENT-FREE TEMPLATE, never model text.** A sanitized free-text rationale can
still carry a verbatim memory fragment (e.g. a quoted `'master'`/`'main'`) into a SIGNED, append-only event that
outlives the memory's deletion — the exact leak I7 forbids. So the stored `why` is built ONLY from structured,
content-free fields: the advisory `winner_hint`, the coarse `confidence_band`, and the two ref KINDS
(note/passage). The model's raw `why` is NEVER persisted (it may be `eprintln!`'d ephemerally for debug only,
where it is transient and unsigned). There is therefore **no `WHY_MAX_CHARS` / `sanitize_why` on the persistence
path** (removed from the plan) — `conflict::templated_why(...)` replaces them (Task 5).

---

## Task 1 — `ConflictRef` typed key codec (unified fights index, key layer)

Design §2, §3.1, §7 (key-space disjointness). Adds the note-key sentinel + the typed ref so a mixed conflict
index (note bodies + session passages) decodes to `Note{event_id}` or `Passage{session_id, passage_id}` without
key collisions. `conflict_search`'s existing passage codec (`decode_chunk_key`) is untouched.

**Files**
- Modify: `crates/bossclaw-core/src/index.rs` (after `event_id_of` `:64`; note `CHUNK_KEY_SEP = '\u{1f}'` `:43`).
- Modify: `crates/bossclaw-core/src/lib.rs` (`pub use index::{HnswIndex, VectorIndex}` `:63`).
- Test: `crates/bossclaw-core/src/index.rs` (`#[cfg(test)] mod tests` `:256`).

**Steps**

1. Write the failing test (append into `index.rs` `mod tests`):

```rust
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
}
```

2. Run → FAIL: `cargo test -p bossclaw-core note_key_and_passage_key_are_disjoint_and_typed`
   Expected: compile error `cannot find function encode_note_key` / `cannot find type ConflictRef`.

3. Implement (in `index.rs`, after `event_id_of`):

```rust
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
    Note { event_id: String },
    /// A live captured-session passage, identified by the session's stable id + passage ordinal.
    Passage { session_id: String, passage_id: usize },
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
```

   Then in `lib.rs` extend the re-export: `pub use index::{ConflictRef, HnswIndex, VectorIndex};`.

4. Run → PASS: `cargo test -p bossclaw-core note_key_and_passage_key_are_disjoint_and_typed`

5. Commit: `feat(rung3-p2): ConflictRef typed key codec + note-key sentinel for the unified fights index`

---

## Task 2 — Note arm of `rebuild_conflict_index` + `conflict_search_refs` + recall-neutral golden

Design §2, §3.1, §6.2. Extends the conflict index to hold note bodies alongside session passages, and adds a
typed search. The recall `vector_index` stays byte-untouched (Phase 1's `vector_index_len` golden).

**Files**
- Modify: `crates/bossclaw-core/src/log.rs` — `rebuild_conflict_index` (`:5956`), add `conflict_search_refs` after
  `conflict_search` (`:6002`). Uses `current_notes()` (`:5189`), `embed_one` (`:8122`), `encode_note_key`.
- Test: `crates/bossclaw-core/src/log.rs` `#[cfg(test)] mod tests` (next to
  `conflict_index_retrieves_by_session_and_leaves_recall_len_unchanged` `:8597`).

**Steps**

1. Write the failing test (append into `log.rs` `mod tests`; helpers `open_log`, `session_meta`, `MockEmbedder`
   are already in scope):

```rust
/// Rung-3 Phase-2 (§2): the conflict index holds BOTH a note body and a session passage; the
/// typed search returns each as its `ConflictRef` kind; the recall `vector_index` stays untouched.
#[test]
fn conflict_index_note_arm_and_passage_arm_are_both_typed_searchable() {
    use crate::index::ConflictRef;
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let emb = MockEmbedder::new(8);

    // A note whose body is embeddable.
    let note_id = log.remember(&emb, "the default git branch is main").unwrap();
    // A captured session with one passage.
    let ev = log.capture_session(&emb, &session_meta("s1", "aa")).unwrap();
    log.store_session_passages(&emb, &ev, &["we deploy on vercel".to_string()]).unwrap();

    log.rebuild_indexes(&emb).unwrap();
    let recall_len = log.vector_index_len();
    log.rebuild_conflict_index(&emb).unwrap();

    // The note is retrievable as a Note ref.
    let note_hits =
        log.conflict_search_refs(&emb.embed(&["default git branch".into()]).unwrap()[0], 8);
    assert!(
        note_hits.iter().any(|(r, _)| *r == ConflictRef::Note { event_id: note_id.clone() }),
        "note body is a typed Note hit"
    );
    // The passage is retrievable as a Passage ref.
    let pass_hits = log.conflict_search_refs(&emb.embed(&["vercel".into()]).unwrap()[0], 8);
    assert!(
        pass_hits
            .iter()
            .any(|(r, _)| *r == ConflictRef::Passage { session_id: "s1".into(), passage_id: 0 }),
        "passage is a typed Passage hit"
    );
    // The recall index was not perturbed by adding the note arm.
    assert_eq!(log.vector_index_len(), recall_len, "recall vector_index byte-untouched");

    // The legacy passage-tuple search still works (memharness contract).
    let legacy = log.conflict_search(&emb.embed(&["vercel".into()]).unwrap()[0], 8);
    assert!(legacy.iter().any(|(sid, pid, _)| sid == "s1" && *pid == 0));
}
```

2. Run → FAIL: `cargo test -p bossclaw-core conflict_index_note_arm_and_passage_arm_are_both_typed_searchable`
   Expected: `no method named conflict_search_refs`.

3. Implement. In `rebuild_conflict_index` (`:5956`), after the passage loop that ends at
   `index.add(&crate::index::encode_chunk_key(session_id, ix), &vec);` (`:5983`) and **before**
   `let boxed: Box<dyn VectorIndex> = Box::new(index);` (`:5985`), add the note arm:

```rust
        // Note arm (Rung-3 Phase-2 §2): add each CURRENT, non-superseded, non-retired memory
        // note's body vector under a DISTINCT note key so notes and passages share ONE fights
        // index without colliding. `current_notes` already applies the supersede + `note_retired`
        // exclusion, so no extra filtering is needed here. Empty-body notes cannot contradict on
        // content and are skipped (a zero-length body would embed to a meaningless vector).
        //
        // I4 / design open-question #2: we RE-EMBED every current note each rebuild rather than
        // persisting note vectors in a dedicated `note_conflict_vectors` table (like
        // `session_passage_vectors`). This is acceptable at Phase-2 scale because the embedder is a
        // STATIC model2vec (a token-vector lookup + mean-pool, NOT a transformer forward pass), so
        // per-note cost is a cheap table lookup. The `note_conflict_vectors` table is DEFERRED (add
        // only if this cost proves material). The `log::debug!` below is the trip-wire that makes
        // the re-embed count observable before any production-enable.
        let mut notes_embedded = 0usize;
        for note in self.current_notes()? {
            if note.text.trim().is_empty() {
                continue;
            }
            let vec = embed_one(embedder, &note.text)?;
            index.add(&crate::index::encode_note_key(&note.event_id), &vec);
            notes_embedded += 1;
        }
        log::debug!("rebuild_conflict_index: re-embedded {notes_embedded} note bodies (I4 trip-wire)");
```

   Then add the typed search after `conflict_search` (`:6018`):

```rust
    /// Typed sibling of [`EventLog::conflict_search`] (Rung-3 Phase-2 §2): returns the `k` nearest
    /// `(ConflictRef, distance)` pairs over the UNIFIED conflict index, decoding both note keys and
    /// passage chunk keys. `conflict_search` (passage-only tuples) is left byte-identical for the
    /// harness caller. Same empty-when-unbuilt policy + `debug_assert` as `conflict_search`.
    pub fn conflict_search_refs(&self, qv: &[f32], k: usize) -> Vec<(crate::index::ConflictRef, f32)> {
        let guard = self.conflict_index.lock().expect(POISON);
        debug_assert!(guard.is_some(), "conflict_search_refs called before rebuild_conflict_index");
        let Some(index) = guard.as_ref() else {
            return Vec::new();
        };
        index
            .search(qv, k)
            .into_iter()
            .filter_map(|(key, score)| crate::index::ConflictRef::decode_key(&key).map(|r| (r, score)))
            .collect()
    }
```

4. Run → PASS: `cargo test -p bossclaw-core conflict_index_note_arm_and_passage_arm_are_both_typed_searchable`
   Then the Phase-1 golden regressions still green:
   `cargo test -p bossclaw-core conflict_index_ passage_retire_hides_one_survives_sweep_and_reverses`

5. Commit: `feat(rung3-p2): rebuild_conflict_index note arm + typed conflict_search_refs (recall-neutral)`

---

## Task 3 — Conflict cursor + new-subject enumeration

Design §3.2, §3.3 (step 2), resolved open-question #1 (a seq cursor, not a pending-pair queue). **The cursor is
`(seq, subject_offset)`, NOT a bare `seq`** — this is the fix for the multi-passage stall (a single
`session_captured` event = one `seq` but N passage subjects; a bare-`seq` cursor + whole-group deferral would
never advance past a capture whose pairs exceed the budget, starving all newer memories). `subject_offset` is the
within-`seq` boundary: a note has one subject (within-seq id 0); a capture's passages use `passage_id` as the
within-seq id. Detection then advances **subject-by-subject** (Task 10), and because each subject's candidate
pairs are capped at `MAX_CANDIDATE_PAIRS_PER_SUBJECT = CONFLICT_JUDGE_PER_SWEEP = 8`, a fresh full budget ALWAYS
fits ≥1 subject → never stalls (I4: grows with new, not total; nothing is ever dropped).

**Files**
- Modify: `crates/bossclaw-core/src/log.rs` — 2-column DDL beside `evolve_cursor` (`:869`); `conflict_cursor()
  -> (i64, usize)` / `set_conflict_cursor(seq, offset)` beside `evolve_cursor`/`set_evolve_cursor`
  (`:6080`/`:6093`); `unprocessed_conflict_subjects_since(cursor_seq, subject_offset, limit)` beside
  `unprocessed_extractable_since` (`:6538`). Uses `fold_sessions` (`:8324`), `session_passage_count` (`:5913`),
  `ConflictRef`. All portable (ungated) — no `build_proposer_event` call.
- Test: `crates/bossclaw-core/src/log.rs` `mod tests`.

**Steps**

1. Write the failing test:

```rust
/// Rung-3 Phase-2 (§3.2/§3.3): the `(seq, subject_offset)` cursor round-trips + survives reopen; the
/// enumeration returns each NEW note (within-seq id 0) and each NEW capture's live passages (within-
/// seq ids = passage_id); and RESUMING at a within-capture offset skips already-judged passages.
#[test]
fn conflict_cursor_and_subject_enumeration_are_incremental() {
    use crate::index::ConflictRef;
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(8);

    {
        let log = open_log(dir.path());
        assert_eq!(log.conflict_cursor().unwrap(), (0, 0), "unset cursor defaults to (0, 0)");
        let note_id = log.remember(&emb, "branch is main").unwrap();
        let ev = log.capture_session(&emb, &session_meta("s1", "aa")).unwrap();
        log.store_session_passages(&emb, &ev, &["p0".to_string(), "p1".to_string()]).unwrap();

        // From (0, 0): the note (within-seq id 0) + the capture's two passages (within-seq ids 0, 1).
        let subjects = log.unprocessed_conflict_subjects_since(0, 0, 64).unwrap();
        let refs: Vec<ConflictRef> = subjects.iter().map(|s| s.subject.clone()).collect();
        assert!(refs.contains(&ConflictRef::Note { event_id: note_id.clone() }));
        assert!(refs.contains(&ConflictRef::Passage { session_id: "s1".into(), passage_id: 0 }));
        assert!(refs.contains(&ConflictRef::Passage { session_id: "s1".into(), passage_id: 1 }));

        // Within-capture resume: from the capture's seq at offset 1, passage 0 is skipped (judged),
        // passage 1 still pends — this is the anti-stall resume.
        let cap_seq = subjects
            .iter()
            .find(|s| matches!(s.subject, ConflictRef::Passage { .. }))
            .unwrap()
            .seq;
        let resumed = log.unprocessed_conflict_subjects_since(cap_seq, 1, 64).unwrap();
        assert!(
            !resumed.iter().any(|s| matches!(&s.subject, ConflictRef::Passage { passage_id: 0, .. })),
            "passage 0 of the in-progress capture is skipped at offset 1"
        );
        assert!(
            resumed.iter().any(|s| matches!(&s.subject, ConflictRef::Passage { passage_id: 1, .. })),
            "passage 1 still pending at offset 1"
        );

        // Advancing past the last subject empties the queue.
        let max_seq = subjects.iter().map(|s| s.seq).max().unwrap();
        let last_off =
            subjects.iter().filter(|s| s.seq == max_seq).map(|s| s.within_seq_id).max().unwrap();
        log.set_conflict_cursor(max_seq, last_off + 1).unwrap();
        assert!(log.unprocessed_conflict_subjects_since(max_seq, last_off + 1, 64).unwrap().is_empty());
    }
    // Restart: the cursor is persistent progress state (survives reopen).
    let log = open_log(dir.path());
    let (cseq, coff) = log.conflict_cursor().unwrap();
    assert!(cseq > 0, "cursor persisted across reopen");
    assert!(
        log.unprocessed_conflict_subjects_since(cseq, coff, 64).unwrap().is_empty(),
        "nothing new after restart"
    );
}
```

2. Run → FAIL: `cargo test -p bossclaw-core conflict_cursor_and_subject_enumeration_are_incremental`
   Expected: `no method named conflict_cursor`.

3. Implement.
   (a) DDL — after the `evolve_cursor` `store.exec(...)` block (`:869`–`:874`), add:

```rust
        // Conflict-detection progress (Rung-3 Phase-2 §3.2 — re-derivable progress state, NOT a
        // Tier-A fold). Single row (id pinned to 0). `(last_seq, subject_offset)` advances
        // subject-by-subject: all subjects of the event at `last_seq` with within-seq id
        // < subject_offset are judged. Losing it only re-searches (idempotent: an already-open
        // pair is never re-proposed).
        store.exec(
            "CREATE TABLE IF NOT EXISTS conflict_cursor (
                id             INTEGER PRIMARY KEY CHECK (id = 0),
                last_seq       INTEGER NOT NULL,
                subject_offset INTEGER NOT NULL
            )",
        )?;
```

   (b) Getter/setter — after `set_evolve_cursor` (`:6101`):

```rust
    /// Read the conflict cursor `(last_seq, subject_offset)`; `(0, 0)` if never set. All subjects of
    /// the event at `last_seq` with within-seq id `< subject_offset` are fully judged (a note is one
    /// subject at id 0; a capture's passages use `passage_id`). Persistent progress state, NOT a fold
    /// (spec §3.2) — losing it only re-searches idempotently.
    pub fn conflict_cursor(&self) -> Result<(i64, usize), BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let row: Option<(i64, i64)> = store
            .conn()
            .query_row(
                "SELECT last_seq, subject_offset FROM conflict_cursor WHERE id = 0",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(row.map(|(s, o)| (s, o as usize)).unwrap_or((0, 0)))
    }

    /// Advance the conflict cursor to `(last_seq, subject_offset)` (idempotent single-row upsert).
    pub fn set_conflict_cursor(
        &self,
        last_seq: i64,
        subject_offset: usize,
    ) -> Result<(), BossclawError> {
        let store = self.inner.lock().expect(POISON);
        store.conn().execute(
            "INSERT INTO conflict_cursor (id, last_seq, subject_offset) VALUES (0, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET last_seq = ?1, subject_offset = ?2",
            rusqlite::params![last_seq, subject_offset as i64],
        )?;
        Ok(())
    }
```

   (c) Subject type + enumeration. Declare `ConflictSubject` at MODULE level (beside `CurrentNote` `:415`) —
   `#![deny(missing_docs)]` (lib.rs:17) requires a `///` on every pub field:

```rust
    /// One conflict-detection SUBJECT: a memory appended after the cursor. A `memory` event yields
    /// ONE `Note` subject; a `session_captured` event yields one `Passage` subject per live
    /// (non-retired) passage.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct ConflictSubject {
        /// The source event's `seq` (the cursor's first coordinate).
        pub seq: i64,
        /// This subject's within-`seq` id — its `passage_id` for a passage, `0` for a note. The
        /// cursor's second coordinate advances to `within_seq_id + 1` once this subject is judged.
        pub within_seq_id: usize,
        /// The typed reference this subject searches the fights index for.
        pub subject: crate::index::ConflictRef,
    }
```

   The enumeration — after `unprocessed_extractable_since` (`:6562`):

```rust
    /// New conflict subjects at or after the cursor position, `(seq ASC, within_seq_id ASC)`, from
    /// at most `limit` source EVENTS (a capture expands to its passages, so the returned Vec may
    /// exceed `limit`). For the IN-PROGRESS event (`seq == cursor_seq`) subjects with within-seq id
    /// `< subject_offset` are skipped (already judged); newer events skip nothing. Notes: only
    /// CURRENT memory events. Passages: only a capture that is the CURRENT head for its `session_id`
    /// (not superseded / tombstoned) and only its non-retired passages, ordered by `passage_id`
    /// (retiring a passage skips it but does NOT renumber siblings, so `subject_offset` stays valid
    /// across cycles).
    pub fn unprocessed_conflict_subjects_since(
        &self,
        cursor_seq: i64,
        subject_offset: usize,
        limit: usize,
    ) -> Result<Vec<ConflictSubject>, BossclawError> {
        use crate::index::ConflictRef;
        // Source events at OR after the cursor's seq (the in-progress event is re-scanned so its
        // not-yet-judged subjects resume), oldest first, bounded.
        let rows: Vec<(i64, String, String)> = {
            let store = self.inner.lock().expect(POISON);
            let conn = store.conn();
            let mut stmt = conn.prepare(
                "SELECT seq, id, event_type FROM events
                 WHERE event_type IN (?1, ?2) AND seq >= ?3 ORDER BY seq ASC LIMIT ?4",
            )?;
            let mapped = stmt.query_map(
                rusqlite::params![
                    MEMORY_EVENT_TYPE,
                    crate::graph::SESSION_CAPTURED_EVENT_TYPE,
                    cursor_seq,
                    limit as i64
                ],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)),
            )?;
            let mut out = Vec::new();
            for row in mapped {
                out.push(row?);
            }
            out
        };
        // Fold once for the current-head + retired-passage checks (deterministic).
        let fold = fold_sessions(&self.session_events_ordered()?);
        let current_note_ids: std::collections::HashSet<String> =
            self.current_notes()?.into_iter().map(|n| n.event_id).collect();

        let mut subjects = Vec::new();
        for (seq, id, etype) in rows {
            // Skip the already-judged prefix ONLY for the in-progress event; newer events skip 0.
            let skip_below = if seq == cursor_seq { subject_offset } else { 0 };
            if etype == MEMORY_EVENT_TYPE {
                // A note is ONE subject at within-seq id 0 — included iff not already judged.
                if skip_below == 0 && current_note_ids.contains(&id) {
                    subjects.push(ConflictSubject {
                        seq,
                        within_seq_id: 0,
                        subject: ConflictRef::Note { event_id: id },
                    });
                }
                continue;
            }
            // session_captured: only the CURRENT head for its session_id contributes passages.
            let Some(cs) = fold.current.iter().find(|cs| cs.event_id == id) else {
                continue; // superseded by a newer capture, or tombstoned — not a subject
            };
            let sid = cs.session_id.clone();
            let n = self.session_passage_count(&id)?;
            for pid in skip_below..n {
                if fold.retired_passages.contains(&(sid.clone(), pid)) {
                    continue; // retired passage — never a subject
                }
                subjects.push(ConflictSubject {
                    seq,
                    within_seq_id: pid,
                    subject: ConflictRef::Passage { session_id: sid.clone(), passage_id: pid },
                });
            }
        }
        Ok(subjects)
    }
```

   Re-export the struct: in `lib.rs`, extend the `pub use log::{...}` block (`:64`) with `ConflictSubject`.

4. Run → PASS: `cargo test -p bossclaw-core conflict_cursor_and_subject_enumeration_are_incremental`

5. Commit: `feat(rung3-p2): conflict cursor + new-subject enumeration (notes + capture passages)`

---

## Task 4 — `ConfigFlag::ConflictDetect` owner gate (default-CLOSED) + boot force-off

Design §3.6, §6.6, invariant I3. Adds the off-by-default flag (mirrors `capture_enabled`), the boot force-off in
`prime_switches`, and the infallible daemon read `conflict_detect_enabled_or_false`.

**Files**
- Modify: `crates/bossclaw-core/src/log.rs` — `CONFLICT_DETECT_ENABLED_KEY` const beside `CAPTURE_ENABLED_KEY`
  (`:254`); `ConfigFlag::ConflictDetect` variant + `key()` arm (`:273`/`:294`); `conflict_detect_enabled`
  (mirror `capture_enabled` `:6445`); `set_conflict_detect_enabled` (mirror `set_mandates_enabled` `:6390`).
- Modify: `crates/bossclawd/src/engine/mod.rs` — `prime_switches` force-off (`:549`);
  `conflict_detect_enabled_or_false` (mirror `evolve_enabled_or_false` `:1006`).
- Test: `crates/bossclaw-core/src/log.rs` `mod tests`; `crates/bossclawd/src/engine/mod.rs` `mod tests`.

**Steps**

1. Write the failing core test:

```rust
/// Rung-3 Phase-2 (§3.6, I3): conflict-detect is DEFAULT-CLOSED, is sticky once set, and
/// registers as explicitly-set (what the boot force-off keys off).
#[test]
fn conflict_detect_flag_is_default_closed_and_sticky() {
    use crate::ConfigFlag;
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    assert!(!log.conflict_detect_enabled().unwrap(), "default CLOSED");
    assert!(!log.explicitly_set(ConfigFlag::ConflictDetect).unwrap(), "never set yet");
    log.set_conflict_detect_enabled(true).unwrap();
    assert!(log.conflict_detect_enabled().unwrap(), "sticky ON after set");
    assert!(log.explicitly_set(ConfigFlag::ConflictDetect).unwrap(), "now explicit");
    log.set_conflict_detect_enabled(false).unwrap();
    assert!(!log.conflict_detect_enabled().unwrap(), "sticky OFF");
}
```

2. Run → FAIL: `cargo test -p bossclaw-core conflict_detect_flag_is_default_closed_and_sticky`
   Expected: `no variant ConflictDetect` / `no method conflict_detect_enabled`.

3. Implement (core).
   (a) Const beside `CAPTURE_ENABLED_KEY` (`:254`):

```rust
/// The `content` key carrying the Rung-3 Phase-2 conflict-detection on/off switch (spec §3.6).
/// Single-sourced (one writer [`EventLog::set_conflict_detect_enabled`], one reader
/// [`EventLog::conflict_detect_enabled`]). DEFAULT CLOSED — detection never runs for a user who
/// never consented (invariant I3), exactly like [`CAPTURE_ENABLED_KEY`].
const CONFLICT_DETECT_ENABLED_KEY: &str = "conflict_detect_enabled";
```

   (b) `ConfigFlag` variant (after `BackfillConsented` `:289`) + `key()` arm (after the `BackfillConsented`
   arm `:303`):

```rust
    /// The Rung-3 Phase-2 conflict-detection on/off switch ([`CONFLICT_DETECT_ENABLED_KEY`]). Default CLOSED.
    ConflictDetect,
```
```rust
            ConfigFlag::ConflictDetect => CONFLICT_DETECT_ENABLED_KEY,
```

   (c) Reader (mirror `capture_enabled` `:6445`) + writer (mirror `set_mandates_enabled` `:6390`), placed after
   `capture_enabled_at` (`:6473`):

```rust
    /// Whether Rung-3 conflict detection is enabled (spec §3.6). STICKY / fail-closed via
    /// [`EventLog::latest_config_value`]'s newest-first scan; DEFAULT CLOSED (a never-set flag
    /// reads `false`), so the sweep never runs for a user who never consented (I3).
    pub fn conflict_detect_enabled(&self) -> Result<bool, BossclawError> {
        Ok(self
            .latest_config_value(ConfigFlag::ConflictDetect.key())?
            .and_then(|v| v.as_bool())
            .unwrap_or(false))
    }

    /// Flip the conflict-detection switch by appending ONE signed + hash-chained control `config`
    /// event `{ "conflict_detect_enabled": <enabled> }`. The ONLY writer of the key (so the reader
    /// can never drift the shape). Carries no model fields → never disturbs `active_model`. Mirrors
    /// [`EventLog::set_mandates_enabled`].
    pub fn set_conflict_detect_enabled(&self, enabled: bool) -> Result<(), BossclawError> {
        self.append(Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: CONFIG_EVENT_TYPE.to_string(),
            content: serde_json::Value::Object({
                let mut m = serde_json::Map::new();
                m.insert(CONFLICT_DETECT_ENABLED_KEY.to_string(), serde_json::Value::Bool(enabled));
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
```

4. Run → PASS (core): `cargo test -p bossclaw-core conflict_detect_flag_is_default_closed_and_sticky`

5. Write the failing daemon test (in `engine/mod.rs` `mod tests`; the harness helpers `new_test_handle`,
   `TestVault`, `tempfile::tempdir` are already used there — mirror an existing `evolve_enabled_or_false` test):

```rust
    #[tokio::test]
    async fn prime_switches_forces_conflict_detect_off_and_or_false_reads_it() {
        let dir = tempfile::tempdir().unwrap();
        let vault = TestVault::new(); // TestVault::new() already returns Arc<TestVault> (mod.rs:1830)
        let h = EngineHandle::new(
            vault,
            dir.path().to_path_buf(),
            std::sync::Arc::new(embed::MockEmbedderProvider::new(8)),
            std::sync::Arc::new(crate::engine::reason::MockReasonerProvider::new("m")),
        );
        let onboarded = true;
        // Fresh brain: prime_switches (run inside get_or_open's first-open) forced an explicit OFF,
        // and the infallible daemon read reports false.
        assert!(!h.conflict_detect_enabled_or_false(onboarded).await, "off by default after boot");
        // Prove prime_switches wrote the tamper-evident EXPLICIT-OFF record (not just that the
        // default reads false): explicitly_set must be true after boot.
        let log = h.get_or_open(onboarded).await.unwrap();
        assert!(
            spawn_blocking(move || log
                .explicitly_set(bossclaw_core::ConfigFlag::ConflictDetect)
                .unwrap())
            .await
            .unwrap(),
            "prime_switches persisted an explicit OFF (explicitly_set == true after boot)"
        );
    }
```

   (Match the exact `EngineHandle::new` / `TestVault` construction of the nearest existing async test in that
   module — see `:1912`/`:1971` for the established shape.)

6. Run → FAIL: `cargo test -p bossclawd prime_switches_forces_conflict_detect_off_and_or_false_reads_it`
   Expected: `no method named conflict_detect_enabled_or_false`.

7. Implement (daemon).
   (a) In `prime_switches` (`:549`, after the capture force-off block) add:

```rust
        // Rung-3 Phase-2 (§3.6, I3): conflict detection is default-CLOSED — its getter already
        // returns false when unset — so, like capture above, we persist an EXPLICIT OFF the first
        // time it was never set (a tamper-evident "this brain has conflict-detect off" record).
        // Idempotent: `explicitly_set` is true afterward.
        if !log.explicitly_set(ConfigFlag::ConflictDetect)? {
            log.set_conflict_detect_enabled(false)?;
        }
```

   (b) After `evolve_enabled_or_false` (`:1011`) add the infallible read:

```rust
    /// The conflict-detection off-switch verdict, defaulting to `false` (OFF) on ANY error (not
    /// onboarded, open failure, …). The gate the conflict sweep reads each cycle — it must never
    /// propagate an error (a transient read failure must not trip detection ON). Mirrors
    /// [`Self::evolve_enabled_or_false`].
    pub async fn conflict_detect_enabled_or_false(&self, onboarded: bool) -> bool {
        let Ok(log) = self.get_or_open(onboarded).await else {
            return false;
        };
        spawn_blocking(move || log.conflict_detect_enabled().unwrap_or(false))
            .await
            .unwrap_or(false)
    }
```

8. Run → PASS: `cargo test -p bossclawd prime_switches_forces_conflict_detect_off_and_or_false_reads_it`

9. Commit: `feat(rung3-p2): ConfigFlag::ConflictDetect default-closed gate + boot force-off + daemon read`

---

## Task 5 — `conflict_proposal` event + `append_conflict_proposal` + output hygiene helpers

Design §3.5, invariants I5/I7/I9. Adds the signed event type, the append builder (mirrors the `#[cfg(unix)]`
`write_proposal` family), and the content-free output helpers. **I7 (owner-mandated): the persisted `why` is a
CONTENT-FREE TEMPLATE** built only from `winner_hint` + `confidence_band` + the two ref KINDS — NEVER the model's
free text (a sanitized rationale can still carry a verbatim memory fragment into a signed, deletion-surviving
event). The proposal stores ONLY typed refs + advisory hint + band + templated `why` + `detected_at` — never
memory bodies. **`append_conflict_proposal` calls `build_proposer_event` (which is `#[cfg(unix)]`), so it — and
its test — are `#[cfg(unix)]`.**

**Files**
- Modify: `crates/bossclaw-core/src/graph.rs` — `CONFLICT_PROPOSAL_EVENT_TYPE` + `CONFLICT_PROPOSER_PRODUCER`
  beside `WRITE_PROPOSAL_EVENT_TYPE` (`:94`) / `M6B_PROPOSER_PRODUCER` (`:102`). (Ungated consts.)
- Modify: `crates/bossclaw-core/src/conflict.rs` — Phase-2 constants (§File Structure list) + `confidence_band`,
  `winner_str`, `bound_judge_text`, **`templated_why`** (after `judge_pair` `:138`). All portable (ungated).
- Modify: `crates/bossclaw-core/src/log.rs` — `#[cfg(unix)] append_conflict_proposal` (uses `build_proposer_event`
  `:3085`, itself `#[cfg(unix)]`).
- Test: `crates/bossclaw-core/src/conflict.rs` `mod tests` (`:140`, ungated); `crates/bossclaw-core/src/log.rs`
  `mod tests` (the append test is `#[cfg(unix)]`).

**Steps**

1. Write the failing hygiene test (in `conflict.rs` `mod tests`):

```rust
    #[test]
    fn templated_why_is_content_free_band_coarse_and_text_bounded() {
        // I7: the persisted `why` is built ONLY from winner + band + ref kinds — never memory text.
        // Feed the two memory strings NOWHERE; the template cannot contain them.
        let w = templated_why("newer", "high", "note", "passage");
        assert!(w.contains("high confidence"), "band phrase present");
        assert!(!w.is_empty());
        // Coarse band: >=85 high, else med (all stored verdicts are already >=70).
        assert_eq!(confidence_band(CONFLICT_BAND_HIGH_MIN), "high");
        assert_eq!(confidence_band(CONFLICT_BAND_HIGH_MIN - 1), "med");
        // Advisory winner serializes to the three stable labels.
        assert_eq!(winner_str(Winner::Older), "older");
        assert_eq!(winner_str(Winner::Newer), "newer");
        assert_eq!(winner_str(Winner::Unclear), "unclear");
        // Judge text is bounded on a char boundary (never panics on multibyte).
        let multi = "é".repeat(MAX_JUDGE_TEXT_BYTES);
        assert!(bound_judge_text(&multi).len() <= MAX_JUDGE_TEXT_BYTES);
    }
```

2. Run → FAIL: `cargo test -p bossclaw-core templated_why_is_content_free_band_coarse_and_text_bounded`
   Expected: `cannot find function templated_why`.

3. Implement.
   (a) `graph.rs` — after `M6B_PROPOSER_PRODUCER` (`:102`):

```rust
/// Rung-3 Phase-2 conflict-detection proposal event type (signed): a "possible conflict" record
/// listing both sides as typed refs, awaiting an owner decision. Detection-only — carries NO
/// mutation. Single-sourced so the builder and every fold/projection filter share the string.
pub const CONFLICT_PROPOSAL_EVENT_TYPE: &str = "conflict_proposal";
/// `model_meta.model_id` producer stamp for Rung-3 conflict proposals.
pub const CONFLICT_PROPOSER_PRODUCER: &str = "rung3-conflict-detector";
```

   (b) `conflict.rs` — add the constants block from §File Structure, then the helpers after `judge_pair`
   (`:138`):

```rust
/// Coarse confidence band for a STORED proposal (I7): the model's numeric confidence is never
/// persisted, only "high"/"med". All stored verdicts already cleared `CONFLICT_CONF_MIN`.
pub fn confidence_band(confidence: u8) -> &'static str {
    if confidence >= CONFLICT_BAND_HIGH_MIN { "high" } else { "med" }
}

/// The stable wire label for an advisory `winner`. The engine resolves the true winner by
/// timestamp; this is a hint only (spec §4d).
pub fn winner_str(w: Winner) -> &'static str {
    match w {
        Winner::Newer => "newer",
        Winner::Older => "older",
        Winner::Unclear => "unclear",
    }
}

/// Bound one snippet handed to the judge to `MAX_JUDGE_TEXT_BYTES`, truncating on a char
/// boundary (never splits a multibyte scalar). The on-disk memory is untouched.
pub fn bound_judge_text(s: &str) -> &str {
    if s.len() <= MAX_JUDGE_TEXT_BYTES {
        return s;
    }
    let mut end = MAX_JUDGE_TEXT_BYTES;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Build the CONTENT-FREE `why` persisted on a proposal (I7). Composed ONLY from structured,
/// non-memory fields — the advisory `winner_hint` ("newer"/"older"/anything-else = unclear), the
/// coarse `confidence_band` ("high"/other = medium), and the two ref KINDS ("note"/"passage") — so
/// a signed proposal NEVER carries a verbatim memory fragment that could outlive the memory's
/// deletion. `a_kind` is the OLDER side, `b_kind` the NEWER (the caller orders by ingest ts). The
/// model's own free-text rationale is discarded (it may be `eprintln!`'d ephemerally for debug).
pub fn templated_why(winner_hint: &str, band: &str, a_kind: &str, b_kind: &str) -> String {
    let subjects = match (a_kind, b_kind) {
        ("note", "note") => "an older note and a newer note",
        ("passage", "passage") => "an older captured-session passage and a newer one",
        ("note", "passage") => "an older note and a newer captured-session passage",
        ("passage", "note") => "an older captured-session passage and a newer note",
        _ => "two memories",
    };
    let relation = match winner_hint {
        "newer" => "the newer appears to supersede the older",
        "older" => "the older appears to remain correct over the newer",
        _ => "they appear to conflict (winner unclear)",
    };
    let band_phrase = if band == "high" { "high confidence" } else { "medium confidence" };
    format!("{subjects} may conflict: {relation}; {band_phrase}")
}
```

4. Run → PASS: `cargo test -p bossclaw-core templated_why_is_content_free_band_coarse_and_text_bounded`

5. Write the failing append test (in `log.rs` `mod tests`):

```rust
/// Rung-3 Phase-2 (§3.5, I5/I7): a conflict proposal is a signed event carrying ONLY typed refs,
/// an advisory winner hint, a coarse band, a CONTENT-FREE templated `why`, and `detected_at` —
/// never a memory body. `#[cfg(unix)]` (mirrors the write_proposal family / `build_proposer_event`).
#[cfg(unix)]
#[test]
fn append_conflict_proposal_stores_typed_refs_and_no_body() {
    use crate::index::ConflictRef;
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let a = ConflictRef::Note { event_id: "n_old".into() };
    let b = ConflictRef::Passage { session_id: "s1".into(), passage_id: 2 };
    let why = crate::conflict::templated_why("newer", "high", "note", "passage");
    let id = log
        .append_conflict_proposal(&a, &b, "newer", "high", &why, 1_720_000_000, &["n_old".into(), "cap_ev".into()])
        .unwrap();
    let ev = log.event_by_id(&id).unwrap().unwrap();
    assert_eq!(ev.event_type, crate::graph::CONFLICT_PROPOSAL_EVENT_TYPE);
    assert_eq!(ConflictRef::from_json(&ev.content["a_ref"]), Some(a));
    assert_eq!(ConflictRef::from_json(&ev.content["b_ref"]), Some(b));
    assert_eq!(ev.content["winner_hint"], "newer");
    assert_eq!(ev.content["confidence_band"], "high");
    assert_eq!(ev.content["why"], why, "stored why is the content-free template");
    assert_eq!(ev.content["detected_at"], 1_720_000_000i64);
    // I7: no memory-body / raw-text / raw-confidence field is persisted (only refs + template why).
    for forbidden in ["text", "a_text", "b_text", "body", "confidence"] {
        assert!(ev.content.get(forbidden).is_none(), "no {forbidden} field on the proposal");
    }
    // Lineage is the referenced memory event ids.
    let sources = ev.model_meta.as_ref().unwrap().source_event_ids.clone();
    assert_eq!(sources, vec!["n_old".to_string(), "cap_ev".to_string()]);
}
```

6. Run → FAIL: `cargo test -p bossclaw-core append_conflict_proposal_stores_typed_refs_and_no_body`
   Expected: `no method named append_conflict_proposal`.

7. Implement (in `log.rs`, near the `write_proposal` family, e.g. after `pending_proposals` `:2842`).
   `#[cfg(unix)]` because it calls the `#[cfg(unix)]` `build_proposer_event`:

```rust
    /// Append a signed Rung-3 `conflict_proposal` (spec §3.5). Content: `{ a_ref, b_ref,
    /// winner_hint, confidence_band, why, detected_at }` — typed stable refs only, NO memory
    /// bodies (I7). `winner_hint`/`confidence_band` are the coarsened forms; `why` MUST be the
    /// CONTENT-FREE `conflict::templated_why` output (never model text). `source_event_ids` is the
    /// referenced memories' lineage (note event id / session capture event id). Mirrors the
    /// `#[cfg(unix)]` `build_proposer_event` shape used by the write-proposal family.
    #[cfg(unix)]
    pub fn append_conflict_proposal(
        &self,
        a_ref: &crate::index::ConflictRef,
        b_ref: &crate::index::ConflictRef,
        winner_hint: &str,
        confidence_band: &str,
        why: &str,
        detected_at: i64,
        source_event_ids: &[String],
    ) -> Result<String, BossclawError> {
        let content = serde_json::json!({
            "a_ref": a_ref.to_json(),
            "b_ref": b_ref.to_json(),
            "winner_hint": winner_hint,
            "confidence_band": confidence_band,
            "why": why,
            "detected_at": detected_at,
        });
        self.append(self.build_proposer_event(
            crate::graph::CONFLICT_PROPOSER_PRODUCER,
            crate::graph::CONFLICT_PROPOSAL_EVENT_TYPE,
            content,
            source_event_ids,
        ))
    }
```

8. Run → PASS: `cargo test -p bossclaw-core append_conflict_proposal_stores_typed_refs_and_no_body`

9. Commit: `feat(rung3-p2): conflict_proposal event + append builder + output-hygiene helpers`

---

## Task 6 — Open-set helper + `is_conflict_proposal_suppressed` (idempotency)

Design §3.5, §3.7, invariant I9. A proposal is OPEN iff BOTH refs still resolve to current memories (the GC
half, Task 7) — Phase 2 has no resolution family, so referential integrity is the only resolver. This task adds
the shared open-set fold and the idempotency predicate keyed on the **unordered pair of typed refs**. Both
projection (Task 7) and the sweep (Task 10) consume it, so it is single-sourced.

**Files**
- Modify: `crates/bossclaw-core/src/log.rs` — `open_conflict_proposals` (internal) + `is_conflict_proposal_suppressed`
  (public). Uses `events_of_types` (`:6568`), `current_notes` (`:5189`), `fold_sessions` (`:8324`),
  `ConflictRef::from_json`/`pair_key`.
- Test: `crates/bossclaw-core/src/log.rs` `mod tests`.

**Steps**

1. Write the failing test:

```rust
/// Rung-3 Phase-2 (§3.5, I9): a proposal for an unordered typed pair suppresses a duplicate for
/// the SAME pair (either order) but not a different pair. `#[cfg(unix)]` (uses the append family).
#[cfg(unix)]
#[test]
fn conflict_proposal_idempotency_is_unordered_by_typed_pair() {
    use crate::index::ConflictRef;
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(8);
    let log = open_log(dir.path());
    // Two CURRENT notes so both refs resolve (open).
    let n1 = log.remember(&emb, "branch is master").unwrap();
    let n2 = log.remember(&emb, "renamed default branch to main").unwrap();
    let a = ConflictRef::Note { event_id: n1.clone() };
    let b = ConflictRef::Note { event_id: n2.clone() };
    let why = crate::conflict::templated_why("newer", "high", "note", "note");
    assert!(!log.is_conflict_proposal_suppressed(&a, &b).unwrap(), "no proposal yet");
    log.append_conflict_proposal(&a, &b, "newer", "high", &why, 1, &[n1.clone(), n2.clone()]).unwrap();
    assert!(log.is_conflict_proposal_suppressed(&a, &b).unwrap(), "same pair suppressed");
    assert!(log.is_conflict_proposal_suppressed(&b, &a).unwrap(), "reversed order also suppressed");
    // A different pair is not suppressed.
    let n3 = log.remember(&emb, "unrelated note").unwrap();
    let c = ConflictRef::Note { event_id: n3 };
    assert!(!log.is_conflict_proposal_suppressed(&a, &c).unwrap(), "different pair not suppressed");
}
```

2. Run → FAIL: `cargo test -p bossclaw-core conflict_proposal_idempotency_is_unordered_by_typed_pair`
   Expected: `no method named is_conflict_proposal_suppressed`.

3. Implement (in `log.rs`). Declare the private row struct at **MODULE level** (a `struct` cannot live inside an
   `impl` block — beside `SessionFold`), then the open-set fold + the predicate in the impl. All three are
   `#[cfg(unix)]` (they underpin the `#[cfg(unix)]` projection + sweep and are dead code otherwise):

```rust
// MODULE level (not inside `impl EventLog`), e.g. beside `SessionFold`.
/// One OPEN conflict proposal (both refs still current). Internal to the projection + idempotency +
/// sweep; the PUBLIC projection row is [`ConflictProposalRow`] (Task 7). Private, so no field docs.
#[cfg(unix)]
struct OpenConflictProposal {
    id: String,
    a_ref: crate::index::ConflictRef,
    b_ref: crate::index::ConflictRef,
    winner_hint: String,
    confidence_band: String,
    why: String,
    detected_at: i64,
}
```

```rust
    // In `impl EventLog`:

    /// Every `conflict_proposal` whose BOTH refs STILL resolve to a current memory — the OPEN set.
    /// A referenced memory being retired / deleted / superseded (edited) drops the ref from the
    /// current sets, so the proposal is auto-withdrawn (I-gc) — no withdrawal event needed. Oldest
    /// first (`events_of_types` is `seq ASC`). Shared by `pending_conflict_proposals` (Task 7) and
    /// `is_conflict_proposal_suppressed`. `#[cfg(unix)]` (feeds the append/projection/sweep family).
    #[cfg(unix)]
    fn open_conflict_proposals(&self) -> Result<Vec<OpenConflictProposal>, BossclawError> {
        use crate::index::ConflictRef;
        // Current membership: notes by event id; sessions by session_id; retired passages.
        let current_note_ids: std::collections::HashSet<String> =
            self.current_notes()?.into_iter().map(|n| n.event_id).collect();
        let fold = fold_sessions(&self.session_events_ordered()?);
        let current_sessions: std::collections::HashSet<&str> =
            fold.current.iter().map(|cs| cs.session_id.as_str()).collect();
        let ref_is_current = |r: &ConflictRef| -> bool {
            match r {
                ConflictRef::Note { event_id } => current_note_ids.contains(event_id),
                ConflictRef::Passage { session_id, passage_id } => {
                    current_sessions.contains(session_id.as_str())
                        && !fold.retired_passages.contains(&(session_id.clone(), *passage_id))
                }
            }
        };
        let mut out = Vec::new();
        for ev in self.events_of_types(&[crate::graph::CONFLICT_PROPOSAL_EVENT_TYPE])? {
            let (Some(a_ref), Some(b_ref)) = (
                ev.content.get("a_ref").and_then(ConflictRef::from_json),
                ev.content.get("b_ref").and_then(ConflictRef::from_json),
            ) else {
                continue; // malformed — never open
            };
            if !ref_is_current(&a_ref) || !ref_is_current(&b_ref) {
                continue; // GC: a side is gone → withdrawn
            }
            out.push(OpenConflictProposal {
                id: ev.id.clone(),
                a_ref,
                b_ref,
                winner_hint: ev.content.get("winner_hint").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                confidence_band: ev.content.get("confidence_band").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                why: ev.content.get("why").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                detected_at: ev.content.get("detected_at").and_then(|v| v.as_i64()).unwrap_or(0),
            });
        }
        Ok(out)
    }

    /// The unordered pair key for two typed refs (sorted `pair_key`s). Two refs in either order
    /// map to the SAME key — the idempotency identity (spec §3.5). `#[cfg(unix)]` (only the sweep +
    /// idempotency predicate — both `#[cfg(unix)]` — use it).
    #[cfg(unix)]
    fn conflict_pair_key(a: &crate::index::ConflictRef, b: &crate::index::ConflictRef) -> String {
        let (ka, kb) = (a.pair_key(), b.pair_key());
        if ka <= kb { format!("{ka}\u{1e}{kb}") } else { format!("{kb}\u{1e}{ka}") }
    }

    /// True iff an OPEN `conflict_proposal` already exists for the unordered typed pair `(a, b)`
    /// (spec §3.5). A GC-withdrawn proposal (a referenced memory gone) does NOT suppress — the
    /// pair may re-propose (so a materially-changed / re-added memory re-opens, resolved Q3).
    /// `#[cfg(unix)]` (consumes `open_conflict_proposals`).
    #[cfg(unix)]
    pub fn is_conflict_proposal_suppressed(
        &self,
        a: &crate::index::ConflictRef,
        b: &crate::index::ConflictRef,
    ) -> Result<bool, BossclawError> {
        let want = Self::conflict_pair_key(a, b);
        Ok(self
            .open_conflict_proposals()?
            .iter()
            .any(|p| Self::conflict_pair_key(&p.a_ref, &p.b_ref) == want))
    }
```

   Note: `conflict_pair_key` is an associated fn (`Self::conflict_pair_key`) so Task 10 can reuse it.

4. Run → PASS: `cargo test -p bossclaw-core conflict_proposal_idempotency_is_unordered_by_typed_pair`

5. Commit: `feat(rung3-p2): open-set fold + unordered-pair conflict-proposal idempotency`

---

## Task 7 — `pending_conflict_proposals` projection + GC withdrawal

Design §3.5, §6.4, invariant I-gc. The public read surface (a later phase's App read), plus the referential-
integrity GC proof: retiring / deleting / editing a referenced memory withdraws its open proposals (and frees
the pair to re-propose), fold-derived so it survives restart.

**Files**
- Modify: `crates/bossclaw-core/src/log.rs` — public `ConflictProposalRow` struct + `pending_conflict_proposals`
  (thin map over `open_conflict_proposals`). Uses `retire_memory` (`:4824`), `retire_passage` (`:4877`),
  `delete_session`-family (`SESSION_DELETED_EVENT_TYPE`), `supersede_note` for the edit case.
- Modify: `crates/bossclaw-core/src/lib.rs` — re-export `ConflictProposalRow`.
- Test: `crates/bossclaw-core/src/log.rs` `mod tests`.

**Steps**

1. Write the failing GC test:

```rust
/// Rung-3 Phase-2 (§6.4, I-gc): pending lists an open proposal; retiring / deleting / editing a
/// referenced memory withdraws it (fold-derived → restart-safe) and frees the pair to re-propose.
/// `#[cfg(unix)]` (uses the append/projection family).
#[cfg(unix)]
#[test]
fn pending_conflict_proposals_project_and_gc_withdraw() {
    use crate::index::ConflictRef;
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(8);
    let log = open_log(dir.path());
    let n1 = log.remember(&emb, "branch is master").unwrap();
    let n2 = log.remember(&emb, "renamed default branch to main").unwrap();
    let a = ConflictRef::Note { event_id: n1.clone() };
    let b = ConflictRef::Note { event_id: n2.clone() };
    let why = crate::conflict::templated_why("newer", "high", "note", "note");
    log.append_conflict_proposal(&a, &b, "newer", "high", &why, 1, &[n1.clone(), n2.clone()]).unwrap();
    assert_eq!(log.pending_conflict_proposals().unwrap().len(), 1, "one open proposal");

    // Retire one referenced note → the proposal is GC-withdrawn and the pair is re-proposable.
    log.retire_memory(&n1).unwrap();
    assert!(log.pending_conflict_proposals().unwrap().is_empty(), "withdrawn on retire");
    assert!(!log.is_conflict_proposal_suppressed(&a, &b).unwrap(), "pair freed to re-propose");

    // Unretire restores currency → the SAME signed proposal is open again (fold-derived).
    log.unretire(&n1).unwrap();
    assert_eq!(log.pending_conflict_proposals().unwrap().len(), 1, "restored on unretire");

    // Editing (supersede) a referenced note mints a NEW id → old ref no longer current → withdrawn.
    log.supersede_note(&emb, &n2, "default branch is main now").unwrap();
    assert!(log.pending_conflict_proposals().unwrap().is_empty(), "withdrawn on edit (supersede)");
}
```

   (Verified core fn names: `retire_memory` `log.rs:4824`, `unretire(retired_event_id)` `log.rs:4847` (the note
   reversal — NOT `unretire_memory`), `supersede_note`; the daemon wraps them at `mod.rs:785`/`:798`/`:758`.)

2. Run → FAIL: `cargo test -p bossclaw-core pending_conflict_proposals_project_and_gc_withdraw`
   Expected: `no method named pending_conflict_proposals`.

3. Implement.
   (a) Public row (beside `CurrentNote` `:415`):

```rust
/// One OPEN conflict proposal for the read surface (spec §3.5). Both refs are still current
/// (withdrawn proposals are absent). Ungated (portable data type). `#![deny(missing_docs)]`
/// requires a `///` on every pub field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictProposalRow {
    /// The `conflict_proposal` event id.
    pub id: String,
    /// The OLDER side (by ingest ts), a typed ref.
    pub a_ref: crate::index::ConflictRef,
    /// The NEWER side (by ingest ts), a typed ref.
    pub b_ref: crate::index::ConflictRef,
    /// Advisory winner label ("newer"/"older"/"unclear") — the engine resolves by ts.
    pub winner_hint: String,
    /// Coarse confidence band ("high"/"med") — the raw numeric confidence is never persisted.
    pub confidence_band: String,
    /// The CONTENT-FREE templated reason (`conflict::templated_why`); never memory text.
    pub why: String,
    /// Wall-clock instant detection recorded this proposal (Unix seconds).
    pub detected_at: i64,
}
```

   (b) Projection (after `open_conflict_proposals`, Task 6). `#[cfg(unix)]` (consumes the unix open-set fold):

```rust
    /// Every OPEN conflict proposal (both refs current), oldest first — the read behind a later
    /// App-only `ListConflicts` (spec §3.5). GC is inherent: `open_conflict_proposals` already
    /// drops any proposal whose referenced memory was retired/deleted/edited (I-gc). Fold-derived,
    /// so it survives restart with no cursor. `#[cfg(unix)]`.
    #[cfg(unix)]
    pub fn pending_conflict_proposals(&self) -> Result<Vec<ConflictProposalRow>, BossclawError> {
        Ok(self
            .open_conflict_proposals()?
            .into_iter()
            .map(|p| ConflictProposalRow {
                id: p.id,
                a_ref: p.a_ref,
                b_ref: p.b_ref,
                winner_hint: p.winner_hint,
                confidence_band: p.confidence_band,
                why: p.why,
                detected_at: p.detected_at,
            })
            .collect())
    }
```

   (c) `lib.rs` — add `ConflictProposalRow` to the `pub use log::{...}` block.

4. Run → PASS: `cargo test -p bossclaw-core pending_conflict_proposals_project_and_gc_withdraw`

5. Commit: `feat(rung3-p2): pending_conflict_proposals projection + fold-derived GC withdrawal`

---

## Task 8 — Pure `decide_conflict_sweep` candidate-finder

Design §3.4, §6.3, invariants I4/I9. The pure, hermetic decision core (mirrors `sweeper::decide_sweep`): given
a subject ref + its retrieved neighbour list + the sim floor + exclusion sets, emit the unordered candidate
pairs to judge. No ANN, no clock, no core — the ANN layer is STUBBED by handing in canned neighbours, so the
sweep is deterministic under HNSW rank non-determinism (§6.3). Caps per-subject pairs so a single subject is
always judgeable within one full budget (no permanent cursor stall) and a near-duplicate flood cannot blow up
judge calls (§7).

**Files**
- Modify: `crates/bossclaw-core/src/conflict.rs` — `FinderInput` + `decide_conflict_sweep`. Uses `ConflictRef`
  (`crate::index`), the Phase-2 constants (Task 5).
- Test: `crates/bossclaw-core/src/conflict.rs` `mod tests`.

**Steps**

1. Write the failing test:

```rust
    #[test]
    fn decide_conflict_sweep_gates_excludes_caps_and_orders() {
        use crate::index::ConflictRef;
        use std::collections::HashSet;
        let subj = ConflictRef::Note { event_id: "x".into() };
        let near = ConflictRef::Note { event_id: "near".into() };   // sim 0.90 (dist 0.10)
        let far = ConflictRef::Passage { session_id: "s".into(), passage_id: 0 }; // sim 0.50 → gated out
        // dist = 1 - sim.
        let neighbors = vec![
            (subj.clone(), 0.00_f32),  // self → excluded
            (near.clone(), 0.10_f32),  // sim 0.90 → kept
            (far.clone(), 0.50_f32),   // sim 0.50 < 0.82 → gated
        ];
        let excluded: HashSet<String> = [subj.pair_key()].into_iter().collect(); // self-exclusion
        let empty: HashSet<String> = HashSet::new();
        let pairs = decide_conflict_sweep(&FinderInput {
            subject: &subj,
            neighbors: &neighbors,
            sim_min: CANDIDATE_SIM_MIN,
            excluded_refs: &excluded,
            open_pairs: &empty,
            max_pairs: MAX_CANDIDATE_PAIRS_PER_SUBJECT,
        });
        assert_eq!(pairs, vec![(subj.clone(), near.clone())], "only the above-floor non-self neighbour");

        // Open-pair exclusion: mark (subj, near) already open → dropped.
        let open: HashSet<String> = [{
            let (ka, kb) = (subj.pair_key(), near.pair_key());
            if ka <= kb { format!("{ka}\u{1e}{kb}") } else { format!("{kb}\u{1e}{ka}") }
        }]
        .into_iter()
        .collect();
        assert!(decide_conflict_sweep(&FinderInput {
            subject: &subj, neighbors: &neighbors, sim_min: CANDIDATE_SIM_MIN,
            excluded_refs: &excluded, open_pairs: &open, max_pairs: MAX_CANDIDATE_PAIRS_PER_SUBJECT,
        }).is_empty(), "already-open pair excluded (idempotency pre-filter)");

        // Near-duplicate flood: 50 above-floor neighbours cap to max_pairs, highest-sim first.
        let flood: Vec<(ConflictRef, f32)> = (0..50)
            .map(|i| (ConflictRef::Note { event_id: format!("d{i}") }, 0.01_f32 + (i as f32) * 0.001))
            .collect();
        let capped = decide_conflict_sweep(&FinderInput {
            subject: &subj, neighbors: &flood, sim_min: CANDIDATE_SIM_MIN,
            excluded_refs: &excluded, open_pairs: &empty, max_pairs: MAX_CANDIDATE_PAIRS_PER_SUBJECT,
        });
        assert_eq!(capped.len(), MAX_CANDIDATE_PAIRS_PER_SUBJECT, "flood capped to the per-subject max");
        assert_eq!(capped[0].1, ConflictRef::Note { event_id: "d0".into() }, "highest-sim (lowest dist) first");
    }
```

2. Run → FAIL: `cargo test -p bossclaw-core decide_conflict_sweep_gates_excludes_caps_and_orders`
   Expected: `cannot find type FinderInput`.

3. Implement (in `conflict.rs`):

```rust
/// The hermetic input to [`decide_conflict_sweep`]: a subject, its already-retrieved neighbours
/// (`(ref, cosine_distance)`), the similarity floor, and the exclusion sets. NO ANN / clock / log
/// — the caller supplies neighbours (stubbable), so the decision is deterministic.
pub struct FinderInput<'a> {
    /// The memory being searched for conflicts.
    pub subject: &'a crate::index::ConflictRef,
    /// `(neighbour_ref, cosine_distance)` from `conflict_search_refs`. `sim = 1.0 - distance`.
    pub neighbors: &'a [(crate::index::ConflictRef, f32)],
    /// Cosine-similarity floor a neighbour must clear ([`CANDIDATE_SIM_MIN`]).
    pub sim_min: f32,
    /// `pair_key`s of refs to skip entirely: the subject itself, plus (Phase 3) resolution-excluded
    /// refs. In Phase 2 this is just `{subject.pair_key()}` (superseded/retired refs are already
    /// absent from the freshly-rebuilt index, so they never appear as neighbours).
    pub excluded_refs: &'a std::collections::HashSet<String>,
    /// Unordered pair keys already OPEN — pre-filtered so the judge is never spent on a duplicate.
    pub open_pairs: &'a std::collections::HashSet<String>,
    /// Max pairs kept for this subject ([`MAX_CANDIDATE_PAIRS_PER_SUBJECT`]).
    pub max_pairs: usize,
}

/// Pure candidate-finder (spec §3.4): the unordered `(subject, neighbour)` pairs worth judging,
/// highest-similarity first, capped at `max_pairs`. Excludes: sub-floor neighbours; the subject
/// itself / any `excluded_refs`; and pairs already OPEN (`open_pairs`). Deterministic; no side
/// effects. Sublinear by construction (operates on a top-k neighbour list).
pub fn decide_conflict_sweep(
    input: &FinderInput,
) -> Vec<(crate::index::ConflictRef, crate::index::ConflictRef)> {
    let unordered_key = |a: &crate::index::ConflictRef, b: &crate::index::ConflictRef| -> String {
        let (ka, kb) = (a.pair_key(), b.pair_key());
        if ka <= kb { format!("{ka}\u{1e}{kb}") } else { format!("{kb}\u{1e}{ka}") }
    };
    let mut scored: Vec<(f32, &crate::index::ConflictRef)> = input
        .neighbors
        .iter()
        .filter_map(|(r, dist)| {
            let sim = 1.0 - *dist;
            if sim < input.sim_min {
                return None; // below the similarity floor
            }
            if input.excluded_refs.contains(&r.pair_key()) {
                return None; // self / resolution-excluded
            }
            if input.open_pairs.contains(&unordered_key(input.subject, r)) {
                return None; // already open (idempotency pre-filter)
            }
            Some((sim, r))
        })
        .collect();
    // Highest similarity first; stable tie-break on the ref's pair_key for determinism.
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.pair_key().cmp(&b.1.pair_key()))
    });
    // Dedup by unordered pair key (a neighbour can appear once), then cap.
    let mut seen = std::collections::HashSet::new();
    scored
        .into_iter()
        .filter(|(_, r)| seen.insert(unordered_key(input.subject, r)))
        .take(input.max_pairs)
        .map(|(_, r)| (input.subject.clone(), r.clone()))
        .collect()
}
```

4. Run → PASS: `cargo test -p bossclaw-core decide_conflict_sweep_gates_excludes_caps_and_orders`

5. Commit: `feat(rung3-p2): pure decide_conflict_sweep candidate-finder (hermetic, capped)`

---

## Task 9 — Shared cloud-consent pre-gate helper + `evolve_once` refactor

Design §3.5 (I2). Factor `evolve_once`'s cloud-consent pre-gate into one `EngineHandle::cloud_consent_ok` so the
conflict sweep reuses the EXACT signed-consent barrier and can never egress tainted content without consent.
`evolve_once` behavior is unchanged (regression-guarded).

**Files**
- Modify: `crates/bossclawd/src/engine/mod.rs` — add `cloud_consent_ok` (near `reasoner_ready_or_false` `:1420`);
  rewrite the `evolve_once` pre-gate (`:926-934`) to call it.
- Test: `crates/bossclawd/src/engine/mod.rs` `mod tests` (there is an existing evolve cloud-gate test — extend
  or add beside it).

**Steps**

1. Write the failing test (mirror the existing "cloud-not-ready → evolve errors" test shape in that module):

```rust
    #[tokio::test]
    async fn cloud_consent_ok_is_true_for_local_and_gates_unready_cloud() {
        let dir = tempfile::tempdir().unwrap();
        let vault = TestVault::new(); // TestVault::new() already returns Arc<TestVault> (mod.rs:1830)
        let h = EngineHandle::new(
            vault,
            dir.path().to_path_buf(),
            std::sync::Arc::new(embed::MockEmbedderProvider::new(8)),
            std::sync::Arc::new(crate::engine::reason::MockReasonerProvider::new("m")),
        );
        // Default config is Local → consent is trivially OK (egresses nothing).
        assert!(h.cloud_consent_ok(true).await, "local mode is always consent-ok");
    }
```

2. Run → FAIL: `cargo test -p bossclawd cloud_consent_ok_is_true_for_local_and_gates_unready_cloud`
   Expected: `no method named cloud_consent_ok`.

3. Implement.
   (a) Add the helper (after `reasoner_ready_or_false` `:1435`):

```rust
    /// The shared cloud-egress consent barrier (spec I2). `true` when the reasoner may run WITHOUT
    /// egressing tainted content without consent: Local mode is always OK (egresses nothing); Cloud
    /// mode requires a signed consent record matching the current config + vault key
    /// (`reasoner_ready_or_false`, fail-closed). BOTH `evolve_once` and the conflict sweep gate on
    /// this before building/running the reasoner.
    pub async fn cloud_consent_ok(&self, onboarded: bool) -> bool {
        if matches!(
            self.reasoner_config_or_default(onboarded).await.mode,
            crate::engine::reason::ReasonerMode::Cloud
        ) {
            self.reasoner_ready_or_false(onboarded).await
        } else {
            true
        }
    }
```

   (b) Replace the `evolve_once` pre-gate (`:926-934`) with:

```rust
        // Consent chokepoint for BOTH the scheduler AND manual `engine_evolve_now` (R1/R5/R8),
        // shared with the conflict sweep (I2). Placed BEFORE the reasoner is built (and any
        // spawn_blocking/network), so a cloud-not-ready tick constructs no reasoner and egresses
        // nothing.
        if !self.cloud_consent_ok(onboarded).await {
            return Err(EngineOpError::Reasoner(
                "cloud reasoner not ready — signed consent or provider key missing".to_string(),
            ));
        }
```

4. Run → PASS (helper + no evolve regression):
   `cargo test -p bossclawd cloud_consent_ok_is_true_for_local_and_gates_unready_cloud`
   then the full evolve suite: `cargo test -p bossclawd evolve`

5. Commit: `refactor(rung3-p2): extract cloud_consent_ok shared pre-gate (evolve behavior unchanged)`

---

## Task 10 — Core orchestration `EventLog::detect_conflicts_once`

Design §3.3–§3.5, §6.3–§6.5, invariants I1/I3/I4/I6/I9. The heart: one cycle = gate → cursor → dirty-check →
rebuild → **subject-by-subject** find → judge (budgeted) → emit (idempotent, ceilinged, CONTENT-FREE `why`) →
advance the `(seq, subject_offset)` cursor past each FULLY-judged subject. **Subject-by-subject (not whole-seq-
group) is the anti-stall fix:** a capture is one `seq` but N passage subjects; because each subject's pairs are
capped at `MAX_CANDIDATE_PAIRS_PER_SUBJECT == CONFLICT_JUDGE_PER_SWEEP`, a fresh full budget always fits ≥1
subject, so detection always advances ≥1 subject/cycle and never stalls (nothing dropped). `#[cfg(unix)]`
(calls the `#[cfg(unix)]` append/idempotency family). Takes the reasoner + a `passage_text` closure (the daemon
supplies passage text; note text comes from core) so the whole method is hermetically testable with
`MockEmbedder` + `ScriptedReasoner`.

**Files**
- Modify: `crates/bossclaw-core/src/log.rs` — `ConflictDetectReport` (ungated, beside `CurrentNote` `:415`);
  `#[cfg(unix)] detect_conflicts_once`. Uses everything from Tasks 2/3/5/6/8 + `judge_pair`/`Verdict`/`Winner`
  (`conflict.rs:131/30/16`), `event_by_id` (`:1025`), `embed_one` (`:8122`), `session_passages_for_model`
  (`:5855`), `session_passage_count` (`:5913`), `fold_sessions` (`:8324`).
- Modify: `crates/bossclaw-core/src/lib.rs` — re-export `ConflictDetectReport` (ungated data type).
- Test: `crates/bossclaw-core/src/log.rs` `mod tests` (`#[cfg(unix)]` tests; use `ScriptedReasoner` +
  `build_conflict_prompt` + **`MockEmbedder::new(64)`** + near-duplicate fixtures — the critic computed the
  dim=8 marquee pair at ≈0.816 < 0.82; dim=64 + one-token-apart strings clear the floor).

**Steps**

1. Write the failing test:

```rust
/// Rung-3 Phase-2 (§3.3–§3.5, I3/I4/I6/I7): one cycle over two contradicting notes emits exactly
/// one proposal with a CONTENT-FREE `why` (the model's raw rationale never persists); a second
/// cycle with nothing new does ZERO judge calls (proven with a PanicReasoner); gate-off is a no-op.
/// `#[cfg(unix)]` (drives the append family).
#[cfg(unix)]
#[test]
fn detect_conflicts_once_proposes_then_is_incremental_and_gated() {
    use crate::conflict::build_conflict_prompt;
    use crate::reason::{Reasoner, ScriptedReasoner};
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(64); // dim=64: the dim=8 marquee pair falls below CANDIDATE_SIM_MIN
    let log = open_log(dir.path());

    // Two near-duplicate notes (one token apart) so the finder clears the similarity floor.
    let older_text = "the default deploy target is vercel";
    let newer_text = "the default deploy target is fly";
    let _older = log.remember(&emb, older_text).unwrap();
    let _newer = log.remember(&emb, newer_text).unwrap();

    // Script the pair as a contradiction whose model `why` embeds a memory fragment SENTINEL — the
    // stored `why` must NOT contain it (I7: persisted why is a content-free template).
    let reasoner = ScriptedReasoner::new("test").with_response(
        crate::conflict::CONFLICT_SYSTEM,
        &build_conflict_prompt(older_text, newer_text),
        serde_json::json!({ "contradicts": true, "winner": "newer", "confidence": 92, "why": "SENTINEL_LEAK vercel vs fly verbatim" }),
    );
    let no_passages = |_sid: &str, _pid: usize| -> Option<String> { None };
    let empty = std::collections::HashSet::new();

    // Gate OFF → skipped, no proposal (I3). PanicReasoner proves zero model calls.
    struct PanicReasoner;
    impl Reasoner for PanicReasoner {
        fn complete_json(&self, _s: &str, _p: &str, _sc: &serde_json::Value) -> Result<serde_json::Value, BossclawError> {
            panic!("reasoner must not be called");
        }
        fn model_id(&self) -> &str { "panic" }
    }
    let off = log.detect_conflicts_once(&emb, &PanicReasoner, &no_passages, &empty, 100).unwrap();
    assert!(off.skipped_disabled && off.proposed == 0, "gate off is a no-op with no model call");

    // Enable + run: exactly one proposal, with a content-free `why`.
    log.set_conflict_detect_enabled(true).unwrap();
    let r1 = log.detect_conflicts_once(&emb, &reasoner, &no_passages, &empty, 100).unwrap();
    assert_eq!(r1.proposed, 1, "one contradiction proposed");
    let pending = log.pending_conflict_proposals().unwrap();
    assert_eq!(pending.len(), 1);
    assert!(!pending[0].why.contains("SENTINEL_LEAK"), "I7: model's raw why never persisted");
    assert!(pending[0].why.contains("confidence"), "why is the content-free template");

    // Second cycle, nothing new since the cursor → ZERO judge calls (PanicReasoner must not fire).
    let r2 = log.detect_conflicts_once(&emb, &PanicReasoner, &no_passages, &empty, 100).unwrap();
    assert_eq!(r2.judged, 0, "no new subjects → no judging (cursor incrementality, I4)");
    assert_eq!(r2.proposed, 0);
    assert_eq!(log.pending_conflict_proposals().unwrap().len(), 1, "still exactly one (idempotent)");
}
```

   (Note: the two notes must clear `CANDIDATE_SIM_MIN` under the REAL `MockEmbedder::new(64)`. If the one-token-
   apart fixture does not, add MORE shared tokens — do NOT change `CANDIDATE_SIM_MIN`. Verify empirically at
   red→green; adjust the fixture text, never the constant.)

2. Run → FAIL: `cargo test -p bossclaw-core detect_conflicts_once_proposes_then_is_incremental_and_gated`
   Expected: `no method named detect_conflicts_once`.

3. Implement.
   (a) Report struct (beside `CurrentNote`):

```rust
/// What one [`EventLog::detect_conflicts_once`] cycle did (spec §3.3). All-zero + `skipped_disabled`
/// when the flag is off (I3).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ConflictDetectReport {
    /// The flag was CLOSED — no scan, no model, no proposals (I3).
    pub skipped_disabled: bool,
    /// New subjects examined this cycle (0 → dirty-gate short-circuit, no rebuild).
    pub scanned_subjects: usize,
    /// Judge calls made (bounded by [`crate::conflict::CONFLICT_JUDGE_PER_SWEEP`]).
    pub judged: usize,
    /// Proposals emitted.
    pub proposed: usize,
    /// Pairs the judge declined (`Ok(None)`) — counted, never proposed.
    pub dropped: usize,
    /// Reasoner transport/decode failures (`Err`) — the cycle stops + retries next time (I6).
    pub reasoner_errors: usize,
    /// The per-cycle judge budget was hit (backlog drips to the next cycle).
    pub budget_hit: bool,
    /// The open-proposal ceiling was hit (stop proposing; surface one quiet count).
    pub ceiling_hit: bool,
}
```

   (b) The method (place after `conflict_search_refs`). `#[cfg(unix)]` (calls the append/idempotency family).
   Full code:

```rust
    /// Run ONE conflict-detection cycle (spec §3.3). Gated on the owner flag (I3). Advances the
    /// `(seq, subject_offset)` cursor past each FULLY-judged subject, so a budget-truncated or
    /// reasoner-interrupted cycle resumes exactly (I6). `passage_text(session_id, passage_id)`
    /// supplies a passage's real text (the daemon reads the `.md`); note text comes from core.
    /// `resolution_excluded_refs` is EMPTY in Phase 2 (Phase 3 fills it). `detected_at` stamps each
    /// proposal. `#[cfg(unix)]` (uses `append_conflict_proposal` / the idempotency fold).
    #[cfg(unix)]
    pub fn detect_conflicts_once(
        &self,
        embedder: &dyn Embedder,
        reasoner: &dyn crate::reason::Reasoner,
        passage_text: &dyn Fn(&str, usize) -> Option<String>,
        resolution_excluded_refs: &std::collections::HashSet<String>,
        detected_at: i64,
    ) -> Result<ConflictDetectReport, BossclawError> {
        use crate::conflict::{
            bound_judge_text, confidence_band, decide_conflict_sweep, templated_why, winner_str,
            FinderInput, CANDIDATE_SIM_MIN, CONFLICT_JUDGE_PER_SWEEP, CONFLICT_OPEN_CEILING,
            CONFLICT_SCAN_BOUND, CONFLICT_SEARCH_K, MAX_CANDIDATE_PAIRS_PER_SUBJECT,
        };
        use crate::index::ConflictRef;
        let mut report = ConflictDetectReport::default();

        // (1) Gate FIRST — no scan, no rebuild, no model when CLOSED (I3).
        if !self.conflict_detect_enabled()? {
            report.skipped_disabled = true;
            return Ok(report);
        }

        // (2) Dirty-gate: nothing new since the cursor → no rebuild, no model (this is what makes a
        // quiet second cycle zero-cost, I4).
        let (cursor_seq, cursor_off) = self.conflict_cursor()?;
        let subjects =
            self.unprocessed_conflict_subjects_since(cursor_seq, cursor_off, CONFLICT_SCAN_BOUND)?;
        if subjects.is_empty() {
            return Ok(report);
        }
        report.scanned_subjects = subjects.len();

        // (3) Rebuild the unified fights index so BOTH sides of any conflict are present + current.
        self.rebuild_conflict_index(embedder)?;

        // (4) One-shot lookups: the passage-vector map (query vectors for Passage subjects), the
        //     session fold (session_id <-> head event id, for ts + lineage), and the OPEN pair set.
        let mut passage_vec: std::collections::HashMap<(String, usize), Vec<f32>> =
            std::collections::HashMap::new();
        let fold = fold_sessions(&self.session_events_ordered()?);
        let head_of: std::collections::HashMap<String, String> = fold
            .current
            .iter()
            .map(|cs| (cs.session_id.clone(), cs.event_id.clone()))
            .collect();
        let session_of_event: std::collections::HashMap<String, String> = fold
            .current
            .iter()
            .map(|cs| (cs.event_id.clone(), cs.session_id.clone()))
            .collect();
        for (event_id, ix, vec) in self.session_passages_for_model(embedder.model_id())? {
            if let Some(sid) = session_of_event.get(&event_id) {
                passage_vec.insert((sid.clone(), ix), vec);
            }
        }
        let opens = self.open_conflict_proposals()?;
        let mut open_pairs: std::collections::HashSet<String> =
            opens.iter().map(|p| Self::conflict_pair_key(&p.a_ref, &p.b_ref)).collect();
        let mut open_count = opens.len();

        // Text / lineage / ts / kind resolvers for a ref (notes from core; passages via the closure).
        let ref_text = |r: &ConflictRef| -> Option<String> {
            match r {
                ConflictRef::Note { event_id } => self
                    .event_by_id(event_id)
                    .ok()
                    .flatten()
                    .and_then(|e| e.content.get("text").and_then(|t| t.as_str()).map(str::to_string))
                    .map(|t| bound_judge_text(&t).to_string()),
                ConflictRef::Passage { session_id, passage_id } => {
                    passage_text(session_id, *passage_id).map(|t| bound_judge_text(&t).to_string())
                }
            }
        };
        let ref_source_event = |r: &ConflictRef| -> Option<String> {
            match r {
                ConflictRef::Note { event_id } => Some(event_id.clone()),
                ConflictRef::Passage { session_id, .. } => head_of.get(session_id).cloned(),
            }
        };
        let ref_ts = |r: &ConflictRef| -> i64 {
            ref_source_event(r)
                .and_then(|id| self.event_by_id(&id).ok().flatten())
                .and_then(|e| DateTime::parse_from_rfc3339(&e.ts).ok().map(|d| d.timestamp()))
                .unwrap_or(0)
        };
        let ref_kind = |r: &ConflictRef| -> &'static str {
            match r {
                ConflictRef::Note { .. } => "note",
                ConflictRef::Passage { .. } => "passage",
            }
        };

        // (5) SUBJECT-BY-SUBJECT (NOT whole-seq-group — the anti-stall fix): each subject's pairs
        //     are capped at MAX_CANDIDATE_PAIRS_PER_SUBJECT == CONFLICT_JUDGE_PER_SWEEP, so a fresh
        //     full budget ALWAYS fits >=1 subject → we advance >=1 subject/cycle → never stall. The
        //     cursor advances to (seq, within+1) after EACH fully-judged subject, so a crash /
        //     reasoner-stop mid-cycle resumes exactly (I6).
        let mut budget_left = CONFLICT_JUDGE_PER_SWEEP;
        for cs in &subjects {
            let subject = &cs.subject;
            // Query vector: a note embeds its body; a passage reuses its stored vector. A subject
            // with no usable query vector has no candidates — mark it done and move on.
            let qv = match subject {
                ConflictRef::Note { event_id } => {
                    match self.event_by_id(event_id)?.and_then(|e| {
                        e.content.get("text").and_then(|t| t.as_str()).map(str::to_string)
                    }) {
                        Some(t) if !t.trim().is_empty() => embed_one(embedder, &t)?,
                        _ => {
                            self.set_conflict_cursor(cs.seq, cs.within_seq_id + 1)?;
                            continue;
                        }
                    }
                }
                ConflictRef::Passage { session_id, passage_id } => {
                    match passage_vec.get(&(session_id.clone(), *passage_id)) {
                        Some(v) => v.clone(),
                        None => {
                            self.set_conflict_cursor(cs.seq, cs.within_seq_id + 1)?;
                            continue;
                        }
                    }
                }
            };
            let mut excluded_refs = resolution_excluded_refs.clone();
            excluded_refs.insert(subject.pair_key());
            let neighbors = self.conflict_search_refs(&qv, CONFLICT_SEARCH_K);
            let pairs = decide_conflict_sweep(&FinderInput {
                subject,
                neighbors: &neighbors,
                sim_min: CANDIDATE_SIM_MIN,
                excluded_refs: &excluded_refs,
                open_pairs: &open_pairs,
                max_pairs: MAX_CANDIDATE_PAIRS_PER_SUBJECT,
            });
            // Budget: if THIS subject's pairs don't fit the remaining budget, stop — the cursor
            // stays AT this subject (we don't advance), so it resumes with a full budget next cycle.
            // pairs.len() <= full budget, so the FIRST subject always fits: no permanent stall.
            if pairs.len() > budget_left {
                report.budget_hit = true;
                break;
            }
            let mut reasoner_failed = false;
            for (a, b) in pairs {
                // Order older/newer by ingest ts (deterministic; spec §4d).
                let (older, newer) = if ref_ts(&a) <= ref_ts(&b) { (a, b) } else { (b, a) };
                let (Some(ot), Some(nt)) = (ref_text(&older), ref_text(&newer)) else {
                    report.dropped += 1; // a side's text is unavailable — cannot judge
                    continue;
                };
                report.judged += 1;
                budget_left -= 1;
                match crate::conflict::judge_pair(reasoner, &ot, &nt) {
                    Ok(Some(v)) => {
                        // The model's raw `v.why` is DISCARDED (never persisted — I7). Debug-only.
                        log::debug!("conflict why (ephemeral, unsigned): {}", v.why);
                        let pk = Self::conflict_pair_key(&older, &newer);
                        if open_pairs.contains(&pk) {
                            continue; // already open (within-cycle or persisted)
                        }
                        if open_count >= CONFLICT_OPEN_CEILING {
                            report.ceiling_hit = true;
                            continue; // stop proposing; the quiet count is pending_conflict_proposals().len()
                        }
                        if self.is_conflict_proposal_suppressed(&older, &newer)? {
                            continue;
                        }
                        // I7: the persisted `why` is a CONTENT-FREE template (winner + band + kinds).
                        let why = templated_why(
                            winner_str(v.winner),
                            confidence_band(v.confidence),
                            ref_kind(&older),
                            ref_kind(&newer),
                        );
                        let sources: Vec<String> = [ref_source_event(&older), ref_source_event(&newer)]
                            .into_iter()
                            .flatten()
                            .collect();
                        self.append_conflict_proposal(
                            &older,
                            &newer,
                            winner_str(v.winner),
                            confidence_band(v.confidence),
                            &why,
                            detected_at,
                            &sources,
                        )?;
                        open_pairs.insert(pk);
                        open_count += 1;
                        report.proposed += 1;
                    }
                    Ok(None) => report.dropped += 1,
                    Err(_) => {
                        // Reasoner unavailable → no-op the rest of the cycle; do NOT advance past
                        // this subject (I6 — resume it next cycle; idempotency prevents dups).
                        report.reasoner_errors += 1;
                        reasoner_failed = true;
                        break;
                    }
                }
            }
            if reasoner_failed {
                break; // leave the cursor AT this subject
            }
            // This subject is fully judged → advance the cursor past it.
            self.set_conflict_cursor(cs.seq, cs.within_seq_id + 1)?;
        }
        Ok(report)
    }
```

   (c) `lib.rs` — re-export `ConflictDetectReport` (ungated data type).

4. Run → PASS: `cargo test -p bossclaw-core detect_conflicts_once_proposes_then_is_incremental_and_gated`

5. **Write the multi-passage NO-STALL test (Major #1 — this MUST fail against a whole-seq-group deferral and
   pass with the subject-by-subject loop).** All Task-10 tests are `#[cfg(unix)]` and use `MockEmbedder::new(64)`:

```rust
/// Rung-3 Phase-2 (§3.3, I4 — the multi-passage NO-STALL fix): a single capture with many near-
/// duplicate passages produces MORE candidate pairs (C(5,2)=10) than one cycle's budget (8).
/// Detection must advance SUBJECT-BY-SUBJECT across cycles, emit passage-pair proposals, judge
/// EVERY passage subject over enough cycles (nothing dropped), and be restart-safe. This FAILS
/// against the old whole-seq-group deferral (which would defer the whole capture forever → 0
/// proposals, cursor never advancing).
#[cfg(unix)]
#[test]
fn detect_conflicts_once_advances_multi_passage_capture_without_stall() {
    use crate::conflict::{build_conflict_prompt, CONFLICT_SYSTEM};
    use crate::reason::ScriptedReasoner;
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(64);
    let log = open_log(dir.path());
    log.set_conflict_detect_enabled(true).unwrap();

    // One capture, 5 near-duplicate passages (one token apart) → all C(5,2)=10 pairs clear the floor.
    let chunks: Vec<String> = ["alpha", "bravo", "charlie", "delta", "echo"]
        .iter()
        .map(|w| format!("config {w} sets the deploy target to vercel"))
        .collect();
    let ev = log.capture_session(&emb, &session_meta("s1", "aa")).unwrap();
    log.store_session_passages(&emb, &ev, &chunks).unwrap();

    // Passage-text resolver (the daemon's job): (s1, pid) -> chunk[pid].
    let chunks2 = chunks.clone();
    let passage_text = move |sid: &str, pid: usize| -> Option<String> {
        if sid == "s1" { chunks2.get(pid).cloned() } else { None }
    };
    // Script every unordered pair {i<j} (older = passage i, newer = passage j; the capture ts is a
    // tie, so the finder's (subject i, neighbour j) order holds and i<j always resolves as older=i).
    let mut reasoner = ScriptedReasoner::new("test");
    for i in 0..chunks.len() {
        for j in (i + 1)..chunks.len() {
            reasoner = reasoner.with_response(
                CONFLICT_SYSTEM,
                &build_conflict_prompt(&chunks[i], &chunks[j]),
                serde_json::json!({ "contradicts": true, "winner": "unclear", "confidence": 90, "why": "same target" }),
            );
        }
    }
    let empty = std::collections::HashSet::new();

    // Cycle 1: budget-bounded, but progress IS made and the cursor advances (would be 0/no-move under
    // the old stall).
    let before = log.conflict_cursor().unwrap();
    let r1 = log.detect_conflicts_once(&emb, &reasoner, &passage_text, &empty, 1).unwrap();
    assert!(r1.judged <= 8, "cycle judging is budget-bounded ({})", r1.judged);
    assert!(r1.proposed >= 1, "passage-pair proposals ARE emitted (0 under the old whole-group stall)");
    assert_ne!(log.conflict_cursor().unwrap(), before, "cursor advanced subject-by-subject");

    // Drain the backlog — MUST terminate (no stall).
    for _ in 0..10 {
        if log.pending_conflict_proposals().unwrap().len() == 10 {
            break;
        }
        log.detect_conflicts_once(&emb, &reasoner, &passage_text, &empty, 1).unwrap();
    }
    assert_eq!(
        log.pending_conflict_proposals().unwrap().len(),
        10,
        "every passage subject judged over enough cycles — NOTHING dropped"
    );

    // Restart-safe: reopen and run again — cursor persisted, no new work, no dup proposals.
    drop(log);
    let log = open_log(dir.path());
    let r = log.detect_conflicts_once(&emb, &reasoner, &passage_text, &empty, 1).unwrap();
    assert_eq!(r.judged, 0, "restart: cursor persisted, nothing re-judged");
    assert_eq!(log.pending_conflict_proposals().unwrap().len(), 10, "no duplicates after restart");
}
```

   Run → this FAILS if Task 10 still uses whole-group deferral; PASSES with subject-by-subject:
   `cargo test -p bossclaw-core detect_conflicts_once_advances_multi_passage_capture_without_stall`

6. Add two more `#[cfg(unix)]` tests (write → red → green; full code, dim=64, near-dup fixtures):
   - `detect_conflicts_once_caps_judges_at_budget`: seed ≥ 9 near-duplicate NOTES (each a distinct subject/seq)
     that pairwise clear the floor; script every pair; assert `report.judged <= CONFLICT_JUDGE_PER_SWEEP` in one
     cycle and `report.budget_hit`; assert a follow-up cycle keeps making progress (cursor advanced ≥1 subject).
   - `detect_conflicts_once_reasoner_error_is_noop_and_resumable`: two contradicting notes, a reasoner with NO
     canned response for the pair → `judge_pair` returns `Err`; assert `reasoner_errors >= 1`, `proposed == 0`,
     and that a LATER cycle with a correctly-scripted reasoner proposes (cursor did NOT skip the subject).

7. Commit: `feat(rung3-p2): EventLog::detect_conflicts_once cycle (subject-by-subject, budgeted, no-stall, fail-safe)`

---

## Task 11 — Engine async wrapper + daemon passage-text resolver

Design §3.3, §3.5. Wraps the core cycle in the standard engine pattern (gate → reasoner → `spawn_blocking`),
serialized by a new `conflict_lock`, supplies the daemon passage-text closure (reads the `.md`, re-chunks with
the SAME `chunk_text` capture used, so passage indices match), and records session telemetry (MINOR #6). `mod
engine` is `#[cfg(unix)]`, so these need no per-fn gate.

**Files**
- Modify: `crates/bossclawd/src/capture/store.rs` — `pub(crate) fn session_passage_text` (uses
  `read_capture_markdown` `:99`, `capture_body` `:535`, `bossclaw_core::chunk_text`, `paths::valid_session_id`;
  shares the heal path's 16-MiB `MAX_CAPTURE_MD_BYTES` `:92` truncation caveat).
- Modify: `crates/bossclawd/src/engine/mod.rs` — `conflict_lock: Mutex<()>` + `conflict_tel: Mutex<ConflictTelemetry>`
  fields (beside `evolve_lock` `:265` / `evolve_tel` `:271` + their `new` inits `:305`/`:307`); `ConflictTelemetry`
  struct (beside `EvolveTelemetry` `:244`); `record_conflict_tick`; `conflict_telemetry` read;
  `set_conflict_detect_enabled` wrapper (mirror `set_evolve_enabled` `:1024`); `detect_conflicts_once` async
  wrapper (uses `data_dir()` `:380`, `cloud_consent_ok` Task 9, `ensure_indexed` `:560`,
  `reasoner_provider.reasoner()` `:936`).
- Test: `crates/bossclawd/src/capture/store.rs` `mod tests` (passage-text round-trip);
  `crates/bossclawd/src/engine/mod.rs` `mod tests` (handle built INLINE with `MockEmbedderProvider::new(64)`).

**Steps**

1. Write the failing passage-text test (in `capture/store.rs` `mod tests`; reuse its existing `store_capture`
   round-trip fixtures at `:531`/`:556` for a real `.md`):

```rust
    #[tokio::test]
    async fn session_passage_text_recovers_the_chunk_by_index() {
        // Write a real capture .md, then recover passage[ix] and prove it equals the chunk the
        // capture would have persisted (same chunk_text over the same body).
        let dir = tempfile::tempdir().unwrap();
        // (Reuse this module's store_capture harness to write <data_dir>/sessions/<id>.md; see the
        // existing `capture_body_round_trips_compose_document_body` test for the setup shape.)
        let data_dir = dir.path();
        let body = "First passage about vercel.\n\nSecond passage about postgres.";
        // ... write the .md via the module's store helper with session_id "s1" and this body ...
        let expected = bossclaw_core::chunk_text(body);
        assert_eq!(
            session_passage_text(data_dir, "s1", 0).as_deref(),
            expected.first().map(String::as_str),
            "passage 0 recovers the first chunk"
        );
        // An invalid session id never touches the filesystem.
        assert_eq!(session_passage_text(data_dir, "../etc", 0), None);
    }
```

2. Run → FAIL: `cargo test -p bossclawd session_passage_text_recovers_the_chunk_by_index`
   Expected: `cannot find function session_passage_text`.

3. Implement.
   (a) `capture/store.rs`:

```rust
/// Recover a captured session's passage TEXT by index (Rung-3 Phase-2): the daemon's side of the
/// conflict judge, since core holds only passage vectors. Reads `<data_dir>/sessions/<id>.md`,
/// strips the front-matter (`capture_body`), and re-chunks with the SAME `chunk_text` the capture
/// used, so `passage_id` maps to the identical chunk. `None` on an invalid id (never touches the
/// filesystem), a missing/unreadable `.md`, or an out-of-range index.
///
/// Shares the heal path's pre-existing 16-MiB caveat: `read_capture_markdown` caps at
/// [`MAX_CAPTURE_MD_BYTES`] (16 MiB) — a >16-MiB body re-reads truncated, so its chunk indices
/// would diverge from the original capture's. Narrow (needs a >16-MiB session body) and already
/// accepted in Phase 1's `persist_passages_if_absent` heal-window; a divergent index just yields a
/// wrong/absent snippet, which the judge treats as a dropped pair — never a corrupted proposal.
pub(crate) fn session_passage_text(
    data_dir: &Path,
    session_id: &str,
    passage_id: usize,
) -> Option<String> {
    if !crate::capture::paths::valid_session_id(session_id) {
        return None;
    }
    let md_path = sessions_dir(data_dir).join(format!("{session_id}.md"));
    let full = read_capture_markdown(&md_path).ok()?;
    bossclaw_core::chunk_text(capture_body(&full)).into_iter().nth(passage_id)
}
```

   (b) `engine/mod.rs` — add the field + init, then the wrapper:

```rust
    /// Serializes manual + scheduled conflict-detection cycles (`try_lock` → `Busy("conflict")`).
    /// Mirrors `evolve_lock`.
    conflict_lock: Mutex<()>,
```
```rust
            conflict_lock: Mutex::new(()),
```
```rust
    /// Run ONE conflict-detection cycle (gated, serialized). Gate → `conflict_lock.try_lock()`
    /// (`Busy("conflict")` on overlap) → shared cloud-consent pre-gate (I2) → `ensure_indexed`
    /// (embedder) → build reasoner → `spawn_blocking(log.detect_conflicts_once)` with the daemon
    /// passage-text resolver. Off-by-default is enforced INSIDE `detect_conflicts_once` (flag gate),
    /// so a disabled brain is a no-op even if this is called.
    pub async fn detect_conflicts_once(
        &self,
        onboarded: bool,
        now: i64,
    ) -> Result<bossclaw_core::ConflictDetectReport, EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        let _guard = self.conflict_lock.try_lock().map_err(|_| EngineOpError::Busy("conflict"))?;
        if !self.cloud_consent_ok(onboarded).await {
            return Err(EngineOpError::Reasoner(
                "cloud reasoner not ready — signed consent or provider key missing".to_string(),
            ));
        }
        let Some(data_dir) = self.data_dir().map(|p| p.to_path_buf()) else {
            return Err(EngineOpError::Core("data dir unresolvable".to_string()));
        };
        let embedder = self.ensure_indexed(&log).await?;
        let reasoner = self.reasoner_provider.reasoner()?;
        let result = spawn_blocking(move || -> Result<bossclaw_core::ConflictDetectReport, EngineOpError> {
            let passage_text = |session_id: &str, passage_id: usize| -> Option<String> {
                crate::capture::store::session_passage_text(&data_dir, session_id, passage_id)
            };
            log.detect_conflicts_once(
                &*embedder,
                &*reasoner,
                &passage_text,
                &std::collections::HashSet::new(),
                now,
            )
            .map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?;
        self.record_conflict_tick(&result); // session telemetry (mirrors evolve's record_tick)
        result
    }
```

   (c) Session telemetry (MINOR #6 — mirror the in-memory `evolve_tel: Mutex<EvolveTelemetry>` precedent
   `mod.rs:271`; the same session-scoped shape evolve uses). A durable lifetime count is separately derivable
   from the append-only log (`pending_conflict_proposals().len()` for open; counting `conflict_proposal` events
   for total), so this stays the light in-memory accumulator:

```rust
    // Beside `EvolveTelemetry` (mod.rs:244).
    /// Session-scoped conflict-detection telemetry (mirrors [`EvolveTelemetry`]; in-memory, cleared
    /// on restart — a durable lifetime count is derivable from the append-only `conflict_proposal`
    /// events, so no table is needed).
    #[derive(Debug, Default, Clone)]
    pub struct ConflictTelemetry {
        /// Wall-clock duration of the most recent cycle, ms.
        pub last_cycle_ms: Option<u128>,
        /// Cumulative proposals emitted this session.
        pub proposed_total: usize,
        /// Cumulative pairs the judge declined this session.
        pub dropped_total: usize,
        /// Cumulative reasoner errors this session.
        pub reasoner_errors_total: usize,
    }
```
```rust
    // On EngineHandle (beside `evolve_tel` mod.rs:271) + its `new` init (beside evolve_tel's, :307):
    conflict_tel: std::sync::Mutex<ConflictTelemetry>,
    // ... in `new`:  conflict_tel: std::sync::Mutex::new(ConflictTelemetry::default()),

    /// Accumulate one cycle's outcome into the session telemetry (poison-tolerant, like evolve's).
    fn record_conflict_tick(&self, result: &Result<bossclaw_core::ConflictDetectReport, EngineOpError>) {
        let mut tel = self.conflict_tel.lock().unwrap_or_else(|p| p.into_inner());
        if let Ok(r) = result {
            tel.proposed_total += r.proposed;
            tel.dropped_total += r.dropped;
            tel.reasoner_errors_total += r.reasoner_errors;
        }
    }

    /// A clone of the session conflict telemetry (poison-recovered). Mirrors the evolve telemetry read.
    pub fn conflict_telemetry(&self) -> ConflictTelemetry {
        self.conflict_tel.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
```

   (Set `last_cycle_ms` too if you time the cycle like `evolve_once` does with `Instant::now()`; optional.)
   Re-export `ConflictTelemetry` from the engine module if the daemon/tests read it.

4. Add the thin `set_conflict_detect_enabled` engine wrapper (it does NOT exist yet — verified) mirroring
   `set_evolve_enabled` (`mod.rs:1024`):

```rust
    /// Flip the sticky conflict-detection off-switch. Gated + `spawn_blocking`. Mirrors
    /// [`Self::set_evolve_enabled`].
    pub async fn set_conflict_detect_enabled(
        &self,
        onboarded: bool,
        enabled: bool,
    ) -> Result<(), EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        spawn_blocking(move || {
            log.set_conflict_detect_enabled(enabled).map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }
```

5. Write the failing engine end-to-end test (in `engine/mod.rs` `mod tests`). Build the handle INLINE with
   `MockEmbedderProvider::new(64)` (the shared `new_test_handle_with_reasoner` hardcodes dim=8, below the floor)
   + `MockReasonerProvider::from_reasoner` + a `ScriptedReasoner` keyed on the exact `build_conflict_prompt`
   (see the module's `:3362` scripted pattern). `remember` engine wrapper exists (`:617`):

```rust
    #[tokio::test]
    async fn engine_detect_conflicts_once_emits_a_proposal_when_enabled() {
        use std::sync::Arc;
        let dir = tempfile::tempdir().unwrap();
        let vault = TestVault::new(); // already Arc<TestVault> (mod.rs:1830)
        // Near-duplicate (one token apart) so the finder clears CANDIDATE_SIM_MIN under dim=64.
        let a = "the default deploy target is vercel";
        let b = "the default deploy target is fly";
        let reasoner: Arc<dyn bossclaw_core::Reasoner> =
            Arc::new(bossclaw_core::ScriptedReasoner::new("test").with_response(
                bossclaw_core::conflict::CONFLICT_SYSTEM,
                &bossclaw_core::conflict::build_conflict_prompt(a, b),
                serde_json::json!({ "contradicts": true, "winner": "newer", "confidence": 92, "why": "renamed" }),
            ));
        let h = EngineHandle::new(
            vault,
            dir.path().to_path_buf(),
            Arc::new(embed::MockEmbedderProvider::new(64)),
            Arc::new(crate::engine::reason::MockReasonerProvider::from_reasoner(reasoner)),
        );
        let onboarded = true;
        h.remember(onboarded, a.to_string()).await.unwrap();
        h.remember(onboarded, b.to_string()).await.unwrap();
        h.set_conflict_detect_enabled(onboarded, true).await.unwrap();
        let report = h.detect_conflicts_once(onboarded, 100).await.unwrap();
        assert_eq!(report.proposed, 1, "engine cycle emits one proposal");
        // Session telemetry accumulated the cycle (MINOR #6).
        assert_eq!(h.conflict_telemetry().proposed_total, 1, "telemetry recorded the proposal");
    }
```

6. Run → PASS: `cargo test -p bossclawd session_passage_text_recovers_the_chunk_by_index engine_detect_conflicts_once_emits_a_proposal_when_enabled`

7. Commit: `feat(rung3-p2): engine detect_conflicts_once wrapper + daemon passage-text resolver`

---

## Task 12 — Background conflict sweeper (`conflict::sweeper`) + report + gate/fail-safe

Design §3.3, §5, §6.5/§6.6. The daemon loop that piggybacks the capture cadence (`SWEEP_INTERVAL = 300s`,
`MissedTickBehavior::Skip`) but stays OFF until the owner enables it. Mirrors `capture::sweeper::{run_sweep_once,
spawn}`. Fail-safe: gated-off / reasoner-down / busy → a quiet no-op report; never panics.

**Files**
- Create: `crates/bossclawd/src/conflict/mod.rs` (`pub mod sweeper;`).
- Create: `crates/bossclawd/src/conflict/sweeper.rs` — `ConflictSweepReport`, `run_conflict_sweep_once`, `spawn`.
- Modify: `crates/bossclawd/src/lib.rs` — `#[cfg(unix)] pub mod conflict;` (the sweeper calls `engine`/`identity`,
  both `#[cfg(unix)]`, so the module is unix-gated like `server`/`telemetry`; place it beside those).
- Create (test): `crates/bossclawd/tests/conflict_sweeper.rs` — integration test copying the bare-engine harness
  `hermetic_engine() -> (EngineHandle, TempDir)` from `crates/bossclawd/tests/sweeper.rs:34` (which uses the
  public `bossclawd::server::test_engine(home)` `:981` + `vault::seed_secret_cache_for_test`). The engine's
  in-module `TestVault` is `#[cfg(test)]`-private, so a bare `EngineHandle` for `run_conflict_sweep_once(&engine,
  …)` must come from `test_engine`, not `TestVault`.

**Steps**

1. Write the failing test as an **integration test** `crates/bossclawd/tests/conflict_sweeper.rs` (NOT a unit
   test inside `conflict/sweeper.rs` — the engine's in-module `TestVault` is `#[cfg(test)]`-private). Copy the
   bare-engine harness `hermetic_engine()` verbatim from `crates/bossclawd/tests/sweeper.rs:34` (it seeds the
   vault, writes `identity.json` so the brain is onboarded, and returns `(EngineHandle, TempDir)` via the public
   `bossclawd::server::test_engine`):

```rust
// crates/bossclawd/tests/conflict_sweeper.rs

use bossclawd::engine::EngineHandle;

/// Onboarded, hermetic engine + its data dir — copied from `tests/sweeper.rs::hermetic_engine`.
fn hermetic_engine() -> (EngineHandle, tempfile::TempDir) {
    bossclawd::vault::seed_secret_cache_for_test(Default::default());
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("identity.json"),
        serde_json::json!({
            "did": "did:wba:example.com:tester",
            "name": "Tester",
            "created_at": "2026-07-11T00:00:00+00:00"
        })
        .to_string(),
    )
    .unwrap();
    (bossclawd::server::test_engine(dir.path().to_path_buf()), dir)
}

#[tokio::test]
async fn gated_off_is_a_quiet_noop() {
    let (engine, dir) = hermetic_engine();
    // Detection is default-CLOSED → the sweep is a quiet no-op (gated_off), never a panic.
    let report =
        bossclawd::conflict::sweeper::run_conflict_sweep_once(&engine, dir.path(), 100).await;
    assert!(report.gated_off, "a fresh (disabled) brain gates off");
    assert_eq!(report.proposed, 0);
}
```

2. Run → FAIL: `cargo test -p bossclawd gated_off_is_a_quiet_noop`
   Expected: `unresolved module conflict` / `cannot find function run_conflict_sweep_once`.

3. Implement.
   (a) `crates/bossclawd/src/conflict/mod.rs`:

```rust
//! Rung-3 Phase-2 conflict DETECTION — the background sweep that notices contradictions between
//! the brain's own memories and records signed `conflict_proposal`s. Off-by-default; never blocks
//! recall/writes; emits records only (no UI, no mutation). Sibling of `crate::capture`.

pub mod sweeper;
```

   (b) `crates/bossclawd/src/conflict/sweeper.rs`:

```rust
//! The conflict-detection sweep loop. Mirrors `crate::capture::sweeper`: a pure gate + a thin
//! tokio loop reading the wall clock at the boundary. All heavy work (find → judge → emit) is one
//! `EngineHandle::detect_conflicts_once` call (itself gated + serialized + `spawn_blocking`).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use crate::capture::sweeper::SWEEP_INTERVAL; // piggyback the capture cadence (300s)
use crate::engine::EngineHandle;

/// What one [`run_conflict_sweep_once`] did. All-zero + `gated_off` on a disabled/non-connected
/// brain (I3). `reasoner_unavailable` marks a cloud-not-ready / reasoner-down no-op (I6).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ConflictSweepReport {
    /// Not onboarded OR conflict-detect disabled — nothing scanned, no model call (I3).
    pub gated_off: bool,
    /// Cloud not consented, reasoner down, or a cycle already running — a safe no-op (I6).
    pub reasoner_unavailable: bool,
    /// Judge calls made this cycle.
    pub judged: usize,
    /// Proposals emitted.
    pub proposed: usize,
    /// Pairs the judge declined.
    pub dropped: usize,
    /// The per-cycle judge budget was hit.
    pub budget_hit: bool,
    /// The open-proposal ceiling was hit.
    pub ceiling_hit: bool,
}

/// Run ONE conflict-detection sweep: gate → delegate → map the core report. `now` is the
/// wall-clock epoch second (read by [`spawn`] at the boundary). Never panics; a reasoner/engine
/// error becomes a quiet `reasoner_unavailable` no-op (I6 — retry next cycle).
pub async fn run_conflict_sweep_once(
    engine: &EngineHandle,
    data_dir: &Path,
    now: i64,
) -> ConflictSweepReport {
    let onboarded = crate::identity::is_onboarded(data_dir);
    if !onboarded || !engine.conflict_detect_enabled_or_false(onboarded).await {
        return ConflictSweepReport { gated_off: true, ..Default::default() };
    }
    match engine.detect_conflicts_once(onboarded, now).await {
        Ok(r) => ConflictSweepReport {
            judged: r.judged,
            proposed: r.proposed,
            dropped: r.dropped,
            budget_hit: r.budget_hit,
            ceiling_hit: r.ceiling_hit,
            ..Default::default()
        },
        // Busy / reasoner-not-ready / transient open failure → a safe no-op this cycle (I6).
        Err(_) => ConflictSweepReport { reasoner_unavailable: true, ..Default::default() },
    }
}

/// Spawn the background conflict-sweep loop (mirrors `capture::sweeper::spawn`). The first tick
/// fires immediately; `MissedTickBehavior::Skip` prevents catch-up bursts. Detection stays OFF
/// until the owner enables it — the gate lives inside `run_conflict_sweep_once`, so a disabled
/// brain does zero work here. A panic in this task cannot take down the daemon.
pub fn spawn(engine: Arc<EngineHandle>, data_dir: PathBuf) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(SWEEP_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let now = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let report = run_conflict_sweep_once(&engine, &data_dir, now).await;
            // Surface only real work (mirrors the capture sweeper's quiet-on-noop discipline).
            if report.proposed > 0 || report.dropped > 0 || report.budget_hit || report.ceiling_hit {
                eprintln!(
                    "conflict-sweep: proposed {} / judged {} (dropped {}, budget-hit {}, ceiling-hit {})",
                    report.proposed, report.judged, report.dropped, report.budget_hit, report.ceiling_hit
                );
            }
        }
    });
}
```

   (c) `crates/bossclawd/src/lib.rs` — add `#[cfg(unix)] pub mod conflict;` beside the other `#[cfg(unix)]`
   modules (`server`/`telemetry`); it calls `engine`/`identity`, both `#[cfg(unix)]`.

4. Run → PASS: `cargo test -p bossclawd gated_off_is_a_quiet_noop`

5. Commit: `feat(rung3-p2): background conflict::sweeper loop (off-by-default, fail-safe report)`

---

## Task 13 — Wire the sweep into `main.rs` + §8 de-conflict doc + first-enable-drip + exit gate

Design §3.7 (§8 de-conflict minimum), §5 (first-enable drip), §6 (exit gate). Ships the feature OFF (nothing
detects until the owner enables), documents the two independent contradiction axes, and closes the exit gate.

**Files**
- Modify: `crates/bossclawd/src/main.rs` — spawn the conflict sweeper after the capture sweeper (`:159`).
- Modify: `crates/bossclaw-core/src/log.rs` — a `//` doc block on `detect_conflicts_once` recording the §8
  minimum (memory-level detection and the extract path's edge-level `reconcile_confirmed_contradiction` `:7730`
  / `ProposedRetraction` `extract.rs:298` are two independent, complementary axes; Phase 2 adds no edge
  cross-check).
- Test: `crates/bossclaw-core/src/log.rs` `mod tests` (first-enable drip).

**Steps**

1. Write the failing first-enable-drip test (design §5): on first enable the cursor is 0, so the whole corpus is
   "new"; the budget + ceiling must make day-one a trickle, not a wall.

```rust
/// Rung-3 Phase-2 (§5): first-enable does not flood — a corpus of many contradicting notes yields
/// at most CONFLICT_JUDGE_PER_SWEEP judge calls and at most that many proposals in one cycle.
/// `#[cfg(unix)]` (drives the append family).
#[cfg(unix)]
#[test]
fn first_enable_is_a_trickle_not_a_wall() {
    use crate::conflict::{build_conflict_prompt, CONFLICT_JUDGE_PER_SWEEP, CONFLICT_SYSTEM};
    use crate::reason::ScriptedReasoner;
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(64); // dim=64 so the near-identical notes clear CANDIDATE_SIM_MIN
    let log = open_log(dir.path());
    log.set_conflict_detect_enabled(true).unwrap();

    // Seed many near-identical "the flag is X" notes so every pair is a candidate.
    let mut reasoner = ScriptedReasoner::new("test");
    let mut texts = Vec::new();
    for i in 0..12 {
        let t = format!("the feature flag is value {i} in the shared config");
        log.remember(&emb, &t).unwrap();
        texts.push(t);
    }
    // Script EVERY ordered pair as a contradiction so nothing is dropped for lack of a response.
    for i in 0..texts.len() {
        for j in 0..texts.len() {
            if i != j {
                reasoner = reasoner.with_response(
                    CONFLICT_SYSTEM,
                    &build_conflict_prompt(&texts[i], &texts[j]),
                    serde_json::json!({ "contradicts": true, "winner": "newer", "confidence": 90, "why": "same flag" }),
                );
            }
        }
    }
    let no_passages = |_: &str, _: usize| None;
    let empty = std::collections::HashSet::new();
    let r = log.detect_conflicts_once(&emb, &reasoner, &no_passages, &empty, 1).unwrap();
    assert!(r.judged <= CONFLICT_JUDGE_PER_SWEEP, "day-one judging is budget-bounded ({})", r.judged);
    assert!(r.proposed <= CONFLICT_JUDGE_PER_SWEEP, "day-one proposals are a trickle ({})", r.proposed);
}
```

2. Run → FAIL first (before the main.rs wiring the doc/test drives nothing new — this test exercises Task 10's
   budget, so it should PASS if Task 10 is correct; if it FAILS, the budget is not enforced across subjects →
   fix Task 10, do not weaken the assertion). Run: `cargo test -p bossclaw-core first_enable_is_a_trickle_not_a_wall`

3. Implement the wiring + docs.
   (a) `main.rs` — after `bossclawd::capture::sweeper::spawn(engine.clone(), data_dir.clone());` (`:159`):

```rust
        // Rung-3 Phase-2: the conflict-detection sweep. OFF by default (gated inside the loop on
        // the owner's `conflict_detect_enabled` flag), so merging ships detection dormant.
        bossclawd::conflict::sweeper::spawn(engine.clone(), data_dir.clone());
```

   (b) `detect_conflicts_once` doc — extend its `///` block with the §8 note:

```rust
    // De-conflict with the extract path (spec §8): Rung-3 memory-level detection (this method) and
    // the evolve/extract path's EDGE-level reconciliation (`ProposedRetraction` →
    // `reconcile_confirmed_contradiction`) are two INDEPENDENT, complementary axes. There is no
    // reverse "memory-claim → invalidated-edge" index and Phase 2 adds none; de-dup happens only
    // WITHIN rung-3 via the proposal-idempotency fold (`is_conflict_proposal_suppressed`).
```

4. Run → PASS: `cargo test -p bossclaw-core first_enable_is_a_trickle_not_a_wall`
   then the full workspace gate (all foreground):
   - `cargo build --workspace`
   - `cargo clippy --workspace --all-targets -- -D warnings`
   - `cargo test -p bossclaw-core`
   - `cargo test -p bossclawd`
   - `cargo test -p memharness` (regression: `conflict_search` tuple contract intact)

5. Exit-gate checklist (map each design §6 item to a landed test; add any missing test before commit):
   - §6.2 unified index + recall-neutral → Task 2 `conflict_index_note_arm_and_passage_arm_are_both_typed_searchable`.
   - §6.3 sweep correctness (hermetic; cursor incrementality; budgets) → Tasks 8 + 10 (`decide_conflict_sweep_*`,
     `detect_conflicts_once_*`, `first_enable_is_a_trickle_not_a_wall`).
   - §6.4 proposal integrity (typed refs; idempotent; GC; restart) → Tasks 5/6/7.
   - §6.5 gate + fail-safe (flag off → no model; reasoner-down → no-op; never panics) → Tasks 4/10/12.
   - §6.6 off-by-default preserved (`prime_switches` force-off; merge ships OFF) → Task 4 + Task 13 wiring.

6. Commit: `feat(rung3-p2): wire conflict sweep into daemon (off-by-default) + §8 de-conflict doc + exit gate`

---

## Self-review

**1. Spec coverage — every design §3 component + §6 exit-gate item + §4 invariant maps to a task.**

| Design item | Task(s) |
| --- | --- |
| §3.1 unified fights index (note arm + typed decode) | 1, 2 |
| §3.2 conflict cursor | 3 |
| §3.3 background sweep (gate→cursor→finder→judge→emit) | 10, 11, 12 |
| §3.4 candidate-finder (exclusions; resolution set empty) | 8, 10 |
| §3.5 judge + proposal record (append; idempotency; GC; content-free `why`) | 5, 6, 7, 10 |
| §3.6 owner gate (default-CLOSED + prime_switches + or_false) | 4 |
| §3.7 / §8 de-conflict minimum (doc + rung-3 dedup) | 13 |
| §6.2 unified + recall-neutral | 2 |
| §6.3 sweep correctness (hermetic; cursor incrementality; **subject-by-subject no-stall**; budgets) | 8, 10 (`detect_conflicts_once_advances_multi_passage_capture_without_stall`), 13 |
| §6.4 proposal integrity (typed; idempotent; GC; restart) | 5, 6, 7, 10 |
| §6.5 gate + fail-safe | 4, 9, 10, 12 |
| §6.6 off-by-default preserved | 4, 13 |
| I1 never auto-retire (emits only) | whole plan — no mutation path exists (only `conflict_proposal` append) |
| I2 local & private (shared consent pre-gate) | 9, 11 |
| I3 off by default | 4, 10, 12 |
| I4 grows with new not total (cursor + dirty-gate; **subject-level no-stall**) | 3, 10 |
| I5 append-only honesty (signed event) | 5 |
| I6 fail-safe (reasoner down → no-op + retry; per-subject resumable cursor) | 10, 12 |
| I7 hostile input AND output (fenced input; CONTENT-FREE `templated_why` — no model text persisted; coarse band; no body) | 5, 10 (SENTINEL-leak assertion) |
| I8 app-only resolution (no guest-reachable op) | 12 (daemon-internal sweep, like capture) — no op added |
| I9 strict-quiet (floor + no dup + budgets/ceiling) | 6, 8, 10 |
| I-gc referential integrity | 6, 7 |

No gaps found.

**Portability discipline (`#[cfg(unix)]`).** `build_proposer_event` (log.rs:3084) and the whole write_proposal
family it mirrors are `#[cfg(unix)]`; so every new core method that calls it is gated the same:
`append_conflict_proposal`, `open_conflict_proposals`, `conflict_pair_key`, `is_conflict_proposal_suppressed`,
`pending_conflict_proposals`, `detect_conflicts_once`, the private `OpenConflictProposal` struct, and their
tests — all `#[cfg(unix)]`. Ungated (portable): `ConflictRef` + key codecs, `conflict_search_refs`, the cursor +
`unprocessed_conflict_subjects_since`, `ConfigFlag::ConflictDetect` + getter/setter, `decide_conflict_sweep`,
`templated_why`/`confidence_band`/`winner_str`/`bound_judge_text`, and the pub data structs (`ConflictSubject`,
`ConflictProposalRow`, `ConflictDetectReport`). On `bossclawd` the whole conflict sweeper module is
`#[cfg(unix)] pub mod conflict;` (it calls the `#[cfg(unix)]` `engine`/`identity`).

**Missing-docs discipline (`#![deny(missing_docs)]`, lib.rs:17).** Per-field `///` docs added to
`ConflictSubject` (`seq`, `within_seq_id`, `subject`) and `ConflictProposalRow` (all 7 pub fields); `ConflictRef`
enum-variant fields are exempt; `ConflictDetectReport`/`FinderInput`/`ConflictSweepReport`/`ConflictTelemetry`
already carry field docs.

**2. Placeholder scan.** No "TBD", "add error handling", "similar to Task N", or undefined types. Every function
a test calls is either verified-existing (with a file:line anchor) or created by an explicit step in the same or
an earlier task: `encode_note_key`/`decode_note_key`/`ConflictRef` (T1) → `conflict_search_refs`/note arm (T2)
→ `conflict_cursor`/`set_conflict_cursor`/`unprocessed_conflict_subjects_since`/`ConflictSubject` (T3) →
`conflict_detect_enabled`/`set_conflict_detect_enabled`/`conflict_detect_enabled_or_false` (T4) →
`append_conflict_proposal`/`templated_why`/`confidence_band`/`winner_str`/`bound_judge_text` (T5) →
`open_conflict_proposals`/`is_conflict_proposal_suppressed`/`conflict_pair_key` (T6) → `ConflictProposalRow`/
`pending_conflict_proposals` (T7) → `FinderInput`/`decide_conflict_sweep` (T8) → `cloud_consent_ok` (T9) →
`ConflictDetectReport`/`detect_conflicts_once` (T10) → `session_passage_text`/engine `detect_conflicts_once`/
`conflict_lock`/`conflict_tel`/`ConflictTelemetry`/`record_conflict_tick`/`conflict_telemetry` (T11) →
`ConflictSweepReport`/`run_conflict_sweep_once`/`spawn` (T12). (`sanitize_why` was DROPPED — the persisted `why`
is `templated_why`, per the I7 revision.)

**3. Type/signature consistency.** `ConflictRef` (T1) is the return element of `conflict_search_refs` (T2), the
field of `ConflictSubject` (T3), the arg of `append_conflict_proposal`/`is_conflict_proposal_suppressed` (T5/T6),
the `FinderInput.subject`/neighbour type (T8), and used throughout `detect_conflicts_once` (T10). **Cursor-shape
ripple (checked):** the cursor is `(i64, usize)` everywhere — `conflict_cursor() -> (i64, usize)` and
`set_conflict_cursor(seq, off)` (T3) are consumed by `detect_conflicts_once` (T10) as `let (cursor_seq,
cursor_off) = self.conflict_cursor()?` and `self.set_conflict_cursor(cs.seq, cs.within_seq_id + 1)`;
`unprocessed_conflict_subjects_since(cursor_seq, subject_offset, limit)` (3 args) matches its T10 call; and
`ConflictSubject.within_seq_id` (T3) is the value T10 advances the cursor by. `judge_pair`'s verified signature
`(&dyn Reasoner, &str, &str) -> Result<Option<Verdict>, _>` (conflict.rs:131) is honored in T10. `templated_why`
(4 `&str` args → `String`, T5) is called in T10 with `winner_str(..)`, `confidence_band(..)`, `ref_kind(older)`,
`ref_kind(newer)`. `conflict_search` keeps its verified `Vec<(String, usize, f32)>` tuple (memharness contract)
— T2 adds the sibling `conflict_search_refs` rather than mutating it. `ConflictDetectReport` (core, T10) is what
the engine wrapper returns (T11) and what `run_conflict_sweep_once` maps into `ConflictSweepReport` (T12). No
drift.

**4. Reality check (every cited-as-existing symbol grep-verified on `main` 64207b5).** `rebuild_conflict_index`
log.rs:5956, `conflict_search` :6002, `current_notes` :5189, `fold_notes` :8420, `fold_sessions` :8324 (fields
`current`/`deleted`/`superseded`/`retired_notes`/`retired_passages`), `CurrentNote{event_id,text,created_at,
superseded_by}` :415, `CurrentSession{event_id,session_id,...}` :387, `session_passages_for_model` :5855,
`session_passage_count` :5913, `derive_entity_vector`/`store_session_passages` :5756/:5819, `embed_one` :8122,
`evolve_cursor`/`set_evolve_cursor` :6080/:6093, `unprocessed_extractable_since` :6538, `events_of_types` :6568,
`event_by_id` :1025, `build_proposer_event` :3085, `append_write_proposal_with`/`is_proposal_suppressed`/
`pending_proposals` :2667/:2733/:2780, `ConfigFlag`+`key()` :273/:294, `capture_enabled`/`set_capture_enabled`/
`explicitly_set`/`latest_config_value` :6445/:6493/:6364/:6283, `retire_memory` :4824, `vector_index_len` :1470,
`MEMORY_EVENT_TYPE`/`SESSION_CAPTURED_EVENT_TYPE`/`SESSION_DELETED_EVENT_TYPE`/`SUPERSEDE_EVENT_TYPE`/
`NOTE_RETIRED_EVENT_TYPE`/`PASSAGE_RETIRED_EVENT_TYPE`/`UNRETIRE_EVENT_TYPE`/`WRITE_PROPOSAL_EVENT_TYPE`/
`M6B_PROPOSER_PRODUCER` graph.rs:23/35/37/33/40/42/44/94/102; `encode_chunk_key`/`decode_chunk_key`/
`CHUNK_KEY_SEP`/`event_id_of` index.rs:46/56/43/64; `judge_pair`/`Verdict`/`Winner`/`CONFLICT_CONF_MIN`/
`CONFLICT_SYSTEM`/`build_conflict_prompt`/`defuse`/`conflict_schema` conflict.rs:131/30/16/123/57/105/95/43;
`Reasoner::complete_json`/`ScriptedReasoner::with_response` reason.rs:35/70; sweeper
`SWEEP_INTERVAL`/`CAPTURE_PER_SWEEP`/`decide_sweep`/`run_sweep_once`/`spawn` :48/59/137/186/284; engine
`evolve_once`/`prime_switches`/`evolve_enabled_or_false`/`reasoner_config_or_default`/`reasoner_ready_or_false`/
`reasoner_provider`/`evolve_lock`/`ensure_indexed`/`current_notes`/`current_sessions`/`data_dir` mod.rs:914/529/
1006/1387/1420/262/265/560/741/727/380; `EngineHandle::new`/`new_test_handle_with_reasoner`/`MockReasonerProvider`/
`MockEmbedderProvider` mod.rs:292/2204 + reason.rs:176 + embed.rs:285; `read_capture_markdown`/`capture_body`/
`sessions_dir` store.rs:99/535/84; `valid_session_id` paths.rs:24; `is_onboarded` identity.rs:36; `chunk_text`
chunk.rs:56 (re-exported lib.rs:55). Verified this revision round: `build_proposer_event` IS `#[cfg(unix)]`
(log.rs:3084) and the whole write_proposal family is too (:2665/:2682/:2696/:2707/:2714/:2732/:2779);
`#![deny(missing_docs)]` bossclaw-core lib.rs:17; `hermetic_engine() -> (EngineHandle, TempDir)`
bossclawd/tests/sweeper.rs:34; `bossclawd::server::test_engine(home)` server.rs:981; `MAX_CAPTURE_MD_BYTES` (16
MiB) store.rs:92; `EvolveTelemetry` mod.rs:244 + `evolve_tel` :271 (init :307) + `record_tick_into` :1584;
bossclawd `pub mod engine`/`identity`/`server`/`telemetry` are `#[cfg(unix)]` while `pub mod capture` is NOT
(lib.rs); `unretire(retired_event_id)` log.rs:4847; `remember` engine wrapper mod.rs:617; `set_evolve_enabled`
engine wrapper mod.rs:1024 (no `set_conflict_detect_enabled` wrapper yet — T11 adds it). All confirmed.

**Open decisions flagged for owner review (NOT invented away):**
- **(a) Passage-text boundary.** The design's §3.5 "actual conflicting passage chunk" implies text access core
  does not have (core stores only vectors; text lives in the `.md`). This plan resolves it with a daemon-supplied
  `passage_text` closure into `detect_conflicts_once`. This is the one substantive deviation from the design's
  implicit "core does it all" model. Confirm the closure approach vs. persisting passage text in a table.
- **(b) `why` persistence — RESOLVED (owner-mandated).** Per the review, the persisted `why` is now a
  CONTENT-FREE template (`conflict::templated_why`, built only from `winner_hint` + `confidence_band` + ref
  kinds); the model's raw rationale is never persisted (debug-only `log::debug!`). No open choice remains here.
- **(c) Provisional constants** (`CANDIDATE_SIM_MIN = 0.82`, `CONFLICT_SEARCH_K = CONFLICT_JUDGE_PER_SWEEP`, `CONFLICT_OPEN_CEILING =
  20`, `CONFLICT_BAND_HIGH_MIN = 85`, `MAX_JUDGE_TEXT_BYTES = 4096`). The design leaves these "pinned
  provisionally; harness/owner-tunable." `CANDIDATE_SIM_MIN` in particular governs cost/precision and should be
  tuned against the P0 harness before enabling in production. (`WHY_MAX_CHARS` was removed with `sanitize_why`.)
- **(d) Cursor stall — RESOLVED (owner-chosen LOSSLESS fix).** The cursor is now `(seq, subject_offset)` and
  detection advances SUBJECT-BY-SUBJECT (Tasks 3 + 10). Because each subject's pairs are capped at
  `MAX_CANDIDATE_PAIRS_PER_SUBJECT == CONFLICT_JUDGE_PER_SWEEP = 8`, a fresh full budget always fits the first
  pending subject → detection always advances ≥1 subject/cycle → **no stall, nothing dropped**. With
  `CONFLICT_SEARCH_K == CONFLICT_JUDGE_PER_SWEEP`, a subject finds at most `budget` above-floor candidates and
  judges ALL of them (strictly lossless — the owner's "never skip"), so no per-subject drop occurs. The
  multi-passage no-stall test
  (`detect_conflicts_once_advances_multi_passage_capture_without_stall`, T10) is the regression guard. No open
  choice remains here.
- **(e) Note re-embed (I4) — DEFERRED table + trip-wire (owner-approved).** `rebuild_conflict_index` re-embeds
  every current note each rebuild (T2); the `note_conflict_vectors` table is deferred. Justified: the embedder
  is a static model2vec (token-vector lookup + mean-pool, not a transformer pass), so per-note cost is a cheap
  lookup; a `log::debug!` embed-count trip-wire makes the cost observable before production-enable. Revisit only
  if the trip-wire shows material cost.
- **(f) Telemetry shape.** Session conflict telemetry is in-memory (`ConflictTelemetry`), mirroring the
  `EvolveTelemetry` precedent exactly; a durable lifetime count is derivable from the append-only
  `conflict_proposal` events (`pending_conflict_proposals().len()` for open), so no table was added. Confirm the
  in-memory shape is sufficient, or request a durable counter table.
