# SP4 — Confirm & Apply — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** After a user enables a folder for edits, the brain proposes file rewrites on confirmed contradictions, and the user reviews each one (inline diff + "Why") and Approves (atomic, undoable, audited) or Declines (final) in a dedicated Review destination.

**Architecture:** Three surgical `bossclaw-core` engine changes (skip-on-no-write-grant at reconcile, `prime_switches` respects explicit user choices, a new `pending_proposals()` projection) land first. The `air_agent_desktop` Tauri crate then adds `EngineHandle` ops + `#[cfg(unix)]` commands + DTOs that funnel through the existing `get_or_open` chokepoint and re-gate every apply against the live file. The React UI adds a layout-agnostic "Review" destination with a pending badge, an inline unified diff, Approve/Decline + loud-confirm, an Undo strip, and a per-folder "Allow edits" toggle in Settings → Folders.

**Tech Stack:** Rust (bossclaw-core engine, air_agent_desktop Tauri crate), TypeScript/React (desktop UI), vitest, cargo test. All new Rust + UI engine-calls are `#[cfg(unix)]`-gated.

---

## File structure

| File | Create/Modify | One responsibility |
|---|---|---|
| `crates/bossclaw-core/src/log.rs` | Modify | Add `#[cfg(unix)]` `PendingProposal` struct (+ `requires_loud_modal()` helper) + `#[cfg(unix)]` `pending_proposals()` (pub); add typed `ConfigFlag` enum + `explicitly_set(flag)` (pub) predicate; hoist the write-grant skip in `reconcile_confirmed_contradiction` + record `base_content_hash` in the proposal `verdict_summary`. |
| `crates/bossclaw-core/src/lib.rs` | Modify | Re-export `ConfigFlag` (unconditional, line 61) + `#[cfg(unix)] pub use log::PendingProposal;` (mirrors the gated `MandateAction`). |
| `crates/bossclaw-core/tests/reconcile.rs` | Modify | Add `pending_proposals_*` test; add `explicitly_set_*` test (typed `ConfigFlag`); rewrite + rename `reconcile_target_outside_write_grant_rejected_at_propose` → `..._skipped_at_propose` (assert SKIP, no `write_rejected`, + grant-then-propose follow-on). |
| `apps/desktop/src-tauri/src/engine/mod.rs` | Modify | Change `prime_switches` to force-off only when not explicitly set (typed `ConfigFlag`; mandates always off); add `ProposalSummary`/`PreviewData`/`ApplyResult` types + ops (`set_folder_writable`, `list_writable`, `set_proposals_enabled`, `list_proposals`, `proposal_preview`, `apply_proposal(…, acknowledged_loud)`, `decline_proposal`, `undo_apply`); add typed `Stale`/`Revoked`/`NeedsLoudConfirm` variants to `EngineOpError`; hermetic tests for each (incl. loud-confirm + fail-loud). |
| `apps/desktop/src-tauri/src/commands/engine.rs` | Modify | Add `ProposalDto`/`PreviewDto`/`ApplyResultDto`, extend `FileRecordDto` with `writable`; add the 8 new `#[tauri::command]`s (`engine_apply_proposal` takes `acknowledged_loud`); add a command-layer IPC arg-binding test. |
| `apps/desktop/src-tauri/Cargo.toml` | Modify | Add `tauri = { version = "2", features = ["test"] }` to `[dev-dependencies]` (for the command-layer IPC test). |
| `apps/desktop/src-tauri/src/main.rs` | Modify | Register each new command with a per-element `#[cfg(unix)]` line in `generate_handler!`. |
| `apps/desktop/src/api/engine.ts` | Modify | TS twin types + `invoke<T>` wrappers for every new command; extend `FileRecordDto` with `writable`. |
| `apps/desktop/src/review/diffView.ts` | Create | Pure: compute inline unified diff lines (old vs new). |
| `apps/desktop/src/review/diffView.test.ts` | Create | vitest for `diffView`. |
| `apps/desktop/src/review/proposalView.ts` | Create | Pure: map a `ProposalDto` to a display row. |
| `apps/desktop/src/review/proposalView.test.ts` | Create | vitest for `proposalView`. |
| `apps/desktop/src/review/ReviewPanel.tsx` | Create | Review queue + per-proposal card (inline diff, Approve/Decline, op-enforced loud-confirm modal, Recently-applied Undo strip). |
| `apps/desktop/src/review/applyFlow.ts` | Create | Pure: classify apply errors (loud/stale) + the approve decision the panel uses (so the ack flow is unit-tested). |
| `apps/desktop/src/review/applyFlow.test.ts` | Create | vitest for `applyFlow` (a loud proposal cannot apply without the ack). |
| `apps/desktop/src/App.tsx` | Modify | `View += "review"`; `ReviewNavButton` (pending-count badge); body-ternary branch. |
| `apps/desktop/src/sources/SourcesPanel.tsx` | Modify | Per-folder "Allow edits" toggle + "Allow All" master; evolve-off inline offer. |
| `apps/desktop/src/sources/writableGrants.ts` | Create | Pure: derive which active grants are write-enabled (from files' `writable`). |
| `apps/desktop/src/sources/writableGrants.test.ts` | Create | vitest for `writableGrants`. |

---

### Task 1: `pending_proposals()` projection (engine, `pub`)

**Files:**
- Modify: `crates/bossclaw-core/src/log.rs`
- Test: `crates/bossclaw-core/tests/reconcile.rs`

- [ ] Write the failing test in `crates/bossclaw-core/tests/reconcile.rs` (append after `pending_projection_open_close_and_suppress`, ~line 297). It mirrors that test's open/close/suppress lifecycle but asserts the returned rows of the new projection:

```rust
#[test]
fn pending_proposals_lists_open_then_excludes_resolved_and_rejected() {
    let (log, _home, dir) = common::open_write_grant_and_external_target();
    let path = dir.join("n.md");
    let canonical = std::fs::canonicalize(&path).unwrap().to_string_lossy().to_string();
    let key = serde_json::json!({"src":"entity:a","relation":"rel","dst":"entity:b"});

    assert!(log.pending_proposals().unwrap().is_empty(), "nothing yet → no open proposals");

    let pid = common::append_minimal_proposal(&log, &canonical, &key);
    let open = log.pending_proposals().unwrap();
    assert_eq!(open.len(), 1, "one OPEN proposal is listed");
    let row = &open[0];
    assert_eq!(row.id, pid);
    assert_eq!(row.target, canonical);
    assert_eq!(row.op, "edit");
    assert_eq!(row.new_content_hash, "deadbeef");
    assert_eq!(row.rationale, "rationale");
    assert_eq!(row.inducing_key, key);
    assert!(!row.source_event_ids.is_empty(), "lineage carried from model_meta");
    // `append_minimal_proposal` passes an empty verdict_summary `{}`, so there is no base hash;
    // the real emit path (Task 2) records it. Absence ⇒ None (apply then re-reads + re-gates).
    assert_eq!(row.base_content_hash, None, "minimal proposal carries no base fingerprint");

    log.decline_write_proposal(&pid, "not now").unwrap();
    assert!(log.pending_proposals().unwrap().is_empty(), "declined → no longer open");

    // A write_rejected on a DIFFERENT (path,key) must not resurface the declined one,
    // and a rejected proposal is never listed as open.
    let key2 = serde_json::json!({"src":"entity:c","relation":"rel","dst":"entity:d"});
    common::append_rejected(&log, &canonical, &key2, "stale_target");
    assert!(log.pending_proposals().unwrap().is_empty(), "rejected (path,key) is not open");
}
```

- [ ] Run it (expect FAIL — `pending_proposals`/`PendingProposal` do not exist):
  `cargo test -p bossclaw-core --test reconcile pending_proposals_lists_open_then_excludes_resolved_and_rejected`
  Expected output: a compile error `no method named pending_proposals found for ... EventLog` (or `cannot find ... PendingProposal`).

- [ ] Implement in `crates/bossclaw-core/src/log.rs`. Add the struct just above the `impl EventLog` block that contains `is_proposal_suppressed` (so it is module-public), and the method inside that `impl`. The fold mirrors `is_proposal_suppressed` (log.rs:2226-2267) but returns rows. `events_of_types` is `pub(crate)`, callable here; the method itself is `pub`:

```rust
/// One open (unresolved, non-terminally-rejected) `write_proposal`, projected for callers
/// outside the crate (e.g. the desktop Review queue). Mirrors the per-proposal fields of
/// `append_write_proposal_with` (`content`) plus the lineage off `model_meta`.
/// `#[cfg(unix)]` like the confirm/apply API it feeds (M1).
#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingProposal {
    /// The proposal event id (the ULID).
    pub id: String,
    /// Canonical target path (`content["target"]`).
    pub target: String,
    /// `"edit"` / `"create"` / `"delete"` (`content["op"]`; the M6b reconciler emits `"edit"`).
    pub op: String,
    /// Hex sha256 of the proposed bytes (`content["new_content_hash"]`).
    pub new_content_hash: String,
    /// Plain-English "Why" (`content["rationale"]`).
    pub rationale: String,
    /// The resolved contradiction `{src, relation, dst}` (`content["inducing_key"]`).
    pub inducing_key: serde_json::Value,
    /// Lineage event ids (`model_meta.source_event_ids`); empty if absent.
    pub source_event_ids: Vec<String>,
    /// The propose-time verdict summary `{requires_loud_modal, taint, allowed, base_content_hash}`
    /// (`content["verdict_summary"]`).
    pub verdict_summary: serde_json::Value,
    /// Hex sha256 of the target file's bytes AT PROPOSE TIME
    /// (`content["verdict_summary"]["base_content_hash"]`; `None` for a Create). The anti-clobber
    /// fingerprint: apply fails closed if the live file no longer hashes to this.
    pub base_content_hash: Option<String>,
}

#[cfg(unix)]
impl PendingProposal {
    /// Single-sourced fail-loud default for a proposal's loud-modal hint: an absent or garbled
    /// `verdict_summary["requires_loud_modal"]` defaults to `true` (fail-loud). Used by both
    /// `ProposalSummary::from_pending` (Task 6) and `proposal_preview` (Task 7) so they cannot drift (m2).
    pub fn requires_loud_modal(&self) -> bool {
        self.verdict_summary
            .get("requires_loud_modal")
            .and_then(|v| v.as_bool())
            .unwrap_or(true)
    }
}

impl EventLog {
    /// Every OPEN `write_proposal`: emitted, not yet resolved by a `file_written`/`write_declined`,
    /// and whose `(target, inducing_key)` is not terminally `write_rejected`. Oldest first
    /// (`events_of_types` returns `seq ASC`). The desktop Review queue source. `#[cfg(unix)]` (M1).
    #[cfg(unix)]
    pub fn pending_proposals(&self) -> Result<Vec<PendingProposal>, BossclawError> {
        use std::collections::{HashMap, HashSet};
        // proposal id → parsed row, in emission order.
        let mut open: Vec<PendingProposal> = Vec::new();
        let mut resolved: HashSet<String> = HashSet::new();
        // (target, inducing_key.to_string()) terminally rejected.
        let mut rejected_keys: HashSet<(String, String)> = HashSet::new();
        let mut proposal_keys: HashMap<String, (String, String)> = HashMap::new();

        for ev in self.events_of_types(&[
            crate::graph::WRITE_PROPOSAL_EVENT_TYPE,
            crate::graph::WRITE_REJECTED_EVENT_TYPE,
            crate::graph::WRITE_DECLINED_EVENT_TYPE,
            crate::graph::FILE_WRITTEN_EVENT_TYPE,
        ])? {
            match ev.event_type.as_str() {
                t if t == crate::graph::WRITE_PROPOSAL_EVENT_TYPE => {
                    // Read ALL fields via borrows of `ev.content` / `ev.model_meta` FIRST, then
                    // build the struct — never move `ev.model_meta` before reading `ev.content`
                    // (that would be a move-after-use, E0382).
                    let id = ev.id.clone();
                    let target = ev.content.get("target").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let op = ev.content.get("op").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let new_content_hash = ev.content.get("new_content_hash").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let rationale = ev.content.get("rationale").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let inducing_key = ev.content.get("inducing_key").cloned().unwrap_or(serde_json::Value::Null);
                    let verdict_summary = ev.content.get("verdict_summary").cloned().unwrap_or(serde_json::Value::Null);
                    let base_content_hash = verdict_summary.get("base_content_hash")
                        .and_then(|v| v.as_str()).map(|s| s.to_string());
                    let source_event_ids = ev.model_meta.as_ref()
                        .map(|m| m.source_event_ids.clone()).unwrap_or_default();
                    proposal_keys.insert(id.clone(), (target.clone(), inducing_key.to_string()));
                    open.push(PendingProposal {
                        id, target, op, new_content_hash, rationale,
                        inducing_key, source_event_ids, base_content_hash, verdict_summary,
                    });
                }
                t if t == crate::graph::WRITE_REJECTED_EVENT_TYPE => {
                    let target = ev.content.get("target").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let key = ev.content.get("inducing_key").cloned().unwrap_or(serde_json::Value::Null);
                    rejected_keys.insert((target, key.to_string()));
                }
                _ => {
                    if let Some(rid) = ev.content.get("resolves_proposal").and_then(|v| v.as_str()) {
                        resolved.insert(rid.to_string());
                    }
                }
            }
        }

        Ok(open
            .into_iter()
            .filter(|p| {
                !resolved.contains(&p.id)
                    && match proposal_keys.get(&p.id) {
                        Some(k) => !rejected_keys.contains(k),
                        None => true,
                    }
            })
            .collect())
    }
}
```

- [ ] Run it (expect PASS): `cargo test -p bossclaw-core --test reconcile pending_proposals_lists_open_then_excludes_resolved_and_rejected`
  Expected output: `test result: ok. 1 passed`.

- [ ] Commit:
  `git add crates/bossclaw-core/src/log.rs crates/bossclaw-core/tests/reconcile.rs`
  `git commit -m "feat(bossclaw-core): add pending_proposals() projection for the Review queue"`

---

### Task 2: Reconcile skip-on-no-write-grant (engine change a)

**Files:**
- Modify: `crates/bossclaw-core/src/log.rs`
- Test: `crates/bossclaw-core/tests/reconcile.rs`

- [ ] Rewrite the existing test (grounding §6d, named `reconcile_target_outside_write_grant_rejected_at_propose`, reconcile.rs:666) to assert the new SKIP behavior, AND **rename it** to `reconcile_target_outside_write_grant_skipped_at_propose` (item 18 — it now asserts a skip, not a reject), adding a grant-then-propose follow-on. The setup through `revoke_write_grant` is unchanged from grounding §6d; the assertion tail flips:

```rust
#[test]
fn reconcile_target_outside_write_grant_skipped_at_propose() {
    let (log, _home, dir) = common::open_log_with_write_grant();
    let emb = MockEmbedder::new(64);

    // ── Tick 1: the FILE establishes "Alice works_at Acme". ──
    let (_file_id, file_src) =
        ingest_md_full(&log, &emb, &dir, "notes.md", b"Alice works at Acme.\n");
    let canonical = std::fs::canonicalize(dir.join("notes.md"))
        .unwrap().to_string_lossy().to_string();
    let r1 = DispatchReasoner::new(add_both_passes(
        ScriptedReasoner::new("m6b-test"),
        &file_src, &[vec![]], &[],
        works_at_pass_a("Alice", "Acme", &file_src),
    ));
    let rep1 = log.evolve_once(&emb, &r1).unwrap();
    assert!(rep1.links_emitted >= 1, "the file established the works_at edge");
    log.rebuild_graph().unwrap();

    // Resolve Alice/Acme to ids — the reconcile builds `inducing_key` from these resolved
    // entity ids, so the anti-poison check below must query suppression with the same key.
    let entities = log.all_entities().unwrap();
    let alice = entities.iter().find(|e| e.label == "Alice").unwrap().entity_id.clone();
    let acme = entities.iter().find(|e| e.label == "Acme").unwrap().entity_id.clone();

    // ── Tick 2: a memory corrects the employer → confirmed contradiction. ──
    let corr = "Correction: Alice works at Globex, not Acme.";
    let _mem_id = seed_memory_full(&log, &emb, corr);
    let nbh = vec!["Alice -works_at-> Acme".to_string()];
    let r2 = DispatchReasoner::new(add_both_passes(
        ScriptedReasoner::new("m6b-test"),
        corr, &[vec![], vec![file_src.clone()]], &nbh,
        correction_pass_a("Alice", "Acme", "Globex", corr),
    ));

    // Revoke the target dir's WRITE grant before evolve (read grant untouched).
    log.revoke_write_grant(&dir).unwrap();

    let rep2 = log.evolve_once(&emb, &r2).unwrap();

    // SP4 change-(a): an un-writable target is SKIPPED, not rejected — no LLM, no propose,
    // no write_rejected. The contradiction is still confirmed.
    assert!(rep2.invalidates_emitted >= 1, "the contradiction is still confirmed");
    assert_eq!(rep2.proposals_emitted, 0, "no proposal for a non-write-granted folder");
    assert_eq!(rep2.proposals_rejected, 0, "skipped, NOT rejected — no permanent dead state");
    assert_eq!(proposals_targeting(&log, &canonical), 0, "no write_proposal leaked");
    assert_eq!(file_written_count(&log), 0, "no file_written event is produced");
    assert_eq!(
        std::fs::read(dir.join("notes.md")).unwrap(),
        b"Alice works at Acme.\n".to_vec(),
        "the file on disk is untouched",
    );

    // Anti-poison: a skip records NO terminal `write_rejected`, so the (target, inducing_key)
    // is NOT suppressed — the proposal stays retryable on a later tick. (The OLD reject path
    // WOULD have suppressed it permanently; not poisoning the target is the change-(a) win.)
    // The contradiction is confirm-once (the invalidate consumes the active edge), so the way
    // to re-surface a proposal is a FRESH contradiction — never a replay of THIS one (which
    // cannot re-confirm); asserting non-suppression captures the durable property directly.
    let inducing_key = serde_json::json!({ "src": alice, "relation": "works_at", "dst": acme });
    assert!(
        !log.is_proposal_suppressed(&canonical, &inducing_key).unwrap(),
        "skip must not terminally suppress the target+key (no write_rejected recorded)",
    );
}
```

- [ ] Run it (expect FAIL — current code rejects, so `proposals_rejected == 0` fails):
  `cargo test -p bossclaw-core --test reconcile reconcile_target_outside_write_grant_skipped_at_propose`
  Expected output: assertion failure on `proposals_rejected` (left `1`, right `0`) — the old code records a `write_rejected`.

- [ ] Implement the hoist in `crates/bossclaw-core/src/log.rs` inside `reconcile_confirmed_contradiction` (log.rs:6140-6281). Add the check at the TOP of the per-target loop, right after the `seen_paths` guard (after grounding line 6169, before step `a`). Insert exactly:

```rust
            // SP4 change-(a): be smart — check the write-grant FIRST. An ingested-but-not-
            // -writable target is SKIPPED (no LLM, no propose, no write_rejected) so the
            // folder stays clean and re-enabling it later starts fresh. We use
            // `is_write_allowed` (not `gate_reject_reason`, which folds `!allowed` into the
            // genuine-reject set) so the pure no-grant case never records terminal dead state.
            if !self.is_write_allowed(std::path::Path::new(&rec.canonical_path))? {
                continue;
            }
