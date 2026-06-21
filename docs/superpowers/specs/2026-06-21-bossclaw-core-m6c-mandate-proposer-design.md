# bossclaw-core M6c — General + Mandate Proposer (Design)

- **Date:** 2026-06-21
- **Status:** **Design — shape approved by Peter 2026-06-21 (brainstorm).** Pending: spec self-review → **user review** → independent **critic + security** review (mandatory; widest confused-deputy surface) → Rev 2 → plan → subagent-driven build (all Opus) → PR.
- **Milestone:** M6c — the general + mandate proposer. **Brick 3 (final) of the M6 actuator program.** The "moon shot": the brain proposes file edits *toward a standing goal*, autonomously, with every safety rail bolted on.
- **Parent:** `docs/superpowers/specs/2026-06-20-m6-actuator-program-design.md` (§5 "M6c"). **Builds on** M6a "Safe Hands" (gated write mechanism, merged `a79adbc`) and M6b "Reconciliation Proposer" (the first autonomous trigger + the whole proposer posture, merged `ee75718`).
- **Crate:** `crates/bossclaw-core` (`#![forbid(unsafe_code)]`, `#![deny(missing_docs)]`).
- **Runway:** `air/m6c-mandate-proposer-prep` (GBrain). Seam-map: verified against merged `main` by an Opus code-explorer 2026-06-21 (all file:line refs below are real).

---

## 1. Goal & framing

M6a gave the engine *hands* (a gated write). M6b gave it a *reflex* (propose a fix when a fact contradicts an ingested file). **M6c gives it a *standing job*: a signed, bounded *mandate* the user grants — "keep file X in sync with source files Y" — and a *general proposer* that works toward that goal on its own, proposing edits whenever a source changes, not only on a contradiction.**

The hard, genuinely-new part the program design flagged: *"how does the proposer decide a concrete edit advances the goal with no contradiction to anchor on?"* M6c **dissolves** that problem by choosing a first mandate **type whose goal-state is computable**: a file is "in sync" iff its bytes equal `recipe(current sources)`. So "autonomous goal pursuit" becomes "compute expected, diff against actual, propose if different" — bounded, deterministic to trigger, and unit-testable. We get the moon-shot *capability* (the brain maintaining your files hands-off) without the moon-shot *risk* (free-form pursuit).

Two pieces, deliberately built and reviewed as separable units:
- **The Lister (Piece 1, security-critical):** the mandate primitive + the recipe-compare proposer. Gets the heavy dual adversarial security review.
- **The Watcher (Piece 2, lower-stakes):** a live OS filesystem watcher + a simple self-driving loop. It can never write — the worst it does is say "look again." Gets its own, simpler review.

**Nothing auto-writes.** "Autonomous" means auto-*propose*; every actual write still passes M6a's gate **and** an explicit human confirm. The Watcher only removes the need for a human to *poke* the brain to look.

---

## 2. Scope decisions (locked 2026-06-21)

| # | Decision | Choice | Rationale |
|---|----------|--------|-----------|
| **D1** | First mandate **type** | **"Keep a file in sync"** (derived-file maintenance) | Computable goal-state ("file == `recipe(sources)`") makes the general trigger tractable + testable. |
| **D2** | **Blast radius** | **Fully-generated file** — the mandate owns the *whole* target file (brain-owned artifact, like a generated index/TOC) | Reuses M6b's whole-file rewrite mechanism unchanged; cleanest contract; **no managed-region machinery in v1.** |
| **D3** | **Proposer spine** | **Recipe + compare** (Approach A), **not** free-form goal pursuit (Approach B) | B is the unbounded, widest-surface trap; deferred as a possible future brick. |
| **D4** | **Watcher scope** | **Pulled INTO M6c** — fully hands-off, end to end | Peter's explicit call: the brain monitors on its own. M6c borrows a *simple* self-driving loop (M7 still owns the battery/thermal-smart scheduler later). |
| **D5** | **Watcher mechanism** | **Live OS watcher** (the `notify` crate; FSEvents / inotify) | Instant true monitoring. Accepts the engine's first new third-party dependency in several milestones + per-OS code paths. |
| **D6** | **OS** | `#[cfg(unix)]` for v1 | Matches the entire `#[cfg(unix)]` actuator/proposer surface (A7/§8); CI is Unix-scoped. |
| **D7** | **Proposal op set** | **Create or Edit only — never Delete** | A sync mandate maintains a file; it never deletes the target. Removes the most dangerous op from the autonomous surface entirely. |
| **D8** | **Off-switch** | A **new, independent** sticky `mandates_enabled` switch (mirrors `proposals_enabled`) | The widest-surface proposer must be killable without disabling M6b reconcile. |

