# Retrieval Floor (Phase 1) — Design — Rev 2 (revised after architect + critic dual review)

**Date:** 2026-07-06 · **Status:** Rev 2 for review · **Owner:** Peter (design approved verbally; Rev 1 dual-reviewed)
**Program context:** Phase 1 of the approved memory strategy (GBrain `air/memory-strategy-2026-07-03-beat-the-stack`). Phase 0 (memharness, PR #71) is merged and produced the live baseline this phase is measured against.

## Rev 2 change summary (architect SOUND-WITH-CHANGES + critic APPROVE-WITH-CHANGES)

| # | Finding (sev) | Where fixed |
|---|---|---|
| 1 | (CRIT, both reviewers independently) Rung-over-rung gates compared runs with a LIVE corpus (874→880 pages in one day) AND per-run regenerated synth queries (Ollama text generation is non-deterministic even at fixed seed; `synth.rs:35` samples over the changed page list) — gates would measure churn, not the rung. The KO gate signal (~+0.03 at n≈75) is the same order as the churn. | §3.0 frozen-measurement protocol: corpus snapshot + persisted case list + per-case results for paired cross-run stats; all rung gates re-stated against it |
| 2 | (IMP, architect) Gate rule had no teeth at n=33/n=5 (CIs ±0.2–0.8; synthetic·ko CI flipped across zero between baselines with zero code change) | §1 success criteria + §3 gates: gating segments = synthetic·en (n≈225) and synthetic·ko (n≈75) ONLY; real·en/real·ko are directional, never gate |
| 3 | (IMP, both) Rung 2 boot migration collides with the EXISTING record-only mismatch handler `bossclawd/src/engine/mod.rs:456-463` → `set_active_model` (`log.rs:1917`, records id WITHOUT re-embed/GC). If ingest stamps first, a config-event-based boot trigger sees "already done" and never migrates → near-empty vector arm. | §3 Rung 2: trigger = "zero `vectors` rows for compiled `MODEL_ID`" (not the config event); the `mod.rs:456-463` path reconciled; migration ordered before `ensure_indexed` |
| 4 | (IMP, critic) The harness NEVER exercises the migration (fresh daemon per run, `main.rs:98`) — rung 2's gate blesses retrieval quality only; migration could ship broken invisibly | §3 Rung 2 + §4: migration correctness gated by dedicated integration tests (incl. a DIM change + mid-migration crash resume), explicitly NOT by the harness gate |
| 5 | (IMP, critic) Rung 2 touch list omitted 5 hardcoded model-path sites (tauri bundle glob, desktop resource paths, daemon default dir, fetch-model.sh DEST/BASE + 3 sha256 pins) | §3 Rung 2 change list enumerates all sites |
| 6 | (IMP, both) Rung 3 under-specified: composite-key encoding vs the `&str`-keyed `VectorIndex` trait; fold-back seam named at the WRONG line (provenance sets are built at `log.rs:1451-1454`, before fusion); `MODEL_ID` must itself become the `+chunks-v1` string or the migration silently no-ops / the read path goes empty; over-fetch needed to keep FUSION_FETCH distinct events after collapse | §3 Rung 3 rewritten: encoding pinned, fold-back INSIDE `vector_search`, single-effective-id rule, over-fetch rule, full touch list |
| 7 | (MIN, critic) Rung 0 "~20 min" undercounted the GBrain CLI floor (~4s × ~338 known-item cases ≈ 22 min) + synth ~15 min | §3 Rung 0 restated ~35–40 min; key-skip edit site named (`main.rs:84-91`); PR #72 merge = hard prerequisite |
| 8 | (MIN, both) Misc: punctuation-only FTS term test; rebuild-latency numeric budget captured in the report; FUSION_FETCH semantics post-chunking; FTS write-path invariance under chunking | §3/§4 respective rungs |

Reviewer-verified good news (no change needed): FTS5 OR-queries safe to 5,000 terms on the exact bundled SQLCipher 3.45.3 build; `active_model()` reader exists and is fresh-brain-safe (`log.rs:1057-1073`); the zero-opens report path is safe (`report.rs:186` + test); `Model2Vec::from_pretrained` is dim-agnostic and format-flexible (`model2vec.rs:62-79,130`); `HnswIndex::search` uses `ef = max(requested, 64)` so FUSION_FETCH=200 has no cost cliff; the candidate model `minishlab/potion-multilingual-128M` exists on HF and loads via the real `from_pretrained` path.

## 1. Goal

Raise `bossclaw-core`'s retrieval floor — measured, never assumed — in three shippable rungs, each A/B'd under a **frozen measurement protocol** (§3.0) with the Phase 0 harness. The compass metric is **known-item success@10 / MRR** (mechanical, judge-free). Open-query win rates stay non-gating (both judges below the ≥85%/κ0.6 trust bar: local qwen 53.4%/κ0.26, cloud Haiku 65.5%/κ0.465).

### Baseline (live runs 2026-07-06, k=10, seed=42 — pre-freeze; the frozen-set Phase 1 baseline is re-measured in Rung 0)

| Segment | n | AIR s@k | GBrain s@k | Status |
|---|---|---|---|---|
| synthetic·en·known-item | 224–225 | 0.589–0.613 | 0.286–0.298 | AIR wins, CI excludes 0 |
| real·en·known-item | 33 | 0.212 | 0.152 | AIR leans, not significant |
| synthetic·ko·known-item | 75–76 | 0.013 | 0.026–0.067 | **AIR loses** (run 2 CI excludes 0) |
| real·ko·known-item | 5 | 0.400 | 0.000 | n too small |

The run-1→run-2 movement with zero code change (en 0.589→0.613; ko CI flipping across zero) is the measured proof of finding #1: unfrozen runs are not comparable.

### Success criteria (Phase 1 overall)

1. **Gating segments** (the only segments gates may cite): synthetic·en·known-item (n≈225) and synthetic·ko·known-item (n≈75). real·en (n=33) and real·ko (n=5) are directional evidence only.
2. On the frozen case set: synthetic·en AIR s@10 improves over the frozen Rung-0 baseline with a paired test (Wilcoxon on per-case success flags across rung states) significant at p<0.05 by the final rung.
3. On the frozen case set: synthetic·ko AIR s@10 reaches at least GBrain's same-run arm, without synthetic·en regressing (paired test shows no significant loss).
4. Every rung ships only through its gate; a failed gate = revert or rework, never ship.
5. Privacy posture unchanged: no new engine egress; HNSW stays unpersisted (`index.rs:1-8` rationale untouched).

## 2. Verified reality (map of record, 2026-07-06 — every claim re-verified by two independent reviewers)

- **Embedding unit = one whole event text.** `embeddable_text()` `bossclaw-core/src/log.rs:6910`; `embed_one()` one-item batch `log.rs:6923`; a file's `content["text"]` is the entire parsed body (`ingest.rs:605,682`). **No chunking anywhere.** One event = one row in `vectors`, PK `(event_id, model_id)` (`log.rs:474-479`).
- **Shipped embedder:** `ResourceModel2Vec` (potion-base-8M) constructed at `bossclawd/src/main.rs:123`; model dir default `bossclawd/src/main.rs:200-205`; `MODEL_ID = "minishlab/potion-base-8M"` at `bossclawd/src/engine/embed.rs:16`. The `EmbedderProvider` seam lives in the **daemon** (`bossclawd/src/engine/embed.rs:19`), NOT in bossclaw-core (core has the lower-level `Embedder` trait, `embed.rs:17`, batch-in/batch-out, dim-agnostic). A FastEmbed bge-small-en path exists behind the `fastembed` feature (`bossclaw-core/src/fastembed.rs:20`) but is never constructed.
- **Dims are runtime-probed, not constants** (`model2vec.rs:130-131`): read `embedder.dim()`; never hardcode.
- **Keyword arm:** documents ARE term-tokenized (FTS5 default `unicode61`, no `tokenize=` clause, contentless table with `fts_map` side-table, `log.rs:492`) and BM25-ranked (`log.rs:1371`). The defect is the QUERY: `escape_fts_query` wraps it as ONE quoted phrase (`keyword.rs:36-39`, called at `log.rs:1367`). Injection safety is the reason for the quoting; any fix must preserve it.
- **Fetch ceiling:** both arms over-fetch a fixed `FUSION_FETCH = 50` (`recall.rs:161`; used `log.rs:1445-1446`); the caller's k applies only at the final `truncate(k)` (`log.rs:1614`). An answer ranked >50 in both arms can never surface.
- **Fusion:** tie-aware rank-only RRF, `RRF_K=60` (`recall.rs:201,226,246`; wired `log.rs:1460`). Provenance sets `vector_set`/`keyword_set` and `resolve_arms` are built at `log.rs:1447-1454` — BEFORE fusion. Boosts (`log.rs:1530-1546`), kinds (`log.rs:1558`), sort (`log.rs:1583-1592`), and the superseded/revoked filter (`log.rs:1597-1613`) all key on bare `Hit.event_id` AFTER fusion.
- **Recall path:** daemon `EngineHandle::recall()` `bossclawd/src/engine/mod.rs:521` → `ensure_indexed` (`mod.rs:501`) → `EventLog::recall` `log.rs:1436` → embed query (`log.rs:1444`) → vector arm (`log.rs:1445` → `index.rs:158`) + keyword arm (`log.rs:1446` → `log.rs:1359`) → fuse → boosts → `NoopReranker` (`log.rs:1566`) → sort/filter/truncate.
- **Migration:** `reembed_migration(embedder)` `log.rs:1834` — signed `config` event (`log.rs:1847-1862`), `rederive_pending` backfills only missing `(event, model_id)` rows (`log.rs:1133`, idempotent/resumable), GC other-model rows (`log.rs:1873`), rebuild (`log.rs:1880`). **No automatic caller — BUT `run_ingest` already handles a model-id mismatch via record-only `set_active_model`** (`bossclawd/src/engine/mod.rs:456-463` → `log.rs:1917`, doc: "without the re-embed/GC"). Any auto-migration design must reconcile with this existing writer. `active_model()` reader exists and returns `Ok(None)` on a fresh brain (`log.rs:1057-1073`).
- **Index:** `VectorIndex` trait is keyed by `event_id: &str` throughout (add/search/remove/last_indexed, `index.rs:50-75`); `HnswIndex::add` de-dups by `event_id` (`index.rs:143`); rebuild reads `vectors_for_model` ordered `event_id ASC` (`log.rs:1183-1190`); HNSW rebuilt on open by design (privacy, `index.rs:1-8`); deep-rank not cross-session deterministic (OS-seeded RNG, `index.rs:93-101`); `search` uses `ef = max(requested, 64)` (`index.rs:169`).
- **Normalization contract:** HNSW cosine (`index.rs:103`); Model2Vec normalizes (`model2vec.rs:66`). Any new embedder MUST L2-normalize.
- **Desktop/bundle model-path sites (rung 2 must touch ALL):** `apps/desktop/src-tauri/tauri.conf.json:29` (bundle glob `resources/models/potion-base-8M/*`), `apps/desktop/src-tauri/src/main.rs:80,88` (resource path), `bossclawd/src/main.rs:200` (default dir), `scripts/fetch-model.sh:8-9,20-22` (DEST/BASE + three committed sha256 pins, fail-closed).
- **Korean nuance:** Korean has spaces → `unicode61` tokenizes it (suboptimally but functionally); the measured KO failure is attributed to the English-only embedder. Spaceless-CJK segmentation is out of scope.

## 3. Design

### 3.0 Frozen measurement protocol (prerequisite for every gate — finding #1)

All Phase 1 gates run under one frozen measurement context, created once at Rung 0:

1. **Frozen corpus snapshot:** one-time copy of `~/brain` → `~/.air-harness/phase1-corpus/` at Phase 1 start. Every rung run uses `--corpus ~/.air-harness/phase1-corpus`. The snapshot id (corpus manifest sha) is recorded in every report; gates citing runs with different snapshot ids are invalid.
2. **Frozen case list:** the harness gains `--save-cases <path>` (serialize the built `QueryCase` list — mined + synthetic — as JSONL after generation) and `--cases <path>` (load it, skipping mining + synth generation entirely). Rung 0 generates and saves the canonical Phase 1 case list ONCE; every rung run loads it. This removes both churn sources (page-list drift and non-deterministic Ollama query text) AND makes post-freeze runs faster (no synth stage).
3. **Per-case results for paired stats:** in known-item mode, `scores.json` gains a per-case array (case id, segment label, per-arm gold rank/success flag). Rung gates compare rung N vs rung N-1 with the existing `wilcoxon_signed_rank` applied to the paired per-case AIR success flags across the two runs (same frozen cases ⇒ genuinely paired). The AIR-side cross-rung delta is the gate signal; the GBrain arm (which still queries the LIVE gbrain index) is reported as a reference arm only — its drift cannot corrupt an AIR-vs-AIR gate.
4. **Determinism note:** HNSW deep-rank is not cross-run deterministic (OS-seeded RNG) — a small jitter floor remains even frozen; the paired test absorbs it (it is exactly the noise the p-value accounts for).

### 3.1 Rung 0 — harness fast mode + freeze tooling (crate `memharness`, dev-only)

*Hard prerequisite:* PR #72 (`--judge` flag) merged or rebased in — the branch is not currently an ancestor of this one.
*Changes:* `--known-item-only` (filter built cases to `gold_page_id.is_some()` before `run_queries`; skip the ANTHROPIC key requirement — edit site `main.rs:84-91`, condition becomes `local_only || known_item_only`); `--save-cases`/`--cases` (§3.0.2); per-case results in `scores.json` (§3.0.3); snapshot id in the report (§3.0.1).
*Expected wall-clock:* first (generating) run ~35–40 min (synth ~15 min + GBrain CLI ~4s × ~338 known-item cases ≈ 22 min + prep/ingest); subsequent frozen runs ~25 min (no synth). The ~1h open-answering stage disappears in this mode.
*Gate (functional):* a frozen known-item run completes; re-running with `--cases` reproduces the identical case list (asserted by case-list sha in the report); known-item segments match a full run's shape; `cargo test -p memharness` green with CLI + hermetic tests for the new flags. **Closes with the frozen Phase 1 baseline run** (the numbers every later rung is paired against).

### 3.2 Rung 1 — query-term tokenization + fetch-cap lift (crate `bossclaw-core`)

*Change A — `keyword.rs`:* per-term quoting: split on Unicode whitespace, escape each term's internal quotes, emit `"t1" OR "t2" … OR "tN"`. Injection safety preserved (every user token stays inside quotes; operators are program-emitted). Empty-after-tokenization keeps current behavior. Critic-verified engine-safe to 5,000 terms on the bundled SQLCipher build.
*Change B — `recall.rs:161`:* `FUSION_FETCH: 50 → 200` (named const; comment records the 880-page rationale, re-tunability, and — for rung 3 — that its unit becomes "chunks before fold-back" with the over-fetch rule in §3.4).
*Gate (frozen, paired):* synthetic·en paired AIR success flags improve vs the Rung-0 frozen baseline (Wilcoxon p<0.05); synthetic·ko shows no significant regression. Directional segments reported.
*Tests:* multi-term OR emission; quote escaping; operator-injection attempts (`OR`, `NEAR`, `*`, `"`); empty query; punctuation-only term (`"foo - bar"` → `-` token); Korean terms; a very long multi-line mined-style query.

### 3.3 Rung 2 — multilingual embedder swap (crates `bossclawd` + app resources)

*Probe first (before any code):* verify `minishlab/potion-multilingual-128M` — license (expect MIT), artifact size, the three standard Model2Vec files load via the existing `from_pretrained` (critic-verified format-flexible), runtime dim, KO/EN smoke similarities. Fallback if the probe fails: evaluate alternatives (other Model2Vec multilingual variants, then `fastembed` multilingual) — design holds, only the model changes.
*Changes:*
1. Model artifacts: `scripts/fetch-model.sh` DEST/BASE + regenerate all three sha256 pins (`fetch-model.sh:8-9,20-22`); `tauri.conf.json:29` bundle glob; desktop resource paths (`apps/desktop/src-tauri/src/main.rs:80,88`); daemon default dir (`bossclawd/src/main.rs:200`); `MODEL_ID` (`bossclawd/src/engine/embed.rs:16`). One named task enumerates all six sites.
2. **Boot auto-migration (reconciled with the existing writer — findings #3/#4):** on daemon boot, BEFORE `ensure_indexed` can serve recall (`mod.rs:501-516`): if the `vectors` table has **zero rows for the compiled `MODEL_ID`** while embeddable events exist → run `reembed_migration(embedder)`. Triggering on actual vector rows (not the config event) is immune to the `set_active_model` record-only stamp. The `run_ingest` mismatch path (`mod.rs:456-463`) is changed to invoke the same migration entry point (or removed in favor of boot-time reconciliation) — one writer, not two. Progress logged; crash-resume via `rederive_pending`'s missing-rows semantics.
3. Old-model vectors stay inert via model-id filtering (`log.rs:1190`) — correctness holds; the migration window is availability-only (recall during migration waits or serves keyword-only, decided in the plan with a test pinning the chosen behavior).
*Gate (frozen, paired):* synthetic·ko paired AIR success flags improve vs Rung 1 (Wilcoxon p<0.05) AND synthetic·ko s@10 ≥ the same-run GBrain reference arm; synthetic·en shows no significant paired regression.
*Migration correctness is NOT gated by the harness* (fresh daemon per run never exercises it — finding #4): dedicated integration tests cover id change, **dim change**, mid-migration crash resume, fresh brain (no config event → `active_model()` `Ok(None)` path), and boot-before-ingest vs ingest-before-boot ordering.

### 3.4 Rung 3 — chunking (crate `bossclaw-core`; the big rung)

*Unit:* heading/paragraph-aware chunks, fixed char budget + overlap (named consts pinned in the plan; ~1,500/200 indicative), char-boundary-safe for Korean; text ≤ budget ⇒ exactly one chunk.
*Design rules (findings #6, both reviewers):*
1. **Single effective id:** ONE compiled string constant — the "effective model id" `"<model>+chunks-v1"` — is used everywhere a model id is written, read, or compared: `derive_vector` (`log.rs:1116`), `rederive_pending` (`log.rs:1169`), `vectors_for_model` (`log.rs:1190`), `reembed_migration` (`log.rs:1853`), and the rung 2 boot trigger. `MODEL_ID` itself becomes this string (or all sites read a shared `effective_model_id()`). This makes rung 2's boot migration fire for rung 3 with zero new machinery — and a test asserts write-id == read-id == trigger-id.
2. **Composite key encoding:** the `VectorIndex` trait stays `&str`-keyed; chunk slots are string-encoded `"{event_id}\x1f{chunk_ix}"` (unit separator cannot appear in event ids). `HnswIndex::add`'s de-dup (`index.rs:143`) then de-dups per chunk naturally. `last_indexed` semantics defined over the encoded keys.
3. **Fold-back INSIDE `vector_search`:** the collapse to best-score-per-`event_id` happens inside `vector_search` before it returns (`log.rs:1283`/`log.rs:1445`) — so `resolve_arms`, `vector_set` (`log.rs:1447-1454`), fusion, boosts, and the pages/files filter all see bare `event_id`s and remain untouched. RRF double-voting is structurally impossible.
4. **Over-fetch rule:** `vector_search` fetches `FUSION_FETCH × CHUNK_OVERFETCH` chunk slots (const, e.g. 4) and folds, so fusion still receives up to FUSION_FETCH distinct events even when hot docs contribute many chunks.
5. **Schema:** `vectors` PK → `(event_id, model_id, chunk_ix)` via SQLite table-rebuild migration (ALTER cannot change a PK); `chunk_ix INTEGER NOT NULL DEFAULT 0`; the `INSERT OR REPLACE` (`log.rs:1167`) and `vectors_for_model` SELECT/ORDER BY (`log.rs:1190`, order extended to `(event_id, chunk_ix)`) updated.
6. **FTS invariance:** the contentless FTS write path (`log.rs:492` + `fts_map`) indexes whole-doc text and is NOT touched by chunking (keyword arm stays whole-doc).
7. **Rebuild budget:** chunking multiplies vector rows ~4–8× at this corpus size; the report/log captures index-rebuild wall-clock, with a stated target (rebuild of the frozen corpus < 30 s on Peter's machine — measured, revisited if exceeded).
*Gate (frozen, paired):* synthetic·en paired improvement vs Rung 2 (Wilcoxon p<0.05); no significant synthetic·ko regression; real segments directional. **Phase 1 closes with one FULL harness run** (all segments, cloud judge, live `~/brain`) recorded as the new program baseline.

## 4. Testing

- Every rung: TDD in the touched crate; `cargo test --workspace`, `clippy -D warnings`, scoped `cargo build -p bossclawd` feature-leak gate (Phase 0 Task 46 set) green.
- Rung 0: CLI parse tests for the three new flags; hermetic test that `--cases` round-trips byte-identically (case-list sha asserted); zero-opens report path (existing `report.rs:186` test extended).
- Rung 1: the §3.2 test list.
- Rung 2: the §3.3 migration integration tests (id change, dim change, crash resume, fresh brain, ordering) + all-six-sites touch test (grep-style guard that no `potion-base-8M` literal survives outside fetch-model history).
- Rung 3: chunker property tests (budget/overlap/char-boundary/KO); single-effective-id test; fold-back test (multi-chunk doc appears once, best score, downstream boosts/filters still apply); over-fetch test (hot doc doesn't starve distinct events); determinism (rebuild order over `(event_id, chunk_ix)`); rebuild-budget measurement.
- Measurement: every gate cites report paths + snapshot id + case-list sha + the paired Wilcoxon result; k=10, seed=42 everywhere.

## 5. Out of scope (later specs)

Local-or-cloud embedder/answerer choice UI + consent (Peter-approved principle; own spec after Phase 1). Reranker activation, score-aware fusion, HNSW persistence, spaceless-CJK FTS segmentation, judge improvement (reasoning-before-verdict experiment), answer-quality (open-query) gating, GBrain-arm freezing (accepted as reference-only under §3.0.3).

## 6. Risks

- **Multilingual model quality unproven on this corpus** — probe + measured gate; named fallbacks without redesign.
- **Frozen snapshot ages** — Phase 1 is scoped to weeks; the closing FULL run re-baselines on the live brain. If Phase 1 stretches, re-freeze deliberately (new snapshot id = new baseline run, never silently).
- **OR-semantics noise on long queries** — BM25 IDF mitigates; the frozen paired gate is now actually capable of catching an EN regression (pre-freeze it was not); fallback AND-of-top-terms documented.
- **Boot auto-migration latency on a large brain** — logged, resumable, once per model change; availability behavior during migration pinned by test (§3.3.3).
- **Chunk-inflated rebuild latency** — measured against a stated budget (§3.4.7).
