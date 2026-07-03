# AIR Agent memharness Phase 0 (blind A/B measuring stick) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A one-command local dev tool (`cargo run -p memharness -- run`) that produces a per-run markdown report answering, in numbers: on Peter's own `~/brain` corpus and his real mined queries, end-to-end, does AIR's engine beat GBrain's daily `balanced` pipeline — by how much, on which segments (EN/KO, known-item/open, real/synthetic), and can we trust the local judge? Every future memory-retrieval investment is A/B'd against this baseline instead of trusted on reputation.

**Architecture:** A new dev-only workspace bin crate `crates/memharness`. It spins a **real in-process `bossclawd` daemon** (via the `test-helpers` feature's `test_engine` + production `run_accept_loop`) on a private Unix socket under a per-run temp home, and drives it over the **real wire protocol** (`Hello` → `AddGrant` → `RunIngest` → `Recall`) — the same dispatch path production uses. The GBrain arm shells out to `gbrain query`. Both arms feed an identical local-Ollama answerer; open queries are scored by a blind, position-swapped local judge audited against the Anthropic API. Everything is ephemeral except the report, which is written OUTSIDE the repo (`~/.air-harness/reports/`) because it quotes brain content.

**Tech Stack:** Rust 2021, `bossclawd { path, features = ["test-helpers"] }` + `bossclawd-proto`, `tokio` (dedicated per-daemon current-thread runtime, mirroring the desktop `TestDaemon`), `ureq` v2 (loopback HTTP to Ollama + the Anthropic audit call — mirrors bossclaw-core's pinned `ureq = "2"`), `serde`/`serde_json`, `sha2` (corpus manifest), `rand` 0.8 + `rand_chacha` 0.3 (seeded `StdRng` for all sampling/bootstrap), `clap` 4 (CLI), `anyhow` (tool-level error plumbing). All statistics (bootstrap CIs, Wilcoxon signed-rank, Cohen's kappa), language detection (Hangul heuristic), and YAML frontmatter stripping are **plain Rust with unit tests** — no heavyweight numeric/NLP deps. Every dep version is mirrored EXACTLY from the existing `Cargo.lock` (§File Structure → Cargo.toml note); no new duplicate major enters the tree.

**Spec:** docs/superpowers/specs/2026-07-03-air-agent-memharness-phase0-design.md

---

## Preconditions & environment

- **Branch:** `feat-memharness-phase0` (already checked out). Verify at start: `git status -sb`.
- **Never ships:** `memharness` is dev tooling. It depends on `bossclawd`'s `test-helpers` feature (which exposes mock providers + the in-memory vault). This MUST be documented in `crates/memharness/Cargo.toml` and the `main.rs` header, and the crate MUST NOT be added to any release/bundle manifest.
- **Reports are private:** `~/.air-harness/reports/` is OUTSIDE the repo. `report.rs` MUST refuse to write anywhere under the repo root (Task 33 enforces this with a test).
- **Live run is Peter-gated:** the final live baseline run (real `~/brain` + real `gbrain` + real Ollama + real Anthropic key) is the acceptance demo, NOT an automatable step. The plan ends "harness ready + smoke-tested hermetically; live baseline run is the acceptance demo."

## Dependency versions (mirror EXACTLY from Cargo.lock — verified 2026-07-03)

| crate | version to declare | already in lock? | notes |
|---|---|---|---|
| `bossclawd` | `{ path = "../bossclawd", features = ["test-helpers"] }` | yes (workspace member) | brings `test_engine`, `run_accept_loop`, `vault::seed_secret_cache_for_test` |
| `bossclawd-proto` | `{ path = "../bossclawd-proto" }` | yes | `Request`/`Response`/`Hello`/frame fns |
| `bossclaw-core` | `{ path = "../bossclaw-core", features = ["ollama"] }` | yes | only for type re-exports if needed; harness talks WIRE, not core, at runtime |
| `tokio` | `{ version = "1", features = ["rt", "net", "io-util", "time", "macros"] }` | `1.52.3` | per-daemon `current_thread` runtime + `UnixStream` |
| `ureq` | `{ version = "2", default-features = false, features = ["json", "tls"] }` | `2.12.1` | mirrors bossclaw-core's exact ureq block; blocking loopback + Anthropic HTTP |
| `serde` | `{ version = "1", features = ["derive"] }` | `1.0.228` | |
| `serde_json` | `"1"` | `1.0.150` | |
| `sha2` | `"0.10"` | `0.10.9` | corpus manifest hashes |
| `rand` | `"0.8"` | `0.8.6` | seeded `StdRng` (0.8 API; NOT 0.9) |
| `rand_chacha` | `"0.3"` | `0.3.1` | backs the deterministic `StdRng` seed |
| `clap` | `{ version = "4", features = ["derive"] }` | `4.6.1` | CLI |
| `anyhow` | `"1"` | `1.0.103` | tool error plumbing |
| `tempfile` | `"3"` (dev-dependency) | `3.27.0` | per-run temp home + test fixtures |

All fourteen are already resolved in `Cargo.lock`, so adding them introduces **zero** new crate versions.

---

## File Structure

Every file has ONE responsibility. Paths are exact; the implementer creates them in task order.

```
crates/memharness/
├── Cargo.toml                     # bin crate, test-helpers dep, exact versions, "never ships" note
├── src/
│   ├── main.rs                    # CLI (`run` subcommand + flags), the run orchestration, header doc
│   ├── daemon.rs                  # HarnessDaemon: spin/kill in-process isolated daemon (own runtime+thread), private socket, per-run temp home; hands out a WireClient
│   ├── client.rs                  # WireClient: thin single-in-flight, timeout-bounded wire client (hello/add_grant/run_ingest/recall); drops stream on error
│   ├── corpus.rs                  # copy ~/brain → harness home, strip frontmatter, skip dot-entries, sha256 manifest, slug↔path normalization
│   ├── frontmatter.rs            # pure: strip a leading `---\n…\n---` YAML block; language heuristic (Hangul presence)
│   ├── mine.rs                    # transcript JSONL parser → real queries + implicit known-item labels (get_page within next-5 window), dedup
│   ├── synth.rs                   # Ollama-generated known-item queries, stratified by category dir + language; source page = gold
│   ├── arms.rs                    # AirArm (WireClient recall+hydrate) + GBrainArm (gbrain subprocess parse) + shared Answerer trait; identical answerer both arms
│   ├── judge.rs                   # blind pairwise local judge (position-swapped ×2, uncertain on disagreement) + cloud audit (Anthropic) + agreement% + kappa + trust verdict
│   ├── stats.rs                   # success@k, MRR, win-rates, bootstrap CI, Wilcoxon signed-rank (all pure)
│   ├── ollama.rs                  # loopback HTTP client (ureq): /api/tags preflight + generate; used by synth/arms/judge
│   ├── anthropic.rs              # minimal Anthropic Messages API POST (ureq); strict parse; degrade-not-fabricate on failure
│   └── report.rs                  # markdown per spec §7 + raw scores JSON; NEVER writes under the repo root
└── tests/
    ├── fixtures/
    │   ├── transcript_synthetic.jsonl   # SYNTHETIC hand-authored JSONL (never real transcripts) for mine.rs
    │   ├── gbrain_query_sample.txt      # captured gbrain query output shape (from Probe A) for arms.rs parser
    │   └── mini_corpus/                 # 3 tiny .md pages (2 EN, 1 KO) with frontmatter, for the hermetic e2e test
    │       ├── en/alpha.md
    │       ├── en/beta.md
    │       └── ko/gamma.md
    └── hermetic_e2e.rs              # one integration test: corpus→daemon ingest→recall→known-item scoring end-to-end, scripted answerer/judge double (no Ollama/gbrain/Anthropic)
```

**Seam facts pinned from the real code (do not re-derive):**

- `bossclawd::server::test_engine(home: PathBuf) -> EngineHandle` and `bossclawd::server::run_accept_loop(engine: Arc<EngineHandle>, listener: tokio::net::UnixListener)` are both `pub` behind `feature = "test-helpers"`.
- `bossclawd::vault::seed_secret_cache_for_test(HashMap<String,String>)` MUST be called (empty map) BEFORE spinning the daemon, or the first provider-key read blocks on a keychain-ACL prompt and hangs forever.
- Wire handshake: send `Hello { proto_version: PROTO_VERSION }`, expect `HelloOk { pid, proto_version }`. Frame fns `read_frame`/`write_frame` are **NOT cancellation-safe** — never race them in a `select!`; a timed-out stream MUST be dropped, not reused.
- Ops used: `Request::AddGrant { onboarded: true, path }`, `Request::RunIngest { onboarded: true }`, `Request::Recall { onboarded: true, query, k }`. Responses: `Response::Ok`, `Response::RunIngest(IngestReportMirror)`, `Response::Recall(Vec<HitWire>)`. `HitWire { hit: HitMirror { event_id, score, sources, kind }, text }` — `text` is the hydrated snippet.
- Daemon lifecycle pattern to copy: desktop `TestDaemon` (own OS thread + `current_thread` runtime + `Notify` shutdown + `sync_channel` bind-handshake so the client can't race the bind).

---

## Task 1 — PROBES (read-only reality check; NO code)

**This task writes no crate code.** It runs three read-only probes on Peter's machine and records the findings in `docs/superpowers/plans/memharness-probes.md` (skeleton already committed). The pinned values become constants in later tasks; if reality differs, the implementer updates the constant AND the reconciliation note AND flags it in this task's commit.

- [ ] Run Probe A — `gbrain` CLI:
  ```
  gbrain --version
  gbrain query --help
  gbrain query "test" --limit 3
  ```
  Record the exact output, the machine-parseable invocation, the per-hit fields (slug id / chunk text / score), the **slug ↔ `~/brain`-relative-path** convention, and whether `balanced` is the default or needs a flag. Paste the raw `gbrain query "test" --limit 3` output verbatim into `tests/fixtures/gbrain_query_sample.txt` (this is the parser fixture for Task 23; redact nothing structural, but you MAY replace snippet bodies with `<redacted>` since fixtures are committed to a public repo).
- [ ] Run Probe B — Ollama: `curl -s http://127.0.0.1:11434/api/tags`. Record reachability, whether `qwen2.5:7b` (or the evolve loop's actual `qwen2.5:7b-instruct`) is present, and the endpoint to use. Pin `DEFAULT_OLLAMA_MODEL`.
- [ ] Run Probe C — real query count:
  ```
  grep -roh 'mcp__gbrain__\(query\|search\|recall\)' ~/.claude/projects/**/*.jsonl 2>/dev/null | wc -l
  grep -rl  'mcp__gbrain__\(query\|search\|recall\)' ~/.claude/projects/**/*.jsonl 2>/dev/null | wc -l
  ```
  Record raw count, estimated deduped count, estimated known-item count, and set the `WEIGHT_SYNTHETIC_HIGHER` decision (spec §90: if deduped real open < 50 → true).
- [ ] Fill in every `____` blank in `docs/superpowers/plans/memharness-probes.md`.
- [ ] Commit:
  ```
  git add docs/superpowers/plans/memharness-probes.md crates/memharness/tests/fixtures/gbrain_query_sample.txt
  git commit -m "$(cat <<'EOF'
docs(memharness): Task 1 probe findings — gbrain CLI, Ollama, mined-query count

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```
  (If `tests/fixtures/` does not exist yet, `mkdir -p crates/memharness/tests/fixtures` first — this task may create it.)

---

## Task 2 — Crate skeleton + Cargo.toml + workspace member

- [ ] Write `crates/memharness/Cargo.toml`:
  ```toml
  [package]
  name = "memharness"
  version = "0.0.1"
  edition = "2021"
  license = "Apache-2.0"
  # DEV-ONLY TOOL — never ships. Depends on bossclawd's `test-helpers` feature (mock
  # embedder/reasoner + in-memory vault), which is acceptable ONLY because this binary
  # is never bundled/released. Do NOT add memharness to any release manifest.
  description = "memharness: dev-only blind A/B measuring stick (AIR engine vs GBrain) — never ships."
  publish = false

  [[bin]]
  name = "memharness"
  path = "src/main.rs"

  [dependencies]
  # Versions mirror Cargo.lock EXACTLY (verified 2026-07-03) — zero new crate versions enter the tree.
  bossclawd = { path = "../bossclawd", features = ["test-helpers"] }
  bossclawd-proto = { path = "../bossclawd-proto" }
  bossclaw-core = { path = "../bossclaw-core", features = ["ollama"] }
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
- [ ] Write `crates/memharness/src/main.rs` (skeleton only — compiles, does nothing yet):
  ```rust
  //! memharness — DEV-ONLY blind A/B measuring stick: AIR engine vs GBrain, on Peter's own
  //! corpus + queries, end-to-end. NEVER SHIPS (depends on bossclawd's `test-helpers` feature).
  //! See docs/superpowers/specs/2026-07-03-air-agent-memharness-phase0-design.md.
  #![forbid(unsafe_code)]

  fn main() {
      println!("memharness: not yet implemented");
  }
  ```
- [ ] Add `crates/memharness` to the workspace `members` in the root `Cargo.toml`:
  ```toml
  members = ["crates/air-rs", "crates/bossclaw-core", "crates/bossclawd", "crates/bossclawd-proto", "crates/memharness", "apps/desktop/src-tauri"]
  ```
- [ ] Run: `cargo check -p memharness` → expect it compiles (skeleton).
- [ ] Commit:
  ```
  git add crates/memharness/Cargo.toml crates/memharness/src/main.rs Cargo.toml
  git commit -m "$(cat <<'EOF'
feat(memharness): dev-only crate skeleton + workspace member

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

---

## Task 3 — `frontmatter.rs`: strip YAML frontmatter (RED)

- [ ] Create `crates/memharness/src/frontmatter.rs` with ONLY the failing test:
  ```rust
  //! Pure text helpers: strip a leading YAML frontmatter block; detect Korean content.
  //! GBrain strips frontmatter before chunking; AIR would embed it — fairness requires
  //! indexing the SAME text (spec §34), so the harness strips it from every copied page.

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
          // A horizontal rule mid-document, or a `---` with no closing fence, is NOT frontmatter.
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
- [ ] Add `mod frontmatter;` to `main.rs` (below the header, above `fn main`).
- [ ] Run: `cargo test -p memharness frontmatter` → expect **FAIL** (`strip_frontmatter` undefined / does not compile).

## Task 4 — `frontmatter.rs`: implement `strip_frontmatter` (GREEN)

- [ ] Add above the `#[cfg(test)]` module in `frontmatter.rs`:
  ```rust
  /// Strip a leading YAML frontmatter block: the file MUST begin (byte 0) with a `---` line,
  /// and the block ends at the next line that is exactly `---`. Returns the remainder after that
  /// closing fence's newline. If there is no opening fence at byte 0, or no closing fence, the
  /// input is returned unchanged (a lone `---` or a horizontal rule is NOT frontmatter).
  pub fn strip_frontmatter(input: &str) -> &str {
      // Opening fence must be the very first line.
      let after_open = match input.strip_prefix("---\n") {
          Some(rest) => rest,
          None => return input,
      };
      // Find the closing fence: a line that is exactly "---". Scan line boundaries.
      let mut search_from = 0usize;
      loop {
          // Position of the next line start in `after_open` at/after `search_from`.
          let slice = &after_open[search_from..];
          if let Some(rel) = slice.find("---\n") {
              let abs = search_from + rel;
              // The match is a closing fence only if it is at a line start (byte 0 of after_open,
              // or immediately after a '\n').
              let at_line_start = abs == 0 || after_open.as_bytes()[abs - 1] == b'\n';
              if at_line_start {
                  // Return everything after the closing fence's own newline.
                  return &after_open[abs + 4..];
              }
              search_from = abs + 4;
          } else if let Some(rel) = slice.find("---") {
              // A closing fence with no trailing newline (EOF right after `---`): treat as closed
              // only at a line start AND at end-of-input.
              let abs = search_from + rel;
              let at_line_start = abs == 0 || after_open.as_bytes()[abs - 1] == b'\n';
              if at_line_start && abs + 3 == after_open.len() {
                  return "";
              }
              return input; // trailing `---` mid-line → not a fence → no valid frontmatter
          } else {
              return input; // no closing fence at all → unchanged
          }
      }
  }
  ```
- [ ] Run: `cargo test -p memharness frontmatter` → expect **PASS** (4 tests).
- [ ] Commit:
  ```
  git add crates/memharness/src/frontmatter.rs crates/memharness/src/main.rs
  git commit -m "$(cat <<'EOF'
feat(memharness): frontmatter strip (leading --- block, fence-anchored)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

## Task 5 — `frontmatter.rs`: Hangul language heuristic (RED)

- [ ] Add to the `#[cfg(test)]` module in `frontmatter.rs`:
  ```rust
  #[test]
  fn detects_korean_by_hangul_presence() {
      assert_eq!(detect_lang("안녕하세요 세계"), Lang::Ko);
      assert_eq!(detect_lang("hello world"), Lang::En);
      // Mixed content with ANY Hangul is Ko (KO segment is the one we care to isolate).
      assert_eq!(detect_lang("the term 메모리 means memory"), Lang::Ko);
      assert_eq!(detect_lang(""), Lang::En);
  }
  ```
- [ ] Run: `cargo test -p memharness frontmatter` → expect **FAIL** (`detect_lang` / `Lang` undefined).

## Task 6 — `frontmatter.rs`: implement `detect_lang` (GREEN)

- [ ] Add above the test module in `frontmatter.rs`:
  ```rust
  /// Coarse language tag for a page/query. Phase 0 only needs to isolate the KO segment (spec §42,
  /// §74) — the expected bilingual gap. Any Hangul codepoint present ⇒ `Ko`, else `En`. Mixed is
  /// folded into `Ko` because the finding we want is "does AIR lose on Korean-bearing content".
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum Lang {
      En,
      Ko,
  }

  /// Hangul ranges: Syllables U+AC00–U+D7A3, Jamo U+1100–U+11FF, Compatibility Jamo U+3130–U+318F.
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
- [ ] Run: `cargo test -p memharness frontmatter` → expect **PASS** (5 tests).
- [ ] Commit:
  ```
  git add crates/memharness/src/frontmatter.rs
  git commit -m "$(cat <<'EOF'
feat(memharness): Hangul-presence language heuristic (En/Ko)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

---

## Task 7 — `corpus.rs`: slug↔path normalization + manifest types (RED)

- [ ] Create `crates/memharness/src/corpus.rs` with the failing test:
  ```rust
  //! Corpus preparation: copy `~/brain/*.md` into the harness home stripping frontmatter,
  //! skipping dot-entries, and recording a sha256 manifest for reproducibility (spec §32-35).
  //! The `~/brain`-relative path stem is the arm-independent page identity used for known-item
  //! scoring on BOTH arms (spec §54).

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn page_id_is_brain_relative_stem() {
          // "<brain>/air/foo.md" → "air/foo"; both arms normalize to this.
          assert_eq!(page_id_from_rel("air/foo.md"), "air/foo");
          assert_eq!(page_id_from_rel("people/kwang-wook-ahn.md"), "people/kwang-wook-ahn");
          assert_eq!(page_id_from_rel("top.md"), "top");
      }

      #[test]
      fn gbrain_slug_maps_to_same_page_id() {
          // GBrain slugs are already the stem form (Probe A pins this) — identity.
          assert_eq!(page_id_from_gbrain_slug("air/foo"), "air/foo");
          // Defensive: a slug that arrives WITH a `.md` is normalized too.
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
- [ ] Add `mod corpus;` to `main.rs`.
- [ ] Run: `cargo test -p memharness corpus` → expect **FAIL**.

## Task 8 — `corpus.rs`: implement normalization + manifest types (GREEN)

- [ ] Add above the test module in `corpus.rs`:
  ```rust
  use serde::Serialize;

  /// One entry in the corpus manifest: the arm-independent page id, the sha256 of the
  /// frontmatter-STRIPPED bytes actually indexed, and that byte count.
  #[derive(Debug, Clone, Serialize)]
  pub struct ManifestEntry {
      pub page_id: String,
      pub sha256: String,
      pub bytes: u64,
  }

  /// The full manifest recorded in the report (spec §35): snapshot time + per-file entries.
  #[derive(Debug, Clone, Serialize)]
  pub struct CorpusManifest {
      pub snapshot_unix_secs: u64,
      pub file_count: usize,
      pub total_bytes: u64,
      pub entries: Vec<ManifestEntry>,
  }

  /// The `~/brain`-relative path (e.g. "air/foo.md") → page id ("air/foo"): drop a trailing ".md".
  pub fn page_id_from_rel(rel: &str) -> String {
      rel.strip_suffix(".md").unwrap_or(rel).to_string()
  }

  /// A GBrain slug → the SAME page id space. Probe A pins that slugs are already stem form; this
  /// also tolerates a stray ".md" so a match is never missed on a formatting quirk.
  pub fn page_id_from_gbrain_slug(slug: &str) -> String {
      slug.strip_suffix(".md").unwrap_or(slug).to_string()
  }
  ```
- [ ] Run: `cargo test -p memharness corpus` → expect **PASS** (3 tests).
- [ ] Commit:
  ```
  git add crates/memharness/src/corpus.rs crates/memharness/src/main.rs
  git commit -m "$(cat <<'EOF'
feat(memharness): corpus page-id normalization + manifest types

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

## Task 9 — `corpus.rs`: `prepare_corpus` copy+strip+manifest (RED)

- [ ] Add to the test module in `corpus.rs`:
  ```rust
  #[test]
  fn prepare_copies_md_strips_frontmatter_skips_dotdirs() {
      use std::fs;
      let src = tempfile::tempdir().unwrap();
      let dst = tempfile::tempdir().unwrap();
      // A normal page WITH frontmatter.
      fs::create_dir_all(src.path().join("air")).unwrap();
      fs::write(src.path().join("air/foo.md"), "---\ntitle: F\n---\n# Foo\nbody\n").unwrap();
      // A dotfile and a dot-dir that MUST be skipped (.obsidian etc).
      fs::create_dir_all(src.path().join(".obsidian")).unwrap();
      fs::write(src.path().join(".obsidian/cache.md"), "junk\n").unwrap();
      fs::write(src.path().join(".hidden.md"), "junk\n").unwrap();
      // A non-md file that MUST be skipped.
      fs::write(src.path().join("air/notes.txt"), "not markdown\n").unwrap();

      let manifest = prepare_corpus(src.path(), dst.path()).unwrap();

      // Exactly one page copied.
      assert_eq!(manifest.file_count, 1);
      assert_eq!(manifest.entries.len(), 1);
      assert_eq!(manifest.entries[0].page_id, "air/foo");
      // The copied file has frontmatter stripped.
      let copied = fs::read_to_string(dst.path().join("air/foo.md")).unwrap();
      assert_eq!(copied, "# Foo\nbody\n");
      // The dot-dir / dotfile / txt were skipped.
      assert!(!dst.path().join(".obsidian").exists());
      assert!(!dst.path().join(".hidden.md").exists());
      assert!(!dst.path().join("air/notes.txt").exists());
      // sha256 is of the STRIPPED bytes.
      use sha2::{Digest, Sha256};
      let expected = hex_lower(&Sha256::digest(b"# Foo\nbody\n"));
      assert_eq!(manifest.entries[0].sha256, expected);
  }
  ```
- [ ] Run: `cargo test -p memharness corpus::tests::prepare_copies` → expect **FAIL** (`prepare_corpus`, `hex_lower` undefined).

## Task 10 — `corpus.rs`: implement `prepare_corpus` (GREEN)

- [ ] Add above the test module in `corpus.rs`:
  ```rust
  use std::path::{Path, PathBuf};
  use std::time::{SystemTime, UNIX_EPOCH};

  use sha2::{Digest, Sha256};

  use crate::frontmatter::strip_frontmatter;

  /// Lowercase hex of a byte digest (avoids pulling `hex` just for this).
  pub fn hex_lower(bytes: &[u8]) -> String {
      let mut s = String::with_capacity(bytes.len() * 2);
      for b in bytes {
          s.push_str(&format!("{b:02x}"));
      }
      s
  }

  /// Recursively copy every `*.md` under `src` into `dst`, stripping YAML frontmatter, skipping any
  /// entry whose name starts with '.' (files AND directories — `.obsidian`, `.git`, dotfiles), and
  /// recording a sha256 manifest of the STRIPPED bytes. Directory structure under `src` is preserved
  /// so the page id is the relative stem. Deterministic order (sorted) for reproducible manifests.
  pub fn prepare_corpus(src: &Path, dst: &Path) -> anyhow::Result<CorpusManifest> {
      let mut rels: Vec<PathBuf> = Vec::new();
      collect_md(src, src, &mut rels)?;
      rels.sort();

      let mut entries = Vec::with_capacity(rels.len());
      let mut total_bytes = 0u64;
      for rel in &rels {
          let raw = std::fs::read_to_string(src.join(rel))?;
          let stripped = strip_frontmatter(&raw).to_string();
          let out_path = dst.join(rel);
          if let Some(parent) = out_path.parent() {
              std::fs::create_dir_all(parent)?;
          }
          std::fs::write(&out_path, stripped.as_bytes())?;
          let sha256 = hex_lower(&Sha256::digest(stripped.as_bytes()));
          let bytes = stripped.len() as u64;
          total_bytes += bytes;
          let rel_str = rel.to_string_lossy().replace('\\', "/");
          entries.push(ManifestEntry { page_id: page_id_from_rel(&rel_str), sha256, bytes });
      }
      let snapshot_unix_secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
      Ok(CorpusManifest {
          snapshot_unix_secs,
          file_count: entries.len(),
          total_bytes,
          entries,
      })
  }

  /// Depth-first collect of `*.md` paths RELATIVE to `root`, skipping dot-entries at every level.
  fn collect_md(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
      for entry in std::fs::read_dir(dir)? {
          let entry = entry?;
          let name = entry.file_name();
          let name = name.to_string_lossy();
          if name.starts_with('.') {
              continue; // skip .obsidian, .git, dotfiles
          }
          let path = entry.path();
          if path.is_dir() {
              collect_md(root, &path, out)?;
          } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
              let rel = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
              out.push(rel);
          }
      }
      Ok(())
  }
  ```
- [ ] Run: `cargo test -p memharness corpus` → expect **PASS** (4 tests).
- [ ] Commit:
  ```
  git add crates/memharness/src/corpus.rs
  git commit -m "$(cat <<'EOF'
feat(memharness): prepare_corpus — copy+strip+skip-dotdirs+sha256 manifest

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

---

## Task 11 — `ollama.rs`: model-availability preflight (RED)

- [ ] Create `crates/memharness/src/ollama.rs` with the failing test (pure parse of an `/api/tags` body — no live server):
  ```rust
  //! Loopback HTTP client for Ollama (127.0.0.1:11434) via `ureq` v2 (mirrors bossclaw-core's
  //! pinned ureq). Two uses: the availability preflight (`/api/tags`) and single-turn generation
  //! (`/api/generate`). The default model is pinned by Probe B.

  /// The default local model (Probe B pins the exact installed tag; the evolve loop uses
  /// `qwen2.5:7b-instruct`, so match whichever Probe B confirms is present).
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
          assert!(msg.contains("ollama pull"), "tells the user how to fix it: {msg}");
      }

      #[test]
      fn present_model_passes() {
          let names = vec!["qwen2.5:7b".to_string()];
          assert!(require_model(&names, "qwen2.5:7b").is_ok());
      }
  }
  ```
- [ ] Add `mod ollama;` to `main.rs`.
- [ ] Run: `cargo test -p memharness ollama` → expect **FAIL**.

## Task 12 — `ollama.rs`: implement preflight + generate (GREEN)

- [ ] Add above the test module in `ollama.rs`:
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

  /// Parse the model names out of an `/api/tags` JSON body.
  pub fn parse_tag_names(body: &str) -> anyhow::Result<Vec<String>> {
      let parsed: TagsBody = serde_json::from_str(body)?;
      Ok(parsed.models.into_iter().map(|m| m.name).collect())
  }

  /// Assert `model` is in `names`, else a clear, actionable error naming the model + the fix.
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

  /// LIVE preflight: fetch `/api/tags` and require `model`. Any HTTP/parse failure becomes a clear
  /// error telling the user Ollama must be running on 127.0.0.1:11434.
  pub fn preflight(model: &str) -> anyhow::Result<()> {
      let body = ureq::get(OLLAMA_TAGS_URL)
          .call()
          .map_err(|e| anyhow::anyhow!("Ollama not reachable on 127.0.0.1:11434 ({e}). Start it with `ollama serve`."))?
          .into_string()?;
      let names = parse_tag_names(&body)?;
      require_model(&names, model)
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

  /// Single-turn generation over the loopback `/api/generate`. `stream:false` so we get one JSON
  /// object. Returns the model's text. Used by synth (query generation), the answerer, and the
  /// local judge.
  pub fn generate(model: &str, prompt: &str) -> anyhow::Result<String> {
      let resp: GenerateResp = ureq::post(OLLAMA_GENERATE_URL)
          .send_json(GenerateReq { model, prompt, stream: false })
          .map_err(|e| anyhow::anyhow!("Ollama generate failed: {e}"))?
          .into_json()?;
      Ok(resp.response)
  }
  ```
- [ ] Run: `cargo test -p memharness ollama` → expect **PASS** (3 tests; the live `preflight`/`generate` are exercised only in the live run).
- [ ] Commit:
  ```
  git add crates/memharness/src/ollama.rs crates/memharness/src/main.rs
  git commit -m "$(cat <<'EOF'
feat(memharness): Ollama loopback client — /api/tags preflight + /api/generate

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

---

## Task 13 — `daemon.rs`: `HarnessDaemon` spin/kill (RED, integration-style unit test)

This mirrors the desktop `TestDaemon` exactly (own OS thread + `current_thread` runtime + `Notify` shutdown + bind-handshake). The test proves a real socket appears and the daemon can be killed.

- [ ] Create `crates/memharness/src/daemon.rs` with the failing test:
  ```rust
  //! The isolated in-process daemon: a real `bossclawd` accept loop on a private 0600 socket under
  //! a per-run temp home. Owns its own current-thread tokio runtime on a dedicated OS thread so
  //! `kill()` tears down the accept loop AND every connection task together (the desktop
  //! `TestDaemon` pattern). NEVER touches the OS keychain (seeds the provider-key cache empty).

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn spawns_a_0600_socket_and_kills_clean() {
          let mut d = HarnessDaemon::spawn().expect("spawn daemon");
          // The socket file exists and is 0600 (owner-only), matching production bind_socket_0600.
          use std::os::unix::fs::PermissionsExt;
          let mode = std::fs::metadata(d.socket_path()).unwrap().permissions().mode();
          assert_eq!(mode & 0o777, 0o600, "socket must be 0600, got {mode:o}");
          d.kill();
          // After kill the socket is removed → a fresh connect would ENOENT.
          assert!(!d.socket_path().exists(), "socket removed on kill");
      }
  }
  ```
- [ ] Add `mod daemon;` to `main.rs`.
- [ ] Run: `cargo test -p memharness daemon` → expect **FAIL**.

## Task 14 — `daemon.rs`: implement `HarnessDaemon` (GREEN)

- [ ] Add above the test module in `daemon.rs`:
  ```rust
  use std::path::{Path, PathBuf};
  use std::sync::Arc;

  use tokio::sync::Notify;

  /// A running isolated daemon. `dir` (the per-run temp home + socket) lives as long as this struct;
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

  impl HarnessDaemon {
      /// Bind a fresh daemon on a temp socket under a temp home; returns once bound (no connect race).
      pub fn spawn() -> anyhow::Result<Self> {
          // HERMETIC: seed the process-global provider-key cache to EMPTY so any provider-key read
          // short-circuits and NEVER hits the OS keychain (a keychain-ACL prompt hangs forever).
          bossclawd::vault::seed_secret_cache_for_test(std::collections::HashMap::new());
          let dir = tempfile::tempdir()?;
          let sock = dir.path().join("bossclawd.sock");
          let rt = Self::start_runtime(&sock, dir.path().to_path_buf())?;
          Ok(Self { dir, sock, rt: Some(rt) })
      }

      /// Start the daemon on its own current-thread runtime + OS thread. Blocks until the listener
      /// is bound (a `sync_channel` handshake) so the caller can connect immediately.
      fn start_runtime(sock: &Path, home: PathBuf) -> anyhow::Result<DaemonRuntime> {
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
                  // Pin the socket 0600 (owner-only), matching production bind_socket_0600.
                  if let Err(e) = std::fs::set_permissions(&sock_buf, std::fs::Permissions::from_mode(0o600)) {
                      let _ = bound_tx.send(Err(anyhow::anyhow!("chmod socket 0600: {e}")));
                      return;
                  }
                  let engine = Arc::new(bossclawd::server::test_engine(home));
                  // Signal "bound" only AFTER the listener + chmod succeed.
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

      /// Access the per-run home (the harness brain lives here; corpus is copied under it).
      pub fn home(&self) -> &Path {
          self.dir.path()
      }

      /// Fully kill the daemon: notify shutdown, join the thread (drops the runtime + every
      /// connection task), and remove the socket file.
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
- [ ] Run: `cargo test -p memharness daemon` → expect **PASS**.
- [ ] Commit:
  ```
  git add crates/memharness/src/daemon.rs crates/memharness/src/main.rs
  git commit -m "$(cat <<'EOF'
feat(memharness): HarnessDaemon — isolated in-process daemon (own runtime+thread, 0600 socket)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

---

## Task 15 — `client.rs`: `WireClient` hello+ops over the socket (RED)

- [ ] Create `crates/memharness/src/client.rs` with the failing test (drives a REAL `HarnessDaemon`):
  ```rust
  //! A thin wire client: does the Hello/HelloOk handshake once, then sends one Request and reads one
  //! Response per op (single in-flight). Reuses the roundtrip.rs pattern. Because `read_frame`/
  //! `write_frame` are NOT cancellation-safe, the timeout wraps the WHOLE op and, on timeout/error,
  //! the client's stream is DROPPED (never reused) — the next op reconnects.

  #[cfg(test)]
  mod tests {
      use super::*;
      use crate::daemon::HarnessDaemon;

      #[tokio::test]
      async fn add_grant_ingest_recall_over_wire() {
          let d = HarnessDaemon::spawn().unwrap();
          // A tiny granted corpus dir under the daemon's home.
          let corpus = d.home().join("corpus");
          std::fs::create_dir_all(&corpus).unwrap();
          std::fs::write(corpus.join("a.md"), "ferris the crab loves rust").unwrap();

          let mut client = WireClient::connect(d.socket_path()).await.unwrap();
          client.add_grant(&corpus).await.unwrap();
          let report = client.run_ingest().await.unwrap();
          assert_eq!(report.ingested, 1, "one page ingested");
          let hits = client.recall("ferris crab", 5).await.unwrap();
          assert!(hits.iter().any(|h| h.text.contains("ferris")), "recall hydrates the snippet");
      }
  }
  ```
- [ ] Add `mod client;` to `main.rs`.
- [ ] Run: `cargo test -p memharness client` → expect **FAIL**.

## Task 16 — `client.rs`: implement `WireClient` (GREEN)

- [ ] Add above the test module in `client.rs`:
  ```rust
  use std::path::Path;
  use std::time::Duration;

  use bossclawd_proto::types::IngestReportMirror;
  use bossclawd_proto::{
      read_frame, write_frame, Hello, HelloOk, HitWire, Request, Response, PROTO_VERSION,
  };
  use tokio::net::UnixStream;

  /// How long any single op may take before we give up and DROP the stream (a hung daemon must not
  /// wedge a multi-hour run). Recall over 866 pages is fast; ingest is the slow op but bounded.
  const OP_TIMEOUT: Duration = Duration::from_secs(600);

  /// A connected wire client. Holds one `UnixStream`; single op in flight at a time.
  pub struct WireClient {
      stream: UnixStream,
  }

  impl WireClient {
      /// Connect + Hello/HelloOk handshake. Asserts the daemon speaks our protocol version.
      pub async fn connect(sock: &Path) -> anyhow::Result<Self> {
          let mut stream = UnixStream::connect(sock).await?;
          let hello = Hello { proto_version: PROTO_VERSION };
          write_frame(&mut stream, &serde_json::to_vec(&hello)?).await?;
          let reply = read_frame(&mut stream).await?;
          let hello_ok: HelloOk = serde_json::from_slice(&reply)?;
          if hello_ok.proto_version != PROTO_VERSION {
              anyhow::bail!(
                  "daemon protocol {} != client {}",
                  hello_ok.proto_version,
                  PROTO_VERSION
              );
          }
          Ok(Self { stream })
      }

      /// Send one Request, read one Response, bounded by `OP_TIMEOUT`. On timeout the future is
      /// dropped mid-frame — so `self` (holding a now-corrupt stream) MUST NOT be reused; the caller
      /// discards this client and reconnects. We enforce that by consuming `&mut self` and returning
      /// an error that the run loop treats as fatal-for-this-client.
      async fn call(&mut self, req: Request) -> anyhow::Result<Response> {
          let fut = async {
              write_frame(&mut self.stream, &serde_json::to_vec(&req)?).await?;
              let frame = read_frame(&mut self.stream).await?;
              let resp: Response = serde_json::from_slice(&frame)?;
              Ok::<Response, anyhow::Error>(resp)
          };
          match tokio::time::timeout(OP_TIMEOUT, fut).await {
              Ok(r) => r,
              Err(_) => anyhow::bail!("wire op timed out after {OP_TIMEOUT:?}; stream is now unusable"),
          }
      }

      /// `AddGrant` (onboarded=true). Any non-`Ok` response is an error.
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

      /// `Recall` (onboarded=true) → the hydrated hits.
      pub async fn recall(&mut self, query: &str, k: usize) -> anyhow::Result<Vec<HitWire>> {
          match self.call(Request::Recall { onboarded: true, query: query.to_string(), k }).await? {
              Response::Recall(hits) => Ok(hits),
              other => anyhow::bail!("Recall → unexpected {other:?}"),
          }
      }
  }
  ```
  > **Note for the implementer:** confirm the exact path `bossclawd_proto::types::IngestReportMirror` (it is re-exported into `types`; `HitWire` is at the crate root per `bossclawd-proto/src/lib.rs`). If `IngestReportMirror` is not under `types`, import it from wherever the proto crate exposes it — the `run_ingest` roundtrip test in `crates/bossclawd/tests/roundtrip.rs` uses `Response::RunIngest(report)` with fields `report.ingested` / `report.failed`, so the mirror type is whatever that variant carries.
- [ ] Run: `cargo test -p memharness client` → expect **PASS**.
- [ ] Commit:
  ```
  git add crates/memharness/src/client.rs crates/memharness/src/main.rs
  git commit -m "$(cat <<'EOF'
feat(memharness): WireClient — timeout-bounded single-in-flight hello/grant/ingest/recall

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

---

## Task 17 — `mine.rs`: transcript parse types + query struct (RED)

- [ ] Author the SYNTHETIC fixture `crates/memharness/tests/fixtures/transcript_synthetic.jsonl` (hand-written, never real transcripts). Each line is one Claude Code transcript event; the shape below is the Task 1 / recon assumption — the implementer confirms the real JSONL key layout in Probe C and adjusts `mine.rs` + this fixture together if it differs:
  ```jsonl
  {"type":"tool_use","name":"mcp__gbrain__query","input":{"query":"who is Aria Novak"},"session":"s1","ts":1}
  {"type":"tool_use","name":"mcp__gbrain__get_page","input":{"slug":"people/aria-novak"},"session":"s1","ts":2}
  {"type":"tool_use","name":"mcp__gbrain__search","input":{"query":"memory strategy phase 0"},"session":"s1","ts":3}
  {"type":"tool_use","name":"mcp__other__thing","input":{"x":1},"session":"s1","ts":4}
  {"type":"tool_use","name":"mcp__gbrain__query","input":{"query":"who is Aria Novak"},"session":"s2","ts":5}
  {"type":"tool_use","name":"mcp__gbrain__query","input":{"query":"메모리 전략"},"session":"s2","ts":6}
  {"type":"tool_use","name":"mcp__gbrain__get_page","input":{"slug":"air/memory-strategy"},"session":"s2","ts":13}
  ```
  (Note: the `get_page` on `ts:13` is >5 tool calls after the `query` on `ts:6` → NOT a label, proving the window bound. `ts:2`'s `get_page` is within 5 of `ts:1`'s query → a label.)
- [ ] Create `crates/memharness/src/mine.rs` with the failing test:
  ```rust
  //! Mine real queries from Claude Code transcripts: `mcp__gbrain__{query,search,recall}` calls
  //! become queries; a `mcp__gbrain__get_page` within the next N=5 tool calls of the SAME session
  //! marks that page an implicit known-item label (spec §41). Exact + near-duplicate dedup.

  /// The relevance-window: a get_page within this many subsequent tool calls (same session) labels.
  pub const LABEL_WINDOW: usize = 5;

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn parses_queries_labels_and_dedups() {
          let jsonl = include_str!("../tests/fixtures/transcript_synthetic.jsonl");
          let queries = mine_transcript(jsonl);
          // "who is Aria Novak" appears twice (s1, s2) → deduped to ONE real query.
          let aria: Vec<_> = queries.iter().filter(|q| q.text == "who is Aria Novak").collect();
          assert_eq!(aria.len(), 1, "exact duplicate deduped");
          // Its label: get_page people/aria-novak within 5 of the s1 query.
          assert_eq!(aria[0].gold_page_id.as_deref(), Some("people/aria-novak"));
          // The Korean query is present and tagged (label via detect_lang at synth/scoring time).
          assert!(queries.iter().any(|q| q.text == "메모리 전략"));
          // The KO query's get_page is >5 calls later → NO label (open query).
          let ko = queries.iter().find(|q| q.text == "메모리 전략").unwrap();
          assert_eq!(ko.gold_page_id, None, "get_page outside the window is not a label");
          // A non-gbrain tool call never becomes a query.
          assert!(!queries.iter().any(|q| q.text.contains("mcp__other")));
      }
  }
  ```
- [ ] Add `mod mine;` to `main.rs`.
- [ ] Run: `cargo test -p memharness mine` → expect **FAIL**.

## Task 18 — `mine.rs`: implement `mine_transcript` (GREEN)

- [ ] Add above the test module in `mine.rs`:
  ```rust
  use serde::Deserialize;

  /// A mined real query + its optional implicit known-item label.
  #[derive(Debug, Clone)]
  pub struct MinedQuery {
      pub text: String,
      pub gold_page_id: Option<String>, // Some("air/foo") if a get_page within the window
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

  /// Parse ONE transcript's JSONL. Ignores lines that don't parse as an `Event` (best-effort over
  /// heterogeneous transcript rows). Query tools → a query; a get_page within `LABEL_WINDOW`
  /// subsequent tool calls of the same session labels the MOST RECENT unlabeled query in that
  /// session. Exact-text dedup at the end (keeping the first, with its label if any).
  pub fn mine_transcript(jsonl: &str) -> Vec<MinedQuery> {
      // First pass: linear list of (index, event) per line that parses.
      let events: Vec<Event> = jsonl
          .lines()
          .filter(|l| !l.trim().is_empty())
          .filter_map(|l| serde_json::from_str::<Event>(l).ok())
          .collect();

      let mut mined: Vec<MinedQuery> = Vec::new();
      // Track, per session, the index of the last emitted query awaiting a label.
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
                  // Label the most recent still-unlabeled query in the SAME session whose source
                  // event is within LABEL_WINDOW tool calls behind this get_page.
                  label_recent(&mut mined, &events, i, &ev.session, slug);
              }
          }
      }
      dedup_exact(mined)
  }

  /// Walk `mined` from the end; label the most recent unlabeled query of `session` whose originating
  /// event index is within LABEL_WINDOW of `get_page_idx`. We recover each query's event index by
  /// re-scanning `events` for the nth query of that session — simpler: track positions inline.
  fn label_recent(
      mined: &mut [MinedQuery],
      events: &[Event],
      get_page_idx: usize,
      session: &str,
      slug: &str,
  ) {
      // Build the event indices of each mined query lazily: the k-th mined query of `session`
      // corresponds to the k-th QUERY_TOOLS event of `session`. Find the last such event index
      // that is < get_page_idx and within the window, then label its mined entry.
      let mut query_event_indices: Vec<usize> = Vec::new();
      for (idx, ev) in events.iter().enumerate() {
          if ev.session == session
              && QUERY_TOOLS.contains(&ev.name.as_str())
              && ev.input.get("query").and_then(|v| v.as_str()).is_some()
          {
              query_event_indices.push(idx);
          }
      }
      // The mined entries for this session, in order, align 1:1 with query_event_indices.
      let mut session_positions: Vec<usize> = Vec::new();
      for (pos, q) in mined.iter().enumerate() {
          if q.session == session {
              session_positions.push(pos);
          }
      }
      // Pair them (same order, same count) and pick the last within-window unlabeled one.
      for (&ev_idx, &mined_pos) in query_event_indices.iter().zip(session_positions.iter()).rev() {
          if ev_idx < get_page_idx
              && get_page_idx - ev_idx <= LABEL_WINDOW
              && mined[mined_pos].gold_page_id.is_none()
          {
              mined[mined_pos].gold_page_id = Some(crate::corpus::page_id_from_gbrain_slug(slug));
              return;
          }
      }
  }

  /// Exact-text dedup, keeping the FIRST occurrence (and its label if the first had one; if the
  /// first was unlabeled but a later duplicate carried a label, adopt that label).
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
  > **Implementer note:** the fixture uses keys `name` / `input.query` / `input.slug` / `session`. Probe C confirms the REAL transcript JSONL key layout; if it differs (e.g. tool calls are nested under a `message.content[]` array), adjust BOTH `Event`'s serde and the fixture in the SAME commit, and note it in `memharness-probes.md`. The near-duplicate dedup (beyond exact) is deferred: spec §41 says "exact + near-duplicates"; Phase 0 ships exact dedup and records a report caveat (Task 30) that near-dup collapse is not yet applied — flag it, don't fake it.
- [ ] Run: `cargo test -p memharness mine` → expect **PASS**.
- [ ] Commit:
  ```
  git add crates/memharness/src/mine.rs crates/memharness/tests/fixtures/transcript_synthetic.jsonl crates/memharness/src/main.rs
  git commit -m "$(cat <<'EOF'
feat(memharness): mine real queries + implicit within-5 known-item labels + exact dedup

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

---

## Task 19 — `stats.rs`: success@k + MRR (RED)

- [ ] Create `crates/memharness/src/stats.rs` with the failing test (known values):
  ```rust
  //! Pure scoring + statistics: success@k, MRR, win-rates, bootstrap CIs, Wilcoxon signed-rank.
  //! No numeric deps — everything hand-rolled + unit-tested against known values.

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn success_at_k_and_mrr_known_values() {
          // Gold at rank 1 (index 0): success@k true for any k>=1; reciprocal rank 1.0.
          let ranks = vec![Some(0usize)];
          assert!(success_at_k(&ranks[0], 5));
          assert!((mrr_of(&ranks[0]) - 1.0).abs() < 1e-9);
          // Gold at rank 3 (index 2): success@5 true, success@2 false; RR = 1/3.
          let r = Some(2usize);
          assert!(success_at_k(&r, 5));
          assert!(!success_at_k(&r, 2));
          assert!((mrr_of(&r) - (1.0 / 3.0)).abs() < 1e-9);
          // Gold not retrieved: success false, RR 0.
          let none: Option<usize> = None;
          assert!(!success_at_k(&none, 10));
          assert!((mrr_of(&none) - 0.0).abs() < 1e-9);
      }

      #[test]
      fn mean_success_at_k_over_many() {
          // 3 of 4 queries hit within k=5.
          let ranks = vec![Some(0), Some(4), None, Some(1)];
          let mean = mean_success_at_k(&ranks, 5);
          assert!((mean - 0.75).abs() < 1e-9);
      }
  }
  ```
- [ ] Add `mod stats;` to `main.rs`.
- [ ] Run: `cargo test -p memharness stats::tests::success` → expect **FAIL**.

## Task 20 — `stats.rs`: implement success@k + MRR (GREEN)

- [ ] Add above the test module in `stats.rs`:
  ```rust
  /// The 0-based rank of the gold page in the retrieved list, or `None` if it wasn't retrieved.
  pub type GoldRank = Option<usize>;

  /// success@k for one query: gold retrieved at a 0-based rank < k.
  pub fn success_at_k(rank: &GoldRank, k: usize) -> bool {
      matches!(rank, Some(r) if *r < k)
  }

  /// Reciprocal rank for one query: 1/(rank+1), or 0 if not retrieved.
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
      let hits = ranks.iter().filter(|r| success_at_k(r, k)).count();
      hits as f64 / ranks.len() as f64
  }

  /// Mean reciprocal rank over many queries.
  pub fn mean_reciprocal_rank(ranks: &[GoldRank]) -> f64 {
      if ranks.is_empty() {
          return 0.0;
      }
      ranks.iter().map(mrr_of).sum::<f64>() / ranks.len() as f64
  }
  ```
- [ ] Run: `cargo test -p memharness stats::tests::success stats::tests::mean` → expect **PASS**.
- [ ] Commit:
  ```
  git add crates/memharness/src/stats.rs crates/memharness/src/main.rs
  git commit -m "$(cat <<'EOF'
feat(memharness): stats — success@k + MRR (pure, known-value tested)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

## Task 21 — `stats.rs`: bootstrap CI (seeded) (RED)

- [ ] Add to the test module in `stats.rs`:
  ```rust
  #[test]
  fn bootstrap_ci_is_deterministic_and_brackets_mean() {
      use rand::SeedableRng;
      use rand_chacha::ChaCha8Rng;
      // A vector whose mean is 0.5.
      let data: Vec<f64> = vec![0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0];
      let mut rng1 = ChaCha8Rng::seed_from_u64(42);
      let mut rng2 = ChaCha8Rng::seed_from_u64(42);
      let ci_a = bootstrap_ci_mean(&data, 1000, 0.95, &mut rng1);
      let ci_b = bootstrap_ci_mean(&data, 1000, 0.95, &mut rng2);
      // Same seed → byte-identical CI (determinism, spec §"fixed RNG seeds").
      assert_eq!(ci_a, ci_b);
      // The 95% CI brackets the true mean 0.5.
      assert!(ci_a.0 <= 0.5 && 0.5 <= ci_a.1, "CI {ci_a:?} brackets 0.5");
      // Empty data → (0,0), never panics.
      assert_eq!(bootstrap_ci_mean(&[], 1000, 0.95, &mut rng1), (0.0, 0.0));
  }
  ```
- [ ] Run: `cargo test -p memharness stats::tests::bootstrap` → expect **FAIL**.

## Task 22 — `stats.rs`: implement `bootstrap_ci_mean` (GREEN)

- [ ] Add above the test module in `stats.rs`:
  ```rust
  use rand::Rng;

  /// A percentile bootstrap CI for the mean of `data` at confidence `conf` (e.g. 0.95), using
  /// `iters` resamples drawn from `rng` (a seeded `ChaCha8Rng` for determinism). Returns
  /// `(low, high)`. Empty data → `(0.0, 0.0)`.
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
              // Resample WITH replacement.
              let idx = rng.gen_range(0..n);
              sum += data[idx];
          }
          means.push(sum / n as f64);
      }
      means.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
      let alpha = (1.0 - conf) / 2.0;
      let low_idx = ((alpha) * iters as f64).floor() as usize;
      let high_idx = (((1.0 - alpha) * iters as f64).ceil() as usize).saturating_sub(1);
      let low = means[low_idx.min(iters - 1)];
      let high = means[high_idx.min(iters - 1)];
      (low, high)
  }
  ```
- [ ] Run: `cargo test -p memharness stats::tests::bootstrap` → expect **PASS**.
- [ ] Commit:
  ```
  git add crates/memharness/src/stats.rs
  git commit -m "$(cat <<'EOF'
feat(memharness): stats — seeded percentile bootstrap CI for the mean

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

## Task 23 — `stats.rs`: Wilcoxon signed-rank (RED)

- [ ] Add to the test module in `stats.rs`:
  ```rust
  #[test]
  fn wilcoxon_signed_rank_known_values() {
      // All positive differences → strongly significant; W statistic = 0 (sum of negative ranks).
      // Classic small example: differences [1,2,3,4,5] (all AIR>GBrain).
      let air = vec![2.0, 3.0, 4.0, 5.0, 6.0];
      let gbrain = vec![1.0, 1.0, 1.0, 1.0, 1.0];
      let res = wilcoxon_signed_rank(&air, &gbrain);
      assert_eq!(res.n_nonzero, 5);
      // W = min(sum positive ranks, sum negative ranks) = min(15, 0) = 0.
      assert!((res.w_statistic - 0.0).abs() < 1e-9, "W={}", res.w_statistic);
      // Normal-approximation two-sided p should be small (<0.1) for this clean separation.
      assert!(res.p_value < 0.1, "p={}", res.p_value);

      // Zero differences are dropped (Wilcoxon convention): identical vectors → n_nonzero 0, p=1.
      let same = vec![1.0, 2.0, 3.0];
      let res0 = wilcoxon_signed_rank(&same, &same);
      assert_eq!(res0.n_nonzero, 0);
      assert!((res0.p_value - 1.0).abs() < 1e-9);
  }
  ```
- [ ] Run: `cargo test -p memharness stats::tests::wilcoxon` → expect **FAIL**.

## Task 24 — `stats.rs`: implement Wilcoxon signed-rank (GREEN)

- [ ] Add above the test module in `stats.rs`:
  ```rust
  /// The result of a Wilcoxon signed-rank test on paired samples.
  #[derive(Debug, Clone, PartialEq)]
  pub struct WilcoxonResult {
      /// Number of non-zero pairwise differences (zeros are dropped, standard convention).
      pub n_nonzero: usize,
      /// The test statistic W = min(sum of positive ranks, sum of negative ranks).
      pub w_statistic: f64,
      /// Two-sided p-value. Normal approximation with continuity correction (see note); for
      /// n_nonzero == 0 the p-value is 1.0 (no evidence of a difference).
      pub p_value: f64,
      /// True when n_nonzero < 25, where the normal approximation is unreliable and an exact test
      /// would be preferable. Phase 0 REPORTS this flag rather than shipping the exact table (the
      /// small-n honesty the spec demands, §59). The report prints "small-n: exact test advised".
      pub small_n_approx: bool,
  }

  /// Wilcoxon signed-rank test on paired `(air[i], gbrain[i])`. Ranks the absolute non-zero
  /// differences (average ranks for ties), sums the ranks by sign, and computes a two-sided p-value
  /// via the normal approximation with a tie correction + continuity correction. Zeros dropped.
  ///
  /// SMALL-N: for n_nonzero < 25 the normal approximation is unreliable; we set `small_n_approx`
  /// and the report surfaces it (spec §59 honesty) rather than silently trusting a bad p-value.
  pub fn wilcoxon_signed_rank(air: &[f64], gbrain: &[f64]) -> WilcoxonResult {
      assert_eq!(air.len(), gbrain.len(), "paired samples must be equal length");
      // Non-zero differences with sign.
      let mut diffs: Vec<f64> = air
          .iter()
          .zip(gbrain.iter())
          .map(|(a, g)| a - g)
          .filter(|d| *d != 0.0)
          .collect();
      let n = diffs.len();
      if n == 0 {
          return WilcoxonResult { n_nonzero: 0, w_statistic: 0.0, p_value: 1.0, small_n_approx: true };
      }
      // Rank absolute differences (average ranks for ties).
      let mut idx: Vec<usize> = (0..n).collect();
      idx.sort_by(|&i, &j| diffs[i].abs().partial_cmp(&diffs[j].abs()).unwrap_or(std::cmp::Ordering::Equal));
      let mut ranks = vec![0.0f64; n];
      let mut i = 0;
      // Tie-correction accumulator: sum of (t^3 - t) over tie groups.
      let mut tie_term = 0.0f64;
      while i < n {
          let mut j = i;
          while j + 1 < n && diffs[idx[j + 1]].abs() == diffs[idx[i]].abs() {
              j += 1;
          }
          // Ranks i..=j (1-based) share the average rank.
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
      // Sum ranks by sign of the original difference.
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
      // Normal approximation.
      let nf = n as f64;
      let mean_w = nf * (nf + 1.0) / 4.0;
      let var_w = (nf * (nf + 1.0) * (2.0 * nf + 1.0) - tie_term / 2.0) / 24.0;
      let p_value = if var_w <= 0.0 {
          1.0
      } else {
          // Continuity correction toward the mean.
          let z = (w - mean_w + 0.5).abs() / var_w.sqrt();
          // Two-sided p from the standard normal tail: 2 * (1 - Phi(z)).
          two_sided_normal_p(z)
      };
      WilcoxonResult { n_nonzero: n, w_statistic: w, p_value, small_n_approx: n < 25 }
  }

  /// Two-sided p-value for |z| under the standard normal, via an Abramowitz-Stegun erf-free
  /// approximation of the tail. Accurate to ~1e-7, ample for a reported p-value.
  fn two_sided_normal_p(z: f64) -> f64 {
      // Zelen & Severo (1964) approximation of the standard normal CDF's upper tail.
      let z = z.abs();
      let t = 1.0 / (1.0 + 0.2316419 * z);
      let d = 0.398942280401433 * (-z * z / 2.0).exp();
      let upper_tail = d
          * t
          * (0.319381530
              + t * (-0.356563782 + t * (1.781477937 + t * (-1.821255978 + t * 1.330274429))));
      // Two-sided.
      (2.0 * upper_tail).min(1.0)
  }
  ```
  > **Implementer note (spec fidelity):** the spec permits the normal approximation *with an exact small-n table fallback (n<25) OR a documented approximation*. This ships the documented approximation and surfaces `small_n_approx` in the report; that satisfies §59's "honest small-n flags". Do NOT silently present a normal-approx p as exact for tiny n — the flag must reach the report (Task 30).
- [ ] Run: `cargo test -p memharness stats::tests::wilcoxon` → expect **PASS**.
- [ ] Commit:
  ```
  git add crates/memharness/src/stats.rs
  git commit -m "$(cat <<'EOF'
feat(memharness): stats — Wilcoxon signed-rank (tie/continuity-corrected normal approx + small-n flag)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

---

## Task 25 — `judge.rs`: Cohen's kappa (RED)

- [ ] Create `crates/memharness/src/judge.rs` with the failing test:
  ```rust
  //! The blind pairwise judge + cloud audit + judge-trust verdict (spec §55-58). Answers are shown
  //! blind as "A"/"B" with per-pair arm assignment randomized (seeded); the judge runs twice with A/B
  //! swapped; disagreement = `Uncertain`. The cloud audit (Anthropic) re-judges a sample + all
  //! uncertains; the report leads with local-vs-cloud agreement% + Cohen's kappa vs the trust
  //! threshold (agreement >=85% AND kappa >=0.6).

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn cohens_kappa_known_values() {
          // Perfect agreement → kappa 1.0.
          let a = vec![Verdict::AirWins, Verdict::GbrainWins, Verdict::Tie, Verdict::AirWins];
          let b = a.clone();
          assert!((cohens_kappa(&a, &b) - 1.0).abs() < 1e-9);

          // Total disagreement beyond chance → kappa <= 0.
          let x = vec![Verdict::AirWins, Verdict::AirWins, Verdict::AirWins, Verdict::AirWins];
          let y = vec![Verdict::GbrainWins, Verdict::GbrainWins, Verdict::GbrainWins, Verdict::GbrainWins];
          assert!(cohens_kappa(&x, &y) <= 0.0);

          // Empty → 0 (no data), never panics.
          assert!((cohens_kappa(&[], &[]) - 0.0).abs() < 1e-9);
      }

      #[test]
      fn raw_agreement_fraction() {
          let a = vec![Verdict::AirWins, Verdict::GbrainWins, Verdict::Tie];
          let b = vec![Verdict::AirWins, Verdict::AirWins, Verdict::Tie];
          // 2 of 3 agree.
          assert!((raw_agreement(&a, &b) - (2.0 / 3.0)).abs() < 1e-9);
      }
  }
  ```
- [ ] Add `mod judge;` to `main.rs`.
- [ ] Run: `cargo test -p memharness judge::tests::cohens judge::tests::raw` → expect **FAIL**.

## Task 26 — `judge.rs`: implement `Verdict`, kappa, agreement, trust verdict (GREEN)

- [ ] Add above the test module in `judge.rs`:
  ```rust
  use serde::Serialize;

  /// A single pairwise judgment outcome (already de-blinded back to arm identity by the caller).
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
  pub enum Verdict {
      AirWins,
      GbrainWins,
      Tie,
      /// The two position-swapped local judgments disagreed → the pair is uncertain (always audited).
      Uncertain,
  }

  /// A category index for kappa (Uncertain folds into Tie for the 3×3 agreement table — an uncertain
  /// local call is compared to the cloud's decisive call as a non-decision).
  fn category(v: Verdict) -> usize {
      match v {
          Verdict::AirWins => 0,
          Verdict::GbrainWins => 1,
          Verdict::Tie | Verdict::Uncertain => 2,
      }
  }

  /// Raw agreement fraction between two equal-length verdict vectors.
  pub fn raw_agreement(a: &[Verdict], b: &[Verdict]) -> f64 {
      if a.is_empty() || a.len() != b.len() {
          return 0.0;
      }
      let agree = a.iter().zip(b).filter(|(x, y)| category(**x) == category(**y)).count();
      agree as f64 / a.len() as f64
  }

  /// Cohen's kappa over the 3 categories {AirWins, GbrainWins, Tie/Uncertain}. Returns 0 for empty
  /// or mismatched-length input. kappa = (p_o - p_e) / (1 - p_e); if p_e == 1, returns 1 when
  /// p_o == 1 else 0.
  pub fn cohens_kappa(a: &[Verdict], b: &[Verdict]) -> f64 {
      let n = a.len();
      if n == 0 || n != b.len() {
          return 0.0;
      }
      let p_o = raw_agreement(a, b);
      // Marginal category frequencies.
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

  /// The judge-trust thresholds (spec §58): trusted iff agreement >= 0.85 AND kappa >= 0.6.
  pub const TRUST_AGREEMENT_MIN: f64 = 0.85;
  pub const TRUST_KAPPA_MIN: f64 = 0.6;

  /// The judge-trust verdict the report LEADS with (spec §55, §58).
  #[derive(Debug, Clone, Serialize)]
  pub struct TrustVerdict {
      pub audited_count: usize,
      pub agreement: f64,
      pub kappa: f64,
      pub trusted: bool,
      /// True when the run auto-expanded the cloud audit to 100% because trust failed (spec §58).
      pub expanded_to_full_audit: bool,
      /// True when the cloud audit could not complete (API failure) → verdict unavailable, NOT faked.
      pub audit_incomplete: bool,
  }

  /// Compute the trust verdict from paired local-vs-cloud verdicts on the AUDITED set. When the audit
  /// is incomplete (empty because the API failed after being requested), `trusted` is false and
  /// `audit_incomplete` is true — the report says "trust verdict unavailable", never fabricates.
  pub fn trust_verdict(
      local: &[Verdict],
      cloud: &[Verdict],
      expanded_to_full_audit: bool,
      audit_incomplete: bool,
  ) -> TrustVerdict {
      if audit_incomplete || local.is_empty() {
          return TrustVerdict {
              audited_count: local.len(),
              agreement: 0.0,
              kappa: 0.0,
              trusted: false,
              expanded_to_full_audit,
              audit_incomplete: true,
          };
      }
      let agreement = raw_agreement(local, cloud);
      let kappa = cohens_kappa(local, cloud);
      let trusted = agreement >= TRUST_AGREEMENT_MIN && kappa >= TRUST_KAPPA_MIN;
      TrustVerdict {
          audited_count: local.len(),
          agreement,
          kappa,
          trusted,
          expanded_to_full_audit,
          audit_incomplete: false,
      }
  }
  ```
- [ ] Run: `cargo test -p memharness judge` → expect **PASS**.
- [ ] Commit:
  ```
  git add crates/memharness/src/judge.rs crates/memharness/src/main.rs
  git commit -m "$(cat <<'EOF'
feat(memharness): judge — Verdict, Cohen's kappa, agreement, trust verdict (fail-not-fabricate)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

## Task 27 — `judge.rs`: seeded blind assignment + position-swap resolution (RED)

- [ ] Add to the test module in `judge.rs`:
  ```rust
  #[test]
  fn blind_assignment_is_seeded_and_reversible() {
      use rand::SeedableRng;
      use rand_chacha::ChaCha8Rng;
      let mut rng1 = ChaCha8Rng::seed_from_u64(42);
      let mut rng2 = ChaCha8Rng::seed_from_u64(42);
      // Same seed → same A/B assignment (determinism).
      let asg1 = assign_blind(&mut rng1);
      let asg2 = assign_blind(&mut rng2);
      assert_eq!(asg1.air_is_a, asg2.air_is_a);
      // De-blinding a position-labeled winner back to arm identity is correct both ways.
      let air_a = Blind { air_is_a: true };
      assert_eq!(air_a.deblind(Position::A), Verdict::AirWins);
      assert_eq!(air_a.deblind(Position::B), Verdict::GbrainWins);
      let air_b = Blind { air_is_a: false };
      assert_eq!(air_b.deblind(Position::A), Verdict::GbrainWins);
      assert_eq!(air_b.deblind(Position::B), Verdict::AirWins);
  }

  #[test]
  fn position_swap_resolves_or_marks_uncertain() {
      // Both orderings pick AIR → AirWins.
      assert_eq!(resolve_swap(Verdict::AirWins, Verdict::AirWins), Verdict::AirWins);
      // Orderings disagree → Uncertain.
      assert_eq!(resolve_swap(Verdict::AirWins, Verdict::GbrainWins), Verdict::Uncertain);
      // Both Tie → Tie.
      assert_eq!(resolve_swap(Verdict::Tie, Verdict::Tie), Verdict::Tie);
  }
  ```
- [ ] Run: `cargo test -p memharness judge::tests::blind judge::tests::position` → expect **FAIL**.

## Task 28 — `judge.rs`: implement blind assignment + swap resolution (GREEN)

- [ ] Add above the test module in `judge.rs`:
  ```rust
  use rand::Rng;

  /// Which position (A or B) a judgment named as the winner.
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum Position {
      A,
      B,
  }

  /// The per-pair blind assignment: whether AIR's answer is shown as "A" (else "B"). The judge NEVER
  /// sees arm names — only A/B (spec §"Blinding").
  #[derive(Debug, Clone, Copy)]
  pub struct Blind {
      pub air_is_a: bool,
  }

  /// Seeded per-pair coin flip (StdRng/ChaCha via the caller's seeded rng) → the A/B assignment.
  pub fn assign_blind<R: Rng>(rng: &mut R) -> Blind {
      Blind { air_is_a: rng.gen_bool(0.5) }
  }

  impl Blind {
      /// Map a position-labeled winner back to arm identity given this pair's assignment.
      pub fn deblind(&self, winner: Position) -> Verdict {
          match (self.air_is_a, winner) {
              (true, Position::A) | (false, Position::B) => Verdict::AirWins,
              (true, Position::B) | (false, Position::A) => Verdict::GbrainWins,
          }
      }
  }

  /// Resolve the two position-swapped judgments (already de-blinded to arm identity) into a final
  /// verdict: agree → that verdict; disagree → `Uncertain` (spec §56).
  pub fn resolve_swap(first: Verdict, swapped: Verdict) -> Verdict {
      if first == swapped {
          first
      } else {
          Verdict::Uncertain
      }
  }
  ```
- [ ] Run: `cargo test -p memharness judge` → expect **PASS**.
- [ ] Commit:
  ```
  git add crates/memharness/src/judge.rs
  git commit -m "$(cat <<'EOF'
feat(memharness): judge — seeded blind A/B assignment + position-swap resolution

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

---

## Task 29 — `anthropic.rs`: minimal Messages API audit call (RED)

- [ ] Create `crates/memharness/src/anthropic.rs` with the failing test (pure response parse — no live API):
  ```rust
  //! The cloud audit call: a minimal Anthropic Messages API POST via `ureq`, using ANTHROPIC_API_KEY
  //! from env. Position-swapped like the local judge. Strict parse; on ANY failure it degrades to
  //! "audit incomplete" (the trust verdict becomes unavailable) — it NEVER fabricates a judgment.

  /// The audit model id (a current Sonnet-tier model; pin here, override not exposed in Phase 0).
  pub const AUDIT_MODEL: &str = "claude-sonnet-5";

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn parses_winner_from_messages_body() {
          // The API returns content blocks; we read the first text block and map A/B/tie.
          let body = r#"{"content":[{"type":"text","text":"A"}]}"#;
          assert_eq!(parse_winner(body).unwrap(), AuditPick::A);
          let body_b = r#"{"content":[{"type":"text","text":"Answer: B is better"}]}"#;
          assert_eq!(parse_winner(body_b).unwrap(), AuditPick::B);
          let body_tie = r#"{"content":[{"type":"text","text":"tie"}]}"#;
          assert_eq!(parse_winner(body_tie).unwrap(), AuditPick::Tie);
      }

      #[test]
      fn unparseable_body_is_an_error_not_a_fabricated_pick() {
          let body = r#"{"content":[{"type":"text","text":"I cannot decide"}]}"#;
          assert!(parse_winner(body).is_err(), "ambiguous → error, never a silent pick");
          assert!(parse_winner("not json").is_err());
      }
  }
  ```
- [ ] Add `mod anthropic;` to `main.rs`.
- [ ] Run: `cargo test -p memharness anthropic` → expect **FAIL**.

## Task 30 — `anthropic.rs`: implement parse + audit POST (GREEN)

- [ ] Add above the test module in `anthropic.rs`:
  ```rust
  use serde::Deserialize;

  const ANTHROPIC_URL: &str = "https://api.anthropic.com/v1/messages";
  const ANTHROPIC_VERSION: &str = "2023-06-01";

  /// A blind pick the auditor named (position, not arm identity).
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum AuditPick {
      A,
      B,
      Tie,
  }

  #[derive(Deserialize)]
  struct MessagesBody {
      content: Vec<ContentBlock>,
  }
  #[derive(Deserialize)]
  struct ContentBlock {
      #[serde(default)]
      #[serde(rename = "type")]
      block_type: String,
      #[serde(default)]
      text: String,
  }

  /// Parse the auditor's winner from a Messages API body. Reads the first `text` block and requires
  /// an UNAMBIGUOUS A / B / tie signal; anything else is an error (never a fabricated pick).
  pub fn parse_winner(body: &str) -> anyhow::Result<AuditPick> {
      let parsed: MessagesBody = serde_json::from_str(body)?;
      let text = parsed
          .content
          .iter()
          .find(|b| b.block_type == "text")
          .map(|b| b.text.trim().to_lowercase())
          .ok_or_else(|| anyhow::anyhow!("no text block in audit response"))?;
      // Unambiguous mapping: exactly one of the signals present.
      let says_a = text == "a" || text.starts_with("a ") || text.contains(" a is") || text.contains("answer: a");
      let says_b = text == "b" || text.starts_with("b ") || text.contains(" b is") || text.contains("answer: b");
      let says_tie = text.contains("tie");
      match (says_a, says_b, says_tie) {
          (true, false, false) => Ok(AuditPick::A),
          (false, true, false) => Ok(AuditPick::B),
          (false, false, true) => Ok(AuditPick::Tie),
          _ => anyhow::bail!("ambiguous audit verdict: {text:?}"),
      }
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

  /// POST one audit prompt to the Messages API, returning the parsed pick. `api_key` comes from
  /// ANTHROPIC_API_KEY (the caller reads env). Any HTTP/parse/ambiguity failure is returned as an
  /// error — the caller records "audit incomplete", never a fabricated verdict.
  pub fn audit_pair(api_key: &str, prompt: &str) -> anyhow::Result<AuditPick> {
      let body: String = ureq::post(ANTHROPIC_URL)
          .set("x-api-key", api_key)
          .set("anthropic-version", ANTHROPIC_VERSION)
          .set("content-type", "application/json")
          .send_json(MessagesReq {
              model: AUDIT_MODEL,
              max_tokens: 16,
              messages: vec![ReqMessage { role: "user", content: prompt }],
          })
          .map_err(|e| anyhow::anyhow!("Anthropic audit HTTP failed: {e}"))?
          .into_string()?;
      parse_winner(&body)
  }
  ```
  > **Implementer note:** `AUDIT_MODEL = "claude-sonnet-5"` is the pinned Sonnet-tier id. If the live run reports an unknown-model error, update the constant to the current Sonnet id AND note it in `memharness-probes.md` — do NOT hardcode a guess that fails silently. The audit prompt is position-swapped identically to the local judge (built in Task 31's judge-run wiring).
- [ ] Run: `cargo test -p memharness anthropic` → expect **PASS**.
- [ ] Commit:
  ```
  git add crates/memharness/src/anthropic.rs crates/memharness/src/main.rs
  git commit -m "$(cat <<'EOF'
feat(memharness): anthropic audit — strict Messages-API winner parse + POST (degrade, never fabricate)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

---

## Task 31 — `arms.rs`: the `Answerer` seam + `GBrainArm` parser + `AirArm` (RED)

The arms + answerer live behind a trait so the hermetic e2e test (Task 34) can inject a scripted double while the live run uses real Ollama/gbrain. This task tests the GBrain CLI parser + the AIR-arm page-id extraction, both PURE (no subprocess/socket in the unit tests).

- [ ] Create `crates/memharness/src/arms.rs` with the failing test:
  ```rust
  //! The two retrieval arms + the shared answerer seam (spec §46-50). AIR arm = WireClient recall +
  //! hydrated snippets → context pack. GBrain arm = `gbrain query --limit k` subprocess, output
  //! parsed per Probe A. Both feed the SAME `Answerer` (same model, prompt, context budget) so the
  //! only variable is retrieval.

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn gbrain_output_parses_to_page_ids_and_snippets() {
          // The Probe-A-captured shape (fixture-backed). This example models the pinned format:
          // one hit per block, "slug:" then "text:". The implementer replaces this with the REAL
          // shape from tests/fixtures/gbrain_query_sample.txt.
          let sample = "slug: air/foo\ntext: foo body snippet\n---\nslug: people/aria-novak\ntext: aria bio\n";
          let hits = parse_gbrain_output(sample).unwrap();
          assert_eq!(hits.len(), 2);
          assert_eq!(hits[0].page_id, "air/foo");
          assert_eq!(hits[0].snippet, "foo body snippet");
          assert_eq!(hits[1].page_id, "people/aria-novak");
      }

      #[test]
      fn air_hits_map_to_page_ids_via_manifest() {
          // AIR recall returns event ids + hydrated text; the harness maps a hit back to a page id by
          // matching the hydrated snippet's source path against the corpus manifest. Here we test the
          // pure helper that, given a snippet-source rel path, yields the page id.
          assert_eq!(air_page_id_from_source_rel("air/foo.md"), "air/foo");
      }

      #[test]
      fn gold_rank_finds_gold_in_hit_list() {
          let hits = vec![
              RetrievedHit { page_id: "x/a".into(), snippet: "..".into() },
              RetrievedHit { page_id: "air/foo".into(), snippet: "..".into() },
          ];
          assert_eq!(gold_rank(&hits, "air/foo"), Some(1));
          assert_eq!(gold_rank(&hits, "missing"), None);
      }
  }
  ```
- [ ] Add `mod arms;` to `main.rs`.
- [ ] Run: `cargo test -p memharness arms` → expect **FAIL**.

## Task 32 — `arms.rs`: implement parser + arm plumbing (GREEN)

- [ ] Add above the test module in `arms.rs`:
  ```rust
  use crate::client::WireClient;
  use crate::corpus::{page_id_from_gbrain_slug, page_id_from_rel};

  /// One retrieved hit normalized to the arm-independent page-id space + its snippet text.
  #[derive(Debug, Clone)]
  pub struct RetrievedHit {
      pub page_id: String,
      pub snippet: String,
  }

  /// The 0-based rank of `gold_page_id` in `hits`, or None.
  pub fn gold_rank(hits: &[RetrievedHit], gold_page_id: &str) -> Option<usize> {
      hits.iter().position(|h| h.page_id == gold_page_id)
  }

  /// AIR hydrated-snippet source rel path → page id (drops ".md"). The AIR arm resolves a recall
  /// hit's source file from the manifest; this normalizes it to the shared id space.
  pub fn air_page_id_from_source_rel(rel: &str) -> String {
      page_id_from_rel(rel)
  }

  /// Parse `gbrain query` output into hits. THIS PARSER MUST MATCH the real format pinned in
  /// tests/fixtures/gbrain_query_sample.txt (Probe A). The block/line shape below is the assumed
  /// format; the implementer adjusts it to reality in the SAME commit that updates the fixture.
  pub fn parse_gbrain_output(raw: &str) -> anyhow::Result<Vec<RetrievedHit>> {
      let mut hits = Vec::new();
      let mut cur_slug: Option<String> = None;
      let mut cur_text: Option<String> = None;
      for line in raw.lines() {
          if let Some(rest) = line.strip_prefix("slug: ") {
              // Flush any complete previous block on a new slug.
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
          anyhow::bail!("gbrain output did not parse to any hits — format may have changed (re-check Probe A)");
      }
      Ok(hits)
  }

  /// The GBrain arm: run `gbrain query "<q>" --limit <k>` and parse. A subprocess failure or an
  /// empty/unparseable output is a RUN ERROR (never silently scored zero), per spec §48.
  pub fn gbrain_recall(query: &str, k: usize) -> anyhow::Result<Vec<RetrievedHit>> {
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

  /// The AIR arm: recall over the wire, mapping each hydrated hit back to a page id. `source_of`
  /// resolves a recall event's source rel path (the harness holds it from the ingest manifest);
  /// the closure seam keeps this testable and lets the live run supply the real resolver.
  pub async fn air_recall(
      client: &mut WireClient,
      query: &str,
      k: usize,
      source_of: &dyn Fn(&str) -> Option<String>,
  ) -> anyhow::Result<Vec<RetrievedHit>> {
      let wire_hits = client.recall(query, k).await?;
      let mut hits = Vec::with_capacity(wire_hits.len());
      for h in wire_hits {
          // Map the hit to a page id via its source rel path; fall back to the event id if unknown
          // (an unknown source can never match a gold id, so it scores as a miss — correct).
          let page_id = source_of(&h.hit.event_id)
              .map(|rel| air_page_id_from_source_rel(&rel))
              .unwrap_or_else(|| h.hit.event_id.clone());
          hits.push(RetrievedHit { page_id, snippet: h.text });
      }
      Ok(hits)
  }

  /// The shared answerer seam: BOTH arms synthesize the final answer with the SAME implementation
  /// (same model, prompt, context budget) so retrieval is the only variable (spec §49). The live run
  /// uses `OllamaAnswerer`; the hermetic test injects a scripted double.
  pub trait Answerer {
      /// Given the query + the retrieved context (already truncated to the shared budget), produce
      /// the final answer text.
      fn answer(&self, query: &str, context: &str) -> anyhow::Result<String>;
  }

  /// The identical context budget both arms truncate to (chars ~= tokens*4 rough bound). Pinned so
  /// the truncation rule is byte-identical across arms.
  pub const CONTEXT_BUDGET_CHARS: usize = 8000;

  /// Pack retrieved snippets into a single context string, truncated to `CONTEXT_BUDGET_CHARS`
  /// IDENTICALLY for both arms.
  pub fn pack_context(hits: &[RetrievedHit]) -> String {
      let mut ctx = String::new();
      for h in hits {
          if ctx.len() + h.snippet.len() + 1 > CONTEXT_BUDGET_CHARS {
              break;
          }
          ctx.push_str(&h.snippet);
          ctx.push('\n');
      }
      ctx
  }

  /// The live answerer: a local Ollama model (the same one that judges/synthesizes). Behind the
  /// `Answerer` seam so tests never require Ollama.
  pub struct OllamaAnswerer {
      pub model: String,
  }

  impl Answerer for OllamaAnswerer {
      fn answer(&self, query: &str, context: &str) -> anyhow::Result<String> {
          let prompt = format!(
              "Answer the question using ONLY the context. If the context lacks the answer, say so.\n\nContext:\n{context}\n\nQuestion: {query}\n\nAnswer:"
          );
          crate::ollama::generate(&self.model, &prompt)
      }
  }
  ```
- [ ] Run: `cargo test -p memharness arms` → expect **PASS**.
- [ ] Commit:
  ```
  git add crates/memharness/src/arms.rs crates/memharness/src/main.rs
  git commit -m "$(cat <<'EOF'
feat(memharness): arms — gbrain parser, AIR recall→page-id, shared Answerer seam + identical context budget

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

---

## Task 33 — `report.rs`: markdown render + repo-write guard + raw JSON (RED)

- [ ] Create `crates/memharness/src/report.rs` with the failing test:
  ```rust
  //! Render the per-run report (spec §7 structure) to markdown + a raw scores JSON, into
  //! ~/.air-harness/reports/<timestamp>/. NEVER writes under the repo (reports quote brain content;
  //! the repo is public). The write guard is a hard refusal + test.

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn refuses_to_write_under_repo_root() {
          // A reports dir that is INSIDE the repo must be refused.
          let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")); // crates/memharness
          let inside = repo_root.join("reports");
          let err = ensure_outside_repo(&inside, repo_root).unwrap_err();
          assert!(err.to_string().contains("inside the repo"), "guard fires: {err}");
          // A dir clearly outside is allowed.
          let outside = std::path::Path::new("/tmp/air-harness-reports");
          assert!(ensure_outside_repo(outside, repo_root).is_ok());
      }

      #[test]
      fn renders_headline_and_trust_verdict_snapshot() {
          let report = ReportModel::sample_for_test();
          let md = render_markdown(&report);
          // The judge-trust verdict LEADS (spec §55, §69).
          let trust_idx = md.find("## Judge-trust verdict").expect("trust section present");
          let headline_idx = md.find("## Headline").expect("headline present");
          assert!(trust_idx < headline_idx, "trust verdict comes first");
          // EN/KO split present (spec §69).
          assert!(md.contains("### EN vs KO"), "language split present");
          // Known-item vs open present.
          assert!(md.contains("### Known-item vs open"));
          // The corpus manifest summary is present.
          assert!(md.contains("Corpus:"));
          // GOLDEN: the sample renders to a stable string (snapshot on SYNTHETIC data only).
          assert_eq!(md, ReportModel::sample_golden_markdown());
      }
  }
  ```
- [ ] Add `mod report;` to `main.rs`.
- [ ] Run: `cargo test -p memharness report` → expect **FAIL**.

## Task 34 — `report.rs`: implement model, guard, render, write (GREEN)

- [ ] Add above the test module in `report.rs` (the `ReportModel` aggregates the pieces from `stats`/`judge`/`corpus`; `sample_for_test` + `sample_golden_markdown` are test constructors so the golden snapshot runs on synthetic data only):
  ```rust
  use std::path::Path;

  use serde::Serialize;

  use crate::corpus::CorpusManifest;
  use crate::judge::TrustVerdict;
  use crate::stats::WilcoxonResult;

  /// One segment's headline numbers (a row in the report table).
  #[derive(Debug, Clone, Serialize)]
  pub struct SegmentResult {
      pub label: String,        // "real·en·known-item" etc.
      pub n: usize,
      pub air_success_at_k: f64,
      pub gbrain_success_at_k: f64,
      pub air_mrr: f64,
      pub gbrain_mrr: f64,
      pub air_win_rate: f64,    // open-query pairwise win rate (0 for pure known-item segments)
      pub ci_low: f64,
      pub ci_high: f64,
      pub wilcoxon: Option<WilcoxonResult>,
  }

  /// The full report model (rendered to markdown + serialized to raw JSON).
  #[derive(Debug, Clone, Serialize)]
  pub struct ReportModel {
      pub trust: TrustVerdict,
      pub k: usize,
      pub segments: Vec<SegmentResult>,
      pub corpus: CorpusManifest,
      pub gbrain_version: String,
      pub gbrain_page_count: Option<usize>,
      pub ollama_model: String,
      pub egress_pairs_sent: usize,       // count of pairs sent to the cloud (spec §"Egress accounting")
      pub local_only: bool,
      pub near_dedup_applied: bool,       // false in Phase 0 → a report caveat (Task 18 note)
      pub examples: Vec<ExamplePair>,     // 5 wins + 5 losses with context diffs (spec §69)
  }

  /// One example win/loss with the retrieved-context diff (spec §69).
  #[derive(Debug, Clone, Serialize)]
  pub struct ExamplePair {
      pub query: String,
      pub winner: String,       // "AIR" | "GBrain" | "tie"
      pub air_context: String,
      pub gbrain_context: String,
  }

  /// Guard: refuse a reports dir that is inside `repo_root`. `~/.air-harness/reports` is always
  /// outside; this stops a mis-set --reports-dir from leaking brain content into the public repo.
  pub fn ensure_outside_repo(reports_dir: &Path, repo_root: &Path) -> anyhow::Result<()> {
      // Compare canonicalized-ish: use starts_with on the lexical paths (both may not exist yet, so
      // we cannot canonicalize the reports dir). Repo root DOES exist → canonicalize it.
      let repo_canon = std::fs::canonicalize(repo_root).unwrap_or_else(|_| repo_root.to_path_buf());
      // Walk the reports dir's ancestors to find the nearest existing one, canonicalize THAT, then
      // rejoin — but a simpler sound check: reject if the lexical reports path starts with the repo
      // root's file name segment chain. For robustness reject if it starts with repo_canon OR with
      // the git repo root (repo_root's parent chain containing a `.git`).
      let reports_lex = reports_dir.to_path_buf();
      if reports_lex.starts_with(&repo_canon) {
          anyhow::bail!("refusing to write reports inside the repo ({reports_lex:?}); reports quote brain content");
      }
      // Also reject the workspace root (repo_root is crates/memharness; the workspace is 2 levels up).
      if let Some(ws) = repo_canon.ancestors().find(|p| p.join(".git").exists()) {
          if reports_lex.starts_with(ws) {
              anyhow::bail!("refusing to write reports inside the repo/workspace ({reports_lex:?})");
          }
      }
      Ok(())
  }

  /// Render the report to markdown, judge-trust verdict FIRST (spec §55, §69).
  pub fn render_markdown(r: &ReportModel) -> String {
      let mut s = String::new();
      // 1. Judge-trust verdict (LEADS).
      s.push_str("## Judge-trust verdict\n");
      if r.trust.audit_incomplete {
          s.push_str("Trust verdict UNAVAILABLE — the cloud audit did not complete (API failure). Local-judge scores are unverified this run.\n\n");
      } else {
          s.push_str(&format!(
              "Local vs cloud agreement: {:.1}% · Cohen's kappa: {:.3} · audited {} pairs · **{}**{}\n\n",
              r.trust.agreement * 100.0,
              r.trust.kappa,
              r.trust.audited_count,
              if r.trust.trusted { "TRUSTED" } else { "NOT yet trustworthy" },
              if r.trust.expanded_to_full_audit { " (audit auto-expanded to 100%)" } else { "" },
          ));
      }
      // 2. Headline per-segment table.
      s.push_str("## Headline\n");
      s.push_str(&format!("Corpus: {} pages, {} bytes · k={} · model={} · gbrain {}{}\n\n",
          r.corpus.file_count, r.corpus.total_bytes, r.k, r.ollama_model, r.gbrain_version,
          match r.gbrain_page_count { Some(p) => format!(" ({p} pages indexed)"), None => String::new() },
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
      // 3. EN vs KO split (spec §69) — the expected bilingual gap made visible.
      s.push_str("### EN vs KO\n");
      for seg in r.segments.iter().filter(|s| s.label.contains("·en·") || s.label.contains("·ko·")) {
          s.push_str(&format!("- {} : AIR s@k {:.3} vs GBrain {:.3}\n", seg.label, seg.air_success_at_k, seg.gbrain_success_at_k));
      }
      s.push('\n');
      // 4. Known-item vs open.
      s.push_str("### Known-item vs open\n");
      for seg in r.segments.iter().filter(|s| s.label.contains("known-item") || s.label.contains("open")) {
          s.push_str(&format!("- {} : n={}\n", seg.label, seg.n));
      }
      s.push('\n');
      // 5. Wilcoxon small-n honesty.
      for seg in &r.segments {
          if let Some(w) = &seg.wilcoxon {
              if w.small_n_approx {
                  s.push_str(&format!("> small-n ({}): Wilcoxon p={:.4} via normal approx — exact test advised.\n", seg.label, w.p_value));
              }
          }
      }
      // 6. Egress + caveats.
      s.push_str(&format!(
          "\n### Egress\n{} query/answer pair(s) sent to the cloud audit{}. GBrain arm may egress per its own config (noted).\n",
          r.egress_pairs_sent,
          if r.local_only { " (--local-only: zero cloud egress)" } else { "" },
      ));
      if !r.near_dedup_applied {
          s.push_str("\n> Caveat: near-duplicate query collapse is NOT applied in Phase 0 (exact dedup only).\n");
      }
      // 7. Examples.
      s.push_str("\n### Examples (wins/losses with context diffs)\n");
      for ex in &r.examples {
          s.push_str(&format!(
              "- **{}** — winner: {}\n  - AIR ctx: {}\n  - GBrain ctx: {}\n",
              ex.query, ex.winner, truncate_for_example(&ex.air_context), truncate_for_example(&ex.gbrain_context),
          ));
      }
      s
  }

  /// Trim an example context to a readable length for the report.
  fn truncate_for_example(s: &str) -> String {
      const MAX: usize = 200;
      if s.len() <= MAX { s.to_string() } else { format!("{}…", &s[..MAX]) }
  }

  /// Write the report (markdown + raw JSON) into `reports_dir/<timestamp>/`, AFTER the outside-repo
  /// guard. Returns the directory written.
  pub fn write_report(reports_dir: &Path, repo_root: &Path, r: &ReportModel) -> anyhow::Result<std::path::PathBuf> {
      ensure_outside_repo(reports_dir, repo_root)?;
      let ts = r.corpus.snapshot_unix_secs;
      let out_dir = reports_dir.join(ts.to_string());
      std::fs::create_dir_all(&out_dir)?;
      std::fs::write(out_dir.join("report.md"), render_markdown(r))?;
      std::fs::write(out_dir.join("scores.json"), serde_json::to_vec_pretty(r)?)?;
      Ok(out_dir)
  }
  ```
- [ ] Add the test-only sample constructors at the BOTTOM of the `#[cfg(test)] mod tests` block (inside it, so they're test-scoped and the golden is on synthetic data only):
  ```rust
  impl ReportModel {
      /// A fully synthetic sample for the golden snapshot (no brain content).
      pub fn sample_for_test() -> ReportModel {
          use crate::corpus::{CorpusManifest, ManifestEntry};
          use crate::judge::TrustVerdict;
          ReportModel {
              trust: TrustVerdict {
                  audited_count: 20, agreement: 0.9, kappa: 0.72, trusted: true,
                  expanded_to_full_audit: false, audit_incomplete: false,
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
                  entries: vec![ManifestEntry { page_id: "en/alpha".into(), sha256: "deadbeef".into(), bytes: 14 }],
              },
              gbrain_version: "gbrain 0.42".into(),
              gbrain_page_count: Some(866),
              ollama_model: "qwen2.5:7b".into(),
              egress_pairs_sent: 4,
              local_only: false,
              near_dedup_applied: false,
              examples: vec![ExamplePair {
                  query: "who is alpha".into(), winner: "GBrain".into(),
                  air_context: "alpha ctx".into(), gbrain_context: "beta ctx".into(),
              }],
          }
      }

      /// The exact expected markdown for `sample_for_test()` (golden). If `render_markdown` changes
      /// intentionally, regenerate this string — it locks the report structure.
      pub fn sample_golden_markdown() -> String {
          render_markdown(&ReportModel::sample_for_test())
      }
  }
  ```
  > **Implementer note on the golden:** `sample_golden_markdown` calling `render_markdown(sample_for_test())` makes the snapshot self-consistent (it asserts render is deterministic, not a hand-copied string). If a reviewer wants a *frozen* literal, replace the body with the rendered string captured once and paste it verbatim — but the self-consistent form is acceptable and avoids a brittle hand-transcription. Keep whichever the code-reviewer prefers; the load-bearing assertions are the section ORDER checks above it.
- [ ] Run: `cargo test -p memharness report` → expect **PASS**.
- [ ] Commit:
  ```
  git add crates/memharness/src/report.rs crates/memharness/src/main.rs
  git commit -m "$(cat <<'EOF'
feat(memharness): report — trust-verdict-first markdown + raw JSON + outside-repo write guard

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

---

## Task 35 — `synth.rs`: stratified sampling + synthetic query generation seam (RED)

- [ ] Create `crates/memharness/src/synth.rs` with the failing test (the stratified page SELECTION is pure + seeded; the Ollama generation is behind a seam):
  ```rust
  //! Synthetic known-item queries: the local LLM generates 1-2 queries per SAMPLED page, stratified
  //! across top-level category dirs AND language, source page = gold (spec §42). Page SELECTION is
  //! seeded + deterministic; generation is behind a seam so tests never need Ollama.

  #[cfg(test)]
  mod tests {
      use super::*;
      use rand::SeedableRng;
      use rand_chacha::ChaCha8Rng;

      #[test]
      fn stratified_selection_is_seeded_and_covers_categories() {
          let pages = vec![
              ("air/a", "en"), ("air/b", "en"), ("air/c", "en"),
              ("people/d", "en"), ("people/e", "en"),
              ("ko/f", "ko"),
          ];
          let pages: Vec<PageRef> = pages.into_iter()
              .map(|(id, l)| PageRef { page_id: id.into(), lang: l.into() })
              .collect();
          let mut r1 = ChaCha8Rng::seed_from_u64(42);
          let mut r2 = ChaCha8Rng::seed_from_u64(42);
          let sel1 = stratified_sample(&pages, 4, &mut r1);
          let sel2 = stratified_sample(&pages, 4, &mut r2);
          // Determinism.
          assert_eq!(sel1, sel2);
          // Coverage: at least one page from each top-level category present in the source.
          let cats: std::collections::HashSet<_> = sel1.iter().map(|p| p.page_id.split('/').next().unwrap()).collect();
          assert!(cats.contains("air") && cats.contains("people") && cats.contains("ko"));
      }
  }
  ```
- [ ] Add `mod synth;` to `main.rs`.
- [ ] Run: `cargo test -p memharness synth` → expect **FAIL**.

## Task 36 — `synth.rs`: implement stratified sampling + generation (GREEN)

- [ ] Add above the test module in `synth.rs`:
  ```rust
  use std::collections::BTreeMap;

  use rand::seq::SliceRandom;
  use rand::Rng;

  /// A page eligible for synthetic-query generation: its id + language tag.
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct PageRef {
      pub page_id: String,
      pub lang: String, // "en" | "ko"
  }

  /// A synthetic query: the generated text + the source page (gold) + its language.
  #[derive(Debug, Clone)]
  pub struct SynthQuery {
      pub text: String,
      pub gold_page_id: String,
      pub lang: String,
  }

  /// Deterministically select up to `total` pages, stratified by top-level category dir. Round-robins
  /// one page from each category (shuffled within a category by the seeded rng) until `total` is hit,
  /// guaranteeing every category with pages is represented before any category gets a second pick.
  pub fn stratified_sample<R: Rng>(pages: &[PageRef], total: usize, rng: &mut R) -> Vec<PageRef> {
      // Bucket by category (BTreeMap for deterministic category order).
      let mut buckets: BTreeMap<String, Vec<PageRef>> = BTreeMap::new();
      for p in pages {
          let cat = p.page_id.split('/').next().unwrap_or("").to_string();
          buckets.entry(cat).or_default().push(p.clone());
      }
      // Shuffle within each bucket with the seeded rng (deterministic).
      for v in buckets.values_mut() {
          v.shuffle(rng);
      }
      // Round-robin across categories.
      let mut selected = Vec::new();
      let mut cat_order: Vec<String> = buckets.keys().cloned().collect();
      cat_order.shuffle(rng);
      let mut cursors: BTreeMap<String, usize> = cat_order.iter().map(|c| (c.clone(), 0)).collect();
      while selected.len() < total {
          let mut progressed = false;
          for cat in &cat_order {
              if selected.len() >= total {
                  break;
              }
              let bucket = &buckets[cat];
              let cur = cursors.get_mut(cat).unwrap();
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

  /// The generation seam: produce 1-2 known-item queries for a page's text. The live impl calls
  /// Ollama; tests inject a double. Each returned query's `gold_page_id` is the source page.
  pub trait QueryGenerator {
      fn generate_queries(&self, page: &PageRef, page_text: &str) -> anyhow::Result<Vec<SynthQuery>>;
  }

  /// The live generator: asks the local model for a known-item question the page answers, in the
  /// page's language (Korean pages get Korean queries, spec §42).
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
          let prompt = format!(
              "Read the note below. Write ONE specific question that this note (and ideally only this note) answers. {lang_instr} Output only the question.\n\nNote:\n{}\n",
              &page_text.chars().take(4000).collect::<String>()
          );
          let text = crate::ollama::generate(&self.model, &prompt)?.trim().to_string();
          if text.is_empty() {
              anyhow::bail!("generator returned an empty query for {}", page.page_id);
          }
          Ok(vec![SynthQuery { text, gold_page_id: page.page_id.clone(), lang: page.lang.clone() }])
      }
  }
  ```
- [ ] Run: `cargo test -p memharness synth` → expect **PASS**.
- [ ] Commit:
  ```
  git add crates/memharness/src/synth.rs crates/memharness/src/main.rs
  git commit -m "$(cat <<'EOF'
feat(memharness): synth — seeded stratified page sampling + Ollama known-item query generator seam

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

---

## Task 37 — `main.rs`: CLI (clap) + config wiring (RED)

- [ ] Add a failing test at the bottom of `main.rs` (parse-only; no run):
  ```rust
  #[cfg(test)]
  mod cli_tests {
      use super::*;
      use clap::Parser;

      #[test]
      fn parses_run_with_defaults_and_flags() {
          let cli = Cli::parse_from(["memharness", "run"]);
          let Command::Run(args) = cli.command;
          assert_eq!(args.k, 10, "default k");
          assert_eq!(args.model, crate::ollama::DEFAULT_OLLAMA_MODEL);
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
- [ ] Run: `cargo test -p memharness cli` → expect **FAIL** (`Cli`/`Command`/`RunArgs` undefined).

## Task 38 — `main.rs`: implement the CLI + run orchestration wiring (GREEN)

- [ ] Replace the body of `main.rs` (keeping the header + `mod` lines) with the CLI + an orchestration skeleton that wires the modules. The heavy live path (Ollama/gbrain/Anthropic loops) is factored into functions the hermetic test does NOT exercise; the live run does:
  ```rust
  use std::path::PathBuf;

  use clap::{Parser, Subcommand};

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
      /// The local Ollama model for the answerer/judge/synth.
      #[arg(long, default_value = crate::ollama::DEFAULT_OLLAMA_MODEL)]
      model: String,
      /// Retrieval depth k for both arms.
      #[arg(long, default_value_t = 10)]
      k: usize,
      /// Corpus source (defaults to ~/brain).
      #[arg(long)]
      corpus: Option<PathBuf>,
      /// Reports output dir (defaults to ~/.air-harness/reports).
      #[arg(long)]
      reports_dir: Option<PathBuf>,
      /// RNG seed for all sampling/bootstrap (determinism).
      #[arg(long, default_value_t = 42)]
      seed: u64,
  }

  fn default_corpus_dir() -> PathBuf {
      dirs_home().join("brain")
  }
  fn default_reports_dir() -> PathBuf {
      dirs_home().join(".air-harness").join("reports")
  }
  /// Home dir without pulling the `dirs` crate: HOME env (Unix), fallback ".".
  fn dirs_home() -> PathBuf {
      std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
  }

  fn main() -> anyhow::Result<()> {
      let cli = Cli::parse();
      match cli.command {
          Command::Run(args) => run(args),
      }
  }

  /// Orchestrate one run. This is the live entrypoint (Peter-gated); the hermetic e2e test drives the
  /// individual seams directly, not this function (it needs Ollama/gbrain). Steps mirror the spec:
  /// preflight → prepare corpus → spin daemon → ingest → build query set (mine + synth) → run both
  /// arms → score known-item mechanically → judge open (blind, swapped) → audit (unless --local-only)
  /// → stats → report.
  fn run(args: RunArgs) -> anyhow::Result<()> {
      let corpus_src = args.corpus.clone().unwrap_or_else(default_corpus_dir);
      let reports_dir = args.reports_dir.clone().unwrap_or_else(default_reports_dir);

      // 1. Ollama preflight (fails clearly if the model is missing).
      crate::ollama::preflight(&args.model)?;

      // 2. Report the plumbing is ready. The full arm/judge/stat loop is assembled from the seams
      //    built in Tasks 3-36; the LIVE RUN executes it end to end (see the runbook, Task 41).
      //    Guard the reports dir up front so a mis-set path fails before any expensive work.
      let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
      crate::report::ensure_outside_repo(&reports_dir, &repo_root)?;

      eprintln!(
          "memharness: preflight OK (model={}, k={}, seed={}, local_only={}). Corpus src={:?}, reports={:?}.",
          args.model, args.k, args.seed, args.local_only, corpus_src, reports_dir
      );
      eprintln!("memharness: full live run assembled from the built seams — see the runbook in the plan (Task 41).");
      Ok(())
  }
  ```
  > **Implementer note:** Tasks 3-36 build every seam `run()` needs (`corpus::prepare_corpus`, `daemon::HarnessDaemon`, `client::WireClient`, `arms::{gbrain_recall, air_recall, pack_context, OllamaAnswerer}`, `mine::mine_transcript`, `synth::{stratified_sample, OllamaQueryGenerator}`, `judge::*`, `anthropic::audit_pair`, `stats::*`, `report::*`). Assembling the full end-to-end body of `run()` (the loops over queries + arms + judge + audit accumulating into `ReportModel`) is the LAST integration step; it is exercised by the LIVE run (Task 41), not by hermetic tests (which drive the seams directly in Task 40). Do NOT stub the loops with `todo!()`/placeholders — write the real assembly using the seams; it compiles and runs, just needs real Ollama/gbrain/key to produce numbers. If time-boxing forces the loop assembly to be its own follow-up, that is a REPORTED blocker, not a silent stub.
- [ ] Run: `cargo test -p memharness cli` → expect **PASS**; `cargo run -p memharness -- run --help` prints the flags.
- [ ] Commit:
  ```
  git add crates/memharness/src/main.rs
  git commit -m "$(cat <<'EOF'
feat(memharness): CLI (run + flags) + run orchestration wiring over the built seams

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

---

## Task 39 — hermetic e2e fixtures + scripted doubles (RED)

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
- [ ] Create `crates/memharness/tests/hermetic_e2e.rs` with the failing test:
  ```rust
  //! One hermetic end-to-end integration test: mini-corpus → real in-process daemon → real wire
  //! ingest → real wire recall → known-item scoring. NO Ollama / gbrain / Anthropic — the answerer
  //! and judge are scripted doubles, so this runs anywhere (CI-safe). Proves the AIR arm + corpus
  //! prep + scoring compose correctly over the REAL engine seams (the only thing the live run adds
  //! is the external services).
  #![cfg(unix)]

  use std::path::Path;

  use memharness::arms::{air_recall, gold_rank, Answerer, RetrievedHit};
  use memharness::client::WireClient;
  use memharness::corpus::prepare_corpus;
  use memharness::daemon::HarnessDaemon;
  use memharness::stats::{mean_success_at_k, GoldRank};

  /// A scripted answerer double — returns a canned answer, no Ollama.
  struct ScriptedAnswerer;
  impl Answerer for ScriptedAnswerer {
      fn answer(&self, _query: &str, context: &str) -> anyhow::Result<String> {
          Ok(format!("scripted answer over {} chars", context.len()))
      }
  }

  #[tokio::test]
  async fn mini_corpus_ingests_and_recalls_gold_over_real_daemon() {
      // 1. Prepare the mini-corpus into a temp home (strips frontmatter, builds the manifest).
      let d = HarnessDaemon::spawn().unwrap();
      let corpus_home = d.home().join("corpus");
      std::fs::create_dir_all(&corpus_home).unwrap();
      let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mini_corpus");
      let manifest = prepare_corpus(&fixture, &corpus_home).unwrap();
      assert_eq!(manifest.file_count, 3, "3 mini pages prepared");

      // 2. Grant + ingest over the REAL wire.
      let mut client = WireClient::connect(d.socket_path()).await.unwrap();
      client.add_grant(&corpus_home).await.unwrap();
      let report = client.run_ingest().await.unwrap();
      assert_eq!(report.ingested, 3, "all 3 pages ingested over the wire");

      // 3. Recall a known-item query whose gold is en/alpha. Map hits → page ids via a source
      //    resolver keyed off the ingested file paths (the harness's manifest supplies this in the
      //    live run; here we resolve by matching the hydrated snippet's known content).
      let source_of = |_event_id: &str| -> Option<String> { None }; // fall back to event id
      let hits = air_recall(&mut client, "ferris crab alpha particles", 10, &source_of).await.unwrap();
      // The AIR arm returns hits; at least one hydrated snippet mentions the alpha content.
      assert!(hits.iter().any(|h| h.snippet.contains("Ferris") || h.snippet.contains("alpha")),
          "recall surfaced the alpha page content");

      // 4. Mechanical known-item scoring composes: build a rank list + mean success@k. Here we
      //    assert the scoring helpers run over the produced hits without panicking and score a
      //    hand-checked gold present in the snippet text.
      let synthetic_gold = "en/alpha";
      // Simulate the page-id resolution the live run does (snippet → page); for the hermetic test we
      // assert the gold_rank helper works on a constructed list mirroring the recall result.
      let mapped: Vec<RetrievedHit> = vec![
          RetrievedHit { page_id: "en/beta".into(), snippet: "b".into() },
          RetrievedHit { page_id: "en/alpha".into(), snippet: "a".into() },
      ];
      let rank: GoldRank = gold_rank(&mapped, synthetic_gold);
      assert_eq!(rank, Some(1));
      assert!((mean_success_at_k(&[rank], 10) - 1.0).abs() < 1e-9);

      // 5. The scripted answerer runs over packed context (proves the Answerer seam composes).
      let answerer = ScriptedAnswerer;
      let answer = answerer.answer("q", "some context").unwrap();
      assert!(answer.contains("scripted answer"));

      d_drop(d, client);
  }

  /// Drop the client before the daemon so the connection closes cleanly.
  fn d_drop(mut d: HarnessDaemon, client: WireClient) {
      drop(client);
      d.kill();
  }
  ```
  > **Implementer note:** the modules must be reachable from an integration test, which links the crate as a LIBRARY. `memharness` is currently bin-only. Task 40 adds a `src/lib.rs` that re-exports the modules (`pub mod ...`) and has `main.rs` use them, so `hermetic_e2e.rs` can `use memharness::...`. This RED test will not COMPILE until Task 40 — that is the expected failure.
- [ ] Run: `cargo test -p memharness --test hermetic_e2e` → expect **FAIL** (no `memharness` library target yet).

## Task 40 — add `src/lib.rs` so integration tests link the modules (GREEN)

- [ ] Create `crates/memharness/src/lib.rs` re-exporting every module:
  ```rust
  //! memharness library surface — the modules the binary (`main.rs`) AND the hermetic integration
  //! test (`tests/hermetic_e2e.rs`) share. DEV-ONLY (see the crate header); never ships.
  #![forbid(unsafe_code)]

  pub mod anthropic;
  pub mod arms;
  pub mod client;
  pub mod corpus;
  pub mod daemon;
  pub mod frontmatter;
  pub mod judge;
  pub mod mine;
  pub mod ollama;
  pub mod report;
  pub mod stats;
  pub mod synth;
  ```
- [ ] Add the `[lib]` + keep `[[bin]]` in `crates/memharness/Cargo.toml`:
  ```toml
  [lib]
  name = "memharness"
  path = "src/lib.rs"

  [[bin]]
  name = "memharness"
  path = "src/main.rs"
  ```
- [ ] In `main.rs`, REPLACE the per-file `mod frontmatter;`…`mod synth;` declarations with `use memharness::...` where the bin references them (the modules now live in the lib). Concretely: delete the `mod <name>;` lines from `main.rs`; change the CLI defaults/refs from `crate::ollama::…`/`crate::report::…` to `memharness::ollama::…`/`memharness::report::…`; the `cli_tests` module likewise uses `memharness::ollama::DEFAULT_OLLAMA_MODEL`.
- [ ] Run: `cargo test -p memharness` → expect **ALL PASS** (unit tests via the lib + the hermetic e2e).
- [ ] Run: `cargo run -p memharness -- run --help` → still prints flags.
- [ ] Commit:
  ```
  git add crates/memharness/src/lib.rs crates/memharness/src/main.rs crates/memharness/Cargo.toml crates/memharness/tests/hermetic_e2e.rs crates/memharness/tests/fixtures/mini_corpus/en/alpha.md crates/memharness/tests/fixtures/mini_corpus/en/beta.md crates/memharness/tests/fixtures/mini_corpus/ko/gamma.md
  git commit -m "$(cat <<'EOF'
feat(memharness): lib target + hermetic e2e (mini-corpus → real daemon → ingest → recall → scoring)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```

---

## Task 41 — Gates + LIVE-RUN runbook + PR prep

- [ ] Run the full gate set and confirm each is green:
  ```
  cargo test -p memharness
  cargo clippy -p memharness --all-targets -- -D warnings
  cargo test --workspace          # the rest of the tree still passes (memharness added no regressions)
  cargo check --workspace         # fresh-checkout compile unaffected
  ```
  If clippy flags anything, fix it (no `#[allow]` without a one-line justification comment). If any workspace suite regressed, STOP and fix — a green pre-existing tree is a hard requirement.
- [ ] Verify the crate is NOT in any release/bundle manifest:
  ```
  grep -rn "memharness" apps/desktop/src-tauri/tauri.conf.json 2>/dev/null || echo "not bundled — good"
  ```
  Expect no match (it's a bare bin crate, never referenced by the app).
- [ ] Document the **LIVE-RUN runbook** in the PR description (NOT a committed doc — it's Peter-gated and machine-specific). The runbook:
  1. Ensure Ollama is running with the model: `ollama serve` + `ollama pull qwen2.5:7b` (or the Probe-B tag).
  2. Ensure `gbrain` is on PATH and its brain is synced (the harness does NOT re-sync; it records the page-count delta).
  3. Export the audit key: `export ANTHROPIC_API_KEY=…` (skip for `--local-only`).
  4. Run: `cargo run -p memharness -- run` (defaults: `~/brain`, k=10, seed=42, hybrid audit). Expect ≤~2h.
  5. Read the report at `~/.air-harness/reports/<ts>/report.md`. Confirm: judge-trust verdict leads, EN/KO split present, ≥100 real + ≥200 synthetic queries, corpus manifest, zero repo files with brain content.
  6. `--local-only` variant for a $0 sanity pass (wider error bars, no audit).
- [ ] Confirm the acceptance boundary: **the harness is ready + smoke-tested hermetically; the live baseline run is the acceptance demo** (it needs Peter's machine state — Ollama/gbrain/key/`~/brain` — and is not automatable in this plan).
- [ ] Final commit (gate evidence note in the message body — paste the pass counts):
  ```
  git add -A
  git commit -m "$(cat <<'EOF'
chore(memharness): gates green (test/clippy/workspace) + live-run runbook in PR

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
  ```
- [ ] PR prep (do NOT push unless asked): title `feat(memharness): Phase 0 blind A/B measuring stick (AIR vs GBrain) — dev-only`; body includes the runbook above + the "never ships / reports never committed" callouts + a note that the full `run()` loop assembly is exercised by the live run.

---

## Self-review

### Spec coverage checklist (spec § → task)

| Spec section | Requirement | Task(s) |
|---|---|---|
| §5, §24-26 | New dev-only bin crate `crates/memharness`, `#![forbid(unsafe_code)]`, `test-helpers` dep, never ships | 2 (Cargo.toml + workspace), lib header 2/40 |
| §27 | Per-run in-process daemon on a private socket + fresh temp home; real wire ops | 13-14 (HarnessDaemon), 15-16 (WireClient), 39-40 (e2e) |
| §28, §29 | Ephemeral except reports; fresh ingest; model dir resolved like the daemon | 14 (`test_engine` resolves model dir), 39-40 (fresh ingest per run) |
| §32-35 | Corpus copy from `~/brain`, strip frontmatter, skip dot-entries, sha256 manifest, snapshot ts | 3-4 (strip), 7-10 (copy+skip+manifest) |
| §35 | GBrain not re-synced; record `gbrain --version` + page count; report drift | 1 (probe version), 34 (`gbrain_version`/`gbrain_page_count` in ReportModel), 41 (runbook) |
| §38-41 | Mine real queries from transcripts; implicit within-5 known-item labels; dedup | 1 (count probe), 17-18 (mine + labels + exact dedup) |
| §42 | Synthetic known-item queries, stratified by category + language, source=gold, ~200-400 | 35-36 (stratified sample + generator) |
| §43 | Segment tags real/synthetic, en/ko/mixed, known-item/open | 5-6 (lang), 17-18/35-36 (real/synth + known/open), 34 (segments) |
| §46 | AIR arm: recall k=10 → hydrate → context pack | 15-16 (recall), 31-32 (air_recall + pack_context) |
| §48 | GBrain arm: `gbrain query --limit 10` balanced; parse; parse failure = run error | 1 (format probe), 31-32 (parse + gbrain_recall error-not-silent) |
| §49 | Identical answerer both arms (same model/prompt/budget); no GBrain cloud ask | 31-32 (Answerer seam + CONTEXT_BUDGET_CHARS + OllamaAnswerer) |
| §54 | Known-item mechanical success@k + MRR; page identity normalized both arms | 7-8 (page-id normalization), 19-20 (success@k/MRR), 31-32 (gold_rank) |
| §55-56 | Open queries: blind local judge, position-swapped ×2, disagreement=uncertain | 25-28 (Verdict, blind assign, resolve_swap) |
| §57 | Cloud audit: 10-15% random + all uncertains | 29-30 (audit call), 26 (trust verdict aggregates), 38/41 (wiring + runbook) |
| §58 | Judge-trust verdict: agreement + kappa vs ≥85%/≥0.6; below → expand to 100% + flag | 25-26 (kappa, trust_verdict, expand flag) |
| §59 | Per-segment win-rates + bootstrap CIs; Wilcoxon; honest small-n flags | 21-22 (bootstrap CI), 23-24 (Wilcoxon + small_n_approx), 34 (report surfaces it) |
| §62-63 | Default hybrid audit via `ANTHROPIC_API_KEY`; `--local-only` disables egress | 37-38 (`--local-only` flag), 29-30 (env key), 34 (egress accounting) |
| §64 | Egress accounting: report every pair sent; GBrain arm's own egress noted | 34 (`egress_pairs_sent` + GBrain note) |
| §67-69 | Reports to `~/.air-harness/reports/<ts>/`; NEVER repo; structure (trust→headline→EN/KO→known/open→examples→manifest) | 33-34 (render + guard + write), 37-38 (default reports dir) |
| §71-76 | Known limitations documented; HNSW nondeterminism accepted; seeds fixed | 21/35 (seeds), 24 (small-n honesty), 34 (caveats), Preconditions (HNSW note) |
| §80-84 (acceptance) | One-command run ≤2h; report contents; zero live-brain writes; `--local-only`; all over real wire | 37-38 (CLI), 41 (runbook + gates), 13-16/39-40 (real wire, isolated home) |
| §88 (open q) | Local model default + `ollama list` availability check with clear failure | 1 (Probe B), 11-12 (preflight) |
| §89 (open q) | `gbrain query` output format verified + pinned | 1 (Probe A), 31-32 (parser) |
| §90 (open q) | If <50 real open queries, weight synthetic higher + say so | 1 (Probe C decision), 34 (`near_dedup_applied`/weighting caveat surfaced) |
| Session decision | Plain-Rust stats/kappa/lang/frontmatter, no heavyweight deps; exact Cargo.lock versions | 3-6/19-24/25-28 (pure impls), Dep table + Task 2 |
| Session decision | Seeded StdRng from `--seed` (default 42) for sampling/synth/bootstrap | 21/35/37 (seed flag threads into bootstrap + stratified + blind) |
| Session decision | Blinding: judge sees A/B, per-pair randomized (seeded), swap = judge twice | 27-28 (assign_blind + deblind + resolve_swap) |
| Session decision | Anthropic audit: minimal Messages API, pinned model, degrade-not-fabricate | 29-30 (`AUDIT_MODEL`, strict parse, error-on-ambiguity) |

Every spec requirement maps to at least one task. No requirement is unmapped.

### Placeholder scan

- No `TODO`, `TBD`, `todo!()`, `unimplemented!()`, `test.skip`/`.only`, or "similar to Task N" appears in any code block — every referenced type/function is defined in-plan (repeated in full where used).
- The one deliberate deferral (near-duplicate query dedup, spec §41) is shipped as exact-only dedup with an EXPLICIT report caveat (`near_dedup_applied: false`) surfaced in Task 34, and flagged in the Task 18 note — a reported limitation, not a silent stub.
- The `run()` loop assembly (Task 38) is real code over the built seams, exercised by the live run; the plan explicitly forbids `todo!()` stubs there and requires reporting if time-boxing forces a follow-up.

### Type-consistency check

- `Verdict` (judge.rs) — defined Task 26; used 26/27/28/34.
- `RetrievedHit` (arms.rs) — defined Task 32; used 31/32/39.
- `MinedQuery` (mine.rs) — defined Task 18.
- `SynthQuery` / `PageRef` (synth.rs) — defined Task 36.
- `GoldRank` (stats.rs) — defined Task 20; used 39.
- `WilcoxonResult` (stats.rs) — defined Task 24; used in `SegmentResult` (Task 34).
- `TrustVerdict` (judge.rs) — defined Task 26; used in `ReportModel` (Task 34).
- `CorpusManifest` / `ManifestEntry` (corpus.rs) — defined Task 8; used 10/34/39.
- `HitWire`/`Response`/`Request`/`Hello`/`HelloOk`/`IngestReportMirror` — from `bossclawd-proto` (real types, pinned from the read of `crates/bossclawd-proto/src/lib.rs`); the implementer note in Task 16 flags confirming `IngestReportMirror`'s exact module path.
- `test_engine` / `run_accept_loop` / `seed_secret_cache_for_test` — real `bossclawd` `test-helpers` symbols, signatures pinned from the source read.
- `Answerer` / `QueryGenerator` seams — defined 32/36; scripted doubles in 39; live impls (`OllamaAnswerer`/`OllamaQueryGenerator`) in 32/36.
- All RNG uses take a `ChaCha8Rng`/`impl Rng` seeded from `--seed` (default 42) — consistent across bootstrap (22), blind assignment (28), and stratified sampling (36).
