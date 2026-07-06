# Retrieval Floor Phase 1 — Rung 0 (freeze tooling) + Rung 1 (query tokenization + fetch cap) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the frozen-measurement tooling (spec §3.0/§3.1) and the two engine quick wins (spec §3.2), ending with the frozen Phase 1 baseline and rung 1's measured gate verdict.

**Architecture:** Rung 0 extends the dev-only `memharness` crate: serde-persistable `QueryCase` lists (JSONL + sha identity), per-case mechanical results in `scores.json`, three CLI flags (`--known-item-only`, `--save-cases`, `--cases`), snapshot/case-list identity rendered in the report, and a `compare` subcommand that pairs two runs' per-case AIR success flags over an identical frozen case list and runs the existing Wilcoxon. Rung 1 changes `bossclaw-core`: `escape_fts_query` emits per-term quoted `OR` queries (injection safety preserved — every user token stays inside quotes) and `FUSION_FETCH` rises 50→200. Rungs 2–3 are deliberately NOT in this plan (they depend on rung 1's measurement + rung 2's model probe).

**Tech Stack:** Rust 2021. Zero new dependencies (serde/serde_json/sha2/clap already in memharness; rung 1 touches only bossclaw-core internals).

**Spec:** docs/superpowers/specs/2026-07-06-retrieval-floor-phase1-design.md (Rev 2, dual-reviewed)

---

## Preconditions

- **PR #72 (`feat-memharness-cloud-judge`) MUST merge to main before Task 1** — rung 0 modifies the same `main.rs` regions (`RunArgs`, the key-skip condition, `JudgeMode`). Peter-gated.
- **Rung 0 branch:** `feat-memharness-freeze-tooling` off post-#72 main. **Rung 1 branch:** `feat-retrieval-rung1` off post-rung-0 main (its Task 14 A/B needs the merged rung 0 harness in-tree).
- Verify at start: `git status -sb` clean; `cargo test -p memharness` green on the branch base.
- The frozen corpus snapshot + baseline run (Task 10) run on Peter's machine: Ollama up (`qwen2.5:7b-instruct`), embedder model present, `gbrain` on PATH. No ANTHROPIC key needed anywhere in this plan (`--known-item-only` skips it).

## File structure

| File | Responsibility |
|---|---|
| `crates/memharness/src/cases.rs` (new) | Frozen case-list persistence: JSONL save/load + sha identity. Pure I/O+serde, no run logic. |
| `crates/memharness/src/compare.rs` (new) | Paired cross-run comparison: extract per-case results from two scores.json, enforce same case-list sha, per-segment paired Wilcoxon. Pure; printing stays in main.rs. |
| `crates/memharness/src/run.rs` | `QueryCase`/`QuerySource` gain serde; new `CaseResult`; `run_queries` records per-case known-item results. |
| `crates/memharness/src/corpus.rs` | New `manifest_sha` (snapshot identity). |
| `crates/memharness/src/report.rs` | `ReportModel` gains `corpus_sha`/`case_list_sha`/`case_results`; render gains the identity line. |
| `crates/memharness/src/main.rs` | Three new flags + validations, `build_query_cases` save/load seam, known-item filter, `compare` subcommand dispatch. |
| `crates/memharness/src/lib.rs` | `pub mod cases;` + `pub mod compare;` lines. |
| `crates/bossclaw-core/src/keyword.rs` | Per-term OR emission (rung 1). |
| `crates/bossclaw-core/src/recall.rs` | `FUSION_FETCH` 200 (rung 1). |

---

# RUNG 0 — branch `feat-memharness-freeze-tooling`

### Task 1: `cases.rs` — save/load/sha (RED)

**Files:**
- Create: `crates/memharness/src/cases.rs`
- Modify: `crates/memharness/src/lib.rs` (add `pub mod cases;` after `pub mod arms;`)
- Modify: `crates/memharness/src/run.rs` (serde derives — needed for the test to compile)

- [ ] **Step 1: Add serde derives in `run.rs`.** Change the two derive lines exactly:

```rust
/// Segment tag: where a query came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum QuerySource {
    Real,
    Synthetic,
}

/// One query case. `gold_page_id`: Some = known-item (mechanical), None = open (judged).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct QueryCase {
    pub text: String,
    pub lang: String, // "en" | "ko"
    pub source: QuerySource,
    pub gold_page_id: Option<String>,
}
```

- [ ] **Step 2: Create `cases.rs` with ONLY the module doc + tests** (functions come in Task 2):

```rust
//! Frozen query-case persistence (spec §3.0.2): save the built case list once, reload it on
//! every later rung run, so all Phase 1 gates compare IDENTICAL cases. Format = JSONL (one
//! `QueryCase` per line) + sha256 over the exact file bytes — the case-list identity the report
//! records and `compare` (spec §3.0.3) enforces before pairing runs.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::{QueryCase, QuerySource};

    fn sample() -> Vec<QueryCase> {
        vec![
            QueryCase {
                text: "memharness probe findings".into(),
                lang: "en".into(),
                source: QuerySource::Real,
                gold_page_id: Some("air/session-start-protocol".into()),
            },
            QueryCase {
                text: "메모리 하니스는 무엇인가?".into(),
                lang: "ko".into(),
                source: QuerySource::Synthetic,
                gold_page_id: None,
            },
        ]
    }

    #[test]
    fn round_trips_identically_with_stable_sha() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("frozen/cases.jsonl"); // parent must be auto-created
        let saved_sha = save_cases(&path, &sample()).unwrap();
        let (loaded, loaded_sha) = load_cases(&path).unwrap();
        assert_eq!(loaded_sha, saved_sha, "sha identity survives the round trip");
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].text, "memharness probe findings");
        assert_eq!(loaded[0].gold_page_id.as_deref(), Some("air/session-start-protocol"));
        assert_eq!(loaded[1].text, "메모리 하니스는 무엇인가?", "KO text byte-identical");
        assert!(matches!(loaded[1].source, QuerySource::Synthetic));
        assert!(loaded[1].gold_page_id.is_none());
        // Saving the same list again produces the same bytes → same sha (determinism).
        let path2 = dir.path().join("again.jsonl");
        assert_eq!(save_cases(&path2, &sample()).unwrap(), saved_sha);
    }

    #[test]
    fn zero_cases_and_corrupt_lines_fail_loud() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty.jsonl");
        std::fs::write(&empty, "").unwrap();
        let err = load_cases(&empty).unwrap_err().to_string();
        assert!(err.contains("zero cases"), "empty frozen list is always a mistake: {err}");

        let corrupt = dir.path().join("corrupt.jsonl");
        std::fs::write(&corrupt, "{\"text\":\"ok\",\"lang\":\"en\",\"source\":\"Real\",\"gold_page_id\":null}\nnot json\n").unwrap();
        let err = load_cases(&corrupt).unwrap_err().to_string();
        assert!(err.contains("line 2"), "corrupt line is named: {err}");

        let missing = dir.path().join("nope.jsonl");
        assert!(load_cases(&missing).is_err(), "missing file errors");
    }
}
```

