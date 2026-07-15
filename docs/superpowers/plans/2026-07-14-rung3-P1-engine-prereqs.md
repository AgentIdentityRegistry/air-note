# Rung 3 — Phase 1: Engine Prerequisites — Implementation Plan (Rev 4, three review rounds folded)

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use `- [ ]`. **Run every `cargo` command SYNCHRONOUSLY in the foreground.** Tests here MIRROR named existing tests (real public methods + inline `Event{}` appends) — do NOT invent private helpers; when a snippet names a fn, it is either an existing fn (line cited) or created by an explicit step in the same task.

**Goal:** Engine prerequisites for Phase-2 detection — (7.1) a *separate* session-passage conflict index fed by passage vectors persisted at capture; (7.3) a reversible `retire_memory` primitive at **note + passage** granularity (App-only) — with recall (rungs 1/2) not regressing, a passage-retire surviving a sweeper cycle, `retire_memory` guest-refused, and `unretire` round-tripping **without ever reversing an ordinary edit**.

**Architecture:** State lives in `crates/bossclaw-core/src/log.rs` (`EventLog`, append-only + signed via `append_event_in_tx`). Retire uses **distinct `note_retired`/`passage_retired` marker events** (NOT a `supersede`, which is byte-identical to an edit — reusing it makes `unretire` unable to distinguish a retire from an edit). The distinct markers are folded into **`SessionFold`'s own new `retired_notes`/`retired_passages` sets** — the same fold the recall arm already consumes at `log.rs:1642` — so recall exclusion actually reaches the arm (fixing round-2 BLOCKER-1). Session passages are chunked + embedded once **at capture time by the daemon** (`store_capture`, which already holds `r.body`) and persisted to a new `session_passage_vectors` table (mirroring `entity_vectors`); `rebuild_conflict_index` reads that table, so the conflict index has a real, restart-surviving source and core stays filesystem-free (fixing round-2 MAJOR: wrong seam). The recall `vector_index` is byte-untouched → "no recall regression" by construction.

**Tech Stack:** Rust, `serde_json`, encrypted SQLite (`store.rs`), `hnsw_rs` via `VectorIndex`/`HnswIndex` (`index.rs`), `model2vec_rs` embedder, `clap` (memharness), ULID ids, Ed25519 signing (automatic in `append_event_in_tx:955`).

**Spec:** `docs/superpowers/specs/2026-07-12-rung3-conflict-resolution-design.md` (§3, §7, §9, §13, I1/I5/I6/I8). **Branch:** `feat-rung3-conflict-resolution`.

**Owner decisions (ratified 2026-07-14):** **D1** session retire is PASSAGE-granularity only (no whole-session retire — nothing invokes it; §4c conflicts at passage level); notes retire via their own marker. **D2** passage vectors persist at capture in `session_passage_vectors`.

**Accepted Phase-1 limitations (documented, deferred):**
- **Model-version survival:** `session_passage_vectors` is keyed by `model_id` (mirroring `entity_vectors`). A rung-2 language-pack swap changes the active `model_id`, so `rebuild_conflict_index` returns empty until sessions are re-captured under the new model. Acceptable because the *consumer* (Phase-2 detection) does not exist yet. A future re-embed hook (mirroring `reembed_prepare`) is out of Phase-1 scope. **Must be stated in a code comment on the table.**
- **Orphan passage rows:** a changed-sha re-capture mints a new capture event id; the old id's passage rows are skipped by `rebuild_conflict_index` (it reads only current fold heads) but remain on disk. Bounded cleanup is a one-line delete in Task 5 Step 3 (delete rows for a superseded capture id); if deferred, document it.
- **Passage-id ordinal fragility:** `passage_id` = chunk ordinal; a changed-sha body re-chunk shifts ordinals, so a marker could mis-target after an edit. Harmless for the Phase-1 primitive (no Retire *action* yet); when the action ships (Phase 3), key the marker on a chunk content-hash.
- **Retire excludes recall/list/embed-gate, not yet derived state:** a `note_retired` note still leaves any already-minted entities/edges (surfaceable via graph-proximity boost) and is not removed from the extraction queue (`unprocessed_extractable_since`). This is the §7.4 edge-invalidation / Task-9 deferral (resolution-time, Phase 3); state it as an I5 completeness caveat in `retire_memory`'s doc comment. Not a Phase-1 blocker (the judge only reads passages/notes; edge-invalidation is a resolution-time concern).

**Exit gate (§3/§13):** passage index built + queried; recall-neutrality (rungs 1/2 unchanged); passage-retire survives a simulated sweeper cycle; `retire_memory` guest-refused; `unretire` round-trips and never un-does an edit; passage-vs-title catch rate on an honest fixture.

