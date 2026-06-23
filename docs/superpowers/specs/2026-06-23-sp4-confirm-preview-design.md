# Desktop Engine — Confirm & Apply (SP4) — Design

**Status:** **Draft** (2026-06-23) — brainstormed interactively with Peter; **pending the independent critic + security review** (same gate SP1–SP3 used). Sub-project **4 of 5** in the "engine-in-the-desktop" milestone.

**⚠️ First milestone with a `bossclaw-core` change.** SP1–SP3 were strictly desktop-only (zero engine edits). SP4 needs **three surgical engine changes** (§Engine changes), each touching the security core — this is the primary reason the spec must clear a full security review before any code.

## Context — the parent milestone

- **SP1 spine** (merged): one live, encrypted `EventLog` behind the `EngineHandle` chokepoint.
- **SP2 ingest** (merged): folder read-grants, ingest, persisted `model2vec` vectors.
- **SP3 recall + evolve** (merged, PR #45 `eb80d55`): Memory tab, hybrid recall, a local-Ollama evolve loop that extracts entities/links/dossiers — **OFF by default**, emitting **zero** `write_proposal`s (all three autonomy flags forced off by `prime_switches`).

The 5 sub-projects: **1 spine ✅ → 2 ingest ✅ → 3 recall + evolve ✅ → 4 confirm/preview (this doc) → 5 mandate management.**

SP4 is the payoff that makes the brain *act*: the evolve loop's reconciliation (M6b) finally **proposes file rewrites**, and the user gets a first-class surface to **review and approve** each one. Approved changes are applied through the engine's existing atomic, undoable, audited actuator.

## Goal

After the user **enables a folder for edits** (and the evolve loop is on), when the brain learns something that contradicts a file's contents, it **proposes a rewrite**. The user:

- sees pending changes in a dedicated **Review** destination (with a count badge),
- opens one to read a plain-English **"Why"** + an **inline before/after diff**,
- **Approves** (applied atomically, with undo + an audit record) or **Declines** (final),
- gets an **extra confirm** for risky edits (secret-looking content) and an **Undo** for anything already applied.

**Two locks, always:** (1) the folder is enabled for edits, **and** (2) the user approves that specific change. Either missing → nothing is written. Off by default.

## Decisions (resolved in brainstorming)

1. **Permission = enable-folder-first** (not ask-at-approval). A folder must carry a write-grant *before* the brain drafts edits for it. Rationale: the engine checks write-permission at draft time; preparing a suggestion for a folder it can't write to is wasted work (and historically left permanent "rejected" state). See §Engine change (a).
2. **Review is its own top-level destination** with a pending-count badge — written **layout-agnostic** (a "destination", not hard-wired as a top tab) so the deferred sidebar/search shell redesign can reposition it with zero rework.
3. **Review card** = file + folder + enabled status · "Why" · **inline unified diff** · Approve / Decline · undo note.
4. **Enabling** = a per-folder "Allow edits" toggle **+ an "Allow All" master**, in Settings → Folders, beside the existing "Read" grant. New folders start read-only (no silent escalation). "Allow All" off = revoke everywhere.
5. **Decline = final** (matches the engine's terminal `write_declined`).
6. **Risky edits** (secret-shaped content) get a loud confirm ("I've reviewed this"); **Undo** lives in a "Recently applied" strip in the Review destination.

## Non-goals (explicitly deferred)

- **Sidebar + global-search app-shell redesign → its own next sub-project.** Reshapes all navigation + the landing page; out of scope here. SP4 only assumes "Review" is a navigable destination.
- **Bulk / batch approve** → later. v1 reviews one change at a time.
- **Mandate management → SP5.** `mandates_enabled` stays forced OFF; the M6c emitter never runs in SP4.
- **Cloud `ReasonerProvider`** → later (the SP3 seam is reused unchanged).
- **Windows → M7.** All new Rust + the Review UI's engine calls are `#[cfg(unix)]`-gated, matching SP1–SP3.
- **Retroactive proposals.** A contradiction processed *while a folder was not yet editable* is not re-offered when the folder is later enabled (the contradiction's `invalidate` is already committed). Named limitation, accepted for SP4.

## Permission model — two locks, enable-folder-first

- **Lock 1 — folder write-grant.** The engine's `WRITE_GRANT` (separate from the SP2 read-grant; a read grant can never authorize a write). UI: Settings → Folders gains a per-folder **"Allow edits"** toggle + an **"Allow All"** master. Toggle on → `EventLog::add_write_grant(root)`; off → `revoke_write_grant(root)`. The write-grant root = the existing ingested (read-grant) folder root.
- **Lock 2 — per-change human approval** in the Review destination.
- **Preconditions for a suggestion to appear:** evolve loop on (SP3, needs local Ollama) **AND** `proposals_enabled` on (managed under the hood) **AND** the target's folder write-granted. When the user enables a folder for edits while evolve is off, the app **offers to turn evolve on** right there.
- **Off by default.** `prime_switches` keeps the engine's dangerous default-ON flags neutralized at startup; enabling proposals is an explicit, sticky user action (see §Engine change (b)).

## Engine changes (`bossclaw-core`) — three, surgical, security-reviewed

> Line numbers below are grounding references against current `main`; the implementation plan re-verifies exact locations (the engine's `log.rs` is ~6873 lines; `execute_write` at ~3096, reconcile gate at ~6250, flag getters at ~4786/4879/4936).

**(a) Reconcile: check-write-grant-first, skip (don't reject) on missing grant.**
In `reconcile_confirmed_contradiction` (`log.rs` ~6140–6280), move an `is_write_allowed(&rec.canonical_path)?` check to the **top** of the per-target loop (right after `is_reconcilable_target`). If not allowed, `continue` — **no LLM rewrite, no `propose_write`, no `append_write_rejected`.** Only a *real* gate failure (a `reject_reason`, e.g. symlink/taint) still records the terminal `write_rejected`. This distinguishes the pure no-grant case (`allowed == false && reject_reason.is_none()`) — which the engine already isolates — from genuine rejections.
- **Why:** matches the agreed "be smart, check the key first" behavior; eliminates wasted synthesis on un-editable folders; removes the footgun where an ingested-but-not-editable folder accrues permanent `write_rejected` markers (so enabling it later starts clean).
- **Test:** repurpose `reconcile_target_outside_write_grant_rejected_at_propose` (`tests/reconcile.rs:667`) → assert **no proposal AND no `write_rejected`** (skipped); add a follow-on asserting that granting write then re-running surfaces a proposal.

**(b) Enablement persistence — `prime_switches` must respect explicit user choices.**
Today `prime_switches` (`engine/mod.rs` ~276) does `if log.<flag>_enabled()? { set_<flag>_enabled(false)? }` for all three flags. Because the getter can't distinguish the engine's *default* `true` (never set) from a *user's explicit* `true`, this risks **resetting the user's choice on every launch**. SP4 fix: prime only force-OFF flags that were **never explicitly set**.
- Approach (finalized in the plan): add an engine predicate "was this key ever explicitly set?" (scan `config` events for the key) and gate the force-off on it; `proposals_enabled` and `evolve_enabled` then persist a user's explicit on/off across launches; **`mandates_enabled` stays forced OFF** until SP5 regardless.
- **This is the primary technical risk of SP4** — flagged for the security review.

**(c) "List pending proposals" projection — the one real engine gap.**
No engine API enumerates open proposals today (only the boolean `is_proposal_suppressed`). Add `pending_proposals()` folding `events_of_types([write_proposal, file_written, write_declined, write_rejected])` → `write_proposal`s with no resolver. Mirror the open/close/suppress semantics proven by `pending_projection_open_close_and_suppress` (`tests/reconcile.rs:280`).

## Desktop architecture

### Review destination (layout-agnostic)
- `App.tsx` `View` gains `"review"`; a nav entry renders with a pending-count badge (mirrors `InboxNavButton`). Written as a *destination* so the future shell redesign repositions it unchanged.
- `src/review/ReviewPanel.tsx` (built from the existing kit: `Button`, `SettingsSectionCard`, `StatusBadge`, `Loading`), plus pure render helpers `src/review/proposalView.ts` + `src/review/diffView.ts` with sibling `vitest` `.test.ts` files (mirrors SP3's `recallView.ts`/`evolveStatus.ts`).
- **Queue → card:** list of pending proposals → per-proposal card: file path + folder + "enabled ✓" · **"Why"** (the rationale) · **inline unified diff** (old = `std::fs::read(target)` via the engine; new = `get_proposal_bytes_checked(id, hash)`) · risk badge · **Approve** / **Decline** · a **"Recently applied"** strip with **Undo**.

### Commands + DTOs (`commands/engine.rs` conventions; registered `#[cfg(unix)]` in `main.rs`)
```
engine_list_proposals()                 -> Vec<ProposalDto>
engine_proposal_preview(id)             -> PreviewDto   // path, folder, rationale, op, old_text, new_text, requires_loud_modal, taint, diff_flags
engine_apply_proposal(id)               -> ApplyResultDto      // re-gate + execute_write_resolving; staleness → typed error
engine_decline_proposal(id, reason)     -> ()                   // decline_write_proposal
engine_undo_apply(file_written_id)      -> ()                   // undo_write
engine_set_folder_writable(path, on)    -> ()                   // add_write_grant / revoke_write_grant
engine_set_proposals_enabled(on)        -> ()                   // sticky; invoked under the hood on first folder-enable
```
Plus: extend the SP2 `FileRecordDto` (or a sibling) with a `writable: bool` flag so the Folders UI shows read/edit state in one call. DTOs (`#[derive(Serialize)]` + `From<…>`): `ProposalDto`, `PreviewDto`, `ApplyResultDto`. Hand-mirrored TS twins in `api/engine.ts`.

### EngineHandle ops (the chokepoint)
Each new op funnels through `get_or_open(onboarded)` → `spawn_blocking(move || log.<core>(…))` → `EngineOpError` mapping, exactly like SP2/SP3 ops. `apply_proposal` relies on the engine's internal `rename_lock` for write serialization (no new desktop lock).

### Apply flow (security-critical) — base-fingerprint check + re-gate at confirm
**Propose-time:** when the M6b reconciler emits a proposal it stores the gate's **`base_content_hash`** (the sha256 of the target's bytes at draft time; `WriteVerdict.base_content_hash`, `Some` for an Edit) inside the proposal's `verdict_summary` JSON — **no new event field or `append_write_proposal` signature**, just one more key in the object it already passes.

On **Approve(id)**:
1. Load the open proposal (carrying its recorded `base_content_hash`).
2. **Anti-clobber check (the true staleness detector):** read the live target, sha256 it, and compare to the recorded `base_content_hash`. If they differ (or no base is recorded) → **fail closed with `Stale`, before proposing or executing**: "the file changed since this was suggested — I'll take another look." *This must precede the fresh propose:* a fresh `propose_write` re-bases on the **live** file, so it can never tell that the file drifted since the proposal was drafted — without this explicit compare an interim user edit would be silently clobbered.
3. `get_proposal_bytes_checked(id, new_content_hash)` → bytes (**fail-closed** if the side-table row is missing/tampered).
4. **Fresh** `propose_write(WriteProposal { target, new_content: bytes, op: Edit, source_event_ids: lineage, rationale })` — re-gates against the current file **and** the write-grant; still guards the micro-TOCTOU window between the hash check and the rename, symlink/op-mismatch, and grant revocation. If `verdict.reject_reason.is_some()` → `Stale`; if `!verdict.allowed` → `Revoked`. **Do not apply.**
5. If `verdict.requires_loud_modal` → show the loud confirm before step 6.
6. `execute_write_resolving(gated, &id)` → atomic temp+rename, appends `file_written` (`resolves_proposal == id`), durably captures undo `pre_bytes`.

**Decline(id)** → `decline_write_proposal(id, reason)` (terminal `write_declined`, resolves the proposal). **Undo** → `undo_write(file_written_id)` (re-gated, hash-verified restore).

## Data flow

`evolve tick (enabled folder, write-granted) → reconcile drafts → write_proposal + proposal_bytes → engine_list_proposals → Review queue (badge) → engine_proposal_preview → user Approve → base-hash anti-clobber check → re-gate (fresh propose_write) → [loud confirm if flagged] → execute_write_resolving → file_written (audit) + undo captured`. Decline → `write_declined`. Folders not write-granted → skipped at reconcile (no proposal). 

## Failure / partial-state matrix

| Scenario | Result |
|---|---|
| Not onboarded → any `engine_*` | `Open(NotOnboarded)`; Review shows "set up your identity first" |
| Evolve off / Ollama down | no new proposals; Review shows empty + a hint to enable evolve |
| Folder not editable | reconcile skips it (engine change a) → no proposal; nothing burned |
| File changed on disk since proposal | the propose-time `base_content_hash` ≠ the live file's hash → apply fails closed as `Stale` BEFORE any propose/execute → "stale, I'll re-look"; nothing written |
| Write-grant revoked between propose and apply | re-gate fails closed → "edits no longer allowed here" |
| Proposal bytes GC'd / tampered | `get_proposal_bytes_checked` fails closed → "couldn't verify the change" |
| `requires_loud_modal` (secret-shaped) | apply blocked behind the "I've reviewed this" confirm |
| Decline | terminal `write_declined`; that exact fix never returns |
| Undo after the file changed again | `undo_write` re-gates + hash-checks → fails closed if diverged |
| Concurrent apply / evolve tick | engine `rename_lock` / `evolve_lock` serialize; no double write |

## Security invariants

- **Two locks, both required.** Folder write-grant **and** per-change approval; either missing → no write. Engine **independently re-enforces** the write-grant at `execute_write` (not just at the UI).
- **Base-fingerprint anti-clobber + re-gate at confirm.** Apply first compares the proposal's stored propose-time `base_content_hash` to the live file's current hash and fails closed (`Stale`) if they diverge — this is what actually catches an interim edit, since a fresh `propose_write` re-bases on live bytes and cannot. Apply never trusts the stored verdict; it then rebuilds the gate against the live file (TOCTOU window, symlink, grant revocation) → staleness/revocation fail closed. Either lock failing → nothing written.
- **Mandates stay OFF.** `mandates_enabled` forced off; the M6c emitter never runs → no autonomous mandate writes in SP4 (that's SP5).
- **Atomic + undoable + audited.** Every apply is temp+rename atomic, captures durable undo `pre_bytes` before mutating, and appends a signed `file_written` event (`verify_chain`-able).
- **Local only.** No new network surface; reasoner stays loopback-only (SP3); embedder network-free (two-graph guard stays green). No new secrets / keychain reads.
- **Taint preserved.** Proposals stay taint-stamped `external`; reasoner output is parsed as data, never authority (engine D2/D8 rules unchanged).
- **Least work (engine change a).** The brain never drafts for folders it can't write to → smaller surface, no permanent dead state.

## Known limitations (named, accepted for SP4)

- **Retroactive proposals** not offered for folders enabled *after* a contradiction was processed (the `invalidate` is consumed).
- **Per-item review only** (no bulk-approve).
- **`prime_switches` persistence fix (engine change b) is the main technical risk** — must distinguish never-set defaults from explicit user choices; gets dedicated security-review attention.
- **N+1 preview reads** (one `event_by_id` / `fs::read` per opened proposal) — fine for the queue sizes SP4 targets.
- **Windows** deferred (Unix-gated), matching SP1–SP3.

## Testing

- **Engine (`bossclaw-core`):**
  - Round-trip mirror of `proposal_round_trip_emit_confirm_execute_resolve_undo` (`tests/reconcile.rs:1057`) — the canonical confirm flow.
  - Change (a): the rewritten `reconcile_target_outside_write_grant_*` test (skip, no `write_rejected`; grant-then-propose follow-on).
  - Change (b): a persistence test — set `proposals_enabled` true, reopen, assert it stays true; `mandates_enabled` stays false.
  - Change (c): `pending_proposals()` projection test (open/close/suppress).
- **Desktop:** hermetic `#[cfg(unix)]` `EngineHandle` tests (`MockVault` + `MockEmbedderProvider` + `MockReasonerProvider`/`ScriptedReasoner`): list → preview → apply → resolve → undo; decline; stale-file fail-closed; not-onboarded. DTO mapping unit tests. `vitest` for `diffView`/`proposalView`/loud-modal/empty states.
- **Gates (all green):** `cargo build/test/clippy -p air_agent_desktop` · `cargo test -p bossclaw-core` · `cargo clippy -p bossclaw-core --features ollama -- -D warnings` · `typecheck` · `vitest` · the **two-graph network guard**.
- **Manual launch:** enable a folder for edits → (Ollama up) evolve produces a proposal → Review → diff → Approve → file changed on disk + undo restores it; Decline path; stale-file path.

## New constants / modules / commands / touch (summary)

- **Engine:** (a) reconcile skip-on-no-grant + test; (b) `prime_switches` respects explicit settings (+ a "key explicitly set?" predicate); (c) `pending_proposals()` projection. No new event types (reuses `write_proposal`/`file_written`/`write_declined`/`write_grant`).
- **Desktop:** `src/review/*` (ReviewPanel + render helpers + tests); `App.tsx` `View += "review"` + nav badge; Settings → Folders "Allow edits" + "Allow All"; new commands/DTOs above + `api/engine.ts` twins; `EngineHandle` ops.

## Resolved by brainstorming (was open questions)

1. Permission model → **enable-folder-first**, two locks (not ask-at-approval).
2. Review location → **own destination + badge**, layout-agnostic.
3. Diff → **inline unified**.
4. Enable UX → per-folder toggle **+ Allow-All**; new folders read-only; evolve-off → offer to enable.
5. Decline → **final**.
6. Risky edits → **loud confirm**; undo via "Recently applied".
7. Engine change → **accepted** (SP4 is not desktop-only); 3 surgical edits, full security review.
8. App-shell sidebar + search → **deferred to its own sub-project**.

## Future hooks (NOT built here)

- **SP5** — mandate management (flip `mandates_enabled`, `add_mandate`), reusing this Review surface for M6c proposals.
- **App-shell redesign** — left sidebar nav + landing search (next sub-project); repositions the Review destination.
- **Cloud `ReasonerProvider`**, **batch approve**, **retroactive reconciliation**, and **M7** (persisted index, Windows, signer-DID verification).