- [ ] **Step 3: Run to verify RED:** `cargo test -p memharness cases::` — Expected: FAIL to compile (`save_cases`/`load_cases` not found).
- [ ] **Step 4: Commit** — `git add crates/memharness/src/cases.rs crates/memharness/src/lib.rs crates/memharness/src/run.rs && git commit -m "test(memharness): frozen case-list persistence — round-trip + sha identity + fail-loud (RED)"`

### Task 2: `cases.rs` — implement (GREEN)

**Files:** Modify: `crates/memharness/src/cases.rs` (insert implementation between the module doc and `#[cfg(test)]`)

- [ ] **Step 1: Implementation:**

```rust
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::run::QueryCase;

/// Serialize cases as JSONL bytes (one JSON object per line). Field order = struct order, so
/// the same list always produces the same bytes (the sha below is a real identity).
fn cases_to_jsonl(cases: &[QueryCase]) -> anyhow::Result<Vec<u8>> {
    let mut out = Vec::new();
    for case in cases {
        serde_json::to_writer(&mut out, case)?;
        out.push(b'\n');
    }
    Ok(out)
}

/// sha256 hex over exact JSONL bytes — the case-list identity recorded in the report.
fn jsonl_sha(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// Save cases as JSONL (creating parent dirs); returns the sha of the bytes written.
pub fn save_cases(path: &Path, cases: &[QueryCase]) -> anyhow::Result<String> {
    let bytes = cases_to_jsonl(cases)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, &bytes)?;
    Ok(jsonl_sha(&bytes))
}

/// Load a frozen case list; returns (cases, sha of the bytes read). A zero-case file fails loud
/// — a frozen set with nothing in it is always a mistake, never a valid measurement input.
pub fn load_cases(path: &Path) -> anyhow::Result<(Vec<QueryCase>, String)> {
    let bytes = std::fs::read(path)
        .map_err(|e| anyhow::anyhow!("reading frozen case list {}: {e}", path.display()))?;
    let sha = jsonl_sha(&bytes);
    let mut cases = Vec::new();
    for (i, line) in bytes.split(|b| *b == b'\n').enumerate() {
        if line.is_empty() {
            continue;
        }
        let case: QueryCase = serde_json::from_slice(line).map_err(|e| {
            anyhow::anyhow!("frozen case list {} line {}: {e}", path.display(), i + 1)
        })?;
        cases.push(case);
    }
    if cases.is_empty() {
        anyhow::bail!("frozen case list {} contains zero cases", path.display());
    }
    Ok((cases, sha))
}
```

- [ ] **Step 2: Run:** `cargo test -p memharness cases::` — Expected: 2 passed.
- [ ] **Step 3: Full crate:** `cargo test -p memharness` — Expected: all green (67 + 2 new).
- [ ] **Step 4: Commit** — `git add -u && git commit -m "feat(memharness): frozen case-list persistence — JSONL + sha identity (GREEN)"`

### Task 3: `run.rs` — `CaseResult` + per-case accumulation (RED)

**Files:** Modify: `crates/memharness/src/run.rs`

- [ ] **Step 1: Extend the existing test** `run_queries_buckets_judges_audits_and_counts_egress` — append at its end (after the pack-totals asserts):

```rust
        // Per-case mechanical results (spec §3.0.3): exactly the ONE known-item case, with
        // ranks/flags matching the bucket math; opens are NOT recorded here.
        assert_eq!(outcome.case_results.len(), 1);
        let cr = &outcome.case_results[0];
        assert_eq!(cr.case_idx, 0, "case identity = index into the (frozen) case list");
        assert_eq!(cr.label, "synthetic·en·known-item");
        assert_eq!(cr.air_rank, Some(0));
        assert_eq!(cr.gbrain_rank, None);
        assert!(cr.air_success && !cr.gbrain_success);
```

- [ ] **Step 2: Verify RED:** `cargo test -p memharness run_queries_buckets` — Expected: FAIL to compile (`case_results` not on `RunOutcome`).
- [ ] **Step 3: Commit** — `git add -u && git commit -m "test(memharness): per-case known-item results on RunOutcome (RED)"`

### Task 4: `run.rs` — implement per-case results (GREEN)

**Files:** Modify: `crates/memharness/src/run.rs`

- [ ] **Step 1: Add the type** (directly below the `RunConfig` block):

```rust
/// One known-item case's mechanical result — persisted in scores.json so a later run over the
/// SAME frozen case list can be compared PAIRED by `case_idx` (spec §3.0.3). Open cases have no
/// mechanical result and never appear here.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CaseResult {
    pub case_idx: usize,
    pub label: String,
    pub air_rank: Option<usize>,
    pub gbrain_rank: Option<usize>,
    pub air_success: bool,
    pub gbrain_success: bool,
}
```