---

## File Structure (verified line refs)
**Core (`crates/bossclaw-core/src/`):**
- `graph.rs:37` (beside `SESSION_DELETED_EVENT_TYPE`) — `NOTE_RETIRED_EVENT_TYPE`, `PASSAGE_RETIRED_EVENT_TYPE`, `UNRETIRE_EVENT_TYPE`. Keep them OUT of `EMBEDDABLE_EVENT_TYPES` (`log.rs:345`).
- `log.rs` — `SessionFold` (`:7838`) gains `retired_notes: HashSet<String>` + `retired_passages: HashSet<(String,usize)>`; `fold_sessions` (`:7859`) learns the 3 types (insert on retire, remove on unretire, seq order); `session_events_ordered` (`:4822`) adds the 3 types; recall memory arm (`:1785`) also excludes `retired_notes` (extracted at `:1642`); `embed_excluded_event_ids` (`:4784`) unions `retired_notes`; `current_notes` (`:4929`)/`fold_notes` (`:7908`) exclude retired notes from the Library list; new `retire_memory`/`unretire`/`retire_passage`/`unretire_passage` + `assert_retirable_note`/`assert_note_retired` (mirror `supersede_note:4720` validation + `delete_session:4898` append shape); new `session_passage_vectors` table + `store_session_passages`/`session_passages_for_model` (mirror `derive_entity_vector:5494`/`entity_vectors_for_model:5529`); new `conflict_index` field + `rebuild_conflict_index` (mirror `rebuild_entity_index:5515`) + `conflict_search` (mirror `entity_search:5554`) + `pub(crate) fn vector_index_len(&self)->usize`.
- `index.rs:45` — port `chunk_text` deps + `encode_chunk_key`/`decode_chunk_key`/`event_id_of` from `origin/feat-retrieval-rung3-chunking`; add `fn len(&self)->usize` to `VectorIndex` + `HnswIndex`.
**Created:** `crates/bossclaw-core/src/chunk.rs` (port).
**Proto (`crates/bossclawd-proto/src/lib.rs`):** `Request::RetireMemory{onboarded, target: RetireTarget}` + `Unretire{onboarded, retired_event_id}` (`:125`); `enum RetireTarget{ Note{event_id}, Passage{session_id, passage_id} }` deriving `Serialize,Deserialize,Clone,PartialEq,Debug`; `Response::Retired(String)` (`:257`). `Role::allows` (`:71`) + `PROTO_VERSION` untouched (auto-refused; every `Response` consumer has an `other=>` catch-all — verified).
**Daemon (`crates/bossclawd/src/`):** `server.rs:243` dispatch arms; `engine/mod.rs:717` async wrappers (mirror `supersede_note`); `capture/store.rs:109` `store_capture` binds the `capture_session` id + chunks `r.body` + persists passages; `heal_orphans` (window a, `store.rs:270`) does the same via `read_capture_markdown`. Retire path must NOT call `delete_capture:163` (it removes the `.md`).
**Harness (`crates/memharness/src/`):** `main.rs` `conflict-grade --retrieval {title|passage}`; `compare.rs` `recall_regressed`; new `fixtures/session-conflict-pairs.jsonl`.

---

## Task 1 — Distinct retire markers + reversible `SessionFold` state (BLOCKER-1 fix)

Adds the 3 event consts and folds them into `SessionFold`'s OWN new sets (the fold recall consumes), so `unretire` reverses a retire and NEVER an edit-supersede. Standalone: the test appends raw `Event{}`s via the in-module `append` (as `delete_session` does), so it compiles without Task 2.

