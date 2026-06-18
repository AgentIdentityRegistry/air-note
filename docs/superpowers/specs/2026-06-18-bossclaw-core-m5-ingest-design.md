# bossclaw-core M5 (Ingest) — Design Spec

- **Date:** 2026-06-18
- **Status:** Approved (design) — awaiting spec review before the implementation plan
- **Parent:** `docs/superpowers/specs/2026-06-15-bossclaw-core-design.md` §12.5 (build sequence item 5) + §5.10/§5.11, §6, §7, §8.1/8.3/8.4/8.6
- **Builds on:** M1 (signed log) · M2 (recall: embed+FTS hybrid) · M3 (graph) · M4a/M4b (evolve/summarize) — all merged to `main`.

## 1. Goal

Read-only ingest of **user-granted folders** into the encrypted, signed event log as recallable
`file_ingested` events, with the full §12.5 safety Definition-of-Done: **fail-closed lineage-walked
taint root + read-side `O_NOFOLLOW`/fd-passing + sandboxed parser + canonicalize-then-contain +
never-touch filter + dedup + supersede**. Ingested file content becomes recallable (embed + FTS) and
is surfaced with file provenance. **Extraction of entities/links FROM file content is DEFERRED** (the
evolve loop stays `memory`-events-only, exactly as M4 left it).

## 2. Scope decisions (locked with Peter 2026-06-18)

