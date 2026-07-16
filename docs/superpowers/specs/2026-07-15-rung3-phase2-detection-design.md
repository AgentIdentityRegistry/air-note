# Rung 3 — Phase 2: Semantic Conflict **Detection** — Design (2026-07-15)

**Status:** Design approved by owner 2026-07-15 (this doc). Refines §4/§7.1 of the parent design
`docs/superpowers/specs/2026-07-12-rung3-conflict-resolution-design.md` with the as-built engine
(Rung 3 Phase 1 merged to `main` `64207b5`) + one owner refinement (the **unified fights index**, §2).

**Parent context.** North Star rung 3 = "Notice & Reconcile" (`air/memory-strategy-2026-07-03-beat-the-stack`).
The parent design's phased build:
- **Phase 0 — grading harness:** SHIPPED (P0 judge `crates/bossclaw-core/src/conflict.rs` + `memharness`
  conflict-grade; §9 binding gate PASSED synthetic + owner-real).
- **Phase 1 — engine prerequisites:** SHIPPED (reversible retire note+passage; separate `conflict_index`;
  session passages persisted at capture; recall byte-untouched). PR #79 → `main` `64207b5`.
- **Phase 2 — DETECTION (this doc):** background sweep → candidate-finder → local judge → signed
  `conflict_proposal`. Off-by-default; never blocks recall/writes; emits records only (no UI, no mutation).
- **Phase 3 — resolution + surfacing:** deferred. The card/where-conflicts-surface decision
  (desktop Library card §11 vs. the 2026-07-14 background-first "surface IN Claude Code" insight,
  `air/vision-background-first-claude-code-native-2026-07-14`) is resolved THERE, not here.

## 1. Goal (this phase)

Teach the daemon to **notice** when two of its own memories contradict each other and record a
**"possible conflict"** — a signed proposal listing both sides — for a later owner decision. Detection
does nothing else: it never picks a winner, never retires, never surfaces UI, and never runs unless the
owner has switched it on. A wrong judge call costs a dropped counter, never a memory (I1).

## 2. Owner refinement — the **unified fights index** (supersedes parent §4b's two-index candidate-finder)

The parent design had the candidate-finder straddle two indexes (notes in the recall index via
`vector_search`, sessions in the `conflict_index` via `conflict_search`). **Owner decision 2026-07-15:
unify them.** Detection queries ONE index.

- The recall/finding index stays **exactly as-is** — the measured recall-neutrality wall (chunk vectors
  must not pollute the mean-pooling recall index, parent §7.1) is preserved. This refinement does NOT
  touch recall.
- The **`conflict_index`** is extended to hold BOTH kinds of conflict-candidate vectors:
  - **session passages** (already there): keyed `encode_chunk_key(session_id, passage_ix)`.
  - **note bodies** (new): a current memory-kind note's body vector, keyed by a distinct
    **note key** (e.g. `encode_note_key(event_id)`), disjoint from the passage key space.
- `conflict_search(qv, k)` therefore returns a mixed candidate list; each hit decodes to a **typed ref**:
  `Note{event_id}` or `Passage{session_id, passage_id}` (the §4e stable-identity scheme, already the keys
  Phase 1 uses). Cross-kind fights (note↔passage) fall out for free — no straddling, no "which fights"
  tradeoff.
- Cost: a note's body vector is duplicated (once in recall, once in the fights index) — cheap; notes are
  short (one vector each). `rebuild_conflict_index` gains a note-embedding arm.

**Recall-neutrality remains a Phase-2 gate:** a test must assert the recall `vector_index` is
byte-untouched after the extended `rebuild_conflict_index` (reuse Phase 1's `vector_index_len` golden
assertion).

## 3. Components (as-built anchors verified 2026-07-15)

### 3.1 The unified fights index (`bossclaw-core`)
Extend, in `crates/bossclaw-core/src/`:
- `index.rs` — add `encode_note_key`/`decode_note_key` beside `encode_chunk_key`/`decode_chunk_key`
  (a distinct sentinel so a note key never collides with a `(session_id, passage_ix)` key). Both id kinds
  are already separator-safe (note event ids are ULIDs; session ids are A5-validated `[A-Za-z0-9_-]`).
