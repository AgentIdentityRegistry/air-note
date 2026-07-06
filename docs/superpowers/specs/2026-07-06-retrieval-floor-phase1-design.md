# Retrieval Floor (Phase 1) — Design

**Date:** 2026-07-06 · **Status:** Draft for review · **Owner:** Peter (approved verbally in session)
**Program context:** Phase 1 of the approved memory strategy (GBrain `air/memory-strategy-2026-07-03-beat-the-stack`). Phase 0 (memharness, PR #71) is merged and produced the live baseline this phase is measured against.

## 1. Goal

Raise `bossclaw-core`'s retrieval floor — measured, never assumed — in three shippable rungs, each A/B'd against the previous state with the Phase 0 harness on Peter's real `~/brain` corpus. The compass metric is **known-item success@10 / MRR** (mechanical, judge-free — the trustworthy metric; both live runs' open-query verdicts are flagged untrusted and are NOT gates here).

### Baseline (live runs 2026-07-06, k=10, seed=42)

| Segment | n | AIR s@k | GBrain s@k | Status |
|---|---|---|---|---|
| synthetic·en·known-item | 224–225 | 0.589–0.613 | 0.286–0.298 | AIR wins, CI excludes 0 |
| real·en·known-item | 33 | 0.212 | 0.152 | AIR leans, not significant |
| synthetic·ko·known-item | 75–76 | 0.013 | 0.026–0.067 | **AIR loses** (run 2 CI excludes 0) |
| real·ko·known-item | 5 | 0.400 | 0.000 | n too small |

Judge-trust (context only): local qwen 53.4%/κ0.26; cloud Haiku 65.5%/κ0.465 — both below the ≥85%/κ0.6 bar; open-query win rates stay non-gating until a judge clears the bar (out of scope here).

### Success criteria (Phase 1 overall)

1. English known-item recall (synthetic + real point estimates) improves over baseline with the synthetic·en CI excluding zero at the final rung.
2. Korean synthetic known-item recall improves from ~0.013 to at least parity with GBrain's arm, without English regressing beyond its CI.
3. Every rung ships only through its measured gate (below); a rung that fails its gate is reverted or reworked, not shipped.
4. Zero change to the privacy posture: no new network egress from the engine; the HNSW stays unpersisted (its privacy rationale, `index.rs:1-8`, is untouched).

## 2. Verified reality (map of record, 2026-07-06)

File:line evidence from a dedicated code exploration; plans MUST build on these facts, not the older strategy-page audit. Corrections to that audit are marked.

- **Embedding unit = one whole event text.** `embeddable_text()` `bossclaw-core/src/log.rs:6910`; `embed_one()` one-item batch `log.rs:6923`; a file's `content["text"]` is the entire parsed body (`ingest.rs:605,682`). **No chunking anywhere.** One event = one row in `vectors`, PK `(event_id, model_id)` (`log.rs:474-479`).
- **Shipped embedder:** `ResourceModel2Vec` (potion-base-8M) constructed at `bossclawd/src/main.rs:123`; model dir default `bossclawd/src/main.rs:200-205`; `MODEL_ID = "minishlab/potion-base-8M"` at `bossclawd/src/engine/embed.rs:16`. The `EmbedderProvider` seam lives in the **daemon** (`bossclawd/src/engine/embed.rs:19`), NOT in bossclaw-core (core has the lower-level `Embedder` trait, `embed.rs:17`, batch-in/batch-out, dim-agnostic). A FastEmbed bge-small-en path exists behind the `fastembed` feature (`bossclaw-core/src/fastembed.rs:20`) but is never constructed.
- **Dims are runtime-probed, not constants** (`model2vec.rs:130-131`): read `embedder.dim()`; never hardcode.
- **Keyword arm (audit CORRECTED):** documents ARE term-tokenized (FTS5 default `unicode61`, no `tokenize=` clause, `log.rs:492`) and BM25-ranked (`log.rs:1371`). The defect is the QUERY: `escape_fts_query` wraps it as ONE quoted phrase (`keyword.rs:36-39`, called at `log.rs:1367`) — multi-word queries only match exact adjacent phrases. Injection safety is the reason for the quoting; any fix must preserve it.
- **Fetch ceiling (new finding):** both arms over-fetch a fixed `FUSION_FETCH = 50` (`recall.rs:161`; used `log.rs:1445-1446`); the caller's k applies only at the final `truncate(k)` (`log.rs:1614`). An answer ranked >50 in both arms can never surface, independent of k.
- **Fusion:** tie-aware rank-only RRF, `RRF_K=60` (`recall.rs:201,226,246`; wired `log.rs:1460`). Score magnitudes never enter.
- **Recall path:** daemon `EngineHandle::recall()` `bossclawd/src/engine/mod.rs:521` → `ensure_indexed` (`mod.rs:501`) → `EventLog::recall` `log.rs:1436` → embed query (`log.rs:1444`) → vector arm (`log.rs:1445` → `index.rs:158`) + keyword arm (`log.rs:1446` → `log.rs:1359`) → fuse (`log.rs:1460`) → boosts (`log.rs:1530-1543`) → `NoopReranker` (`log.rs:1566`; trait `recall.rs:53`) → sort/filter/truncate (`log.rs:1583-1614`).
- **Migration exists, manual:** `reembed_migration(embedder)` `log.rs:1834` — appends a signed `config` event (new `active_model_id` + dim, `log.rs:1847-1862`), backfills only missing `(event, model_id)` vectors (`rederive_pending` `log.rs:1133` — idempotent/resumable), GCs other-model rows (`log.rs:1873`), rebuilds indexes (`log.rs:1880`). **Nothing calls it automatically** (0 callers in bossclawd).
- **Index rebuild determinism:** `vectors_for_model` ordered `event_id ASC` (`log.rs:1190`); `HnswIndex::add` de-dups by `event_id` (`index.rs:143`) — a second vector per event is silently dropped today. HNSW rebuilt on open by design (privacy, `index.rs:1-8`); deep-rank is not cross-session deterministic (OS-seeded RNG, `index.rs:93-101`).
- **Normalization contract:** HNSW uses cosine (`index.rs:103`) and assumes L2-normalized vectors; Model2Vec normalizes (`model2vec.rs:66`). Any new embedder MUST L2-normalize.
- **Korean nuance:** Korean text has spaces → `unicode61` tokenizes it (suboptimally but functionally). The measured Korean failure is therefore attributed to the English-only embedder, not FTS. (`unicode61` does not segment spaceless CJK — noted, out of scope.)