| # | Decision | Choice |
|---|----------|--------|
| D1 | Scope | **The spec's M5**: pipeline + all safety + the **sandboxed markitdown parser** (§8.6) + recallable `file_ingested` + dedup/supersede. **Extraction from file content DEFERRED.** |
| D2 | Parser | A `Parser` **seam** (trait) — real markitdown impl feature-gated (mirrors M4a's `Reasoner`/`ollama`), mock for hermetic tests. Adapt the desktop's `apps/desktop/src-tauri/markitdown.rs` as reference. |
| D3 | Cross-platform | Ingest ON all 3 OSes. POSIX (macOS/Linux): `O_NOFOLLOW` + fd-passing (full TOCTOU-safe). Windows: canonicalize-then-contain + **reparse-point rejection** — blocks symlink/junction escape; **documented as the slightly-weaker Windows guarantee** on the fast swap-after-check race. |

## 3. New event types + projections

- **`grant` / `revoke`** events → a **`grants`** projection (`canonical_root`, `granted_at`, `revoked`). Public API: `add_grant(path)`, `revoke_grant(path)`, `grants()`.
- **`file_ingested`** event — `content.text` = the converted text (so Tier-A embeds + FTS-indexes it exactly like a `memory` event), plus provenance in `model_meta`/content: `canonical_path`, `content_hash` (sha256 of file bytes), `size_bytes`, `modified_at` (mtime, RFC 3339), `parser_id`, `grant_root`, and **`origin = "external"`** (the taint root). Signed + hash-chained via the existing serialized writer.
- **Resist widening `events`** (§7): `grants` is a projection of `grant`/`revoke`; no new authoritative table beyond what a projection needs.

## 4. Components (each a focused, independently-testable unit)

- **`ingest.rs`** (new module) — the orchestrator. Public: `ingest_grant(root) -> IngestReport`, `ingest_all() -> IngestReport`. `IngestReport { ingested, skipped, failed, superseded }` (counts + reasons, for observability §15).
- **The safe walk** (`ingest.rs`): per active grant root, iterate entries with **no symlink follow**; **canonicalize-then-contain** (each entry's real path ⊆ the grant root, else reject); apply the **never-touch deny-list**; enforce **size + type caps**; open via `O_NOFOLLOW` (POSIX) / reparse-reject (Windows) and **pass the fd** to the parser.
- **`Parser` seam** (trait, D2): `fn convert(&self, file: &ContainedFile, hint: &PathHint) -> Result<String, IngestError>`, where `ContainedFile` wraps the **already-opened, contained handle** (the `O_NOFOLLOW` fd on POSIX) so the child never re-resolves the path. Real markitdown impl runs the **restricted child** (§8.6); mock returns canned text for hermetic tests.
- **Tier-A integration** — add `file_ingested` to the indexed/recallable event-type set at `log.rs:120` (currently `[MEMORY_EVENT_TYPE, PAGE_EVENT_TYPE]`) so it embeds + FTS-indexes and is rebuildable byte-identically.
- **Recall integration** — `file_ingested` hits get `Hit.kind = "file_ingested"` + path provenance; **superseded versions excluded before `truncate(k)`** (M4b lesson).
- **Taint** — `origin = "external"`; the **evolve cursor stays `MEMORY_EVENT_TYPE`-only** (verified at `log.rs:2455/3249`) so file content is **never silently laundered** into the graph; the **fail-closed lineage-taint walk** lands as the root (refuses to derive trust from an `external`/unresolvable lineage id — extends the M3 F2 guard + the M4a "fail-closed on unresolvable lineage" rule).

## 5. Data flow (§6)

`add_grant(folder)` → `ingest_grant` → **walk** (no-symlink, contain, never-touch, caps) → **`O_NOFOLLOW` open + fd** → **sandboxed `Parser.convert`** → **dedup** (content_hash seen? skip) / **supersede** (same path, new hash → `append_pair` supersede+file_ingested, M4b) → **append signed `file_ingested`** → Tier-A derive (embed + FTS) → **recallable** with provenance.

## 6. Safety / §12.5 hard DoD (the heart of M5)

1. **Fence:** `file_ingested` text is **data, never instructions** wherever surfaced to a reasoner (§8.4 — raises the bar against direct injection; does NOT by itself stop confused-deputy proposals, which is M6's concern since M5 has no writes).
2. **Read-side containment:** no symlink follow; canonicalize-then-contain on **every** entry; POSIX `O_NOFOLLOW`+fd-passing (TOCTOU-safe), Windows reparse-reject + contain (D3).
3. **Never-touch deny-list** (conservative, content-aware): skip dot-dirs `.ssh .aws .gnupg .git .config/gh` and name/glob patterns `.env *.key *.pem id_rsa* *.keychain *.kdbx` (+ a documented, extensible list). Skipped paths never opened.
4. **Sandboxed parser (§8.6):** restricted child — **timeout, memory cap, output-size cap, no network**, archive depth+size budget. *Defaults (tunable in the plan):* 30 s/file timeout, ≤8 MB converted-output cap, archive depth ≤4 and expanded-total ≤100 MB. Breach → kill child, skip file, record `failed`.
5. **Fail-closed lineage taint:** `external` origin root; the taint walk fails **closed** on an unresolvable lineage id (never "external" silently becomes trusted).
6. **Dedup + supersede:** dedup on `content_hash`; supersede on **canonical path identity** (idempotency keyed on `(path, content_hash)` — structured, never prose, M4b lesson).
7. **Resource caps** *(defaults; tunable in the plan):* max single file ≤10 MB (larger skipped), ≤50 000 files per ingest run, ≤2 GB total byte budget — fail-closed (skip past caps, don't OOM).

## 7. Error handling

Per-file **best-effort**: a parse failure / oversize / never-touch / containment violation **skips that file and continues** the walk (recorded in `IngestReport` with a reason) — one bad file never aborts the whole ingest. **Safety violations fail closed** (an escape attempt rejects that entry; a sandbox breach kills the child). No partial `file_ingested` is ever appended (append only after a clean convert + dedup decision). Append uses the existing serialized writer (no new concurrency model).

## 8. Testing

- **Hermetic** (temp homes only, mock `Parser`) fixtures: symlink-escape ✗, `..`-traversal ✗, never-touch skipped, **dedup** (same bytes → no-op), **changed file → supersede** (recall returns only current), **recall an ingested file**, **superseded excluded**, oversize skipped, archive-budget enforced, **fail-closed taint** on unresolvable lineage.
- **Sandbox caps**: a mock/real parser that simulates runaway → timeout/output-cap kills it; recorded `failed`, walk continues.
- **Live markitdown gate** (feature-gated, like M4a's `#[ignore]` live-Ollama): a real PDF/docx/markdown converts end-to-end (grounded, recall-surfaced, supersede-on-change, dedup-on-repeat).
- **Tier-A**: `file_ingested` included in the byte-identical rebuild; `recall@k` fixture unaffected by adding the new kind.
- `clippy -D warnings` (default + parser feature), **zero `unsafe`** except the justified `O_NOFOLLOW` fd path (minimized + commented).

## 9. Deferred (explicitly NOT in M5)

- **Extraction of entities/links from file content** (evolve over `file_ingested`) — a later milestone; the taint root M5 builds is what makes it safe later.
- **Actuator / confirm-each writes** (M6, the v1 cut-line).
- **Content-scan secret detection** — v1 uses the path/name never-touch deny-list only (content-shaped-secret diff guard is an M6/write-side concern).
- Windows full-strength TOCTOU lock (the fd-passing equivalent) — deferred; v1 ships the documented weaker Windows guarantee (D3).

## 10. Risks

- **Parser sandbox is the largest attack surface** (attacker-controlled files into a Python stack) → mitigated by §8.6 caps + seam isolation + supply-chain audit (`pip-audit` in CI).
- **Windows weaker TOCTOU** (D3) — documented honestly, narrow theoretical gap.
- **Taint-root correctness** (fail-closed) — directly tested; this is the §12.5 DoD acceptance criterion.

## 11. Review basis

M5 is the **most security-sensitive milestone to date** (first attacker-controllable input). Matching the
M1–M4 rhythm, this spec should get an **independent security + critic second opinion before the
implementation plan** (M1 had 3 reviewers; M2–M4 a critic+security pass). The §12.5/§8.6 acceptance
criteria are the security review's checklist.
