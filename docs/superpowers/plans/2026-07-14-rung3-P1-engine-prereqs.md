# Rung 3 — Phase 1: Engine Prerequisites — Implementation Plan (Rev 2, review-folded)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax. **Run every `cargo` command SYNCHRONOUSLY in the foreground** (the standing P0 lesson).

**Goal:** Build the engine prerequisites Phase 2 detection needs — (7.1) a *separate* session-passage conflict index fed by persisted passage vectors, and (7.3) a reversible `retire_memory` primitive at **note + passage** granularity (App-only) — such that recall (rungs 1/2) does not regress, a passage-retire survives a sweeper cycle, `retire_memory` is guest-refused, and `unretire` round-trips **without ever reversing an ordinary edit**.

**Architecture:** State logic lives in `crates/bossclaw-core/src/log.rs` (`EventLog`, append-only + signed + fold-derived). Retire uses **distinct `note_retired`/`passage_retired` marker events** (NOT a bare `supersede` — a supersede is byte-identical to an edit, so reusing it makes `unretire` unable to tell a retire from an edit → silent resurrection; review BLOCKER-1/M1). A distinct marker tracked in its own `retired` set makes `unretire` refusable and safe. Session passages are **chunked + embedded once at capture time by the daemon and persisted** to a new encrypted `session_passage_vectors` table (mirroring the existing `entity_vectors`), so the separate `conflict_index` has a real, restart-surviving data source and core stays filesystem-free (review BLOCKER-2/C1). The recall `vector_index` is **byte-untouched**, making "no recall regression" true by construction.

**Tech Stack:** Rust, `serde`/`serde_json`, encrypted SQLite (`store.rs`), `hnsw_rs` via `VectorIndex`/`HnswIndex` (`index.rs`), `model2vec_rs` embedder, `clap` (memharness), ULID ids, Ed25519 signing (automatic in `append_event_in_tx:955`).

**Spec:** `docs/superpowers/specs/2026-07-12-rung3-conflict-resolution-design.md` (§3, §7, §9, §13, I1/I5/I6/I8). **Branch:** `feat-rung3-conflict-resolution`. **Rev 2** folds the architect+critic plan review (see "Review resolutions" at the end).

**Design decisions folded (owner-ratified 2026-07-14):**
- **D1 — Session retire is PASSAGE-granularity only.** No whole-session retire (no §6 action invokes it; sessions conflict at the passage level per §4c). Notes retire via their own distinct marker. → removes the sweeper `include_candidate` change and the session-level retired-set/`retired_session_ids` entirely.
- **D2 — Passage vectors persist at capture time** in a new `session_passage_vectors` table (mirror `entity_vectors`), written by the daemon capture path; `rebuild_conflict_index` reads that table. Core never reads `.md` bodies.

**Exit gate (spec §3 / §13):** passage index built + queried; **recall-neutrality** (rungs 1/2 unchanged) proven; a passage-retire survives a simulated sweeper cycle; `retire_memory` guest-refused; `unretire` round-trips (and never un-does an edit); passage-vs-title catch rate measured on an honest fixture.

---

## File Structure

**Modified — core (`crates/bossclaw-core/src/`):**
- `graph.rs:37` — consts `NOTE_RETIRED_EVENT_TYPE`, `PASSAGE_RETIRED_EVENT_TYPE`, `UNRETIRE_EVENT_TYPE` (all non-embeddable; do NOT add to `EMBEDDABLE_EVENT_TYPES:345`).
- `log.rs` — note-fold `retired` set + reversal (`fold_notes:7908`, recall memory arm `:1785`, `embed_excluded_event_ids:4784`, `current_notes` query `:4930`, `superseded_event_ids:4761`); `SessionFold:7838` gains `retired_passages`; `fold_sessions:7859` + `session_events_ordered:4822` learn the new types; new `retire_memory`/`unretire`/`retire_passage`/`unretire_passage` primitives; new `session_passage_vectors` table + `store_session_passages`/`session_passages_for_model` (mirror `entity_vectors`/`entity_vectors_for_model:5528`); new `conflict_index` field + `rebuild_conflict_index:mirror(5515)` + `conflict_search:mirror(1420)`; `VectorIndex::len` accessor (`index.rs:45`).
- `index.rs` — port `CHUNK_KEY_SEP`/`encode_chunk_key`/`decode_chunk_key`/`event_id_of` from `origin/feat-retrieval-rung3-chunking`; add `len()` to the `VectorIndex` trait + `HnswIndex`.