```

  Keep the existing step `i` exactly as-is, but change its reject discriminator from the folded `gate_reject_reason()` to the genuine-only `reject_reason`, so a not-granted target (which cannot reach here anymore, but defends against a TOCTOU revoke between the hoisted check and `propose_write`) is skipped rather than rejected. Replace the step `i` block:

```rust
            // i. A GENUINE gate failure (symlink/taint/op×existence) → write_rejected.
            //    `reject_reason` (NOT `gate_reject_reason`) is the genuine-reject signal:
            //    a bare `!allowed` (grant revoked between the hoisted check above and here)
            //    is skipped, never recorded as terminal dead state.
            if let Some(reason) = gated.verdict.reject_reason.as_deref() {
                self.append_write_rejected(
                    Some(&rec.canonical_path),
                    reason,
                    &inducing_key,
                    &lineage,
                )?;
                report.proposals_rejected += 1;
                continue;
            }
            if !gated.verdict.allowed {
                // Grant vanished mid-tick — skip (retryable), do not reject.
                continue;
            }
```

- [ ] Persist the proposal's base fingerprint for the apply-time anti-clobber check. In the SAME fn, step `j` builds the `verdict_summary` JSON immediately before `append_write_proposal` (grounding §4, log.rs ~6260-6264). Add ONE field — `base_content_hash` from the gate's already-computed `gated.verdict.base_content_hash` (`Option<String>`, `Some` for an Edit; grounding §4 `WriteVerdict`). NO signature change: it rides inside the `verdict_json` that `append_write_proposal` already takes. Replace the step `j` `verdict_summary` literal:

```rust
            // j. Record the gated proposal + its bytes (the worklist side table). The
            //    verdict_summary also carries the base fingerprint (`base_content_hash`) so the
            //    desktop apply can fail closed if the file diverged since this propose
            //    (a fresh re-propose at apply re-bases on LIVE bytes and cannot see the drift).
            let verdict_summary = serde_json::json!({
                "requires_loud_modal": gated.verdict.requires_loud_modal,
                "taint": format!("{:?}", gated.verdict.taint),
                "allowed": gated.verdict.allowed,
                "base_content_hash": gated.verdict.base_content_hash,
            });
```

- [ ] Run it (expect PASS): `cargo test -p bossclaw-core --test reconcile reconcile_target_outside_write_grant_skipped_at_propose`
  Expected output: `test result: ok. 1 passed`.

- [ ] Run the full reconcile suite to confirm no regression (the round-trip + suppress tests must stay green):
  `cargo test -p bossclaw-core --test reconcile`
  Expected output: `test result: ok.` with all tests passing.

- [ ] Commit:
  `git add crates/bossclaw-core/src/log.rs crates/bossclaw-core/tests/reconcile.rs`
  `git commit -m "fix(bossclaw-core): skip (not reject) reconcile targets outside a write grant"`

---

### Task 3: `prime_switches` persistence + `explicitly_set` predicate (engine change b)

**Files:**
- Modify: `crates/bossclaw-core/src/log.rs` (add `explicitly_set`)
- Test: `crates/bossclaw-core/tests/reconcile.rs` (engine predicate test)
- Modify: `apps/desktop/src-tauri/src/engine/mod.rs` (`prime_switches`)
- Test: `apps/desktop/src-tauri/src/engine/mod.rs` (tests mod: persistence test)

- [ ] Write the failing engine-predicate test in `crates/bossclaw-core/tests/reconcile.rs` (append after the Task 1 test). It needs a bare `EventLog`; reuse `common::open_log_with_write_grant` (returns `(log, home, dir)`):

```rust
#[test]
fn explicitly_set_distinguishes_default_from_user_choice() {
    use bossclaw_core::ConfigFlag;
    let (log, _home, _dir) = common::open_log_with_write_grant();

    // Never set → not explicit (even though the getter defaults to true).
    assert!(log.proposals_enabled().unwrap(), "getter default-open");
    assert!(!log.explicitly_set(ConfigFlag::Proposals).unwrap(), "never set → not explicit");

    // Explicit set flips false → true.
    log.set_proposals_enabled(true).unwrap();
    assert!(log.explicitly_set(ConfigFlag::Proposals).unwrap(), "an explicit flip is detected");

    // A DIFFERENT flag's flip does not mark Proposals explicit.
    let (log2, _home2, _dir2) = common::open_log_with_write_grant();
    log2.set_evolve_enabled(true).unwrap();
    assert!(!log2.explicitly_set(ConfigFlag::Proposals).unwrap(), "another flag's event is ignored");
    assert!(log2.explicitly_set(ConfigFlag::Evolve).unwrap(), "the flipped flag is explicit");
}
```

- [ ] Run it (expect FAIL — `ConfigFlag` / `explicitly_set` do not exist):
  `cargo test -p bossclaw-core --test reconcile explicitly_set_distinguishes_default_from_user_choice`
  Expected output: compile error `cannot find type ConfigFlag` / `no method named explicitly_set found`.

- [ ] Implement a typed `ConfigFlag` enum + `explicitly_set(flag)` in `crates/bossclaw-core/src/log.rs` (M2/MIN-4 — a typed identifier so a KEY-const rename can't silently re-break change-(b); the enum maps to the private `*_KEY` consts, so a rename is compile-checked). Add the enum near the KEY consts (log.rs:166-189) and the method in the same `impl EventLog` block as the flag getters (~log.rs:4879):

```rust
/// A typed identifier for the three autonomy config flags, mapping to the private `*_KEY`
/// consts. Used by `EventLog::explicitly_set` so callers (e.g. the desktop `prime_switches`)
/// reference a compile-checked variant instead of a stringly-typed key that could drift on a
/// rename (M2). `#[cfg(unix)]` is unnecessary — config flags exist on all platforms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFlag {
    Evolve,
    Proposals,
    Mandates,
}

impl ConfigFlag {
    /// The private content-key const this flag is stored under.
    fn key(self) -> &'static str {
        match self {
            ConfigFlag::Evolve => EVOLVE_ENABLED_KEY,
            ConfigFlag::Proposals => PROPOSALS_ENABLED_KEY,
            ConfigFlag::Mandates => MANDATES_ENABLED_KEY,
        }
    }
}
```

  And the method:

```rust
    /// Was a config flag ever EXPLICITLY set (regardless of its value)? Scans `config` events for
    /// a bool under the flag's key, returning true on the first hit. Distinguishes the engine's
    /// never-set default-open from a user's explicit choice — the input `prime_switches` needs to
    /// avoid clobbering a user `true` on every launch (SP4 change-b). A typed `ConfigFlag` (not a
    /// raw key string) keeps the const single-sourced (M2/MIN-4).
    pub fn explicitly_set(&self, flag: ConfigFlag) -> Result<bool, BossclawError> {
        let key = flag.key();
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT payload FROM events WHERE event_type = ?1 ORDER BY seq DESC",
        )?;
        let rows = stmt.query_map([CONFIG_EVENT_TYPE], |r| r.get::<_, String>(0))?;
        for row in rows {
            let ev: Event = serde_json::from_str(&row?)?;
            if ev.content.get(key).and_then(|v| v.as_bool()).is_some() {
                return Ok(true); // some config event carries this key as a bool ⇒ explicit
            }
        }
        Ok(false)
    }
```

  Re-export `ConfigFlag` from the crate root in `crates/bossclaw-core/src/lib.rs` — add it to the unconditional line-61 list (config flags are all-platform, unlike the `#[cfg(unix)]` `PendingProposal`): `pub use log::{ActiveModel, ConfigFlag, EventLog, ReembedStats, SynthCacheRow, SCHEMA_VERSION};`.

- [ ] Run it (expect PASS): `cargo test -p bossclaw-core --test reconcile explicitly_set_distinguishes_default_from_user_choice`
  Expected output: `test result: ok. 1 passed`.

- [ ] Commit the engine half:
  `git add crates/bossclaw-core/src/log.rs crates/bossclaw-core/src/lib.rs crates/bossclaw-core/tests/reconcile.rs`
  `git commit -m "feat(bossclaw-core): typed ConfigFlag + explicitly_set(flag) presence predicate"`

- [ ] Write the failing desktop persistence test in `apps/desktop/src-tauri/src/engine/mod.rs` (tests mod, after `first_open_forces_all_autonomy_switches_off` ~line 676). It sets `proposals_enabled = true`, drops the handle so a fresh `EngineHandle`/`EventLog` re-runs `prime_switches`, and asserts the explicit `true` survives while `mandates_enabled` stays forced off:

```rust
    #[tokio::test]
    async fn prime_switches_preserves_explicit_proposals_but_forces_mandates_off() {
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault.clone(), &dir);
        let log = handle.get_or_open(true).await.unwrap();
        // After first open everything is forced off (never-set defaults).
        assert!(!log.proposals_enabled().unwrap());
        assert!(!log.mandates_enabled().unwrap());

        // The user explicitly enables proposals.
        log.set_proposals_enabled(true).unwrap();
        assert!(log.proposals_enabled().unwrap());
        drop(log);

        // Re-open with a FRESH handle (same vault + db_path) → prime_switches runs again.
        let handle2 = new_test_handle(vault, &dir);
        let log2 = handle2.get_or_open(true).await.unwrap();
        assert!(log2.proposals_enabled().unwrap(), "an explicit user true MUST persist across opens");
        assert!(!log2.mandates_enabled().unwrap(), "mandates stay forced OFF until SP5");
    }
```

  Note: `new_test_handle` (grounding §7a) takes `&dir` and uses `dir.path()` as `data_dir`, so a second handle on the same `dir` + `vault` shares `brain.db` and the keystore. `vault` is `Arc<TestVault>` (cloneable).

- [ ] Run it (expect FAIL — current `prime_switches` resets the explicit true):
  `cargo test -p air_agent_desktop prime_switches_preserves_explicit_proposals_but_forces_mandates_off`
  Expected output: assertion failure `an explicit user true MUST persist across opens` (left `false`, right `true`).

- [ ] Implement the `prime_switches` change in `apps/desktop/src-tauri/src/engine/mod.rs` (lines 273-287). Replace the whole fn body so each persistable flag is forced off only when NOT explicitly set, and `mandates_enabled` is always forced off:

```rust
    /// Neutralize the engine's dangerous default-ON autonomy flags at startup, WITHOUT
    /// clobbering a user's explicit choice. `evolve`/`proposals` are forced off ONLY when the
    /// user never explicitly set them (`!explicitly_set`), so an explicit on/off persists across
    /// opens (SP4 change-b). `mandates_enabled` is ALWAYS forced off until SP5, regardless of any
    /// prior setting. Each setter is sticky; runs inside `get_or_open`'s first-open closure.
    fn prime_switches(log: &EventLog) -> Result<(), bossclaw_core::BossclawError> {
        use bossclaw_core::ConfigFlag;
        if !log.explicitly_set(ConfigFlag::Evolve)? && log.evolve_enabled()? {
            log.set_evolve_enabled(false)?;
        }
        if !log.explicitly_set(ConfigFlag::Proposals)? && log.proposals_enabled()? {
            log.set_proposals_enabled(false)?;
        }
        // SP5 not shipped: mandates stay forced OFF even if a prior build set them.
        if log.mandates_enabled()? {
            log.set_mandates_enabled(false)?;
        }
        Ok(())
    }
```

- [ ] Run it (expect PASS): `cargo test -p air_agent_desktop prime_switches_preserves_explicit_proposals_but_forces_mandates_off`
  Expected output: `test result: ok. 1 passed`.

- [ ] Confirm the existing `first_open_forces_all_autonomy_switches_off` still passes (never-set defaults still go off):
  `cargo test -p air_agent_desktop first_open_forces_all_autonomy_switches_off`
  Expected output: `test result: ok. 1 passed`.

- [ ] Commit the desktop half:
  `git add apps/desktop/src-tauri/src/engine/mod.rs`
  `git commit -m "fix(desktop): prime_switches respects explicit proposal/evolve choices; mandates stay off"`

---

### Task 4: Write-grant plumbing — `set_folder_writable` + `writable` on the files DTO

**Files:**
- Modify: `apps/desktop/src-tauri/src/engine/mod.rs` (op + a writable-files helper)
- Modify: `apps/desktop/src-tauri/src/commands/engine.rs` (command + `FileRecordDto.writable`)
- Modify: `apps/desktop/src-tauri/src/main.rs` (register command)
- Test: `apps/desktop/src-tauri/src/engine/mod.rs` (tests mod)

- [ ] Write the failing hermetic test in `apps/desktop/src-tauri/src/engine/mod.rs` (tests mod). It grants read on a dir, ingests nothing (uses `add_write_grant` directly via the op), and asserts the writable set reflects the toggle. Use the dir guard pattern from grounding §7a:

```rust
    #[tokio::test]
    async fn set_folder_writable_toggles_the_write_grant_and_list_writable_reflects_it() {
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        // A real folder to grant (must exist; add_write_grant canonicalizes + fails closed).
        let target = tempfile::tempdir().unwrap();
        let path = target.path().to_path_buf();
        let canonical = std::fs::canonicalize(&path).unwrap().to_string_lossy().to_string();

        // Not onboarded → gate.
        assert!(matches!(
            handle.set_folder_writable(false, path.clone(), true).await,
            Err(EngineOpError::Open(EngineError::NotOnboarded))
        ));

        // Enable → listed writable.
        handle.set_folder_writable(true, path.clone(), true).await.unwrap();
        let writable = handle.list_writable(true).await.unwrap();
        assert!(writable.contains(&canonical), "enabled root is listed writable");

        // Disable → not listed.
        handle.set_folder_writable(true, path.clone(), false).await.unwrap();
        let writable = handle.list_writable(true).await.unwrap();
        assert!(!writable.contains(&canonical), "revoked root drops from the writable list");
    }
```

