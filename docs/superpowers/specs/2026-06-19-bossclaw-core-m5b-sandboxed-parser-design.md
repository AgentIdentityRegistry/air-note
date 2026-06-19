# bossclaw-core M5b — Sandboxed `markitdown` Parser (Design Spec)

- **Milestone:** M5b (brick 1 of the Vault-Brain architecture — GBrain `air/vault-brain-architecture` — the universal safe door for untrusted bytes)
- **Date:** 2026-06-19
- **Status:** **IMPLEMENTED** (T1–T12 on branch `bossclaw-core-m5b-sandboxed-parser`). All jail proofs green on macOS (network-denied with teeth, real-PDF e2e, no DB-handle/secret fd leak, hostile-doc → zero egress, fail-closed degradation). The **mandated security review of the built jail = SHIP-WITH-FIXES, 0 Critical** — all 8 §11 invariants verified to hold *in the code*. Review #2 (CI version-drift guard) + #3 (test hooks moved behind a `sandbox-test-hooks` feature so they don't ship in the `markitdown` build) folded. **Pre-merge follow-up: review #1** — commit a real hash-pinned `requirements-linux-x86_64.lock` (needs a Linux x86_64 env; the dev box is macOS-only — CI currently regenerates it fresh). The Linux jail (bwrap/PID-ns) is validated by the CI Ubuntu leg, not locally. Spec Rev 2 design below is unchanged. See §0 changelog for the plan-review history.
- **Builds on:** M5a (Ingest Pipeline, `main 6fa2b51`) — read-only folder ingest + the taint root
- **Crate:** `crates/bossclaw-core` (engine; `#![forbid(unsafe_code)]`)

---

## 0. Rev 2 changelog (what the second opinion changed)

Two independent reviewers (security + critic) read Rev 1 against the **actual M5a code** and the **real markitdown 0.1.6 source**. Both: SHIP-WITH-FIXES. Folded fixes:

| # | Severity | Fix folded |
|---|---|---|
| **F1** | CRIT | **The 10 MiB M5a cap silently no-ops the feature.** `MAX_FILE_BYTES` rejects oversize files *before* the parser runs; a routine 15 MB PDF was being dropped. → **parser-aware byte budget** (§5.5, §7). |
| **F2** | CRIT | **The jail is the SSRF control, NOT `convert_stream`.** markitdown creates `requests.Session()` unconditionally and keeps net-active converters registered. → reframed (§3); **wrapper strips the converter registry to a minimal allowlist** (§6.1). |
| **F3** | CRIT | **macOS `sandbox-exec` is deprecated + the probe checked presence, not efficacy.** → **active egress probe** as a runtime startup gate + per-spawn fail-closed + pinned `(deny default)` profile + removal contingency (§5.2). |
| **F4** | CRIT | **rlimit-after-exec window** — Python `setrlimit` runs after interpreter startup. → rlimits demoted to defense-in-depth; **authoritative guarantees = Rust wall-clock + output cap + OS jail**; prefer a `ulimit`-exec shim (§5.1, §5.4, §5.5). |
| **F5** | CRIT | **DEK/signing-key leakage via fd/argv/cwd, not just env.** → explicit no-secret-by-any-channel invariant + **O_CLOEXEC audit** + fd-enumeration test (§5.1, §10, §11). |
| **F6** | CRIT | **False claim "killpg via rustix wrappers already used in M5a."** rustix is `fs`-only; `nix` absent; M5a uses no process syscalls. → corrected: enable rustix `process` feature (`kill_process_group`), add to audit; note `Child::kill()` may suffice if markitdown doesn't fork (§5.4, §9). |
| **F7** | IMP | **"Reuse M5a unchanged" understated the work.** → §2/§7 now **enumerate the concrete M5a edits** (router seam, error-arm, cap). |
| **F8** | IMP | **O6 is not a free choice** — `SandboxUnavailable` must be its own variant routed to `skipped` (mapping onto `Parse` would wrongly mark it `failed`) (§7, §8). |
| **F9** | IMP | **Extension dot-prefix bug + non-seekable buffering.** `PathHint.ext` has no dot; `StreamInfo(extension=…)` wants `.pdf`. `convert_stream` buffers the whole non-seekable stdin → couples `RLIMIT_AS` to input size (§6.1, §5.5). |
| **F10** | IMP | **Process-group escape** (hostile `setsid`/double-fork) → **Linux PID namespace** (`bwrap --unshare-pid`); macOS residual documented under net-deny (§5.3, §11). |
| **F11** | IMP | **stdin/stdout pipe deadlock** → mandate **concurrent** pump + **incremental** output cap + bounded stderr cap (§5.5). |
| **F12** | IMP | **Empty extraction pollutes recall** (scanned image-only PDF → `""`). → empty/whitespace-only extraction is **`skipped` ("no extractable text")**, not an empty ingest (§7, §8). |
| **F13** | IMP | **Linux O3 resolved fail-closed** — no `bwrap`/userns/netns ⇒ skip-with-report; Python-seccomp is not accepted as a "network-hard" tier unless it passes the active egress probe with `no_new_privs` + full syscall set pre-import (§5.2). |
| **F14** | IMP | **magika picks the in-child converter by content, not the extension** — abuse surface = union of enabled converters. → minimal converter allowlist is the real boundary (§3, §6.1). |
| **F15** | IMP/MIN | **Supply chain:** per-platform hashed lockfiles; vendor magika's ONNX model (no first-run fetch); native wheels (lxml/numpy/pandas/onnxruntime) = priority audit set (§9). Plus: `parser_id` naming, dedup-on-upgrade residual, determinism, success criteria, A2A-door scoping (below). |

