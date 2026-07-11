# Rung 3 — "Notice & Reconcile": Semantic Conflict Detection + Confirm-Gated Resolution

**Status:** Design (brainstormed 2026-07-12, pre-review). North Star rung 3 of the memory strategy
([[air/memory-strategy-2026-07-03-beat-the-stack]] Phase 3). Builds directly on SP3's `supersede`
plumbing and Milestone D's Reasoner. **Not yet reviewed; not yet planned.**

## 1. Product goal

After SP3, AIR Agent captures your Claude Code sessions and remembers notes — but recall hands the
agent **every** matching memory, including ones that **contradict each other**. If a note says
"deploy on Vercel" and a later captured session says "migrated off Vercel to Fly," the agent sees
both and guesses.

Rung 3 makes memory **notice when two of its own memories disagree** and **ask the owner who's
right** — then retire the loser through the existing supersede machinery so recall stops surfacing
stale, contradicted facts. This is the industry's hardest unsolved memory axis (≤7% on multi-hop
knowledge conflicts); we win it not by a cleverer model but by (a) a strict, measured judge and
(b) keeping the human as the final arbiter so a wrong guess costs a dismissed card, never a lost
memory.

**Decisions locked in brainstorming (owner, 2026-07-12):**
- **Detection scope = semantic contradiction** (judge-gated), not just explicit temporal supersession.
- **Resolution posture = propose + confirm** (the owner decides; nothing auto-deletes).
- **Comparable unit = real text**, never a lossy summary: a note's body, or the *actual matched
  passage* of a captured session (retrieval already pinpoints it).
- **Scope of memories = notes + captured-session facts** (human-meaningful memories), not raw file
  chunks.

## 2. Non-goals (explicit)

- **No auto-resolution.** The judge never retires a memory on its own. Ever. (Invariant I1.)
- **No new embedder / retrieval change.** Reuses the shipped recall/vector search as the candidate
  finder. Rung 3 is orthogonal to rungs 1–2.
- **No cloud dependency.** The judge is the local Reasoner by default; cloud only if the owner already
  enabled it (Milestone D consent). Off-key / offline degrades to "pass doesn't run," never a failure.
- **No summarization of session bodies.** Explicitly rejected in brainstorming — summaries drop the
  position that the contradiction lives in. We judge real text only.
- **No blocking work.** Detection is a background pass; it never sits in the write path or the recall
  path.
