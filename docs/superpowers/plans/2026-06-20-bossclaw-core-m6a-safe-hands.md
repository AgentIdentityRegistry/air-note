# M6a — "Safe Hands": implementation plan

- **Date:** 2026-06-20
- **Spec:** `docs/superpowers/specs/2026-06-20-bossclaw-core-m6a-safe-hands-design.md` (Rev 2, reviewed).
- **Crate:** `crates/bossclaw-core` (`#![forbid(unsafe_code)]`, rustix-only).
- **Method:** subagent-driven, TDD red-first. Each task: write the failing test(s) → minimal impl → green → per-task two-stage review (spec-compliance → code-quality). Then a whole-impl Opus SHIP review (cross-task probes + **revert-sensitivity** on every security test). No live-model gate (deterministic).
- **Branch:** `m6a-safe-hands`.

## Symbol-visibility decisions (the extraction lesson — verified)
- **In-crate unit tests** (`#[cfg(test)] mod` inside `src/*.rs`) see `pub(crate)` + private items → use for the atomic-write helper, the writable careful-open, `fold_write_grants`, `is_write_allowed` internals, gate-logic units.
- **Integration tests** (`tests/actuator.rs`, own binary, **public API only, no cross-binary imports**) → use for end-to-end propose→execute, TOCTOU/identity swaps (need real FS + the public methods), ingest-then-write (the L11 cite-around proof needs `ingest_file` + `propose_write`).
- The `tests/extraction.rs` harness preamble (`open_log`, `seed_memory`, `ingest_file`, `MockEmbedder`, `DEK`/`KEY_BYTES`) is **copied** into `tests/actuator.rs` (cannot be imported — verified: `extraction.rs:4-5`).
- Frozen vector goes in `tests/vectors.rs` (existing binary; pattern `tainted_page_canonicalization_is_frozen` `vectors.rs:147`).

## Global gates (every task)
`cargo test -p bossclaw-core` green · `cargo clippy -p bossclaw-core --all-targets -D warnings` clean **and** `--features ollama` clean · `#![forbid(unsafe_code)]` intact · `cargo-deny check` + `cargo audit` green (**no new dependency** — W5) · sole `INSERT INTO events` path unchanged (no second insert; `file_written` goes through `append`).

---