- [ ] **Step 2: Add the field to `RunOutcome`** (after `segments`): `pub case_results: Vec<CaseResult>,`
- [ ] **Step 3: Accumulate.** In `run_queries`, add `let mut case_results: Vec<CaseResult> = Vec::new();` beside the other accumulators, and replace the known-item match arm body:

```rust
            // Known-item: mechanical success@k/MRR, no judge (spec §5) — recorded per-case for
            // paired cross-run comparison (spec §3.0.3).
            Some(gold) => {
                let air_rank = gold_rank(&air_hits, gold);
                let gbrain_rank = gold_rank(&gbrain_hits, gold);
                case_results.push(CaseResult {
                    case_idx,
                    label: bucket_label(case),
                    air_rank,
                    gbrain_rank,
                    air_success: success_at_k(&air_rank, cfg.k),
                    gbrain_success: success_at_k(&gbrain_rank, cfg.k),
                });
                bucket.air_ranks.push(air_rank);
                bucket.gbrain_ranks.push(gbrain_rank);
            }
```

(`success_at_k` is already imported at the top of run.rs.)
- [ ] **Step 4: Return it** — add `case_results,` to the `Ok(RunOutcome { ... })` literal.
- [ ] **Step 5: Run:** `cargo test -p memharness` — Expected: the Task 3 asserts pass; the two hermetic e2e files call `run_queries` and only read existing fields, so they stay green.
- [ ] **Step 6: Commit** — `git add -u && git commit -m "feat(memharness): record per-case known-item results for paired cross-run stats (GREEN)"`

### Task 5: `report.rs` + `corpus.rs` — snapshot/case identities (RED)

**Files:** Modify: `crates/memharness/src/report.rs`, `crates/memharness/src/corpus.rs`

- [ ] **Step 1: corpus test** — append inside `corpus.rs`'s `mod tests`:

```rust
    #[test]
    fn manifest_sha_is_stable_and_content_sensitive() {
        let m = CorpusManifest {
            snapshot_unix_secs: 1,
            file_count: 2,
            total_bytes: 10,
            entries: vec![
                ManifestEntry { page_id: "a".into(), sha256: "s1".into(), bytes: 5 },
                ManifestEntry { page_id: "b".into(), sha256: "s2".into(), bytes: 5 },
            ],
        };
        let sha = manifest_sha(&m);
        assert_eq!(sha.len(), 64, "sha256 hex");
        assert_eq!(sha, manifest_sha(&m), "deterministic");
        let mut m2 = m.clone();
        m2.entries[1].sha256 = "s3".into();
        assert_ne!(manifest_sha(&m2), sha, "content change changes the id");
        let mut m3 = m.clone();
        m3.snapshot_unix_secs = 999; // copy TIME must not change identity
        assert_eq!(manifest_sha(&m3), sha, "snapshot time is not part of identity");
    }
```

(If `CorpusManifest`/`ManifestEntry` lack `Clone`, add `Clone` to their derive lists — additive.)
- [ ] **Step 2: report tests** — in `report.rs`'s tests, append to `renders_trust_first_drift_banner_and_caveats` (before the golden assert):

```rust
        assert!(
            md.contains("Corpus snapshot: "),
            "snapshot identity rendered (spec §3.0.1)"
        );
        assert!(
            md.contains("UNFROZEN"),
            "ad-hoc case list is loudly labeled not-comparable: {md}"
        );
```

and add a new test after `cloud_judge_egress_is_disclosed_when_set`:

```rust
    #[test]
    fn frozen_case_list_sha_is_rendered_and_cases_land_in_scores_json() {
        let mut report = ReportModel::sample_for_test();
        report.case_list_sha = Some("f".repeat(64));
        report.case_results = vec![crate::run::CaseResult {
            case_idx: 0,
            label: "synthetic·en·known-item".into(),
            air_rank: Some(0),
            gbrain_rank: None,
            air_success: true,
            gbrain_success: false,
        }];
        let md = render_markdown(&report);
        assert!(md.contains("frozen case list: ffffffffffffffff"), "16-char sha prefix: {md}");
        assert!(!md.contains("UNFROZEN"));
        assert!(
            !md.contains("case_idx"),
            "per-case results are scores.json-only, never markdown"
        );
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["case_results"][0]["case_idx"], 0, "per-case results serialize");
        assert_eq!(json["corpus_sha"], report.corpus_sha);
    }
```

- [ ] **Step 3: Verify RED:** `cargo test -p memharness report:: corpus::` — Expected: FAIL to compile (missing fields/fn).
- [ ] **Step 4: Commit** — `git add -u && git commit -m "test(memharness): snapshot + case-list identity in report/scores (RED)"`

### Task 6: `report.rs` + `corpus.rs` — implement identities (GREEN)

**Files:** Modify: `crates/memharness/src/corpus.rs`, `crates/memharness/src/report.rs`

- [ ] **Step 1: `corpus.rs`** — add near `CorpusManifest`:

```rust
/// Snapshot identity (spec §3.0.1): sha256 over each entry's `page_id\0sha256\n` in manifest
/// order (entries are already sorted). Deliberately EXCLUDES `snapshot_unix_secs` — two copies
/// of byte-identical corpora taken at different times are the SAME snapshot; gates may only
/// compare runs whose ids match.
pub fn manifest_sha(manifest: &CorpusManifest) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for e in &manifest.entries {
        hasher.update(e.page_id.as_bytes());
        hasher.update([0]);
        hasher.update(e.sha256.as_bytes());
        hasher.update([b'\n']);
    }
    format!("{:x}", hasher.finalize())
}
```

- [ ] **Step 2: `ReportModel` fields** — after `drift_fraction`:

