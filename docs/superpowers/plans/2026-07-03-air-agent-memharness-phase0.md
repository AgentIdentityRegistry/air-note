# AIR Agent memharness Phase 0 (blind A/B measuring stick) Implementation Plan — Rev 2 — revised after architect+critic review

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A one-command local dev tool (`cargo run -p memharness -- run`) that produces a per-run markdown report answering, in numbers: on Peter's own `~/brain` corpus and his real mined queries, end-to-end, does AIR's engine beat GBrain's daily `balanced` pipeline — by how much, on which segments (EN/KO, known-item/open, real/synthetic), and can we trust the local judge? Every future memory-retrieval investment is A/B'd against this baseline instead of trusted on reputation.

**Architecture:** A new dev-only workspace lib+bin crate `crates/memharness`. It spins a **real in-process `bossclawd` daemon** (via the `test-helpers` feature's engine constructor + production `run_accept_loop`) on a private Unix socket under a per-run temp home, and drives it over the **real wire protocol** (`Hello` → `AddGrant` → `RunIngest` → `ListFiles` → `Recall`). **The AIR arm runs the PRODUCTION embedder** (`ResourceModel2Vec`, potion-base-8M) via a new tiny feature-gated helper `test_engine_with_embedder`; mocks appear only in hermetic plumbing tests. Recall hits map to page ids through the `ListFiles` `event_id → canonical_path` bridge (fail-loud, no fallback). The GBrain arm shells out to `gbrain query`. Both arms feed an identical local-Ollama answerer (context budget = k hits, both arms); open queries are scored by a blind, position-swapped local judge audited against the Anthropic API (floor `max(30, 15%)` ∪ uncertains). All external seams (AIR/GBrain retrieval, answerer, judge, auditor) are traits, so the run loop itself is hermetically tested with doubles. Reports are written OUTSIDE the repo (`~/.air-harness/reports/`) behind a symlink-resistant guard.

**Tech Stack:** Rust 2021, `bossclawd { path, features = ["test-helpers"] }` + `bossclawd-proto`, `tokio` (dedicated per-daemon current-thread runtime, mirroring the desktop `TestDaemon`), `ureq` v2 (loopback HTTP to Ollama + the Anthropic audit call — mirrors bossclaw-core's pinned `ureq = "2"`), `serde`/`serde_json`, `sha2` (corpus manifest), `rand` 0.8 + `rand_chacha` 0.3 (seeded RNG for all sampling/bootstrap/blinding), `clap` 4 (CLI), `anyhow`. All statistics (bootstrap CIs, Wilcoxon signed-rank, Cohen's kappa), language detection (Hangul heuristic), and YAML frontmatter stripping are **plain Rust with unit tests** — no heavyweight numeric/NLP deps. Every dep version mirrors the existing `Cargo.lock` exactly; no new duplicate major enters the tree.

**Spec:** docs/superpowers/specs/2026-07-03-air-agent-memharness-phase0-design.md (Rev 2)

---

## Rev 2 change summary (architect SOUND-WITH-CHANGES + critic REWORK punch list)

| # | Finding | Where fixed in this plan |
|---|---|---|
| 1 (CRIT) | AIR arm must measure the REAL embedder, not `MockEmbedder::new(8)` | Tasks 13–14 (`test_engine_with_embedder` in bossclawd, TDD, feature-gated), Tasks 15–16 (`HarnessDaemon::spawn_real` + model-dir resolution `BOSSCLAWD_MODEL_DIR` → repo fallback + preflight); hermetic tests keep the mock with the explicit "plumbing only" comment; spec §1 Rev 2 |
| 2 (CRIT) | `event_id → page_id` bridge undefined; silent fallback would fabricate 0.0 | Tasks 17–18 (`WireClient::list_files`), Tasks 19–20 (`resolve.rs::PageResolver`, invariant in code comments, FAIL-LOUD, fallback REMOVED), `dedup_by_page` before rank scoring (Task 35–36), e2e #1 un-rigged to assert gold through the REAL resolver (Task 44); spec §5 Rev 2 |
| 3 (CRIT) | test-helpers dependency form unresolved | Task 2 Cargo.toml documents the runtime-dep rationale (divergence from desktop dev-dep precedent); Task 46 gates add the scoped `cargo build -p bossclawd` (no features) proof; spec §1 Rev 2 |
| 4 (MAJ) | `run()` assembly was prose; seams not injectable | Tasks 41–42 (`run.rs::run_queries` — full code: per-query loop, segment bucketing, win rates, audit sampling `max(30,15%)` ∪ uncertains, expand-to-100%, egress counting), traits `AirRetriever`/`GbrainRetriever`/`PairJudge` (judge AND auditor share one trait), Task 45 second hermetic e2e driving `run_queries` with ALL seams doubled |
| 5 (MAJ) | Wilcoxon untested on tie-heavy (binary) data | Task 27 adds two tie-heavy fixtures with independently computed reference values (W=0/p≈0.036888; W=13.5/p≈0.529651) + `two_sided_normal_p(1.959964)≈0.05` sanity; report caveat on known-item segments (Task 40); spec §8 Rev 2 |
| 6 (MAJ) | No Anthropic fail-fast | Task 34 `anthropic::preflight` (one-token call, pinned model) wired before the loop in Task 43; Probe D added to memharness-probes.md |
| 7 (MAJ) | Report guard bypassable (non-existent path / symlink) | Tasks 39–40: `canonicalize_nearest_existing` + workspace-root comparison; tests for workspace-root path outside crates/memharness AND a symlinked dir |
| 8 (MAJ) | Probe A gaps (balanced flag; frontmatter indexing) | Probe A extended in memharness-probes.md; `corpus::STRIP_FRONTMATTER` probe-pinned flag; `prepare_corpus(.., strip: bool)` (Tasks 9–10); spec §2/§4 Rev 2 |
| 9 (MAJ) | Char-budget context pack = chunk-vs-page confound | Tasks 35–36: `pack_context` budgets by NUMBER OF HITS (k), per-snippet safety cap identical both arms, `PackStats` per arm in the report + granularity note; spec §4 Rev 2 |
| 10 (MAJ) | Corpus drift was a footnote | Task 40: drift >5% renders an INVALID-RUN banner at the top of the report (`DRIFT_INVALID_FRACTION`); spec §2/§8 Rev 2 |
| 11 (MIN) | `truncate_for_example` byte-slice panics on Korean | Task 40: char-boundary-safe truncation (`chars().take`) |
| 12 (MIN) | `parse_winner` fragile | Tasks 31–34: judge prompt constrained to EXACTLY one token (A/B/TIE); `parse_pick_token` exact-match-first, tokenized-substring fallback only if exactly ONE signal; ambiguous → `Uncertain` (never dropped) |
| 13 (MIN) | bin→lib refactor window | Task 2 creates lib+bin from the start; no rename window; single `use memharness::…` in main.rs |
| 14 (MIN) | retrieval-k vs scoring-k | One `k` knob; stated in `RunConfig` docs (Task 41), report headline (Task 40), spec §4 Rev 2 |
| 15 (MIN) | O(events×get_pages) mining | Kept, documented as a code comment (accepted at Phase-0 scale) in Task 22 |

---

## Preconditions & environment

- **Branch:** `feat-memharness-phase0` (already checked out). Verify at start: `git status -sb`.
- **Never ships:** `memharness` is dev tooling. It depends on `bossclawd`'s `test-helpers` feature as a NORMAL dependency because the helpers are needed at RUNTIME (the live run spins the daemon) — a documented divergence from the desktop's dev-dep precedent (test-time only). Shipped-binary safety is compile-time: helper items are `#[cfg(any(test, feature = "test-helpers"))]` and the shipped daemon builds SCOPED (`cargo build -p bossclawd`, as `scripts/dev-build-signed.sh` does), where memharness's feature request does not participate in feature resolution. Task 46 gates assert this.
- **Reports are private:** `~/.air-harness/reports/` is OUTSIDE the repo. `report.rs` refuses to write anywhere under the WORKSPACE root, through symlinks and not-yet-created paths (Tasks 39–40 enforce with tests).
- **Live run is Peter-gated:** the final live baseline run (real `~/brain` + real `gbrain` + real Ollama + real Anthropic key) is the acceptance demo, NOT an automatable step. The plan ends "harness ready + smoke-tested hermetically; live baseline run is the acceptance demo."

## Dependency versions (mirror EXACTLY from Cargo.lock — verified 2026-07-03)

| crate | version to declare | already in lock? | notes |
|---|---|---|---|
| `bossclawd` | `{ path = "../bossclawd", features = ["test-helpers"] }` | yes (workspace member) | `test_engine_with_embedder` (new, Task 14), `run_accept_loop`, `vault::seed_secret_cache_for_test`; ALSO the production `engine::embed::{EmbedderProvider, ResourceModel2Vec}` (not feature-gated) |
| `bossclawd-proto` | `{ path = "../bossclawd-proto" }` | yes | `Request`/`Response`/`Hello`/`HitWire`/frame fns; `types::{IngestReportMirror, FileRecordMirror}` |
| `tokio` | `{ version = "1", features = ["rt", "net", "io-util", "time", "macros"] }` | `1.52.3` | per-daemon `current_thread` runtime + `UnixStream` |
| `ureq` | `{ version = "2", default-features = false, features = ["json", "tls"] }` | `2.12.1` | mirrors bossclaw-core's exact ureq block (v2, NOT the tree's other v3.3.0) |
| `serde` | `{ version = "1", features = ["derive"] }` | `1.0.228` | |
| `serde_json` | `"1"` | `1.0.150` | |
| `sha2` | `"0.10"` | `0.10.9` | corpus manifest hashes |
| `rand` | `"0.8"` | `0.8.6` | 0.8 API (`gen_range`/`gen_bool`/`shuffle`); NOT the tree's 0.9.4 |
| `rand_chacha` | `"0.3"` | `0.3.1` | `ChaCha8Rng` deterministic seeding |
| `clap` | `{ version = "4", features = ["derive"] }` | `4.6.1` | CLI |
| `anyhow` | `"1"` | `1.0.103` | tool error plumbing |
| `tempfile` | `"3"` (dev-dependency) | `3.27.0` | temp homes + test fixtures |

All are already resolved in `Cargo.lock` → **zero** new crate versions. **Rev 2: `bossclaw-core` is DROPPED from memharness's deps** (Rev 1 held it "for type re-exports if needed"; the resolver works entirely on proto mirrors — leaner tree). `bossclaw-core` remains a dev-dep of `bossclawd` (already is) for the Task 13 test's custom provider.

---

## File Structure

```
crates/bossclawd/src/server.rs        # MODIFIED (Tasks 13–14): + test_engine_with_embedder (feature-gated)
crates/bossclawd/tests/roundtrip.rs   # MODIFIED (Task 13): + custom-embedder roundtrip test

crates/memharness/
├── Cargo.toml                    # lib+bin, test-helpers RUNTIME-dep rationale, exact versions, "never ships"
├── src/
│   ├── lib.rs                    # pub mod surface (created Task 2; each module task adds its line)
│   ├── main.rs                   # CLI (`run` + flags) + live-seam construction + full run() assembly
│   ├── daemon.rs                 # HarnessDaemon: spawn_with_provider / spawn_mock_for_plumbing_tests / spawn_real (real-embedder model-dir resolution + preflight)
│   ├── client.rs                 # WireClient: hello/add_grant/run_ingest/list_files/recall — single in-flight, timeout, drop-on-error
│   ├── corpus.rs                 # copy ~/brain → harness home (strip flag probe-pinned), skip dot-entries, sha256 manifest, page-id normalization
│   ├── frontmatter.rs            # pure: strip leading `---` YAML block; Hangul language heuristic
│   ├── resolve.rs                # PageResolver: event_id → page_id bridge (ListFiles records; FAIL-LOUD invariant)
│   ├── mine.rs                   # transcript JSONL → real queries + implicit within-5 labels; exact dedup; mine_all merge
│   ├── synth.rs                  # seeded stratified page sampling + Ollama known-item query generator (trait seam)
│   ├── arms.rs                   # RetrievedHit, dedup_by_page, pack_context (k-hit budget + PackStats), gbrain parser/CLI arm, LiveAirArm, AirRetriever/GbrainRetriever/Answerer traits
│   ├── judge.rs                  # Verdict/PosPick, parse_pick_token, blind assignment, position-swap, PairJudge (judge+auditor), OllamaJudge, select_audit_indices, kappa/agreement/trust
│   ├── stats.rs                  # success@k, MRR, bootstrap CI, Wilcoxon signed-rank (tie/continuity-corrected + small-n flag)
│   ├── ollama.rs                 # loopback ureq client: /api/tags preflight + /api/generate
│   ├── anthropic.rs              # Messages API: extract_text, audit pick via parse_pick_token, one-token preflight, AnthropicAuditor
│   ├── run.rs                    # run_queries: per-query loop, bucketing, audit ladder, egress counting, RunOutcome
│   └── report.rs                 # ReportModel + hardened outside-repo guard + markdown render (drift banner, pack stats, caveats) + raw JSON
└── tests/
    ├── fixtures/
    │   ├── transcript_synthetic.jsonl   # SYNTHETIC hand-authored JSONL (never real transcripts)
    │   ├── gbrain_query_sample.txt      # captured gbrain output shape (Probe A)
    │   └── mini_corpus/                 # 3 tiny synthetic .md pages (2 EN, 1 KO) with frontmatter
    │       ├── en/alpha.md
    │       ├── en/beta.md
    │       └── ko/gamma.md
    ├── hermetic_e2e.rs           # e2e #1: corpus→daemon ingest→ListFiles bridge→recall→REAL-resolver known-item scoring (mock embedder = plumbing only)
    └── hermetic_run_e2e.rs       # e2e #2: run_queries with ALL seams doubled (audit ladder, expansion, local-only, egress counts)
```

**Seam facts pinned from the real code (verified, do not re-derive):**

- `bossclawd::server::test_engine(home: PathBuf) -> EngineHandle` and `run_accept_loop(engine: Arc<EngineHandle>, listener: UnixListener)` are `pub` behind `feature = "test-helpers"`. `test_engine` hardwires `TestEmbedderProvider` → `MockEmbedder::new(8)` — hence Task 14's new constructor.
- `bossclawd::engine::embed` is `pub`: `pub trait EmbedderProvider { fn embedder(&self) -> Result<Arc<dyn bossclaw_core::Embedder>, EngineOpError> }`, `pub struct ResourceModel2Vec { pub fn new(model_dir: PathBuf) -> Self }` (production, NOT feature-gated), `pub const MODEL_ID: &str = "minishlab/potion-base-8M"`.
- Daemon model-dir resolution (main.rs): `BOSSCLAWD_MODEL_DIR` env override → `<data_dir>/models/potion-base-8M`. The repo's copy lives at `apps/desktop/src-tauri/resources/models/potion-base-8M/` (verified present: `model.safetensors`, `config.json`, `tokenizer.json`) — the harness's fallback.
- `bossclawd::vault::seed_secret_cache_for_test(HashMap<String,String>)` MUST be called (empty) before spinning the daemon (keychain-ACL hang hazard).
- `Request::ListFiles { onboarded } → Response::ListFiles(Vec<FileRecordMirror>)`; `FileRecordMirror { canonical_path: String, file_event_id: String, content_hash: String, grant_root: String }`. `file_event_id` is the ingest event's id (`graph.rs:607`) — the bridge invariant's anchor.
- `Request::Recall { onboarded, query, k } → Response::Recall(Vec<HitWire>)`; `HitWire { hit: HitMirror { event_id, score, sources, kind }, text }`.
- Frame fns are NOT cancellation-safe: a timed-out stream MUST be dropped, never reused.
- Daemon lifecycle pattern: desktop `TestDaemon` (own OS thread + `current_thread` runtime + `Notify` shutdown + `sync_channel` bind-handshake).

---

## Task 1 — PROBES (read-only reality check; NO code)

Runs four read-only probes on Peter's machine, recorded in `docs/superpowers/plans/memharness-probes.md` (Rev 2 skeleton committed). Pinned values become constants; if reality differs, update the constant AND the reconciliation note AND flag it in this task's commit.