- [ ] Run it (expect FAIL — `set_folder_writable`/`list_writable` do not exist):
  `cargo test -p air_agent_desktop set_folder_writable_toggles_the_write_grant_and_list_writable_reflects_it`
  Expected output: compile error `no method named set_folder_writable`.

- [ ] Implement both ops in `apps/desktop/src-tauri/src/engine/mod.rs` (add to the `impl EngineHandle` block, near `add_grant`/`revoke_grant` ~line 213). Mirror the mutate-with-PathBuf template (§2h) and the read template (§2g):

```rust
    /// Enable (`on=true` → `add_write_grant`) or disable (`on=false` → `revoke_write_grant`)
    /// edits for `path`. Lock 1 of two. Gated. The engine canonicalizes + fails closed on a
    /// missing path; execute re-checks the grant at write time regardless.
    pub async fn set_folder_writable(&self, onboarded: bool, path: PathBuf, on: bool) -> Result<(), EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        tokio::task::spawn_blocking(move || {
            let r = if on { log.add_write_grant(&path) } else { log.revoke_write_grant(&path) };
            r.map(|_| ()).map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// The canonical roots of every ACTIVE write-grant (revoked ones excluded). The UI uses
    /// this to mark folders + files writable. Gated.
    pub async fn list_writable(&self, onboarded: bool) -> Result<Vec<String>, EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        tokio::task::spawn_blocking(move || {
            let grants = log.write_grants().map_err(|e| EngineOpError::Core(e.to_string()))?;
            Ok(grants.into_iter().filter(|g| !g.revoked).map(|g| g.canonical_root).collect())
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }
```

- [ ] Run it (expect PASS): `cargo test -p air_agent_desktop set_folder_writable_toggles_the_write_grant_and_list_writable_reflects_it`
  Expected output: `test result: ok. 1 passed`.