**Created — core:** `crates/bossclaw-core/src/chunk.rs` (port; `chunk_text`, `CHUNK_BUDGET_CHARS=600`).

**Modified — proto (`crates/bossclawd-proto/src/lib.rs`):** `Request::RetireMemory{onboarded, target: RetireTarget}` + `Request::Unretire{...}` (`:125`); `enum RetireTarget{ Note{event_id}, Passage{session_id, passage_id} }` (derives `Serialize,Deserialize,Clone,PartialEq,Debug`); `Response::Retired(String)` (`:257`); allowlist tests (`:820`,`:846`). `Role::allows` + `PROTO_VERSION` untouched.

**Modified — daemon (`crates/bossclawd/src/`):** `server.rs:258` dispatch arms; `engine/mod.rs:717` async wrappers (mirror `supersede_note`); `capture/store.rs` capture path chunks+embeds+persists passages; `capture/store.rs` retire path must **NOT** call `delete_capture:182` (it deletes the `.md`).

**Modified — harness (`crates/memharness/src/`):** `main.rs` `conflict-grade --retrieval {title|passage}` mode; `compare.rs` `recall_regressed` helper; new fixture `fixtures/session-conflict-pairs.jsonl`.

---

## Task 1 — Distinct reversible retire markers + fold state (§7.2/§7.3 foundation)

Adds `note_retired`/`passage_retired`/`unretire` events and folds them into a **separate** `retired` set (never the `superseded` set), so `unretire` can only ever reverse a retire — never an edit.

**Files:** `graph.rs:37`; `log.rs:7838`(`SessionFold`), `:7859`(`fold_sessions`), `:7908`(`fold_notes`), `:4822`(`session_events_ordered`), `:4930`(`current_notes`). Test: mirror `superseded_note_excluded_but_replacement_recallable:8232`.

- [ ] **Step 1: consts** (`graph.rs:37`):
```rust
/// Rung-3 retire markers — DISTINCT from `supersede` (which is byte-identical to an edit).
/// A distinct type is what lets `unretire` reverse a retire without ever reversing an edit.
pub const NOTE_RETIRED_EVENT_TYPE: &str = "note_retired";       // content: {"retires": <note_event_id>}
pub const PASSAGE_RETIRED_EVENT_TYPE: &str = "passage_retired"; // content: {"session_id","passage_id"}
pub const UNRETIRE_EVENT_TYPE: &str = "unretire";              // content: {"unretires": <event_id>} OR {"session_id","passage_id"}
```
- [ ] **Step 2: failing fold test** (helpers `open_log:7990`, `MockEmbedder::new(8)`):
```rust
#[test]
fn note_retired_is_reversible_and_never_reverses_an_edit() {
    let log = open_log(); let emb = MockEmbedder::new(8);
    let note = log.external_note_event_committed("uses Vercel");   // existing note-write path; see Step 4 helper note
    let edited = log.supersede_note(&emb, &note, "left Vercel").unwrap();  // an ORDINARY edit
    // retire the (already-edited) note via a DISTINCT note_retired event, then unretire it:
    log.append_note_retired(&edited).unwrap();
    assert!(log.note_fold_retired_for_test().contains(&edited));
    log.append_unretire(&edited).unwrap();
    assert!(!log.note_fold_retired_for_test().contains(&edited));
    // The ORIGINAL edit-supersede is still in `superseded` — unretire never touched it:
    assert!(log.superseded_event_ids().unwrap().contains(&note), "edit-supersede is untouched by unretire");
}
```
- [ ] **Step 3: run → FAIL.**
- [ ] **Step 4: implement fold.**
  - `SessionFold` (`:7838`) — add `retired_passages: HashSet<(String, usize)>`.
  - Add a note-level `retired: HashSet<String>` to `fold_notes`'s return (extend its struct/tuple).
  - `session_events_ordered` (`:4822`) and `current_notes` query (`:4930`) — add `NOTE_RETIRED`, `PASSAGE_RETIRED`, `UNRETIRE` to `events_of_types([...])`.
  - `fold_notes` (`:7908`) + `fold_sessions` (`:7859`) — process events in `seq ASC`: `NOTE_RETIRED` → `retired.insert(retires)`; `PASSAGE_RETIRED` → `retired_passages.insert((sid,pid))`; `UNRETIRE` → remove the matching id/pair. **Do not touch the `superseded` set** — retire and supersede are now disjoint universes.
