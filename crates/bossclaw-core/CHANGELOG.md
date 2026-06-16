# Changelog

All notable changes to `bossclaw-core` are documented in this file.

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versions track BossClaw milestones, not semver releases (the crate is
pre-1.0 and not yet published to crates.io).

---

## [Unreleased]

### M3 — Graph (2026-06-16)

Bi-temporal link graph layered on top of the M1/M2 append-only encrypted
event log. Every structure below is derived and rebuildable from the log;
the `edges`/`nodes` tables are a deterministic Tier-A projection, just as
`vectors`/`fts` are in M2.

#### Added

- **`link`/`invalidate` Tier-B events** — `EventLog::link(src, relation,
  dst, valid_time, source_event_ids)` and `EventLog::invalidate(…)` append
  signed events through the single-writer `append` path. `model_id =
  "manual"` (named const `MANUAL_LINK_PRODUCER`). When `source_event_ids`
  is empty the helper defaults to `[src, dst]` for the manual producer only;
  a non-manual caller with an empty source set is rejected (taint-laundering
  guard — an empty default there would erase the inducing event from the §5.11
  lineage walk). Each `link` event ULID is the edge's stable identity.

- **Deterministic bi-temporal fold — `rebuild_graph`** (`src/log.rs`) —
  wipes `edges`/`nodes` and refolds every `link`/`invalidate` event in
  `seq ASC` order under one transaction. Byte-identical across rebuilds
  (proven by hermetic test). Timestamps are normalized to fixed-width UTC
  microseconds + `Z` (`normalize_ts` in `src/graph.rs`) so SQL `TEXT`
  comparison equals chronological comparison. Malformed graph events (missing
  `src`/`relation`/`dst`) are skipped and logged as a warning rather than
  failing the fold. `open_with_recall` calls `rebuild_graph` so the graph
  and its recall boost are live on open.

- **`invalidate` closes, never deletes** — each closed assertion row keeps
  `valid_to` (world-clock end) and `invalidated_at`/`invalidated_by` (learned-
  clock end + closing event id). Re-linking after an invalidate opens a fresh
  assertion (new edge row, new validity interval). One `invalidate` closes
  *all* currently-active assertions for the same `(src, relation, dst)` key.

- **`neighbors(node)`** — current edges touching `node` in either direction
  (`invalidated_at IS NULL`). Backlinks are the subset with `dst == node`.
  `ORDER BY edge_id ASC` for deterministic output.

- **`as_of(node, AsOf)`** — two-axis bi-temporal query. `valid_time` filters
  world-clock (`valid_from ≤ t < valid_to`); `known_as_of` filters learned-
  clock (`ingested_at ≤ t < invalidated_at`). Both `None` = current (identical
  to `neighbors`). Query timestamps are normalized before comparison.

- **`all_edges` / `all_nodes`** — full Tier-A table reads in `edge_id` /
  `node_id` ASC order (deterministic; used in rebuild-idempotency tests).

- **Live graph-proximity recall boost** (`src/log.rs`, `src/recall.rs`) —
  after computing the fused RRF scores, `recall` runs a 1-hop BFS over the
  current `edges` table (undirected) seeded from either the caller-supplied
  `RecallOptions::graph_seeds` or (when empty) the top-1 fused hit
  (`GRAPH_AUTO_SEED_TOPK = 1`). Each neighbor within `GRAPH_MAX_HOPS = 1`
  is multiplied by `1 + GRAPH_WEIGHT × GRAPH_HOP_DECAY^(hops-1)` where
  `GRAPH_WEIGHT = 0.4` and `GRAPH_HOP_DECAY = 0.5`. Only *current* edges
  contribute (retired edges give no boost — proven by test). A graph error
  degrades to no boost, never fails recall.

- **Pure types + helpers** (`src/graph.rs`) — `Edge`, `Node`, `AsOf`,
  `MANUAL_LINK_PRODUCER`, `normalize_ts`, `parse_link_content`, `fold_edges`.
  No SQL, no I/O — mirrors the `recall`/`keyword` module split.

