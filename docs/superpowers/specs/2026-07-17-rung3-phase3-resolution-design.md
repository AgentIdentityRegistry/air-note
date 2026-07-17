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

## Rev 3 — convergence re-verification findings folded (2026-07-17)
Rev 2 went through a focused re-verification (fresh Opus security + correctness). Security returned **LOW /
CONVERGED**; correctness returned **REVISE** with two MAJORs that **share one root cause** (both re-reviewers
landed on it independently): the resolve path reuses Phase 1's `retire_memory`/`retire_passage`, whose
markers are **byte-identical** whether the retire is conflict-driven or a manual App retire
(`retire_memory` writes `{"retires": id}` `log.rs:5063`; the same primitive backs the App `RetireMemory` op
`server.rs:450-459`), AND a successful retire **withdraws the proposal from the OPEN set** (retired ref →
non-current → `open_conflict_proposals` drops it, `log.rs:2827-2839`). One unifying fix closes both:

- **[MAJOR-1] Idempotency universe.** Reading the terminal check over the OPEN set breaks a *legitimate*
  retire retry (the proposal is already withdrawn → "unknown id") and the torn-write case. **Fix:** the
  idempotency/terminal check reads a fold of **ALL** `conflict_resolved`/`coexist_allowed`/`dismissed`
  events keyed by `proposal_id`, and an **all-proposals** by-id reader recovers `a_ref`/`b_ref` (not
  open-only). Torn-write retry (loser already retired, no `conflict_resolved`) → **roll forward** (append
  the missing marker, no-op success). §2.1/§2.3 rewritten.
- **[MAJOR-2 = security N1] Retire-marker provenance.** The digest can't be *both* conflict-scoped *and*
  torn-write-safe from an untagged marker (counting all retire markers mislabels manual retires; joining to
  `conflict_resolved` loses torn-write retires). **Fix:** the conflict path stamps the retire marker with
  provenance — `retire_memory`/`retire_passage` gain an optional `source_proposal_id` that adds
  `{"via":"conflict","proposal_id":…}` to the marker content (same event *type*, so the retire fold is
  untouched; App path passes `None`). The digest's R-count reads `via=="conflict"` markers since the digest
  cursor — conflict-scoped AND from the first-written marker. (MAJOR-1's roll-forward gate is separately
  keyed on **retired-set membership**, NOT on this tag — the tag is digest-scoping only.) §2.1/§2.4/§3.4
  reconciled.
- **[MINOR-1] `sanitize_injected` crate boundary.** It lives in `bossclawd` (`snapshot.rs:104`), but
  `air-memory-mcp` depends on `bossclawd` only as a **dev-dependency** — `mcp.rs` cannot call it in prod.
  **Fix:** sanitize **daemon-side** when building the `ListConflicts` response (in `bossclawd`, in-crate).
  §2.4 corrected.
- **[MINOR-2] Poison cursor-advance rule.** **Fix:** the cursor does **not** advance past a subject while it
  has an outstanding *sub-budget* pair error (so a transient reasoner outage retries, preserving I6); once a
  pair reaches `CONFLICT_PAIR_ERROR_BUDGET` it is `poison_skipped` and no longer blocks advance. Accepted
  tradeoff (stated): a transient outage lasting > budget cycles could falsely poison-skip. §3.3.
- **[MINOR-N2] Onboarding guard wording.** Even a flag-less op needs an explicit passthrough arm in
  `override_onboarding_for_guest` (a missing arm → `None` → refused). §2.3 says "add a passthrough/rewrite
  arm" unconditionally.
- **[resolved Open-Q1] Single source of truth.** ONE `resolution_exclusions()` reader feeds BOTH the finder
  union and the reader filter, so `session_heads` liveness is evaluated once (no drift). §2.2.
- Everything else in Rev 2 verified accurate (all anchors, the frozen-loser, key-space, cursor-rewind,
  stop-nagging-reader, and finder-union fixes). Cosmetic: `SNAPSHOT_MAX_BYTES` is `snapshot.rs:62` (not :60).

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

