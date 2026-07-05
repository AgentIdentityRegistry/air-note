# AIR Agent Memory Strategy Phase 0 — `memharness`: the blind A/B measuring stick

**Date:** 2026-07-03
**Status:** Rev 2 — revised after architect+critic review (2026-07-03)
**Crate under change:** new `crates/memharness` (dev-only tool; never ships). Plus ONE tiny feature-gated addition to `bossclawd`'s `test-helpers` (`test_engine_with_embedder`) so the harness measures the REAL embedder. No app changes.

## Program context (why this exists)

The approved memory strategy (canonical: GBrain `air/memory-strategy-2026-07-03-beat-the-stack`) reordered the roadmap to **measure first**: before any retrieval investment, build a blind end-to-end QA harness that replays Peter's REAL queries against both his current stack (GBrain, in its daily configuration) and AIR's engine, on the identical corpus. Verified 2026 research shows retrieval-recall headlines overstate usable memory quality by ~30pp and that "obvious" upgrades (rerankers) can *degrade* tuned pipelines — so every subsequent phase gets A/B'd against this harness instead of trusted on reputation. The North Star it serves (Peter, verbatim, in the strategy page): AIR Agent must make Claude Code/Codex "just never forget," and claims about being better than the current stack must be numbers, not vibes.

## Goal / non-goals

**Goal:** a one-command local tool that produces a per-run markdown report answering: *on Peter's own corpus and queries, end-to-end, does AIR's engine beat GBrain's daily pipeline — by how much, on which segments, and can we trust the judge?*

**Non-goals (explicitly out of scope):**
- Any retrieval improvement (embedder/reranker/chunking/fusion — Phase 1, gated on this harness's baseline).
- The Claude Code client / one-click integration (Phase 2).
- Importing into Peter's LIVE AIR brain (the harness uses an isolated brain; user-facing import is a later feature).
- CI wiring, UI, packaging (local dev tool only).
- Publishing results: **reports contain brain-derived content and MUST NOT be committed** to this public repo (see Reports).

## Design

### 1. Crate + isolation

- New workspace member `crates/memharness` (lib+bin, `#![forbid(unsafe_code)]`). Depends on `bossclawd` **with the `test-helpers` feature** + `bossclawd-proto`. This is acceptable ONLY because memharness is dev tooling that never ships; the feature exposes the hermetic engine constructor (in-memory vault, no OS keychain) and the production `run_accept_loop`. **Rev 2 — dependency form resolved explicitly:** memharness needs the helpers at RUNTIME (the live run spins the daemon), so `test-helpers` rides a NORMAL dependency — a deliberate divergence from the desktop's dev-dep precedent (which only needs the helpers at test time), documented in memharness's Cargo.toml. Shipped-binary safety: the helper items are `#[cfg(any(test, feature = "test-helpers"))]`, and the shipped daemon is built SCOPED (`cargo build -p bossclawd`, as `scripts/dev-build-signed.sh` does), where memharness's feature request does not participate in resolution — so workspace feature-unification cannot leak helper code into shipped binaries. The plan's final gates assert the scoped no-feature build.
- Per run, the harness spins an **in-process daemon** on a private socket under a per-run temp dir, with a fresh harness brain home. Real wire ops (`Hello` → `AddGrant` → `RunIngest` → `Recall`) drive everything — the same dispatch path production uses — so the measurement includes AIR's real seams, not a shortcut.
- Everything is ephemeral except reports. Fresh ingest per run keeps runs deterministic and sidesteps DEK persistence (866 pages through the current static embedder is minutes).
- **The AIR arm measures the REAL production embedder — never the mock (Rev 2, review-critical).** `test_engine` hardwires `MockEmbedder::new(8)` (a dim-8 hashed bag-of-words toy); comparing that against GBrain would poison every roadmap decision. `bossclawd`'s `test-helpers` gains `test_engine_with_embedder(home, provider)` (feature-gated, TDD'd), and the harness live path constructs the production `ResourceModel2Vec::new(model_dir)` (potion-base-8M) with the model dir resolved `BOSSCLAWD_MODEL_DIR` override → repo `apps/desktop/src-tauri/resources/models/potion-base-8M` fallback, plus a preflight that the dir + `model.safetensors` exist (mirroring the Ollama preflight). The in-memory vault stays (keychain-free). Hermetic tests MAY keep the mock, marked explicitly "plumbing test only — quality numbers come from the live run with the real embedder."

### 2. Corpus preparation