#### Tests

- **Hermetic graph suite** (`tests/graph.rs`, 13 tests) — fold determinism
  (byte-identical rebuild); invalidate-closes-not-deletes + re-link opens new
  interval; `invalidate` with no active assertion is a no-op; one `invalidate`
  closes *all* active assertions for a key; `neighbors` + backlinks;
  `as_of` valid-time axis, `as_of` known-as-of axis, `as_of` both axes
  together; `nodes` kind (`"memory"` vs `"external"`); self-loop edge/node
  count; malicious relation label is inert data (SQL-injection regression).

- **Recall boost tests** (`tests/recall.rs`, +3 tests, total 53) — auto-seed
  boost fires on a current edge and disappears after invalidation (score-based,
  not rank-based); explicit `graph_seeds` boost their neighbor above the auto-
  seed baseline; invalidating an edge does not suppress the memory from recall
  (never-forget contract).

---

### M2 — Recall (2026-06-15)

The full semantic + lexical hybrid recall stack, built entirely on top of the
M1 append-only encrypted event log. Every structure below is derived and
rebuildable from the log; nothing in M2 adds a second source of truth.

#### Added

- **`Embedder` trait** (`src/embed.rs`) — common interface for all embedding
  backends: `embed(&[String]) -> Result<Vec<Vec<f32>>>`, `dim()`, `model_id()`.
  `MockEmbedder` (FNV-1a bag-of-words, L2-normalised) ships in the same module
  for deterministic hermetic tests.

- **`Model2Vec` embedder** (`src/model2vec.rs`) — pure-Rust default backend
  wrapping `model2vec-rs 0.2`. Loads from a local directory via
  `Model2Vec::from_pretrained(&Path, model_id)`. No network access at runtime;
  no ONNX Runtime dependency. `from_borrowed` constructor enables the planned
  M7 single-binary bundle (static `include_bytes!` weights).

- **`FastEmbed` embedder** (`src/fastembed.rs`, behind `--features fastembed`)
  — opt-in quality upgrade using `fastembed 5.16` + `BAAI/bge-small-en-v1.5`
  (384-dim, ONNX Runtime). Downloads weights on first use; subsequent calls are
  purely local. Wrapped in a `Mutex` to satisfy `Send + Sync`.

- **Tier-A vectors table** (`src/log.rs`) — `vectors(event_id, model_id, dim,
  embedding)` created inside `EventLog::open` alongside the rest of the log
  schema. Blobs are little-endian `f32` arrays (no additional encryption layer;
  the page-level SQLCipher AES-256 protects them). `rederive_pending(embedder)`
  backfills missing rows; `derive_vector(embedder, event)` upserts a single
  row. `vectors_for_model(model_id)` returns rows in `event_id ASC` order (C4
  active-model filter, no cross-model bleed).

- **In-memory HNSW index** (`src/index.rs`) — `HnswIndex` wraps `hnsw_rs
  0.3.4`. Rebuilt from the `vectors` table on every open via
  `rebuild_indexes(embedder)` — no plaintext index file is ever written to
  disk (verified by `tests/no_plaintext.rs`). Supports `add`, `search`,
  `remove` (tombstone), and `last_indexed` (tracks the most recently added
  event id; no incremental-index path exists in v1 — rebuilds are full).

- **FTS5 keyword index** (`src/log.rs`) — `fts` virtual table
  (`USING fts5(body, content='')`, contentless) plus `fts_map(rowid, event_id)`
  side-table, both created inside `EventLog::open`. `keyword_add(id, body)` is
  idempotent via a **transactional dedup-check** (checks `fts_map` inside a
  transaction, skips if already present — `INSERT OR REPLACE` is impossible on
  a contentless FTS5 table). `keyword_search(q, k)` escapes the query to a
  quoted FTS5 phrase via `escape_fts_query` (in `src/keyword.rs`).
  `rebuild_indexes` wipes and repopulates both `fts` and `fts_map` in
  `seq ASC` order, also idempotent.