- `log.rs` — `rebuild_conflict_index(&emb)` (currently `:~5838`) gains a note arm: for each **current,
  non-superseded, non-retired** memory note (reuse `current_notes`/`fold_notes` exclusion + the fold's
  `retired_notes`), embed its body and `index.add(&encode_note_key(event_id), &vec)`. Passage arm
  unchanged. A new `conflict_search` result mapping returns `enum ConflictRef { Note{event_id},
  Passage{session_id, passage_id} }` + score. Note bodies come from the memory event content (already
  embeddable); reuse the same embedder path.

### 3.2 The conflict cursor (`bossclaw-core` state + `bossclawd` read)
The capture sweeper (`crates/bossclawd/src/capture/sweeper.rs`) is **stateless-rescan — no cursor to
reuse**. Add a conflict cursor mirroring the **evolve cursor** (`log.rs` `set_evolve_cursor` `:~6090`):
a persisted "last conflict-swept seq" so each cycle only examines memories appended/changed since
(I4: grows with new, not total). New `conflict_cursor()` / `set_conflict_cursor(seq)` on `EventLog`.

### 3.3 The background sweep (`bossclawd`)
Piggyback the capture sweeper's cadence (`sweeper.rs::spawn`, `SWEEP_INTERVAL = 300s`,
`MissedTickBehavior::Skip`). Add a `run_conflict_sweep_once(engine, now)` invoked from the same loop
(or a sibling `spawn`), **gated first** on the owner flag (§3.6). Per cycle:
1. If flag off → return immediately (no scan, no model). (I3.)
2. Advance from `conflict_cursor()`; collect new/changed current memories (notes + session capture heads)
   up to a scan bound.
3. For each, run the candidate-finder (§3.4); accumulate candidate pairs.
4. Judge pairs up to the per-cycle budget `CONFLICT_JUDGE_PER_SWEEP = 8` (backlog drips across cycles,
   mirroring `CAPTURE_PER_SWEEP = 8`). Excess pairs are left for the next cycle (cursor only advances
   past fully-processed items, or a separate "pending pairs" queue — plan decides the simplest correct
   form).