- Source: `~/brain` (the GBrain write-through mirror both systems can see; ~866 `.md` pages, ~32MB).
- The harness copies `*.md` into the harness home, **stripping YAML frontmatter CONDITIONALLY (Rev 2)**: Probe A verifies whether GBrain actually strips frontmatter before chunking. If GBrain strips → the harness strips (fairness = indexing the same text). If GBrain indexes frontmatter → the harness does NOT strip. The stripper is a probe-pinned flag, not an assumption. Skips dotfiles/dirs (`.obsidian`, etc.) either way.
- A **corpus manifest** (file count, total bytes, per-file sha256, snapshot timestamp) is recorded in the report for reproducibility. GBrain is NOT re-synced by the harness — it queries GBrain as-is and records `gbrain --version` + page count. **Corpus drift (Rev 2): a page-count delta >5% between `~/brain` and GBrain's index renders an INVALID-RUN banner at the top of the report** (the comparison is not apples-to-apples), not a footnote; ≤5% is reported as a delta.

### 3. Query set builder

Two sources, tagged separately end-to-end:

- **Real queries (primary):** mine `~/.claude/projects/**/*.jsonl` transcripts for `mcp__gbrain__{query,search,recall}` calls (recon 2026-07-03: 118 calls across 40 files). Capture query text, timestamp, session. **Implicit relevance labels:** a `mcp__gbrain__get_page` call within the next N=5 tool calls of the same session marks that page as the used answer (known-item label). Dedup exact + near-duplicates.
- **Synthetic known-item queries (volume):** the local LLM generates 1–2 queries per sampled page (stratified across top-level categories AND language — Korean queries for Korean-content pages), where the source page is the gold answer. Target ~200–400. This is the single-user statistical-power mitigation from `air/reranker-personalization-eval-2026-06-25`.
- Every query carries segment tags: `real|synthetic`, `en|ko|mixed`, `known-item|open`.

### 4. The two arms (fairness rules)

