# M5b Sandboxed `markitdown` Parser — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ingest rich documents (PDF/docx/pptx/xlsx/xls/msg) by running `markitdown` in a hard-jailed subprocess (deny-network fail-closed, resource-capped, secret-scrubbed, fd→stdin), plugging into M5a's pipeline as `origin:"external"` taint-root events.

**Architecture:** One new `Parser` impl (`SandboxedMarkitdownParser`, feature `markitdown`) + a `ParserRouter` selector that dispatches rich extensions to it and everything else to the existing `NativeTextParser`. The child is a bundled pinned venv running a first-party `convert_stdin.py` wrapper (`convert_stream`, stripped converter registry). Jail = OS tool (`sandbox-exec`/`bwrap`) proven by an active egress probe; resource bounds enforced authoritatively by the Rust pump. `#![forbid(unsafe_code)]` preserved (no `pre_exec`).

**Tech Stack:** Rust (rustix `process` feature for group-kill, `std::process::Command`), Python 3.12/3.13 venv (markitdown 0.1.x), macOS Seatbelt (`sandbox-exec`), Linux bubblewrap (`bwrap`).

**Spec:** `docs/superpowers/specs/2026-06-19-bossclaw-core-m5b-sandboxed-parser-design.md` (Rev 2). Read it first — this plan implements it.

---

## File Structure

| File | Action | Responsibility |
|---|---|---|
| `crates/bossclaw-core/Cargo.toml` | modify | `markitdown` feature → enables `rustix/process`; `tempfile` to deps (was dev-only) |
| `crates/bossclaw-core/src/ingest.rs` | modify | `is_rich_ext`, `max_rich_file_bytes`, parser-aware caps, `IngestError` variants + routing, empty→skipped, `ParserRouter`, `NullRichParser` |
| `crates/bossclaw-core/src/sandbox.rs` | create | `SandboxedMarkitdownParser`, venv discovery, per-OS jail builder, egress probe, concurrent pump, group-kill (all `#[cfg(all(unix, feature = "markitdown"))]`) |
| `crates/bossclaw-core/src/lib.rs` | modify | gated `mod sandbox;` + re-export |
| `crates/bossclaw-core/python/convert_stdin.py` | create | first-party wrapper: registry strip, `convert_stream`, UTF-8, rlimit backstop |
| `crates/bossclaw-core/python/requirements-{macos-arm64,linux-x86_64}.lock` | create | per-platform `--require-hashes` lockfiles |
| `crates/bossclaw-core/scripts/build-venv.sh` | create | reproducible venv build + `pip-audit` |
| `crates/bossclaw-core/tests/sandbox.rs` | create | gated real-subprocess jail proofs |
| `.github/workflows/*.yml` | modify | `pip-audit` job + gated jail-test job |

**Naming locked (use verbatim across tasks):** `is_rich_ext`, `RICH_EXTS`, `MAX_RICH_FILE_BYTES`, `max_rich_file_bytes`, `IngestError::SandboxUnavailable(String)`, `IngestError::Timeout`, `ParserRouter`, `ParserRouter::{new,native_only,uniform,pick}`, `NullRichParser`, `SandboxedMarkitdownParser`, `Jail`, `probe_egress`, `build_jailed_command`, `run_pump`, `kill_group`, `discover_venv`.

---

## Task 1: Cargo feature + rich-extension classifier + parser-aware byte budget

This is spec **F1** — the most important fix: M5a's 10 MiB cap (`ingest.rs:27`, gate at `:490`, read at `:611`) silently drops routine rich docs before any parser runs.

**Files:**
- Modify: `crates/bossclaw-core/Cargo.toml`
- Modify: `crates/bossclaw-core/src/ingest.rs` (consts ~27, `WalkLimits` ~354, walk gate ~490, read ~611)

- [ ] **Step 1: Add the feature + move `tempfile` to deps.** In `Cargo.toml`, under `[features]` add (note: `markitdown` enables the rustix `process` feature for group-kill, per spec §5.4):

```toml
# M5b: sandboxed markitdown subprocess parser. Enables rustix `process`
# (kill_process_group for the timeout group-kill). Default build is feature-off:
# rich files skip-with-report, native text/markdown ingest unaffected.
markitdown = ["rustix/process", "dep:tempfile"]
```

Move `tempfile` from `[dev-dependencies]` to `[dependencies]` as optional:

```toml
[dependencies.tempfile]
version = "3"
optional = true
```

