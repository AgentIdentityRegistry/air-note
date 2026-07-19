# Rung 4 — Reflection (sleep-time consolidation, miss-driven) — Design

**Status:** Rev 3 — Rev 1 owner-approved conversationally 2026-07-19; independently reviewed
(architect SOUND-WITH-CHANGES + critic APPROVE-WITH-CHANGES → Rev 2 folded all findings); re-verified same
day (architect SOUND; critic APPROVE-WITH-CHANGES with ONE new Blocker in Rev 2's own harness-scoring fix +
convergent minors) → Rev 3 folds the re-verification round (changelog §9). Awaiting final reviewer
confirmation + file-level owner review before planning.
**North Star anchor:** `air/memory-strategy-2026-07-03-beat-the-stack` Phase 4 — "M3 reflection,
dossier-centric. Continuous sleep-time consolidation; evolve page dossiers as the primary answer substrate.
(Dreaming-style +15pp is NOT yet neutrally verified — open question; build instrumented.)" Beat-the-stack
criterion #6: continuous reflection (vs GBrain's batch ~11-min `dream`).
**Prior art in-tree:** the evolve loop already performs *awake-time* reflection on NEW memories — entity
extraction, machine links, invalidates, and citation-floored `page` dossiers (`log.rs:8270`
`EventLog::evolve_once`, `log.rs:8111` `summarize_topics`). Evolve already writes dossiers autonomously with
no review surface; Rung 4 adds a new TRIGGER class (quiet-time, miss-driven), not a new writer power. What
nothing does today: work the EXISTING corpus during quiet time, driven by recorded recall failures.

---

## §0 Goal + posture

