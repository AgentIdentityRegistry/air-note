# bossclaw-core M5b — Sandboxed `markitdown` Parser (Design Spec)

- **Milestone:** M5b (brick 1 of the Vault-Brain architecture — GBrain `air/vault-brain-architecture` — the universal safe door for untrusted bytes)
- **Date:** 2026-06-19
- **Status:** DRAFT — Rev 1, pending independent critic + security second opinion, then Peter review
- **Builds on:** M5a (Ingest Pipeline, `main 6fa2b51`) — read-only folder ingest + the taint root
- **Crate:** `crates/bossclaw-core` (engine; `#![forbid(unsafe_code)]`)

---

## 1. Goal & Non-Goals

### Goal
Make **rich document formats** (PDF, Word, PowerPoint, Excel, legacy Excel, Outlook `.msg`) ingestable by `bossclaw-core` by running Microsoft's `markitdown` inside a **hard-jailed subprocess** that treats every input byte as hostile. Output plugs into M5a's existing pipeline unchanged: the extracted Markdown becomes an `origin:"external"` taint-root `file_ingested` event, dedup/supersede/recall all reuse M5a.

This is **brick 1** of the Vault-Brain program: the same jailed parser is the door for **both** local-file ingest (today) and **A2A-received files** (future) — any untrusted bytes entering the brain pass through here.