**Files:** `graph.rs:37`; `log.rs:7838`(struct), `:7859`(`fold_sessions`), `:4822`(`session_events_ordered`). Test: mirror `deleted_session_tombstones_in_fold` (session-fold test in `log.rs`'s `#[cfg(test)] mod tests`; setup `let dir = tempfile::tempdir().unwrap(); let log = open_log(dir.path());`).

- [ ] **Step 1: consts** (`graph.rs:37`):
```rust
/// Rung-3 retire markers — DISTINCT from `supersede` (a supersede is byte-identical to an edit;
/// a distinct type is what lets `unretire` reverse a retire without ever reversing an edit).
pub const NOTE_RETIRED_EVENT_TYPE: &str = "note_retired";       // content: {"retires": <note_event_id>}
pub const PASSAGE_RETIRED_EVENT_TYPE: &str = "passage_retired"; // content: {"session_id","passage_id"}
pub const UNRETIRE_EVENT_TYPE: &str = "unretire";              // content: {"unretires": id}  OR  {"session_id","passage_id"}
```
- [ ] **Step 2: failing test** (append raw markers inline; assert fold reversal + edit-disjointness):
```rust
#[test]
fn note_retire_folds_reversibly_and_leaves_edit_supersedes_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let emb = MockEmbedder::new(8);
    // a real note + a real edit-supersede of it (existing public path):
    let n = log.remember(&emb, "uses Vercel").unwrap();
    let edited = log.supersede_note(&emb, &n, "left Vercel").unwrap(); // n now in fold.superseded
    // retire the (edited) head via a DISTINCT note_retired marker, appended inline like delete_session:
    log.append(Event { id: String::new(), ts: String::new(), valid_time: None,
        event_type: crate::graph::NOTE_RETIRED_EVENT_TYPE.to_string(),
        content: serde_json::json!({ "retires": edited }),
        model_meta: None, prev_hash: String::new(), hash: None,
        signed_by_did: log.signer_did(), signature: None }).unwrap();
    let fold = fold_sessions(&log.session_events_ordered().unwrap());
    assert!(fold.retired_notes.contains(&edited));
    assert!(fold.superseded.contains(&n), "the ORIGINAL edit-supersede is untouched");
    // unretire removes ONLY from retired_notes:
    log.append(Event { /* ..UNRETIRE.. */ content: serde_json::json!({ "unretires": edited }), ../*same shape*/ }).unwrap();
    let fold = fold_sessions(&log.session_events_ordered().unwrap());
    assert!(!fold.retired_notes.contains(&edited));
    assert!(fold.superseded.contains(&n), "unretire did NOT reverse the edit");
}
```
- [ ] **Step 3: run → FAIL** (`retired_notes` field absent). `cargo test -p bossclaw-core --lib note_retire_folds_reversibly`.
- [ ] **Step 4: implement.** `SessionFold` (`:7838`) — add `retired_notes: HashSet<String>` + `retired_passages: HashSet<(String, usize)>`. `session_events_ordered` (`:4822`) — add `NOTE_RETIRED_EVENT_TYPE, PASSAGE_RETIRED_EVENT_TYPE, UNRETIRE_EVENT_TYPE` to the `events_of_types([...])`. `fold_sessions` (`:7859`) first loop — add arms (events are already `seq ASC`):
```rust
crate::graph::NOTE_RETIRED_EVENT_TYPE => { if let Some(id)=ev.content.get("retires").and_then(|v|v.as_str()) { retired_notes.insert(id.into()); } }
crate::graph::PASSAGE_RETIRED_EVENT_TYPE => { if let (Some(s),Some(p))=(sid(ev),pid(ev)) { retired_passages.insert((s,p)); } }
crate::graph::UNRETIRE_EVENT_TYPE => {
    if let Some(id)=ev.content.get("unretires").and_then(|v|v.as_str()) { retired_notes.remove(id); }
    else if let (Some(s),Some(p))=(sid(ev),pid(ev)) { retired_passages.remove(&(s,p)); }
}
```
(`sid`/`pid` = tiny inline closures reading `content["session_id"]`/`content["passage_id"]`.) Return them in the `SessionFold{..}`.
- [ ] **Step 5: run → PASS. Step 6: commit** `feat(rung3-p1): distinct reversible retire markers folded into SessionFold (disjoint from supersede)`.

---

## Task 2 — `retire_memory`(note) + `unretire` primitives + recall/list exclusion (BLOCKER-1 wiring)

**Files:** `log.rs` new `retire_memory`/`unretire`/`assert_retirable_note`/`assert_note_retired` (append shape from `delete_session:4898`; validation from `supersede_note:4726-4745`); recall arm `:1642`+`:1785`; `embed_excluded_event_ids:4784`; `current_notes:4929`/`fold_notes:7908`. Test: mirror `superseded_note_excluded_but_replacement_recallable` (`log.rs:8232`) — swap the supersede for a retire.

- [ ] **Step 1: failing test** (mirror `:8232` setup; note retired → gone from recall AND `current_notes`; unretire restores; unretire refuses a non-retired id):
```rust
#[test]
fn retire_memory_note_excludes_from_recall_and_list_and_unretire_round_trips() {
    let dir = tempfile::tempdir().unwrap(); let log = open_log(dir.path()); let emb = MockEmbedder::new(8);
    let ev = log.remember(&emb, "we deploy on Vercel").unwrap();
    log.rebuild_indexes(&emb).unwrap();
    assert!(log.recall(&emb, "Vercel", 10, &RecallOptions::default()).unwrap().iter().any(|h| h.event_id == ev));
    log.retire_memory(&ev).unwrap();
    assert!(!log.recall(&emb, "Vercel", 10, &RecallOptions::default()).unwrap().iter().any(|h| h.event_id == ev), "retired note excluded from recall");
    assert!(!log.current_notes().unwrap().iter().any(|n| n.event_id == ev), "retired note excluded from the Library list");
    assert!(matches!(log.unretire("not-a-retired-id"), Err(BossclawError::InvalidInput(_))), "unretire refuses a non-retired id");
    log.unretire(&ev).unwrap();
    assert!(log.recall(&emb, "Vercel", 10, &RecallOptions::default()).unwrap().iter().any(|h| h.event_id == ev), "unretire restores recall");
    assert!(matches!(log.retire_memory("nope"), Err(BossclawError::InvalidInput(_))));
}
```
- [ ] **Step 2: run → FAIL.**
- [ ] **Step 3: implement.**
```rust
/// Retire a memory-kind note (rung-3 "Retire older") via a DISTINCT `note_retired` marker
/// (reversible, no replacement). App-only (guest-refused at proto). Validation mirrors
/// `supersede_note` (exists, memory-kind, not already superseded), plus not-already-retired.
pub fn retire_memory(&self, target_event_id: &str) -> Result<String, BossclawError> {
    self.assert_retirable_note(target_event_id)?;
    self.append(Event { id: String::new(), ts: String::new(), valid_time: None,
        event_type: crate::graph::NOTE_RETIRED_EVENT_TYPE.to_string(),
        content: serde_json::json!({ "retires": target_event_id }),
        model_meta: None, prev_hash: String::new(), hash: None,
        signed_by_did: self.signer_did(), signature: None })
}
pub fn unretire(&self, retired_event_id: &str) -> Result<String, BossclawError> {
    self.assert_note_retired(retired_event_id)?;  // must be in fold_sessions().retired_notes — else InvalidInput
    self.append(Event { /* UNRETIRE, content: {"unretires": retired_event_id}, same 10 fields */ })
}
```
  - `assert_retirable_note`: copy `supersede_note`'s three checks (`:4730-4745`) EXCEPT blank-text, and add `if fold_sessions(&self.session_events_ordered()?).retired_notes.contains(id) { return Err(InvalidInput("already retired")) }`.
  - `assert_note_retired`: `if !fold_sessions(...).retired_notes.contains(id) { return Err(InvalidInput("not retired")) }`.
  - Recall wiring: at `:1642` also bind `retired_notes` from the fold → change the tuple to `(current_session_event_ids, superseded_ids, retired_note_ids)`; memory arm (`:1785`) → `return !superseded_ids.contains(&h.event_id) && !retired_note_ids.contains(&h.event_id);`.
  - `embed_excluded_event_ids` (`:4784`) — after building `excluded`, `excluded.extend(fold.retired_notes)`.
  - Library list: `current_notes` (`:4929`) add `NOTE_RETIRED_EVENT_TYPE, UNRETIRE_EVENT_TYPE` to its `events_of_types`; `fold_notes` (`:7908`) build a `retired: HashSet<&str>` (insert on `NOTE_RETIRED.retires`, remove on `UNRETIRE.unretires`) and add `&& !retired.contains(ev.id.as_str())` to the filter (`:7921`).
- [ ] **Step 4: run → PASS**, then `cargo test -p bossclaw-core` (no supersede/delete regression). **Step 5: commit** `feat(rung3-p1): retire_memory + unretire note primitives (distinct marker; recall+list excluded; guarded reversal)`.

---

## Task 3 — Proto op + allowlist + server + engine wrapper (§7.3, guest-refused)
**Files:** `proto/lib.rs:125`/`:257`/`:820`/`:846`; `server.rs:243`(dispatch)/`:1055`(override test); `engine/mod.rs:717`.
- [ ] **Step 1: failing tests** — extend `memory_client_allows_exactly_four_ops:820` (add `RetireMemory`/`Unretire` to the refused list), `new_variants_round_trip_serde:846` (both variants), and a `server.rs` assert that `override_onboarding_for_guest(RetireMemory{..})` is `None` (`:1055`).
- [ ] **Step 2: run → FAIL** (variants missing → non-exhaustive `dispatch` won't compile — the intended RED).
- [ ] **Step 3: add** `Request::{RetireMemory,Unretire}` + `RetireTarget` + `Response::Retired`. `Role::allows` untouched; `PROTO_VERSION` = 1.
- [ ] **Step 4: dispatch + wrappers** (`server.rs:243`; helpers here are `op_result`/`not_permitted_response()`/`unit_result` — the Passage placeholder returns a Rejected via the engine, NOT an invented fn):
```rust
Request::RetireMemory { target, .. } => match target {
    RetireTarget::Note { event_id } => op_result(engine.retire_memory(event_id).await, Response::Retired),
    RetireTarget::Passage { .. } => op_result(Err(EngineOpError::Rejected("passage retire lands in Task 7".into())), Response::Retired),
},
Request::Unretire { retired_event_id, .. } => op_result(engine.unretire(retired_event_id).await, Response::Retired),
```
`EngineHandle::retire_memory`/`unretire` mirror `supersede_note:717` (`is_onboarded_local`, `spawn_blocking`, `InvalidInput→Rejected`) but do NOT force `indexed=false` — recall exclusion is fold-time (`:1642`), so no rebuild is needed (contrast `supersede_note`'s wrapper, which does set it).
- [ ] **Step 5: `cargo test -p bossclawd-proto -p bossclawd` → PASS. Step 6: commit** `feat(rung3-p1): retire_memory/unretire proto op — App-only, guest-refused, server-wired`.

---

## Task 4 — Port `chunk.rs` + key helpers + `VectorIndex::len` + `vector_index_len`
- [ ] **Step 1: port** `git show origin/feat-retrieval-rung3-chunking:crates/bossclaw-core/src/chunk.rs > crates/bossclaw-core/src/chunk.rs`; `mod chunk; pub use chunk::chunk_text;` in `lib.rs`; copy `CHUNK_KEY_SEP`/`encode_chunk_key`/`decode_chunk_key`/`event_id_of` (+ their tests) into `index.rs`.
- [ ] **Step 2:** add `fn len(&self)->usize` to the `VectorIndex` trait (`index.rs:45`) + `HnswIndex`; add `pub(crate) fn vector_index_len(&self)->usize` on `EventLog` returning the recall index's `len()` (0 if unbuilt) — used by Task 6's recall-untouched assertion.
- [ ] **Step 3: `cargo test -p bossclaw-core --lib chunk index` → PASS. Step 4: commit** `feat(rung3-p1): port chunk_text + composite key helpers + index len accessors`.

---

## Task 5 — Persist session passages at capture (§7.1 data source, MAJOR fix)
Core gets the table + store/read (mirror `derive_entity_vector:5494`/`entity_vectors_for_model:5529`); the daemon `store_capture` (the REAL capture path) binds the returned event id, chunks `r.body`, and persists.

**Files:** `log.rs` (table DDL beside the `entity_vectors` migration; `store_session_passages`; `session_passages_for_model`); `engine/mod.rs` wrapper; `capture/store.rs:109` `store_capture` + `:270` `heal_orphans`.
- [ ] **Step 1: failing core test** — persist 2 passages under a capture id, survive reopen:
```rust
#[test]
fn store_session_passages_persists_and_survives_reopen() {
    let dir = tempfile::tempdir().unwrap(); let emb = MockEmbedder::new(8);
    let chunks = vec!["we deploy on Vercel".to_string(), "db is Postgres".to_string()];
    { let log = open_log(dir.path());
      log.store_session_passages(&emb, "cap1", &chunks).unwrap(); }
    let log = open_log(dir.path());  // reopen
    let rows = log.session_passages_for_model(emb.model_id()).unwrap(); // Vec<(event_id, passage_ix, Vec<f32>)>
    assert_eq!(rows.iter().filter(|(e,_,_)| e == "cap1").count(), 2);
}
```
- [ ] **Step 2: run → FAIL. Step 3: implement.**
  - Table (unchanged): `session_passage_vectors(session_captured_event_id TEXT, passage_ix INTEGER, model_id TEXT, dim INTEGER, embedding BLOB, PRIMARY KEY(session_captured_event_id, passage_ix, model_id))` with a `-- model_id-scoped; a language-pack swap empties this until re-capture (Phase-1 accepted limitation)` comment.
  - **Core signature — take the EMBEDDER and embed internally (mirror `derive_entity_vector:5494`; the `model_id`/`dim` come from `embedder`, fixing round-3 BLOCKER):** `pub fn store_session_passages(&self, embedder: &dyn Embedder, event_id: &str, chunks: &[String]) -> Result<(), BossclawError>` — embeds each chunk, `INSERT OR REPLACE` a row per `(event_id, ix)` tagged `embedder.model_id()`/`dim()` (`vec_to_blob`). `session_passages_for_model(model_id)` mirrors `entity_vectors_for_model:5529` but SELECTs `passage_ix` too → `Vec<(String /*event_id*/, usize /*passage_ix*/, Vec<f32>)>`, ordered `session_captured_event_id, passage_ix ASC`. Add `pub fn session_passages_absent(&self, event_id: &str) -> Result<bool, BossclawError>` (`SELECT 1 FROM session_passage_vectors WHERE session_captured_event_id=?1 LIMIT 1`).
  - **Engine wrapper** `EngineHandle::store_session_passages(&self, event_id: String, chunks: Vec<String>)` resolves `embedder_for(&log)` INTERNALLY (as `capture_session:650` does) and calls the core fn — so the daemon never needs an embedder. Also `EngineHandle::session_passages_absent(event_id)`.
  - **Daemon `store_capture`** (`store.rs:156`): bind the id — `let ev = engine.capture_session(meta).await.map_err(..)?;` — then skip re-embed on a same-sha no-op: `if engine.session_passages_absent(ev.clone()).await.map_err(..)? { engine.store_session_passages(ev, chunk_text(&r.body)).await.map_err(..)?; }`. Add the same in `heal_orphans` (`store.rs:270`) reading the body via `read_capture_markdown` **then stripping the front-matter block first** (so heal chunks the SAME text `store_capture` does — `r.body` only, not the whole `.md`; use the body-extractor `front_matter_block`). Optional orphan GC: on a changed-sha supersede, delete rows for the old capture id.
- [ ] **Step 4: run → PASS. Step 5: commit** `feat(rung3-p1): persist session-passage vectors at capture (store_capture seam, restart-safe)`.

---

## Task 6 — Separate `conflict_index` + passage retrieval (§7.1, BLOCKER-2 test fix)
**Files:** `log.rs:445`(field, init `None` like `entity_index`), `rebuild_conflict_index`(mirror `:5515`), `conflict_search`(mirror `:5554`). Test **captures first** so the event id maps to a real `session_id`.
- [ ] **Step 1: failing test.**
```rust
#[test]
fn conflict_index_retrieves_by_session_and_leaves_recall_len_unchanged() {
    let dir = tempfile::tempdir().unwrap(); let log = open_log(dir.path()); let emb = MockEmbedder::new(8);
    let ev = log.capture_session(&emb, &session_meta("s1", "aa")).unwrap();  // real capture → fold head "s1"
    let chunks = vec!["we deploy on Vercel".to_string(), "db is Postgres".to_string()];
    log.store_session_passages(&emb, &ev, &chunks).unwrap();
    log.rebuild_indexes(&emb).unwrap();
    let recall_len = log.vector_index_len();
    log.rebuild_conflict_index(&emb).unwrap();
    let hits = log.conflict_search(&emb.embed(&["Vercel".into()]).unwrap()[0], 8); // k ≥ #chunks → membership stable
    assert!(hits.iter().any(|(sid, pid, _)| sid == "s1" && *pid == 0));            // session id resolved via fold head
    assert_eq!(log.vector_index_len(), recall_len, "recall vector_index byte-untouched");
}
```
- [ ] **Step 2: run → FAIL. Step 3: implement.** `rebuild_conflict_index`: build `event_id→session_id` from `fold_sessions().current`; for each current session's rows from `session_passages_for_model`, skip `(session_id, ix) ∈ fold.retired_passages`, `index.add(&encode_chunk_key(&session_id, ix), &vec)`; box into `conflict_index`. `conflict_search(qv,k) -> Vec<(String,usize,f32)>` decodes each hit key via `decode_chunk_key`. **Do NOT touch `rebuild_indexes`/`vector_search`/`EMBEDDABLE_EVENT_TYPES`.** Assert set-membership only; keep `k ≥ index size` (HNSW rank is non-deterministic across rebuilds, spec §13).
- [ ] **Step 4: run → PASS. Step 5: commit** `feat(rung3-p1): separate session-passage conflict index + retrieval (recall untouched)`.

---

## Task 7 — Passage-level retire + sweeper-cycle durability (§7.2/§7.3)
**Files:** `log.rs` `retire_passage`/`unretire_passage` (emit `PASSAGE_RETIRED`/`UNRETIRE`, validate the passage exists in the current session body); `server.rs` real Passage dispatch arm; `engine/mod.rs` wrapper. Test captures first, retires after.
- [ ] **Step 1: failing test.**
```rust
#[test]
fn passage_retire_hides_one_survives_sweep_and_reverses() {
    let dir = tempfile::tempdir().unwrap(); let log = open_log(dir.path()); let emb = MockEmbedder::new(8);
    let ev = log.capture_session(&emb, &session_meta("s1","aa")).unwrap();
    let chunks = vec!["Vercel".to_string(), "Postgres".to_string()];
    log.store_session_passages(&emb, &ev, &chunks).unwrap();
    log.rebuild_conflict_index(&emb).unwrap();
    log.retire_passage("s1", 0).unwrap();
    log.rebuild_conflict_index(&emb).unwrap();
    let hit = |q:&str| log.conflict_search(&emb.embed(&[q.into()]).unwrap()[0], 8).iter().any(|(s,p,_)| s=="s1" && *p==0);
    assert!(!hit("Vercel"), "retired passage 0 hidden");
    assert!(log.conflict_search(&emb.embed(&["Postgres".into()]).unwrap()[0],8).iter().any(|(s,p,_)| s=="s1" && *p==1), "sibling kept");
    // SWEEP durability: same-sha re-capture is a no-op (returns the same id); marker persists across rebuild.
    log.capture_session(&emb, &session_meta("s1","aa")).unwrap();
    log.rebuild_conflict_index(&emb).unwrap();
    assert!(!hit("Vercel"), "retire survives a sweeper cycle");
    log.unretire_passage("s1", 0).unwrap(); log.rebuild_conflict_index(&emb).unwrap();
    assert!(hit("Vercel"), "unretire restores the passage");
}
```
- [ ] **Step 2: run → FAIL. Step 3: implement** `retire_passage`/`unretire_passage` (append `PASSAGE_RETIRED`/`UNRETIRE` with `{session_id,passage_id}`) + the real `server.rs` Passage arm + `engine/mod.rs` wrapper. **Validation source (model-agnostic, so it works before any `rebuild_indexes`):** resolve the session's current fold-head capture event id via `fold_sessions().current`, then reject `passage_id >= N` where `N` = the count of `session_passage_vectors` rows for that event id (a `SELECT COUNT(*) ... WHERE session_captured_event_id=?1` — do NOT scope by model_id here). (Ordinal-shift limitation documented in the header.) **Step 4: run → PASS. Step 5: commit** `feat(rung3-p1): passage-granular retire — one hidden, siblings intact, survives sweep, reversible`.

---

## Task 8 — Harness: recall-neutrality + honest passage-vs-title (§9/§13)
**Files:** `compare.rs` `recall_regressed`; `main.rs` `--retrieval {title|passage}`; new fixture.
- [ ] **Step 1: recall-neutrality — by construction + measured.** (a) A `bossclaw-core` golden test: capture a session, `store_session_passages` + `rebuild_conflict_index` + `retire_passage`, and assert a note-recall over a fixed corpus is byte-identical before/after (the conflict index provably cannot perturb `vector_index`). (b) `recall_regressed(&[SegmentComparison]) -> Option<&SegmentComparison>` in `compare.rs` (first gating segment with a significant negative s@k delta) + unit test. Runbook (owner-gated, frozen corpus): pre/post-Phase-1 `memharness run` → `compare` → assert `recall_regressed == None`.
- [ ] **Step 2: honest passage-vs-title.** Fixture `session-conflict-pairs.jsonl` schema `{"session_id","title","body","conflicting_query","label":"contradicts|coexist"}` where the conflict lives in `body` (title generic), WITH coexist hard-negatives. `flagged = did the retrieval front-end surface the conflicting passage as a candidate` (judge-free). Assert ABSOLUTE bars, not a bare `>`:
```rust
#[test]
fn passage_index_meaningfully_beats_title_only() {
    let f = load_session_pairs(include_str!("../fixtures/session-conflict-pairs.jsonl"));
    let (title, passage) = (grade_retrieval(&f, Retrieval::TitleOnly), grade_retrieval(&f, Retrieval::PassageIndex));
    assert!(title.recall < 0.30, "title-only genuinely misses body conflicts: {}", title.recall);
    assert!(passage.recall >= 0.70, "passage index catches them: {}", passage.recall);
    assert!(passage.precision >= title.precision, "passage index does not over-surface (hard negatives)");
}
```
Define `load_session_pairs`/`grade_retrieval`/`Retrieval` in the harness (memharness already depends on `bossclaw-core` in-process, so `grade_retrieval` calls the real index directly — no wire op).
- [ ] **Step 3: run → FAIL, implement, run → PASS. Step 4: live gate (owner-gated)** — record numbers in a RESULTS section. **Step 5: commit** `feat(rung3-p1): harness recall-neutrality (golden + guard) + honest passage-vs-title`.

---

## Task 9 (OPTIONAL — defer to Phase 3) — §7.4 edge-invalidation
Stamp `Edge.invalidated_at`/`invalidated_by` (`graph.rs:147/149`) inside a confirmed retire when a retired note maps to a derived edge. The Retire *action* is Phase 3; leave a one-line TODO in `retire_memory`.

---

## Downstream / product-direction (owner, 2026-07-14)
Per [[air/vision-background-first-claude-code-native-2026-07-14]]: background-first, reachable from Claude Code — not a destination app. Phase 1 is interface-agnostic engine work (aligned). **Phase 3 implication:** surface the conflict + draft the retire THROUGH the Claude Code session, with the confirm on a trusted surface (resolution stays App-only/guest-refused, I1/I8). Don't over-invest in desktop UI.

## Global gates + invariants
`cargo clippy --workspace --all-targets -- -D warnings` and `cargo test -p bossclaw-core -p bossclawd-proto -p bossclawd -p memharness`. I1 (retire is always an explicit op), I5 (append+sign automatic; retire markers distinct from supersede so honesty holds), I6 (double-retire/unretire-non-retired refused; fold-rebuild self-heals), I8 (allowlist gains nothing). Never call `delete_capture:163` in retire.

## Second-review resolutions (round 2, architect + critic)
- **BLOCKER note-retire wired to wrong fold** → `retired_notes` added to `SessionFold`, populated in `fold_sessions`, read at recall `:1642`/`:1785` + `embed_excluded` + `current_notes`/`fold_notes` (Tasks 1-2).
- **BLOCKER Tasks 6/7 test identity** → tests `capture_session` FIRST to get the real event id, then `store_session_passages(&ev,...)`, then assert by `session_id` (Tasks 6-7).
- **MAJOR wrong capture seam** → Task 5 targets `store_capture:109` (holds `r.body`), binds the discarded `capture_session` id, covers `heal_orphans`.
- **MAJOR model-version survival** → documented accepted limitation (Phase-2 consumer doesn't exist yet); table comment required.
- **`event_of` nonexistent / free-fn claim wrong** → real `self.append(Event{..})` literal (as `delete_session:4898`); `assert_*` helpers are explicit new methods.
- **Invented/miscalled helpers** → tests mirror named existing tests (`deleted_session_tombstones_in_fold`, `superseded_note_excluded_but_replacement_recallable:8232`) using REAL public methods (`remember`, `recall`, `current_notes`, `capture_session`, `rebuild_indexes`) + inline `Event{}`; `vector_index_len` added as an explicit step (Task 4); `open_log(dir.path())` arity correct.
- **`current_notes` still listed retired (M7)** → `fold_notes` now excludes retired notes (Task 2).
- **Orphan rows / ordinal fragility / boot-rebuild** → documented limitations; orphan GC is an optional one-liner (Task 5).
- **Line refs** → corrected (`current_notes:4929`, `entity_vectors_for_model:5529`, `EMBEDDABLE_EVENT_TYPES` is `log.rs:345`).

## Round-3 verification resolutions (fresh critic on Rev 3 → folded here as Rev 4)
A fresh independent critic verified Rev 3 against the real source and CONFIRMED the round-2 fixes hold (retired-note fold wired correctly; capture-seam retargeted; every helper exists or is created-by-step except one). It found ONE blocker cluster, folded here:
- **BLOCKER `store_session_passages` had no `model_id` source** → core signature now takes `embedder: &dyn Embedder` and embeds internally (mirror `derive_entity_vector`); all 3 test call-sites updated to `store_session_passages(&emb, id, &chunks)`.
- **MAJOR daemon has no embedder** → the engine wrapper resolves `embedder_for` internally; the daemon passes only `chunks`.
- **MAJOR dangling `session_passages_absent`** → now an explicit core method + engine wrapper (existence check for the same-sha re-embed skip).
- **MINOR heal_orphans chunks whole `.md`** → strip front-matter first (chunk `r.body`-equivalent).
- **retire_passage validation** → count persisted rows for the fold-head event id (model-agnostic).
- Retired-notes-in-derived-state (extraction queue / edges) added as an I5 completeness caveat (Task-9/Phase-3 deferral).

## Self-review honesty
Written against the ACTUAL source (SessionFold/fold_sessions `:7838-7893`, recall arms `:1636-1795`, `delete_session:4889`, `external_note_event:4686`, `entity_vectors` `:5494-5548`, `store_capture:109`, `capture_session` wrapper `engine/mod.rs:644`) and hardened across THREE independent review rounds (findings shrank each round: design blockers → wiring blockers → one mechanical signature fix). Residual items are execution-time (a handful of MINOR non-load-bearing line-ref nits the round-3 critic flagged as non-misleading: `delete_session` fn `:4889`, recall arm return `:1791`, guest-override assert `:1113`, mirror test `delete_session_tombstones_in_fold`) — the kind TDD surfaces on the first `cargo test`. The design and every load-bearing seam are verified. Recommended: build, not a 4th plan-review round. Grep-verify each named fn at execution as a cheap final check.
