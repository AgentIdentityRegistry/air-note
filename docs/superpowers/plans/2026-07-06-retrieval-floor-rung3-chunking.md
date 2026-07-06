# Retrieval Floor Phase 1 — Rung 3 (chunking) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Every task is TDD: the RED test is committed BEFORE its implementation.

**Status:** Rev 2 (revised after architect + critic dual review). Architect: SOUND-WITH-CHANGES; critic: APPROVE-WITH-CHANGES; spine verified correct (no rework). → subagent execution.

**Goal:** Ship heading/paragraph-aware chunking of embeddable text in `bossclaw-core` (spec §3.4), measured PASS on the frozen harness (synthetic·en paired improvement, no synthetic·ko regression), then ship it safely to EXISTING brains via a schema table-rebuild + boot auto-migration. The base embedder stays potion-base-8M (dim 256, unchanged); the effective model id becomes `"minishlab/potion-base-8M+chunks-v1"`.

## Rev 2 change summary (architect SOUND-WITH-CHANGES + critic APPROVE-WITH-CHANGES)

| # | Finding (sev) | Where fixed |
|---|---|---|
| X1 | (CRIT, both) A mid-EVENT crash leaves partial chunks: the row-based boot trigger sees count>0 (dead) AND `collect_pending`'s event-granular LEFT JOIN (log.rs:1625) skips the event forever → tail chunks lost permanently | Task A8: BOTH write paths wrap "DELETE (event_id, model_id) rows + INSERT all chunks" in ONE per-event transaction (`unchecked_transaction`, precedent log.rs:1315) → an event is atomically all-chunks-or-zero. Task B5 RED test crashes MID-event; dropped the "sweep converges partial state" language (atomicity is the mechanism) |
| X2 | (CRIT, architect) HNSW seeds its RNG from OS randomness per build (index.rs:93-98); the 4-8× bigger chunked index is STRUCTURALLY noisier than the rung-1 baseline → a paired p<0.05 could credit graph-size jitter as a chunking win. seed=42 does NOT reach the HNSW RNG (no seed API) | Task A-gate: mandatory sub-step runs the candidate ≥3× on frozen inputs, reports the synthetic·en s@10 jitter band; the gate PASSES only if the chunking delta EXCEEDS the run-to-run band |
| X3 | (CRIT, architect) Fixed `CHUNK_OVERFETCH=4` starves distinct events: the frozen corpus has a 1.55M-char KO gold page (~1000+ chunks) + 36k-char EN pages (~24 chunks); `k×4` slots collapse to <k distinct events when fat docs dominate → reintroduces the exact ceiling rung 1 lifted | Task A9/A10: `vector_search` over-fetch is ADAPTIVE — fetch `min(index_len, k×mult)`, fold, grow the multiplier until distinct ≥ k or index exhausted; realistic fat-tail RED test at small k |
| X4 | (MAJOR, critic M1) Boot-hook site is a compile-blocker: the first-open closure is a SYNC `spawn_blocking` (mod.rs:347-358) capturing only keystore+db_path by move — no `self`, `.await`, or async in scope | Task B4 Step 2: run migration AFTER `spawn_blocking` returns the `Arc<EventLog>` (mod.rs:360-364, before `*guard = Some`), where `self`, `.await`, and the opened `log` are all in scope; migration in its OWN `spawn_blocking` |
| X5 | (MAJOR) "Has embeddable events" guard must not full-scan + JSON-parse every event on first open | Task B4 Step 2: cheap `SELECT COUNT(*) FROM events WHERE event_type IN (...)`, no deserialize |
| X6 | (MAJOR, architect M2) The "ingest-before-boot" ordering test is untestable as framed (after a real ingest, effective-id rows exist so the trigger correctly does NOT fire) | Task B3: rewritten to the REAL hazard — seed OLD-id rows, boot → migration runs + old rows GC'd; variant with SOME effective-id rows pre-boot → remaining old-only events backfilled, no event left old-id-only. Dropped the `set_active_model` framing |
| X7 | (MAJOR, architect M3) Worry that 1,500 chars overflows potion-base-8M's context (silent KO tail truncation) | Confirmed NON-issue: model2vec `StaticModel` mean-pools tokens, `seq_length: 1000000` (no transformer window) → no truncation; the budget is a GRANULARITY knob. A1 documents it as a measurement subject; A-gate adds a cheap token-count PROBE to confirm no truncation. 1,500 NOT lowered |
| X8 | (MAJOR, architect M4) A real irreversible schema change while `SCHEMA_VERSION=1` (log.rs:45) makes the persisted `schema_version` config field a lie | Task B2: bump the constant to 2; gate the rebuild on `stored_schema_version < 2` IN ADDITION to the `PRAGMA table_info` idempotency check |
| m1–m10 | (MINOR) anchor `mod.rs:456-463` (not 455); `prime_switches` runs at L353; A1 mod-order phrasing; `derive_vector_for` transitivity note; drop `#[ignore]` escape hatch; `split_once`/`rsplit_once` consistency + 2-sep test; results-derived fold-back assert; `count_vectors_for_model` counts ROWS; <30s budget informational-only; "no test pins old MODEL_ID" | folded into A1/A4/A6/A8/A9/A-gate/B2/B3/B4 inline |

**Working branch:** `feat-retrieval-rung3-chunking` (off `origin/main` = `f6c4cbc`; rung 1 merged, FUSION_FETCH=200 + per-term keyword OR already in tree — verified on this branch). This branch currently holds ONLY the plan doc; the build lands the code here.

