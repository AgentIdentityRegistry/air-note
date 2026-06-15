# bossclaw-core — Milestone 2 (Recall) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax. Build TDD: failing test first, then implementation.

**Goal:** Give the M1 event log a **finder** — turn the signed, encrypted log into searchable memory: semantic recall (embeddings → `hnsw_rs`), keyword recall (FTS5), a hybrid that fuses both, and a no-op-default reranker slot. Everything stays **Tier-A: rebuildable from the encrypted log, with zero plaintext index on disk.**

**Architecture:** All recall structures are **Tier-A derived** (spec §4) — pure functions of the M1 event log under a *fixed active embedding model*. The vector index is held **in memory and rebuilt on open from the log** (proven no-plaintext; see Spike results). FTS5 lives **inside the SQLCipher DB** (whole-file encrypted; proven no-plaintext). `EventLog` (M1) **owns** the recall subsystem (embedder + indexes) and exposes the public recall API; the `Embedder`/`VectorIndex`/`Reranker` are swap-traits but the SQL-touching wiring stays **inside the crate** (the seam fix, Finding 3). The pipeline embeds the query, runs vector + keyword search, fuses with Reciprocal Rank Fusion, applies recency/pin boosts, returns top-N with provenance.

**Tech Stack (spike-confirmed):** Rust 2021 · `hnsw_rs` 0.3.4 (pure-Rust ANN) · `model2vec-rs` 0.2.1 (pure-Rust embedder, **default**) · `fastembed` 5.16.2 (`bge-small`, **opt-in** behind a `fastembed` cargo feature) · FTS5 via the existing `rusqlite` `bundled-sqlcipher` · `anndists` (distances; re-exported by `hnsw_rs::prelude`). Builds on M1's `EventLog`/`Store`/`Event`.

**Spec:** `docs/superpowers/specs/2026-06-15-bossclaw-core-design.md` (Rev 3). Implements §12 Milestone 2 + §5.3 Embedder, §5.4 VectorIndex, §5.5 keyword, §5.7 recall+reranker, §11 recall@k + no-plaintext tests, §15 re-embed migration budget.

## Revision log
- **Rev 2 (2026-06-15):** folded an independent critic review (verdict SHIP-WITH-FIXES). All 3 Criticals + 4 Majors + minors/gaps folded — see the inline `[critic Fn]` tags. Key: the M1 `config` event type was claimed-but-absent (now defined as a convention); the cross-crate `Store`-visibility seam (recall API moves onto `EventLog`); the flaky exact-top-k rebuild assertion (now top-1 + set-equality with a mandated `ORDER BY`).
- **Rev 1 (2026-06-15):** initial plan from the spec §12-M2 + the resolved spikes.

---

## Spike results — the M2 go/no-go gate (RESOLVED 2026-06-15: **GO**)

Both gating spikes from spec §14 passed empirically *before* this plan:

1. **Encryption — no plaintext index on disk (§8.1):** **PASS.** FTS5 inside SQLCipher leaks **0** plaintext marker tokens to disk; header encrypted; wrong key rejected (`temp_store=MEMORY` stops plaintext temp spill). The vector index is Tier-A: rebuilding `hnsw` from the same source vectors is **deterministic** (identical top-5 across rebuilds, serial insert) and finds the query point as top-1. So v1 **rebuilds the index in memory on open from the encrypted log → ZERO plaintext index file on disk, by construction.**
2. **ort offline bundling (§14.2):** **GO with refinement.** `fastembed`/ort compiles on macOS, but ORT's native lib is **fetched at runtime** → a true offline single binary needs explicit per-platform bundling (residual cross-platform risk). `model2vec-rs` is **pure-Rust** (no ort), supports `from_static_slices` (embed model in the binary) and `from_pretrained(local_dir)` → a guaranteed offline, zero-native-lib path exists today.