- [ ] **Step 5: run → PASS.** **Step 6: commit** `feat(rung3-p1): distinct reversible retire markers (note/passage), disjoint from supersede`.

---

## Task 2 — `retire_memory`(note) + `unretire` core primitives (§7.3)

**Files:** `log.rs` new `retire_memory`/`unretire` (mirror `delete_session:4889` for the append shape, `supersede_note:4720` for validation) + `assert_retirable_note`/`assert_note_retired` helpers; recall memory arm `:1785`; `embed_excluded_event_ids:4784`; `superseded_event_ids:4761` unchanged (retire is not a supersede). Test: mirror `deleted_session_absent_from_recall_even_by_keyword:8206`.

- [ ] **Step 1: failing test — note retire drops from recall; unretire restores; refuses non-retired.**
```rust
#[test]
fn retire_memory_note_excludes_from_recall_and_unretire_round_trips() {
    let log = open_log(); let emb = MockEmbedder::new(8);
    let ev = log.external_note_event_committed("we deploy on Vercel");
    log.rebuild_indexes(&emb).unwrap();
    assert!(recall_contains(&log, &emb, "Vercel", &ev));
    log.retire_memory(&ev).unwrap();
    assert!(!recall_contains(&log, &emb, "Vercel", &ev), "retired note excluded");
    assert!(matches!(log.unretire("not-retired-id"), Err(BossclawError::InvalidInput(_))), "unretire refuses a non-retired id");
    log.unretire(&ev).unwrap();
    assert!(recall_contains(&log, &emb, "Vercel", &ev), "unretire restores");
    assert!(matches!(log.retire_memory("nope"), Err(BossclawError::InvalidInput(_))));
}
```
(Add the thin test helpers `external_note_event_committed` = append `external_note_event:4686` via `append`; `recall_contains` = `recall(...).any(event_id==)`. These are test-module helpers — add them explicitly, do not assume they exist.)
- [ ] **Step 2: run → FAIL.**
- [ ] **Step 3: implement** (distinct marker, NOT supersede; free-fn calls are free-fn, not `self.`):
```rust
/// Retire a memory-kind note (rung-3 "Retire older"): append a DISTINCT `note_retired` marker.
/// Reversible via `unretire`; App-only (guest-refused at proto). No replacement, no vector.
pub fn retire_memory(&self, target_event_id: &str) -> Result<String, BossclawError> {
    self.assert_retirable_note(target_event_id)?;   // exists, memory-kind, not already superseded/retired
    let ev = event_of(NOTE_RETIRED_EVENT_TYPE, serde_json::json!({ "retires": target_event_id }));
    self.append(ev)                                 // auto hash+sign (append_event_in_tx:955)
}
pub fn unretire(&self, retired_event_id: &str) -> Result<String, BossclawError> {
    self.assert_note_retired(retired_event_id)?;    // must be in fold_notes().retired — refuses anything else
    let ev = event_of(UNRETIRE_EVENT_TYPE, serde_json::json!({ "unretires": retired_event_id }));
    self.append(ev)
}
```
  - Recall memory arm (`:1785`) — change to `return !superseded_ids.contains(&h.event_id) && !retired_ids.contains(&h.event_id);` (both sets from the single `fold` at `:1642`; add `retired_ids`).
  - `embed_excluded_event_ids` (`:4784`) — union in the note-retired ids so a retired note isn't re-vectorized on migration.
