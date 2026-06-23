# Desktop Engine — Mandate Management (SP5) — Design

**Status:** **Draft v3** (2026-06-24) — brainstormed interactively with Peter; **critic + security review of v1 AND a focused security re-review of v2's taint change complete** (all SHIP-WITH-FIXES; the v2 re-review verified the core taint-trust rule is sound and safe — MEDIUM risk, 0 Critical, 1 High + 2 Medium, all fixed in v3). **v3 folds in every must-fix and adopts the deliberate "Option B" scope increase the review forced into the open.** Sub-project **5 of 5 — the final one**. H1/M2/M1/L1/L2 are now pinned in §Engine changes (b)(c)(d), §Security invariants, and §Testing.

**⚠️ SP5 is NOT thin. It changes the security core (the taint model) and introduces the app's first AUTONOMOUS writes.** v1 assumed "auto-apply clean / queue risky" worked out of the box. The security review proved it was **dead code**: every ingested file is stamped `external` (`ingest.rs:617`, single-sourced), so every mandate rewrite is `Untrusted` → always loud → *everything* routed to Review → zero autonomy. To deliver the hands-off feature Peter chose, v2 adds a **scoped taint-trust rule** (a mandate's *own* authorized sources stop tainting *that mandate's* target) plus the **engine-level loud-gate hardening** both reviewers recommended. Four engine changes, two of them security-critical (§Engine changes c, d). **This v2 must clear its own focused security review before any code.**

## Context — the parent milestone