### Refinements adopted (deviations from the spec's default assumptions — blessed by the plan review)
- **R1 — Embedder default = `model2vec` (pure-Rust), not `fastembed`.** `fastembed`/`bge-small` ships behind a `fastembed` cargo feature as the opt-in quality upgrade. **The §11 recall@k fixture + a per-platform offline-bundling CI check make the final default call at the end of M2** (spec §3.3: "the recall fixture is the empirical tiebreak").
- **R2 — Vector index persistence = rebuild-in-memory-on-open** for v1 (no persisted sidecar). The spec's "encrypted sidecar" becomes a post-v1 startup optimization, gated on the Task-9 rebuild-time budget. **Scope note (critic + T5 finding):** for the in-memory hnsw graph, "Tier-A rebuildable" means **behaviorally-equivalent recall** (top-1 identity + recall stability — a known item stays in top-k across rebuilds), NOT top-k set/order identity and NOT the literal byte-identity spec §4/§11 (spec:260) requires of *persisted* Tier-A tables. **Measured in T5:** `hnsw_rs` 0.3.4 reseeds level-assignment from OS randomness on every `Hnsw::new` (no seed API) — over 12 rebuilds top-1 was 12/12 identical but the deep-rank set varied (~half), so the index is rebuild-stable for *which* memory is most relevant, not for deep-rank order. Cross-session rank determinism is the deferred encrypted-sidecar's job. The `vectors` TABLE itself IS byte-deterministic under a fixed model; only the un-persisted hnsw graph relaxes to behavioral equivalence. Task 5 also prints a rebuild-time line so the startup cost is visible before Task 9, not just at the end.
- **R3 — FTS5 stays inside SQLCipher** (proven no-plaintext; transactional with the log).

---

## File structure

| File | Responsibility |
|---|---|
| `crates/bossclaw-core/Cargo.toml` | add `hnsw_rs`, `model2vec-rs`, `anndists`; `fastembed` under `[features] fastembed = ["dep:fastembed"]` |
| `crates/bossclaw-core/src/lib.rs` | add `embed`, `index`, `keyword`, `recall` module decls + re-exports |
| `crates/bossclaw-core/src/embed.rs` | `Embedder` trait + `Model2Vec` (default) + `FastEmbed` (feature-gated) + `MockEmbedder` (test) |
| `crates/bossclaw-core/src/index.rs` | `VectorIndex` trait (PURE: operates on `(event_id, vec)` — no `Store`) + `HnswIndex` |
| `crates/bossclaw-core/src/keyword.rs` | FTS5 keyword index helpers (run by `EventLog`, inside the SQLCipher store) + the `fts_map` rowid↔event_id table |
| `crates/bossclaw-core/src/recall.rs` | `Hit` result + `Reranker` trait (no-op default) + RRF fuse + recency/pin boosts |
| `crates/bossclaw-core/src/log.rs` | (extend) **owns** recall: `open_with_recall`, `derive_vector`, `rederive_pending`, `rebuild_indexes`, `recall`, `reembed_migration`, `verify_chain_since`; the `config` convention + `active_model_id()` |
| `crates/bossclaw-core/tests/recall.rs` | pipeline integration tests via `EventLog` (MockEmbedder, hermetic) |
| `crates/bossclaw-core/tests/no_plaintext.rs` | promote the FTS5 no-plaintext-on-disk security test |
| `crates/bossclaw-core/tests/fixtures/recall.json` | labelled recall@k corpus (created in Task 10) |
| `crates/bossclaw-core/tests/recall_fixture.rs` | `#[ignore]` recall@k fixture (real models; the embedder-default gate) |

M3 graph, M4 reasoner/evolve, M5 ingest, M6 actuator, M7 desktop are **not** in M2. The recall pipeline's **graph-proximity boost is deferred to M3** (M2 boosts = recency + pin only).

---

## Task 1: `config` event convention + active-model lookup + Embedder trait + MockEmbedder

