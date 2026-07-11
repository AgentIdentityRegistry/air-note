# Rung 3 — "Notice & Reconcile": Semantic Conflict Detection + Confirm-Gated Resolution

**Status:** Design v2 (brainstormed 2026-07-12; revised after architect + critic + security second-opinion
reviews). North Star rung 3 ([[air/memory-strategy-2026-07-03-beat-the-stack]] Phase 3). **Owner
approved full scope (notes + sessions) via the phased build below; not yet planned.**

> **Revision note (v1 → v2):** Three independent pre-code reviews found that captured *sessions* cannot
> be done on the current engine as v1 assumed — sessions are indexed by *title only* (bodies are never
> chunked/embedded/returned), whole-session retire would drop unrelated facts and be reversed by SP3's
> sweeper, and v1's "reuse supersede" overclaimed (no retire-without-replacement primitive exists).
> The owner chose to **build the session engine prerequisites** rather than ship notes-only. v2
> restructures into four gated phases and folds in every review finding (see §16 for the finding→
> resolution map). The core bet is unchanged and was verified sound: a strict, measured local judge +
> a human confirm-gate, with **no code path from judge output to a memory mutation without an owner
> click** (security-verified, I1).

## 1. Product goal

After SP3, AIR Agent captures your Claude Code sessions and remembers notes — but recall hands the
agent **every** matching memory, including ones that **contradict each other**, and it guesses. Rung 3
makes memory **notice when two of its own memories disagree** and **ask the owner who's right**, then
retire the loser so recall stops surfacing stale, contradicted facts. This is the industry's hardest
unsolved memory axis (≤7% on multi-hop knowledge conflicts); we win it not with a cleverer model but
with (a) a strict, measured judge and (b) the human as final arbiter, so a wrong guess costs a
dismissed card — **never a lost memory** (the promise §12 I1 protects).

**Owner decisions (2026-07-12):** semantic contradiction detection (not just temporal); propose +
confirm (never auto-delete); compare **real text**, never a lossy summary; scope = notes **and**
captured-session facts, delivered via the phased build (§3); build the session engine prerequisites up
front rather than defer sessions.

## 2. Non-goals

- **No auto-resolution.** The judge never retires anything on its own (I1).
- **No cloud dependency in cut 1.** The judge is the **local** Reasoner only; cloud is deferred (§15),
  and if ever added must replicate the Milestone-D egress consent gate at its own call-site (I2).
