# M5b Sandboxed `markitdown` Parser — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** **Rev 2** — independent critic + security second opinion on the plan folded (both SHIP-WITH-FIXES, verified against real M5a code + markitdown 0.1.6 + rustix 0.38.44 + bundled SQLCipher). See §0 changelog.

**Goal:** Ingest rich documents (PDF/docx/pptx/xlsx/xls/msg) by running `markitdown` in a hard-jailed subprocess (deny-network fail-closed, resource-capped, secret-scrubbed, fd→stdin), plugging into M5a's pipeline as `origin:"external"` taint-root events.

**Architecture:** One new `Parser` impl (`SandboxedMarkitdownParser`, feature `markitdown`) + a `ParserRouter` selector dispatching rich extensions to it, others to `NativeTextParser`. The child is a bundled pinned venv (markitdown **0.1.6**) running a first-party `convert_stdin.py` (`convert_stream`, **stripped + asserted** converter registry). Jail = OS tool (`sandbox-exec`/`bwrap`) **proven by an errno-specific active egress probe**; resource bounds enforced authoritatively by the Rust pump. `#![forbid(unsafe_code)]` preserved (no `pre_exec`).

**Tech Stack:** Rust (rustix `process` feature → `kill_process_group`, `std::process::Command`), Python 3.12/3.13 venv (markitdown 0.1.6), macOS Seatbelt (`sandbox-exec`), Linux bubblewrap (`bwrap --unshare-net --unshare-pid`).

**Spec:** `docs/superpowers/specs/2026-06-19-bossclaw-core-m5b-sandboxed-parser-design.md` (Rev 2). Read it first.

---

## 0. Rev 2 changelog (plan review fixes)

Two reviewers verified the plan against the actual code. Folded:

