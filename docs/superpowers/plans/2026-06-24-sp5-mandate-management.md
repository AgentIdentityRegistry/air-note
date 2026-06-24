# Mandate Management (SP5) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Implement EXACTLY one `### Task N` at a time, commit, then move on. Every code block below is complete and grounded in the real code at `main` HEAD `d9a6e8c` — do not invent signatures.

**Goal:** A **mandate** is a standing, user-granted goal ("keep file `target` in sync with the sources under `source_scope`, following `recipe`"). After the user turns mandates on and grants one, the brain — on each ~5-min evolve tick — proposes a whole-file rewrite when `target` drifts from `recipe`+sources. The desktop app **auto-applies** a **clean** rewrite (every cited source authorized in-scope, content not secret-shaped) with no per-change confirm, and **parks a risky one** in the SP4 Review queue. Every auto-applied write is recorded in a persistent **Mandate-activity** list with **Undo**. The user can create / list / revoke mandates and flip a global **Mandates on/off** switch. **Off by default.**

**Architecture:** Four `bossclaw-core` engine changes land first — two security-critical: **(c)** a scoped taint-trust rule in `propose_write` (a mandate's own in-scope authorized sources stop tainting that mandate's target, making "clean" reachable) and **(d)** an engine-level loud-gate inside `execute_write_inner` (a loud write is refused without an explicit `acknowledged_loud`, threaded through all three callers); plus **(a)** surfacing the proposal's `producer` on `PendingProposal` and **(b)** a `mandate_writes()` attribution join. The `air_agent_desktop` Tauri crate then guards `prime_switches`, adds `set_mandates_enabled` + mandate-CRUD ops/commands/DTOs, fixes the Create-apply path, surfaces `producer`, adds the `mandate_writes` op, and adds the **auto-apply sweep** to `scheduler.rs`. The React UI adds a layout-agnostic **Mandates** destination (toggle, New-mandate form, active list, activity list + Undo) and labels risky mandate proposals "from mandate" in the reused SP4 Review queue.

**Tech Stack:** Rust (`bossclaw-core` engine, `air_agent_desktop` Tauri crate), TypeScript/React (desktop UI), vitest, cargo test. All new Rust + UI engine-calls are `#[cfg(unix)]`-gated. Engine clippy runs with `--features ollama -- -D warnings`.

---

## Spec/code discrepancies found while grounding (read before starting)

1. **Engine change (a) producer plumbing is PARTLY DONE.** The spec frames `producer` as new, but `append_write_proposal_with(..., producer)` and `decline_write_proposal_with(..., producer)` ALREADY exist, and the M6c phase in `evolve_once` ALREADY stamps `crate::graph::M6C_PROPOSER_PRODUCER` (log.rs ~6232-6242). So change (a) reduces to **surfacing** the existing `model_meta.model_id` on `PendingProposal` (Task 3). No proposer-side change is needed.
2. **The SP4 op-map already exists.** `apply_proposal` (engine/mod.rs:659-664) already maps `p.op` → `WriteOp` fail-closed. So Task 9 (Create-apply) is NOT "add op-mapping"; it is "reorder the op-map ABOVE the base-hash arm and skip the base-hash fail-closed arm for `op == "create"`", because `propose_write` sets `base_content_hash = None` for a Create (log.rs:3155-3156) and the current `None => Stale` arm (engine/mod.rs:649-652) wrongly rejects every Create.
3. **`MANDATE_AUTOAPPLY_PER_SWEEP` lives desktop-side** (in `scheduler.rs`, Task 11) — there is no engine const for it, and the sweep is an app-side action per Decision 3 / Approach A.
4. **The SP4 plan doc does not contain the Tauri ACL `__allow_command` test**; the real discipline is the committed test `engine_undo_apply_binds_camelcase_arg_over_ipc` (commands/engine.rs:479-551). Task 11's sweep tests are EngineHandle-level (no IPC), so they do NOT need `__allow_command`; only a command-LAYER IPC test would. Task 7 adds one command-layer IPC test for `engine_add_mandate` that DOES use the `__allow_command` discipline verbatim.
5. **Pinned engine-test harness names (`crates/bossclaw-core/tests/common/mod.rs`, verified).** The engine tests below use the REAL helpers — do NOT invent siblings:
   - `common::open_log_with_write_grant() -> (EventLog, TempDir, PathBuf)` — opens an onboarded log and grants BOTH read+write on a created `home/files` dir (the returned `PathBuf`). This is the base opener; for a scope/dest split, call `add_grant`/`add_write_grant` on additional tempdirs against the returned `log`.
   - `common::ingest_one(log: &EventLog, path: &Path) -> String` — **2 args, NO embedder** (it constructs its own `MockEmbedder::new(64)` internally). Earlier draft prose that called `ingest_one(&log, &emb, &path)` was wrong and is corrected throughout.
   - `common::seed_memory(log, text) -> String` — appends a `memory` event, returns its id (there is no `seed_one_memory`/`seed_one_memory_id` in the ENGINE harness).
   - `common::open_write_grant_and_external_target() -> (EventLog, TempDir, PathBuf)` — opens + ingests a tracked `n.md` target.
   - `common::append_minimal_proposal` / `common::append_rejected` — the SP4 proposal/rejection factories.
   - Mock arity: `bossclaw_core::embed::MockEmbedder::new(dim: usize)`. The DESKTOP tests use their OWN local helpers — `seed_one_memory_id(log, text) -> String` (engine/mod.rs:1052) and `bossclaw_ingest_one(log, path) -> String` (engine/mod.rs:1070) — NOT the engine `common::*`; the desktop op tests stay on those.
6. **`M6B_PROPOSER_PRODUCER = "m6b-reconciler"` is `pub` in graph.rs:91** (T11/T4 use it alongside `M6C_PROPOSER_PRODUCER` at graph.rs:98). Confirmed.
7. **The desktop crate has NO `log`/`tracing` dependency** (`apps/desktop/src-tauri/Cargo.toml` `[dependencies]` is `air-rs` + `tauri` + serde/tokio/etc.; the engine `bossclaw-core` is a `[target.'cfg(unix)'.dependencies]` dep with `features = ["ollama"]`, so `bossclaw_core::graph::*` resolves in the Unix-gated scheduler op). The only existing desktop diagnostic is `eprintln!` (vault.rs:65,90). So MF5's sweep observability uses **`eprintln!`** in the desktop scheduler — NOT `log::warn!` (which the spec/review referenced because the ENGINE crate has the `log` facade at log.rs:5896). Prescribing `log::warn!` in the desktop crate would require adding a dependency and break the build; `eprintln!` matches the real desktop convention.

---

## File structure

| File | Create/Modify | One responsibility |
|---|---|---|
| `crates/bossclaw-core/src/log.rs` | Modify | (c) Step-1 mandate taint-trust exception in `propose_write`; (d) `acknowledged_loud` threaded into `execute_write`/`execute_write_resolving`/`execute_write_inner` (+ `undo_write` exemption); (a) `producer` on `PendingProposal` + read in `pending_proposals`; (b) `MandateWriteRecord` + `mandate_writes()`. |
| `crates/bossclaw-core/src/lib.rs` | Modify | `#[cfg(unix)] pub use log::MandateWriteRecord;` (mirrors the gated `PendingProposal` re-export). |
| `crates/bossclaw-core/tests/reconcile.rs` | Modify | Engine tests for (c) trust rule (clean/out-of-scope/sibling/secret/post-revoke/unresolvable + M6b scoping proof), (d) loud-gate through each entry + undo-of-tainted, (a) producer surfaced, (b) `mandate_writes` attribution. |
| `apps/desktop/src-tauri/src/engine/mod.rs` | Modify | `prime_switches` mandate guard (+ flip the existing test); `set_mandates_enabled`/`add_mandate`/`revoke_mandate`/`list_mandates`/`mandate_writes` ops + `MandateSummary`/`MandateWriteSummary`; surface `producer` on `ProposalSummary`; Create-apply fix in `apply_proposal`; sweep helper + tests. |
| `apps/desktop/src-tauri/src/commands/engine.rs` | Modify | `MandateDto`/`MandateWriteDto`, `producer` on `ProposalDto`; the 5 new `#[tauri::command]`s + grant-rejection→typed-error mapping; a command-LAYER IPC arg-binding test for `engine_add_mandate` (the `__allow_command` discipline). |
| `apps/desktop/src-tauri/src/main.rs` | Modify | Register each new command with a per-element `#[cfg(unix)]` line in `generate_handler!`. |
| `apps/desktop/src-tauri/src/engine/scheduler.rs` | Modify | The auto-apply sweep (`MANDATE_AUTOAPPLY_PER_SWEEP`, oldest-first, producer-filtered, per-item `mandates_enabled` re-read, `apply(false)`, swallow risky/stale/revoked) + a pure `sweep_candidates` helper + sweep tests. |
| `apps/desktop/src/api/engine.ts` | Modify | TS twin types + `invoke<T>` wrappers for the 5 new commands; `producer` on `ProposalDto`. |
| `apps/desktop/src/mandates/mandateForm.ts` + `.test.ts` | Create | Pure New-mandate form validation. |
| `apps/desktop/src/mandates/mandateView.ts` + `.test.ts` | Create | Pure mandate-list + activity-list view mappers. |
| `apps/desktop/src/mandates/MandatesPanel.tsx` | Create | Mandates destination: toggle, New-mandate form, active list (Revoke), activity list (Undo). |
| `apps/desktop/src/App.tsx` | Modify | `View += "mandates"` + `MandatesNavButton` + body-ternary arm. |
| `apps/desktop/src/review/proposalView.ts` | Modify | Surface a "from mandate" label from the proposal's `producer`. |

---

### Task 1: (c) Mandate taint-trust rule in `propose_write` (SECURITY-CRITICAL)

**Files:**
- Modify: `crates/bossclaw-core/src/log.rs` (`propose_write` Step 1 + a new Step-1.5 escalation)
- Test: `crates/bossclaw-core/tests/reconcile.rs`

**What changes (grounded against the real `propose_write`, log.rs:3021-3193):** Today Step 1 (3045-3063) escalates `taint = Untrusted` the moment a cited source resolves to an `is_external` event. The rule: an external source must NOT escalate iff an active mandate authorizes it for THIS target. Ordering is load-bearing — Step 1 must record external candidates WITHOUT escalating; after Step 2 yields `Some(canonical_target)`, a new step escalates each candidate UNLESS authorized; an unresolvable target escalates ALL candidates (taint, never skip).