- [ ] Probe A — `gbrain` CLI: `gbrain --version`, `gbrain query --help`, `gbrain query "test" --limit 3`. Record the machine-parseable invocation, per-hit fields, the slug↔path convention, **(Rev 2) whether `balanced` needs an explicit mode flag or is the default** (record the exact argv the arm must use), and **(Rev 2) whether GBrain indexes YAML frontmatter** (query a frontmatter-only term; a hit = indexes) → sets `corpus::STRIP_FRONTMATTER`. Paste raw output into `crates/memharness/tests/fixtures/gbrain_query_sample.txt` (snippet bodies MAY be `<redacted>` — public repo).
- [ ] Probe B — Ollama: `curl -s http://127.0.0.1:11434/api/tags`. Pin `DEFAULT_OLLAMA_MODEL` (evolve loop uses `qwen2.5:7b-instruct`-tier; match what's installed) + confirm `/api/generate`.
- [ ] Probe C — real query count: `grep -roh 'mcp__gbrain__\(query\|search\|recall\)' ~/.claude/projects/**/*.jsonl 2>/dev/null | wc -l` (+ file count variant). Decide `WEIGHT_SYNTHETIC_HIGHER` (deduped real open < 50 → true, spec §90).
- [ ] **(Rev 2) Probe D — Anthropic preflight:** the one-token Messages `curl` from the probes file (model `claude-sonnet-5`). Pin/adjust `AUDIT_MODEL`; confirm the response shape (`content[0].type == "text"`). Skip + note only if this machine will never run hybrid.
- [ ] Fill every `____` in `docs/superpowers/plans/memharness-probes.md`.
- [ ] Commit:
  ```
  mkdir -p crates/memharness/tests/fixtures
  git add docs/superpowers/plans/memharness-probes.md crates/memharness/tests/fixtures/gbrain_query_sample.txt
  git commit -m "$(cat <<'EOF'
docs(memharness): Task 1 probe findings — gbrain CLI+frontmatter, Ollama, query count, Anthropic preflight

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

---

## Task 2 — Crate skeleton: lib+bin from the start + workspace member

**(Rev 2, finding 13: lib+bin from day one — no mid-plan refactor window. Finding 3: dependency rationale documented.)**

- [ ] Write `crates/memharness/Cargo.toml`:
  ```toml
  [package]
  name = "memharness"
  version = "0.0.1"
  edition = "2021"
  license = "Apache-2.0"
  # DEV-ONLY TOOL — never ships. Do NOT add memharness to any release/bundle manifest.
  #
  # Why `test-helpers` rides a NORMAL dependency (not the desktop's dev-dep precedent):
  # memharness needs the helpers at RUNTIME — the live run spins the in-process daemon via
  # `test_engine_with_embedder` + `run_accept_loop`. The desktop only needs them at test time.
  # Shipped-binary safety is compile-time: every helper item is
  # `#[cfg(any(test, feature = "test-helpers"))]`, and the shipped daemon is built SCOPED
  # (`cargo build -p bossclawd`, per scripts/dev-build-signed.sh) where THIS crate's feature
  # request does not participate in feature resolution — helpers compile OUT. Task 46 gates it.
  description = "memharness: dev-only blind A/B measuring stick (AIR engine vs GBrain) — never ships."
  publish = false

  [lib]
  name = "memharness"
  path = "src/lib.rs"

  [[bin]]
  name = "memharness"
  path = "src/main.rs"

  [dependencies]
  # Versions mirror Cargo.lock EXACTLY (verified 2026-07-03) — zero new crate versions.
  bossclawd = { path = "../bossclawd", features = ["test-helpers"] }
  bossclawd-proto = { path = "../bossclawd-proto" }
  tokio = { version = "1", features = ["rt", "net", "io-util", "time", "macros"] }
  ureq = { version = "2", default-features = false, features = ["json", "tls"] }
  serde = { version = "1", features = ["derive"] }
  serde_json = "1"
  sha2 = "0.10"
  rand = "0.8"
  rand_chacha = "0.3"
  clap = { version = "4", features = ["derive"] }
  anyhow = "1"

  [dev-dependencies]
  tempfile = "3"
  ```
- [ ] Write `crates/memharness/src/lib.rs` (modules land one per task; the list grows — each module task's GREEN step adds its `pub mod` line):
  ```rust
  //! memharness library surface — shared by the binary (`main.rs`) and the hermetic integration
  //! tests. DEV-ONLY (see Cargo.toml header); never ships.
  #![forbid(unsafe_code)]
  // Module lines are added by their tasks, keeping every intermediate commit compiling:
  // pub mod frontmatter;  (Task 4)     pub mod corpus;   (Task 8)
  // pub mod ollama;       (Task 12)    pub mod daemon;   (Task 16)
  // pub mod client;       (Task 18)    pub mod resolve;  (Task 20)
  // pub mod mine;         (Task 22)    pub mod stats;    (Task 24)
  // pub mod judge;        (Task 30)    pub mod anthropic;(Task 34)
  // pub mod arms;         (Task 36)    pub mod synth;    (Task 38)
  // pub mod report;       (Task 40)    pub mod run;      (Task 42)
  ```
- [ ] Write `crates/memharness/src/main.rs`:
  ```rust
  //! memharness — DEV-ONLY blind A/B measuring stick: AIR engine vs GBrain, on Peter's own
  //! corpus + queries, end-to-end. NEVER SHIPS (see Cargo.toml). Spec (Rev 2):
  //! docs/superpowers/specs/2026-07-03-air-agent-memharness-phase0-design.md
  #![forbid(unsafe_code)]

  fn main() {
      println!("memharness: not yet implemented");
  }
  ```
- [ ] Add to root `Cargo.toml` members:
  ```toml
  members = ["crates/air-rs", "crates/bossclaw-core", "crates/bossclawd", "crates/bossclawd-proto", "crates/memharness", "apps/desktop/src-tauri"]
  ```
- [ ] Run: `cargo check -p memharness` → compiles.
- [ ] Commit:
  ```
  git add crates/memharness/Cargo.toml crates/memharness/src/lib.rs crates/memharness/src/main.rs Cargo.toml
  git commit -m "$(cat <<'EOF'
feat(memharness): dev-only lib+bin skeleton + workspace member (runtime test-helpers rationale documented)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

---

## Task 3 — `frontmatter.rs`: strip YAML frontmatter (RED)

- [ ] Create `crates/memharness/src/frontmatter.rs` with ONLY the failing tests:
  ```rust
  //! Pure text helpers: strip a leading YAML frontmatter block; detect Korean content.
  //! Whether stripping is APPLIED is probe-pinned (`corpus::STRIP_FRONTMATTER`, spec §2 Rev 2) —
  //! this module only provides the mechanism.

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn strips_leading_frontmatter_block() {
          let input = "---\ntitle: Hello\ntags: [a, b]\n---\n# Body\ntext here\n";
          assert_eq!(strip_frontmatter(input), "# Body\ntext here\n");
      }

      #[test]
      fn no_frontmatter_is_returned_unchanged() {
          let input = "# Body\nno frontmatter\n";
          assert_eq!(strip_frontmatter(input), input);
      }

      #[test]
      fn a_lone_triple_dash_is_not_frontmatter() {
          let input = "# Body\n---\nmore\n";
          assert_eq!(strip_frontmatter(input), input);
          let unclosed = "---\ntitle: x\nnever closes\n";
          assert_eq!(strip_frontmatter(unclosed), unclosed);
      }

      #[test]
      fn frontmatter_must_start_at_byte_zero() {
          let input = "\n---\ntitle: x\n---\nbody\n";
          assert_eq!(strip_frontmatter(input), input, "leading blank line means no frontmatter");
      }
  }
  ```
- [ ] Add `pub mod frontmatter;` to `lib.rs`.
- [ ] Run: `cargo test -p memharness frontmatter` → expect **FAIL** (`strip_frontmatter` undefined).

## Task 4 — `frontmatter.rs`: implement `strip_frontmatter` (GREEN)

- [ ] Add above the test module:
  ```rust
  /// Strip a leading YAML frontmatter block: the file MUST begin (byte 0) with a `---` line,
  /// and the block ends at the next line that is exactly `---`. Returns the remainder after that
  /// closing fence's newline. No opening fence at byte 0 or no closing fence → input unchanged
  /// (a lone `---` / horizontal rule is NOT frontmatter).
  pub fn strip_frontmatter(input: &str) -> &str {
      let after_open = match input.strip_prefix("---\n") {
          Some(rest) => rest,
          None => return input,
      };
      let mut search_from = 0usize;
      loop {
          let slice = &after_open[search_from..];
          if let Some(rel) = slice.find("---\n") {
              let abs = search_from + rel;
              let at_line_start = abs == 0 || after_open.as_bytes()[abs - 1] == b'\n';
              if at_line_start {
                  return &after_open[abs + 4..];
              }
              search_from = abs + 4;
          } else if let Some(rel) = slice.find("---") {
              let abs = search_from + rel;
              let at_line_start = abs == 0 || after_open.as_bytes()[abs - 1] == b'\n';
              if at_line_start && abs + 3 == after_open.len() {
                  return ""; // closing fence at EOF with no trailing newline
              }
              return input;
          } else {
              return input;
          }
      }
  }
  ```
- [ ] Run: `cargo test -p memharness frontmatter` → **PASS** (4 tests).
- [ ] Commit:
  ```
  git add crates/memharness/src/frontmatter.rs crates/memharness/src/lib.rs
  git commit -m "$(cat <<'EOF'
feat(memharness): frontmatter strip (fence-anchored)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

## Task 5 — `frontmatter.rs`: Hangul language heuristic (RED)

- [ ] Add to the test module:
  ```rust
  #[test]
  fn detects_korean_by_hangul_presence() {
      assert_eq!(detect_lang("안녕하세요 세계"), Lang::Ko);
      assert_eq!(detect_lang("hello world"), Lang::En);
      assert_eq!(detect_lang("the term 메모리 means memory"), Lang::Ko);
      assert_eq!(detect_lang(""), Lang::En);
  }
  ```
- [ ] Run: `cargo test -p memharness frontmatter` → **FAIL** (`detect_lang`/`Lang` undefined).

## Task 6 — `frontmatter.rs`: implement `detect_lang` (GREEN)

- [ ] Add above the test module:
  ```rust
  /// Coarse language tag. Phase 0 only isolates the KO segment (the expected bilingual gap,
  /// spec §3/§8). ANY Hangul codepoint ⇒ `Ko`; mixed folds into `Ko` deliberately.
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum Lang {
      En,
      Ko,
  }

  /// Hangul ranges: Syllables U+AC00–U+D7A3, Jamo U+1100–U+11FF, Compat Jamo U+3130–U+318F.
  pub fn detect_lang(text: &str) -> Lang {
      let has_hangul = text.chars().any(|c| {
          let u = c as u32;
          (0xAC00..=0xD7A3).contains(&u)
              || (0x1100..=0x11FF).contains(&u)
              || (0x3130..=0x318F).contains(&u)
      });
      if has_hangul { Lang::Ko } else { Lang::En }
  }
  ```
- [ ] Run: `cargo test -p memharness frontmatter` → **PASS** (5 tests).
- [ ] Commit:
  ```
  git add crates/memharness/src/frontmatter.rs
  git commit -m "$(cat <<'EOF'
feat(memharness): Hangul-presence language heuristic

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

---

## Task 7 — `corpus.rs`: page-id normalization + manifest types (RED)

- [ ] Create `crates/memharness/src/corpus.rs`:
  ```rust
  //! Corpus preparation: copy `~/brain/*.md` into the harness home (frontmatter stripping is
  //! PROBE-PINNED, spec §2 Rev 2), skip dot-entries, record a sha256 manifest. The
  //! `~/brain`-relative path stem is the arm-independent page identity (spec §5).

  /// Probe-A-pinned (Rev 2): strip frontmatter ONLY if GBrain strips it before chunking; if
  /// GBrain indexes frontmatter, both systems must index it. Default assumes GBrain strips —
  /// Task 1 confirms and the implementer flips this if reality differs.
  pub const STRIP_FRONTMATTER: bool = true;

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn page_id_is_brain_relative_stem() {
          assert_eq!(page_id_from_rel("air/foo.md"), "air/foo");
          assert_eq!(page_id_from_rel("people/kwang-wook-ahn.md"), "people/kwang-wook-ahn");
          assert_eq!(page_id_from_rel("top.md"), "top");
      }

      #[test]
      fn gbrain_slug_maps_to_same_page_id() {
          assert_eq!(page_id_from_gbrain_slug("air/foo"), "air/foo");
          assert_eq!(page_id_from_gbrain_slug("air/foo.md"), "air/foo");
      }

      #[test]
      fn manifest_entry_holds_id_and_hash() {
          let e = ManifestEntry { page_id: "air/foo".into(), sha256: "abc".into(), bytes: 12 };
          assert_eq!(e.page_id, "air/foo");
          assert_eq!(e.bytes, 12);
      }
  }
  ```
- [ ] Add `pub mod corpus;` to `lib.rs`.
- [ ] Run: `cargo test -p memharness corpus` → **FAIL**.

## Task 8 — `corpus.rs`: implement normalization + manifest types (GREEN)

- [ ] Add above the test module:
  ```rust
  use serde::Serialize;

  /// One manifest entry: page id + sha256 of the bytes actually indexed + byte count.
  #[derive(Debug, Clone, Serialize)]
  pub struct ManifestEntry {
      pub page_id: String,
      pub sha256: String,
      pub bytes: u64,
  }

  /// The full manifest recorded in the report (spec §2): snapshot time + per-file entries.
  #[derive(Debug, Clone, Serialize)]
  pub struct CorpusManifest {
      pub snapshot_unix_secs: u64,
      pub file_count: usize,
      pub total_bytes: u64,
      pub entries: Vec<ManifestEntry>,
  }

  /// `~/brain`-relative path ("air/foo.md") → page id ("air/foo").
  pub fn page_id_from_rel(rel: &str) -> String {
      rel.strip_suffix(".md").unwrap_or(rel).to_string()
  }

  /// GBrain slug → the SAME page id space (Probe A pins slugs as stem form; a stray ".md" is
  /// tolerated so a match is never missed on a formatting quirk).
  pub fn page_id_from_gbrain_slug(slug: &str) -> String {
      slug.strip_suffix(".md").unwrap_or(slug).to_string()
  }
  ```
- [ ] Run: `cargo test -p memharness corpus` → **PASS** (3 tests).
- [ ] Commit:
  ```
  git add crates/memharness/src/corpus.rs crates/memharness/src/lib.rs
  git commit -m "$(cat <<'EOF'
feat(memharness): corpus page-id normalization + manifest types

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

## Task 9 — `corpus.rs`: `prepare_corpus` with the probe-pinned strip flag (RED)

- [ ] Add to the test module:
  ```rust
  #[test]
  fn prepare_copies_md_strips_frontmatter_skips_dotdirs() {
      use std::fs;
      let src = tempfile::tempdir().unwrap();
      let dst = tempfile::tempdir().unwrap();
      fs::create_dir_all(src.path().join("air")).unwrap();
      fs::write(src.path().join("air/foo.md"), "---\ntitle: F\n---\n# Foo\nbody\n").unwrap();
      fs::create_dir_all(src.path().join(".obsidian")).unwrap();
      fs::write(src.path().join(".obsidian/cache.md"), "junk\n").unwrap();
      fs::write(src.path().join(".hidden.md"), "junk\n").unwrap();
      fs::write(src.path().join("air/notes.txt"), "not markdown\n").unwrap();

      let manifest = prepare_corpus(src.path(), dst.path(), true).unwrap();

      assert_eq!(manifest.file_count, 1);
      assert_eq!(manifest.entries[0].page_id, "air/foo");
      let copied = fs::read_to_string(dst.path().join("air/foo.md")).unwrap();
      assert_eq!(copied, "# Foo\nbody\n");
      assert!(!dst.path().join(".obsidian").exists());
      assert!(!dst.path().join(".hidden.md").exists());
      assert!(!dst.path().join("air/notes.txt").exists());
      use sha2::{Digest, Sha256};
      assert_eq!(manifest.entries[0].sha256, hex_lower(&Sha256::digest(b"# Foo\nbody\n")));
  }

  #[test]
  fn prepare_with_strip_false_keeps_frontmatter() {
      // Rev 2 (spec §2): if Probe A finds GBrain INDEXES frontmatter, the harness must not strip.
      let src = tempfile::tempdir().unwrap();
      let dst = tempfile::tempdir().unwrap();
      let raw = "---\ntitle: F\n---\n# Foo\nbody\n";
      std::fs::write(src.path().join("foo.md"), raw).unwrap();
      let manifest = prepare_corpus(src.path(), dst.path(), false).unwrap();
      assert_eq!(std::fs::read_to_string(dst.path().join("foo.md")).unwrap(), raw);
      assert_eq!(manifest.entries[0].bytes, raw.len() as u64);
  }
  ```
- [ ] Run: `cargo test -p memharness corpus::tests::prepare` → **FAIL** (`prepare_corpus`, `hex_lower` undefined).

## Task 10 — `corpus.rs`: implement `prepare_corpus` (GREEN)

- [ ] Add above the test module:
  ```rust
  use std::path::{Path, PathBuf};
  use std::time::{SystemTime, UNIX_EPOCH};

  use sha2::{Digest, Sha256};

  use crate::frontmatter::strip_frontmatter;

  /// Lowercase hex of a digest (avoids pulling `hex` for one call site).
  pub fn hex_lower(bytes: &[u8]) -> String {
      let mut s = String::with_capacity(bytes.len() * 2);
      for b in bytes {
          s.push_str(&format!("{b:02x}"));
      }
      s
  }

  /// Recursively copy every `*.md` under `src` into `dst`, optionally stripping YAML frontmatter
  /// (`strip` is the probe-pinned `STRIP_FRONTMATTER`), skipping any entry whose name starts with
  /// '.' (files AND dirs), recording a sha256 manifest of the bytes ACTUALLY indexed. Sorted for
  /// reproducible manifests.
  pub fn prepare_corpus(src: &Path, dst: &Path, strip: bool) -> anyhow::Result<CorpusManifest> {
      let mut rels: Vec<PathBuf> = Vec::new();
      collect_md(src, src, &mut rels)?;
      rels.sort();

      let mut entries = Vec::with_capacity(rels.len());
      let mut total_bytes = 0u64;
      for rel in &rels {
          let raw = std::fs::read_to_string(src.join(rel))?;
          let text = if strip { strip_frontmatter(&raw).to_string() } else { raw };
          let out_path = dst.join(rel);
          if let Some(parent) = out_path.parent() {
              std::fs::create_dir_all(parent)?;
          }
          std::fs::write(&out_path, text.as_bytes())?;
          let sha256 = hex_lower(&Sha256::digest(text.as_bytes()));
          let bytes = text.len() as u64;
          total_bytes += bytes;
          let rel_str = rel.to_string_lossy().replace('\\', "/");
          entries.push(ManifestEntry { page_id: page_id_from_rel(&rel_str), sha256, bytes });
      }
      let snapshot_unix_secs =
          SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
      Ok(CorpusManifest { snapshot_unix_secs, file_count: entries.len(), total_bytes, entries })
  }

  /// Depth-first collect of `*.md` RELATIVE paths, skipping dot-entries at every level.
  fn collect_md(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
      for entry in std::fs::read_dir(dir)? {
          let entry = entry?;
          let name = entry.file_name();
          if name.to_string_lossy().starts_with('.') {
              continue;
          }
          let path = entry.path();
          if path.is_dir() {
              collect_md(root, &path, out)?;
          } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
              out.push(path.strip_prefix(root).unwrap_or(&path).to_path_buf());
          }
      }
      Ok(())
  }
  ```
- [ ] Run: `cargo test -p memharness corpus` → **PASS** (5 tests).
- [ ] Commit:
  ```
  git add crates/memharness/src/corpus.rs
  git commit -m "$(cat <<'EOF'
feat(memharness): prepare_corpus — copy + probe-pinned strip flag + dot-skip + sha256 manifest

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

---

## Task 11 — `ollama.rs`: model-availability preflight (RED)

- [ ] Create `crates/memharness/src/ollama.rs`:
  ```rust
  //! Loopback HTTP client for Ollama (127.0.0.1:11434) via `ureq` v2 (mirrors bossclaw-core's
  //! pin). Two uses: the availability preflight (`/api/tags`) and single-turn generation
  //! (`/api/generate`). Default model pinned by Probe B.

  /// Probe-B-pinned default local model (the evolve loop's tier is `qwen2.5:7b-instruct`;
  /// match whichever tag is actually installed).
  pub const DEFAULT_OLLAMA_MODEL: &str = "qwen2.5:7b";

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn parses_model_names_from_tags_body() {
          let body = r#"{"models":[{"name":"qwen2.5:7b"},{"name":"llama3:8b"}]}"#;
          let names = parse_tag_names(body).unwrap();
          assert!(names.contains(&"qwen2.5:7b".to_string()));
          assert!(names.contains(&"llama3:8b".to_string()));
      }

      #[test]
      fn missing_model_yields_clear_error() {
          let names = vec!["llama3:8b".to_string()];
          let err = require_model(&names, "qwen2.5:7b").unwrap_err();
          let msg = err.to_string();
          assert!(msg.contains("qwen2.5:7b"), "names the missing model: {msg}");
          assert!(msg.contains("ollama pull"), "tells the user the fix: {msg}");
      }

      #[test]
      fn present_model_passes() {
          let names = vec!["qwen2.5:7b".to_string()];
          assert!(require_model(&names, "qwen2.5:7b").is_ok());
      }
  }
  ```
- [ ] Add `pub mod ollama;` to `lib.rs`.
- [ ] Run: `cargo test -p memharness ollama` → **FAIL**.

## Task 12 — `ollama.rs`: implement preflight + generate (GREEN)

- [ ] Add above the test module:
  ```rust
  use serde::Deserialize;

  const OLLAMA_TAGS_URL: &str = "http://127.0.0.1:11434/api/tags";
  const OLLAMA_GENERATE_URL: &str = "http://127.0.0.1:11434/api/generate";

  #[derive(Deserialize)]
  struct TagsBody {
      models: Vec<TagModel>,
  }
  #[derive(Deserialize)]
  struct TagModel {
      name: String,
  }

  /// Parse model names out of an `/api/tags` body.
  pub fn parse_tag_names(body: &str) -> anyhow::Result<Vec<String>> {
      let parsed: TagsBody = serde_json::from_str(body)?;
      Ok(parsed.models.into_iter().map(|m| m.name).collect())
  }

  /// Require `model` in `names`, else a clear actionable error.
  pub fn require_model(names: &[String], model: &str) -> anyhow::Result<()> {
      if names.iter().any(|n| n == model) {
          Ok(())
      } else {
          anyhow::bail!(
              "Ollama model '{model}' is not installed (have: {}). Run: `ollama pull {model}` \
               (or pass --model <installed-tag>).",
              names.join(", ")
          )
      }
  }

  /// LIVE preflight: `/api/tags` + require `model`. Failures name the fix.
  pub fn preflight(model: &str) -> anyhow::Result<()> {
      let body = ureq::get(OLLAMA_TAGS_URL)
          .call()
          .map_err(|e| anyhow::anyhow!(
              "Ollama not reachable on 127.0.0.1:11434 ({e}). Start it with `ollama serve`."
          ))?
          .into_string()?;
      require_model(&parse_tag_names(&body)?, model)
  }

  #[derive(serde::Serialize)]
  struct GenerateReq<'a> {
      model: &'a str,
      prompt: &'a str,
      stream: bool,
  }
  #[derive(Deserialize)]
  struct GenerateResp {
      response: String,
  }

  /// Single-turn generation (`stream:false` → one JSON object). Used by synth, the answerer,
  /// and the local judge.
  pub fn generate(model: &str, prompt: &str) -> anyhow::Result<String> {
      let resp: GenerateResp = ureq::post(OLLAMA_GENERATE_URL)
          .send_json(GenerateReq { model, prompt, stream: false })
          .map_err(|e| anyhow::anyhow!("Ollama generate failed: {e}"))?
          .into_json()?;
      Ok(resp.response)
  }
  ```
- [ ] Run: `cargo test -p memharness ollama` → **PASS** (3 tests; `preflight`/`generate` are live-run-only).
- [ ] Commit:
  ```
  git add crates/memharness/src/ollama.rs crates/memharness/src/lib.rs
  git commit -m "$(cat <<'EOF'
feat(memharness): Ollama loopback client — tags preflight + generate

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

---

## Task 13 — `bossclawd` test-helpers: `test_engine_with_embedder` (RED)

**(Rev 2, finding 1 — CRITICAL.)** `test_engine` hardwires `MockEmbedder::new(8)`; the harness must inject the PRODUCTION embedder. This task adds a tiny, feature-gated, TDD'd constructor to `bossclawd` — the plan's only change outside `crates/memharness`.

- [ ] Add a failing integration test to `crates/bossclawd/tests/roundtrip.rs` (bottom of the file):
  ```rust
  // ── test-helpers seam: a caller-supplied embedder provider reaches the engine (memharness
  //    Phase 0 needs to inject the PRODUCTION ResourceModel2Vec; this proves the seam with a
  //    distinguishable custom mock). ──

  /// A custom provider with a non-default dimension, so passing it through is observable.
  struct WideMockEmbedderProvider;
  impl bossclawd::engine::embed::EmbedderProvider for WideMockEmbedderProvider {
      fn embedder(
          &self,
      ) -> Result<std::sync::Arc<dyn bossclaw_core::Embedder>, bossclawd::engine::EngineOpError> {
          Ok(std::sync::Arc::new(bossclaw_core::MockEmbedder::new(16)))
      }
  }

  #[tokio::test]
  async fn custom_embedder_provider_reaches_recall_over_the_wire() {
      use std::os::unix::fs::PermissionsExt;
      use tokio::net::UnixListener;

      bossclawd::vault::seed_secret_cache_for_test(Default::default());
      let dir = tempfile::tempdir().unwrap();
      let sock_path = dir.path().join("bossclawd.sock");
      let engine = std::sync::Arc::new(bossclawd::server::test_engine_with_embedder(
          dir.path().to_path_buf(),
          std::sync::Arc::new(WideMockEmbedderProvider),
      ));
      let listener = UnixListener::bind(&sock_path).expect("bind test socket");
      std::fs::set_permissions(&sock_path, std::fs::Permissions::from_mode(0o600)).unwrap();
      tokio::spawn(bossclawd::server::run_accept_loop(engine, listener));

      let src = tempfile::tempdir().unwrap();
      std::fs::write(src.path().join("a.txt"), "ferris the crab loves rust").unwrap();
      let mut client = Client::connect(&sock_path).await;
      assert!(matches!(
          client.call(Request::AddGrant { onboarded: true, path: src.path().to_path_buf() }).await,
          Response::Ok
      ));
      assert!(matches!(
          client.call(Request::RunIngest { onboarded: true }).await,
          Response::RunIngest(_)
      ));
      match client.call(Request::Recall { onboarded: true, query: "ferris crab".into(), k: 5 }).await {
          Response::Recall(hits) => {
              assert!(hits.iter().any(|h| h.text.contains("ferris")),
                  "recall works with the injected provider");
          }
          other => panic!("expected Recall, got {other:?}"),
      }
  }
  ```
- [ ] Run: `cargo test -p bossclawd --test roundtrip custom_embedder` → expect **FAIL** (`test_engine_with_embedder` undefined).

## Task 14 — `bossclawd`: implement `test_engine_with_embedder` (GREEN)

- [ ] In `crates/bossclawd/src/server.rs`, REPLACE the existing `test_engine` with the pair (same cfg gate, same section):
  ```rust
  /// Build a hermetic `EngineHandle` for tests: in-memory vault + mock embedder + mock reasoner.
  /// Uses `bossclaw_core::MockEmbedder`/`ScriptedReasoner` (public, non-cfg-gated in core) so the
  /// integration test — which can't reach the lib's `#[cfg(test)]` mock providers — still gets a
  /// keychain-free engine.
  #[cfg(any(test, feature = "test-helpers"))]
  pub fn test_engine(home: std::path::PathBuf) -> EngineHandle {
      test_engine_with_embedder(home, Arc::new(TestEmbedderProvider))
  }

  /// Like [`test_engine`] but with a CALLER-SUPPLIED embedder provider. Added for `memharness`
  /// (memory-strategy Phase 0): the harness must measure the REAL production embedder
  /// (`engine::embed::ResourceModel2Vec`), not the dim-8 mock — while keeping the in-memory
  /// vault (keychain-free) and the scripted reasoner (no evolve, no reasoner egress). Behind
  /// `test-helpers` like its sibling; never reaches production builds.
  #[cfg(any(test, feature = "test-helpers"))]
  pub fn test_engine_with_embedder(
      home: std::path::PathBuf,
      embedder: Arc<dyn crate::engine::embed::EmbedderProvider>,
  ) -> EngineHandle {
      EngineHandle::new(Arc::new(TestVault::default()), home, embedder, Arc::new(TestReasonerProvider))
  }
  ```
- [ ] Run: `cargo test -p bossclawd` → **ALL PASS** (existing roundtrip suite + the new test; `test_engine` callers are unaffected — it delegates).
- [ ] Run: `cargo build -p bossclawd` (no features) → compiles; the helpers are cfg'd out (finding 3's compile-time proof, re-asserted in Task 46).
- [ ] Commit:
  ```
  git add crates/bossclawd/src/server.rs crates/bossclawd/tests/roundtrip.rs
  git commit -m "$(cat <<'EOF'
feat(bossclawd): test_engine_with_embedder — injectable embedder for the memharness real-embedder arm

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

---

## Task 15 — `daemon.rs`: `HarnessDaemon` with injectable embedder + real-model-dir resolution (RED)

- [ ] Create `crates/memharness/src/daemon.rs` with the failing tests:
  ```rust
  //! The isolated in-process daemon: a real `bossclawd` accept loop on a private 0600 socket under
  //! a per-run temp home, on its own current-thread runtime + OS thread (the desktop `TestDaemon`
  //! pattern — killing the runtime tears down the accept loop AND every connection task). NEVER
  //! touches the OS keychain (provider-key cache seeded empty).
  //!
  //! Embedder (spec §1 Rev 2): the LIVE run injects the PRODUCTION `ResourceModel2Vec`
  //! (potion-base-8M) via `spawn_real`; `spawn_mock_for_plumbing_tests` exists ONLY for hermetic
  //! plumbing tests — quality numbers come from the live run with the real embedder.

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn spawns_a_0600_socket_and_kills_clean() {
          // Mock embedder: PLUMBING TEST ONLY — quality numbers come from the live run with the
          // real embedder (spec §1 Rev 2).
          let mut d = HarnessDaemon::spawn_mock_for_plumbing_tests().expect("spawn daemon");
          use std::os::unix::fs::PermissionsExt;
          let mode = std::fs::metadata(d.socket_path()).unwrap().permissions().mode();
          assert_eq!(mode & 0o777, 0o600, "socket must be 0600, got {mode:o}");
          d.kill();
          assert!(!d.socket_path().exists(), "socket removed on kill");
      }

      #[test]
      fn model_dir_resolution_prefers_override_then_repo_fallback() {
          // Override wins when it points at a dir holding model.safetensors.
          let fake = tempfile::tempdir().unwrap();
          std::fs::write(fake.path().join("model.safetensors"), b"weights").unwrap();
          let got = resolve_model_dir_from(Some(fake.path().to_path_buf())).unwrap();
          assert_eq!(got, fake.path());

          // An override pointing at a dir WITHOUT the model file fails with the fetch hint.
          let empty = tempfile::tempdir().unwrap();
          let err = resolve_model_dir_from(Some(empty.path().to_path_buf())).unwrap_err();
          assert!(err.to_string().contains("fetch-model.sh"), "actionable: {err}");

          // No override → the repo fallback path is named (existence checked at spawn_real time;
          // on this checkout it exists because fetch-model.sh has been run).
          let fallback = repo_model_dir_fallback();
          assert!(fallback.ends_with("apps/desktop/src-tauri/resources/models/potion-base-8M"));
      }
  }
  ```
- [ ] Add `pub mod daemon;` to `lib.rs`.
- [ ] Run: `cargo test -p memharness daemon` → **FAIL**.

## Task 16 — `daemon.rs`: implement `HarnessDaemon` (GREEN)

- [ ] Add above the test module:
  ```rust
  use std::path::{Path, PathBuf};
  use std::sync::Arc;

  use bossclawd::engine::embed::{EmbedderProvider, ResourceModel2Vec};
  use tokio::sync::Notify;

  /// A running isolated daemon. `dir` (temp home + socket) lives as long as this struct;
  /// `rt` is `None` after `kill()`.
  pub struct HarnessDaemon {
      dir: tempfile::TempDir,
      sock: PathBuf,
      rt: Option<DaemonRuntime>,
  }

  struct DaemonRuntime {
      shutdown: Arc<Notify>,
      thread: std::thread::JoinHandle<()>,
  }

  /// The env override the production daemon also honors.
  const ENV_MODEL_DIR: &str = "BOSSCLAWD_MODEL_DIR";

  /// The repo checkout's model dir (populated by scripts/fetch-model.sh).
  pub fn repo_model_dir_fallback() -> PathBuf {
      PathBuf::from(env!("CARGO_MANIFEST_DIR"))
          .join("../../apps/desktop/src-tauri/resources/models/potion-base-8M")
  }

  /// Model-dir resolution with an explicit override (testable core): the override or the repo
  /// fallback MUST contain `model.safetensors`, else a fail-fast error naming the fix — this is
  /// the real-embedder preflight, mirroring the Ollama preflight (spec §1 Rev 2).
  pub fn resolve_model_dir_from(env_override: Option<PathBuf>) -> anyhow::Result<PathBuf> {
      let dir = env_override.unwrap_or_else(repo_model_dir_fallback);
      if dir.join("model.safetensors").is_file() {
          Ok(dir)
      } else {
          anyhow::bail!(
              "embedder model dir {dir:?} is missing model.safetensors — run scripts/fetch-model.sh \
               (or set {ENV_MODEL_DIR} to a populated model dir)"
          )
      }
  }

  /// Live resolution: `BOSSCLAWD_MODEL_DIR` env → repo fallback, preflighted.
  pub fn resolve_real_model_dir() -> anyhow::Result<PathBuf> {
      resolve_model_dir_from(std::env::var_os(ENV_MODEL_DIR).map(PathBuf::from))
  }

  impl HarnessDaemon {
      /// LIVE-RUN constructor: the PRODUCTION embedder (`ResourceModel2Vec`, potion-base-8M),
      /// model dir resolved + preflighted. This is the ONLY constructor `main.rs` uses.
      pub fn spawn_real() -> anyhow::Result<Self> {
          let model_dir = resolve_real_model_dir()?;
          Self::spawn_with_provider(Arc::new(ResourceModel2Vec::new(model_dir)))
      }

      /// PLUMBING TESTS ONLY: the dim-8 mock embedder (via bossclawd's `test_engine` default).
      /// Quality numbers come from the live run with the real embedder — never from this.
      pub fn spawn_mock_for_plumbing_tests() -> anyhow::Result<Self> {
          Self::spawn_inner(None)
      }

      /// Spawn with an explicit embedder provider (the `spawn_real` path; also lets a future
      /// experiment inject a candidate embedder behind the same seam).
      pub fn spawn_with_provider(provider: Arc<dyn EmbedderProvider>) -> anyhow::Result<Self> {
          Self::spawn_inner(Some(provider))
      }

      fn spawn_inner(provider: Option<Arc<dyn EmbedderProvider>>) -> anyhow::Result<Self> {
          // HERMETIC: seed the process-global provider-key cache EMPTY so provider-key reads
          // short-circuit and never hit the OS keychain (keychain-ACL hang hazard).
          bossclawd::vault::seed_secret_cache_for_test(std::collections::HashMap::new());
          let dir = tempfile::tempdir()?;
          let sock = dir.path().join("bossclawd.sock");
          let rt = Self::start_runtime(&sock, dir.path().to_path_buf(), provider)?;
          Ok(Self { dir, sock, rt: Some(rt) })
      }

      /// Own current-thread runtime + OS thread; blocks until the listener is bound
      /// (`sync_channel` handshake) so a client connect can't race the bind.
      fn start_runtime(
          sock: &Path,
          home: PathBuf,
          provider: Option<Arc<dyn EmbedderProvider>>,
      ) -> anyhow::Result<DaemonRuntime> {
          use std::os::unix::fs::PermissionsExt;
          let shutdown = Arc::new(Notify::new());
          let shutdown_for_thread = shutdown.clone();
          let sock_buf = sock.to_path_buf();
          let (bound_tx, bound_rx) = std::sync::mpsc::sync_channel::<anyhow::Result<()>>(0);
          let thread = std::thread::spawn(move || {
              let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                  Ok(rt) => rt,
                  Err(e) => {
                      let _ = bound_tx.send(Err(anyhow::anyhow!("build daemon runtime: {e}")));
                      return;
                  }
              };
              rt.block_on(async move {
                  let listener = match tokio::net::UnixListener::bind(&sock_buf) {
                      Ok(l) => l,
                      Err(e) => {
                          let _ = bound_tx.send(Err(anyhow::anyhow!("bind socket: {e}")));
                          return;
                      }
                  };
                  // Pin 0600 (owner-only), matching production bind_socket_0600.
                  if let Err(e) = std::fs::set_permissions(
                      &sock_buf,
                      std::fs::Permissions::from_mode(0o600),
                  ) {
                      let _ = bound_tx.send(Err(anyhow::anyhow!("chmod socket 0600: {e}")));
                      return;
                  }
                  let engine = Arc::new(match provider {
                      Some(p) => bossclawd::server::test_engine_with_embedder(home, p),
                      None => bossclawd::server::test_engine(home),
                  });
                  if bound_tx.send(Ok(())).is_err() {
                      return; // caller gone
                  }
                  tokio::select! {
                      _ = bossclawd::server::run_accept_loop(engine, listener) => {}
                      _ = shutdown_for_thread.notified() => {}
                  }
              });
              // Runtime dropped at end of scope → all daemon tasks gone.
          });
          bound_rx.recv().map_err(|_| anyhow::anyhow!("daemon thread died before binding"))??;
          Ok(DaemonRuntime { shutdown, thread })
      }

      /// The private socket path (for a `WireClient`).
      pub fn socket_path(&self) -> &Path {
          &self.sock
      }

      /// The per-run home (corpus is copied under it).
      pub fn home(&self) -> &Path {
          self.dir.path()
      }

      /// Fully kill the daemon: notify shutdown, join the thread (drops the runtime + every
      /// connection task), remove the socket file.
      pub fn kill(&mut self) {
          if let Some(rt) = self.rt.take() {
              rt.shutdown.notify_waiters();
              let _ = rt.thread.join();
          }
          let _ = std::fs::remove_file(&self.sock);
      }
  }

  impl Drop for HarnessDaemon {
      fn drop(&mut self) {
          if let Some(rt) = self.rt.take() {
              rt.shutdown.notify_waiters();
              let _ = rt.thread.join();
          }
      }
  }
  ```
- [ ] Run: `cargo test -p memharness daemon` → **PASS** (2 tests).
- [ ] Commit:
  ```
  git add crates/memharness/src/daemon.rs crates/memharness/src/lib.rs
  git commit -m "$(cat <<'EOF'
feat(memharness): HarnessDaemon — injectable embedder, spawn_real preflights the production model dir

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

---

## Task 17 — `client.rs`: `WireClient` incl. `list_files` (RED)

- [ ] Create `crates/memharness/src/client.rs` with the failing test (drives a REAL `HarnessDaemon`):
  ```rust
  //! Thin wire client: Hello/HelloOk once, then one Request → one Response per op (single
  //! in-flight). `read_frame`/`write_frame` are NOT cancellation-safe, so the timeout wraps the
  //! WHOLE op and a timed-out/error'd stream is DROPPED, never reused.

  #[cfg(test)]
  mod tests {
      use super::*;
      use crate::daemon::HarnessDaemon;

      #[tokio::test]
      async fn grant_ingest_list_files_recall_over_wire() {
          // Mock embedder: PLUMBING TEST ONLY — quality numbers come from the live run with the
          // real embedder (spec §1 Rev 2).
          let d = HarnessDaemon::spawn_mock_for_plumbing_tests().unwrap();
          let corpus = d.home().join("corpus");
          std::fs::create_dir_all(&corpus).unwrap();
          std::fs::write(corpus.join("a.md"), "ferris the crab loves rust").unwrap();

          let mut client = WireClient::connect(d.socket_path()).await.unwrap();
          client.add_grant(&corpus).await.unwrap();
          let report = client.run_ingest().await.unwrap();
          assert_eq!(report.ingested, 1, "one page ingested");

          // ListFiles: the event_id → canonical_path bridge's source (spec §5 Rev 2).
          let files = client.list_files().await.unwrap();
          assert_eq!(files.len(), 1);
          assert!(files[0].canonical_path.ends_with("a.md"));
          assert!(!files[0].file_event_id.is_empty());

          let hits = client.recall("ferris crab", 5).await.unwrap();
          assert!(hits.iter().any(|h| h.text.contains("ferris")), "recall hydrates the snippet");
      }
  }
  ```
- [ ] Add `pub mod client;` to `lib.rs`.
- [ ] Run: `cargo test -p memharness client` → **FAIL**.

## Task 18 — `client.rs`: implement `WireClient` (GREEN)

- [ ] Add above the test module:
  ```rust
  use std::path::Path;
  use std::time::Duration;

  use bossclawd_proto::types::{FileRecordMirror, IngestReportMirror};
  use bossclawd_proto::{
      read_frame, write_frame, Hello, HelloOk, HitWire, Request, Response, PROTO_VERSION,
  };
  use tokio::net::UnixStream;

  /// Per-op bound so a hung daemon can't wedge a multi-hour run. Ingest of ~866 pages through
  /// the real embedder is minutes; 600s is ample headroom.
  const OP_TIMEOUT: Duration = Duration::from_secs(600);

  /// A connected wire client: one `UnixStream`, one op in flight at a time.
  pub struct WireClient {
      stream: UnixStream,
  }

  impl WireClient {
      /// Connect + Hello/HelloOk handshake; verifies the protocol version.
      pub async fn connect(sock: &Path) -> anyhow::Result<Self> {
          let mut stream = UnixStream::connect(sock).await?;
          let hello = Hello { proto_version: PROTO_VERSION };
          write_frame(&mut stream, &serde_json::to_vec(&hello)?).await?;
          let reply = read_frame(&mut stream).await?;
          let hello_ok: HelloOk = serde_json::from_slice(&reply)?;
          if hello_ok.proto_version != PROTO_VERSION {
              anyhow::bail!("daemon protocol {} != client {}", hello_ok.proto_version, PROTO_VERSION);
          }
          Ok(Self { stream })
      }

      /// One Request → one Response, bounded by `OP_TIMEOUT`. On timeout the frame future is
      /// dropped mid-I/O — the stream is corrupt and MUST NOT be reused; the error tells the
      /// caller to discard this client.
      async fn call(&mut self, req: Request) -> anyhow::Result<Response> {
          let fut = async {
              write_frame(&mut self.stream, &serde_json::to_vec(&req)?).await?;
              let frame = read_frame(&mut self.stream).await?;
              Ok::<Response, anyhow::Error>(serde_json::from_slice(&frame)?)
          };
          match tokio::time::timeout(OP_TIMEOUT, fut).await {
              Ok(r) => r,
              Err(_) => anyhow::bail!("wire op timed out after {OP_TIMEOUT:?}; stream is now unusable"),
          }
      }

      /// `AddGrant` (onboarded=true).
      pub async fn add_grant(&mut self, path: &Path) -> anyhow::Result<()> {
          match self.call(Request::AddGrant { onboarded: true, path: path.to_path_buf() }).await? {
              Response::Ok => Ok(()),
              other => anyhow::bail!("AddGrant → unexpected {other:?}"),
          }
      }

      /// `RunIngest` (onboarded=true) → the ingest report.
      pub async fn run_ingest(&mut self) -> anyhow::Result<IngestReportMirror> {
          match self.call(Request::RunIngest { onboarded: true }).await? {
              Response::RunIngest(r) => Ok(r),
              other => anyhow::bail!("RunIngest → unexpected {other:?}"),
          }
      }

      /// `ListFiles` (onboarded=true) → the current file records: the `event_id → page_id`
      /// bridge's source (spec §5 Rev 2).
      pub async fn list_files(&mut self) -> anyhow::Result<Vec<FileRecordMirror>> {
          match self.call(Request::ListFiles { onboarded: true }).await? {
              Response::ListFiles(files) => Ok(files),
              other => anyhow::bail!("ListFiles → unexpected {other:?}"),
          }
      }

      /// `Recall` (onboarded=true) → the hydrated hits.
      pub async fn recall(&mut self, query: &str, k: usize) -> anyhow::Result<Vec<HitWire>> {
          match self.call(Request::Recall { onboarded: true, query: query.to_string(), k }).await? {
              Response::Recall(hits) => Ok(hits),
              other => anyhow::bail!("Recall → unexpected {other:?}"),
          }
      }
  }
  ```
  > **Implementer note:** confirm `IngestReportMirror`/`FileRecordMirror` module paths (both live in `bossclawd_proto::types` per `types.rs`; adjust the `use` if the re-export differs).
- [ ] Run: `cargo test -p memharness client` → **PASS**.
- [ ] Commit:
  ```
  git add crates/memharness/src/client.rs crates/memharness/src/lib.rs
  git commit -m "$(cat <<'EOF'
feat(memharness): WireClient — grant/ingest/list_files/recall, timeout-bounded, drop-on-error

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

---

## Task 19 — `resolve.rs`: the `event_id → page_id` bridge (RED)

**(Rev 2, finding 2 — CRITICAL.)**

- [ ] Create `crates/memharness/src/resolve.rs` with the failing tests:
  ```rust
  //! The `event_id → page_id` bridge (spec §5 Rev 2, review-critical).
  //!
  //! INVARIANT (load-bearing): the harness NEVER runs an evolve tick, so every recall hit is a
  //! `file_ingested` event whose `event_id` equals the `file_event_id` that `ListFiles` reports
  //! for its source file (bossclaw-core `graph.rs`: `file_event_id: ev.id`). If a recall hit does
  //! not map through this table, that invariant has broken (e.g. someone added an evolve call to
  //! the harness, whose minted memory events have no file mapping) → the mapping FAILS LOUD as a
  //! run error. There is deliberately NO fallback to the raw event id: a silent fallback could
  //! never match a gold page id, would score AIR 0.0 on every known-item query, and would
  //! fabricate a losing baseline.

  #[cfg(test)]
  mod tests {
      use super::*;
      use bossclawd_proto::types::FileRecordMirror;

      fn record(root: &std::path::Path, rel: &str, event_id: &str) -> FileRecordMirror {
          FileRecordMirror {
              canonical_path: root.join(rel).to_string_lossy().to_string(),
              file_event_id: event_id.to_string(),
              content_hash: "h".to_string(),
              grant_root: root.to_string_lossy().to_string(),
          }
      }

      #[test]
      fn maps_event_ids_to_page_ids() {
          let root = tempfile::tempdir().unwrap();
          let canon = std::fs::canonicalize(root.path()).unwrap();
          let records = vec![record(&canon, "air/foo.md", "ev1"), record(&canon, "top.md", "ev2")];
          let r = PageResolver::from_file_records(&records, root.path()).unwrap();
          assert_eq!(r.page_id_of("ev1").unwrap(), "air/foo");
          assert_eq!(r.page_id_of("ev2").unwrap(), "top");
      }

      #[test]
      fn unmapped_event_id_fails_loud_naming_the_invariant() {
          let root = tempfile::tempdir().unwrap();
          let r = PageResolver::from_file_records(&[], root.path()).unwrap();
          let err = r.page_id_of("evolve-minted-event").unwrap_err();
          assert!(err.to_string().contains("invariant"), "names the broken invariant: {err}");
      }

      #[test]
      fn record_outside_corpus_root_is_an_error() {
          let root = tempfile::tempdir().unwrap();
          let other = tempfile::tempdir().unwrap();
          let canon_other = std::fs::canonicalize(other.path()).unwrap();
          let records = vec![record(&canon_other, "x.md", "ev1")];
          assert!(PageResolver::from_file_records(&records, root.path()).is_err());
      }
  }
  ```
- [ ] Add `pub mod resolve;` to `lib.rs`.
- [ ] Run: `cargo test -p memharness resolve` → **FAIL**.

## Task 20 — `resolve.rs`: implement `PageResolver` (GREEN)

- [ ] Add above the test module:
  ```rust
  use std::collections::HashMap;
  use std::path::Path;

  use bossclawd_proto::types::FileRecordMirror;

  use crate::corpus::page_id_from_rel;

  /// The bridge table: `file_event_id → page_id`. Built once per run, right after ingest.
  pub struct PageResolver {
      by_event: HashMap<String, String>,
  }

  impl PageResolver {
      /// Build from `ListFiles` records: `file_event_id → canonical_path` → strip the
      /// (canonicalized) corpus-root prefix → page id. A record outside the corpus root is an
      /// error (the harness grants exactly one root).
      pub fn from_file_records(
          records: &[FileRecordMirror],
          corpus_root: &Path,
      ) -> anyhow::Result<Self> {
          let root = std::fs::canonicalize(corpus_root)
              .map_err(|e| anyhow::anyhow!("canonicalize corpus root {corpus_root:?}: {e}"))?;
          let root_str = root.to_string_lossy().to_string();
          let mut by_event = HashMap::with_capacity(records.len());
          for r in records {
              let rel = r
                  .canonical_path
                  .strip_prefix(&root_str)
                  .map(|s| s.trim_start_matches('/'))
                  .ok_or_else(|| anyhow::anyhow!(
                      "ingested file {} is outside the corpus root {root_str}",
                      r.canonical_path
                  ))?;
              by_event.insert(r.file_event_id.clone(), page_id_from_rel(rel));
          }
          Ok(Self { by_event })
      }

      /// Map a recall hit's event id to its page id. FAILS LOUD on an unmapped id — see the
      /// module docs: no evolve ⇒ every hit is a file_ingested event; an unmapped hit means the
      /// invariant broke, and scoring must stop rather than silently zero AIR's scores.
      pub fn page_id_of(&self, event_id: &str) -> anyhow::Result<String> {
          self.by_event.get(event_id).cloned().ok_or_else(|| anyhow::anyhow!(
              "recall hit event {event_id} does not map to an ingested file — the no-evolve \
               invariant broke (or ListFiles is stale); refusing to score (no silent fallback)"
          ))
      }
  }
  ```
- [ ] Run: `cargo test -p memharness resolve` → **PASS** (3 tests).
- [ ] Commit:
  ```
  git add crates/memharness/src/resolve.rs crates/memharness/src/lib.rs
  git commit -m "$(cat <<'EOF'
feat(memharness): PageResolver — ListFiles event_id→page_id bridge, fail-loud invariant

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

---

## Task 21 — `mine.rs`: transcript parse + labels + dedup (RED)

- [ ] Author the SYNTHETIC fixture `crates/memharness/tests/fixtures/transcript_synthetic.jsonl` (hand-written — never real transcripts; the row shape is the recon assumption, confirmed/adjusted with the fixture in the same commit if Probe C finds it differs):
  ```jsonl
  {"type":"tool_use","name":"mcp__gbrain__query","input":{"query":"who is Aria Novak"},"session":"s1","ts":1}
  {"type":"tool_use","name":"mcp__gbrain__get_page","input":{"slug":"people/aria-novak"},"session":"s1","ts":2}
  {"type":"tool_use","name":"mcp__gbrain__search","input":{"query":"memory strategy phase 0"},"session":"s1","ts":3}
  {"type":"tool_use","name":"mcp__other__thing","input":{"x":1},"session":"s1","ts":4}
  {"type":"tool_use","name":"mcp__gbrain__query","input":{"query":"who is Aria Novak"},"session":"s2","ts":5}
  {"type":"tool_use","name":"mcp__gbrain__query","input":{"query":"메모리 전략"},"session":"s2","ts":6}
  {"type":"tool_use","name":"mcp__gbrain__get_page","input":{"slug":"air/memory-strategy"},"session":"s2","ts":13}
  ```
  (The `ts:13` get_page is >5 tool calls after `ts:6` → NOT a label; `ts:2` is within 5 of `ts:1` → a label.)
- [ ] Create `crates/memharness/src/mine.rs` with the failing test:
  ```rust
  //! Mine real queries from Claude Code transcripts: `mcp__gbrain__{query,search,recall}` calls
  //! become queries; a `mcp__gbrain__get_page` within the next N=5 tool calls of the SAME session
  //! labels that page as the used answer (spec §3). Exact dedup (near-dup deferred — a REPORTED
  //! caveat, `near_dedup_applied: false`).

  /// A get_page within this many subsequent tool calls (same session) labels a query.
  pub const LABEL_WINDOW: usize = 5;

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn parses_queries_labels_and_dedups() {
          let jsonl = include_str!("../tests/fixtures/transcript_synthetic.jsonl");
          let queries = mine_transcript(jsonl);
          let aria: Vec<_> = queries.iter().filter(|q| q.text == "who is Aria Novak").collect();
          assert_eq!(aria.len(), 1, "exact duplicate deduped");
          assert_eq!(aria[0].gold_page_id.as_deref(), Some("people/aria-novak"));
          assert!(queries.iter().any(|q| q.text == "메모리 전략"));
          let ko = queries.iter().find(|q| q.text == "메모리 전략").unwrap();
          assert_eq!(ko.gold_page_id, None, "get_page outside the window is not a label");
          assert!(!queries.iter().any(|q| q.text.contains("mcp__other")));
      }

      #[test]
      fn mine_all_dedups_across_documents() {
          let a = r#"{"type":"tool_use","name":"mcp__gbrain__query","input":{"query":"q1"},"session":"a","ts":1}"#;
          let b = r#"{"type":"tool_use","name":"mcp__gbrain__query","input":{"query":"q1"},"session":"b","ts":1}
{"type":"tool_use","name":"mcp__gbrain__get_page","input":{"slug":"x/y"},"session":"b","ts":2}"#;
          let merged = mine_all([a, b]);
          assert_eq!(merged.len(), 1, "cross-file exact dedup");
          assert_eq!(merged[0].gold_page_id.as_deref(), Some("x/y"), "a later duplicate's label fills in");
      }
  }
  ```
- [ ] Add `pub mod mine;` to `lib.rs`.
- [ ] Run: `cargo test -p memharness mine` → **FAIL**.

## Task 22 — `mine.rs`: implement mining (GREEN)

- [ ] Add above the test module:
  ```rust
  use serde::Deserialize;

  /// A mined real query + its optional implicit known-item label.
  #[derive(Debug, Clone)]
  pub struct MinedQuery {
      pub text: String,
      pub gold_page_id: Option<String>,
      pub session: String,
  }

  #[derive(Deserialize)]
  struct Event {
      #[serde(default)]
      name: String,
      #[serde(default)]
      input: serde_json::Value,
      #[serde(default)]
      session: String,
  }

  const QUERY_TOOLS: [&str; 3] =
      ["mcp__gbrain__query", "mcp__gbrain__search", "mcp__gbrain__recall"];
  const GET_PAGE_TOOL: &str = "mcp__gbrain__get_page";

  /// Parse ONE transcript's JSONL (lines that don't parse are skipped — transcripts are
  /// heterogeneous). Query tools → queries; a get_page within `LABEL_WINDOW` subsequent tool
  /// calls of the same session labels the most recent unlabeled query. Exact-text dedup last.
  pub fn mine_transcript(jsonl: &str) -> Vec<MinedQuery> {
      let events: Vec<Event> = jsonl
          .lines()
          .filter(|l| !l.trim().is_empty())
          .filter_map(|l| serde_json::from_str::<Event>(l).ok())
          .collect();

      let mut mined: Vec<MinedQuery> = Vec::new();
      for (i, ev) in events.iter().enumerate() {
          if QUERY_TOOLS.contains(&ev.name.as_str()) {
              if let Some(text) = ev.input.get("query").and_then(|v| v.as_str()) {
                  mined.push(MinedQuery {
                      text: text.to_string(),
                      gold_page_id: None,
                      session: ev.session.clone(),
                  });
              }
          } else if ev.name == GET_PAGE_TOOL {
              if let Some(slug) = ev.input.get("slug").and_then(|v| v.as_str()) {
                  label_recent(&mut mined, &events, i, &ev.session, slug);
              }
          }
      }
      dedup_exact(mined)
  }

  /// Merge-mine MANY transcripts, then exact-dedup the union (a query repeated across files
  /// counts once; a later duplicate's label fills an unlabeled first occurrence).
  pub fn mine_all<'a>(docs: impl IntoIterator<Item = &'a str>) -> Vec<MinedQuery> {
      let mut all = Vec::new();
      for d in docs {
          all.extend(mine_transcript(d));
      }
      dedup_exact(all)
  }

  /// Label the most recent unlabeled query of `session` whose originating event index is within
  /// LABEL_WINDOW of `get_page_idx`.
  ///
  /// COMPLEXITY NOTE (accepted for Phase 0): this re-scans `events` per get_page —
  /// O(events × get_pages) worst case. At Phase-0 scale (~118 calls across 40 files) this is
  /// microseconds; not worth an index structure.
  fn label_recent(
      mined: &mut [MinedQuery],
      events: &[Event],
      get_page_idx: usize,
      session: &str,
      slug: &str,
  ) {
      let mut query_event_indices: Vec<usize> = Vec::new();
      for (idx, ev) in events.iter().enumerate() {
          if ev.session == session
              && QUERY_TOOLS.contains(&ev.name.as_str())
              && ev.input.get("query").and_then(|v| v.as_str()).is_some()
          {
              query_event_indices.push(idx);
          }
      }
      let mut session_positions: Vec<usize> = Vec::new();
      for (pos, q) in mined.iter().enumerate() {
          if q.session == session {
              session_positions.push(pos);
          }
      }
      // Same order, same count: the k-th mined query of a session IS its k-th query event.
      for (&ev_idx, &mined_pos) in query_event_indices.iter().zip(session_positions.iter()).rev() {
          if ev_idx < get_page_idx
              && get_page_idx - ev_idx <= LABEL_WINDOW
              && mined[mined_pos].gold_page_id.is_none()
          {
              mined[mined_pos].gold_page_id =
                  Some(crate::corpus::page_id_from_gbrain_slug(slug));
              return;
          }
      }
  }

  /// Exact-text dedup keeping the FIRST occurrence; a later duplicate's label fills an
  /// unlabeled first.
  fn dedup_exact(mined: Vec<MinedQuery>) -> Vec<MinedQuery> {
      use std::collections::HashMap;
      let mut order: Vec<String> = Vec::new();
      let mut by_text: HashMap<String, MinedQuery> = HashMap::new();
      for q in mined {
          match by_text.get_mut(&q.text) {
              Some(existing) => {
                  if existing.gold_page_id.is_none() {
                      existing.gold_page_id = q.gold_page_id;
                  }
              }
              None => {
                  order.push(q.text.clone());
                  by_text.insert(q.text.clone(), q);
              }
          }
      }
      order.into_iter().filter_map(|t| by_text.remove(&t)).collect()
  }
  ```
  > **Implementer note:** the fixture assumes keys `name`/`input.query`/`input.slug`/`session`. If the REAL transcript JSONL nests tool calls differently (e.g. under `message.content[]`), adjust `Event`'s serde AND the fixture in the SAME commit, noting it in memharness-probes.md. Near-duplicate dedup is deferred: exact-only ships, and the report carries `near_dedup_applied: false` as an explicit caveat (Task 40) — flagged, not faked.
- [ ] Run: `cargo test -p memharness mine` → **PASS** (2 tests).
- [ ] Commit:
  ```
  git add crates/memharness/src/mine.rs crates/memharness/tests/fixtures/transcript_synthetic.jsonl crates/memharness/src/lib.rs
  git commit -m "$(cat <<'EOF'
feat(memharness): mine — real queries + within-5 labels + exact dedup (single + cross-file)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

---

## Task 23 — `stats.rs`: success@k + MRR (RED)

- [ ] Create `crates/memharness/src/stats.rs` with the failing tests:
  ```rust
  //! Pure scoring + statistics: success@k, MRR, bootstrap CIs, Wilcoxon signed-rank. No numeric
  //! deps — hand-rolled + unit-tested against independently computed reference values.

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn success_at_k_and_mrr_known_values() {
          let ranks = vec![Some(0usize)];
          assert!(success_at_k(&ranks[0], 5));
          assert!((mrr_of(&ranks[0]) - 1.0).abs() < 1e-9);
          let r = Some(2usize);
          assert!(success_at_k(&r, 5));
          assert!(!success_at_k(&r, 2));
          assert!((mrr_of(&r) - (1.0 / 3.0)).abs() < 1e-9);
          let none: Option<usize> = None;
          assert!(!success_at_k(&none, 10));
          assert!((mrr_of(&none) - 0.0).abs() < 1e-9);
      }

      #[test]
      fn mean_success_at_k_over_many() {
          let ranks = vec![Some(0), Some(4), None, Some(1)];
          assert!((mean_success_at_k(&ranks, 5) - 0.75).abs() < 1e-9);
      }
  }
  ```
- [ ] Add `pub mod stats;` to `lib.rs`.
- [ ] Run: `cargo test -p memharness stats` → **FAIL**.

## Task 24 — `stats.rs`: implement success@k + MRR (GREEN)

- [ ] Add above the test module:
  ```rust
  /// The 0-based rank of the gold page in the (page-deduped) retrieved list; `None` = missed.
  pub type GoldRank = Option<usize>;

  /// success@k: gold at 0-based rank < k. NOTE: k here is the SAME `--k` used for retrieval
  /// (retrieval-k == scoring-k, spec §4 Rev 2 — one knob).
  pub fn success_at_k(rank: &GoldRank, k: usize) -> bool {
      matches!(rank, Some(r) if *r < k)
  }

  /// Reciprocal rank: 1/(rank+1), or 0 if missed.
  pub fn mrr_of(rank: &GoldRank) -> f64 {
      match rank {
          Some(r) => 1.0 / (*r as f64 + 1.0),
          None => 0.0,
      }
  }

  /// Mean success@k over many queries.
  pub fn mean_success_at_k(ranks: &[GoldRank], k: usize) -> f64 {
      if ranks.is_empty() {
          return 0.0;
      }
      ranks.iter().filter(|r| success_at_k(r, k)).count() as f64 / ranks.len() as f64
  }

  /// Mean reciprocal rank over many queries.
  pub fn mean_reciprocal_rank(ranks: &[GoldRank]) -> f64 {
      if ranks.is_empty() {
          return 0.0;
      }
      ranks.iter().map(mrr_of).sum::<f64>() / ranks.len() as f64
  }
  ```
- [ ] Run: `cargo test -p memharness stats` → **PASS** (2 tests).
- [ ] Commit:
  ```
  git add crates/memharness/src/stats.rs crates/memharness/src/lib.rs
  git commit -m "$(cat <<'EOF'
feat(memharness): stats — success@k + MRR (retrieval-k == scoring-k)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

## Task 25 — `stats.rs`: seeded bootstrap CI (RED)

- [ ] Add to the test module:
  ```rust
  #[test]
  fn bootstrap_ci_is_deterministic_and_brackets_mean() {
      use rand::SeedableRng;
      use rand_chacha::ChaCha8Rng;
      let data: Vec<f64> = vec![0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0];
      let mut rng1 = ChaCha8Rng::seed_from_u64(42);
      let mut rng2 = ChaCha8Rng::seed_from_u64(42);
      let ci_a = bootstrap_ci_mean(&data, 1000, 0.95, &mut rng1);
      let ci_b = bootstrap_ci_mean(&data, 1000, 0.95, &mut rng2);
      assert_eq!(ci_a, ci_b, "same seed → identical CI");
      assert!(ci_a.0 <= 0.5 && 0.5 <= ci_a.1, "CI {ci_a:?} brackets the true mean 0.5");
      assert_eq!(bootstrap_ci_mean(&[], 1000, 0.95, &mut rng1), (0.0, 0.0), "empty → (0,0)");
  }
  ```
- [ ] Run: `cargo test -p memharness stats::tests::bootstrap` → **FAIL**.

## Task 26 — `stats.rs`: implement `bootstrap_ci_mean` (GREEN)

- [ ] Add above the test module:
  ```rust
  use rand::Rng;

  /// Percentile bootstrap CI for the mean at confidence `conf`, `iters` resamples from a SEEDED
  /// rng (determinism, spec §8). Empty data → (0.0, 0.0).
  pub fn bootstrap_ci_mean<R: Rng>(
      data: &[f64],
      iters: usize,
      conf: f64,
      rng: &mut R,
  ) -> (f64, f64) {
      if data.is_empty() || iters == 0 {
          return (0.0, 0.0);
      }
      let n = data.len();
      let mut means: Vec<f64> = Vec::with_capacity(iters);
      for _ in 0..iters {
          let mut sum = 0.0;
          for _ in 0..n {
              sum += data[rng.gen_range(0..n)]; // resample WITH replacement
          }
          means.push(sum / n as f64);
      }
      means.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
      let alpha = (1.0 - conf) / 2.0;
      let low_idx = (alpha * iters as f64).floor() as usize;
      let high_idx = (((1.0 - alpha) * iters as f64).ceil() as usize).saturating_sub(1);
      (means[low_idx.min(iters - 1)], means[high_idx.min(iters - 1)])
  }
  ```
- [ ] Run: `cargo test -p memharness stats::tests::bootstrap` → **PASS**.
- [ ] Commit:
  ```
  git add crates/memharness/src/stats.rs
  git commit -m "$(cat <<'EOF'
feat(memharness): stats — seeded percentile bootstrap CI

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

## Task 27 — `stats.rs`: Wilcoxon signed-rank incl. tie-heavy fixtures (RED)

**(Rev 2, finding 5: tie-heavy binary-flag fixtures with independently computed reference values + a normal-CDF sanity point.)**

- [ ] Add to the test module:
  ```rust
  #[test]
  fn wilcoxon_signed_rank_known_values() {
      // Clean separation: differences [1,2,3,4,5] all positive → W = min(15, 0) = 0.
      let air = vec![2.0, 3.0, 4.0, 5.0, 6.0];
      let gbrain = vec![1.0, 1.0, 1.0, 1.0, 1.0];
      let res = wilcoxon_signed_rank(&air, &gbrain);
      assert_eq!(res.n_nonzero, 5);
      assert!((res.w_statistic - 0.0).abs() < 1e-9);
      assert!(res.p_value < 0.1, "p={}", res.p_value);

      // Identical vectors → all zero diffs dropped → n 0, p 1.
      let same = vec![1.0, 2.0, 3.0];
      let res0 = wilcoxon_signed_rank(&same, &same);
      assert_eq!(res0.n_nonzero, 0);
      assert!((res0.p_value - 1.0).abs() < 1e-9);
  }

  #[test]
  fn wilcoxon_tie_heavy_binary_flags() {
      // KNOWN-ITEM reality (Rev 2): paired binary success flags — every non-zero diff is ±1,
      // one big tie group. Reference values computed INDEPENDENTLY in Python (exact math.erfc
      // normal CDF; same tie-corrected variance + continuity correction; matches the
      // scipy.stats.wilcoxon(zero_method='wilcox', correction=True, method='approx') convention).
      //
      // A: five +1 diffs, five zeros → n=5, ranks all tie at 3.0, W=min(15,0)=0,
      //    var = (5·6·11 − 120/2)/24 = 11.25, z = |0−7.5+0.5|/√11.25 = 2.08700, p ≈ 0.036888.
      let air = vec![1.0, 1.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0, 1.0, 1.0];
      let gbrain = vec![0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0];
      let res = wilcoxon_signed_rank(&air, &gbrain);
      assert_eq!(res.n_nonzero, 5);
      assert!((res.w_statistic - 0.0).abs() < 1e-9);
      assert!((res.p_value - 0.036888).abs() < 1e-4, "p={}", res.p_value);
      assert!(res.small_n_approx, "n=5 < 25 → approximation flagged");

      // B: mixed signs, heavy ties — five +1, three −1, two zeros → n=8, ranks all 4.5,
      //    W = min(22.5, 13.5) = 13.5, var = (8·9·17 − 504/2)/24 = 40.5,
      //    z = |13.5−18+0.5|/√40.5 = 0.62854, p ≈ 0.529651.
      let air_b = vec![1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 0.0];
      let gbrain_b = vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 0.0, 1.0];
      let res_b = wilcoxon_signed_rank(&air_b, &gbrain_b);
      assert_eq!(res_b.n_nonzero, 8);
      assert!((res_b.w_statistic - 13.5).abs() < 1e-9);
      assert!((res_b.p_value - 0.529651).abs() < 1e-4, "p={}", res_b.p_value);
  }

  #[test]
  fn normal_p_sanity_at_the_5_percent_point() {
      // The classic two-sided 5% z: p(1.959964) must be ≈ 0.05 (Zelen–Severo ≈ exact to ~1e-7).
      assert!((two_sided_normal_p(1.959964) - 0.05).abs() < 1e-4);
  }
  ```
- [ ] Run: `cargo test -p memharness stats::tests::wilcoxon stats::tests::normal_p` → **FAIL**.

## Task 28 — `stats.rs`: implement Wilcoxon signed-rank (GREEN)

- [ ] Add above the test module:
  ```rust
  /// The result of a Wilcoxon signed-rank test on paired samples.
  #[derive(Debug, Clone, PartialEq, serde::Serialize)]
  pub struct WilcoxonResult {
      /// Non-zero pairwise differences (zeros dropped — the standard 'wilcox' convention).
      pub n_nonzero: usize,
      /// W = min(sum of positive ranks, sum of negative ranks).
      pub w_statistic: f64,
      /// Two-sided p via the tie-corrected normal approximation with continuity correction.
      /// n_nonzero == 0 → 1.0 (no evidence of a difference).
      pub p_value: f64,
      /// True when n_nonzero < 25: the normal approximation is unreliable there. Phase 0 REPORTS
      /// this flag (spec §5 honesty) instead of shipping the exact table. On binary-flag data
      /// (known-item segments) the test is additionally tie-heavy — the report carries that
      /// caveat too (spec §8 Rev 2).
      pub small_n_approx: bool,
  }

  /// Wilcoxon signed-rank on paired `(air[i], gbrain[i])`: rank |non-zero diffs| (average ranks
  /// for ties), sum ranks by sign, two-sided p via the normal approximation with tie correction
  /// (−Σ(t³−t)/2 in the variance) + continuity correction.
  pub fn wilcoxon_signed_rank(air: &[f64], gbrain: &[f64]) -> WilcoxonResult {
      assert_eq!(air.len(), gbrain.len(), "paired samples must be equal length");
      let diffs: Vec<f64> = air
          .iter()
          .zip(gbrain.iter())
          .map(|(a, g)| a - g)
          .filter(|d| *d != 0.0)
          .collect();
      let n = diffs.len();
      if n == 0 {
          return WilcoxonResult { n_nonzero: 0, w_statistic: 0.0, p_value: 1.0, small_n_approx: true };
      }
      let mut idx: Vec<usize> = (0..n).collect();
      idx.sort_by(|&i, &j| {
          diffs[i].abs().partial_cmp(&diffs[j].abs()).unwrap_or(std::cmp::Ordering::Equal)
      });
      let mut ranks = vec![0.0f64; n];
      let mut tie_term = 0.0f64;
      let mut i = 0;
      while i < n {
          let mut j = i;
          while j + 1 < n && diffs[idx[j + 1]].abs() == diffs[idx[i]].abs() {
              j += 1;
          }
          let group_len = j - i + 1;
          let avg_rank = ((i + 1) as f64 + (j + 1) as f64) / 2.0;
          for k in i..=j {
              ranks[idx[k]] = avg_rank;
          }
          if group_len > 1 {
              let t = group_len as f64;
              tie_term += t * t * t - t;
          }
          i = j + 1;
      }
      let mut sum_pos = 0.0f64;
      let mut sum_neg = 0.0f64;
      for k in 0..n {
          if diffs[k] > 0.0 {
              sum_pos += ranks[k];
          } else {
              sum_neg += ranks[k];
          }
      }
      let w = sum_pos.min(sum_neg);
      let nf = n as f64;
      let mean_w = nf * (nf + 1.0) / 4.0;
      let var_w = (nf * (nf + 1.0) * (2.0 * nf + 1.0) - tie_term / 2.0) / 24.0;
      let p_value = if var_w <= 0.0 {
          1.0
      } else {
          let z = (w - mean_w + 0.5).abs() / var_w.sqrt();
          two_sided_normal_p(z)
      };
      WilcoxonResult { n_nonzero: n, w_statistic: w, p_value, small_n_approx: n < 25 }
  }

  /// Two-sided p for |z| under the standard normal — Zelen & Severo (1964) tail approximation
  /// (~1e-7 accurate; verified against exact erfc at z ∈ {0.6285, 1.959964, 2.087}).
  fn two_sided_normal_p(z: f64) -> f64 {
      let z = z.abs();
      let t = 1.0 / (1.0 + 0.2316419 * z);
      let d = 0.398942280401433 * (-z * z / 2.0).exp();
      let upper_tail = d
          * t
          * (0.319381530
              + t * (-0.356563782 + t * (1.781477937 + t * (-1.821255978 + t * 1.330274429))));
      (2.0 * upper_tail).min(1.0)
  }
  ```
- [ ] Run: `cargo test -p memharness stats` → **PASS** (6 tests).
- [ ] Commit:
  ```
  git add crates/memharness/src/stats.rs
  git commit -m "$(cat <<'EOF'
feat(memharness): stats — Wilcoxon signed-rank, tie-heavy binary fixtures with verified reference values

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

---

## Task 29 — `judge.rs`: Verdict, kappa, agreement, trust verdict (RED)

- [ ] Create `crates/memharness/src/judge.rs` with the failing tests:
  ```rust
  //! The blind pairwise judging layer (spec §5): verdicts, Cohen's kappa, the trust verdict,
  //! blind A/B assignment, position-swap resolution, the shared `PairJudge` trait (the LOCAL
  //! judge AND the CLOUD auditor are both blind position-swapped pickers), and the audit-sample
  //! selection (`max(30, 15%)` ∪ uncertains, Rev 2).

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn cohens_kappa_known_values() {
          let a = vec![Verdict::AirWins, Verdict::GbrainWins, Verdict::Tie, Verdict::AirWins];
          let b = a.clone();
          assert!((cohens_kappa(&a, &b) - 1.0).abs() < 1e-9, "perfect agreement → 1");
          let x = vec![Verdict::AirWins; 4];
          let y = vec![Verdict::GbrainWins; 4];
          assert!(cohens_kappa(&x, &y) <= 0.0, "total disagreement → ≤ 0");
          assert!((cohens_kappa(&[], &[]) - 0.0).abs() < 1e-9, "empty → 0, no panic");
      }

      #[test]
      fn raw_agreement_fraction() {
          let a = vec![Verdict::AirWins, Verdict::GbrainWins, Verdict::Tie];
          let b = vec![Verdict::AirWins, Verdict::AirWins, Verdict::Tie];
          assert!((raw_agreement(&a, &b) - (2.0 / 3.0)).abs() < 1e-9);
      }

      #[test]
      fn trust_verdict_thresholds_and_flags() {
          // 9/10 agree, decisive both sides → trusted.
          let mut local = vec![Verdict::AirWins; 5];
          local.extend(vec![Verdict::GbrainWins; 5]);
          let mut cloud = local.clone();
          cloud[9] = Verdict::AirWins; // one disagreement → 90% agreement
          let t = trust_verdict(&local, &cloud, false, false, false);
          assert!(t.trusted, "agreement {} kappa {}", t.agreement, t.kappa);
          assert!(!t.audit_incomplete && !t.audit_n_too_small);

          // Audit incomplete → NOT trusted, flagged, never fabricated.
          let t = trust_verdict(&[], &[], false, true, false);
          assert!(!t.trusted && t.audit_incomplete);

          // Rev 2: open pool < AUDIT_FLOOR → indicative-only flag carried through.
          let t = trust_verdict(&local, &cloud, false, false, true);
          assert!(t.audit_n_too_small);
      }
  }
  ```
- [ ] Add `pub mod judge;` to `lib.rs`.
- [ ] Run: `cargo test -p memharness judge` → **FAIL**.

## Task 30 — `judge.rs`: implement Verdict/kappa/agreement/trust (GREEN)

- [ ] Add above the test module:
  ```rust
  use serde::Serialize;

  /// A pairwise judgment outcome, de-blinded to arm identity.
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
  pub enum Verdict {
      AirWins,
      GbrainWins,
      Tie,
      /// The two position-swapped judgments disagreed, or a reply was ambiguous → uncertain
      /// (always audited; never dropped).
      Uncertain,
  }

  /// Kappa category (Uncertain folds into Tie in the 3×3 table — an uncertain call compared to a
  /// decisive one is a non-decision, counted as disagreement with any decisive pick).
  fn category(v: Verdict) -> usize {
      match v {
          Verdict::AirWins => 0,
          Verdict::GbrainWins => 1,
          Verdict::Tie | Verdict::Uncertain => 2,
      }
  }

  /// Raw agreement fraction between equal-length verdict vectors.
  pub fn raw_agreement(a: &[Verdict], b: &[Verdict]) -> f64 {
      if a.is_empty() || a.len() != b.len() {
          return 0.0;
      }
      let agree = a.iter().zip(b).filter(|(x, y)| category(**x) == category(**y)).count();
      agree as f64 / a.len() as f64
  }

  /// Cohen's kappa over {AirWins, GbrainWins, Tie/Uncertain}. Empty/mismatched → 0.
  pub fn cohens_kappa(a: &[Verdict], b: &[Verdict]) -> f64 {
      let n = a.len();
      if n == 0 || n != b.len() {
          return 0.0;
      }
      let p_o = raw_agreement(a, b);
      let mut ca = [0.0f64; 3];
      let mut cb = [0.0f64; 3];
      for i in 0..n {
          ca[category(a[i])] += 1.0;
          cb[category(b[i])] += 1.0;
      }
      let nf = n as f64;
      let p_e: f64 = (0..3).map(|c| (ca[c] / nf) * (cb[c] / nf)).sum();
      if (1.0 - p_e).abs() < 1e-12 {
          return if (p_o - 1.0).abs() < 1e-12 { 1.0 } else { 0.0 };
      }
      (p_o - p_e) / (1.0 - p_e)
  }

  /// Trust thresholds (spec §5): trusted iff agreement ≥ 0.85 AND kappa ≥ 0.6.
  pub const TRUST_AGREEMENT_MIN: f64 = 0.85;
  pub const TRUST_KAPPA_MIN: f64 = 0.6;

  /// The judge-trust verdict the report LEADS with.
  #[derive(Debug, Clone, Serialize)]
  pub struct TrustVerdict {
      pub audited_count: usize,
      pub agreement: f64,
      pub kappa: f64,
      pub trusted: bool,
      /// The run auto-expanded the audit to 100% because trust failed (spec §5).
      pub expanded_to_full_audit: bool,
      /// The cloud audit could not complete (API failure / --local-only) → verdict UNAVAILABLE,
      /// never fabricated.
      pub audit_incomplete: bool,
      /// Rev 2: the open-query pool was smaller than AUDIT_FLOOR → "audit n too small; trust
      /// verdict indicative only" in the report.
      pub audit_n_too_small: bool,
  }

  /// Compute the trust verdict from paired local-vs-cloud verdicts on the AUDITED set.
  pub fn trust_verdict(
      local: &[Verdict],
      cloud: &[Verdict],
      expanded_to_full_audit: bool,
      audit_incomplete: bool,
      audit_n_too_small: bool,
  ) -> TrustVerdict {
      if audit_incomplete || local.is_empty() {
          return TrustVerdict {
              audited_count: local.len(),
              agreement: 0.0,
              kappa: 0.0,
              trusted: false,
              expanded_to_full_audit,
              audit_incomplete: true,
              audit_n_too_small,
          };
      }
      let agreement = raw_agreement(local, cloud);
      let kappa = cohens_kappa(local, cloud);
      TrustVerdict {
          audited_count: local.len(),
          agreement,
          kappa,
          trusted: agreement >= TRUST_AGREEMENT_MIN && kappa >= TRUST_KAPPA_MIN,
          expanded_to_full_audit,
          audit_incomplete: false,
          audit_n_too_small,
      }
  }
  ```
- [ ] Run: `cargo test -p memharness judge` → **PASS** (3 tests).
- [ ] Commit:
  ```
  git add crates/memharness/src/judge.rs crates/memharness/src/lib.rs
  git commit -m "$(cat <<'EOF'
feat(memharness): judge — Verdict, kappa, agreement, trust verdict (incomplete + n-too-small flags)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

## Task 31 — `judge.rs`: PosPick, one-token parsing, blinding, swap, PairJudge, audit selection (RED)

**(Rev 2, findings 4 + 12.)**

- [ ] Add to the test module:
  ```rust
  #[test]
  fn parse_pick_token_exact_then_tokenized_fallback() {
      // Exact one-token replies (the prompt DEMANDS these) parse first.
      assert_eq!(parse_pick_token("A"), Some(PosPick::A));
      assert_eq!(parse_pick_token(" b\n"), Some(PosPick::B));
      assert_eq!(parse_pick_token("TIE"), Some(PosPick::Tie));
      assert_eq!(parse_pick_token("tie"), Some(PosPick::Tie));
      // Tokenized fallback ONLY when exactly one signal token appears.
      assert_eq!(parse_pick_token("Answer: B"), Some(PosPick::B));
      assert_eq!(parse_pick_token("The answer is A."), Some(PosPick::A));
      // Ambiguous → None (recorded Uncertain — never dropped, never fabricated).
      assert_eq!(parse_pick_token("A or B, hard to say"), None);
      assert_eq!(parse_pick_token("I cannot decide"), None);
      // "a" as an article + "tie" → two signals → ambiguous (safe: goes to audit as Uncertain).
      assert_eq!(parse_pick_token("It's a tie between them"), None);
  }

  #[test]
  fn blind_assignment_is_seeded_and_deblinding_is_correct() {
      use rand::SeedableRng;
      use rand_chacha::ChaCha8Rng;
      let mut rng1 = ChaCha8Rng::seed_from_u64(42);
      let mut rng2 = ChaCha8Rng::seed_from_u64(42);
      assert_eq!(assign_blind(&mut rng1).air_is_a, assign_blind(&mut rng2).air_is_a);
      let air_a = Blind { air_is_a: true };
      // Unswapped: A holds AIR.
      assert_eq!(deblind_pick(air_a, PosPick::A, false), Verdict::AirWins);
      assert_eq!(deblind_pick(air_a, PosPick::B, false), Verdict::GbrainWins);
      // Swapped: positions exchanged.
      assert_eq!(deblind_pick(air_a, PosPick::A, true), Verdict::GbrainWins);
      assert_eq!(deblind_pick(air_a, PosPick::Tie, true), Verdict::Tie);
      let air_b = Blind { air_is_a: false };
      assert_eq!(deblind_pick(air_b, PosPick::A, false), Verdict::GbrainWins);
      assert_eq!(deblind_pick(air_b, PosPick::A, true), Verdict::AirWins);
  }

  #[test]
  fn resolve_swap_and_judge_pair_blind() {
      assert_eq!(resolve_swap(Verdict::AirWins, Verdict::AirWins), Verdict::AirWins);
      assert_eq!(resolve_swap(Verdict::AirWins, Verdict::GbrainWins), Verdict::Uncertain);
      assert_eq!(resolve_swap(Verdict::Tie, Verdict::Tie), Verdict::Tie);

      /// A content-based double: prefers the answer containing "GOOD" (blind-compatible — it
      /// judges CONTENT, not position, so it survives blinding and swapping).
      struct GoodJudge;
      impl PairJudge for GoodJudge {
          fn pick(&self, _q: &str, a: &str, b: &str) -> anyhow::Result<Option<PosPick>> {
              Ok(match (a.contains("GOOD"), b.contains("GOOD")) {
                  (true, false) => Some(PosPick::A),
                  (false, true) => Some(PosPick::B),
                  _ => Some(PosPick::Tie),
              })
          }
      }
      /// An ambiguity double: always returns None.
      struct MumbleJudge;
      impl PairJudge for MumbleJudge {
          fn pick(&self, _q: &str, _a: &str, _b: &str) -> anyhow::Result<Option<PosPick>> {
              Ok(None)
          }
      }

      for air_is_a in [true, false] {
          let blind = Blind { air_is_a };
          // AIR's answer holds GOOD → AirWins regardless of the blind assignment.
          let v = judge_pair_blind(&GoodJudge, blind, "q", "GOOD air", "meh gbrain").unwrap();
          assert_eq!(v, Verdict::AirWins, "air_is_a={air_is_a}");
          // GBrain's answer holds GOOD → GbrainWins regardless.
          let v = judge_pair_blind(&GoodJudge, blind, "q", "meh air", "GOOD gbrain").unwrap();
          assert_eq!(v, Verdict::GbrainWins, "air_is_a={air_is_a}");
      }
      // Ambiguous reply on either call → Uncertain.
      let v = judge_pair_blind(&MumbleJudge, Blind { air_is_a: true }, "q", "x", "y").unwrap();
      assert_eq!(v, Verdict::Uncertain);
  }

  #[test]
  fn audit_selection_floor_union_uncertains_seeded() {
      use rand::SeedableRng;
      use rand_chacha::ChaCha8Rng;
      // 200 open queries → target max(30, ceil(30)) = 30 random ∪ uncertains {5, 199}.
      let mut rng = ChaCha8Rng::seed_from_u64(42);
      let sel = select_audit_indices(200, &[5, 199], &mut rng);
      assert!(sel.len() >= 30 && sel.len() <= 32, "30 random ∪ 2 uncertains, deduped: {}", sel.len());
      assert!(sel.contains(&5) && sel.contains(&199), "ALL uncertains included");
      // Determinism.
      let mut rng2 = ChaCha8Rng::seed_from_u64(42);
      assert_eq!(sel, select_audit_indices(200, &[5, 199], &mut rng2));
      // A pool smaller than the floor → the WHOLE pool.
      let mut rng3 = ChaCha8Rng::seed_from_u64(42);
      let all = select_audit_indices(10, &[], &mut rng3);
      assert_eq!(all.len(), 10);
  }
  ```
- [ ] Run: `cargo test -p memharness judge` → **FAIL** (new symbols undefined).

## Task 32 — `judge.rs`: implement picking/blinding/selection (GREEN)

- [ ] Add above the test module:
  ```rust
  use rand::seq::SliceRandom;
  use rand::Rng;
  use std::collections::BTreeSet;

  /// A position-level pick (what a blind judge names): A, B, or TIE.
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum PosPick {
      A,
      B,
      Tie,
  }

  /// The pairwise-judging seam: ONE trait serves both the LOCAL judge (Ollama) and the CLOUD
  /// auditor (Anthropic) — both are blind, position-swapped pickers. `Ok(None)` = the reply was
  /// ambiguous → the caller records `Uncertain` (never dropped, never fabricated).
  pub trait PairJudge {
      fn pick(&self, query: &str, answer_a: &str, answer_b: &str) -> anyhow::Result<Option<PosPick>>;
  }

  /// The shared judging prompt: both the local judge and the cloud auditor use IT (identical
  /// instructions; only the model differs), constrained to EXACTLY one output token.
  pub fn pairwise_prompt(query: &str, answer_a: &str, answer_b: &str) -> String {
      format!(
          "You are comparing two answers to the same question.\n\nQuestion: {query}\n\n\
           Answer A:\n{answer_a}\n\nAnswer B:\n{answer_b}\n\n\
           Which answer is better (more correct, more complete, better grounded)? \
           Reply with exactly one token: A, B, or TIE."
      )
  }

  /// Parse a one-token judge reply. Exact match first (trimmed, case-folded); tokenized-substring
  /// fallback ONLY if exactly one signal token appears among {A, B, TIE}. Anything else → None
  /// (→ `Uncertain`).
  pub fn parse_pick_token(text: &str) -> Option<PosPick> {
      let t = text.trim().to_uppercase();
      match t.as_str() {
          "A" => return Some(PosPick::A),
          "B" => return Some(PosPick::B),
          "TIE" => return Some(PosPick::Tie),
          _ => {}
      }
      let tokens: BTreeSet<&str> = t
          .split(|c: char| !c.is_ascii_alphanumeric())
          .filter(|s| !s.is_empty())
          .collect();
      let mut found: Vec<PosPick> = Vec::new();
      if tokens.contains("A") {
          found.push(PosPick::A);
      }
      if tokens.contains("B") {
          found.push(PosPick::B);
      }
      if tokens.contains("TIE") {
          found.push(PosPick::Tie);
      }
      if found.len() == 1 { Some(found[0]) } else { None }
  }

  /// The local judge: the SAME Ollama model as the answerer, behind the shared trait.
  pub struct OllamaJudge {
      pub model: String,
  }

  impl PairJudge for OllamaJudge {
      fn pick(&self, query: &str, answer_a: &str, answer_b: &str) -> anyhow::Result<Option<PosPick>> {
          let reply = crate::ollama::generate(&self.model, &pairwise_prompt(query, answer_a, answer_b))?;
          Ok(parse_pick_token(&reply))
      }
  }

  /// Per-pair blind assignment: whether AIR's answer is shown as "A". The judge NEVER sees arm
  /// names — only positions (spec §5 blinding).
  #[derive(Debug, Clone, Copy)]
  pub struct Blind {
      pub air_is_a: bool,
  }

  /// Seeded per-pair coin flip.
  pub fn assign_blind<R: Rng>(rng: &mut R) -> Blind {
      Blind { air_is_a: rng.gen_bool(0.5) }
  }

  /// De-blind a position pick to arm identity. `swapped` = this pick came from the SECOND
  /// (position-swapped) judging call, where A shows what was B.
  pub fn deblind_pick(blind: Blind, pick: PosPick, swapped: bool) -> Verdict {
      let air_is_a = if swapped { !blind.air_is_a } else { blind.air_is_a };
      match pick {
          PosPick::Tie => Verdict::Tie,
          PosPick::A => {
              if air_is_a { Verdict::AirWins } else { Verdict::GbrainWins }
          }
          PosPick::B => {
              if air_is_a { Verdict::GbrainWins } else { Verdict::AirWins }
          }
      }
  }

  /// Resolve the two de-blinded, position-swapped judgments: agree → that verdict; disagree →
  /// `Uncertain` (spec §5).
  pub fn resolve_swap(first: Verdict, swapped: Verdict) -> Verdict {
      if first == swapped { first } else { Verdict::Uncertain }
  }

  /// Judge one pair blind + position-swapped: two `pick` calls (assigned order, then swapped),
  /// each de-blinded; any ambiguous reply OR ordering disagreement → `Uncertain`. Used for BOTH
  /// the local judge and the cloud auditor (identical protocol; only the model differs).
  pub fn judge_pair_blind(
      judge: &dyn PairJudge,
      blind: Blind,
      query: &str,
      air_answer: &str,
      gbrain_answer: &str,
  ) -> anyhow::Result<Verdict> {
      let (first_a, first_b) = if blind.air_is_a {
          (air_answer, gbrain_answer)
      } else {
          (gbrain_answer, air_answer)
      };
      let Some(p1) = judge.pick(query, first_a, first_b)? else {
          return Ok(Verdict::Uncertain);
      };
      let Some(p2) = judge.pick(query, first_b, first_a)? else {
          return Ok(Verdict::Uncertain);
      };
      Ok(resolve_swap(deblind_pick(blind, p1, false), deblind_pick(blind, p2, true)))
  }

  /// Audit-sample floor + fraction (spec §5 Rev 2).
  pub const AUDIT_FLOOR: usize = 30;
  pub const AUDIT_FRACTION: f64 = 0.15;

  /// Seeded audit selection: a random `min(open_count, max(AUDIT_FLOOR, ceil(15% of open)))`
  /// sample ∪ ALL uncertain indices (deduped union). A pool smaller than the floor is audited
  /// in full (and the caller sets `audit_n_too_small`).
  pub fn select_audit_indices<R: Rng>(
      open_count: usize,
      uncertain: &[usize],
      rng: &mut R,
  ) -> BTreeSet<usize> {
      let target = AUDIT_FLOOR
          .max((open_count as f64 * AUDIT_FRACTION).ceil() as usize)
          .min(open_count);
      let mut all: Vec<usize> = (0..open_count).collect();
      all.shuffle(rng);
      let mut set: BTreeSet<usize> = all.into_iter().take(target).collect();
      set.extend(uncertain.iter().copied().filter(|i| *i < open_count));
      set
  }
  ```
- [ ] Run: `cargo test -p memharness judge` → **PASS** (7 tests).
- [ ] Commit:
  ```
  git add crates/memharness/src/judge.rs
  git commit -m "$(cat <<'EOF'
feat(memharness): judge — one-token parsing, seeded blinding, swap protocol, PairJudge seam, audit floor selection

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

---

## Task 33 — `anthropic.rs`: strict extract + preflight + auditor (RED)

- [ ] Create `crates/memharness/src/anthropic.rs` with the failing tests:
  ```rust
  //! The cloud audit: a minimal Anthropic Messages API POST via `ureq`, key from
  //! `ANTHROPIC_API_KEY` env (read by main.rs). The auditor implements the SAME `PairJudge`
  //! trait as the local judge — blind, position-swapped, one-token replies. Strict parse; any
  //! failure degrades to "audit incomplete — trust verdict unavailable" (never fabricates).
  //! A one-token PREFLIGHT runs before the expensive loop (fail fast, spec §5 Rev 2).

  /// The pinned audit model (a current Sonnet-tier id — confirmed/adjusted by Probe D).
  pub const AUDIT_MODEL: &str = "claude-sonnet-5";

  #[cfg(test)]
  mod tests {
      use super::*;
      use crate::judge::PosPick;

      #[test]
      fn extracts_first_text_block() {
          let body = r#"{"content":[{"type":"text","text":"A"}]}"#;
          assert_eq!(extract_text(body).unwrap(), "A");
          assert!(extract_text(r#"{"content":[]}"#).is_err(), "no text block = error");
          assert!(extract_text("not json").is_err());
      }

      #[test]
      fn audit_reply_parses_via_the_shared_one_token_rule() {
          // The SAME parse_pick_token as the local judge (finding 12): exact first, tokenized
          // fallback, ambiguous → None (→ Uncertain — not dropped).
          assert_eq!(crate::judge::parse_pick_token("B"), Some(PosPick::B));
          assert_eq!(crate::judge::parse_pick_token("Answer: TIE"), Some(PosPick::Tie));
          assert_eq!(crate::judge::parse_pick_token("I cannot decide"), None);
      }
  }
  ```
- [ ] Add `pub mod anthropic;` to `lib.rs`.
- [ ] Run: `cargo test -p memharness anthropic` → **FAIL** (`extract_text` undefined).

## Task 34 — `anthropic.rs`: implement POST + preflight + `AnthropicAuditor` (GREEN)

- [ ] Add above the test module:
  ```rust
  use serde::Deserialize;

  use crate::judge::{pairwise_prompt, parse_pick_token, PairJudge, PosPick};

  const ANTHROPIC_URL: &str = "https://api.anthropic.com/v1/messages";
  const ANTHROPIC_VERSION: &str = "2023-06-01";

  #[derive(Deserialize)]
  struct MessagesBody {
      content: Vec<ContentBlock>,
  }
  #[derive(Deserialize)]
  struct ContentBlock {
      #[serde(default, rename = "type")]
      block_type: String,
      #[serde(default)]
      text: String,
  }

  /// Extract the first text block from a Messages body (strict: none = error).
  pub fn extract_text(body: &str) -> anyhow::Result<String> {
      let parsed: MessagesBody = serde_json::from_str(body)?;
      parsed
          .content
          .iter()
          .find(|b| b.block_type == "text")
          .map(|b| b.text.clone())
          .ok_or_else(|| anyhow::anyhow!("no text block in Messages response"))
  }

  #[derive(serde::Serialize)]
  struct MessagesReq<'a> {
      model: &'a str,
      max_tokens: u32,
      messages: Vec<ReqMessage<'a>>,
  }
  #[derive(serde::Serialize)]
  struct ReqMessage<'a> {
      role: &'a str,
      content: &'a str,
  }

  /// One Messages POST → the reply text. HTTP/parse failures are errors (the caller records
  /// "audit incomplete"; a fabricated verdict is never produced).
  fn post_message(api_key: &str, prompt: &str, max_tokens: u32) -> anyhow::Result<String> {
      let body = ureq::post(ANTHROPIC_URL)
          .set("x-api-key", api_key)
          .set("anthropic-version", ANTHROPIC_VERSION)
          .set("content-type", "application/json")
          .send_json(MessagesReq {
              model: AUDIT_MODEL,
              max_tokens,
              messages: vec![ReqMessage { role: "user", content: prompt }],
          })
          .map_err(|e| anyhow::anyhow!("Anthropic Messages call failed: {e}"))?
          .into_string()?;
      extract_text(&body)
  }

  /// Fail-fast preflight (Rev 2, finding 6): ONE tiny call with the pinned model BEFORE the
  /// expensive loop — a bad key or retired model id fails in seconds, not after 2h. The reply
  /// content is irrelevant; only success matters.
  pub fn preflight(api_key: &str) -> anyhow::Result<()> {
      post_message(api_key, "Reply with exactly: OK", 4).map(|_| ()).map_err(|e| {
          anyhow::anyhow!(
              "Anthropic preflight failed ({e}). Check ANTHROPIC_API_KEY and that model \
               '{AUDIT_MODEL}' exists (update AUDIT_MODEL per Probe D if retired). \
               Or run with --local-only."
          )
      })
  }

  /// The cloud auditor: the SAME `PairJudge` protocol as the local judge (blind +
  /// position-swapped via `judge_pair_blind`; identical prompt; one-token reply parsed by the
  /// shared rule — ambiguous → None → Uncertain).
  pub struct AnthropicAuditor {
      pub api_key: String,
  }

  impl PairJudge for AnthropicAuditor {
      fn pick(&self, query: &str, answer_a: &str, answer_b: &str) -> anyhow::Result<Option<PosPick>> {
          let reply = post_message(&self.api_key, &pairwise_prompt(query, answer_a, answer_b), 8)?;
          Ok(parse_pick_token(&reply))
      }
  }
  ```
- [ ] Run: `cargo test -p memharness anthropic` → **PASS** (2 tests; POST/preflight are live-run-only).
- [ ] Commit:
  ```
  git add crates/memharness/src/anthropic.rs crates/memharness/src/lib.rs
  git commit -m "$(cat <<'EOF'
feat(memharness): anthropic — strict Messages extract, one-token audit via shared PairJudge, fail-fast preflight

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

---

## Task 35 — `arms.rs`: hits, page-dedup, k-hit packing, gbrain parser, retriever traits (RED)

**(Rev 2, findings 2 + 9.)**

- [ ] Create `crates/memharness/src/arms.rs` with the failing tests:
  ```rust
  //! The two retrieval arms + the shared answerer seam (spec §4). AIR = wire recall → PageResolver
  //! (fail-loud) → hits. GBrain = `gbrain query` subprocess, output parsed per Probe A. Both feed
  //! the SAME `Answerer` with a context budgeted by NUMBER OF HITS (k) — not chars (Rev 2: a char
  //! budget reintroduces the chunk-vs-page truncation confound). Retrieval seams are traits so the
  //! run loop tests hermetically.

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn gbrain_output_parses_to_page_ids_and_snippets() {
          // Probe-A-pinned shape (the committed fixture is authoritative; the implementer adjusts
          // parser + fixture together if the real format differs).
          let sample = "slug: air/foo\ntext: foo body snippet\n---\nslug: people/aria-novak\ntext: aria bio\n";
          let hits = parse_gbrain_output(sample).unwrap();
          assert_eq!(hits.len(), 2);
          assert_eq!(hits[0].page_id, "air/foo");
          assert_eq!(hits[0].snippet, "foo body snippet");
          assert_eq!(hits[1].page_id, "people/aria-novak");
      }

      #[test]
      fn dedup_by_page_keeps_first_occurrence() {
          // Rev 2 (finding 2): multi-hits from one page must not distort success@k/MRR — rank
          // metrics are per-PAGE; the first (best-ranked) occurrence wins.
          let hits = vec![
              RetrievedHit { page_id: "x/a".into(), snippet: "rank0".into() },
              RetrievedHit { page_id: "x/b".into(), snippet: "rank1".into() },
              RetrievedHit { page_id: "x/a".into(), snippet: "rank2-dup".into() },
          ];
          let deduped = dedup_by_page(hits);
          assert_eq!(deduped.len(), 2);
          assert_eq!(deduped[0].snippet, "rank0", "first occurrence kept");
          assert_eq!(deduped[1].page_id, "x/b");
      }

      #[test]
      fn gold_rank_on_deduped_list() {
          let hits = vec![
              RetrievedHit { page_id: "x/a".into(), snippet: "..".into() },
              RetrievedHit { page_id: "air/foo".into(), snippet: "..".into() },
          ];
          assert_eq!(gold_rank(&hits, "air/foo"), Some(1));
          assert_eq!(gold_rank(&hits, "missing"), None);
      }

      #[test]
      fn pack_context_budgets_by_hit_count_with_identical_snippet_cap() {
          // Rev 2 (finding 9): ALL hits are packed (budget = k); the ONLY truncation is the
          // per-snippet safety cap, identical for both arms, counted, char-boundary-safe.
          let long_ko = "감".repeat(PER_SNIPPET_CHAR_CAP + 10); // multibyte — must not panic
          let hits = vec![
              RetrievedHit { page_id: "a".into(), snippet: "short one".into() },
              RetrievedHit { page_id: "b".into(), snippet: long_ko },
              RetrievedHit { page_id: "c".into(), snippet: "third".into() },
          ];
          let (ctx, stats) = pack_context(&hits);
          assert_eq!(stats.sources_packed, 3, "every hit packed — no char-budget drop");
          assert_eq!(stats.snippets_truncated, 1, "only the oversized snippet was capped");
          assert!(ctx.contains("short one") && ctx.contains("third"));
          assert_eq!(
              stats.chars_packed,
              "short one".chars().count() + PER_SNIPPET_CHAR_CAP + "third".chars().count()
          );
      }
  }
  ```
- [ ] Add `pub mod arms;` to `lib.rs`.
- [ ] Run: `cargo test -p memharness arms` → **FAIL**.

## Task 36 — `arms.rs`: implement arms + traits (GREEN)

- [ ] Add above the test module:
  ```rust
  use std::path::Path;

  use crate::corpus::page_id_from_gbrain_slug;
  use crate::resolve::PageResolver;

  /// One retrieved hit, normalized to the arm-independent page-id space.
  #[derive(Debug, Clone)]
  pub struct RetrievedHit {
      pub page_id: String,
      pub snippet: String,
  }

  /// 0-based rank of `gold_page_id` in the (page-deduped) hits, or None.
  pub fn gold_rank(hits: &[RetrievedHit], gold_page_id: &str) -> Option<usize> {
      hits.iter().position(|h| h.page_id == gold_page_id)
  }

  /// Dedup by page id, FIRST occurrence (best rank) wins — applied to BOTH arms before rank
  /// scoring (Rev 2, finding 2: multi-chunk hits from one page must not distort success@k/MRR).
  pub fn dedup_by_page(hits: Vec<RetrievedHit>) -> Vec<RetrievedHit> {
      let mut seen = std::collections::HashSet::new();
      hits.into_iter().filter(|h| seen.insert(h.page_id.clone())).collect()
  }

  // ── Context packing (spec §4 Rev 2): budget = NUMBER OF HITS (k). ──

  /// The ONLY truncation: a per-snippet safety cap for the local model's context window,
  /// applied IDENTICALLY to both arms and counted. In chars (not bytes) — KO-safe.
  pub const PER_SNIPPET_CHAR_CAP: usize = 4000;

  /// Per-arm packing stats, recorded in the report (with the chunk-vs-page granularity note).
  #[derive(Debug, Clone, Default, serde::Serialize)]
  pub struct PackStats {
      pub sources_packed: usize,
      pub snippets_truncated: usize,
      pub chars_packed: usize,
  }

  /// Pack ALL hits (budget = hit count = k). Char-boundary-safe per-snippet cap; stats returned.
  pub fn pack_context(hits: &[RetrievedHit]) -> (String, PackStats) {
      let mut ctx = String::new();
      let mut stats = PackStats::default();
      for h in hits {
          let n_chars = h.snippet.chars().count();
          let snippet: std::borrow::Cow<'_, str> = if n_chars > PER_SNIPPET_CHAR_CAP {
              stats.snippets_truncated += 1;
              std::borrow::Cow::Owned(h.snippet.chars().take(PER_SNIPPET_CHAR_CAP).collect())
          } else {
              std::borrow::Cow::Borrowed(h.snippet.as_str())
          };
          stats.sources_packed += 1;
          stats.chars_packed += snippet.chars().count();
          ctx.push_str(&snippet);
          ctx.push('\n');
      }
      (ctx, stats)
  }

  // ── Retrieval seams (Rev 2, finding 4: injectable for the hermetic run-loop test). ──

  /// The AIR arm seam. `&mut` because the live impl drives an async wire client on an owned
  /// runtime.
  pub trait AirRetriever {
      fn retrieve(&mut self, query: &str, k: usize) -> anyhow::Result<Vec<RetrievedHit>>;
  }

  /// The GBrain arm seam.
  pub trait GbrainRetriever {
      fn retrieve(&self, query: &str, k: usize) -> anyhow::Result<Vec<RetrievedHit>>;
  }

  /// LIVE AIR arm: wire recall → PageResolver (FAIL-LOUD on an unmapped hit — the no-evolve
  /// invariant, see resolve.rs; NO event-id fallback).
  pub struct LiveAirArm {
      rt: tokio::runtime::Runtime,
      client: crate::client::WireClient,
      resolver: PageResolver,
  }

  impl LiveAirArm {
      /// Owns a current-thread runtime so the sync run loop can drive the async client.
      pub fn new(
          rt: tokio::runtime::Runtime,
          client: crate::client::WireClient,
          resolver: PageResolver,
      ) -> Self {
          Self { rt, client, resolver }
      }
  }

  /// Map wire hits → RetrievedHits through the resolver. Free function so e2e #1 exercises the
  /// EXACT same mapping the live arm uses (Rev 2, finding 2 — the un-rigged path).
  pub fn map_hits(
      resolver: &PageResolver,
      wire_hits: Vec<bossclawd_proto::HitWire>,
  ) -> anyhow::Result<Vec<RetrievedHit>> {
      wire_hits
          .into_iter()
          .map(|h| {
              Ok(RetrievedHit {
                  page_id: resolver.page_id_of(&h.hit.event_id)?, // loud: unmapped = run error
                  snippet: h.text,
              })
          })
          .collect()
  }

  impl AirRetriever for LiveAirArm {
      fn retrieve(&mut self, query: &str, k: usize) -> anyhow::Result<Vec<RetrievedHit>> {
          let wire_hits = self.rt.block_on(self.client.recall(query, k))?;
          map_hits(&self.resolver, wire_hits)
      }
  }

  /// LIVE GBrain arm: `gbrain query "<q>" --limit <k>` in balanced mode. ARGV IS PROBE-A-PINNED —
  /// if balanced needs an explicit flag, add it HERE (and note it in memharness-probes.md).
  #[derive(Default)]
  pub struct GbrainCli;

  impl GbrainRetriever for GbrainCli {
      fn retrieve(&self, query: &str, k: usize) -> anyhow::Result<Vec<RetrievedHit>> {
          let out = std::process::Command::new("gbrain")
              .arg("query")
              .arg(query)
              .arg("--limit")
              .arg(k.to_string())
              .output()
              .map_err(|e| anyhow::anyhow!("failed to spawn `gbrain query`: {e}"))?;
          if !out.status.success() {
              anyhow::bail!(
                  "`gbrain query` exited {}: {}",
                  out.status,
                  String::from_utf8_lossy(&out.stderr)
              );
          }
          parse_gbrain_output(&String::from_utf8_lossy(&out.stdout))
      }
  }

  /// Parse `gbrain query` output. MUST match the format pinned in
  /// tests/fixtures/gbrain_query_sample.txt (Probe A); a non-empty output that yields no hits is
  /// a RUN ERROR — never silently scored (spec §4).
  pub fn parse_gbrain_output(raw: &str) -> anyhow::Result<Vec<RetrievedHit>> {
      let mut hits = Vec::new();
      let mut cur_slug: Option<String> = None;
      let mut cur_text: Option<String> = None;
      for line in raw.lines() {
          if let Some(rest) = line.strip_prefix("slug: ") {
              if let (Some(s), Some(t)) = (cur_slug.take(), cur_text.take()) {
                  hits.push(RetrievedHit { page_id: page_id_from_gbrain_slug(&s), snippet: t });
              }
              cur_slug = Some(rest.trim().to_string());
          } else if let Some(rest) = line.strip_prefix("text: ") {
              cur_text = Some(rest.trim().to_string());
          } else if line.trim() == "---" {
              if let (Some(s), Some(t)) = (cur_slug.take(), cur_text.take()) {
                  hits.push(RetrievedHit { page_id: page_id_from_gbrain_slug(&s), snippet: t });
              }
          }
      }
      if let (Some(s), Some(t)) = (cur_slug, cur_text) {
          hits.push(RetrievedHit { page_id: page_id_from_gbrain_slug(&s), snippet: t });
      }
      if hits.is_empty() && !raw.trim().is_empty() {
          anyhow::bail!(
              "gbrain output did not parse to any hits — format may have changed (re-check Probe A)"
          );
      }
      Ok(hits)
  }

  /// `gbrain --version` + indexed page count for the drift check (spec §2 Rev 2). The count
  /// command is Probe-A-pinned; unavailable → None (the report then says "drift unknown").
  pub fn gbrain_version_and_count() -> (String, Option<usize>) {
      let version = std::process::Command::new("gbrain")
          .arg("--version")
          .output()
          .ok()
          .filter(|o| o.status.success())
          .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
          .unwrap_or_else(|| "unknown".to_string());
      // Probe A pins the page-count command (e.g. `gbrain stats`); parse the count out of it.
      // Until pinned, a failed/missing command is None — honest "drift unknown", never a guess.
      let count = std::process::Command::new("gbrain")
          .arg("stats")
          .output()
          .ok()
          .filter(|o| o.status.success())
          .and_then(|o| {
              let text = String::from_utf8_lossy(&o.stdout).to_string();
              // Grab the first integer following "pages" (case-insensitive) — adjust per Probe A.
              let lower = text.to_lowercase();
              let idx = lower.find("pages")?;
              lower[..idx]
                  .split(|c: char| !c.is_ascii_digit())
                  .filter(|s| !s.is_empty())
                  .last()
                  .and_then(|s| s.parse::<usize>().ok())
          });
      (version, count)
  }

  // ── The shared answerer seam (spec §4: identical model + prompt + budget on both arms). ──

  /// Both arms synthesize the final answer through the SAME implementation — retrieval is the
  /// only variable. Live = Ollama; tests inject doubles.
  pub trait Answerer {
      fn answer(&self, query: &str, context: &str) -> anyhow::Result<String>;
  }

  /// The live answerer: the local Ollama model (same one that judges + synthesizes).
  pub struct OllamaAnswerer {
      pub model: String,
  }

  impl Answerer for OllamaAnswerer {
      fn answer(&self, query: &str, context: &str) -> anyhow::Result<String> {
          let prompt = format!(
              "Answer the question using ONLY the context. If the context lacks the answer, say so.\n\n\
               Context:\n{context}\n\nQuestion: {query}\n\nAnswer:"
          );
          crate::ollama::generate(&self.model, &prompt)
      }
  }

  ```
  > **Implementer note:** the `use std::path::Path;` import at the top is only needed if `Path` appears in this file's signatures — drop it if clippy `-D warnings` flags it. `parse_gbrain_output` and `gbrain_version_and_count`'s count-parse are the two Probe-A-pinned surfaces: adjust BOTH to the fixture in the same commit if reality differs.
- [ ] Run: `cargo test -p memharness arms` → **PASS** (4 tests).
- [ ] Commit:
  ```
  git add crates/memharness/src/arms.rs crates/memharness/src/lib.rs
  git commit -m "$(cat <<'EOF'
feat(memharness): arms — page-dedup, k-hit packing + PackStats, gbrain CLI arm, LiveAirArm via fail-loud resolver, trait seams

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

---

## Task 37 — `synth.rs`: seeded stratified sampling + generator seam (RED)

- [ ] Create `crates/memharness/src/synth.rs` with the failing test:
  ```rust
  //! Synthetic known-item queries: 1–2 per SAMPLED page, stratified across top-level category
  //! dirs AND language, source page = gold (spec §3). Page SELECTION is seeded + deterministic;
  //! generation is a trait seam (tests never need Ollama).

  #[cfg(test)]
  mod tests {
      use super::*;
      use rand::SeedableRng;
      use rand_chacha::ChaCha8Rng;

      #[test]
      fn stratified_selection_is_seeded_and_covers_categories() {
          let pages: Vec<PageRef> = [
              ("air/a", "en"), ("air/b", "en"), ("air/c", "en"),
              ("people/d", "en"), ("people/e", "en"),
              ("ko/f", "ko"),
          ]
          .into_iter()
          .map(|(id, l)| PageRef { page_id: id.into(), lang: l.into() })
          .collect();
          let mut r1 = ChaCha8Rng::seed_from_u64(42);
          let mut r2 = ChaCha8Rng::seed_from_u64(42);
          let sel1 = stratified_sample(&pages, 4, &mut r1);
          assert_eq!(sel1, stratified_sample(&pages, 4, &mut r2), "seeded → deterministic");
          let cats: std::collections::HashSet<_> =
              sel1.iter().map(|p| p.page_id.split('/').next().unwrap()).collect();
          assert!(cats.contains("air") && cats.contains("people") && cats.contains("ko"),
              "every category represented before any gets a second pick");
      }
  }
  ```
- [ ] Add `pub mod synth;` to `lib.rs`.
- [ ] Run: `cargo test -p memharness synth` → **FAIL**.

## Task 38 — `synth.rs`: implement sampling + generator (GREEN)

- [ ] Add above the test module:
  ```rust
  use std::collections::BTreeMap;

  use rand::seq::SliceRandom;
  use rand::Rng;

  /// A page eligible for synthetic-query generation.
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct PageRef {
      pub page_id: String,
      pub lang: String, // "en" | "ko"
  }

  /// A synthetic query: generated text + source page (gold) + language.
  #[derive(Debug, Clone)]
  pub struct SynthQuery {
      pub text: String,
      pub gold_page_id: String,
      pub lang: String,
  }

  /// Deterministically select up to `total` pages, stratified by top-level category dir:
  /// round-robin one page per category (seeded shuffle within each) until `total` — every
  /// category with pages is represented before any gets a second pick.
  pub fn stratified_sample<R: Rng>(pages: &[PageRef], total: usize, rng: &mut R) -> Vec<PageRef> {
      let mut buckets: BTreeMap<String, Vec<PageRef>> = BTreeMap::new();
      for p in pages {
          let cat = p.page_id.split('/').next().unwrap_or("").to_string();
          buckets.entry(cat).or_default().push(p.clone());
      }
      for v in buckets.values_mut() {
          v.shuffle(rng);
      }
      let mut cat_order: Vec<String> = buckets.keys().cloned().collect();
      cat_order.shuffle(rng);
      let mut cursors: BTreeMap<String, usize> =
          cat_order.iter().map(|c| (c.clone(), 0)).collect();
      let mut selected = Vec::new();
      while selected.len() < total {
          let mut progressed = false;
          for cat in &cat_order {
              if selected.len() >= total {
                  break;
              }
              let bucket = &buckets[cat];
              let cur = cursors.get_mut(cat).expect("cursor exists");
              if *cur < bucket.len() {
                  selected.push(bucket[*cur].clone());
                  *cur += 1;
                  progressed = true;
              }
          }
          if !progressed {
              break; // all buckets exhausted
          }
      }
      selected
  }

  /// The generation seam: 1–2 known-item queries for a page. Live = Ollama; tests inject doubles.
  pub trait QueryGenerator {
      fn generate_queries(&self, page: &PageRef, page_text: &str) -> anyhow::Result<Vec<SynthQuery>>;
  }

  /// Live generator: asks the local model for ONE specific question the page answers, in the
  /// page's language (Korean pages get Korean queries, spec §3).
  pub struct OllamaQueryGenerator {
      pub model: String,
  }

  impl QueryGenerator for OllamaQueryGenerator {
      fn generate_queries(&self, page: &PageRef, page_text: &str) -> anyhow::Result<Vec<SynthQuery>> {
          let lang_instr = if page.lang == "ko" {
              "Write the question in Korean."
          } else {
              "Write the question in English."
          };
          let excerpt: String = page_text.chars().take(4000).collect();
          let prompt = format!(
              "Read the note below. Write ONE specific question that this note (and ideally only \
               this note) answers. {lang_instr} Output only the question.\n\nNote:\n{excerpt}\n"
          );
          let text = crate::ollama::generate(&self.model, &prompt)?.trim().to_string();
          if text.is_empty() {
              anyhow::bail!("generator returned an empty query for {}", page.page_id);
          }
          Ok(vec![SynthQuery { text, gold_page_id: page.page_id.clone(), lang: page.lang.clone() }])
      }
  }
  ```
- [ ] Run: `cargo test -p memharness synth` → **PASS**.
- [ ] Commit:
  ```
  git add crates/memharness/src/synth.rs crates/memharness/src/lib.rs
  git commit -m "$(cat <<'EOF'
feat(memharness): synth — seeded stratified sampling + QueryGenerator seam

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

---

## Task 39 — `report.rs`: hardened guard + drift banner + render (RED)

**(Rev 2, findings 7 + 10 + 11 + 14.)**

- [ ] Create `crates/memharness/src/report.rs` with the failing tests:
  ```rust
  //! The per-run report (spec §7): markdown + raw scores JSON into
  //! `~/.air-harness/reports/<timestamp>/`. NEVER under the repo/workspace — the guard resolves
  //! symlinks and not-yet-created paths (Rev 2). Renders the INVALID-RUN drift banner (>5%),
  //! the trust verdict FIRST, per-arm pack stats, and every honesty caveat.

  #[cfg(test)]
  mod tests {
      use super::*;
      use std::path::Path;

      /// The workspace root (nearest ancestor with .git) — what the guard must protect.
      fn workspace_root() -> std::path::PathBuf {
          let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
          crate_root
              .ancestors()
              .find(|p| p.join(".git").exists())
              .expect("workspace root")
              .to_path_buf()
      }

      #[test]
      fn refuses_repo_workspace_and_symlinked_paths() {
          let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")); // crates/memharness
          // (a) Inside this crate.
          assert!(ensure_outside_repo(&repo_root.join("reports"), repo_root).is_err());
          // (b) Rev 2: under the WORKSPACE root but outside crates/memharness.
          let ws_target = workspace_root().join("target/harness-reports");
          let err = ensure_outside_repo(&ws_target, repo_root).unwrap_err();
          assert!(err.to_string().contains("inside the repo"), "workspace guarded: {err}");
          // (c) Rev 2: a symlink that LOOKS outside but resolves INTO the workspace.
          let tmp = tempfile::tempdir().unwrap();
          let link = tmp.path().join("sneaky");
          std::os::unix::fs::symlink(workspace_root().join("target"), &link).unwrap();
          assert!(
              ensure_outside_repo(&link.join("reports"), repo_root).is_err(),
              "symlink into the repo is refused"
          );
          // (d) A genuinely outside dir (possibly not yet created) is allowed.
          assert!(ensure_outside_repo(&tmp.path().join("ok/reports"), repo_root).is_ok());
      }

      #[test]
      fn truncation_is_char_boundary_safe_for_korean() {
          // Rev 2 (finding 11): byte-slicing panics mid-codepoint; chars() must not.
          let ko = "감마선은".repeat(200); // > 200 chars, all multibyte
          let out = truncate_for_example(&ko);
          assert!(out.chars().count() <= 201, "200 chars + ellipsis");
      }

      #[test]
      fn renders_trust_first_drift_banner_and_caveats() {
          let mut report = ReportModel::sample_for_test();
          let md = render_markdown(&report);
          let trust_idx = md.find("## Judge-trust verdict").expect("trust section present");
          let headline_idx = md.find("## Headline").expect("headline present");
          assert!(trust_idx < headline_idx, "trust verdict LEADS");
          assert!(md.contains("### EN vs KO"));
          assert!(md.contains("### Known-item vs open"));
          assert!(md.contains("retrieval k == scoring k"), "one-knob statement (finding 14)");
          assert!(md.contains("Context packing"), "per-arm pack stats present");
          assert!(md.contains("near-duplicate"), "exact-only dedup caveat present");
          assert!(md.contains("binary success flags"), "tie-heavy Wilcoxon caveat present");
          assert!(!md.contains("INVALID RUN"), "no banner at 0 drift");
          // GOLDEN on synthetic data: render is deterministic.
          assert_eq!(md, ReportModel::sample_golden_markdown());

          // Rev 2 (finding 10): drift > 5% → the INVALID-RUN banner leads the whole report.
          report.drift_fraction = Some(0.12);
          let md2 = render_markdown(&report);
          assert!(md2.starts_with("# ⚠ INVALID RUN"), "banner is the FIRST line");

          // audit_n_too_small renders its indicative-only note.
          report.drift_fraction = None;
          report.trust.audit_n_too_small = true;
          assert!(render_markdown(&report).contains("indicative only"));
      }
  }
  ```
- [ ] Add `pub mod report;` to `lib.rs`.
- [ ] Run: `cargo test -p memharness report` → **FAIL**.

## Task 40 — `report.rs`: implement model + guard + render + write (GREEN)

- [ ] Add above the test module:
  ```rust
  use std::path::{Path, PathBuf};

  use serde::Serialize;

  use crate::arms::PackStats;
  use crate::corpus::CorpusManifest;
  use crate::judge::TrustVerdict;
  use crate::stats::WilcoxonResult;

  /// Drift above this fraction invalidates the run (spec §2 Rev 2).
  pub const DRIFT_INVALID_FRACTION: f64 = 0.05;

  /// One segment row.
  #[derive(Debug, Clone, Serialize)]
  pub struct SegmentResult {
      pub label: String, // "real·en·known-item" etc.
      pub n: usize,
      pub air_success_at_k: f64,
      pub gbrain_success_at_k: f64,
      pub air_mrr: f64,
      pub gbrain_mrr: f64,
      pub air_win_rate: f64, // open segments only (0.0 on known-item rows)
      pub ci_low: f64,
      pub ci_high: f64,
      pub wilcoxon: Option<WilcoxonResult>,
  }

  /// Per-arm packing totals across the run (Rev 2, finding 9).
  #[derive(Debug, Clone, Default, Serialize)]
  pub struct PackTotals {
      pub sources_packed: usize,
      pub snippets_truncated: usize,
      pub chars_packed: usize,
  }

  impl PackTotals {
      pub fn add(&mut self, s: &PackStats) {
          self.sources_packed += s.sources_packed;
          self.snippets_truncated += s.snippets_truncated;
          self.chars_packed += s.chars_packed;
      }
  }

  /// One example win/loss with retrieved-context diffs (spec §7).
  #[derive(Debug, Clone, Serialize)]
  pub struct ExamplePair {
      pub query: String,
      pub winner: String, // "AIR" | "GBrain"
      pub air_context: String,
      pub gbrain_context: String,
  }

  /// The full report model (markdown + raw JSON).
  #[derive(Debug, Clone, Serialize)]
  pub struct ReportModel {
      pub trust: TrustVerdict,
      pub k: usize,
      pub segments: Vec<SegmentResult>,
      pub corpus: CorpusManifest,
      pub gbrain_version: String,
      pub gbrain_page_count: Option<usize>,
      /// |gbrain_pages − corpus_pages| / corpus_pages; None = count unavailable ("drift unknown").
      pub drift_fraction: Option<f64>,
      pub ollama_model: String,
      pub egress_pairs_sent: usize,
      pub local_only: bool,
      pub near_dedup_applied: bool, // false in Phase 0 → explicit caveat
      pub air_pack: PackTotals,
      pub gbrain_pack: PackTotals,
      pub examples: Vec<ExamplePair>,
  }

  /// Canonicalize the nearest EXISTING ancestor of `p`, rejoining the non-existing tail — so a
  /// symlinked or not-yet-created reports dir resolves to its REAL location before the guard
  /// compares (Rev 2, finding 7).
  fn canonicalize_nearest_existing(p: &Path) -> PathBuf {
      let mut existing = p;
      loop {
          if existing.as_os_str().is_empty() {
              return p.to_path_buf();
          }
          if let Ok(canon) = std::fs::canonicalize(existing) {
              let tail = p.strip_prefix(existing).unwrap_or_else(|_| Path::new(""));
              return canon.join(tail);
          }
          match existing.parent() {
              Some(parent) => existing = parent,
              None => return p.to_path_buf(),
          }
      }
  }

  /// Refuse any reports dir under the WORKSPACE (nearest `.git` ancestor of `repo_root`) —
  /// reports quote brain content and the repo is public. Symlink- and nonexistent-path-resistant.
  pub fn ensure_outside_repo(reports_dir: &Path, repo_root: &Path) -> anyhow::Result<()> {
      let repo_canon = std::fs::canonicalize(repo_root).unwrap_or_else(|_| repo_root.to_path_buf());
      let forbidden = repo_canon
          .ancestors()
          .find(|p| p.join(".git").exists())
          .map(|p| p.to_path_buf())
          .unwrap_or(repo_canon);
      let resolved = canonicalize_nearest_existing(reports_dir);
      if resolved.starts_with(&forbidden) {
          anyhow::bail!(
              "refusing to write reports inside the repo ({resolved:?} is under {forbidden:?}); \
               reports quote brain content"
          );
      }
      Ok(())
  }

  /// Char-boundary-safe example truncation (Rev 2, finding 11 — Korean text!).
  fn truncate_for_example(s: &str) -> String {
      const MAX: usize = 200;
      if s.chars().count() <= MAX {
          s.to_string()
      } else {
          let head: String = s.chars().take(MAX).collect();
          format!("{head}…")
      }
  }

  /// Render the report. Order: INVALID-RUN banner (if drift > threshold) → judge-trust verdict →
  /// headline → EN/KO → known-item vs open → caveats → egress → examples (spec §7 Rev 2).
  pub fn render_markdown(r: &ReportModel) -> String {
      let mut s = String::new();
      // 0. Drift banner (Rev 2, finding 10) — the FIRST thing a reader sees.
      if let Some(d) = r.drift_fraction {
          if d > DRIFT_INVALID_FRACTION {
              s.push_str(&format!(
                  "# ⚠ INVALID RUN — corpus drift {:.1}% (> {:.0}% threshold)\n\n\
                   GBrain's index and ~/brain diverge too far for an apples-to-apples comparison. \
                   Re-sync GBrain and re-run.\n\n",
                  d * 100.0,
                  DRIFT_INVALID_FRACTION * 100.0
              ));
          }
      }
      // 1. Judge-trust verdict (LEADS).
      s.push_str("## Judge-trust verdict\n");
      if r.trust.audit_incomplete {
          if r.local_only {
              s.push_str("No audit this run (--local-only). Local-judge scores are unverified.\n\n");
          } else {
              s.push_str(
                  "Trust verdict UNAVAILABLE — the cloud audit did not complete (API failure). \
                   Local-judge scores are unverified this run.\n\n",
              );
          }
      } else {
          s.push_str(&format!(
              "Local vs cloud agreement: {:.1}% · Cohen's kappa: {:.3} · audited {} pairs · **{}**{}{}\n\n",
              r.trust.agreement * 100.0,
              r.trust.kappa,
              r.trust.audited_count,
              if r.trust.trusted { "TRUSTED" } else { "the local judge is not yet trustworthy" },
              if r.trust.expanded_to_full_audit { " (audit auto-expanded to 100%)" } else { "" },
              if r.trust.audit_n_too_small { " · audit n too small; trust verdict indicative only" } else { "" },
          ));
      }
      // 2. Headline.
      s.push_str("## Headline\n");
      s.push_str(&format!(
          "Corpus: {} pages, {} bytes · k={} (retrieval k == scoring k) · model={} · gbrain {}{}{}\n\n",
          r.corpus.file_count,
          r.corpus.total_bytes,
          r.k,
          r.ollama_model,
          r.gbrain_version,
          match r.gbrain_page_count {
              Some(p) => format!(" ({p} pages indexed)"),
              None => " (page count unavailable — drift unknown)".to_string(),
          },
          match r.drift_fraction {
              Some(d) => format!(" · drift {:.1}%", d * 100.0),
              None => String::new(),
          },
      ));
      s.push_str("| segment | n | AIR s@k | GBrain s@k | AIR MRR | GBrain MRR | AIR win% | 95% CI |\n");
      s.push_str("|---|---|---|---|---|---|---|---|\n");
      for seg in &r.segments {
          s.push_str(&format!(
              "| {} | {} | {:.3} | {:.3} | {:.3} | {:.3} | {:.1}% | [{:.3}, {:.3}] |\n",
              seg.label, seg.n, seg.air_success_at_k, seg.gbrain_success_at_k,
              seg.air_mrr, seg.gbrain_mrr, seg.air_win_rate * 100.0, seg.ci_low, seg.ci_high,
          ));
      }
      s.push('\n');
      // 3. EN vs KO (the expected bilingual gap made visible).
      s.push_str("### EN vs KO\n");
      for seg in r.segments.iter().filter(|s| s.label.contains("·en·") || s.label.contains("·ko·")) {
          s.push_str(&format!(
              "- {} : AIR s@k {:.3} vs GBrain {:.3}\n",
              seg.label, seg.air_success_at_k, seg.gbrain_success_at_k
          ));
      }
      s.push('\n');
      // 4. Known-item vs open.
      s.push_str("### Known-item vs open\n");
      for seg in &r.segments {
          s.push_str(&format!("- {} : n={}\n", seg.label, seg.n));
      }
      s.push('\n');
      // 5. Context packing (Rev 2, finding 9) + granularity note.
      s.push_str("### Context packing (budget = k hits, both arms)\n");
      s.push_str(&format!(
          "AIR: {} sources, {} truncated, {} chars · GBrain: {} sources, {} truncated, {} chars\n",
          r.air_pack.sources_packed, r.air_pack.snippets_truncated, r.air_pack.chars_packed,
          r.gbrain_pack.sources_packed, r.gbrain_pack.snippets_truncated, r.gbrain_pack.chars_packed,
      ));
      s.push_str(
          "> Granularity note: GBrain returns CHUNKS, AIR returns page-level snippets — hit-count \
           budgeting avoids a char-truncation confound, but per-hit sizes differ by design; \
           per-arm chars are reported for transparency.\n\n",
      );
      // 6. Honesty caveats.
      for seg in &r.segments {
          if let Some(w) = &seg.wilcoxon {
              if w.small_n_approx {
                  s.push_str(&format!(
                      "> small-n ({}): Wilcoxon p={:.4} via normal approx — exact test advised.\n",
                      seg.label, w.p_value
                  ));
              }
          }
      }
      s.push_str(
          "> Known-item Wilcoxon runs on binary success flags (heavy ties) — read it as \
           sign-test-like (spec §8 Rev 2).\n",
      );
      if !r.near_dedup_applied {
          s.push_str("> Caveat: near-duplicate query collapse is NOT applied in Phase 0 (exact dedup only).\n");
      }
      // 7. Egress accounting.
      s.push_str(&format!(
          "\n### Egress\n{} query/answer pair(s) egressed to the cloud audit (2 position-swapped \
           calls each){}. GBrain's own pipeline may egress per its config (its normal behavior).\n",
          r.egress_pairs_sent,
          if r.local_only { " — zero: --local-only" } else { "" },
      ));
      // 8. Examples.
      s.push_str("\n### Examples (wins/losses with context diffs)\n");
      for ex in &r.examples {
          s.push_str(&format!(
              "- **{}** — winner: {}\n  - AIR ctx: {}\n  - GBrain ctx: {}\n",
              ex.query,
              ex.winner,
              truncate_for_example(&ex.air_context),
              truncate_for_example(&ex.gbrain_context),
          ));
      }
      s
  }

  /// Write markdown + raw JSON into `reports_dir/<timestamp>/` AFTER the guard. Returns the dir.
  pub fn write_report(
      reports_dir: &Path,
      repo_root: &Path,
      r: &ReportModel,
  ) -> anyhow::Result<PathBuf> {
      ensure_outside_repo(reports_dir, repo_root)?;
      let out_dir = reports_dir.join(r.corpus.snapshot_unix_secs.to_string());
      std::fs::create_dir_all(&out_dir)?;
      std::fs::write(out_dir.join("report.md"), render_markdown(r))?;
      std::fs::write(out_dir.join("scores.json"), serde_json::to_vec_pretty(r)?)?;
      Ok(out_dir)
  }
  ```
- [ ] Add the test-scoped sample constructors INSIDE the `#[cfg(test)] mod tests` block:
  ```rust
  impl ReportModel {
      /// Fully synthetic sample for the golden snapshot (no brain content).
      pub fn sample_for_test() -> ReportModel {
          use crate::corpus::{CorpusManifest, ManifestEntry};
          use crate::judge::TrustVerdict;
          ReportModel {
              trust: TrustVerdict {
                  audited_count: 20, agreement: 0.9, kappa: 0.72, trusted: true,
                  expanded_to_full_audit: false, audit_incomplete: false, audit_n_too_small: false,
              },
              k: 10,
              segments: vec![SegmentResult {
                  label: "synthetic·en·known-item".into(), n: 3,
                  air_success_at_k: 0.667, gbrain_success_at_k: 1.0,
                  air_mrr: 0.5, gbrain_mrr: 0.833, air_win_rate: 0.0,
                  ci_low: 0.3, ci_high: 1.0, wilcoxon: None,
              }],
              corpus: CorpusManifest {
                  snapshot_unix_secs: 1000, file_count: 3, total_bytes: 42,
                  entries: vec![ManifestEntry {
                      page_id: "en/alpha".into(), sha256: "deadbeef".into(), bytes: 14,
                  }],
              },
              gbrain_version: "gbrain 0.42".into(),
              gbrain_page_count: Some(866),
              drift_fraction: Some(0.01),
              ollama_model: "qwen2.5:7b".into(),
              egress_pairs_sent: 4,
              local_only: false,
              near_dedup_applied: false,
              air_pack: PackTotals { sources_packed: 30, snippets_truncated: 1, chars_packed: 9000 },
              gbrain_pack: PackTotals { sources_packed: 30, snippets_truncated: 0, chars_packed: 4000 },
              examples: vec![ExamplePair {
                  query: "who is alpha".into(), winner: "GBrain".into(),
                  air_context: "alpha ctx".into(), gbrain_context: "beta ctx".into(),
              }],
          }
      }

      /// Expected markdown for `sample_for_test()` — self-consistent golden (asserts render
      /// determinism; the load-bearing structure assertions are the section-order checks).
      pub fn sample_golden_markdown() -> String {
          render_markdown(&ReportModel::sample_for_test())
      }
  }
  ```
- [ ] Run: `cargo test -p memharness report` → **PASS** (3 tests).
- [ ] Commit:
  ```
  git add crates/memharness/src/report.rs crates/memharness/src/lib.rs
  git commit -m "$(cat <<'EOF'
feat(memharness): report — symlink-resistant workspace guard, drift INVALID banner, pack stats, honesty caveats

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

---

## Task 41 — `run.rs`: the run-loop assembly (RED)

**(Rev 2, finding 4 — the assembly is CODE, not prose, and hermetically testable through the trait seams.)**

- [ ] Create `crates/memharness/src/run.rs` with the failing unit test (content-based doubles — a content judge survives blinding/swapping because it judges ANSWERS, not positions):
  ```rust
  //! Run orchestration (spec §4–§6): the per-query loop, segment bucketing, blind judging, the
  //! audit ladder (sample → trust → expand-to-100%), and egress accounting. Pure over the
  //! injected seams (`AirRetriever`/`GbrainRetriever`/`Answerer`/`PairJudge`) so e2e #2 drives it
  //! hermetically; `main.rs` supplies the live seams.

  #[cfg(test)]
  mod tests {
      use super::*;
      use crate::arms::{AirRetriever, Answerer, GbrainRetriever, RetrievedHit};
      use crate::judge::{PairJudge, PosPick};

      /// AIR double: gold at rank 0 for known-item cases; a GOODCTX snippet for open case 1.
      struct AirDouble;
      impl AirRetriever for AirDouble {
          fn retrieve(&mut self, query: &str, _k: usize) -> anyhow::Result<Vec<RetrievedHit>> {
              Ok(match query {
                  "known-air-wins" => vec![
                      RetrievedHit { page_id: "en/alpha".into(), snippet: "a".into() },
                      // A duplicate page hit: dedup must collapse it BEFORE ranking.
                      RetrievedHit { page_id: "en/alpha".into(), snippet: "a-dup".into() },
                  ],
                  "open-air-good" => vec![RetrievedHit { page_id: "x".into(), snippet: "GOODCTX".into() }],
                  _ => vec![RetrievedHit { page_id: "y".into(), snippet: "meh".into() }],
              })
          }
      }
      /// GBrain double: misses the gold; a GOODCTX snippet for open case 2.
      struct GbrainDouble;
      impl GbrainRetriever for GbrainDouble {
          fn retrieve(&self, query: &str, _k: usize) -> anyhow::Result<Vec<RetrievedHit>> {
              Ok(match query {
                  "open-gbrain-good" => vec![RetrievedHit { page_id: "z".into(), snippet: "GOODCTX".into() }],
                  _ => vec![RetrievedHit { page_id: "en/beta".into(), snippet: "b".into() }],
              })
          }
      }
      /// Answerer double: echoes its context (so the judge can see which arm had GOODCTX).
      struct EchoAnswerer;
      impl Answerer for EchoAnswerer {
          fn answer(&self, _q: &str, context: &str) -> anyhow::Result<String> {
              Ok(format!("answer[{context}]"))
          }
      }
      /// Judge double: prefers the answer containing GOODCTX (content-based → blind-compatible).
      struct GoodJudge;
      impl PairJudge for GoodJudge {
          fn pick(&self, _q: &str, a: &str, b: &str) -> anyhow::Result<Option<PosPick>> {
              Ok(match (a.contains("GOODCTX"), b.contains("GOODCTX")) {
                  (true, false) => Some(PosPick::A),
                  (false, true) => Some(PosPick::B),
                  _ => Some(PosPick::Tie),
              })
          }
      }

      fn cases() -> Vec<QueryCase> {
          vec![
              QueryCase { text: "known-air-wins".into(), lang: "en".into(), source: QuerySource::Synthetic, gold_page_id: Some("en/alpha".into()) },
              QueryCase { text: "open-air-good".into(), lang: "en".into(), source: QuerySource::Real, gold_page_id: None },
              QueryCase { text: "open-gbrain-good".into(), lang: "ko".into(), source: QuerySource::Real, gold_page_id: None },
          ]
      }

      #[test]
      fn run_queries_buckets_judges_audits_and_counts_egress() {
          let cfg = RunConfig { k: 10, seed: 42, local_only: false };
          let outcome = run_queries(
              &cfg, &cases(), &mut AirDouble, &GbrainDouble, &EchoAnswerer, &GoodJudge,
              Some(&GoodJudge), // auditor = same content judge → 100% agreement
          )
          .unwrap();

          // Buckets: synthetic·en·known-item, real·en·open, real·ko·open (BTreeMap order).
          let labels: Vec<&str> = outcome.segments.iter().map(|s| s.label.as_str()).collect();
          assert!(labels.contains(&"synthetic·en·known-item"));
          assert!(labels.contains(&"real·en·open"));
          assert!(labels.contains(&"real·ko·open"));

          // Known-item: AIR found gold at rank 0 (after page-dedup), GBrain missed.
          let ki = outcome.segments.iter().find(|s| s.label == "synthetic·en·known-item").unwrap();
          assert!((ki.air_success_at_k - 1.0).abs() < 1e-9);
          assert!((ki.gbrain_success_at_k - 0.0).abs() < 1e-9);
          assert!((ki.air_mrr - 1.0).abs() < 1e-9, "dup page hit did not displace the rank");

          // Open verdicts: AIR wins its GOODCTX case, loses the other.
          let en_open = outcome.segments.iter().find(|s| s.label == "real·en·open").unwrap();
          assert!((en_open.air_win_rate - 1.0).abs() < 1e-9);
          let ko_open = outcome.segments.iter().find(|s| s.label == "real·ko·open").unwrap();
          assert!((ko_open.air_win_rate - 0.0).abs() < 1e-9);

          // Audit: pool (2 open) < AUDIT_FLOOR → whole pool audited, flag set, perfect agreement.
          assert!(outcome.trust.trusted);
          assert!(outcome.trust.audit_n_too_small);
          assert_eq!(outcome.trust.audited_count, 2);
          assert_eq!(outcome.egress_pairs_sent, 2, "one egressed pair per audited open query");

          // Examples: one win + one loss captured with contexts.
          assert_eq!(outcome.examples.len(), 2);

          // Pack totals: every open-case hit packed on both arms.
          assert_eq!(outcome.air_pack.sources_packed, 2);
          assert_eq!(outcome.gbrain_pack.sources_packed, 2);
      }

      #[test]
      fn local_only_skips_audit_and_reports_it() {
          let cfg = RunConfig { k: 10, seed: 42, local_only: true };
          let outcome = run_queries(
              &cfg, &cases(), &mut AirDouble, &GbrainDouble, &EchoAnswerer, &GoodJudge, None,
          )
          .unwrap();
          assert!(outcome.trust.audit_incomplete, "no audit this run");
          assert!(!outcome.trust.trusted);
          assert_eq!(outcome.egress_pairs_sent, 0, "--local-only ⇒ zero cloud egress");
      }
  }
  ```
- [ ] Add `pub mod run;` to `lib.rs`.
- [ ] Run: `cargo test -p memharness run` → **FAIL**.

## Task 42 — `run.rs`: implement `run_queries` (GREEN)

- [ ] Add above the test module:
  ```rust
  use std::collections::{BTreeMap, BTreeSet};

  use rand::SeedableRng;
  use rand_chacha::ChaCha8Rng;

  use crate::arms::{dedup_by_page, gold_rank, pack_context, AirRetriever, Answerer, GbrainRetriever};
  use crate::judge::{
      assign_blind, judge_pair_blind, select_audit_indices, trust_verdict, Blind, PairJudge,
      TrustVerdict, Verdict, AUDIT_FLOOR,
  };
  use crate::mine::MinedQuery;
  use crate::report::{ExamplePair, PackTotals, SegmentResult};
  use crate::stats::{
      bootstrap_ci_mean, mean_reciprocal_rank, mean_success_at_k, success_at_k,
      wilcoxon_signed_rank, GoldRank,
  };
  use crate::synth::SynthQuery;

  /// Segment tag: where a query came from.
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum QuerySource {
      Real,
      Synthetic,
  }

  /// One query case. `gold_page_id`: Some = known-item (mechanical), None = open (judged).
  #[derive(Debug, Clone)]
  pub struct QueryCase {
      pub text: String,
      pub lang: String, // "en" | "ko"
      pub source: QuerySource,
      pub gold_page_id: Option<String>,
  }

  /// Run knobs. `k` is BOTH retrieval depth and scoring depth — retrieval-k == scoring-k, ONE
  /// knob (spec §4 Rev 2, finding 14).
  pub struct RunConfig {
      pub k: usize,
      pub seed: u64,
      pub local_only: bool,
  }

  /// Everything the loop produces; `report.rs` renders it.
  pub struct RunOutcome {
      pub segments: Vec<SegmentResult>,
      pub trust: TrustVerdict,
      pub egress_pairs_sent: usize,
      pub examples: Vec<ExamplePair>,
      pub air_pack: PackTotals,
      pub gbrain_pack: PackTotals,
  }

  /// Build the unified case list from mined real + synthetic queries (language of a real query
  /// comes from its own text via the Hangul heuristic).
  pub fn cases_from(mined: Vec<MinedQuery>, synth: Vec<SynthQuery>) -> Vec<QueryCase> {
      let mut out = Vec::with_capacity(mined.len() + synth.len());
      for m in mined {
          let lang = match crate::frontmatter::detect_lang(&m.text) {
              crate::frontmatter::Lang::Ko => "ko".to_string(),
              crate::frontmatter::Lang::En => "en".to_string(),
          };
          out.push(QueryCase { lang, source: QuerySource::Real, gold_page_id: m.gold_page_id, text: m.text });
      }
      for s in synth {
          out.push(QueryCase {
              text: s.text,
              lang: s.lang,
              source: QuerySource::Synthetic,
              gold_page_id: Some(s.gold_page_id),
          });
      }
      out
  }

  /// A judged open query's full record (kept for auditing + examples).
  struct OpenRecord {
      case_idx: usize,
      air_answer: String,
      gbrain_answer: String,
      air_ctx: String,
      gbrain_ctx: String,
      blind: Blind,
      local_verdict: Verdict,
  }

  /// Per-segment accumulators.
  #[derive(Default)]
  struct Bucket {
      air_ranks: Vec<GoldRank>,
      gbrain_ranks: Vec<GoldRank>,
      open_air_scores: Vec<f64>, // 1.0 AirWins / 0.0 GbrainWins / 0.5 Tie|Uncertain
      n: usize,
  }

  fn bucket_label(case: &QueryCase) -> String {
      let src = match case.source {
          QuerySource::Real => "real",
          QuerySource::Synthetic => "synthetic",
      };
      let kind = if case.gold_page_id.is_some() { "known-item" } else { "open" };
      format!("{src}·{}·{kind}", case.lang)
  }

  /// AIR's numeric score for a verdict (ties/uncertains split the point; gbrain = 1 − air).
  fn air_score(v: Verdict) -> f64 {
      match v {
          Verdict::AirWins => 1.0,
          Verdict::GbrainWins => 0.0,
          Verdict::Tie | Verdict::Uncertain => 0.5,
      }
  }

  const BOOTSTRAP_ITERS: usize = 2000;
  const CI_CONF: f64 = 0.95;

  /// The whole measurement, over injected seams (spec §4–§6). Deterministic given the seed
  /// (fixed query order; seeded blinding/sampling/bootstrap).
  pub fn run_queries(
      cfg: &RunConfig,
      cases: &[QueryCase],
      air: &mut dyn AirRetriever,
      gbrain: &dyn GbrainRetriever,
      answerer: &dyn Answerer,
      judge: &dyn PairJudge,
      auditor: Option<&dyn PairJudge>,
  ) -> anyhow::Result<RunOutcome> {
      let mut rng = ChaCha8Rng::seed_from_u64(cfg.seed);
      let mut buckets: BTreeMap<String, Bucket> = BTreeMap::new();
      let mut opens: Vec<OpenRecord> = Vec::new();
      let mut air_pack = PackTotals::default();
      let mut gbrain_pack = PackTotals::default();

      // ── Per-query loop (fixed order = deterministic). ──
      for (case_idx, case) in cases.iter().enumerate() {
          // Both arms retrieve at the SAME k; page-dedup BEFORE any scoring (finding 2).
          let air_hits = dedup_by_page(air.retrieve(&case.text, cfg.k)?);
          let gbrain_hits = dedup_by_page(gbrain.retrieve(&case.text, cfg.k)?);
          let bucket = buckets.entry(bucket_label(case)).or_default();
          bucket.n += 1;

          match &case.gold_page_id {
              // Known-item: mechanical success@k/MRR, no judge (spec §5).
              Some(gold) => {
                  bucket.air_ranks.push(gold_rank(&air_hits, gold));
                  bucket.gbrain_ranks.push(gold_rank(&gbrain_hits, gold));
              }
              // Open: identical answerer both arms → blind position-swapped local judging.
              None => {
                  let (air_ctx, a_stats) = pack_context(&air_hits);
                  let (gbrain_ctx, g_stats) = pack_context(&gbrain_hits);
                  air_pack.add(&a_stats);
                  gbrain_pack.add(&g_stats);
                  let air_answer = answerer.answer(&case.text, &air_ctx)?;
                  let gbrain_answer = answerer.answer(&case.text, &gbrain_ctx)?;
                  let blind = assign_blind(&mut rng);
                  let local_verdict =
                      judge_pair_blind(judge, blind, &case.text, &air_answer, &gbrain_answer)?;
                  opens.push(OpenRecord {
                      case_idx, air_answer, gbrain_answer, air_ctx, gbrain_ctx, blind, local_verdict,
                  });
              }
          }
      }

      // ── Audit ladder (spec §5 Rev 2): sample ∪ uncertains → trust → expand if untrusted. ──
      let uncertain_idx: Vec<usize> = opens
          .iter()
          .enumerate()
          .filter(|(_, o)| o.local_verdict == Verdict::Uncertain)
          .map(|(i, _)| i)
          .collect();
      let audit_n_too_small = opens.len() < AUDIT_FLOOR;
      let mut egress_pairs_sent = 0usize;
      let mut audit_failed = false;
      let mut audited: BTreeMap<usize, Verdict> = BTreeMap::new();

      let trust = match auditor {
          Some(auditor) => {
              let initial = select_audit_indices(opens.len(), &uncertain_idx, &mut rng);
              audit_set(&initial, &opens, cases, auditor, &mut audited, &mut egress_pairs_sent, &mut audit_failed);
              let mut t = trust_from(&opens, &audited, false, audit_failed, audit_n_too_small);
              if !t.trusted && !t.audit_incomplete {
                  // Auto-expand to 100% of open queries and recompute (spec §5).
                  let all: BTreeSet<usize> = (0..opens.len()).collect();
                  audit_set(&all, &opens, cases, auditor, &mut audited, &mut egress_pairs_sent, &mut audit_failed);
                  t = trust_from(&opens, &audited, true, audit_failed, audit_n_too_small);
              }
              t
          }
          // --local-only: no audit; the report says so plainly (spec §6).
          None => trust_verdict(&[], &[], false, true, audit_n_too_small),
      };

      // ── Fold LOCAL verdicts into buckets. (The audit measures the JUDGE; it does not replace
      //    the judge's scores — spec §5's trust contract.) ──
      for o in &opens {
          let label = bucket_label(&cases[o.case_idx]);
          buckets
              .get_mut(&label)
              .expect("bucket created in the loop above")
              .open_air_scores
              .push(air_score(o.local_verdict));
      }

      // ── Per-bucket stats (BTreeMap order + one threaded rng = deterministic). ──
      let mut segments = Vec::with_capacity(buckets.len());
      for (label, b) in &buckets {
          segments.push(segment_result(label, b, cfg.k, &mut rng));
      }

      let examples = pick_examples(&opens, cases);
      Ok(RunOutcome { segments, trust, egress_pairs_sent, examples, air_pack, gbrain_pack })
  }

  /// Audit every not-yet-audited index in `set`: the auditor judges the SAME pair blind +
  /// position-swapped (2 API calls = ONE egressed pair). A call failure sets `audit_failed`
  /// (→ "audit incomplete", trust unavailable — never fabricated) and stops further egress.
  fn audit_set(
      set: &BTreeSet<usize>,
      opens: &[OpenRecord],
      cases: &[QueryCase],
      auditor: &dyn PairJudge,
      audited: &mut BTreeMap<usize, Verdict>,
      egress_pairs_sent: &mut usize,
      audit_failed: &mut bool,
  ) {
      for &i in set {
          if audited.contains_key(&i) || *audit_failed {
              continue;
          }
          let o = &opens[i];
          *egress_pairs_sent += 1;
          match judge_pair_blind(auditor, o.blind, &cases[o.case_idx].text, &o.air_answer, &o.gbrain_answer) {
              Ok(v) => {
                  audited.insert(i, v);
              }
              Err(e) => {
                  eprintln!("memharness: cloud audit failed on pair {i}: {e} — trust verdict will be unavailable");
                  *audit_failed = true;
              }
          }
      }
  }

  /// Pair local vs cloud verdicts over the audited set → the trust verdict.
  fn trust_from(
      opens: &[OpenRecord],
      audited: &BTreeMap<usize, Verdict>,
      expanded: bool,
      audit_failed: bool,
      audit_n_too_small: bool,
  ) -> TrustVerdict {
      let local: Vec<Verdict> = audited.keys().map(|&i| opens[i].local_verdict).collect();
      let cloud: Vec<Verdict> = audited.values().copied().collect();
      let incomplete = audit_failed || (local.is_empty() && !opens.is_empty());
      trust_verdict(&local, &cloud, expanded, incomplete, audit_n_too_small)
  }

  /// One segment row. Known-item buckets: success@k/MRR both arms + CI/Wilcoxon over paired
  /// binary success flags (tie-heavy — the report carries the caveat). Open buckets: win-rate =
  /// mean AIR score + CI/Wilcoxon over paired scores (gbrain = 1 − air by construction).
  fn segment_result<R: rand::Rng>(label: &str, b: &Bucket, k: usize, rng: &mut R) -> SegmentResult {
      if !b.air_ranks.is_empty() {
          let air_flags: Vec<f64> =
              b.air_ranks.iter().map(|r| if success_at_k(r, k) { 1.0 } else { 0.0 }).collect();
          let gb_flags: Vec<f64> =
              b.gbrain_ranks.iter().map(|r| if success_at_k(r, k) { 1.0 } else { 0.0 }).collect();
          let diffs: Vec<f64> = air_flags.iter().zip(&gb_flags).map(|(a, g)| a - g).collect();
          let (ci_low, ci_high) = bootstrap_ci_mean(&diffs, BOOTSTRAP_ITERS, CI_CONF, rng);
          SegmentResult {
              label: label.to_string(),
              n: b.n,
              air_success_at_k: mean_success_at_k(&b.air_ranks, k),
              gbrain_success_at_k: mean_success_at_k(&b.gbrain_ranks, k),
              air_mrr: mean_reciprocal_rank(&b.air_ranks),
              gbrain_mrr: mean_reciprocal_rank(&b.gbrain_ranks),
              air_win_rate: 0.0, // known-item rows are mechanical, not win-rated
              ci_low,
              ci_high,
              wilcoxon: Some(wilcoxon_signed_rank(&air_flags, &gb_flags)),
          }
      } else {
          let air_scores = &b.open_air_scores;
          let gb_scores: Vec<f64> = air_scores.iter().map(|s| 1.0 - s).collect();
          let (ci_low, ci_high) = bootstrap_ci_mean(air_scores, BOOTSTRAP_ITERS, CI_CONF, rng);
          let win_rate = if air_scores.is_empty() {
              0.0
          } else {
              air_scores.iter().sum::<f64>() / air_scores.len() as f64
          };
          SegmentResult {
              label: label.to_string(),
              n: b.n,
              air_success_at_k: 0.0,
              gbrain_success_at_k: 0.0,
              air_mrr: 0.0,
              gbrain_mrr: 0.0,
              air_win_rate: win_rate,
              ci_low,
              ci_high,
              wilcoxon: Some(wilcoxon_signed_rank(air_scores, &gb_scores)),
          }
      }
  }

  /// Up to 5 AIR wins + 5 AIR losses, each with both retrieved contexts (spec §7).
  fn pick_examples(opens: &[OpenRecord], cases: &[QueryCase]) -> Vec<ExamplePair> {
      let mut out = Vec::new();
      let (mut wins, mut losses) = (0usize, 0usize);
      for o in opens {
          let winner = match o.local_verdict {
              Verdict::AirWins if wins < 5 => {
                  wins += 1;
                  "AIR"
              }
              Verdict::GbrainWins if losses < 5 => {
                  losses += 1;
                  "GBrain"
              }
              _ => continue,
          };
          out.push(ExamplePair {
              query: cases[o.case_idx].text.clone(),
              winner: winner.to_string(),
              air_context: o.air_ctx.clone(),
              gbrain_context: o.gbrain_ctx.clone(),
          });
      }
      out
  }
  ```
- [ ] Run: `cargo test -p memharness run` → **PASS** (2 tests).
- [ ] Commit:
  ```
  git add crates/memharness/src/run.rs crates/memharness/src/lib.rs
  git commit -m "$(cat <<'EOF'
feat(memharness): run_queries — per-query loop, bucketing, audit ladder with floor+expansion, egress accounting

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

---

## Task 43 — `main.rs`: CLI + live-seam assembly (RED then GREEN)

- [ ] Add the failing CLI test at the bottom of `main.rs`:
  ```rust
  #[cfg(test)]
  mod cli_tests {
      use super::*;
      use clap::Parser;

      #[test]
      fn parses_run_with_defaults_and_flags() {
          let cli = Cli::parse_from(["memharness", "run"]);
          let Command::Run(args) = cli.command;
          assert_eq!(args.k, 10, "default k (retrieval == scoring)");
          assert_eq!(args.model, memharness::ollama::DEFAULT_OLLAMA_MODEL);
          assert_eq!(args.seed, 42, "default seed");
          assert!(!args.local_only);

          let cli = Cli::parse_from([
              "memharness", "run", "--local-only", "--k", "5", "--model", "llama3:8b",
              "--seed", "7", "--corpus", "/tmp/brain", "--reports-dir", "/tmp/reports",
          ]);
          let Command::Run(args) = cli.command;
          assert!(args.local_only);
          assert_eq!(args.k, 5);
          assert_eq!(args.model, "llama3:8b");
          assert_eq!(args.seed, 7);
          assert_eq!(args.corpus, Some("/tmp/brain".into()));
          assert_eq!(args.reports_dir, Some("/tmp/reports".into()));
      }
  }
  ```
- [ ] Run: `cargo test -p memharness cli` → **FAIL** — then replace `main.rs`'s body (keeping the header) with the full live assembly:
  ```rust
  use std::path::{Path, PathBuf};

  use clap::{Parser, Subcommand};

  use memharness::corpus::CorpusManifest;
  use memharness::frontmatter::Lang;
  use memharness::judge::PairJudge;
  use memharness::run::{cases_from, QueryCase, RunConfig};
  use memharness::synth::{PageRef, QueryGenerator};

  #[derive(Parser)]
  #[command(name = "memharness", about = "DEV-ONLY blind A/B measuring stick: AIR engine vs GBrain.")]
  struct Cli {
      #[command(subcommand)]
      command: Command,
  }

  #[derive(Subcommand)]
  enum Command {
      /// Run one A/B measurement and write a report.
      Run(RunArgs),
  }

  #[derive(clap::Args)]
  struct RunArgs {
      /// Disable ALL cloud egress (no Anthropic audit; wider error bars).
      #[arg(long)]
      local_only: bool,
      /// Local Ollama model for answerer/judge/synth.
      #[arg(long, default_value = memharness::ollama::DEFAULT_OLLAMA_MODEL)]
      model: String,
      /// Retrieval depth AND scoring depth for both arms (one knob: retrieval-k == scoring-k).
      #[arg(long, default_value_t = 10)]
      k: usize,
      /// Corpus source (default ~/brain).
      #[arg(long)]
      corpus: Option<PathBuf>,
      /// Reports output dir (default ~/.air-harness/reports).
      #[arg(long)]
      reports_dir: Option<PathBuf>,
      /// RNG seed for all sampling/blinding/bootstrap.
      #[arg(long, default_value_t = 42)]
      seed: u64,
  }

  fn dirs_home() -> PathBuf {
      std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
  }

  fn main() -> anyhow::Result<()> {
      match Cli::parse().command {
          Command::Run(args) => run(args),
      }
  }

  /// Synthetic-query volume target (spec §3: ~200–400).
  const SYNTH_TARGET: usize = 300;

  fn run(args: RunArgs) -> anyhow::Result<()> {
      let corpus_src = args.corpus.clone().unwrap_or_else(|| dirs_home().join("brain"));
      let reports_dir = args
          .reports_dir
          .clone()
          .unwrap_or_else(|| dirs_home().join(".air-harness").join("reports"));
      let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

      // ── Fail-fast preflights BEFORE any expensive work (spec §5 Rev 2). ──
      memharness::report::ensure_outside_repo(&reports_dir, &repo_root)?;
      memharness::ollama::preflight(&args.model)?;
      let api_key = if args.local_only {
          None
      } else {
          let key = std::env::var("ANTHROPIC_API_KEY")
              .map_err(|_| anyhow::anyhow!("ANTHROPIC_API_KEY not set (or pass --local-only)"))?;
          memharness::anthropic::preflight(&key)?; // one-token call — a bad key/model dies HERE
          Some(key)
      };

      // ── Daemon with the REAL embedder (spec §1 Rev 2 — never the mock on the live path). ──
      let mut daemon = memharness::daemon::HarnessDaemon::spawn_real()?;
      let corpus_home = daemon.home().join("corpus");
      std::fs::create_dir_all(&corpus_home)?;
      let manifest = memharness::corpus::prepare_corpus(
          &corpus_src,
          &corpus_home,
          memharness::corpus::STRIP_FRONTMATTER, // Probe-A-pinned (spec §2 Rev 2)
      )?;
      eprintln!("memharness: corpus prepared — {} pages, {} bytes", manifest.file_count, manifest.total_bytes);

      // ── Ingest over the wire + build the event→page bridge (spec §5 Rev 2). ──
      let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
      let mut client = rt.block_on(memharness::client::WireClient::connect(daemon.socket_path()))?;
      rt.block_on(client.add_grant(&corpus_home))?;
      let ingest = rt.block_on(client.run_ingest())?;
      eprintln!("memharness: ingested {} ({} failed)", ingest.ingested, ingest.failed.len());
      if !ingest.failed.is_empty() {
          anyhow::bail!("{} ingest failures — refusing to score a partial corpus", ingest.failed.len());
      }
      let records = rt.block_on(client.list_files())?;
      let resolver = memharness::resolve::PageResolver::from_file_records(&records, &corpus_home)?;

      // ── Query set: mined real + synthetic (both seeded/deterministic). ──
      let cases = build_query_cases(&args, &manifest, &corpus_home)?;
      eprintln!("memharness: {} query cases built", cases.len());

      // ── Seams. ──
      let mut air = memharness::arms::LiveAirArm::new(rt, client, resolver);
      let gbrain = memharness::arms::GbrainCli;
      let answerer = memharness::arms::OllamaAnswerer { model: args.model.clone() };
      let judge = memharness::judge::OllamaJudge { model: args.model.clone() };
      let auditor = api_key.map(|api_key| memharness::anthropic::AnthropicAuditor { api_key });
      let cfg = RunConfig { k: args.k, seed: args.seed, local_only: args.local_only };

      let outcome = memharness::run::run_queries(
          &cfg,
          &cases,
          &mut air,
          &gbrain,
          &answerer,
          &judge,
          auditor.as_ref().map(|a| a as &dyn PairJudge),
      )?;

      // ── Drift check + report (spec §2/§7 Rev 2). ──
      let (gbrain_version, gbrain_page_count) = memharness::arms::gbrain_version_and_count();
      let drift_fraction = gbrain_page_count.map(|p| {
          (p as f64 - manifest.file_count as f64).abs() / (manifest.file_count.max(1)) as f64
      });
      let report = memharness::report::ReportModel {
          trust: outcome.trust,
          k: args.k,
          segments: outcome.segments,
          corpus: manifest,
          gbrain_version,
          gbrain_page_count,
          drift_fraction,
          ollama_model: args.model.clone(),
          egress_pairs_sent: outcome.egress_pairs_sent,
          local_only: args.local_only,
          near_dedup_applied: false, // exact-only dedup — an EXPLICIT caveat, not a silent gap
          air_pack: outcome.air_pack,
          gbrain_pack: outcome.gbrain_pack,
          examples: outcome.examples,
      };
      let out_dir = memharness::report::write_report(&reports_dir, &repo_root, &report)?;
      eprintln!("memharness: report written to {}", out_dir.display());
      daemon.kill();
      Ok(())
  }

  /// Mined real queries (every transcript under ~/.claude/projects) + seeded stratified
  /// synthetic generation over the prepared corpus.
  fn build_query_cases(
      args: &RunArgs,
      manifest: &CorpusManifest,
      corpus_home: &Path,
  ) -> anyhow::Result<Vec<QueryCase>> {
      // Real: read every *.jsonl under ~/.claude/projects (best-effort per file).
      let mut docs: Vec<String> = Vec::new();
      collect_jsonl(&dirs_home().join(".claude/projects"), &mut docs);
      let mined = memharness::mine::mine_all(docs.iter().map(String::as_str));
      eprintln!("memharness: {} real queries after dedup", mined.len());

      // Synthetic: language per page from its prepared text; seeded stratified selection.
      use rand::SeedableRng;
      let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(args.seed);
      let pages: Vec<PageRef> = manifest
          .entries
          .iter()
          .map(|e| {
              let text = std::fs::read_to_string(corpus_home.join(format!("{}.md", e.page_id)))
                  .unwrap_or_default();
              let lang = match memharness::frontmatter::detect_lang(&text) {
                  Lang::Ko => "ko".to_string(),
                  Lang::En => "en".to_string(),
              };
              PageRef { page_id: e.page_id.clone(), lang }
          })
          .collect();
      let sampled = memharness::synth::stratified_sample(&pages, SYNTH_TARGET, &mut rng);
      let generator = memharness::synth::OllamaQueryGenerator { model: args.model.clone() };
      let mut synth = Vec::with_capacity(sampled.len());
      for page in &sampled {
          let text = std::fs::read_to_string(corpus_home.join(format!("{}.md", page.page_id)))?;
          synth.extend(generator.generate_queries(page, &text)?);
      }
      eprintln!("memharness: {} synthetic queries generated", synth.len());
      Ok(cases_from(mined, synth))
  }

  /// Recursively gather *.jsonl file contents (unreadable files are skipped — transcripts are
  /// best-effort input, and a single unreadable file must not kill the run).
  fn collect_jsonl(dir: &Path, out: &mut Vec<String>) {
      let Ok(entries) = std::fs::read_dir(dir) else { return };
      for entry in entries.flatten() {
          let path = entry.path();
          if path.is_dir() {
              collect_jsonl(&path, out);
          } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
              if let Ok(content) = std::fs::read_to_string(&path) {
                  out.push(content);
              }
          }
      }
  }
  ```
- [ ] Run: `cargo test -p memharness cli` → **PASS**; `cargo run -p memharness -- run --help` prints the flags; `cargo build -p memharness` compiles the FULL live assembly (no stubs — it runs end-to-end given Ollama/gbrain/key).
- [ ] Commit:
  ```
  git add crates/memharness/src/main.rs
  git commit -m "$(cat <<'EOF'
feat(memharness): CLI + full live run() assembly — preflights, real-embedder daemon, bridge, seams, drift, report

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

---

## Task 44 — hermetic e2e #1: daemon plumbing through the REAL resolver (RED→GREEN)

**(Rev 2, finding 2: un-rigged — the gold is found through the real `ListFiles → PageResolver → map_hits → dedup → gold_rank` path, not a hand-built vec.)**

- [ ] Create the mini-corpus fixtures (SYNTHETIC content, WITH frontmatter to exercise stripping):
  - `crates/memharness/tests/fixtures/mini_corpus/en/alpha.md`:
    ```
    ---
    title: Alpha
    ---
    Alpha is the first Greek letter. Ferris the crab studies alpha particles.
    ```
  - `crates/memharness/tests/fixtures/mini_corpus/en/beta.md`:
    ```
    ---
    title: Beta
    ---
    Beta testing is the second phase. The beta release ships on Friday.
    ```
  - `crates/memharness/tests/fixtures/mini_corpus/ko/gamma.md`:
    ```
    ---
    title: Gamma
    ---
    감마는 세 번째 그리스 문자입니다. 감마선은 방사선의 한 종류입니다.
    ```
- [ ] Create `crates/memharness/tests/hermetic_e2e.rs`:
  ```rust
  //! Hermetic e2e #1: mini-corpus → REAL in-process daemon → real wire ingest → ListFiles bridge
  //! → real recall → the REAL resolver path (map_hits → dedup_by_page → gold_rank) → scoring.
  //!
  //! MOCK EMBEDDER: PLUMBING TEST ONLY — quality numbers come from the live run with the real
  //! embedder (spec §1 Rev 2). No Ollama / gbrain / Anthropic anywhere.
  #![cfg(unix)]

  use std::path::Path;

  use memharness::arms::{dedup_by_page, gold_rank, map_hits};
  use memharness::client::WireClient;
  use memharness::corpus::prepare_corpus;
  use memharness::daemon::HarnessDaemon;
  use memharness::resolve::PageResolver;
  use memharness::stats::mean_success_at_k;

  #[tokio::test]
  async fn gold_page_is_found_through_the_real_resolver_path() {
      // 1. Prepare the mini-corpus into the daemon's home (strips frontmatter, manifest).
      let mut daemon = HarnessDaemon::spawn_mock_for_plumbing_tests().unwrap();
      let corpus_home = daemon.home().join("corpus");
      std::fs::create_dir_all(&corpus_home).unwrap();
      let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mini_corpus");
      let manifest = prepare_corpus(&fixture, &corpus_home, true).unwrap();
      assert_eq!(manifest.file_count, 3, "3 mini pages prepared");
      let alpha = std::fs::read_to_string(corpus_home.join("en/alpha.md")).unwrap();
      assert!(!alpha.starts_with("---"), "frontmatter stripped in the prepared copy");

      // 2. Grant + ingest + ListFiles over the REAL wire; build the REAL bridge.
      let mut client = WireClient::connect(daemon.socket_path()).await.unwrap();
      client.add_grant(&corpus_home).await.unwrap();
      let report = client.run_ingest().await.unwrap();
      assert_eq!(report.ingested, 3, "all 3 pages ingested over the wire");
      let records = client.list_files().await.unwrap();
      assert_eq!(records.len(), 3);
      let resolver = PageResolver::from_file_records(&records, &corpus_home).unwrap();

      // 3. Recall → the REAL mapping (fail-loud). k=10 ≥ corpus size, so with the mock
      //    embedder + keyword source the gold page is retrievable; EVERY hit must map (the
      //    no-evolve invariant, exercised live here).
      let wire_hits = client.recall("ferris crab alpha particles", 10).await.unwrap();
      assert!(!wire_hits.is_empty(), "recall returned hits");
      let hits = dedup_by_page(map_hits(&resolver, wire_hits).unwrap());

      // 4. The gold is found through the REAL path — no hand-built vec (Rev 2, finding 2).
      let rank = gold_rank(&hits, "en/alpha");
      assert!(rank.is_some(), "gold page en/alpha retrieved and resolved: {hits:?}");
      assert!((mean_success_at_k(&[rank], 10) - 1.0).abs() < 1e-9);

      drop(client);
      daemon.kill();
  }
  ```
- [ ] Run: `cargo test -p memharness --test hermetic_e2e` → **PASS**.
- [ ] Commit:
  ```
  git add crates/memharness/tests/hermetic_e2e.rs crates/memharness/tests/fixtures/mini_corpus/en/alpha.md crates/memharness/tests/fixtures/mini_corpus/en/beta.md crates/memharness/tests/fixtures/mini_corpus/ko/gamma.md
  git commit -m "$(cat <<'EOF'
test(memharness): hermetic e2e #1 — gold found through the REAL ListFiles→resolver→dedup path

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

---

## Task 45 — hermetic e2e #2: `run_queries` with ALL seams doubled (RED→GREEN)

**(Rev 2, finding 4: the run loop itself — audit ladder, expansion, local-only, egress — proven hermetically.)**

- [ ] Create `crates/memharness/tests/hermetic_run_e2e.rs`:
  ```rust
  //! Hermetic e2e #2: `run_queries` end-to-end with EVERY external seam doubled — AIR retrieval,
  //! GBrain retrieval, answerer, local judge, cloud auditor. Proves the audit ladder (floor
  //! sample → trust → expand-to-100%), the local-only path, and egress accounting, with zero
  //! network anywhere.

  use memharness::arms::{AirRetriever, Answerer, GbrainRetriever, RetrievedHit};
  use memharness::judge::{PairJudge, PosPick};
  use memharness::run::{run_queries, QueryCase, QuerySource, RunConfig};

  struct AirDouble;
  impl AirRetriever for AirDouble {
      fn retrieve(&mut self, _q: &str, _k: usize) -> anyhow::Result<Vec<RetrievedHit>> {
          Ok(vec![RetrievedHit { page_id: "p/air".into(), snippet: "GOODCTX".into() }])
      }
  }
  struct GbrainDouble;
  impl GbrainRetriever for GbrainDouble {
      fn retrieve(&self, _q: &str, _k: usize) -> anyhow::Result<Vec<RetrievedHit>> {
          Ok(vec![RetrievedHit { page_id: "p/gb".into(), snippet: "meh".into() }])
      }
  }
  struct EchoAnswerer;
  impl Answerer for EchoAnswerer {
      fn answer(&self, _q: &str, context: &str) -> anyhow::Result<String> {
          Ok(format!("answer[{context}]"))
      }
  }
  /// Content judge: prefers GOODCTX (blind-compatible — judges answers, not positions).
  struct GoodJudge;
  impl PairJudge for GoodJudge {
      fn pick(&self, _q: &str, a: &str, b: &str) -> anyhow::Result<Option<PosPick>> {
          Ok(match (a.contains("GOODCTX"), b.contains("GOODCTX")) {
              (true, false) => Some(PosPick::A),
              (false, true) => Some(PosPick::B),
              _ => Some(PosPick::Tie),
          })
      }
  }
  /// Contrarian auditor: prefers the answer WITHOUT GOODCTX → 0% agreement with GoodJudge.
  struct ContrarianAuditor;
  impl PairJudge for ContrarianAuditor {
      fn pick(&self, _q: &str, a: &str, b: &str) -> anyhow::Result<Option<PosPick>> {
          Ok(match (a.contains("GOODCTX"), b.contains("GOODCTX")) {
              (true, false) => Some(PosPick::B),
              (false, true) => Some(PosPick::A),
              _ => Some(PosPick::Tie),
          })
      }
  }

  fn open_cases(n: usize) -> Vec<QueryCase> {
      (0..n)
          .map(|i| QueryCase {
              text: format!("open query {i}"),
              lang: "en".into(),
              source: QuerySource::Real,
              gold_page_id: None,
          })
          .collect()
  }

  #[test]
  fn agreeing_auditor_trusts_at_the_floor_sample() {
      // 40 open queries > AUDIT_FLOOR → the initial sample is max(30, 6) = 30 pairs; agreement
      // is perfect → trusted, NO expansion, egress == 30.
      let cfg = RunConfig { k: 10, seed: 42, local_only: false };
      let outcome = run_queries(
          &cfg, &open_cases(40), &mut AirDouble, &GbrainDouble, &EchoAnswerer, &GoodJudge,
          Some(&GoodJudge),
      )
      .unwrap();
      assert!(outcome.trust.trusted);
      assert!(!outcome.trust.expanded_to_full_audit);
      assert!(!outcome.trust.audit_n_too_small, "pool 40 ≥ floor 30");
      assert_eq!(outcome.trust.audited_count, 30, "the max(30, 15%·40)=30 sample");
      assert_eq!(outcome.egress_pairs_sent, 30);
      // AIR won every pair (GOODCTX on the AIR side) → win rate 1.0.
      assert!((outcome.segments[0].air_win_rate - 1.0).abs() < 1e-9);
  }

  #[test]
  fn disagreeing_auditor_forces_full_expansion_and_flags_untrusted() {
      let cfg = RunConfig { k: 10, seed: 42, local_only: false };
      let outcome = run_queries(
          &cfg, &open_cases(40), &mut AirDouble, &GbrainDouble, &EchoAnswerer, &GoodJudge,
          Some(&ContrarianAuditor),
      )
      .unwrap();
      assert!(!outcome.trust.trusted, "0% agreement can never trust");
      assert!(outcome.trust.expanded_to_full_audit, "auto-expanded to 100% (spec §5)");
      assert_eq!(outcome.trust.audited_count, 40, "every open pair audited after expansion");
      assert_eq!(outcome.egress_pairs_sent, 40);
  }

  #[test]
  fn local_only_never_egresses() {
      let cfg = RunConfig { k: 10, seed: 42, local_only: true };
      let outcome = run_queries(
          &cfg, &open_cases(40), &mut AirDouble, &GbrainDouble, &EchoAnswerer, &GoodJudge, None,
      )
      .unwrap();
      assert!(outcome.trust.audit_incomplete, "no audit this run");
      assert_eq!(outcome.egress_pairs_sent, 0);
  }
  ```
- [ ] Run: `cargo test -p memharness --test hermetic_run_e2e` → **PASS** (3 tests).
- [ ] Commit:
  ```
  git add crates/memharness/tests/hermetic_run_e2e.rs
  git commit -m "$(cat <<'EOF'
test(memharness): hermetic e2e #2 — run_queries audit ladder, expansion, local-only, egress (all seams doubled)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

---

## Task 46 — Gates + LIVE-RUN runbook + PR prep

- [ ] Run the full gate set; every line must be green:
  ```
  cargo test -p memharness
  cargo clippy -p memharness --all-targets -- -D warnings
  cargo test -p bossclawd                     # the touched crate's own suite (incl. the Task 13 test)
  cargo test --workspace                      # no regressions anywhere
  cargo check --workspace                     # fresh-checkout compile unaffected
  ```
  Clippy findings are fixed, not `#[allow]`'d (any allow needs a one-line justification comment).
- [ ] **(Rev 2, finding 3) Shipped-daemon feature-leak gate:** the scoped build the ship script uses must compile the helpers OUT:
  ```
  cargo build -p bossclawd
  nm target/debug/bossclawd 2>/dev/null | grep -ci 'test_engine\|spawn_for_test\|seed_secret_cache' || echo 0
  ```
  Expect the scoped build to succeed and the symbol count to print `0`. The authoritative proof is the compile-time cfg: every helper is `#[cfg(any(test, feature = "test-helpers"))]`, and a `-p bossclawd` build resolves features WITHOUT memharness's request (feature unification only merges the packages selected into the same build) — the `nm` grep is a belt-and-braces check (mangled names may hide matches; `0` is expected, but the cfg is the proof).
- [ ] Verify the crate is NOT in any release/bundle manifest:
  ```
  grep -rn "memharness" apps/desktop/src-tauri/tauri.conf.json 2>/dev/null || echo "not bundled — good"
  ```
- [ ] Document the **LIVE-RUN runbook** in the PR description (NOT a committed doc — machine-specific, Peter-gated):
  1. Ollama running with the model: `ollama serve` + `ollama pull qwen2.5:7b` (or the Probe-B tag).
  2. Embedder model present: `ls apps/desktop/src-tauri/resources/models/potion-base-8M/model.safetensors` (else `scripts/fetch-model.sh`) — the run preflights this and fails with the same instruction.
  3. `gbrain` on PATH and its brain synced (the harness does NOT re-sync; drift >5% renders the INVALID-RUN banner).
  4. `export ANTHROPIC_API_KEY=…` (skip for `--local-only`); the run preflights it with a one-token call.
  5. Run: `cargo run -p memharness -- run` (defaults: `~/brain`, k=10, seed=42, hybrid audit). Expect ≤~2h.
  6. Read `~/.air-harness/reports/<ts>/report.md`. Confirm: no INVALID-RUN banner, judge-trust verdict leads (agreement+kappa vs ≥85%/≥0.6), EN/KO split, ≥100 real + ≥200 synthetic queries, per-arm pack stats, corpus manifest in scores.json, zero repo files with brain content (`git status -s` clean of reports).
  7. Optional: `cargo run -p memharness -- run --local-only` for a $0 sanity pass.
- [ ] Confirm the acceptance boundary: **harness ready + smoke-tested hermetically; the live baseline run is the acceptance demo** (it needs Peter's machine state and is not automatable here).
- [ ] Final commit (paste the gate outputs' pass counts into the body):
  ```
  git add -A
  git commit -m "$(cat <<'EOF'
chore(memharness): gates green (memharness + bossclawd + workspace + scoped no-feature build)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```
- [ ] PR prep (do NOT push unless asked): title `feat(memharness): Phase 0 blind A/B measuring stick (AIR vs GBrain) — dev-only`; body = the runbook + the "never ships / reports never committed / real embedder on the live path" callouts + the Rev 2 review-fix summary table.

---

## Self-review (Rev 2)

### Spec coverage checklist (spec Rev 2 § → task)

| Spec section (Rev 2) | Requirement | Task(s) |
|---|---|---|
| §1 crate+isolation | Dev-only lib+bin crate, `#![forbid(unsafe_code)]`, never ships | 2 |
| §1 (Rev 2) | test-helpers as a RUNTIME dep, rationale documented, scoped-build leak gate | 2 (Cargo.toml), 46 (gate) |
| §1 (Rev 2, CRIT) | REAL embedder on the live path; `test_engine_with_embedder`; model-dir resolution + preflight; mock = plumbing-only with explicit comment | 13–14 (bossclawd seam), 15–16 (spawn_real/resolve/preflight), 17/44 (plumbing-only comments) |
| §1 | In-process daemon, private 0600 socket, per-run temp home, real wire ops | 15–16 (daemon), 17–18 (client), 44 (e2e) |
| §2 | Corpus copy, dot-skip, sha256 manifest, snapshot ts | 7–10 |
| §2 (Rev 2) | Frontmatter strip CONDITIONAL on Probe A | 1 (probe), 9–10 (`strip` flag + both-branch tests), 43 (`STRIP_FRONTMATTER` wired) |
| §2 (Rev 2) | Drift >5% → INVALID-RUN banner (not a footnote) | 36 (`gbrain_version_and_count`), 39–40 (`DRIFT_INVALID_FRACTION` + banner-first render + test), 43 (drift computed) |
| §3 | Mine real queries + within-5 implicit labels + dedup | 1 (Probe C), 21–22 (incl. cross-file `mine_all`; O(n·m) note as code comment — finding 15) |
| §3 | Synthetic known-item, stratified by category + language, source=gold, ~200–400 | 37–38, 43 (`SYNTH_TARGET = 300`) |
| §3 | Segment tags real/synthetic × en/ko × known/open | 42 (`bucket_label`), 41 (test asserts all three buckets) |
| §4 (Rev 2) | Retrieval-k == scoring-k, one knob, stated | 24 (docs), 42 (`RunConfig` docs), 40 (headline renders it), 43 (one `--k`) |
| §4 (Rev 2) | GBrain arm balanced-mode argv Probe-A-pinned | 1, 36 (`GbrainCli` argv comment) |
| §4 | gbrain parse failure = run error, never silent | 36 (`parse_gbrain_output` bails; subprocess exit checked) |
| §4 (Rev 2, MAJ 9) | Context budget = k hits; identical per-snippet cap; per-arm PackStats + granularity note | 35–36 (`pack_context` + test incl. KO multibyte), 40 (render + note), 42 (totals accumulated) |
| §4 | Identical answerer both arms; no GBrain cloud ask | 36 (`Answerer` seam + `OllamaAnswerer`), 42 (ONE answerer for both contexts) |
| §5 (Rev 2, CRIT 2) | event_id→page_id bridge via ListFiles; invariant in code; FAIL-LOUD, no fallback; dedup by page before ranking | 17–18 (`list_files`), 19–20 (`PageResolver` + invariant docs + loud test), 35–36 (`dedup_by_page`, `map_hits`), 42 (dedup applied both arms), 44 (un-rigged e2e through the real path) |
| §5 | Known-item mechanical success@k + MRR | 23–24, 42 (`segment_result`) |
| §5 | Blind, position-swapped local judge; disagreement/ambiguity → uncertain | 31–32 (`assign_blind`/`deblind_pick`/`resolve_swap`/`judge_pair_blind` + tests both blind values) |
| §5 (Rev 2) | Audit floor `max(30, 15%)` ∪ ALL uncertains, deduped; pool<30 → audit all + "indicative only" | 31–32 (`select_audit_indices` + floor test), 29–30 (`audit_n_too_small` flag), 40 (render), 45 (floor + expansion e2e) |
| §5 (Rev 2, MAJ 6) | Anthropic one-token preflight before the loop | 1 (Probe D), 33–34 (`preflight`), 43 (wired before any expensive work) |
| §5 | Trust verdict agreement≥85% AND kappa≥0.6; below → expand to 100% + plain statement | 29–30 (thresholds + trust_verdict), 42 (expansion ladder), 45 (contrarian-auditor e2e), 40 (render) |
| §5 | Audit failure → "incomplete", never fabricate | 30 (flag), 42 (`audit_set` stops + flags), 34 (strict parse; ambiguous→Uncertain not fabricated) |
| §5/§8 | Bootstrap CIs; Wilcoxon; small-n honesty | 25–26, 27–28 (incl. tie-heavy fixtures w/ verified values + 1.96 sanity — finding 5), 40 (caveat lines) |
| §6 | Hybrid default via env key; `--local-only` = zero egress; egress counted per pair; GBrain's own egress noted | 43 (key/flag), 41/45 (egress asserts incl. zero), 40 (render), 42 (counting) |
| §7 | Reports outside the repo; structure order; examples with context diffs | 39–40 (guard hardened — finding 7; order-asserted render; `pick_examples` in 42) |
| §8 (Rev 2) | Binary-flag Wilcoxon tie caveat; HNSW note; drift handling | 28 (docs), 40 (caveat render), 42 (fixed query order comment) |
| Acceptance 1–5 | One-command ≤2h; report contents; zero live writes; local-only; real wire | 43 (CLI+assembly), 46 (runbook), 15–18/44 (wire, isolated home) |
| Acceptance 6 (Rev 2) | Production embedder on the live path | 13–16, 43 (`spawn_real` is the ONLY constructor main uses) |
| Acceptance 7 (Rev 2) | Unmapped hit fails loud | 19–20, 36 (`map_hits`), 44 (live-exercised) |
| Open questions | Model default + availability check; gbrain format pinned; <50-real weighting decision | 1 + 11–12; 1 + 36; 1 (Probe C decision) + 40 (`near_dedup_applied` caveat) |

Every Rev 2 spec requirement and every punch-list finding maps to at least one task; the change-summary table at the top cross-references finding → task.

### Placeholder scan

- No `TODO`, `TBD`, `todo!()`, `unimplemented!()`, `.skip`/`.only`, or "similar to Task N" in any code block. Every referenced type/function is defined in-plan at a named task.
- `run()` is FULL code (Task 43) — the per-query loop lives in `run_queries` (Task 42, full code), hermetically proven in Tasks 41/45. No stubbed loops.
- Deliberate deferrals are all REPORTED, not silent: near-dup dedup (`near_dedup_applied: false` caveat), Wilcoxon exact-table (small-n + tie caveats rendered), gbrain page-count parse (None → "drift unknown" in the report, never a guess).
- Probe-pinned surfaces are explicitly marked at their code sites: `STRIP_FRONTMATTER`, `DEFAULT_OLLAMA_MODEL`, `AUDIT_MODEL`, `GbrainCli` argv, `parse_gbrain_output`, the `gbrain stats` count parse.

### Type-consistency check (across the new seams)

- `RetrievedHit`/`PackStats`/`AirRetriever`/`GbrainRetriever`/`Answerer`/`map_hits`/`dedup_by_page`/`gold_rank` — defined Task 36; consumed by run.rs (42), e2e #1 (44), e2e #2 (45).
- `PosPick`/`PairJudge`/`parse_pick_token`/`pairwise_prompt`/`Blind`/`assign_blind`/`deblind_pick`/`resolve_swap`/`judge_pair_blind`/`select_audit_indices`/`AUDIT_FLOOR` — defined Task 32; consumed by anthropic.rs (34: `AnthropicAuditor: PairJudge`), run.rs (42), tests (31/41/45).
- `Verdict`/`TrustVerdict`/`trust_verdict` (5-arg Rev 2 signature incl. `audit_n_too_small`) — defined Task 30; consumed by 42 (`trust_from`), 40 (render), tests.
- `PageResolver::{from_file_records, page_id_of}` — defined Task 20; consumed by `LiveAirArm`/`map_hits` (36), main (43), e2e #1 (44).
- `FileRecordMirror`/`IngestReportMirror` — real proto types (fields verified against `types.rs`); consumed by client (18), resolver (20), tests (17/19).
- `test_engine_with_embedder(PathBuf, Arc<dyn EmbedderProvider>) -> EngineHandle` — defined Task 14 (matches `EngineHandle::new(vault, data_dir, embedder_provider, reasoner_provider)`, verified); consumed by daemon (16), bossclawd test (13).
- `ResourceModel2Vec::new(PathBuf)` + `EmbedderProvider` — real bossclawd types (verified pub via `pub mod embed`); consumed by `spawn_real` (16).
- `QueryCase`/`QuerySource`/`RunConfig`/`RunOutcome`/`cases_from` — defined Task 42; consumed by main (43), e2e #2 (45), tests (41).
- `SegmentResult`/`PackTotals`/`ExamplePair`/`ReportModel`/`ensure_outside_repo`/`write_report`/`DRIFT_INVALID_FRACTION` — defined Task 40; consumed by run.rs (42), main (43).
- `MinedQuery`/`mine_all` (22), `SynthQuery`/`PageRef`/`stratified_sample`/`QueryGenerator` (38), `Lang`/`detect_lang` (6), `CorpusManifest`/`prepare_corpus(src,dst,strip)` (10), `WilcoxonResult` (28: now `Serialize`, required by `SegmentResult`) — all cross-referenced consistently.
- All RNG uses are `ChaCha8Rng::seed_from_u64(seed)` threads: blinding + audit selection + bucket CIs share ONE rng in `run_queries` (42); synth sampling seeds its own from the same `--seed` (43); every seeded test asserts determinism.