**Architecture (two phases, prove-the-win-first):** Phase A builds the chunking CORE — the chunker, the single effective-model-id, the composite `(event_id, model_id, chunk_ix)` key, the fold-back-to-best-score-per-event INSIDE `vector_search`, the over-fetch rule, and the FTS-invariance guard — and ENDS with the frozen-harness measurement gate. The memharness spins a FRESH daemon + FRESH DB per run (`crates/memharness/src/daemon.rs:82` = `tempfile::tempdir()`), so its `vectors` table is CREATE-d new with the new PK from the start — **no schema migration is exercised by the measurement**. Phase B (built only if Phase A's gate passes) is the migration machinery a fresh DB never needs but a real user's existing brain does: a schema table-rebuild (`vectors` PK `(event_id, model_id)` → `(event_id, model_id, chunk_ix)`) and the boot auto-migration (spec §3.3.2) that re-chunks + re-embeds existing events under the new effective id and GCs the old-id rows.

**Tech Stack:** Rust 2021. Zero new dependencies (the chunker is pure std; migration uses the existing `rusqlite`/`sha2`/`serde` already in the crate).

**Spec:** docs/superpowers/specs/2026-07-06-retrieval-floor-phase1-design.md (Rev 2, §3.4 Rung 3; §2 verified reality, §3.0 frozen protocol, §3.3.2 boot migration are load-bearing context).

---

## CRITICAL sequencing note (this plan INVERTS the spec's Rung 2→3 order)

The spec §3.4.1 assumed Rung 2 (the multilingual embedder swap) shipped FIRST and built the boot auto-migration that Rung 3 would then reuse "with zero new machinery." **Product decision: chunking (Rung 3) ships FIRST; the multilingual swap (Rung 2) is deferred.** Consequences baked into this plan:

1. **Rung 3 now OWNS the boot auto-migration** described in spec §3.3.2 (the "zero `vectors` rows for the compiled effective `MODEL_ID` while embeddable events exist → run `reembed_migration`" path, reconciled with the record-only `set_active_model` writer at `bossclawd/src/engine/mod.rs:456-463`). It was going to be Rung 2's; it is Rung 3's now. This is Phase B.
2. **No dim change in this rung.** The base embedder is unchanged (potion-base-8M, dim 256). Only the effective *string id* changes (`…+chunks-v1`). A dim-change migration test is included as forward-proofing (Task B7) but is explicitly labeled NOT triggered by this rung — it belongs to the deferred Rung 2.
3. **When Rung 2 later swaps the embedder,** it changes only the base id in `effective_model_id()` and reuses Rung 3's migration verbatim (the effective id changes → the same "zero rows for the compiled id" trigger fires → the same table + re-embed migration runs). See Forward notes.

---

## Verified current anchors (re-verified by reading each file 2026-07-06 — line numbers had DRIFTED from the spec's stated values; the corrected values below are authoritative for this plan)

`crates/bossclaw-core/src/log.rs`:
- `SCHEMA_VERSION: u32 = 1` — **L45** ("reserved"; format-gating deferred; no migration framework exists).
- `vectors` CREATE TABLE — **L474-481**, PK `(event_id, model_id)` at **L479**, `CREATE TABLE IF NOT EXISTS`.
- FTS contentless write path — `CREATE VIRTUAL TABLE … fts5(body, content='')` **L492**; `fts_map` **L495-499**.
- `active_model()` reader — **L1057-1073**; returns `Ok(None)` on a fresh brain (loop finds no config → `Ok(None)`).
- `derive_vector()` — **L1101**; `INSERT OR REPLACE INTO vectors (event_id, model_id, dim, embedding)` at **L1114-1117** (params: `event.id`, `embedder.model_id()`, `embedder.dim()`, `blob`).
- `rederive_pending()` — **L1133**; its `INSERT OR REPLACE` at **L1167-1170**; backfills only events lacking a `(event, model_id)` vector (via `collect_pending`), idempotent/resumable, per-event best-effort.
- `vectors_for_model()` — **L1183**; `SELECT event_id, embedding FROM vectors WHERE model_id = ?1 ORDER BY event_id ASC` at **L1190**.
- `rebuild_indexes()` — **L1226**; vector half reads `vectors_for_model` and `index.add(&event_id, &vec)` at **L1232-1233**; FTS half reads `collect_embeddable_events_ordered` (whole-doc) at **L1247/L1264**.
- `vector_search()` — **L1283-1295**; a thin wrapper: `index.search(query_vec, k)` at **L1290**. **This is the fold-back seam.**
- `keyword_search()` — **L1359**; escapes via `keyword::escape_fts_query` at **L1367**, `WHERE fts MATCH ?1` at **L1374** (whole-doc, NOT chunked).
- `recall()` — **L1436**; arms run at **L1444-1447** (`vector_search(&qv, FUSION_FETCH)` at **L1445**); `resolve_arms` at **L1447**; provenance `vector_set`/`keyword_set` built at **L1451-1454** (BEFORE fusion — fold-back MUST happen inside `vector_search` so these see bare event_ids); fusion at **L1460**; boosts/kinds/pages/files filters all key on bare `id` at **L1521-1562**.
- `derive_vector_for()` — **L4314** (ingest convenience; loads payload, calls `derive_vector`). Callers: `ingest.rs:706,712`.
- `collect_pending()` — **L1625** (LEFT JOIN `vectors` on `(event_id, model_id)`; returns events with NO row for `model_id`, `seq ASC`).
- `collect_embeddable_events_ordered()` — **L1668** (whole-doc `(event_id, text)`, `seq ASC`; feeds FTS).
- `reembed_migration()` — **L1834**: config event **L1847-1862**, `rederive_pending` **L1865**, GC `DELETE FROM vectors WHERE model_id != ?1` **L1872-1875**, `rebuild_indexes` **L1880**. Uses `embedder.model_id()` throughout (so if that returns the effective id, GC removes old-id rows automatically).
- `set_active_model()` — **L1917** (records config id WITHOUT re-embed/GC; the record-only writer to reconcile).
- `count()` — **L864**; `embeddable_text()` — **L6910** (returns `content["text"]` for `memory`/`page` types; the chunker's text source).

`crates/bossclaw-core/src/index.rs`:
- `VectorIndex` trait — **L45**; `add(&mut self, event_id: &str, …)` **L50**, `search(&self, vec, k) -> Vec<(String, f32)>` **L63**, `remove` **L70**, `last_indexed` **L75** — ALL `&str`-keyed.
- `HnswIndex::add` de-dups via `id_to_slot.contains_key(event_id)` at **L143**; `id_to_slot`/`slot_to_id`/`last_indexed` at **L104-107**.
- Composite chunk keys are string-encoded `"{event_id}\x1f{chunk_ix}"` (unit separator `0x1f` cannot appear in event ULIDs). **The trait stays `&str`-keyed** — the encoding lives entirely in `log.rs`.

`crates/bossclaw-core/src/recall.rs`:
- `FUSION_FETCH: usize = 200` — **L166** (its doc already forward-notes the chunk-slot meaning). `RRF_K: f32 = 60.0` — **L98**. Add `CHUNK_OVERFETCH` here.

`crates/bossclawd/src/engine/embed.rs`:
- `MODEL_ID: &str = "minishlab/potion-base-8M"` — **L16**; passed to `Model2Vec::from_pretrained(&self.model_dir, MODEL_ID)` at **L42**. **Key finding:** `from_pretrained(dir, model_id)` takes the physical DIRECTORY and the reported id as SEPARATE args (`model2vec.rs:62-79`; `model_id()` returns exactly this string, `model2vec.rs:152`). So making `MODEL_ID` the effective id `…+chunks-v1` changes the DB `model_id` WITHOUT renaming the directory (`self.model_dir` is unchanged). No physical model-path site is touched by Rung 3.

`crates/bossclawd/src/engine/mod.rs`:
- `get_or_open()` first-open path — the `EventLog::open` runs inside a SYNC `tokio::task::spawn_blocking` (**L347-358**) that captures ONLY `keystore` + `db_path` by move; `prime_switches` is CALLED at **L353** (inside that sync closure — no `self`/`.await`/async in scope). The opened `Arc<EventLog>` is returned at **L360** and stored at `*guard = Some(log.clone())` **L364**. **The boot migration hooks in AFTER L360, BEFORE L364** — that is the only site where `self`, `.await`, and the opened `log` are all in scope (fixes X4).
- `run_ingest()` record-only mismatch handler — **L456-463** (`active_model()` vs `embedder.model_id()` at L456-459; if changed/absent → `set_active_model` at L460-462). Reconcile here.
- `ensure_indexed()` — **L501-516** (first-recall lazy rebuild; sets `*self.indexed` true only on success).
- `recall()` — **L521** → `get_or_open` → `ensure_indexed` → `spawn_blocking(log.recall)`.

`crates/bossclaw-core/src/model2vec.rs` (X7 — confirms the budget is a granularity knob, not a truncation risk):
- `Model2Vec::from_pretrained` → `StaticModel::from_pretrained` **L66**; the embedder MEAN-POOLS token embeddings (model2vec `StaticModel`), so there is NO transformer context window — the model's `config.json` carries `seq_length: 1000000` (effectively unbounded). A 1,500-char chunk cannot silently truncate; the budget only controls mean-pool dilution (granularity), which is exactly what the `+chunks-v1` measurement evaluates.

**Model-path sites that Rung 3 does NOT touch** (physical model DIRECTORY — unchanged for chunking; only the deferred multilingual Rung 2 touches these): `apps/desktop/src-tauri/tauri.conf.json:29`, `apps/desktop/src-tauri/src/main.rs:80,90`, `bossclawd/src/main.rs:200,205`, `crates/memharness/src/daemon.rs:35`, `scripts/fetch-model.sh`. The bossclaw-core test literals `"minishlab/potion-base-8M"` at `log.rs:7056,7059,7064,7065` are model-agnostic `set_active_model` tests, not the daemon's `MODEL_ID` — leave them.

---

## Preconditions

- Verify at start: `git status -sb` clean on `feat-retrieval-rung3-chunking`; `cargo test --workspace` green; `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Frozen artifacts (from rung 0, already on disk — do NOT regenerate): `~/.air-harness/phase1-corpus/` (corpus snapshot), `~/.air-harness/phase1-cases.jsonl` (396 cases, verified present). Record their identities from the report the FIRST run prints.
- **Phase A baseline for the paired compare** = a potion-base-8M **rung-1** (pre-chunking) run on the SAME frozen inputs. If the frozen rung-1 baseline `scores.json` from PR #74 is on disk, reuse it; otherwise Task A9 re-measures it first (checkout the pre-chunking tree, run, save — same corpus + `--cases`, so it is a valid paired baseline).
- Live-run prerequisites (Task A9 / A-gate + Phase B closing run): embedder model present at the repo fallback dir, `gbrain` on PATH (GBrain arm is reference-only). No ANTHROPIC key needed (`--known-item-only` skips it).

## File structure

| File | Responsibility |
|---|---|
| `crates/bossclaw-core/src/chunk.rs` (new) | Pure chunker: heading/paragraph-aware, char-budget + overlap, char-boundary-safe. No I/O, no SQL. Named consts. |
| `crates/bossclaw-core/src/lib.rs` | `pub mod chunk;` line. |
| `crates/bossclaw-core/src/recall.rs` | `CHUNK_OVERFETCH` const. |
| `crates/bossclaw-core/src/index.rs` | Composite-key encode/decode helpers (still `&str`-keyed trait). |
| `crates/bossclaw-core/src/log.rs` | Chunked write path (`derive_vector`/`rederive_pending`); `vectors_for_model` selects `chunk_ix`; fold-back inside `vector_search`; schema PK + `chunk_ix` column; **Phase B** table-rebuild migration + trigger. |
| `crates/bossclawd/src/engine/embed.rs` | `MODEL_ID` → effective id (single source), decoupled from the model dir. |
| `crates/bossclawd/src/engine/mod.rs` | **Phase B** boot auto-migration ordered before `ensure_indexed`; `run_ingest` reconciliation. |

---

# Phase A — chunking CORE (measurable on the fresh harness, no migration)

> Phase A ends at the measurement gate (Task A-gate). The fresh harness DB is CREATE-d with the new PK from the start, so NO schema migration runs here. Ship/no-go for Phase B is decided by the A-gate.

### Task A1: `chunk.rs` — chunker contract (RED)

**why:** Pin the chunk unit's exact behavior (budget/overlap/heading-split/short-doc/Korean char-safety) as executable contract before any implementation — the chunk boundaries decide what gets embedded, so they are the load-bearing decision of this rung.

**Files:** Create `crates/bossclaw-core/src/chunk.rs`; modify `crates/bossclaw-core/src/lib.rs` (add `pub mod chunk;` immediately after `pub mod actuator;` at L19 — alphabetical: `actuator` < `chunk` < `embed`).

- [ ] **Step 1: `lib.rs`** — add `pub mod chunk;` (alphabetical: immediately after `pub mod actuator;` at L19).
- [ ] **Step 2: Create `chunk.rs` with ONLY the module doc + named consts + tests** (the `chunk_text` fn arrives in A2):

```rust
//! Heading/paragraph-aware text chunking (retrieval-floor spec Rev 2 §3.4).
//!
//! One embeddable event's text is split into 1..N overlapping chunks so that a
//! long document contributes several focused embedding targets instead of one
//! averaged-out vector. Pure: no I/O, no SQL. The caller (`log.rs`) embeds each
//! returned chunk and writes it under a composite `(event_id, model_id, chunk_ix)`
//! key; recall folds the chunks back to one best-scoring hit per event.
//!
//! Invariants (asserted by the tests below):
//! - text whose char-length ≤ `CHUNK_BUDGET_CHARS` ⇒ EXACTLY ONE chunk equal to
//!   the input (short docs are byte-identical to today — no behavior change).
//! - splits prefer paragraph/heading boundaries; a paragraph larger than the
//!   budget is hard-split on a CHAR boundary (never a byte boundary — Korean and
//!   other multi-byte scripts must never be sliced mid-codepoint).
//! - consecutive chunks share `CHUNK_OVERLAP_CHARS` of trailing/leading context.
//! - chunk indices are dense and 0-based (`0..n`), stable for a given input.

/// Max chars per chunk. This is a GRANULARITY knob, NOT a context-window guard:
/// potion-base-8M is a model2vec `StaticModel` that MEAN-POOLS token embeddings
/// (no transformer window; `config.json` seq_length = 1_000_000), so a larger
/// chunk never truncates — it only DILUTES the mean over more tokens. Smaller
/// chunks = sharper, less-diluted matches; the win is measured by the frozen
/// gate (a v2 re-tune bumps the effective-id suffix and re-migrates). ~1,500
/// chars keeps most memory/page events at ONE chunk (common case unchanged).
/// Char count, never byte count. A measurement subject, not a tuned truth.
pub const CHUNK_BUDGET_CHARS: usize = 1_500;

/// Chars of overlap carried between adjacent chunks so a fact spanning a split
/// point still appears whole in at least one chunk. ~13% of the budget — enough
/// to bridge a sentence, small enough to avoid ~2× row inflation.
pub const CHUNK_OVERLAP_CHARS: usize = 200;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_exactly_one_unchanged_chunk() {
        let t = "A single short paragraph.\n\nTwo short paragraphs, still tiny.";
        let chunks = chunk_text(t);
        assert_eq!(chunks.len(), 1, "≤ budget ⇒ one chunk");
        assert_eq!(chunks[0], t, "short doc is byte-identical to input");
    }

    #[test]
    fn empty_and_whitespace_yield_no_chunks() {
        assert!(chunk_text("").is_empty(), "empty text ⇒ zero chunks (nothing to embed)");
        assert!(chunk_text("   \n\t \n").is_empty(), "whitespace-only ⇒ zero chunks");
    }

    #[test]
    fn long_text_splits_on_paragraph_boundaries_within_budget() {
        // Three paragraphs each ~700 chars; budget 1500 ⇒ para1+para2 in chunk 0,
        // para3 in chunk 1 (a paragraph is never split when it fits whole).
        let para = "x".repeat(700);
        let t = format!("{para}\n\n{para}\n\n{para}");
        let chunks = chunk_text(&t);
        assert!(chunks.len() >= 2, "3×700 chars must exceed one 1500-char chunk: {}", chunks.len());
        for c in &chunks {
            assert!(c.chars().count() <= CHUNK_BUDGET_CHARS, "each chunk within budget");
        }
    }

    #[test]
    fn oversized_paragraph_hard_splits_on_char_boundary_never_mid_codepoint() {
        // 4,000 Korean chars in ONE paragraph (no split points) forces a hard split.
        let t: String = "가".repeat(4_000);
        let chunks = chunk_text(&t);
        assert!(chunks.len() >= 3, "4000 KO chars over a 1500 budget ⇒ ≥3 chunks");
        for c in &chunks {
            assert!(c.chars().count() <= CHUNK_BUDGET_CHARS);
            // The load-bearing KO safety property: every chunk is valid UTF-8 with
            // only whole '가' codepoints — a byte slice would corrupt these.
            assert!(c.chars().all(|ch| ch == '가'), "no mid-codepoint slice: {:?}", &c[..c.len().min(8)]);
        }
        // Reassembling the chunks minus the overlaps recovers every codepoint.
        let total: usize = chunks.iter().map(|c| c.chars().count()).sum();
        assert!(total >= 4_000, "no characters dropped (overlap only adds)");
    }

    #[test]
    fn adjacent_chunks_overlap_by_the_configured_budget() {
        let para = "y".repeat(1_400);
        let t = format!("{para}\n\n{para}"); // 2 paras, each near-budget ⇒ 2 chunks
        let chunks = chunk_text(&t);
        assert!(chunks.len() >= 2);
        // The tail of chunk[i] and the head of chunk[i+1] must share overlap chars.
        let tail: String = chunks[0].chars().rev().take(CHUNK_OVERLAP_CHARS).collect();
        let head: String = chunks[1].chars().take(CHUNK_OVERLAP_CHARS).collect();
        let tail_fwd: String = tail.chars().rev().collect();
        assert!(
            chunks[1].starts_with(&head) && chunks[0].ends_with(&tail_fwd),
            "adjacent chunks carry {CHUNK_OVERLAP_CHARS} chars of shared context"
        );
    }

    #[test]
    fn chunking_is_deterministic() {
        let t = format!("{}\n\n{}", "z".repeat(2_000), "w".repeat(2_000));
        assert_eq!(chunk_text(&t), chunk_text(&t), "same input ⇒ same chunks (stable ix)");
    }
}
```

- [ ] **Step 3: Verify RED:** `cargo test -p bossclaw-core chunk::` — Expected: FAIL to compile (`chunk_text` not found).
- [ ] **Step 4: Commit** — `git add crates/bossclaw-core/src/chunk.rs crates/bossclaw-core/src/lib.rs && git commit -m "test(bossclaw-core): heading/paragraph-aware chunker contract — budget/overlap/KO char-safety/short-doc (RED)"`

### Task A2: `chunk.rs` — implement `chunk_text` (GREEN)

**why:** Turn the contract green with a char-boundary-safe splitter; this is the one place mid-codepoint slicing could corrupt Korean, so it uses `chars()` exclusively — never byte indexing.

**Files:** Modify `crates/bossclaw-core/src/chunk.rs` (insert between doc/consts and `#[cfg(test)]`).

- [ ] **Step 1: Implementation.** Design: (a) split the input into paragraph blocks on blank-line boundaries (`\n\n`+), preserving heading lines as their own block starts; (b) greedily pack whole blocks into a chunk until the next block would exceed `CHUNK_BUDGET_CHARS`; (c) a single block larger than the budget is hard-split into budget-sized windows on CHAR boundaries; (d) between emitted chunks, prepend the previous chunk's last `CHUNK_OVERLAP_CHARS` chars. All indexing is via `chars().collect::<Vec<char>>()` slices or `char_indices()` — NEVER `&s[i..j]` on a byte range that could land mid-codepoint. Sketch:

```rust
/// Split `text` into 1..N overlapping, char-boundary-safe chunks (spec §3.4).
/// Returns an empty Vec for empty/whitespace-only input. Text within budget
/// returns exactly `[text.to_string()]` (short docs unchanged).
pub fn chunk_text(text: &str) -> Vec<String> {
    if text.trim().is_empty() {
        return Vec::new();
    }
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= CHUNK_BUDGET_CHARS {
        return vec![text.to_string()]; // fast path: one unchanged chunk
    }
    // 1) block boundaries (paragraphs / headings). 2) greedy pack ≤ budget.
    //    3) hard-split any single oversize block on char windows.
    //    4) prepend CHUNK_OVERLAP_CHARS of the prior chunk's tail.
    // (Full implementation packs `blocks: Vec<&str>` and assembles `Vec<char>`
    //  windows so slicing is always on char boundaries.)
    // …
}
```

- Guard rails for the implementer: (i) never return an empty-string chunk; (ii) every returned chunk's `chars().count() <= CHUNK_BUDGET_CHARS`; (iii) overlap is derived from already-chunked char slices (so overlap can never split a codepoint); (iv) if `CHUNK_OVERLAP_CHARS >= CHUNK_BUDGET_CHARS` the code must still make forward progress (assert-documented invariant `OVERLAP < BUDGET`, add a `const _: () = assert!(CHUNK_OVERLAP_CHARS < CHUNK_BUDGET_CHARS);` compile-time guard).
- [ ] **Step 2: Run:** `cargo test -p bossclaw-core chunk::` — Expected: all 6 pass.
- [ ] **Step 3: Commit** — `git add -u && git commit -m "feat(bossclaw-core): char-boundary-safe heading/paragraph chunker (GREEN)"`

### Task A3: single effective model id (RED)

**why:** The write id, read id, and (Phase B) migration-trigger id MUST be ONE string (spec §3.4.1) — if they diverge the migration silently no-ops or recall reads an empty arm. A test pins write-id == read-id == trigger-id.

**Files:** Modify `crates/bossclawd/src/engine/embed.rs`.

- [ ] **Step 1: Add a RED test** in `embed.rs`'s test module (create a `#[cfg(test)] mod tests` if absent):

```rust
#[cfg(test)]
mod effective_id_tests {
    use super::*;

    #[test]
    fn model_id_is_the_effective_chunks_id() {
        // The single source of truth carries the +chunks-v1 suffix so vectors are
        // written, read, and (Phase B) migration-triggered under ONE id.
        assert_eq!(MODEL_ID, "minishlab/potion-base-8M+chunks-v1");
        // The base directory-loader id is separate and unchanged (still the HF slug),
        // so no model DIRECTORY rename is implied by the effective id.
        assert_eq!(BASE_MODEL_DIR_ID, "minishlab/potion-base-8M");
    }
}
```

- [ ] **Step 2: Verify RED:** `cargo test -p bossclawd effective_id` — Expected: FAIL to compile (`BASE_MODEL_DIR_ID` missing) / assert fail (`MODEL_ID` lacks suffix).
- [ ] **Step 3: Commit** — `git add -u && git commit -m "test(bossclawd): effective model id carries +chunks-v1; base dir id unchanged (RED)"`

### Task A4: single effective model id (GREEN)

**why:** Make `model_id()` return the effective id everywhere it is written/read/compared while keeping the physical model directory the same, by decoupling the two args of `from_pretrained`.

**Files:** Modify `crates/bossclawd/src/engine/embed.rs`.

- [ ] **Step 1: Split the constant** at L14-16:

```rust
/// The HF slug identifying the physical model artifacts on disk. Passed ONLY as
/// the directory-loader hint; NOT the id stamped on vectors. Unchanged by chunking
/// — the model files are the same potion-base-8M.
pub const BASE_MODEL_DIR_ID: &str = "minishlab/potion-base-8M";

/// The single source of truth for the EFFECTIVE embedding model id — the id
/// stamped on every `vectors` row, read back by the index rebuild, and (Phase B)
/// compared by the boot migration trigger. The `+chunks-v1` suffix means "same
/// weights, chunked write path": changing the chunking contract (e.g. a different
/// budget) bumps to `+chunks-v2` and the boot migration re-embeds. Ingest and
/// recall-open construct `Model2Vec` reporting THIS id (spec §3.4.1: write-id ==
/// read-id == trigger-id).
pub const MODEL_ID: &str = "minishlab/potion-base-8M+chunks-v1";
```

- [ ] **Step 2: Decouple the loader** at L42: `Model2Vec::from_pretrained(&self.model_dir, MODEL_ID)` stays — but `MODEL_ID` is now the effective id, which `model2vec.rs:76` stores as `self.model_id` and `model2vec.rs:152` returns via `model_id()`. The DIRECTORY is `self.model_dir` (unchanged), so the physical files still load from `potion-base-8M/`. Confirm no other site in embed.rs passes `MODEL_ID` as a path. (The `BASE_MODEL_DIR_ID` const is currently unused by the loader — it documents intent and is asserted by the A3 test; if clippy flags it dead, mark it `pub` (already) so it is a crate-public API item, which suppresses dead-code.)
- [ ] **Step 3: Run:** `cargo test -p bossclawd effective_id` then `cargo test -p bossclawd`. **Verified (critic-confirmed): NO bossclawd test currently pins the old `MODEL_ID` value**, so nothing else needs updating here — but grep `rg 'potion-base-8M"' crates/bossclawd/` before committing to reconfirm on the live tree, and if a new pin has appeared, update it to the effective id WITH a one-line justification. Also `cargo clippy -p bossclawd --all-targets -- -D warnings`.
- [ ] **Step 4: Commit** — `git add -u && git commit -m "feat(bossclawd): effective model id +chunks-v1 (decoupled from the physical model dir) (GREEN)"`

### Task A5: composite-key encode/decode (RED)

**why:** The `VectorIndex` trait stays `&str`-keyed; chunk slots are string-encoded `"{event_id}\x1f{chunk_ix}"`. Centralizing the encoding + its inverse (used by fold-back) in one tested place prevents the separator/parse logic from drifting between the write path and the fold-back.

**Files:** Modify `crates/bossclaw-core/src/index.rs` (add pure helpers + tests).

- [ ] **Step 1: RED tests** appended to `index.rs`'s `mod tests` (or a new one):

```rust
    #[test]
    fn chunk_key_round_trips_and_separator_is_ulid_safe() {
        let key = encode_chunk_key("01J8Z3ABCDXYZ", 7);
        assert_eq!(key, "01J8Z3ABCDXYZ\u{1f}7");
        let (id, ix) = decode_chunk_key(&key).unwrap();
        assert_eq!(id, "01J8Z3ABCDXYZ");
        assert_eq!(ix, 7);
        // 0x1f never appears in a Crockford-base32 ULID, so decoding is unambiguous.
        assert_eq!(decode_chunk_key("no-separator-here"), None);
        // Malformed index is rejected (not silently 0).
        assert_eq!(decode_chunk_key("id\u{1f}notanumber"), None);
    }

    #[test]
    fn event_id_of_and_decode_agree_on_split_direction() {
        // Both use the FIRST separator (split_once) — so a key with a stray extra
        // separator decodes/reduces identically. (event ULIDs never contain 0x1f,
        // so this only guards defensive/mixed-data robustness, not a real ULID.)
        let weird = "01J8Z3ABCDXYZ\u{1f}2\u{1f}5"; // two separators
        assert_eq!(event_id_of(weird), "01J8Z3ABCDXYZ", "reduce key = first field");
        // decode_chunk_key rejects it: the index field "2\u{1f}5" isn't a number.
        assert_eq!(decode_chunk_key(weird), None, "ambiguous multi-sep key is not a valid chunk key");
    }

    #[test]
    fn event_id_of_extracts_the_bare_id_for_foldback() {
        assert_eq!(event_id_of("01J8Z3ABCDXYZ\u{1f}3"), "01J8Z3ABCDXYZ");
        // A bare (non-chunk) key is returned unchanged — defensive for mixed data.
        assert_eq!(event_id_of("01J8Z3ABCDXYZ"), "01J8Z3ABCDXYZ");
    }
```

- [ ] **Step 2: Verify RED:** `cargo test -p bossclaw-core index::` — Expected: FAIL to compile.
- [ ] **Step 3: Commit** — `git add -u && git commit -m "test(bossclaw-core): composite chunk-key encode/decode + event_id_of (RED)"`

### Task A6: composite-key encode/decode (GREEN)

**why:** Provide the single encoding used by the write path and the fold-back so the two never disagree.

**Files:** Modify `crates/bossclaw-core/src/index.rs`.

- [ ] **Step 1: Implement** (near the top of index.rs, pub within the crate):

```rust
/// Unit-separator that joins an event id and a chunk index into one `VectorIndex`
/// key. `0x1f` (US) cannot appear in a Crockford-base32 ULID, so `event_id_of`
/// below can always recover the bare id — the property the fold-back relies on.
pub const CHUNK_KEY_SEP: char = '\u{1f}';

/// Encode `(event_id, chunk_ix)` as the composite `VectorIndex` key.
pub fn encode_chunk_key(event_id: &str, chunk_ix: usize) -> String {
    format!("{event_id}{CHUNK_KEY_SEP}{chunk_ix}")
}

/// Decode a composite key back to `(event_id, chunk_ix)`, or `None` if it is not
/// a well-formed chunk key (no separator, or a non-numeric index). Uses the FIRST
/// separator (`split_once`) so it agrees byte-for-byte with `event_id_of` on the
/// event-id field — a stray extra separator makes the index non-numeric ⇒ None
/// (never a silently-wrong parse). Event ULIDs never contain 0x1f, so real keys
/// always have exactly one separator.
pub fn decode_chunk_key(key: &str) -> Option<(&str, usize)> {
    let (id, ix) = key.split_once(CHUNK_KEY_SEP)?;
    Some((id, ix.parse().ok()?))
}

/// The bare event id for a composite (or already-bare) key — the fold-back reduce
/// key. A key without the separator is returned unchanged (defensive). Uses the
/// FIRST separator, matching `decode_chunk_key`.
pub fn event_id_of(key: &str) -> &str {
    key.split_once(CHUNK_KEY_SEP).map_or(key, |(id, _)| id)
}
```

- [ ] **Step 2: Run:** `cargo test -p bossclaw-core index::` — Expected: green.
- [ ] **Step 3: Commit** — `git add -u && git commit -m "feat(bossclaw-core): composite chunk-key codec (event_id \\x1f chunk_ix) (GREEN)"`

### Task A7: schema (fresh) + chunked write + `vectors_for_model` chunk-aware (RED)

**why:** On a FRESH DB (the harness case) the `vectors` table must be born with PK `(event_id, model_id, chunk_ix)` and `chunk_ix INTEGER NOT NULL DEFAULT 0`; the two write paths must emit one row per chunk; `vectors_for_model` must return chunk-encoded ids ordered deterministically over `(event_id, chunk_ix)` so rebuild is stable.

**Files:** Modify `crates/bossclaw-core/src/log.rs` (schema L474-481, `derive_vector` L1101, `rederive_pending` L1133, `vectors_for_model` L1183) — RED tests first.

- [ ] **Step 1: RED tests** in `log.rs`'s test module. Use a `MockEmbedder` (already present in the crate's tests) whose text→vector is deterministic. Assertions:
  - A memory event with text longer than `CHUNK_BUDGET_CHARS` (built from `chunk::CHUNK_BUDGET_CHARS`) produces **>1 rows** in `vectors` for the model, with `chunk_ix` dense `0..n` — query `SELECT chunk_ix FROM vectors WHERE event_id=?1 ORDER BY chunk_ix`.
  - A short memory event produces **exactly 1 row** with `chunk_ix = 0` (back-compat: short docs are one chunk).
  - `vectors_for_model(effective_id)` returns keys that `decode_chunk_key` parses, ordered by `(event_id ASC, chunk_ix ASC)`; the count equals the total chunk count across events.
  - The fresh schema has the column: `PRAGMA table_info(vectors)` includes a `chunk_ix` column (this drives Task A8's Phase-B trigger too, so pin it now).

```rust
    #[test]
    fn long_event_writes_one_vector_row_per_chunk_short_event_writes_one() {
        let log = /* open fresh in-memory log */;
        let emb = MockEmbedder::new(256);
        let long = "가".repeat(bossclaw_core::chunk::CHUNK_BUDGET_CHARS * 3);
        let long_id = /* append a memory event with content.text = long */;
        let short_id = /* append a memory event with a 20-char body */;
        log.rederive_pending(&emb).unwrap();
        let n_long = /* SELECT COUNT(*) FROM vectors WHERE event_id = long_id AND model_id = emb.model_id() */;
        assert!(n_long >= 3, "long doc chunked into ≥3 rows: {n_long}");
        let ixs: Vec<i64> = /* SELECT chunk_ix … ORDER BY chunk_ix */;
        assert_eq!(ixs, (0..n_long as i64).collect::<Vec<_>>(), "dense 0..n");
        let n_short = /* COUNT WHERE event_id = short_id */;
        assert_eq!(n_short, 1, "short doc is one chunk");
        // fresh schema carries chunk_ix
        let cols: Vec<String> = /* PRAGMA table_info(vectors) → name column */;
        assert!(cols.iter().any(|c| c == "chunk_ix"));
    }

    #[test]
    fn vectors_for_model_returns_chunk_keys_ordered_by_event_then_ix() {
        // … after rederive_pending on ≥2 multi-chunk events …
        let rows = log.vectors_for_model(emb.model_id()).unwrap();
        let keys: Vec<&str> = rows.iter().map(|(k, _)| k.as_str()).collect();
        for k in &keys { assert!(bossclaw_core::index::decode_chunk_key(k).is_some()); }
        let mut sorted = keys.clone();
        sorted.sort_by(|a, b| {
            let (ai, ax) = bossclaw_core::index::decode_chunk_key(a).unwrap();
            let (bi, bx) = bossclaw_core::index::decode_chunk_key(b).unwrap();
            ai.cmp(bi).then(ax.cmp(&bx))
        });
        assert_eq!(keys, sorted, "deterministic (event_id, chunk_ix) order");
    }
```

- [ ] **Step 2: Verify RED:** `cargo test -p bossclaw-core log::` — Expected: FAIL (writes are still one-row; no `chunk_ix` column).
- [ ] **Step 3: Commit** — `git add -u && git commit -m "test(bossclaw-core): fresh vectors schema + one-row-per-chunk writes + chunk-ordered read (RED)"`

### Task A8: schema (fresh) + chunked write + read (GREEN)

**why:** Implement the chunked write/read so the fresh-DB harness measures real chunking.

**Files:** Modify `crates/bossclaw-core/src/log.rs`.

- [ ] **Step 1: Schema (fresh path only, L474-481).** Change the CREATE TABLE to:

```rust
"CREATE TABLE IF NOT EXISTS vectors (
    event_id  TEXT NOT NULL,
    model_id  TEXT NOT NULL,
    chunk_ix  INTEGER NOT NULL DEFAULT 0,
    dim       INTEGER NOT NULL,
    embedding BLOB NOT NULL,
    PRIMARY KEY(event_id, model_id, chunk_ix)
)"
```

Update the table's doc comment (L469-472) to describe one row per `(event, model, chunk)`. NOTE: this is the fresh-DB shape only; an EXISTING DB with the old 2-col PK is upgraded in Phase B (Task B1) — this CREATE is `IF NOT EXISTS`, so it does NOT alter an existing table.

- [ ] **Step 2: Chunked write in `derive_vector` (L1101-1119) — ATOMIC per event (X1).** Replace the single-embed/single-insert with:
  1. `let chunks = chunk::chunk_text(&text); if chunks.is_empty() { return Ok(false); }`
  2. Embed ALL chunks FIRST (outside the store lock — never hold the lock across `embed_one`): `let embedded: Vec<(usize, Vec<f32>)> = chunks.iter().enumerate().map(|(ix, c)| Ok((ix, embed_one(embedder, c)?))).collect::<Result<_,_>>()?;`
  3. Then take the store lock ONCE and wrap "delete old rows for this `(event_id, model_id)` + insert all chunks" in ONE transaction (`let tx = conn.unchecked_transaction()?;` — precedent `keyword_add` at log.rs:1315): `tx.execute("DELETE FROM vectors WHERE event_id = ?1 AND model_id = ?2", params![event.id, embedder.model_id()])?;` then loop `tx.execute("INSERT INTO vectors (event_id, model_id, chunk_ix, dim, embedding) VALUES (?1,?2,?3,?4,?5)", …)` for each `(ix, vec)`; `tx.commit()?;`.
  - **Why atomic (X1):** without the DELETE+INSERT-all-in-one-tx, a crash after chunks 0,1 of a 4-chunk event leaves the event with SOME rows. `collect_pending`'s LEFT JOIN (L1635) is event-granular (`v.event_id IS NULL`), so it would treat the event as "done" and NEVER write chunks 2,3 — silent permanent loss. And the Phase B boot trigger counts rows, so count>0 makes it dead too. One transaction per event ⇒ an event is atomically ALL-chunks-or-ZERO-chunks, which makes event-granular resume correct again. `INSERT` (plain) is safe now because the DELETE clears any prior rows in the same tx; use `INSERT` not `INSERT OR REPLACE` so a duplicate `(…, chunk_ix)` within one event is a loud bug, not a silent overwrite.
  - Return `Ok(true)` (≥1 chunk written).
- [ ] **Step 3: Chunked write in `rederive_pending` (L1133-1174) — same per-event transaction.** Inside the per-event loop: chunk `embeddable_text`, embed all chunks (lock released across embed), then the SAME `unchecked_transaction` DELETE-then-INSERT-all block, committing ONCE per event. Count `derived` per EVENT (one increment per event whose chunks were committed) to keep the return-value contract meaningful. `collect_pending`'s LEFT JOIN (L1635) stays correct: with atomic writes an event has EITHER all its chunks OR none, so "any row exists ⇒ done" is now sound (this is the property X1's atomicity restores).
- [ ] **Step 4: `vectors_for_model` (L1183-1201).** Change SELECT to `SELECT event_id, chunk_ix, embedding FROM vectors WHERE model_id = ?1 ORDER BY event_id ASC, chunk_ix ASC`; map each row to `(encode_chunk_key(&event_id, chunk_ix as usize), blob_to_vec(&blob)?)`. The return type stays `Vec<(String, Vec<f32>)>` — callers (`rebuild_indexes` L1229-1234) now `index.add(&composite_key, &vec)`, and `HnswIndex::add`'s de-dup (L143) naturally de-dups per chunk. Update the doc comment (L1176-1182) to say the key is chunk-encoded and order is `(event_id, chunk_ix)`.
- [ ] **Step 4b (m4 — no separate work, just don't miss it):** `derive_vector_for` (log.rs:4314; callers `ingest.rs:706,712`) calls `derive_vector`, so the ingest single-event path inherits the chunked+atomic write transitively — do NOT add a second chunking implementation there and do NOT leave it un-chunked. A quick assert in an ingest test (a long ingested file yields >1 vector rows) confirms the transitive path.
- [ ] **Step 5: Run:** `cargo test -p bossclaw-core log:: chunk:: index::` then `cargo test -p bossclaw-core`. **A9/A10 (fold-back) MUST be implemented in the SAME work session immediately after A8 — the crate is NEVER committed red (m5: no `#[ignore]` escape hatch, no `test.skip`).** Rationale: once the write path is chunked, the raw vector arm returns composite ids; any recall test that asserts bare ids will fail until fold-back lands. So treat A8→A10 as one atomic landing: do A8 GREEN, then A9 RED, then A10 GREEN, and only the A10 commit needs the whole recall suite green. If A8's own new tests (schema/write/read) pass but a pre-existing recall test goes red on composite ids, that is the expected transient — carry straight into A9/A10 to resolve it; do not commit between A8 and A10 with a red recall test.
- [ ] **Step 6: Commit** — `git add -u && git commit -m "feat(bossclaw-core): atomic per-event chunked vector write path + chunk-keyed read (fresh schema) (GREEN)"`

### Task A9: adaptive over-fetch + fold-back inside `vector_search` (RED)

**why:** Fold-back to best-score-per-`event_id` MUST happen INSIDE `vector_search` before it returns, so `resolve_arms`/provenance/fusion/boosts/filters (built at L1451-1454 and after) all keep seeing bare event_ids — RRF double-voting becomes structurally impossible. Over-fetch must be **ADAPTIVE (X3):** a FIXED `k × 4` chunk-slot fetch STARVES distinct events on this corpus — the frozen snapshot has a 1.55M-char KO gold page (~1,000+ chunks) and 36k-char EN pages (~24 chunks), so a fat doc's chunks can fill all `k × 4` slots and collapse to `<k` distinct events, reintroducing the exact FUSION_FETCH ceiling rung 1 lifted. `vector_search` must grow the fetch until it has `k` distinct events OR the index is exhausted.

**Files:** Modify `crates/bossclaw-core/src/recall.rs` (const) and `crates/bossclaw-core/src/log.rs` (`vector_search` L1283) — RED tests first.

- [ ] **Step 1: RED tests** in `log.rs` tests:
  - **Fold-back (m7 — assert against the RETURNED set, not a precomputed const):** build a multi-chunk event plus several single-chunk events; call `vector_search(&qv, k)` and assert the multi-chunk event's bare id appears **exactly once**; all returned ids are bare (`decode_chunk_key(id).is_none()`); and its folded score equals the **minimum distance among THIS event's chunks that ACTUALLY appear in the raw neighbor set** — computed from a direct `index.search` call in the test, NOT a precomputed `best_chunk_distance` constant (HNSW deep-rank is OS-nondeterministic, so which chunks surface can vary run to run).
  - **Adaptive over-fetch (realistic fat-tail at SMALL k — the test that can actually fail if starvation isn't handled):** index several 25-chunk docs + one 60-chunk doc + enough single-chunk docs, and request a SMALL `k` (e.g. `k = 10`). Assert the returned DISTINCT-event count is exactly `min(k, total_distinct_events)` — i.e. the fat docs do NOT starve the result down below `k`. A fixed `k × 4 = 40`-slot fetch fails this (40 slots ≈ one-and-a-half fat docs → <10 distinct); the adaptive fetch passes it.

```rust
    #[test]
    fn vector_search_folds_chunks_to_one_best_scored_bare_event() {
        // … index a multi-chunk event (multi_id) + several single-chunk events …
        let k = 10;
        let hits = log.vector_search(&qv, k).unwrap();
        let ids: Vec<&str> = hits.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids.iter().filter(|id| **id == multi_id).count(), 1, "folded to one");
        for (id, _) in &hits {
            assert!(bossclaw_core::index::decode_chunk_key(id).is_none(), "bare id, no separator");
        }
        // m7: derive the expected best from the RAW neighbor set actually returned,
        // not a hardcoded distance — HNSW deep-rank is OS-nondeterministic.
        let raw = /* call the underlying index.search(&qv, k * some_large_mult) via a test seam
                     OR reconstruct from the same HnswIndex the test built */;
        let expected_best = raw.iter()
            .filter(|(key, _)| bossclaw_core::index::event_id_of(key) == multi_id)
            .map(|(_, d)| *d)
            .fold(f32::INFINITY, f32::min);
        let folded = hits.iter().find(|(id, _)| id == &multi_id).unwrap().1;
        assert!((folded - expected_best).abs() < 1e-6, "folded score = min chunk distance in the returned set");
    }

    #[test]
    fn adaptive_over_fetch_returns_k_distinct_events_despite_fat_docs_at_small_k() {
        // 3×25-chunk docs + 1×60-chunk doc + 20 single-chunk docs = 24 distinct events;
        // request small k so a FIXED k×4 fetch would starve.
        let k = 10;
        let hits = log.vector_search(&qv, k).unwrap();
        let distinct: std::collections::HashSet<&str> = hits.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(distinct.len(), k.min(24),
            "adaptive fetch yields k distinct events; fat docs must not starve it: got {}", distinct.len());
        assert!(hits.len() == distinct.len(), "no duplicate events after fold");
    }
```

- [ ] **Step 2: Verify RED:** `cargo test -p bossclaw-core log::` — Expected: FAIL (search returns composite ids, no fold-back; and even a naive fixed-multiplier fold would fail the small-k starvation assert).
- [ ] **Step 3: Commit** — `git add -u && git commit -m "test(bossclaw-core): vector_search folds chunks→best-per-event; adaptive over-fetch survives fat docs at small k (RED)"`

### Task A10: over-fetch const + fold-back (GREEN)

**why:** Implement the fold-back + over-fetch so recall sees bare, de-duplicated events and fusion is unaffected.

**Files:** Modify `crates/bossclaw-core/src/recall.rs` (const) and `crates/bossclaw-core/src/log.rs` (`vector_search`).

- [ ] **Step 1: `recall.rs` — add `CHUNK_OVERFETCH`** near `FUSION_FETCH` (L166):

```rust
/// INITIAL chunk over-fetch multiplier. `vector_search` first asks the ANN for
/// `k × CHUNK_OVERFETCH` CHUNK slots, folds them to best-score-per-`event_id`,
/// and — if fat documents collapsed the result below `k` distinct events — GROWS
/// the fetch and retries until it has `k` distinct events OR the index is
/// exhausted (spec §3.4.4). The growth is what keeps a 1,000+-chunk doc from
/// starving the distinct-event count on this corpus (a FIXED multiple does not).
/// 4 is the starting point (cheap on the common case where docs are 1–few
/// chunks; HNSW `ef = max(requested, 64)`). Re-tunable — a measurement subject.
pub const CHUNK_OVERFETCH: usize = 4;
```

- [ ] **Step 2: `log.rs` — rewrite `vector_search` (L1283-1295)** to ADAPTIVELY over-fetch, then fold:

```rust
pub fn vector_search(&self, query_vec: &[f32], k: usize) -> Result<Vec<(String, f32)>, BossclawError> {
    use crate::index::event_id_of;
    use crate::recall::CHUNK_OVERFETCH;
    let guard = self.vector_index.lock().expect(POISON);
    let index = guard.as_ref().ok_or_else(|| {
        BossclawError::InvalidInput("vector index not built — call rebuild_indexes".into())
    })?;
    if k == 0 {
        return Ok(Vec::new());
    }
    // Adaptive over-fetch (X3): grow the CHUNK-slot request until we have k
    // DISTINCT events OR the index is exhausted (search returns fewer slots than
    // asked — HnswIndex::search internally clamps to id_to_slot.len(), index.rs:167).
    // Doubling the multiplier keeps this O(log(index/k)) index calls, each a cheap
    // in-memory HNSW query. Re-searching from scratch (not incremental) keeps the
    // fold simple and correct; the common case (docs ~1 chunk) folds on the first
    // pass and never loops.
    let mut mult = CHUNK_OVERFETCH;
    let mut folded: Vec<(String, f32)> = Vec::new();
    loop {
        let want = k.saturating_mul(mult);
        let raw = index.search(query_vec, want);
        let exhausted = raw.len() < want; // fewer slots than asked ⇒ nothing more to get
        let mut best: std::collections::HashMap<String, f32> = std::collections::HashMap::new();
        for (key, dist) in raw {
            let id = event_id_of(&key).to_string();
            best.entry(id).and_modify(|d| { if dist < *d { *d = dist; } }).or_insert(dist);
        }
        folded = best.into_iter().collect();
        if folded.len() >= k || exhausted {
            break;
        }
        mult = mult.saturating_mul(2); // grow and retry — fat docs collapsed us below k
    }
    // Ascending distance = better; id tie-break for determinism. Truncate to k
    // DISTINCT events.
    folded.sort_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    folded.truncate(k);
    Ok(folded)
}
```

Note: `HnswIndex::search` already clamps the requested count to the index size (index.rs:167), so an over-large `want` is safe and `raw.len() < want` is a reliable "index exhausted" signal — no public `len()` accessor is needed on the trait.

- [ ] **Step 3: Downstream-untouched assert.** Add ONE recall-level assert (in an existing recall test or a new one) that a multi-chunk event still receives its recency/pin/graph boost and still passes the pages/files + superseded/revoked filters — proving fold-back left the bare-`event_id` downstream contract intact. (No `#[ignore]` to un-gate: per A8 Step 5, A8→A10 is one atomic landing, so nothing was committed red.)
- [ ] **Step 4: Run:** `cargo test -p bossclaw-core && cargo test -p bossclawd && cargo clippy --workspace --all-targets -- -D warnings` — Expected: green.
- [ ] **Step 5: Commit** — `git add -u && git commit -m "feat(bossclaw-core): fold chunks→best-per-event inside vector_search + adaptive over-fetch (GREEN)"`

### Task A11: FTS invariance guard (RED then GREEN)

**why:** Spec §3.4.6: the keyword arm stays WHOLE-DOC. A test must pin that chunking did NOT leak into the FTS write path — otherwise a future edit could accidentally chunk the keyword index and change BM25 semantics.

**Files:** Modify `crates/bossclaw-core/src/log.rs` (test only — the invariance holds by construction after A8, since `collect_embeddable_events_ordered` (L1668) and `keyword_add`/`rebuild_indexes` FTS half were untouched).

- [ ] **Step 1: RED/uncertain test.** For an event whose text is long enough to chunk into ≥3 vector rows, assert the FTS side has exactly ONE `fts_map` row for that event id (`SELECT COUNT(*) FROM fts_map WHERE event_id = ?1` == 1) after `rebuild_indexes`, and that a keyword query matching a term only present in the document's TAIL (beyond the first chunk's budget) still returns the event — proving the keyword arm indexes the whole doc, not chunk 0.

```rust
    #[test]
    fn fts_stays_whole_doc_when_vectors_are_chunked() {
        let head = "alpha ".repeat(500);                 // > budget on its own
        let needle = "zzunlikelyneedle";
        let text = format!("{head}\n\n{needle} in the tail");
        // … append memory event, rederive_pending, rebuild_indexes …
        let map_rows: i64 = /* COUNT(*) FROM fts_map WHERE event_id = id */;
        assert_eq!(map_rows, 1, "keyword arm is one whole-doc row, not one per chunk");
        let hits = log.keyword_search(needle, 10).unwrap();
        assert!(hits.iter().any(|(id, _)| id == &event_id), "tail term matches whole doc");
    }
```

- [ ] **Step 2: Run:** `cargo test -p bossclaw-core log::` — Expected: GREEN immediately if A8 left the FTS path whole-doc (the intended outcome). If it is RED, the fix is to ensure the FTS write path still consumes `embeddable_text` (whole doc), NOT `chunk_text` — revert any accidental chunking of `keyword_add`/`collect_embeddable_events_ordered`.
- [ ] **Step 3: Commit** — `git add -u && git commit -m "test(bossclaw-core): FTS keyword arm stays whole-doc under chunking (invariance guard)"`

### Task A-gate: Phase A gates + measured go/no-go (the ship decision for Phase B)

**why:** Phase A's whole point is proving the win on the frozen harness before building migration plumbing. This is the go/no-go.

- [ ] **Step 1: Workspace gates** (every line green):

```
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build -p bossclawd        # feature-leak gate (Phase 0 Task 46 set)
cargo check --workspace
```

- [ ] **Step 2: Establish/confirm the paired baseline.** The baseline is the potion-base-8M **rung-1** (pre-chunking) `scores.json` on the SAME frozen inputs. If PR #74's frozen rung-1 baseline scores.json is on disk (check the report dir), record its path + identities (corpus snapshot sha, case-list sha). Otherwise re-measure it: `git stash`/worktree the pre-chunking tree (or checkout `f6c4cbc`), run the command in Step 3 against it, save that `scores.json` as `<BASELINE>`; the effective id there is the OLD `minishlab/potion-base-8M` — **the compare tool pairs on `case_idx`, not model id, so a pre/post-chunking pair is valid** as long as `--corpus` and `--cases` match.
- [ ] **Step 2b (X7 — truncation PROBE, cheap, one call).** Before the runs, confirm 1,500-char chunks do NOT truncate on the real model: load the production `Model2Vec` from the repo model dir and tokenize one 1,500-char EN chunk and one 1,500-char KO chunk (call the model's tokenizer / `encode_single` on a probe and read the token count). Log `token_count` + `token/char` ratio for EN and KO. Expected: model2vec `StaticModel` mean-pools with no context window (`seq_length` effectively unbounded), so the token count reflects the WHOLE chunk (no cap at 512/8192). If any KO chunk's token count looks suspiciously capped (a round number like 512), STOP — that would mean truncation and the budget must drop; otherwise record "no truncation confirmed" and proceed. This is a one-off confirmation, not a gate.
- [ ] **Step 3: Candidate runs — ≥3× for the jitter band (X2).** The chunked candidate index is 4-8× bigger and full of intra-doc near-duplicate chunk vectors, and **HNSW seeds its level-assignment RNG from OS randomness per construction (index.rs:93-98) — `seed=42` does NOT reach it (there is no seed API)**. So the candidate is STRUCTURALLY noisier than the rung-1 baseline; a single run's paired p could credit graph-size jitter as a chunking win. Run the SAME frozen candidate command **at least 3 times** (each `--cases` run skips synth, ~25 min):

```
# run 3× (or more); each is an independent HNSW build → independent deep-rank jitter
cargo run -p memharness -- run --known-item-only \
  --corpus ~/.air-harness/phase1-corpus \
  --cases ~/.air-harness/phase1-cases.jsonl
```

For EACH run record: the report dir, the `Corpus snapshot: <sha> · frozen case list: <sha>` line, and the **index-rebuild wall-clock** from the `rebuilt vector index: N vectors in Xms` log line. Compute the **synthetic·en s@10 spread across the ≥3 runs = the run-to-run jitter band** (max − min, and ideally the std). This band is the noise floor the chunking delta must beat.
- [ ] **Step 4: Paired compare** (baseline vs EACH candidate run — pick a representative candidate for the headline table, but report all three deltas):

```
cargo run -p memharness -- compare \
  --baseline <BASELINE-rung1>/scores.json \
  --candidate <candidate-run-i>/scores.json
```

- [ ] **Step 5: Read the gate (spec §3.4 / §1 + X2 jitter guard).** SHIP Phase B iff ALL hold: (a) `synthetic·en·known-item` improves with paired Wilcoxon **p < 0.05**; (b) **the synthetic·en chunking delta (candidate − baseline s@10) EXCEEDS the run-to-run jitter band from Step 3** — if the delta is within the band, it is graph-size noise, NOT a chunking win: treat as FAIL; (c) `synthetic·ko·known-item` shows **no significant regression** (no p < 0.05 in the wrong direction). real·en/real·ko are directional color only. k=10 everywhere; note that seed=42 pins synth/case selection but NOT the HNSW RNG (hence the band). If the gate FAILS: do NOT proceed to Phase B; record the numbers, and either re-tune `CHUNK_BUDGET_CHARS`/`CHUNK_OVERLAP_CHARS` (bumping the suffix to `+chunks-v2`) and re-measure, or revert (spec §1.4). Capture every compare table verbatim.
- [ ] **Step 5b (m9 — rebuild budget is INFORMATIONAL-ONLY).** The spec §3.4.7 <30s rebuild target is REPORTED, not a gate: record the per-run rebuild wall-clock, and if it exceeds 30 s flag it for follow-up (a chunk-inflated rebuild-latency ticket) — but the ONLY thing that gates Phase B is the paired Wilcoxon + jitter-band check in Step 5. Do not block on rebuild time.
- [ ] **Step 6: Commit the measurement record** (paths + identities + all rebuild wall-clocks + the jitter band + every compare table; NEVER file contents) — `git commit --allow-empty -m "measure(rung3): Phase A frozen A/B gate — <PASS/FAIL>, en Δ<...> vs jitter band <...>, p<...>, ko Δ<...> p<...>, rebuild <X>s (informational)"`.

---

# Phase B — ship safely to EXISTING brains (built ONLY if the A-gate PASSED)

> A fresh DB never needs this; a real user's existing brain does. Phase B upgrades the on-disk `vectors` PK and re-chunks/re-embeds existing events under the effective id on boot, before recall can serve. All migration correctness is covered by dedicated integration tests — the harness NEVER exercises it (fresh daemon per run; spec §3.3.2/§4 finding #4).

### Task B1: schema table-rebuild migration — trigger + rebuild (RED)

**why:** There is **NO migration framework** in this code (`SCHEMA_VERSION=1` at log.rs:45 is "reserved"; `vectors` is `CREATE TABLE IF NOT EXISTS` at L474, so it never alters an existing table). An existing brain has the old 2-col PK; ALTER cannot change a PK, so we detect + table-rebuild. Idempotent + crash-safe.

**Files:** Modify `crates/bossclaw-core/src/log.rs` (add a `migrate_vectors_schema` fn + call it once at open, right after the CREATE TABLE block) — RED tests first.

- [ ] **Step 1: RED tests** in `log.rs`. Simulate an EXISTING brain by creating a `vectors` table with the OLD schema (2-col PK, no `chunk_ix`), inserting rows, then invoking the migration:

```rust
    #[test]
    fn migrates_old_two_col_vectors_pk_to_three_col_preserving_rows() {
        // build a DB with the OLD schema + 3 rows (chunk_ix implicitly 0)
        // … CREATE TABLE vectors ( event_id, model_id, dim, embedding, PRIMARY KEY(event_id, model_id) ) …
        // … INSERT 3 rows …
        migrate_vectors_schema(&conn, /* stored_schema_version = */ 0).unwrap();
        // new column exists, PK is 3-col, rows preserved with chunk_ix = 0
        let cols: Vec<String> = /* PRAGMA table_info(vectors) names */;
        assert!(cols.contains(&"chunk_ix".to_string()));
        let n: i64 = /* COUNT(*) */; assert_eq!(n, 3, "no rows lost");
        let ix0: i64 = /* SELECT chunk_ix FROM vectors LIMIT 1 */; assert_eq!(ix0, 0);
        // idempotent: a second call is a no-op (PRAGMA structural guard sees chunk_ix present)
        migrate_vectors_schema(&conn, 2).unwrap();
        let n2: i64 = /* COUNT(*) */; assert_eq!(n2, 3);
    }

    #[test]
    fn migration_is_a_noop_on_a_fresh_three_col_table() {
        // fresh open already has chunk_ix (Task A8) → migration detects & returns Ok, unchanged
        migrate_vectors_schema(&conn, 2).unwrap();
        let cols: Vec<String> = /* names */;
        assert_eq!(cols.iter().filter(|c| *c == "chunk_ix").count(), 1);
    }

    #[test]
    fn migration_wraps_rebuild_in_a_transaction_crash_safe() {
        // The rebuild (CREATE new, copy, DROP old, RENAME) runs inside ONE
        // transaction so a crash mid-rebuild leaves either the old or the new
        // table whole — never a half-copied table. Assert: after a simulated
        // failure injected before COMMIT, the ORIGINAL table + rows survive intact.
        // (Inject via a poisoned copy step or a manual ROLLBACK in the test.)
    }

    #[test]
    fn schema_version_is_bumped_to_2_and_stamped_on_fresh_config() {
        // X8: a freshly-written active-model config carries schema_version 2, not 1.
        // … open fresh log, set_active_model(...), read active_model() …
        assert_eq!(/* m.schema_version */ 2, 2, "irreversible schema change is recorded as v2");
    }
```

- [ ] **Step 2: Verify RED:** `cargo test -p bossclaw-core log::` — FAIL (`migrate_vectors_schema` not found).
- [ ] **Step 3: Commit** — `git add -u && git commit -m "test(bossclaw-core): vectors PK 2-col→3-col table-rebuild migration — preserve/idempotent/crash-safe (RED)"`

### Task B2: schema table-rebuild migration + SCHEMA_VERSION bump (GREEN)

**why:** Implement the detect-and-rebuild so an existing brain gains `chunk_ix` without data loss; and (X8) record the irreversible change by bumping `SCHEMA_VERSION` — leaving it at 1 makes the persisted `schema_version` config field a lie.

**Files:** Modify `crates/bossclaw-core/src/log.rs`.

- [ ] **Step 1: Bump `SCHEMA_VERSION` 1 → 2 (X8, log.rs:45).** New config events written by `reembed_migration` (L1842-1855) and `set_active_model` (L1918-1930) will now stamp `schema_version: 2` (they inherit the existing value if a config already exists, else use `SCHEMA_VERSION`). Update the existing test at log.rs:7061 (`assert_eq!(m.schema_version, SCHEMA_VERSION)`) — it references the const so it stays green automatically, but add a dedicated assertion that a freshly-stamped config carries version 2 (guards against a silent revert). Update the `SCHEMA_VERSION` doc comment (L40-45) from "reserved" to "v2 = chunked vectors table (event_id, model_id, chunk_ix) — bumped by Rung 3."
- [ ] **Step 2: Implement `migrate_vectors_schema(conn: &Connection, stored_schema_version: u32) -> Result<(), BossclawError>`:**
  - Detect (idempotency guard): `PRAGMA table_info(vectors)` → if a `chunk_ix` column is present, return `Ok(())` (already migrated / fresh). This is the STRUCTURAL guard.
  - Version gate (X8): the caller passes `stored_schema_version` (from `active_model()?.map(|m| m.schema_version).unwrap_or(SCHEMA_VERSION)`); only rebuild when `stored_schema_version < 2` AND the column is missing. The PRAGMA check stays the authoritative idempotency guard (a brain whose config is missing/old but whose table is somehow already 3-col must not be re-rebuilt); the version becomes the durable RECORD of the change. Both conditions agreeing = rebuild.
  - Rebuild (ALTER cannot change a PK) inside ONE transaction (`conn.unchecked_transaction()` — precedent log.rs:1315):
    1. `CREATE TABLE vectors_new ( event_id TEXT NOT NULL, model_id TEXT NOT NULL, chunk_ix INTEGER NOT NULL DEFAULT 0, dim INTEGER NOT NULL, embedding BLOB NOT NULL, PRIMARY KEY(event_id, model_id, chunk_ix) )`.
    2. `INSERT INTO vectors_new (event_id, model_id, chunk_ix, dim, embedding) SELECT event_id, model_id, 0, dim, embedding FROM vectors` (old rows become chunk 0).
    3. `DROP TABLE vectors; ALTER TABLE vectors_new RENAME TO vectors`; `tx.commit()`.
  - The transaction makes it crash-safe (a crash before COMMIT rolls back to the intact old table); the `chunk_ix`-present check makes it idempotent.
  - NOTE: the `schema_version` field is only bumped to 2 in the CONFIG EVENT (stamped by the boot migration's `reembed_migration` / `set_active_model` in Phase B Task B4, which run AFTER this schema rebuild). So the version gate reads the OLD (pre-migration) stored version here; the config stamp to 2 lands during the same boot, after the table rebuild + re-embed. Order within B4: schema rebuild (B2) → boot re-embed (B4, which writes the v2 config).
- [ ] **Step 3: Call it once at open**, immediately AFTER the `vectors` `CREATE TABLE IF NOT EXISTS` (L481) so a fresh DB (already 3-col) short-circuits and an existing DB is upgraded before any read. (Find the open/init fn that runs the CREATE batch; it must read the stored schema version — via a lightweight config read — and pass it in. If reading the config at that point is awkward, pass `0` as a conservative "unknown/old" and rely on the PRAGMA structural guard for idempotency; document the choice.)
- [ ] **Step 4: Run:** `cargo test -p bossclaw-core log:: && cargo test -p bossclaw-core` — Expected: green (fresh-DB tests from Phase A still pass because the migration no-ops on the 3-col table; the version-bump test asserts 2). Update the B1 RED test signatures to pass the `stored_schema_version` arg (e.g. `0` for the old-schema fixtures, `2` for the already-migrated fixture).
- [ ] **Step 5: Commit** — `git add -u && git commit -m "feat(bossclaw-core): detect-and-rebuild vectors schema to 3-col chunk PK + SCHEMA_VERSION 1→2, idempotent + crash-safe (GREEN)"`

### Task B3: boot auto-migration trigger + reconciliation (RED)

**why:** An existing brain's rows are all under the OLD id `minishlab/potion-base-8M` — zero rows exist for the effective `…+chunks-v1` id, so recall would read an EMPTY vector arm. On boot, before `ensure_indexed` can serve, if there are ZERO `vectors` rows for the compiled effective id while embeddable events exist → run `reembed_migration(embedder)` (re-chunk + re-embed under the effective id, GC old-id rows). Triggering on ZERO ROWS (not the config event) is immune to the record-only `set_active_model` stamp (spec finding #3). This is the auto-migration that Rung 2 was going to own — it's Rung 3's now.

**Files:** Add a core helper + a daemon integration test. Modify `crates/bossclaw-core/src/log.rs` (a public `count_vectors_for_model(model_id) -> Result<usize>` reader) and `crates/bossclawd/src/engine/mod.rs` (the boot hook) — RED tests first.

- [ ] **Step 1: RED — core reader test** in `log.rs`: `count_vectors_for_model(effective_id)` returns the number of `vectors` rows for that id (0 on a brain whose rows are all old-id). Assert 0 for a fresh unmigrated-data brain and N after `rederive_pending`.
- [ ] **Step 2: RED — daemon boot test** in `crates/bossclawd/src/engine/mod.rs` tests (using `MockEmbedderProvider`, dim 256, whose `model_id()` returns a test EFFECTIVE id, distinct from a test OLD id). The realistic "existing brain" fixture: a log with embeddable events whose `vectors` rows are all under the OLD id (insert them directly, or ingest under an old-id embedder), so ZERO rows exist for the effective id. Assert:
  - **Migration runs + GCs old rows:** after boot, `count_vectors_for_model(effective_id) > 0` (events re-chunked/re-embedded under the new id) AND `count_vectors_for_model(old_id) == 0` (old-id rows GC'd by `reembed_migration`'s `DELETE … WHERE model_id != ?1`, log.rs:1873).
  - **Ordering — boot-before-serve:** a `recall` immediately after engine construction returns folded bare-id hits (not empty), proving migration completed before recall served.
  - **X6 — the REAL ordering hazard (partial pre-boot state):** a variant where SOME events already have effective-id rows before boot (a prior partial migration wrote a few) while OTHER events still have ONLY old-id rows. After boot, assert NO event is left with only old-id rows — every embeddable event has an effective-id chunk set, and no old-id rows remain. (This is the finding-#3 reconciliation stated as a testable property: the row-based trigger + `rederive_pending`'s per-event backfill converge every event, regardless of any `set_active_model` config stamp — the config stamp is irrelevant because the trigger counts ROWS. Note in the test comment that we deliberately do NOT test "after a full `run_ingest` the trigger fires" — after a real ingest, effective-id rows exist so the trigger correctly does NOT fire; that is not a hazard, it's correct.)
  - **Fresh brain:** an engine whose log has NO embeddable events and `active_model()` == `Ok(None)` does NOT error and does NOT migrate — `recall` returns empty cleanly.

```rust
    #[tokio::test]
    async fn boot_migrates_old_id_vectors_to_effective_id_and_gcs_old_before_serving_recall() { /* … */ }
    #[tokio::test]
    async fn boot_converges_partial_migration_no_event_left_old_id_only() { /* X6: mixed pre-boot state … */ }
    #[tokio::test]
    async fn fresh_brain_no_events_boot_is_a_clean_noop() { /* … */ }
```

- [ ] **Step 3: Verify RED:** `cargo test -p bossclaw-core count_vectors && cargo test -p bossclawd engine::` — FAIL (reader + boot hook absent).
- [ ] **Step 4: Commit** — `git add -u && git commit -m "test: row-based boot auto-migration trigger (fires after record-only stamp; fresh-brain no-op; ordering) (RED)"`

### Task B4: boot auto-migration — implement + ordering (GREEN)

**why:** Wire the row-based trigger into daemon boot before `ensure_indexed`, reusing `reembed_migration` (which already re-embeds + GCs other-model rows via `embedder.model_id()`).

**Files:** Modify `crates/bossclaw-core/src/log.rs` (`count_vectors_for_model`) and `crates/bossclawd/src/engine/mod.rs` (boot hook + `run_ingest` reconciliation).

- [ ] **Step 1: `log.rs` — `count_vectors_for_model`:** `SELECT COUNT(*) FROM vectors WHERE model_id = ?1`, return `usize`. Pub. **Doc comment (m8): state it counts ROWS, not distinct events — under chunking one event has many rows, so a caller must NOT use this as an event count.** Also add a cheap `has_embeddable_events(&self) -> Result<bool>` (or `count_embeddable_events`) helper: `SELECT COUNT(*) FROM events WHERE event_type IN (...)` using the same `EMBEDDABLE_EVENT_TYPES` placeholder pattern as `collect_pending` (L1627-1652) but WITHOUT deserializing any payload (X5).
- [ ] **Step 2: `mod.rs` — boot hook, at the CORRECT site (X4).** The first-open `EventLog::open` runs inside a SYNC `spawn_blocking` (mod.rs:347-358) that captures only `keystore` + `db_path` by move — `self`, `.await`, and async are NOT in scope there, so the hook CANNOT live inside that closure (it would not compile). Instead, insert the migration in `get_or_open` **AFTER** the `spawn_blocking` returns the `Arc<EventLog>` at **L360** and **BEFORE** `*guard = Some(log.clone())` at **L364** — the only point where `self`, `.await`, and the opened `log` are all in scope (and still under the load-bearing first-open `guard`, so it runs exactly once per process). The step:
  1. Build the embedder: `let embedder = self.embedder_provider.embedder()?;` (cheap — cached cell).
  2. Cheap guard (X5): `let needs = log.has_embeddable_events()? && log.count_vectors_for_model(embedder.model_id())? == 0;` — NO full scan / JSON parse; two COUNT(*) queries.
  3. If `needs`: run `reembed_migration` in its OWN `spawn_blocking` (it is blocking SQLite + embedding work): `let (l2, e2) = (log.clone(), embedder.clone()); tokio::task::spawn_blocking(move || l2.reembed_migration(&*e2)).await.map_err(EngineError::Join)?.map_err(|e| EngineError::KeystoreDbMismatch(e.to_string()))?;` then `*self.indexed.lock().await = true;` (Step 4). Log progress (spec §3.3.2).
  4. If `!needs`: do nothing (fast path — the two COUNTs are the entire cost on a normal boot). Leave `indexed` false so `ensure_indexed` builds lazily as today.
  - The schema rebuild (B2) already ran inside `EventLog::open` (Step 3 of B2 calls it at open), so by the time we reach this hook the table is already 3-col; this hook only handles the DATA migration (old-id → effective-id rows).
- [ ] **Step 3: `run_ingest` reconciliation (mod.rs:456-463).** Leave the record-only `set_active_model` stamp in place — it is now harmless because the boot trigger is ROW-based (finding #3): the config stamp cannot suppress a migration that keys on `count_vectors_for_model == 0`. Add a one-line comment at L456 noting the authoritative migration is the boot row-based trigger and this stamp is only a fast-path record when vectors were just written under the effective id. (Do NOT add a second migration path here — one writer.)
- [ ] **Step 4: `ensure_indexed` interaction.** `reembed_migration` already calls `rebuild_indexes` (L1880), so after a boot migration the index is current; set `*self.indexed.lock().await = true` after a successful boot migration to skip a redundant first-recall rebuild (mirror `run_ingest` L470). If no migration was needed, leave `indexed` false so `ensure_indexed` builds it lazily.
- [ ] **Step 5: Recall-during-migration behavior (spec §3.3.3 / §4).** The migration runs in `get_or_open` before the first-open `guard` is released (L364), and EVERY caller (`recall`, `status`, `run_ingest`) goes through `get_or_open` and awaits the same `guard`. So no recall can proceed until migration completes — **recall WAITS for migration** (availability-only delay; never a partial/empty vector arm). Pin this with B6. Document the choice in the boot-hook doc comment.
- [ ] **Step 6: Run:** `cargo test -p bossclaw-core && cargo test -p bossclawd && cargo clippy --workspace --all-targets -- -D warnings` — green.
- [ ] **Step 7: Commit** — `git add -u && git commit -m "feat: row-based boot auto-migration before ensure_indexed; reconcile run_ingest record-only stamp (GREEN)"`

### Task B5: mid-EVENT crash resume (RED then GREEN)

**why (X1):** A boot migration on a large brain can be interrupted MID-EVENT. The dangerous case is a 4-chunk event that has chunks 0,1 written when the process dies. WITHOUT atomic per-event writes (A8 X1), that event has count>0 (so the ZERO-ROWS boot trigger is dead) AND `collect_pending`'s event-granular LEFT JOIN (log.rs:1635) sees "a row exists ⇒ done" → chunks 2,3 are lost PERMANENTLY. WITH A8's per-event transaction, the event is atomically ALL-4-chunks-or-ZERO, so a crash mid-event rolls that event back to zero rows and the next boot re-does it whole. This task PROVES that property end-to-end.

**Files:** Modify `crates/bossclawd/src/engine/mod.rs` (or `crates/bossclaw-core/src/log.rs`) tests.

- [ ] **Step 1: RED test — crash INSIDE a single event's chunk write.** The prior sketch crashed BETWEEN events, which event-granular resume already handles — a rigged test that would pass with the bug present. The correct test injects the crash MID-EVENT: build a brain with a 4-chunk event (+ maybe others); begin a chunked write for it but abort AFTER chunks 0,1 would have been staged and BEFORE the per-event transaction commits (e.g. inject a failing embedder on chunk 2, or roll back the tx manually to simulate the crash), then drop the log. Re-open → assert the interrupted event has **either 0 rows or all 4 rows, never 2** (the atomicity property), and that a normal boot/ingest then converges it to exactly 4 chunks. Assert final state: every embeddable event has a complete, dense `0..n` chunk set for the effective id; NO event has a partial set.

```rust
    #[tokio::test]
    async fn crash_mid_event_leaves_zero_or_all_chunks_never_partial_then_converges() {
        // 4-chunk event; crash injected on chunk 2 (before per-event tx commit).
        // Re-open: assert count for that event ∈ {0, 4}, never 2.
        // Then run the normal path (boot migration / rederive_pending) → assert exactly 4.
    }
```

- [ ] **Step 2: GREEN.** No NEW resume machinery is needed beyond A8's per-event transaction — the atomicity IS the mechanism. This task's job is to (a) confirm A8's DELETE+INSERT-all-in-one-tx actually holds under a mid-event failure, and (b) ensure the failure path in `rederive_pending`/`derive_vector` does NOT commit a partial event: on any per-chunk embed error, the whole event's transaction must roll back (do not commit a partial), and the event stays "pending" so the next `rederive_pending` re-does it. If A8 already rolls back on error (the `?` inside the tx scope drops the tx → rollback), this is a test-only task; if a code gap is found (e.g. committing before all chunks are inserted), fix it here. Do NOT add a separate "rederive sweep" — that was the confused Rev-1 design; with atomic writes, `collect_pending`'s event-granular backfill on the next boot/ingest is already correct and sufficient.
- [ ] **Step 3: Run + Commit** — `cargo test -p bossclawd && git add -u && git commit -m "feat/test: crash mid-event leaves zero-or-all chunks (per-event atomicity), then converges (GREEN)"`

### Task B6: recall-during-migration availability test (RED then GREEN)

**why:** Spec risk §6 requires the availability behavior during migration be pinned by a test. B4 chose "recall waits" — this task makes that explicit and regression-proof.

**Files:** Modify `crates/bossclawd/src/engine/mod.rs` tests.

- [ ] **Step 1: Test.** With a brain needing migration, issue `recall` immediately after engine construction (which triggers first-open → boot migration). Assert the recall RESULT is the full folded set (migration completed before recall served) — i.e. it never returns an empty vector arm mid-migration. If the design were "serve keyword-only during migration," the assertion would differ; since B4 chose "wait," assert completeness. Document the choice in the test name + comment.
- [ ] **Step 2: Run + Commit** — `git add -u && git commit -m "test(bossclawd): recall waits for boot migration (never a partial/empty vector arm)"`

### Task B7: forward-proofing dim-change migration test (RED then GREEN) — NOT triggered by this rung

**why:** Spec §3.3.2/§4 wants a DIM-change migration test. This rung has NO dim change (base stays potion-base-8M, dim 256). Include the test as forward-proofing for the deferred Rung 2, clearly labeled as not-triggered-by-this-rung, so Rung 2 inherits coverage.

**Files:** Modify `crates/bossclaw-core/src/log.rs` tests.

- [ ] **Step 1: Test (labeled).** Using two `MockEmbedder`s with DIFFERENT dims (e.g. 256 → 384) and different effective ids, assert that `reembed_migration` under the new-dim embedder GCs the old-dim rows (`DELETE FROM vectors WHERE model_id != ?1`, L1873) and writes new rows at the new dim, and `rebuild_indexes` builds a coherent index at the new dim (no mixed-dim reads — `vectors_for_model` filters by id). Name it `dim_change_migration_gcs_old_and_rebuilds_at_new_dim__forward_proof_for_rung2` with a doc comment: "Rung 3 does NOT change dim; this pins the Rung-2 multilingual swap's migration contract in advance."
- [ ] **Step 2: Run + Commit** — `git add -u && git commit -m "test(bossclaw-core): dim-change migration forward-proof for deferred Rung 2 (not triggered by Rung 3)"`

### Task B-gate: Phase B gates + PR

**why:** Migration correctness is gated by these integration tests (NOT the harness — fresh daemon never exercises it). Then ship the whole rung.

- [ ] **Step 1: Full gate set** (green):

```
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build -p bossclawd
cargo check --workspace
```

- [ ] **Step 2: Migration integration coverage checklist** (spec §3.3.2/§4 — all must be green tests): effective-id change + old-row GC (B3/B4), schema PK rebuild on an existing DB + SCHEMA_VERSION bump (B1/B2), mid-EVENT crash resume / per-event atomicity (B5), fresh brain `active_model()==Ok(None)` no-op (B3), partial-pre-boot-state convergence (no event left old-id-only) (B3, X6), recall-waits-for-migration availability (B6), dim-change forward-proof (B7).
- [ ] **Step 3: (optional, Peter-gated) closing FULL harness run** (spec §3.4 close): one FULL run (all segments, cloud judge, live `~/brain`) recorded as the new program baseline — this is a program-level baseline refresh, NOT the rung gate (the rung gate was the Phase A frozen compare). Defer if a live cloud key isn't in scope; note it as a follow-up.
- [ ] **Step 4: Commit + PR** — title `feat(bossclaw-core): rung 3 — chunking (effective id +chunks-v1, fold-back, boot auto-migration) [measured gate: <PASS>]`; body = the Phase A compare table verbatim + report paths + both baseline identities (corpus snapshot sha + case-list sha) + the rebuild wall-clock + the migration integration checklist. Do not merge — Peter-gated.

---

## Forward notes (how the deferred Rung 2 multilingual swap reuses this rung)

- **Rung 2 reuses Rung 3's migration verbatim.** When the multilingual embedder lands, Rung 2 changes ONLY the base id in `embed.rs` (`BASE_MODEL_DIR_ID` → the new model's HF slug, and `MODEL_ID` → e.g. `"minishlab/potion-multilingual-128M+chunks-v1"`), regenerates the model artifacts + the physical-path sites (spec §3.3 change list: `tauri.conf.json:29`, `apps/desktop/src-tauri/src/main.rs:80,90`, `bossclawd/src/main.rs:200,205`, `crates/memharness/src/daemon.rs:35`, `scripts/fetch-model.sh` DEST/BASE + 3 sha256 pins). The effective id changes → the SAME row-based boot trigger ("zero rows for the compiled effective id") fires → the SAME `reembed_migration` re-chunks + re-embeds + GCs the old-id rows → the SAME schema (already 3-col from Rung 3) needs no further change. **Zero new migration machinery** — exactly the reuse the spec anticipated, just built one rung earlier.
- **Rung 2 brings the real dim change** (potion-base-8M dim 256 → the multilingual model's dim). Task B7's dim-change test already pins that contract, so Rung 2 inherits it. The `vectors.dim` column and `vectors_for_model`'s id-filtered read already prevent mixed-dim bleed.
- **Rung 2's measured result is already known** (probe/measurement done separately): the multilingual swap yields **+0.213 synthetic·ko, −0.049 synthetic·en (paired, on top of rung 1)**. So Rung 2's gate (synthetic·ko improves, synthetic·en no significant regression) is expected to pass on ko and needs the en delta checked against the paired-significance bar. Sequencing Rung 3 first means Rung 3's en gain (this rung) and Rung 2's small en dip are measured independently against their own baselines — cleaner attribution.

---

## Self-review

**Spec coverage:** §3.4 unit/consts (A1/A2) · §3.4.1 single effective id, write==read==trigger (A3/A4, B3) · §3.4.2 composite key vs `&str` trait (A5/A6) · §3.4.3 fold-back INSIDE `vector_search`, downstream untouched (A9/A10) · §3.4.4 adaptive over-fetch (A9/A10, X3) · §3.4.5 schema PK `(event_id, model_id, chunk_ix)` — fresh (A7/A8) + existing-DB rebuild + SCHEMA_VERSION bump (B1/B2, X8) · §3.4.6 FTS whole-doc invariance (A11) · §3.4.7 rebuild budget reported, informational-only (A-gate, m9) · §3.3.2 boot auto-migration reconciled with `mod.rs:456-463`, hooked at the correct site L360-364 (B3/B4, X4) · §3.3.3 recall-waits-for-migration pinned (B4/B6) · §4 migration integration tests: id change (B3), dim change (forward-proof B7), mid-EVENT crash resume (B5, X1), fresh brain (B3), partial-pre-boot convergence (B3, X6) · §3.0 frozen paired gate + jitter-band guard (A-gate, X2). Rung 2 physical-path sites: deliberately OUT (deferred; Forward notes).

**Sequencing safety:** Phase A leaves the crate green at every commit boundary. A8→A9→A10 is ONE atomic landing (write path + fold-back land together); no `#[ignore]`, no `.skip`, no red commit (Rev 2 m5 dropped the escape hatch). Phase B is built only after the A-gate PASSES.

**Anchor freshness:** every log.rs/index.rs/recall.rs/embed.rs/mod.rs line number was re-read on 2026-07-06/07 and corrected against drift (the spec's stated `derive_vector`/`fold-back`/`FUSION_FETCH` lines were stale by 10-20 lines; Rev 2 re-verified the `get_or_open` boot-hook site L347-364, `prime_switches` call at L353, `run_ingest` reconcile L456-463, `unchecked_transaction` precedent L1315, `SCHEMA_VERSION` L45). The implementer MUST still re-read before editing — line numbers drift with every commit in this plan.

**Placeholders:** the `chunk_text` body (A2) and the test bodies with `/* … */` are explicit implementation sketches for the executor, not gaps — each carries its exact contract, consts, and assertions. No `test.skip`/`.only`/TODO-hack ships.

**Design decisions — resolved in Rev 2 (previously open questions, now closed by the dual review):**
1. **Chunk consts** `CHUNK_BUDGET_CHARS = 1_500`, `CHUNK_OVERLAP_CHARS = 200`: char-based (KO-safe). X7 CONFIRMED the budget is a GRANULARITY knob, not a truncation risk (model2vec mean-pools, no context window) — validated by the A-gate token-count probe. Not a token budget by design; the gate measures whether the granularity helps.
2. **Over-fetch** is now ADAPTIVE (X3), not a fixed `k×4` — the frozen corpus's 1,000+-chunk KO page proved a fixed multiple starves distinct events. `CHUNK_OVERFETCH=4` is only the initial multiplier; `vector_search` grows it until `k` distinct events or index exhaustion.
3. **Schema migration** (X8): transactional CREATE-new/copy/DROP/RENAME, gated on `PRAGMA table_info` (structural idempotency) AND `SCHEMA_VERSION < 2` (durable record; bumped 1→2 so the persisted `schema_version` field is honest).
4. **Boot trigger is ROW-based** (`count_vectors_for_model(effective_id) == 0`), immune to any config stamp; the REAL ordering hazard (partial pre-boot state) is pinned by B3's convergence test (X6).
5. **Recall-during-migration = WAIT:** all callers await the same first-open `guard` (mod.rs:364), so recall never serves a partial/empty vector arm. Pinned by B6.
6. **Crash resume** (X1): the mechanism is A8's PER-EVENT transaction (all-chunks-or-zero), NOT a separate sweep. B5 proves a mid-EVENT crash leaves {0, all} rows, never partial, then converges via `collect_pending`'s event-granular backfill.

**Measurement-integrity guard (X2):** the A-gate runs the candidate ≥3× and requires the chunking delta to EXCEED the HNSW run-to-run jitter band (seed=42 does not reach the HNSW RNG; the bigger chunked index is structurally noisier). A within-band "win" is treated as noise → FAIL.
