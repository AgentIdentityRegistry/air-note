# Desktop Engine — Mandate Management (SP5) — Design

**Status:** **Draft** (2026-06-24) — brainstormed interactively with Peter; **pending the independent critic + security review** (same gate SP1–SP4 used). Sub-project **5 of 5 — the final one** in the "engine-in-the-desktop" milestone.

**⚠️ Only one tiny `bossclaw-core` change — but SP5 introduces the first AUTONOMOUS writes.** Unlike SP4's three surgical engine edits, SP5 needs just **one (possibly two)** additive, read-only engine change(s) (§Engine change). The whole mandate machinery (data model, M6c proposer, even an unused watcher) is **already built and merged** in `bossclaw-core`. The reason SP5 still gets a full security review is behavioral, not surface-area: for the first time the app **writes files without a per-change human confirm** (auto-apply). That is the thing to review.

## Context — the parent milestone

- **SP1 spine** (merged): one live, encrypted `EventLog` behind the `EngineHandle` chokepoint.
- **SP2 ingest** (merged): folder read-grants, ingest, persisted `model2vec` vectors.
- **SP3 recall + evolve** (merged, PR #45): Memory tab, hybrid recall, a local-Ollama evolve loop — OFF by default.
- **SP4 confirm/preview** (merged, PR #46 `93aa8c3`): the Review destination — proposal queue, before/after diff, Approve/Decline, engine-enforced loud confirm, Undo; `prime_switches` persistence via a typed `ConfigFlag`/`explicitly_set`; the **T-I1 op-mapping** (`apply_proposal` maps the proposal's own `op` so a future Create/Delete isn't mis-gated as Edit).

The 5 sub-projects: **1 spine ✅ → 2 ingest ✅ → 3 recall + evolve ✅ → 4 confirm/preview ✅ → 5 mandate management (this doc, final).**

SP5 is the capstone that lets the brain pursue **standing goals**: the M6c mandate proposer finally runs, and the user can grant / list / revoke mandates and flip a global on/off switch. The brain's resulting rewrites are **auto-applied when clean** and **routed to SP4's Review queue when risky**.

## Goal

A **mandate** is a standing, user-granted goal: *"keep file `target` in sync with the sources under `source_scope`, following `recipe`."* After the user turns mandates on and grants one, the brain — on each ~5-min evolve tick — checks whether `target` still matches what `recipe` + the on-disk sources imply. If not, it proposes a whole-file rewrite. The app then:

- **auto-applies** the rewrite when it's **clean** (the engine's fresh re-gate verdict is *not* loud — all sources trusted/local, content not secret-shaped) — no per-change confirm,
- **parks it in the SP4 Review queue** when it's **risky** (any untrusted/external taint, or secret-shaped `diff_flags`) — a human approves before any write,
- records **every** auto-applied write in a **persistent Mandate-activity list** with **Undo**.

The user can **create / list / revoke** mandates and flip a global **Mandates on/off** switch. **Off by default.**

**The autonomy model — the SP5 decision that matters most:** the **mandate grant *is* the standing consent**. Reactive edits (M6b) stay per-change reviewed (that's SP4, unchanged); standing mandates (M6c) auto-apply when clean. "Clean vs. risky" reuses the engine's existing `requires_loud_modal` verdict — no new judgment is invented.

## Decisions (resolved in brainstorming)

1. **Auto-apply clean / queue risky.** A mandate rewrite whose **fresh re-gate verdict is not loud** is applied automatically (no per-change confirm). A **loud** verdict (`Untrusted`/external taint, or secret-shaped `diff_flags`) is left in the SP4 Review queue for the user. Rationale: the grant is the standing consent; the engine's taint verdict is the auto-apply gate; the one genuinely dangerous case (untrusted-data-derived writes) always keeps a human. Mandates are Create/Edit only (never Delete — engine design D7), so the Delete loud-trigger never applies.
2. **Polled detection (~5 min); reuse the existing evolve scheduler.** The M6c phase already runs *inside* `evolve_once` (gated by `mandates_enabled`), and the SP3 scheduler already calls `evolve_once` on a timer — so once mandates are on and one exists, M6c is driven **for free**. The engine's `watch.rs` live OS watcher stays **unwired** (clean fast-follow). Trade-off: up to ~5-min latency, accepted.
3. **Approach A — app-driven auto-apply.** The engine emits mandate proposals into the queue exactly as today; the **desktop scheduler**, right after each evolve tick, auto-applies the **clean** mandate proposals (reusing SP4's `apply_proposal` path *without* the confirm) and leaves **risky** ones queued. The engine keeps its "**never auto-write on its own**" guarantee — the auto-apply *policy and action* live in the app; the engine still only emits proposals and fails safe (a proposal sits in the queue forever absent a caller). *(Approach B — engine writes clean files directly, sealing the rule inside the core — and Approach C — engine stamps an auto-apply flag — were considered and deferred; see §Future hooks.)*
4. **Persistent Mandate-activity list + Undo (IN scope).** Auto-apply has no pre-confirm, so the user needs a visible *"here's what I changed"* trail. Built cheaply on the event log: list the auto-applied writes (**M6c-attributed `file_written` events** — the attribution mechanism is finalized in the plan and may need the thin engine read-helper of §Engine change (b)), newest-first, each with **Undo** (reuses SP4's `undo_write`). **Survives restarts** — delivers part of SP4's deferred cross-session undo, scoped to exactly where it matters (autonomous writes).
5. **Global Mandates on/off switch, off by default**, with an explicit "on" that **persists across launches** (the `prime_switches` symmetry fix — §Desktop backend 1).
6. **Mandate creation honors the engine's grant-time guards.** The "New mandate" form surfaces the engine's rejections (recipe ≤ `MAX_RECIPE_LEN` 2048; target must be under an active **write**-grant; target must **not** be under any active **read**-grant root — the load-bearing self-loop guard) instead of failing silently.

## Non-goals (explicitly deferred)

- **Live OS watcher (`watch.rs`) → fast-follow.** True instant detection; SP5 is polled. The engine watcher is built+tested but has no desktop caller.
- **Engine-sealed auto-apply (Approach B) → future hardening.** SP5's auto-apply policy is app-side; the engine still fails safe (queues) for any non-desktop caller.
- **Per-mandate trust setting (Approach C from the tainted-path question) → deferred.** SP5 uses one global clean/risky rule.
- **Per-flag "looks like a secret" warning** (needs `diff_flags` in `PreviewDto`) → still deferred (carried from SP4). **Safe note:** secret-shaped content already sets `requires_loud_modal` at the engine level, so it **auto-queues for review** — the dangerous case is covered without the per-flag UI.
- **Engine-enforced `requires_loud_modal` inside `bossclaw-core` `execute_write_inner`** → still deferred (carried from SP4). SP5's auto-apply gate is the desktop apply op's fresh re-gate. (Flagged below — it matters more now that writes are autonomous.)
- **Managed-section sync, multi-root `source_scope`, timed mandate expiry, Windows** → engine non-goals (all new Rust is `#[cfg(unix)]`-gated, matching SP1–SP4).
- **Mandate editing** → revoke + re-create (no in-place edit). **Bulk mandate ops** → later.
- **`pending_proposals()` projection-table optimization** → perf, deferred (now also scanned each sweep; fine for SP5 queue sizes).

## Autonomy / permission model

- **Mandate grant = standing consent.** Granting a mandate is the user's one-time, explicit authorization for the brain to keep that one file in sync. No per-change confirm for the clean path — that is the whole point of a "standing chore."
- **The taint verdict is the auto-apply gate.** Auto-apply fires **only** when the fresh re-gate `verdict.requires_loud_modal == false`. `requires_loud_modal = Untrusted || Delete || diff_flags` (engine); for mandates (Create/Edit) that reduces to **untrusted/external taint OR secret-shaped content → loud → Review**; everything else → auto-apply.
- **Two locks still hold for the file itself.** (1) the folder is write-granted (engine re-enforced at `execute_write`, not just UI), and (2) for the *risky* path, per-change human approval. For the *clean* path, lock 2 is satisfied once, at grant time.
- **Producer filter preserves SP4's contract.** The auto-apply sweep applies **only** `m6c-mandate-proposer` proposals. M6b reconcile edits are **never** auto-applied — SP4's "you approve every edit" promise is untouched.
- **Off by default; explicit + sticky.** `mandates_enabled` is forced off until the user flips it; then it persists (switch-fix). The M6c phase re-reads the flag per mandate (fast-kill).

## Engine change (`bossclaw-core`) — one–two, surgical, additive

> Line numbers are grounding references against current `main` (verified 2026-06-24); the implementation plan re-verifies exact locations.

**(a) Surface the proposal's producer in `pending_proposals()`.**
Today `PendingProposal` (`crates/bossclaw-core/src/log.rs:369`, built by `pending_proposals()` at `log.rs:2341`) carries no producer, so the desktop cannot tell an **M6c mandate** proposal from an **M6b reconcile** one. Add a `producer: String` field, read from the `write_proposal` event's `model_meta.model_id` (M6c stamps `M6C_PROPOSER_PRODUCER = "m6c-mandate-proposer"`, `graph.rs:98`). Additive, read-only — no new event, no behavior change to existing folds.
- **Why:** the auto-apply sweep must apply **only** mandate proposals; auto-applying an M6b reconcile proposal would break SP4's contract.
- **Test:** a proposal emitted by M6c surfaces `producer == "m6c-mandate-proposer"`; an M6b one surfaces its reconcile producer.

**(b) (If needed) a thin read-helper to attribute resolved `file_written` events to their proposer** — the Mandate-activity list (Decision 4) shows *applied* (closed) M6c writes, which `pending_proposals()` (open only) does **not** cover. The plan confirms whether existing event queries suffice (join `file_written.resolves_proposal` → the originating `write_proposal`'s producer) or a small additive read-helper is warranted. Read-only either way; no new events.

*Everything else reuses engine APIs that already exist:* `add_mandate` (`log.rs:2800`), `revoke_mandate`, `active_mandates` (`log.rs:2896`), `set_mandates_enabled` (`log.rs:5060`), `mandates_enabled`, `explicitly_set` (`log.rs:5034`), `ConfigFlag::Mandates`, the M6c phase inside `evolve_once`, and the `Mandate` model (`graph.rs:497`) with its `mandate_grant`/`mandate_revoke` events.

## Desktop backend (`apps/desktop/src-tauri`) — the bulk

**1. Switch-fix — `prime_switches` respects an explicit mandate choice.**
`apps/desktop/src-tauri/src/engine/mod.rs:359` currently force-OFFs mandates **unconditionally** (`:368–369`), unlike evolve (`:361`) and proposals (`:364`) which are gated behind `!log.explicitly_set(…)?`. Add the same guard:
```rust
if !log.explicitly_set(ConfigFlag::Mandates)? && log.mandates_enabled()? {
    log.set_mandates_enabled(false)?;
}
```
Now a user's explicit "on" persists across launches. One-line symmetry change. **Flip** the existing test `prime_switches_preserves_explicit_proposals_but_forces_mandates_off` (`engine/mod.rs:933`) → `…_preserves_explicit_mandates` (assert an explicit on survives a re-open).

**2. On/off op + command.** `EngineHandle::set_mandates_enabled(on)` + `engine_set_mandates_enabled` command, registered `#[cfg(unix)]` in `main.rs` (mirror `engine_set_proposals_enabled`). Sticky, funnels through `get_or_open → spawn_blocking → EngineOpError` like every SP2–SP4 op.

**3. Mandate CRUD ops + commands + DTOs.**
```
engine_add_mandate(target, source_scope, recipe) -> MandateDto   // surfaces grant-time rejections as typed errors
engine_revoke_mandate(mandate_grant_id)          -> ()
engine_list_mandates()                           -> Vec<MandateDto>
```
`MandateDto { mandate_grant_id, target, source_scope, recipe, granted_at, revoked }` (`#[derive(Serialize)]` + `From<Mandate>`, `graph.rs:497`). Hand-mirrored TS twins in `api/engine.ts`. `add_mandate`'s engine guards (recipe length; target write-granted; target not under a read-grant root — the self-loop guard) map to typed `EngineOpError`s the form renders.

**4. New-file (Create) apply fix.** `apply_proposal` (`engine/mod.rs:629`) reads the live target and, at `match &p.base_content_hash` (`:643`), fails closed (`Stale`) when there's no base hash — so a **Create** proposal (target absent) is rejected *before* the T-I1 op-mapping (`:660–661`) ever runs. Special-case `op == "create"`: **skip the live-file fingerprint**; the Create anti-clobber invariant is instead *"the target must still be absent"* (re-check absence immediately before the write). Edit path unchanged (mandates never Delete). *(This is the single code-level gap the Explore pass surfaced; without it, no mandate Create could ever apply.)*

**5. Auto-apply sweep (the heart of SP5).** In the scheduler (`apps/desktop/src-tauri/src/engine/scheduler.rs`), immediately after `evolve_once` returns, run a sweep: `list pending proposals`; for each whose `producer == "m6c-mandate-proposer"`, call `apply_proposal(id, acknowledged_loud = false)` and branch on the result:
- **clean** → applies (atomic write, `file_written` recorded, undo `pre_bytes` captured);
- **`NeedsLoudConfirm`** (risky) → swallow; the proposal stays open → surfaces in the SP4 Review queue;
- **`Stale` / `Revoked`** → swallow; skip (re-proposed next tick).

The sweep **never** touches M6b reconcile proposals (producer filter) — SP4 unchanged. It respects a per-sweep cap (mirrors the engine's `MAX_PROPOSALS_PER_TICK` spirit) and re-reads `mandates_enabled` so flipping the switch off fast-stops it. Surface `producer` through `ProposalSummary` → `ProposalDto` so the UI can label "from mandate" (from engine change a).

## Desktop frontend (`apps/desktop/src`) — the screens

- **Mandates destination** — `src/mandates/MandatesPanel.tsx` + pure render/validation helpers with sibling `vitest` tests (mirrors `src/review/*`):
  - the global **Mandates on/off** toggle (off by default);
  - a **"New mandate"** form: target-file picker, source-folder picker, recipe textarea, with inline validation + clear display of engine rejections (must be write-granted; can't sit inside an ingested/read folder; recipe length);
  - an **active-mandate list** (target · sources · recipe · granted_at) each with **Revoke**;
  - the **Mandate-activity list** — auto-applied (M6c-attributed) `file_written` events, newest-first, each with **Undo**.
- `App.tsx` `View += "mandates"` + a nav entry, written **layout-agnostic** (a destination, like SP4's Review), so the deferred app-shell redesign repositions it with zero rework.
- **Risky** mandate proposals reuse the **SP4 Review queue unchanged** (optionally labeled "from mandate" via the surfaced producer) — no new review UI.

## Data flow

`user grants mandate (target + source_scope + recipe) → signed mandate_grant → [every ~5 min] evolve tick → (mandates_enabled) M6c phase: recipe-compare target vs on-disk sources → write_proposal (clean | tainted) → scheduler auto-apply sweep:` **clean** `→ apply_proposal(false) → execute_write_resolving → file_written (+undo)`; **risky** `→ NeedsLoudConfirm → stays queued → SP4 Review → user Approve/Decline`. The Mandate-activity list reads the M6c-attributed `file_written` events; **Undo** → `undo_write`. Mandates off / none granted → M6c phase no-ops.

## Failure / partial-state matrix

| Scenario | Result |
|---|---|
| Mandates off / none granted | M6c phase no-ops; no proposals |
| Bad grant (recipe > 2048 / target not write-granted / target under a read-grant root) | engine rejects → "New mandate" form shows *why*; no mandate created |
| Clean mandate rewrite | auto-applied silently; appears in Mandate-activity with Undo |
| Risky (tainted / secret-shaped) mandate rewrite | sweep gets `NeedsLoudConfirm` → parked in SP4 Review; **nothing auto-written** |
| Any M6b reconcile proposal | producer filter excludes it from the sweep → still needs manual approval (SP4 unchanged) |
| Target changed on disk since the proposal | base-hash anti-clobber → `Stale` → skipped; re-proposed next tick; never clobbered |
| Create proposal but target reappeared | Create anti-clobber ("still absent") fails → skipped; never overwrites |
| Write-grant revoked between propose and sweep | fresh re-gate → `Revoked` → skipped |
| Mandates flipped off mid-sweep | per-item `mandates_enabled` re-read → stops fast |
| Relaunch with an explicit mandates-on | `prime_switches` preserves it (switch-fix); M6c resumes |

## Security invariants

- **Grant is consent; taint verdict is the gate.** Auto-apply happens **only** when the fresh re-gate verdict is **not loud**. Any untrusted/external taint or secret-shaped `diff_flags` → `requires_loud_modal` → **not** auto-applied → parked in Review. The confused-deputy case (untrusted data flowing into a trusted file) always keeps a human.
- **Engine still never auto-writes on its own.** The auto-apply *action* is the **desktop scheduler** calling `apply_proposal`; the engine only emits proposals and fails safe. Approach A keeps the core exactly as safe as SP4.
- **Producer-filtered sweep preserves SP4.** Only `m6c-mandate-proposer` proposals auto-apply; M6b reconcile edits never do.
- **Same anti-clobber + re-gate at apply.** The clean path uses the *exact* SP4 apply chain (base-hash anti-clobber → fresh `propose_write` re-gate → T-I1 op-mapping → atomic temp+rename → durable undo `pre_bytes` → signed `file_written`). For **Create**, anti-clobber = *"target still absent."*
- **Two locks still hold for the file.** Folder write-grant (engine-re-enforced at `execute_write`) + (risky path) human approval. A mandate target must be write-granted **and** outside every read-grant root (`add_mandate` self-loop guard) — so a mandate can never rewrite a file it is also ingesting.
- **Off by default; explicit + sticky; re-read per item.** `mandates_enabled` forced off until flipped, then persists; the M6c phase re-reads it per mandate.
- **Local only.** No new network surface (reuses SP3's loopback reasoner + network-free embedder; the **two-graph network guard** stays green). No new secrets / keychain reads.
- **Carried residual (from SP4, NOT closed here) — flagged for review.** Loud-confirm is enforced at the **desktop** apply op, not yet inside `bossclaw-core execute_write_inner`; first-open `verify_chain` is still advisory. These matter **more** now that writes are autonomous (a wider confused-deputy surface). The recommended next hardening is pushing the loud-gate into the engine (Approach B territory) — deferred, but explicitly on the table for the security reviewer.

## Known limitations (named, accepted for SP5)

- **Polled, not instant** — up to ~5-min lag before a source change is noticed (watcher deferred).
- **Auto-apply policy is app-side** (Approach A), not engine-sealed; the engine fails safe (queues) for any non-desktop caller.
- **No per-mandate trust** — one global clean/risky rule.
- **Mandate editing = revoke + re-create** (no in-place edit); **no bulk ops**.
- **`pending_proposals()` re-folds the actuator stream `O(events)` per call** — now also scanned each sweep; fine for SP5 queue sizes; the projection-table is the future fix.
- **Windows deferred** (Unix-gated), matching SP1–SP4.

## Testing

- **Engine (`bossclaw-core`):** `pending_proposals()` surfaces `producer` correctly for an M6c vs. an M6b proposal.
- **Desktop backend** (`#[cfg(unix)]` `EngineHandle` tests with `MockVault` + `MockEmbedderProvider` + `ScriptedReasoner`):
  - **switch-fix:** set mandates on → reopen with a fresh handle → assert still on (the flipped `…_preserves_explicit_mandates` test); the engine `tests/mandate.rs` sticky-default test still holds.
  - **Create-apply:** a Create proposal applies (file written); refused if the target reappeared.
  - **auto-apply sweep — the three that matter:** ① clean mandate proposal → auto-applied; ② risky (tainted) mandate proposal → stays queued, never auto-written; ③ **an M6b reconcile proposal → NEVER auto-applied.** Built with SP4's hard-won Tauri ACL discipline (real `__allow_command` grant for a `Remote{http://tauri.localhost}` origin + a **positive** op-ran signature + a **mutation-verify** so the test can't pass vacuously).
  - **CRUD round-trip:** add → list → revoke; grant-time rejections (recipe length, no write-grant, read-grant self-loop) surface as typed errors.
  - **Mandate-activity:** lists M6c `file_written` events; Undo restores prior bytes.
- **Front-end:** `vitest` for New-mandate form validation, the mandate-list view, and the activity-list view (pure helpers, like SP4's `diffView`/`proposalView`).
- **Gates (all green):** `cargo build/test/clippy -p air_agent_desktop` · `cargo test -p bossclaw-core` · `cargo clippy -p bossclaw-core --features ollama -- -D warnings` · `typecheck` · `vitest` · the **two-graph network guard**.
- **Manual launch:** turn mandates on → grant a mandate (target in a write-granted folder, sources in a read folder) → trigger an evolve tick → a clean change **auto-applies** + shows in Mandate-activity + Undo restores it; introduce an external/tainted source → that rewrite **parks in Review** → Approve; **Revoke** stops further writes; relaunch → mandates **still on**.

## New constants / modules / commands / touch (summary)

- **Engine:** (a) `producer` field on `PendingProposal` + `pending_proposals()` + test; (b) *if needed* a thin read-helper attributing resolved `file_written` events to their proposer (Mandate-activity). **No new events** (reuses `mandate_grant`/`mandate_revoke`/`write_proposal`/`file_written`, all already built).
- **Desktop backend:** `prime_switches` mandate guard (+ flipped test); `set_mandates_enabled` op+command; `add`/`revoke`/`list_mandates` ops+commands + `MandateDto`; Create-apply fix in `apply_proposal`; scheduler auto-apply sweep; `producer` through `ProposalSummary`/`ProposalDto`; `api/engine.ts` twins.
- **Desktop frontend:** `src/mandates/*` (MandatesPanel + form/list/activity helpers + tests); `App.tsx` `View += "mandates"` + nav; reuse SP4 Review for the risky path.

## Resolved by brainstorming (was open questions)

1. Autonomy → **auto-apply** (mandate grant = standing consent), not per-change review.
2. Tainted path → **auto-apply clean, queue risky** (the engine's `requires_loud_modal` is the gate).
3. Detection → **polled ~5 min** (reuse the evolve scheduler); live watcher deferred.
4. Build approach → **A, app-driven** (engine stays light, keeps its "never auto-write on its own" guarantee).
5. Audit/undo → **persistent Mandate-activity list + Undo, IN scope** (the price of no pre-confirm).
6. Create-apply gap → **fix in the desktop apply op** (absence-based anti-clobber for Create).

## Future hooks (NOT built here)

- **Live OS watcher** (`watch.rs`) → instant detection.
- **Engine-sealed auto-apply + loud-gate inside `execute_write_inner`** (Approach B) → the recommended next hardening for autonomous writes.
- **Per-mandate trust tier** (Approach C), **mandate editing**, **bulk ops**.
- **App-shell redesign** → repositions the Mandates + Review destinations.
- **M7** → battery/thermal-smart scheduler, persisted index, Windows, signer-DID verification.
