# Changelog

All notable changes to `bossclaw-core` are documented in this file.

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versions track BossClaw milestones, not semver releases (the crate is
pre-1.0 and not yet published to crates.io).

---

## [Unreleased]

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

- **Tier-A vectors table** (`src/store.rs`) — `vectors(event_id, model_id,
  vec_blob)` stored inside the existing SQLCipher database. Blobs are
  little-endian `f32` arrays (no additional encryption layer; the page-level
  SQLCipher AES-256 protects them). `rederive_pending(embedder)` backfills
  missing rows; `derive_vector(embedder, event)` upserts a single row.
  `vectors_for_model(model_id)` returns rows in `event_id ASC` order (C4
  active-model filter, no cross-model bleed).

- **In-memory HNSW index** (`src/index.rs`) — `HnswIndex` wraps `hnsw_rs
  0.3.4`. Rebuilt from the `vectors` table on every open via
  `rebuild_indexes(embedder)` — no plaintext index file is ever written to
  disk (verified by `tests/no_plaintext.rs`). Supports `add`, `search`,
  `remove` (tombstone), and `last_indexed` cursor for incremental updates.

- **FTS5 keyword index** (`src/keyword.rs`) — `fts_bodies` virtual table +
  `fts_map(event_id, rowid)` inside the SQLCipher store. `keyword_add(id,
  body)` is idempotent (INSERT OR REPLACE); `keyword_search(q, k)` escapes
  the query to a quoted FTS5 phrase via `escape_fts_query`. `rebuild_indexes`
  repopulates FTS from event content, also idempotent.

- **Hybrid recall** (`src/log.rs`, `src/recall.rs`) — `EventLog::recall(
  embedder, query, k, &RecallOptions)` runs both arms in parallel, fuses with
  Reciprocal Rank Fusion (`rrf_fuse`), applies a recency-decay boost
  (`HALF_LIFE_SECS = 7 days`) and a pin multiplier (`PIN_MULTIPLIER = 3.0`),
  then returns `Vec<Hit>` with per-hit `sources` provenance
  (`RecallSource::Vector`, `RecallSource::Keyword`, or both). Degrades
  gracefully: vector-arm failure falls back to keyword-only; keyword-arm
  failure falls back to vector-only; both failing returns `Err`.
  `NoopReranker` wires the reranker seam end-to-end; a real cross-encoder
  lands in a later milestone.

- **`verify_chain_since(cursor)`** (`src/log.rs`) — bounded chain
  verification starting from a known-good event id. Identical cryptographic
  logic to `verify_chain()` but O(n − cursor) rather than O(n); used by the
  sync layer (M4) to verify only the tail appended since the last checkpoint.

- **`reembed_migration(new_embedder)`** (`src/log.rs`) — atomic model-switch:
  appends a `config` event (new active model), backfills vectors for all
  events under the new model, garbage-collects stale rows from the old model,
  rebuilds both indexes, and returns `ReembedStats { reembedded, gc_removed,
  elapsed_ms }`. Idempotent: a second call on the same model is a no-op.

- **`config` event convention** — active model is the content of the latest
  `config`-typed event (`active_model_id`, `dim`, `schema_version`). Parsed
  by `EventLog::active_model() -> Option<ActiveModel>`.

- **`SCHEMA_VERSION = 1`** — reserved constant for future format-gating.

#### Tests

- **Hermetic suite** (`tests/recall.rs`) — 47 tests, all passing, no network
  access. Covers every public API path: embedder trait, Tier-A vectors,
  HNSW index, FTS5 index, hybrid recall, RRF fusion, recency/pin boosts,
  keyword-only degradation, `resolve_arms` unit tests, re-embed migration
  (happy path, idempotency, integrity, initial setup), `verify_chain_since`.

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
