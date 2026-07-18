# Rung 4 — Reflection (sleep-time consolidation, miss-driven) — Design

**Status:** Rev 1 — Peter-approved conversationally 2026-07-19 (driver, powers, architecture, decomposition);
awaiting file-level owner review + independent design review before planning.
**North Star anchor:** `air/memory-strategy-2026-07-03-beat-the-stack` Phase 4 — "M3 reflection,
dossier-centric. Continuous sleep-time consolidation; evolve page dossiers as the primary answer substrate.
(Dreaming-style +15pp is NOT yet neutrally verified — open question; build instrumented.)" Beat-the-stack
criterion #6: continuous reflection (vs GBrain's batch ~11-min `dream`).
**Prior art in-tree:** the evolve loop already performs *awake-time* reflection on NEW memories — entity
extraction, machine links, invalidates, and citation-floored `page` dossiers (`log.rs:8270`
`EventLog::evolve_once`, `log.rs:8111` `summarize_topics`). Rung 4 adds what nothing does today: working the
EXISTING corpus during quiet time, driven by recorded recall failures.

---

## §0 Goal + posture

Give the daemon a **night cleaner**: a fourth background loop that, when the room is quiet, (1) repairs the
specific gaps the owner actually hit — the SP3 recall-miss telemetry that today nothing reads — and (2) does
bounded corpus tidying: refreshing dossiers whose cited sources went stale (the direct aftermath of Rung-3
resolutions) and detecting duplicate entities. It **adds freely, asks to restructure**: dossier writes and
links are autonomous (additive, originals untouched); entity merges are owner-approved proposals resolved
from Claude Code, reusing the Rung-3 surface pattern. Every claim of benefit is measured, not asserted
(§5) — the strategy's "build instrumented" requirement is a first-class deliverable, because the +15pp
reflection literature is unverified.

A wrong reflection costs a superseded dossier revision or a reversible merge — never a lost memory.

## §1 Scope — two sub-projects (each: own reviewed plan → subagent build → PR)

- **R4-A — the sleep loop (additive-only; ships value alone).** `ConfigFlag::Reflect` + sweeper + the
  miss-repair pipeline + the stale-dossier refresh job + the scoreboard + the harness non-regression gate.
  NO new proposal types, NO merge machinery, NO new wire ops beyond telemetry surfacing.
- **R4-B — entity merge (structural, owner-gated).** Duplicate detection → pair-keyed idempotent
  `merge_proposal` events → reversible `entity_merged`/`entity_unmerged` fold markers → Code-native
  `list_merge_proposals`/`resolve_merge_proposal` MCP tools mirroring the Rung-3 conflict surface (I8
  precedent, daemon-side sanitize, guest onboarding-override).

R4-A merges before R4-B starts. Both ship dormant.

## §2 Architecture — R4-A, the sleep loop

### §2.1 The tick (sibling #4 of evolve/capture/conflict)

Follows the proven three-sibling shape exactly (`crates/bossclawd/src/engine/scheduler.rs` pure
`decide_tick` + 300s `spawn` + `MissedTickBehavior::Skip`; `capture/sweeper.rs`; `conflict/sweeper.rs`):

- **Gate (all must hold, else `gated_off`/no-op):** onboarded ∧ `reflect_enabled_or_false()` (new
  fail-closed getter) ∧ **quiet** ∧ reasoner ready (`select_ready`, cloud never silently falls back local).
- **Quiet predicate (the "sleep-time" semantic):** no new memory-class appends since the previous reflect
  tick — a max-seq watermark over capture/memory events, NOT a coupling to the Evolve flag (reflection must
  be usable with evolve off; and when evolve is on, an idle evolve queue and an unchanged watermark
  coincide). Exact event-class set is a plan-time detail; the watermark mechanism is the design.