- **Load + terminal-state guard (idempotency, resolve_conflict OWNS it — MAJOR-1 fix).** A retire withdraws
  the proposal from the OPEN set (retired ref → non-current), so the guard must NOT key off open-set
  membership. Use an **all-proposals by-id reader** to load the `conflict_proposal` event by id (it always
  exists in the log; open-ness is a derived filter) and a **resolution fold over ALL**
  `conflict_resolved`/`coexist_allowed`/`dismissed` events keyed by `proposal_id`:
  - **Unknown/never-existed `proposal_id`** (no `conflict_proposal` event) → **error**.
  - **Already resolved by the SAME action** → **no-op success** (idempotent retry).
  - **Already resolved by a DIFFERENT action** → **reject** (`InvalidInput`, "already resolved"; first
    resolution wins — no `coexist`+`retire` for one pair).
  - **Torn-write retry** (a Retire action, no `conflict_resolved` marker, but the frozen loser is already in
    the fold's retired set — the §3.4 crash window) → **roll forward**: append the missing
    `conflict_resolved`, return no-op success. Do NOT call the primitive again (it is fail-loud,
    `Err("already retired")`, `log.rs:5130-5134`/`5214-5218`).
  - **Unresolved + loser not yet retired** → proceed. Because the guard runs first, the fail-loud primitives
    are never reached on a repeat.
- **RetireOlder / RetireNewer.** The loser is the **frozen** ref: `RetireOlder` retires `a_ref`,
  `RetireNewer` retires `b_ref` (detection already stored older→a_ref, newer→b_ref via
  `ref_ts(a) <= ref_ts(b)`, `log.rs:6453`). **Do NOT recompute "older by ts" at resolve time** — a passage's
  `ref_ts` tracks the session's current head, which a re-capture can flip. Dispatch on the loser's kind to a
  **provenance-stamped** retire (MAJOR-2 fix): `retire_memory(event_id, source_proposal_id=Some(id))`
  (`log.rs:5056`) / `retire_passage(session_id, passage_id, source_proposal_id=Some(id))` (`:5109`) — a new
  optional param that adds `{"via":"conflict","proposal_id":id}` to the SAME `note_retired`/`passage_retired`
  marker content (the retire fold keys on `retires`/`session_id+passage_id` and is untouched; the App path
  passes `None`, byte-identical to today). Then append `conflict_resolved{proposal_id, action,
  retired_event_id}` **after** the retire marker (§3.4 ordering; the tagged retire marker — written first —
  is the torn-write-safe, conflict-scoped source the digest counts).
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
  outcome arm. Core needs a **by-id ALL-proposals reader** (find the `conflict_proposal` event by id
  regardless of open-ness — MAJOR-1) to recover `a_ref`/`b_ref` for a retire retry after the proposal has
  left the open set; `ListConflicts` still reads the open + resolution-filtered `pending_conflict_proposals`
  (`log.rs:2869`).
- **`Role::allows`** (`bossclawd-proto/src/lib.rs:71`): grant **both** ops to `MemoryClient`. **This single
  allowlist edit IS the I8 relaxation** — commented inline with the owner decision + date.
- **`override_onboarding_for_guest`** (`server.rs:210`): add an explicit **passthrough/rewrite arm** for
  BOTH ops — unconditionally, even if they carry no `onboarded` field. This function returns `None` for any
  variant it does not list, and the guest dispatch maps `None → not_permitted_response` (`server.rs:253`),
  so a missing arm silently refuses the op. If an op does carry `onboarded`, rewrite it from
  `engine.is_onboarded_local()`; if not, return `Some(req)` unchanged. (On a not-onboarded brain there are
  no proposals, so the mint-forge risk is nil; this preserves the fail-closed parity pattern.)
- **Rate limiting:** `ListConflicts` and `ResolveConflict` are **NOT** added to `is_rate_limited_op`.
  Per §0 the per-connection limiter cannot bound a reconnecting client, so we do not pretend it is a
  security control here. (An anti-chatter guard is out of scope; §8.)
- Dispatch arms fail-closed like the rest (`server.rs:253` guest arm).

### 2.4 MCP + snapshot digest — the Code-native surface (visibility MUST work)
- `air-memory-mcp` (`mcp.rs`): two tools beside `TOOL_RECALL`/`TOOL_REMEMBER` (`:22`):
  - `list_conflicts()` → the pending proposals (already excludes coexist/dismissed per §2.2 item 2). Fields
    are sanitized **daemon-side** when the `bossclawd` handler builds the `ListConflicts` response, via
    `sanitize_injected` (`snapshot.rs:104`, in-crate there — MINOR-1: `air-memory-mcp` has `bossclawd` only
    as a dev-dependency and cannot call it in prod). Belt-and-suspenders even though fields are ids + the
    content-free `templated_why` today, so a future change that puts model text in `why` cannot regress into
    an unfenced injection.
  - `resolve_conflict(proposal_id, action)` → the `ResolveConflict` wire op.
- **Snapshot digest (`capture/snapshot.rs`):** render the conflict digest in the **never-truncated region**
  — adjacent to `FENCE_OPEN`/the preamble, BEFORE the droppable entries that `assemble_fence` sheds trailing
  (`snapshot.rs:62`/`:426-441`). Two daemon-authored lines within the `SNAPSHOT_MAX_BYTES = 4096` budget:
  - *"N memory conflicts pending — ask me to review"* (from the §2.2-filtered pending count).
  - *"Since last session: R retired, D dismissed, K kept-both via conflict resolution."* — counts **all**
    suppressive actions (not just retires), so Dismiss cannot silence a conflict with zero signal. **R is
    derived from the provenance-stamped retire markers** (`note_retired`/`passage_retired` whose content has
    `via=="conflict"` — the §2.1 MAJOR-2 tag), which are written FIRST, so the count is both conflict-scoped
    (manual App retires, tagless, are excluded) AND torn-write-safe (§3.4).
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
- **Cursor-advance rule (MINOR-2).** The subject cursor does **NOT** advance past a subject while it has an
  outstanding *sub-budget* pair error (`0 < consecutive_errors < CONFLICT_PAIR_ERROR_BUDGET`) — so a
  transient reasoner outage (all pairs erroring) simply retries that subject next cycle, preserving I6. Once
  a pair reaches the budget it is `poison_skipped` and no longer holds the cursor. **Accepted tradeoff
  (stated):** a transient outage lasting **>** `CONFLICT_PAIR_ERROR_BUDGET` consecutive cycles could falsely
  poison-skip a real pair; the budget (3) is chosen so a brief blip retries but a genuinely deterministic
  failure is bounded.

### 3.4 Non-atomic retire → still visible
`RetireOlder/Newer` is two appends (the provenance-stamped retire marker, then `conflict_resolved`), not
atomic (per-append lock). A crash between them retires the memory but drops the `conflict_resolved` marker.
The retire marker itself already carries `{"via":"conflict","proposal_id":id}` (§2.1 MAJOR-2) and is written
FIRST, so the digest's R-count sees it regardless of the torn write — **conflict-scoped AND torn-write-safe
from one marker**. A torn write leaves: memory retired (reversible), proposal GC-withdrawn from the open
set, tagged retire marker present (visible + attributable), `conflict_resolved` marker absent. A re-resolve
is a **clean no-op** via §2.1's roll-forward, whose trigger is **retired-set membership** — the guard sees
the frozen loser already in the fold's retired set (regardless of which action/proposal/manual-retire put it
there) and appends the missing `conflict_resolved` instead of re-calling the fail-loud primitive. The
`via`/`proposal_id` tag's job is **digest R-count scoping ONLY, not the roll-forward gate.** No silent
retire, no mis-attribution.

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
2. **Idempotency + terminal state (all-proposals fold, not open-set):** repeat same-action = no-op success
   EVEN AFTER the retire withdrew the proposal from the open set; different action on a resolved proposal =
   reject; unknown id = error; a torn-write retry (loser retired, `conflict_resolved` missing) rolls forward
   to no-op success — never a primitive `Err` bubbling to the agent.