**Files:** create `src/embed.rs`; modify `src/lib.rs`, `src/log.rs`; test in `tests/recall.rs`.

**[critic F1] M1 does NOT have a `config` event type** — `Event.event_type` is a free `String` and nothing recognizes `"config"`. M2 introduces the convention here.

- [ ] **Step 1 — define the `config` convention** (`src/log.rs` doc + helper): a config event is `append`ed with `event_type = "config"` and `content = {"active_model_id": String, "dim": u32, "schema_version": u32}`. Add `EventLog::active_model() -> Result<Option<ActiveModel>>` = the **latest** row `WHERE event_type='config' ORDER BY seq DESC LIMIT 1`, parsed from `content`. `schema_version` rides here (see "schema_version" note below); format-gating logic is deferred, the field is reserved now so it need not be retrofitted.
- [ ] **Step 2 — failing test** (`tests/recall.rs`): append two `config` events (model A then model B); `active_model()` returns B's `active_model_id`/`dim`. Append none → `Ok(None)`.
- [ ] **Step 3 — Embedder trait + mock:**
```rust
/// Text → vector. Exactly one ACTIVE model per store (the latest `config` event).
/// Vectors are only ever compared within one model_id (§5.4 / fix C4).
pub trait Embedder: Send + Sync {
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, BossclawError>;
    fn dim(&self) -> usize;
    fn model_id(&self) -> &str;
}
/// Deterministic, dependency-free embedder for hermetic tests (hashes tokens into
/// a fixed-dim L2-normalized bag-of-words vector). NOT production recall quality.
pub struct MockEmbedder { dim: usize }
```
MockEmbedder MUST L2-normalize (so `DistCosine` math is consistent with the real embedders) and be deterministic per input.
- [ ] **Step 4** run (PASS). **Step 5** commit `feat(bossclaw-core): config-event convention + active-model lookup + Embedder trait + MockEmbedder`.

## Task 2: Model2Vec embedder (pure-Rust DEFAULT)

**Files:** modify `src/embed.rs`, `Cargo.toml`.

- [ ] **Step 1** `cargo add model2vec-rs@0.2.1`. API: `model2vec_rs::model::StaticModel` with `from_pretrained<P: AsRef<Path>>(repo_or_path, token, normalize, subfolder)`, `from_static_slices(...)`, `encode(&[String]) -> Vec<Vec<f32>>`.
- [ ] **Step 2 — failing test** (`#[ignore]`, needs a model dir): `Model2Vec::from_pretrained(dir)`, embed two sentences, assert non-zero `dim()` and a near-paraphrase is closer (cosine) than an unrelated sentence.
- [ ] **Step 3 — implement** `Model2Vec { inner: StaticModel, model_id, dim }`. **Pass `normalize = Some(true)`** so output is unit-norm (DistCosine consistency; the stored `vectors` BLOB is the normalized vector — [critic open-Q]). Provide BOTH `from_pretrained(&Path)` and `from_static_slices(...)`; the `include_bytes!` bundling decision itself is desktop M7.
- [ ] **Step 4** run with a locally-cached `potion-base-8M` (document the fetch in a comment; test is `#[ignore]` so the hermetic suite never hits the network). **Step 5** commit `feat(bossclaw-core): Model2Vec pure-Rust embedder (default)`.

## Task 3: FastEmbed embedder (OPT-IN, feature `fastembed`)

**Files:** modify `src/embed.rs`, `Cargo.toml`.

