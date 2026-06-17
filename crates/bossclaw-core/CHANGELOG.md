# Changelog

All notable changes to `bossclaw-core` are documented in this file.

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versions track BossClaw milestones, not semver releases (the crate is
pre-1.0 and not yet published to crates.io).

---

## [Unreleased]

### M4b — Summarizer (2026-06-17)

The summarize half of the evolve loop: it turns the M4a entity graph into
*understanding* — a per-entity, model-written **dossier** (`page`) that recall
returns as synthesis, kept current via `supersede`, with every claim leashed to
a signed source. It reuses M4a's `evolve_once` runtime + `Reasoner` seam: after
extraction + `rebuild_graph`, a summarize phase gathers a bounded fact-set per
dirty topic (entity + current edges + lineage memories — **never another
page**), the model composes a draft of discrete cited claims, the deterministic
**citation floor** subtracts every ungrounded claim, and the survivor is written
atomically (`supersede` + `page` in one transaction). The model's prose is
**data, never authority**: cited-but-fallible, machine-origin-lower-trust, never
itself a summary-source (the one-way rule), supersede-not-delete.

#### Added

- **`page`/`supersede` Tier-B events + `pages` projection** (`src/graph.rs`,
  `src/log.rs`) — `EventLog::page(topic_id, title, text, claims, tags, producer,
  source_event_ids)` mints a signed dossier for an `entity:<ulid>` topic;
  `supersede(prior)` retires one. Both are NON-MANUAL producers → empty
  `source_event_ids` is rejected (the F4 taint guard, extended). `rebuild_graph`
  folds them via `fold_pages` into a `pages` table (`topic_id` PRIMARY KEY) — the
  **current** (un-superseded, `seq`-max) page per topic, **at most one** (zero is
  a benign transient orphan-supersede state, F9). Byte-identical on rebuild. The
  body lives in `content.text` so it embeds + recalls with zero embed-path change
  (`EMBEDDABLE_EVENT_TYPES` already carried `"page"`); a `supersede` carries only
  `{supersedes}` → non-embeddable by construction.
- **Atomic `append_pair` + `emit_page`** (`src/log.rs`, F5) — `append` is
  refactored around a private `append_event_in_tx` core so `append_pair(first,
  second)` can chain two events in ONE transaction (the second reads the chain
  tip inside the shared tx, seeing the uncommitted first). `emit_page(…,
  prior_page_id)` uses it to emit `supersede`+`page` together when regenerating —
  never a durable orphan supersede; a topic is never left page-less. The Tier-B
  non-empty-lineage guard (`reject_empty_tier_b`) runs before either path opens a
  transaction.
- **Pure summarizer pipeline** (`src/summarize.rs`, PURE — mirrors `extract.rs`)
  — `FactSet`/`DraftPage`/`DraftClaim`/`RenderedPage` types; `compose_schema()`
  (`{title, claims:[{text, cites:[string]}]}`); `build_compose_prompt` (the
  fenced fact-set, each memory tagged with its event id so the model can cite it,
  edges as lines, via the shared M4a source-fence helper); `parse_draft`
  (tolerant — a malformed draft degrades to fewer claims, never a panic); the
  **`citation_floor`** (subtract-only: keep a claim ONLY if its `cites` is
  non-empty AND every cite is in the fact-set — an anti-fabrication bar-raiser,
  NOT a relevance/entailment boundary, F8); and `assemble` (renders the body +
  the sorted+deduped union of surviving cites, returns `None` when nothing
  survived so the empty-floor path never reaches `append`, F4). The cap
  (`MAX_CLAIMS_PER_PAGE`) is applied **before** the signed content is built (F7).
- **The `evolve_once` summarize phase** (`src/log.rs`, `src/evolve.rs`) — a
  persistent `summarize_cursor` (sibling of `evolve_cursor`, NOT a fold, F1);
  `dirty_entities_since` re-derives the dirty topic set each tick from
  `entity:`-prefixed endpoints of `link`/`invalidate`/`entity` events past the
  cursor (no per-tick accumulator). `summarize_topics` gathers each topic's
  fact-set (`gather_fact_set`, ≤ `SUMMARY_BATCH` topics/tick), composes, runs the
  floor, and emits via `emit_page` **only when the cited-source SET differs from
  the current page's** — idempotency keyed on grounding, never prose (F6: a
  temperature-0 model still rewords across runs; comparing wording would churn a
  supersede every tick). A per-topic `continue` on any gather/compose/parse/emit
  error (F4): extraction is already committed, so one topic's failure never
  breaks the batch or blocks the cursor. `EvolveReport` gains `pages_emitted` +
  `pages_superseded` (F10).