- **Not reflection/consolidation** (that's rung 4) and **not signed export** (rung 5).

## 3. Architecture overview

```
                         (background sweep, cursor over new/changed memories)
new/changed memory ──► candidate-finder ──► judge (local Reasoner) ──► conflict_proposal event
   (note or session)      │ top-k similar        │ structured verdict        │ (signed, in log)
                          │ neighbors above      │ {contradicts, winner,     │
                          │ a similarity bar     │  confidence, why}         ▼
                          │ (reuse recall)       │ high-conf only     Library conflict card
                          ▼                      ▼                     [Retire older][Keep both][Dismiss]
                    real text pairs        drop unclear/low-conf              │
                    (note body / matched   (counted for the harness)          ▼ owner decides
                     session passage)                              ┌──────────┴───────────┐
                                                                   ▼                      ▼
                                                          supersede loser        coexist_allowed
                                                          (bi-temporal invalidate) (never re-propose)
                                                                   ▼
                                                          recall excludes it (SP3 mechanism)
```

Every arrow into the log is a **signed append-only event** — no hidden state; survives restart;
auditable; undoable.

## 4. Detection

### 4a. The background sweep

A new engine pass, modeled on SP3's capture sweeper (`crates/bossclawd/src/…`): periodic, gated by
an owner flag (§9), cursor-based over memories appended/changed since the last pass. It **never**
blocks writes or recall. Fail-safe: if the Reasoner is unavailable, the pass no-ops and retries next
cycle (I6).

**Cost governor (invariant I4 — "grows with new, not total"):** the sweep processes only
*new/changed* memories (cursor), and for each pulls only its top-k *similar* neighbors. Work per
cycle scales with new memories × k, independent of total corpus size. No O(N²) all-pairs scan exists
anywhere in the design.

### 4b. Candidate-finder (cheap, no LLM)

For each new/changed memory, query the existing recall/vector search for its top-k neighbors, keep
only those above a **similarity threshold** `CANDIDATE_SIM_MIN`. Rationale: two memories that aren't
about the same thing can't meaningfully contradict — pre-filtering by similarity is both the cost
governor and a precision boost (removes most false alarms before the judge runs). Exclude:
already-superseded memories, the memory's own supersede lineage, and pairs already marked
`coexist_allowed` (§6) or recently dismissed.

### 4c. The comparable unit (real text, never a summary)

- **Note ↔ note:** compare the two note bodies directly (already short atomic claims).
- **Note ↔ session** or **session ↔ session:** compare the *matched passage* the candidate-finder
  returned for the session (the retrieval hit's chunk/snippet), **not** a summary and **not** the
  whole transcript. The passage is already short and is real text.
- Passages/bodies are bounded (`MAX_JUDGE_TEXT_BYTES`) and sanitized on the same external-taint
  discipline as SP3's snapshot (captured content is `EXTERNAL_ORIGIN`; the judge prompt fences it as
  untrusted data — see I7).

### 4d. The judge (local Reasoner)

For each candidate pair, one `reasoner.complete_json(CONFLICT_SYSTEM, prompt, conflict_schema())`
call returning a **structured verdict**:

```json
{ "contradicts": true, "winner": "newer" | "older" | "unclear", "confidence": 0-100, "why": "…" }
```

- `winner` defaults reasoning to recency but the field is advisory — the owner can override at the
  card (§6).
- Only `contradicts == true && confidence >= CONFLICT_CONF_MIN` becomes a proposal. `unclear`,
  `false`, or low-confidence pairs are **dropped** — but **counted** (telemetry) so the harness can
  measure catch-rate vs cry-wolf-rate (§8).
- Deterministic-pipeline caveat: the judge is the *only* LLM in rung 3, and it only ever produces a
  *proposal* — never a mutation. (Contrast SP3's I5: render/snapshot/telemetry stay LLM-free.)

### 4e. The proposal record

`conflict_proposal` — a new signed event type: `{ newer_id, older_id, winner, confidence, why,
detected_at }`. The Library card and badge read from the set of open (unresolved) proposals. Writing
it as an event (not transient state) means it survives restart and is auditable. Idempotent: never
emit a second open proposal for the same unordered `{a,b}` pair.

## 5. Resolution primitives (reuse)

Recall already excludes superseded memories (SP3: the retain closure's exclusion set + fold-derived
session exclusion). Rung 3 adds **no new recall filtering** — it feeds the existing mechanism.

- `EventLog::supersede` / `supersede_note` (log.rs) — retire a memory by appending a supersede pair.
- Bi-temporal edges (`valid_from`/`valid_to`/`invalidated_at`/`invalidated_by`, graph.rs) +
  `as_of()` (log.rs) — when the retired memory maps to a derived graph edge, also stamp
  `invalidated_at`/`invalidated_by` so time-travel stays truthful. (First cut: memory-level supersede
  is the load-bearing path; edge invalidation is a bonus where an edge exists.)

## 6. Resolution operations (App-only, owner-confirmed)

The Library conflict card exposes three actions; all are App-only (a MemoryClient/guest can never
invoke them — same fail-closed role allowlist SP3 enforces):

- **Retire older** (default) — supersede the loser (owner may flip which side is the loser). The
  retired memory is set aside with an honest label ("retired <date>, replaced by a newer memory"),
  drops out of recall, stays on disk + in the log, time-travel preserved, **undoable**. Resolves the
  proposal.
- **Keep both** — append a `coexist_allowed` event for the unordered pair; both stay active; the
  sweep never re-proposes this pair. Resolves the proposal.
- **Dismiss** — resolve the proposal and suppress re-proposal of this pair (lighter than
  `coexist_allowed`: no permanent "these are fine" claim; a materially changed memory could re-open).

All three are signed log entries. "Undo retire" is another honest append (reuses SP3's undo pattern).

## 7. Recall / evolve wiring

- **Recall:** unchanged filtering — retired memories are already excluded via the supersede exclusion
  set. Rung 3's only recall-facing effect is that more memories become supersede targets.
- **Evolve/extract (I7 taint):** the judge reading captured (external-tainted) content is analysis,
  and the *only* action it can trigger is a proposal the owner must confirm — so no auto-apply of
  tainted content. A resulting supersede is a user action, not an engine auto-write. Consistent with
  the engine's taint rules and SP3's I6.

## 8. The grading harness (measure-first — ships before the feature turns on)

Rung-0 discipline: **prove the judge before it can bug the owner.**

- A **frozen** labelled set of memory pairs: true-contradictions (with the correct winner) + hard
  negatives that look similar but don't contradict (tabs-in-Python / spaces-in-Go; same-topic
  non-conflicts; unrelated). Seeded from the owner's real corpus contradictions + synthetic.
- Grade two numbers, paired per-case (Wilcoxon + CIs per the harness convention):
  - **Catch rate (recall):** of true contradictions, how many flagged.
  - **Cry-wolf rate (1 − precision):** of flagged pairs, how many were not real conflicts.
- **Ship gate:** the feature only turns on if it clears a bar with **precision weighted over recall**
  (a wrong card erodes trust more than a missed conflict — see I-tuning). If it flunks: tune
  threshold/prompt, or fall back to a deterministic temporal-only mode (supersede only when an
  explicit bi-temporal edge already retired the fact — no judge).
- Lives in `memharness` (reuses the rungs-0–2 scaffolding); frozen corpus + frozen pairs so tuning is
  measured paired/apples-to-apples. Doubles as the `CONFLICT_CONF_MIN` tuning dial.

## 9. UI + control surface

- **Off by default, opt-in** (like capture): a Brain/Settings toggle "Notice conflicting memories"
  with plain-English disclosure (uses the local model; surfaces cards; never deletes without you).
  Off → the sweep never runs (I3).
- **Library conflict card** (new): reuses SP3's Library panel idioms + the honest-consent pattern.
  Shows both memories, dates, the judge's suggested winner + confidence, and `[Retire older]`
  `[Keep both]` `[Dismiss]` (with an affordance to flip which side is retired). Tokens only.
- **Badge/count:** open-proposal count on the Library nav (quiet — no modal interrupts).

## 10. Invariants

- **I1 — Never auto-delete/auto-retire.** Detection proposes; the owner disposes. No code path retires
  a memory without an owner-confirmed App action.
- **I2 — Local & private by default.** Judge = local Reasoner; no egress unless cloud was already
  enabled + consented (Milestone D). Captured text never leaves the machine for detection by default.
- **I3 — Off by default, consent-gated.** The whole pass is opt-in; off → zero side effects, no model
  calls, no proposals.
- **I4 — Grows with new, not total.** Cursor-incremental + similarity-gated + background; per-cycle
  cost scales with new memories, not corpus size. No all-pairs scan.
- **I5 — Append-only honesty.** Every detection/resolution is a signed event; history + `as_of`
  time-travel preserved; retire is undoable; nothing is silently mutated.
- **I6 — Fail-safe.** Reasoner unavailable/offline → pass no-ops and retries; crash between propose
  and resolve leaves a valid open proposal (idempotent); retire is crash-safe (SP3 patterns).
- **I7 — Hostile-input discipline.** Captured content is `EXTERNAL_ORIGIN`; the judge prompt fences it
  as untrusted data (SP3 snapshot sanitization family), bounded bytes; a contradiction verdict can
  never itself become an executed instruction.
- **I8 — App-only resolution.** Retire/KeepBoth/Dismiss are App-only; guest/MemoryClient roles are
  refused (SP3 fail-closed allowlist).
- **I9 — Strict-quiet.** High confidence threshold; unclear dropped; resolved/keep-both/dismiss
  suppress re-nagging; no duplicate open proposals per pair.

## 11. Testing

- Candidate-finder: similarity gate, exclusions (superseded, own lineage, coexist, dismissed), cursor
  incrementality, empty/degenerate corpora.
- Judge layer: schema-validated verdict parse; threshold gate (drop unclear/low-conf); external-taint
  fencing of session passages; deterministic given a stubbed Reasoner.
- Proposal: idempotent per unordered pair; signed; survives restart; resolves correctly.
- Resolution: Retire → supersede appended, memory excluded from recall, undoable, `as_of` still shows
  it historically; flip-which-side; KeepBoth → coexist_allowed, never re-proposed; Dismiss → suppressed.
- Role/consent: guest cannot resolve; pass off → zero side effects; local vs cloud judge selection.
- Fail-safe: Reasoner down → no-op; crash between propose/resolve → recoverable.
- Grading harness: frozen set, catch/cry-wolf metrics, the ship-gate assertion, deterministic-mode
  fallback.
- Frontend vitest: conflict card render, three actions, flip-side, badge count, off-state, tokens/0
  hardcoded colors.

## 12. Deferred (tracked, not in rung 3)

- Auto-retire (silent, with undo) for very-high-confidence pairs — only after the harness earns trust.
- Cloud judge as default (stays opt-in; local is the baseline).
- Multi-way conflicts (3+ memories on one claim) — first cut is pairwise.
- Conflict-aware recall re-ranking without resolution (option C from brainstorming) — folded in only
  if measurement wants a no-mutation surface.
- Graph-edge-level conflict UI (this cut acts memory-first; edge invalidation is a byproduct).
- Cross-machine conflict sync (rung 5 territory).

## 13. Open questions for review

1. **First-cut memory scope:** notes + sessions (via matched passage) from day one, or **notes-only**
   first (highest precision, zero passage-noise) then add sessions once the harness clears them? The
   spec assumes notes+sessions with the harness as the gate and a notes-only fallback seam.
2. **Sweep cadence + trigger:** pure timer, or piggyback on the capture sweeper's cycle, or
   post-write debounce? (Perf invariant I4 holds regardless.)
3. **`winner` advisory vs. computed:** trust the judge's `winner`, or always default the retire target
   to the strictly-newer memory (recency) and treat the judge's `winner` as a tie-break hint?
4. **Confidence surfacing:** show the numeric confidence on the card, or a coarse High/Medium band?
5. **Deterministic fallback bar:** what precision floor flips us from judge-mode to
   temporal-only-mode if the harness flunks?