Give the daemon a **night cleaner**: a fourth background loop that, when the room is quiet, (1) repairs the
specific recall gaps the owner actually hit — consuming the SP3 recall-miss telemetry, which today has no
consumer that *acts on* it (the App's read-only RecallStats panel displays it) — and (2) does bounded corpus
tidying: refreshing dossiers whose cited sources went stale (the direct aftermath of Rung-3 resolutions) and
detecting duplicate entities. It **adds freely, asks to restructure**: dossier revisions and links are
autonomous (append-only; a revision supersedes the prior page IN RECALL while the prior stays recoverable in
the log); entity merges are owner-approved proposals resolved from Claude Code, reusing the Rung-3 surface
pattern. Benefit is measured, not asserted (§5) — and the operational scoreboard is deliberately NOT the
evidence instrument (§5 separates the two, per review finding B1/M3-arch).

A wrong reflection costs a superseded dossier revision (a *good* prior revision can be displaced from recall
until re-superseded — recoverable from the log, same posture evolve has today) or a reversible merge — never
a lost memory.

**Scope honesty (critic M5):** R4 builds the reflection machinery and PROVES IT HARMLESS; whether dossiers
should become the PRIMARY answer substrate remains an open strategy question that R4's harness probe (§5.3)
generates the first real evidence for. R4 does NOT re-rank recall in favor of pages (§6).

## §1 Scope — two sub-projects (each: own reviewed plan → subagent build → PR)

- **R4-A — the sleep loop (additive-only; ships value alone).** `ConfigFlag::Reflect` + sweeper + the
  miss-repair pipeline (§2.2) + the stale-dossier refresh (§2.3, including the shared gather-side
  retired/superseded exclusion it requires) + the durable miss backlog + the scoreboard + the **memharness
  reflection workstream** (§5.3 — net-new harness scope, named as such) + the **enable path** (§2.5).
  Wire-op surface: exactly two additive App-only ops (`SetReflectEnabled` + telemetry surfacing in Status) —
  no guest-reachable ops in R4-A.
- **R4-B — entity merge (structural, owner-gated).** Duplicate detection → pair-keyed idempotent
  `merge_proposal` events → reversible `entity_merged`/`entity_unmerged` fold markers → Code-native
  `list_merge_proposals`/`resolve_merge_proposal` MCP tools mirroring the Rung-3 conflict surface (I8
  precedent, daemon-side sanitize, guest onboarding-override) → **plus a read-only reflection-activity
  listing** (the deferred review surface, §4 I-vis).

R4-A merges before R4-B starts. Both ship dormant.

## §2 Architecture — R4-A, the sleep loop

### §2.1 The tick (sibling #4 of evolve/capture/conflict)

Follows the proven three-sibling shape (`engine/scheduler.rs` pure decide fn + 300s `spawn` +
`MissedTickBehavior::Skip`):

- **Gate (all must hold, else `gated_off`/no-op):** onboarded ∧ `reflect_enabled_or_false()` (new
  fail-closed getter) ∧ **quiet** ∧ reasoner ready (`select_ready`; cloud never silently falls back local).
- **Quiet predicate (idle-window semantics, house pattern — NOT a bespoke watermark).** Quiet = no
  memory-class append (memory + session-capture event types) within the last `REFLECT_QUIET_SECS`
  (provisional 600, the capture sweeper's `QUIET_SECS` precedent). The recency check reads the newest
  relevant event's timestamp/seq **fresh each tick** — there is no state that "latches," so the
  deadlock reading (quiet-since-last-RUN) is explicitly ruled out: one append delays reflection by one quiet
  window, never forever. Implementation note: per-class newest-seq is a table scan today (no `event_type`
  index — acceptable at 300s cadence, same cost class as `dirty_entities_since`; add a partial index only if
  measured to matter).
- **Starvation floor (arch M5 / critic M3):** if the durable backlog (§2.2) holds unrepaired, unparked
  misses AND more than `REFLECT_STALENESS_FLOOR_SECS` (provisional: 6h) have passed since the last completed
  reflect run, run ONE budgeted tick. **Precedence (re-verify convergence, arch residual = critic
  New-Minor-2): the floor overrides BOTH the quiet gate AND the evolve-backlog defer** — a wedged evolve
  queue (e.g. reasoner down, poisoned retry) can never starve reflection indefinitely; a floor-fired tick on
  an incomplete graph just yields more honest `no_material` (bounded harm, no minting). The floor fires at
  most once per interval, tracked by a last-floor-fire timestamp (not per-300s re-fires). Stated honestly:
  a floor tick deliberately runs bounded, consent-gated reasoner work during WAKE time — a small, accepted
  dilution of "sleep-time" in exchange for bounded staleness.
- **Evolve-backlog rule:** when Evolve is ENABLED and its unprocessed queue is non-empty, reflection defers
  (the daytime helper goes first — its extraction feeds the entity graph reflection anchors on), EXCEPT when
  the starvation floor fires (above). When Evolve is DISABLED, reflection still runs against whatever graph
  exists; misses that resolve to no known entity are `no_material` (reflection never does evolve's
  extraction job — no minting, §2.2).
- **Serialization:** dedicated `reflect_lock.try_lock()` → `Busy` (the Rung-3 dedicated-lock lesson).
- **Budget:** small fixed per-tick caps (provisional, harness-tunable, single consts block): misses
  attempted ≤ 4, dossiers refreshed ≤ 4. Priority: misses first, refresh with the remainder.
- **Writes:** only through the single serialized `EventLog::append` / `emit_page` path — not a privileged
  writer. **Consent chokepoint:** `cloud_consent_ok` before reasoner construction (byte-same posture as
  evolve + conflict).

### §2.2 Miss repair (the operational heart — instrumented honestly)

**Input + durable backlog (critic M4).** The SP3 recent-miss ring holds only the 20 newest miss queries
(`RECENT_MISSES_CAP=20`, queries only). Reflection therefore maintains its own re-derivable
`reflect_miss_backlog` table (normalized-query key → first_seen, attempts, state ∈ open|repaired|no_material
|parked), **seeded from the ring every tick** — so ring churn between ticks cannot silently drop a miss that
was once seen. Normalization v1 = trimmed casefold hash (semantic dedup is NOT v1; the bloat risk is bounded
because pages are ENTITY-keyed — near-duplicate queries about the same topic converge on the same dossier).
Losing the table only re-learns misses from the ring; NOT a Tier-A fold input. Scope note (critic m3): a
"miss" is strictly `hits == 0` — R4 targets COVERAGE gaps, not ranking quality (ranking lives in the
harness).

**Per attempted open miss, in order:**

1. **Re-run recall** with the missed query. Hit → state `repaired_by_time` (no reasoner call).
2. **Resolve query → topics (the bridge — arch B1, read-only by design):** `entity_search(query)` over the
   entity resolution index → top-N candidate KNOWN topics (N provisional 2, similarity floor provisional,
   consts block). Reflection **never mints entities** — minting is evolve's job and a write; a miss that
   resolves to no known entity above the floor → state `no_material` (an honest "we never knew this").
   Consequence, stated plainly: R4-A repairs only misses about topics the graph already knows.
3. **Refresh the resolved topics' dossiers additively** through the existing citation-floored machinery
   (`gather_fact_set` → compose → `citation_floor` → `emit_page` atomic supersede), with the §2.3 lineage
   exclusion applied. Emit machine links only on already-resolved entity ids. No new write primitives.
   Reach, stated plainly (arch re-verify observation): because step 3 recomposes the resolved entity's OWN
   lineage (never injecting the miss's candidate material into a prompt as new facts), a true repair occurs
   only where a known topic's dossier was under-composed or stale — a deliberately narrow surface that
   overlaps §2.3. The operational repair rate will be LOW BY DESIGN; §5.3(d) measures the actual reach, with
   a pre-registered success threshold agreed at plan review BEFORE the live dogfood (§5.5).
4. **Replay the original query** against recall. Hit → state `candidate_repaired` — an OPERATIONAL counter,
   not evidence (renamed from Rev 1's `repaired` per B1: a query-derived page ranking for its own query is
   near-tautological; the citation floor guards fabrication, not answer quality — §5.3 carries the evidence
   burden). Still miss → attempts += 1; at `REFLECT_MISS_ATTEMPT_BUDGET` (provisional 3) → `parked`
   (bounded loss — the Rung-3 poison-budget lesson).

**Prompt discipline:** any new reasoner phase copies the `conflict.rs` shape — trusted-frame system const,
fenced+defused untrusted data, structured JSON schema, `ScriptedReasoner`-testable pure builders. Model
output is data, never authority. Threat-model carry-forward (critic M2): reflection composes from whatever
material exists, INCLUDING booby-trapped ingested files — the same Rung-3 residual risk; guards are the
citation floor (provenance), taint propagation on sources, I7 output discipline, reversibility, and the §4
I-vis visibility commitments.

### §2.3 Tidy job (v1 = exactly one autonomous job) + the shared lineage exclusion

**Stale-dossier refresh:** for current `page` events, detect pages whose cited `source_event_ids` intersect
`SessionFold.retired_notes ∪ superseded` — precisely what a Rung-3 `resolve_conflict` retire produces.

**Load-bearing prerequisite (arch M2 — Rev 1's silent defect):** the gather path
(`gather_fact_set`/`fact_texts_for_ids`) does NOT currently exclude retired/superseded lineage — so a
refresh would re-gather the identical (stale-inclusive) set, the cited-set-diff idempotency guard
(`log.rs:8162`) would skip the emit, and the job would detect rot nightly while never healing it. R4-A
therefore adds **retired/superseded exclusion INSIDE the shared gather path** (one source, consumed by both
evolve's summarize and reflection's refresh — the I9 single-source lesson; a refresh-only variant would let
the two writers' cited sets diverge and fight). This changes evolve's own dossier output for
stale-lineage topics (a healing, not a regression — dossiers stop citing retired memories); the §5.3 harness
gate guards the blast radius.

**Thin-set residual (arch re-verify, accepted + surfaced):** a topic whose lineage is ENTIRELY
retired/superseded and which has no current edges falls below `PAGE_MIN_FACTS` (`summarize.rs:26`; the
summarize path skips thin emits at `log.rs:8131`) — so its stale page cannot legally be re-emitted, and
reflection never retires pages (I1). "The next night heals it" is therefore NOT universal: such pages stay
stale-but-provenance-true until new current facts arrive. Surfaced as a distinct scoreboard outcome
(`unhealable_thin`) rather than silently retried; the per-tick budget stops it wasting nights. Plan-level
correctness note: the exclusion must shrink BOTH the gathered memory texts AND the cited sets — the
per-claim CITES UNION that is the emit-idempotency key (`current_page_for_topic`, `log.rs:8051-8064`,
compared at `log.rs:8162`) and the DISTINCT page-level D8 `source_event_ids` provenance anchor — excluding
only the texts would leave both unchanged and the heal would still no-op.

**Writer-coordination / stability note (critic gap):** evolve's summarize and reflection's refresh converge
by construction — both route through the same gather (same exclusion) and the same set-diff idempotent
`emit_page`; for a fixed corpus state they compute the same cited set, so alternation cannot oscillate.
Churn occurs only when the corpus actually changes between runs; a later evolve supersede of a
reflection-refreshed page is ACCEPTED, measured churn (it shows in the scoreboard as a re-miss/re-repair
cycle), not a regression.

**Page growth bound:** pages are entity-keyed (one current page per topic, superseded in place) and
reflection mints no entities — so reflection cannot grow the page population at all; it only revises
existing topics' pages. No new ceiling needed in R4-A (R4-B's proposals carry the conflict-style open-count
cap instead).

### §2.4 Scoreboard (operational telemetry — NOT the evidence instrument)

`ReflectReport` per tick: `attempted / candidate_repaired / repaired_by_time / no_material / parked /
dossiers_refreshed / unhealable_thin / merge_proposed (0 until R4-B) / gated_off / reasoner_errors` +
cumulative counters in the telemetry family. Snapshot digest line (Rung-3 never-truncated preamble,
integer-only), in deliberately NEUTRAL copy (critic New-Minor-1 — the digest must not present an operational
counter as proven benefit): default-include "`N` dossiers refreshed for recently-missed topics, `M`
unknown-topic gaps since last session" — `M` (= no_material) is the most actionable output for the owner
("your memory never knew this; consider telling it"), per critic's open question. Disclosure copy for the miss store updates in the same PR (critic m4): it is no longer a passive
read-only signal; it actively drives reflection work (and, with cloud consent ON, gathered material may
egress under the existing consent).

### §2.5 Enable path (arch m8 — without this, §5.4 is impossible)

None of the dormant sibling flags has a user-facing enable path today (conflict-detect included). R4-A adds
the minimal one, inside the desktop's sanctioned settings/consent role (NOT a reflection UI): an App-only
`SetReflectEnabled` wire op (the `SetCaptureEnabled` pattern, `server.rs:465`-family) + a single toggle in
the desktop settings panel. Guest role cannot reach it. This also establishes the enable-path pattern the
Rung-3 dogfood needs.

## §3 Architecture — R4-B, entity merge (summary; own spec-level detail at its plan)

- **Detection (in the reflect tick, budgeted):** conservative duplicate candidates over the folded entity
  set — normalized-name equality / alias overlap / high name-embedding similarity (thresholds provisional,
  consts block, harness-tunable).
- **Proposal:** signed `merge_proposal` — pair-keyed (`unordered_pair_key` idiom), idempotent,
  open-count-capped, stop-nagging (dismissed pairs excluded via ONE exclusions reader feeding both proposer
  and listing — I9).
- **Resolution (owner, from Code):** `list_merge_proposals` / `resolve_merge_proposal{approve|dismiss}` MCP
  tools → proto ops under the established I8 relaxation posture; guest onboarding-override; daemon-side
  sanitize; NOT rate-limited (Rung-3 §0 rationale); direct structural ops stay App-only. Plus the read-only
  reflection-activity listing (§4 I-vis).
- **Apply (reversible, append-only):** `entity_merged{winner, loser}` marker; fold projects loser's aliases
  + current edges onto winner and drops loser from listings; `entity_unmerged` restores. **Named blast
  radius (arch m6):** the fold projection, `rebuild_entity_index` (the loser's resolution vector must not
  keep winning `resolve_mention` — else mentions re-resolve to the merged-away id and diverge), the recall
  graph boost (follows `rebuild_graph`), and summarize dirty-topics (winner goes dirty → §2.3 refreshes its
  dossier next night). Positive finding carried from review: `ConflictRef` has NO Entity variant — merge
  does not perturb conflict pair keys.

## §4 Invariants (house numbering, extended)

| Inv | Statement | Where upheld |
| --- | --- | --- |
| I1 never-destroy | Reflection writes are append-only dossier REVISIONS (supersede = replace-in-recall; prior revision recoverable from the log — a good revision can be displaced until re-superseded, accepted and inherited from evolve's existing posture) + machine links; merge is marker-based and reversible; originals never mutated or deleted; reflection NEVER mints entities or retires anything. | §2.2/§2.3/§3 |
| I2 no silent egress | Reasoner phases behind `cloud_consent_ok`; local default; cloud fail-closed. Miss QUERIES drive only local search; gathered MATERIAL reaches a reasoner only inside the consent envelope. | §2.1/§2.2 |
| I3 dormant (scoped honestly) | `ConfigFlag::Reflect` default-closed + `prime_switches` force-off; the REFLECTION loop does nothing until the explicit App-only enable (§2.5). ONE deliberate Reflect-independent change ships with R4-A (critic New-Major-1): the shared gather-path exclusion (§2.3) means an EVOLVE-enabled brain with retired/superseded lineage emits healed dossier revisions on its next evolve tick even with Reflect OFF — a bounded, gate-guarded healing (dossiers stop citing retired memories), named here rather than hidden under "dormant". Fresh-brain trip-wire moves **5 → 6** in ALL THREE sites (`bossclawd/tests/roundtrip.rs:173`, `engine/mod.rs:2237`, desktop `engine/client.rs:973`) as one conscious, documented act. | §2.1/§2.3/§2.5 |
| I5 append-only | All durable state via signed events; `reflect_miss_backlog` is re-derivable progress state, not history. | §2.2 |
| I6 fail-safe | Per-miss attempt budget → parked; per-tick caps; starvation floor bounded to one tick per interval; `Busy` on overlap; torn ticks re-try idempotently (set-diff emit). | §2.1/§2.2 |
| I7 hostile-output | Dossier content citation-floored (subtract-only); no raw model text logged; merge/activity listings sanitized daemon-side; digest lines integer-only. | §2.2/§2.4/§3 |
| I8 (relaxed, scoped) | R4-B merge resolution reachable from `MemoryClient` per the Rung-3 owner decision + compensating controls. R4-A adds NO guest-reachable ops; `SetReflectEnabled` is App-only. | §2.5/§3 |
| I9 stop-nagging | Merge proposals pair-keyed + single-source exclusions; parked misses stop being attempted; backlog dedup by normalized key. | §2.2/§3 |
| I-vis visibility | Autonomous-writer visibility is staged, honestly: R4-A = scoreboard + digest counts + the signed log (accepted: no per-item review surface yet, matching evolve's existing dossier posture — reflection adds a trigger, not a power); R4-B ships the read-only reflection-activity listing alongside the merge tools. | §2.4/§3 |

## §5 Instrumentation + exit gates ("build instrumented" is a deliverable)

Two instruments with different jobs — the scoreboard OPERATES, the harness EVIDENCES:

1. **Operational scoreboard (§2.4).** `candidate_repaired` et al. are explicitly labeled operational (the
   build-then-replay loop is self-confirming; it verifies the mechanism fired, not that memory improved).
2. **Harness non-regression gate (SHIP/NO-SHIP).** On the frozen corpus: reflected brain (loop driven to
   quiescence) vs unreflected baseline, paired per-case; `recall_regressed` must flag NO segment.
3. **Harness reflection workstream (net-new memharness scope — critic B2; a named R4-A work item, not "the
   existing mechanism"):**
   (a) a run-to-quiescence reflection driver over the frozen corpus + a frozen synthetic miss set;
   (b) a PAGE ARM: `PageResolver` today is fail-loud on any non-file hit (the Phase-0 no-evolve invariant,
   `arms.rs:76,106`) — extend it so reflected-brain runs do not abort on dossier hits. **Gate scoring rule
   (critic re-verify New-Blocker-1 — Rev 2's union-credit rule is REJECTED):** in the §5.2 SHIP gate, the
   gold page scores ONLY as itself; a dossier hit NEVER substitutes for the gold it cites. A dossier that
   crowds the gold page out of top-k therefore registers as the regression it is — the exact harm the gate
   exists to catch — and the gate stays free of the unproven dossier-primacy assumption (which belongs to
   (e), not the gate). Union-style "dossier covers gold at rank r" is computed as a SEPARATE, REPORTED
   coverage metric only, never gated (this also keeps the harness's single-page-id hit/dedup/rank model
   intact — `arms.rs:13,19,24`);
   (c) the reflected pass RUNS EVOLVE TOO (quiescence = both loops drained) AND SEEDS retired/superseded
   lineage into the frozen corpus (a scripted Rung-3 `resolve_conflict` retire) — the fresh-ingested Phase-0
   corpus contains no retirements, so without seeding the §2.3 gather-exclusion path would never execute
   under the gate (critic New-Major-1);
   (d) **held-out generalization probe (B1 fix):** reflect on miss set A, then measure success@k on a
   DISJOINT paraphrase/query set B over the same topics — repair must generalize past the verbatim query;
   (e) **dossier-vs-source answer A/B (B1/M5 fix — the primacy evidence-generator):** blind position-swapped
   judging of answers composed from the dossier page vs from its raw cited memories, on the open-case set.
   The judge must clear the Phase-0 trust contract (agreement ≥85% / κ ≥0.6 vs the audit ladder) or (e)'s
   lift numbers are reported as UNINTERPRETABLE rather than as evidence.
   Outputs (d)+(e) are REPORTED, not SHIP-gated in R4-A (the SHIP bar is non-regression; the lift data
   informs the future dossier-primacy decision honestly — including the future question of whether a
   dossier judged equal-or-better by (e) may legitimately substitute for its gold, which stays OUT of the
   R4-A gate by construction).
4. **Dormancy proof:** trip-wire `==5 → ==6` updated in all THREE sites in the same task that adds the flag.
5. **Live evidence (Peter-gated, post-merge, via §2.5):** enable Reflect on the real brain; the field
   metrics are the miss-counter trend + the digest counts over ≥1 week of nights. Field churn (evolve
   re-superseding reflected pages) is measured, not hidden (§2.3). Stated plainly (critic re-verify): this
   first live run happens BEFORE R4-B's read-only review surface exists — accepted because it is the
   owner's own brain, enable is App-only and owner-gated, and the signed log + Library remain the
   inspection backstops (I-vis). The §5.3(d) pre-registered success threshold is agreed at plan review
   before this run, so the week's verdict is read against a bar set in advance.

## §6 Boundary — explicitly OUT of Rung 4

- Recall re-ranking in favor of pages ("dossier-primacy") — future, decided on §5.3(e) evidence.
- Wiring bi-temporal `as_of` into recall (test-only today) — its own arc.
- Decay/archive tiers; orphan adoption; `Wide` dossier reach; semantic miss dedup.
- Desktop UI beyond the single settings toggle (§2.5); Codex parity; cross-machine sync.
- Entity minting from reflection; any autonomous structural mutation.
- Parsing rotated `recall.jsonl` history (backlog seeds from the live ring only, v1).

## §7 Open questions (defaults chosen; revisit at plan review)

1. **Quiet event classes** — default: memory-class + session-capture appends reset the idle window.
2. **entity_search floor + N** — question→entity-label similarity is unproven territory (arch B1 option-A
   cost, accepted): start conservative (high floor, N=2); tune via §5.3(d); a floor too high just yields
   more honest `no_material`.
3. **Backlog hygiene** — `repaired_by_time`/`candidate_repaired` rows age out after a fixed horizon
   (provisional 30d) to keep the table bounded; `parked`/`no_material` persist (they carry information).
4. **R4-B thresholds** — conservative-first consts, one documented block (the
   `CONFLICT_PAIR_ERROR_BUDGET` pattern).

## §8 Key seam anchors (verified against main `7fb1e8a`, 2026-07-19; re-verified by independent review)

`evolve_once` `log.rs:8270` · `summarize_topics` `log.rs:8111` · `dirty_entities_since` `log.rs:7907` ·
`gather_fact_set` `log.rs:8076` (entity-anchored) · `fact_texts_for_ids` `log.rs:7945` (pages-only filter
today — §2.3 adds the exclusion) · `emit_page` `log.rs:2772` · page idempotency `log.rs:8162` ·
`entity_search`/`resolve_mention` `log.rs:6988/:6979` (resolve MINTS — reflection uses entity_search only) ·
recall excludes entity-kind `log.rs:972` · scheduler `engine/scheduler.rs:53,70,90` · capture sweeper
`capture/sweeper.rs:48-59` (`QUIET_SECS=600` precedent) · conflict sweeper `conflict/sweeper.rs:40,68` ·
`prime_switches` `engine/mod.rs:564` (force-off precedents `:584,:591`) · `cloud_consent_ok`
`engine/mod.rs:1692` · miss telemetry `bossclawd/src/telemetry.rs` (`RECENT_MISSES_CAP=20`; `is_miss =
hits==0`; RecallStats is the existing read-only consumer) · recall + per-kind retain `log.rs:1790,1981-2024`
· `SessionFold` sets `log.rs:9519+` · entities mint-once `graph.rs:283,308` · `rebuild_entity_index`
`log.rs:6425` (R4-B blast radius) · `ConflictRef` no-Entity `index.rs:89-96` · prompt pattern
`conflict.rs:57,95,105` · `ScriptedReasoner` `reason.rs:56` · trip-wire sites ×3 `roundtrip.rs:173` +
`engine/mod.rs:2237` + desktop `client.rs:973` · `PageResolver` fail-loud `memharness/src/arms.rs:76,106` ·
`recall_regressed` `memharness/src/compare.rs:136` · `SetCaptureEnabled` op family `server.rs:465`. Line
anchors drift — re-grep at plan/build time.

## §9 Rev 2 changelog (findings → resolutions)

- Arch B1 + critic M1 (query→topic bridge unspecified): §2.2 step 2 — read-only `entity_search`, no minting,
  entity-keyed pages, unknown topic → `no_material`. Page growth thereby bounded (no ceiling needed).
- Arch M2 (refresh never heals): §2.3 — retired/superseded exclusion added INSIDE the shared gather path.
- Arch M3 + critic B1 (repaired tautology): renamed `candidate_repaired`, labeled operational; evidence
  moved to §5.3(d) held-out probe + (e) dossier-vs-source A/B.
- Critic B2 (gate cannot run): §5.3 harness workstream named as net-new R4-A scope — quiescence driver, page
  arm with cited-union scoring, evolve runs in the reflected pass.
- Arch M4 (third trip-wire): all three `==5` sites listed (I3, §5.4, §8).
- Arch M5 + critic M3 (quiet ambiguity/starvation): §2.1 — idle-window semantics (no latch, deadlock reading
  ruled out), `QUIET_SECS` house pattern, starvation floor, evolve-backlog defer rule.
- Critic M4 (ring churn): §2.2 durable `reflect_miss_backlog` seeded from the ring.
- Critic M2 (visibility): §4 I-vis — staged visibility, accepted-and-justified for R4-A (evolve precedent),
  read-only listing committed in R4-B; threat-model carry-forward noted in §2.2.
- Critic M5 (goal fit): §0 scope-honesty paragraph + §5.3(e) as the primacy evidence-generator.
- Arch m6 (R4-B blast radius): §3 names `rebuild_entity_index` + graph boost + dirty-topics; ConflictRef
  no-Entity positive carried.
- Arch m7 (watermark scan cost): noted in §2.1.
- Arch m8 (no enable path): §2.5 App-only `SetReflectEnabled` + settings toggle, named R4-A scope.
- Critic m1 (false "nothing reads it"): reworded (§0). m2 (I1 wording): tightened (§0, §4). m3 (miss =
  coverage): scoped (§2.2). m4 (disclosure copy): §2.4 task. m5 (normalization bloat): bounded by
  entity-keyed pages (§2.2); semantic dedup stays out (§6).
- Critic gaps (stability, contention-in-gate, ceiling, floor, backlog, positive-lift): §2.3 convergence
  note, §5.3(c), §2.3 growth bound, §2.1 floor, §2.2 backlog, §5.3(d,e).
- Critic open Qs: no_material surfaced in the digest (§2.4); evolve-supersede churn = accepted + measured
  (§2.3); harness runs evolve (§5.3(c)).

**Rev 3 (re-verification round — architect SOUND with residuals; critic APPROVE-WITH-CHANGES, 1 new Blocker
in Rev 2's own fix; convergent floor/defer minor):**
- Critic New-Blocker-1 (union-credit defeats the gate + presupposes primacy + breaks the single-page-id hit
  model): §5.3(b) — gate scores gold ONLY as itself; dossier never substitutes; crowding-out = regression by
  design; union-coverage demoted to a separate reported metric.
- Critic New-Major-1 (gather exclusion is Reflect-independent + untested on a fresh corpus): I3 reworded to
  name the evolve-visible healing honestly; §5.3(c) seeds retired/superseded lineage into the reflected pass
  so the exclusion path executes under the gate.
- Arch residual 1 (thin-set unhealable): §2.3 residual paragraph + `unhealable_thin` scoreboard outcome +
  the texts-AND-cited-ids exclusion correctness note.
- Arch residual 3 = critic New-Minor-2 (floor vs defer precedence, CONVERGED): §2.1 — floor overrides both
  gates; wedged evolve queue cannot starve reflection; last-floor-fire timestamp; honest wake-time-work
  sentence.
- Critic New-Minor-1 (digest copy leaked benefit framing): §2.4 neutral copy ("dossiers refreshed for
  recently-missed topics").
- Arch residual 2 (narrow repair surface): §2.2 step 3 reach paragraph + pre-registered (d) threshold
  before the dogfood; §5.5 names the review-surface-absent first live run; §5.3(e) gains the Phase-0
  judge-trust contract (≥85% / κ≥0.6, else lift reported uninterpretable).

**Rev 4 (whole-branch dual review of R4-A — both reviewers SHIP; 2 conscious-acceptance Info residuals,
DOC-ONLY, no behavior change):**
- Concurrent page-writer churn — a floor-fired reflect tick can race an evolve summarize on the same
  topic; both may supersede the same prior page, leaving one recoverable orphan page event. Benign by
  construction (`fold_pages` last-write-wins keyed by topic + `rebuild_graph`'s atomic DELETE+INSERT →
  always exactly one current page per topic); it is design §2.3's accepted churn reached via concurrency.
  Surfaced by the whole-branch security review.
- Floor cannot bootstrap an empty backlog on a continuously-active brain — the starvation floor arms only
  on a non-empty durable backlog, which is seeded only inside a Run tick; a brain that is never quiet
  (memory-class append every <600s) AND evolve-backlogged never seeds it, so misses live only in the ≤20
  SP3 ring until the first quiet gap (earliest lost if >20 churn first). Design-faithful (§2.1/I5:
  best-effort, non-empty-backlog-predicated); practically bounded (sleep-time loop). Minimal-close if
  dogfood shows it matters: seed the backlog from the ring in `reflect_gate_inputs` unconditionally per
  sweep. Surfaced by the whole-branch integration review; decide at dogfood.