3. **Stop-nagging (both sides):** KeepBoth/Dismiss ⇒ the proposal disappears from `list_conflicts` AND the
   pending count drops AND it is not re-proposed by the finder.
4. **Re-open rules:** edited note re-proposes; a re-captured session lapses a dismissed passage pair
   (`session_heads`); coexist re-eval on member edit (new id).
5. **Rewind:** unretire/unretire_passage rewinds the 2-D cursor (lexicographic min, never advances); next
   sweep re-examines the un-retired memory.
6. **Poison budget:** a deterministically-erroring pair is skipped after N cycles WITHOUT hiding the
   subject's other pairs; the counter persists across cycles and resets on success; the sweep never stalls.
7. **Visibility:** the digest counts retire+dismiss+keep-both since the last session; the retire count comes
   from the `via=="conflict"`-tagged retire markers — conflict-scoped (a manual App retire is NOT counted)
   AND torn-write-safe (counted even if `conflict_resolved` was lost); both digest lines survive a
   max-overflow snapshot (never-truncated region).
8. **Guest reachability + dormancy:** `MemoryClient` may call exactly the two new ops (positive-allowlist
   test) and still be refused `RetireMemory`/`Unretire`/`Teardown`/etc.; onboarding-assertion guard wired;
   with detection OFF the whole feature is inert (merging changes nothing at runtime).