## 3. Design — four increments ("rungs")

Each rung: own branch + PR; subagent-driven TDD with two-stage reviews; harness A/B recorded in the PR body; independent review before merge; merge only if the gate passes.

### Rung 0 — harness `--known-item-only` (enabler; crate `memharness`, dev-only)

*Problem:* a full run is ~1.5h because of local-LLM answering/judging on open queries; retrieval rungs only need the mechanical metric.
*Change:* new flag `--known-item-only`: build the case list, then filter to `gold_page_id.is_some()` before `run_queries`; skip the Anthropic key requirement entirely in this mode (no opens → no judging/audit); report renders "no open queries this run" via the existing no-opens path. Expected wall-clock ≈ 20 min (synth generation remains the floor; open-answering ~1h disappears).
*Stacking:* builds on PR #72 (`--judge` flag); implement on top of that branch or after its merge.
*Gate (functional, not statistical):* a `--known-item-only` run completes, produces known-item segments identical in shape to a full run, and `cargo test -p memharness` stays green with a CLI + hermetic test for the flag.

### Rung 1 — query-term tokenization + fetch-cap lift (crate `bossclaw-core`)

*Change A — `keyword.rs`:* replace whole-query phrase quoting with per-term quoting: split the query on Unicode whitespace, escape each term's internal quotes, emit `"t1" OR "t2" … OR "tN"`. Injection safety is preserved (every term remains inside quotes; `OR` is ours, not user input). Empty/whitespace-only queries keep current behavior. BM25 already ranks multi-term matches higher — no ranking change needed.
*Change B — `recall.rs:161`:* `FUSION_FETCH: 50 → 200` (named const, comment records the 880-page-corpus rationale and that it is a re-tunable measurement subject).
*Gate:* known-item-only A/B vs baseline: synthetic·en s@10 point estimate improves and NO segment regresses beyond its CI. (Expected mechanism: keyword arm finally contributes multi-word matches; cap stops truncating the candidate pool.)

### Rung 2 — multilingual embedder swap (crates `bossclawd` + app resources)