| # | Sev | Fix | Task |
|---|---|---|---|
| **PR1** | CRIT | Tests using `#[cfg(unix)]`-only symbols (`walk_grant`/`WalkLimits`/`ingest_grant_inner`) must live in `#[cfg(all(test, unix))]` modules, not the ungated `mod tests` — else **Windows CI fails to compile**. `is_rich_ext`/`is_skip`/`Display` pure-logic tests stay ungated. | 1,2,3 |
| **PR2** | CRIT | `lib.rs` must `pub use sandbox::sandbox_test_hooks` (gated) — a private `mod sandbox` makes `bossclaw_core::sandbox_test_hooks` unreachable → integration tests won't compile. | 5,7 |
| **PR3** | CRIT | `run_pump` non-success arm binds `bytes` unused → `clippy -D warnings` fails the plan's own gate → rename `_bytes`. | 5 |
| **PR4** | CRIT | `wrap_jail` is macOS/Linux-only but the module gate `all(unix, feature)` admits BSD → gate module/callers to `any(target_os="macos", target_os="linux")`. | 5,6 |
| **PR5** | CRIT | **The egress probe could FALSE-PASS** (`Parse(_)` = "any non-zero exit" conflates a blocked connect with a missing interpreter/bad venv/jail-tool error). Rewrite: child prints a **sentinel** (`CONNECTED`/`REFUSED_PERM`/`REFUSED_CONN`/`OTHER:errno`); probe returns "jail proven" **only** on a jail-layer refusal (EPERM/ENETUNREACH); everything else → fail-closed (skip). | 7 |
| **PR6** | CRIT | The listener `accept()` is racy (single non-blocking call; kernel completes the handshake in the backlog) → a **dedicated accept thread with a bounded timeout**; any accepted socket = definitive jail failure. | 7 |
| **PR7** | CRIT | Process-group kill does NOT contain a `setsid`/double-fork escapee, and the per-OS guarantee differs: **Linux** (PID-ns via `bwrap --unshare-pid`) → assert *no surviving process*; **macOS** (no PID-ns) → assert *no surviving process holds a network socket* + document the residual. The escapee proof must run the **real jail**, not the mock. | 6,10 |
| **PR8** | CRIT | The "no secret by fd" proof must spawn the child while a **live `EventLog`** (real SQLCipher DB handle + signing key) is open, enumerating child fds = exactly stdio. (SQLCipher opens `O_CLOEXEC` — verified — so the posture is safe; the *proof* was too weak.) | 9,10 |
| **PR9** | IMP | **CI runs no `bossclaw-core` job today** (tests run only from `apps/desktop/src-tauri`, which doesn't depend on the crate). Task 11 must **ADD** a default `-p bossclaw-core` job AND the gated `markitdown` job — not "keep the existing one green." | 11 |
| **PR10** | IMP | Spec F13 (Linux without `bwrap` → fail-closed) has no test → add one (point the jail builder at a missing tool → `SandboxUnavailable` → rich files skip). | 10 |
| **PR11** | IMP | `pip-audit` must audit the **lockfile from a separate tool env** (`pip-audit -r <lock> --strict --require-hashes`), not be installed into the shipped venv (which pollutes the audited set). | 4,11 |
| **PR12** | IMP | Pin **markitdown 0.1.6** before Task 4 (verified on disk; registry class names match). `MARKITDOWN_VERSION` const must equal the lockfile; add a CI **drift guard** (`markitdown.__version__ == const`). No literal `0.1.x` in `parser_id`. | 4,8,11 |
| **PR13** | IMP | The wrapper must **assert the registry strip worked** (post-strip set non-empty AND no `Rss/Wikipedia/YouTube/BingSerp` survive), exiting a distinct code if not — a markitdown bump that breaks the strip must fail loudly, not silently strip-all. | 4,10 |
| **PR14** | IMP | Per-spawn fail-closed: `convert()` trusts a cached boot bool. State the mitigation — a spawn whose jail tool is missing/non-exec fails closed (covered), AND document that boot-probe + `(deny default)` is the accepted stance for the silently-no-op case (the mandated post-build security review re-checks). | 7,8 |
| **PR15** | IMP | `run_pump` must `kill_group` + `wait` on **every** error return (incl. `Io`/reader-panic) so no path leaks the child. | 5 |
| **PR16** | IMP | Add a hermetic test that the parser-aware **read cap** (`:611`) lets a >10 MiB rich body through to the parser (Task 1 only tested the walk gate `:490`). | 1 |
| **PR17** | MIN | Delete `ParserRouter::uniform` (half-baked, "no dead code"). `/lib64` + `--ro-bind-try` in the bwrap args. Reconcile `RICH_OUTPUT_CAP`(32 MiB)/`RLIMIT_AS`(1 GiB)/`MAX_RICH_FILE_BYTES`(100 MiB) and tune vs a real 50 MB fixture. `apply_scrub` defined ONCE, all paths call it, the env-scrub test exercises the `wrap_jail` path. Residuals: rich ingest inert until M7 wires the venv path; macOS escapee-until-reboot residual. `cargo audit`/`cargo-deny` cover the new `rustix/process` surface. macOS hardened-runtime packaging (O2) is a **hard** release gate, not prose. | 3,5,6,8,10,11,12 |

**Verified-good (don't relitigate):** the `&dyn Parser → &ParserRouter` migration scope is correct (desktop doesn't depend on the crate; only `ingest.rs` tests call `ingest_all`). rustix names are correct (`kill_process_group`, `Pid::from_raw` rejects 0, `Signal::Kill`). M5a walk fds + SQLCipher fds are all `O_CLOEXEC`. The M5a seam is genuinely safe. markitdown 0.1.6 registry strip class names all match.

---

## File Structure

| File | Action | Responsibility |
|---|---|---|
| `crates/bossclaw-core/Cargo.toml` | modify | `markitdown` feature → `rustix/process` + `dep:tempfile` |
| `crates/bossclaw-core/src/ingest.rs` | modify | `is_rich_ext`, `max_rich_file_bytes`, parser-aware caps, `IngestError` variants + `is_skip`, empty→skipped, `ParserRouter`, `NullRichParser` (tests `#[cfg(all(test, unix))]`) |
| `crates/bossclaw-core/src/sandbox.rs` | create | parser, venv discovery, per-OS jail, **errno-sentinel egress probe**, concurrent pump, group-kill, `sandbox_test_hooks` (`#[cfg(all(any(target_os="macos",target_os="linux"), feature="markitdown"))]`) |
| `crates/bossclaw-core/src/lib.rs` | modify | gated `mod sandbox;` + `pub use` parser **and** `sandbox_test_hooks` |
| `crates/bossclaw-core/python/convert_stdin.py` | create | wrapper: registry strip **+ assertion**, `convert_stream`, UTF-8, rlimit backstop |
| `crates/bossclaw-core/python/requirements-{macos-arm64,linux-x86_64}.lock` | create | per-platform `--require-hashes` lockfiles (markitdown 0.1.6) |
| `crates/bossclaw-core/scripts/build-venv.sh` | create | reproducible venv + `pip-audit -r <lock>` from a separate env |
| `crates/bossclaw-core/tests/sandbox.rs` | create | gated jail proofs (probe, escapee, fd-with-live-EventLog, bwrap-absent, real conversions) |
| `.github/workflows/*.yml` | modify | **NEW** `bossclaw-core` default job + gated `markitdown` job + `pip-audit` + drift guard |

**Naming locked:** `is_rich_ext`, `RICH_EXTS`, `MAX_RICH_FILE_BYTES`, `max_rich_file_bytes`, `IngestError::{SandboxUnavailable(String),Timeout}`, `is_skip`, `ParserRouter::{new,native_only,pick}` (NO `uniform`), `NullRichParser`, `SandboxedMarkitdownParser`, `Venv`, `apply_scrub`, `wrap_jail`, `probe_egress`, `ProbeOutcome`, `run_pump`, `kill_group`, `discover_venv`, `sandbox_test_hooks`.

---

## Task 1: Cargo feature + `is_rich_ext` + parser-aware byte budget (F1)

**Files:** modify `Cargo.toml`, `ingest.rs` (consts ~27, `WalkLimits` ~354, walk gate ~490, read ~611). **Test gating (PR1):** the walk/read tests go in `#[cfg(all(test, unix))]`; `is_rich_ext_matches` (pure) stays in `mod tests`.

- [ ] **Step 1: `Cargo.toml`.** Add feature + optional tempfile:

```toml
[features]
markitdown = ["rustix/process", "dep:tempfile"]

[dependencies.tempfile]
version = "3"
optional = true
```
(Leave the existing `[dev-dependencies] tempfile = "3"` so hermetic tests keep it.)

- [ ] **Step 2: Failing tests.** Pure-logic test in `#[cfg(test)] mod tests`:

```rust
#[test]
fn is_rich_ext_matches_only_the_sandboxed_set() {
    for e in ["pdf","docx","pptx","xlsx","xls","msg"] { assert!(is_rich_ext(Some(e)), "{e}"); }
    for e in ["txt","md","csv","json","html","rs"] { assert!(!is_rich_ext(Some(e)), "{e}"); }
    assert!(!is_rich_ext(None));
}
```
Walk-budget + read-cap tests in `#[cfg(all(test, unix))] mod rich_budget_tests` (PR1 + PR16):

```rust
#[test]
fn walk_applies_rich_budget_to_rich_ext_and_native_budget_to_others() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("big.pdf"), vec![b'a'; 500]).unwrap();
    std::fs::write(dir.path().join("big.txt"), vec![b'a'; 500]).unwrap();
    let limits = WalkLimits { max_file_bytes: 100, max_rich_file_bytes: 10_000, ..Default::default() };
    let (mut seen, mut report, mut walked) = (std::collections::HashSet::new(), IngestReport::default(), Vec::new());
    walk_grant(dir.path(), &limits, Instant::now(), &mut seen, &mut report, |wf| { walked.push(wf.canonical_path); Ok(()) }).unwrap();
    assert!(walked.iter().any(|p| p.ends_with("big.pdf")));
    assert!(report.skipped.iter().any(|(p,r)| p.ends_with("big.txt") && r=="oversize"));
}

#[test]
fn read_cap_lets_a_large_rich_body_through_to_the_parser() {
    // PR16: prove the READ cap (:611), not just the walk gate (:490), is rich-aware.
    // Build a grant with a >native-cap .pdf of valid UTF-8; ingest with a MockParser;
    // assert it is NOT skipped "oversize" and the mock text lands.
    // (Mirror the M5a orchestrator test scaffolding: EventLog::open + add_grant + MockEmbedder.)
}
```

- [ ] **Step 3: Run → fail.** `cargo test -p bossclaw-core is_rich_ext walk_applies_rich read_cap_lets 2>&1 | tail` → FAIL.

- [ ] **Step 4: Implement** (as Rev 1: add `MAX_RICH_FILE_BYTES = 100*1024*1024`, `RICH_EXTS`, `is_rich_ext`, the `max_rich_file_bytes` field + Default, the parser-aware `cap` at the walk gate `:490`, and the parser-aware `read_cap` at `:611`). Full code in Rev 1 §Task 1 Step 4 — unchanged except the test placement above.

- [ ] **Step 5: Run → pass + no regressions.** `cargo test -p bossclaw-core 2>&1 | grep "test result"` green on mac/Linux; confirm the new tests compile under `--target x86_64-pc-windows-msvc` check is N/A (they're `unix`-gated, PR1).

- [ ] **Step 6: Commit.** `git commit -m "feat(bossclaw-core): M5b T1 — markitdown feature + parser-aware byte budget (F1)"`

---

## Task 2: `IngestError` variants + `is_skip` routing (F8)

**Files:** `ingest.rs` (enum ~96, Display ~111, the parse match ~616). Pure asserts in `mod tests` (ungated); any pipeline-routing assertion in `#[cfg(all(test, unix))]`.

- [ ] **Step 1: Failing test** (ungated — `IngestError` + `is_skip` are not unix-gated):

```rust
#[test]
fn error_skip_classification_is_correct() {
    assert_eq!(IngestError::SandboxUnavailable("x".into()).to_string(), "sandbox unavailable: x");
    assert_eq!(IngestError::Timeout.to_string(), "parser timed out");
    assert!(IngestError::SandboxUnavailable("x".into()).is_skip());
    assert!(IngestError::NonUtf8.is_skip());
    assert!(!IngestError::Timeout.is_skip());
    assert!(!IngestError::Parse("y".into()).is_skip());
}
```

- [ ] **Step 2–4: Run → fail → implement → pass.** Add the `SandboxUnavailable(String)`/`Timeout` variants + Display, and `is_skip` (Rev 1 §Task 2 Step 3). Add a one-line comment that `TooLarge` in `is_skip` is for completeness — the read path intercepts `TooLarge` before `convert` (`:613`). Replace the parse match to use `e.is_skip()`.

- [ ] **Step 5: Commit.** `git commit -m "feat(bossclaw-core): M5b T2 — IngestError Sandbox/Timeout + is_skip routing (F8)"`

---

## Task 3: `ParserRouter` + `NullRichParser` + dispatch + empty→skipped (F7/F12)

**Files:** `ingest.rs` (new types ~143; `ingest_all` ~518; `ingest_grant_inner` ~582). Tests in `#[cfg(all(test, unix))]` (they touch the orchestrator) except the pure dispatch test.

- [ ] **Step 1: Failing tests.** Pure dispatch (ungated):

```rust
#[test]
fn router_dispatches_by_extension() {
    let r = ParserRouter::new(Box::new(MockParser{output:"N".into()}), Box::new(MockParser{output:"R".into()}));
    let pdf = PathHint{ext:Some("pdf".into())}; let txt = PathHint{ext:Some("txt".into())};
    assert_eq!(r.pick(&pdf).convert(b"",&pdf).unwrap(), "R");
    assert_eq!(r.pick(&txt).convert(b"",&txt).unwrap(), "N");
}
#[test]
fn native_only_router_skips_rich_with_sandbox_unavailable() {
    let r = ParserRouter::native_only(); let pdf = PathHint{ext:Some("pdf".into())};
    assert!(matches!(r.pick(&pdf).convert(b"%PDF",&pdf).unwrap_err(), IngestError::SandboxUnavailable(_)));
}
```
Empty-skip in `#[cfg(all(test, unix))] mod orchestrator_tests` named `empty_extraction_is_skipped_not_an_empty_event` — feed a grant + `ParserRouter::new(Box::new(MockParser{output:"   ".into()}), Box::new(NullRichParser))`, assert `report.skipped` contains `"no extractable text"` and `report.ingested == 0`.

- [ ] **Step 2–4: Run → fail → implement → pass.** Add `NullRichParser` + `ParserRouter` **without `uniform`** (PR17):

```rust
pub struct NullRichParser;
impl Parser for NullRichParser {
    fn convert(&self, _r: &[u8], _h: &PathHint) -> Result<String, IngestError> {
        Err(IngestError::SandboxUnavailable("markitdown parser not available".into()))
    }
    fn parser_id(&self) -> &str { "null-rich" }
}
pub struct ParserRouter { native: Box<dyn Parser>, rich: Box<dyn Parser> }
impl ParserRouter {
    pub fn new(native: Box<dyn Parser>, rich: Box<dyn Parser>) -> Self { Self { native, rich } }
    pub fn native_only() -> Self { Self { native: Box::new(NativeTextParser), rich: Box::new(NullRichParser) } }
    pub fn pick(&self, hint: &PathHint) -> &dyn Parser {
        if is_rich_ext(hint.ext.as_deref()) { self.rich.as_ref() } else { self.native.as_ref() }
    }
}
```
Change `ingest_all`/`ingest_grant_inner` to take `&ParserRouter`; in the loop: `let parser = router.pick(&wf.hint);` then convert + the `if text.trim().is_empty()` skip + `file_ingested_content(..., parser.parser_id(), ...)` (Rev 1 §Task 3 Step 3c). Migrate the M5a test callers (`grep -rn "ingest_all(" crates/bossclaw-core` → wrap each in `ParserRouter::new(Box::new(<the parser>), Box::new(NullRichParser))` or `native_only()`).

- [ ] **Step 5: Commit.** `git commit -m "feat(bossclaw-core): M5b T3 — ParserRouter dispatch + empty->skipped (F7/F12)"`

---

## Task 4: Wrapper + venv + lockfiles — pinned 0.1.6, asserted strip, audited lockfile (F2/F9/F15, PR11–13)

**Files:** create `python/convert_stdin.py`, `scripts/build-venv.sh`, the two `.lock` files.

- [ ] **Step 1: Pin the version FIRST (PR12).** Confirm the target release: `markitdown==0.1.6`. This literal is reused in the lockfile, the `parser_id` const (Task 8), and the CI drift guard (Task 11). Do not proceed with `0.1.x`.

- [ ] **Step 2: Wrapper** `python/convert_stdin.py` — with the strip **assertion** (PR13):

```python
#!/usr/bin/env python3
"""First-party markitdown entry point for the jailed child (M5b). Bytes on
stdin → Markdown on stdout. argv[1] = extension hint (no path). Strips the
converter registry to a minimal OFFLINE set AND asserts the strip worked, so a
markitdown bump that breaks it fails LOUDLY (exit 9) instead of silently."""
import sys, resource
for r, l in ((resource.RLIMIT_AS, 1 << 30), (resource.RLIMIT_CPU, 20), (resource.RLIMIT_FSIZE, 64 << 20)):
    try: resource.setrlimit(r, (l, l))
    except (ValueError, OSError): pass
sys.stdout.reconfigure(encoding="utf-8")
from markitdown import MarkItDown, StreamInfo
# requests.Session() is created unconditionally inside MarkItDown — that latent
# network capability is acceptable ONLY because the jail denies network. The
# strip below is defense-in-depth + attack-surface minimization (F2/F14).
ALLOW = {"PdfConverter","DocxConverter","PptxConverter","XlsxConverter","XlsConverter","OutlookMsgConverter","PlainTextConverter"}
BANNED = {"RssConverter","WikipediaConverter","YouTubeConverter","BingSerpConverter"}
md = MarkItDown(enable_plugins=False, enable_builtins=True)
md._converters = [c for c in md._converters if type(getattr(c, "converter", c)).__name__ in ALLOW]
names = {type(getattr(c, "converter", c)).__name__ for c in md._converters}
if not names or (names & BANNED):
    sys.stderr.write(f"registry strip failed: kept={sorted(names)}\n"); sys.exit(9)   # PR13: loud
ext = sys.argv[1] if len(sys.argv) > 1 else None
si = StreamInfo(extension=("." + ext)) if ext else None
sys.stdout.write(md.convert_stream(sys.stdin.buffer, stream_info=si).text_content)
```

- [ ] **Step 3: Build script** `scripts/build-venv.sh` — audit the **lockfile from a separate env** (PR11):

```bash
#!/usr/bin/env bash
set -euo pipefail
PYBIN="${PYBIN:-python3.12}"; DEST="${1:?usage: build-venv.sh <dest>}"
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) LOCK="$(dirname "$0")/../python/requirements-macos-arm64.lock" ;;
  Linux-x86_64) LOCK="$(dirname "$0")/../python/requirements-linux-x86_64.lock" ;;
  *) echo "unsupported: $(uname -s)-$(uname -m)" >&2; exit 1 ;;
esac
"$PYBIN" -m venv "$DEST"
"$DEST/bin/pip" install --upgrade pip
"$DEST/bin/pip" install --require-hashes -r "$LOCK"
# PR11: audit the LOCKFILE from a throwaway env — never install pip-audit into $DEST.
AUDIT=$(mktemp -d); "$PYBIN" -m venv "$AUDIT/v"; "$AUDIT/v/bin/pip" install pip-audit
"$AUDIT/v/bin/pip-audit" --strict --require-hashes -r "$LOCK"; rm -rf "$AUDIT"
ver=$("$DEST/bin/python" -c "import markitdown; print(markitdown.__version__)")
[ "$ver" = "0.1.6" ] || { echo "version drift: venv has $ver, expected 0.1.6" >&2; exit 1; }   # PR12
cp "$(dirname "$0")/../python/convert_stdin.py" "$DEST/convert_stdin.py"
```

- [ ] **Step 4: Generate the per-platform lockfiles** (`pip-compile --generate-hashes` for `markitdown[pdf,docx,pptx,xlsx,xls,outlook]==0.1.6` on each platform) and confirm magika's ONNX model is **vendored** (`python -c "import magika,pathlib; assert list(pathlib.Path(magika.__path__[0]).rglob('*.onnx'))"` — if empty, add an offline-model step; **do not** ship a first-run fetch). Commit both `.lock`.

- [ ] **Step 5: Smoke + assert the strip (PR13).**

```bash
chmod +x crates/bossclaw-core/scripts/build-venv.sh
crates/bossclaw-core/scripts/build-venv.sh /tmp/m5b-venv
printf 'hello **world**' | /tmp/m5b-venv/bin/python /tmp/m5b-venv/convert_stdin.py txt   # markdown, exit 0
/tmp/m5b-venv/bin/python /tmp/m5b-venv/convert_stdin.py pdf < some.pdf | head
# Prove the strip removed the net converters (must NOT exit 9; len < default):
/tmp/m5b-venv/bin/python -c "import convert_stdin" 2>&1 || true
```
If exit 9 / strip mismatch → fix the `_converters`/`.converter` attribute access against 0.1.6 before committing.

- [ ] **Step 6: Commit.** `git commit -m "feat(bossclaw-core): M5b T4 — convert_stdin (asserted strip) + pinned audited venv (F2/F9/F15)"`

---

## Task 5: Concurrent I/O pump + group-kill — kill on ALL paths, OS-gated module (F4/F6/F11, PR2/3/4/15)

**Files:** create `sandbox.rs`; modify `lib.rs`.

- [ ] **Step 1: Module registration with the correct gate + re-exports (PR2/PR4).** In `lib.rs`:

```rust
#[cfg(all(any(target_os = "macos", target_os = "linux"), feature = "markitdown"))]
mod sandbox;
#[cfg(all(any(target_os = "macos", target_os = "linux"), feature = "markitdown"))]
pub use sandbox::{SandboxedMarkitdownParser, sandbox_test_hooks};   // PR2: hooks reachable
```

- [ ] **Step 2: Failing pump tests** (Rev 1 §Task 5 Step 2 — `pump_streams_stdin_to_stdout`, `pump_kills_on_timeout`, `pump_enforces_output_cap`, `pump_does_not_deadlock_on_large_interleaved_io`).

- [ ] **Step 3: Run → fail.** `cargo test -p bossclaw-core --features markitdown pump_ 2>&1 | tail` → FAIL.

- [ ] **Step 4: Implement `run_pump` + `kill_group`** (Rev 1 §Task 5 Step 4) with two corrections:
  - **PR3:** the non-success arm binds `_bytes`: `Ok(_bytes) if !status.success() => {...}`.
  - **PR15:** route EVERY error return through `kill_group(pid)` + `let _ = child.wait();` — including the `Io`/reader-panic paths. Wrap the tail logic so a panic in a reader thread or a `try_wait` error still kills + reaps the child before returning. (Add a small `finish_err(pid, &mut child, e)` helper that kills+waits+returns `Err(e)`.)
  - `kill_group` uses `rustix::process::kill_process_group(Pid::from_raw(pid)?, Signal::Kill)` (verified-correct names).

- [ ] **Step 5: Run → pass + clippy + unsafe check.** `cargo test -p bossclaw-core --features markitdown pump_` PASS; `cargo clippy -p bossclaw-core --features markitdown --all-targets -- -D warnings` clean (PR3 makes this pass); `grep -n "unsafe" crates/bossclaw-core/src/sandbox.rs` → none.

- [ ] **Step 6: Commit.** `git commit -m "feat(bossclaw-core): M5b T5 — concurrent pump, kill-on-all-paths, group-kill (F4/F6/F11)"`

---

## Task 6: Venv discovery + per-OS jail + single `apply_scrub` (F5, PR4/PR7/PR17)

**Files:** `sandbox.rs`; create `sandbox_profiles/seatbelt.sb`.

- [ ] **Step 1: Failing env-scrub test — exercise the `wrap_jail` path (PR17).** Set a fake secret env var, build the REAL jailed command around `/bin/sh` (so the test proves the *production* scrub path, not a test-only one), assert the child sees `CLEAN` + the scratch cwd.

- [ ] **Step 2–4: Run → fail → implement → pass.** Implement `discover_venv` + `Venv` (Rev 1 §Task 6 Step 3) and **one** `apply_scrub(&mut Command, &Path)` (env_clear + allowlist + `current_dir`) called by BOTH the test helper and `wrap_jail` (PR17 — no scrub drift). 

- [ ] **Step 5: OS jail (`wrap_jail`) — gated, with `/lib64` (PR4/PR17).** macOS: `sandbox-exec -D SCRATCH=<dir> -p <profile>` (the `seatbelt.sb` from Rev 1 §Task 6 Step 5, `(deny default)` + `(deny network*)`). Linux: `bwrap --unshare-net --unshare-pid --unshare-ipc --die-with-parent --ro-bind-try /usr /usr --ro-bind-try /bin /bin --ro-bind-try /lib /lib --ro-bind-try /lib64 /lib64 --proc /proc --dev /dev --bind <scratch> <scratch> --chdir <scratch>`. `--unshare-pid` is what makes the Linux escapee guarantee real (PR7). Both `#[cfg(target_os=...)]`; the module gate (PR4) ensures no other OS compiles this.

- [ ] **Step 6: Commit.** `git commit -m "feat(bossclaw-core): M5b T6 — venv discovery + per-OS jail + shared apply_scrub (F5)"`

---

## Task 7: Errno-specific active egress probe — no false-pass (F3, PR5/PR6/PR14)

**Files:** `sandbox.rs`; create `tests/sandbox.rs`.

- [ ] **Step 1: Failing gated test** in `tests/sandbox.rs`:

```rust
#![cfg(all(any(target_os = "macos", target_os = "linux"), feature = "markitdown"))]
#[test] #[ignore]
fn egress_probe_proves_network_denied_and_fails_closed_on_non_network_error() {
    assert!(bossclaw_core::sandbox_test_hooks::probe_egress_blocks(), "jail must deny network");
    // Negative control (PR5): an UN-jailed probe MUST report not-proven (it connects).
    assert!(!bossclaw_core::sandbox_test_hooks::unjailed_probe_blocks(), "un-jailed must connect — probe has teeth");
}
```

- [ ] **Step 2: Run → fail.**

- [ ] **Step 3: Implement the sentinel probe (PR5/PR6).** The child prints exactly one sentinel; the parent reads the errno class — "proven" ONLY on a jail-layer refusal:

```rust
enum ProbeOutcome { Connected, RefusedPerm, RefusedConn, Other(String), ChildError(String) }

/// True iff the jail genuinely denies network. Spec §5.2/F3. Fail-closed: any
/// outcome that is not a positively-identified jail-layer refusal → false.
pub(crate) fn probe_egress(venv: &Venv) -> bool { run_probe(venv, /*jailed=*/true) }

fn run_probe(venv: &Venv, jailed: bool) -> bool {
    let listener = match TcpListener::bind("127.0.0.1:0") { Ok(l) => l, Err(_) => return false };
    let port = listener.local_addr().unwrap().port();
    // PR6: a dedicated accept thread with a bounded window — ANY accept = jail failed.
    let accepted = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let acc2 = accepted.clone();
    listener.set_nonblocking(false).ok();
    let acc_thread = std::thread::spawn(move || {
        let _ = listener.set_nonblocking(false);
        // bounded by the pump timeout; uses a connect-from-self trick to unblock, or SO_RCVTIMEO.
        if let Ok((_s, _)) = accept_with_timeout(&listener, std::time::Duration::from_secs(4)) {
            acc2.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    });
    // Child prints ONE sentinel describing the connect result.
    let script = format!(
        "import socket,sys,errno\ntry:\n socket.create_connection(('127.0.0.1',{port}),timeout=2); print('CONNECTED')\nexcept OSError as e:\n print('REFUSED_PERM' if e.errno in (errno.EPERM,errno.EACCES,errno.ENETUNREACH,errno.EAFNOSUPPORT) else ('REFUSED_CONN' if e.errno==errno.ECONNREFUSED else 'OTHER:%d'%(e.errno or -1)))");
    let scratch = match tempfile::tempdir() { Ok(d) => d, Err(_) => return false };
    let py = venv.python.to_str().unwrap();
    let cmd = if jailed { wrap_jail(scratch.path(), py, &["-c".into(), script]) }
              else { let mut c = Command::new(py); c.args(["-c", &script]); apply_scrub(&mut c, scratch.path()); c };
    let outcome = match run_pump(cmd, b"", 1 << 16, 64 << 10, std::time::Duration::from_secs(5)) {
        Ok(s) => match s.trim() {
            "CONNECTED" => ProbeOutcome::Connected,
            "REFUSED_PERM" => ProbeOutcome::RefusedPerm,
            "REFUSED_CONN" => ProbeOutcome::RefusedConn,
            o => ProbeOutcome::Other(o.to_string()),
        },
        Err(e) => ProbeOutcome::ChildError(e.to_string()),   // missing python / jail-tool error → NOT proven
    };
    let _ = acc_thread.join();
    let any_accept = accepted.load(std::sync::atomic::Ordering::SeqCst);
    // PROVEN only if: child reported a jail-layer refusal AND nothing was accepted.
    matches!(outcome, ProbeOutcome::RefusedPerm) && !any_accept
}

pub mod sandbox_test_hooks {
    pub fn probe_egress_blocks() -> bool { super::discover_venv().map(|v| super::probe_egress(&v)).unwrap_or(false) }
    pub fn unjailed_probe_blocks() -> bool { super::discover_venv().map(|v| super::run_probe(&v, false)).unwrap_or(false) }
}
```
(`accept_with_timeout` = set `SO_RCVTIMEO` via a `socket2` helper or a non-blocking poll loop bounded to the window; implement minimally without `unsafe`.)

- [ ] **Step 4: Iterate until BOTH assertions pass** on macOS + Linux: the jailed probe → `RefusedPerm`+no-accept → true; the un-jailed probe → `Connected`/accept → false. **PR5 teeth check is now a committed test, not manual.** If the jailed child reports `ChildError` (jail tool broke), the probe correctly returns false (fail-closed) — verify that path too.

- [ ] **Step 5: Commit.** `git commit -m "feat(bossclaw-core): M5b T7 — errno-sentinel egress probe, no false-pass (F3)"`

---

## Task 8: `SandboxedMarkitdownParser` — pinned id, reconciled caps (PR12/PR14/PR17)

**Files:** `sandbox.rs`; gated test in `tests/sandbox.rs`.

- [ ] **Step 1: Failing gated test** `converts_real_pdf_and_reports_parser_id` (Rev 1 §Task 8 Step 1) — assert `parser_id() == "markitdown-sandboxed-v0.1.6"`.

- [ ] **Step 2–4: Run → fail → implement → pass.** As Rev 1 §Task 8 Step 3, with:
  - `const MARKITDOWN_VERSION: &str = "0.1.6";` (PR12 — must equal the lockfile).
  - **Cap reconciliation (PR17):** document the relationship `MAX_RICH_FILE_BYTES (input, 100 MiB) ≥` expected output; set `RICH_OUTPUT_CAP` to cover a legit large doc's *markdown* (markdown is usually << input, 32 MiB is generous) and `RLIMIT_AS` ≥ input + parser working set (F9: `convert_stream` buffers the whole stdin). Tune against the 50 MB fixture in Task 10 before locking; leave a `// TODO(O4): confirm against 50MB xlsx` ONLY if a Task-10 test pins it — otherwise pick conservative values now.
  - **Per-spawn fail-closed (PR14):** doc-comment that `discover()` runs the probe as the boot gate; a spawn whose jail tool is missing/non-exec fails closed via `run_pump`'s spawn error → `SandboxUnavailable`; the silently-no-op'd-tool case is covered by the boot probe + `(deny default)` profile and re-verified by the mandated post-build security review.

- [ ] **Step 5: Commit.** `git commit -m "feat(bossclaw-core): M5b T8 — SandboxedMarkitdownParser (pinned 0.1.6, fail-closed)"`

---

## Task 9: End-to-end ingest + the live-EventLog fd proof (PR8)

**Files:** `tests/sandbox.rs`; confirm `ingest.rs` wiring.

- [ ] **Step 1: Failing gated tests.** `ingest_grant_with_pdf_creates_external_taint_event` (Rev 1) **plus** the fd proof must run here where a real `EventLog` exists (PR8):

```rust
#[test] #[ignore]
fn child_inherits_no_fd_while_a_live_eventlog_is_open() {
    // Open a REAL EventLog (SQLCipher DB handle + signing key in memory), THEN
    // run the jailed wrapper that enumerates /proc/self/fd (Linux) or /dev/fd
    // (macOS) and prints its open fds. Assert exactly stdin/stdout/stderr — proves
    // the DB handle (CLOEXEC) and key never reach the child.
}
```

- [ ] **Step 2–5: Implement + the hermetic feature-off regression** (Rev 1 §Task 9 Step 5 — `native_only` router → `.pdf` → `SandboxUnavailable` skip). Commit.

`git commit -m "test(bossclaw-core): M5b T9 — e2e ingest + live-EventLog fd proof (PR8)"`

---

## Task 10: Adversarial jail proofs — per-OS escapee, bwrap-absent, strip mis-fire (PR7/PR10/PR13/PR17)

**Files:** `tests/sandbox.rs`.

- [ ] **Step 1: Write each `#[ignore]` proof**, with the per-OS and fail-closed corrections:

```rust
#[test] #[ignore] fn no_path_arg_reaches_child() {/* argv = only ext + wrapper path */}
#[test] #[ignore] fn rust_wallclock_fires_without_wrapper_rlimit() {/* wrapper never setrlimits + sleeps → Timeout */}
#[test] #[ignore] fn rlimit_as_backstop_kills_oversized_alloc() {/* > RLIMIT_AS alloc → clean fail, not host OOM */}
#[test] #[ignore] fn malformed_pdf_fails_without_hang_or_crash() {}
#[test] #[ignore] fn large_50mb_doc_ingests() {/* PR17: tune caps so a legit 50MB doc ingests, not "failed" */}
#[test] #[ignore] fn embedded_url_makes_no_outbound_connection() {/* RSS/HTML-sniffable bytes → zero accepts; proves the strip (PR13) */}

// PR7: per-OS escapee guarantee differs.
#[cfg(target_os = "linux")]
#[test] #[ignore] fn double_fork_escapee_leaves_no_survivor() {/* real jail; PID-ns → no surviving pid at all */}
#[cfg(target_os = "macos")]
#[test] #[ignore] fn double_fork_escapee_leaves_no_networked_survivor() {/* real jail; assert no survivor holds a socket; document the until-reboot residual */}

// PR10: F13 fail-closed when the jail tool is absent.
#[test] #[ignore] fn missing_jail_tool_fails_closed() {/* point wrap_jail at a non-existent tool → probe false → discover() => SandboxUnavailable => rich files skip */}

// PR8 fd proof can also live here if Task 9 didn't host it.
```

- [ ] **Step 2: Implement + iterate** until all pass on the right OS. The escapee + `embedded_url` tests MUST run the **real jail** (not the `/bin/sh` mock). For `missing_jail_tool_fails_closed`, add a test seam (e.g. `BOSSCLAW_JAIL_TOOL` override or a `wrap_jail` injectable tool path) so the test can point at `/nonexistent`.

- [ ] **Step 3: Commit.** `git commit -m "test(bossclaw-core): M5b T10 — adversarial proofs (per-OS escapee, fail-closed, strip)"`

---

## Task 11: CI — ADD the bossclaw-core jobs + pip-audit + drift guard (PR9/PR11/PR12, §9)

**Files:** `.github/workflows/*.yml` (likely `build.yml` and/or `conformance.yml` — grep for existing bossclaw-core usage first).

- [ ] **Step 1: VERIFY the gap, then ADD (PR9).** `grep -rn "bossclaw-core\|cargo test" .github/workflows`. Confirm no `cargo test -p bossclaw-core` job exists today (tests run only from `apps/desktop/src-tauri`). **Add** a `bossclaw-core` job (macOS + Ubuntu) running `cargo test -p bossclaw-core` + `cargo clippy -p bossclaw-core --all-targets -- -D warnings` on default features (keeps the pure build honest).

- [ ] **Step 2: Add the gated `markitdown-jail` job** (macOS + Ubuntu): install `python3.12` + (Ubuntu) `bubblewrap`; run `scripts/build-venv.sh /tmp/m5b-venv` (which runs the lockfile `pip-audit` from a separate env — PR11 — and the 0.1.6 drift check — PR12); then `BOSSCLAW_MARKITDOWN_VENV=/tmp/m5b-venv cargo test -p bossclaw-core --features markitdown --test sandbox -- --ignored` + `cargo clippy --features markitdown --all-targets -- -D warnings`.

- [ ] **Step 3: Supply-chain extras.** Add `cargo audit`/`cargo-deny` covering the new `rustix/process` surface (§9). Add a step asserting `markitdown.__version__ == "0.1.6"` (drift guard, PR12) — redundant with build-venv.sh but visible in CI.

- [ ] **Step 4: macOS packaging — HARD gate (PR17/O2).** A step (or a required manual release gate, documented as blocking) proving a **signed/notarized hardened-runtime** build can spawn `sandbox-exec` + the embedded interpreter. If it can't, M7 desktop wiring is blocked. Not optional prose.

- [ ] **Step 5: Commit.** `git commit -m "ci(bossclaw-core): M5b T11 — ADD bossclaw-core + gated jail jobs, pip-audit, drift guard (PR9/11/12)"`

---

## Task 12: Docs, residuals, status (PR17)

- [ ] **Step 1: Flip the spec Status** to "IMPLEMENTED — pending dedicated security review of the built jail." Residuals (spec §13 + new): rich ingest is **inert until M7 wires `BOSSCLAW_MARKITDOWN_VENV`/the bundled path** (default desktop skips rich files); macOS escapee survives-until-reboot (net-denied); Linux without `bwrap` skips rich files; `sandbox-exec` deprecation contingency; dedup-on-parser-upgrade won't re-land improved extraction; Windows deferred.
- [ ] **Step 2: `python/README.md`** — venv build, lockfile regen (0.1.6), pip-audit cadence, the strip-assertion contract.
- [ ] **Step 3: Full gate** (Rev 1 §Task 12 Step 3 commands) — default green, both clippy configs clean, gated jail suite green with a built venv, zero `unsafe`.
- [ ] **Step 4: Commit.** `git commit -m "docs(bossclaw-core): M5b T12 — status, residuals, venv README"`

---

## Mandated post-implementation gate

Per spec §10 + the Vault-Brain parent doc: a **dedicated security review of the BUILT jail** before merge — re-run the §11 invariant→proof map (the plan-review's table), confirm the egress probe's teeth on real hardware, and run a live adversarial pass (a crafted malicious PDF against the running jail). Only then open the PR. The plan-review found the *plan's* proofs can pass against a jail that isn't closed; the built review is the backstop that the *implementation* actually closes it.