- [ ] Add the command + DTO field in `apps/desktop/src-tauri/src/commands/engine.rs`. First extend `FileRecordDto` (grounding §1a, lines 30-47) — add `writable: bool` and stop using the `From` impl for it (a file's `writable` is computed against the write-grants, not on the record). Replace the `FileRecordDto` block:

```rust
#[derive(Serialize)]
pub struct FileRecordDto {
    pub canonical_path: String,
    pub file_event_id: String,
    pub content_hash: String,
    pub grant_root: String,
    /// True iff this file's `grant_root` is under an active write-grant (Lock 1).
    pub writable: bool,
}
impl FileRecordDto {
    /// Build from a record + the set of active writable roots (a file is writable iff its
    /// ingest grant_root is one of them — same root identity the write-grant uses).
    pub fn from_parts(f: bossclaw_core::graph::FileRecord, writable_roots: &std::collections::HashSet<String>) -> Self {
        let writable = writable_roots.contains(&f.grant_root);
        Self {
            canonical_path: f.canonical_path,
            file_event_id: f.file_event_id,
            content_hash: f.content_hash,
            grant_root: f.grant_root,
            writable,
        }
    }
}
```

  Update `engine_list_files` (grounding §1b, lines 131-136) to join in writable roots:

```rust
#[tauri::command]
pub async fn engine_list_files(state: State<'_, AppState>) -> Result<Vec<FileRecordDto>, String> {
    let onboarded = state.identity_store.is_onboarded();
    let files = state.engine.list_files(onboarded).await.map_err(|e| e.to_string())?;
    let writable_roots: std::collections::HashSet<String> =
        state.engine.list_writable(onboarded).await.map_err(|e| e.to_string())?.into_iter().collect();
    Ok(files.into_iter().map(|f| FileRecordDto::from_parts(f, &writable_roots)).collect())
}
```

  Add the new command (after `engine_revoke_grant`, ~line 115), mirroring the mutate-with-path template (§1d):

```rust
#[tauri::command]
pub async fn engine_set_folder_writable(path: String, on: bool, state: State<'_, AppState>) -> Result<(), String> {
    let onboarded = state.identity_store.is_onboarded();
    state.engine.set_folder_writable(onboarded, std::path::PathBuf::from(path), on).await.map_err(|e| e.to_string())
}
```

- [ ] Register the command in `apps/desktop/src-tauri/src/main.rs` (in `generate_handler!`, grounding §1f, after `engine_revoke_grant` ~line 151). Add one `#[cfg(unix)]` line:

```rust
            #[cfg(unix)]
            commands::engine::engine_set_folder_writable,
```

- [ ] Build the desktop crate to confirm the command + DTO compile (no new unit test needed for the command wrapper; the op is tested):
  `cargo build -p air_agent_desktop`
  Expected output: `Finished` with no errors.

- [ ] Commit:
  `git add apps/desktop/src-tauri/src/engine/mod.rs apps/desktop/src-tauri/src/commands/engine.rs apps/desktop/src-tauri/src/main.rs`
  `git commit -m "feat(desktop): set_folder_writable op + command; FileRecordDto.writable"`

---

### Task 5: `set_proposals_enabled` op + command

**Files:**
- Modify: `apps/desktop/src-tauri/src/engine/mod.rs` (op)
- Modify: `apps/desktop/src-tauri/src/commands/engine.rs` (command)
- Modify: `apps/desktop/src-tauri/src/main.rs` (register)
- Test: `apps/desktop/src-tauri/src/engine/mod.rs` (tests mod)

- [ ] Write the failing test in `apps/desktop/src-tauri/src/engine/mod.rs` (tests mod), mirroring `set_evolve_enabled_toggles_the_engine_flag` (grounding §7c reference, lines 894-905):

```rust
    #[tokio::test]
    async fn set_proposals_enabled_toggles_the_engine_flag() {
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        let log = handle.get_or_open(true).await.unwrap();
        assert!(!log.proposals_enabled().unwrap(), "primed off at first open");
        drop(log);

        handle.set_proposals_enabled(true, true).await.unwrap();
        let log = handle.get_or_open(true).await.unwrap();
        assert!(log.proposals_enabled().unwrap(), "the op flips the sticky flag on");

        // Not onboarded → gate.
        assert!(matches!(
            handle.set_proposals_enabled(false, true).await,
            Err(EngineOpError::Open(EngineError::NotOnboarded))
        ));
    }
```

- [ ] Run it (expect FAIL): `cargo test -p air_agent_desktop set_proposals_enabled_toggles_the_engine_flag`
  Expected output: compile error `no method named set_proposals_enabled`.

- [ ] Implement the op in `apps/desktop/src-tauri/src/engine/mod.rs` (next to `set_evolve_enabled`, §2f, ~line 437):

```rust
    /// Flip the sticky engine proposals off-switch (Lock-1 enablement; turned on under the hood
    /// on first folder-enable). Gated.
    pub async fn set_proposals_enabled(&self, onboarded: bool, enabled: bool) -> Result<(), EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        spawn_blocking(move || {
            log.set_proposals_enabled(enabled).map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }
```

- [ ] Run it (expect PASS): `cargo test -p air_agent_desktop set_proposals_enabled_toggles_the_engine_flag`
  Expected output: `test result: ok. 1 passed`.

- [ ] Add the command in `apps/desktop/src-tauri/src/commands/engine.rs` (after `engine_set_evolve_enabled`, §1c ~line 241), mirroring the bool-setter template:

```rust
/// Flip the sticky proposals off-switch (invoked under the hood on first folder-enable).
#[tauri::command]
pub async fn engine_set_proposals_enabled(enabled: bool, state: State<'_, AppState>) -> Result<(), String> {
    let onboarded = state.identity_store.is_onboarded();
    state.engine.set_proposals_enabled(onboarded, enabled).await.map_err(|e| e.to_string())
}
```

- [ ] Register in `apps/desktop/src-tauri/src/main.rs` (after `engine_set_folder_writable`):

```rust
            #[cfg(unix)]
            commands::engine::engine_set_proposals_enabled,
```

- [ ] Build: `cargo build -p air_agent_desktop`
  Expected output: `Finished` with no errors.

- [ ] Commit:
  `git add apps/desktop/src-tauri/src/engine/mod.rs apps/desktop/src-tauri/src/commands/engine.rs apps/desktop/src-tauri/src/main.rs`
  `git commit -m "feat(desktop): set_proposals_enabled op + command"`

---

### Task 6: `list_proposals` op + `ProposalDto` + command

**Files:**
- Modify: `apps/desktop/src-tauri/src/engine/mod.rs` (`ProposalSummary` + op)
- Modify: `apps/desktop/src-tauri/src/commands/engine.rs` (`ProposalDto` + command)
- Modify: `apps/desktop/src-tauri/src/main.rs` (register)
- Test: `apps/desktop/src-tauri/src/engine/mod.rs` (tests mod)

- [ ] Write the failing test in `apps/desktop/src-tauri/src/engine/mod.rs` (tests mod). It opens the log, appends one minimal proposal directly through the `EventLog`, and asserts `list_proposals` returns one summary. The `EventLog` `append_write_proposal` needs a non-empty lineage; seed a memory event id as lineage (mirrors the engine harness `append_minimal_proposal`, grounding §6a):

```rust
    #[tokio::test]
    async fn list_proposals_returns_open_summaries() {
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        let log = handle.get_or_open(true).await.unwrap();

        // Seed a lineage event so the Tier-B proposal append is valid.
        let lineage = seed_one_memory_id(&log, "Alice works at Acme");
        let key = serde_json::json!({"src":"entity:a","relation":"works_at","dst":"entity:acme"});
        let pid = log.append_write_proposal(
            "/tmp/acme/notes.md", "edit", "deadbeef", 0, "Alice now works at Globex",
            &key, &serde_json::json!({"requires_loud_modal": false, "taint": "Clean", "allowed": true}),
            std::slice::from_ref(&lineage),
        ).unwrap();
        drop(log);

        let proposals = handle.list_proposals(true).await.unwrap();
        assert_eq!(proposals.len(), 1);
        let p = &proposals[0];
        assert_eq!(p.id, pid);
        assert_eq!(p.target, "/tmp/acme/notes.md");
        assert_eq!(p.op, "edit");
        assert_eq!(p.rationale, "Alice now works at Globex");
        assert!(!p.requires_loud_modal, "verdict_summary.requires_loud_modal projected");

        // Not onboarded → gate.
        assert!(matches!(
            handle.list_proposals(false).await,
            Err(EngineOpError::Open(EngineError::NotOnboarded))
        ));
    }
```

  This test references a helper `seed_one_memory_id` returning the appended id. The existing `seed_one_memory` (grounding §7b) returns `()`; add a sibling in the tests mod near it:

```rust
    fn seed_one_memory_id(log: &EventLog, text: &str) -> String {
        log.append(bossclaw_core::event::Event {
            id: String::new(), ts: String::new(), valid_time: None,
            event_type: "memory".to_string(),
            content: serde_json::json!({ "text": text }),
            model_meta: None, prev_hash: String::new(), hash: None,
            signed_by_did: "did:wba:AIR-TEST".to_string(), signature: None,
        }).unwrap()
    }
```

- [ ] Run it (expect FAIL): `cargo test -p air_agent_desktop list_proposals_returns_open_summaries`
  Expected output: compile error `no method named list_proposals` / `cannot find ProposalSummary`.

- [ ] Implement `ProposalSummary` + the op in `apps/desktop/src-tauri/src/engine/mod.rs`. Add the struct near the top of the file with the other engine types (after `EngineOpError`, ~line 66) and the op in the `impl EngineHandle` block. `pending_proposals()` is `pub` (Task 1):

```rust
/// A row in the Review queue, projected from one open `PendingProposal`. The
/// `requires_loud_modal` is lifted out of the propose-time `verdict_summary` for the badge/card.
#[derive(Debug, Clone)]
pub struct ProposalSummary {
    pub id: String,
    pub target: String,
    pub op: String,
    pub new_content_hash: String,
    pub rationale: String,
    pub requires_loud_modal: bool,
}

impl ProposalSummary {
    fn from_pending(p: bossclaw_core::PendingProposal) -> Self {
        // Single-sourced fail-loud default (m2): `PendingProposal::requires_loud_modal()` returns
        // true when the verdict is absent/garbled. This is a UI HINT to pre-show the modal; the
        // authoritative loud-confirm gate is the FRESH re-gate inside `apply_proposal` (Task 8).
        let requires_loud_modal = p.requires_loud_modal();
        Self {
            id: p.id,
            target: p.target,
            op: p.op,
            new_content_hash: p.new_content_hash,
            rationale: p.rationale,
            requires_loud_modal,
        }
    }
}
```

  Op (read template §2g):

```rust
    /// Every open proposal, projected for the Review queue. Gated.
    pub async fn list_proposals(&self, onboarded: bool) -> Result<Vec<ProposalSummary>, EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        tokio::task::spawn_blocking(move || {
            let pending = log.pending_proposals().map_err(|e| EngineOpError::Core(e.to_string()))?;
            Ok(pending.into_iter().map(ProposalSummary::from_pending).collect())
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }
```

  `bossclaw_core::PendingProposal` must be re-exported from the crate root, **cfg-gated** (M1) so it mirrors the type's own `#[cfg(unix)]` and the existing gated `MandateAction` re-export. In `crates/bossclaw-core/src/lib.rs`, the unconditional list at line 61 is `pub use log::{ActiveModel, EventLog, ReembedStats, SynthCacheRow, SCHEMA_VERSION};` and line 63 already has `#[cfg(unix)] pub use log::MandateAction;` — add directly beneath it:

```rust
#[cfg(unix)]
pub use log::PendingProposal;
```

  (Do NOT add `PendingProposal` to the unconditional line-61 list — the struct is `#[cfg(unix)]`, so an unconditional re-export would fail to compile on non-unix.)

- [ ] Run it (expect PASS): `cargo test -p air_agent_desktop list_proposals_returns_open_summaries`
  Expected output: `test result: ok. 1 passed`.

- [ ] Write + run the fail-loud default test (MIN-1) in the same tests mod. A garbled (non-object) `verdict_summary` must project `requires_loud_modal == true` via the single-sourced `PendingProposal::requires_loud_modal()` (m2):

```rust
    #[tokio::test]
    async fn garbled_verdict_summary_projects_requires_loud_modal_true() {
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        let log = handle.get_or_open(true).await.unwrap();
        let lineage = seed_one_memory_id(&log, "Alice works at Acme");
        let key = serde_json::json!({"src":"a","relation":"works_at","dst":"acme"});
        // verdict_summary is a STRING, not the expected object → `.get("requires_loud_modal")` is
        // None → fail-loud default true.
        let pid = log.append_write_proposal("/tmp/x/notes.md", "edit", "deadbeef", 0, "why",
            &key, &serde_json::json!("garbled"), std::slice::from_ref(&lineage)).unwrap();
        drop(log);

        let proposals = handle.list_proposals(true).await.unwrap();
        assert_eq!(proposals.len(), 1);
        assert!(proposals[0].id == pid && proposals[0].requires_loud_modal,
            "an absent/garbled verdict_summary fails loud (requires_loud_modal == true)");
    }
```

  Run it (expect FAIL before, PASS after the op exists): `cargo test -p air_agent_desktop garbled_verdict_summary_projects_requires_loud_modal_true`
  Expected output: `test result: ok. 1 passed`.

- [ ] Add `ProposalDto` + command in `apps/desktop/src-tauri/src/commands/engine.rs` (DTO with the `EvolveReportDto` `From` template §1e; command with the read template §1b):

```rust
#[derive(Serialize)]
pub struct ProposalDto {
    pub id: String,
    pub target: String,
    pub op: String,
    pub new_content_hash: String,
    pub rationale: String,
    pub requires_loud_modal: bool,
}
impl From<crate::engine::ProposalSummary> for ProposalDto {
    fn from(p: crate::engine::ProposalSummary) -> Self {
        Self {
            id: p.id, target: p.target, op: p.op,
            new_content_hash: p.new_content_hash, rationale: p.rationale,
            requires_loud_modal: p.requires_loud_modal,
        }
    }
}

#[tauri::command]
pub async fn engine_list_proposals(state: State<'_, AppState>) -> Result<Vec<ProposalDto>, String> {
    let onboarded = state.identity_store.is_onboarded();
    let proposals = state.engine.list_proposals(onboarded).await.map_err(|e| e.to_string())?;
    Ok(proposals.into_iter().map(ProposalDto::from).collect())
}
```

- [ ] Register in `apps/desktop/src-tauri/src/main.rs`:

```rust
            #[cfg(unix)]
            commands::engine::engine_list_proposals,
```

- [ ] Build: `cargo build -p air_agent_desktop`
  Expected output: `Finished` with no errors.

- [ ] Commit:
  `git add crates/bossclaw-core/src/lib.rs apps/desktop/src-tauri/src/engine/mod.rs apps/desktop/src-tauri/src/commands/engine.rs apps/desktop/src-tauri/src/main.rs`
  `git commit -m "feat(desktop): list_proposals op + ProposalDto + command"`

---

### Task 7: `proposal_preview` op + `PreviewDto` + command

**Files:**
- Modify: `apps/desktop/src-tauri/src/engine/mod.rs` (`PreviewData` + op)
- Modify: `apps/desktop/src-tauri/src/commands/engine.rs` (`PreviewDto` + command)
- Modify: `apps/desktop/src-tauri/src/main.rs` (register)
- Test: `apps/desktop/src-tauri/src/engine/mod.rs` (tests mod)

- [ ] Write the failing test in `apps/desktop/src-tauri/src/engine/mod.rs` (tests mod). It writes a real file under a write-granted dir, ingests it, drives one evolve tick to emit a real proposal (reuse the `DispatchReasoner`-style flow is engine-side; here, simpler: append a proposal whose bytes are stored via the engine so the preview can read both old + new). Because `proposal_preview` reads old text from the current file and new bytes via `get_proposal_bytes_checked`, the test must store bytes with `put_proposal_bytes`:

```rust
    #[tokio::test]
    async fn proposal_preview_returns_old_and_new_text_fail_closed_on_missing_bytes() {
        use bossclaw_core::actuator::{WriteOp, WriteProposal};
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        let log = handle.get_or_open(true).await.unwrap();

        // A real write-granted file with known "old" bytes.
        let folder = tempfile::tempdir().unwrap();
        log.add_grant(folder.path()).unwrap();
        log.add_write_grant(folder.path()).unwrap();
        let path = folder.path().join("notes.md");
        std::fs::write(&path, b"Alice works at Acme.\n").unwrap();
        let file_id = bossclaw_ingest_one(&log, &path); // see helper note below

        // Build a real gated proposal for new bytes, then record it + its bytes (the engine
        // emit path), so preview can read both halves.
        let new_bytes = b"Alice works at Globex.\n".to_vec();
        let gated = log.propose_write(WriteProposal {
            target: path.clone(), new_content: new_bytes.clone(), op: WriteOp::Edit,
            source_event_ids: vec![file_id.clone()], rationale: "correction".to_string(),
        }).unwrap();
        let hash = {
            use sha2::{Digest, Sha256};
            hex::encode(Sha256::digest(&new_bytes))
        };
        let canonical = std::fs::canonicalize(&path).unwrap().to_string_lossy().to_string();
        let key = serde_json::json!({"src":"entity:a","relation":"works_at","dst":"entity:acme"});
        let verdict_summary = serde_json::json!({
            "requires_loud_modal": gated.verdict.requires_loud_modal,
            "taint": format!("{:?}", gated.verdict.taint),
            "allowed": gated.verdict.allowed,
        });
        let pid = log.append_write_proposal(
            &canonical, "edit", &hash, new_bytes.len() as u64, "correction",
            &key, &verdict_summary, std::slice::from_ref(&file_id),
        ).unwrap();
        log.put_proposal_bytes(&pid, &new_bytes, &hash).unwrap();
        drop(log);

        let preview = handle.proposal_preview(true, pid.clone()).await.unwrap();
        assert_eq!(preview.path, canonical);
        assert_eq!(preview.old_text, "Alice works at Acme.\n");
        assert_eq!(preview.new_text, "Alice works at Globex.\n");
        assert_eq!(preview.op, "edit");
        assert_eq!(preview.rationale, "correction");

        // Fail-closed: an unknown id errors (no bytes / no proposal).
        assert!(handle.proposal_preview(true, "nonexistent".to_string()).await.is_err());

        // Fail-loud (MIN-1): a proposal whose verdict_summary is garbled previews as loud.
        let log = handle.get_or_open(true).await.unwrap();
        let new2 = b"Alice works at Initech.\n".to_vec();
        let hash2 = { use sha2::{Digest, Sha256}; hex::encode(Sha256::digest(&new2)) };
        let pid2 = log.append_write_proposal(&canonical, "edit", &hash2, new2.len() as u64,
            "correction2", &key, &serde_json::json!("garbled"), std::slice::from_ref(&file_id)).unwrap();
        log.put_proposal_bytes(&pid2, &new2, &hash2).unwrap();
        drop(log);
        let preview2 = handle.proposal_preview(true, pid2).await.unwrap();
        assert!(preview2.requires_loud_modal, "garbled verdict_summary previews fail-loud");
    }
```

  Helper note: the test needs to ingest a single written file and get its `file_ingested` id. The engine harness has `common::ingest_one`, but that is in the `bossclaw-core` test crate, not the desktop crate. In the desktop tests mod, add a local equivalent using the public API (the embedder is `MockEmbedderProvider`'s inner mock; the desktop crate already wires `embed::MockEmbedderProvider`). Add to the tests mod:

```rust
    fn bossclaw_ingest_one(log: &EventLog, path: &std::path::Path) -> String {
        let embedder = bossclaw_core::embed::MockEmbedder::new(8);
        log.ingest_all(&bossclaw_core::ingest::ParserRouter::native_only(), &embedder).unwrap();
        let canonical = std::fs::canonicalize(path).unwrap().to_string_lossy().to_string();
        log.current_files().unwrap().into_iter()
            .find(|r| r.canonical_path == canonical)
            .map(|r| r.file_event_id)
            .expect("ingested file id")
    }
```

- [ ] Run it (expect FAIL): `cargo test -p air_agent_desktop proposal_preview_returns_old_and_new_text_fail_closed_on_missing_bytes`
  Expected output: compile error `no method named proposal_preview` / `cannot find PreviewData`.

- [ ] Implement `PreviewData` + the op in `apps/desktop/src-tauri/src/engine/mod.rs`. The op: load the open proposal (find it in `pending_proposals()` by id), read the current file bytes for `old_text` (via the engine — the target is a canonical path; read it directly with `std::fs::read` inside the blocking closure, which is local-only and within the spawn_blocking already used by every op), get `new_text` via `get_proposal_bytes_checked(id, new_content_hash)` (fail-closed), and project the loud-modal/taint flags from the proposal's `verdict_summary`:

```rust
/// Everything the Review card renders for one proposal: paths, the "Why", op, both text halves,
/// and the propose-time loud-modal/taint flags. `old_text`/`new_text` are lossy-UTF8 (the engine
/// only proposes against UTF-8 targets; non-UTF8 is rejected at synthesis).
#[derive(Debug, Clone)]
pub struct PreviewData {
    pub path: String,
    pub folder: String,
    pub rationale: String,
    pub op: String,
    pub old_text: String,
    pub new_text: String,
    pub requires_loud_modal: bool,
    pub taint: String,
}

impl EngineHandle {
    /// Build the before/after preview for one open proposal. Fail-closed: an unknown id, a
    /// proposal whose bytes are missing/tampered (`get_proposal_bytes_checked`), or an
    /// unreadable target all return `Err`. Gated.
    pub async fn proposal_preview(&self, onboarded: bool, id: String) -> Result<PreviewData, EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        tokio::task::spawn_blocking(move || {
            let pending = log.pending_proposals().map_err(|e| EngineOpError::Core(e.to_string()))?;
            let p = pending.into_iter().find(|p| p.id == id)
                .ok_or_else(|| EngineOpError::Core("proposal not found or already resolved".to_string()))?;
            // new bytes — fail closed unless they hash to the signed proposal's recorded hash.
            let new_bytes = log.get_proposal_bytes_checked(&p.id, &p.new_content_hash)
                .map_err(|e| EngineOpError::Core(e.to_string()))?;
            // old bytes — the current on-disk file (local read; the target is canonical).
            let old_bytes = std::fs::read(&p.target)
                .map_err(|e| EngineOpError::Core(format!("could not read target: {e}")))?;
            let folder = std::path::Path::new(&p.target)
                .parent().map(|d| d.to_string_lossy().to_string()).unwrap_or_default();
            // Single-sourced fail-loud default (m2) — read into a local BEFORE the struct moves
            // `p`'s fields. A UI hint only; the authoritative gate is `apply_proposal` (Task 8).
            let requires_loud_modal = p.requires_loud_modal();
            let taint = p.verdict_summary.get("taint").and_then(|v| v.as_str()).unwrap_or("Untrusted").to_string();
            Ok(PreviewData {
                path: p.target,
                folder,
                rationale: p.rationale,
                op: p.op,
                old_text: String::from_utf8_lossy(&old_bytes).to_string(),
                new_text: String::from_utf8_lossy(&new_bytes).to_string(),
                requires_loud_modal,
                taint,
            })
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }
}
```

- [ ] Run it (expect PASS): `cargo test -p air_agent_desktop proposal_preview_returns_old_and_new_text_fail_closed_on_missing_bytes`
  Expected output: `test result: ok. 1 passed`.

- [ ] Add `PreviewDto` + command in `apps/desktop/src-tauri/src/commands/engine.rs`:

```rust
#[derive(Serialize)]
pub struct PreviewDto {
    pub path: String,
    pub folder: String,
    pub rationale: String,
    pub op: String,
    pub old_text: String,
    pub new_text: String,
    pub requires_loud_modal: bool,
    pub taint: String,
}
impl From<crate::engine::PreviewData> for PreviewDto {
    fn from(p: crate::engine::PreviewData) -> Self {
        Self {
            path: p.path, folder: p.folder, rationale: p.rationale, op: p.op,
            old_text: p.old_text, new_text: p.new_text,
            requires_loud_modal: p.requires_loud_modal, taint: p.taint,
        }
    }
}

#[tauri::command]
pub async fn engine_proposal_preview(id: String, state: State<'_, AppState>) -> Result<PreviewDto, String> {
    let onboarded = state.identity_store.is_onboarded();
    let preview = state.engine.proposal_preview(onboarded, id).await.map_err(|e| e.to_string())?;
    Ok(PreviewDto::from(preview))
}
```

- [ ] Register in `apps/desktop/src-tauri/src/main.rs`:

```rust
            #[cfg(unix)]
            commands::engine::engine_proposal_preview,
```

- [ ] Build: `cargo build -p air_agent_desktop`
  Expected output: `Finished` with no errors.

- [ ] Commit:
  `git add apps/desktop/src-tauri/src/engine/mod.rs apps/desktop/src-tauri/src/commands/engine.rs apps/desktop/src-tauri/src/main.rs`
  `git commit -m "feat(desktop): proposal_preview op + PreviewDto + command (fail-closed bytes)"`

---

### Task 8: `apply_proposal` op (re-gate, staleness + loud-confirm fail-closed) + `ApplyResultDto` + command

**Files:**
- Modify: `apps/desktop/src-tauri/src/engine/mod.rs` (`EngineOpError::Stale`/`Revoked`/`NeedsLoudConfirm` + `ApplyResult` + op with `acknowledged_loud`)
- Modify: `apps/desktop/src-tauri/src/commands/engine.rs` (`ApplyResultDto` + command with `acknowledged_loud`)
- Modify: `apps/desktop/src-tauri/src/main.rs` (register)
- Test: `apps/desktop/src-tauri/src/engine/mod.rs` (tests mod, incl. stale-file + loud-confirm)

> **Loud-modal is enforced HERE, on the FRESH re-gate verdict (security fix G1/IMP-1a).** The
> propose-time `requires_loud_modal` on `ProposalDto` is only a UI hint. `apply_proposal` recomputes
> the verdict against the live file and, if it is loud, REFUSES to write unless the caller passes
> `acknowledged_loud == true`. The desktop op is the sole write entry point besides undo, so this
> closes the "React-only enforcement off a stale flag" gap. (Follow-up, deferred: enforce
> `requires_loud_modal` inside `bossclaw-core` `execute_write_inner` itself — out of scope for SP4.)

- [ ] Write the failing tests in `apps/desktop/src-tauri/src/engine/mod.rs` (tests mod): one happy-path apply that mutates the file + resolves the proposal, and one stale-file fail-closed (mutate the file after propose; assert apply errors AND the file is unchanged). Reuse the `bossclaw_ingest_one` helper from Task 7:

```rust
    #[tokio::test]
    async fn apply_proposal_writes_file_and_resolves_then_stale_fails_closed() {
        use bossclaw_core::actuator::{WriteOp, WriteProposal};
        use sha2::{Digest, Sha256};

        // ---- happy path ----
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        let log = handle.get_or_open(true).await.unwrap();
        let folder = tempfile::tempdir().unwrap();
        log.add_grant(folder.path()).unwrap();
        log.add_write_grant(folder.path()).unwrap();
        let path = folder.path().join("notes.md");
        let original = b"Alice works at Acme.\n".to_vec();
        std::fs::write(&path, &original).unwrap();
        let file_id = bossclaw_ingest_one(&log, &path);
        let new_bytes = b"Alice works at Globex.\n".to_vec();
        let hash = hex::encode(Sha256::digest(&new_bytes));
        let canonical = std::fs::canonicalize(&path).unwrap().to_string_lossy().to_string();
        let key = serde_json::json!({"src":"a","relation":"works_at","dst":"acme"});
        let gated = log.propose_write(WriteProposal {
            target: path.clone(), new_content: new_bytes.clone(), op: WriteOp::Edit,
            source_event_ids: vec![file_id.clone()], rationale: "fix".to_string(),
        }).unwrap();
        let vs = serde_json::json!({"requires_loud_modal": gated.verdict.requires_loud_modal,
            "taint": format!("{:?}", gated.verdict.taint), "allowed": gated.verdict.allowed,
            "base_content_hash": gated.verdict.base_content_hash});
        let pid = log.append_write_proposal(&canonical, "edit", &hash, new_bytes.len() as u64,
            "fix", &key, &vs, std::slice::from_ref(&file_id)).unwrap();
        log.put_proposal_bytes(&pid, &new_bytes, &hash).unwrap();
        drop(log);

        // Not-loud proposal applies with acknowledged_loud=false.
        let result = handle.apply_proposal(true, pid.clone(), false).await.unwrap();
        assert!(!result.file_written_id.is_empty(), "an apply returns the file_written id");
        assert_eq!(std::fs::read(&path).unwrap(), new_bytes, "the file gained the corrected bytes");
        // the proposal is no longer pending (resolved by the file_written).
        assert!(handle.list_proposals(true).await.unwrap().iter().all(|p| p.id != pid));
        // G4: the emitted file_written carries resolves_proposal == pid (the resolution mechanism).
        let log = handle.get_or_open(true).await.unwrap();
        let fw = log.event_by_id(&result.file_written_id).unwrap().unwrap();
        assert_eq!(fw.event_type, "file_written");
        assert_eq!(fw.content["resolves_proposal"], serde_json::json!(pid),
            "the file_written resolves the exact proposal id");
        drop(log);

        // ---- stale path: mutate the file AFTER a fresh propose, assert apply fails closed ----
        let (vault2, dir2) = test_vault_and_dir();
        let handle2 = new_test_handle(vault2, &dir2);
        let log2 = handle2.get_or_open(true).await.unwrap();
        let folder2 = tempfile::tempdir().unwrap();
        log2.add_grant(folder2.path()).unwrap();
        log2.add_write_grant(folder2.path()).unwrap();
        let path2 = folder2.path().join("notes.md");
        let orig2 = b"Alice works at Acme.\n".to_vec();
        std::fs::write(&path2, &orig2).unwrap();
        let fid2 = bossclaw_ingest_one(&log2, &path2);
        let new2 = b"Alice works at Globex.\n".to_vec();
        let hash2 = hex::encode(Sha256::digest(&new2));
        let canon2 = std::fs::canonicalize(&path2).unwrap().to_string_lossy().to_string();
        let k2 = serde_json::json!({"src":"a","relation":"works_at","dst":"acme"});
        let g2 = log2.propose_write(WriteProposal { target: path2.clone(), new_content: new2.clone(),
            op: WriteOp::Edit, source_event_ids: vec![fid2.clone()], rationale: "fix".to_string() }).unwrap();
        // The proposal records its base fingerprint = sha256("Alice works at Acme.") at propose.
        let vs2 = serde_json::json!({"requires_loud_modal": g2.verdict.requires_loud_modal,
            "taint": format!("{:?}", g2.verdict.taint), "allowed": g2.verdict.allowed,
            "base_content_hash": g2.verdict.base_content_hash});
        assert_eq!(g2.verdict.base_content_hash.as_deref(), Some(hex::encode(Sha256::digest(&orig2)).as_str()),
            "the gate fingerprinted the original on-disk bytes");
        let pid2 = log2.append_write_proposal(&canon2, "edit", &hash2, new2.len() as u64, "fix",
            &k2, &vs2, std::slice::from_ref(&fid2)).unwrap();
        log2.put_proposal_bytes(&pid2, &new2, &hash2).unwrap();
        drop(log2);

        // Someone edits the file out from under the proposal (live bytes no longer match base).
        std::fs::write(&path2, b"Alice retired.\n").unwrap();

        let stale = handle2.apply_proposal(true, pid2.clone(), false).await;
        assert!(matches!(stale, Err(EngineOpError::Stale(_))), "a changed file fails closed as Stale: {stale:?}");
        assert_eq!(std::fs::read(&path2).unwrap(), b"Alice retired.\n".to_vec(),
            "the file is untouched when the apply fails closed (no propose, no execute)");
    }
```

- [ ] Write + run the loud-confirm enforcement test (G1/IMP-1a) in the same tests mod. Craft `new_content` containing a high-entropy token (≥32 unbroken ASCII alphanumerics → `propose_write` sets `touches_secret_shaped` ⇒ `requires_loud_modal == true`, per actuator `DiffFlags`). Applying with `acknowledged_loud=false` must return `NeedsLoudConfirm` and leave the file unchanged; with `true` it applies:

```rust
    #[tokio::test]
    async fn apply_proposal_loud_needs_ack_then_applies() {
        use bossclaw_core::actuator::{WriteOp, WriteProposal};
        use sha2::{Digest, Sha256};
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        let log = handle.get_or_open(true).await.unwrap();
        let folder = tempfile::tempdir().unwrap();
        log.add_grant(folder.path()).unwrap();
        log.add_write_grant(folder.path()).unwrap();
        let path = folder.path().join("secrets.md");
        let original = b"placeholder\n".to_vec();
        std::fs::write(&path, &original).unwrap();
        let file_id = bossclaw_ingest_one(&log, &path);
        // A >=32-char unbroken alphanumeric run trips the secret-shaped diff flag → loud.
        let new_bytes = b"token=ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcd\n".to_vec();
        let hash = hex::encode(Sha256::digest(&new_bytes));
        let canonical = std::fs::canonicalize(&path).unwrap().to_string_lossy().to_string();
        let key = serde_json::json!({"src":"a","relation":"r","dst":"b"});
        let gated = log.propose_write(WriteProposal { target: path.clone(), new_content: new_bytes.clone(),
            op: WriteOp::Edit, source_event_ids: vec![file_id.clone()], rationale: "fix".to_string() }).unwrap();
        assert!(gated.verdict.requires_loud_modal, "secret-shaped content forces the loud modal");
        let vs = serde_json::json!({"requires_loud_modal": gated.verdict.requires_loud_modal,
            "taint": format!("{:?}", gated.verdict.taint), "allowed": gated.verdict.allowed,
            "base_content_hash": gated.verdict.base_content_hash});
        let pid = log.append_write_proposal(&canonical, "edit", &hash, new_bytes.len() as u64,
            "fix", &key, &vs, std::slice::from_ref(&file_id)).unwrap();
        log.put_proposal_bytes(&pid, &new_bytes, &hash).unwrap();
        drop(log);

        // acknowledged_loud=false → refuses, file unchanged.
        let needs = handle.apply_proposal(true, pid.clone(), false).await;
        assert!(matches!(needs, Err(EngineOpError::NeedsLoudConfirm(_))),
            "a loud write without ack is refused: {needs:?}");
        assert_eq!(std::fs::read(&path).unwrap(), original, "no write happened without the ack");

        // acknowledged_loud=true → applies.
        handle.apply_proposal(true, pid, true).await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), new_bytes, "the ack lets the loud write through");
    }
```

  Run it (PASS after the op): `cargo test -p air_agent_desktop apply_proposal_loud_needs_ack_then_applies`
  Expected output: `test result: ok. 1 passed`.

- [ ] Run it (expect FAIL): `cargo test -p air_agent_desktop apply_proposal_writes_file_and_resolves_then_stale_fails_closed`
  Expected output: compile error `no method named apply_proposal` / `no variant ... Stale`.

- [ ] Implement the typed errors + the op in `apps/desktop/src-tauri/src/engine/mod.rs`. First add three variants to `EngineOpError` (grounding §2a, lines 50-66) and their `Display` arms:

```rust
    /// The on-disk file changed since the proposal was drafted; the re-gate at confirm
    /// fails closed. Carries the reason. Nothing is written.
    Stale(String),
    /// The folder's write-grant was revoked between propose and apply; re-gate fails closed.
    Revoked(String),
    /// The FRESH re-gate verdict is loud (secret-/value-shaped or Delete) but the caller did not
    /// pass `acknowledged_loud == true`. The op refuses to write — the UI must show the
    /// "I've reviewed this" confirm and retry with the ack. Carries the reason.
    NeedsLoudConfirm(String),
```

  Add to `impl Display for EngineOpError` (grounding §2a continues in the engine file; mirror the existing arms):

```rust
            EngineOpError::Stale(m) => write!(f, "the file changed since this was suggested: {m}"),
            EngineOpError::Revoked(m) => write!(f, "edits aren't allowed in this folder anymore: {m}"),
            EngineOpError::NeedsLoudConfirm(m) => write!(f, "this change needs an explicit review confirmation: {m}"),
```

  Then the result type + op (multi-step spawn_blocking template §2i, modeled on the engine round-trip test §6e):

```rust
/// The outcome of a successful apply: the audit `file_written` id (also the handle for Undo).
#[derive(Debug, Clone)]
pub struct ApplyResult {
    pub file_written_id: String,
}

impl EngineHandle {
    /// Approve + apply one proposal (Lock 2). The anti-clobber check is the EXPLICIT base-hash
    /// compare: read the live target, sha256 it, and if the proposal's recorded
    /// `base_content_hash` differs → fail closed as `Stale` BEFORE proposing or executing (a fresh
    /// re-propose re-bases on live bytes and could not see the drift). Only then does it fetch the
    /// verified bytes, re-gate with a FRESH `propose_write` against the LIVE file + current
    /// write-grant (this still guards the micro-TOCTOU window + grant revocation). The loud-confirm
    /// is decided from the FRESH verdict (NOT the stale propose-time flag): if it is loud and
    /// `acknowledged_loud == false` the op REFUSES (`NeedsLoudConfirm`) — never a silent write.
    /// Only then does it execute (atomic temp+rename, durable undo, signed `file_written`). Nothing
    /// is written on any failure. Gated.
    pub async fn apply_proposal(&self, onboarded: bool, id: String, acknowledged_loud: bool) -> Result<ApplyResult, EngineOpError> {
        use sha2::{Digest, Sha256};
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        tokio::task::spawn_blocking(move || {
            let pending = log.pending_proposals().map_err(|e| EngineOpError::Core(e.to_string()))?;
            let p = pending.into_iter().find(|p| p.id == id)
                .ok_or_else(|| EngineOpError::Stale("proposal not found or already resolved".to_string()))?;

            // ── ANTI-CLOBBER: compare the live file to the proposal's propose-time fingerprint. ──
            // This is the TRUE staleness detector (a fresh propose_write below re-bases on the live
            // file and cannot detect that it changed). Edit-only proposals always carry a base.
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
                    // No recorded base (e.g. a legacy/minimal proposal) → cannot prove freshness.
                    return Err(EngineOpError::Stale("proposal has no base fingerprint to verify against".to_string()));
                }
                _ => {} // base matches live → proceed.
            }

            // Verified bytes (fail closed if the side-table row is missing/tampered).
            let bytes = log.get_proposal_bytes_checked(&p.id, &p.new_content_hash)
                .map_err(|e| EngineOpError::Core(e.to_string()))?;
            // FRESH gate against the current disk + grant (never trust the stored verdict). Guards
            // the micro-TOCTOU window between the hash check above and the rename, + grant revoke.
            let gated = log.propose_write(bossclaw_core::actuator::WriteProposal {
                target: std::path::PathBuf::from(&p.target),
                new_content: bytes,
                op: bossclaw_core::actuator::WriteOp::Edit,
                source_event_ids: p.source_event_ids.clone(),
                rationale: p.rationale.clone(),
            }).map_err(|e| EngineOpError::Core(e.to_string()))?;
            // reject_reason set ⇒ symlink/op-mismatch/unresolvable; !allowed ⇒ grant revoked.
            if let Some(reason) = gated.verdict.reject_reason.as_deref() {
                return Err(EngineOpError::Stale(reason.to_string()));
            }
            if !gated.verdict.allowed {
                return Err(EngineOpError::Revoked("target not under an active write grant".to_string()));
            }
            // LOUD-CONFIRM on the FRESH verdict (G1/IMP-1a): a loud write requires the explicit ack.
            // This is the authoritative gate — the propose-time flag was only a UI hint.
            if gated.verdict.requires_loud_modal && !acknowledged_loud {
                return Err(EngineOpError::NeedsLoudConfirm(
                    "secret-/value-shaped or delete change — confirm review before applying".to_string(),
                ));
            }
            // execute is atomic temp+rename: it never partially writes, so a failure here also
            // leaves the file untouched. (Defensive: any execute error surfaces as Core.)
            let fw_id = log.execute_write_resolving(gated, &p.id)
                .map_err(|e| EngineOpError::Core(e.to_string()))?;
            Ok(ApplyResult { file_written_id: fw_id })
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }
}
```

- [ ] Run it (expect PASS): `cargo test -p air_agent_desktop apply_proposal_writes_file_and_resolves_then_stale_fails_closed`
  Expected output: `test result: ok. 1 passed`. The happy path's base matches the live file → applies; the stale path's recorded base (`sha256("Alice works at Acme.")`) differs from the live `sha256("Alice retired.")` → `Stale` is returned before any propose/execute, so the file stays `"Alice retired."`.

- [ ] Add `ApplyResultDto` + command in `apps/desktop/src-tauri/src/commands/engine.rs`:

```rust
#[derive(Serialize)]
pub struct ApplyResultDto {
    pub file_written_id: String,
}
impl From<crate::engine::ApplyResult> for ApplyResultDto {
    fn from(r: crate::engine::ApplyResult) -> Self {
        Self { file_written_id: r.file_written_id }
    }
}

#[tauri::command]
pub async fn engine_apply_proposal(id: String, acknowledged_loud: bool, state: State<'_, AppState>) -> Result<ApplyResultDto, String> {
    let onboarded = state.identity_store.is_onboarded();
    let result = state.engine.apply_proposal(onboarded, id, acknowledged_loud).await.map_err(|e| e.to_string())?;
    Ok(ApplyResultDto::from(result))
}
```

> Idiomatic snake_case Rust param `acknowledged_loud` (MIN-3); the JS twin calls `invoke("engine_apply_proposal", { id, acknowledgedLoud })` and Tauri's documented camelCase↔snake_case arg mapping bridges `acknowledgedLoud` → `acknowledged_loud`. Same convention as `engine_undo_apply`'s `file_written_id` ↔ `fileWrittenId` (Task 10).

- [ ] Register in `apps/desktop/src-tauri/src/main.rs`:

```rust
            #[cfg(unix)]
            commands::engine::engine_apply_proposal,
```

- [ ] Build: `cargo build -p air_agent_desktop`
  Expected output: `Finished` with no errors.

- [ ] Commit:
  `git add apps/desktop/src-tauri/src/engine/mod.rs apps/desktop/src-tauri/src/commands/engine.rs apps/desktop/src-tauri/src/main.rs`
  `git commit -m "feat(desktop): apply_proposal op (re-gate + base-hash anti-clobber + loud-confirm fail-closed) + command"`

---

### Task 9: `decline_proposal` op + command

**Files:**
- Modify: `apps/desktop/src-tauri/src/engine/mod.rs` (op)
- Modify: `apps/desktop/src-tauri/src/commands/engine.rs` (command)
- Modify: `apps/desktop/src-tauri/src/main.rs` (register)
- Test: `apps/desktop/src-tauri/src/engine/mod.rs` (tests mod)

- [ ] Write the failing test in `apps/desktop/src-tauri/src/engine/mod.rs` (tests mod). It appends a proposal, declines it, and asserts it is no longer pending. Reuse `seed_one_memory_id` (Task 6):

```rust
    #[tokio::test]
    async fn decline_proposal_removes_it_from_pending() {
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        let log = handle.get_or_open(true).await.unwrap();
        let lineage = seed_one_memory_id(&log, "Alice works at Acme");
        let key = serde_json::json!({"src":"a","relation":"works_at","dst":"acme"});
        let pid = log.append_write_proposal("/tmp/x/notes.md", "edit", "deadbeef", 0, "why",
            &key, &serde_json::json!({"requires_loud_modal": false, "taint": "Clean", "allowed": true}),
            std::slice::from_ref(&lineage)).unwrap();
        drop(log);

        assert_eq!(handle.list_proposals(true).await.unwrap().len(), 1);
        handle.decline_proposal(true, pid.clone(), "not now".to_string()).await.unwrap();
        assert!(handle.list_proposals(true).await.unwrap().is_empty(), "declined → no longer pending");

        assert!(matches!(
            handle.decline_proposal(false, pid, "x".to_string()).await,
            Err(EngineOpError::Open(EngineError::NotOnboarded))
        ));
    }
```

- [ ] Run it (expect FAIL): `cargo test -p air_agent_desktop decline_proposal_removes_it_from_pending`
  Expected output: compile error `no method named decline_proposal`.

- [ ] Implement the op in `apps/desktop/src-tauri/src/engine/mod.rs` (mutate template; `decline_write_proposal` grounding §5):

```rust
    /// Decline a proposal — terminal `write_declined` (resolves it; the fix never returns). Gated.
    pub async fn decline_proposal(&self, onboarded: bool, id: String, reason: String) -> Result<(), EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        tokio::task::spawn_blocking(move || {
            log.decline_write_proposal(&id, &reason).map(|_| ()).map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }
```

- [ ] Run it (expect PASS): `cargo test -p air_agent_desktop decline_proposal_removes_it_from_pending`
  Expected output: `test result: ok. 1 passed`.

- [ ] Add the command in `apps/desktop/src-tauri/src/commands/engine.rs`:

```rust
#[tauri::command]
pub async fn engine_decline_proposal(id: String, reason: String, state: State<'_, AppState>) -> Result<(), String> {
    let onboarded = state.identity_store.is_onboarded();
    state.engine.decline_proposal(onboarded, id, reason).await.map_err(|e| e.to_string())
}
```

- [ ] Register in `apps/desktop/src-tauri/src/main.rs`:

```rust
            #[cfg(unix)]
            commands::engine::engine_decline_proposal,
```

- [ ] Build: `cargo build -p air_agent_desktop`
  Expected output: `Finished` with no errors.

- [ ] Commit:
  `git add apps/desktop/src-tauri/src/engine/mod.rs apps/desktop/src-tauri/src/commands/engine.rs apps/desktop/src-tauri/src/main.rs`
  `git commit -m "feat(desktop): decline_proposal op + command"`

---

### Task 10: `undo_apply` op + command (+ command-layer arg-binding round-trip test)

**Files:**
- Modify: `apps/desktop/src-tauri/src/engine/mod.rs` (op)
- Modify: `apps/desktop/src-tauri/src/commands/engine.rs` (command + command-layer test)
- Modify: `apps/desktop/src-tauri/src/main.rs` (register)
- Modify: `apps/desktop/src-tauri/Cargo.toml` (add the `tauri` `test` feature to dev-deps for the command test)
- Test: `apps/desktop/src-tauri/src/engine/mod.rs` (tests mod)

- [ ] Write the failing test in `apps/desktop/src-tauri/src/engine/mod.rs` (tests mod). It reuses the happy-path apply flow, then undoes it and asserts the file is restored. Build it standalone (full setup; mirrors the engine round-trip §6e):

```rust
    #[tokio::test]
    async fn undo_apply_restores_the_original_bytes() {
        use bossclaw_core::actuator::{WriteOp, WriteProposal};
        use sha2::{Digest, Sha256};
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        let log = handle.get_or_open(true).await.unwrap();
        let folder = tempfile::tempdir().unwrap();
        log.add_grant(folder.path()).unwrap();
        log.add_write_grant(folder.path()).unwrap();
        let path = folder.path().join("notes.md");
        let original = b"Alice works at Acme.\n".to_vec();
        std::fs::write(&path, &original).unwrap();
        let file_id = bossclaw_ingest_one(&log, &path);
        let new_bytes = b"Alice works at Globex.\n".to_vec();
        let hash = hex::encode(Sha256::digest(&new_bytes));
        let canonical = std::fs::canonicalize(&path).unwrap().to_string_lossy().to_string();
        let key = serde_json::json!({"src":"a","relation":"works_at","dst":"acme"});
        let gated = log.propose_write(WriteProposal { target: path.clone(), new_content: new_bytes.clone(),
            op: WriteOp::Edit, source_event_ids: vec![file_id.clone()], rationale: "fix".to_string() }).unwrap();
        let vs = serde_json::json!({"requires_loud_modal": gated.verdict.requires_loud_modal,
            "taint": format!("{:?}", gated.verdict.taint), "allowed": gated.verdict.allowed});
        let pid = log.append_write_proposal(&canonical, "edit", &hash, new_bytes.len() as u64, "fix",
            &key, &vs, std::slice::from_ref(&file_id)).unwrap();
        log.put_proposal_bytes(&pid, &new_bytes, &hash).unwrap();
        drop(log);

        let applied = handle.apply_proposal(true, pid, false).await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), new_bytes, "applied");
        handle.undo_apply(true, applied.file_written_id.clone()).await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), original, "undo restored the original bytes");

        assert!(matches!(
            handle.undo_apply(false, applied.file_written_id).await,
            Err(EngineOpError::Open(EngineError::NotOnboarded))
        ));
    }
```

- [ ] Run it (expect FAIL): `cargo test -p air_agent_desktop undo_apply_restores_the_original_bytes`
  Expected output: compile error `no method named undo_apply`.

- [ ] Implement the op in `apps/desktop/src-tauri/src/engine/mod.rs` (`undo_write` grounding §5):

```rust
    /// Undo a prior apply — re-gated, hash-verified restore of the pre-write bytes (LIFO per
    /// target); fails closed if the file diverged since. Gated.
    pub async fn undo_apply(&self, onboarded: bool, file_written_id: String) -> Result<(), EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        tokio::task::spawn_blocking(move || {
            log.undo_write(&file_written_id).map(|_| ()).map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }
```

- [ ] Run it (expect PASS): `cargo test -p air_agent_desktop undo_apply_restores_the_original_bytes`
  Expected output: `test result: ok. 1 passed`.

- [ ] Add the command in `apps/desktop/src-tauri/src/commands/engine.rs`:

```rust
#[tauri::command]
pub async fn engine_undo_apply(file_written_id: String, state: State<'_, AppState>) -> Result<(), String> {
    let onboarded = state.identity_store.is_onboarded();
    state.engine.undo_apply(onboarded, file_written_id).await.map_err(|e| e.to_string())
}
```

  (Idiomatic snake_case Rust param per MIN-3; the TS twin in Task 11 calls `invoke("engine_undo_apply", { fileWrittenId })` and Tauri's documented camelCase↔snake_case arg mapping bridges `fileWrittenId` → `file_written_id`.)

- [ ] Register in `apps/desktop/src-tauri/src/main.rs`:

```rust
            #[cfg(unix)]
            commands::engine::engine_undo_apply,
```

- [ ] Add the `tauri` `test` feature to dev-deps (m4) — enables `tauri::test::mock_builder`. In `apps/desktop/src-tauri/Cargo.toml`, the `[dev-dependencies]` block currently has only `tempfile = "3"`; add:

```toml
[dev-dependencies]
tempfile = "3"
tauri = { version = "2", features = ["test"] }
```

- [ ] Write + run the command-LAYER round-trip test (m4) in `apps/desktop/src-tauri/src/commands/engine.rs` (test mod, after the DTO tests at line 280+). It builds a real Tauri mock app managing a genuine onboarded `AppState`, then invokes `engine_undo_apply` over IPC with the camelCase `{ fileWrittenId }` body — proving Tauri binds it to the snake_case `file_written_id` param and the op runs (an unknown id surfaces an `Err`, confirming the arg reached `undo_write`). Mirrors the `TestVault` from `commands/identity.rs:110-116`:

```rust
    use crate::commands::identity::AppState;
    use crate::secrets::SecretsVault;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use tauri::Manager;

    struct CmdTestVault { store: Mutex<HashMap<String, String>> }
    impl CmdTestVault { fn new() -> Arc<Self> { Arc::new(Self { store: Mutex::new(HashMap::new()) }) } }
    impl SecretsVault for CmdTestVault {
        fn set(&self, k: &str, v: &str) -> Result<(), String> { self.store.lock().unwrap().insert(k.into(), v.into()); Ok(()) }
        fn get(&self, k: &str) -> Result<Option<String>, String> { Ok(self.store.lock().unwrap().get(k).cloned()) }
        fn delete(&self, k: &str) -> Result<(), String> { self.store.lock().unwrap().remove(k); Ok(()) }
    }

    #[test]
    fn engine_undo_apply_binds_camelcase_arg_over_ipc() {
        use crate::air::identity::{IdentityMetadata, IdentityStore};
        use crate::air::types::Did;
        let dir = tempfile::tempdir().unwrap();
        let vault = CmdTestVault::new();
        // Make is_onboarded() true: save a signing key + metadata (the engine then opens).
        // IdentityMetadata has no Default — build it explicitly (Did is a pub-field tuple struct).
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
        let state = AppState { identity_store, engine };

        let app = tauri::test::mock_builder()
            .invoke_handler(tauri::generate_handler![engine_undo_apply])
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build mock app");
        app.manage(state);
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default()).build().unwrap();

        // Invoke with the camelCase key the TS twin sends.
        let res = tauri::test::get_ipc_response(
            &webview,
            tauri::webview::InvokeRequest {
                cmd: "engine_undo_apply".into(),
                callback: tauri::ipc::CallbackFn(0),
                error: tauri::ipc::CallbackFn(1),
                url: "http://tauri.localhost".parse().unwrap(),
                body: tauri::ipc::InvokeBody::Json(serde_json::json!({ "fileWrittenId": "no-such-id" })),
                headers: Default::default(),
                invoke_key: tauri::test::INVOKE_KEY.to_string(),
            },
        );
        // The arg bound + the op ran: an unknown file_written_id fails (Err), NOT a
        // "missing required key file_written_id" deserialization error.
        let err = res.expect_err("unknown id makes undo_write fail");
        let msg = err.to_string();
        assert!(!msg.contains("missing required key") && !msg.contains("fileWrittenId"),
            "the camelCase arg bound to file_written_id (not a deserialize error): {msg}");
    }
```

  Run it (expect PASS after the command + feature exist): `cargo test -p air_agent_desktop engine_undo_apply_binds_camelcase_arg_over_ipc`
  Expected output: `test result: ok. 1 passed`.

- [ ] Build: `cargo build -p air_agent_desktop`
  Expected output: `Finished` with no errors.

- [ ] Commit:
  `git add apps/desktop/src-tauri/src/engine/mod.rs apps/desktop/src-tauri/src/commands/engine.rs apps/desktop/src-tauri/src/main.rs apps/desktop/src-tauri/Cargo.toml`
  `git commit -m "feat(desktop): undo_apply op + command + command-layer IPC arg-binding test"`

---

### Task 11: TS twins + pure render helpers (`diffView`, `proposalView`)

**Files:**
- Modify: `apps/desktop/src/api/engine.ts`
- Create: `apps/desktop/src/review/diffView.ts`, `apps/desktop/src/review/diffView.test.ts`
- Create: `apps/desktop/src/review/proposalView.ts`, `apps/desktop/src/review/proposalView.test.ts`

- [ ] Extend `apps/desktop/src/api/engine.ts` (grounding §4b). Add the new DTO types + `writable` on `FileRecordDto`, and the `invoke<T>` wrappers. Edit the `FileRecordDto` type:

```ts
export type FileRecordDto = { canonical_path: string; file_event_id: string; content_hash: string; grant_root: string; writable: boolean };
```

  Append the new types + wrappers at the end of the file:

```ts
export type ProposalDto = {
  id: string;
  target: string;
  op: string;
  new_content_hash: string;
  rationale: string;
  requires_loud_modal: boolean;
};
export type PreviewDto = {
  path: string;
  folder: string;
  rationale: string;
  op: string;
  old_text: string;
  new_text: string;
  requires_loud_modal: boolean;
  taint: string;
};
export type ApplyResultDto = { file_written_id: string };

export const setFolderWritable = (path: string, on: boolean): Promise<void> =>
  invoke<void>("engine_set_folder_writable", { path, on });
export const setProposalsEnabled = (enabled: boolean): Promise<void> =>
  invoke<void>("engine_set_proposals_enabled", { enabled });
export const listProposals = (): Promise<ProposalDto[]> => invoke<ProposalDto[]>("engine_list_proposals");
export const proposalPreview = (id: string): Promise<PreviewDto> =>
  invoke<PreviewDto>("engine_proposal_preview", { id });
export const applyProposal = (id: string, acknowledgedLoud: boolean): Promise<ApplyResultDto> =>
  invoke<ApplyResultDto>("engine_apply_proposal", { id, acknowledgedLoud });
export const declineProposal = (id: string, reason: string): Promise<void> =>
  invoke<void>("engine_decline_proposal", { id, reason });
export const undoApply = (fileWrittenId: string): Promise<void> =>
  invoke<void>("engine_undo_apply", { fileWrittenId });
```

- [ ] Write the failing `diffView` test `apps/desktop/src/review/diffView.test.ts` (vitest; mirrors `recallView.test.ts` §4c):

```ts
import { describe, it, expect } from "vitest";
import { inlineDiff } from "./diffView";

describe("inlineDiff", () => {
  it("marks a changed line as a removal then an addition", () => {
    const lines = inlineDiff("Alice works at Acme.\n", "Alice works at Globex.\n");
    expect(lines).toEqual([
      { kind: "del", text: "Alice works at Acme." },
      { kind: "add", text: "Alice works at Globex." },
    ]);
  });

  it("keeps unchanged lines as context", () => {
    const lines = inlineDiff("a\nb\nc\n", "a\nB\nc\n");
    expect(lines).toEqual([
      { kind: "ctx", text: "a" },
      { kind: "del", text: "b" },
      { kind: "add", text: "B" },
      { kind: "ctx", text: "c" },
    ]);
  });

  it("handles a pure addition (empty old)", () => {
    expect(inlineDiff("", "new line\n")).toEqual([{ kind: "add", text: "new line" }]);
  });

  it("returns nothing for identical input", () => {
    expect(inlineDiff("same\n", "same\n")).toEqual([{ kind: "ctx", text: "same" }]);
  });
});
```

- [ ] Run it (expect FAIL — `diffView` does not exist):
  `npm run test --workspace @air-agent/desktop -- src/review/diffView.test.ts`
  Expected output: vitest fails to resolve `./diffView`.

- [ ] Implement `apps/desktop/src/review/diffView.ts`. A simple line-based LCS unified diff (deterministic, no deps), emitting context/del/add lines. Trailing newline trimmed per line; an empty string yields no lines:

```ts
/** One rendered diff line. `del` = removed (old), `add` = added (new), `ctx` = unchanged. */
export type DiffLine = { kind: "del" | "add" | "ctx"; text: string };

/** Split into lines, dropping a single trailing empty line from a trailing newline. */
function toLines(s: string): string[] {
  if (s === "") return [];
  const parts = s.split("\n");
  if (parts.length > 0 && parts[parts.length - 1] === "") parts.pop();
  return parts;
}

/**
 * Compute an inline unified diff (old vs new) as a flat list of lines. Uses a classic
 * longest-common-subsequence (LCS) line match so unchanged lines stay context and a changed
 * line shows as a `del` immediately followed by an `add`. Pure + deterministic.
 */
export function inlineDiff(oldText: string, newText: string): DiffLine[] {
  const a = toLines(oldText);
  const b = toLines(newText);
  const n = a.length;
  const m = b.length;
  // LCS length table.
  const lcs: number[][] = Array.from({ length: n + 1 }, () => new Array<number>(m + 1).fill(0));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      lcs[i][j] = a[i] === b[j] ? lcs[i + 1][j + 1] + 1 : Math.max(lcs[i + 1][j], lcs[i][j + 1]);
    }
  }
  const out: DiffLine[] = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (a[i] === b[j]) {
      out.push({ kind: "ctx", text: a[i] });
      i++;
      j++;
    } else if (lcs[i + 1][j] >= lcs[i][j + 1]) {
      out.push({ kind: "del", text: a[i] });
      i++;
    } else {
      out.push({ kind: "add", text: b[j] });
      j++;
    }
  }
  while (i < n) out.push({ kind: "del", text: a[i++] });
  while (j < m) out.push({ kind: "add", text: b[j++] });
  return out;
}
```

- [ ] Run it (expect PASS): `npm run test --workspace @air-agent/desktop -- src/review/diffView.test.ts`
  Expected output: `4 passed`.

- [ ] Write the failing `proposalView` test `apps/desktop/src/review/proposalView.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { toProposalRow } from "./proposalView";
import type { ProposalDto } from "../api/engine";

const base: ProposalDto = {
  id: "p1",
  target: "/home/me/notes/alice.md",
  op: "edit",
  new_content_hash: "abc",
  rationale: "Alice now works at Globex",
  requires_loud_modal: false,
};

describe("toProposalRow", () => {
  it("derives the basename + folder and passes through the Why", () => {
    const r = toProposalRow(base);
    expect(r.id).toBe("p1");
    expect(r.fileName).toBe("alice.md");
    expect(r.folder).toBe("/home/me/notes");
    expect(r.why).toBe("Alice now works at Globex");
    expect(r.risky).toBe(false);
    expect(r.opLabel).toBe("Edit");
  });

  it("flags a loud-modal proposal as risky and labels delete", () => {
    const r = toProposalRow({ ...base, requires_loud_modal: true, op: "delete" });
    expect(r.risky).toBe(true);
    expect(r.opLabel).toBe("Delete");
  });

  it("falls back to the full path when there is no separator", () => {
    const r = toProposalRow({ ...base, target: "alice.md" });
    expect(r.fileName).toBe("alice.md");
    expect(r.folder).toBe("");
  });
});
```

- [ ] Run it (expect FAIL): `npm run test --workspace @air-agent/desktop -- src/review/proposalView.test.ts`
  Expected output: vitest fails to resolve `./proposalView`.

- [ ] Implement `apps/desktop/src/review/proposalView.ts`:

```ts
import type { ProposalDto } from "../api/engine";

/** A display row for one queued proposal (pure: path split, op label, risk flag). */
export type ProposalRow = {
  id: string;
  fileName: string;
  folder: string;
  why: string;
  risky: boolean;
  opLabel: string;
};

const OP_LABEL: Record<string, string> = { edit: "Edit", create: "Create", delete: "Delete" };

/** Map a proposal DTO to a display row. `risky` mirrors the propose-time loud-modal flag. */
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
  };
}
```

- [ ] Run it (expect PASS): `npm run test --workspace @air-agent/desktop -- src/review/proposalView.test.ts`
  Expected output: `3 passed`.

- [ ] Typecheck the new TS:
  `npm run typecheck`
  Expected output: no errors.

- [ ] Commit:
  `git add apps/desktop/src/api/engine.ts apps/desktop/src/review/diffView.ts apps/desktop/src/review/diffView.test.ts apps/desktop/src/review/proposalView.ts apps/desktop/src/review/proposalView.test.ts`
  `git commit -m "feat(desktop): engine.ts twins for SP4 commands + diffView/proposalView helpers"`

---

### Task 12: Review destination — `App.tsx` nav badge + `ReviewPanel.tsx`

**Files:**
- Modify: `apps/desktop/src/App.tsx`
- Create: `apps/desktop/src/review/ReviewPanel.tsx`
- Create: `apps/desktop/src/review/applyFlow.ts`, `apps/desktop/src/review/applyFlow.test.ts` (pure loud-confirm decision helper + vitest, so the ack flow is unit-tested)

- [ ] Add the `"review"` view + a `ReviewNavButton` with a pending-count badge + the body-ternary branch in `apps/desktop/src/App.tsx` (grounding §3). Edit the `View` type (line 30):

```ts
type View = "identity" | "inbox" | "memory" | "review" | "settings";
```

  Add the panel import near the other panel imports (e.g. after the MemoryPanel import, §3 note). `useIdentity` is the existing hook `Shell()` already uses (grounding §3b):

```tsx
import { ReviewPanel } from "./review/ReviewPanel";
import { listProposals } from "./api/engine";
```

  Add the nav button + ternary arm. Replace the `<nav>` line for memory/settings to insert Review between them, and extend the body ternary (grounding §3b):

```tsx
        <Button variant={view === "memory" ? "primary" : "secondary"} onClick={() => setView("memory")}>Memory</Button>
        <ReviewNavButton active={view === "review"} onClick={() => setView("review")} />
        <Button variant={view === "settings" ? "primary" : "secondary"} onClick={() => setView("settings")}>Settings</Button>
      </nav>
      {view === "identity" ? <IdentityPanel /> : view === "inbox" ? <InboxPanel /> : view === "memory" ? <MemoryPanel /> : view === "review" ? <ReviewPanel /> : <AirSettings />}
```

  Add `ReviewNavButton` after `InboxNavButton` (grounding §3c). M4 fix: it does NOT poll pre-onboarding (the engine returns `NotOnboarded` and would spam) — it gates on `useIdentity().identity`, fetches the count once when identity appears, and only runs the 5 s interval while the Review tab is `active`. Failures silently hide the badge (count 0):

```tsx
function ReviewNavButton({ active, onClick }: { active: boolean; onClick: () => void }) {
  const { identity } = useIdentity();
  const [count, setCount] = useState(0);
  useEffect(() => {
    if (!identity) { setCount(0); return; } // no engine before onboarding → don't poll.
    let alive = true;
    const refresh = () => {
      listProposals()
        .then((ps) => { if (alive) setCount(ps.length); })
        .catch(() => { if (alive) setCount(0); });
    };
    refresh(); // one-shot whenever identity present (keeps the badge fresh on tab switch).
    // Only the active tab pays the 5 s poll; inactive tabs rely on the one-shot above.
    const id = active ? setInterval(refresh, 5000) : undefined;
    return () => { alive = false; if (id) clearInterval(id); };
  }, [identity, active]);
  return (
    <Button variant={active ? "primary" : "secondary"} onClick={onClick}>
      Review{count > 0 ? ` (${count})` : ""}
    </Button>
  );
}
```

  Ensure `useEffect`/`useState` are imported at the top of `App.tsx` (the file already uses `useState`; add `useEffect` if absent). `useIdentity` is already imported (used by `Shell`).

- [ ] Create `apps/desktop/src/review/ReviewPanel.tsx` using the INLINE-style `Button`/`Card` family (grounding §4a/§4d — MemoryPanel/SourcesPanel pattern; NOT the `ui/` kit, NOT `ChangeCard.tsx`). It lists proposals, opens a per-proposal preview with inline diff, Approve / Decline, and a "Recently applied" Undo strip. The loud-confirm modal is shown either when the propose-time `requires_loud_modal` HINT is set OR when the `apply_proposal` op bounces back `NeedsLoudConfirm` (the op is authoritative — item 2); the modal's "Apply anyway" retries with `acknowledged_loud = true`. Handlers use the `setBusy/try/catch/finally` pattern (§4d note):

```tsx
import { useEffect, useState } from "react";
import { Card } from "../components/Card";
import { Button } from "../components/Button";
import {
  listProposals, proposalPreview, applyProposal, declineProposal, undoApply,
  type ProposalDto, type PreviewDto,
} from "../api/engine";
import { toProposalRow } from "./proposalView";
import { inlineDiff } from "./diffView";

/** How often the queue refreshes while the Review tab is open. */
const POLL_MS = 5000;

type Applied = { fileWrittenId: string; fileName: string };

export function ReviewPanel() {
  const [proposals, setProposals] = useState<ProposalDto[]>([]);
  const [unavailable, setUnavailable] = useState(false);
  const [openId, setOpenId] = useState<string | null>(null);
  const [preview, setPreview] = useState<PreviewDto | null>(null);
  const [previewing, setPreviewing] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirmFor, setConfirmFor] = useState<string | null>(null);
  const [reviewed, setReviewed] = useState(false);
  const [applied, setApplied] = useState<Applied[]>([]);

  const refresh = async () => {
    try {
      setProposals(await listProposals());
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

  const onOpen = async (id: string) => {
    setOpenId(id);
    setPreview(null);
    setError(null);
    setPreviewing(true);
    try {
      setPreview(await proposalPreview(id));
    } catch (e) {
      setError(String(e));
    } finally {
      setPreviewing(false);
    }
  };

  // The engine op is authoritative for the loud-confirm (item 2 / G1): we call
  // applyProposal(id, acknowledged). When the FRESH re-gate is loud and we passed false, the op
  // returns a `NeedsLoudConfirm` error string → open the modal and retry with true. A `Stale`
  // error means the file changed under us → clear + reload the preview (MIN-2) so the next render
  // shows the new baseline, not the stale diff.
  const isNeedsLoudConfirm = (msg: string) => msg.includes("needs an explicit review confirmation");
  const isStale = (msg: string) => msg.includes("changed since this was suggested");

  const doApply = async (id: string, fileName: string, acknowledged: boolean) => {
    setBusy(true);
    setError(null);
    try {
      const r = await applyProposal(id, acknowledged);
      setApplied((prev) => [{ fileWrittenId: r.file_written_id, fileName }, ...prev]);
      setOpenId(null);
      setPreview(null);
      setConfirmFor(null);
      setReviewed(false);
      await refresh();
    } catch (e) {
      const msg = String(e);
      if (!acknowledged && isNeedsLoudConfirm(msg)) {
        // Authoritative loud gate fired — surface the "I've reviewed this" modal, no error text.
        setConfirmFor(id);
        setReviewed(false);
      } else if (isStale(msg)) {
        setError("The file changed since this was suggested — reloading the diff.");
        setConfirmFor(null);
        setReviewed(false);
        if (openId === id) { setPreview(null); void onOpen(id); } // MIN-2: reload the baseline.
        await refresh();
      } else {
        setError(msg);
      }
    } finally {
      setBusy(false);
    }
  };

  // Pre-show the modal if the propose-time HINT is loud; otherwise try the op (which still
  // re-checks loud on the fresh verdict and will bounce back NeedsLoudConfirm if needed).
  const onApprove = (id: string, requiresLoudHint: boolean, fileName: string) => {
    if (requiresLoudHint) {
      setConfirmFor(id);
      setReviewed(false);
    } else {
      void doApply(id, fileName, false);
    }
  };

  const onDecline = async (id: string) => {
    setBusy(true);
    setError(null);
    try {
      await declineProposal(id, "declined in Review");
      setOpenId(null);
      setPreview(null);
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
      await undoApply(fileWrittenId);
      setApplied((prev) => prev.filter((a) => a.fileWrittenId !== fileWrittenId));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  if (unavailable) {
    return (
      <Card>
        <h2 style={{ margin: 0 }}>Review</h2>
        <p style={{ color: "#666" }}>Couldn’t reach the memory engine. Set up your identity first, then enable a folder for edits.</p>
      </Card>
    );
  }

  return (
    <div>
      <h2 style={{ margin: "0 0 8px" }}>Review</h2>
      {error ? <p style={{ color: "#b00", fontSize: 13 }}>{error}</p> : null}

      {proposals.length === 0 ? (
        <Card>
          <p style={{ color: "#666" }}>
            No changes to review. When the brain learns something that contradicts a file in an
            edit-enabled folder (and evolve is on), proposed rewrites appear here.
          </p>
          <p style={{ color: "#888", fontSize: 12 }}>
            Note: only contradictions learned <em>after</em> you enable a folder for edits are
            proposed — corrections learned earlier won’t be re-applied retroactively.
          </p>
        </Card>
      ) : (
        proposals.map((p) => {
          const row = toProposalRow(p);
          const isOpen = openId === p.id;
          return (
            <Card key={p.id}>
              <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", gap: 8 }}>
                <div>
                  <div style={{ fontWeight: 600 }}>
                    {row.opLabel}: <code>{row.fileName}</code>{" "}
                    {row.risky ? <span style={{ color: "#b00", fontSize: 12 }}>⚠ needs careful review</span> : null}
                  </div>
                  <div style={{ color: "#666", fontSize: 12 }}><code>{row.folder}</code> · enabled ✓</div>
                  <div style={{ fontSize: 13, marginTop: 4 }}>Why: {row.why}</div>
                </div>
                <Button variant="secondary" onClick={() => (isOpen ? setOpenId(null) : void onOpen(p.id))}>
                  {isOpen ? "Hide" : "Preview"}
                </Button>
              </div>

              {isOpen ? (
                <div style={{ marginTop: 8 }}>
                  {previewing ? (
                    <p style={{ color: "#666", fontSize: 13 }}>Loading preview…</p>
                  ) : preview ? (
                    <>
                      <pre style={{ background: "#f6f6f6", padding: 8, fontSize: 12, overflowX: "auto", margin: 0 }}>
                        {inlineDiff(preview.old_text, preview.new_text).map((line, idx) => (
                          <div
                            key={idx}
                            style={{
                              color: line.kind === "del" ? "#b00" : line.kind === "add" ? "#070" : "#444",
                              background: line.kind === "del" ? "#fdecea" : line.kind === "add" ? "#eafaef" : "transparent",
                            }}
                          >
                            {line.kind === "del" ? "- " : line.kind === "add" ? "+ " : "  "}
                            {line.text}
                          </div>
                        ))}
                      </pre>
                      <div style={{ display: "flex", gap: 8, marginTop: 8 }}>
                        <Button variant="primary" disabled={busy} onClick={() => onApprove(p.id, preview.requires_loud_modal, row.fileName)}>
                          Approve
                        </Button>
                        <Button variant="secondary" disabled={busy} onClick={() => void onDecline(p.id)}>
                          Decline
                        </Button>
                      </div>
                    </>
                  ) : null}
                </div>
              ) : null}
            </Card>
          );
        })
      )}

      {confirmFor ? (
        <Card>
          <div style={{ fontWeight: 600, color: "#b00" }}>This change looks risky</div>
          <p style={{ fontSize: 13, color: "#444" }}>
            The new content matches a secret- or value-shaped pattern. Confirm you’ve read the diff before applying.
          </p>
          <label style={{ display: "flex", gap: 6, alignItems: "center", fontSize: 13 }}>
            <input type="checkbox" checked={reviewed} onChange={(e) => setReviewed(e.target.checked)} />
            I’ve reviewed this
          </label>
          <div style={{ display: "flex", gap: 8, marginTop: 8 }}>
            <Button
              variant="primary"
              disabled={!reviewed || busy}
              onClick={() => {
                const target = proposals.find((p) => p.id === confirmFor);
                if (target) void doApply(confirmFor, toProposalRow(target).fileName, true);
              }}
            >
              Apply anyway
            </Button>
            <Button variant="secondary" disabled={busy} onClick={() => { setConfirmFor(null); setReviewed(false); }}>
              Cancel
            </Button>
          </div>
        </Card>
      ) : null}

      {applied.length > 0 ? (
        <Card>
          <div style={{ fontWeight: 600 }}>Recently applied</div>
          <ul style={{ paddingLeft: 18, fontSize: 13 }}>
            {applied.map((a) => (
              <li key={a.fileWrittenId} style={{ marginBottom: 4 }}>
                <code>{a.fileName}</code>{" "}
                <button onClick={() => void onUndo(a.fileWrittenId)} disabled={busy} style={{ marginLeft: 8 }}>Undo</button>
              </li>
            ))}
          </ul>
        </Card>
      ) : null}
    </div>
  );
}
```

- [ ] Typecheck:
  `npm run typecheck`
  Expected output: no errors.

- [ ] Write + run the `applyFlow` unit test (item 2 — proves the panel CANNOT reach a successful apply for a loud proposal without the ack). Create `apps/desktop/src/review/applyFlow.test.ts`:

```ts
import { describe, it, expect, vi } from "vitest";
import { runApprove, classifyApplyError } from "./applyFlow";

describe("classifyApplyError", () => {
  it("detects NeedsLoudConfirm and Stale from the engine message text", () => {
    expect(classifyApplyError("this change needs an explicit review confirmation: ...")).toBe("loud");
    expect(classifyApplyError("the file changed since this was suggested: ...")).toBe("stale");
    expect(classifyApplyError("some other error")).toBe("other");
  });
});

describe("runApprove", () => {
  it("a loud proposal cannot apply without the ack: first call bounces, second (acked) succeeds", async () => {
    // Mock op: rejects NeedsLoudConfirm when acknowledged=false, succeeds when true.
    const op = vi.fn(async (_id: string, acknowledged: boolean) => {
      if (!acknowledged) throw new Error("this change needs an explicit review confirmation: x");
      return { file_written_id: "fw1" };
    });
    // First attempt without ack → returns { needsLoud: true }, op called once with false, no write.
    const first = await runApprove(op, "p1", false);
    expect(first).toEqual({ needsLoud: true });
    expect(op).toHaveBeenCalledTimes(1);
    expect(op).toHaveBeenLastCalledWith("p1", false);
    // Second attempt WITH ack → applied.
    const second = await runApprove(op, "p1", true);
    expect(second).toEqual({ applied: "fw1" });
    expect(op).toHaveBeenLastCalledWith("p1", true);
  });

  it("a non-loud proposal applies on the first call", async () => {
    const op = vi.fn(async (_id: string, _ack: boolean) => ({ file_written_id: "fw2" }));
    expect(await runApprove(op, "p2", false)).toEqual({ applied: "fw2" });
  });
});
```

- [ ] Run it (expect FAIL): `npm run test --workspace @air-agent/desktop -- src/review/applyFlow.test.ts`
  Expected output: vitest fails to resolve `./applyFlow`.

- [ ] Implement `apps/desktop/src/review/applyFlow.ts` (pure: the loud/stale classification + the approve decision the panel uses, so it is unit-testable away from React):

```ts
import type { ApplyResultDto } from "../api/engine";

/** Classify an engine apply error by its message text (the typed errors stringify distinctly). */
export type ApplyErrorKind = "loud" | "stale" | "other";
export function classifyApplyError(message: string): ApplyErrorKind {
  if (message.includes("needs an explicit review confirmation")) return "loud";
  if (message.includes("changed since this was suggested")) return "stale";
  return "other";
}

/** The outcome of one approve attempt. */
export type ApproveOutcome =
  | { applied: string }   // file_written id
  | { needsLoud: true }   // the op refused — caller must show the modal + retry with ack=true
  | { stale: true }       // the file changed — caller reloads the preview
  | { error: string };

/** Run an approve through the apply op. `op(id, acknowledged)` is `applyProposal`. The op is the
 *  authoritative loud gate: without the ack a loud proposal throws NeedsLoudConfirm → `needsLoud`. */
export async function runApprove(
  op: (id: string, acknowledged: boolean) => Promise<ApplyResultDto>,
  id: string,
  acknowledged: boolean,
): Promise<ApproveOutcome> {
  try {
    const r = await op(id, acknowledged);
    return { applied: r.file_written_id };
  } catch (e) {
    const msg = String(e instanceof Error ? e.message : e);
    const kind = classifyApplyError(msg);
    if (kind === "loud" && !acknowledged) return { needsLoud: true };
    if (kind === "stale") return { stale: true };
    return { error: msg };
  }
}
```

  > The `ReviewPanel`'s `doApply` (above) inlines the same classification for brevity; if you prefer zero duplication, have `doApply` call `runApprove(applyProposal, id, acknowledged)` and switch on the outcome. Either way the unit test pins the invariant: **no `applied` result for a loud proposal unless `acknowledged === true`.**

- [ ] Run it (expect PASS): `npm run test --workspace @air-agent/desktop -- src/review/applyFlow.test.ts`
  Expected output: `3 passed`.

- [ ] Run the full desktop vitest suite to confirm helpers + existing tests are green:
  `npm run test --workspace @air-agent/desktop`
  Expected output: all test files pass (incl. `diffView`, `proposalView`, `applyFlow`, `recallView`, etc.).

- [ ] Commit:
  `git add apps/desktop/src/App.tsx apps/desktop/src/review/ReviewPanel.tsx apps/desktop/src/review/applyFlow.ts apps/desktop/src/review/applyFlow.test.ts`
  `git commit -m "feat(desktop): Review destination — nav badge + ReviewPanel (diff, op-enforced loud-confirm, undo)"`

---

### Task 13: Settings → Folders — "Allow edits" toggle + "Allow All" master

**Files:**
- Modify: `apps/desktop/src/sources/SourcesPanel.tsx`
- Create: `apps/desktop/src/sources/writableGrants.ts`, `apps/desktop/src/sources/writableGrants.test.ts`

- [ ] Write the failing helper test `apps/desktop/src/sources/writableGrants.test.ts`. The helper derives, per active grant root, whether it is write-enabled — computed from the files' `writable` flag (a grant root is writable iff any file under it is writable; with no files, fall back to false). It also computes whether ALL active roots are writable (for the master toggle):

```ts
import { describe, it, expect } from "vitest";
import { writableRoots, allWritable } from "./writableGrants";
import type { FileRecordDto } from "../api/engine";

const file = (grant_root: string, writable: boolean): FileRecordDto => ({
  canonical_path: `${grant_root}/x.md`, file_event_id: "e", content_hash: "h", grant_root, writable,
});

describe("writableRoots", () => {
  it("marks a root writable when any of its files is writable", () => {
    const roots = writableRoots([file("/a", true), file("/a", false), file("/b", false)]);
    expect(roots.has("/a")).toBe(true);
    expect(roots.has("/b")).toBe(false);
  });

  it("is empty with no files", () => {
    expect(writableRoots([]).size).toBe(0);
  });
});

describe("allWritable", () => {
  it("true only when every active root is writable", () => {
    const active = ["/a", "/b"];
    expect(allWritable(active, new Set(["/a", "/b"]))).toBe(true);
    expect(allWritable(active, new Set(["/a"]))).toBe(false);
  });
  it("false when there are no active roots", () => {
    expect(allWritable([], new Set())).toBe(false);
  });
});
```

- [ ] Run it (expect FAIL): `npm run test --workspace @air-agent/desktop -- src/sources/writableGrants.test.ts`
  Expected output: vitest fails to resolve `./writableGrants`.

- [ ] Implement `apps/desktop/src/sources/writableGrants.ts`:

```ts
import type { FileRecordDto } from "../api/engine";

/** The set of grant roots that are write-enabled (any file under the root is `writable`). */
export function writableRoots(files: FileRecordDto[]): Set<string> {
  const out = new Set<string>();
  for (const f of files) {
    if (f.writable) out.add(f.grant_root);
  }
  return out;
}

/** True iff there is at least one active root and every one of them is writable. */
export function allWritable(activeRoots: string[], writable: Set<string>): boolean {
  return activeRoots.length > 0 && activeRoots.every((r) => writable.has(r));
}
```

- [ ] Run it (expect PASS): `npm run test --workspace @air-agent/desktop -- src/sources/writableGrants.test.ts`
  Expected output: `4 passed`.

- [ ] Wire the toggles into `apps/desktop/src/sources/SourcesPanel.tsx` (grounding §5). Add imports + helper usage, a per-row "Allow edits" raw `<button>` (mirrors the raw "Revoke" button), an "Allow All" header button, and an inline evolve-off offer. Edit the import block (grounding §5, lines 1-7):

```tsx
import { useEffect, useState } from "react";
import { Button } from "../components/Button";
import {
  pickFolder, addGrant, revokeGrant, listGrants, runIngest, listFiles,
  setFolderWritable, setProposalsEnabled, setEvolveEnabled, evolveStatus,
  type GrantDto, type FileRecordDto, type IngestReportDto,
} from "../api/engine";
import { activeGrants } from "./grants";
import { ingestSummary } from "./ingestSummary";
import { writableRoots, allWritable } from "./writableGrants";
```

  Add `evolveOn` state next to the others (after `ingestError`):

```tsx
  const [evolveOn, setEvolveOn] = useState<boolean | null>(null);
```

  In `refresh`, also read evolve status (so the offer knows whether evolve is off). Replace the body of `refresh`:

```tsx
  const refresh = async () => {
    try {
      const [g, f, ev] = await Promise.all([listGrants(), listFiles(), evolveStatus()]);
      setGrants(g);
      setFiles(f);
      setEvolveOn(ev.enabled);
      setUnavailable(false);
    } catch {
      setUnavailable(true);
    }
  };
```

  **A1 — `active` binding (definitive):** move the single existing `const active = activeGrants(grants);` line (grounding §5 line 71) to sit **immediately after the state declarations**, before the handlers that reference it. Do NOT add a second `active` binding anywhere — there is exactly one, and it is in scope for both the handlers below and the render JSX.

  Add handlers after `onRevoke` (grounding §5 ~line 74). Enabling a folder for edits is the first-folder-enable trigger that turns proposals on under the hood. Both mutators use the standard busy/error pattern (M5): they `setBusy(true)`, clear the error, run, `setError` on failure, and `setBusy(false)` in `finally`; `onAllowAll` collects per-folder failures (continue-and-report) and only flips proposals on after the loop. A shared `busy` already exists in the panel (grounding §5 declares `const [busy, setBusy]`); reuse it so the toggle buttons disable while a mutation is in flight:

```tsx
  const onToggleWritable = async (root: string, on: boolean) => {
    setBusy(true);
    setEditError(null);
    try {
      await setFolderWritable(root, on);
      if (on) await setProposalsEnabled(true); // Lock-1 enablement, under the hood.
      await refresh();
    } catch (e) {
      setEditError(String(e));
    } finally {
      setBusy(false);
    }
  };
  const onAllowAll = async (on: boolean) => {
    setBusy(true);
    setEditError(null);
    const failures: string[] = [];
    try {
      for (const g of active) {
        try {
          await setFolderWritable(g.canonical_root, on);
        } catch (e) {
          failures.push(`${g.canonical_root}: ${String(e)}`); // continue-and-report.
        }
      }
      if (on && failures.length < active.length) await setProposalsEnabled(true);
      if (failures.length > 0) setEditError(`Some folders failed: ${failures.join("; ")}`);
      await refresh();
    } finally {
      setBusy(false);
    }
  };
  const onEnableEvolve = async () => {
    setBusy(true);
    setEditError(null);
    try {
      await setEvolveEnabled(true);
      await refresh();
    } catch (e) {
      setEditError(String(e));
    } finally {
      setBusy(false);
    }
  };
```

  Add an `editError` state next to the others (M5 — a dedicated error line for the edit toggles, separate from `ingestError`):

```tsx
  const [editError, setEditError] = useState<string | null>(null);
```

  Compute the writable derivation in the render body (after the relocated `const active = activeGrants(grants);`):

```tsx
  const writable = writableRoots(files);
  const everyWritable = allWritable(active.map((g) => g.canonical_root), writable);
```

  Add the "Allow All" master in the header action area (grounding §5 lines 64-69, beside the existing Add/Ingest buttons), disabled while busy (M5):

```tsx
        <Button variant="secondary" onClick={() => void onAllowAll(!everyWritable)} disabled={busy || active.length === 0}>
          {everyWritable ? "Disallow all edits" : "Allow all edits"}
        </Button>
```

  Surface the edit-toggle error (M5), beside the existing ingest error line:

```tsx
      {editError ? <p style={{ fontSize: 13, color: "#b00" }}>{editError}</p> : null}
```

  Extend each grant row (grounding §5 lines 76-81) to show + toggle "Allow edits" beside "Revoke":

```tsx
        {active.map((g) => {
          const editable = writable.has(g.canonical_root);
          return (
            <li key={g.canonical_root} style={{ marginBottom: 4 }}>
              <code>{g.canonical_root}</code>{" "}
              <button disabled={busy} onClick={() => void onToggleWritable(g.canonical_root, !editable)} style={{ marginLeft: 8 }}>
                {editable ? "Disallow edits" : "Allow edits"}
              </button>
              <button disabled={busy} onClick={() => onRevoke(g.canonical_root)} style={{ marginLeft: 8 }}>Revoke</button>
            </li>
          );
        })}
```

  Add the evolve-off offer below the grant list. **G2 fix:** the trigger is `writable.size > 0 && evolveOn === false` — it must show whenever ANY folder is editable but evolve is off, including when the user enables their ONLY folder (where `everyWritable` is true, so the old `everyWritable === false` clause wrongly hid it):

```tsx
      {writable.size > 0 && evolveOn === false ? (
        <p style={{ fontSize: 13, color: "#b00" }}>
          Edits are enabled, but the learning loop is off, so no changes will be proposed.{" "}
          <button disabled={busy} onClick={() => void onEnableEvolve()}>Turn learning on</button>
        </p>
      ) : null}
```

  Add the M3 one-time "retroactive" note, shown whenever at least one folder is editable (so a user who just enabled a folder learns that prior contradictions won't be re-applied):

```tsx
      {writable.size > 0 ? (
        <p style={{ fontSize: 12, color: "#888" }}>
          Heads up: corrections the brain learned <em>before</em> you enabled a folder won’t be
          re-applied retroactively — only new contradictions are proposed.
        </p>
      ) : null}
```

  (New folders default read-only: a freshly added grant has no write-grant and no `writable` file, so `writable.has(root)` is false until "Allow edits" is clicked. No code needed beyond the default.)

- [ ] Typecheck:
  `npm run typecheck`
  Expected output: no errors.

- [ ] Run the full desktop vitest suite:
  `npm run test --workspace @air-agent/desktop`
  Expected output: all test files pass (incl. `writableGrants`).

- [ ] Commit:
  `git add apps/desktop/src/sources/SourcesPanel.tsx apps/desktop/src/sources/writableGrants.ts apps/desktop/src/sources/writableGrants.test.ts`
  `git commit -m "feat(desktop): Settings → Folders Allow-edits toggle + Allow-All + evolve-off offer"`

---

### Task 14: Gates + manual-launch checklist (no code)

**Files:** none (verification-only task).

- [ ] Engine build + tests green:
  `cargo test -p bossclaw-core`
  Expected output: `test result: ok.` with 0 failed.

- [ ] Engine clippy (the security-feature lint gate):
  `cargo clippy -p bossclaw-core --features ollama -- -D warnings`
  Expected output: `Finished` with no warnings (no clippy errors).

- [ ] Desktop build + tests + clippy green:
  `cargo build -p air_agent_desktop && cargo test -p air_agent_desktop && cargo clippy -p air_agent_desktop -- -D warnings`
  Expected output: `Finished`; `test result: ok.`; clippy clean.

- [ ] Frontend typecheck + vitest:
  `npm run typecheck --workspace @air-agent/desktop && npm run test --workspace @air-agent/desktop`
  Expected output: no type errors; all vitest files pass.

- [ ] Two-graph network-posture guard stays green (the SP1–SP3 invariant: embedder is network-free; reasoner is loopback-only; no new network surface in SP4). Run the exact check CI uses (`.github/workflows/build.yml` "Engine network-posture guard (two-graph)") — neither grep may match:
  ```bash
  cargo tree -p bossclaw-core -e normal --prefix none | grep -qE '^(hf-hub|ureq|reqwest)( |$)' && echo "FAIL: network crate in DEFAULT graph" || echo "default graph OK (zero network clients)"
  cargo tree -p bossclaw-core -e normal --features ollama --prefix none | grep -qE '^(hf-hub|reqwest)( |$)' && echo "FAIL: hf-hub/reqwest in ollama graph" || echo "ollama graph OK (ureq-only)"
  ```
  Expected output: `default graph OK (zero network clients)` and `ollama graph OK (ureq-only)` — neither grep matches, so no new outbound surface.

- [ ] Manual launch (signed debug build per `scripts/dev-build-signed.sh`; fixtures dir e.g. `~/air-note-qa`, identity "Aria Novak"):
  - [ ] Settings → Folders: add a folder, click "Allow edits"; with evolve off, confirm the inline "Turn learning on" offer appears and enables evolve.
  - [ ] With Ollama up + evolve on, seed a contradiction (ingest a file, then a memory that corrects it) and run evolve until a proposal appears; confirm the Review nav badge shows a count.
  - [ ] Review → open the proposal → confirm the inline before/after diff renders → Approve → confirm the file on disk changed → use "Recently applied" → Undo → confirm the file is restored.
  - [ ] Decline a different proposal → confirm it leaves the queue and never returns.
  - [ ] Stale-file path: after a proposal exists, edit the target file by hand, then Approve → confirm the apply fails closed with a "the file changed since this was suggested" message, the file is left as the manual edit, and the diff reloads to the new baseline (MIN-2).
  - [ ] Risky path: if a proposal touches secret/value-shaped content, confirm Approve shows the loud "I've reviewed this" modal AND that the write only lands after the ack — the engine `apply_proposal` returns `NeedsLoudConfirm` and refuses the first (un-acked) attempt (item 2 / G1).
  - [ ] EXPECTED behavior (not a bug): a contradiction the brain learned BEFORE a folder was enabled for edits is NOT re-proposed when you later enable that folder (retroactive proposals are out of scope for SP4; the `invalidate` was already consumed). Confirm the Settings note + Review empty-state hint communicate this.

- [ ] Commit (checklist completion / any doc note only — no source changes expected here):
  `git add docs/superpowers/plans/2026-06-23-sp4-confirm-preview.md`
  `git commit -m "docs(sp4): record SP4 confirm/preview manual-QA checklist completion"`

---

## Decisions honored

- **Two locks** — folder write-grant (`set_folder_writable`, Task 4) AND per-change approval (`apply_proposal`, Task 8); either missing → no write. Engine re-enforces at `execute_write_resolving`.
- **Enable-folder-first** — proposals only for write-granted folders (engine change a, Task 2; reconcile skips un-writable targets).
- **Review = layout-agnostic top-level destination + badge** (Task 12: `View += "review"`, `ReviewNavButton` polls `listProposals().length`).
- **Inline unified diff** (`diffView.ts`, Task 11; rendered in `ReviewPanel`, Task 12).
- **Allow-All master** (Task 13).
- **Decline = final** (`decline_write_proposal`, Task 9; terminal `write_declined`).
- **Loud confirm for `requires_loud_modal`, engine-enforced** (Task 8: `apply_proposal(id, acknowledged_loud)` recomputes loud on the FRESH verdict and returns `NeedsLoudConfirm` without the ack; Task 12 modal with "I've reviewed this" supplies it). The propose-time flag is only a UI hint.
- **Mandates stay OFF** (Task 3: `prime_switches` always forces `mandates_enabled` off).
- **Base-fingerprint anti-clobber + re-gate at confirm, staleness fail-closed** (Task 8: compare the proposal's stored `base_content_hash` to the live file hash and fail closed as `Stale` BEFORE the fresh `propose_write` — the fresh propose re-bases on live bytes and cannot detect drift; then re-gate for the TOCTOU window + grant revoke; typed `Stale`/`Revoked`; file untouched on failure). Base persisted at emit in `verdict_summary` (Task 2) and projected on `PendingProposal` (Task 1).
- **Local-only, atomic + undo + audit** (Task 8/Task 10: `execute_write_resolving` temp+rename + signed `file_written`; `undo_write`).
- **Verdict shapes** — propose-time `{requires_loud_modal, taint, allowed}` (Task 6/Task 7 projection); apply-time `gated.verdict.reject_reason` (Option) re-gate (Task 8).
- **Per-element `#[cfg(unix)]` in `generate_handler!`** (one attribute line per new command, Tasks 4-10).