*Probe first (before any code):* verify the candidate model — `minishlab/potion-multilingual-128M` (same Model2Vec runtime) — exists, license (expect MIT), artifact size, loads via the existing `Model2Vec` loader, runtime dim, and produces sane KO/EN smoke similarities. If it fails the probe, fall back to evaluating `fastembed` multilingual variants; the rung's design holds, only the model changes.
*Changes:*
1. `scripts/fetch-model.sh` + Tauri resources: bundle the new model dir alongside/instead of potion-base-8M (size approved: quality first).
2. `MODEL_ID` (`bossclawd/src/engine/embed.rs:16`) → the new model id; model-dir default updated (`bossclawd/src/main.rs:200-205`).
3. **Boot auto-migration (the missing piece):** on daemon boot, read the brain's recorded `active_model_id` (the `config` event `reembed_migration` writes); if it differs from the compiled `MODEL_ID`, run `reembed_migration(new_embedder)` before serving recall. Idempotent by construction (`log.rs:1808-1821`); progress logged; a mid-migration crash resumes (rederive_pending backfills only missing rows).
4. Old-model vectors are inert immediately (model-id filtering, `log.rs:1190`) — no corruption window.
*Gate:* known-item-only A/B vs rung 1: synthetic·ko s@10 improves from ~0.01 to ≥ GBrain's measured arm on the same run, AND synthetic·en does not regress beyond its CI. Harness reports must show the new model id (PackStats/manifest unchanged otherwise).

### Rung 3 — chunking (crate `bossclaw-core`; the big rung)

*Unit:* split each embeddable event text into heading/paragraph-aware chunks with a fixed character budget (const, e.g. ~1,500 chars ≈ a few paragraphs) and fixed overlap (const, e.g. 200 chars); char-boundary-safe for Korean (`chars()`, never byte slicing); whole text ≤ budget ⇒ exactly one chunk (small docs unchanged).
*Touch points (from the map — each is a named task in the plan):*
1. Schema: `vectors` PK `(event_id, model_id)` → `(event_id, model_id, chunk_ix)` (SQLite table rebuild migration), `chunk_ix INTEGER NOT NULL DEFAULT 0`.
2. Derivation: `derive_vector`/`rederive_pending` (`log.rs:1101,1133`) loop over chunks; `embed_one` generalizes to a batched `embed_chunks`.
3. Index: HNSW slot key becomes `(event_id, chunk_ix)` (today `index.rs:143` silently drops a second add per event — that de-dup must move up); rebuild read (`log.rs:1183-1190`) orders by `(event_id, chunk_ix)` for determinism.
4. **Fold-back before fusion (correctness-critical):** vector-arm hits collapse to best-score-per-`event_id` BEFORE `fuse_scored_arms` (`log.rs:1460`) so multi-chunk docs cannot double-vote in RRF. Keyword arm unchanged (whole-doc FTS).
5. Migration: encode the chunking scheme in the stored model id (`"<model>+chunks-v1"`) so the EXISTING `reembed_migration` + rung 2's boot auto-migration handle re-embedding with zero new migration machinery.
*Gate:* known-item-only A/B vs rung 2: synthetic·en s@10 improves with CI excluding zero vs the rung 2 run; real·en point estimate improves; no segment regresses beyond CI. A final FULL harness run (all segments + cloud judge) is recorded as the Phase 1 closing baseline.

## 4. Testing

- Every rung: TDD unit tests in the touched crate; `cargo test --workspace`, `clippy -D warnings`, the scoped `cargo build -p bossclawd` feature-leak gate (Phase 0 Task 46 set) stay green.
- Rung 1: `keyword.rs` tests — multi-term OR emission, quote escaping, operator-injection attempts (`OR`, `NEAR`, `*`, `"`), empty query, Korean terms.
- Rung 2: migration tests — boot with mismatched model id triggers exactly one migration; re-boot is a no-op; mid-migration resume (kill between backfill and GC).
- Rung 3: chunker property tests (budget/overlap/boundary-safety, KO text); fold-back test (multi-chunk doc appears once in fused results, best score wins); determinism test (rebuild order).
- Measurement: every gate cites a harness report path + the numbers; runs use k=10, seed=42 for comparability.

## 5. Out of scope (later specs)

Local-or-cloud embedder/answerer choice UI + consent (Peter-approved principle; own spec after Phase 1). Reranker activation, score-aware fusion, HNSW persistence, spaceless-CJK FTS segmentation, judge improvement (reasoning-before-verdict experiment), answer-quality (open-query) gating.

## 6. Risks

- **Multilingual model quality is unproven on this corpus** — probe + measured gate; fall back to alternatives without redesign.
- **Chunking inflates index size ~(chunks/doc)×** — acceptable at 880-page scale (rebuild-on-open measured in the harness ingest stage; if boot latency degrades noticeably, it shows up in the rung 3 run and is addressed then).
- **Query OR-semantics can add noise on stopword-heavy queries** — BM25 IDF down-weights stopwords; the A/B gate is the arbiter (if EN regresses, revisit with AND-of-top-terms or stopword filtering).
- **Boot auto-migration on a large brain takes minutes** — logged progress; resumable; runs once per model change.