```rust
    /// Corpus snapshot identity (`corpus::manifest_sha`) — gates only compare equal ids (§3.0.1).
    pub corpus_sha: String,
    /// sha of the frozen case-list JSONL when `--cases`/`--save-cases` ran; None = ad-hoc
    /// generation, loudly labeled UNFROZEN (not comparable across runs).
    pub case_list_sha: Option<String>,
    /// Per-known-item-case mechanical results (spec §3.0.3) — scores.json only, never markdown.
    pub case_results: Vec<crate::run::CaseResult>,
```

- [ ] **Step 3: Render** — insert directly after the `GBrain pipeline:` push_str block:

```rust
    let short = |s: &str| s.chars().take(16).collect::<String>();
    s.push_str(&format!(
        "Corpus snapshot: {}{} (gates only compare runs with matching identities — spec §3.0)\n\n",
        short(&r.corpus_sha),
        match &r.case_list_sha {
            Some(sha) => format!(" · frozen case list: {}", short(sha)),
            None => " · case list: ad-hoc (UNFROZEN — not comparable across runs)".to_string(),
        },
    ));
```

- [ ] **Step 4: Fixture** — in `sample_for_test()`, after `drift_fraction: Some(0.01),`:

```rust
                corpus_sha: "ab".repeat(32),
                case_list_sha: None,
                case_results: vec![],
```

- [ ] **Step 5: Run:** `cargo test -p memharness` — Expected: all green (golden re-renders through the same function, so it self-updates).
- [ ] **Step 6: Commit** — `git add -u && git commit -m "feat(memharness): corpus snapshot + frozen case-list identities in report/scores (GREEN)"`

### Task 7: `main.rs` — flags, validations, wiring (RED then GREEN)

**Files:** Modify: `crates/memharness/src/main.rs`

- [ ] **Step 1 (RED): extend `cli_tests`** — append inside `parses_run_with_defaults_and_flags` (after the `--judge cloud` block):

```rust
        let cli = Cli::parse_from([
            "memharness", "run", "--known-item-only", "--cases", "/tmp/frozen.jsonl",
        ]);
        let Command::Run(args) = cli.command;
        assert!(args.known_item_only);
        assert_eq!(args.cases, Some("/tmp/frozen.jsonl".into()));
        assert!(args.save_cases.is_none());

        // --save-cases and --cases contradict (regenerate vs load) — clap rejects the pair.
        assert!(Cli::try_parse_from([
            "memharness", "run", "--save-cases", "/tmp/a.jsonl", "--cases", "/tmp/b.jsonl",
        ])
        .is_err());
```

NOTE: keep the existing plain destructuring form `let Command::Run(args) = cli.command;` here — while `Command` has a single variant, a `let … else` would trip the `irrefutable_let_patterns` lint under `-D warnings`. Task 9 converts ALL these destructurings to `let … else` in the same step that adds the `Compare` variant (which makes the pattern refutable).
- [ ] **Step 2: Verify RED:** `cargo test -p memharness cli_tests` — Expected: FAIL to compile (unknown fields).
- [ ] **Step 3 (GREEN): add to `RunArgs`** (after `judge`):

```rust
    /// Score ONLY known-item cases (mechanical, judge-free): the fast retrieval-A/B mode.
    /// Skips open-query answering/judging/audit AND the ANTHROPIC_API_KEY requirement.
    #[arg(long)]
    known_item_only: bool,
    /// After building the case list (mined + synthetic), save it as JSONL and proceed.
    #[arg(long, conflicts_with = "cases")]
    save_cases: Option<PathBuf>,
    /// Load a frozen case list (skips transcript mining AND synth generation entirely).
    #[arg(long)]
    cases: Option<PathBuf>,
```

- [ ] **Step 4: validations + key skip.** At the top of `run()` next to the existing cloud/local-only bail:

```rust
    if args.judge == JudgeMode::Cloud && args.known_item_only {
        anyhow::bail!(
            "--judge cloud is meaningless with --known-item-only (no open queries to judge)"
        );
    }
```