- [ ] Add the test-support fixture + the load-bearing tests in `crates/bossclaw-core/tests/reconcile.rs` (append at end of file). These use the REAL harness (grounded, discrepancy note 5): `common::open_log_with_write_grant() -> (EventLog, TempDir, PathBuf)` and the **2-arg** `common::ingest_one(log, path)` (it builds its own `MockEmbedder` — do NOT pass one).
  **CRITICAL grant invariant (verified against `add_mandate` log.rs:2807-2847):** a mandate TARGET must be under a WRITE grant AND **outside every active READ-grant root** (guard #4 rejects a read-granted target with `"mandate target must be outside every read-grant root"`). The harness's returned `files` dir is granted BOTH read AND write (`common/mod.rs:40-41`), so it **cannot** hold a mandate target. Therefore the fixture **ignores** the harness `files` dir, puts the mandate target in a FRESH **write-ONLY** tempdir (`add_write_grant`, NO `add_grant`), and keeps the read-granted SOURCE in a separate `scope` tempdir. It returns the `home`/`dest`/`scope` TempDirs so the caller keeps them alive:

```rust
// ── SP5 (c) trust-rule tests ────────────────────────────────────────────────
// Fixture: `dest` is a FRESH WRITE-ONLY-granted tempdir holding the mandate target (a mandate
// target MUST be outside every read root — add_mandate guard #4); a SEPARATE read-granted `scope`
// tempdir holds one ingested source. The mandate authorizes `scope` as the source for
// `dest/synced.md`. Returns (log, home, dest, scope, gated) — home/dest/scope kept alive by the
// caller; `gated` is the verdict for the rewrite citing the ingested source.
#[cfg(unix)]
fn trust_fixture(
    source_body: &[u8],
    new_body: &[u8],
) -> (bossclaw_core::EventLog, tempfile::TempDir, tempfile::TempDir, tempfile::TempDir, bossclaw_core::actuator::GatedProposal) {
    use bossclaw_core::actuator::{WriteOp, WriteProposal};
    // The harness gives an onboarded log; its `files` dir is read+write so we do NOT use it for a
    // mandate target — create a fresh WRITE-ONLY dest dir instead.
    let (log, home, _files) = common::open_log_with_write_grant();
    let dest = tempfile::tempdir().unwrap();
    log.add_write_grant(dest.path()).unwrap(); // write-ONLY (no add_grant) → valid mandate target root.
    let target = dest.path().join("synced.md");
    std::fs::write(&target, b"stale\n").unwrap();
    // A SEPARATE read-granted scope dir holds the source; ingest it so it is `external`.
    let scope = tempfile::tempdir().unwrap();
    log.add_grant(scope.path()).unwrap();
    let src_path = scope.path().join("src.md");
    std::fs::write(&src_path, source_body).unwrap();
    let src_id = common::ingest_one(&log, &src_path);
    // Grant the mandate: target in `dest` (write-only), sources under `scope` (read).
    log.add_mandate(&target, scope.path(), "sync the target from scope").unwrap();
    log.rebuild_graph().unwrap();
    let gated = log.propose_write(WriteProposal {
        target: target.clone(),
        new_content: new_body.to_vec(),
        op: WriteOp::Edit,
        source_event_ids: vec![src_id],
        rationale: "mandate sync".to_string(),
    }).unwrap();
    (log, home, dest, scope, gated)
}

#[cfg(unix)]
#[test]
fn mandate_in_scope_clean_source_is_not_loud() {
    let (_log, _home, _dest, _scope, gated) = trust_fixture(b"clean source text\n", b"clean new text\n");
    assert_eq!(gated.verdict.taint, bossclaw_core::actuator::Taint::Clean,
        "an in-scope authorized source must NOT taint the mandate's target");
    assert!(!gated.verdict.requires_loud_modal, "clean + in-scope ⇒ not loud (auto-appliable)");
}

#[cfg(unix)]
#[test]
fn mandate_secret_shaped_content_is_loud_even_in_scope() {
    // All-in-scope, but the NEW content is secret-shaped (>=32-char alnum run) → diff_flags → loud.
    let (_log, _home, _dest, _scope, gated) = trust_fixture(
        b"clean source\n",
        b"token=ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcd\n",
    );
    assert!(gated.verdict.requires_loud_modal,
        "secret-shaped content forces loud via diff_flags even when sources are in-scope");
}

#[cfg(unix)]
#[test]
fn mandate_out_of_scope_source_is_loud() {
    use bossclaw_core::actuator::{WriteOp, WriteProposal};
    let (log, _home, dest, scope_a, _gated) = trust_fixture(b"clean\n", b"new\n");
    // scope_a is the mandate's authorized scope (from the fixture); scope_b is a DIFFERENT
    // read-granted dir whose source is OUT of scope.
    let scope_b = tempfile::tempdir().unwrap();
    log.add_grant(scope_b.path()).unwrap();
    let outside = scope_b.path().join("evil.md");
    std::fs::write(&outside, b"out of scope source\n").unwrap();
    let outside_id = common::ingest_one(&log, &outside);
    let _ = &scope_a; // the in-scope dir is kept alive; the proposal cites the OUT-of-scope source.
    let target = dest.path().join("synced.md"); // `dest` is now a TempDir (write-only mandate root).
    let gated = log.propose_write(WriteProposal {
        target, new_content: b"new\n".to_vec(), op: WriteOp::Edit,
        source_event_ids: vec![outside_id], rationale: "x".to_string(),
    }).unwrap();
    assert_eq!(gated.verdict.taint, bossclaw_core::actuator::Taint::Untrusted,
        "an out-of-scope external source still taints");
    assert!(gated.verdict.requires_loud_modal, "out-of-scope ⇒ loud → queued, never auto-applied");
}

#[cfg(unix)]
#[test]
fn mandate_sibling_scope_prefix_is_loud_segment_aware() {
    // L1: a source under `<scope>-evil` (a sibling sharing a string prefix) must NOT be cleared —
    // containment is segment-aware, so `/p/scope-evil/x` is not "under" `/p/scope`.
    use bossclaw_core::actuator::{WriteOp, WriteProposal};
    let (log, _home, _files) = common::open_log_with_write_grant();
    // The mandate target lives in a FRESH WRITE-ONLY dir (outside every read root — guard #4).
    let dest = tempfile::tempdir().unwrap();
    log.add_write_grant(dest.path()).unwrap();
    let target = dest.path().join("synced.md");
    std::fs::write(&target, b"stale\n").unwrap();
    // Build sibling dirs `scope` and `scope-evil` under the SAME parent.
    let parent = tempfile::tempdir().unwrap();
    let scope = parent.path().join("scope");
    let evil = parent.path().join("scope-evil");
    std::fs::create_dir(&scope).unwrap();
    std::fs::create_dir(&evil).unwrap();
    log.add_grant(&scope).unwrap();
    log.add_grant(&evil).unwrap();
    let evil_src = evil.join("s.md");
    std::fs::write(&evil_src, b"sibling source\n").unwrap();
    let evil_id = common::ingest_one(&log, &evil_src);
    log.add_mandate(&target, &scope, "sync from scope only").unwrap();
    log.rebuild_graph().unwrap();
    let gated = log.propose_write(WriteProposal {
        target: target.clone(), new_content: b"new\n".to_vec(), op: WriteOp::Edit,
        source_event_ids: vec![evil_id], rationale: "x".to_string(),
    }).unwrap();
    assert_eq!(gated.verdict.taint, bossclaw_core::actuator::Taint::Untrusted,
        "a sibling `scope-evil` path must NOT be cleared (segment-aware containment)");
    assert!(gated.verdict.requires_loud_modal);
}

#[cfg(unix)]
#[test]
fn mandate_after_revoke_is_loud_again() {
    use bossclaw_core::actuator::{WriteOp, WriteProposal};
    let (log, _home, _dest, scope, gated_before) = trust_fixture(b"clean\n", b"new\n");
    assert_eq!(gated_before.verdict.taint, bossclaw_core::actuator::Taint::Clean, "clean while granted");
    // Revoke the only active mandate, then re-gate the SAME shape → taints again.
    let m = log.active_mandates().unwrap().into_iter().next().unwrap();
    log.revoke_mandate(&m.mandate_grant_id).unwrap();
    log.rebuild_graph().unwrap();
    // Fetch the already-ingested source id from the projection (ingest is idempotent; no re-ingest).
    let canonical_src = std::fs::canonicalize(scope.path().join("src.md")).unwrap()
        .to_string_lossy().to_string();
    let src_id = log.current_files().unwrap().into_iter()
        .find(|r| r.canonical_path == canonical_src).map(|r| r.file_event_id).unwrap();
    // Reuse the original target directly off the prior gated proposal (GatedProposal.proposal pub).
    let target = gated_before.proposal.target.clone();
    let gated_after = log.propose_write(WriteProposal {
        target, new_content: b"new\n".to_vec(), op: WriteOp::Edit,
        source_event_ids: vec![src_id], rationale: "x".to_string(),
    }).unwrap();
    assert_eq!(gated_after.verdict.taint, bossclaw_core::actuator::Taint::Untrusted,
        "after revoke there is no active mandate → the source re-taints");
    assert!(gated_after.verdict.requires_loud_modal);
}

#[cfg(unix)]
#[test]
fn mandate_unresolvable_target_taints_all_candidates() {
    // M2: if the target cannot be canonicalized, every external candidate must be escalated
    // (taint, never skip). We force unresolvability by proposing an EDIT to a path that does
    // not exist (Edit canonicalizes the target itself, which then fails → canonical == None).
    use bossclaw_core::actuator::{WriteOp, WriteProposal};
    let (log, _home, _files) = common::open_log_with_write_grant();
    // Mandate target + the propose target both live in a FRESH WRITE-ONLY dir (guard #4).
    let dest = tempfile::tempdir().unwrap();
    log.add_write_grant(dest.path()).unwrap();
    let scope = tempfile::tempdir().unwrap();
    log.add_grant(scope.path()).unwrap();
    let src = scope.path().join("s.md");
    std::fs::write(&src, b"clean\n").unwrap();
    let src_id = common::ingest_one(&log, &src);
    // A mandate whose target is a real (write-granted, read-free) file, so add_mandate's guards pass…
    let real_target = dest.path().join("real.md");
    std::fs::write(&real_target, b"x\n").unwrap();
    log.add_mandate(&real_target, scope.path(), "r").unwrap();
    log.rebuild_graph().unwrap();
    // …but PROPOSE against a NON-EXISTENT target path (Edit ⇒ canonicalize fails ⇒ None).
    let missing = dest.path().join("does-not-exist.md");
    let gated = log.propose_write(WriteProposal {
        target: missing, new_content: b"new\n".to_vec(), op: WriteOp::Edit,
        source_event_ids: vec![src_id], rationale: "x".to_string(),
    }).unwrap();
    assert!(gated.verdict.reject_reason.is_some(), "an unresolvable Edit target is a reject_reason");
    assert_eq!(gated.verdict.taint, bossclaw_core::actuator::Taint::Untrusted,
        "an unresolvable target escalates ALL external candidates (taint, never skip)");
}

#[cfg(unix)]
#[test]
fn m6b_reconcile_target_stays_loud_trust_rule_did_not_leak() {
    // The SCOPING PROOF: an M6b-style proposal whose TARGET is an INGESTED file (inside a read
    // root) must still be Untrusted/loud — the trust rule keys on `m.target == proposal target`,
    // and a mandate target is OUTSIDE every read root (the add_mandate self-loop guard), so the
    // two sets are disjoint and the rule can never clear an ingested-target write.
    use bossclaw_core::actuator::{WriteOp, WriteProposal};
    // The harness `dir` is read+write-granted; an ingested file under it is a valid M6b target.
    let (log, _home, dir) = common::open_log_with_write_grant();
    let scope = tempfile::tempdir().unwrap();
    log.add_grant(scope.path()).unwrap();
    let src = scope.path().join("s.md");
    std::fs::write(&src, b"clean\n").unwrap();
    let src_id = common::ingest_one(&log, &src);
    // The TARGET is itself an ingested file under a read root.
    let ingested_target = dir.join("note.md");
    std::fs::write(&ingested_target, b"old\n").unwrap();
    let _tgt_id = common::ingest_one(&log, &ingested_target);
    // A mandate exists for some OTHER target (so active_mandates is non-empty), proving the rule
    // is consulted yet still does not clear THIS ingested-target write.
    let other = tempfile::tempdir().unwrap();
    log.add_write_grant(other.path()).unwrap();
    let other_target = other.path().join("synced.md");
    std::fs::write(&other_target, b"x\n").unwrap();
    log.add_mandate(&other_target, scope.path(), "r").unwrap();
    log.rebuild_graph().unwrap();
    // Propose a rewrite of the INGESTED target citing the clean source.
    let gated = log.propose_write(WriteProposal {
        target: ingested_target.clone(), new_content: b"new\n".to_vec(), op: WriteOp::Edit,
        source_event_ids: vec![src_id], rationale: "reconcile".to_string(),
    }).unwrap();
    assert_eq!(gated.verdict.taint, bossclaw_core::actuator::Taint::Untrusted,
        "Step-4 engine-anchored taint keeps an ingested-TARGET write loud (rule did not leak)");
    assert!(gated.verdict.requires_loud_modal);
}
```

  Grounding note (discrepancy 5): these tests call the REAL `common::open_log_with_write_grant()` (returns `(EventLog, TempDir, PathBuf)`, read+write-granted `dest`) and the 2-arg `common::ingest_one(log, path)`. There is NO `open_log_for_dir` and NO 3-arg `ingest_one` — do not add either. `GatedProposal.proposal` is `pub` (actuator.rs:167-173), so `gated_before.proposal.target.clone()` reads the target directly (the broken `_dest_path` helper and the dead `let emb` are gone — MF1).

- [ ] Run them (expect FAIL — the trust rule does not exist, so an in-scope source still taints and `mandate_in_scope_clean_source_is_not_loud` fails on `taint == Clean`):
  `cargo test -p bossclaw-core --test reconcile mandate_`
  Expected output: `mandate_in_scope_clean_source_is_not_loud` fails its `assert_eq!(taint, Clean)` (left `Untrusted`, right `Clean`); the others that assert loud may already pass (they assert the CURRENT always-taint behavior), which is fine — the new ones pin the post-change behavior.

- [ ] Implement the trust rule in `crates/bossclaw-core/src/log.rs` `propose_write`. Two edits:

  **Edit 1 — Step 1 records external candidates WITHOUT escalating** (replace the current Step-1 `for src in &p.source_event_ids` loop body at log.rs:3047-3063). The new loop pushes provenance and collects `(event_id, canonical_path)` for each external source into a `candidates` vec, escalating taint only for an UNRESOLVABLE id (which is target-independent, always fail-closed):

```rust
        // Candidate external sources gathered in Step 1 but NOT escalated yet (SP5 c): a
        // mandate may authorize them against THIS target, which we can only test after Step 2
        // resolves the canonical target. Each is `(event_id, ingested canonical_path)`.
        let mut external_candidates: Vec<(String, Option<String>)> = Vec::new();
        if p.source_event_ids.is_empty() {
            reject_reason.get_or_insert_with(|| "source_event_ids is empty".to_string());
        } else {
            for src in &p.source_event_ids {
                match self.event_by_id(src)? {
                    Some(ev) => {
                        let prov = Self::provenance_from_event(&ev);
                        if prov.is_external {
                            // DEFER escalation: record the candidate + its ingested path. The
                            // `origin_path` on the provenance is the source's canonical path
                            // (from the files projection), the exact stored form we compare
                            // against `m.source_scope` — never re-canonicalize a live path (M2).
                            external_candidates.push((prov.event_id.clone(), prov.origin_path.clone()));
                        }
                        provenance.push(prov);
                    }
                    None => {
                        // Unresolvable cited source ⇒ fail closed over the set (target-independent).
                        taint = Taint::Untrusted;
                    }
                }
            }
        }
```

  **Edit 2 — a new Step-1.5 that escalates the deferred candidates unless authorized**, inserted IMMEDIATELY AFTER Step 2 computes `canonical` and `allowed` (after log.rs:3090, before Step 3's `symlink_metadata` probe at 3098). Read `active_mandates()` ONCE; compare STORED canonical forms segment-aware:

```rust
        // ── Step 1.5: escalate deferred external candidates unless a mandate authorizes them ──
        // (SP5 change c, SECURITY-CRITICAL.) An external cited source does NOT taint iff some
        // ACTIVE mandate `m` has `m.target == canonical_target` AND the source's ingested
        // canonical_path is segment-aware UNDER `m.source_scope`. FAIL-CLOSED ORDERING (M2/L1):
        //   • if the target is unresolvable (`canonical == None`) → escalate EVERY candidate;
        //   • else escalate each candidate UNLESS authorized.
        // `active_mandates()` is read ONCE here, inside this gate evaluation (an in-flight revoke
        // is caught by the apply-time re-gate). Both sides of the containment test are STORED
        // canonical forms (scope canonical-at-grant; source canonical-from-projection) compared
        // with segment-aware `Path::starts_with` — never re-canonicalize a live (symlinkable) path.
        if !external_candidates.is_empty() {
            match &canonical {
                None => {
                    // Unresolvable target ⇒ cannot authorize anything ⇒ taint ALL (never skip).
                    taint = Taint::Untrusted;
                }
                Some(canonical_target) => {
                    let canonical_target_str = canonical_target.to_string_lossy().to_string();
                    let mandates = self.active_mandates()?;
                    for (_src_id, src_canonical) in &external_candidates {
                        let authorized = match src_canonical {
                            // A candidate with no recorded ingested path cannot be proven
                            // in-scope → fail closed (taint).
                            None => false,
                            Some(src_path) => mandates.iter().any(|m| {
                                m.target == canonical_target_str
                                    && std::path::Path::new(src_path)
                                        .starts_with(std::path::Path::new(&m.source_scope))
                            }),
                        };
                        if !authorized {
                            taint = Taint::Untrusted;
                        }
                    }
                }
            }
        }
```

  Nothing else in `propose_write` changes: Step 4 (engine-anchored target taint, 3112-3146) still unions in, Step 6's `requires_loud_modal` rule (3177-3179) is untouched, and `base_content_hash` capture is untouched.

- [ ] Run them (expect PASS): `cargo test -p bossclaw-core --test reconcile mandate_`
  Expected output: `test result: ok.` with all `mandate_*` tests passing — `mandate_in_scope_clean_source_is_not_loud` now sees `taint == Clean`; the out-of-scope / sibling / secret-shaped / post-revoke / unresolvable / M6b-scoping tests all stay loud/tainted.

- [ ] Run the full propose/reconcile suites to confirm no regression (the SP4 reconcile + the existing propose tests must stay green):
  `cargo test -p bossclaw-core --test reconcile`
  Expected output: `test result: ok.` with all tests passing.

- [ ] Commit:
  `git add crates/bossclaw-core/src/log.rs crates/bossclaw-core/tests/reconcile.rs crates/bossclaw-core/tests/common/mod.rs`
  `git commit -m "$(cat <<'EOF'
feat(bossclaw-core): mandate-authorized in-scope sources don't taint that mandate's target

SP5 engine change (c). propose_write defers external-source escalation past target
canonicalization, then escalates each external candidate unless an active mandate
authorizes it (m.target == canonical target AND source canonical_path segment-aware
under m.source_scope). Unresolvable target taints all; sibling/out-of-scope/secret-shaped
stay loud; M6b ingested-target writes stay loud (rule provably can't leak).

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"`

---

### Task 2: (d) Engine-enforced loud-gate in `execute_write_inner` + all 3 callers (SECURITY-CRITICAL)

**Files:**
- Modify: `crates/bossclaw-core/src/log.rs` (`execute_write_inner` + `execute_write` + `execute_write_resolving` + `undo_write`)
- Modify: `apps/desktop/src-tauri/src/engine/mod.rs` (the `apply_proposal` call site that breaks when `execute_write_resolving`'s signature changes)
- Test: `crates/bossclaw-core/tests/reconcile.rs`

**What changes (grounded against the real code):** `execute_write_inner` (log.rs:3261-3702) currently has the Step-1a verdict checks at 3281-3286 (`reject_reason` then `!allowed`). Add an `acknowledged_loud: bool` parameter and, immediately AFTER those two checks, fail closed if `verdict.requires_loud_modal && !acknowledged_loud`. Thread the flag through all three callers:
- `execute_write` (log.rs:3230-3237) — the public entry; gains an `acknowledged_loud` param, passed straight through.
- `execute_write_resolving` (log.rs:3245-3251) — gains an `acknowledged_loud` param (the desktop `apply_proposal` passes the user's value; the sweep in Task 11 passes `false`).
- `undo_write` (log.rs:3915) — passes `true` as a DELIBERATE COMMENTED exemption (an undo is a hash-verified inverse of an already-approved write).

**Caller audit for `execute_write`:** `grep -n '\.execute_write(' crates/bossclaw-core/` shows the only callers are in-crate tests (clean proposals). So adding an explicit `acknowledged_loud` param to the public `execute_write` is safe — update each test call to pass `false` (clean proposals don't need the ack). Run the grep in the step below and update whatever it finds.

- [ ] Write the failing tests in `crates/bossclaw-core/tests/reconcile.rs` (append after the Task 1 tests). They drive a loud proposal through each public entry without the ack (must fail closed), permit with `true`, and prove an undo of a tainted-file write still succeeds. Reuse `trust_fixture` is not enough (it returns Clean); build a loud proposal directly (secret-shaped content forces loud regardless of taint):

```rust
// ── SP5 (d) engine loud-gate tests ──────────────────────────────────────────
/// Build a gated LOUD proposal (secret-shaped content ⇒ requires_loud_modal) against a real
/// write-granted, ingested file. Returns (log, home-keepalive, dir-keepalive, gated, target).
#[cfg(unix)]
fn loud_gated(
) -> (bossclaw_core::EventLog, tempfile::TempDir, std::path::PathBuf, bossclaw_core::actuator::GatedProposal, std::path::PathBuf) {
    use bossclaw_core::actuator::{WriteOp, WriteProposal};
    // `dir` is read+write-granted; `home` is the keepalive TempDir.
    let (log, home, dir) = common::open_log_with_write_grant();
    let target = dir.join("note.md");
    std::fs::write(&target, b"placeholder\n").unwrap();
    let id = common::ingest_one(&log, &target); // ingested ⇒ external ⇒ Untrusted too
    let gated = log.propose_write(WriteProposal {
        target: target.clone(),
        new_content: b"token=ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcd\n".to_vec(),
        op: WriteOp::Edit,
        source_event_ids: vec![id],
        rationale: "loud".to_string(),
    }).unwrap();
    assert!(gated.verdict.requires_loud_modal, "secret-shaped + ingested ⇒ loud");
    (log, home, dir, gated, target)
}

#[cfg(unix)]
#[test]
fn execute_write_loud_without_ack_fails_closed_then_permits_with_ack() {
    let (log, _home, _dir, gated, target) = loud_gated();
    let original = std::fs::read(&target).unwrap();
    // Public entry without ack → fail closed, file untouched.
    let err = log.execute_write(gated.clone(), false).unwrap_err();
    assert!(err.to_string().contains("loud write requires acknowledged_loud"),
        "a loud write without the ack must fail closed: {err}");
    assert_eq!(std::fs::read(&target).unwrap(), original, "no write happened without the ack");
    // With ack → it writes.
    let fw = log.execute_write(gated, true).unwrap();
    assert!(!fw.is_empty(), "the ack lets the loud write through");
    assert_ne!(std::fs::read(&target).unwrap(), original, "the file changed after the acked write");
}

#[cfg(unix)]
#[test]
fn execute_write_resolving_loud_without_ack_fails_closed() {
    use bossclaw_core::actuator::{WriteOp, WriteProposal};
    let (log, _home, dir) = common::open_log_with_write_grant();
    let target = dir.join("note.md");
    std::fs::write(&target, b"placeholder\n").unwrap();
    let fid = common::ingest_one(&log, &target);
    // Record an open proposal so execute_write_resolving has an id to resolve.
    let new_bytes = b"token=ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcd\n".to_vec();
    let hash = { use sha2::{Digest, Sha256}; hex::encode(Sha256::digest(&new_bytes)) };
    let canonical = std::fs::canonicalize(&target).unwrap().to_string_lossy().to_string();
    let key = serde_json::json!({"src":"a","relation":"r","dst":"b"});
    let gated = log.propose_write(WriteProposal { target: target.clone(), new_content: new_bytes.clone(),
        op: WriteOp::Edit, source_event_ids: vec![fid.clone()], rationale: "loud".to_string() }).unwrap();
    let vs = serde_json::json!({"requires_loud_modal": gated.verdict.requires_loud_modal,
        "taint": format!("{:?}", gated.verdict.taint), "allowed": gated.verdict.allowed,
        "base_content_hash": gated.verdict.base_content_hash});
    let pid = log.append_write_proposal(&canonical, "edit", &hash, new_bytes.len() as u64, "loud",
        &key, &vs, std::slice::from_ref(&fid)).unwrap();
    log.put_proposal_bytes(&pid, &new_bytes, &hash).unwrap();
    let original = std::fs::read(&target).unwrap();
    // ack=false → fail closed; the proposal stays open + the file is untouched.
    let err = log.execute_write_resolving(gated, &pid, false).unwrap_err();
    assert!(err.to_string().contains("loud write requires acknowledged_loud"), "fail closed: {err}");
    assert_eq!(std::fs::read(&target).unwrap(), original, "no write without the ack");
}

#[cfg(unix)]
#[test]
fn undo_of_a_tainted_write_succeeds_via_the_exemption() {
    // The undo exemption (H1): an undo cites the original file_written (external ⇒ re-gate is
    // loud), so WITHOUT the `acknowledged_loud = true` exemption inside undo_write, every undo of
    // a tainted-file write would fail closed. Assert it succeeds.
    let (log, _home, _dir, gated, target) = loud_gated();
    let original = std::fs::read(&target).unwrap();
    let fw = log.execute_write(gated, true).unwrap(); // apply the loud write (acked)
    let changed = std::fs::read(&target).unwrap();
    assert_ne!(changed, original, "applied");
    // Undo must restore the original even though the re-gate of the inverse is loud.
    log.undo_write(&fw).unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), original, "undo restored the pre-write bytes");
}
```

- [ ] Run them (expect FAIL — `execute_write`/`execute_write_resolving` take 1 arg today, so the calls don't compile):
  `cargo test -p bossclaw-core --test reconcile loud_ execute_write_loud undo_of_a_tainted`
  Expected output: compile error `this function takes 1 argument but 2 arguments were supplied` (or `execute_write_resolving takes 2 arguments but 3 were supplied`).

- [ ] Implement the loud-gate in `crates/bossclaw-core/src/log.rs`.

  **`execute_write_inner`** (log.rs:3261): add the param to the signature:

```rust
    fn execute_write_inner(
        &self,
        confirmed: crate::actuator::GatedProposal,
        undo_of: Option<&str>,
        resolves_proposal: Option<&str>,
        acknowledged_loud: bool,
    ) -> Result<String, BossclawError> {
```

  And insert the gate IMMEDIATELY AFTER the Step-1a `!verdict.allowed` check (after log.rs:3286, before the `rename_lock` acquire at 3289):

```rust
        // ── Step 1a.5: ENGINE-ENFORCED loud-gate (SP5 change d, SECURITY-CRITICAL) ─
        // A loud write (Untrusted ∪ Delete ∪ secret/value-shaped) is refused unless the caller
        // passed `acknowledged_loud == true`. This makes "a loud write needs an explicit ack" an
        // engine INVARIANT for every caller — desktop apply (threads the user's value), the
        // autonomous sweep (passes false ⇒ a loud mandate write can never auto-apply), and any
        // future caller. The ONLY sanctioned ack-without-UI path is `undo_write` (a hash-verified
        // inverse of an already-approved write), which passes true with a documented exemption.
        if verdict.requires_loud_modal && !acknowledged_loud {
            return Err(reject("loud write requires acknowledged_loud (refused fail-closed)"));
        }
```

  **`execute_write`** (log.rs:3230): add the param + thread it:

```rust
    pub fn execute_write(
        &self,
        confirmed: crate::actuator::GatedProposal,
        acknowledged_loud: bool,
    ) -> Result<String, BossclawError> {
        // The public entry: a normal (non-undo) write carries no `undo_of` and resolves no
        // proposal. The caller's loud acknowledgement is threaded to the engine loud-gate.
        self.execute_write_inner(confirmed, None, None, acknowledged_loud)
    }
```

  **`execute_write_resolving`** (log.rs:3245): add the param + thread it:

```rust
    pub fn execute_write_resolving(
        &self,
        confirmed: crate::actuator::GatedProposal,
        resolves_proposal: &str,
        acknowledged_loud: bool,
    ) -> Result<String, BossclawError> {
        self.execute_write_inner(confirmed, None, Some(resolves_proposal), acknowledged_loud)
    }
```

  **`undo_write`** (log.rs:3915, the `execute_write_inner(gated, Some(file_written_id), None)` call): pass the exemption:

```rust
        // An undo records NO frame and resolves NO M6b proposal (it carries `undo_of`, not
        // `resolves_proposal`). It passes `acknowledged_loud = true` as the SOLE sanctioned
        // ack-without-UI exemption (SP5 change d): an undo is a hash-verified inverse-restore of
        // `pre_bytes` already validated against the recorded base_content_hash — the inverse of an
        // already-approved write, never fresh untrusted content. Its re-gate is loud (the inverse
        // cites the original external file_written), so without this exemption every undo of a
        // tainted-file write would fail closed.
        let undo_event_id = self.execute_write_inner(gated, Some(file_written_id), None, true)?;
```

- [ ] Audit + fix the in-crate `execute_write` callers (clean proposals → pass `false`):
  `grep -rn '\.execute_write(' crates/bossclaw-core/`
  For EACH match (they are all `#[cfg(test)]` round-trip tests with clean proposals), add `, false` as the second argument. Likewise `grep -rn '\.execute_write_resolving(' crates/bossclaw-core/` and add `, false` (clean) where the existing call passes only `(gated, &id)`. There should be no live non-test production caller of either (the desktop `apply_proposal` is the production `execute_write_resolving` caller, fixed in the next step).

- [ ] Fix the desktop call site that the `execute_write_resolving` signature change broke. In `apps/desktop/src-tauri/src/engine/mod.rs` `apply_proposal` (the `execute_write_resolving(gated, &p.id)` call at line 693), pass the caller's ack. The op already evaluated the loud-confirm above (engine/mod.rs:686-690) and only reaches here when `acknowledged_loud || !requires_loud_modal`, so threading `acknowledged_loud` is exactly correct (it is `true` only when the user acked; the engine gate then permits):

```rust
            // execute is atomic temp+rename. Thread the caller's ack to the ENGINE loud-gate
            // (SP5 change d): this op already refused above unless acked-or-not-loud, so the
            // engine gate sees a consistent value (defense-in-depth — the same check now lives
            // in execute_write_inner for every caller).
            let fw_id = log.execute_write_resolving(gated, &p.id, acknowledged_loud)
                .map_err(|e| EngineOpError::Core(e.to_string()))?;
```

- [ ] Run the engine tests (expect PASS): `cargo test -p bossclaw-core --test reconcile loud_ execute_write_loud undo_of_a_tainted`
  Expected output: `test result: ok.` — loud-without-ack fails closed through each entry; with-ack permits; undo of the tainted write restores the original.

- [ ] Confirm the workspace still compiles end-to-end (the desktop call-site fix must keep it green) and the existing engine round-trip tests still pass with their new `, false` args:
  `cargo test -p bossclaw-core && cargo build -p air_agent_desktop`
  Expected output: both `Finished` / `test result: ok.` with 0 failed.

- [ ] Commit:
  `git add crates/bossclaw-core/src/log.rs crates/bossclaw-core/tests/reconcile.rs apps/desktop/src-tauri/src/engine/mod.rs`
  `git commit -m "$(cat <<'EOF'
feat(bossclaw-core): enforce the loud-gate inside execute_write_inner for every caller

SP5 engine change (d). execute_write_inner gains acknowledged_loud and fails closed on a
requires_loud_modal write without it. Threaded through execute_write (public),
execute_write_resolving (desktop apply passes the user's value; the sweep passes false),
and undo_write (passes true as the sole sanctioned ack-without-UI exemption — a hash-verified
inverse of an already-approved write). Desktop apply_proposal call site updated.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"`

---

### Task 3: (a) Surface the proposal's `producer` on `PendingProposal`

**Files:**
- Modify: `crates/bossclaw-core/src/log.rs` (`PendingProposal` struct + `pending_proposals` fold)
- Test: `crates/bossclaw-core/tests/reconcile.rs`

**What changes (grounded):** `PendingProposal` (log.rs:363-404) has no `producer` field; the proposer side ALREADY stamps it (`append_write_proposal_with(..., M6C_PROPOSER_PRODUCER)` for M6c, `M6B_PROPOSER_PRODUCER` for M6b). Add `producer: String` read from `model_meta.model_id` (the same `model_meta` the fold already reads `source_event_ids` from). Empty/unknown when `model_meta` is absent.

- [ ] Write the failing test in `crates/bossclaw-core/tests/reconcile.rs` (append after the Task 2 tests). It appends one M6c-stamped proposal and one M6b-stamped proposal via the existing `*_with` API and asserts each row's `producer`:

```rust
#[cfg(unix)]
#[test]
fn pending_proposals_surface_producer_for_m6c_vs_m6b() {
    let (log, _home, dir) = common::open_write_grant_and_external_target();
    let path = dir.join("n.md");
    let canonical = std::fs::canonicalize(&path).unwrap().to_string_lossy().to_string();
    let lineage = common::seed_memory(&log, "Alice works at Acme"); // returns the memory id
    let key_a = serde_json::json!({"src":"entity:a","relation":"r","dst":"entity:b"});
    let key_b = serde_json::json!({"src":"entity:c","relation":"r","dst":"entity:d"});
    let vs = serde_json::json!({"requires_loud_modal": false, "taint": "Clean", "allowed": true});

    let m6c = log.append_write_proposal_with(&canonical, "edit", "deadbeef", 0, "why-c",
        &key_a, &vs, std::slice::from_ref(&lineage), bossclaw_core::graph::M6C_PROPOSER_PRODUCER).unwrap();
    let m6b = log.append_write_proposal_with(&canonical, "edit", "feedface", 0, "why-b",
        &key_b, &vs, std::slice::from_ref(&lineage), bossclaw_core::graph::M6B_PROPOSER_PRODUCER).unwrap();

    let pending = log.pending_proposals().unwrap();
    let by_id = |id: &str| pending.iter().find(|p| p.id == id).unwrap().clone();
    assert_eq!(by_id(&m6c).producer, "m6c-mandate-proposer", "M6c producer surfaced");
    assert_eq!(by_id(&m6b).producer, "m6b-reconciler", "M6b producer surfaced");
}
```

  Grounding note (discrepancy 5/6): this uses the REAL `common::open_write_grant_and_external_target() -> (EventLog, TempDir, PathBuf)` and `common::seed_memory(log, text) -> String` (returns the memory id — there is NO `seed_one_memory`/`seed_one_memory_id` in the engine harness, so do not reference one). `append_write_proposal_with` already exists and `M6C_PROPOSER_PRODUCER` (graph.rs:98) / `M6B_PROPOSER_PRODUCER` (graph.rs:91) are `pub`.

- [ ] Run it (expect FAIL — `producer` field does not exist on `PendingProposal`):
  `cargo test -p bossclaw-core --test reconcile pending_proposals_surface_producer_for_m6c_vs_m6b`
  Expected output: compile error `no field 'producer' on type '&bossclaw_core::PendingProposal'`.

- [ ] Implement in `crates/bossclaw-core/src/log.rs`.

  Add the field to `PendingProposal` (after `source_event_ids`, log.rs:383):

```rust
    /// The proposer's producer stamp (`model_meta.model_id`): `"m6b-reconciler"` for an M6b
    /// reconcile proposal, `"m6c-mandate-proposer"` for an M6c mandate proposal; empty when
    /// `model_meta` is absent. The desktop sweep auto-applies iff this is exactly the M6c stamp.
    pub producer: String,
```

  In `pending_proposals` (the `WRITE_PROPOSAL_EVENT_TYPE` arm, after the `source_event_ids` read at log.rs:2371), read the producer and add it to the struct literal:

```rust
                    let producer = ev.model_meta.as_ref()
                        .map(|m| m.model_id.clone()).unwrap_or_default();
```

  And update the `open.push(PendingProposal { ... })` literal to include `producer` (the field order is free; add it next to `source_event_ids`):

```rust
                    open.push(PendingProposal {
                        id, target, op, new_content_hash, rationale,
                        inducing_key, source_event_ids, producer, base_content_hash, verdict_summary,
                    });
```

- [ ] Run it (expect PASS): `cargo test -p bossclaw-core --test reconcile pending_proposals_surface_producer_for_m6c_vs_m6b`
  Expected output: `test result: ok. 1 passed`.

- [ ] Confirm the existing SP4 `pending_proposals_*` test still compiles + passes (the struct literal gained a field; the SP4 test only reads fields, so it is unaffected, but the `PendingProposal { ... }` construction in `pending_proposals` is the single build site):
  `cargo test -p bossclaw-core --test reconcile pending_proposals`
  Expected output: `test result: ok.` with both `pending_proposals_*` tests passing.

- [ ] Commit:
  `git add crates/bossclaw-core/src/log.rs crates/bossclaw-core/tests/reconcile.rs`
  `git commit -m "$(cat <<'EOF'
feat(bossclaw-core): surface the proposal producer on PendingProposal

SP5 engine change (a). PendingProposal gains producer (model_meta.model_id), read in the
pending_proposals fold. The proposer side already stamps M6B/M6C_PROPOSER_PRODUCER; this only
surfaces it so the desktop sweep can filter to m6c-mandate-proposer proposals.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"`

---

### Task 4: (b) `mandate_writes()` attribution join + `MandateWriteRecord`

**Files:**
- Modify: `crates/bossclaw-core/src/log.rs` (`MandateWriteRecord` + `mandate_writes()`)
- Modify: `crates/bossclaw-core/src/lib.rs` (re-export `MandateWriteRecord`)
- Test: `crates/bossclaw-core/tests/reconcile.rs`

**What changes (grounded):** Applied writes are stamped `model_meta.model_id = ACTUATOR_PRODUCER` ("m6a-actuator", graph.rs:78) regardless of proposer; the only discriminator on a `file_written` is `content.resolves_proposal` → the proposal id, and resolved proposals are not in `pending_proposals()`. So attributing an applied write to a mandate REQUIRES a join: fold `file_written ∪ write_proposal`, keep each `file_written` whose resolved proposal's `producer == M6C_PROPOSER_PRODUCER`, returning `{file_written_id, target, written_at, undone}`. `undone` = a later `file_written` carries `undo_of == this.file_written_id`. Two invariants (L2 + security L3): **(i) completeness** — the join is TOTAL in practice (an M6c proposal is retained while its `file_written` is live, so every applied M6c write is attributable); **(ii) fail-closed against false attribution** — a row is included only when its resolved proposal is PROVABLY M6c, so an unprovable-producer write is EXCLUDED (claiming a write is a mandate write when it can't be proven is worse than omitting it; by (i) this exclusion is unreachable for a real M6c write).

- [ ] Write the failing test in `crates/bossclaw-core/tests/reconcile.rs` (append after the Task 3 test). It applies one real M6c write and one M6b write through the engine, then asserts `mandate_writes()` attributes only the M6c one, and flips `undone` after an undo. Build the proposals via `append_write_proposal_with` + `execute_write_resolving` so the `file_written` carries `resolves_proposal`:

```rust
#[cfg(unix)]
#[test]
fn mandate_writes_attributes_m6c_excludes_m6b_and_flips_undone() {
    use bossclaw_core::actuator::{WriteOp, WriteProposal};
    use sha2::{Digest, Sha256};
    // `dir` is a read+write-granted PathBuf (the harness grants both).
    let (log, _home, dir) = common::open_log_with_write_grant();

    // Helper: write `file` with `old`, ingest it, then apply a rewrite to `new` resolving a
    // proposal stamped with `producer`. Returns (file_written_id, canonical_target).
    let apply_via = |file: &str, old: &[u8], new: &[u8], producer: &str| -> (String, String) {
        let path = dir.join(file);
        std::fs::write(&path, old).unwrap();
        let fid = common::ingest_one(&log, &path);
        let hash = hex::encode(Sha256::digest(new));
        let canonical = std::fs::canonicalize(&path).unwrap().to_string_lossy().to_string();
        let key = serde_json::json!({"src":"a","relation":"r","dst":file});
        let gated = log.propose_write(WriteProposal { target: path.clone(), new_content: new.to_vec(),
            op: WriteOp::Edit, source_event_ids: vec![fid.clone()], rationale: "sync".to_string() }).unwrap();
        let vs = serde_json::json!({"requires_loud_modal": gated.verdict.requires_loud_modal,
            "taint": format!("{:?}", gated.verdict.taint), "allowed": gated.verdict.allowed,
            "base_content_hash": gated.verdict.base_content_hash});
        let pid = log.append_write_proposal_with(&canonical, "edit", &hash, new.len() as u64, "sync",
            &key, &vs, std::slice::from_ref(&fid), producer).unwrap();
        log.put_proposal_bytes(&pid, new, &hash).unwrap();
        // Apply (acked — these are loud because ingested ⇒ Untrusted; the activity list test
        // only cares about attribution, so pass true to let the write land).
        let fw = log.execute_write_resolving(gated, &pid, true).unwrap();
        (fw, canonical)
    };

    let (m6c_fw, m6c_target) = apply_via("mandated.md", b"old-c\n", b"new-c\n",
        bossclaw_core::graph::M6C_PROPOSER_PRODUCER);
    let (_m6b_fw, _m6b_target) = apply_via("reconciled.md", b"old-b\n", b"new-b\n",
        bossclaw_core::graph::M6B_PROPOSER_PRODUCER);

    let writes = log.mandate_writes().unwrap();
    assert_eq!(writes.len(), 1, "only the M6c write is attributed to a mandate");
    let w = &writes[0];
    assert_eq!(w.file_written_id, m6c_fw);
    assert_eq!(w.target, m6c_target);
    assert!(!w.undone, "not undone yet");
    assert!(!w.written_at.is_empty(), "written_at carried");

    // Undo the M6c write → its row flips `undone` (the undo is a file_written carrying undo_of,
    // no resolves_proposal, so it is excluded from the join and only flips the original's flag).
    log.undo_write(&m6c_fw).unwrap();
    let writes2 = log.mandate_writes().unwrap();
    assert_eq!(writes2.len(), 1, "the undo does not add an attributed row (it has no resolves_proposal)");
    assert!(writes2[0].undone, "undone flips true after undo_write");
}
```

- [ ] Run it (expect FAIL — `mandate_writes` does not exist):
  `cargo test -p bossclaw-core --test reconcile mandate_writes_attributes_m6c_excludes_m6b_and_flips_undone`
  Expected output: compile error `no method named mandate_writes found for ... EventLog`.

- [ ] Implement in `crates/bossclaw-core/src/log.rs`. Add the struct above the `impl EventLog` block that holds `pending_proposals` (so it is module-public, mirroring `PendingProposal`), and the method inside that impl. It folds `write_proposal` (id → producer) and `file_written` (the applied writes + the undo markers) in one `events_of_types` pass:

```rust
/// One applied write attributed to a mandate (M6c), projected for the desktop Mandate-activity
/// list. Built by [`EventLog::mandate_writes`] via a join — an applied write is stamped with the
/// actuator producer, not the proposer, so the discriminator is the resolved proposal's producer.
/// `#[cfg(unix)]` like the rest of the mandate/confirm surface.
#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MandateWriteRecord {
    /// The `file_written` event id (also the handle Undo passes to `undo_write`).
    pub file_written_id: String,
    /// Canonical target path written (`content["target"]`).
    pub target: String,
    /// RFC-3339 time the write was recorded (`ts`).
    pub written_at: String,
    /// True iff a LATER `file_written` carries `undo_of == this.file_written_id`.
    pub undone: bool,
}

impl EventLog {
    /// Every applied write attributable to a MANDATE (M6c), newest-LAST in event order
    /// (`events_of_types` returns `seq ASC`; the desktop reverses for newest-first display).
    ///
    /// Attribution requires a JOIN: a `file_written` is stamped `ACTUATOR_PRODUCER`, so the only
    /// link to a mandate is `content.resolves_proposal` → a `write_proposal` whose
    /// `model_meta.model_id == M6C_PROPOSER_PRODUCER`. Two invariants govern the join (SP5 L2 +
    /// security L3):
    ///   • COMPLETENESS — because Option B removed the preventive review, an applied M6c write that
    ///     never surfaced (with no Undo offered) would be an invisible autonomous change. In PRACTICE
    ///     the join is TOTAL: an M6c `write_proposal` is never GC'd while its `file_written` is live
    ///     (a resolved proposal is retained), so every applied M6c write is attributable here.
    ///   • FAIL-CLOSED against FALSE attribution — a row is included ONLY when its resolved proposal's
    ///     producer is PROVABLY `M6C_PROPOSER_PRODUCER`. A `file_written` whose `resolves_proposal`
    ///     cannot be resolved to a known M6c producer is EXCLUDED (not "degraded to target-only"):
    ///     claiming an unprovable write is a mandate write would be worse than omitting it, and the
    ///     completeness invariant means this exclusion is unreachable for a real M6c write anyway.
    /// `#[cfg(unix)]` (mandate surface).
    #[cfg(unix)]
    pub fn mandate_writes(&self) -> Result<Vec<MandateWriteRecord>, BossclawError> {
        use std::collections::{HashMap, HashSet};
        // proposal id → producer (from write_proposal events).
        let mut producer_of: HashMap<String, String> = HashMap::new();
        // file_written_id → (target, written_at) for the FORWARD (non-undo) applied writes that
        // resolve a proposal.
        let mut applied: Vec<(String, String, String, String)> = Vec::new(); // (fw_id, target, ts, resolves)
        // the set of file_written ids that a later undo cites (`undo_of`).
        let mut undone_ids: HashSet<String> = HashSet::new();

        for ev in self.events_of_types(&[
            crate::graph::WRITE_PROPOSAL_EVENT_TYPE,
            crate::graph::FILE_WRITTEN_EVENT_TYPE,
        ])? {
            match ev.event_type.as_str() {
                t if t == crate::graph::WRITE_PROPOSAL_EVENT_TYPE => {
                    let producer = ev.model_meta.as_ref()
                        .map(|m| m.model_id.clone()).unwrap_or_default();
                    producer_of.insert(ev.id.clone(), producer);
                }
                _ => {
                    // file_written. An UNDO carries `undo_of` and NO `resolves_proposal` — record
                    // the undone id and skip it as an attributed row.
                    if let Some(undone) = ev.content.get("undo_of").and_then(|v| v.as_str()) {
                        undone_ids.insert(undone.to_string());
                        continue;
                    }
                    if let Some(resolves) = ev.content.get("resolves_proposal").and_then(|v| v.as_str()) {
                        let target = ev.content.get("target").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        applied.push((ev.id.clone(), target, ev.ts.clone(), resolves.to_string()));
                    }
                }
            }
        }

        Ok(applied
            .into_iter()
            .filter(|(_fw, _t, _ts, resolves)| {
                // Keep ONLY writes whose resolved proposal is an M6c mandate proposal. An
                // unresolvable producer (GC'd proposal) cannot be proven M6c ⇒ excluded (in
                // practice unreachable — an M6c proposal outlives its file_written).
                producer_of.get(resolves).map(|p| p == crate::graph::M6C_PROPOSER_PRODUCER).unwrap_or(false)
            })
            .map(|(fw, target, ts, _resolves)| {
                let undone = undone_ids.contains(&fw);
                MandateWriteRecord { file_written_id: fw, target, written_at: ts, undone }
            })
            .collect())
    }
}
```

  Re-export the type from the crate root in `crates/bossclaw-core/src/lib.rs` — add directly beneath the gated `PendingProposal` re-export (lib.rs:65):

```rust
#[cfg(unix)]
pub use log::MandateWriteRecord;
```

- [ ] Run it (expect PASS): `cargo test -p bossclaw-core --test reconcile mandate_writes_attributes_m6c_excludes_m6b_and_flips_undone`
  Expected output: `test result: ok. 1 passed`.

- [ ] Run the full engine suite + the security clippy gate to confirm the four engine changes are clean:
  `cargo test -p bossclaw-core && cargo clippy -p bossclaw-core --features ollama -- -D warnings`
  Expected output: `test result: ok.` (0 failed) and `Finished` with no clippy warnings.

- [ ] Commit:
  `git add crates/bossclaw-core/src/log.rs crates/bossclaw-core/src/lib.rs crates/bossclaw-core/tests/reconcile.rs`
  `git commit -m "$(cat <<'EOF'
feat(bossclaw-core): mandate_writes() attribution join + MandateWriteRecord

SP5 engine change (b). mandate_writes() joins file_written -> resolves_proposal ->
write_proposal.producer == M6C, returning {file_written_id, target, written_at, undone};
undone flips when a later file_written carries undo_of == the id. Never silently drops an
attributed write. Powers the desktop Mandate-activity list + Undo.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"`

---

### Task 5: `prime_switches` switch-fix (persist explicit mandates-on) + flip the existing test

**Files:**
- Modify: `apps/desktop/src-tauri/src/engine/mod.rs` (`prime_switches` + flip the test)
- Test: `apps/desktop/src-tauri/src/engine/mod.rs` (tests mod — the FLIPPED existing test)

**What changes (grounded):** `prime_switches` (engine/mod.rs:359-372) already guards evolve + proposals with `!explicitly_set(ConfigFlag::...)`, but force-offs mandates UNCONDITIONALLY (lines 368-369: `if log.mandates_enabled()? { log.set_mandates_enabled(false)?; }`). SP5 ships mandates, so apply the same explicit-choice guard. The existing test `prime_switches_preserves_explicit_proposals_but_forces_mandates_off` (engine/mod.rs:932-951) asserts mandates STAY off across re-open — that contract flips: an explicit mandates-on must now ALSO persist.

- [ ] FLIP the existing test in `apps/desktop/src-tauri/src/engine/mod.rs` (tests mod, currently at line 932). Rename it to drop "forces_mandates_off" and assert BOTH explicit proposals-on AND explicit mandates-on survive a re-open (do not drop the proposals coverage). Replace the whole `prime_switches_preserves_explicit_proposals_but_forces_mandates_off` test:

```rust
    #[tokio::test]
    async fn prime_switches_preserves_explicit_proposals_and_mandates() {
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault.clone(), &dir);
        let log = handle.get_or_open(true).await.unwrap();
        // After first open everything is forced off (never-set defaults).
        assert!(!log.proposals_enabled().unwrap());
        assert!(!log.mandates_enabled().unwrap());

        // The user explicitly enables BOTH proposals and mandates.
        log.set_proposals_enabled(true).unwrap();
        log.set_mandates_enabled(true).unwrap();
        assert!(log.proposals_enabled().unwrap());
        assert!(log.mandates_enabled().unwrap());
        drop(log);

        // Re-open with a FRESH handle (same vault + db_path) → prime_switches runs again.
        let handle2 = new_test_handle(vault, &dir);
        let log2 = handle2.get_or_open(true).await.unwrap();
        assert!(log2.proposals_enabled().unwrap(), "an explicit proposals true MUST persist across opens");
        assert!(log2.mandates_enabled().unwrap(), "an explicit mandates true MUST persist across opens (SP5)");
    }
```

- [ ] Run it (expect FAIL — current `prime_switches` force-offs the explicit mandates true):
  `cargo test -p air_agent_desktop prime_switches_preserves_explicit_proposals_and_mandates`
  Expected output: assertion failure `an explicit mandates true MUST persist across opens (SP5)` (left `false`, right `true`).

- [ ] Implement the `prime_switches` change in `apps/desktop/src-tauri/src/engine/mod.rs` (lines 359-372). Replace the mandate force-off arm (lines 367-369) so it mirrors evolve/proposals:

```rust
        // SP5 ships mandates: persist an explicit user choice (force off ONLY when never set),
        // exactly like evolve/proposals above. A fresh install still primes off (default-open,
        // never-set ⇒ explicitly_set is false ⇒ force off).
        if !log.explicitly_set(ConfigFlag::Mandates)? && log.mandates_enabled()? {
            log.set_mandates_enabled(false)?;
        }
```

  The full function after the edit (for reference — `ConfigFlag` is already `use`d at the top of the fn):

```rust
    fn prime_switches(log: &EventLog) -> Result<(), bossclaw_core::BossclawError> {
        use bossclaw_core::ConfigFlag;
        if !log.explicitly_set(ConfigFlag::Evolve)? && log.evolve_enabled()? {
            log.set_evolve_enabled(false)?;
        }
        if !log.explicitly_set(ConfigFlag::Proposals)? && log.proposals_enabled()? {
            log.set_proposals_enabled(false)?;
        }
        // SP5 ships mandates: persist an explicit user choice (force off ONLY when never set).
        if !log.explicitly_set(ConfigFlag::Mandates)? && log.mandates_enabled()? {
            log.set_mandates_enabled(false)?;
        }
        Ok(())
    }
```

- [ ] Run it (expect PASS): `cargo test -p air_agent_desktop prime_switches_preserves_explicit_proposals_and_mandates`
  Expected output: `test result: ok. 1 passed`.

- [ ] Confirm the existing `first_open_forces_all_autonomy_switches_off` still passes (never-set defaults still go off — a fresh install must not auto-arm mandates):
  `cargo test -p air_agent_desktop first_open_forces_all_autonomy_switches_off`
  Expected output: `test result: ok. 1 passed`.

- [ ] Commit:
  `git add apps/desktop/src-tauri/src/engine/mod.rs`
  `git commit -m "$(cat <<'EOF'
fix(desktop): prime_switches persists an explicit mandates-on choice (SP5)

Mandates now ship, so prime_switches force-offs mandates only when never explicitly set
(matching evolve/proposals). A fresh install still primes off; an explicit on persists across
relaunch. Flipped the existing test to assert both proposals-on AND mandates-on survive a reopen.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"`

---

### Task 6: `set_mandates_enabled` op + command + register

**Files:**
- Modify: `apps/desktop/src-tauri/src/engine/mod.rs` (op)
- Modify: `apps/desktop/src-tauri/src/commands/engine.rs` (command)
- Modify: `apps/desktop/src-tauri/src/main.rs` (register)
- Test: `apps/desktop/src-tauri/src/engine/mod.rs` (tests mod)

- [ ] Write the failing test in `apps/desktop/src-tauri/src/engine/mod.rs` (tests mod), mirroring the existing `set_proposals_enabled_toggles_the_engine_flag`:

```rust
    #[tokio::test]
    async fn set_mandates_enabled_toggles_the_engine_flag() {
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        let log = handle.get_or_open(true).await.unwrap();
        assert!(!log.mandates_enabled().unwrap(), "primed off at first open");
        drop(log);

        handle.set_mandates_enabled(true, true).await.unwrap();
        let log = handle.get_or_open(true).await.unwrap();
        assert!(log.mandates_enabled().unwrap(), "the op flips the sticky flag on");
        drop(log);

        // Not onboarded → gate.
        assert!(matches!(
            handle.set_mandates_enabled(false, true).await,
            Err(EngineOpError::Open(EngineError::NotOnboarded))
        ));
    }
```

- [ ] Run it (expect FAIL): `cargo test -p air_agent_desktop set_mandates_enabled_toggles_the_engine_flag`
  Expected output: compile error `no method named set_mandates_enabled` (on `EngineHandle`).

- [ ] Implement the op in `apps/desktop/src-tauri/src/engine/mod.rs` (next to `set_proposals_enabled`). Use the same `spawn_blocking` template the existing ops use (note: `engine/mod.rs` imports `use tokio::task::spawn_blocking;` at the top, so the bare `spawn_blocking` is valid; the `apply_proposal`/`list_proposals` ops use the fully-qualified `tokio::task::spawn_blocking` — either is fine, match the neighbor `set_proposals_enabled` which uses the bare `spawn_blocking`):

```rust
    /// Flip the sticky engine mandates off-switch (SP5; gates the autonomous M6c proposer + the
    /// desktop auto-apply sweep). Off by default; an explicit choice persists across launches via
    /// `prime_switches`. Gated.
    pub async fn set_mandates_enabled(&self, onboarded: bool, enabled: bool) -> Result<(), EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        spawn_blocking(move || {
            log.set_mandates_enabled(enabled).map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// Read the sticky mandates on/off flag (SF5 — the UI toggle's mount-time read, so it reflects
    /// the persisted state after relaunch rather than defaulting to OFF until clicked). Gated
    /// `Result` form (a not-onboarded state surfaces via `Open(NotOnboarded)`). The sweep uses the
    /// infallible `mandates_enabled_or_false` (Task 11) instead.
    pub async fn mandates_enabled(&self, onboarded: bool) -> Result<bool, EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        spawn_blocking(move || {
            log.mandates_enabled().map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }
```

  Add a test for the getter alongside the toggle test (tests mod):

```rust
    #[tokio::test]
    async fn mandates_enabled_reflects_the_persisted_flag() {
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        // Off by default at first open.
        assert!(!handle.mandates_enabled(true).await.unwrap(), "default off");
        handle.set_mandates_enabled(true, true).await.unwrap();
        assert!(handle.mandates_enabled(true).await.unwrap(), "the getter reflects the flip");
        // Not onboarded → gate.
        assert!(matches!(
            handle.mandates_enabled(false).await,
            Err(EngineOpError::Open(EngineError::NotOnboarded))
        ));
    }
```

- [ ] Run them (expect PASS): `cargo test -p air_agent_desktop set_mandates_enabled_toggles_the_engine_flag mandates_enabled_reflects_the_persisted_flag`
  Expected output: `test result: ok.` with both passing.

- [ ] Add the commands in `apps/desktop/src-tauri/src/commands/engine.rs` (after `engine_set_proposals_enabled`), mirroring the bool-setter + bool-reader templates:

```rust
/// Flip the sticky mandates off-switch (SP5 global Mandates on/off). Off by default.
#[tauri::command]
pub async fn engine_set_mandates_enabled(enabled: bool, state: State<'_, AppState>) -> Result<(), String> {
    let onboarded = state.identity_store.is_onboarded();
    state.engine.set_mandates_enabled(onboarded, enabled).await.map_err(|e| e.to_string())
}

/// Read the sticky mandates flag (SF5 — the UI toggle reads this on mount to reflect persisted state).
#[tauri::command]
pub async fn engine_mandates_enabled(state: State<'_, AppState>) -> Result<bool, String> {
    let onboarded = state.identity_store.is_onboarded();
    state.engine.mandates_enabled(onboarded).await.map_err(|e| e.to_string())
}
```

- [ ] Register both in `apps/desktop/src-tauri/src/main.rs` (in `generate_handler!`, after `engine_set_proposals_enabled` ~line 144):

```rust
            #[cfg(unix)]
            commands::engine::engine_set_mandates_enabled,
            #[cfg(unix)]
            commands::engine::engine_mandates_enabled,
```

- [ ] Build: `cargo build -p air_agent_desktop`
  Expected output: `Finished` with no errors.

- [ ] Commit:
  `git add apps/desktop/src-tauri/src/engine/mod.rs apps/desktop/src-tauri/src/commands/engine.rs apps/desktop/src-tauri/src/main.rs`
  `git commit -m "$(cat <<'EOF'
feat(desktop): set_mandates_enabled + mandates_enabled getter ops + commands (SP5 on/off)

The getter (engine_mandates_enabled) lets the UI toggle read the persisted flag on mount so it
reflects an explicit on after relaunch instead of defaulting to off until clicked (SF5).

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"`

---

### Task 7: `add`/`revoke`/`list_mandates` ops + commands + `MandateDto` + grant-rejection→typed-error mapping + a command-layer IPC test

**Files:**
- Modify: `apps/desktop/src-tauri/src/engine/mod.rs` (`MandateSummary` + 3 ops + a `Rejected` error variant)
- Modify: `apps/desktop/src-tauri/src/commands/engine.rs` (`MandateDto` + 3 commands + the command-LAYER IPC ACL test)
- Modify: `apps/desktop/src-tauri/src/main.rs` (register 3 commands)
- Test: `apps/desktop/src-tauri/src/engine/mod.rs` (tests mod — CRUD round-trip + grant-rejection)

**Grounding:** `add_mandate(target, source_scope, recipe) -> Result<String, BossclawError>` (log.rs:2800) returns a `BossclawError::InvalidInput` on a grant-time guard failure (recipe too long, target not write-granted, target under a read root). `active_mandates() -> Vec<Mandate>` (log.rs:2896). `revoke_mandate(id)` (log.rs:2871). `Mandate` has six fields (graph.rs:494-511). The op layer maps an `InvalidInput` grant rejection to a typed `EngineOpError::Rejected(String)` so the UI shows *why*.

- [ ] Write the failing tests in `apps/desktop/src-tauri/src/engine/mod.rs` (tests mod): a CRUD round-trip (add → list → revoke → list-empty) and a grant rejection (recipe too long → typed `Rejected`):

```rust
    #[tokio::test]
    async fn mandate_crud_round_trip_and_grant_rejection_is_typed() {
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        let log = handle.get_or_open(true).await.unwrap();
        // A mandate target must be WRITE-granted AND outside every read root (add_mandate guard #4),
        // so `dest` is WRITE-ONLY (no add_grant); the read-granted `scope` holds the sources.
        let dest = tempfile::tempdir().unwrap();
        let scope = tempfile::tempdir().unwrap();
        log.add_write_grant(dest.path()).unwrap(); // write-ONLY → valid mandate target root.
        log.add_grant(scope.path()).unwrap();
        let target = dest.path().join("synced.md");
        std::fs::write(&target, b"x\n").unwrap();
        drop(log);

        // add → returns a MandateSummary with the canonical fields.
        let m = handle.add_mandate(true, target.clone(), scope.path().to_path_buf(),
            "keep it synced".to_string()).await.unwrap();
        assert!(!m.mandate_grant_id.is_empty());
        assert_eq!(m.recipe, "keep it synced");
        assert!(!m.revoked);

        // list → one active mandate.
        let listed = handle.list_mandates(true).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].mandate_grant_id, m.mandate_grant_id);

        // revoke → list empty.
        handle.revoke_mandate(true, m.mandate_grant_id.clone()).await.unwrap();
        assert!(handle.list_mandates(true).await.unwrap().is_empty(), "revoked → no active mandates");

        // Grant rejection surfaces as a TYPED Rejected error. The recipe cap is guard #1 in
        // add_mandate (log.rs:2807) — it fires BEFORE the write-grant (#3) and read-root (#4) checks,
        // so a > MAX_RECIPE_LEN (2048) recipe rejects for the recipe reason, mapped to Rejected.
        let huge = "a".repeat(3000);
        let err = handle.add_mandate(true, target, scope.path().to_path_buf(), huge).await.unwrap_err();
        assert!(matches!(err, EngineOpError::Rejected(_)),
            "a grant-time guard failure (here: recipe too long) maps to a typed Rejected error: {err:?}");

        // Not onboarded → gate.
        assert!(matches!(
            handle.list_mandates(false).await,
            Err(EngineOpError::Open(EngineError::NotOnboarded))
        ));
    }
```

- [ ] Run it (expect FAIL): `cargo test -p air_agent_desktop mandate_crud_round_trip_and_grant_rejection_is_typed`
  Expected output: compile error `no method named add_mandate` / `no variant ... Rejected`.

- [ ] Implement the `Rejected` variant + the `MandateSummary` type + the three ops in `apps/desktop/src-tauri/src/engine/mod.rs`.

  Add a variant to `EngineOpError` (next to `Stale`/`Revoked`/`NeedsLoudConfirm`, engine/mod.rs ~73-87):

```rust
    /// A mandate grant was refused by an engine grant-time guard (recipe too long, > 256 sources,
    /// target not write-granted, or target under a read-grant root). Carries the reason so the
    /// New-mandate form can show *why*. Distinct from `Core` so the UI can style it as a validation
    /// error, not an engine fault.
    Rejected(String),
```

  Add its `Display` arm (in `impl std::fmt::Display for EngineOpError`):

```rust
            EngineOpError::Rejected(m) => write!(f, "{m}"),
```

  Add the `MandateSummary` type (near `ProposalSummary`, engine/mod.rs ~95):

```rust
/// A mandate row for the desktop Mandates list, projected from `bossclaw_core::Mandate` (the six
/// fields map 1:1).
#[derive(Debug, Clone)]
pub struct MandateSummary {
    pub mandate_grant_id: String,
    pub target: String,
    pub source_scope: String,
    pub recipe: String,
    pub granted_at: String,
    pub revoked: bool,
}

impl From<bossclaw_core::Mandate> for MandateSummary {
    fn from(m: bossclaw_core::Mandate) -> Self {
        Self {
            mandate_grant_id: m.mandate_grant_id,
            target: m.target,
            source_scope: m.source_scope,
            recipe: m.recipe,
            granted_at: m.granted_at,
            revoked: m.revoked,
        }
    }
}
```

  Add the three ops (in `impl EngineHandle`). `add_mandate` maps an `InvalidInput` rejection to `Rejected`, then re-reads the just-granted mandate from `active_mandates()` to return its full row:

```rust
    /// Grant a mandate (SP5). On success returns the new mandate's row. A grant-time guard
    /// failure (recipe too long, > 256 sources, target not write-granted, target under a read
    /// root) is surfaced as a TYPED `Rejected` error so the form can show *why*. Gated.
    pub async fn add_mandate(&self, onboarded: bool, target: PathBuf, source_scope: PathBuf, recipe: String) -> Result<MandateSummary, EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        spawn_blocking(move || {
            let id = log.add_mandate(&target, &source_scope, &recipe).map_err(|e| match e {
                // The engine's grant-time guards reject with InvalidInput — show the reason.
                bossclaw_core::BossclawError::InvalidInput(m) => EngineOpError::Rejected(m),
                other => EngineOpError::Core(other.to_string()),
            })?;
            // Re-read the just-granted mandate to return its full row (active_mandates is the
            // single source of truth; the id we just minted must be present).
            let mandate = log.active_mandates()
                .map_err(|e| EngineOpError::Core(e.to_string()))?
                .into_iter().find(|m| m.mandate_grant_id == id)
                .ok_or_else(|| EngineOpError::Core("granted mandate not found after add".to_string()))?;
            Ok(MandateSummary::from(mandate))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// Revoke a mandate by its grant id (sticky; a revoke of an unknown id is a harmless no-op in
    /// the engine). Gated.
    pub async fn revoke_mandate(&self, onboarded: bool, mandate_grant_id: String) -> Result<(), EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        spawn_blocking(move || {
            log.revoke_mandate(&mandate_grant_id).map(|_| ()).map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// Every ACTIVE mandate, oldest-first (the engine orders by `granted_at ASC`). Gated.
    pub async fn list_mandates(&self, onboarded: bool) -> Result<Vec<MandateSummary>, EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        spawn_blocking(move || {
            let mandates = log.active_mandates().map_err(|e| EngineOpError::Core(e.to_string()))?;
            Ok(mandates.into_iter().map(MandateSummary::from).collect())
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }
```

- [ ] Run it (expect PASS): `cargo test -p air_agent_desktop mandate_crud_round_trip_and_grant_rejection_is_typed`
  Expected output: `test result: ok. 1 passed`.

- [ ] Add `MandateDto` + the three commands in `apps/desktop/src-tauri/src/commands/engine.rs`. `add_mandate`'s grant rejection already stringifies via `.map_err(|e| e.to_string())` (the `Rejected` Display arm is the bare reason), so the JS catch sees the reason text:

```rust
#[derive(Serialize)]
pub struct MandateDto {
    pub mandate_grant_id: String,
    pub target: String,
    pub source_scope: String,
    pub recipe: String,
    pub granted_at: String,
    pub revoked: bool,
}
impl From<crate::engine::MandateSummary> for MandateDto {
    fn from(m: crate::engine::MandateSummary) -> Self {
        Self {
            mandate_grant_id: m.mandate_grant_id, target: m.target, source_scope: m.source_scope,
            recipe: m.recipe, granted_at: m.granted_at, revoked: m.revoked,
        }
    }
}

#[tauri::command]
pub async fn engine_add_mandate(target: String, source_scope: String, recipe: String, state: State<'_, AppState>) -> Result<MandateDto, String> {
    let onboarded = state.identity_store.is_onboarded();
    let m = state.engine.add_mandate(onboarded, std::path::PathBuf::from(target),
        std::path::PathBuf::from(source_scope), recipe).await.map_err(|e| e.to_string())?;
    Ok(MandateDto::from(m))
}

#[tauri::command]
pub async fn engine_revoke_mandate(mandate_grant_id: String, state: State<'_, AppState>) -> Result<(), String> {
    let onboarded = state.identity_store.is_onboarded();
    state.engine.revoke_mandate(onboarded, mandate_grant_id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn engine_list_mandates(state: State<'_, AppState>) -> Result<Vec<MandateDto>, String> {
    let onboarded = state.identity_store.is_onboarded();
    let mandates = state.engine.list_mandates(onboarded).await.map_err(|e| e.to_string())?;
    Ok(mandates.into_iter().map(MandateDto::from).collect())
}
```

  > The JS twin (Task 12) calls `invoke("engine_add_mandate", { target, sourceScope, recipe })`; Tauri's documented camelCase↔snake_case arg mapping bridges `sourceScope` → `source_scope`. Same convention as `engine_undo_apply`'s `fileWrittenId` ↔ `file_written_id`.

- [ ] Register the three commands in `apps/desktop/src-tauri/src/main.rs` (after `engine_set_mandates_enabled`):

```rust
            #[cfg(unix)]
            commands::engine::engine_add_mandate,
            #[cfg(unix)]
            commands::engine::engine_revoke_mandate,
            #[cfg(unix)]
            commands::engine::engine_list_mandates,
```

- [ ] Add the command-LAYER IPC arg-binding test in `apps/desktop/src-tauri/src/commands/engine.rs` (test mod, after the existing `engine_undo_apply_binds_camelcase_arg_over_ipc` at ~line 551). This reuses the EXACT `__allow_command` ACL discipline (a `tauri::test` mock app has no capability grant, so the request would be rejected before arg deserialization without the grant). It proves `sourceScope` binds to `source_scope` AND the op ran (an un-write-granted target makes `add_mandate` reject with the engine's `"target not write-granted"` guard signature — a string that can only appear PAST the arg binding + the op):

```rust
    #[test]
    fn engine_add_mandate_binds_camelcase_arg_over_ipc() {
        use crate::air::identity::{IdentityMetadata, IdentityStore};
        use crate::air::types::Did;
        let dir = tempfile::tempdir().unwrap();
        let vault = CmdTestVault::new();
        let identity_store = IdentityStore::new(vault.clone(), dir.path().to_path_buf());
        identity_store.save_signing_key(&[7u8; 32]).unwrap();
        identity_store.save_metadata(&IdentityMetadata {
            did: Did("did:wba:AIR-TEST:cmd".to_string()),
            name: "Test".to_string(),
            created_at: "2026-06-23T00:00:00Z".to_string(),
        }).unwrap();
        let engine = Arc::new(crate::engine::EngineHandle::new(
            vault, dir.path().to_path_buf(),
            Arc::new(crate::engine::embed::MockEmbedderProvider::new(8)),
            Arc::new(crate::engine::reason::MockReasonerProvider::new("m")),
        ));
        let state = AppState {
            air_client: Arc::new(crate::air::MockAirClient::new()),
            identity_store,
            inbox: Arc::new(crate::inbox::manager::InboxManager::new()),
            engine,
        };

        // ACL: a mock app has NO capability grant, so without this the request is rejected with
        // "engine_add_mandate not allowed. Plugin not found" BEFORE arg deserialization — and a
        // negative assertion would vacuously pass. The invoke URL `http://tauri.localhost` is a
        // Remote origin, so the grant must name that exact origin.
        let origin: tauri::utils::acl::RemoteUrlPattern =
            "http://tauri.localhost".parse().expect("valid remote url pattern");
        let mut context = tauri::test::mock_context(tauri::test::noop_assets());
        context.runtime_authority_mut().__allow_command(
            "engine_add_mandate".to_string(),
            tauri::utils::acl::ExecutionContext::Remote { url: origin },
        );
        let app = tauri::test::mock_builder()
            .invoke_handler(tauri::generate_handler![engine_add_mandate])
            .build(context)
            .expect("build mock app");
        app.manage(state);
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default()).build().unwrap();

        // A real (existing) but NOT write-granted target dir + a scope dir, so the op REACHES the
        // engine grant guard and rejects with a deterministic signature (proving the op ran).
        let target_dir = tempfile::tempdir().unwrap();
        let scope_dir = tempfile::tempdir().unwrap();
        let target = target_dir.path().join("synced.md");
        std::fs::write(&target, b"x\n").unwrap();

        let res = tauri::test::get_ipc_response(
            &webview,
            tauri::webview::InvokeRequest {
                cmd: "engine_add_mandate".into(),
                callback: tauri::ipc::CallbackFn(0),
                error: tauri::ipc::CallbackFn(1),
                url: "http://tauri.localhost".parse().unwrap(),
                // camelCase `sourceScope` — must bind to the snake_case `source_scope` param.
                body: tauri::ipc::InvokeBody::Json(serde_json::json!({
                    "target": target.to_string_lossy(),
                    "sourceScope": scope_dir.path().to_string_lossy(),
                    "recipe": "r",
                })),
                headers: Default::default(),
                invoke_key: tauri::test::INVOKE_KEY.to_string(),
            },
        );
        // POSITIVE op-ran assertion: the engine grant guard's "target not write-granted" signature
        // can ONLY appear past the arg binding + the op. A routing reject, a missing-key
        // deserialize error, or a wrong key (`source_scope` never bound) could never produce it.
        let err = res.expect_err("an un-write-granted target makes add_mandate reject");
        let msg = err.to_string();
        assert!(msg.contains("not write-granted"),
            "expected the engine grant-guard signature (sourceScope bound + op ran), got: {msg}");
    }
```

  Run it (expect PASS after the command exists): `cargo test -p air_agent_desktop engine_add_mandate_binds_camelcase_arg_over_ipc`
  Expected output: `test result: ok. 1 passed`.

- [ ] Build: `cargo build -p air_agent_desktop`
  Expected output: `Finished` with no errors.

- [ ] Commit:
  `git add apps/desktop/src-tauri/src/engine/mod.rs apps/desktop/src-tauri/src/commands/engine.rs apps/desktop/src-tauri/src/main.rs`
  `git commit -m "$(cat <<'EOF'
feat(desktop): add/revoke/list_mandates ops + commands + MandateDto

SP5 mandate CRUD. add_mandate maps the engine's grant-time guard failures (InvalidInput) to a
typed EngineOpError::Rejected so the New-mandate form shows why. Command-layer IPC test asserts
sourceScope binds to source_scope via the Tauri __allow_command ACL discipline + a positive
op-ran signature (the engine "not write-granted" guard).

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"`

---

### Task 8: Surface `producer` through `ProposalSummary` / `ProposalDto`

**Files:**
- Modify: `apps/desktop/src-tauri/src/engine/mod.rs` (`ProposalSummary` + `from_pending`)
- Modify: `apps/desktop/src-tauri/src/commands/engine.rs` (`ProposalDto` + `From`)
- Test: `apps/desktop/src-tauri/src/engine/mod.rs` (tests mod)

**Why:** the Review UI labels a risky mandate proposal "from mandate" so the user understands why a non-contradiction rewrite appeared. `ProposalSummary` (engine/mod.rs ~95-120) and `ProposalDto` (commands/engine.rs:235-252) need the `producer` field surfaced from `PendingProposal.producer` (Task 3).

- [ ] Write the failing test in `apps/desktop/src-tauri/src/engine/mod.rs` (tests mod). It appends an M6c-stamped proposal via `append_write_proposal_with` and asserts `list_proposals` carries the producer:

```rust
    #[tokio::test]
    async fn list_proposals_surfaces_producer() {
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        let log = handle.get_or_open(true).await.unwrap();
        let lineage = seed_one_memory_id(&log, "Alice works at Acme");
        let key = serde_json::json!({"src":"a","relation":"r","dst":"b"});
        let vs = serde_json::json!({"requires_loud_modal": false, "taint": "Clean", "allowed": true});
        let pid = log.append_write_proposal_with("/tmp/x/n.md", "edit", "deadbeef", 0, "why",
            &key, &vs, std::slice::from_ref(&lineage), bossclaw_core::graph::M6C_PROPOSER_PRODUCER).unwrap();
        drop(log);

        let proposals = handle.list_proposals(true).await.unwrap();
        let p = proposals.iter().find(|p| p.id == pid).unwrap();
        assert_eq!(p.producer, "m6c-mandate-proposer", "the M6c producer is surfaced on the summary");
    }
```

  Helper note: `seed_one_memory_id` already exists in the desktop tests mod (engine/mod.rs ~1052). `bossclaw_core::graph::M6C_PROPOSER_PRODUCER` is `pub`.

- [ ] Run it (expect FAIL): `cargo test -p air_agent_desktop list_proposals_surfaces_producer`
  Expected output: compile error `no field 'producer' on type '&ProposalSummary'`.

- [ ] Implement in `apps/desktop/src-tauri/src/engine/mod.rs`. Add the field to `ProposalSummary` (after `requires_loud_modal`):

```rust
    /// The proposer's producer stamp (`"m6b-reconciler"` / `"m6c-mandate-proposer"`), surfaced so
    /// the Review UI can label a mandate-driven rewrite "from mandate".
    pub producer: String,
```

  And set it in `from_pending` (the `Self { ... }` literal gains `producer: p.producer`):

```rust
    fn from_pending(p: bossclaw_core::PendingProposal) -> Self {
        let requires_loud_modal = p.requires_loud_modal();
        Self {
            id: p.id,
            target: p.target,
            op: p.op,
            new_content_hash: p.new_content_hash,
            rationale: p.rationale,
            requires_loud_modal,
            producer: p.producer,
        }
    }
```

- [ ] Run it (expect PASS): `cargo test -p air_agent_desktop list_proposals_surfaces_producer`
  Expected output: `test result: ok. 1 passed`.

- [ ] Add `producer` to `ProposalDto` + its `From` in `apps/desktop/src-tauri/src/commands/engine.rs` (the struct at 235-252). Add the field to the struct:

```rust
    pub producer: String,
```

  And to the `From<crate::engine::ProposalSummary>` impl body:

```rust
impl From<crate::engine::ProposalSummary> for ProposalDto {
    fn from(p: crate::engine::ProposalSummary) -> Self {
        Self {
            id: p.id, target: p.target, op: p.op,
            new_content_hash: p.new_content_hash, rationale: p.rationale,
            requires_loud_modal: p.requires_loud_modal, producer: p.producer,
        }
    }
}
```

- [ ] Build: `cargo build -p air_agent_desktop`
  Expected output: `Finished` with no errors.

- [ ] Commit:
  `git add apps/desktop/src-tauri/src/engine/mod.rs apps/desktop/src-tauri/src/commands/engine.rs`
  `git commit -m "$(cat <<'EOF'
feat(desktop): surface proposal producer through ProposalSummary/ProposalDto

So the Review UI can label a mandate-driven (m6c) rewrite "from mandate".

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"`

---

### Task 9: Create-apply fix in `apply_proposal` (skip the base-hash arm for `op == "create"`)

**Files:**
- Modify: `apps/desktop/src-tauri/src/engine/mod.rs` (`apply_proposal`)
- Test: `apps/desktop/src-tauri/src/engine/mod.rs` (tests mod)

**Grounding — the exact bug:** `propose_write` sets `base_content_hash = None` for a Create (log.rs:3155-3156, `WriteOp::Create => (None, None)`). The desktop `apply_proposal` base-hash match (engine/mod.rs:643-654) has `None => return Err(EngineOpError::Stale(...))`, so it rejects EVERY Create before it can run. The op-map (engine/mod.rs:659-664) ALREADY exists but currently runs AFTER the base-hash check. Fix: move the op-map ABOVE the base-hash check, and for `op == "create"` SKIP the base-hash arm entirely (a Create has no base; the engine's atomic no-clobber create at the syscall is the real anti-clobber — `atomic_write(.., no_clobber=true)`, actuator.rs:560-595). Do NOT add a desktop absence pre-check (it would be a strictly weaker TOCTOU check than the engine's atomic no-clobber).

- [ ] Write the failing tests in `apps/desktop/src-tauri/src/engine/mod.rs` (tests mod): a Create proposal applies (the file appears) and a Create whose target reappeared is refused by the engine's atomic no-clobber (file untouched). Build the Create proposal directly:

```rust
    #[tokio::test]
    async fn apply_create_proposal_writes_new_file_and_refuses_if_target_reappeared() {
        use bossclaw_core::actuator::{WriteOp, WriteProposal};
        use sha2::{Digest, Sha256};

        // ---- happy path: a Create lands the new file ----
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        let log = handle.get_or_open(true).await.unwrap();
        let folder = tempfile::tempdir().unwrap();
        log.add_grant(folder.path()).unwrap();
        log.add_write_grant(folder.path()).unwrap();
        // A lineage event so the Tier-B proposal is valid (a Create cites SOME source).
        let lineage = seed_one_memory_id(&log, "make a synced file");
        let target = folder.path().join("new.md"); // does NOT exist yet → Create
        let new_bytes = b"freshly synced content\n".to_vec();
        let hash = hex::encode(Sha256::digest(&new_bytes));
        // Gate a Create proposal (base_content_hash is None for a Create).
        let gated = log.propose_write(WriteProposal { target: target.clone(), new_content: new_bytes.clone(),
            op: WriteOp::Create, source_event_ids: vec![lineage.clone()], rationale: "create".to_string() }).unwrap();
        assert!(gated.verdict.base_content_hash.is_none(), "a Create carries no base hash");
        // The recorded target is the canonical PARENT-joined path (Create canonicalizes the parent).
        let canonical = gated.verdict.target_canonical.as_ref().unwrap().to_string_lossy().to_string();
        let key = serde_json::json!({"src":"a","relation":"r","dst":"b"});
        let vs = serde_json::json!({"requires_loud_modal": gated.verdict.requires_loud_modal,
            "taint": format!("{:?}", gated.verdict.taint), "allowed": gated.verdict.allowed,
            "base_content_hash": gated.verdict.base_content_hash});
        let pid = log.append_write_proposal(&canonical, "create", &hash, new_bytes.len() as u64,
            "create", &key, &vs, std::slice::from_ref(&lineage)).unwrap();
        log.put_proposal_bytes(&pid, &new_bytes, &hash).unwrap();
        drop(log);

        // A Create is loud (ingested-target Step-4 taint doesn't apply, but a brand-new write to a
        // tracked folder may still be non-loud if Clean; pass acknowledged_loud=false and, if the
        // gate is loud, retry true — robust either way). Try false first:
        let first = handle.apply_proposal(true, pid.clone(), false).await;
        let applied = match first {
            Ok(r) => r,
            Err(EngineOpError::NeedsLoudConfirm(_)) => handle.apply_proposal(true, pid.clone(), true).await.unwrap(),
            Err(e) => panic!("unexpected create apply error: {e:?}"),
        };
        assert!(!applied.file_written_id.is_empty(), "the Create returned a file_written id");
        assert_eq!(std::fs::read(&target).unwrap(), new_bytes, "the new file was created with the bytes");

        // ---- refuse path: a Create whose target now EXISTS is refused (engine atomic no-clobber) ----
        let (vault2, dir2) = test_vault_and_dir();
        let handle2 = new_test_handle(vault2, &dir2);
        let log2 = handle2.get_or_open(true).await.unwrap();
        let folder2 = tempfile::tempdir().unwrap();
        log2.add_grant(folder2.path()).unwrap();
        log2.add_write_grant(folder2.path()).unwrap();
        let lineage2 = seed_one_memory_id(&log2, "make a synced file");
        let target2 = folder2.path().join("appears.md"); // absent at propose
        let new2 = b"would-be content\n".to_vec();
        let hash2 = hex::encode(Sha256::digest(&new2));
        let g2 = log2.propose_write(WriteProposal { target: target2.clone(), new_content: new2.clone(),
            op: WriteOp::Create, source_event_ids: vec![lineage2.clone()], rationale: "create".to_string() }).unwrap();
        let canon2 = g2.verdict.target_canonical.as_ref().unwrap().to_string_lossy().to_string();
        let vs2 = serde_json::json!({"requires_loud_modal": g2.verdict.requires_loud_modal,
            "taint": format!("{:?}", g2.verdict.taint), "allowed": g2.verdict.allowed,
            "base_content_hash": g2.verdict.base_content_hash});
        let pid2 = log2.append_write_proposal(&canon2, "create", &hash2, new2.len() as u64, "create",
            &serde_json::json!({"src":"a","relation":"r","dst":"b"}), &vs2, std::slice::from_ref(&lineage2)).unwrap();
        log2.put_proposal_bytes(&pid2, &new2, &hash2).unwrap();
        drop(log2);

        // The target reappears on disk BEFORE apply (a racer created it).
        std::fs::write(&target2, b"already here\n").unwrap();
        // Apply must fail closed (SF1): the FRESH `propose_write` re-gate runs `classify_op_existence`
        // and, seeing op=Create against an EXISTING target, sets `reject_reason = "create target
        // already exists"`; `apply_proposal` maps a `reject_reason` to `EngineOpError::Stale` (the
        // `gated.verdict.reject_reason.is_some() => Stale` arm), so it fails BEFORE execute — the
        // syscall atomic no-clobber is the deeper backstop but is not what fires here. Assert the
        // SPECIFIC Stale variant, and that the racer's file is untouched.
        let refused = handle2.apply_proposal(true, pid2, true).await;
        assert!(matches!(refused, Err(EngineOpError::Stale(_))),
            "a Create whose target reappeared must fail closed as Stale (re-gate classify_op_existence): {refused:?}");
        assert_eq!(std::fs::read(&target2).unwrap(), b"already here\n".to_vec(),
            "the racer's file is untouched (the apply never reached execute)");
    }
```

- [ ] Run it (expect FAIL — the current `None => Stale` arm rejects the Create before it can run):
  `cargo test -p air_agent_desktop apply_create_proposal_writes_new_file_and_refuses_if_target_reappeared`
  Expected output: the happy path fails — `apply_proposal` returns `Stale("proposal has no base fingerprint to verify against")` instead of creating the file.

- [ ] Implement the fix in `apps/desktop/src-tauri/src/engine/mod.rs` `apply_proposal`. Reorder so the op-map runs FIRST, then gate the base-hash anti-clobber on `op != Create`. Replace the block from the `// ── ANTI-CLOBBER` comment (engine/mod.rs:637) through the end of the op-map (line 664) with:

```rust
            // Map the proposal's OWN op back to a `WriteOp` FIRST (fail-closed on an unknown
            // string — NEVER default to Edit), because the base-hash anti-clobber below applies
            // only to Edit/Delete (a Create has no base).
            let op = match p.op.as_str() {
                "edit" => bossclaw_core::actuator::WriteOp::Edit,
                "create" => bossclaw_core::actuator::WriteOp::Create,
                "delete" => bossclaw_core::actuator::WriteOp::Delete,
                other => return Err(EngineOpError::Core(format!("unknown proposal op: {other}"))),
            };

            // ── ANTI-CLOBBER (Edit/Delete only): compare the live file to the proposal's
            // propose-time fingerprint. This is the TRUE staleness detector (a fresh propose_write
            // below re-bases on the live file and cannot detect that it changed). A CREATE has no
            // base (target absent at propose) — its anti-clobber is the engine's ATOMIC no-clobber
            // create at the syscall (RENAME_NOREPLACE on Linux; statat+renameat on macOS). We do
            // NOT add a desktop absence pre-check: it would be a strictly weaker TOCTOU check than
            // the engine's atomic no-clobber. So skip the base-hash arm entirely for a Create.
            if op != bossclaw_core::actuator::WriteOp::Create {
                let live_bytes = std::fs::read(&p.target)
                    .map_err(|e| EngineOpError::Stale(format!("could not read target: {e}")))?;
                let live_hash = hex::encode(Sha256::digest(&live_bytes));
                match &p.base_content_hash {
                    Some(base) if *base != live_hash => {
                        return Err(EngineOpError::Stale(format!(
                            "the file changed since this was suggested (base {base} != live {live_hash})"
                        )));
                    }
                    None => {
                        // No recorded base on an Edit/Delete (legacy/minimal) → cannot prove freshness.
                        return Err(EngineOpError::Stale("proposal has no base fingerprint to verify against".to_string()));
                    }
                    _ => {} // base matches live → proceed.
                }
            }
```

  Note: `WriteOp` derives `PartialEq` (actuator.rs:20), so `op != WriteOp::Create` compiles. The rest of `apply_proposal` (the `get_proposal_bytes_checked`, the fresh `propose_write` using `op`, the reject/allowed/loud checks, and `execute_write_resolving(gated, &p.id, acknowledged_loud)` from Task 2) is unchanged.

- [ ] Run it (expect PASS): `cargo test -p air_agent_desktop apply_create_proposal_writes_new_file_and_refuses_if_target_reappeared`
  Expected output: `test result: ok. 1 passed`. The Create now applies; the reappeared-target Create fails closed as `Stale` because the fresh `propose_write` re-gate's `classify_op_existence` sees `op=Create` against an existing file → `reject_reason "create target already exists"` → mapped to `Stale` (the syscall atomic no-clobber is the deeper backstop, not the arm that fires here).

- [ ] Confirm the SP4 Edit apply tests still pass (the Edit path's base-hash check is unchanged, just wrapped in the `op != Create` guard):
  `cargo test -p air_agent_desktop apply_proposal_writes_file_and_resolves_then_stale_fails_closed apply_proposal_loud_needs_ack_then_applies`
  Expected output: `test result: ok.` with both passing.

- [ ] Commit:
  `git add apps/desktop/src-tauri/src/engine/mod.rs`
  `git commit -m "$(cat <<'EOF'
fix(desktop): apply_proposal supports Create (skip the base-hash arm for op==create)

A Create has no propose-time base hash, so the base-hash anti-clobber arm wrongly rejected
every Create as Stale. Reorder the op-map above the base-hash check and skip the base-hash arm
for Create — the engine's atomic no-clobber create (RENAME_NOREPLACE / statat+renameat) is the
real anti-clobber. No desktop absence pre-check (would be a weaker TOCTOU check).

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"`

---

### Task 10: `mandate_writes` desktop op + command + `MandateWriteDto`

**Files:**
- Modify: `apps/desktop/src-tauri/src/engine/mod.rs` (`MandateWriteSummary` + op)
- Modify: `apps/desktop/src-tauri/src/commands/engine.rs` (`MandateWriteDto` + command)
- Modify: `apps/desktop/src-tauri/src/main.rs` (register)
- Test: `apps/desktop/src-tauri/src/engine/mod.rs` (tests mod)

- [ ] Write the failing test in `apps/desktop/src-tauri/src/engine/mod.rs` (tests mod). It applies one M6c write (via `append_write_proposal_with` + `apply_proposal`) and asserts `mandate_writes` returns one record:

```rust
    #[tokio::test]
    async fn mandate_writes_op_returns_applied_m6c_writes() {
        use sha2::{Digest, Sha256};
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        let log = handle.get_or_open(true).await.unwrap();
        let folder = tempfile::tempdir().unwrap();
        log.add_grant(folder.path()).unwrap();
        log.add_write_grant(folder.path()).unwrap();
        let path = folder.path().join("mandated.md");
        std::fs::write(&path, b"old\n").unwrap();
        let fid = bossclaw_ingest_one(&log, &path);
        let new_bytes = b"new\n".to_vec();
        let hash = hex::encode(Sha256::digest(&new_bytes));
        let canonical = std::fs::canonicalize(&path).unwrap().to_string_lossy().to_string();
        let key = serde_json::json!({"src":"a","relation":"r","dst":"b"});
        // Stamp the proposal M6c so it is attributable as a mandate write.
        let gated = log.propose_write(bossclaw_core::actuator::WriteProposal {
            target: path.clone(), new_content: new_bytes.clone(),
            op: bossclaw_core::actuator::WriteOp::Edit, source_event_ids: vec![fid.clone()],
            rationale: "sync".to_string() }).unwrap();
        let vs = serde_json::json!({"requires_loud_modal": gated.verdict.requires_loud_modal,
            "taint": format!("{:?}", gated.verdict.taint), "allowed": gated.verdict.allowed,
            "base_content_hash": gated.verdict.base_content_hash});
        let pid = log.append_write_proposal_with(&canonical, "edit", &hash, new_bytes.len() as u64,
            "sync", &key, &vs, std::slice::from_ref(&fid), bossclaw_core::graph::M6C_PROPOSER_PRODUCER).unwrap();
        log.put_proposal_bytes(&pid, &new_bytes, &hash).unwrap();
        drop(log);

        // Apply (loud because ingested ⇒ Untrusted → ack=true).
        handle.apply_proposal(true, pid, true).await.unwrap();
        let writes = handle.mandate_writes(true).await.unwrap();
        assert_eq!(writes.len(), 1, "the applied M6c write is listed");
        assert_eq!(writes[0].target, canonical);
        assert!(!writes[0].undone);
        assert!(!writes[0].file_written_id.is_empty());

        // Not onboarded → gate.
        assert!(matches!(
            handle.mandate_writes(false).await,
            Err(EngineOpError::Open(EngineError::NotOnboarded))
        ));
    }
```

  Helper note: `bossclaw_ingest_one` already exists in the desktop tests mod (engine/mod.rs ~1070).

- [ ] Run it (expect FAIL): `cargo test -p air_agent_desktop mandate_writes_op_returns_applied_m6c_writes`
  Expected output: compile error `no method named mandate_writes` / `cannot find MandateWriteSummary`.

- [ ] Implement `MandateWriteSummary` + the op in `apps/desktop/src-tauri/src/engine/mod.rs`. Add the type near `MandateSummary`:

```rust
/// One Mandate-activity row, projected from `bossclaw_core::MandateWriteRecord`.
#[derive(Debug, Clone)]
pub struct MandateWriteSummary {
    pub file_written_id: String,
    pub target: String,
    pub written_at: String,
    pub undone: bool,
}

impl From<bossclaw_core::MandateWriteRecord> for MandateWriteSummary {
    fn from(r: bossclaw_core::MandateWriteRecord) -> Self {
        Self { file_written_id: r.file_written_id, target: r.target, written_at: r.written_at, undone: r.undone }
    }
}
```

  And the op (read template):

```rust
    /// Every applied write attributed to a mandate (M6c), for the Mandate-activity list. Gated.
    pub async fn mandate_writes(&self, onboarded: bool) -> Result<Vec<MandateWriteSummary>, EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        spawn_blocking(move || {
            let writes = log.mandate_writes().map_err(|e| EngineOpError::Core(e.to_string()))?;
            Ok(writes.into_iter().map(MandateWriteSummary::from).collect())
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }
```

  `bossclaw_core::MandateWriteRecord` is re-exported `#[cfg(unix)]` from the crate root (Task 4).

- [ ] Run it (expect PASS): `cargo test -p air_agent_desktop mandate_writes_op_returns_applied_m6c_writes`
  Expected output: `test result: ok. 1 passed`.

- [ ] Add `MandateWriteDto` + command in `apps/desktop/src-tauri/src/commands/engine.rs`:

```rust
#[derive(Serialize)]
pub struct MandateWriteDto {
    pub file_written_id: String,
    pub target: String,
    pub written_at: String,
    pub undone: bool,
}
impl From<crate::engine::MandateWriteSummary> for MandateWriteDto {
    fn from(r: crate::engine::MandateWriteSummary) -> Self {
        Self { file_written_id: r.file_written_id, target: r.target, written_at: r.written_at, undone: r.undone }
    }
}

#[tauri::command]
pub async fn engine_mandate_writes(state: State<'_, AppState>) -> Result<Vec<MandateWriteDto>, String> {
    let onboarded = state.identity_store.is_onboarded();
    let writes = state.engine.mandate_writes(onboarded).await.map_err(|e| e.to_string())?;
    Ok(writes.into_iter().map(MandateWriteDto::from).collect())
}
```

- [ ] Register in `apps/desktop/src-tauri/src/main.rs` (after `engine_list_mandates`):

```rust
            #[cfg(unix)]
            commands::engine::engine_mandate_writes,
```

- [ ] Build: `cargo build -p air_agent_desktop`
  Expected output: `Finished` with no errors.

- [ ] Commit:
  `git add apps/desktop/src-tauri/src/engine/mod.rs apps/desktop/src-tauri/src/commands/engine.rs apps/desktop/src-tauri/src/main.rs`
  `git commit -m "$(cat <<'EOF'
feat(desktop): mandate_writes op + command + MandateWriteDto (activity list)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"`

---

### Task 11: Auto-apply sweep in `scheduler.rs` (the heart of SP5)

**Files:**
- Modify: `apps/desktop/src-tauri/src/engine/mod.rs` (a `mandate_autoapply_sweep` op on `EngineHandle`)
- Modify: `apps/desktop/src-tauri/src/engine/scheduler.rs` (`MANDATE_AUTOAPPLY_PER_SWEEP`, a pure `sweep_candidates` helper + its test, the sweep call after `evolve_once`)
- Test: `apps/desktop/src-tauri/src/engine/mod.rs` (tests mod — the three sweep tests)

**Design (grounded against the real scheduler):** `scheduler::spawn` (scheduler.rs:54-72) calls `engine.evolve_once(onboarded)` on each `Run` wake. After it returns, sweep: list pending proposals (already oldest-first, `seq ASC`); filter to `producer == M6C_PROPOSER_PRODUCER`; cap at `MANDATE_AUTOAPPLY_PER_SWEEP = 8`; for each, re-read `mandates_enabled` (fast-kill), then `apply_proposal(id, acknowledged_loud=false)`; swallow `NeedsLoudConfirm`/`Stale`/`Revoked`/"already resolved". The pure candidate-selection (filter + cap) is a unit-tested helper `sweep_candidates`; the I/O sweep is an `EngineHandle` op so the three behavioral tests can exercise it directly (no live scheduler loop needed).

- [ ] Write the failing pure-helper test in `apps/desktop/src-tauri/src/engine/scheduler.rs` (tests mod, after `tick_decision_gates_correctly`):

```rust
    #[test]
    fn sweep_candidates_filters_to_m6c_oldest_first_and_caps() {
        // (id, producer) pairs in oldest-first order; only m6c survive, capped at the limit.
        let pending = vec![
            ("p1".to_string(), "m6b-reconciler".to_string()),
            ("p2".to_string(), "m6c-mandate-proposer".to_string()),
            ("p3".to_string(), "m6c-mandate-proposer".to_string()),
            ("p4".to_string(), "".to_string()), // empty/unknown → never
            ("p5".to_string(), "m6c-mandate-proposer".to_string()),
        ];
        let picked = sweep_candidates(&pending, 2);
        assert_eq!(picked, vec!["p2".to_string(), "p3".to_string()],
            "only m6c, oldest-first, capped at 2 (p5 spills to the next tick)");
        // An empty/unknown producer is NEVER auto-appliable.
        let none = sweep_candidates(&[("x".to_string(), "".to_string())], 8);
        assert!(none.is_empty(), "empty producer is never swept");
    }
```

- [ ] Run it (expect FAIL): `cargo test -p air_agent_desktop sweep_candidates_filters_to_m6c_oldest_first_and_caps`
  Expected output: compile error `cannot find function sweep_candidates`.

- [ ] Implement the const + the pure helper in `apps/desktop/src-tauri/src/engine/scheduler.rs` (near `EVOLVE_INTERVAL`, after the module doc):

```rust
/// Hard cap on mandate proposals auto-applied per sweep (mirrors the engine's per-tick proposal
/// cap). Excess clean proposals spill to the next tick (~5-min-per-excess latency, accepted —
/// see the spec failure matrix). Lives desktop-side: the sweep is an app action (Approach A).
pub const MANDATE_AUTOAPPLY_PER_SWEEP: usize = 8;

/// PURE candidate selection for the auto-apply sweep (the unit-tested core). Given `(id, producer)`
/// pairs in oldest-first order, return up to `cap` ids whose producer is EXACTLY the M6c mandate
/// proposer — fail-closed: any other value, including empty/unknown, is excluded (the producer
/// filter is a contract/UX boundary; the taint/loud gate at apply is the security gate). Keeps
/// oldest-first so the sweep is fair.
pub fn sweep_candidates(pending: &[(String, String)], cap: usize) -> Vec<String> {
    pending
        .iter()
        .filter(|(_id, producer)| producer == bossclaw_core::graph::M6C_PROPOSER_PRODUCER)
        .take(cap)
        .map(|(id, _producer)| id.clone())
        .collect()
}
```

- [ ] Run it (expect PASS): `cargo test -p air_agent_desktop sweep_candidates_filters_to_m6c_oldest_first_and_caps`
  Expected output: `test result: ok. 1 passed`.

- [ ] Write the three behavioral sweep tests in `apps/desktop/src-tauri/src/engine/mod.rs` (tests mod). They exercise a new `mandate_autoapply_sweep` op: ① a CLEAN mandate proposal → auto-applied (file changes, proposal resolved); ② a RISKY (secret-shaped) mandate proposal → stays queued (file untouched); ③ an M6b reconcile proposal → NEVER auto-applied (producer filter). All build a real write-granted + ingested file and stamp the proposal's producer. Test ① needs a CLEAN proposal, which requires the Task 1 trust rule (an in-scope mandate-authorized source + non-secret content):

```rust
    // Shared builder: ingest a source under a read-granted `scope`, grant a mandate for a target
    // in a write-granted `dest`, and emit an M6c proposal rewriting the target from the source.
    // Returns (handle, dest-keepalive, scope-keepalive, target path, canonical target, pid).
    #[cfg(unix)]
    async fn seed_clean_mandate_proposal(
    ) -> (EngineHandle, tempfile::TempDir, tempfile::TempDir, tempfile::TempDir, std::path::PathBuf, String, String) {
        use bossclaw_core::actuator::{WriteOp, WriteProposal};
        use sha2::{Digest, Sha256};
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        let log = handle.get_or_open(true).await.unwrap();
        // Mandate target dir is WRITE-ONLY (outside every read root — add_mandate guard #4); the
        // read-granted `scope` holds the source.
        let dest = tempfile::tempdir().unwrap();
        let scope = tempfile::tempdir().unwrap();
        log.add_write_grant(dest.path()).unwrap();
        log.add_grant(scope.path()).unwrap();
        let target = dest.path().join("synced.md");
        std::fs::write(&target, b"old\n").unwrap();
        let src = scope.path().join("s.md");
        std::fs::write(&src, b"clean source body\n").unwrap();
        let src_id = bossclaw_ingest_one(&log, &src);
        log.add_mandate(&target, scope.path(), "sync from scope").unwrap();
        log.rebuild_graph().unwrap();
        // A CLEAN rewrite: in-scope authorized source + non-secret content ⇒ not loud (Task 1 rule).
        let new_bytes = b"clean new content\n".to_vec();
        let gated = log.propose_write(WriteProposal { target: target.clone(), new_content: new_bytes.clone(),
            op: WriteOp::Edit, source_event_ids: vec![src_id.clone()], rationale: "sync".to_string() }).unwrap();
        assert!(!gated.verdict.requires_loud_modal, "fixture must be CLEAN for the auto-apply test");
        let hash = hex::encode(Sha256::digest(&new_bytes));
        let canonical = std::fs::canonicalize(&target).unwrap().to_string_lossy().to_string();
        let vs = serde_json::json!({"requires_loud_modal": gated.verdict.requires_loud_modal,
            "taint": format!("{:?}", gated.verdict.taint), "allowed": gated.verdict.allowed,
            "base_content_hash": gated.verdict.base_content_hash});
        let pid = log.append_write_proposal_with(&canonical, "edit", &hash, new_bytes.len() as u64,
            "sync", &serde_json::json!({"src":"a","relation":"r","dst":"b"}), &vs,
            std::slice::from_ref(&src_id), bossclaw_core::graph::M6C_PROPOSER_PRODUCER).unwrap();
        log.put_proposal_bytes(&pid, &new_bytes, &hash).unwrap();
        drop(log);
        (handle, dir, dest, scope, target, canonical, pid)
    }

    #[tokio::test]
    async fn sweep_auto_applies_a_clean_mandate_proposal() {
        let (handle, _dir, _dest, _scope, target, _canonical, pid) = seed_clean_mandate_proposal().await;
        // Mandates ON (the sweep re-reads it per item).
        handle.set_mandates_enabled(true, true).await.unwrap();
        let applied = handle.mandate_autoapply_sweep(true).await.unwrap();
        assert_eq!(applied, 1, "the clean mandate proposal was auto-applied");
        // POSITIVE mutation-verify: the file changed AND the proposal is resolved.
        assert_eq!(std::fs::read(&target).unwrap(), b"clean new content\n".to_vec(), "the file gained the synced bytes");
        assert!(handle.list_proposals(true).await.unwrap().iter().all(|p| p.id != pid), "proposal resolved");
        // And it appears in the mandate-activity list with Undo.
        let writes = handle.mandate_writes(true).await.unwrap();
        assert_eq!(writes.len(), 1, "the auto-applied write is recorded for Undo");
    }

    #[tokio::test]
    async fn sweep_leaves_a_risky_mandate_proposal_queued() {
        use bossclaw_core::actuator::{WriteOp, WriteProposal};
        use sha2::{Digest, Sha256};
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        let log = handle.get_or_open(true).await.unwrap();
        // Mandate target dir is WRITE-ONLY (add_mandate guard #4); read-granted `scope` holds the source.
        let dest = tempfile::tempdir().unwrap();
        let scope = tempfile::tempdir().unwrap();
        log.add_write_grant(dest.path()).unwrap();
        log.add_grant(scope.path()).unwrap();
        let target = dest.path().join("synced.md");
        std::fs::write(&target, b"old\n").unwrap();
        let src = scope.path().join("s.md");
        std::fs::write(&src, b"clean source\n").unwrap();
        let src_id = bossclaw_ingest_one(&log, &src);
        log.add_mandate(&target, scope.path(), "sync").unwrap();
        log.rebuild_graph().unwrap();
        // RISKY: secret-shaped new content forces loud even though the source is in-scope.
        let new_bytes = b"token=ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcd\n".to_vec();
        let gated = log.propose_write(WriteProposal { target: target.clone(), new_content: new_bytes.clone(),
            op: WriteOp::Edit, source_event_ids: vec![src_id.clone()], rationale: "sync".to_string() }).unwrap();
        assert!(gated.verdict.requires_loud_modal, "secret-shaped ⇒ loud");
        let hash = hex::encode(Sha256::digest(&new_bytes));
        let canonical = std::fs::canonicalize(&target).unwrap().to_string_lossy().to_string();
        let vs = serde_json::json!({"requires_loud_modal": gated.verdict.requires_loud_modal,
            "taint": format!("{:?}", gated.verdict.taint), "allowed": gated.verdict.allowed,
            "base_content_hash": gated.verdict.base_content_hash});
        let pid = log.append_write_proposal_with(&canonical, "edit", &hash, new_bytes.len() as u64,
            "sync", &serde_json::json!({"src":"a","relation":"r","dst":"b"}), &vs,
            std::slice::from_ref(&src_id), bossclaw_core::graph::M6C_PROPOSER_PRODUCER).unwrap();
        log.put_proposal_bytes(&pid, &new_bytes, &hash).unwrap();
        drop(log);

        handle.set_mandates_enabled(true, true).await.unwrap();
        let applied = handle.mandate_autoapply_sweep(true).await.unwrap();
        assert_eq!(applied, 0, "a risky (loud) mandate proposal is NOT auto-applied");
        assert_eq!(std::fs::read(&target).unwrap(), b"old\n".to_vec(), "the file is untouched");
        // SF3: the risky proposal stays queued AND still carries the m6c producer, so the Review
        // surface can render its "from a mandate" label (the label path survives the sweep).
        let queued = handle.list_proposals(true).await.unwrap();
        let row = queued.iter().find(|p| p.id == pid).expect("the risky proposal stays queued for SP4 Review");
        assert_eq!(row.producer, "m6c-mandate-proposer",
            "the queued risky proposal keeps its m6c producer (the 'from mandate' label path)");
    }

    #[tokio::test]
    async fn sweep_never_auto_applies_an_m6b_reconcile_proposal() {
        use bossclaw_core::actuator::{WriteOp, WriteProposal};
        use sha2::{Digest, Sha256};
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        let log = handle.get_or_open(true).await.unwrap();
        let folder = tempfile::tempdir().unwrap();
        log.add_grant(folder.path()).unwrap();
        log.add_write_grant(folder.path()).unwrap();
        let target = folder.path().join("note.md");
        std::fs::write(&target, b"old\n").unwrap();
        let fid = bossclaw_ingest_one(&log, &target);
        let new_bytes = b"new\n".to_vec();
        let gated = log.propose_write(WriteProposal { target: target.clone(), new_content: new_bytes.clone(),
            op: WriteOp::Edit, source_event_ids: vec![fid.clone()], rationale: "reconcile".to_string() }).unwrap();
        let hash = hex::encode(Sha256::digest(&new_bytes));
        let canonical = std::fs::canonicalize(&target).unwrap().to_string_lossy().to_string();
        let vs = serde_json::json!({"requires_loud_modal": gated.verdict.requires_loud_modal,
            "taint": format!("{:?}", gated.verdict.taint), "allowed": gated.verdict.allowed,
            "base_content_hash": gated.verdict.base_content_hash});
        // Stamp M6b (the reconciler) — the producer filter must exclude it from the sweep.
        let pid = log.append_write_proposal_with(&canonical, "edit", &hash, new_bytes.len() as u64,
            "reconcile", &serde_json::json!({"src":"a","relation":"r","dst":"b"}), &vs,
            std::slice::from_ref(&fid), bossclaw_core::graph::M6B_PROPOSER_PRODUCER).unwrap();
        log.put_proposal_bytes(&pid, &new_bytes, &hash).unwrap();
        drop(log);

        handle.set_mandates_enabled(true, true).await.unwrap();
        let applied = handle.mandate_autoapply_sweep(true).await.unwrap();
        assert_eq!(applied, 0, "an M6b reconcile proposal is NEVER auto-applied (producer filter)");
        assert_eq!(std::fs::read(&target).unwrap(), b"old\n".to_vec(), "the file is untouched");
        assert!(handle.list_proposals(true).await.unwrap().iter().any(|p| p.id == pid),
            "the M6b proposal stays queued for human review (SP4 unchanged)");
    }

    // Security L2: the per-item fast-kill. With TWO clean M6c proposals queued but mandates turned
    // OFF, the sweep's per-item `mandates_enabled_or_false` read gates the FIRST iteration and breaks,
    // so NOTHING is applied — proving the guard reads the LIVE flag, not a snapshot taken before the
    // loop. (A true "flip AFTER the first apply, stop the second" assertion would need a production
    // test-hook between iterations, which is out of scope; gating-with-the-flag-off pins the same
    // live-read invariant deterministically.)
    #[tokio::test]
    async fn sweep_fast_kills_when_mandates_off_even_with_clean_candidates() {
        use bossclaw_core::actuator::{WriteOp, WriteProposal};
        use sha2::{Digest, Sha256};
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        let log = handle.get_or_open(true).await.unwrap();
        // Both mandate targets live under a WRITE-ONLY dest (add_mandate guard #4); read-granted
        // `scope` holds the shared source.
        let dest = tempfile::tempdir().unwrap();
        let scope = tempfile::tempdir().unwrap();
        log.add_write_grant(dest.path()).unwrap();
        log.add_grant(scope.path()).unwrap();
        let src = scope.path().join("s.md");
        std::fs::write(&src, b"clean source\n").unwrap();
        let src_id = bossclaw_ingest_one(&log, &src);
        // Two distinct mandates+targets, each yielding a CLEAN (in-scope, non-secret) M6c proposal.
        let mut pids = Vec::new();
        for name in ["a.md", "b.md"] {
            let target = dest.path().join(name);
            std::fs::write(&target, b"old\n").unwrap();
            log.add_mandate(&target, scope.path(), "sync").unwrap();
            log.rebuild_graph().unwrap();
            let new_bytes = format!("clean new {name}\n").into_bytes();
            let gated = log.propose_write(WriteProposal { target: target.clone(), new_content: new_bytes.clone(),
                op: WriteOp::Edit, source_event_ids: vec![src_id.clone()], rationale: "sync".to_string() }).unwrap();
            assert!(!gated.verdict.requires_loud_modal, "fixture proposals must be CLEAN");
            let hash = hex::encode(Sha256::digest(&new_bytes));
            let canonical = std::fs::canonicalize(&target).unwrap().to_string_lossy().to_string();
            let vs = serde_json::json!({"requires_loud_modal": gated.verdict.requires_loud_modal,
                "taint": format!("{:?}", gated.verdict.taint), "allowed": gated.verdict.allowed,
                "base_content_hash": gated.verdict.base_content_hash});
            let pid = log.append_write_proposal_with(&canonical, "edit", &hash, new_bytes.len() as u64,
                "sync", &serde_json::json!({"src":"a","relation":"r","dst":name}), &vs,
                std::slice::from_ref(&src_id), bossclaw_core::graph::M6C_PROPOSER_PRODUCER).unwrap();
            log.put_proposal_bytes(&pid, &new_bytes, &hash).unwrap();
            pids.push(pid);
        }
        drop(log);

        // Mandates OFF (never enabled). The per-item kill-switch read gates the first iteration.
        let applied = handle.mandate_autoapply_sweep(true).await.unwrap();
        assert_eq!(applied, 0, "with mandates off the per-item fast-kill applies NOTHING");
        // Both clean proposals are untouched on disk and still queued.
        assert_eq!(std::fs::read(dest.path().join("a.md")).unwrap(), b"old\n".to_vec(), "a.md untouched");
        assert_eq!(std::fs::read(dest.path().join("b.md")).unwrap(), b"old\n".to_vec(), "b.md untouched");
        let queued = handle.list_proposals(true).await.unwrap();
        assert!(pids.iter().all(|pid| queued.iter().any(|p| &p.id == pid)),
            "both clean proposals stay queued when the sweep fast-kills");
    }
```

  > Note on Tauri ACL discipline: these four tests call `mandate_autoapply_sweep` directly on the `EngineHandle` (not over IPC), so they do NOT need `__allow_command` — that discipline is only required for command-LAYER `get_ipc_response` tests (which Task 7 covers for `engine_add_mandate`). Each sweep test here uses a POSITIVE signature (the return count) + a mutation-verify (the file bytes + the queue state) so it cannot pass vacuously.

- [ ] Run them (expect FAIL): `cargo test -p air_agent_desktop sweep_auto_applies sweep_leaves sweep_never sweep_fast_kills`
  Expected output: compile error `no method named mandate_autoapply_sweep`.

- [ ] Implement the `mandate_autoapply_sweep` op in `apps/desktop/src-tauri/src/engine/mod.rs` (in `impl EngineHandle`). It lists pending, picks candidates via the pure `crate::engine::scheduler::sweep_candidates` (MF4: `pub mod scheduler;` at engine/mod.rs:8, and `bossclaw-core` is a `[target.'cfg(unix)'.dependencies]` dep with `features=["ollama"]`, so `bossclaw_core::graph::M6C_PROPOSER_PRODUCER` inside `sweep_candidates` resolves in this Unix-gated context), then per item re-reads `mandates_enabled` (fast-kill) and calls `apply_proposal(id, false)`, swallowing the EXPECTED risky/stale/revoked outcomes but **logging the unexpected** (MF5 / security L1 — the autonomous loop must not ship blind; the desktop crate has no `log`/`tracing` dep, so use `eprintln!`, matching vault.rs:65,90):

```rust
    /// The SP5 auto-apply sweep: after an evolve tick, auto-apply the CLEAN mandate (M6c)
    /// proposals and leave risky ones queued for SP4 Review. Lists pending proposals (oldest-first),
    /// filters to the M6c producer, caps at `MANDATE_AUTOAPPLY_PER_SWEEP`, and for each calls
    /// `apply_proposal(id, acknowledged_loud=false)`:
    ///   • CLEAN → applies (the engine loud-gate permits a non-loud write with ack=false);
    ///   • `NeedsLoudConfirm` (risky) → swallowed; stays open → surfaces in Review;
    ///   • `Stale` / `Revoked` / not-found → swallowed; skipped (retried next tick);
    ///   • any OTHER error → swallowed BUT logged (`eprintln!`) so one bad proposal cannot abort the
    ///     sweep yet the autonomous loop is never silent about an unexpected fault.
    /// Re-reads `mandates_enabled` PER ITEM (fast-kill if the user flips it off mid-sweep).
    /// Returns the number applied. Gated. NOTE: each `apply_proposal` re-folds `pending_proposals`
    /// internally, so a K-item sweep is 1+K O(events) folds — bounded by the cap (projection-table
    /// optimization is the future fix).
    pub async fn mandate_autoapply_sweep(&self, onboarded: bool) -> Result<usize, EngineOpError> {
        // 1. Snapshot the candidate ids (pure filter + cap over the producer-tagged pending list).
        let pending = self.list_proposals(onboarded).await?;
        let pairs: Vec<(String, String)> =
            pending.into_iter().map(|p| (p.id, p.producer)).collect();
        let candidates = crate::engine::scheduler::sweep_candidates(
            &pairs, crate::engine::scheduler::MANDATE_AUTOAPPLY_PER_SWEEP);

        // 2. Apply each, re-reading the kill-switch per item. Risky/stale/revoked are swallowed.
        let mut applied = 0usize;
        for id in candidates {
            // Fast-kill: stop the moment mandates are turned off mid-sweep.
            if !self.mandates_enabled_or_false(onboarded).await {
                break;
            }
            // Keep a copy of the id for an observability message (apply_proposal consumes it).
            let id_for_log = id.clone();
            match self.apply_proposal(onboarded, id, false).await {
                Ok(_) => applied += 1,
                // A loud (risky) proposal refuses without the ack → leave it queued for Review.
                Err(EngineOpError::NeedsLoudConfirm(_)) => {}
                // The file drifted / grant revoked / already resolved → skip; retried next tick.
                Err(EngineOpError::Stale(_)) | Err(EngineOpError::Revoked(_)) => {}
                // Any OTHER error (a transient/unexpected engine fault) is swallowed so one bad
                // proposal cannot abort the whole sweep — but it is LOGGED so the autonomous loop
                // is never silent about an anomaly (MF5 / security L1). Desktop has no log facade,
                // so eprintln! (matching the existing vault.rs convention).
                Err(e) => eprintln!("mandate sweep: proposal {id_for_log} apply failed unexpectedly (skipped): {e}"),
            }
        }
        Ok(applied)
    }

    /// `mandates_enabled`, defaulting to false on any error (the sweep's per-item kill-switch read).
    /// Mirrors `evolve_enabled_or_false`. Gated read; never panics the sweep.
    pub async fn mandates_enabled_or_false(&self, onboarded: bool) -> bool {
        let log = match self.get_or_open(onboarded).await {
            Ok(l) => l,
            Err(_) => return false,
        };
        spawn_blocking(move || log.mandates_enabled().unwrap_or(false))
            .await
            .unwrap_or(false)
    }
```

- [ ] Run them (expect PASS): `cargo test -p air_agent_desktop sweep_auto_applies sweep_leaves sweep_never sweep_fast_kills`
  Expected output: `test result: ok.` with all four passing — clean applied (file changed + resolved + in activity list), risky queued (file untouched, m6c producer survives), M6b never (file untouched), fast-kill applies nothing with mandates off (both clean proposals stay queued).

- [ ] Wire the sweep into the scheduler loop in `apps/desktop/src-tauri/src/engine/scheduler.rs` `spawn` (after `evolve_once`, scheduler.rs:68). Replace the `let _ = engine.evolve_once(onboarded).await;` line. The sweep is **intentionally coupled to the `Run` tick** (it only runs when `decide_tick == Run`, i.e. Ollama up + evolve on): on Ollama-down no tick runs, so a clean proposal queued by a PRIOR tick simply waits until Ollama returns — the same accepted Ollama-down coupling the spec's failure matrix names. The production call site logs a one-line summary (`eprintln!`) when it applied anything or hit an error, so the autonomous loop surfaces telemetry the way `evolve_once` does (MF5 / security L1):

```rust
                // Records telemetry inside; a `Busy` (manual tick overlap) is a harmless skip.
                let _ = engine.evolve_once(onboarded).await;
                // SP5: right after the tick, auto-apply the CLEAN mandate proposals it produced and
                // leave risky ones queued. INTENTIONALLY coupled to the Run tick — on Ollama-down no
                // tick runs, so an already-queued clean proposal waits until Ollama returns (accepted
                // coupling, spec failure matrix). No-ops when mandates are off (the sweep re-reads the
                // flag per item; with none on it finds no M6c candidates). Per-item errors are
                // swallowed+logged inside the sweep so one bad proposal can't break the cadence; here
                // we surface a one-line summary so the autonomous loop is observable (MF5 / security L1).
                match engine.mandate_autoapply_sweep(onboarded).await {
                    Ok(0) => {} // nothing applied this tick — stay quiet.
                    Ok(n) => eprintln!("mandate sweep: auto-applied {n} clean mandate write(s)"),
                    Err(e) => eprintln!("mandate sweep: aborted before applying: {e}"),
                }
```

- [ ] Build + run the scheduler + sweep tests + the full desktop suite to confirm the wiring compiles and nothing regressed:
  `cargo build -p air_agent_desktop && cargo test -p air_agent_desktop sweep_ tick_decision`
  Expected output: `Finished`; `test result: ok.` for `sweep_candidates_*`, `sweep_auto_applies`, `sweep_leaves`, `sweep_never`, `tick_decision_gates_correctly`.

- [ ] Commit:
  `git add apps/desktop/src-tauri/src/engine/mod.rs apps/desktop/src-tauri/src/engine/scheduler.rs`
  `git commit -m "$(cat <<'EOF'
feat(desktop): mandate auto-apply sweep in the evolve scheduler (SP5 heart)

After each evolve tick the scheduler sweeps pending proposals: pure sweep_candidates filters to
the M6c producer, oldest-first, capped at MANDATE_AUTOAPPLY_PER_SWEEP=8; each is applied with
acknowledged_loud=false so the engine loud-gate auto-applies only genuinely-clean writes and
refuses risky ones (left queued for SP4 Review). Per-item mandates_enabled re-read (fast-kill).
Three behavioral tests: clean→applied+recorded, risky→queued, M6b→never (producer filter).

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"`

---

### Task 12: `api/engine.ts` twins + `producer` on `ProposalDto`

**Files:**
- Modify: `apps/desktop/src/api/engine.ts`

**Grounding:** `api/engine.ts` imports `invoke` from `@tauri-apps/api/core` (line 1) and uses the `export const name = (args): Promise<T> => invoke<T>("engine_...", { camelCaseArgs })` style. `ProposalDto` is at lines 46-53.

- [ ] Add `producer` to the existing `ProposalDto` type in `apps/desktop/src/api/engine.ts` (lines 46-53) so it matches the Rust DTO (Task 8):

```ts
export type ProposalDto = {
  id: string;
  target: string;
  op: string;
  new_content_hash: string;
  rationale: string;
  requires_loud_modal: boolean;
  producer: string;
};
```

- [ ] Append the mandate DTO types + wrappers at the end of `apps/desktop/src/api/engine.ts`:

```ts
export type MandateDto = {
  mandate_grant_id: string;
  target: string;
  source_scope: string;
  recipe: string;
  granted_at: string;
  revoked: boolean;
};
export type MandateWriteDto = {
  file_written_id: string;
  target: string;
  written_at: string;
  undone: boolean;
};

export const setMandatesEnabled = (enabled: boolean): Promise<void> =>
  invoke<void>("engine_set_mandates_enabled", { enabled });
export const mandatesEnabled = (): Promise<boolean> => invoke<boolean>("engine_mandates_enabled");
export const addMandate = (target: string, sourceScope: string, recipe: string): Promise<MandateDto> =>
  invoke<MandateDto>("engine_add_mandate", { target, sourceScope, recipe });
export const revokeMandate = (mandateGrantId: string): Promise<void> =>
  invoke<void>("engine_revoke_mandate", { mandateGrantId });
export const listMandates = (): Promise<MandateDto[]> => invoke<MandateDto[]>("engine_list_mandates");
export const mandateWrites = (): Promise<MandateWriteDto[]> =>
  invoke<MandateWriteDto[]>("engine_mandate_writes");
```

  > camelCase arg keys (`sourceScope`, `mandateGrantId`) bridge to the snake_case Rust params (`source_scope`, `mandate_grant_id`) via Tauri's documented arg mapping — the same convention as `applyProposal`'s `acknowledgedLoud` and `undoApply`'s `fileWrittenId`.

- [ ] Typecheck (the new types must compile; `producer` is now required on `ProposalDto`, so any existing consumer that constructs a `ProposalDto` literal in TS — only test fixtures do — must add it; Task 13 updates the proposalView test fixture):
  `npm run typecheck --workspace @air-agent/desktop`
  Expected output: no errors (if the existing `proposalView.test.ts` fixture errors on the missing `producer`, that is fixed in Task 13 — run Task 13 before re-typechecking, or add `producer: ""` to that fixture now).

- [ ] Commit:
  `git add apps/desktop/src/api/engine.ts`
  `git commit -m "$(cat <<'EOF'
feat(desktop): engine.ts twins for SP5 mandate commands + producer on ProposalDto

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"`

---

### Task 13: `src/mandates` pure helpers (form validation, list view, activity view) + vitest

**Files:**
- Create: `apps/desktop/src/mandates/mandateForm.ts`, `apps/desktop/src/mandates/mandateForm.test.ts`
- Create: `apps/desktop/src/mandates/mandateView.ts`, `apps/desktop/src/mandates/mandateView.test.ts`
- Modify: `apps/desktop/src/review/proposalView.test.ts` (add `producer` to the fixture)

- [ ] First fix the existing `proposalView.test.ts` fixture so it compiles against the `producer`-extended `ProposalDto` (Task 12). In `apps/desktop/src/review/proposalView.test.ts`, add `producer: ""` to the `base` fixture:

```ts
const base: ProposalDto = {
  id: "p1",
  target: "/home/me/notes/alice.md",
  op: "edit",
  new_content_hash: "abc",
  rationale: "Alice now works at Globex",
  requires_loud_modal: false,
  producer: "",
};
```

- [ ] Write the failing `mandateForm` test `apps/desktop/src/mandates/mandateForm.test.ts` (vitest; mirrors `proposalView.test.ts` import style). The pure validator checks the three New-mandate fields client-side BEFORE the engine call (the engine is still the authority; this is fast UX feedback):

```ts
import { describe, it, expect } from "vitest";
import { validateMandateForm, MAX_RECIPE_LEN } from "./mandateForm";

describe("validateMandateForm", () => {
  const ok = { target: "/dest/synced.md", sourceScope: "/scope", recipe: "keep it synced" };

  it("accepts a complete form", () => {
    expect(validateMandateForm(ok)).toEqual({ ok: true });
  });

  it("rejects an empty target", () => {
    expect(validateMandateForm({ ...ok, target: "  " })).toEqual({ ok: false, error: "Pick a target file." });
  });

  it("rejects an empty source scope", () => {
    expect(validateMandateForm({ ...ok, sourceScope: "" })).toEqual({ ok: false, error: "Pick a source folder." });
  });

  it("rejects an empty recipe", () => {
    expect(validateMandateForm({ ...ok, recipe: "   " })).toEqual({ ok: false, error: "Describe how to keep it in sync (the recipe)." });
  });

  it("rejects a recipe over the engine cap", () => {
    const huge = "a".repeat(MAX_RECIPE_LEN + 1);
    expect(validateMandateForm({ ...ok, recipe: huge })).toEqual({
      ok: false, error: `The recipe is too long (max ${MAX_RECIPE_LEN} characters).`,
    });
  });

  it("rejects target == source scope (a self-loop the engine would reject anyway)", () => {
    expect(validateMandateForm({ target: "/scope/x.md", sourceScope: "/scope/x.md", recipe: "r" }))
      .toEqual({ ok: false, error: "The target must be outside the source folder." });
  });
});
```

- [ ] Run it (expect FAIL): `npm run test --workspace @air-agent/desktop -- src/mandates/mandateForm.test.ts`
  Expected output: vitest fails to resolve `./mandateForm`.

- [ ] Implement `apps/desktop/src/mandates/mandateForm.ts`:

```ts
/** The engine's recipe cap (`MAX_RECIPE_LEN`, graph.rs) — mirrored for fast client-side feedback;
 *  the engine remains the authority and re-checks on add. */
export const MAX_RECIPE_LEN = 2048;

/** The raw New-mandate form fields. */
export type MandateFormInput = { target: string; sourceScope: string; recipe: string };

/** A pure validation result: ok, or a single human-readable error to show inline. */
export type MandateFormResult = { ok: true } | { ok: false; error: string };

/**
 * Validate the New-mandate form CLIENT-SIDE for fast feedback (the engine's grant-time guards are
 * still the authority — `add_mandate` re-checks recipe length, source count, write-grant, and the
 * read-root self-loop, surfacing a typed Rejected error the form also shows). Pure + deterministic.
 */
export function validateMandateForm(input: MandateFormInput): MandateFormResult {
  const target = input.target.trim();
  const sourceScope = input.sourceScope.trim();
  const recipe = input.recipe.trim();
  if (target === "") return { ok: false, error: "Pick a target file." };
  if (sourceScope === "") return { ok: false, error: "Pick a source folder." };
  if (recipe === "") return { ok: false, error: "Describe how to keep it in sync (the recipe)." };
  if (recipe.length > MAX_RECIPE_LEN) {
    return { ok: false, error: `The recipe is too long (max ${MAX_RECIPE_LEN} characters).` };
  }
  // A quick self-loop sanity check (the engine's segment-aware guard is the real one); catch the
  // obvious case where the target is the source scope itself.
  if (target === sourceScope) {
    return { ok: false, error: "The target must be outside the source folder." };
  }
  return { ok: true };
}
```

- [ ] Run it (expect PASS): `npm run test --workspace @air-agent/desktop -- src/mandates/mandateForm.test.ts`
  Expected output: `6 passed` (one per `it`).

- [ ] Write the failing `mandateView` test `apps/desktop/src/mandates/mandateView.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { toMandateRow, toActivityRow } from "./mandateView";
import type { MandateDto, MandateWriteDto } from "../api/engine";

const mandate: MandateDto = {
  mandate_grant_id: "m1",
  target: "/home/me/dest/synced.md",
  source_scope: "/home/me/scope",
  recipe: "keep it synced",
  granted_at: "2026-06-24T10:00:00Z",
  revoked: false,
};

describe("toMandateRow", () => {
  it("derives the target basename + folder and passes through scope/recipe", () => {
    const r = toMandateRow(mandate);
    expect(r.id).toBe("m1");
    expect(r.targetName).toBe("synced.md");
    expect(r.targetFolder).toBe("/home/me/dest");
    expect(r.sourceScope).toBe("/home/me/scope");
    expect(r.recipe).toBe("keep it synced");
    expect(r.grantedAt).toBe("2026-06-24T10:00:00Z");
  });

  it("falls back to the full path when there is no separator", () => {
    const r = toMandateRow({ ...mandate, target: "synced.md" });
    expect(r.targetName).toBe("synced.md");
    expect(r.targetFolder).toBe("");
  });
});

const write: MandateWriteDto = {
  file_written_id: "fw1",
  target: "/home/me/dest/synced.md",
  written_at: "2026-06-24T11:00:00Z",
  undone: false,
};

describe("toActivityRow", () => {
  it("maps an applied write to a row with Undo enabled", () => {
    const r = toActivityRow(write);
    expect(r.fileWrittenId).toBe("fw1");
    expect(r.fileName).toBe("synced.md");
    expect(r.writtenAt).toBe("2026-06-24T11:00:00Z");
    expect(r.canUndo).toBe(true);
    expect(r.label).toBe("Synced");
  });

  it("disables Undo and relabels when undone", () => {
    const r = toActivityRow({ ...write, undone: true });
    expect(r.canUndo).toBe(false);
    expect(r.label).toBe("Undone");
  });
});
```

- [ ] Run it (expect FAIL): `npm run test --workspace @air-agent/desktop -- src/mandates/mandateView.test.ts`
  Expected output: vitest fails to resolve `./mandateView`.

- [ ] Implement `apps/desktop/src/mandates/mandateView.ts`:

```ts
import type { MandateDto, MandateWriteDto } from "../api/engine";

/** A display row for one active mandate. */
export type MandateRow = {
  id: string;
  targetName: string;
  targetFolder: string;
  sourceScope: string;
  recipe: string;
  grantedAt: string;
};

/** A display row for one Mandate-activity entry (an auto-applied write). */
export type ActivityRow = {
  fileWrittenId: string;
  fileName: string;
  writtenAt: string;
  canUndo: boolean;
  label: string;
};

/** Split a canonical path into (basename, folder); folder is "" when there is no separator. */
function splitPath(path: string): { name: string; folder: string } {
  const slash = path.lastIndexOf("/");
  return slash >= 0 ? { name: path.slice(slash + 1), folder: path.slice(0, slash) } : { name: path, folder: "" };
}

/** Map a mandate DTO to a display row (pure: path split + pass-through). */
export function toMandateRow(m: MandateDto): MandateRow {
  const { name, folder } = splitPath(m.target);
  return {
    id: m.mandate_grant_id,
    targetName: name,
    targetFolder: folder,
    sourceScope: m.source_scope,
    recipe: m.recipe,
    grantedAt: m.granted_at,
  };
}

/** Map a mandate-write DTO to an activity row. `undone` disables Undo and relabels. */
export function toActivityRow(w: MandateWriteDto): ActivityRow {
  const { name } = splitPath(w.target);
  return {
    fileWrittenId: w.file_written_id,
    fileName: name,
    writtenAt: w.written_at,
    canUndo: !w.undone,
    label: w.undone ? "Undone" : "Synced",
  };
}
```

- [ ] Run it (expect PASS): `npm run test --workspace @air-agent/desktop -- src/mandates/mandateView.test.ts`
  Expected output: `4 passed`.

- [ ] Typecheck the new TS + the fixed fixture:
  `npm run typecheck --workspace @air-agent/desktop`
  Expected output: no errors.

- [ ] Commit:
  `git add apps/desktop/src/mandates/mandateForm.ts apps/desktop/src/mandates/mandateForm.test.ts apps/desktop/src/mandates/mandateView.ts apps/desktop/src/mandates/mandateView.test.ts apps/desktop/src/review/proposalView.test.ts`
  `git commit -m "$(cat <<'EOF'
feat(desktop): src/mandates pure helpers — form validation + list/activity views + vitest

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"`

---

### Task 14: `MandatesPanel.tsx` + `App.tsx` nav + Review "from mandate" label

**Files:**
- Create: `apps/desktop/src/mandates/MandatesPanel.tsx`
- Modify: `apps/desktop/src/App.tsx` (`View += "mandates"` + `MandatesNavButton` + body-ternary arm)
- Modify: `apps/desktop/src/review/proposalView.ts` (surface a "from mandate" label from `producer`)

**Grounding:** `App.tsx` `View` type is at line 35; the `<nav>` block at lines 48-54; the body ternary at line 55; `ReviewNavButton` at lines 69-90; `useIdentity` and `useEffect`/`useState` are already imported (lines 1-2). The `ReviewPanel`/`MemoryPanel`/`SourcesPanel` inline style uses `Card` (`../components/Card`) + `Button` (`../components/Button`) and the `setBusy(true)/try/catch/setError/finally setBusy(false)` handler pattern (ReviewPanel lines 2056-2124). `pickFolder()` exists in `api/engine.ts` (returns `string | null`) for the file/folder pickers.

- [ ] Surface a "from mandate" label in `apps/desktop/src/review/proposalView.ts` so the reused SP4 Review queue explains a risky mandate rewrite. Add a `fromMandate` field to `ProposalRow` and set it from the producer:

```ts
import type { ProposalDto } from "../api/engine";

/** A display row for one queued proposal (pure: path split, op label, risk flag, source label). */
export type ProposalRow = {
  id: string;
  fileName: string;
  folder: string;
  why: string;
  risky: boolean;
  opLabel: string;
  fromMandate: boolean;
};

const OP_LABEL: Record<string, string> = { edit: "Edit", create: "Create", delete: "Delete" };
/** The engine's M6c mandate-proposer producer stamp (graph.rs M6C_PROPOSER_PRODUCER). */
const M6C_PRODUCER = "m6c-mandate-proposer";

/** Map a proposal DTO to a display row. `risky` mirrors the propose-time loud-modal flag;
 *  `fromMandate` is true for an M6c mandate-driven rewrite (so the queue can label it). */
export function toProposalRow(p: ProposalDto): ProposalRow {
  const slash = p.target.lastIndexOf("/");
  const fileName = slash >= 0 ? p.target.slice(slash + 1) : p.target;
  const folder = slash >= 0 ? p.target.slice(0, slash) : "";
  return {
    id: p.id,
    fileName,
    folder,
    why: p.rationale,
    risky: p.requires_loud_modal,
    opLabel: OP_LABEL[p.op] ?? p.op,
    fromMandate: p.producer === M6C_PRODUCER,
  };
}
```

  Update the existing `proposalView.test.ts` to assert the new field (append one `it` to the existing `describe("toProposalRow")`):

```ts
  it("flags an m6c proposal as from a mandate", () => {
    expect(toProposalRow({ ...base, producer: "m6c-mandate-proposer" }).fromMandate).toBe(true);
    expect(toProposalRow({ ...base, producer: "m6b-reconciler" }).fromMandate).toBe(false);
  });
```

  And surface the label in `apps/desktop/src/review/ReviewPanel.tsx` where the row header renders (the `row.risky` badge line, ReviewPanel ~2161). Add a "from mandate" tag next to the op label:

```tsx
                  <div style={{ fontWeight: 600 }}>
                    {row.opLabel}: <code>{row.fileName}</code>{" "}
                    {row.fromMandate ? <span style={{ color: "#06c", fontSize: 12 }}>· from a mandate</span> : null}{" "}
                    {row.risky ? <span style={{ color: "#b00", fontSize: 12 }}>⚠ needs careful review</span> : null}
                  </div>
```

- [ ] Create `apps/desktop/src/mandates/MandatesPanel.tsx`. It mirrors `ReviewPanel`'s inline `Card`/`Button` style + the `setBusy/try/catch/finally` handler pattern. Sections: a global on/off toggle (off by default), a New-mandate form (target picker, source picker, recipe textarea) with inline validation + engine-rejection display, an active-mandate list (each with Revoke), and the Mandate-activity list (each with Undo, disabled when `undone`):

```tsx
import { useEffect, useState } from "react";
import { Card } from "../components/Card";
import { Button } from "../components/Button";
import {
  pickFolder, setMandatesEnabled, mandatesEnabled as readMandatesEnabled, addMandate, revokeMandate,
  listMandates, mandateWrites, undoApply,
  type MandateDto, type MandateWriteDto,
} from "../api/engine";
import { validateMandateForm } from "./mandateForm";
import { toMandateRow, toActivityRow } from "./mandateView";

/** How often the active list + activity list refresh while the Mandates tab is open. */
const POLL_MS = 5000;

export function MandatesPanel() {
  const [enabled, setEnabled] = useState(false);
  const [mandates, setMandates] = useState<MandateDto[]>([]);
  const [writes, setWrites] = useState<MandateWriteDto[]>([]);
  const [unavailable, setUnavailable] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // New-mandate form fields.
  const [target, setTarget] = useState("");
  const [sourceScope, setSourceScope] = useState("");
  const [recipe, setRecipe] = useState("");
  const [formError, setFormError] = useState<string | null>(null);

  const refresh = async () => {
    try {
      // SF5: read the persisted mandates flag too, so the toggle reflects an explicit "on" after
      // relaunch (write-then-reflect alone would show OFF until clicked). Failures hide the toggle
      // state as off (the list reads below set `unavailable`).
      const [on, ms, ws] = await Promise.all([readMandatesEnabled(), listMandates(), mandateWrites()]);
      setEnabled(on);
      setMandates(ms);
      setWrites(ws);
      setUnavailable(false);
    } catch {
      setUnavailable(true);
    }
  };

  useEffect(() => {
    void refresh();
    const id = setInterval(() => void refresh(), POLL_MS);
    return () => clearInterval(id);
  }, []);

  const onToggle = async (on: boolean) => {
    setBusy(true);
    setError(null);
    try {
      await setMandatesEnabled(on);
      await refresh(); // re-read the persisted flag so the displayed state always matches the engine.
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const onPickTarget = async () => {
    // The folder picker returns a directory; the user appends the file name in the field. (A
    // dedicated file picker is a fast-follow; for SP5 the path field is editable.)
    const dir = await pickFolder();
    if (dir) setTarget(dir.endsWith("/") ? dir : `${dir}/`);
  };
  const onPickScope = async () => {
    const dir = await pickFolder();
    if (dir) setSourceScope(dir);
  };

  const onCreate = async () => {
    const form = { target, sourceScope, recipe };
    const v = validateMandateForm(form);
    if (!v.ok) {
      setFormError(v.error);
      return;
    }
    setBusy(true);
    setFormError(null);
    try {
      await addMandate(target.trim(), sourceScope.trim(), recipe.trim());
      setTarget("");
      setSourceScope("");
      setRecipe("");
      await refresh();
    } catch (e) {
      // The engine's typed grant rejection stringifies to its bare reason (Rejected Display).
      setFormError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const onRevoke = async (id: string) => {
    setBusy(true);
    setError(null);
    try {
      await revokeMandate(id);
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const onUndo = async (fileWrittenId: string) => {
    setBusy(true);
    setError(null);
    try {
      // Undo reuses the SP4 engine undo (re-gated, hash-verified restore) — statically imported.
      await undoApply(fileWrittenId);
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  if (unavailable) {
    return (
      <Card>
        <h2 style={{ margin: 0 }}>Mandates</h2>
        <p style={{ color: "#666" }}>Couldn’t reach the memory engine. Set up your identity first.</p>
      </Card>
    );
  }

  return (
    <div>
      <h2 style={{ margin: "0 0 8px" }}>Mandates</h2>
      {error ? <p style={{ color: "#b00", fontSize: 13 }}>{error}</p> : null}

      <Card>
        <label style={{ display: "flex", gap: 8, alignItems: "center", fontSize: 14 }}>
          <input type="checkbox" checked={enabled} disabled={busy} onChange={(e) => void onToggle(e.target.checked)} />
          Mandates {enabled ? "on" : "off"} — when on, the brain keeps each mandate’s target file in
          sync and auto-applies clean changes (risky ones go to Review).
        </label>
      </Card>

      <Card>
        <div style={{ fontWeight: 600, marginBottom: 8 }}>New mandate</div>
        <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
          <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
            <input
              placeholder="Target file (in an edit-enabled folder)"
              value={target}
              onChange={(e) => setTarget(e.target.value)}
              style={{ flex: 1, padding: 6, fontFamily: "inherit", fontSize: 13 }}
            />
            <Button variant="secondary" disabled={busy} onClick={() => void onPickTarget()}>Pick folder…</Button>
          </div>
          <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
            <input
              placeholder="Source folder (read-granted)"
              value={sourceScope}
              onChange={(e) => setSourceScope(e.target.value)}
              style={{ flex: 1, padding: 6, fontFamily: "inherit", fontSize: 13 }}
            />
            <Button variant="secondary" disabled={busy} onClick={() => void onPickScope()}>Pick folder…</Button>
          </div>
          <textarea
            placeholder="Recipe: how to keep the target in sync from the sources"
            value={recipe}
            onChange={(e) => setRecipe(e.target.value)}
            rows={3}
            style={{ padding: 6, fontFamily: "inherit", fontSize: 13 }}
          />
          {formError ? <p style={{ color: "#b00", fontSize: 13, margin: 0 }}>{formError}</p> : null}
          <div>
            <Button variant="primary" disabled={busy} onClick={() => void onCreate()}>Create mandate</Button>
          </div>
        </div>
      </Card>

      <Card>
        <div style={{ fontWeight: 600, marginBottom: 8 }}>Active mandates</div>
        {mandates.length === 0 ? (
          <p style={{ color: "#666", fontSize: 13 }}>No mandates yet.</p>
        ) : (
          <ul style={{ paddingLeft: 18, fontSize: 13 }}>
            {mandates.map((m) => {
              const row = toMandateRow(m);
              return (
                <li key={row.id} style={{ marginBottom: 8 }}>
                  <div><code>{row.targetName}</code> <span style={{ color: "#666" }}>in {row.targetFolder}</span></div>
                  <div style={{ color: "#666", fontSize: 12 }}>from <code>{row.sourceScope}</code></div>
                  <div style={{ fontSize: 12 }}>Recipe: {row.recipe}</div>
                  <button disabled={busy} onClick={() => void onRevoke(row.id)} style={{ marginTop: 4 }}>Revoke</button>
                </li>
              );
            })}
          </ul>
        )}
      </Card>

      <Card>
        <div style={{ fontWeight: 600, marginBottom: 8 }}>Recent mandate activity</div>
        {writes.length === 0 ? (
          <p style={{ color: "#666", fontSize: 13 }}>No mandate changes yet.</p>
        ) : (
          <ul style={{ paddingLeft: 18, fontSize: 13 }}>
            {[...writes].reverse().map((w) => {
              const row = toActivityRow(w);
              return (
                <li key={row.fileWrittenId} style={{ marginBottom: 4 }}>
                  <code>{row.fileName}</code> <span style={{ color: "#666", fontSize: 12 }}>· {row.label}</span>{" "}
                  <button disabled={busy || !row.canUndo} onClick={() => void onUndo(row.fileWrittenId)} style={{ marginLeft: 8 }}>
                    Undo
                  </button>
                </li>
              );
            })}
          </ul>
        )}
      </Card>
    </div>
  );
}
```

  > The `mandateWrites()` op returns rows oldest-first (engine `seq ASC`); the panel reverses for newest-first display (`[...writes].reverse()`). The `onToggle` sets local `enabled` optimistically after the op succeeds — the engine flag is sticky, so a refresh of the toggle state on mount is a fast-follow (the toggle is a write-then-reflect, matching the off-by-default contract).

- [ ] Add the `"mandates"` view + a `MandatesNavButton` + the body-ternary arm in `apps/desktop/src/App.tsx`. Edit the `View` type (line 35) to insert `"mandates"`:

```ts
type View = "identity" | "inbox" | "memory" | "review" | "mandates" | "settings";
```

  Add the panel import near the `ReviewPanel` import (App.tsx line 13):

```tsx
import { MandatesPanel } from "./mandates/MandatesPanel";
```

  Insert the nav button in the `<nav>` block (App.tsx lines 48-54), between Review and Settings:

```tsx
        <ReviewNavButton active={view === "review"} onClick={() => setView("review")} />
        <Button variant={view === "mandates" ? "primary" : "secondary"} onClick={() => setView("mandates")}>Mandates</Button>
        <Button variant={view === "settings" ? "primary" : "secondary"} onClick={() => setView("settings")}>Settings</Button>
```

  Extend the body ternary (App.tsx line 55) to add the mandates arm before the settings fallback:

```tsx
      {view === "identity" ? <IdentityPanel /> : view === "inbox" ? <InboxPanel /> : view === "memory" ? <MemoryPanel /> : view === "review" ? <ReviewPanel /> : view === "mandates" ? <MandatesPanel /> : <AirSettings />}
```

  > A plain `Button` (not a polling badge) is used for Mandates — there is no pending-count to badge (the Mandates destination is not a queue). The risky mandate proposals still surface their count on the existing Review badge.

- [ ] Typecheck:
  `npm run typecheck --workspace @air-agent/desktop`
  Expected output: no errors.

- [ ] Run the full desktop vitest suite (the proposalView fixture change + the new mandate helpers + existing tests all green):
  `npm run test --workspace @air-agent/desktop`
  Expected output: all test files pass (incl. `mandateForm`, `mandateView`, `proposalView`, `diffView`, `applyFlow`).

- [ ] Commit:
  `git add apps/desktop/src/mandates/MandatesPanel.tsx apps/desktop/src/App.tsx apps/desktop/src/review/proposalView.ts apps/desktop/src/review/proposalView.test.ts apps/desktop/src/review/ReviewPanel.tsx`
  `git commit -m "$(cat <<'EOF'
feat(desktop): Mandates destination — toggle, New-mandate form, active list, activity+Undo

MandatesPanel mirrors the Review surface (inline Card/Button + busy/try/catch pattern): global
on/off (off by default), a New-mandate form with client-side validation + engine-rejection
display, an active-mandate list with Revoke, and the mandate-activity list with Undo (disabled
when undone). App.tsx gains the "mandates" View + nav. Review labels an m6c rewrite "from a mandate".

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"`

---

### Task 15: Gates + manual-launch checklist (no code)

**Files:** none (verification-only task).

- [ ] Engine build + tests green:
  `cargo test -p bossclaw-core`
  Expected output: `test result: ok.` with 0 failed.

- [ ] Engine clippy (the security-feature lint gate — the SP-wide standard):
  `cargo clippy -p bossclaw-core --features ollama -- -D warnings`
  Expected output: `Finished` with no warnings (no clippy errors).

- [ ] Desktop build + tests + clippy green:
  `cargo build -p air_agent_desktop && cargo test -p air_agent_desktop && cargo clippy -p air_agent_desktop -- -D warnings`
  Expected output: `Finished`; `test result: ok.`; clippy clean.

- [ ] Frontend typecheck + vitest:
  `npm run typecheck --workspace @air-agent/desktop && npm run test --workspace @air-agent/desktop`
  Expected output: no type errors; all vitest files pass.

- [ ] Two-graph network-posture guard stays green (the SP1–SP4 invariant: embedder network-free; reasoner loopback-only; no new network surface in SP5 — SP5 adds no deps). Run the exact check CI uses (`.github/workflows/build.yml` "Engine network-posture guard (two-graph)") — neither grep may match:
  ```bash
  cargo tree -p bossclaw-core -e normal --prefix none | grep -qE '^(hf-hub|ureq|reqwest)( |$)' && echo "FAIL: network crate in DEFAULT graph" || echo "default graph OK (zero network clients)"
  cargo tree -p bossclaw-core -e normal --features ollama --prefix none | grep -qE '^(hf-hub|reqwest)( |$)' && echo "FAIL: hf-hub/reqwest in ollama graph" || echo "ollama graph OK (ureq-only)"
  ```
  Expected output: `default graph OK (zero network clients)` and `ollama graph OK (ureq-only)` — neither grep matches.

- [ ] `cargo audit` both crates (the design introduces no new deps; this confirms it):
  `cargo audit`
  Expected output: no new advisories attributable to SP5 (the workspace lockfile is unchanged by this work).

- [ ] Manual launch (signed debug build per `scripts/dev-build-signed.sh`; fixtures dir e.g. `~/air-note-qa`, identity "Aria Novak"):
  - [ ] Mandates tab → flip **Mandates on** → confirm the toggle sticks.
  - [ ] New mandate: pick a **target** file in a *write-granted* (edit-enabled) folder + a **source** *read-granted* folder + a recipe → Create → confirm it appears in **Active mandates**. Try an invalid grant (e.g. a target NOT under a write-grant) → confirm the form shows the engine's *why* (the typed `Rejected` reason).
  - [ ] With Ollama up + evolve on, make the source drift so the recipe implies a **clean** rewrite of the target → wait a tick (~5 min) → confirm the target file **auto-applies** (changes on disk with no per-change confirm) AND appears in **Recent mandate activity** → click **Undo** → confirm the file is restored.
  - [ ] Drop a **secret-shaped** or **out-of-scope** source so the next rewrite is risky → confirm that rewrite **parks in Review** (labeled "from a mandate"), is NOT auto-applied, and Approve there still requires the "I’ve reviewed this" confirm.
  - [ ] **Revoke** the mandate → confirm no further auto-writes happen (a subsequent drift produces nothing, or a queued risky one that you can decline).
  - [ ] **Relaunch** the app → confirm the Mandates toggle **shows on** (the engine flag persists via the `prime_switches` fix — Task 5 — AND the panel reads it on mount via `mandatesEnabled()` — SF5/Task 14 — so it is not falsely OFF until clicked) and the active mandate + activity list survive.
  - [ ] EXPECTED (not a bug): an M6b reconcile (contradiction) proposal still requires human approval in Review and is **never** auto-applied — only M6c mandate proposals sweep.

- [ ] Commit (checklist completion / any doc note only — no source changes expected here):
  `git add docs/superpowers/plans/2026-06-24-sp5-mandate-management.md`
  `git commit -m "$(cat <<'EOF'
docs(sp5): record SP5 mandate-management manual-QA checklist completion

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"`

---

## Decisions honored

- **Auto-apply clean / queue risky** — the sweep (Task 11) applies M6c proposals with `acknowledged_loud=false`; the engine loud-gate (Task 2) auto-applies only genuinely-clean writes and refuses loud ones (left queued). "Clean" is made reachable by the trust rule (Task 1).
- **Polled ~5 min, reuse the evolve scheduler** — the sweep runs right after `evolve_once` in `scheduler::spawn` (Task 11); `watch.rs` stays unwired.
- **Approach A, app-driven** — the auto-apply action lives in the desktop scheduler; the engine never auto-writes (it emits proposals + fails safe; the loud-gate is enforced for every caller).
- **Persistent Mandate-activity + Undo (mandatory)** — `mandate_writes()` join (Task 4) → desktop op (Task 10) → activity list (Task 14) with Undo via SP4 `undo_write`.
- **Global Mandates on/off, off by default, explicit + sticky** — `set_mandates_enabled` (Task 6) + the `prime_switches` persistence fix (Task 5).
- **Grant-time guards honored, rejections surfaced** — `add_mandate` maps `InvalidInput` to the typed `Rejected` (Task 7); the form validates client-side + shows the engine reason (Tasks 13/14).
- **(c) Mandate-authorized sources don't taint that mandate's target** — Task 1 (deferred escalation, segment-aware, fail-closed, scoped; with the M6b non-leak proof).
- **(d) Loud-gate is an engine invariant** — Task 2 (`execute_write_inner` + all three callers; undo exemption).
- **(a) producer surfaced; (b) mandate_writes attribution** — Tasks 3/4 (engine) + Tasks 8/10 (desktop).
- **Create-apply via the engine's atomic no-clobber, no desktop pre-check** — Task 9.
- **Risky path reuses the SP4 Review queue** — labeled "from a mandate" (Task 14).
- **Per-element `#[cfg(unix)]` in `generate_handler!`** — Tasks 6/7/10.
- **No new events, no new network surface, no new deps** — confirmed by the gates (Task 15).