## 6. Testing (mirrors Phase 2's discipline)
- **Resolve ops (core, hermetic):** every action's marker + effect; frozen-side selection (re-capture does
  not flip the loser); ref-kind dispatch; **idempotent repeat = no-op EVEN after the retire withdrew the
  proposal from the open set** (all-proposals fold); **torn-write retry rolls forward** (loser retired,
  `conflict_resolved` missing → append it, no-op success — no primitive `Err`); different-action-on-resolved
  = reject; unknown id = error; restart-survival.
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
- **Visibility:** digest counts all three action kinds; retire count derived from `via=="conflict"`-tagged
  markers (torn-write test: count survives a dropped `conflict_resolved`; scope test: a **manual App retire
  is NOT counted**); both lines survive a max-overflow snapshot; digest cursor advances per served snapshot.
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
- **Phase 1 retire primitives (fail-LOUD; gain optional `source_proposal_id`):** `retire_memory` `:5056`
  (marker `{"retires": id}` `:5063` — extend with `via`/`proposal_id`), `assert_retirable_note` err
  `:5214-5218`, `unretire` `:5079`, `retire_passage` `:5109` (marker `{session_id, passage_id}` `:5140`),
  passage-already-retired err `:5130-5134`, `unretire_passage` `:5160`. Manual App path that must keep
  passing `None`: `bossclawd/src/server.rs` `RetireMemory` op `:450-459`.
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
1. **~~Exclusion reader shape~~ (RESOLVED, Rev 3):** ONE `resolution_exclusions()` reader is consumed by BOTH
   the finder's `open_pairs` union AND `pending_conflict_proposals`, so `session_heads` liveness is evaluated
   once — no drift between finder and reader. Single source of truth.
2. **`session_heads` cost:** the dismissed-passage liveness check reads each referenced session's head per
   fold; memoize per fold (mirror Phase 2's `head_passage_counts` memo ~`log.rs:2822`) if it approaches
   O(proposals × sessions) at the open-proposal ceiling.
3. **Poison counter home:** a column on the `conflict_cursor` row vs a small `conflict_pair_errors` table.
   Plan picks the simplest restart-safe form.
4. **Digest cursor:** confirm no reusable SP3 last-session boundary exists (review found none in the snapshot
   builder); if truly none, add a dedicated `conflict_digest_cursor` advanced on snapshot serve.
5. **Cursor rewind on a never-enabled brain:** `unretire`/`unretire_passage` are callable via the App wire op
   even when detection was never enabled; the rewind is a `conflict_cursor` upsert. Confirm the row is
   created-if-absent (benign upsert) so the rewind never errors on a brain that never ran detection.
6. **Backstop asymmetry (note, not a blocker):** the durable pre-append re-check
   `is_conflict_proposal_suppressed` (`log.rs:6483`) is open-proposal-only; coexist/dismissed re-proposal
   suppression relies solely on the in-memory `open_pairs` union (rebuilt each cycle from the fold). Correct
   while the fold is authoritative; documented so the single-guard choice is deliberate, not an oversight.
7. **Roll-forward `retired_event_id` source:** the `conflict_resolved` appended by a torn-write roll-forward
   must record the frozen loser resolved via the **all-proposals by-id reader** (not an open-set read, which
   is empty after withdrawal), so the marker is well-formed. Targeted test.
8. **Digest window boundary = seq, not marker-id:** the R-count enumerates `via=="conflict"`
   `note_retired`/`passage_retired` since `conflict_digest_cursor`; make that a **seq** boundary so a torn
   write between the retire marker and `conflict_resolved` cannot slip the retire marker out of the counted
   window.
9. **Accepted benign edge (one-line note, no code):** a torn-write `RetireOlder` followed by a deliberate
   `RetireNewer` on the same proposal can retire BOTH sides (the torn write left no `conflict_resolved`, and
   `b_ref` is not yet retired, so the guard proceeds). Bounded, reversible, and both retires are
   `via=="conflict"`-tagged (visible in the digest) — consistent with §0's reversibility+visibility model;
   the idempotency guarantee is scoped to "repeat SAME action," so this is an accepted edge, not a
   contradiction.
10. **Executor precision (§2.4):** the two digest lines go in the `render_fence` **preamble** (right after
    `FENCE_OPEN`, `snapshot.rs:446`), NOT as `entries` (which `assemble_fence` trailing-drops at `:439`) —
    that is what makes them survive a max-overflow snapshot.