- **The one-way rule, enforced at the reader** (`src/log.rs`, F3) — a fact-set is
  raw memories + edges only, never another page. Enforced at BOTH arms:
  `fact_texts_for_ids` drops `page`-typed ids by construction (a page id reaching
  the fact-set is a contract violation, never silently summarized), AND the
  evolve loop's internal extraction recall passes `exclude_pages: true`.
- **Recall surfaces current dossiers** (`src/recall.rs`, `src/log.rs`, F2) — a
  `Hit.kind` field (the event's type) lets callers distinguish synthesis (a
  dossier) from ground truth (a raw memory); a `RecallOptions.exclude_pages`
  (default `false`) implements the one-way rule for the internal recall.
  `candidate_event_types` fetches per-candidate kinds, and recall drops
  excluded/superseded pages **before** `truncate(k)` — so a superseded page can
  never crowd out a valid lower-ranked candidate. Only the *current* page for a
  topic surfaces; superseded pages stay in the log (auditable, `as_of`) but leave
  the projection + recall.
- **Recall-neutral in v1** (F11) — no `PAGE_RECALL_WEIGHT`, no model-critique
  pass (`SUMMARY_REFLECT` dropped): the deterministic floor is the subtract
  mechanism, and safety is structural (supersede-exclusion + the one-way rule +
  machine-origin-lower-trust + the actuator never reading a page).
- **Named constants** (`src/summarize.rs`, `src/extract.rs`) — `PAGE_REACH`
  (`Tight`: entity + its edges + their lineage), `PAGE_MIN_FACTS` (`2` — no
  dossier for a bare name), `MAX_CLAIMS_PER_PAGE` (`32`), `SUMMARY_BATCH` (`8`).
  No magic numbers.

#### Tests

- **Hermetic suite (CI, `ScriptedReasoner` + `MockEmbedder`)** — page/supersede
  append + empty-lineage rejection, `fold_pages` current-per-topic +
  supersede-retire + at-most-one orphan-supersede + byte-identical rebuild
  (`tests/graph.rs`); compose prompt shape + `parse_draft` + citation-floor
  subtract-only + assemble empty→None + sorted/deduped cites + cap
  (`tests/summarize.rs`); the summarize phase end-to-end — grounded page emit,
  cursor-drain + cited-set idempotency, the one-way rule (a page body never
  re-enters the fact-set), empty-floor→no-page-without-breaking-the-batch
  (`tests/evolve.rs`); recall surfaces the current page + excludes superseded +
  `exclude_pages` hides all + superseded-at-rank-1-does-not-crowd-out
  (`tests/recall.rs`). **M4b security suite** (`tests/evolve.rs`): the page
  lineage is event-ids-only (never an `entity:`/topic id), a SQL-injection
  payload in a page title/text is inert literal data that survives a rebuild, a
  `supersede` is never embeddable (no vector row, never in recall), and an
  injection in a memory cannot plant an uncited claim or emit a `config` — the
  floor surgically drops the fabricated claims while keeping the one faithful,
  grounded claim (a per-claim subtract), and the machine-origin page carries full
  lineage.
- **Live-Ollama behavioral gate** (`tests/live_ollama.rs`, `#[ignore]`, feature
  `ollama`) — asserts properties not bytes against the real
  `qwen2.5:7b-instruct`: after extraction + summarize ticks a `page` exists for
  the entity; EVERY surviving claim cites only ids in the gathered fact-set
  (grounded — the floor's contract holding over a real draft); `recall` surfaces
  the dossier as a `page` hit; a contradicting memory + re-tick SUPERSEDES the
  page (a fresh current page, the prior dropped from the projection, still
  grounded); a re-tick with no new facts emits no new page (F6 idempotency,
  LIVE). Bounded retries absorb the 7b's run-to-run output variance (mirroring
  the M4a contradiction gate); the property is unchanged. Run locally with
  `cargo test -p bossclaw-core --features ollama --test live_ollama -- --ignored`;
  never part of the hermetic CI suite.

### M4a — Clever Linker (2026-06-16)

The LLM auto-linker that *populates* the M3 graph. A local model
(`qwen2.5:7b-instruct` via Ollama) reads each new memory, extracts entities +
typed relationships, resolves entities against the existing graph, retires
contradicted facts, and appends signed Tier-B `entity`/`link`/`invalidate`
events through the single-writer `append` — feeding the M3 fold, which makes
the next recall smarter. The closed loop (recall → extract → graph → recall) is
the architecture; the model's output is data, never authority
(invalidate-not-delete, confidence/trust-gating, every emit serialized).

#### Added

- **Reasoner seam** (`src/reason.rs`) — the `Reasoner` trait
  (`complete_json` + `model_id`); a deterministic `ScriptedReasoner` test double
  (canned JSON keyed by SHA-256 of `(system, prompt)`); the extraction +
  adjudication JSON-schema builders. Pure — no I/O.