- [ ] **Step 1** `Cargo.toml`: `fastembed = { version = "5.16", optional = true }` + `[features] fastembed = ["dep:fastembed"]`. Entire `FastEmbed` impl is `#[cfg(feature = "fastembed")]`.
- [ ] **Step 2** implement `FastEmbed` over `fastembed::TextEmbedding` with the bge-small model. **[critic F8] the enum variant name (`EmbeddingModel::BGESmallENV15`) and init API differ across fastembed minors — VERIFY against the vendored 5.16 source/docs before coding** (`TextEmbedding::try_new(InitOptions::new(model))` then `.embed(docs, None)`). `model_id()` = `"bge-small-en-v1.5"`. Ensure embeddings are L2-normalized (bge default) for DistCosine consistency.
- [ ] **Step 3 — failing test** `#[cfg(feature="fastembed")] #[ignore]` (downloads ONNX once): paraphrase-closer assertion.
- [ ] **Step 4** verify `cargo build -p bossclaw-core` pulls **no** ort; `--features fastembed` does. **Step 5** commit `feat(bossclaw-core): FastEmbed bge-small embedder behind opt-in feature`.

## Task 4: vectors table (Tier-A) + derive/backfill ON EventLog

**Files:** modify `src/log.rs`; test in `tests/recall.rs`. **[critic F3] all SQL-touching recall code lives on `EventLog` (same crate, can use the `pub(crate)` conn). Tests drive `EventLog`, never `Store` directly.**

- [ ] **Step 1 — failing test:** append N `memory` events; `EventLog::derive_pending(embedder)` (or derive-on-append) fills the `vectors` table with N rows for the active `model_id`; a vector under a *different* model_id is never returned by active-model reads (§5.4/C4).
- [ ] **Step 2 — schema** (encrypted store): `CREATE TABLE IF NOT EXISTS vectors (event_id TEXT NOT NULL, model_id TEXT NOT NULL, dim INTEGER NOT NULL, embedding BLOB NOT NULL, PRIMARY KEY(event_id, model_id))`. Embedding BLOB = little-endian f32 bytes.
- [ ] **Step 3 — derive + backfill on EventLog:**
  - `derive_vector(&self, embedder, event)` — embeds the event's text (`content["text"]` for `memory`, summary text for `page`), upserts the row. **[critic open-Q] invoke this AFTER `append` commits, re-acquiring the write path** (do NOT nest inside M1's `unchecked_transaction` critical section). **Best-effort:** on embed error, log + skip (do not fail the append) — §10 keyword-only degrade.
  - **[critic F6] real retry mechanism:** `rederive_pending(&self, embedder)` runs `SELECT e.id ... FROM events e LEFT JOIN vectors v ON v.event_id=e.id AND v.model_id=? WHERE v.event_id IS NULL AND e.event_type IN ('memory','page') ORDER BY e.seq ASC` and embeds the gap. This is the §10 "retryable from the log" hook.
- [ ] **Step 4 — best-effort test [critic F6]:** with a MockEmbedder that returns `Err`, append succeeds and recall degrades to keyword-only; then `rederive_pending` with a working embedder backfills and the vector appears.
- [ ] **Step 5** run (PASS). **Step 6** commit `feat(bossclaw-core): Tier-A vectors table + derive/backfill (best-effort, active-model filtered)`.

## Task 5: VectorIndex trait (pure) + HnswIndex, rebuilt by EventLog

**Files:** create `src/index.rs`; modify `src/log.rs`; test in `tests/recall.rs`.

