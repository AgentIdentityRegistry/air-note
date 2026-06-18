# bossclaw-core M5a (Ingest Pipeline) — Design Spec (Rev 3)

- **Date:** 2026-06-18
- **Status:** Rev 3 — folds a second independent **critic + security** review (this time of the *implementation plan*), which converged on one genuine spec-level over-claim: "no laundering" was asserted from **evolve-cursor** exclusion alone, but `file_ingested` becoming recallable also opens the evolve **recall-context** path. Rev 3 closes it with an `exclude_files` recall flag (mirrors `exclude_pages`/F3), acknowledges the hardlink-into-grant residual, and reconciles the macOS containment wording. (Rev 2 folded the security + critic review of Rev 1.)
- **Parent:** `docs/superpowers/specs/2026-06-15-bossclaw-core-design.md` §12.5 + §5.10/§5.11, §6, §7, §8.1/8.3/8.4/8.6
- **Builds on:** M1–M4 (all merged to `main`, last `b06d928`).
- **Review basis:** independent **security + critic** reviews of Rev 1 (2026-06-18) — both **SHIP-WITH-FIXES**, converged on the same defects. Rev 2 folds them. Per both reviewers, **M5 is SPLIT** into **M5a (this spec)** + **M5b (the sandboxed parser, follow-on)**.

## 1. Goal

M5a: read-only ingest of **user-granted folders** into the signed log as recallable `file_ingested`
events — the complete **safe pipeline** with **native text/markdown parsing in-process** (no
subprocess), the §12.5 read-side safety (kernel-enforced containment + no-symlink + never-touch
filter + dedup + path-keyed version supersede), and the **taint root** (`file_ingested` durably marked
external-origin in signed content). **Rich-format parsing (PDF/docx/…) via the sandboxed `markitdown`
subprocess is M5b.** Extraction of entities/links *from* file content is deferred to a later milestone.

## 2. Scope (decisions locked 2026-06-18)

| # | Decision | Choice |
|---|----------|--------|
| D1 | **Split** | M5 → **M5a (this: pipeline + native parser + all read-side safety + taint root)** + **M5b (the sandboxed `markitdown` subprocess)**. Mirrors M4a/M4b. |
| D2 | Parser | A `Parser` **seam**. M5a ships a **native plaintext/markdown impl** (in-process UTF-8 read) + a **mock**. The `markitdown`-backed sandboxed impl is **M5b** (feature-gated). |
| D3 | Cross-platform open | Via **`rustix`** so the crate keeps `#![forbid(unsafe_code)]`. Linux: `openat2(RESOLVE_BENEATH \| RESOLVE_NO_SYMLINKS)`. macOS: `openat`-from-verified-dir-fd chain + `O_NOFOLLOW`. Windows: canonicalize-then-contain + reparse-point rejection. |
| D4 | `file_ingested` kind | **Ground-truth event** — plain `append` (NOT the Tier-B `append_pair`/`reject_empty_tier_b` path). Taint/`origin` lives as a key **inside the signed `content`** (canonical + rebuild-stable). |
| D5 | Taint scope | M5a **plants the taint root** (durably mark external) + keeps the empty-`source_event_ids` reject + adds a taint **classifier** + tests. The fail-closed lineage **walk** that *consumes* the root is **M6 (actuator)** — M5a has no writes and does **not** claim an end-to-end write gate. |

> **Why Rev 2:** Rev 1 claimed M5 would "reuse" three existing seams — supersede/recall-exclusion, an `origin` field, and a fail-closed taint walk. Verified against code by both reviewers, **all three were false**: the supersede fold + recall exclusion are `page`/`topic_id`-only (`graph.rs:319`, `log.rs:1095`); `origin` is an `edges`-table column, not an event field; the taint *walk*'s consumer is the unbuilt M6 actuator. Rev 2 relabels each as **net-new** and scopes the DoD to what M5a actually proves. (The Rev 1 spec ironically *cited* this exact M4b lesson and still tripped on it.)

## 3. New event types + projections