**Verified-good (both reviewers, against code):** the M5a seam is genuinely safe — `PathHint` is one field (ext only, no path), the parser gets bytes from M5a's already-contained `careful_open_file` read, `#![forbid(unsafe_code)]` is real, and M5b opens **no new taint path** (output treated identically to native external text; evolve doors + recall exclusion intact). The bones, threat model, milestone cut, and fail-closed posture are sound; the bundled-venv decision is correct (a frozen binary wouldn't shrink the pandas/pdfminer/lxml weight and would lose `pip-audit`).

---

## 1. Goal, Non-Goals & Success Criteria

### Goal
Make **rich document formats** (PDF, Word, PowerPoint, Excel, legacy Excel, Outlook `.msg`) ingestable by running Microsoft's `markitdown` inside a **hard-jailed subprocess** that treats every input byte as hostile. Output plugs into M5a's pipeline: extracted Markdown becomes an `origin:"external"` taint-root `file_ingested` event; dedup/supersede/recall reuse M5a.

This is **brick 1** of the Vault-Brain program — the reusable safe **parser** for untrusted bytes.

### Measurable success criteria (acceptance bar)
- A non-trivial real PDF/DOCX/PPTX/XLSX (e.g. a 15–50 MB report) **ingests** to non-empty Markdown. *(Forces the F1 cap fix — Rev 1 would have failed this.)*
- An **image-only / scanned** PDF (no extractable text) is reported **`skipped` ("no extractable text")**, not ingested as empty.
- A **malicious** file that hijacks the parser **cannot** reach the network, read our secrets, escape the resource caps, or hang the host — proven by the §10 jail tests.
- Turning the `markitdown` feature off (or a missing venv) degrades to **exactly M5a behavior** (text/markdown still ingests; rich files skip-with-report).