### Non-Goals (explicitly deferred)
- **Windows** ingest (M5a's `ingest_all` is already `#[cfg(unix)]`; M5b stays unix-first: macOS + Linux). Windows job-object jail is a documented follow-up.
- **Cloud formats** — image-OCR, audio transcription, video, YouTube. These require network + cloud credentials (Azure Document Intelligence / Content Understanding, online speech recognition) and are structurally incompatible with the deny-network jail. Permanently out unless re-architected.
- **Archive recursion** (ZIP "iterate contents") — a zip-bomb surface; deferred with its own depth/size budget design.
- **The egress gate / lineage walk** that decides what tainted content may influence — that is brick 2 / M6.
- **Sensitivity compartments + secret vaulting** — brick 3.
- **Broadened ingest scope** (whole-disk / on-demand reach) — brick 5. M5b changes the *parser*, not the *scope* (M5a's folder-grant model is unchanged).

---

## 2. Context — where M5b plugs in

M5a left a precise seam. From `crates/bossclaw-core/src/ingest.rs`:

```rust
pub trait Parser {
    fn convert(&self, raw: &[u8], hint: &PathHint) -> Result<String, IngestError>;
    fn parser_id(&self) -> &str;
}
```

- `PathHint` carries **only a lowercased extension** — never a path. This is the security contract: a parser cannot re-resolve a filesystem location.
- M5a ships `NativeTextParser` (strict-UTF-8) + `MockParser` (tests). The orchestrator (`ingest_grant_inner`) owns the contained read + identity hashing; it hands the parser **bytes**.

M5b adds **one** `Parser` impl — `SandboxedMarkitdownParser` — behind a cargo feature `markitdown` (mirroring the existing `ollama` feature gate). **Nothing else in M5a changes**: the careful-open walk, `ContainedFile`, dedup/supersede, the `origin:"external"` stamp, `EMBEDDABLE_EVENT_TYPES`, the recall exclusion arm, and the two shut evolve doors are all reused as-is.

---

## 3. Threat Model

**Trust boundary:** input bytes are **fully untrusted** — a local file could be attacker-authored (downloaded, emailed, "invoice.pdf"); an A2A file comes from another agent. The parser code (`markitdown` + `pdfminer.six` + `pdfplumber` + `pandas` + `lxml` + `python-pptx` + `mammoth` + …) is tens of thousands of lines we did not write. Document parsers are a historically heavy RCE surface.

**Vendor confirms the hazard.** markitdown's own README: *"MarkItDown performs I/O with the privileges of the current process… it can handle local files, **remote URIs**, and byte streams. Sanitize your inputs in untrusted environments, and call the narrowest `convert_*` function."* Its permissive `convert()` will **fetch remote URLs** → an SSRF vector if used naively.

**Assume breach.** Design premise: *a malicious file will eventually hijack the parser. Then what?* The jail must make "then what" be: **nothing leaves, nothing is fetched, nothing persists, the process dies on a clock.**

| Attack | Defense in M5b |
|---|---|
| **Exfiltration** (read local data, send it out) | **Deny-network, fail-closed** (no socket at all) |
| **Stage-2 download / C2** | Deny-network |
| **SSRF / remote-URI fetch** (markitdown `convert()` follows URIs) | Use `convert_stream()` on raw bytes only; deny-network; **no path/URI ever passed to the child** |
| **Secret theft from our env** (DEK, API keys) | `env_clear()` + minimal allowlist; the child inherits no secrets |
| **Path re-resolution / TOCTOU** | Child receives **bytes on stdin**, never a path; M5a's careful-open already produced the contained bytes |
| **Resource bomb** (memory/CPU/disk via crafted file or zip-bomb) | `setrlimit` AS/CPU/FSIZE/NOFILE; output-size cap; wall-clock timeout |
| **Hang / DoS** (parser blocks forever) | wall-clock timeout → **process-GROUP kill** (`killpg`, not bare child) |
| **Filesystem tampering** (write/read outside) | scratch-only cwd; FSIZE=small; best-effort fs-jail (Seatbelt/bwrap) denies writes; reads contained where the OS jail supports it |
| **Output flooding** | streamed read with a hard byte cap; kill at cap, don't trust the child to stop |

**What M5b does NOT claim:** it is *not* the end-to-end write gate. It plants safe, tainted text. The fail-closed lineage walk that governs what tainted text may *do* is M6. (Same honest boundary M5a drew.)

---

## 4. Architecture

```
                        bossclaw-core (Rust, #![forbid(unsafe_code)])
  ┌───────────────────────────────────────────────────────────────────────┐
  │  ingest_grant_inner (M5a, unchanged)                                    │
  │    walk → ContainedFile → read_all_capped(bytes) ─┐                     │
  │                                                   │ dispatch by ext     │
  │   ┌───────────────────────────────────────────────▼──────────────────┐ │
  │   │  parser dispatch (NEW, small)                                     │ │
  │   │   .txt/.md/.csv/.json/.xml… → NativeTextParser (in-process, M5a)  │ │
  │   │   .pdf/.docx/.pptx/.xlsx/.xls/.msg → SandboxedMarkitdownParser ─┐  │ │
  │   └────────────────────────────────────────────────────────────────┼──┘ │
  │                                                                     │    │
  │   SandboxedMarkitdownParser::convert(&[u8], &PathHint) (NEW)        │    │
  │     1. locate bundled venv + wrapper                                │    │
  │     2. build per-OS jailed Command (no unsafe — see §5)             │    │
  │     3. spawn; stream bytes → child stdin                            │    │
  │     4. read child stdout under output cap + wall-clock timeout      │    │
  │     5. on timeout/cap/error → killpg + IngestError                  │    │
  └─────────────────────────────────────────┼──────────────────────────┘    │
                                            │ (raw bytes on stdin / md on stdout)
        ┌───────────────────────────────────▼──────────────────────────────┐
        │  JAILED CHILD  (deny-net · rlimits · env-scrubbed · scratch cwd)   │
        │   python convert_stdin.py <ext>                                   │
        │     MarkItDown().convert_stream(sys.stdin.buffer, hint=<ext>)     │
        │     → writes Markdown to stdout                                    │
        └───────────────────────────────────────────────────────────────────┘
```

Returned Markdown re-enters M5a's pipeline → `file_ingested` event with `origin:"external"` → dedup/supersede → recall. **The taint root and both shut evolve doors are inherited verbatim.**

### Components
1. **`SandboxedMarkitdownParser`** (`Parser` impl, feature `markitdown`) — orchestrates locate → spawn → pump → reap.
2. **Per-OS sandbox launcher** — builds the jailed `Command` (§5).
3. **The I/O pump** — bounded, timeout-guarded stdin-write / stdout-read with process-group kill.
4. **`convert_stdin.py`** — a tiny **first-party** wrapper we ship & control (§6).
5. **Parser dispatch** — extension → parser routing (§7).

---

## 5. The Sandbox (network-hard, fs-jail best-effort)

Per Peter's decision: the **non-negotiables are always hard-guaranteed**; the heavier filesystem lock is **layered when the OS supports it**; if the non-negotiable **network denial cannot be guaranteed, we fail closed** (skip the file, report it) rather than run.

### 5.1 Always-on (every supported platform)
- **env scrub:** `Command::env_clear()` then add a minimal allowlist (`PATH` to the venv bin, `LC_ALL=C.UTF-8`, `HOME=<scratch>`, `PYTHONNOUSERSITE=1`, `PYTHONDONTWRITEBYTECODE=1`). The child inherits **no** DEK, API keys, or ambient secrets.
- **process group:** `std::os::unix::process::CommandExt::process_group(0)` (safe, stable) → the child leads its own group so a timeout can `killpg` the whole tree (markitdown may spawn helpers).
- **resource limits:** `RLIMIT_AS` (e.g. 1 GiB), `RLIMIT_CPU` (e.g. 20 s), `RLIMIT_FSIZE` (e.g. 64 MiB), `RLIMIT_NOFILE` (small) — **set inside `convert_stdin.py` via Python's `resource` module before importing markitdown** (keeps the Rust crate free of `unsafe pre_exec`; see §5.4).
- **scratch cwd:** `Command::current_dir(<fresh tempdir>)`; shredded after. markitdown's temp work stays contained.
- **fd hygiene:** only stdin (read), stdout (write), stderr (capped) are connected; no inherited fds.
- **no path to child:** the file path is never an argument or env var — only the lowercased **extension** is passed (`PathHint`), bytes flow over stdin.
- **wall-clock timeout** in the Rust pump (e.g. 30 s) → `killpg(SIGKILL)`; independent of the child's own `RLIMIT_CPU`.
- **output cap** in the Rust pump (e.g. 32 MiB) → kill at the cap; never buffer unbounded.

### 5.2 Network denial (fail-closed, the crown jewel)
Provided by the strongest available mechanism, checked at startup:
- **macOS:** wrap with **`sandbox-exec`** (Seatbelt) using a profile that `(deny network*)` and `(deny file-write*)` except the scratch dir. `sandbox-exec` is a system binary (present on all macOS); if absent/erroring → fail closed.
- **Linux:** prefer **`bwrap --unshare-net --unshare-ipc --die-with-parent --ro-bind … --tmpfs …`** (bubblewrap); else a **seccomp-bpf** filter installed in the wrapper (via a pinned, audited lib) blocking `socket`/`connect`/`socketcall`; else **fail closed**.
- **Capability probe** at parser construction caches which mechanism is live. If none can guarantee no-network on this host → `SandboxedMarkitdownParser` reports unavailable and rich files are **skipped-with-report** (never run un-jailed).

> The "best-effort" in "network-hard, fs best-effort" applies **only** to the *extra* filesystem-read confinement (Seatbelt/bwrap give it; the seccomp-only baseline does not). Network denial is **never** best-effort — it's guaranteed-or-skip.

### 5.3 macOS / Linux specifics
| Concern | macOS | Linux |
|---|---|---|
| Network deny | `sandbox-exec` `(deny network*)` | `bwrap --unshare-net` › seccomp `socket`/`connect` block › fail-closed |
| FS write deny | Seatbelt `(deny file-write*)` + scratch allow | `bwrap` tmpfs + `--ro-bind` |
| FS read confine | Seatbelt (best-effort) | `bwrap` ro-binds (best-effort) |
| rlimits | wrapper `resource.setrlimit` | wrapper `resource.setrlimit` |
| group kill | `process_group(0)` + `killpg` | `process_group(0)` + `killpg` |

### 5.4 `#![forbid(unsafe_code)]` preservation (design constraint)
bossclaw-core forbids `unsafe`. The naive sandbox path — `CommandExt::pre_exec` to call `setrlimit`/`setsid` in the child — is **`unsafe`** and therefore **banned**. The design routes around it:
- **process group** via the safe, stable `CommandExt::process_group(0)` (no `pre_exec`).
- **rlimits** set in the **Python wrapper** (`resource.setrlimit`) before importing markitdown — not in Rust.
- **env / cwd / stdio** via safe `Command` methods.
- **network/fs jail** by **exec'ing external jail tools** (`sandbox-exec`, `bwrap`) and/or a seccomp filter installed **in the wrapper** — no Rust `unsafe`.
- Signals (`killpg`) via the `rustix`/`nix` safe wrappers already used in M5a (rustix keeps `forbid(unsafe)` intact).

This is a primary review target: **prove the Rust side stays `unsafe`-free while the jail still holds.**

---

## 6. Bundled venv + first-party wrapper

### 6.1 The wrapper (`convert_stdin.py`)
A ~20-line script **we author and ship** (not markitdown's CLI), so we control the entry point precisely:
```python
import sys, resource
# rlimits first, before any heavy import
resource.setrlimit(resource.RLIMIT_AS,    (1<<30, 1<<30))
resource.setrlimit(resource.RLIMIT_CPU,   (20, 20))
resource.setrlimit(resource.RLIMIT_FSIZE, (64<<20, 64<<20))
# (optional) install seccomp net-block here on Linux baseline
from markitdown import MarkItDown, StreamInfo
ext = sys.argv[1] if len(sys.argv) > 1 else None     # extension hint only, no path
si  = StreamInfo(extension=ext) if ext else None
out = MarkItDown(enable_plugins=False).convert_stream(sys.stdin.buffer, stream_info=si)
sys.stdout.write(out.text_content)
```
- Uses **`convert_stream()`** — Microsoft's documented *narrowest* API (no local-file or remote-URI handling). Magika (a markitdown core dep) sniffs type from bytes; the `PathHint` extension is a belt-and-suspenders hint.
- `enable_plugins=False` — no third-party plugin surface.
- *(Exact `convert_stream` hint param/`StreamInfo` shape to be pinned against the chosen markitdown version at implementation; confirmed `convert_stream(stream, …)` exists per vendor README. **Open item O1.**)*

### 6.2 The bundled environment
- A **pinned** `markitdown[pdf,docx,pptx,xlsx,xls,outlook]` venv shipped inside the app bundle. Measured size ≈ **284 MB** site-packages + ~50 MB interpreter (no PyTorch/Whisper — the heavy deps are `pandas`/`pdfminer`/`pdfplumber`).
- **Pin Python 3.12 or 3.13** for full wheel coverage (3.14 lacked wheels for some deps in testing → source builds; avoid).
- **Reproducible build:** a lockfile (`requirements.txt` with `--hash=` pins) + a build script per OS; `pip-audit` at install-build AND in CI.
- **Location & discovery:** the Rust crate locates the venv via an explicit, validated path (app-resources dir for the desktop; a configurable path for headless/tests). If the venv is missing/invalid → rich files **skipped-with-report** (graceful: text/markdown ingest via `NativeTextParser` still works).
- *(How the venv is produced & shipped in the Tauri bundle vs. the headless `bossclaw-core` test harness — **open item O2**.)*

---

## 7. Parser dispatch & M5a integration

A small dispatch by `PathHint` extension, inside `ingest_grant_inner` where the parser is chosen. **`NativeTextParser` stays the default** — it succeeds on any valid UTF-8 (`txt/md/csv/json/xml/html/…`) and returns `NonUtf8`→skip on binary, exactly as M5a does today. M5b only **overrides specific rich-binary extensions** to the jail:
- `pdf, docx, pptx, xlsx, xls, msg` → `SandboxedMarkitdownParser` (feature `markitdown`; if the feature is off or the venv/jail is unavailable → skip-with-report).
- **everything else** → `NativeTextParser` (in-process, no subprocess) — UTF-8 ingested, binary skipped-with-report.

**No changes** to: `content_hash` identity, dedup/supersede, the `origin:"external"` stamp, `EMBEDDABLE_EVENT_TYPES`, the recall exclusion arm, `exclude_files`, or the evolve doors. The Markdown produced by the jail is treated **identically** to native-parsed text — already tainted-external, already gated downstream.

Feature-flag behavior: with `--no-default-features` (no `markitdown`), the crate compiles and rich files skip-with-report — text/markdown ingest unaffected. Mirrors the `ollama` gate.

---

## 8. Error handling & failure posture

Extend `IngestError` minimally (or map onto existing `Parse`): add `SandboxUnavailable(String)` and `Timeout`. Every failure mode is **loud and contained** — surfaced in `IngestReport.failed`/`skipped`, **never** a silent drop, **never** a hang:

| Condition | Result |
|---|---|
| feature off / venv missing / jail unavailable | `skipped` (reason recorded); native ingest continues |
| child non-zero exit / parse error | `failed` (stderr tail recorded, truncated) |
| wall-clock timeout | `killpg` → `failed: Timeout` |
| output exceeds cap | `killpg` → `failed: output cap` |
| empty/garbage extraction | ingested as empty/short text *iff* valid UTF-8, else `failed` |

Fail-closed network: if the network jail can't be established, the parser refuses (skip), it does **not** silently run un-jailed.

---

## 9. Supply chain & build

- **Pin** markitdown + full transitive tree with hashes (`--require-hashes`); record the resolved set.
- **`pip-audit`** at venv-build time and in CI; CI fails on a known CVE in the pinned set.
- **`cargo-deny`/`cargo audit`** already cover the Rust side; add any new sandbox crate (e.g. a seccomp lib) to the audit set.
- Reproducible per-OS venv build artifacts; document the rebuild procedure.
- Rotate the pin on a cadence; re-run `pip-audit` on bump.

---

## 10. Testing strategy

**Hermetic (always run):**
- Pipeline tests reuse `MockParser` — dispatch routing, skip/failed accounting, dedup/supersede with a rich-ext file (mock bytes), recall-exclusion still holds. No real subprocess.
- Dispatch table: each extension routes to the expected parser; unknown → skip.

**Real-subprocess jail proofs (gated on `markitdown` feature + a built venv; `#[ignore]` in default CI like M5a's live-Ollama tests, run in a dedicated job):**
1. **Network denied** — a wrapper variant that attempts to connect to a local listener is **blocked** (proves the jail, not just config).
2. **Timeout → killed** — a wrapper that sleeps forever is `killpg`'d; the pump returns `Timeout`; no orphan process/group survives.
3. **Output cap → killed** — a wrapper emitting unbounded bytes is cut at the cap.
4. **Env scrubbed** — a wrapper dumping `os.environ` shows none of a planted secret env var.
5. **No path leak** — wrapper asserts it received no path arg / cwd is the scratch dir.
6. **rlimit enforced** — a wrapper allocating > `RLIMIT_AS` dies cleanly (reported, not a host OOM).
7. **Real conversions** — a tiny fixture PDF/DOCX/XLSX/PPTX converts to expected-substring Markdown.
8. **Malformed input** — truncated/garbage PDF → `failed`, no hang, no crash of the host.

**Gate:** clippy `-D warnings` clean (default + `markitdown`), `#![forbid(unsafe_code)]` intact (zero `unsafe`), all hermetic green.

**Mandated:** a **dedicated security review** of the implemented jail before merge (per parent spec §11 / Vault-Brain). The spec itself also gets an independent critic + security second opinion (this Rev).

---

## 11. Security invariants (reviewer checklist)

A reviewer must be able to confirm each:
- [ ] **No network** reachable from the child (guaranteed-or-skip; never best-effort).
- [ ] **No path/URI** ever reaches the child — bytes on stdin, extension-only hint.
- [ ] **No secrets** in the child env (`env_clear` + allowlist; no DEK/API keys).
- [ ] **No hang** — wall-clock timeout always fires; `killpg` reaps the whole group.
- [ ] **No unbounded resource use** — rlimits + output cap + scratch FSIZE.
- [ ] **No `unsafe`** added to the Rust crate (`#![forbid(unsafe_code)]` holds).
- [ ] **No new taint path** — output is treated exactly as M5a external text; the two evolve doors and recall exclusion are untouched.
- [ ] **Fail-closed** everywhere a guarantee can't be met (skip-with-report, never degrade-to-unsafe).
- [ ] **No plugin / convert() permissiveness** — `enable_plugins=False`, `convert_stream` only.

---

## 12. Open questions for reviewers
- **O1.** Exact `convert_stream` hint API (`StreamInfo(extension=…)` vs `file_extension=…`) for the pinned markitdown version — pin at implementation.
- **O2.** Venv packaging: how it's produced and shipped in the Tauri bundle vs. located by the headless `bossclaw-core` test harness; the discovery path contract.
- **O3.** Linux baseline when neither `bwrap` nor a userns is available — is a wrapper-installed seccomp net-block sufficient to call "network-hard," or do we fail closed there? (Leaning fail-closed.)
- **O4.** rlimit/timeout/output-cap concrete values — defaults above are starting points; tune against real large-but-legit docs.
- **O5.** Should `.msg` (Outlook) ship in v1 or defer with archives? (Currently in; tiny dep `olefile`.)
- **O6.** Is `IngestError::Timeout`/`SandboxUnavailable` worth new variants, or map onto `Parse`/`Io`? (Leaning new variants for clear reporting.)

---

## 13. Milestone boundary (honest residuals)
- Unix-only (macOS + Linux). Windows = follow-up.
- No archives, no cloud formats, no scope-broadening, no egress gate (those are later bricks).
- The jail confines a *breach*; it does not make markitdown *correct*. Garbage-in → garbage-Markdown-out is acceptable (it's tainted-external text either way; M6 governs use).
- Linux fs-read confinement is best-effort (only as strong as `bwrap` availability); network denial is not.