- **AIR arm:** `Recall { query, k=10 }` over the socket → hydrate hit texts → map each hit to its source page id → context pack. **Retrieval-k == scoring-k (Rev 2, stated explicitly): the same single `--k` value is used for retrieval on both arms AND for success@k scoring.**
- **GBrain arm:** `gbrain query --limit 10` in **`balanced`** mode (Peter's daily driver; `zerank-2` reranker OFF — this matches what AIR must actually beat day-to-day). Probe A verifies whether `balanced` is the default or needs an explicit mode flag — the arm must pin the right pipeline. Optionally record a `tokenmax` secondary arm (reranker ON) in the same run for reference. Output chunks parsed from the CLI (format verified during implementation; a parse failure is a run error, never silently scored).
- **Identical answerer:** both arms synthesize the final answer with the SAME local Ollama model, same prompt template, same context budget. **Context budget (Rev 2): budgeted by NUMBER OF HITS (k), not characters** — a char budget would reintroduce a chunk-vs-page truncation confound (GBrain returns chunks, AIR returns page-level snippets). All k hits are packed on both arms; a single per-snippet safety cap (identical on both arms, for the local model's context window) is the only truncation, and per-arm pack stats (sources packed, snippets truncated, chars packed) are recorded in the report with the granularity difference noted explicitly. GBrain's own cloud `ask` is NOT used — it would confound retrieval quality with answerer quality and add egress.
- Same k, same corpus text, same answerer ⇒ the only variable is retrieval + memory organization.

### 5. Scoring + the judge-trust contract

- **Known-item queries (mechanical, no judge):** success@k and MRR — did the gold page appear in the retrieved set. Page identity is normalized to the `~/brain`-relative path stem on BOTH arms. **AIR's `event_id → page_id` bridge (Rev 2, review-critical — designed, not hand-waved):** after ingest, `ListFiles` returns `Vec<FileRecordMirror { canonical_path, file_event_id, .. }>`; the harness builds `HashMap<file_event_id → canonical_path>`, strips the harness-corpus prefix, and normalizes to the page id. **Load-bearing invariant: the harness never runs an evolve tick, so every recall hit is a `file_ingested` event whose `event_id` equals a `file_event_id`.** If a hit does not map, that invariant broke → the run FAILS LOUD (unmapped hit = run error; there is NO silent event-id fallback). Retrieved hits are **deduped by page id (first occurrence wins)** before rank scoring so multi-hits from one page cannot distort success@k/MRR. GBrain slugs map via its `path ↔ slug` convention (pinned by Probe A). Applies to synthetic queries + the real queries with implicit labels.
- **Open queries (blind pairwise judge):**
  - Local judge (Ollama) scores each pair blind, **position-swapped** (2 judgments; disagreement between orderings = `uncertain`).
  - **Cloud audit** (Anthropic API, Peter's key): re-judges a **seeded random sample of `max(30, 15% of open queries)` ∪ ALL `uncertain` calls (deduped union) (Rev 2 floor)**. If the open-query pool itself is smaller than 30, the whole pool is audited and the report states "audit n too small; trust verdict indicative only." The audit call is preflighted (one-token Messages call with the pinned model id) BEFORE the expensive loop, so a bad key/model fails fast, not after 2h.
  - **Judge-trust verdict (Peter's explicit requirement):** every report leads with local-vs-cloud agreement on the audited set — raw agreement % and Cohen's kappa — against a stated threshold (**trusted = agreement ≥85% AND kappa ≥0.6**). Below threshold: the run auto-expands the cloud audit to 100% of open queries, and the report says plainly "the local judge is not yet trustworthy."
- **Stats:** per-segment win-rates with bootstrap CIs; Wilcoxon signed-rank on paired per-query scores; honest small-n flags (the ~100 real open queries may be underpowered — reported, not hidden, per the June eval research).

### 6. Egress + consent

- Default mode = hybrid (matches Peter's decision): cloud audit ON, using `ANTHROPIC_API_KEY` from env; the report states exactly how many query/answer pairs egressed. `--local-only` flag disables all egress ($0, wider error bars, judge-trust section reports "no audit this run").
- No other network use. GBrain arm may itself call ZeroEntropy per its own config — that's Peter's existing stack behaving normally, noted in the report.

### 7. Reports

- Written to `~/.air-harness/reports/<timestamp>/report.md` (+ raw scores JSON). **Never** into the repo (public; reports quote brain content). The repo gets only the tool.
- Report structure: judge-trust verdict → headline per-segment table (AIR vs GBrain-balanced [vs GBrain-tokenmax]) → EN/KO split (the expected bilingual gap made visible) → known-item vs open → 5 example wins/losses each with retrieved-context diffs → corpus manifest + config + versions.

### 8. Known limitations (accepted for Phase 0)

- HNSW deep-rank nondeterminism across rebuilds (documented engine behavior): mitigated by fixed query order + reporting success@k/MRR (rank-1-stable) over fine-grained rank metrics.
- AIR's current embedder is English-centric; KO segments will likely lose badly in the baseline. That is a *finding*, not a bug — it feeds Phase 1's multilingual embedder decision.
- Local judge quality is unknown until the first audit — hence the trust contract.
- GBrain's index vs `~/brain` mirror may drift slightly (≤5% reported as a delta; >5% renders the INVALID-RUN banner, Rev 2 §2).
- **Wilcoxon on binary outcomes (Rev 2):** known-item segments feed binary success flags into the signed-rank test — real data is almost all ties/equal-magnitude differences, so the tie-corrected normal approximation is coarse there. Reported as a caveat on known-item segments (a sign-test-like reading); the small-n (`n_nonzero < 25`) approximation flag is also surfaced per segment.

## Acceptance criteria

1. `cargo run -p memharness -- run` (defaults) completes on Peter's machine in ≤ ~2h, producing the report.
2. The report contains: judge-trust verdict with agreement+kappa, per-segment AIR-vs-GBrain results with CIs, EN/KO split, ≥100 real + ≥200 synthetic queries, corpus manifest.
3. Zero writes to Peter's live AIR brain, live GBrain, or `~/brain`; zero repo files containing brain content.
4. `--local-only` works (no egress); default hybrid audits `max(30, 15%)` ∪ uncertains via Peter's key.
5. All engine interaction over the real wire protocol (no private engine shortcuts).
6. **(Rev 2)** The AIR arm runs the PRODUCTION embedder (`ResourceModel2Vec`, potion-base-8M); mock embedders appear only in hermetic plumbing tests, each marked as such.
7. **(Rev 2)** Every AIR recall hit maps to a page id through the `ListFiles` bridge; an unmapped hit fails the run loud (no silent zero-scores).

## Open questions (resolve during planning/implementation)

- Which local model judges/answers: default = the engine's own local reasoner model class (`qwen2.5:7b`-tier, what the evolve loop already uses), overridable via `--model`; the plan's first task verifies availability via `ollama list` and fails with clear instructions if absent.
- `gbrain query` CLI output format for chunk extraction (verify + pin during Task 1 of the plan).
- Whether the 118 mined calls dedup to enough real open queries; if <50, weight the synthetic set higher and say so in reports.