and change the key condition (finding #7's named edit site):

```rust
    let api_key = if args.local_only || args.known_item_only {
        // --known-item-only scores mechanically — no judge, no audit, no key, zero egress.
        None
    } else {
```

- [ ] **Step 5: `build_query_cases` save/load seam.** Change its signature to return the sha too, and short-circuit on `--cases`:

```rust
fn build_query_cases(
    args: &RunArgs,
    manifest: &CorpusManifest,
    corpus_home: &Path,
) -> anyhow::Result<(Vec<QueryCase>, Option<String>)> {
    if let Some(path) = &args.cases {
        let (cases, sha) = memharness::cases::load_cases(path)?;
        eprintln!(
            "memharness: {} frozen cases loaded from {} (sha {}…)",
            cases.len(),
            path.display(),
            &sha[..16]
        );
        return Ok((cases, Some(sha)));
    }
    // … existing body unchanged through the absent_gold eprintln …
```

and at its end, replace `Ok(cases)`-equivalent tail:

```rust
    let sha = match &args.save_cases {
        Some(path) => {
            let sha = memharness::cases::save_cases(path, &cases)?;
            eprintln!(
                "memharness: case list saved to {} (sha {}…) — the FROZEN Phase 1 list",
                path.display(),
                &sha[..16]
            );
            Some(sha)
        }
        None => None,
    };
    Ok((cases, sha))
```

- [ ] **Step 6: call-site + filter in `run()`:**

```rust
    let (mut cases, case_list_sha) = build_query_cases(&args, &manifest, &corpus_home)?;
    if args.known_item_only {
        cases.retain(|c| c.gold_page_id.is_some());
        eprintln!("memharness: --known-item-only — {} known-item cases retained", cases.len());
        if cases.is_empty() {
            anyhow::bail!("--known-item-only left zero cases — nothing to measure");
        }
    }
    eprintln!("memharness: {} query cases built", cases.len());
```

- [ ] **Step 7: report wiring.** Before the `ReportModel` literal add `let corpus_sha = memharness::corpus::manifest_sha(&manifest);` (BEFORE `corpus: manifest` moves the manifest), and add to the literal: `corpus_sha,` `case_list_sha,` `case_results: outcome.case_results,`.
- [ ] **Step 8: Run:** `cargo test -p memharness` — Expected: all green. Also `cargo clippy -p memharness --all-targets -- -D warnings`.
- [ ] **Step 9: Commit** — `git add -u && git commit -m "feat(memharness): --known-item-only + --save-cases/--cases + identity wiring"`

### Task 8: `compare.rs` — paired cross-run comparison (RED)

**Files:** Create: `crates/memharness/src/compare.rs` · Modify: `crates/memharness/src/lib.rs` (add `pub mod compare;` after `pub mod client;`)

- [ ] **Step 1: module doc + tests only:**

```rust
//! Paired cross-run comparison (spec §3.0.3) — the rung gate's statistics. Joins two runs'
//! per-case AIR success flags by `case_idx`, REFUSING unless both runs carry the SAME frozen
//! case-list sha (different lists ⇒ unpaired noise, the exact flaw the freeze protocol exists
//! to kill). Per segment: paired Wilcoxon (candidate vs baseline) + s@k delta.

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal scores.json with `n` known-item cases; `successes` = how many are AIR-successes.
    fn scores(sha: Option<&str>, corpus: &str, n: usize, successes: usize, label: &str) -> String {
        let results: Vec<String> = (0..n)
            .map(|i| {
                format!(
                    r#"{{"case_idx":{i},"label":"{label}","air_rank":{rank},"gbrain_rank":null,"air_success":{success},"gbrain_success":false}}"#,
                    rank = if i < successes { "0" } else { "null" },
                    success = i < successes,
                )
            })
            .collect();
        format!(
            r#"{{"case_list_sha":{sha},"corpus_sha":"{corpus}","case_results":[{results}]}}"#,
            sha = sha.map_or("null".to_string(), |s| format!("\"{s}\"")),
            results = results.join(","),
        )
    }

    #[test]
    fn refuses_unfrozen_and_mismatched_runs() {
        let frozen = scores(Some("aaa"), "cc", 30, 10, "synthetic·en·known-item");
        let unfrozen = scores(None, "cc", 30, 10, "synthetic·en·known-item");
        let other_list = scores(Some("bbb"), "cc", 30, 10, "synthetic·en·known-item");
        let other_corpus = scores(Some("aaa"), "dd", 30, 10, "synthetic·en·known-item");
        assert!(
            compare_runs(&unfrozen, &frozen).unwrap_err().to_string().contains("not frozen"),
            "unfrozen baseline refused"
        );
        assert!(
            compare_runs(&frozen, &other_list).unwrap_err().to_string().contains("sha mismatch"),
            "different frozen lists refused"
        );
        assert!(
            compare_runs(&frozen, &other_corpus)
                .unwrap_err()
                .to_string()
                .contains("corpus snapshot mismatch"),
            "different corpus snapshots refused (spec §3.0.1 — gates across snapshots are invalid)"
        );
    }

    #[test]
    fn paired_improvement_is_detected_per_segment() {
        // Same frozen list + corpus; candidate turns 10/30 successes into 20/30 (10 new wins).
        let baseline = scores(Some("aaa"), "cc", 30, 10, "synthetic·en·known-item");
        let candidate = scores(Some("aaa"), "cc", 30, 20, "synthetic·en·known-item");
        let rows = compare_runs(&baseline, &candidate).unwrap();
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.label, "synthetic·en·known-item");
        assert_eq!(row.n, 30);
        assert!((row.baseline_s_at_k - 10.0 / 30.0).abs() < 1e-9);
        assert!((row.candidate_s_at_k - 20.0 / 30.0).abs() < 1e-9);
        assert!(
            row.wilcoxon.p_value < 0.05,
            "10 concordant improvements, 0 regressions must be significant: p={}",
            row.wilcoxon.p_value
        );
        // Identical runs: delta 0; the paired test must NOT claim significance.
        let same = compare_runs(&baseline, &baseline).unwrap();
        assert!((same[0].candidate_s_at_k - same[0].baseline_s_at_k).abs() < 1e-9);
        assert!(same[0].wilcoxon.p_value > 0.5, "all-tie pairing is insignificant");
    }

    #[test]
    fn corrupt_pairings_fail_loud() {
        let baseline = scores(Some("aaa"), "cc", 3, 1, "synthetic·en·known-item");
        let candidate = scores(Some("aaa"), "cc", 2, 1, "synthetic·en·known-item");
        assert!(
            compare_runs(&baseline, &candidate).unwrap_err().to_string().contains("length mismatch"),
        );
        let relabeled = scores(Some("aaa"), "cc", 3, 1, "synthetic·ko·known-item");
        assert!(
            compare_runs(&baseline, &relabeled).unwrap_err().to_string().contains("label mismatch"),
        );
    }
}
```

- [ ] **Step 2: Verify RED:** `cargo test -p memharness compare::` — FAIL to compile.
- [ ] **Step 3: Commit** — `git add crates/memharness/src/compare.rs crates/memharness/src/lib.rs && git commit -m "test(memharness): paired cross-run compare — freeze enforcement + paired Wilcoxon (RED)"`

### Task 9: `compare.rs` — implement + CLI subcommand (GREEN)

**Files:** Modify: `crates/memharness/src/compare.rs`, `crates/memharness/src/main.rs`

- [ ] **Step 1: implementation** (between doc and tests):

```rust
use std::collections::BTreeMap;

use crate::run::CaseResult;
use crate::stats::{wilcoxon_signed_rank, WilcoxonResult};

/// One segment's paired comparison row.
pub struct SegmentComparison {
    pub label: String,
    pub n: usize,
    pub baseline_s_at_k: f64,
    pub candidate_s_at_k: f64,
    /// Paired Wilcoxon over per-case success flags, candidate vs baseline.
    pub wilcoxon: WilcoxonResult,
}

/// Pull (case_list_sha, corpus_sha, case_results) out of a scores.json string. Parses via
/// `Value` because `ReportModel` deliberately does not derive Deserialize — only these three
/// fields matter here.
fn extract_run(scores_json: &str) -> anyhow::Result<(String, String, Vec<CaseResult>)> {
    let v: serde_json::Value = serde_json::from_str(scores_json)?;
    let case_sha = v
        .get("case_list_sha")
        .and_then(|s| s.as_str())
        .map(str::to_string)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "scores.json carries no case_list_sha — the run was not frozen \
                 (--cases/--save-cases); compare refuses unfrozen runs"
            )
        })?;
    let corpus_sha = v
        .get("corpus_sha")
        .and_then(|s| s.as_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("scores.json has no corpus_sha (pre-rung-0 run?)"))?;
    let results: Vec<CaseResult> = serde_json::from_value(
        v.get("case_results")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("scores.json has no case_results (pre-rung-0 run?)"))?,
    )?;
    if results.is_empty() {
        anyhow::bail!("case_results is empty — nothing to pair");
    }
    Ok((case_sha, corpus_sha, results))
}

/// Compare candidate vs baseline PAIRED by `case_idx` over an identical frozen case list AND
/// an identical corpus snapshot (spec §3.0.1: gates across snapshots are invalid — enforced
/// here in the tool, not left to a human reading two reports).
pub fn compare_runs(baseline: &str, candidate: &str) -> anyhow::Result<Vec<SegmentComparison>> {
    let (sha_b, corpus_b, base) = extract_run(baseline)?;
    let (sha_c, corpus_c, cand) = extract_run(candidate)?;
    if corpus_b != corpus_c {
        anyhow::bail!(
            "corpus snapshot mismatch ({corpus_b} vs {corpus_c}) — the runs ingested different \
             corpora; a gate across snapshots is invalid (spec §3.0.1)"
        );
    }
    if sha_b != sha_c {
        anyhow::bail!(
            "case-list sha mismatch ({sha_b} vs {sha_c}) — the runs used different frozen \
             lists; pairing them would be exactly the churn the freeze protocol exists to kill"
        );
    }
    if base.len() != cand.len() {
        anyhow::bail!("case_results length mismatch ({} vs {})", base.len(), cand.len());
    }
    let base_by_idx: BTreeMap<usize, &CaseResult> = base.iter().map(|c| (c.case_idx, c)).collect();
    let mut by_label: BTreeMap<String, (Vec<f64>, Vec<f64>)> = BTreeMap::new();
    for c in &cand {
        let b = base_by_idx.get(&c.case_idx).ok_or_else(|| {
            anyhow::anyhow!("case_idx {} in candidate but not baseline — corrupt pairing", c.case_idx)
        })?;
        if b.label != c.label {
            anyhow::bail!("case_idx {} label mismatch ({} vs {})", c.case_idx, b.label, c.label);
        }
        let entry = by_label.entry(c.label.clone()).or_default();
        entry.0.push(if b.air_success { 1.0 } else { 0.0 });
        entry.1.push(if c.air_success { 1.0 } else { 0.0 });
    }
    Ok(by_label
        .into_iter()
        .map(|(label, (b, c))| {
            let n = b.len();
            SegmentComparison {
                label,
                n,
                baseline_s_at_k: b.iter().sum::<f64>() / n as f64,
                candidate_s_at_k: c.iter().sum::<f64>() / n as f64,
                // wilcoxon_signed_rank(a, b) tests diffs a[i]−b[i]: candidate first.
                wilcoxon: wilcoxon_signed_rank(&c, &b),
            }
        })
        .collect())
}
```

- [ ] **Step 2: CLI.** In `main.rs`: add the variant + args + dispatch. Adding the `Compare` variant makes `Command` multi-variant, so in the SAME step convert every `let Command::Run(args) = cli.command;` in `cli_tests` (four sites after Task 7) to `let Command::Run(args) = cli.command else { panic!("run expected") };` — the pattern is now refutable, so the lint concern from Task 7's NOTE no longer applies:

```rust
    /// Compare two frozen runs PAIRED (per-case, same case-list sha) — the rung-gate stats.
    Compare(CompareArgs),
```

```rust
#[derive(clap::Args)]
struct CompareArgs {
    /// Baseline run's scores.json (e.g. the frozen Phase 1 baseline).
    #[arg(long)]
    baseline: PathBuf,
    /// Candidate run's scores.json (the rung under test).
    #[arg(long)]
    candidate: PathBuf,
}
```

```rust
        Command::Compare(args) => compare(args),
```

```rust
/// Print the paired comparison table (spec §3.0.3). The GATE read: only the two gating
/// segments (synthetic·en/ko·known-item, spec §1) may pass or fail a rung.
fn compare(args: CompareArgs) -> anyhow::Result<()> {
    let baseline = std::fs::read_to_string(&args.baseline)
        .with_context(|| format!("reading baseline {}", args.baseline.display()))?;
    let candidate = std::fs::read_to_string(&args.candidate)
        .with_context(|| format!("reading candidate {}", args.candidate.display()))?;
    println!("| segment | n | baseline s@k | candidate s@k | Δ | paired Wilcoxon p |");
    println!("|---|---|---|---|---|---|");
    for row in memharness::compare::compare_runs(&baseline, &candidate)? {
        println!(
            "| {} | {} | {:.3} | {:.3} | {:+.3} | {:.4}{} |",
            row.label,
            row.n,
            row.baseline_s_at_k,
            row.candidate_s_at_k,
            row.candidate_s_at_k - row.baseline_s_at_k,
            row.wilcoxon.p_value,
            if row.wilcoxon.small_n_approx { " (small-n approx)" } else { "" },
        );
    }
    println!("\nGating segments (spec §1): synthetic·en·known-item, synthetic·ko·known-item ONLY.");
    Ok(())
}
```

- [ ] **Step 3: Run:** `cargo test -p memharness && cargo clippy -p memharness --all-targets -- -D warnings` — Expected: all green (incl. Task 8's tests and the converted cli_tests destructurings).
- [ ] **Step 4: Commit** — `git add -u && git commit -m "feat(memharness): compare subcommand — paired per-case Wilcoxon over an enforced frozen list (GREEN)"`

### Task 10: Rung 0 gates + FREEZE + frozen baseline + PR

- [ ] **Step 1: Full gate set** (every line green):

```
cargo test -p memharness
cargo clippy -p memharness --all-targets -- -D warnings
cargo test --workspace
cargo check --workspace
```

- [ ] **Step 2: Freeze the measurement context** (spec §3.0 — local, no cloud):

```
cp -R ~/brain ~/.air-harness/phase1-corpus
cargo run -p memharness -- run --known-item-only \
  --corpus ~/.air-harness/phase1-corpus \
  --save-cases ~/.air-harness/phase1-cases.jsonl
```

Expected: completes in ~35–40 min; stderr shows the saved case-list sha; the report shows `Corpus snapshot: <id> · frozen case list: <sha>`; record the report dir — **this is the FROZEN PHASE 1 BASELINE**.
- [ ] **Step 3: Reproduce check:** re-run with `--cases ~/.air-harness/phase1-cases.jsonl` (same `--corpus`), then `cargo run -p memharness -- compare --baseline <baseline>/scores.json --candidate <rerun>/scores.json`. Expected: identical case counts + same shas; deltas ~0 with insignificant p (only HNSW deep-rank jitter — spec §3.0.4). Record both report paths.
- [ ] **Step 4: Commit + PR** — title `feat(memharness): rung 0 — frozen measurement protocol (--known-item-only, frozen case lists, paired compare)`; body includes the gate outputs, both report paths (paths only — NEVER contents), and the reproduce-check deltas. Do not merge — Peter-gated.

---

# RUNG 1 — branch `feat-retrieval-rung1` (off main AFTER rung 0 merges)

### Task 11: `keyword.rs` — per-term OR tests (RED)

**Files:** Modify: `crates/bossclaw-core/src/keyword.rs`

- [ ] **Step 1: Replace the tests module** with the new contract (old phrase-pinning tests updated in place — they pin the OLD defect):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multi_word_queries_become_per_term_or() {
        // The rung-1 fix (retrieval-floor spec Rev 2 §3.2): terms match INDEPENDENTLY, ranked
        // by the BM25 that is already there — not as one exact adjacent phrase.
        assert_eq!(escape_fts_query("hello world"), r#""hello" OR "world""#);
    }

    #[test]
    fn single_term_stays_one_quoted_phrase() {
        assert_eq!(escape_fts_query("hello"), r#""hello""#);
    }

    #[test]
    fn operator_words_are_quoted_terms_never_operators() {
        // User tokens that spell FTS5 operators stay INSIDE quotes — injection-safe.
        assert_eq!(escape_fts_query("foo OR bar"), r#""foo" OR "OR" OR "bar""#);
        assert_eq!(escape_fts_query("NOT valid"), r#""NOT" OR "valid""#);
        assert_eq!(escape_fts_query("a AND b"), r#""a" OR "AND" OR "b""#);
        assert_eq!(escape_fts_query("near(foo bar)"), r#""near(foo" OR "bar)""#);
        assert_eq!(escape_fts_query("prefix*"), r#""prefix*""#);
    }

    #[test]
    fn embedded_double_quotes_are_doubled_per_term() {
        assert_eq!(escape_fts_query(r#"a"b"#), r#""a""b""#);
        assert_eq!(escape_fts_query(r#"say "hi"#), r#""say" OR """hi""#);
    }

    #[test]
    fn punctuation_only_terms_are_tolerated() {
        // Critic-verified: FTS5 tolerates a quoted punctuation token; wasteful but never an error.
        assert_eq!(escape_fts_query("foo - bar"), r#""foo" OR "-" OR "bar""#);
    }

    #[test]
    fn korean_terms_tokenize_on_whitespace() {
        assert_eq!(escape_fts_query("메모리 하니스"), r#""메모리" OR "하니스""#);
    }

    #[test]
    fn multiline_mined_queries_split_across_lines() {
        assert_eq!(
            escape_fts_query("line one\nline two"),
            r#""line" OR "one" OR "line" OR "two""#
        );
    }

    #[test]
    fn empty_and_whitespace_only_keep_the_empty_phrase_contract() {
        assert_eq!(escape_fts_query(""), "\"\"");
        assert_eq!(escape_fts_query("   \n\t"), "\"\"");
    }
}
```

- [ ] **Step 2: Verify RED:** `cargo test -p bossclaw-core keyword::` — Expected: FAIL (current impl emits one phrase).
- [ ] **Step 3: Commit** — `git add -u && git commit -m "test(bossclaw-core): keyword query becomes per-term OR — injection-safe tokenization contract (RED)"`

### Task 12: `keyword.rs` — implement (GREEN, incl. doc/doctests)

**Files:** Modify: `crates/bossclaw-core/src/keyword.rs`

- [ ] **Step 1: Replace `escape_fts_query` and its doc comment:**

```rust
/// Escape an arbitrary user string so it can be used safely as an FTS5
/// `MATCH` expression, matching terms INDEPENDENTLY.
///
/// FTS5 parses its `MATCH` argument as a query language that recognises
/// operators such as `OR`, `AND`, `NOT`, `NEAR`, `*`, and unbalanced
/// double-quotes as phrase-open/close. A user-supplied string can contain
/// any of these tokens and would otherwise mutate query semantics or cause
/// a parse error.
///
/// The input is split on Unicode whitespace; each term is individually wrapped
/// as an FTS5 **quoted phrase** (internal `"` doubled per the FTS5 rule) and
/// the phrases are joined with program-emitted `OR`:
/// ```text
/// "term1" OR "term2" OR "term3"
/// ```
/// Every user token stays inside quotes, so no operator injection is possible
/// — the `OR`s are ours, never the user's. Documents matching MORE terms rank
/// higher via bm25 (retrieval-floor spec Rev 2 §3.2; previously the whole
/// query was ONE quoted phrase, so multi-word queries only matched exact
/// adjacent runs — the measured keyword-arm recall ceiling).
///
/// # Examples
/// ```
/// use bossclaw_core::keyword::escape_fts_query;
///
/// // Terms match independently.
/// assert_eq!(escape_fts_query("hello world"), r#""hello" OR "world""#);
///
/// // FTS5 operator words are neutralised inside per-term quotes.
/// assert_eq!(escape_fts_query("foo OR bar"), r#""foo" OR "OR" OR "bar""#);
///
/// // Embedded double-quote — doubled per FTS5 escaping rules.
/// assert_eq!(escape_fts_query(r#"a"b"#), r#""a""b""#);
/// ```
pub fn escape_fts_query(raw: &str) -> String {
    let terms: Vec<String> = raw
        .split_whitespace()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect();
    if terms.is_empty() {
        // Historical contract for empty/whitespace-only input: one empty quoted
        // phrase, which FTS5 parses cleanly and matches nothing.
        return "\"\"".to_string();
    }
    terms.join(" OR ")
}
```

- [ ] **Step 2: Run:** `cargo test -p bossclaw-core keyword::` then `cargo test -p bossclaw-core --doc` — Expected: PASS (doctests updated together).
- [ ] **Step 3: Whole-crate + daemon:** `cargo test -p bossclaw-core && cargo test -p bossclawd`. OR-semantics only WIDENS the keyword match set; if any existing recall test fails, inspect it: a test asserting an ABSENCE that now legitimately matches one term may have its expectation updated WITH a justification comment; a test asserting rank order that flips indicates a real problem — stop and reassess, do not paper over.
- [ ] **Step 4: Commit** — `git add -u && git commit -m "feat(bossclaw-core): keyword arm matches query terms independently (per-term OR, injection-safe)"`

### Task 13: `FUSION_FETCH` 50 → 200

**Files:** Modify: `crates/bossclaw-core/src/recall.rs:151-161`

- [ ] **Step 1: Replace the const + its doc:**

```rust
/// How many candidates each arm fetches before fusion. Over-fetching well beyond
/// the caller's final `k` lets RRF see enough of each arm's tail to reorder
/// correctly (an id ranked, say, #20 by keyword but #1 by vector should still be
/// fusible). Was 50 — measured as a hard recall ceiling on an ~880-page corpus:
/// an id ranked >50 in BOTH arms could never surface regardless of `k`
/// (retrieval-floor spec Rev 2 §2). 200 ≈ 23% of that corpus and stays cheap
/// (HNSW search uses ef = max(requested, 64); post-fusion boosts are
/// O(candidates) multiplies). Re-tunable — a measurement subject, not a tuned
/// truth. NOTE for the chunking rung: once chunking lands this counts CHUNK
/// slots before per-event fold-back (spec §3.4.4 over-fetch rule).
pub const FUSION_FETCH: usize = 200;
```

- [ ] **Step 2: Run:** `cargo test -p bossclaw-core && cargo test -p bossclawd` — Expected: green (no test pins 50; the earlier grep found only the const + call sites + a doc mention at `log.rs:1394`, which references the const by name and needs no edit).
- [ ] **Step 3: Commit** — `git add -u && git commit -m "feat(bossclaw-core): lift FUSION_FETCH 50→200 — remove the both-arms candidate ceiling"`

### Task 14: Rung 1 gates + measured A/B gate + PR

- [ ] **Step 1: Full gate set** (green): `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo build -p bossclawd && cargo check --workspace`
- [ ] **Step 2: The A/B (frozen, paired — spec §3.2 gate):**

```
cargo run -p memharness -- run --known-item-only \
  --corpus ~/.air-harness/phase1-corpus \
  --cases ~/.air-harness/phase1-cases.jsonl
cargo run -p memharness -- compare \
  --baseline <FROZEN-BASELINE>/scores.json \
  --candidate <this-run>/scores.json
```

- [ ] **Step 3: Read the gate.** SHIP iff: `synthetic·en·known-item` improves with paired Wilcoxon p<0.05 AND `synthetic·ko·known-item` shows no significant regression (p<0.05 in the wrong direction). Real segments are directional color only. If the gate FAILS: do not merge; record the numbers and reassess (spec §1.4).
- [ ] **Step 4: Commit + PR** — title `feat(bossclaw-core): rung 1 — per-term keyword matching + FUSION_FETCH 200 (measured gate: <PASS/FAIL>)`; body = the compare table verbatim + report paths + both baseline identities (corpus snapshot id + case-list sha). Do not merge — Peter-gated.

---

## Self-review

**Spec coverage:** §3.0.1 snapshot (T5/6/10) · §3.0.2 save/load (T1/2/7) · §3.0.3 per-case + paired compare (T3/4/8/9) · §3.0.4 jitter note (T10 reproduce check) · §3.1 flags/key-skip/estimates (T7/T10) · §3.2 tokenization+cap+tests incl. punctuation/KO/multiline (T11-13) · §3.2 gate (T14) · §4 gates (T10/T14). Rungs 2-3: deliberately out (planned post-measurement, spec-ordered).
**Placeholders:** none — every step carries code or an exact command; the one `每`-typo note in T12 is an explicit instruction, not a gap.
**Type consistency:** `CaseResult` fields (T4) = the JSON keys in T8's fixtures = extract in T9; `save_cases`/`load_cases` (T2) = call sites (T7); `manifest_sha` (T6) = call site (T7); `WilcoxonResult.p_value`/`.small_n_approx` verified against stats.rs/report.rs usage; cli_tests destructuring conversion (T7 step 1) precedes the enum growth (T9 step 2).