- **Serialization:** dedicated `reflect_lock.try_lock()` → `Busy` on overlap (the `evolve_lock` idiom;
  Rung-3's dedicated-lock lesson — never share a lock with a loop that can hold it for minutes).
- **Budget:** small fixed per-tick work caps (house style, `CAPTURE_PER_SWEEP=8`-class consts, provisional
  and harness-tunable). Priority: sticky notes first, tidying with the remainder.
- **Writes:** only through the single serialized `EventLog::append` / `emit_page` path. The loop is not a
  privileged writer.
- **Consent chokepoint:** `cloud_consent_ok` (`engine/mod.rs:1692`) before reasoner construction — byte-same
  posture as evolve + conflict sweeps.

### §2.2 Miss repair (the measurable heart)

Input: the SP3 recall-miss telemetry (`crates/bossclawd/src/telemetry.rs` — recent-miss ring, queries only,
`RECENT_MISSES_CAP=20`, plus durable counters). Today it has zero consumers. Per attempted miss, in order:

1. **Re-run recall** with the missed query. Hit now → outcome `repaired_by_time` (new memories arrived since;
   no work, no reasoner call).
2. **Gather material:** search raw substrates (memory notes, session passages — the same substrates recall
   fuses) for candidates relevant to the query. Nothing relevant → outcome `no_material` (an honest "we never
   knew this"; the gap is the owner's to fill, not the cleaner's to hallucinate).
3. **Consolidate additively:** route the material through the EXISTING citation-floored dossier machinery
   (`gather_fact_set` → compose → `citation_floor` → `emit_page` atomic supersede; `summarize.rs`), and/or
   emit machine links on resolved entities — the same event types evolve emits. No new write primitives.
4. **Replay the miss** — run the original query against recall again. Hit → `repaired`; still miss →
   attempt recorded, and after `REFLECT_MISS_ATTEMPT_BUDGET` (provisional: 3) cumulative attempts →
   `parked` (stops consuming nights — the Rung-3 poison-budget lesson: bounded loss, never a frozen loop,
   never a hidden sibling work item).

State: a small re-derivable `reflect_miss_attempts` table (normalized-query key → attempts, last outcome).
Losing it only re-tries a miss. NOT a Tier-A fold input.

**Prompt discipline:** any new reasoner phase copies the `conflict.rs` shape — trusted-frame system const,
fenced untrusted data (`build_*_prompt` with defused embedded fences), structured JSON schema,
`ScriptedReasoner`-testable pure builders. Model output is data, never authority; the citation floor is the
gate that makes dossier content trustworthy, exactly as in evolve today.

### §2.3 Tidy job (v1 = exactly one autonomous job)

**Stale-dossier refresh:** for current `page` events, detect pages whose cited `source_event_ids` are no
longer current (retired or superseded per `SessionFold` — precisely what a Rung-3 `resolve_conflict` retire
produces). Budgeted per tick; refresh through the existing summarize path (idempotency preserved: emit only
when the cited-source set changes, `log.rs:8162`). This closes the loop Rung 3 opened: resolving a conflict
retires a source → the dossier citing it is quietly wrong → the next night heals it.

Duplicate-entity detection is R4-B (it produces proposals, not autonomous writes). No other tidy jobs in v1
— decay/archive, orphan adoption, Wide reach are all out (§6).

## §3 Architecture — R4-B, entity merge (summary; own spec-level detail at its plan)

- **Detection (in the reflect tick, budgeted):** conservative duplicate candidates over the folded entity
  set — normalized-name equality / alias overlap / high name-embedding similarity (thresholds provisional,
  harness-tunable). Entities are mint-once with `aliases` and NO merge primitive today (`graph.rs:283,308`)
  — that primitive is R4-B's core deliverable.
- **Proposal:** signed `merge_proposal` event — pair-keyed (unordered, the `unordered_pair_key` idiom),
  idempotent, open-count-capped, stop-nagging (a dismissed pair is not re-proposed while both sides'
  fold-relevant state is unchanged — the I9 single-source lesson: ONE exclusions reader feeds both the
  proposer and the listing).
- **Resolution (owner, from Code):** `list_merge_proposals` / `resolve_merge_proposal{approve|dismiss}` MCP
  tools → proto ops granted to `MemoryClient` under the established I8 relaxation posture; guest
  onboarding-override; daemon-side sanitize on the listing; NOT rate-limited (same §0 rationale as Rung 3);
  direct structural ops stay App-only.
- **Apply (reversible, append-only):** `entity_merged{winner, loser}` marker; the fold projects the loser's
  aliases and current edges onto the winner and drops the loser from entity listings; `entity_unmerged`
  restores (the unretire philosophy). Winner's topic page goes dirty → the refresh job (§2.3) regenerates
  its dossier next night. No event rewriting, no index surgery outside the normal rebuild paths.

## §4 Invariants (house numbering, extended)

| Inv | Statement | Where upheld |
| --- | --- | --- |
| I1 never-destroy | Reflection is additive (dossier supersede-revisions + links); merge is marker-based and reversible; originals never mutated or deleted. | §2.2/§2.3 write paths; §3 markers |
| I2 no silent egress | Reasoner phases sit behind `cloud_consent_ok`; local default; cloud fail-closed. | §2.1 chokepoint |
| I3 dormant | `ConfigFlag::Reflect` default-closed + `prime_switches` boot force-off; merging R4-A changes nothing at runtime. The fresh-brain config-event trip-wire moves **5 → 6** in BOTH sites (`bossclawd/tests/roundtrip.rs:173`, desktop `engine/client.rs:973`) as a conscious, documented act — the trip-wire firing loudly is it working. | §2.1 gate; plan task |
| I5 append-only | All state via signed events; `reflect_miss_attempts` is re-derivable progress state, not history. | §2.2 |
| I6 fail-safe | Per-miss attempt budget → `parked` (bounded loss); per-tick work caps; `Busy` on lock overlap; a torn tick re-tries idempotently (dossier emit is set-diff idempotent). | §2.2/§2.1 |
| I7 hostile-output | Dossier content is citation-floored (subtract-only) — model text never becomes uncited "fact"; merge listings sanitized daemon-side; no raw model text logged (daemons persist stderr). | §2.2 prompt discipline; §3 |
| I8 (relaxed, scoped) | Merge resolution reachable from `MemoryClient` per the Rung-3 owner decision; compensating controls: reversibility + signed log + visibility. Direct structural ops stay App-only. | §3 |
| I9 stop-nagging | Merge proposals pair-keyed + exclusion-fed from one reader; parked misses stop being attempted. | §3; §2.2 |

## §5 Instrumentation + exit gates ("build instrumented" is a deliverable)

1. **Scoreboard:** `ReflectReport` per tick (attempted / repaired / repaired_by_time / no_material / parked /
   dossiers_refreshed / merge_proposed / gated_off / reasoner_errors) + cumulative counters in the telemetry
   file family. The replay-the-miss check (§2.2 step 4) makes "repaired" a verified outcome, not a claim.
   Plus one integer digest line in the session-start snapshot preamble, Rung-3 style ("N memory gaps
   repaired since last session") — never-truncated placement + integer-only discipline already exist
   (default per §7.3; drop only if plan review finds a concrete reason).
2. **Harness non-regression gate (SHIP/NO-SHIP):** on the frozen corpus, a reflected brain (loop run to
   quiescence) must not regress recall on ANY segment vs the unreflected baseline — the existing paired
   Wilcoxon `recall_regressed` mechanism (`memharness/src/compare.rs:136`). Reflection artifacts (extra
   pages) must earn their rank, not crowd out ground truth.
3. **Dormancy proof:** trip-wire `==5 → ==6` updated in both sites within the same task that adds the flag,
   with the design-doc citation in the diff; everything else byte-identical at boot.
4. **Live evidence (Peter-gated, post-merge):** enable Reflect on the real brain, watch the scoreboard for a
   week of nights; the recall-miss counters' trend is the honest field metric. (The strategy's open question
   — do reflection gains replicate? — gets its first real data here.)

## §6 Boundary — explicitly OUT of Rung 4

- Wiring bi-temporal `as_of` into recall (`log.rs:6221`, currently test-only) — the strategy's Phase-3
  leftover; deserves its own arc with its own measurement.
- Decay / archive tiers (SP3-deferred remains deferred). Orphan-memory adoption. `Wide` dossier reach
  (`summarize.rs:13,18` stays deferred).
- Desktop UI (background-first: Code is the surface; desktop = settings/consent only — no reflection panel).
- Codex parity; cross-machine sync; any change to recall ranking in favor of pages ("dossier-primacy"
  re-ranking is future, evidence-gated by §5.2/§5.4 data).
- Any autonomous structural mutation (merge without approval, retire-into-dossier consolidation).

## §7 Open questions (defaults chosen; revisit at plan review)

1. **Quiet-predicate event classes** — which append types reset the watermark (memory only, or memory +
   capture)? Default: memory-class + session-capture appends (both mean "the room is active").
2. **Miss normalization** — how a missed query keys `reflect_miss_attempts` (case/whitespace fold minimum;
   semantic dedup is NOT v1). Default: trimmed casefold hash.
3. **Digest line** — include the repaired-count line in the snapshot preamble in R4-A or defer? Default:
   include (one integer line; the never-truncated preamble + integer-only discipline already exist).
4. **R4-B thresholds** — duplicate-candidate similarity floors; conservative-first, harness-tunable consts
   in one place (the `CONFLICT_PAIR_ERROR_BUDGET` documentation pattern).

## §8 Key seam anchors (verified against main `7fb1e8a`, 2026-07-19)

`evolve_once` `log.rs:8270` · `summarize_topics` `log.rs:8111` · `gather_fact_set` `log.rs:8076` ·
`emit_page` `log.rs:2772` · page idempotency `log.rs:8162` · scheduler pattern `engine/scheduler.rs:53,90` ·
capture sweeper `capture/sweeper.rs:48-59` · conflict sweeper `conflict/sweeper.rs:40,68` · `prime_switches`
`engine/mod.rs:564` (5 boot events at `:2233-2236`) · `cloud_consent_ok` `engine/mod.rs:1692` ·
`select_ready` `scheduler.rs:70` · miss telemetry `bossclawd/src/telemetry.rs` (`RECENT_MISSES_CAP=20`) ·
recall surfaces + per-kind retain `log.rs:1790,1981-2024` · `SessionFold` `log.rs:9439+` · entities
mint-once/no-merge `graph.rs:283,308` · prompt pattern `conflict.rs:57,105,131` · `ScriptedReasoner`
`reason.rs:56` · trip-wire sites `bossclawd/tests/roundtrip.rs:173` + desktop `engine/client.rs:973` ·
`recall_regressed` `memharness/src/compare.rs:136`. Line anchors drift — re-grep at plan/build time.