- [ ] **Step 1 — failing tests:** (a) `HnswIndex` add/search returns nearest event_ids; (b) **[critic F2 / T5 finding]** after a fresh `EventLog::open(...)` + `rebuild_indexes(embedder)`, recall reproduces **top-1 identity** (use a query EQUAL to an inserted vector, distance ~0, so it can't flake) **AND recall stability** (a known item stays in top-k across 3 rebuilds) — NOT top-k set/order identity (hnsw_rs reseeds from OS randomness each build); (c) **[critic F4]** `remove(event_id)` drops it from results; (d) **[critic gap]** feeding mixed-model rows, only active-model vectors are indexed (C4 at the index layer).
- [ ] **Step 2 — pure trait (no Store):**
```rust
/// In-memory ANN over the ACTIVE model's vectors. NOT persisted in v1 — rebuilt
/// from the encrypted log on open (proven: zero plaintext index on disk).
pub trait VectorIndex: Send + Sync {
    fn add(&mut self, event_id: &str, vec: &[f32]);
    fn search(&self, vec: &[f32], k: usize) -> Vec<(String, f32)>; // (event_id, distance)
    fn remove(&mut self, event_id: &str);                          // §5.4 — tombstone+filter (hnsw_rs has no cheap delete; full drop on next rebuild)
    fn last_indexed(&self) -> Option<String>;                      // last event_id folded in
}
pub struct HnswIndex { /* hnsw + id<->slot BiMap + tombstones + last_indexed */ }
```
`Hnsw::<f32, DistCosine>::new(16, max_elems, 16, 200, DistCosine{})`; map `event_id` (String) ↔ slot (usize); `remove` = mark the slot tombstoned and filter it out of `search` results (orphan node reclaimed on the next full rebuild); embeddings are unit-norm so cosine is meaningful.
- [ ] **Step 3 — `EventLog::rebuild_indexes(&self, embedder)`:** reads `SELECT event_id, embedding FROM vectors WHERE model_id=? **ORDER BY event_id ASC**` (**[critic F2] pinned deterministic order**) and **serially** `add`s each into a fresh `HnswIndex`; also rebuilds the FTS (Task 6). **Print a rebuild-time line** (events/sec) so startup cost is visible now, not only at Task 9.
- [ ] **Step 4** run (PASS). **Step 5** commit `feat(bossclaw-core): pure VectorIndex trait + in-memory HnswIndex (rebuild-on-open, active-model-filtered)`.

## Task 6: FTS5 keyword index + fts_map + the no-plaintext security test

**Files:** create `src/keyword.rs`; modify `src/log.rs`; create `tests/no_plaintext.rs`.

- [ ] **Step 1 — failing security test** (`tests/no_plaintext.rs`): write events with a unique marker into FTS5 inside the SQLCipher store; after close, scan every on-disk file (`.db`, `-wal`, `-shm`) → assert **0** marker bytes + non-plaintext header; reopen with key → MATCH finds it; wrong key → error. (Promotes the proven spike into a permanent §11 regression guard.)
- [ ] **Step 2 — implement [critic F5]:** `CREATE VIRTUAL TABLE IF NOT EXISTS fts USING fts5(body, content='')` (contentless — the log is the content of record) + a side table `CREATE TABLE IF NOT EXISTS fts_map (rowid INTEGER PRIMARY KEY, event_id TEXT NOT NULL UNIQUE)`. **Drop the "unindexed column" option — `content=''` forbids readable stored columns.** `PRAGMA temp_store = MEMORY` on open. `add(event_id, text)` inserts into `fts` and records `(fts.rowid, event_id)` in `fts_map`; `search(query, k)` runs `MATCH` and maps `rowid → event_id` via `fts_map`. Rebuild repopulates BOTH `fts` and `fts_map` in `ORDER BY seq ASC`.
- [ ] **Step 3** run (PASS). **Step 4** commit `feat(bossclaw-core): FTS5 keyword index + fts_map + no-plaintext-on-disk test`.

## Task 7: Hybrid recall pipeline (RRF + recency/pin boosts)

**Files:** create `src/recall.rs`; modify `src/log.rs`; test in `tests/recall.rs`.

- [ ] **Step 1 — failing test:** MockEmbedder + a small seeded corpus; assert the relevant event ranks top; a keyword-only match AND a semantic-only match both surface (fusion uses both arms); a more-recent event tie-breaks above an identical older one; a pinned id is boosted above an equal non-pinned one.
- [ ] **Step 2 — implement** `EventLog::recall(&self, query, k, opts) -> Vec<Hit>`:
```rust
pub struct Hit { pub event_id: String, pub score: f32, pub source: Vec<String> /* evidence */ }
pub trait Reranker: Send + Sync { fn rerank(&self, query: &str, hits: Vec<Hit>) -> Vec<Hit>; }
pub struct NoopReranker; // v1 default — identity (§5.7/I2)
```
Recipe: embed query → `vector.search(qv, k)` + `keyword.search(q, k)` → **Reciprocal Rank Fusion**: `fused(d) = Σ 1/(RRF_K + rank_i(d))`, `const RRF_K: f32 = 60.0;` (Cormack et al. default — documented). **[critic F11] boosts scaled to the RRF magnitude (~1/60):** recency is **multiplicative** `fused *= 1 + RECENCY_WEIGHT * exp(-age_secs / HALF_LIFE_SECS)`; pin is **multiplicative** `fused *= PIN_MULTIPLIER` for ids in `opts.pinned` — all named consts with sourced comments, none additive against the tiny RRF scale. Then `reranker.rerank(...)` (no-op default) → top-N `Hit`s with `source` provenance. (Graph-proximity boost → M3.)
- [ ] **Step 3** run (PASS). **Step 4** commit `feat(bossclaw-core): hybrid recall (RRF + recency/pin) + no-op reranker trait`.

## Task 8: `verify_chain_since(cursor)` — bound chain verify at scale

**Files:** modify `src/log.rs`; test in `tests/chain.rs`. *(Carried M1 note.)*

- [ ] **Step 1 — failing test:** append many events; `verify_chain_since(Some(tip_minus_k))` verifies only the tail and still detects a tamper there; verifies links from the trusted prior hash.
- [ ] **Step 2 — implement** `verify_chain_since(&self, from_event_id: Option<&str>)` reusing M1's recompute+link+signature loop from the cursor's `prev_hash` baseline. **Step 3** run (PASS). **Step 4** commit `feat(bossclaw-core): verify_chain_since (bounded chain verification)`.

## Task 9: Re-embed migration + the time budget (§15) + integrity-signal note

**Files:** modify `src/log.rs`; test in `tests/recall.rs`.

- [ ] **Step 1 — failing test:** model A active + vectors derived; switch active model to B; `reembed_migration(embedder_b)` → all `vectors` rows are model B, stale A rows GC'd, index rebuilt under B, recall works.
- [ ] **Step 2 — implement** `reembed_migration`: append a `config` event for B **via `EventLog::append`** (single-writer, §4) → `rederive_pending(embedder_b)` re-embeds → delete stale `WHERE model_id != active` → `rebuild_indexes`. **[critic gap] resumable:** a crash between switch and GC is safe (recall is active-model-filtered, C4) and the LEFT-JOIN backfill + a stale-row sweep on next open finish the job. **Print a timing line** (events/sec) → record the **re-embed time budget** in the handoff (§15 deliverable).
- [ ] **Step 3 — [critic F7] §15 integrity signal:** add a one-line note (here + in "Carried"): the `config` model-switch event is **signed + hash-chained** (tamper-evident via M1, so a forged/replayed switch is detectable by `verify_chain`); **desktop surfacing** of model-switch events as a recall-integrity alert is **deferred to M7**.
- [ ] **Step 4** run (PASS). **Step 5** commit `feat(bossclaw-core): re-embed migration + timing budget`.

## Task 10: recall@k fixture (the embedder-default gate) + final gates

**Files:** create `tests/fixtures/recall.json` + `tests/recall_fixture.rs`; CHANGELOG.

- [ ] **Step 1 — [critic F9] create `tests/fixtures/recall.json`** — a small **labelled** corpus (queries → relevant doc ids).
- [ ] **Step 2** an `#[ignore]` test computes **recall@k** for `Model2Vec` and (with `--features fastembed`) `FastEmbed`, printing both → the empirical default decision (R1): keep `model2vec` default unless bge-small clears a documented recall@k margin justifying the ort offline-bundling cost.
- [ ] **Step 3** `cargo test -p bossclaw-core` (hermetic suite green; `#[ignore]` model tests excluded) + `cargo clippy -p bossclaw-core -- -D warnings` clean. Run the `#[ignore]` fixture once locally; record recall@k + re-embed timing in the handoff.
- [ ] **Step 4** update CHANGELOG; commit `feat(bossclaw-core): recall@k fixture + M2 gates`.

---

## Milestone 2 — Definition of Done
- [ ] `Embedder` trait + `Model2Vec` (default, normalized) + `FastEmbed` (opt-in feature) + `MockEmbedder` (test).
- [ ] `cargo build -p bossclaw-core` pulls **no ort**; `--features fastembed` does (R1/R3 verified).
- [ ] `config` convention + `active_model()` defined (NOT assumed from M1); `schema_version` reserved in the config content.
- [ ] vectors table Tier-A, active-model filtered; derive is best-effort; `rederive_pending` backfills gaps (§10 proven by test).
- [ ] pure `VectorIndex` trait incl. `remove`; `HnswIndex` rebuilt by `EventLog` from `vectors ORDER BY event_id ASC`; rebuild reproduces **top-1 + recall stability** (NOT top-k set identity — hnsw_rs reseeds per build; R2 scope note); rebuild-time line printed.
- [ ] FTS5 inside SQLCipher + `fts_map`; the **no-plaintext-on-disk** test is a permanent guard.
- [ ] Hybrid recall (RRF + multiplicative recency/pin) returns provenance-tagged `Hit`s; reranker no-op-default trait.
- [ ] `verify_chain_since` bounds verification; re-embed migration works, is resumable, and **a time-budget number is recorded**.
- [ ] recall@k fixture exists; the embedder-default call is data-backed.
- [ ] `cargo test -p bossclaw-core` green (hermetic; temp homes only — M1 discipline); `clippy -D warnings` clean; zero `unsafe`.

**Carried into later milestones:** encrypted sidecar (post-v1 startup opt, gated on the Task-9 budget) · graph-proximity boost in recall (M3) · cross-encoder `bge-reranker` impl behind the trait (post-v1) · `EmbeddingGemma` upgrade + `sqlite-vec` opt-in (deferred) · per-platform ort offline-bundling CI proof before bge-small can be the shipped default · `schema_version` format-gating logic (field reserved now) · desktop surfacing of the §15 model-switch integrity signal (M7).

---

## Self-Review
**Spec coverage (§12 M2):** embedder ✓(T1–T3) · hnsw index ✓(T5) · FTS5 ✓(T6) · hybrid ✓(T7) · reranker no-op trait ✓(T7) · tiny corpus to feed the fixture ✓(T10) · go/no-go spikes ✓(resolved) · re-embed migration budget ✓(T9). §5.4 trait now complete incl. `remove` ✓. §11 recall@k + no-plaintext ✓(T10/T6). §15 budget ✓ + integrity-signal honestly deferred ✓.
**Critic fold (Rev 2):** F1 config-convention defined (T1) · F2 rebuild assertion = top-1+set-equality + mandated ORDER BY + serial insert (T5) · F3 recall API on EventLog, tests never touch `Store` (T4/T5) · F4 `remove` restored (T5) · F5 `fts_map` pinned, invalid unindexed-column dropped (T6) · F6 LEFT-JOIN backfill + best-effort test (T4) · F7 §15 integrity signal noted (T9) · F8/F9/F11 + gaps (ORDER BY, schema_version, re-embed resumability, normalize, derive-after-append) all folded.
**No magic numbers:** `RRF_K`, `HALF_LIFE_SECS`, `RECENCY_WEIGHT`, `PIN_MULTIPLIER` are named consts with sourced comments.
**Hermeticity:** default tests use `MockEmbedder` + temp homes; real-model tests `#[ignore]`/feature-gated → CI stays network-free.