---

## 3. The pipeline (every mandate, every tick)

```
  user edits a source file
        │
  ①  Watcher (live OS, notify)        "a source changed!"            ← NEW (Piece 2, cfg(unix))
        │  debounced (EVOLVE_DEBOUNCE_MS=2000); event storms coalesced
  ②  Re-ingest changed sources        ingest_all → dedup/supersede   ← REUSE  (ingest.rs:582, :697-715)
        │
  ③  Evolve tick                      evolve_once                    ← REUSE  (log.rs:4854) + simple self-driver (NEW)
        │
  ④  Mandate phase (the Lister):                                     ← NEW (Piece 1)
        for each active mandate M:
          expected = cached_or_synth(recipe ⊕ FENCED sources)  // synth ONCE per source-state, deterministic, cached   ← REUSE fence (extract.rs:185) + synth pattern (reconcile.rs:106)
          actual   = std::fs::read(M.target)        // ON-DISK bytes, never the stale projection hash
          if expected != actual  and  not suppressed  and  under caps:
              lineage = {M.mandate_id} ∪ {source file_ingested ids read}   // engine-gathered, never model cites
              gated  = propose_write(WriteProposal{target=M.target, new_content=expected, op, source_event_ids=lineage})  ← REUSE (log.rs:2368)
              record  write_proposal + cache proposal_bytes          ← REUSE (log.rs:1966, :2063)
        │
  ⑤  YOU confirm (loud modal if tainted) → execute_write_resolving → file_written   ← REUSE (log.rs:2598)
```

Steps ②③⑤ are hardened machinery from M5/M6a/M6b. The genuinely new code is ① the Watcher + self-driver, and ④ the mandate primitive + recipe-compare proposer.

---

## 4. The mandate primitive (Piece 1a)

### 4.1 What a mandate is + schema

A **mandate** is a signed, bounded, standing goal the user grants, stored as a ground-truth event and folded into a projection — **mirroring the write-grant pattern exactly** (`add_write_grant` → `write_grants` table → `WriteGrant` type, `log.rs:2213` / `:345` / `graph.rs:438`).