(Keep it available to tests by adding `markitdown` to the test feature set in Task 11's CI, or keep a `tempfile` dev-dep line too — simplest: leave the existing `[dev-dependencies] tempfile = "3"` line in place AND add the optional dep above; cargo dedups.)

- [ ] **Step 2: Write the failing test** for the classifier + rich budget. Add to the `#[cfg(test)] mod tests` in `ingest.rs`:

```rust
#[test]
fn is_rich_ext_matches_only_the_sandboxed_set() {
    for e in ["pdf", "docx", "pptx", "xlsx", "xls", "msg"] {
        assert!(is_rich_ext(Some(e)), "{e} should be rich");
    }
    for e in ["txt", "md", "csv", "json", "html", "rs"] {
        assert!(!is_rich_ext(Some(e)), "{e} should be native");
    }
    assert!(!is_rich_ext(None));
}

#[test]
fn walk_applies_rich_budget_to_rich_ext_and_native_budget_to_others() {
    let dir = tempfile::tempdir().unwrap();
    // 500-byte files: a .pdf (rich) and a .txt (native).
    std::fs::write(dir.path().join("big.pdf"), vec![b'a'; 500]).unwrap();
    std::fs::write(dir.path().join("big.txt"), vec![b'a'; 500]).unwrap();
    let limits = WalkLimits { max_file_bytes: 100, max_rich_file_bytes: 10_000, ..Default::default() };
    let mut seen = std::collections::HashSet::new();
    let mut report = IngestReport::default();
    let mut walked = Vec::new();
    walk_grant(dir.path(), &limits, Instant::now(), &mut seen, &mut report, |wf| { walked.push(wf.canonical_path); Ok(()) }).unwrap();
    // .pdf (500 < rich cap 10_000) is surfaced; .txt (500 > native cap 100) is skipped oversize.
    assert!(walked.iter().any(|p| p.ends_with("big.pdf")), "rich file should pass its larger budget");
    assert!(!walked.iter().any(|p| p.ends_with("big.txt")), "native file over native cap should be skipped");
    assert!(report.skipped.iter().any(|(p, r)| p.ends_with("big.txt") && r == "oversize"));
}
```

- [ ] **Step 3: Run it to verify it fails.** `cargo test -p bossclaw-core is_rich_ext_matches walk_applies_rich_budget 2>&1 | tail -20` → FAIL (`is_rich_ext` / `max_rich_file_bytes` not found).

- [ ] **Step 4: Implement.** In `ingest.rs`:

(a) Add the const + classifier near the other consts (~line 39):

```rust
/// Larger byte cap for rich formats routed to the sandboxed parser. M5a's
/// `MAX_FILE_BYTES` (10 MiB) is sized for text/notes; PDFs/Office docs are
/// routinely larger, so the walk applies this cap to rich extensions instead
/// (spec F1). The sandboxed parser's `RLIMIT_AS`/output-cap derive from this.
const MAX_RICH_FILE_BYTES: usize = 100 * 1024 * 1024;

/// Extensions routed to the sandboxed `markitdown` parser (lowercase, no dot).
/// Single source of truth for BOTH the parser-aware byte budget (the walk) and
/// the dispatch (`ParserRouter::pick`).
const RICH_EXTS: &[&str] = &["pdf", "docx", "pptx", "xlsx", "xls", "msg"];

/// True if `ext` (lowercased, no dot) is a rich format handled by the sandboxed
/// parser. `None` (no extension) is native.
pub(crate) fn is_rich_ext(ext: Option<&str>) -> bool {
    ext.is_some_and(|e| RICH_EXTS.contains(&e))
}
```

(b) Add the field to `WalkLimits` (~line 354) and `Default` (~line 362):

```rust
pub(crate) struct WalkLimits {
    pub(crate) max_file_bytes: usize,
    pub(crate) max_rich_file_bytes: usize,   // NEW (spec F1)
    pub(crate) wall_clock: Duration,
    pub(crate) max_walk_depth: usize,
    pub(crate) max_dir_entries: usize,
}
// in Default::default():
        Self {
            max_file_bytes: MAX_FILE_BYTES,
            max_rich_file_bytes: MAX_RICH_FILE_BYTES,   // NEW
            wall_clock: INGEST_WALL_CLOCK,
            max_walk_depth: MAX_WALK_DEPTH,
            max_dir_entries: MAX_DIR_ENTRIES,
        }
```

(c) In `walk_grant`, make the oversize gate parser-aware. Replace the block at `ingest.rs:490`:

```rust
            let ext = std::path::Path::new(&name).extension().map(|e| e.to_string_lossy().to_lowercase());
            let cap = if is_rich_ext(ext.as_deref()) { limits.max_rich_file_bytes } else { limits.max_file_bytes };
            if cf.size() > cap as u64 {
                report.skipped.push((grant_root.join(&rel_child), "oversize".into()));
                continue;
            }
```

(Reuse `ext` for the `PathHint` a few lines down instead of recomputing: `let hint = PathHint { ext };`.)

(d) In `ingest_grant_inner`, make the read cap parser-aware. Replace the read at `ingest.rs:611`:

```rust
            let read_cap = if is_rich_ext(wf.hint.ext.as_deref()) { limits.max_rich_file_bytes } else { limits.max_file_bytes };
            let raw = match wf.file.read_all_capped(read_cap) {
```

- [ ] **Step 5: Run tests to verify pass.** `cargo test -p bossclaw-core is_rich_ext_matches walk_applies_rich_budget 2>&1 | tail -20` → PASS. Then `cargo test -p bossclaw-core 2>&1 | grep "test result"` → all green (no regressions; existing `WalkLimits{..Default::default()}` test constructors still compile because the new field has a Default).

- [ ] **Step 6: Commit.**

```bash
git add crates/bossclaw-core/Cargo.toml crates/bossclaw-core/src/ingest.rs
git commit -m "feat(bossclaw-core): M5b T1 — markitdown feature + parser-aware byte budget (F1)"
```

---

## Task 2: `IngestError` variants + error routing (`SandboxUnavailable`→skipped, `Timeout`→failed)

Spec **F8** — these MUST be own variants; mapping onto `Parse` would wrongly route a missing venv to `failed`.

**Files:** Modify `crates/bossclaw-core/src/ingest.rs` (enum ~96, Display ~111, match ~616–620)

- [ ] **Step 1: Write the failing test.** A test-only parser that returns a chosen error, then assert routing. Add to `mod tests`:

```rust
struct ErrParser(fn() -> IngestError);
impl Parser for ErrParser {
    fn convert(&self, _raw: &[u8], _hint: &PathHint) -> Result<String, IngestError> { Err((self.0)()) }
    fn parser_id(&self) -> &str { "err" }
}

#[test]
fn sandbox_unavailable_routes_to_skipped_timeout_to_failed() {
    // Routing is exercised by ingest_grant_inner; assert the Display + the
    // classification helper the match uses. (Full pipeline routing is covered
    // by the integration test in Task 9.)
    assert_eq!(IngestError::SandboxUnavailable("x".into()).to_string(), "sandbox unavailable: x");
    assert_eq!(IngestError::Timeout.to_string(), "parser timed out");
    assert!(IngestError::SandboxUnavailable("x".into()).is_skip());
    assert!(!IngestError::Timeout.is_skip());
    assert!(!IngestError::Parse("y".into()).is_skip());
    assert!(IngestError::NonUtf8.is_skip());
}
```

- [ ] **Step 2: Run to verify it fails.** `cargo test -p bossclaw-core sandbox_unavailable_routes 2>&1 | tail -20` → FAIL (`SandboxUnavailable`/`Timeout`/`is_skip` not found).

- [ ] **Step 3: Implement.** In `ingest.rs`:

(a) Add variants to `IngestError` (~line 96):

```rust
    /// The sandbox could not be established (feature off, venv missing/invalid,
    /// or the egress probe did not prove network denial). Skipped, not failed —
    /// native ingest continues (spec F8, fail-closed).
    SandboxUnavailable(String),
    /// The jailed parser exceeded the wall-clock budget and was killed. Failed.
    Timeout,
```

(b) Add Display arms (~line 113):

```rust
            IngestError::SandboxUnavailable(m) => write!(f, "sandbox unavailable: {m}"),
            IngestError::Timeout => write!(f, "parser timed out"),
```

(c) Add a routing helper (replaces the ad-hoc `NonUtf8`-only match arm with a single classifier — DRY):

```rust
impl IngestError {
    /// True if this error should be recorded as a `skipped` (benign: file not
    /// ingestable here) rather than a `failed` (a safety/IO problem). Single
    /// source of truth for the `ingest_grant_inner` routing.
    pub(crate) fn is_skip(&self) -> bool {
        matches!(self, IngestError::NonUtf8 | IngestError::TooLarge | IngestError::SandboxUnavailable(_))
    }
}
```

(d) Replace the parser-call match in `ingest_grant_inner` (`ingest.rs:616–620`) to use it:

```rust
            let text = match parser.convert(&raw, &wf.hint) {
                Ok(t) => t,
                Err(e) if e.is_skip() => { report.skipped.push((wf.canonical_path, e.to_string())); continue; }
                Err(e) => { report.failed.push((wf.canonical_path, e.to_string())); continue; }
            };
```

- [ ] **Step 4: Run to verify pass.** `cargo test -p bossclaw-core sandbox_unavailable_routes 2>&1 | tail -20` → PASS. `cargo test -p bossclaw-core 2>&1 | grep "test result"` → green.

- [ ] **Step 5: Commit.**

```bash
git add crates/bossclaw-core/src/ingest.rs
git commit -m "feat(bossclaw-core): M5b T2 — IngestError Sandbox/Timeout variants + skip routing (F8)"
```

---

## Task 3: `ParserRouter` + `NullRichParser` + dispatch + empty→skipped

The selector that routes rich exts to the sandboxed parser and others to native, keeping provenance correct (`parser_id` from the *chosen* parser). Also spec **F12** (empty extraction → skipped).

**Files:** Modify `crates/bossclaw-core/src/ingest.rs` (new types near `NativeTextParser` ~143; `ingest_all` ~518; `ingest_grant_inner` ~582 the parse block)

- [ ] **Step 1: Write the failing test.**

```rust
#[test]
fn router_dispatches_by_extension_and_reports_chosen_parser_id() {
    let router = ParserRouter::new(
        Box::new(MockParser { output: "NATIVE".into() }),
        Box::new(MockParser { output: "RICH".into() }),
    );
    let pdf = PathHint { ext: Some("pdf".into()) };
    let txt = PathHint { ext: Some("txt".into()) };
    assert_eq!(router.pick(&pdf).convert(b"", &pdf).unwrap(), "RICH");
    assert_eq!(router.pick(&txt).convert(b"", &txt).unwrap(), "NATIVE");
}

#[test]
fn native_only_router_skips_rich_with_sandbox_unavailable() {
    let router = ParserRouter::native_only();
    let pdf = PathHint { ext: Some("pdf".into()) };
    let err = router.pick(&pdf).convert(b"%PDF-1.7", &pdf).unwrap_err();
    assert!(matches!(err, IngestError::SandboxUnavailable(_)));
    assert!(err.is_skip());
}
```

- [ ] **Step 2: Run to verify it fails.** `cargo test -p bossclaw-core router_dispatches native_only_router 2>&1 | tail -20` → FAIL.

- [ ] **Step 3: Implement.** Add after `NativeTextParser` (~line 143):

```rust
/// The feature-off / no-jail stand-in for the rich parser: every rich file is
/// reported `SandboxUnavailable` (→ skipped), so the engine degrades to exactly
/// M5a behavior when `markitdown` is disabled or no venv is present.
pub struct NullRichParser;
impl Parser for NullRichParser {
    fn convert(&self, _raw: &[u8], _hint: &PathHint) -> Result<String, IngestError> {
        Err(IngestError::SandboxUnavailable("markitdown parser not available".into()))
    }
    fn parser_id(&self) -> &str { "null-rich" }
}

/// Selects the parser for a file by its extension hint (spec §7). Holds the
/// native + rich parsers; the orchestrator calls `pick` then uses the SAME
/// returned parser for both `convert` and `parser_id` (correct provenance).
pub struct ParserRouter {
    native: Box<dyn Parser>,
    rich: Box<dyn Parser>,
}
impl ParserRouter {
    /// Production / general constructor.
    pub fn new(native: Box<dyn Parser>, rich: Box<dyn Parser>) -> Self { Self { native, rich } }
    /// M5a-equivalent default: native text + a null rich parser (rich → skip).
    pub fn native_only() -> Self { Self { native: Box::new(NativeTextParser), rich: Box::new(NullRichParser) } }
    /// Test helper: route everything to one parser.
    #[cfg(test)]
    pub fn uniform(p: Box<dyn Parser>) -> Self {
        // Cheap clone-by-reconstruct isn't possible for trait objects; tests that
        // need distinct behavior use `new`. `uniform` wraps a single shared impl.
        Self { native: p, rich: Box::new(NullRichParser) }
    }
    /// The chosen parser for `hint`.
    pub fn pick(&self, hint: &PathHint) -> &dyn Parser {
        if is_rich_ext(hint.ext.as_deref()) { self.rich.as_ref() } else { self.native.as_ref() }
    }
}
```

> Note: `uniform` as written routes only native; tests needing a single mock for both arms should use `ParserRouter::new(Box::new(MockParser{..}), Box::new(MockParser{..}))`. Remove `uniform` if unused (YAGNI) once Task 9 lands.

(b) Change `ingest_all` (`ingest.rs:518`) and `ingest_grant_inner` (`ingest.rs:582`) to take `&ParserRouter` instead of `&dyn Parser`:

```rust
    pub fn ingest_all(&self, router: &ParserRouter, embedder: &dyn crate::embed::Embedder) -> Result<IngestReport, crate::error::BossclawError> {
        // ... unchanged, but pass `router` through:
        self.ingest_grant_inner(std::path::Path::new(&root), router, embedder, started, &mut seen, &mut report)?;
```
```rust
    pub(crate) fn ingest_grant_inner(&self, grant_root: &std::path::Path, router: &ParserRouter, embedder: &dyn crate::embed::Embedder, started: Instant, seen: &mut std::collections::HashSet<FileIdentity>, report: &mut IngestReport) -> Result<(), crate::error::BossclawError> {
```

(c) In `ingest_grant_inner`, select the parser, then convert + handle empty (spec F12). Replace the parse block (the `let text = match parser.convert...` from Task 2 and the `file_ingested_content(... parser.parser_id() ...)` at `:621`):

```rust
            let parser = router.pick(&wf.hint);
            let text = match parser.convert(&raw, &wf.hint) {
                Ok(t) => t,
                Err(e) if e.is_skip() => { report.skipped.push((wf.canonical_path, e.to_string())); continue; }
                Err(e) => { report.failed.push((wf.canonical_path, e.to_string())); continue; }
            };
            // F12: empty / whitespace-only extraction (e.g. a scanned image-only
            // PDF) is NOT a signed empty event — record a skip instead.
            if text.trim().is_empty() {
                report.skipped.push((wf.canonical_path, "no extractable text".into()));
                continue;
            }
            let content = file_ingested_content(&text, &canonical_path, &raw, &grant_root_str, parser.parser_id(), &modified_at);
```

(d) Migrate existing callers. Every `ingest_all(&some_parser, &embedder)` in M5a tests becomes `ingest_all(&ParserRouter::new(Box::new(some_parser), Box::new(NullRichParser)), &embedder)` — or `&ParserRouter::native_only()` where the test used `NativeTextParser`. Grep and fix:

```bash
grep -rn "ingest_all(" crates/bossclaw-core/src crates/bossclaw-core/tests
```

- [ ] **Step 4: Run to verify pass + no regressions.** `cargo test -p bossclaw-core 2>&1 | grep "test result"` → all green. Add an empty-skip assertion to an existing orchestrator test or a new one feeding a `MockParser{ output: "   ".into() }` and asserting a `"no extractable text"` skip.

- [ ] **Step 5: Commit.**

```bash
git add crates/bossclaw-core/src/ingest.rs crates/bossclaw-core/tests
git commit -m "feat(bossclaw-core): M5b T3 — ParserRouter dispatch + empty->skipped (F7/F12)"
```

---

## Task 4: The `convert_stdin.py` wrapper + venv build script + lockfiles

Spec §6. The first-party wrapper (registry strip = the real SSRF/attack-surface control, F2/F14) and the reproducible, audited venv.

**Files:**
- Create: `crates/bossclaw-core/python/convert_stdin.py`
- Create: `crates/bossclaw-core/scripts/build-venv.sh`
- Create: `crates/bossclaw-core/python/requirements-macos-arm64.lock`, `requirements-linux-x86_64.lock`

- [ ] **Step 1: Write the wrapper** `crates/bossclaw-core/python/convert_stdin.py`:

```python
#!/usr/bin/env python3
"""First-party markitdown entry point for the jailed child (M5b).
Reads bytes on stdin, writes Markdown on stdout. Receives an extension hint
(no path) as argv[1]. Strips markitdown's converter registry to a minimal,
audited, OFFLINE set so no network-active converter is reachable even if
magika content-sniffs the bytes as something else (spec F2/F14)."""
import sys, resource

# Secondary backstop only — authoritative limits are the Rust pump + OS jail.
for res, lim in ((resource.RLIMIT_AS, 1 << 30), (resource.RLIMIT_CPU, 20), (resource.RLIMIT_FSIZE, 64 << 20)):
    try:
        resource.setrlimit(res, (lim, lim))
    except (ValueError, OSError):
        pass

sys.stdout.reconfigure(encoding="utf-8")
from markitdown import MarkItDown, StreamInfo

# VERIFY against the pinned markitdown version: the registry attribute is
# `_converters`, a list of registration objects each exposing `.converter`
# whose class name identifies it. Keep ONLY the offline document converters.
ALLOW = {"PdfConverter", "DocxConverter", "PptxConverter", "XlsxConverter",
         "XlsConverter", "OutlookMsgConverter", "PlainTextConverter"}
md = MarkItDown(enable_plugins=False, enable_builtins=True)
md._converters = [c for c in md._converters if type(getattr(c, "converter", c)).__name__ in ALLOW]

ext = sys.argv[1] if len(sys.argv) > 1 else None
si = StreamInfo(extension=("." + ext)) if ext else None     # F9: dot-prefixed
out = md.convert_stream(sys.stdin.buffer, stream_info=si)
sys.stdout.write(out.text_content)
```

- [ ] **Step 2: Write the venv build script** `crates/bossclaw-core/scripts/build-venv.sh`:

```bash
#!/usr/bin/env bash
# Build the pinned, audited markitdown venv for M5b. Reproducible: installs from
# a per-platform --require-hashes lockfile, then pip-audit (fails on CVE).
set -euo pipefail
PYBIN="${PYBIN:-python3.12}"
DEST="${1:?usage: build-venv.sh <dest-venv-dir>}"
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) LOCK="$(dirname "$0")/../python/requirements-macos-arm64.lock" ;;
  Linux-x86_64) LOCK="$(dirname "$0")/../python/requirements-linux-x86_64.lock" ;;
  *) echo "unsupported platform: $(uname -s)-$(uname -m)" >&2; exit 1 ;;
esac
"$PYBIN" -m venv "$DEST"
"$DEST/bin/pip" install --upgrade pip
"$DEST/bin/pip" install --require-hashes -r "$LOCK"
"$DEST/bin/pip" install pip-audit
"$DEST/bin/pip-audit" --strict           # non-zero exit on any known CVE
"$DEST/bin/python" -c "import markitdown; print('markitdown', markitdown.__version__)"
cp "$(dirname "$0")/../python/convert_stdin.py" "$DEST/convert_stdin.py"
```

- [ ] **Step 3: Generate the lockfiles** (one per platform; do this on each platform or via CI). Document the procedure in a comment header; the lockfile is `pip install 'markitdown[pdf,docx,pptx,xlsx,xls,outlook]'` resolved with hashes:

```bash
# On macOS arm64 (and again on Linux x86_64), with python3.12:
python3.12 -m venv /tmp/mkresolve && /tmp/mkresolve/bin/pip install --upgrade pip pip-tools
echo "markitdown[pdf,docx,pptx,xlsx,xls,outlook]==<PINNED>" > /tmp/req.in
/tmp/mkresolve/bin/pip-compile --generate-hashes --output-file crates/bossclaw-core/python/requirements-macos-arm64.lock /tmp/req.in
# Verify magika's ONNX model is vendored in the wheel (no first-run fetch):
/tmp/mkresolve/bin/python -c "import magika, pathlib; print(list(pathlib.Path(magika.__path__[0]).rglob('*.onnx')))"
```

Pin `<PINNED>` to a specific markitdown release; record it in the spec's `parser_id`. Commit both `.lock` files. (If the ONNX model is NOT vendored, add an explicit offline-model step — do not ship a parser that fetches at first run.)

- [ ] **Step 4: Smoke the wrapper manually** (not a Rust test yet — a human/CI check):

```bash
chmod +x crates/bossclaw-core/scripts/build-venv.sh
crates/bossclaw-core/scripts/build-venv.sh /tmp/m5b-venv
printf 'hello **world**' | /tmp/m5b-venv/bin/python /tmp/m5b-venv/convert_stdin.py txt   # → markdown
# A real fixture:
/tmp/m5b-venv/bin/python /tmp/m5b-venv/convert_stdin.py pdf < some-fixture.pdf | head
```

Expected: Markdown on stdout, exit 0. If `md._converters` strip raises, fix the attribute access against the pinned version (Step 1 VERIFY note) before proceeding.

- [ ] **Step 5: Commit.**

```bash
git add crates/bossclaw-core/python crates/bossclaw-core/scripts
git commit -m "feat(bossclaw-core): M5b T4 — convert_stdin wrapper + pinned audited venv (F2/F9/F15)"
```

---

## Task 5: The concurrent I/O pump (no deadlock, incremental caps, wall-clock + group-kill)

Spec §5.5. The authoritative resource guarantee. Built + tested with a *mock* child (`/bin/sh` / a trivial python) — no jail, no markitdown yet, so it's fast and hermetic-ish (gated on unix).

**Files:** Create `crates/bossclaw-core/src/sandbox.rs`; modify `lib.rs`.

- [ ] **Step 1: Register the module.** In `lib.rs`:

```rust
#[cfg(all(unix, feature = "markitdown"))]
mod sandbox;
#[cfg(all(unix, feature = "markitdown"))]
pub use sandbox::SandboxedMarkitdownParser;
```

- [ ] **Step 2: Write the failing tests** in `sandbox.rs` (`#[cfg(test)]`), driving the pump with a mock command:

```rust
#[cfg(test)]
mod pump_tests {
    use super::*;
    use std::process::Command;
    use std::time::Duration;

    fn sh(script: &str) -> Command { let mut c = Command::new("/bin/sh"); c.arg("-c").arg(script); c }

    #[test]
    fn pump_streams_stdin_to_stdout() {
        let out = run_pump(sh("cat"), b"hello", 1 << 20, 64 << 10, Duration::from_secs(5)).unwrap();
        assert_eq!(out, "hello");
    }
    #[test]
    fn pump_kills_on_timeout() {
        let err = run_pump(sh("sleep 30"), b"", 1 << 20, 64 << 10, Duration::from_millis(300)).unwrap_err();
        assert!(matches!(err, crate::ingest::IngestError::Timeout));
    }
    #[test]
    fn pump_enforces_output_cap() {
        let err = run_pump(sh("yes aaaaaaaa"), b"", 4096, 64 << 10, Duration::from_secs(5)).unwrap_err();
        assert!(matches!(err, crate::ingest::IngestError::Parse(_))); // "output cap exceeded"
    }
    #[test]
    fn pump_does_not_deadlock_on_large_interleaved_io() {
        // Child echoes 1 MiB while we feed 1 MiB: must not deadlock (concurrent pump).
        let out = run_pump(sh("cat"), &vec![b'x'; 1 << 20], 4 << 20, 64 << 10, Duration::from_secs(10)).unwrap();
        assert_eq!(out.len(), 1 << 20);
    }
}
```

- [ ] **Step 3: Run to verify they fail.** `cargo test -p bossclaw-core --features markitdown pump_ 2>&1 | tail -20` → FAIL (`run_pump` not defined).

- [ ] **Step 4: Implement `run_pump` + `kill_group`** in `sandbox.rs`:

```rust
use std::io::{Read, Write};
use std::os::unix::process::CommandExt;          // process_group
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use crate::ingest::IngestError;

/// Spawn `cmd` in its own process group, stream `input` to stdin on a writer
/// thread (so we never deadlock against a full stdout pipe), read stdout under
/// `out_cap` incrementally, read stderr into a bounded buffer, enforce
/// `timeout` with a group-kill. Returns stdout as UTF-8 text.
pub(crate) fn run_pump(mut cmd: Command, input: &[u8], out_cap: usize, stderr_cap: usize, timeout: Duration)
    -> Result<String, IngestError>
{
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    cmd.process_group(0); // own group → group-kill reaps helpers (spec §5.1)
    let mut child = cmd.spawn().map_err(|e| IngestError::SandboxUnavailable(format!("spawn: {e}")))?;
    let pid = child.id() as i32;

    // Writer thread: own the stdin pipe, write all input, drop to close it.
    let mut stdin = child.stdin.take().expect("piped");
    let input_owned = input.to_vec();
    let writer = std::thread::spawn(move || { let _ = stdin.write_all(&input_owned); /* EPIPE if child died: ignore */ });

    // Reader threads with caps.
    let mut stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    let out_reader = std::thread::spawn(move || read_capped(&mut stdout, out_cap));
    let err_reader = std::thread::spawn(move || { let mut b = Vec::new(); let _ = (&mut stderr).take(stderr_cap as u64).read_to_end(&mut b); b });

    // Wall-clock wait with group-kill on expiry (authoritative timeout).
    let started = Instant::now();
    let status = loop {
        match child.try_wait().map_err(|e| IngestError::Io(e.to_string()))? {
            Some(s) => break s,
            None if started.elapsed() >= timeout => { kill_group(pid); let _ = child.wait(); return Err(IngestError::Timeout); }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    let _ = writer.join();
    let out = out_reader.join().map_err(|_| IngestError::Io("stdout reader panicked".into()))?;
    let _err = err_reader.join().unwrap_or_default();

    match out {
        Err(()) => { kill_group(pid); Err(IngestError::Parse("output cap exceeded".into())) }
        Ok(bytes) if !status.success() => {
            let tail = String::from_utf8_lossy(&_err); let tail = tail.trim();
            Err(IngestError::Parse(format!("markitdown exit {:?}: {}", status.code(), &tail[..tail.len().min(200)])))
        }
        Ok(bytes) => String::from_utf8(bytes).map_err(|_| IngestError::NonUtf8),
    }
}

/// Read from `r` into a Vec, returning Err(()) the moment it would exceed `cap`.
fn read_capped(r: &mut impl Read, cap: usize) -> Result<Vec<u8>, ()> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        match r.read(&mut chunk) {
            Ok(0) => return Ok(buf),
            Ok(n) => { if buf.len() + n > cap { return Err(()); } buf.extend_from_slice(&chunk[..n]); }
            Err(_) => return Ok(buf), // pipe closed (e.g. after kill)
        }
    }
}

/// Kill the whole process group led by `pid` (spec §5.4; rustix `process`
/// feature, no `unsafe`). VERIFY the exact fn name against rustix 0.38:
/// `rustix::process::kill_process_group(Pid, Signal)`.
fn kill_group(pid: i32) {
    if let Some(p) = rustix::process::Pid::from_raw(pid) {
        let _ = rustix::process::kill_process_group(p, rustix::process::Signal::Kill);
    }
}
```

> VERIFY step inside Step 4: run `cargo doc -p rustix --features process` or check `rustix::process` — confirm `kill_process_group`, `Pid::from_raw`, `Signal::Kill` spellings; adjust if the 0.38 API differs (e.g. `Signal::KILL`). This is the one external-API spot to pin.

- [ ] **Step 5: Run to verify pass.** `cargo test -p bossclaw-core --features markitdown pump_ 2>&1 | tail -20` → PASS (all four). Then `cargo clippy -p bossclaw-core --features markitdown --all-targets 2>&1 | tail -5` → clean. Confirm `#![forbid(unsafe_code)]` still holds (no `unsafe` added) — `grep -rn "unsafe" crates/bossclaw-core/src/sandbox.rs` → only the word in comments, if any.

- [ ] **Step 6: Commit.**

```bash
git add crates/bossclaw-core/src/sandbox.rs crates/bossclaw-core/src/lib.rs
git commit -m "feat(bossclaw-core): M5b T5 — concurrent I/O pump + group-kill (F4/F6/F11)"
```

---

## Task 6: Venv discovery + the per-OS jail builder (env scrub, scratch cwd, OS tool wrap)

Spec §5.1–5.4. Builds the jailed `Command` (no network probe yet — Task 7). The exact Seatbelt profile / bwrap flags are **driven by the egress test in Task 7**; this task wires the scaffolding + scrub + scratch + the OS-tool wrap.

**Files:** Modify `crates/bossclaw-core/src/sandbox.rs`; create `crates/bossclaw-core/src/sandbox_profiles/seatbelt.sb` (embedded via `include_str!`).

- [ ] **Step 1: Write the failing test** (env scrub + scratch cwd are assertable with a mock wrapper that dumps its environment):

```rust
#[test]
fn jailed_command_scrubs_env_and_sets_scratch_cwd() {
    // SAFETY of the test only: set a fake secret in our env, assert the child can't see it.
    std::env::set_var("FAKE_DEK", "supersecret");
    let scratch = tempfile::tempdir().unwrap();
    let cmd = build_jailed_command_for_test(scratch.path(), "/bin/sh", &["-c".into(), "echo \"${FAKE_DEK:-CLEAN}\"; pwd".into()]);
    let out = run_pump(cmd, b"", 1 << 20, 64 << 10, std::time::Duration::from_secs(5)).unwrap();
    assert!(out.contains("CLEAN"), "env must be scrubbed, got: {out}");
    assert!(out.contains(scratch.path().to_str().unwrap()), "cwd must be the scratch dir");
}
```

- [ ] **Step 2: Run to verify it fails.** `cargo test -p bossclaw-core --features markitdown jailed_command_scrubs 2>&1 | tail -20` → FAIL.

- [ ] **Step 3: Implement** the Command builder + venv discovery in `sandbox.rs`:

```rust
use std::path::{Path, PathBuf};

/// Located, validated venv (the bundled, pinned markitdown environment).
pub(crate) struct Venv { python: PathBuf, wrapper: PathBuf }

/// Find the venv via an explicit path: env override for tests/headless
/// (`BOSSCLAW_MARKITDOWN_VENV`), else the app-resources path (wired by the
/// desktop in M7). Returns SandboxUnavailable if missing/invalid → skip.
pub(crate) fn discover_venv() -> Result<Venv, IngestError> {
    let root = std::env::var_os("BOSSCLAW_MARKITDOWN_VENV")
        .map(PathBuf::from)
        .ok_or_else(|| IngestError::SandboxUnavailable("no venv path configured".into()))?;
    let python = root.join("bin").join("python");
    let wrapper = root.join("convert_stdin.py");
    if !python.exists() || !wrapper.exists() {
        return Err(IngestError::SandboxUnavailable(format!("venv incomplete at {}", root.display())));
    }
    Ok(Venv { python, wrapper })
}

/// Build the base (un-jailed) command core: env-scrubbed, scratch cwd, allowlist.
/// `program`+`args` is what actually runs (python wrapper, or `/bin/sh` in tests).
fn base_command(scratch: &Path, program: &str, args: &[String]) -> Command {
    let mut c = Command::new(program);
    c.args(args);
    c.env_clear();                                   // spec §5.1 (F5: no secret via env)
    c.env("PATH", "/usr/bin:/bin");
    c.env("LC_ALL", "C.UTF-8");
    c.env("HOME", scratch);
    c.env("PYTHONNOUSERSITE", "1");
    c.env("PYTHONDONTWRITEBYTECODE", "1");
    c.env("PYTHONHASHSEED", "0");
    c.current_dir(scratch);                          // scratch-only cwd
    c
}

#[cfg(test)]
pub(crate) fn build_jailed_command_for_test(scratch: &Path, program: &str, args: &[String]) -> Command {
    base_command(scratch, program, args)             // tests exercise scrub+cwd without the OS jail
}
```

- [ ] **Step 4: Run to verify pass.** `cargo test -p bossclaw-core --features markitdown jailed_command_scrubs 2>&1 | tail -20` → PASS.

- [ ] **Step 5: Add the OS-jail wrap** (no test yet — Task 7's egress probe is its test). Add the Seatbelt profile `crates/bossclaw-core/src/sandbox_profiles/seatbelt.sb`:

```scheme
(version 1)
(deny default)
(allow process-fork)
(allow signal (target self))
(allow sysctl-read)
(allow file-read* (subpath "/usr") (subpath "/System") (subpath "/Library") (literal "/dev/null") (literal "/dev/urandom"))
(allow file-read* file-write* (subpath (param "SCRATCH")))
(deny network*)
```

And the wrap function (network jail; Linux prefers `bwrap`):

```rust
/// Wrap `base` with the per-OS network+fs jail. macOS: sandbox-exec + the
/// pinned Seatbelt profile. Linux: bwrap (unshare net+pid+ipc, ro-bind /usr,
/// tmpfs scratch). The EFFICACY of this wrap is proven by `probe_egress` (Task 7).
#[cfg(target_os = "macos")]
fn wrap_jail(scratch: &Path, program: &str, args: &[String]) -> Command {
    let mut c = Command::new("/usr/bin/sandbox-exec");
    c.arg("-D").arg(format!("SCRATCH={}", scratch.display()));
    c.arg("-p").arg(include_str!("sandbox_profiles/seatbelt.sb"));
    c.arg(program).args(args);
    apply_scrub(&mut c, scratch);    // env_clear + allowlist + cwd, as in base_command
    c
}
#[cfg(target_os = "linux")]
fn wrap_jail(scratch: &Path, program: &str, args: &[String]) -> Command {
    let mut c = Command::new("bwrap");
    c.args(["--unshare-net","--unshare-pid","--unshare-ipc","--die-with-parent",
            "--ro-bind","/usr","/usr","--ro-bind","/bin","/bin","--ro-bind","/lib","/lib",
            "--proc","/proc","--dev","/dev"]);
    c.arg("--bind").arg(scratch).arg(scratch).arg("--chdir").arg(scratch);
    c.arg(program).args(args);
    apply_scrub(&mut c, scratch);
    c
}
```

(Refactor `base_command`'s env/cwd into `apply_scrub(&mut Command, &Path)` so both the test path and `wrap_jail` share it — DRY.)

- [ ] **Step 6: Commit.**

```bash
git add crates/bossclaw-core/src/sandbox.rs crates/bossclaw-core/src/sandbox_profiles
git commit -m "feat(bossclaw-core): M5b T6 — venv discovery + per-OS jail builder + env scrub (F5)"
```

---

## Task 7: The active egress probe (network denial proven, fail-closed)

Spec §5.2 / **F3** — the crown jewel. Prove the jail blocks a real `connect`; cache the result; fail-closed if it doesn't.

**Files:** Modify `crates/bossclaw-core/src/sandbox.rs`; create `crates/bossclaw-core/tests/sandbox.rs` (the gated jail proof).

- [ ] **Step 1: Write the failing test** (`tests/sandbox.rs`, gated `#[ignore]` — needs a built venv/jail; run in a dedicated CI job):

```rust
//! Real-subprocess jail proofs. Gated: require `--features markitdown` AND a
//! built venv at $BOSSCLAW_MARKITDOWN_VENV AND the OS jail tool present.
//! Run: BOSSCLAW_MARKITDOWN_VENV=/tmp/m5b-venv cargo test -p bossclaw-core --features markitdown --test sandbox -- --ignored
#![cfg(all(unix, feature = "markitdown"))]

#[test] #[ignore]
fn egress_probe_proves_network_denied() {
    // The probe spins up a loopback listener and asserts the jailed child's
    // connect is REFUSED at the jail layer (EPERM/ENETUNREACH), not just that
    // nobody answered (ECONNREFUSED). A passing probe => jail proven.
    assert!(bossclaw_core::sandbox_test_hooks::probe_egress_blocks(), "network must be denied by the jail");
}
```

- [ ] **Step 2: Run to verify it fails / is ignored.** `cargo test -p bossclaw-core --features markitdown --test sandbox -- --ignored 2>&1 | tail -20` → FAIL (hook not defined).

- [ ] **Step 3: Implement `probe_egress`** in `sandbox.rs`. The probe: bind a loopback `TcpListener` on an ephemeral port, run the jailed wrapper-equivalent that attempts to connect to it, and assert the connect FAILS with a jail error (no connection is accepted within a short window). A tiny probe script (shipped beside the wrapper, or a `python -c`) attempts the connect and prints the errno class:

```rust
use std::net::TcpListener;

/// True iff the jail genuinely denies network: a jailed child's connect to our
/// loopback listener is refused/blocked (no accept within the window). Cached
/// by the parser at construction AND fail-closed per-spawn (a jail-tool error
/// at spawn → SandboxUnavailable). Spec §5.2 / F3.
pub(crate) fn probe_egress(venv: &Venv) -> bool {
    let listener = match TcpListener::bind("127.0.0.1:0") { Ok(l) => l, Err(_) => return false };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    listener.set_nonblocking(true).ok();
    let scratch = match tempfile::tempdir() { Ok(d) => d, Err(_) => return false };
    // python one-liner: try to connect; exit 0 ONLY if connect SUCCEEDS.
    let probe = format!(
        "import socket,sys\ntry:\n s=socket.create_connection(('127.0.0.1',{port}),timeout=2); sys.exit(0)\nexcept OSError:\n sys.exit(7)");
    let cmd = wrap_jail(scratch.path(), venv.python.to_str().unwrap(), &["-c".into(), probe]);
    // If the child connects (exit 0) OR we accept a socket, the jail FAILED.
    let connected = run_pump(cmd, b"", 1 << 16, 64 << 10, std::time::Duration::from_secs(5))
        .err()
        .map(|e| matches!(e, IngestError::Parse(_)))   // non-zero exit (7) => connect refused => good
        .unwrap_or(false);
    let accepted = listener.accept().is_ok();          // any inbound accept => jail FAILED
    connected && !accepted
}

#[cfg(feature = "markitdown")]
pub mod sandbox_test_hooks {
    pub fn probe_egress_blocks() -> bool {
        let venv = match super::discover_venv() { Ok(v) => v, Err(_) => return false };
        super::probe_egress(&venv)
    }
}
```

> VERIFY: refine the exit-code/accept logic so a *passing* probe means "child could NOT connect AND we accepted nothing." Tune the success/fail mapping against `run_pump`'s return shape. The test is the spec here — iterate until `egress_probe_proves_network_denied` passes on macOS (Seatbelt) and Linux (bwrap), and **fails** if you remove the `wrap_jail` (sanity: confirm an un-jailed run DOES connect, proving the test has teeth).

- [ ] **Step 4: Iterate impl until the test passes** on macOS and Linux. Confirm teeth: temporarily swap `wrap_jail` for `base_command` → the test must FAIL (un-jailed connects). Restore.

- [ ] **Step 5: Commit.**

```bash
git add crates/bossclaw-core/src/sandbox.rs crates/bossclaw-core/tests/sandbox.rs
git commit -m "feat(bossclaw-core): M5b T7 — active egress probe, network denial proven (F3)"
```

---

## Task 8: `SandboxedMarkitdownParser` — assemble the `Parser` impl

Tie venv discovery + the cached egress probe + the jail builder + the pump into the `Parser::convert` seam. Spec §4/§6.1.

**Files:** Modify `crates/bossclaw-core/src/sandbox.rs`.

- [ ] **Step 1: Write the failing test** (gated, in `tests/sandbox.rs`): a real conversion + the skip/fail mappings.

```rust
#[test] #[ignore]
fn converts_real_pdf_and_reports_parser_id() {
    let p = bossclaw_core::SandboxedMarkitdownParser::discover().expect("venv");
    let bytes = std::fs::read("crates/bossclaw-core/tests/fixtures/hello.pdf").unwrap();
    let hint = bossclaw_core::ingest::PathHint { ext: Some("pdf".into()) };
    let md = p.convert(&bytes, &hint).expect("convert");
    assert!(md.to_lowercase().contains("hello"));
    assert!(p.parser_id().starts_with("markitdown-sandboxed-v"));
}
```

(Add a tiny committed `tests/fixtures/hello.pdf` containing the text "hello".)

- [ ] **Step 2: Run to verify it fails.** `... --test sandbox -- --ignored converts_real_pdf 2>&1 | tail` → FAIL.

- [ ] **Step 3: Implement** the parser in `sandbox.rs`:

```rust
use crate::ingest::{Parser, PathHint};

/// Pinned markitdown version — keep in lockstep with the venv lockfiles.
const MARKITDOWN_VERSION: &str = "0.1.x";   // set to the exact pinned release
const RICH_OUTPUT_CAP: usize = 32 * 1024 * 1024;
const RICH_STDERR_CAP: usize = 64 * 1024;
const RICH_WALL_CLOCK: std::time::Duration = std::time::Duration::from_secs(30);

/// The sandboxed rich-document parser (spec brick 1). Holds the located venv and
/// the cached egress-probe verdict; rebuilds the jail per call and fails closed.
pub struct SandboxedMarkitdownParser {
    venv: Venv,
    jail_proven: bool,
    id: String,
}

impl SandboxedMarkitdownParser {
    /// Locate the venv and prove the jail once. Returns SandboxUnavailable (→skip)
    /// if either fails — never constructs an unjailed parser.
    pub fn discover() -> Result<Self, IngestError> {
        let venv = discover_venv()?;
        let jail_proven = probe_egress(&venv);
        if !jail_proven {
            return Err(IngestError::SandboxUnavailable("network jail could not be proven".into()));
        }
        Ok(Self { venv, jail_proven, id: format!("markitdown-sandboxed-v{MARKITDOWN_VERSION}") })
    }
}

impl Parser for SandboxedMarkitdownParser {
    fn convert(&self, raw: &[u8], hint: &PathHint) -> Result<String, IngestError> {
        if !self.jail_proven {
            return Err(IngestError::SandboxUnavailable("jail not proven".into()));
        }
        let scratch = tempfile::tempdir().map_err(|e| IngestError::Io(e.to_string()))?;
        let ext = hint.ext.clone().unwrap_or_default();
        let cmd = wrap_jail(
            scratch.path(),
            self.venv.python.to_str().ok_or_else(|| IngestError::SandboxUnavailable("non-utf8 venv path".into()))?,
            &[self.venv.wrapper.to_string_lossy().into_owned(), ext],
        );
        run_pump(cmd, raw, RICH_OUTPUT_CAP, RICH_STDERR_CAP, RICH_WALL_CLOCK)
        // scratch tempdir drops here on every path (success/timeout/cap) → shredded.
    }
    fn parser_id(&self) -> &str { &self.id }
}
```

- [ ] **Step 4: Run to verify pass.** `... --test sandbox -- --ignored converts_real_pdf 2>&1 | tail` → PASS. `cargo clippy -p bossclaw-core --features markitdown --all-targets` → clean.

- [ ] **Step 5: Commit.**

```bash
git add crates/bossclaw-core/src/sandbox.rs crates/bossclaw-core/tests/fixtures/hello.pdf
git commit -m "feat(bossclaw-core): M5b T8 — SandboxedMarkitdownParser Parser impl"
```

---

## Task 9: End-to-end ingest integration (router wired to the real parser)

Prove the full path: a granted folder with a PDF ingests via the jail into a taint-root event; a scanned PDF → skipped; feature-off → skip.

**Files:** Modify `crates/bossclaw-core/tests/sandbox.rs` (gated e2e); confirm `ingest.rs` wiring.

- [ ] **Step 1: Write the failing gated e2e test:**

```rust
#[test] #[ignore]
fn ingest_grant_with_pdf_creates_external_taint_event() {
    // open a temp EventLog, grant a temp dir containing hello.pdf, ingest via a
    // router with the real sandboxed parser, assert ingested==1 and the event is is_external.
    // (Mirror the M5a fresh_then_dedup_then_supersede test setup; swap the router.)
    // ... see M5a ingest tests for EventLog/embedder/grant scaffolding ...
}

#[test] #[ignore]
fn scanned_image_pdf_is_skipped_no_extractable_text() { /* fixture: image-only PDF → report.skipped contains "no extractable text" */ }
```

- [ ] **Step 2–4: Run → fail → implement scaffolding → pass.** Construct `ParserRouter::new(Box::new(NativeTextParser), Box::new(SandboxedMarkitdownParser::discover()?))`, run `ingest_all`, assert the report + `is_external` on the appended event. Reuse the M5a test helpers (`EventLog::open`, `MockEmbedder`, `add_grant`).

- [ ] **Step 5: Hermetic feature-off regression.** Add a NON-gated hermetic test: with `ParserRouter::native_only()`, a `.pdf` in a grant → `report.skipped` contains a `SandboxUnavailable` reason (no jail needed — `NullRichParser`). Proves graceful degradation.

- [ ] **Step 6: Commit.**

```bash
git add crates/bossclaw-core/tests/sandbox.rs crates/bossclaw-core/tests/fixtures
git commit -m "test(bossclaw-core): M5b T9 — e2e ingest via jail + feature-off degradation"
```

---

## Task 10: The remaining security jail proofs

Spec §10/§11. The adversarial suite that makes "the jail holds" testable, not asserted. All gated `#[ignore]` in `tests/sandbox.rs`.

**Files:** Modify `crates/bossclaw-core/tests/sandbox.rs`.

- [ ] **Step 1: Write each proof as an `#[ignore]` test** (run → implement any missing hook → pass), one commit-worthy group:

```rust
#[test] #[ignore] fn no_secret_via_env_or_fd() {/* planted secret env var + open fd: child sees neither (enumerate /proc/self/fd | /dev/fd) */}
#[test] #[ignore] fn no_path_arg_reaches_child() {/* wrapper-echo argv: only the ext + wrapper path, never the source file path */}
#[test] #[ignore] fn double_fork_escapee_leaves_no_networked_survivor() {/* child setsid+sleeps; after Timeout kill, assert no surviving process holds a socket (Linux PID-ns) */}
#[test] #[ignore] fn rust_wallclock_fires_without_wrapper_rlimit() {/* wrapper that never setrlimits + sleeps → still Timeout-killed by the Rust pump */}
#[test] #[ignore] fn rlimit_as_backstop_kills_oversized_alloc() {/* wrapper allocs > RLIMIT_AS → clean failure, not host OOM */}
#[test] #[ignore] fn malformed_pdf_fails_without_hang_or_crash() {/* truncated PDF → report.failed, host fine */}
#[test] #[ignore] fn large_15mb_pdf_ingests() {/* proves F1 end-to-end: a 15 MB text-PDF ingests, not "oversize" */}
#[test] #[ignore] fn embedded_url_makes_no_outbound_connection() {/* HTML/RSS-magika bytes w/ a URL: loopback listener accepts nothing (F2/F14) */}
```

- [ ] **Step 2: Implement + iterate** until all pass on macOS and Linux. For `double_fork_escapee` and `embedded_url`, reuse the Task 7 listener-accept pattern. For `no_secret_via_env_or_fd`, the wrapper enumerates its fds and prints them; assert exactly stdio.

- [ ] **Step 3: Commit.**

```bash
git add crates/bossclaw-core/tests/sandbox.rs
git commit -m "test(bossclaw-core): M5b T10 — adversarial jail proofs (network/fd/escape/dos)"
```

---

## Task 11: CI — pip-audit job + gated jail-test job + macOS packaging check

Spec §9/§10. Wire the supply-chain gate and the gated jail suite into CI so they actually run.

**Files:** Modify the relevant `.github/workflows/*.yml`.

- [ ] **Step 1: Add a `markitdown-jail` CI job** (macOS + Ubuntu matrix) that: installs python3.12 + bwrap (Ubuntu), runs `scripts/build-venv.sh /tmp/m5b-venv` (which runs `pip-audit --strict`), then `BOSSCLAW_MARKITDOWN_VENV=/tmp/m5b-venv cargo test -p bossclaw-core --features markitdown --test sandbox -- --ignored`. Document the exact YAML (mirror the existing bossclaw-core CI job; add `apt-get install -y bubblewrap` on Ubuntu, `brew`/system `sandbox-exec` on macOS).

- [ ] **Step 2: Keep the default job pure.** Ensure the existing default `cargo test -p bossclaw-core` (no `markitdown`) still runs and stays green (feature-off path). Add `cargo clippy -p bossclaw-core --features markitdown --all-targets -- -D warnings` to the matrix.

- [ ] **Step 3: macOS packaging probe (O2).** Add a step (or a documented manual gate) that a **signed/notarized hardened-runtime** build can spawn `sandbox-exec` + the embedded interpreter — the one place F3/I5 bites in production. If it can't, that's a release blocker to resolve before M7 desktop wiring.

- [ ] **Step 4: Commit.**

```bash
git add .github/workflows
git commit -m "ci(bossclaw-core): M5b T11 — pip-audit + gated markitdown jail tests"
```

---

## Task 12: Docs, residuals, and the spec Status header

**Files:** Modify the spec's Status; add a short README note; update GBrain at session end (handoff).

- [ ] **Step 1: Flip the spec Status** to "IMPLEMENTED — pending dedicated security review of the built jail." Note the honest residuals shipped (spec §13): Linux-without-bwrap skips rich files; macOS `sandbox-exec` deprecation contingency; dedup-on-parser-upgrade won't re-land improved extraction; Windows deferred.
- [ ] **Step 2: Add a one-paragraph `crates/bossclaw-core/python/README.md`** explaining the venv build + lockfile regeneration + pip-audit cadence.
- [ ] **Step 3: Run the full gate** one last time:

```bash
cargo test -p bossclaw-core 2>&1 | grep "test result"                                   # default: green
cargo clippy -p bossclaw-core --all-targets -- -D warnings 2>&1 | tail -3               # default: clean
cargo clippy -p bossclaw-core --features markitdown --all-targets -- -D warnings 2>&1 | tail -3
BOSSCLAW_MARKITDOWN_VENV=/tmp/m5b-venv cargo test -p bossclaw-core --features markitdown --test sandbox -- --ignored 2>&1 | grep "test result"
grep -rn "unsafe" crates/bossclaw-core/src/sandbox.rs                                    # forbid(unsafe) intact
```

- [ ] **Step 4: Commit.**

```bash
git add docs/superpowers/specs/2026-06-19-bossclaw-core-m5b-sandboxed-parser-design.md crates/bossclaw-core/python/README.md
git commit -m "docs(bossclaw-core): M5b T12 — flip spec status, residuals, venv README"
```

---

## Mandated post-implementation gate

Per the spec (§10) and the Vault-Brain parent doc: **a dedicated security review of the BUILT jail** (not just this plan) before merge — re-run the §11 invariant checklist against the real code, ideally a fresh security-reviewer + a live adversarial pass (a crafted malicious PDF against the running jail). Only then open the PR.