- **Hybrid recall** (`src/log.rs`, `src/recall.rs`) — `EventLog::recall(
  embedder, query, k, &RecallOptions)` runs both arms, fuses with
  Reciprocal Rank Fusion (`rrf_fuse`), applies a recency-decay boost
  (`HALF_LIFE_SECS = 7 days`) and a pin multiplier (`PIN_MULTIPLIER = 2.0`),
  then returns `Vec<Hit>` with per-hit `sources` provenance
  (`RecallSource::Vector`, `RecallSource::Keyword`, or both). Degrades
  gracefully: vector-arm failure falls back to keyword-only; keyword-arm
  failure falls back to vector-only; both failing returns `Err`.
  `NoopReranker` wires the reranker seam end-to-end; a real cross-encoder
  lands in a later milestone.

- **`open_with_recall(path, dek, key, embedder)`** (`src/log.rs`) —
  convenience constructor that calls `open` then `rebuild_indexes` in one step,
  returning a recall-ready `EventLog`. Events appended after this call are not
  in the vector index until `rebuild_indexes(embedder)` is called again
  (spec §10 graceful degradation: keyword arm still finds them). An incremental
  `index_event` path is deferred to M7.

- **`verify_chain_since(cursor)`** (`src/log.rs`) — bounded chain
  verification starting from a known-good event id. Identical cryptographic
  logic to `verify_chain()` but O(n − cursor) rather than O(n); used by the
  sync layer (M4) to verify only the tail appended since the last checkpoint.

- **`reembed_migration(new_embedder)`** (`src/log.rs`) — atomic model-switch:
  appends a `config` event (new active model), backfills vectors for all
  events under the new model, garbage-collects stale rows from the old model,
  rebuilds both indexes, and returns `ReembedStats { reembedded, gc_removed,
  elapsed_ms }`. Idempotent: a second call on the same model is a no-op.
  Re-embed throughput: **~1250 events/sec** (model2vec/potion-base-8M — measured
  over the 15-doc fixture: 15 docs re-embedded in 12 ms via the
  `model2vec_reembed_budget` `#[ignore]` test) → re-embedding ~10k memories ≈ 8 s.
  `reembed_migration` emits this figure (`ReembedStats.elapsed_ms` + a `log::info!`
  line) at real corpus sizes.

- **`config` event convention** — active model is the content of the latest
  `config`-typed event (`active_model_id`, `dim`, `schema_version`). Parsed
  by `EventLog::active_model() -> Option<ActiveModel>`.

- **`SCHEMA_VERSION = 1`** — reserved constant for future format-gating.

#### Tests

- **Hermetic suite** (`tests/recall.rs`) — 49 tests, all passing, no network
  access. Covers every public API path: embedder trait, Tier-A vectors,
  HNSW index, FTS5 index, hybrid recall, RRF fusion, recency/pin boosts,
  keyword-only degradation, `resolve_arms` unit tests, re-embed migration
  (happy path, idempotency, integrity, initial setup), `verify_chain_since`,
  and the new `open_with_recall` lifecycle tests.

- **Recall@K empirical gate** (`tests/recall_fixture.rs`, `#[ignore]`) —
  labelled corpus of 15 short distinct-topic memory documents and 8 queries
  (mix of paraphrase/semantic and keyword-ish) in
  `tests/fixtures/recall.json`. Computes recall@K = (queries with a relevant
  hit in top-K) / total queries.

  **Measured recall@3 (potion-base-8M, first run): 1.0000 (8/8 queries)**

  Regression floor set at 0.625 (5/8) — well below the observed value to
  avoid flakiness from HNSW non-determinism on tiny corpora while still
  catching genuine embedding regressions.

  FastEmbed gate (`fastembed_recall_at_k`, `--features fastembed`) uses the
  same fixture and floor; ONNX measurement deferred (requires download).

---

## [M1 — Bedrock]

Encrypted, append-only, Ed25519-signed event log backed by SQLCipher.
`EventLog::open`, `append`, `stream_all`, `verify_chain`. No recall
layer in this milestone.