### Non-Goals (deferred)
- **Windows** ingest (M5a's `ingest_all` is `#[cfg(unix)]`; M5b stays macOS + Linux). Windows job-object jail = follow-up.
- **Cloud formats** (image-OCR, audio, video, YouTube) — need network + cloud credentials; structurally incompatible with the deny-network jail. Out unless re-architected.
- **Archive recursion** (ZIP) — zip-bomb surface; deferred with its own budget.
- **The egress gate / lineage walk** governing what tainted content may *do* — brick 2 / M6.
- **Sensitivity compartments + secret vaulting** — brick 3.
- **The A2A byte ingest *entry point*** — see §2 "A2A scoping." M5b delivers the reusable *parser*; the wire-bytes ingest path is a later brick (brick 4).
- **Broadened ingest scope** (whole-disk / on-demand) — brick 5.

### Considered & rejected
**Per-format pure-Rust parsers** (lopdf/`calamine`/`docx`) instead of jailed Python: rejected for brick 1 — it fragments the parser surface into many libraries, Rust PDF text-extraction is materially weaker on messy real-world PDFs, and it loses markitdown's maintained breadth. We standardize on **one** safe door. (Revisitable per-format later as an optimization behind the same seam.)

---

## 2. Context — where M5b plugs in, and the real M5a edits

M5a left a precise seam (`crates/bossclaw-core/src/ingest.rs`):

```rust
pub trait Parser {                                  // ingest.rs:126
    fn convert(&self, raw: &[u8], hint: &PathHint) -> Result<String, IngestError>;
    fn parser_id(&self) -> &str;
}
// PathHint { ext: Option<String> }  — lowercased, NO dot, NO path (ingest.rs:87, 498)
```

M5b adds `SandboxedMarkitdownParser` behind cargo feature `markitdown` (mirrors `ollama`). **The taint root, dedup/supersede, `origin:"external"` stamp, `EMBEDDABLE_EVENT_TYPES`, the recall exclusion arm, `exclude_files`, and both shut evolve doors are reused verbatim.** But "nothing else changes" was wrong — the honest, enumerated M5a edits:

1. **Parser routing.** `ingest_grant_inner` (`#[cfg(unix)]`) currently calls a single `parser.convert(&raw, &wf.hint)` (`ingest.rs:616`) with one `&dyn Parser` threaded from `ingest_all`. M5b introduces a **router** (dispatch by extension, §7) — either a `RouterParser` wrapping native + sandboxed, or constructing the sandboxed parser inside the orchestrator. Touches `ingest_all`/`ingest_grant_inner` internals.
2. **Error routing.** The convert `match` (`ingest.rs:616–620`) routes `NonUtf8 → skipped`, else `→ failed`. A new `SandboxUnavailable → skipped` arm is required (§8).
3. **Parser-aware byte budget.** The 10 MiB `MAX_FILE_BYTES` (`ingest.rs:27`, comment: *"rich/large formats wait for M5b"*) must become parser-aware (§5.5) — else rich files are rejected before the jail.

**A2A scoping (honest).** Rev 1 claimed M5b is "the same door for local AND A2A files." Verified: M5a has **no byte-source ingest entry point** — only the folder walk produces `ContainedFile`s. So M5b delivers the **reusable parser component** (`convert(&[u8], &PathHint)` is already byte-source-agnostic); the **A2A ingest entry point** (dedup/taint/append from wire bytes + the signed envelope) is **brick 4**, which will reuse this parser. M5b should keep the parse step free of any walk/`ContainedFile` coupling so that future reuse is a thin adapter, not a refactor — but it does not build the A2A path now.

---

## 3. Threat Model

**Trust boundary:** input bytes are **fully untrusted** (attacker-authored local file, or — later — an A2A file). The parser stack (`markitdown` + `pdfminer.six` + `pdfplumber` + `pandas` + `lxml` + `python-pptx` + `mammoth` + `magika`/ONNX + `requests` …) is ~284 MB of third-party code, several **native wheels**. Document parsers are a heavy historical RCE surface. **Assume breach:** a malicious file *will* eventually hijack the parser — the jail makes "then what" be *nothing leaves, nothing is fetched, nothing persists, it dies on a clock.*

markitdown's own README: *"MarkItDown performs I/O with the privileges of the current process… handles local files, **remote URIs**, and byte streams. Sanitize inputs in untrusted environments."* Confirmed in source (0.1.6): `requests.Session()` is created unconditionally and net-active converters (Rss/Wikipedia/YouTube/BingSerp) stay registered — inert only because we pass no `url`. **So the network capability lives in the child regardless of API choice.**

| Attack | Primary defense | Defense-in-depth |
|---|---|---|
| Exfiltration / C2 / stage-2 download | **Deny-network jail, fail-closed (the boundary)** | no path/URI to child |
| **SSRF / remote-URI fetch** | **Deny-network jail** | `convert_stream` + no `url` + `enable_plugins=False` + **stripped converter registry** (defense-in-depth, NOT the boundary — F2/F14) |
| Secret theft (DEK / signing key) | `env_clear`+allowlist **AND** O_CLOEXEC fds + no key via argv/cwd (F5) | DEK never serialized near the child |
| Path re-resolution / TOCTOU | bytes-on-stdin only; M5a's contained read | one-field `PathHint` |
| Resource bomb / zip-bomb | **Rust wall-clock + output cap (authoritative)** + OS-jail tmpfs/FSIZE | Python `setrlimit` (secondary, F4) |
| Hang / DoS | wall-clock timeout → **PID-ns death / `killpg`** | concurrent pump (no self-deadlock) |
| Process escape (setsid/double-fork) | **Linux PID namespace**; macOS net-deny residual (F10) | — |
| FS tamper / read-out | OS jail (Seatbelt/bwrap) deny-write + scratch cwd | best-effort fs-read confine |

**Important (F14):** the Rust dispatch routes by extension, but **inside** the child markitdown picks the converter by **magika content-sniff**, ignoring the extension. So routing `.pdf` to the jail exposes the bytes to *whichever* enabled converter magika selects. The real attack surface is **the set of converters registered in the wrapper** — which is why §6.1 strips it to a minimal allowlist.

**Not claimed:** M5b is not the end-to-end write gate. It plants safe, tainted text; the fail-closed lineage walk governing use is M6.

---

## 4. Architecture

```
                       bossclaw-core (Rust, #![forbid(unsafe_code)])
 ┌────────────────────────────────────────────────────────────────────────┐
 │ ingest_grant_inner (M5a; EDITED: router seam, error arm, parser-aware cap)│
 │   walk → ContainedFile → read_all_capped(bytes, parser-aware cap) ─┐      │
 │   ┌─────────────────────────────────────────────────────────────────▼──┐ │
 │   │ RouterParser (NEW)                                                  │ │
 │   │   .pdf/.docx/.pptx/.xlsx/.xls/.msg → SandboxedMarkitdownParser ─┐    │ │
 │   │   else → NativeTextParser (in-process; UTF-8 in, binary→skip)   │    │ │
 │   └────────────────────────────────────────────────────────────────┼────┘ │
 │   SandboxedMarkitdownParser::convert(&[u8], &PathHint)  (NEW)        │      │
 │     locate venv+wrapper → ACTIVE egress probe (cached+per-spawn FC)  │      │
 │     → build jailed Command (no unsafe, §5) → CONCURRENT pump:        │      │
 │       writer thread: bytes → child stdin                            │      │
 │       reader: child stdout → cap(incremental) ; stderr → bounded     │      │
 │     → on timeout/cap/err → PID-ns death / killpg → IngestError       │      │
 └─────────────────────────────────────────┼───────────────────────────┘      │
   (bytes on stdin / markdown on stdout)    │  FIRST production subprocess in the crate
       ┌───────────────────────────────────▼─────────────────────────────────┐
       │ JAILED CHILD  (deny-net[fail-closed] · OS-jail · scratch cwd · scrubbed)│
       │   [ulimit shim] exec python convert_stdin.py <ext>                   │
       │     setrlimit (backstop) ; registry stripped to allowlist            │
       │     MarkItDown(enable_plugins=False).convert_stream(stdin, .ext)      │
       │     → Markdown to stdout (utf-8)                                      │
       └──────────────────────────────────────────────────────────────────────┘
```

Returned Markdown → M5a pipeline → `file_ingested` (`origin:"external"`) → dedup/supersede → recall. Taint root + evolve doors inherited verbatim.

---

## 5. The Sandbox (network-hard, fs-jail best-effort)

Decision: **non-negotiables always hard-guaranteed; the heavier fs lock layered when the OS supports it; if network denial can't be *proven*, fail closed** (skip-with-report, never run un-jailed).

### 5.1 Always-on (macOS + Linux)
- **env scrub:** `Command::env_clear()` + minimal allowlist (`PATH`→venv bin, `LC_ALL=C.UTF-8`, `HOME=<scratch>`, `PYTHONNOUSERSITE=1`, `PYTHONDONTWRITEBYTECODE=1`, `PYTHONHASHSEED=0`). No DEK/API keys.
- **no secret by ANY channel (F5):** the DEK and Ed25519 signing key (held by `EventLog`, `log.rs:128,148`) must never reach the child via env, **argv**, **cwd contents**, or an **inherited fd**. All long-lived parent fds (the SQLCipher DB handle, the log file) MUST be `O_CLOEXEC`; spawn is audited so **only stdio crosses**. (M5a already opens walk fds `O_CLOEXEC`, e.g. `ingest.rs:266,289`; confirm `rusqlite`/sqlcipher opens CLOEXEC.)
- **process group:** `std::os::unix::process::CommandExt::process_group(0)` (safe stable; std performs the `setpgid` in its own audited `unsafe`, not ours) → group-kill for cooperative helpers.
- **scratch cwd:** `Command::current_dir(<fresh tempdir>)`, owned by a guard dropped on **every** exit path (success/timeout/cap/panic); "shred" = remove-tree.
- **fd hygiene:** only stdin/stdout/stderr connected.
- **no path to child:** only the lowercased extension (`PathHint`) is passed; bytes flow on stdin.
- **resource limits (DEFENSE-IN-DEPTH, F4):** AS/CPU/FSIZE/NOFILE. Authoritative enforcement is the Rust pump + OS jail (below); rlimits are a secondary in-child backstop and are **preferably applied from `exec`** via a `ulimit` shim (`sh -c 'ulimit -v … -t … -f …; exec python convert_stdin.py …'`) so they cover interpreter startup, with Python `resource.setrlimit` as belt-and-suspenders. The spec does **not** count rlimits in the "no hang / no unbounded" guarantee.

### 5.2 Network denial — fail-closed, proven not assumed (F3, F13)
- **Active egress probe (startup gate):** at first use, run the *real* jailed command on a wrapper that attempts `connect()` to a throwaway loopback listener we own; **require refusal** (`EPERM`/`ENETUNREACH`, distinguished from `ECONNREFUSED`). Only then cache "jail proven." Re-assert cheaply per-spawn (a spawn whose jail tool errors fails closed). Presence ≠ efficacy.
- **macOS:** wrap with **`sandbox-exec`** + a pinned Seatbelt profile with **`(deny default)`** semantics, `(deny network*)`, `(deny file-write*)` except scratch. ⚠️ `sandbox-exec` is **deprecated** (still present on macOS 26.x): the profile is reviewed/tested like code, and **if it is ever removed or the egress probe passes through, macOS rich-file ingest skips-with-report** (§13).
- **Linux:** prefer **`bwrap --unshare-net --unshare-pid --unshare-ipc --die-with-parent` + tmpfs + ro-binds**. If unavailable: a seccomp net-block is accepted **only** if it (a) is installed via a pinned audited lib, (b) sets `no_new_privs`, (c) denies the **full** net set (`socket`,`connect`,`bind`,`sendto`,`sendmsg`,`recvfrom`,`socketcall`,`io_uring_setup`,`io_uring_enter`), (d) is installed **before** any markitdown import, **and** (e) passes the active egress probe. **Otherwise fail closed** (O3 resolved). Linux-without-`bwrap`/userns/netns therefore **ingests no rich files** — a real coverage hole, stated in §13, not a footnote.

> "Best-effort" applies **only** to the extra filesystem-read confinement. Network denial is **never** best-effort — proven-or-skip.

### 5.3 Per-OS specifics
| Concern | macOS | Linux |
|---|---|---|
| Network deny | `sandbox-exec` `(deny default)`+`(deny network*)`, **active-probed** | `bwrap --unshare-net` › audited seccomp(full set, probed) › **fail-closed** |
| Subtree death | `process_group(0)`+`killpg` (residual: escapee stays net-denied) | **`bwrap --unshare-pid`** (PID-ns init reaps whole subtree — robust, F10) |
| FS write deny | Seatbelt `(deny file-write*)`+scratch allow | `bwrap` tmpfs + `--ro-bind` |
| rlimits | `ulimit` shim from exec (+ Python backstop) | `ulimit` shim from exec (+ Python backstop) |

### 5.4 `#![forbid(unsafe_code)]` preservation (corrected, F6)
- **process group:** safe `CommandExt::process_group(0)` — no `pre_exec`.
- **rlimits:** `ulimit` shim and/or Python `resource` — not Rust `unsafe`.
- **network/fs jail:** by exec'ing external tools (`sandbox-exec`,`bwrap`) — no Rust `unsafe`.
- **group kill:** **enable rustix's `process` feature** and use `rustix::process::kill_process_group` (safe wrapper). ⚠️ Rev 1 falsely said this was "already used in M5a" — it is **not**: `Cargo.toml` has rustix `features=["fs"]` only, `nix` is absent. Enabling `process` is a **new dependency-surface** → add to the §9 audit set. *Empirical check during impl:* if markitdown never forks under `enable_plugins=False` + no-network, `std::process::Child::kill()` (direct child only) may suffice and group-kill becomes optional — verify before adding the feature.

### 5.5 The I/O pump (authoritative resource guarantees)
- **Concurrent (F11):** stdin-write and stdout-read run **concurrently** (dedicated writer thread or `poll` both fds) — never write-all-then-read (pipe deadlock: child blocks on a full stdout pipe → stops reading stdin → parent blocks). Tested with a 1-byte-out-per-MiB-in wrapper.
- **Incremental output cap (F11):** stdout read in fixed chunks into a `Vec` bounded by `cap+chunk`; on exceeding the cap → kill immediately. **stderr** read into a separate **bounded** buffer (e.g. 64 KiB ring) — never grow-unbounded (pdfminer is chatty). Both caps are named constants (no magic numbers).
- **Wall-clock timeout (authoritative):** independent of the child's `RLIMIT_CPU`; on expiry → PID-ns death (Linux) / `killpg` (macOS). The wrapper must **not** trap `SIGXCPU`. Wall-clock (e.g. 30 s) > `RLIMIT_CPU` (e.g. 20 s) so the soft limit fires first when it works, the wall-clock when it doesn't.
- **Input/AS coupling (F9):** `convert_stream` on a **non-seekable** stdin buffers the entire stream into memory before parsing → peak ≈ input + parser working set. Size `RLIMIT_AS` and the parser-aware input ceiling **together** (§7, O4).

---

## 6. Bundled venv + first-party wrapper

### 6.1 The wrapper (`convert_stdin.py`) — we author & ship it
```python
import sys, resource
# secondary backstop (authoritative limits come from the ulimit shim + Rust pump)
for res, lim in ((resource.RLIMIT_AS, 1<<30), (resource.RLIMIT_CPU, 20), (resource.RLIMIT_FSIZE, 64<<20)):
    resource.setrlimit(res, (lim, lim))
sys.stdout.reconfigure(encoding="utf-8")               # explicit UTF-8 out
from markitdown import MarkItDown, StreamInfo
md = MarkItDown(enable_plugins=False, enable_builtins=True)
# F2/F14: strip the registry to the minimal, audited converter set — removes
# the entire net-active surface (Rss/Wikipedia/YouTube/BingSerp) and any
# LLM/Azure/audio converters, regardless of magika's content guess.
md._converters = keep_only(md._converters, {"Pdf","Docx","Pptx","Xlsx","Xls","OutlookMsg","PlainText"})
ext = sys.argv[1] if len(sys.argv) > 1 else None        # extension hint only, NO path
si  = StreamInfo(extension=("." + ext)) if ext else None # F9: dot-prefixed ".pdf"
out = md.convert_stream(sys.stdin.buffer, stream_info=si)
sys.stdout.write(out.text_content)
```
- **`convert_stream`** is markitdown's narrowest API. **The registry strip (F2/F14) is the real attack-surface control** — `convert_stream` alone leaves `requests.Session()` + net converters live; magika picks the converter by content, so only the *registered* set matters.
- `enable_plugins=False` blocks third-party plugin entry points.
- **No `url=`** (keeps any residual net converter inert).
- *(Exact registry-strip mechanism pinned against the shipped markitdown version at impl; `md._converters` is a plain list in 0.1.6. **O1 resolved:** `StreamInfo(extension=".pdf")`, dot-prefixed; `file_extension=` is deprecated.)*
- **`parser_id`:** `"markitdown-sandboxed-v<markitdown-version>"` — stable provenance stamped on the event (`ingest.rs:621`); lets a future parser-upgrade supersede be reasoned about (see §7 residual).

### 6.2 The bundled environment
- **Pinned** `markitdown[pdf,docx,pptx,xlsx,xls,outlook]` venv in the app bundle. Measured ≈ **284 MB** site-packages + ~50 MB interpreter (no PyTorch/Whisper).
- **Pin Python 3.12 or 3.13** (3.14 lacked wheels for some deps → source builds).
- **Reproducible + audited (F15):** **per-platform** fully-resolved `--require-hashes` lockfiles (macOS arm64 **and** Linux x86_64 are different wheel sets). `pip-audit` at build AND CI (fail on CVE). **Vendor magika's ONNX model** (confirm it ships in the wheel — no first-run network fetch). Priority CVE-watch the **native wheels**: `lxml`, `numpy`, `pandas`, `onnxruntime`.
- **Discovery:** the crate locates the venv via an explicit validated path (app-resources for desktop; configurable for headless/tests). Missing/invalid venv → rich files **skipped-with-report**.
- *(O2: venv production + Tauri-bundle packaging, **and** whether macOS hardened-runtime entitlements permit spawning `sandbox-exec` + an embedded interpreter — must be proven on a signed/notarized build, not just dev.)*

---

## 7. Parser dispatch & M5a integration

`NativeTextParser` stays the **default** (succeeds on any valid UTF-8: `txt/md/csv/json/xml/html/…`; `NonUtf8`→skip on binary, as today). M5b **overrides specific rich-binary extensions** to the jail via a router:
- `pdf, docx, pptx, xlsx, xls, msg` → `SandboxedMarkitdownParser` (feature `markitdown`; feature-off / venv-or-jail-unavailable → **skip-with-report**).
- everything else → `NativeTextParser`.

**Parser-aware byte budget (F1).** Add `max_rich_file_bytes` (e.g. 100 MiB) to `WalkLimits`; apply the existing 10 MiB cap only on the native path, the larger cap on the rich path. Re-derive `RLIMIT_AS` / `RLIMIT_FSIZE` / output-cap from that ceiling **plus** the `convert_stream` full-buffer factor (§5.5). Without this, M5a's `ingest.rs:490` oversize gate silently drops routine rich docs **before** the parser is chosen.

**Empty extraction (F12).** Whitespace-only / empty extracted text → **`skipped` ("no extractable text")**, never a signed empty `file_ingested` event (which would pollute embeddings/recall and the byte-hash dedup).

**Residual — dedup-on-parser-upgrade (gap).** M5a dedups on **raw-byte** `content_hash` (`ingest.rs:621`), not `parser_id`. A file whose extraction *improves* after a markitdown bump has identical raw bytes → **dedups as unchanged**, so the better text never lands. Documented residual; a `parser_id`-aware re-parse path is a follow-up (not M5b).

Unchanged: `content_hash` identity, `origin:"external"` stamp, `EMBEDDABLE_EVENT_TYPES`, recall exclusion, `exclude_files`, evolve doors. Feature-off compiles and degrades to exact M5a behavior.

---

## 8. Error handling & failure posture

Extend `IngestError` (F8 — **own variants**, not mapped onto `Parse`): add `SandboxUnavailable(String)` (→ **`skipped`**) and `Timeout` (→ **`failed`**). The `ingest_grant_inner` match gains a `SandboxUnavailable → skipped` arm. Every failure is **loud + contained** — in `IngestReport.failed`/`skipped`, never a silent drop, never a hang. Display strings must **not** leak the file path or secrets.

| Condition | Result |
|---|---|
| feature off / venv missing / **jail unprovable** | `skipped` (`SandboxUnavailable`) — native ingest continues |
| child non-zero / parse error | `failed` (bounded stderr tail) |
| wall-clock timeout | PID-ns death / `killpg` → `failed: Timeout` |
| output cap exceeded | kill → `failed: output cap` |
| **empty / whitespace-only extraction** | `skipped: no extractable text` |
| child stdout not valid UTF-8 | `failed` (`NonUtf8`) |

Fail-closed network: if the egress probe doesn't prove denial → `skipped`, never run un-jailed.

---

## 9. Supply chain & build (F6, F15)
- **Per-platform** `--require-hashes` lockfiles over the **fully-resolved transitive** tree (macOS arm64 + Linux x86_64).
- **`pip-audit`** at venv-build and CI; CI fails on a known CVE. Priority set: `lxml`, `numpy`, `pandas`, `onnxruntime`, `pdfminer.six`, `pdfplumber`.
- **Vendor magika's ONNX model** (no first-run fetch).
- **Rust side:** `cargo-deny`/`cargo audit`; **add rustix `process` feature** (and any seccomp lib) to the audit set — these are new.
- Reproducible per-OS venv artifacts; documented rebuild; re-`pip-audit` on every pin bump.

---

## 10. Testing strategy

**Hermetic (always):** dispatch routing; skip/failed accounting incl. `SandboxUnavailable→skipped` and empty→skipped; dedup/supersede on a rich-ext mock; recall exclusion still holds; the parser-aware cap routes a >10 MiB rich mock to the rich path. Reuse `MockParser`.

**Real-subprocess jail proofs** (gated on `markitdown` feature + a built venv; `#[ignore]` in default CI, run in a dedicated job — *and the network probe of #1 is also a runtime startup gate, not only a test*):
1. **Network denied** — a connect-attempting wrapper is **refused at the jail layer** (`EPERM`/`ENETUNREACH`, NOT `ECONNREFUSED`); a crafted HTML/RSS stream with an embedded URL makes **zero** outbound connections.
2. **Timeout → killed**, incl. a **double-fork/`setsid` escapee** → assert **no surviving networked process** (PID-ns on Linux).
3. **Output cap → killed**; **stderr flood** bounded (no host OOM).
4. **No secret by env OR fd** — child `environ` shows no planted secret; child `/proc/self/fd`(Linux)/`/dev/fd`(macOS) is exactly stdin/stdout/stderr.
5. **No path leak** — wrapper sees no path arg; cwd is the scratch dir.
6. **Rust wall-clock fires even if the wrapper never calls `setrlimit`** (proves the authoritative guarantee is the Rust side, not the in-child backstop).
7. **rlimit backstop** — over-`RLIMIT_AS` alloc dies cleanly (reported, not host OOM).
8. **Real conversions** — fixture PDF/DOCX/XLSX/PPTX → expected-substring Markdown; a **15 MB** fixture ingests (proves F1); a **scanned image-only** PDF → `skipped: no extractable text` (proves F12).
9. **Malformed input** — truncated/garbage PDF → `failed`, no hang, no host crash.
10. **No deadlock** — 1-byte-out-per-MiB-in (and inverse) respects the cap without stalling.

**macOS packaging check (O2):** prove a **signed/notarized hardened-runtime** Tauri build can spawn `sandbox-exec` + the embedded interpreter (where I5/F3 bites in production).

**Gate:** clippy `-D warnings` (default + `markitdown`), `#![forbid(unsafe_code)]` intact (zero `unsafe`), all hermetic green. **Mandated:** a dedicated **security review of the built jail** before merge.

---

## 11. Security invariants (reviewer checklist)
- [ ] **Network proven-denied** (active egress probe gates startup AND per-spawn; never best-effort; fail-closed otherwise).
- [ ] **No path/URI** to the child (one-field `PathHint`, bytes on stdin).
- [ ] **No secret by ANY channel** — not env, argv, cwd, or **inherited fd**; long-lived parent fds are `O_CLOEXEC`; spawn leaks only stdio.
- [ ] **No hang** — Rust wall-clock always fires; subtree dies (PID-ns / `killpg`); escapees (if any) remain net-denied.
- [ ] **No self-deadlock** — concurrent stdin/stdout pump; incremental caps.
- [ ] **Bounded resources** — Rust output cap + bounded stderr + OS-jail FSIZE are authoritative; rlimits are secondary.
- [ ] **Minimal converter surface** — registry stripped to the audited allowlist (magika can't reach a stripped converter).
- [ ] **No `unsafe`** added (`#![forbid(unsafe_code)]` holds; group-kill via rustix `process`).
- [ ] **No new taint path** — output treated exactly as M5a external text; evolve doors + recall exclusion untouched.
- [ ] **Fail-closed** wherever a guarantee can't be proven (skip-with-report, never degrade-to-unsafe).

---

## 12. Open questions for reviewers
- **O2.** venv packaging in the Tauri bundle vs. headless test discovery; **does a signed/notarized macOS hardened-runtime build permit spawning `sandbox-exec` + the embedded interpreter?**
- **O3.** *(resolved → fail-closed)* Linux without `bwrap`/userns/netns ingests no rich files — confirm that's an acceptable residual for AIR's deployment targets (desktop macOS + typical Linux), or is hardened-container Linux a real target needing another mechanism?
- **O4.** Concrete `max_rich_file_bytes` / `RLIMIT_AS` / output-cap / timeout values — coupled to the `convert_stream` full-buffer factor; tune against real large-but-legit docs.
- **O5.** `.msg` (Outlook/`olefile`) is the **highest-risk** format in the set (legacy OLE, historic malware carrier) — keep it (it's the *best argument for* the jail), but flag as the one to watch.
- **New.** Does markitdown fork under `enable_plugins=False` + stripped registry + no-network? (Determines whether group-kill / the rustix `process` feature is needed at all.)
- **New.** Embedder behavior on empty/short text (informs whether F12's skip is strictly required or just preferred).
- **New.** Determinism: is markitdown extraction byte-stable (with `PYTHONHASHSEED=0`) enough for M5a's byte-identical-rebuild guarantees, or may `text_hash` vary across runs?

---

## 13. Milestone boundary (honest residuals)
- **Unix-only** (macOS + Linux); Windows = follow-up.
- **Linux without `bwrap`/userns/netns ingests no rich files** (fail-closed) — a real coverage hole.
- **macOS network denial rests on the deprecated `sandbox-exec`** — if removed, macOS rich ingest skips-with-report.
- **Dedup-on-parser-upgrade:** improved extraction after a markitdown bump won't re-land (raw-byte dedup); follow-up.
- **M5b introduces the crate's first production subprocess** (M5a's only `Command` use is a test `mkfifo`).
- No archives, no cloud formats, no scope-broadening, no egress gate, no A2A ingest entry point (later bricks).
- The jail confines a *breach*; it does not make markitdown *correct*. Garbage-in → garbage-Markdown-out is acceptable (tainted-external either way; M6 governs use).
