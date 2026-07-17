# Rung 3 — Phase 3: Conflict **Resolution** (Code-native) — Design (2026-07-17)

**Status:** Design **Rev 2** (2026-07-17). Rev 1 approved-in-principle by owner, then sent through THREE
independent Opus reviews (architect / critic / security) which returned **REVISE** with convergent,
source-verified findings. Rev 2 folds every finding. Consumes the `conflict_proposal` records that Rung 3
Phase 2 (conflict **detection**) emits (merged to `main` `2cf0ccb`, PR #81). Completes rung 3
("Notice & Reconcile") of the North Star `air/memory-strategy-2026-07-03-beat-the-stack`.

## Rev 2 — review findings folded (what changed from Rev 1)
Rev 1 over-claimed in three load-bearing places; all three reviewers independently caught the first two.
Everything below is now grounded in re-verified source.

- **[BLOCKER ×3] The "rate budget" bounded nothing.** `CaptureRateLimiter` is per-connection
  (`server.rs:161`), and the MCP adapter reconnects per call (`air-memory-mcp/src/daemon.rs:3`), so every
  resolve got a fresh empty bucket — the limiter's own doc-comment (`server.rs:63-67`) says it does NOT
  bound reconnect-spam. **Owner decision (Honest-minimal, 2026-07-17):** DROP the rate-budget claim
  entirely. Safety rests on **reversibility (Phase 1) + a visibility digest that actually works + the
  signed log**. No hard cap, no desktop notification. §0/§2.3/§4 rewritten to say this honestly.
- **[BLOCKER] Keep-both / Dismiss never stopped the nagging.** They retire nothing, so both refs stay
  current and the proposal stays OPEN forever — `list_conflicts` and the pending count kept surfacing it
  every session. Rev 1 wired exclusion into the *finder* but not the *reader*. Fixed: the OPEN-set reader
  and the snapshot count now also drop proposals whose pair is coexist/dismissed (§2.2, §2.4).
- **[BLOCKER/MAJOR] Idempotency false premise.** The retire primitives are fail-LOUD
  (`Err("already retired")`, `log.rs:5130-5134` / `5214-5218`), not no-ops. `resolve_conflict` now owns its
  idempotency: it short-circuits on an existing terminal marker BEFORE calling a primitive (§2.1).
- **[BLOCKER] Key-space confusion.** `resolution_excluded_refs` (the param Rev 1 named) feeds the
  SINGLE-ref `excluded_refs` space (`log.rs:6432`); coexist/dismiss are PAIR facts. Rev 2 unions them into
  the internally-derived `open_pairs` (`unordered_pair_key` space) and explicitly leaves
  `resolution_excluded_refs` empty/removed (§2.2).
- **[MAJOR] "Older by ts" could flip the loser.** A passage's `ref_ts` resolves to the session's current
  head, which a re-capture can bump. Rev 2: `RetireOlder` = retire the FROZEN `a_ref`, `RetireNewer` = the
  FROZEN `b_ref` (older/newer were fixed at detection, `log.rs:6453`). No recompute in the resolve path.
- **[MAJOR] Visibility digest was unbuilt + evadable.** Now specified: a `conflict_digest_cursor`; the digest
  counts ALL suppressive actions (retire + dismiss + keep-both); it is rendered in the snapshot's
  **never-truncated** region (beside the fence preamble, `snapshot.rs:60/426`); and the retired count is
  derived from the **retire markers** (written first) so a torn write is still visible (§2.4, §3.4).
- **[MAJOR] Poison-pair budget dropped the whole subject + had no storage.** Now pair-granular (skip only
  the erroring pair, keep judging the subject's other pairs) with a persisted per-subject consecutive-error
  counter and a reset-on-progress rule (§3.3).
- **[MAJOR] Terminal-state guard missing.** A proposal resolved by any marker is terminal; a second,
  different action is rejected (no `coexist_allowed` + `note_retired` for the same pair). First resolution
  wins (§2.1).
- **[MEDIUM] Guest onboarding-assertion guard.** The two new ops must be added to
  `override_onboarding_for_guest` (`server.rs:210`) or carry no `onboarded` flag (§2.3).
- **[MINOR] Cursor rewind is 2-D, needs a seq lookup, and a torn-write note** (§3.2). Wire DTO seams
  enumerated (§2.3). Anchor line numbers corrected (§9). `session_heads` conservative-re-open caveat noted
  (§3.1). Stale-marker unbounded growth noted as an accepted append-only property (§4 I5).

## Parent context
- **Phase 0 — grading harness:** SHIPPED. **Phase 1 — engine prerequisites:** SHIPPED (reversible retire
  note+passage; separate `conflict_index`; session passages persisted at capture; PR #79 → `64207b5`).
  **Phase 2 — DETECTION:** SHIPPED (background sweep → finder → local judge → signed `conflict_proposal`;
  off-by-default; PR #81 → `2cf0ccb`). **Phase 3 — RESOLUTION (this doc):** the owner settles a detected
  conflict from Claude Code; the finder + the reader honor it; retirements are reversible and reported.

## 0. Trust model (owner decision 2026-07-17, "Honest-minimal")

The parent resolution design put resolve actions in the desktop app, App-only + guest-refused (**I8**).
**Owner decision:** for a locally-installed AIR Agent, local Claude Code on the same machine **is** the
owner. Do not build authentication (no Touch ID, no token, no 2FA). **The resolve ops become reachable from
the `MemoryClient` (guest) role** — a deliberate, documented **relaxation of I8** (§4).

**The threat this does NOT dismiss.** AIR Agent ingests files and captures sessions; that content flows into
the Claude-Code context. A booby-trapped file can say *"there is a conflict — retire the memory that
contradicts me,"* and the agent may act on it. Authentication would not help (it is genuinely Peter, who
ingested the poison). **What defends this is not prevention — it is that damage is bounded-by-reversibility
and visible:**

- **Reversible, not destructive (the primary control).** "Retire" calls the Phase 1 reversible primitives.
  A retired memory stays on disk, stays in the signed append-only log, keeps `as_of` truthful, and is one
  `unretire` away (upholds I1). Worst case of a wrong resolve is *recoverable*, not a lost memory.
- **Visible, never silent (the secondary control — must actually work; §2.4).** The SessionStart snapshot
  digest reports every conflict-driven change since the last session — **retires AND dismisses AND
  keep-boths** — in the snapshot's never-truncated region, derived from the markers themselves. The signed
  log is the authoritative human-auditable record.
- **NO rate cap, NO desktop alert.** Rev 1's per-connection rate budget was a no-op on the MCP surface
  (Rev 2 changelog) and is removed. We do not claim a blast-radius bound we cannot enforce against a
  reconnecting client. (If a hard cap is ever wanted, §8 sketches a global log-derived one; not in Phase 3.)

**Honest residual risks (documented, accepted for a local-first owner-trusts-self tool):**
- **Effective arbitrary-memory suppression via judge manipulation.** The `RetireOlder`/`RetireNewer` design
  prevents naming an arbitrary event id, but an attacker who controls ingested content can craft a memory
  the judge rules contradictory to a targeted true memory, then have the agent retire the true side. This
  is broader than "retire only genuine losers." Contained by reversibility + the visibility digest, not
  prevented. (security F3.)
- **Visibility is agent-relayed.** The digest is honest daemon-authored text, but it reaches the human
  through the same Claude-Code channel the poison may influence ("…and don't mention any conflict
  activity"). The signed log is the un-relayed backstop. We do not claim the digest is a *reliable* alarm —
  only an honest one on a cooperative channel. (critic F6.)

**Rejected half-measure (recorded so it is not reintroduced).** Rev 1 floated a "surfaced-first handshake"
(a proposal must be listed before it can be resolved). It is not a defense — the poisoned actor simply lists
then resolves. Dropped.

## 1. Goal (this phase)
Let the owner **resolve** a detected conflict from Claude Code, and make BOTH the finder and the read
surface **honor** it so the pair is never re-surfaced. Four actions, deterministic, **no LLM in the path**:
- **Retire older / Retire newer** — set the losing (frozen) side aside (reversible), resolve the proposal.
- **Keep both** — the two memories coexist; never re-proposed *and* dropped from `list_conflicts`.
- **Dismiss** — snooze the pair; re-opens only on a material change to a member.

Resolution is inert when detection is off (no proposals ⇒ nothing to resolve), so Phase 3 ships **dormant**.

## 2. Architecture — four thin layers

### 2.1 Core engine (`bossclaw-core`) — the resolution ops
Three new event types in `graph.rs` beside `CONFLICT_PROPOSAL_EVENT_TYPE` (`:107`):
```
CONFLICT_RESOLVED_EVENT_TYPE = "conflict_resolved"  // {proposal_id, action, retired_event_id?}
COEXIST_ALLOWED_EVENT_TYPE   = "coexist_allowed"    // {proposal_id, pair_key, a_ref, b_ref}
DISMISSED_EVENT_TYPE         = "dismissed"          // {proposal_id, pair_key, a_ref, b_ref, session_heads}
```
One new op `resolve_conflict(proposal_id, action) -> ResolveOutcome`, `action ∈ {RetireOlder, RetireNewer,
KeepBoth, Dismiss}`:

- **Load + terminal-state guard (idempotency, resolve_conflict OWNS it).** Look up the proposal by id over
  the open set (§2.3 by-id reader). Then, BEFORE touching a primitive, check the resolution fold for an
  existing terminal marker (`conflict_resolved`/`coexist_allowed`/`dismissed`) for this `proposal_id`:
  - **Same proposal already resolved by the SAME action** → **no-op success** (idempotent retry).
  - **Already resolved by a DIFFERENT action** → **reject** (`InvalidInput`, "already resolved"; first
    resolution wins — no `coexist`+`retire` for one pair).
  - **Unknown/never-existed `proposal_id`** → **error** (distinct from already-resolved).
  - **Open + unresolved** → proceed. This makes idempotency correct even though the retire primitives are
    fail-loud (`Err("already retired")`, `log.rs:5130-5134`/`5214-5218`) — we never reach them on a repeat.
- **RetireOlder / RetireNewer.** The loser is the **frozen** ref: `RetireOlder` retires `a_ref`,
  `RetireNewer` retires `b_ref` (detection already stored older→a_ref, newer→b_ref via
  `ref_ts(a) <= ref_ts(b)`, `log.rs:6453`). **Do NOT recompute "older by ts" at resolve time** — a passage's
  `ref_ts` tracks the session's current head, which a re-capture can flip. Dispatch on the loser's kind:
  `ConflictRef::Note{event_id}` → `retire_memory` (`log.rs:5056`); `ConflictRef::Passage{session_id,
  passage_id}` → `retire_passage` (`:5109`). Append `conflict_resolved{proposal_id, action,
  retired_event_id}` **after** the retire marker (§3.4 ordering).
- **KeepBoth** → append `coexist_allowed{proposal_id, pair_key = unordered_pair_key(a,b), a_ref, b_ref}`.
- **Dismiss** → append `dismissed{proposal_id, pair_key, a_ref, b_ref, session_heads}` (§3.1).

`resolve_conflict` performs **no** embedding and calls **no** Reasoner — pure engine + append (I2 needs no
gate; there is no egress in this path at all).

### 2.2 The exclusion wiring — finder AND reader must both honor resolution
The proposal fold gains two derived sets (a sibling reader `resolution_exclusions() -> {coexist_pairs,
dismissed_pairs}`, both keyed by `unordered_pair_key`; live-dismissed per §3.1):

1. **Finder (re-proposal suppression).** `detect_conflicts_once` (`log.rs:6305`) already assembles an
   `open_pairs` set at `:6365` and passes it to the finder at `:6444`. **Union `coexist_pairs ∪
   dismissed_pairs` into that `open_pairs` set** (same `unordered_pair_key` space — `conflict_pair_key`,
   `log.rs:2891`). The pure finder needs zero reshape. **Do NOT use the `resolution_excluded_refs`
   parameter** (`log.rs:6310`) — it feeds the SINGLE-ref `excluded_refs` space (`:6432`), which
   `decide_conflict_sweep` matches against `r.pair_key()` (conflict.rs), so pair keys placed there would
   silently never match. Rev 2 leaves `resolution_excluded_refs` empty and (plan may) delete the dead param
   + its `engine/mod.rs:1091` empty call site.
2. **Reader (stop-nagging — the BLOCKER fix).** `pending_conflict_proposals` (`log.rs:2869`, the read behind
   `ListConflicts`) and the snapshot pending-count must ALSO drop any proposal whose `unordered_pair_key ∈
   coexist_pairs ∪ dismissed_pairs`. Retire already drops via the existing currency-GC (a retired ref
   leaves `current`); KeepBoth/Dismiss retire nothing, so without this filter they would re-surface forever
   (I9 violation). This is the same live-set, applied at the reader.

Both are fold-derived → restart-safe, no cursor.

### 2.3 Proto + daemon — two wire ops
- `bossclawd-proto`: `Request::ListConflicts` and `Request::ResolveConflict{proposal_id, action}`, a wire
  `ResolveAction` enum, a wire `ConflictProposal` DTO (mirroring `ConflictProposalRow` by hand, as proto
  already hand-mirrors core types — e.g. `RetireTarget`), a wire `ConflictRef` representation +
  core↔wire conversions, and `Response::ListConflicts(Vec<ConflictProposal>)` / a `ResolveConflict`
  outcome arm. Core needs a **by-id open-proposal reader** (today `open_conflict_proposals` returns a `Vec`,
  `log.rs:2807`; add a by-id lookup or filter).
- **`Role::allows`** (`bossclawd-proto/src/lib.rs:71`): grant **both** ops to `MemoryClient`. **This single
  allowlist edit IS the I8 relaxation** — commented inline with the owner decision + date.
- **`override_onboarding_for_guest`** (`server.rs:210`): add both ops (resolve onboarding from
  `engine.is_onboarded_local()`), OR define `ResolveConflict`/`ListConflicts` to carry NO `onboarded` field.
  Otherwise the guest dispatch's fail-closed `None` (`server.rs:253`) silently refuses them. (On a
  not-onboarded brain there are no proposals, so the mint-forge risk is nil; this preserves parity.)
- **Rate limiting:** `ListConflicts` and `ResolveConflict` are **NOT** added to `is_rate_limited_op`.
  Per §0 the per-connection limiter cannot bound a reconnecting client, so we do not pretend it is a
  security control here. (An anti-chatter guard is out of scope; §8.)
- Dispatch arms fail-closed like the rest (`server.rs:253` guest arm).

### 2.4 MCP + snapshot digest — the Code-native surface (visibility MUST work)
- `air-memory-mcp` (`mcp.rs`): two tools beside `TOOL_RECALL`/`TOOL_REMEMBER` (`:22`):
  - `list_conflicts()` → the pending proposals (already excludes coexist/dismissed per §2.2 item 2). Every
    rendered field routed through the snapshot sanitizer (`sanitize_injected`, `snapshot.rs:104`, `pub`) —
    belt-and-suspenders even though fields are ids + the content-free `templated_why` today, so a future
    change that puts model text in `why` cannot regress into an unfenced injection.
  - `resolve_conflict(proposal_id, action)` → the `ResolveConflict` wire op.
- **Snapshot digest (`capture/snapshot.rs`):** render the conflict digest in the **never-truncated region**
  — adjacent to `FENCE_OPEN`/the preamble, BEFORE the droppable entries that `assemble_fence` sheds trailing
  (`snapshot.rs:60/426-441`). Two daemon-authored lines within the `SNAPSHOT_MAX_BYTES = 4096` budget:
  - *"N memory conflicts pending — ask me to review"* (from the §2.2-filtered pending count).
  - *"Since last session: R retired, D dismissed, K kept-both via conflict resolution."* — counts **all**
    suppressive actions (not just retires), so Dismiss cannot silence a conflict with zero signal. **R is
    derived from the retire markers** (`note_retired`/`passage_retired` whose source is a `conflict_resolved`
    event), which are written FIRST, so a torn write (§3.4) is still counted.
  - Both lines pinned in the un-dropped region; an exit-gate test asserts they survive a max-overflow
    snapshot.
- Boundary: a `conflict_digest_cursor` marks "last session's" position; advanced when a snapshot is served
  (§3.4). The signed log remains the authoritative record (§0).

## 3. Wrinkles (each with an explicit mechanism + test)

### 3.1 Dismiss re-open — notes for free, passages via `session_heads`
A Note ref is keyed by `event_id`; editing a note mints a new id → new `unordered_pair_key` → the dismissed
set no longer matches → re-proposes for free. A Passage ref is `(session_id, passage_id)` and survives
re-capture, so a dismissed passage pair would stay dismissed forever. **Fix:** store `dismissed` with
`session_heads` = the current head `event_id` of each referenced session at dismiss time. `dismissed_pairs`
counts a dismissal **live** only while every referenced session's current head still equals the stored head;
if a head advanced (re-capture / material change), the dismissal lapses and the pair may re-propose. Notes
need no head. **Cross-kind (Note↔Passage) pair:** governed by BOTH rules — lapses if the note id changed OR
the passage's session head advanced (the more permissive re-open; conservative-safe). **Accepted caveat
(critic minor):** head-granularity is coarser than passage-granularity — re-capturing an *unrelated*
passage in the same session lapses the dismissal. Conservative (may re-ask) rather than wrong (never hides);
acceptable for v1, refine later if noisy.

### 3.2 Unretire needs a conflict-cursor rewind (the re-scan hook)
`unretire`/`unretire_passage` make a memory current again, but the conflict cursor already swept past it.
**Fix:** on unretire, rewind the cursor to re-examine that memory. The cursor is **2-D** `(last_seq,
subject_offset)` (`log.rs:6612`, table columns `last_seq`/`subject_offset`; subjects enumerate notes at
`(seq,0)` and passages at `(capture_seq, passage_id)`). Rewind = lexicographic `min((S, within), current)`
where `S` is the un-retired memory's seq and `within` = `0` for a note, `passage_id` for a passage — a
`conflict_cursor` upsert. Needs a **seq lookup seam**: `unretire` has the note event id (resolve its seq);
`unretire_passage` resolves the session's current-head capture seq. **Ordering:** append the unretire marker
FIRST, then rewind (a torn write leaves the cursor un-rewound → the memory is current but not re-examined
until the next natural sweep past it — a benign delay, not a lost memory). Rewind is monotone (never
advances). Wired into the primitives themselves so any caller is correct.

### 3.3 Poison-pair budget — pair-granular + persistent
Detection today `break`s the whole cycle on the first reasoner `Err` without advancing (`log.rs:6510-6518`),
so a deterministically-erroring pair re-stalls forever, AND pairs ordered after it are never judged.
**Fix (two parts):**
- **Pair-granular, not subject-granular.** On a pair `Err`, skip **only that pair** and continue judging the
  subject's other pairs this cycle. A poison pair must never hide a subject's *other* real conflicts (critic
  F8).
- **Persistent per-pair consecutive-error counter.** Distinguishing a transient reasoner-down (retry, I6)
  from deterministic poison requires memory ACROSS cycles (each `detect_conflicts_once` is a separate call).
  Persist a small `(pair_key, consecutive_errors)` set (new column on the cursor row, or a small
  `conflict_pair_errors` table). Increment on `Err`, **reset to 0 on any successful judge of that pair**;
  once `>= CONFLICT_PAIR_ERROR_BUDGET` (3), mark the pair `poison_skipped` (telemetry) and stop judging it.
  A permanent stall becomes a bounded dropped-counter on ONE pair, never a frozen sweep, never a hidden
  sibling conflict.

### 3.4 Non-atomic retire → still visible
`RetireOlder/Newer` is two appends (retire marker, then `conflict_resolved`), not atomic (per-append lock).
A crash between them retires the memory but drops the `conflict_resolved` marker. **Fix:** derive the
digest's "retired" count from the **retire markers** (`note_retired`/`passage_retired`, written FIRST),
tagged with their `conflict_resolved` source when present — so a torn write is still counted as a retire in
the digest (§2.4). A torn write leaves: memory retired (reversible), proposal GC-withdrawn from the open
set, retire marker present (visible), `conflict_resolved` marker absent (a re-resolve is a clean no-op via
§2.1). No silent retire.

## 4. Invariants — stated honestly
- **I1 — HOLDS, meaning narrowed (precise wording).** "No retirement without an explicit `resolve_conflict`
  call — which may be **agent-initiated** — and every retirement is **reversible + signed**." It does NOT
  mean "human-deliberated" and does NOT mean "App-only." A future reviewer must read I1 as
  *explicit-call + reversible*, not *trusted-surface* and not *human-intent*.
- **I2 — untouched (incidentally strengthened).** The resolve path has no LLM and no egress. Nothing to gate.
- **I3 — off-by-default preserved.** Detection stays default-CLOSED; no proposals ⇒ nothing to resolve.
  Merging ships Phase 3 dormant.
- **I5 — append-only honesty.** Every resolve is a signed event; retire/unretire already signed.
  `coexist_allowed`/`dismissed` markers for a since-edited member linger (never GC'd) — an accepted
  unbounded-growth property consistent with append-only; note for a future fold-GC.
- **I6 — fail-safe.** `resolve_conflict` owns idempotency (§2.1) → crash/retry safe; the poison-pair budget
  (§3.3) removes the one non-transient stall; torn writes are benign (§3.2, §3.4).
- **I7 — hostile output discipline.** Proposals carry no verbatim memory content (Phase 2 `templated_why`);
  `list_conflicts` fields are sanitized/fenced (§2.4). Confirmed by review: `pending_conflict_proposals`
  returns ids + coarse hint + band only (security F6/INFO).
- **I8 — RELAXED (owner decision 2026-07-17).** Resolve ops reachable from `MemoryClient`. **Compensating
  controls (honest):** reversible retire (primary), the working visibility digest (secondary),
  the signed log (authoritative). **NOT** a rate cap (Rev 1's was a no-op; removed). Recorded here + inline
  at `Role::allows`.
- **I9 — strict-quiet.** Resolved/coexist/dismissed suppress re-proposal (§2.2 finder) AND drop from the
  read surface (§2.2 reader) — both, or the nagging never stops.
- **I-gc — referential integrity.** `open_conflict_proposals` withdraws a proposal whose ref went
  non-current (Phase 2); a retire therefore auto-withdraws any *other* open proposal referencing the same
  memory (pairwise coherence) for free.

## 5. Exit gate (Phase 3)
1. **Resolve correctness:** each action → right marker + effect; loser = frozen a_ref/b_ref (NOT recomputed);
   right primitive per ref kind; restart-survival (fold-derived).
2. **Idempotency + terminal state:** repeat same-action = no-op success; different action on a resolved
   proposal = reject; unknown id = error; a mid-resolve crash leaves a re-resolvable or clean-no-op state
   (never a primitive `Err` bubbling to the agent).
3. **Stop-nagging (both sides):** KeepBoth/Dismiss ⇒ the proposal disappears from `list_conflicts` AND the
   pending count drops AND it is not re-proposed by the finder.
4. **Re-open rules:** edited note re-proposes; a re-captured session lapses a dismissed passage pair
   (`session_heads`); coexist re-eval on member edit (new id).
5. **Rewind:** unretire/unretire_passage rewinds the 2-D cursor (lexicographic min, never advances); next
   sweep re-examines the un-retired memory.
6. **Poison budget:** a deterministically-erroring pair is skipped after N cycles WITHOUT hiding the
   subject's other pairs; the counter persists across cycles and resets on success; the sweep never stalls.
7. **Visibility:** the digest counts retire+dismiss+keep-both since the last session; the retire count comes
   from the retire markers (survives a torn write); both digest lines survive a max-overflow snapshot
   (never-truncated region).
8. **Guest reachability + dormancy:** `MemoryClient` may call exactly the two new ops (positive-allowlist
   test) and still be refused `RetireMemory`/`Unretire`/`Teardown`/etc.; onboarding-assertion guard wired;
   with detection OFF the whole feature is inert (merging changes nothing at runtime).

## 6. Testing (mirrors Phase 2's discipline)
- **Resolve ops (core, hermetic):** every action's marker + effect; frozen-side selection (re-capture does
  not flip the loser); ref-kind dispatch; idempotent repeat = no-op; different-action-on-resolved = reject;
  unknown id = error; restart-survival.
- **Reader exclusion:** KeepBoth/Dismiss drop the proposal from `pending_conflict_proposals` + pending count;
  retire drops via currency-GC; finder skips excluded pairs (reuse Phase 2 seeded/stubbed ANN determinism —
  assert engine invariants, not ANN counts).
- **Re-open:** edited-note re-proposes; `session_heads` lapse re-opens a dismissed passage pair (and does NOT
  lapse when nothing changed); cross-kind pair governed by both rules.
- **Rewind:** unretire pulls the 2-D cursor to the lexicographic min; never advances; next sweep re-examines;
  torn write (marker without rewind) is benign.
- **Poison budget:** a deterministically-erroring stub → that pair skipped after N cycles, the subject's
  OTHER pairs still judged; counter persists + resets on success; sweep never stalls.
- **Proto/daemon:** `Role::allows` grants exactly the two new ops to `MemoryClient`, still refuses the other
  destructive ops (positive-allowlist, mirror `memory_client_allows_exactly_*`); onboarding-assertion guard;
  DTO round-trips; dispatch arms fail-closed.
- **Visibility:** digest counts all three action kinds; retire count derived from retire markers (torn-write
  test); both lines survive a max-overflow snapshot; digest cursor advances per served snapshot.
- **Dormant when detection off** (no model call, no proposals, no digest activity).

## 7. Constants (provisional; owner/harness-tunable)
`CONFLICT_PAIR_ERROR_BUDGET = 3` (per-pair consecutive-error cap, §3.3) · digest lines share the existing
`SNAPSHOT_MAX_BYTES = 4096` / `SNAPSHOT_FIELD_MAX` budget · **no** resolve rate constant (removed, §0).

## 8. Deferred (not Phase 3)
- **Desktop conflict card + nav badge** (background-first; de-prioritized).
- **A hard global retirement cap.** If ever wanted: a persistent, log-derived rolling-window count of
  conflict-driven retires that `resolve_conflict` refuses past a threshold (escalating to an explicit
  owner raise). Deliberately NOT built in Phase 3 (owner "Honest-minimal").
- **An out-of-agent alert channel** (desktop/OS notification the poison can't suppress via the agent).
- **An `unretire`/undo MCP tool** (§3.2 wires the cursor rewind into the primitive; a Code-facing "undo the
  last retire" tool is a small optional follow-up).
- **Full multi-way (3+) resolution** beyond pairwise coherence; **auto-resolve** for high-confidence pairs
  (P0 found confidence self-report does not separate true/false contradictions — no auto-resolve now);
  **fold-GC of stale coexist/dismissed markers** (I5 note); **an anti-chatter guard** on the resolve op.

## 9. As-built anchor index (corrected 2026-07-17)
- **Proposal read/append + GC:** `log.rs` `append_conflict_proposal` `:2772`, `open_conflict_proposals`
  **`:2807`** (fn; inner event loop ~`:2841`), `pending_conflict_proposals` **`:2869`**, `conflict_pair_key`
  (== `unordered_pair_key`) `:2891`, `is_conflict_proposal_suppressed` `:2900`.
- **Detection orchestrator + exclusion:** `log.rs` `detect_conflicts_once` `:6305`, `resolution_excluded_refs`
  param `:6310` (feeds SINGLE-ref `excluded_refs` at `:6432` — NOT the vehicle), `open_pairs` assembly
  `:6365`, finder call `:6444`, per-subject error `break` `:6510-6518`, `ref_ts` older/newer `:6453`; daemon
  wrapper `bossclawd/src/engine/mod.rs` `detect_conflicts_once` `:1060`, empty excluded pass `:1091`.
- **Cursor:** `log.rs` `conflict_cursor` `:6612` (2-D `last_seq`/`subject_offset`), `set_conflict_cursor`
  `:6626`, subject enumeration ~`:7190-7216`.
- **Phase 1 retire primitives (fail-LOUD):** `retire_memory` `:5056`, `assert_retirable_note` err
  `:5214-5218`, `unretire` `:5079`, `retire_passage` `:5109`, passage-already-retired err `:5130-5134`,
  `unretire_passage` `:5160`.
- **Pair/ref keys:** `index.rs` `ConflictRef::pair_key` `:120`, `unordered_pair_key` `:132`.
- **Event types:** `graph.rs` `CONFLICT_PROPOSAL_EVENT_TYPE` `:107`, retire markers `NOTE_RETIRED`/
  `PASSAGE_RETIRED`/`UNRETIRE` `:40-44`, `SESSION_CAPTURED`/`SESSION_DELETED` `:35/:37`.
- **Gate/flag:** `log.rs` `conflict_detect_enabled` `:7068`.
- **Proto/role/guest guard:** `bossclawd-proto/src/lib.rs` `Role` `:55`, `Role::allows` `:71`, hand-mirrored
  `RetireTarget`/`Response::Retired` ~`:230-253/:337`; `bossclawd/src/server.rs`
  `override_onboarding_for_guest` `:210`, guest dispatch `:253`, `CaptureRateLimiter` `:68` /
  `is_rate_limited_op` `:93` (do NOT add resolve ops).
- **MCP surface:** `air-memory-mcp/src/mcp.rs` `TOOL_RECALL`/`TOOL_REMEMBER` `:22`, `tools_list_result`
  `:82`; reconnect-per-call `air-memory-mcp/src/daemon.rs` `:3`/`:95`, snapshot fetch `:192`.
- **Snapshot builder:** `bossclawd/src/capture/snapshot.rs` `SNAPSHOT_MAX_BYTES` `:62`, `SNAPSHOT_FIELD_MAX`
  `:66`, `sanitize_injected` `:104`, `assemble_fence` (drops TRAILING; preamble/close/affordance never
  dropped) `:426-441`, `FENCE_OPEN` `:84`.

## 10. Open questions for the plan
1. **Exclusion reader shape:** one `resolution_exclusions()` reader consumed by BOTH the finder's `open_pairs`
   union AND `pending_conflict_proposals`, vs two derivations. Prefer one (single source of truth).
2. **`session_heads` cost:** the dismissed-passage liveness check reads each referenced session's head per
   fold; memoize per fold (mirror Phase 2's `head_passage_counts` memo ~`log.rs:2822`) if it approaches
   O(proposals × sessions) at the open-proposal ceiling.
3. **Poison counter home:** a column on the `conflict_cursor` row vs a small `conflict_pair_errors` table.
   Plan picks the simplest restart-safe form.
4. **Digest cursor:** confirm no reusable SP3 last-session boundary exists (review found none in the snapshot
   builder); if truly none, add a dedicated `conflict_digest_cursor` advanced on snapshot serve.