- **SP1 spine** (merged): one live, encrypted `EventLog` behind the `EngineHandle` chokepoint.
- **SP2 ingest** (merged): folder read-grants, ingest, persisted `model2vec` vectors.
- **SP3 recall + evolve** (merged, PR #45): Memory tab, hybrid recall, a local-Ollama evolve loop — OFF by default.
- **SP4 confirm/preview** (merged, PR #46 `93aa8c3`): the Review destination — proposal queue, before/after diff, Approve/Decline, **engine-enforced loud confirm at the desktop apply op**, Undo; `prime_switches` persistence via a typed `ConfigFlag`/`explicitly_set`; the **T-I1 op-mapping** (`apply_proposal` maps the proposal's own `op` so a Create/Delete isn't mis-gated as Edit).

The 5 sub-projects: **1 spine ✅ → 2 ingest ✅ → 3 recall + evolve ✅ → 4 confirm/preview ✅ → 5 mandate management (this doc, final).**

## Goal

A **mandate** is a standing, user-granted goal: *"keep file `target` in sync with the sources under `source_scope`, following `recipe`."* The grant is the user's **standing consent** and a **trust declaration** for those specific sources. After the user turns mandates on and grants one, the brain — on each ~5-min evolve tick — checks whether `target` still matches what `recipe` + the on-disk sources imply. If not, it proposes a whole-file rewrite, and the app:

- **auto-applies** the rewrite when it's **clean** — i.e. every cited source is **authorized by an active mandate for that target** (in-scope) **and** the content is **not secret-shaped** (no `diff_flags`) — with no per-change confirm;
- **parks it in the SP4 Review queue** when it's **risky** — secret-shaped content, a source **outside** the mandate's authorized scope, an unresolvable source, or any other reason the fresh re-gate verdict is loud;
- records **every** auto-applied write in a **persistent Mandate-activity list** with **Undo**.

The user can **create / list / revoke** mandates and flip a global **Mandates on/off** switch. **Off by default.**

**Why this needs an engine change (the v1→v2 correction).** Without a trust rule, "clean" is unreachable: a mandate syncs *from* ingested files, and **every** ingested file is `external` → `Untrusted` → loud (this is the *same* fact that makes SP4 always-loud). v2's taint-trust rule (§Engine change c) is precisely the missing "clean" path: a write to a mandate's target, derived only from that mandate's *authorized in-scope* sources, is **not** tainted by those sources — so it can be clean and auto-apply. Everything the taint model protected before stays protected (out-of-scope sources, secret-shaped content, the engine-anchored target floor, M6b reconcile writes).

## Decisions (resolved in brainstorming + the v1 review)

1. **Auto-apply clean / queue risky.** Clean ⇒ apply automatically (no per-change confirm). Risky ⇒ park in the SP4 Review queue. "Clean vs risky" = the engine's fresh re-gate `requires_loud_modal`, now made *reachable* by Decision 7.
2. **Polled detection (~5 min); reuse the existing evolve scheduler.** The M6c phase already runs inside `evolve_once` (gated by `mandates_enabled`), and the SP3 scheduler already calls it on a timer. The engine's `watch.rs` live OS watcher stays **unwired** (clean fast-follow). Latency note: a clean rewrite lands within ~5 min in the best case; under the shared `MAX_PROPOSALS_PER_TICK` cap with several mandates + active M6b, a given mandate can take multiple ticks (§Known limitations).
3. **Approach A — app-driven auto-apply.** The engine emits mandate proposals into the queue; the **desktop scheduler**, right after each evolve tick, auto-applies the **clean** ones (reusing SP4's `apply_proposal(id, acknowledged_loud=false)` path) and leaves **risky** ones queued. The auto-apply *action* lives in the app; the engine never auto-writes on its own.
4. **Persistent Mandate-activity list + Undo (IN scope, MANDATORY).** Auto-apply has no pre-confirm, so the user needs a guaranteed "here's what I changed" trail. Built on the event log via a small engine attribution helper (§Engine change b) — applied writes are stamped with the actuator producer, not the proposer, so attribution *requires a join*; this is not optional.
5. **Global Mandates on/off switch, off by default**, with an explicit "on" that **persists across launches** (the `prime_switches` symmetry fix).
6. **Mandate creation honors the engine's grant-time guards** (recipe ≤ `MAX_RECIPE_LEN` 2048; ≤ `MAX_SOURCES_PER_MANDATE` 256; target under an active **write**-grant; target **not** under any active **read**-grant root — the self-loop guard). The form surfaces rejections clearly.
7. **(NEW — the v2 centerpiece) A mandate's authorized sources don't taint that mandate's target.** In `propose_write`, an external cited source does **not** escalate taint **iff** an active mandate `m` has `m.target == this proposal's target` **and** the source's ingested path is under `m.source_scope`. Out-of-scope or unresolvable sources still taint (fail-closed over the set); `diff_flags` (secret-shaped) still force loud; the Step-4 engine-anchored target taint is unchanged (and moot for mandates — the self-loop guard keeps the target un-ingested). Re-gate-safe (the same `propose_write` runs at propose and at apply) and scoped (mandate targets ∩ M6b-reconcile targets = ∅).
8. **(NEW — review hardening) Enforce `requires_loud_modal` inside `bossclaw-core` `execute_write_inner`.** Thread an `acknowledged_loud` flag through the execute path and fail closed if a loud write lacks the ack — so the human-in-the-loop guarantee is an **engine invariant** for *every* caller (desktop, the autonomous sweep, future cloud), not a desktop-layer convention an autonomous actor could satisfy with a bool.

## Non-goals (explicitly deferred)

- **Live OS watcher (`watch.rs`) → fast-follow.** SP5 is polled.
- **Per-mandate trust *tier* / multiple trust levels** → SP5 has one rule: a mandate trusts its own declared `source_scope`.
- **Per-flag "looks like a secret" *warning* UI** (needs `diff_flags` in `PreviewDto`) → deferred; but the *protection* is live — secret-shaped content forces loud → queues, so a mandate never auto-writes a secret.
- **Managed-section sync, multi-root `source_scope`, timed mandate expiry, Windows** → engine non-goals (all new Rust is `#[cfg(unix)]`-gated).
- **Mandate editing** → revoke + re-create. **Bulk mandate ops** → later.
- **`pending_proposals()` projection-table optimization** → perf, deferred (§Known limitations).
- **First-open `verify_chain` fail-closed gate** → still deferred (carried from SP4; ruled acceptable — see §Security invariants).

## Autonomy / permission / trust model

- **The mandate grant is BOTH standing consent AND a source-trust declaration.** By granting "keep `target` synced from `source_scope`," the user authorizes (a) the brain to write `target` without re-asking, and (b) `source_scope`'s contents as trusted *inputs for that target*. The signed `mandate_grant` event is the trust root (mandates are **user-created only** — `engine_add_mandate`; the reasoner returns data bytes, never events, so it cannot mint a grant).
- **What stays gated even for a mandate (the residual protections):**
  - **Secret-shaped content** → `diff_flags` → loud → Review (never auto-written).
  - **Out-of-scope / unresolvable sources** → still taint → loud → Review (the trust rule only clears *in-scope, resolvable* sources; it never `filter_map`s the set).
  - **The engine-anchored target floor (Step 4)** is untouched — and the self-loop guard means a mandate target is never an ingested file, so a mandate can't launder taint through its own output.
  - **Both write-locks** still hold: the folder write-grant (engine-re-enforced at `execute_write`) and, for the risky path, per-change human approval.
- **Accepted residual of Option B (eyes-open, v2-review-confirmed MEDIUM).** A mandate auto-applies content **model-synthesized** over its in-scope sources — so a single attacker-authored file in a watched folder can steer the rewrite in **transformed** (not just verbatim) ways, and only secret-shaped output is gated. The *preventive* review is removed on the clean path; the activity-list + Undo is the compensating **detective** control. Bounded by the explicit per-mandate grant, write-grant containment, the secret-shaped gate, and the audit trail. See §Security invariants for the full statement.
- **Off by default; explicit + sticky; re-read per item.** `mandates_enabled` forced off until flipped, then persists; the M6c phase re-reads it per mandate.

## Engine changes (`bossclaw-core`) — four (a, b additive · c, d security-critical)

> Line numbers are grounding references against `main` (verified 2026-06-24, HEAD `e6e6991`); the plan re-verifies exact locations.

**(a) Surface the proposal's producer in `pending_proposals()`.** `PendingProposal` (`log.rs:369`, built by `pending_proposals()` at `log.rs:2341`) gains a `producer: String` read from the `write_proposal` event's `model_meta.model_id` (M6c stamps `M6C_PROPOSER_PRODUCER = "m6c-mandate-proposer"`, `graph.rs:98`). Additive, read-only. **Fail-closed contract:** the sweep auto-applies **iff** `producer == M6C_PROPOSER_PRODUCER`; any other value, *including empty/unknown*, is left for manual review. (Note: the producer filter is a **contract/UX boundary, not the security gate** — even a mislabeled proposal still faces the taint/loud gate at apply.)

**(b) Mandate-write attribution helper — `mandate_writes()` (MANDATORY, not "if needed").** Applied writes are stamped `model_meta.model_id = ACTUATOR_PRODUCER` ("m6a-actuator", `log.rs:3675`) regardless of proposer; the only discriminator on a `file_written` is `content.resolves_proposal` → the proposal id, and resolved proposals are **not** in `pending_proposals()` (open-only). So attributing an applied write to a mandate **requires a join**. Add a `#[cfg(unix)]` `mandate_writes() -> Vec<MandateWriteRecord>` folding `file_written` ∪ `write_proposal`, keeping `file_written`s whose resolved proposal's producer is `M6C_PROPOSER_PRODUCER`, returning `{ file_written_id, target, written_at, undone }`. **`undone`** = an existing later `file_written` carries `undo_of == this.file_written_id` (an Undo is itself a `file_written` with `undo_of`, no `resolves_proposal`, so it's excluded from the join and flips the original's flag). **Completeness is load-bearing (v2-review L2):** because Option B removes the preventive check, a silently-dropped row is an *invisible autonomous write* with no Undo offered. So an M6c `write_proposal` MUST NOT be GC'd while its `file_written` is live, and a degraded join falls back to **target-only attribution** ("a mandate changed this file"; Undo still offered via the `file_written_id`) rather than dropping the row.

**(c) (SECURITY-CRITICAL) Mandate-authorized sources don't taint that mandate's target.** In `propose_write` (the source-taint escalation currently at `log.rs:3048-3062`), when a cited source resolves to an `is_external` event, before escalating to `Untrusted` consult the active mandates: if some active mandate `m` has canonical `m.target` == this proposal's **canonical** target **and** the source's ingested `canonical_path` is under `m.source_scope` — **segment-aware** containment, matching the `add_mandate` self-loop guard (`log.rs:2839`) so `/notes-evil` never matches `/notes` — the source is **authorized** and does **not** escalate taint. Unauthorized external sources, and unresolvable sources, still taint (fail-closed over the set is preserved for everything not explicitly authorized). **Ordering + fail-closed (v2-review M2/L1), pinned:** (a) in Step 1, record each candidate external source as `(event_id, ingested canonical_path)` **without** escalating yet; (b) after Step 2 yields `Some(canonical_target)`, escalate each candidate to `Untrusted` **unless** an active mandate authorizes it; (c) **if the target is unresolvable** (`canonical_target == None`), escalate **all** candidates — the unresolvable-target default is **taint, never skip-escalation**. Read `active_mandates()` **once**, at the authorization test, inside the same gate evaluation (an in-flight revoke is caught by the apply-time re-gate). Both sides of the containment test are the **stored canonical forms** — `m.source_scope` (canonical at grant, `log.rs:2811`) and the source's `canonical_path` from the `files` projection — compared segment-aware; **never re-canonicalize from a live (possibly symlinked) path** at gate time. Nothing else in `propose_write` changes: Step 4 (target anchor), Step 6 (`requires_loud_modal = Untrusted || Delete || diff_flags.any()`), and the base/identity capture are untouched.
- **Soundness:** the trust is a pure function of (the signed active mandates, the proposal target, the cited sources' paths) — reproduced identically at propose **and** at the apply-time re-gate, so there is no stored-flag trust. Revoking the mandate makes its in-flight proposals fall back to tainted → loud → queued.
- **Scoping (the key safety argument):** a mandate `target` is, by the `add_mandate` self-loop guard, **outside every read-grant** (`log.rs:2839`); an M6b reconcile `target` is an **ingested** file (inside a read-grant). The two sets are disjoint, so this rule can **never** clear taint for an M6b reconcile (or any non-mandate) write. **Tested explicitly** (§Testing).

**(d) (SECURITY-CRITICAL) Enforce the loud-gate inside `execute_write_inner`.** Thread `acknowledged_loud: bool` into `execute_write_inner`; immediately after the Step-1a verdict checks (`log.rs:3281-3286`), fail closed if `verdict.requires_loud_modal && !acknowledged_loud`. This makes "a loud write needs an explicit ack" an engine invariant for *every* caller. **`execute_write_inner` has THREE callers — all must be handled explicitly (v2-review H1), or the hole moves instead of closing:**
- **`execute_write_resolving`** (the apply path, `log.rs:3245`) — threads the caller's ack: the desktop `apply_proposal` passes the user's `true`; the autonomous sweep passes `false`. So a loud mandate proposal can never auto-write — it's refused → stays queued.
- **`undo_write`** (`log.rs:3915`) — passes `acknowledged_loud = true` as a **deliberate, commented exemption.** An undo cites the original `file_written` (external ⇒ the re-gate is `Untrusted` ⇒ loud), so without the exemption *every* undo of a tainted-file write would fail closed — and Undo is the mandatory Option-B audit affordance (Decision 4). The exemption is correct security semantics: an undo is a hash-verified inverse-restore of `pre_bytes` already validated against the recorded `base_content_hash` (`log.rs:3864`) — the inverse of an *already-approved* write, not fresh untrusted content. It must be the **only** sanctioned `acknowledged_loud=true`-without-UI path.
- **`execute_write`** (the public M6a entry, `log.rs:3230`) — must **NOT** become a defaulted un-acked loud-write path. Give it an explicit `acknowledged_loud` parameter (audit its callers — currently tests with clean proposals; `grep '\.execute_write('`), or, if it has no live non-test production caller, narrow it to `pub(crate)`/`#[cfg(test)]`. The plan states which.

Closes the SP4-deferred residual; both reviewers rated it should-close now that writes are autonomous.

*Everything else reuses engine APIs that already exist:* `add_mandate` (`log.rs:2800`), `revoke_mandate`, `active_mandates` (`log.rs:2896`), `set_mandates_enabled` (`log.rs:5060`), `mandates_enabled`, `explicitly_set` (`log.rs:5034`), `ConfigFlag::Mandates`, the M6c phase inside `evolve_once`, and the `Mandate` model (`graph.rs:497`) with its `mandate_grant`/`mandate_revoke` events.

## Desktop backend (`apps/desktop/src-tauri`)

**1. Switch-fix — `prime_switches` respects an explicit mandate choice.** `engine/mod.rs:359` force-OFFs mandates **unconditionally** (`:368–369`), unlike evolve (`:361`) and proposals (`:364`). Add the same `!log.explicitly_set(ConfigFlag::Mandates)?` guard. **Flip** the existing test `prime_switches_preserves_explicit_proposals_but_forces_mandates_off` (`engine/mod.rs:933`) → assert **both** explicit proposals-on **and** mandates-on survive a re-open (don't drop the proposals coverage).

**2. On/off op + command.** `EngineHandle::set_mandates_enabled(on)` + `engine_set_mandates_enabled`, registered `#[cfg(unix)]` in `main.rs` (mirror `engine_set_proposals_enabled`). Sticky; funnels through `get_or_open → spawn_blocking → EngineOpError`.

**3. Mandate CRUD ops + commands + DTOs.**
```
engine_add_mandate(target, source_scope, recipe) -> MandateDto   // surfaces grant-time rejections as typed errors
engine_revoke_mandate(mandate_grant_id)          -> ()
engine_list_mandates()                           -> Vec<MandateDto>
```
`MandateDto { mandate_grant_id, target, source_scope, recipe, granted_at, revoked }` (`From<Mandate>`, `graph.rs:497`; the six fields map 1:1). TS twins in `api/engine.ts`.

**4. New-file (Create) apply fix.** `apply_proposal` (`engine/mod.rs:629`) fails closed at `match &p.base_content_hash` (`:643`) when there's no base hash — so a **Create** (target absent) is rejected before the T-I1 op-mapping. Fix: special-case `op == "create"` to **skip the base-hash fail-closed arm** and map `"create"` → `WriteOp::Create` (already present at `:661`). **Do NOT add a desktop absence pre-check** — the engine's Create path is already atomic-no-clobber at the syscall (`RENAME_NOREPLACE` on Linux; macOS `statat`+`renameat`, see §Known limitations), which is the *real* anti-clobber; a desktop "check-then-write" would be strictly weaker (TOCTOU). The Create's safety = the engine's atomic no-clobber create.

**5. Auto-apply sweep (the heart of SP5).** In `engine/scheduler.rs`, immediately after `evolve_once` returns, sweep: list pending proposals (already oldest-first, `log.rs` `seq ASC`); for each whose `producer == M6C_PROPOSER_PRODUCER`, up to a **hard cap `MANDATE_AUTOAPPLY_PER_SWEEP = 8`** (mirrors `MAX_PROPOSALS_PER_TICK`), call `apply_proposal(id, acknowledged_loud=false)`:
- **clean** → applies (atomic write, `file_written` + undo `pre_bytes`);
- **`NeedsLoudConfirm`** (risky) → swallow; stays open → surfaces in SP4 Review;
- **`Stale` / `Revoked` / "already resolved"** → swallow; skip.

Re-read `mandates_enabled` **per item** (fast-kill). Excess beyond the cap is retried next tick (accepted ~5-min-per-excess latency — stated in the failure matrix). **Cost note (honest):** `apply_proposal` re-folds `pending_proposals()` internally (`engine/mod.rs:633`), so a K-item sweep does 1+K O(events) folds; bounded by the cap; the projection-table optimization is the future fix. Surface `producer` through `ProposalSummary`/`ProposalDto` for the UI label.

## Desktop frontend (`apps/desktop/src`)

- **Mandates destination** — `src/mandates/MandatesPanel.tsx` + pure helpers + vitest (mirrors `src/review/*`): global on/off toggle (off by default); a **"New mandate"** form (target-file picker, source-folder picker, recipe textarea) with inline validation + engine-rejection display; an **active-mandate list** (target · sources · recipe · granted_at) each with **Revoke**; the **Mandate-activity list** — `mandate_writes()` rows, newest-first, with **Undo** (disabled/relabeled when `undone`).
- `App.tsx` `View += "mandates"` + a nav entry (layout-agnostic destination, like SP4's Review).
- **Risky** mandate proposals reuse the **SP4 Review queue** (labeled "from mandate" via the surfaced producer, so the user understands why a non-contradiction rewrite appeared).

## Data flow

`user grants mandate (target + source_scope + recipe) → signed mandate_grant → [~5 min] evolve tick → (mandates_enabled) M6c phase: recipe-compare target vs in-scope sources → write_proposal → scheduler sweep → apply_proposal(false) → propose_write re-gate {Step-1 trust rule clears in-scope sources; diff_flags / out-of-scope still loud} →` **clean** `→ execute_write_resolving (loud-gate engine-checked, ack=false ok) → file_written (+undo)`; **loud** `→ NeedsLoudConfirm → stays queued → SP4 Review → user Approve (ack=true) / Decline`. Mandate-activity reads `mandate_writes()`; **Undo** → `undo_write`. Revoke / mandates-off → next re-gate taints (no active grant) → loud → queued; M6c phase no-ops when off.

## Failure / partial-state matrix

| Scenario | Result |
|---|---|
| Mandates off / none granted | M6c phase no-ops; no proposals |
| Bad grant (recipe > 2048 / > 256 sources / target not write-granted / target under a read-grant root) | engine rejects → "New mandate" form shows *why*; no mandate created |
| Clean rewrite (all sources in-scope+authorized, not secret-shaped) | **auto-applied** silently; appears in Mandate-activity with Undo |
| Secret-shaped rewrite | `diff_flags` → loud → `NeedsLoudConfirm` → parked in Review; **never auto-written** |
| Source outside the mandate's `source_scope`, or unresolvable | still tainted → loud → parked in Review |
| Mandate revoked between propose and sweep | re-gate finds no active grant → source re-taints → loud → parked (or `Revoked` if the write-grant also went) |
| Any M6b reconcile proposal | producer filter excludes it from the sweep; trust rule can't apply (target is ingested, not a mandate target) → still human-approved (SP4 unchanged) |
| Target changed on disk since the proposal | base-hash anti-clobber → `Stale` → skipped; re-proposed next tick |
| Create proposal, target reappeared | engine atomic no-clobber create fails closed → skipped; never overwrites |
| Write-grant revoked between propose and apply | re-gate → `Revoked` → skipped |
| Mandates flipped off mid-sweep | per-item `mandates_enabled` re-read → stops fast |
| > 8 eligible clean proposals in one tick | first 8 applied (oldest-first); remainder retried next tick (~5-min-per-excess) |
| Ollama down | no tick runs → no sweep; a clean proposal queued from a prior tick waits until Ollama returns (accepted coupling) |
| Concurrent apply (sweep vs SP4 UI) on the same clean proposal | engine resolve-check + `rename_lock` serialize; the loser gets `Stale`/"already resolved" and is skipped — no double write |
| Relaunch with explicit mandates-on | `prime_switches` preserves it; M6c resumes |

## Security invariants

- **The trust rule is scoped, signed, and re-gate-safe (Engine change c).** In-scope authorized sources stop tainting *only* a mandate's own target; everything else (out-of-scope/unresolvable sources, secret-shaped content, the Step-4 target floor) still taints. The trust root is the signed `mandate_grant` (user-created only). M6b-reconcile and mandate targets are disjoint by the self-loop guard, so the rule provably never clears taint for a non-mandate write — **with a test that asserts it.**
- **The loud-gate is now an engine invariant (Engine change d).** `execute_write_inner` refuses any `requires_loud_modal` write without `acknowledged_loud`. No caller — desktop, the autonomous sweep, or a future one — can write a loud proposal without the ack. The sweep hardcodes `acknowledged_loud=false`, so it can only ever auto-apply genuinely-clean writes.
- **Engine still never auto-writes on its own.** The auto-apply *action* is the desktop scheduler; the engine emits proposals and fails safe.
- **Same anti-clobber + fresh re-gate at apply.** The clean path uses the exact SP4 apply chain (base-hash anti-clobber → fresh `propose_write` re-gate → T-I1 op-mapping → atomic temp+rename → durable undo → signed `file_written`). For Create, anti-clobber = the engine's atomic no-clobber create.
- **Accepted residual of Option B (eyes-open, v2-review-confirmed, MEDIUM):** any writable file in a mandate's `source_scope` is a **write-influence vector** for that mandate's target. The rewrite is **model-synthesized** over *all* in-scope sources, so influence is **not** limited to verbatim source text — a poisoned source can steer the output in transformed ways. The only content backstop is `diff_guard`, an **admitted denylist** (secret/value-shaped; `actuator.rs:96` — "never a boundary, misses obfuscation"). So non-secret-but-harmful output (false facts, a non-shell-shaped poisoned config, misleading text) **auto-applies** into a file the user typically trusts *more* than the raw sources. The preventive control is gone on the clean path; the **detective** control (the persistent activity-list + Undo, §Engine change b) is therefore **load-bearing and must never silently drop an auto-applied write.** Bounded by: the explicit per-mandate grant (the user chose the folder), write-grant containment (can't escape the target's folder), the secret-shaped gate, and the activity+Undo trail. This is the deliberate price of "trust your own watched folders."
- **`verify_chain` stays advisory (carried from SP4 — ruled ACCEPTABLE).** Its threat (forging a valid re-encrypted event — e.g. a fake `mandate_grant`) already implies DEK/SQLCipher compromise, which autonomy does not widen. The eventual hardening (gate `get_or_open` on `verify_chain`) remains a deferred, non-blocking follow-up.
- **Local only.** No new network surface (loopback reasoner + network-free embedder; the two-graph network guard stays green). No new secrets / keychain reads. `cargo audit` both crates at implementation time (no new deps introduced by this design).

## Known limitations (named, accepted for SP5)

- **Polled, not instant**; best-case ~5-min latency, more under cap contention, and **worst-case unbounded** under sustained multi-mandate source churn (oldest-first fairness + the shared cap can starve a given mandate for many ticks) (Decision 2; v2-review L3).
- **macOS Create no-clobber is `statat`+`renameat`, not `O_EXCL`-atomic** (`actuator.rs` macOS branch) — a local racer could lose-then-overwrite a file **inside the user's own write-granted folder**. Contained by canonicalize + write-grant + `O_NOFOLLOW` (cannot escape the grant or follow a planted symlink); Linux uses kernel-atomic `RENAME_NOREPLACE`. Narrow, accepted.
- **Auto-apply policy is app-side** (Approach A); the engine fails safe (queues) for any non-desktop caller. (Engine change d still enforces the loud-gate for all callers.)
- **One trust rule, not a tier**; **mandate editing = revoke + re-create**; **no bulk ops**.
- **`pending_proposals()` re-folds `O(events)` per call** and the sweep does 1+K folds (capped at 8) — projection-table is the future fix.
- **Windows deferred** (Unix-gated).

## Testing

- **Engine (`bossclaw-core`):**
  - **(c) trust rule — the load-bearing tests:** a mandate proposal over in-scope authorized sources, non-secret content → `Clean` / `requires_loud_modal == false`; a source **outside** `source_scope` → `Untrusted`/loud; a **sibling** `source_scope-evil` path → **not** cleared (segment-aware, L1); secret-shaped content → loud (via `diff_flags`) even with all-in-scope sources; **after revoke** → loud again; an **unresolvable target** → taint (never skip-escalation, M2). **Scoping proof:** an **M6b reconcile** proposal (target = an ingested file) is **still `Untrusted`/loud** — the trust rule did not leak.
  - **(d) loud-gate:** a loud proposal driven through **each** public write entry without an ack fails closed; permits with `true`; a clean write needs no ack; **an undo of a tainted-file write succeeds** (asserting the `undo_write` exemption is the *only* sanctioned ack-without-UI path — H1).
  - **(a) producer** surfaced in `pending_proposals()` for M6c vs M6b; empty/unknown producer ⇒ not auto-appliable.
  - **(b) `mandate_writes()`** attributes an M6c write, **excludes** an M6b/manual write, flips `undone` after an Undo, and **never silently drops** an auto-applied M6c write — an applied write always appears with an Undo offered (L2).
- **Desktop backend** (`#[cfg(unix)]` `EngineHandle` + `MockVault`/`MockEmbedder`/`ScriptedReasoner`):
  - switch-fix (both flags persist across re-open); Create-apply (applies; refused if target reappeared);
  - **auto-apply sweep — the three that matter:** ① clean mandate proposal → auto-applied; ② risky (secret-shaped / out-of-scope) → stays queued; ③ an M6b reconcile proposal → **NEVER** auto-applied (producer filter *and* trust-scoping). Built with SP4's Tauri ACL discipline (real `__allow_command` grant for a `Remote{http://tauri.localhost}` origin + a positive op-ran signature + mutation-verify).
  - CRUD round-trip + grant-time rejections surface as typed errors.
- **Front-end:** vitest for New-mandate validation, the mandate-list view, and the activity-list view (incl. `undone` disabling Undo).
- **Gates (all green):** `cargo build/test/clippy -p air_agent_desktop` · `cargo test -p bossclaw-core` · `cargo clippy -p bossclaw-core --features ollama -- -D warnings` · `typecheck` · `vitest` · two-graph network guard · `cargo audit` both crates.
- **Manual launch:** mandates on → grant a mandate (target in a write-granted folder, sources in a *read* folder) → tick → a clean change **auto-applies** + shows in Mandate-activity + Undo restores; drop a secret-shaped or out-of-scope source → that rewrite **parks in Review** → Approve; **Revoke** stops further writes; relaunch → mandates **still on**.

## New constants / modules / commands / touch (summary)

- **Engine:** (a) `producer` on `PendingProposal`; (b) `mandate_writes()` + `MandateWriteRecord`; (c) the Step-1 trust exception in `propose_write`; (d) `acknowledged_loud` threaded into `execute_write*`. New const `MANDATE_AUTOAPPLY_PER_SWEEP` may live engine-side or desktop-side (plan decides). **No new events.**
- **Desktop backend:** `prime_switches` mandate guard (+ flipped test); `set_mandates_enabled` op+command; `add`/`revoke`/`list_mandates` ops+commands + `MandateDto`; Create-apply fix; scheduler auto-apply sweep; `producer` through `ProposalSummary`/`ProposalDto`; `api/engine.ts` twins.
- **Desktop frontend:** `src/mandates/*` (MandatesPanel + form/list/activity helpers + tests); `App.tsx` `View += "mandates"` + nav; reuse SP4 Review for the risky path.

## Resolved by brainstorming + review (was open questions)

1. Autonomy → **auto-apply** (mandate grant = standing consent).
2. Tainted path → **auto-apply clean, queue risky**; "clean" made reachable by the trust rule (Decision 7).
3. Detection → **polled ~5 min**; live watcher deferred.
4. Build approach → **A, app-driven** sweep; engine keeps "never auto-write on its own."
5. Audit/undo → **persistent Mandate-activity list + Undo, IN scope** (mandatory join helper).
6. Create-apply gap → **fix in the desktop apply op**; rely on the engine's atomic no-clobber (no desktop pre-check).
7. **(v1-review correction)** "auto-apply clean" was dead code (all ingested sources are `external`) → adopt the **scoped taint-trust rule (Engine change c)** to make it real.
8. **(v1-review hardening)** Move the loud-gate into `execute_write_inner` (Engine change d).

## Future hooks (NOT built here)

- **Live OS watcher** (`watch.rs`) → instant detection.
- **Per-mandate trust tiers**, **mandate editing**, **bulk ops**.
- **Grant-time `source_scope` hygiene warning** — a "this folder should hold only files you author/control" caution (the write-influence analogue of the read-grant warning; v2-review M1).
- **First-open `verify_chain` fail-closed gate** (carried deferral).
- **App-shell redesign** → repositions the Mandates + Review destinations.
- **M7** → battery/thermal-smart scheduler, persisted index, Windows, signer-DID verification.