- **`grant` / `revoke`** → a `grants` projection (`canonical_root`, `granted_at`, `revoked`); persisted as signed events (survive restart via replay). API: `add_grant(path)`, `revoke_grant(path)`, `grants()`. `ingest_all()` iterates **active grants only**.
- **`file_ingested`** — **ground-truth, plain `append`** (D4). Signed `content`:
  - `text` — converted text (in M5a: native UTF-8 read; M5b: markitdown output).
  - `provenance` — `canonical_path`, `content_hash` (sha256 of file **bytes**, the identity key), `text_hash` (sha256 of converted text), `size_bytes`, `modified_at` (mtime — **provenance-only; NEVER an identity/dedup key**), `parser_id`, `grant_root`.
  - **`origin: "external"`** — the taint stamp, **inside the signed content** (part of JCS canonicalization, the byte-identical rebuild, and the frozen test-vector set).
- **NEW path-keyed `file_ingested` supersede projection** — "latest `file_ingested` per `canonical_path` wins" (analogous to `fold_pages` but keyed on path, not `topic_id`). **Net-new** — the existing fold is page/topic-only.

## 4. Components (M5a)

- **`ingest.rs`** orchestrator: `ingest_grant(root) -> IngestReport`, `ingest_all() -> IngestReport`. **`IngestReport { ingested, superseded, deduped, skipped: Vec<(path, reason)>, failed: Vec<(path, reason)> }`** — **loud** (e.g. "N files matched the never-touch filter; everything else was ingested wholesale").
- **Safe walk:** no-symlink-follow; **kernel-enforced containment via the careful open** (D3 — per-component, so an intermediate-dir swap can't escape on Linux); **inode/hardlink-seen set** (a given inode ingested once per run); the **never-touch filter** (§6.3); size/type caps + a **whole-run wall-clock budget**; appends per file (yields the serialized writer between files).
- **Careful open** (`rustix`, no in-crate `unsafe`): D3 per OS. Returns a `ContainedFile` (the opened handle) — the path string is never re-resolved downstream.
- **`Parser` seam:** `fn convert(&self, file: &ContainedFile, hint: &PathHint) -> Result<String, IngestError>`. **`PathHint` = sanitized type-hint ONLY** (extension/MIME for dispatch; carries **no resolvable path**). M5a impls: **native plaintext/markdown** (in-process UTF-8 read of the handle; non-UTF-8/unsupported → `skipped`) + **mock**. (M5b adds the sandboxed markitdown impl behind a feature.)
- **Append + Tier-A:** plain `append` of `file_ingested` (D4); add `file_ingested` to the indexed/recallable event-type set (`log.rs:120`, today `[MEMORY_EVENT_TYPE, PAGE_EVENT_TYPE]`) → embed + FTS → recallable + byte-identically rebuildable.
- **Recall integration:** `Hit.kind = "file_ingested"` + path provenance; **NEW recall-exclusion arm** for `kind == "file_ingested"` (keep only the current id per path, **before `truncate(k)`**) — net-new (the existing arm at `log.rs:1095` is page-only); also exclude `file_ingested` whose `grant_root` is now **revoked** (walk to the current `grants` projection — never-forget storage ≠ must-surface).
- **Dedup/supersede — one decision table (per `canonical_path`):** lookup by path → **absent** → ingest; **present & same `content_hash`** → no-op (dedup); **present & different `content_hash`** → **supersede** (emit a path-keyed supersede + new `file_ingested`). **Cross-path identical bytes → ingest both** (provenance differs; dedup is **per-path, never global**).
- **Taint:** `origin="external"` in signed content (D4); a taint **classifier** (`is_external(event)`); the empty-`source_event_ids` reject is kept. **No-laundering requires closing BOTH evolve doors:** (1) the evolve **cursor** (what is extracted *from*) is `MEMORY_EVENT_TYPE`-only (verified `log.rs:2455/3249`), so files are never extraction *subjects*; AND (2) the evolve loop's internal **recall context** (what the reasoner reads *as* context, `evolve_once` → `recall(.., exclude_pages:true)` at `log.rs:2951-2964`) must also drop files — because `file_ingested` is now recallable, an unfiltered context recall would feed external file text into `extract::propose`, and the derived `entity`/`link`/`page` would **not** be marked external (laundering). Rev 3 adds an **`exclude_files: bool`** to `RecallOptions` (mirrors the F3 `exclude_pages` one-way rule) and sets it `true` in the evolve recall. The DoD proves a planted file's text never reaches a derived event's `source_event_ids`.

## 5. Data flow

`add_grant(folder)` → `ingest_grant` → **safe walk** (contain · no-symlink · never-touch · caps · inode-seen) → **careful open** (`rustix`) → **`Parser.convert`** (native text in M5a) → **dedup/supersede decision** (per path) → **plain `append` signed `file_ingested`** (ground-truth) → Tier-A (embed + FTS) → **recallable** (current-version-only, active-grant-only).

## 6. Safety / DoD (M5a's honest scope)

1. **Containment** — kernel-enforced by the careful open (D3): Linux `openat2(RESOLVE_BENEATH|RESOLVE_NO_SYMLINKS)` defeats **final- and intermediate-component** symlink/`..` swaps; macOS `openat`-chain + `O_NOFOLLOW`; **Windows reparse-reject + contain is final-component-strong; the intermediate-dir swap-after-check race is a documented residual** (same honesty as the macOS chain). No path string re-resolved after the contain check.
2. **No-symlink-follow + inode/hardlink guard + wall-clock + byte/file budgets** — all fail-closed (skip past caps; never OOM/hang).
3. **Never-touch filter** — **a hazard-reduction filter, NOT a containment boundary** (the boundary is the grant + the user's informed consent — carry parent §5.12's honesty into the UX). Single-sourced, testable const: dot-dirs (`.ssh .aws .azure .gnupg .git .kube .docker gcloud .config/gh`) + names/globs (`.env .netrc .pgpass .git-credentials *.key *.pem id_* *.keychain *.kdbx *.jks *.ppk *.mobileconfig *.ovpn wallet.dat`). A content high-entropy-line skip is a documented **fast-follow**. The `IngestReport` surfaces the match count loudly.
4. **Taint root** (D5) — `file_ingested` durably external-origin in signed content; classifier + empty-lineage reject; **both evolve doors closed** (cursor exclusion + `exclude_files` recall-context exclusion, §4); the consuming fail-closed lineage **walk is M6** (M5a claims no end-to-end write gate).
5. **Fence honesty** — `recall()` returns **RAW** text; M5a does **not** fence at recall. The fence applies at the **reasoner-prompt boundary** (the M4 `push_fenced_source` path, not exercised by M5a). Consumers (the desktop Memory panel) **must treat `file_ingested` text as untrusted**; the fence **raises the bar, it does not make text inert**. (No "wherever surfaced" over-claim.)
6. **Dedup/supersede integrity** (§4 table) — `content_hash` is identity; `mtime` is provenance-only; two distinct paths never supersede each other.
7. **At-rest** — file content lands in the whole-DB SQLCipher store; Tier-A indexes are in-memory only → the §8.1 no-plaintext-index invariant holds. (M5a has **no subprocess/scratch-tempdir** — that §8.1 side-channel is an M5b concern.)

## 7. Error handling

Per-file **best-effort**: parse-fail / oversize / never-touch / unsupported-type → **skip + record reason, continue**. **Safety violations fail closed** (containment reject drops that entry). **No partial `file_ingested`** (append only after a clean convert + dedup decision). Uses the existing serialized writer, **append-per-file** (releases the lock between files, so a long ingest doesn't starve the evolve loop). `revoke_grant` mid-ingest → re-check the grant is active **before each append**.

## 8. Testing (must prove the DoD)

- **Hermetic** (temp homes, native/mock parser). Fixtures: static symlink-escape ✗, `..`-traversal ✗, **TOCTOU swap** (replace the final component with a symlink **after** containment passes via a test seam → assert the careful open **refuses** (`ELOOP`/`RESOLVE_BENEATH` error) — this proves `openat2`/`O_NOFOLLOW`, which a static-symlink test does **not**), never-touch skipped (+ count surfaced), **dedup** (same path+hash → no-op), **changed file → supersede → recall returns ONLY current** (exercises the NEW exclusion arm — distinct from the page test), **two distinct paths never supersede each other**, **cross-path identical bytes → both ingested**, recall-an-ingested-file, **revoked grant → excluded from recall**, inode/hardlink not re-ingested, oversize/wall-clock skip, **mtime changed but bytes identical → no spurious supersede**.
- **Taint:** a `file_ingested`-derived id is classified `external`; empty-lineage still rejected.
- **Tier-A:** `file_ingested` in the byte-identical rebuild; the **frozen canonicalization vector extended** for the new content shape; `recall@k` fixture unaffected by the new kind.
- `clippy -D warnings`; the crate **keeps `#![forbid(unsafe_code)]`** (`rustix` encapsulates the `unsafe`).

## 9. Deferred

- **M5b (next milestone, own spec + security review): the sandboxed `markitdown` subprocess.** Per-OS sandbox (macOS `sandbox-exec`/Seatbelt deny network+file-write; Linux `bwrap`/seccomp or `setrlimit` RLIMIT_AS/CPU/FSIZE/NOFILE; Windows job object) · **process-GROUP kill** on timeout (`setsid`+`killpg`, not a bare child kill) · **minimal allowlisted env** (no inherited DEK-process secrets) · **all fds `CLOEXEC` except a read-only input** · dedicated **scratch cwd** (shredded; on an encrypted volume — the §8.1 side-channel) · **fd→stdin byte-streaming** (the child never re-resolves a path — markitdown via a stdin wrapper, NOT the path-arg CLI the desktop's `markitdown.rs` uses) · **output-cap streaming** (kill at the cap, don't trust the child) · archive depth/size budget · **pinned markitdown version+hashes** + install-time & CI `pip-audit` · defined no-parser-installed behavior. Rich formats (PDF/docx/…) become ingestable in M5b.
- **Extraction** of entities/links from file content (evolve over `file_ingested`) — later milestone; the M5a taint root is what makes it safe.
- **Actuator / writes (M6)** — including the fail-closed lineage **walk** that consumes the taint root, and the confused-deputy write defenses (§8.4).

## 10. Risks

- **Residual TOCTOU — Windows only.** The review (corroborated by both reviewers) found the macOS framing was over-conservative: the Unix walk holds **real directory fds reached via per-descent `O_NOFOLLOW`**, so an already-open ancestor fd is immune to a later name swap and each descent's `NOFOLLOW` (plus the final careful open) closes the per-component symlink-swap window — macOS is effectively as symlink-tight as Linux `openat2` here. The genuine residual is **Windows**, whose `canonicalize`-then-`File::open` is non-atomic (an intermediate dir can be swapped between the contain-check and the open); documented + narrow. (CI must run the containment tests on **both** macOS and Linux so each `cfg` branch is gated, and assert the Linux refusal is `ELOOP`-class so `RESOLVE_NO_SYMLINKS` is what fired.)
- **Hardlink-into-grant is NOT defended (residual).** A hardlink placed inside a granted folder, pointing at an inode whose other name is *outside* the grant (same filesystem), is caught by **neither** `O_NOFOLLOW` **nor** `openat2 RESOLVE_BENEATH` (a hardlink has no parent pointer; the name genuinely is beneath the root) **nor** the name-based never-touch filter. The inode-seen set is **dedup, not containment** (and must never be sold as containment). The boundary is the grant + informed consent: an attacker who can write hardlinks into your granted folder can equally copy the bytes in. Optional hardening (documented fast-follow): skip-or-flag `st_nlink > 1` files loudly in the `IngestReport` (note: this would also skip *legitimate* within-grant hardlinks, which the inode-seen set otherwise ingests once — so it is a tradeoff, not a free win).
- **Never-touch is hazard-reduction, not a wall** — surfaced honestly; a forgotten secret in a granted folder *will* be ingested. **Matching is case-insensitive** (the primary platform, macOS/APFS, is case-insensitive — a case-sensitive filter would let `.SSH`/`ID_RSA`/`Server.PEM` bypass it).
- **M5a ingests text/markdown only** (rich formats wait for M5b) — acceptable: ships the pipeline + all safety + the user's notes; the sandbox lands deliberately, separately reviewed.

## 11. Review basis

Security + critic both **SHIP-WITH-FIXES** (2026-06-18), converging on: false reuse claims (supersede / `origin` / taint-walk — all relabeled **net-new** here), sandbox under-specification (→ **M5b**), and milestone sizing (→ **split**). Headline lesson (re-earned): **a spec claim about existing code MUST be verified by reading the code** — Rev 1 cited that lesson and still made three false-reuse claims; both reviewers caught all three. M5b (sandbox-heavy) gets its own spec + dedicated security review when M5a lands.