- [ ] **Step 4: run → PASS**, then `cargo test -p bossclaw-core` (confirms no supersede/delete regression). **Step 5: commit** `feat(rung3-p1): retire_memory + unretire note primitives (distinct marker, recall-excluded, guarded reversal)`.

---

## Task 3 — Proto op + allowlist + server + engine wrapper (§7.3, guest-refused)

**Files:** `proto/lib.rs:125`(Request), `:257`(Response), `:820`/`:846`(tests); `server.rs:258`(dispatch), `:1055`(override test); `engine/mod.rs:717`(wrappers).

- [ ] **Step 1: failing allowlist/serde tests.** Extend `memory_client_allows_exactly_four_ops:820` — add `RetireMemory`/`Unretire` to the `no=[...]` asserting `!MemoryClient.allows`; add both to `new_variants_round_trip_serde:846`; add a `server.rs` assertion that `override_onboarding_for_guest(RetireMemory{..})` is `None` (`:1055`).
- [ ] **Step 2: run → FAIL** (variants missing → won't compile).
- [ ] **Step 3: add variants + Response::Retired + RetireTarget** (derives as listed). `Role::allows` untouched (auto-refused); `PROTO_VERSION` stays 1.
- [ ] **Step 4: dispatch + wrappers.** In `dispatch` (`:258`, exhaustive):
```rust
Request::RetireMemory { target, .. } => match target {
    RetireTarget::Note { event_id } => op_result(engine.retire_memory(event_id).await, Response::Retired),
    RetireTarget::Passage { .. } => not_permitted_or_rejected("passage retire lands in Task 7"), // Rejected, keeps wire shape
},
Request::Unretire { retired_event_id, .. } => op_result(engine.unretire(retired_event_id).await, Response::Retired),
```
Add `EngineHandle::retire_memory`/`unretire` (mirror `supersede_note:717`: `is_onboarded_local`, `spawn_blocking`, `InvalidInput→Rejected`; note retire needs no index rebuild, so **do not** force `indexed=false`).
- [ ] **Step 5: run `cargo test -p bossclawd-proto -p bossclawd` → PASS. Step 6: commit** `feat(rung3-p1): retire_memory/unretire proto op — App-only, guest-refused, server-wired`.

---

## Task 4 — Port `chunk.rs` + composite key helpers + `VectorIndex::len`

- [ ] **Step 1: port.** `git show origin/feat-retrieval-rung3-chunking:crates/bossclaw-core/src/chunk.rs > crates/bossclaw-core/src/chunk.rs`; add `mod chunk; pub use chunk::chunk_text;` to `lib.rs`; copy `CHUNK_KEY_SEP`/`encode_chunk_key`/`decode_chunk_key`/`event_id_of` (+ tests) into `index.rs`.
- [ ] **Step 2: add `len()`** to the `VectorIndex` trait (`index.rs:45`) + `HnswIndex` (return element count) — needed for the recall-untouched assertion in Task 6.
- [ ] **Step 3: run `cargo test -p bossclaw-core --lib chunk index` → PASS** (ported tests unchanged). **Step 4: commit** `feat(rung3-p1): port chunk_text + composite key helpers + VectorIndex::len`.

---

## Task 5 — Persist session passages at capture (§7.1 data source, D2)

Core gets a `session_passage_vectors` table + store/read (mirror `entity_vectors`/`entity_vectors_for_model:5528`); the **daemon** capture path chunks the body, embeds, and persists — so bodies never enter core.

**Files:** `log.rs` (table DDL beside the `entity_vectors` migration, `store_session_passages`, `session_passages_for_model`); `crates/bossclawd/src/capture/store.rs` (in the capture path, after `read_capture_markdown:99`, `chunk_text` → `embed` → `engine.store_session_passages(session_captured_event_id, &chunks, &vecs)`); `engine/mod.rs` wrapper.

- [ ] **Step 1: failing test — capturing a 2-passage body persists 2 rows, readable after reopen.**
```rust
#[test]
fn capture_persists_session_passage_vectors_surviving_reopen() {
    let dir = tempdir(); let emb = MockEmbedder::new(8);
    { let log = open_log_at(&dir);
      log.store_session_passages("cap1", &["we deploy on Vercel".into(), "db is Postgres".into()],
                                  &emb.embed(&["we deploy on Vercel".into(),"db is Postgres".into()]).unwrap()).unwrap(); }
    let log = open_log_at(&dir);   // reopen → rows must survive
    let rows = log.session_passages_for_model(emb.model_id()).unwrap();
    assert_eq!(rows.iter().filter(|r| r.event_id == "cap1").count(), 2);
}
```
- [ ] **Step 2: run → FAIL. Step 3: implement** the table (`session_passage_vectors(session_captured_event_id TEXT, passage_ix INTEGER, model_id TEXT, embedding BLOB, PRIMARY KEY(session_captured_event_id, passage_ix, model_id))`) + store/read (LE-f32 BLOB like `vectors`), and the daemon capture wiring. **Step 4: run → PASS. Step 5: commit** `feat(rung3-p1): persist session-passage vectors at capture (separate table, restart-safe)`.

---

## Task 6 — Separate `conflict_index` + passage retrieval (§7.1)

**Files:** `log.rs:429-457`(field, init `None` like `entity_index:445`), `rebuild_conflict_index`(mirror `rebuild_entity_index:5515`), `conflict_search`(mirror `vector_search:1420`).

- [ ] **Step 1: failing test.**
```rust
#[test]
fn conflict_index_retrieves_passages_and_leaves_recall_len_unchanged() {
    let log = open_log(); let emb = MockEmbedder::new(8);
    log.store_session_passages("cap1", &["we deploy on Vercel".into(),"db is Postgres".into()], &two_vecs(&emb)).unwrap();
    log.rebuild_indexes(&emb).unwrap();
    let recall_len = log.vector_index_len();                  // Task 4 accessor
    log.rebuild_conflict_index(&emb).unwrap();
    let hits = log.conflict_search(&emb.embed(&["Vercel".into()]).unwrap()[0], 8); // k ≥ total chunks → membership stable (HNSW)
    assert!(hits.iter().any(|(sid, pid, _)| sid == "s1" && *pid == 0));            // session id resolved from cap1's fold head
    assert_eq!(log.vector_index_len(), recall_len, "recall vector_index byte-untouched");
}
```
- [ ] **Step 2: run → FAIL. Step 3: implement.** `rebuild_conflict_index`: for each **current, non-retired-passage** session (map `fold_sessions().current` event_id→session_id), read its rows via `session_passages_for_model`, skip `(session_id, ix) ∈ retired_passages`, `add(encode_chunk_key(session_id, ix), vec)` into a fresh `HnswIndex`. `conflict_search` decodes keys back to `(session_id, passage_id, score)`. **Do not touch `rebuild_indexes`/`vector_search`/`EMBEDDABLE_EVENT_TYPES`** (recall-neutrality by construction). **HNSW hermeticity (spec §13):** assert set-membership, keep `k ≥ index size`, never assert rank order.
- [ ] **Step 4: run → PASS. Step 5: commit** `feat(rung3-p1): separate session-passage conflict index + passage retrieval (recall untouched)`.

---

## Task 7 — Passage-level retire + sweeper-cycle durability (§7.2/§7.3)

`retire_memory` on a `RetireTarget::Passage` hides one `(session_id, passage_id)` from the conflict index without dropping the session; it survives a re-capture + rebuild; `unretire` restores it.

**Files:** `log.rs` `retire_passage`/`unretire_passage` (emit `PASSAGE_RETIRED`/`UNRETIRE`); `rebuild_conflict_index` exclusion (Task 6 already reads `retired_passages`); `server.rs` real Passage dispatch arm + `engine/mod.rs` wrapper.

- [ ] **Step 1: failing test — hide one passage, keep siblings, survive a sweep, reverse.**
```rust
#[test]
fn passage_retire_hides_one_survives_sweep_and_reverses() {
    let log = open_log(); let emb = MockEmbedder::new(8);
    log.store_session_passages("cap1", &["Vercel".into(),"Postgres".into()], &two_vecs(&emb)).unwrap();
    log.rebuild_conflict_index(&emb).unwrap();
    log.retire_passage("s1", 0).unwrap();
    log.rebuild_conflict_index(&emb).unwrap();
    assert!(!has_hit(&log,&emb,"Vercel","s1",0) && has_hit(&log,&emb,"Postgres","s1",1), "one hidden, sibling kept");
    // SWEEP durability: a same-sha re-capture is a no-op; the passage marker persists across a rebuild.
    log.capture_session(&emb, &session_meta("s1","aa")).unwrap();  // same sha → dedup no-op
    log.rebuild_conflict_index(&emb).unwrap();
    assert!(!has_hit(&log,&emb,"Vercel","s1",0), "retire survives a sweeper cycle");
    log.unretire_passage("s1", 0).unwrap(); log.rebuild_conflict_index(&emb).unwrap();
    assert!(has_hit(&log,&emb,"Vercel","s1",0), "unretire restores the passage");
}
```
- [ ] **Step 2: run → FAIL. Step 3: implement** `retire_passage`/`unretire_passage` (validate the passage exists in the current body; emit the markers) + the real `server.rs` Passage dispatch arm + `engine/mod.rs` wrapper. **Note the known limitation** (do NOT fix in Phase 1): `passage_id` is the chunk ordinal; a *changed-sha* re-capture re-chunks and shifts ordinals, so a marker could mis-target after a body edit. Harmless for the Phase-1 primitive (no Retire *action* yet); when the action ships (Phase 3), key the marker on a chunk content-hash. **Step 4: run → PASS. Step 5: commit** `feat(rung3-p1): passage-granular retire — one passage hidden, siblings intact, survives sweep, reversible`.

---

## Task 8 — Harness: recall-neutrality + honest passage-vs-title (§9/§13 exit gate)

**Files:** `compare.rs` `recall_regressed`; `main.rs` `conflict-grade --retrieval {title|passage}`; new fixture.

- [ ] **Step 1: recall-neutrality — enforce by construction, not just runbook.** Add a `bossclaw-core` test asserting the recall paths are literally untouched: e.g. a golden test that a note-recall over a corpus is byte-identical before/after a session passage is indexed/retired (proves the conflict index can't perturb recall). Plus `recall_regressed(&[SegmentComparison]) -> Option<&SegmentComparison>` in `compare.rs` (first gating segment with a significant negative s@k delta) + its unit test. **Runbook (owner-gated, needs the frozen corpus):** freeze once → run pre/post-Phase-1 binaries → `memharness compare` → assert `recall_regressed == None`.
- [ ] **Step 2: honest passage-vs-title fixture + non-tautological assertion.** `fixtures/session-conflict-pairs.jsonl` schema: `{"session_id","title","body","conflicting_query","label":"contradicts|coexist"}` where the conflict genuinely lives in `body` (title is generic) — **include coexist hard negatives** so passage retrieval must also NOT over-surface. Assert an ABSOLUTE bar, not a bare `>`:
```rust
#[test]
fn passage_index_meaningfully_beats_title_only() {
    let f = load_session_pairs(include_str!("../fixtures/session-conflict-pairs.jsonl"));
    let title = grade_retrieval(&f, Retrieval::TitleOnly);
    let passage = grade_retrieval(&f, Retrieval::PassageIndex);
    assert!(title.recall < 0.30, "title-only genuinely misses body conflicts: {}", title.recall);
    assert!(passage.recall >= 0.70, "passage index actually catches them: {}", passage.recall);
    assert!(passage.precision >= title.precision, "passage index does not over-surface (hard negatives)");
}
```
`flagged = did the retrieval front-end surface the conflicting passage as a candidate` (judge-free — the Phase-2 judge is deliberately out of this measurement). Define `load_session_pairs`/`grade_retrieval`/`Retrieval` in the harness.
- [ ] **Step 3: run → FAIL, implement, run → PASS. Step 4: live gate (owner-gated, not CI)** — record numbers in this plan's RESULTS section. **Step 5: commit** `feat(rung3-p1): harness recall-neutrality guard + honest passage-vs-title catch-rate`.

---

## Task 9 (OPTIONAL — defer to Phase 3) — §7.4 edge-invalidation
Stamp `Edge.invalidated_at`/`invalidated_by` (already present, `graph.rs:147/149`) inside a confirmed retire when a retired note maps to a derived edge. The Retire *action* is Phase 3, so this naturally lands there; the Phase-1 primitive works without it. Leave a one-line TODO reference in `retire_memory`.

---

## Downstream / product-direction note (owner, 2026-07-14)
Per [[air/vision-background-first-claude-code-native-2026-07-14]]: AIR Agent should be **background-first, reachable from Claude Code**, not a destination app. Phase 1 here is pure background engine (interface-agnostic) — fully aligned. **Implication for Phase 3 (Resolution + UI):** surface the conflict + draft the retire *through the Claude Code session*, NOT only as a desktop card. Keep the confirm on a trusted surface (resolution stays App-only/guest-refused, I1/I8 — the MCP channel must not be able to delete memories), but the *notification and drafting* should reach the user where they work. Do not over-invest in desktop UI polish.

## Global gates (before every commit)
```bash
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p bossclaw-core -p bossclawd-proto -p bossclawd -p memharness
```
Invariants: I1 (no auto-retire — every retire is an explicit op), I5 (append-only+signed automatically via `append_event_in_tx:955`; retire markers distinct from supersede so honesty is preserved), I6 (double-retire refused, fold-rebuild self-heals), I8 (allowlist gains nothing). Never reuse `delete_capture:182` (deletes the `.md`).

## Review resolutions (architect + critic, 2026-07-14)
- **BLOCKER retire-incoherence / M1 unretire-reverses-edit** → distinct `note_retired`/`passage_retired` markers in a separate `retired` set (Tasks 1-2); `unretire` refuses non-retired ids; `superseded_event_ids` untouched. Whole-session retire dropped (dead scaffolding) → **sweeper task removed** (D1).
- **BLOCKER Task-6 body seam / C1** → passages persisted at capture in `session_passage_vectors` (Task 5, D2); core stays filesystem-free; index survives restart.
- **MAJOR compile-order** → Task 3 Passage arm returns Rejected; real arm + wrapper in Task 7.
- **M2 invented helpers** → each test helper is now an explicit "add this helper" step; free-fn calls corrected (`event_of`/`external_note_event` are free fns); `VectorIndex::len` added in Task 4.
- **M3 rigged benchmark** → honest fixture (real body-conflicts + coexist hard-negatives) + absolute-floor assertion + defined schema (Task 8).
- **Minors** → HNSW membership-not-rank + `k ≥ size` (Tasks 6-8); passage-id ordinal fragility documented (Task 7); recall-neutrality enforced by a byte-unchanged golden test, not only a runbook (Task 8); `RetireTarget` derives + `Response::Retired` pinned (Task 3).

## Self-review (spec coverage)
§7.1 → Tasks 4,5,6. §7.2 (sweeper-safe, passage granularity) → Tasks 1,7. §7.3 (retire_memory + proto + allowlist + unretire) → Tasks 2,3,7. §7.4 → Task 9 (Phase 3). §9/§13 (passage-vs-title, recall-neutrality, guest-refused, unretire round-trip, survives sweep) → Tasks 8,3,2,7. I8 → Task 3. Exit gate fully covered; every helper the tests use is created by a step; no forward references compile-break.