- **`OllamaReasoner`** (`src/ollama.rs`, feature `ollama`) — POSTs `/api/chat`
  to loopback `127.0.0.1:11434`, `format` = the schema, `options.temperature =
  0`, a digest-pinnable model tag, refusing any non-numeric-loopback host
  (no egress; bare `localhost` rejected — DNS-rebind hazard). Behind the feature
  so the default build stays pure (no network dep). The structured output is
  parsed from a JSON string in `message.content` (verified against the live
  Ollama response shape).
- **`entity` Tier-B event + `entities` projection** (`src/graph.rs`,
  `src/log.rs`) — `EventLog::entity(label, aliases, entity_type, producer,
  source_event_ids)` mints a stable `entity:<ulid>` node; a NON-manual producer
  → `source_event_ids` MUST be non-empty (the F2 taint guard, extended).
  `rebuild_graph` folds entities into an `entities` table + marks those node ids
  `kind="entity"`. Byte-identical on rebuild.
- **Embedding entity resolution** (`src/extract.rs`, `src/log.rs`) — embed the
  mention, search a dedicated entity vector index (`entity_vectors`,
  kind-isolated from recall), apply `RESOLVE_HIGH`/`RESOLVE_LOW`, route the
  mid-band to the reasoner to adjudicate. No duplicate entities.
- **Retrieval-augmented extraction** (`src/extract.rs`, PURE) — Pass A (propose:
  cheat-sheet prompt + seed relation vocabulary + few-shot → parse to
  `{entities, relations, retractions}` each with confidence + a mandatory
  `supported_by` span); Pass B (critique: a pure span-verify floor the model
  can never override, then one model critique turn that may only subtract via
  `intersect_keep_floor`, confirming contradictions against a relation-
  cardinality table). The Pass-B prompt lists the proposed **retractions** as
  well as the relations and renders the graph neighborhood by human-readable
  entity name (not the opaque `entity:<ulid>`), so a small local model can
  confirm a contradiction without mangling the endpoint identifiers. Bounded by
  `MAX_REFLECT`.
- **`edges` origin/confidence + trust-gated boost + intra-result reinforcement**
  (`src/graph.rs`, `src/log.rs`, `src/recall.rs`) — the M3 `edges` fold gains
  `origin` (`'manual'` iff `model_id == MANUAL_LINK_PRODUCER`, else `'machine'`)
  + `confidence_milli` (from the `link` content, NULL for manual). `link.content`
  extends to `{src, relation, dst, confidence_milli?}`; confidence lives in the
  signed content as an INTEGER milli-unit (never a raw float, never in
  `ModelMeta`). The recall proximity boost now gates on `origin='manual' OR
  confidence ≥ TRUST_MIN`, and auto-seeds from the top `GRAPH_REINFORCE_TOPK`
  fused hits (not just top-1).
- **Evolve runtime** (`src/evolve.rs`, `src/log.rs`) — a persistent
  `evolve_cursor` (progress state, NOT a fold); `evolve_once()` runs the full
  tick (recall → Pass A → resolve → augment → Pass B → emit via `append` →
  advance cursor), idempotent (skip active edge-keys, reuse resolved entities);
  a hard off-switch (`config` `evolve_enabled=false`, honored before any model
  call); a pure `debounce_due` scheduler decision; an `EvolveStatus`
  observability surface (queue depth, last tick, error counts, enabled).

#### Tests

- **Hermetic suite (CI, `ScriptedReasoner` + `MockEmbedder`)** — reasoner
  determinism + schema shape (`tests/reason.rs`); resolution thresholds + Pass A
  parse + Pass B critique/cardinality (`tests/extract.rs`); entity resolution
  merge/mint/adjudicate (`tests/entity_resolution.rs`); evolve `evolve_once`
  end-to-end + idempotency + cursor persistence + off-switch + provenance +
  injection containment + resolved-id contradiction retirement
  (`tests/evolve.rs`); entity fold + byte-identical-rebuild-with-entities +
  `edges` origin/confidence (`tests/graph.rs`); trust-gate boost + intra-result
  reinforcement (`tests/recall.rs`).
- **Live-Ollama behavioral gate** (`tests/live_ollama.rs`, `#[ignore]`, feature
  `ollama`) — asserts properties not bytes against the real model: a person →
  ≥1 entity; a relationship → a machine link carrying confidence; a
  contradiction across two ticks → an `invalidate` retiring the prior
  `works_at_primary` edge (the F4 path, proven LIVE); re-run is idempotent. Run
  locally with `cargo test -p bossclaw-core --features ollama --test live_ollama
  -- --ignored`; never part of the hermetic CI suite.

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