## T1 — Write-grants + `is_write_allowed` (the separate write authority)
**Builds:** L8 + §6 #1/#2 + §7.1.
**Files:** `src/graph.rs` (consts `WRITE_GRANT_EVENT_TYPE="write_grant"`, `WRITE_REVOKE_EVENT_TYPE="write_revoke"`; `WriteGrant` struct mirror of `Grant` `graph.rs:361`; `fold_write_grants` mirror of `fold_grants` `graph.rs:380`), `src/log.rs` (`write_grants` table in `open` `log.rs:150+`; `add_write_grant`/`revoke_write_grant`/`write_grants` mirror `add_grant` `log.rs:1818`; `is_write_allowed(path)`), `src/lib.rs` (re-export `WriteGrant`).
**`is_write_allowed` (greenfield — §6 #1):** canonicalize the candidate (**for a not-yet-existing Create target, canonicalize the PARENT** — `std::fs::canonicalize` fails closed on absent paths, `log.rs:1819`; seam-map M4), then require **path-segment descent** from an active (`!revoked`) write-granted `canonical_root`. Document it is **advisory** (string-segment, weaker than the read-side fd-walk) — the §9 fd-relative execute is the real boundary.
**Red-first tests (`tests/actuator.rs`):**
- add a write-grant on dir D → `is_write_allowed(D/f)` true; `is_write_allowed(D)` true; sibling outside D false.
- revoke → false. Re-grant → true (last-writer-wins fold).
- **read-grant-only dir → `is_write_allowed` false** (R5: a read grant never authorizes a write).
- create-target-absent: `is_write_allowed(D/new.txt)` true via parent canonicalization.
**Gate:** the global gates. **Verify** no shared fold/projection lets a `grant` event flip `write_grants` (read the two folds).

## T2 — Pure gate types + `propose_write`
**Builds:** §8 (all gate logic) + L10/L11.
**Files:** new `src/actuator.rs` (un-gated pure types: `WriteOp`, `WriteProposal`, `Taint`, `FileId`, `Provenance`, `DiffFlags`, `WriteVerdict`, `GatedProposal`; the pure helpers — diff-guard scan, the op×existence classifier), `src/log.rs` (`propose_write`), `src/lib.rs` (re-exports).
**Gate logic (spec §8):** (1) sources non-empty else reject; per-source `event_by_id` → **any unresolvable ⇒ whole proposal tainted** (L10/W7, over the set — not filter-then-judge); (2) canonicalize (parent for Create); `allowed = is_write_allowed`; (3) **op×existence matrix** reject (Create-of-existing; Edit/Delete-of-absent; final-component-symlink); (4) **engine-anchored taint (L11):** resolve via the files projection (`FileRecord`/`fold_files` `graph.rs:404/431`; a new point-lookup-by-canonical-path on `current_files_active` `log.rs:1898`) whether the target is a tracked ingested file → its `file_ingested` event is external by construction ⇒ `Untrusted`; union with (1); (5) base `content_hash` + `FileId` (dev/ino/size via `rustix::fs::fstat`) + display diff for Edit/Delete (via a no-symlink open); (6) `requires_loud_modal = Untrusted || Delete || diff_flags.any()` (monotonic).
**Red-first tests (`tests/actuator.rs`, revert-sensitive ones marked):**
- **L11 cite-around (REVERT-SENSITIVE):** `ingest_file` F under a read-grant (→ external `file_ingested`); add a write-grant over F's dir; `propose_write(Edit F, cites=[clean_memory_id])` → `taint==Untrusted`. *Must FAIL if the L11 anchor is removed.*
- **L10 fail-closed-over-set (REVERT-SENSITIVE):** cites `[clean_id, "NONEXISTENT"]` → `Untrusted`. *Must FAIL if changed to `filter_map(resolvable).any(external)`.*
- op×existence: Create-of-existing, Edit-of-absent, Delete-of-absent, symlink-final-component → each `reject_reason`.
- monotonic modal: a clean non-delete with a secret-shaped diff → loud; a clean non-delete plain edit → not loud; any delete → loud.
- provenance populated (origin_path/ingested_at for a file-derived source).
**Gate:** global. **In-crate unit tests** in `actuator.rs` for the pure classifiers (diff-guard, op×existence) where they're `pub(crate)`.

## T3 — Writable careful-open + atomic-write helper + actuator mutex
**Builds:** §9 steps 2/5 + L13.
**Files:** `src/actuator.rs` (`#[cfg(unix)]`): `open_dir_for_write(parent) -> OwnedFd` (new writable careful-open — `openat2 BENEATH|NO_SYMLINKS` Linux / `openat O_DIRECTORY|NOFOLLOW` macOS; mirrors `careful_open_file` `ingest.rs:310` which is RDONLY/`pub(crate)` and **cannot be reused**); `atomic_write(dir_fd, name, bytes) -> Result<()>` (named `O_CREAT|O_EXCL|O_NOFOLLOW` temp in the dir → write → `fsync` → finalize: **Linux** `renameat2(RENAME_NOREPLACE)` create / `renameat` edit; **macOS** fd-relative existence pre-check + `renameat`, residual documented); a per-actuator `rename_lock: Mutex<()>` on the `EventLog`/`Store`.
**Verified primitive notes:** `O_TMPFILE`/`renameat2` are **Linux-only**, absent from pinned `rustix` 0.38.44 — the macOS branch must NOT reference them (critic C1). Use `#[cfg(target_os="linux")]` / `#[cfg(not(target_os="linux"))]` splits like `careful_open_file` already does (`ingest.rs:318/347`).
**Red-first tests (in-crate `#[cfg(test)]` in `actuator.rs` — helpers are `pub(crate)`):**
- atomicity dir-scan: after `atomic_write`, dir has no `*.tmp`; target holds exactly the new bytes.
- create no-clobber: `atomic_write` create onto an existing name → error, original untouched.
- symlink-safety: a symlinked final component / symlinked parent → refused.
**Gate:** global (incl. clippy on both OS cfg branches — note CI compiles the macOS + Linux branches).

## T4 — `execute_write` + `file_written` + frozen vector
**Builds:** §9 (the critical section) + §7.2 + L9/L12.
**Files:** `src/graph.rs` (`FILE_WRITTEN_EVENT_TYPE="file_written"`; NOT in `EMBEDDABLE_EVENT_TYPES`), `src/log.rs` (`execute_write`), `tests/vectors.rs` (frozen vector).
**`execute_write` (critical section under `rename_lock`):** re-canonicalize + re-`is_write_allowed` (fail-closed); `open_dir_for_write`; **base guard** — `fstat` target fd → require `(dev,ino,size)==verdict.base_identity` AND re-hash==`base_content_hash` (Edit/Delete); durable undo capture (T5 wires the store; here capture-before-mutate ordering); mutate (`atomic_write` create/edit, fd-relative `unlinkat` delete — **hard-delete, no trash**, W5); **sole constructor** of `file_written` (Tier-B, `model_meta.model_id=producer`, `prompt_hash=""` per `link_machine` `log.rs:1571`, `source_event_ids = caller_cites ∪ engine target-provenance` L11), minted ULID == undo key.
**Red-first tests (`tests/actuator.rs`):**
- happy create/edit/delete gated → `file_written` appended, FS mutated.
- **TOCTOU path-swap:** pass the gate, then `symlink`/`rename` the target out-of-grant before execute → fail-closed.
- **same-content/different-inode swap (REVERT-SENSITIVE, L12):** replace target with a same-bytes different-inode file before execute → fail-closed. *Must FAIL if the `(dev,ino)` check is dropped.*
- base content-change between propose/execute → fail-closed.
- **grant revoked between propose and execute** → fail-closed.
- **no Tier-A `file_written`:** assert the public API cannot emit a `file_written` with `model_meta: None` (the sole-constructor invariant).
- **L11 end-to-end:** editing a tracked ingested file (citing only clean ids) → the recorded `file_written` is stamped `origin:"external"` (the augmented sources carry the `file_ingested` id).
- frozen vector `file_written_canonicalization_is_frozen` (independent JCS-hash; `prompt_hash=""`).
**Gate:** global + the frozen vector reproduced by hand once.

## T5 — N-deep undo store + `undo_write`
**Builds:** §7.3 + L3 + W8/W9.
**Files:** `src/log.rs` (`undo_state` table in `open`; the capture+GC in `execute_write`; `undo_write`), `src/graph.rs` if a projection helper is cleaner.
**Mechanics:** `undo_state(file_written_id PK, canonical_target, op, pre_bytes BLOB NULL, base_content_hash, created_at)` inside the SQLCipher store (encrypted automatically). **Ordering (W8):** mint the `file_written` id, write+commit the undo row keyed by it **before** the FS mutation is observable. **GC:** keep last `N=16` per `canonical_target`. **`undo_write(id)`:** rebuild a `WriteProposal` (restore pre-bytes / delete an undone create) → run the **full** propose→critical-section path against **current** grants + identity (W9), additionally verifying restored bytes hash to the recorded `prev_content_hash`; record a `file_written(undo_of=id, source=[id])`.
**Red-first tests (`tests/actuator.rs`):**
- N+1 sequential edits → oldest pre-bytes GC'd; the last N undo correctly in LIFO.
- undo of create removes the file; undo of delete recreates it from pre-bytes.
- **undo re-gates (REVERT-SENSITIVE):** revoke the write-grant after the original write → `undo_write` fail-closed; diverged target identity → fail-closed; tampered `pre_bytes` (hash ≠ recorded `prev_content_hash`) → fail-closed. *Must FAIL if undo skips the re-gate.*
- crash-ordering invariant: the undo row is durable before the mutation (documented assertion / capture-before-mutate unit check).
**Gate:** global.

## T6 — Security consolidation + final gates
**Builds:** §10 consolidation.
**Does:** ensure every revert-sensitive test (L10, L11, L12, undo re-gate) is **proven sensitive** — the plan's whole-impl review reverts each fix and confirms the test FAILS (the D8 lesson). Add the anti-fatigue structural assertion (the engine surface is single-target; `requires_loud_modal` monotonic). Confirm `content` is always a JSON object so the chokepoint stamp can apply (§6 M1). Run the full matrix.
**Gate:** full `cargo test -p bossclaw-core` + clippy default & ollama + `forbid(unsafe)` grep + `cargo-deny`/`audit` (no new dep) + sole-INSERT confirmation.

---

## Whole-impl review (before PR)
Opus, adversarial: cross-task integration probes (does T4's execute honor T2's verdict exactly? does T5's undo path re-run T1's `is_write_allowed`?); **revert-sensitivity audit** of all 4 security tests; confirm the §14 convergence fixes (D8-for-writes anchor, `(dev,ino)`, no-trash) are present in code, not just prose. SHIP only when 0 Critical/Important.

## PR
Resolve the base: if program-design **PR #33 merges** first, rebase `m6a-safe-hands` onto `main`; else stack the PR on `m6-actuator-program-spec`. CI green (macOS/Ubuntu/Windows — note the write path is `#[cfg(unix)]`; Windows compiles the un-gated types only). Then the GBrain handoff + protocol/roadmap/lessons updates.