- **No summarization of bodies for judging.** Judge real text only (note body / session passage).
- **No blocking work.** Detection is a background pass; never in the write or recall path.
- **Not reflection/consolidation** (rung 4) and **not signed export** (rung 5).
- **Session-body chunking is scoped to a SEPARATE conflict-detection index** — it must not regress the
  rungs-1/2 recall index (see §7.1; a prior measurement showed chunking hurts the mean-pooling
  embedder's recall). Recall-neutrality is a Phase-1 gate.

## 3. Phased build (each phase gated; harness-first)

| Phase | Deliverable | Exit gate |
|-------|-------------|-----------|
| **0** | Grading harness: frozen labelled conflict set + catch/cry-wolf metrics (§9) | Harness runs on frozen data; produces paired metrics |
| **1** | Engine prerequisites (§7): (a) session-body chunk+embed into a **separate** conflict index; (b) passage retrieval; (c) the new **`retire_memory`** primitive (note + passage granularity) that is sweeper-safe, App-only, reversible | Recall (rungs 1/2) **does not regress** on the harness; retire survives a sweeper cycle; guest refused |
| **2** | Detection (§4): cursor sweep → candidate-finder → local judge → `conflict_proposal`, with budgets (§10) | Judge clears the precision gate (§9); budgets enforced; determinism-hermetic tests pass |
| **3** | Resolution + UI (§6, §11): card with provenance labels, Retire / Keep-both / Dismiss, badge, off-by-default toggle | Security guardrails present (provenance, output-fencing, text-only render); proposal GC works |

The feature is **off by default** and only turns on after Phase 2's judge clears the gate.

## 4. Detection

### 4a. Background sweep
A new engine pass modeled on SP3's capture sweeper: periodic (default: **piggyback the capture
sweeper's cycle** — no new scheduler), gated by the owner flag (§11), cursor over memories
appended/changed since last pass. Never blocks writes or recall. **Fail-safe:** local Reasoner
unavailable → the pass no-ops and retries next cycle (I6).

### 4b. Candidate-finder (cheap, no LLM)
For each new/changed memory, query for its top-k nearest neighbors above `CANDIDATE_SIM_MIN`:
- **Notes** match on their full body (already embedded).
- **Sessions** match on their **passages** via the Phase-1 conflict index (§7.1) — NOT the title-only
  vector (which the architect showed misses most real session conflicts). A candidate is a
  `(memory_or_passage, memory_or_passage)` pair, each carrying a stable identity (§4e).
Exclude: already-superseded/retired memories, the memory's own supersede lineage, pairs marked
`coexist_allowed`, recently-dismissed pairs, and (de-conflict, §8) pairs whose claim maps to an edge
the evolve/extract path already invalidated.

### 4c. Comparable unit = real text
Note ↔ note: the two note bodies. Any pair involving a session: the **actual conflicting passage**
(the Phase-1 retrieval hit's chunk), never a summary, never the whole transcript. Bodies/passages are
bounded by `MAX_JUDGE_TEXT_BYTES` (inherit SP3's snapshot field cap) and fenced as untrusted external
input (I7).

### 4d. The judge (local Reasoner)
One `reasoner.complete_json(CONFLICT_SYSTEM, prompt, conflict_schema())` per candidate pair →
`{ contradicts, winner: "newer"|"older"|"unclear", confidence, why }`. Only
`contradicts && confidence >= CONFLICT_CONF_MIN` becomes a proposal; the rest are **dropped but
counted** (harness telemetry). The judge's `why`/`confidence` are **model self-reports over
attacker-influenceable input** → they are treated as untrusted output (I7): the card never renders
`why` as authoritative, `confidence` is shown only as a coarse High/Med band, and `why` is sanitized +
length-bounded and **must not embed verbatim memory content** (so a signed proposal event can't outlive
a later deletion — security Finding 6). "older" is defined by **ingest `ts`** (deterministic), with the
judge's `winner` a tie-break hint only (resolves §-open Q3).

### 4e. Proposal record
`conflict_proposal` — a signed event `{ a_ref, b_ref, winner_hint, confidence_band, why_sanitized,
detected_at }` where each `*_ref` is a **stable identity**: a note event id, or a
`(session_id, passage_id)` for a session passage (so re-capture that re-mints the session event id does
not orphan the proposal — critic M1). Idempotent: never two open proposals for the same unordered pair.
**Proposal GC (I-gc):** on any supersede/retire/delete/edit of a referenced memory, open proposals that
reference it auto-withdraw or re-target; resolving a proposal against an already-retired target is a
no-op that just closes it (idempotent resolve — security Finding 5).

## 5. (moved) — see §6 resolution operations and §7.3 the retire primitive.

## 6. Resolution operations (App-only, owner-confirmed)
The Library conflict card exposes three actions, all **App-only** (guest/MemoryClient refused by the
fail-closed allowlist — I8; a NEW allowlisted proto op is required, §7.3):

- **Retire older** (default target = strictly-older by ingest `ts`; owner may flip). Calls the new
  `retire_memory` op (§7.3): for a note, a bare supersede; for a session passage, a passage-inert
  marker that survives the sweeper (§7.2). The loser is set aside with an honest "retired <date>"
  label, drops from recall, stays on disk + in the signed log, `as_of` time-travel preserved,
  **reversible** (§7.3). Resolves the proposal.
- **Keep both** — append `coexist_allowed` for the unordered pair (keyed to stable identities, §4e);
  both stay active; never re-proposed. If a coexist'd memory is later materially edited (new event id),
  the pact is re-evaluated (mN2).
- **Dismiss** — resolve + snooze the pair. **Re-open rule (defined):** a dismissed pair re-opens only
  if one member is materially changed (a new event id via edit) — otherwise it stays snoozed. (This is
  the concrete distinction from Keep-both; without it the two collapse — critic "what's missing".)

Edge-invalidation (§7.4) happens **only** inside a confirmed Retire, atomically with the supersede —
never during detection (closes the I1 hole, security Finding 4).

## 7. Engine prerequisites (Phase 1)

### 7.1 Session-body passage index (separate from recall)
Chunk captured session bodies (on-disk transcripts) and embed chunks into a **separate conflict index**,
not the rungs-1/2 recall index. Rationale: a prior frozen measurement showed chunking *hurts* the
mean-pooling embedder's recall; isolating the chunk vectors keeps conflict-detection recall high without
regressing answer-quality recall. **Gate:** the harness must show recall (rungs 1/2) unchanged. Chunk
identity = `(session_id, passage_id)`; stable across re-capture where the passage bytes are unchanged.

### 7.2 Sweeper-safe retire state
SP3's sweeper re-captures any session not in `fold.deleted` (the `None` arm re-appends). A bare session
supersede is therefore reversed within one cycle. Phase 1 adds a **`passage_retired`/`memory_retired`
state** distinct from `session_deleted`: recognized by `capture_session`'s fold so a retired session (or
retired passage) is **not** re-captured, yet is **not** delete-forever (unlike the `session_deleted`
tombstone). This is the state SP3 lacked; it is what makes session retire durable AND reversible.

### 7.3 The `retire_memory` primitive (honest — this is NEW, not reuse)
A new engine op + **new proto `Request` variant** + **new entry in the fail-closed role allowlist**
(guest refused; App-only). It bare-supersedes a note (no replacement text — unlike `supersede_note`
which forces an edit-in-place replacement) or marks a session passage retired (§7.2). Reversibility is a
new append-only `unretire` event honored by `superseded_ids`/`fold_notes`/recall (the architect showed
recall exclusion is currently one-way — this is a small new fold, budgeted, NOT free reuse). Tests must
cover guest-refusal of the new op.

### 7.4 Edge-invalidation (bonus, resolution-time only)
Where a retired memory maps to a derived graph edge, stamp `invalidated_at`/`invalidated_by` inside the
confirmed Retire transaction (reuse `append_pair`), keeping `as_of` truthful. Detection never touches
edges (I1).

## 8. Recall / evolve wiring + de-conflict with the extract path
- Recall filtering is unchanged — retired memories are excluded via the existing supersede exclusion
  set + the new retire state; rung 3 only feeds these mechanisms.
- **De-conflict (architect):** the evolve/extract path already reconciles *edge-level* contradictions
  (`ProposedRetraction` → `reconcile_confirmed_contradiction`). Rung 3 is memory-level and complementary,
  but must **suppress** a conflict proposal whose claim maps to an edge the extract path already
  invalidated (else double-cards for facts the graph already handled). At minimum, document the two
  independent axes; preferably filter (§4b exclusion).
- **I7 taint:** the judge reading external-tainted content is analysis; the only action it can trigger
  is an owner-confirmed proposal → no auto-apply of tainted content.

## 9. Grading harness (Phase 0 — ships before the feature turns on)
Rung-0 discipline in `memharness` (reuses the frozen-corpus scaffolding):
- A **frozen** labelled set: true-contradictions (with correct winner) + hard negatives (same-topic
  non-conflicts like tabs-in-Python/spaces-in-Go, unrelated look-alikes). Seeded from the owner's real
  contradictions + synthetic. Also measures, for sessions, **passage-index vs title-only catch rate**
  (proves §7.1 earns its cost).
- Metrics, paired per-case (Wilcoxon + CIs): **catch rate (recall)** and **cry-wolf rate (1−precision)**.
- **Ship gate (provisional, owner-tunable):** **precision ≥ 0.90 (bootstrap-CI lower bound, conf 0.90)
  at recall ≥ 0.30** on the frozen set, tuned via `CONFLICT_CONF_MIN`. (A single-arm precision
  *proportion* → a bootstrap/Wilson interval is the right tool; "Wilcoxon" is *paired*, used only for
  the passage-vs-title comparison.) If it flunks: tune, or fall back to a deterministic temporal-only
  mode (retire only where an explicit bi-temporal edge already retired the fact — no judge). A number,
  not "we'll know it when we see it" (critic M2).
- **The binding gate needs a big-enough set (post-review, owner decision 2026-07-12).** A precision CI
  over the tiny plumbing seed is degenerate (~5 flags → pass↔fail on one example). So **Phase 0 is a
  plumbing + first-signal SMOKE** (exit = 0 false positives AND ≥ 2/5 caught on the seed, + record the
  live judge's raw numbers); the binding ≥0.90-CI gate above applies only after a **50+ true-
  contradiction + matched-hard-negative** owner-sourced frozen set is assembled — built next, only if
  the smoke signal is promising (don't hand-label 50+ pairs before the model shows it can judge at all).

## 10. Volume / flood control (cry-wolf by count, not just rate)
I4 bounds *compute*; these bound *cards* and *judge calls* (critic M3, security Finding 3):
- **`CONFLICT_JUDGE_PER_SWEEP`** (default 8, mirroring SP3's capture cap) — per-sweep judge-call budget;
  backlog drips across sweeps.
- **Open-proposal ceiling** — on exceed, the sweep stops proposing and surfaces ONE quiet "many
  conflicts pending" state, not N cards.
- **First-enable backfill drip** — the cursor sees the whole existing corpus as "new" on first enable;
  cap new proposals per cycle so day-one is a trickle, not a wall.
- Confirm the SP3 guest token-bucket rate limit covers `Remember` so a hostile client can't force
  unbounded sweep work.

## 11. UI + control surface
- **Off by default, opt-in** Brain/Settings toggle ("Notice conflicting memories") with plain-English
  disclosure (local model; surfaces cards; never deletes without you). Off → sweep never runs (I3).
- **Library conflict card** (new; reuses SP3 Library idioms + honest-consent pattern + **text-only
  `<pre>` rendering, never `dangerouslySetInnerHTML`** — inherits SP3's XSS guard): shows both sides
  **each labelled with provenance** ("your note" vs "**untrusted** captured session — <source, date>"),
  the coarse confidence band, and `[Retire older] [Keep both] [Dismiss]` with a flip-which-side
  affordance. Tokens only. Provenance labelling is the primary defense against the deceptive-card attack
  (security Finding 1) — it reframes the owner's question from "which sounds right?" to "one of these is
  untrusted."
- **Badge:** open-proposal count on the Library nav (quiet; no modal interrupts).

## 12. Invariants
- **I1 — Never auto-retire.** No code path retires/invalidates a memory or edge without an owner
  App action. Edge-invalidation is resolution-time-only (§7.4). *(Security-verified: holds even under
  full judge compromise.)*
- **I2 — Local & private.** Cut 1 judge is local-only; zero egress. If cloud is ever added, it replicates
  the Milestone-D signed-consent gate at the sweep call-site AND feeds the R4 transparency banner AND is
  disclosed in the opt-in copy.
- **I3 — Off by default, consent-gated.** Off → zero side effects, no model calls, no proposals.
- **I4 — Grows with new, not total.** Cursor-incremental + similarity-gated + background; per-cycle cost
  ≈ new × k, **sublinear** in corpus size (HNSW ANN), never O(N²), no re-embed in the finder.
- **I5 — Append-only honesty.** Every detection/resolution is a signed event; `as_of` preserved; retire
  reversible; nothing silently mutated.
- **I6 — Fail-safe.** Reasoner down → no-op; crash mid-resolve is idempotent/self-healing (no double
  supersede); orphan states healed on boot (SP3 pattern).
- **I7 — Hostile-input discipline (input AND output).** Judge input fenced as untrusted; judge **output**
  (`why`/`confidence`) treated as untrusted too — never authoritative, sanitized, no verbatim content in
  the stored proposal; card render is text-only.
- **I8 — App-only resolution.** Retire/KeepBoth/Dismiss + the new `retire_memory` op are App-only; guest
  refused by the fail-closed allowlist (do NOT add them to the `MemoryClient` arm).
- **I9 — Strict-quiet.** High threshold; unclear dropped; resolved/keep-both/dismiss suppress re-nagging;
  no duplicate open proposals; volume budgets (§10).
- **I-gc — Referential integrity.** Open proposals auto-withdraw/re-target when a referenced memory is
  retired/deleted/edited; resolve is idempotent against an already-retired target.
- **I-multiway — Pairwise coherence.** On retire of X, auto-withdraw all open proposals referencing X;
  "Keep both" is pair-scoped. (Full 3+-way resolution deferred, but no incoherent state — critic M4.)

## 13. Testing
- Phase 1: chunk/passage index built + queried; **recall-neutrality** assertion (rungs 1/2 unchanged);
  retire survives a simulated sweeper cycle; `retire_memory` guest-refused; unretire round-trips.
- Detection: similarity gate, exclusions, cursor incrementality; **hermetic determinism** — the
  candidate-finder/HNSW layer is seeded or stubbed (HNSW top-k is non-deterministic across rebuilds —
  architect), not just the Reasoner; schema-validated verdict; threshold drop; taint-fencing of passages.
- Proposal: stable-identity refs; idempotent per pair; GC on referenced-memory change; survives restart.
- Resolution: Retire → supersede/passage-retire, excluded from recall, reversible, `as_of` intact,
  flip-side; KeepBoth → coexist, never re-proposed; Dismiss → snooze + material-change re-open;
  idempotent double-resolve; edge-invalidation only inside confirmed Retire.
- Security: guest cannot create/resolve proposals; deceptive-card provenance labels present; `why` not
  authoritative + no verbatim content; local-only (no egress) in cut 1; volume budgets enforced;
  hostile near-duplicate corpus does not blow up judge calls or proposal count.
- Harness: frozen set, catch/cry-wolf metrics, the pinned ship-gate assertion, deterministic-mode
  fallback, passage-vs-title catch-rate comparison.
- Frontend vitest: card render + 3 actions + flip-side + provenance labels + coarse band + badge +
  off-state + 0 hardcoded colors.

## 14. Constants (pinned provisionally — tune in the harness)
`CANDIDATE_SIM_MIN` (start conservative-high; cost governor + precision boost) · `CONFLICT_CONF_MIN`
(start high; the strict-quiet dial) · `MAX_JUDGE_TEXT_BYTES` (inherit SP3 snapshot field cap) ·
`CONFLICT_JUDGE_PER_SWEEP` = 8 · open-proposal ceiling (start ~20) · sweep cadence = piggyback the
capture sweeper. Exact values are owner/harness-tuned; they are named so the plan is sizable.

## 15. Deferred (tracked, not rung 3 cut 1)
Cloud judge (stays opt-in + gated + disclosed). Full multi-way (3+) conflict resolution. Auto-retire for
very-high-confidence pairs (only after the harness earns trust). Conflict-aware recall re-ranking without
resolution (fold in only if measurement wants a no-mutation surface). Cross-machine conflict sync
(rung 5). Digest/notification-center UX beyond the "many pending" state.

## 16. Review findings → resolution map (v1 reviews)
- **Architect C1/C2 (sessions title-only; retire fights sweeper; undo not reuse)** → §7 engine
  prerequisites (passage index, sweeper-safe retire state, honest new primitive + budgeted unretire).
- **Architect (memory-first correct; de-conflict extract path; I4 sublinear; HNSW test hermeticity)** →
  §8 de-conflict, I4 wording, §13 seeded candidate-finder.
- **Critic C1 (whole-session retire loses unrelated facts)** → §7.1/§7.2 passage-level retire.
- **Critic C2 (§5 overclaims reuse)** → §7.3 named new primitive + proto + allowlist + tests.
- **Critic M1 (proposal referential integrity)** → §4e stable identities + I-gc.
- **Critic M2 (undefined ship-gate)** → §9 pinned precision ≥0.90 CI-lower at recall ≥0.30.
- **Critic M3 (volume flood / first-enable)** → §10 budgets + backfill drip.
- **Critic M4 (pairwise incoherence)** → I-multiway.
- **Critic mN1/mN2/mN3 + Dismiss/Keep-both over-build** → §4d "older"=ingest ts, §6 re-open rule, §4d
  bounded/sanitized `why`.
- **Security 1 (deceptive card)** → §11 provenance labels + §4d output-fencing + coarse band.
- **Security 2 (cloud egress bypass)** → I2 local-only cut 1 + gate-at-call-site if ever cloud.
- **Security 3 (DoS amplification)** → §10 budgets + ceiling.
- **Security 4/5/6 (edge-invalidation I1 hole; crash double-supersede; verbatim `why` leak)** → §7.4
  resolution-time-only, I-gc idempotent resolve, §4d no-verbatim-`why`.

## 17. Open questions for owner/next review
1. Ship-gate numbers (§9): confirm precision ≥0.90 / recall ≥0.30 as the provisional bar.
2. Is a **separate** conflict index (§7.1) acceptable disk/complexity cost, vs. accepting some recall
   regression from a shared chunked index? (Harness can quantify the regression to decide.)
3. Phase ordering: is the harness-first, engine-prereq-second ordering acceptable given it delays the
   visible feature by the Phase-1 engine work?