5. Emit proposals (§3.5); on the open-proposal ceiling, stop proposing and surface one quiet state.
6. Reasoner unavailable at any step → **no-op this cycle, retry next** (I6); never panics (mirror
   `run_sweep_once`'s swallow-and-count discipline).

### 3.4 Candidate-finder (cheap, no LLM)
For each new/changed memory X: embed X (note body or the session's passage(s)); `conflict_search` the
unified index for top-k above `CANDIDATE_SIM_MIN`. A candidate is an unordered `(ref_a, ref_b)` pair of
typed `ConflictRef`s. **Exclude (Phase 2):** X paired with itself / its own supersede lineage; already
superseded/retired refs (fold sets); and (de-conflict §3.7) pairs already covered by an OPEN
`conflict_proposal` (the idempotency fold). **Exclude (Phase 3, no-op in Phase 2):** pairs marked
`coexist_allowed` and recently-`dismissed` pairs — those events can only be created by the Phase-3
resolve ops, so in Phase 2 the finder consults an **empty** resolution set. Write the finder to take a
resolution-exclusion set (empty in Phase 2) so Phase 3 fills it without reshaping the finder. Sublinear:
HNSW ANN top-k, no re-embed of the corpus, no O(N²).

### 3.5 The judge + proposal record
- **Judge:** order the pair older/newer by ingest `ts`; `conflict::judge_pair(&*reasoner, older_text,
  newer_text) -> Result<Option<Verdict>, _>`. The Reasoner is obtained via
  `self.reasoner_provider.reasoner()?` **after** the evolve loop's cloud-consent pre-gate
  (`engine/mod.rs` `evolve_once` `:~926-934`) — factor that gate into a shared helper so a conflict sweep
  can NEVER egress tainted content without consent (I2). `Some(Verdict)` → proposal; `None` → increment a
  dropped counter (telemetry); `Err` → treat as reasoner-unavailable for this pair (I6).
- **Comparable unit = real text** (parent §4c): note body or the actual conflicting passage chunk —
  never a summary, never the whole transcript. Bounded by `MAX_JUDGE_TEXT_BYTES` (inherit SP3's snapshot
  field cap) and fenced as untrusted (the judge already `defuse()`s fence-mimicry internally).
- **Proposal record:** a signed `conflict_proposal` event, mirroring the `write_proposal` family
  (`log.rs` `append_write_proposal_with` `:~2667`, idempotency `is_proposal_suppressed` `:~2733`,
  projection `pending_proposals` `:~2780`). Content:
  `{ a_ref, b_ref, winner_hint, confidence_band, why_sanitized, detected_at }` where each `*_ref` is a
  typed stable identity (§2). **Idempotent:** never two OPEN proposals for the same unordered pair.
  **I7 hostile-output discipline:** `why` is sanitized + length-bounded and **must not embed verbatim
  memory content** (so a signed proposal can't outlive a later deletion of the memory it quotes);
  `confidence` stored only as a coarse band (High/Med); the judge's `winner` is an advisory hint —
  "older" is defined by ingest `ts` (deterministic).
- **Add (Phase 2):** `CONFLICT_PROPOSAL_EVENT_TYPE = "conflict_proposal"` in `graph.rs` beside
  `WRITE_PROPOSAL_EVENT_TYPE`. The proposal fold computes the OPEN set (idempotency) and applies **GC
  against events that already exist in Phase 2** — a referenced memory's `note_retired`/`passage_retired`/
  supersede/`session_deleted` auto-withdraws the open proposal (I-gc). The **resolution family**
  (`conflict_resolved`/`coexist_allowed`/`dismissed`) is **Phase 3** (added when the resolve ops that
  emit them ship); Phase 2 adds no dead event types.

### 3.6 Owner gate (off-by-default)
Add `ConfigFlag::ConflictDetect` + `CONFLICT_DETECT_ENABLED_KEY` (`log.rs` `:~273`), a
`set_conflict_detect_enabled`/`conflict_detect_enabled` pair mirroring the **default-CLOSED**
`capture_enabled` template (`:~6445`/`:~6493`), a `conflict_detect_enabled_or_false` infallible daemon
read for the sweep gate (mirror `evolve_enabled_or_false` `:~1006`), and a `prime_switches` force-off
line (`engine/mod.rs` `:~529`, CLOSED template). Off → the sweep is a single early return; zero model
calls, zero proposals (I3).

### 3.7 De-conflict with the extract path (§8 — minimum)
The evolve/extract path already handles EDGE-level contradictions
(`ProposedRetraction` `extract.rs:~297` → `reconcile_confirmed_contradiction` `log.rs:~7730` →
`write_proposal`). There is **no reverse "memory-claim → invalidated-edge" index**, and building one is
out of scope. **Minimum (owner-approved):** (a) document that rung-3 memory-level detection and the
extract path's edge-level reconciliation are two independent, complementary axes; (b) dedupe within
rung-3 via the proposal-idempotency fold. No edge cross-check in Phase 2.

## 4. Invariants (inherited from parent §12; the ones Phase 2 must uphold)
- **I1 — Never auto-retire.** Detection emits proposals only; no code path mutates a memory/edge. (No
  resolution actions in this phase at all.)
- **I2 — Local & private.** The judge's Reasoner egress (if Cloud is configured) passes the same
  signed-consent pre-gate the evolve loop uses; Local default egresses nothing.
- **I3 — Off by default.** Flag CLOSED → sweep never runs (no scan, no model, no proposals).
- **I4 — Grows with new, not total.** Conflict-cursor-incremental + similarity-gated + background;
  per-cycle ≈ new×k, sublinear, never O(N²), no corpus re-embed in the finder.
- **I5 — Append-only honesty.** Every proposal is a signed event; nothing silently mutated.
- **I6 — Fail-safe.** Reasoner down → no-op + retry; the sweep never panics; a partial cycle is safe to
  resume (cursor only advances past fully-processed items).
- **I7 — Hostile input AND output.** Judge input fenced as untrusted; `why`/`confidence` treated as
  untrusted output — sanitized, coarse band, no verbatim content in the stored proposal.
- **I8 — App-only resolution (forward-looking).** The `conflict_proposal` READ surface may be exposed to
  the App later; the resolve ops (Phase 3) are App-only/guest-refused. Phase 2 adds NO guest-reachable
  op (the sweep is daemon-internal, like capture).
- **I9 — Strict-quiet.** High threshold (`CONFLICT_CONF_MIN = 70`, already tuned); unclear dropped;
  no duplicate open proposals; volume budgets (§5).
- **I-gc — Referential integrity.** Open proposals auto-withdraw/re-target when a referenced memory is
  retired/deleted/edited (folded, mirroring the retire fold + `resolves_proposal` set).

## 5. Volume / flood control (parent §10)
- `CONFLICT_JUDGE_PER_SWEEP = 8` — per-cycle judge-call budget; backlog drips across cycles.
- **Open-proposal ceiling** (start ~20) — on exceed, stop proposing; surface ONE quiet "many conflicts
  pending" count, not N records.
- **First-enable backfill drip** — on first enable the cursor sees the whole corpus as "new"; cap new
  proposals per cycle so day-one is a trickle, not a wall.
- Confirm the existing SP3 guest rate-limit on `Remember` still bounds how fast a client can create
  sweep work.

## 6. Exit gate (Phase 2)
1. **Judge accuracy:** already cleared in P0 (§9 binding gate) — not re-run here; Phase 2 depends on it.
2. **Unified index + recall-neutral:** notes + passages both retrievable via `conflict_search`; recall
   `vector_index` byte-untouched (golden assertion).
3. **Sweep correctness (hermetic-deterministic):** the candidate-finder/HNSW layer is **seeded or
   stubbed** (HNSW top-k is non-deterministic across rebuilds); cursor incrementality proven (a second
   cycle with no new memories does zero judge calls); budgets enforced (≤8 judges/cycle; ceiling caps
   open proposals).
4. **Proposal integrity:** stable typed refs; idempotent per unordered pair; GC withdraws on
   referenced-memory retire/delete/edit; survives restart (fold-derived).
5. **Gate + fail-safe:** flag off → zero side effects (no model call); reasoner-down → no-op + retry;
   the sweep never panics.
6. **Off-by-default preserved:** `prime_switches` forces CLOSED on first open; merging ships the feature
   OFF (nothing detects until the owner enables).

## 7. Testing (parent §13, detection slice)
- Unified index: note + passage retrieval by `conflict_search`; key-space disjointness
  (`encode_note_key` vs `encode_chunk_key` never collide); recall-neutrality golden.
- Candidate-finder: similarity gate; every exclusion (self/lineage/superseded/retired/coexist/dismissed);
  cursor incrementality; **hermetic determinism** (seed/stub the ANN layer, not just the Reasoner).
- Judge wiring: older/newer by ts; `Some`→proposal, `None`→counted, `Err`→no-op; schema-validated
  verdict; taint-fencing of the compared text.
- Proposal: typed stable refs; idempotent per pair; GC on referenced-memory change; restart-survival;
  `why` sanitized + no verbatim content; confidence coarsened.
- Budgets: ≤8 judges/cycle; open-proposal ceiling → one quiet state; first-enable drip; a hostile
  near-duplicate corpus does not blow up judge calls or proposal count.
- Gate/fail-safe: off→no-op (no model call asserted); reasoner-down→no-op; `prime_switches` force-off;
  crash mid-cycle idempotent/resumable.

## 8. Constants (pinned provisionally; harness/owner-tunable)
`CANDIDATE_SIM_MIN` (start conservative-high — cost governor + precision) · `CONFLICT_CONF_MIN = 70`
(existing, tuned in P0) · `MAX_JUDGE_TEXT_BYTES` (inherit SP3 snapshot field cap) ·
`CONFLICT_JUDGE_PER_SWEEP = 8` · open-proposal ceiling ≈ 20 · sweep cadence = piggyback the capture
sweeper (300s).

## 9. Deferred (not Phase 2)
- **Phase 3:** the conflict card/badge + Retire/Keep-both/Dismiss resolution ops + the surfacing decision
  (desktop vs. IN Claude Code). Detection emits the `conflict_proposal` records Phase 3 consumes.
- Cloud judge (stays opt-in + gated + disclosed). Full multi-way (3+) resolution. Auto-retire for
  very-high-confidence pairs (only after the harness earns trust). A true edge-cross-check for §8.
- Conflict-aware recall re-ranking without resolution.

## 10. As-built anchor index (for the plan)
- Sweep: `bossclawd/src/capture/sweeper.rs` `spawn` `:284`, `run_sweep_once` `:186`, `SWEEP_INTERVAL` `:48`,
  `CAPTURE_PER_SWEEP` `:59`, pure `decide_sweep` `:137`.
- Evolve/reasoner: `bossclawd/src/engine/mod.rs` `evolve_once` `:914`, reasoner obtain `:936`, cloud
  pre-gate `:926-934`, `reasoner_provider` `:262`, `prime_switches` `:529`; provider
  `engine/reason.rs` (`ReasonerProvider::reasoner` `:16`, `ConfigReasonerProvider` `:50`,
  `REASONER_MODEL_ID` `:13`).
- Judge: `bossclaw-core/src/conflict.rs` `judge_pair` `:131`, `Verdict` `:30`, `Winner` `:16`,
  `CONFLICT_CONF_MIN` `:123`, `CONFLICT_SYSTEM` `:57`, `conflict_schema` `:43`.
- Reasoner trait: `bossclaw-core/src/reason.rs` `Reasoner::complete_json` `:29`.
- Conflict index / keys: `log.rs` `rebuild_conflict_index` `:5838`, `conflict_search` `:5874`,
  `session_passages_for_model`; `index.rs` `encode_chunk_key`/`decode_chunk_key` `:46/:56`.
- Proposal precedent: `log.rs` `append_write_proposal_with` `:2667`, `build_proposer_event` `:2677`,
  `is_proposal_suppressed` `:2733`, `pending_proposals` `:2780`.
- Event types: `graph.rs` `WRITE_PROPOSAL_EVENT_TYPE` `:94`, retire markers `:40-44`.
- Gate/flags: `log.rs` `ConfigFlag` `:273`, `capture_enabled` `:6445`, `set_capture_enabled` `:6493`,
  evolve cursor `set_evolve_cursor` `:6090`; `mod.rs` `evolve_enabled_or_false` `:1006`.
- Extract-path de-conflict: `extract.rs` `ProposedRetraction` `:297`; `log.rs`
  `reconcile_confirmed_contradiction` `:7730`.

## 11. Open questions for the plan / next review
1. **Cursor vs. pending-pair queue:** simplest correct form for "budget 8/cycle, drip the backlog" —
   a seq cursor that only advances past fully-judged items, or a small persisted pending-pairs set? (Plan
   picks; both must be restart-safe and satisfy I4/I6.)
2. **Note re-embed cost:** `rebuild_conflict_index` re-embedding every current note each rebuild vs.
   persisting note conflict-vectors in a table (like `session_passage_vectors`). Start with re-embed if
   cheap; measure; the plan may add a `note_conflict_vectors` table if rebuild cost is material.
3. **Changed-note re-proposal:** an edited note mints a new event id → the conflict cursor sees it as new
   and the pair may re-propose. Confirm the idempotency key (unordered typed refs) + the material-change
   re-open rule (parent §6) interact correctly (a dismissed pair re-opens on material change by design).