`mandate_grant` event (Tier-A ground-truth, `model_meta: None`, signed): content
```json
{
  "target":                "<canonical path>", // the brain-owned file (whole-file owned)
  "source_scope":          "<canonical dir prefix>", // sources it derives from; MUST exclude target
  "recipe":                "<plain-English rule, user-authored>", // e.g. "an index of every file's title + a one-line summary"
  "max_proposals_per_day": 8,                  // per-mandate calm cap (u32)
  "expiry":                "<ISO-8601 | null>" // optional; null = until revoked
}
```
**Mandate identity = the `mandate_grant` event id** (assigned by the append chokepoint, exactly as a file record's identity is its `file_event_id`) — *not* a content-supplied ULID. This is load-bearing for taint: the identity is then a **real, readable ground-truth event id**, so including it in a proposal's `source_event_ids` (§5.3) contributes no taint and never trips the chokepoint's fail-closed "unreadable source ⇒ external" rule (`source_is_external_in_tx`, `log.rs:532`). A free-standing content ULID would be unreadable as an event → would wrongly stamp **every** mandate proposal external.

`mandate_revoke` event: content `{ "mandate_grant_id": "<event id>" }` — sets `revoked=1` in the projection (**sticky + fail-closed**, like `write_revoke`).

Projection `mandates` (new table, folded in `rebuild_graph` beside the `write_grants` fold at `log.rs:3692`):
`(mandate_grant_id TEXT PK, target TEXT, source_scope TEXT, recipe TEXT, max_proposals_per_day INTEGER, expiry TEXT, granted_at TEXT, revoked INTEGER DEFAULT 0)`. Throughout this spec, **`mandate_id` is shorthand for this `mandate_grant_id`.**
Read type `Mandate` in `graph.rs` (re-exported in `lib.rs`). Reader `active_mandates() -> Vec<Mandate>` (revoked=0 and not expired).

The `recipe` is **user-authored and engine-trusted**, but is still sanitized into the prompt's trusted frame (§5.2) for defense-in-depth. The mandate is **not** embeddable/recallable (left out of `EMBEDDABLE_EVENT_TYPES`, `log.rs:143`, exactly like `file_written`).

### 4.2 Grant / revoke

- App-side UX grants/revokes; the engine exposes `add_mandate(...)` / `revoke_mandate(mandate_id)` mirroring `add_write_grant`/`revoke_write_grant`. Re-grant of the same `mandate_id` uses `append_pair` (atomic supersede + new) if needed.
- **Grant-time guard (UX):** `add_mandate` refuses if `target` is not currently under an active **write**-grant (`is_write_allowed(target)`, `log.rs:2276`) — you cannot create a mandate the brain could never act on. This is a convenience check, **not** the security boundary.

### 4.3 The two iron rules (load-bearing)

1. **A mandate NEVER widens a write-grant.** The *load-bearing* enforcement is at **propose time**: every proposal goes through `propose_write`, whose step 2 calls `is_write_allowed(target)` (`log.rs:2368` / `:2276`). If the target's write-grant was revoked after the mandate was granted, the gate returns `allowed=false` with a `reject_reason` → M6c records `write_rejected`, **no write**. Granting a mandate ≠ granting write; revoking the write-grant makes the mandate inert. (Grant-time check in §4.2 is only UX.)
2. **A mandate NEVER sheds taint.** Two engine-anchored enforcement points, neither trusting the model:
   - The proposal's `source_event_ids` is **engine-gathered** = `{mandate_id} ∪ {the file_ingested ids of the sources the engine actually read}` (§5.3). Any external source → the append chokepoint `append_event_in_tx` stamps `origin:"external"` on the `write_proposal` (`log.rs:562-570`).
   - `propose_write` step 4 independently re-derives taint from the **target's** own provenance via path/`(dev,ino)` (`log.rs:2465-2499`) — so even a mandate that (mis)targets a tracked ingested file is marked `Untrusted` + loud.

---

## 5. The general proposer (the Lister — Piece 1b)

### 5.1 Trigger: recipe-compare (computable expected-state)

Runs as a **new top-level phase in `evolve_once`**, after `rebuild_graph` (`log.rs:5091`) so the graph + `files` projection it reads are current, and before/around the summarize phase. Gated by the new `mandates_enabled` switch (read once per tick) **and** the existing evolve off-switch (if evolve is off, nothing runs).

For each active mandate `M`:
1. **Gather sources:** `current_files()` (`log.rs:3322`) filtered by **segment-aware** descent under `M.source_scope` (path-component `starts_with`, *not* raw-string prefix — the same discipline as `is_write_allowed`, so `/a/b` never matches `/a/bc`) **and excluding `M.target`** (a recipe's output is never its own input). *(Note: no path-prefix query exists — filter `current_files()` in-engine; A6/§8.)* Read each source's bytes from disk; collect their `file_event_id`s.
2. **Compute expected — cached per source-state (§5.2):** form `sources_hash` = hash of the sorted `(path, content_hash)` set of the in-scope sources. If the synthesis cache holds `(M.mandate_id, sources_hash)`, **reuse the cached `expected_bytes`** (no LLM call); else `expected_bytes = reasoner(build_recipe_prompt(M.recipe, fenced sources))` (deterministic mode) and cache it. `expected` is thus a **stable function of the source-state**, recomputed only when sources change.
3. **Read actual:** `actual = std::fs::read(M.target)` — the **on-disk bytes** (or absent → op = Create). **Never the `files` projection `content_hash`**, which is stale after an actuator write (A6/§8). Comparing the **cached** expected against on-disk actual is what makes the loop *converge* (§6.4) — robustly, even if the model is nondeterministic.
4. **Decide:** if `expected != actual` → candidate proposal; else nothing.
5. **Suppress / cap / emit** (§5.4, §5.5): if not suppressed and under caps, build the lineage (§5.3), call `propose_write`, and on `allowed` record `append_write_proposal` + `put_proposal_bytes`; on gate-reject record `append_write_rejected`.

**Op selection (D7):** target missing → `Create`; target exists → `Edit`. **Never `Delete`.**

### 5.2 Synthesis (fenced — reuse M6b's posture)

New pure helper `build_recipe_prompt(recipe: &str, fenced_sources: &str) -> String` in `reconcile.rs` (or a new `mandate.rs`), mirroring `build_rewrite_prompt` (`reconcile.rs:106`):
- **Trusted frame** holds the sanitized `recipe` (via the shared `summarize::sanitize_ident`, `summarize.rs:167` — strips all 12/12 Unicode bidi controls + separators) and capped length. The recipe is the *only* instruction.
- **Untrusted data** = each source file body, fenced via `crate::extract::push_fenced_source` (`extract.rs:185`) — the exact fence M6b uses; embedded `<<<SOURCE_*>>>` markers neutralized with ZWSP. Combined fenced sources capped at `MAX_INPUT_TEXT_BYTES=16_384` (`extract.rs:95`); over-cap sources truncate (v1 limitation, logged).
- **Output schema** `recipe_schema()` requires exactly one string `synced_content`, `additionalProperties:false` (mirrors `rewrite_schema`, `reconcile.rs:127`). `expected_bytes = synced_content.into_bytes()`.

The model reads fenced data + a trusted recipe and emits the whole derived file — identical trust posture to M6b's whole-file rewrite, then gated + human-confirmed.

**Deterministic + cached synthesis (load-bearing for convergence).** An LLM is not bit-exact across calls, so re-synthesizing every tick would make `expected` drift and the proposer would churn even with the file already in sync (a self-loop). The engine therefore **synthesizes once per source-state and caches the bytes**, keyed on `(mandate_id, sources_hash)` in an encrypted `mandate_synthesis_cache` table (§8); later ticks reuse the cached bytes for the compare and skip the LLM entirely until the sources change. Synthesis also requests deterministic decoding (temperature 0 + fixed seed) where the `Reasoner` seam supports it. The cache is a **convergence/efficiency aid, never an authorization source** — confirm re-gates the bytes through M6a (`get_proposal_bytes_checked`, `log.rs:2093`), exactly as for `proposal_bytes`.

### 5.3 Lineage (engine-gathered D8 — the anti-laundering rule)

New pure-ish helper `mandate_lineage(mandate_id: &str, source_ids_read: &[String]) -> Result<Vec<String>, BossclawError>`, mirroring `reconciliation_lineage` (`log.rs:3412`): `union({mandate_id}, source_ids_read)` then `sort` + `dedup`. **The model's citations are never consulted.** Callers MUST propagate `Err` (never `unwrap_or_default`) — swallowing would launder taint to an empty lineage. The `source_ids_read` are precisely the `file_event_id`s of the sources the engine read in §5.1 step 1 — not the over-reaching entity graph, not the model's word.

This lineage feeds `WriteProposal.source_event_ids` (gate-rejected if empty, `actuator.rs:35-50`) and the recorded `write_proposal` event's `model_meta.source_event_ids`, so the chokepoint stamps external taint (§4.3 rule 2).

### 5.4 Idempotency + decline-stickiness (the no-spam rule)

Idempotency key (distinct JSON shape so it shares no namespace with M6b's `{src,relation,dst}`, A2/§8):
```json
{ "mandate": "<mandate_id>", "target": "<canonical>", "sources_hash": "<hash of the sorted (path,content_hash) source set>" }
```
Keying on the **source-state** (not the expected-output hash) is deliberate: the trigger for a sync *is* "the sources changed," and it sidesteps LLM nondeterminism — a fixed source-state maps to exactly one key even if re-synthesis would yield slightly different bytes.
M6c uses a **new** suppression predicate `is_mandate_proposal_suppressed(target, key)` (the existing `is_proposal_suppressed`, `log.rs:2010`, suppresses only *open* + *rejected*, **not declined**). M6c's predicate suppresses if there exists, for this exact `(target, key)`, any of:
- an **open** `write_proposal` (unresolved) — don't double-ask;
- a `write_rejected` — terminal failure (existing rule);
- a **`write_declined`** — **the new rule:** if you said "not now" to the sync for *this source-state*, M6c will not re-propose it until the sources change (a new `sources_hash` → a fresh ask). (A source-state that recurs after a decline stays declined — calm by default; revoke+re-grant resets.)

Post-*confirm* convergence does not even reach suppression: with the cached expected (§5.2), once the file equals the cached bytes the compare yields no candidate at all.

Cap-elision and the off-switch emit **no** event, so they remain retryable (never suppress) — carrying M6b's lesson exactly (`log.rs:2034-2036`).

### 5.5 Caps + off-switch

- **Global per-tick cap (shared):** the mandate phase shares `MAX_PROPOSALS_PER_TICK=8` via `report.proposals_emitted` (`extract.rs:75`, `log.rs:5176`) so M6b + M6c together never exceed 8 proposals/tick.
- **Per-mandate daily cap (new):** before emitting, count this mandate's `write_proposal`s (producer `m6c-mandate-proposer`, this `mandate_id`) in the trailing 24h by event `ts`; if `>= M.max_proposals_per_day`, **elide** (emit nothing, stays retryable) + `report.proposals_elided_cap += 1`.
- **Off-switch (new, D8):** `mandates_enabled` — a sticky, fail-closed, default-open `config` event mirroring `proposals_enabled` (`set/get` at `log.rs:4291`/`:4327`). Independent of M6b's `proposals_enabled`. Read once per tick.

### 5.6 Producer + events reused

New producer const `M6C_PROPOSER_PRODUCER = "m6c-mandate-proposer"` (`graph.rs`, beside `M6B_PROPOSER_PRODUCER` at `graph.rs:91`). The proposal-record helpers `append_write_proposal` (`log.rs:1966`) + `append_write_rejected` (`log.rs:1982`) are **generalized to take a producer** (today they hardcode `m6b-reconciler` via `build_m6b_event`, `log.rs:2127`; M6c passes `M6C_PROPOSER_PRODUCER`) — a small additive change, *not* a rewrite, and the event shapes are unchanged. M6c **reuses unchanged**: `decline_write_proposal` (`log.rs:1992`), `put_proposal_bytes`/`get_proposal_bytes_checked` (`log.rs:2063`/`:2093`), `propose_write` (`log.rs:2368`), `execute_write_resolving` (`log.rs:2598`), `file_written` (`graph.rs:72`). Best-effort isolation: any per-mandate failure is `log::warn!`-ed and the loop continues — never unwinds committed graph/summarize work (mirrors M6b, `log.rs:5048-5057`).

---

## 6. The Watcher + self-driver (Piece 2)

### 6.1 + 6.2 notify-based watcher → debounce → drive

A new `#[cfg(unix)]` module (e.g. `watch.rs`) using the `notify` crate watches the **active read-grant source roots** (the ingest scope). On filesystem events it debounces via `debounce_due` (`evolve.rs:88`, `EVOLVE_DEBOUNCE_MS=2000`) and then runs one **drive step**:
```
ingest_all(router, embedder)   // incremental: dedup/supersede picks up changed sources (ingest.rs:582)
evolve_once(embedder, reasoner) // runs all phases incl. the new mandate phase
```
The self-driver is a *simple* long-lived loop (a thread that owns the watcher receiver + debounce timer). It is the minimal version of what `evolve.rs:19-22` defers to M7; **M7 still owns** the idle/charging/thermal-smart scheduler + `EvolveStatus` wiring.

### 6.3 Single-writer discipline

The watcher thread **never opens a second writer**. It calls the existing `EventLog` methods, which serialize on the inner mutex (`log.rs:133`) and `rename_lock` (`log.rs:2642`). One consumer, one writer — exactly the daemon "one-puller" discipline from the messaging stack.

### 6.4 The three new dangers (created by pulling the Watcher in)

1. **Self-write → watcher feedback loop** 🌀 (HIGH; A6/§8). `execute_write` mutates the target but does **not** re-ingest; a live watcher could see the engine's own write as a change and re-fire forever. **Three guards, layered:**
   - *Primary (convergence):* the proposer compares the **cached** expected (synthesized once per source-state, §5.2) vs the target's **on-disk bytes** (§5.1 step 3). After a confirmed write, `actual == cached expected` → the next tick proposes nothing. Because expected is cached (not re-synthesized while sources are unchanged), the fixpoint holds **even if the model is nondeterministic** — bounded to one extra no-op tick.
   - *Structural:* the target is excluded from `source_scope` and **should live outside the watched read-source roots** (output ≠ input) — then the engine's write doesn't even fire a source event.
   - *Belt:* the drive step ignores watcher events whose path matches a `file_written` target committed in the last debounce window.
2. **Event storm / DoS** 🌊. A noisy editor or a malicious bulk-write fires thousands of events. **Bounded by:** debounce coalescing + ingest's existing wall-clock/entry/never-touch budgets (`ingest.rs:44-71`) + `EVOLVE_BATCH` + `MAX_PROPOSALS_PER_TICK`. The watcher feeds a **bounded** channel; overflow drops to a "rescan" flag rather than growing unboundedly.
3. **Confirm spam** 🔔. A hands-off proposer that pipes up too often trains rubber-stamping. **Bounded by:** idempotency + decline-stickiness (§5.4) + per-mandate daily cap + global per-tick cap (§5.5).

---

## 7. Security model

M6c is the **widest confused-deputy surface** in the whole engine: a booby-trapped source file, the instant it lands, is auto-ingested, auto-evolved, and fed to a goal-directed proposer with no human poke. A malicious file's aim becomes **hijacking the mandate** (steering the recipe synthesis toward an attacker's bytes). Defense-in-depth — **every control required, none load-bearing alone:**

| Control | What it stops | Where |
|---|---|---|
| **Mandate can't widen write-grants** — gate re-checks `is_write_allowed(target)` every propose | a mandate reaching a file you never write-granted | §4.3 #1 (`log.rs:2276`) |
| **Mandate can't shed taint** — engine-gathered lineage + target-anchored taint, never model cites | a tainted source laundered into a "clean" write | §4.3 #2, §5.3 |
| **Fenced synthesis** — recipe trusted-frame-only (Unicode/bidi-sanitized), sources fenced as data | a source file *commanding* the synthesis (direct injection) | §5.2 |
| **Human confirm, every write; loud modal if tainted** | any autonomous write landing without a human; a tainted write being silent | §3 ⑤ (unchanged from M6a/M6b) |
| **Never Delete (D7)** | the most destructive op being autonomous | §5.1 |
| **Volume bounds** — per-tick + per-mandate/day + idempotency + decline-stickiness | proposal floods → confirm-fatigue → rubber-stamping | §5.4–5.5 |
| **Independent sticky off-switch** | inability to kill the widest-surface proposer fast | §5.5 (D8) |
| **Convergent compare-vs-disk** | a self-write feedback loop | §6.4 #1 |

**The load-bearing proofs (must be tests; dual adversarial review on ★):**
1. ★ **Cannot widen grant:** grant mandate → revoke target's write-grant → tick → gate rejects, **no `file_written`**, a `write_rejected` recorded.
2. ★ **Cannot shed taint:** mandate with an external source → emitted `write_proposal` carries the source `file_ingested` id, is stamped `origin:"external"`, verdict `taint=Untrusted` + `requires_loud_modal`.
3. ★ **Lineage is engine-gathered:** a scripted reasoner that *tries to cite* a forbidden id → that id is **absent** from `source_event_ids`; the set == `{mandate_id} ∪ {actual sources read}`.
4. **Convergence (no self-loop), incl. a nondeterministic model:** after a confirmed write with unchanged sources, the next tick proposes nothing. Test with a reasoner that returns **different bytes each call** to prove convergence comes from the per-source-state cache (§5.2), not from any LLM-determinism assumption.
5. **Decline-stickiness:** decline a sync proposal → next tick (unchanged sources) proposes nothing for that `content_hash`; then change sources → proposes the new version.
6. **Caps:** per-mandate/day + global per-tick elide (emit nothing, stay retryable — no permanent suppress).
7. **Off-switch:** `mandates_enabled=false` → mandate phase emits nothing; sticky + fail-closed.
8. **Event-storm bounded:** a burst of N watcher events coalesces to a bounded number of ticks/proposals.
9. **Live-Ollama oracle** (`#[ignore]`): real `qwen2.5:7b` drives a real sync end-to-end (a source changes → a grounded, in-sync proposal), like M4a/M6b.

---

## 8. Data model (new + reused)

**New events** (all additive — `Event` is a generic struct, *no* enum exhaustiveness; A10/§8): `mandate_grant`, `mandate_revoke` (ground-truth, `model_meta:None`); a `mandates_enabled` `config` control event.
**New projection:** `mandates` table + `Mandate` read type (mirror `write_grants`/`WriteGrant`).
**New side-table:** `mandate_synthesis_cache (mandate_grant_id, sources_hash) → (expected_hash, expected_bytes BLOB, created_at)` — encrypted in the `Store`; the convergence/efficiency cache (§5.2), re-gated at confirm, **never an authorization source**.
**New consts:** `MANDATE_GRANT_EVENT_TYPE`, `MANDATE_REVOKE_EVENT_TYPE`, `M6C_PROPOSER_PRODUCER`, `MANDATES_ENABLED_KEY` (`graph.rs`/`log.rs`).
**Files to touch to add the event types (A10):** `graph.rs` (consts + `Mandate` type), `log.rs` (writers near `:2213`; `CREATE TABLE` near `:335-413`; fold in `rebuild_graph` near `:3692`; `active_mandates`/`is_mandate_proposal_suppressed`/per-mandate-cap queries; generalize `append_write_proposal`/`append_write_rejected` to take a producer, `:1966`/`:1982`), `lib.rs` (re-export `Mandate`), `evolve.rs` (`EvolveReport` already has `proposals_*` counts — reuse), plus the new `mandate.rs`/`watch.rs` modules. **No change** to `event.rs` (generic struct), no signing/serialization match.
**Reused unchanged:** `write_proposal` / `write_rejected` / `write_declined` / `file_written`; `propose_write` / `execute_write_resolving` / `undo_write`; `proposal_bytes` side-table; `push_fenced_source`; `sanitize_ident`; `is_write_allowed`; the append chokepoint + `reject_empty_tier_b`.
**New dependency:** `notify` (FSEvents/inotify). Its internal `unsafe` is in a separate crate — **does not** violate this crate's `#![forbid(unsafe_code)]`. Verify no feature pulls `unsafe`/extra weight into bossclaw-core; every new `pub` item needs a doc comment (`#![deny(missing_docs)]`).

---

## 9. Build sequence (two layers; all subagents Opus)

Each layer: spec-task → implement → per-task review → integration. Then a whole-impl Opus SHIP and PR. Per Peter's standing directive, **every subagent runs on Opus** (`use-opus-for-all-subagents`).

1. **Layer 1 — the Lister (security-critical, built + reviewed first, in isolation):**
   - 1a: mandate primitive — events, `mandates` projection, `add_mandate`/`revoke_mandate`/`active_mandates`, grant-time write-grant guard, `mandates_enabled` switch.
   - 1b: `build_recipe_prompt` + `recipe_schema` + `mandate_lineage` (pure; the synthesis + anti-laundering).
   - 1c: the `evolve_once` mandate phase — gather/synthesize/compare/suppress/cap/emit, compare-vs-disk, per-mandate cap, decline-sticky suppression.
   - **→ dual adversarial security review** on proofs 1/2/3 (cannot-widen, cannot-shed-taint, engine-gathered-lineage), distinct lenses.
2. **Layer 2 — the Watcher + self-driver:** `watch.rs` (`notify`, debounce, bounded channel, single-writer drive step, self-write suppression). **→ its own simpler review** (storm bounding + single-writer + self-loop convergence).
3. **Whole-impl Opus SHIP** (cross-layer integration probes: convergence, off-switch, caps, taint end-to-end) → PR.

---

## 10. Test plan

- **Hermetic** (`tempfile::tempdir()` + in-memory `EventLog` + `MockEmbedder` + a scripted-delegating `Reasoner` that answers the synthesis turn, the M6b `tests/reconcile.rs` pattern): all proofs 1–8 (§7), plus mandate grant/revoke/expiry, Create-vs-Edit, source-prefix filtering + target exclusion, input-cap truncation, fenced-marker breakout attempt.
- **Live-Ollama** (`#[ignore]`, `tests/live_ollama.rs`): proof 9 — real model, real file synced end-to-end + idempotent + supersede-on-source-change.
- **Gates:** `cargo test -p bossclaw-core`; `clippy -D warnings` (default + `ollama`); `cargo build` (proves `deny(missing_docs)`); taint chokepoint **byte-unchanged**; `forbid(unsafe)` intact; `notify` is the only new dep.

---

## 11. Non-goals (v1)

- **Managed-section sync** inside a hand-edited file (D2: whole-file only).
- **Free-form goal pursuit** (Approach B) — deferred as a possible future brick.
- **Mandate types beyond "keep a file in sync"** (polish, living-record, etc.).
- **M7's battery/thermal-smart scheduler** + `EvolveStatus` wiring — M6c uses a *simple* loop.
- **The desktop confirm/preview UI** — app-side, per the engine-only boundary (program design D3).
- **Windows** write/watch path (D6).

---

## 12. Runway open questions — resolved

1. **Mandate schema / signing:** §4.1 — `mandate_grant`/`mandate_revoke` ground-truth events + `mandates` projection, mirroring write-grants.
2. **Grant / scope / revoke:** §4.2 — engine primitive + app UX; scope is a write-granted subtree; sticky fail-closed revoke.
3. **The general trigger (the hard part):** §5.1 — recipe-compare against on-disk bytes; computable goal-state, no contradiction needed.
4. **Volume bounding:** §5.4–5.5 — per-tick (shared) + per-mandate/day + idempotency + decline-stickiness.
5. **Composition with taint + grants:** §4.3 + §7 proofs 1–3 — proven a mandate can neither widen a grant nor shed taint; dual review.
6. **Lineage for a goal-directed proposal:** §5.3 — `{mandate_id} ∪ {sources read}`, engine-gathered, never model cites.

---

## 13. Cross-links

`docs/superpowers/specs/2026-06-20-m6-actuator-program-design.md` (§5 M6c) · `…m6a-safe-hands-design.md` · `…m6b-reconciliation-proposer-design.md` · GBrain `air/m6c-mandate-proposer-prep` (runway) · `air/lessons-learned-canonical` (D8 generalization; Unicode/bidi sanitize; `write_rejected` permanence; verify-existing-code) · `air/forever-companion-architecture` ("acts on your behalf under a Mandate") · `air/vault-brain-architecture` (M6 = brick 2; M6c is its last brick). Seam-map source-of-truth: this spec's file:line refs, verified against merged `main` 2026-06-21.
