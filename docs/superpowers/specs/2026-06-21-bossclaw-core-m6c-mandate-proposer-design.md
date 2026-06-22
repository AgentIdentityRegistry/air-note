# bossclaw-core M6c — General + Mandate Proposer (Design)

- **Date:** 2026-06-21
- **Status:** **Rev 2 — design approved by Peter; independent critic + security review folded 2026-06-21.** Both reviewers returned **SHIP-WITH-FIXES** (no redesign; the seam-map verified with **zero stale refs** across ~25 spot-checks — the M4b false-assertion regression did not recur). All Critical + Important findings folded (changelog §14). Pending: implementation plan → subagent-driven build (all Opus; dual adversarial review on the Lister) → PR.
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
  "target":       "<canonical path>",           // brain-owned file (whole-file owned); MUST be outside every read-grant root (§4.2)
  "source_scope": "<canonical dir prefix>",      // a SINGLE subtree it derives from; MUST exclude target
  "recipe":       "<plain-English rule>"         // user-authored; e.g. "an index of every file's title + a one-line summary"; ≤ MAX_RECIPE_LEN
}
```
**Mandate identity = the `mandate_grant` event id** (assigned by the append chokepoint, exactly as a file record's identity is its `file_event_id`) — *not* a content-supplied ULID. This is load-bearing for taint: the identity is then a **real, readable ground-truth event id**, so including it in a proposal's `source_event_ids` (§5.3) contributes no taint and never trips the chokepoint's fail-closed "unreadable source ⇒ external" rule (`source_is_external_in_tx`, `log.rs:532`). A free-standing content ULID would be unreadable as an event → would wrongly stamp **every** mandate proposal external.

`mandate_revoke` event: content `{ "mandate_grant_id": "<event id>" }` — sets `revoked=1` in the projection (**sticky + fail-closed**, like `write_revoke`).

Projection `mandates` (new table, folded in `rebuild_graph` beside the `write_grants` fold at `log.rs:3692`):
`(mandate_grant_id TEXT PK, target TEXT, source_scope TEXT, recipe TEXT, granted_at TEXT, revoked INTEGER DEFAULT 0)`. Throughout this spec, **`mandate_id` is shorthand for this `mandate_grant_id`.** (Timed expiry is **deferred** — Rev 2 / finding C: it needs the same untestable wall-clock seam as the dropped daily cap; revoke is instant + sticky, so v1 mandates run until revoked.)
Read type `Mandate` in `graph.rs` (re-exported in `lib.rs`). Reader `active_mandates() -> Vec<Mandate>` (revoked=0).

The `recipe` is **user-authored and engine-trusted**, but is still sanitized into the prompt's trusted frame (§5.2) for defense-in-depth. The mandate is **not** embeddable/recallable (left out of `EMBEDDABLE_EVENT_TYPES`, `log.rs:143`, exactly like `file_written`).

### 4.2 Grant / revoke

- App-side UX grants/revokes; the engine exposes `add_mandate(...)` / `revoke_mandate(mandate_grant_id)` mirroring `add_write_grant`/`revoke_write_grant`. `target` and `source_scope` are **canonicalized at grant** (resolve `..`/symlinks) so all later `Path::starts_with` checks are on real paths.
- **Grant-time guard (UX):** `add_mandate` refuses if `target` is not currently under an active **write**-grant (`is_write_allowed(target)`, `log.rs:2276`) — you cannot create a mandate the brain could never act on. This is a convenience check, **not** the security boundary (the load-bearing check is `propose_write`'s re-gate, §4.3 #1).
- **Grant-time guard (LOAD-BEARING — convergence, Rev 2 / finding A):** `add_mandate` **MUST reject** if the canonical `target` resolves under **any active read-grant root** (the watched source roots, §6.1). This is *enforced*, not a "should." It structurally guarantees the engine's own confirmed write to `target` can never fire a source watcher event or be re-ingested as a source — so the recipe can never fold its own output back into its input, and the convergence fixpoint (§6.4 #1) holds unconditionally. Test: a mandate whose target is under the read root → `add_mandate` rejects.
- **Recipe cap at grant (Rev 2 / finding D):** `add_mandate` rejects a `recipe` longer than `MAX_RECIPE_LEN` (≈2048 bytes) rather than letting it be silently truncated at synth time — so the signed `mandate_grant` recipe and the prompt's recipe can never disagree.

### 4.3 The two iron rules (load-bearing)

1. **A mandate NEVER widens a write-grant.** The *load-bearing* enforcement is at **propose time**: every proposal goes through `propose_write`, whose step 2 calls `is_write_allowed(target)` (`log.rs:2368` / `:2276`). If the target's write-grant was revoked after the mandate was granted, the gate returns `allowed=false` with **no** `reject_reason` (the never-widen signal is `allowed` alone; `WriteVerdict::gate_reject_reason()` folds `!allowed` into a reject so both M6b and M6c honor it) → M6c records `write_rejected`, **no write**. Granting a mandate ≠ granting write; revoking the write-grant makes the mandate inert. (Grant-time check in §4.2 is only UX.)
2. **A mandate NEVER sheds taint.** Two engine-anchored enforcement points, neither trusting the model:
   - The proposal's `source_event_ids` is **engine-gathered** = `{mandate_id} ∪ {the file_ingested ids of the sources the engine actually read}` (§5.3). Any external source → the append chokepoint `append_event_in_tx` stamps `origin:"external"` on the `write_proposal` (`log.rs:562-570`).
   - `propose_write` step 4 independently re-derives taint from the **target's** own provenance via path/`(dev,ino)` (`log.rs:2465-2499`) — so even a mandate that (mis)targets a tracked ingested file is marked `Untrusted` + loud.

---

## 5. The general proposer (the Lister — Piece 1b)

### 5.1 Trigger: recipe-compare (computable expected-state)

Runs as a **new top-level phase in `evolve_once`**, placed as a new step **after the summarize phase (step 9, `log.rs:5100`)** (Rev 2 / minor M1 — the earlier `log.rs:5091` `rebuild_graph` is *conditional* and may not run on a pure mandate tick). Projection staleness is irrelevant to the compare because §5.1 step 3 reads the target's **on-disk bytes**; the phase relies only on the caller (the watcher drive step, §6.2) having run `ingest_all` first so the source `file_event_id`s used for the lineage are fresh. **Invariant (minor M2): the lineage must cite the same source-state the bytes were synthesized from** — guaranteed by ingest-before-evolve in the watcher path, and by the synth-time-lineage stored in the cache (§5.2) on a cache hit. Gated by the new `mandates_enabled` switch (re-read per mandate, §5.5) **and** the existing evolve off-switch (if evolve is off, nothing runs).

For each active mandate `M`:
1. **Gather sources:** `current_files()` (`log.rs:3322`) filtered by **segment-aware** descent under `M.source_scope` (path-component `starts_with`, *not* raw-string prefix — the same discipline as `is_write_allowed`, so `/a/b` never matches `/a/bc`) **and excluding `M.target`** (a recipe's output is never its own input). *(Note: no path-prefix query exists — filter `current_files()` in-engine; A6/§8.)* Read each source's bytes from disk; collect their `file_event_id`s.
2. **Compute expected — cached per source-state (§5.2):** form `sources_hash` = hash of the sorted `(path, content_hash)` set of the in-scope sources. If the synthesis cache holds `(M.mandate_id, sources_hash)`, **reuse the cached `expected_bytes`** (no LLM call); else `expected_bytes = reasoner(build_recipe_prompt(M.recipe, fenced sources))` (deterministic mode) and cache it. `expected` is thus a **stable function of the source-state**, recomputed only when sources change.
3. **Read actual:** `actual = std::fs::read(M.target)` — the **on-disk bytes** (or absent → op = Create). **Never the `files` projection `content_hash`**, which is stale after an actuator write (A6/§8). Comparing the **cached** expected against on-disk actual is what makes the loop *converge* (§6.4) — robustly, even if the model is nondeterministic.
4. **Decide:** if `expected != actual` → candidate proposal; else nothing.
5. **Suppress / cap / emit** (§5.4, §5.5): if not suppressed and under caps, build the lineage (§5.3), call `propose_write`, and on `allowed` record `append_write_proposal` + `put_proposal_bytes`; on gate-reject record `append_write_rejected`.

**Op selection (D7):** target missing → `Create`; target exists → `Edit`. **Never `Delete`.**

### 5.2 Synthesis (fenced — reuse M6b's posture)

New pure helper `build_recipe_prompt(recipe: &str, fenced_sources: &str) -> String` in `reconcile.rs` (or a new `mandate.rs`), mirroring `build_rewrite_prompt` (`reconcile.rs:106`):
- **Trusted frame** holds the sanitized `recipe`. **The recipe gets its OWN sanitize path (Rev 2 / finding D), not `sanitize_ident`** — that helper's 200-byte `MAX_PROMPT_IDENT_LEN` is for short entity labels and would silently truncate a multi-clause rule. Factor the bidi/control char-filter out of `sanitize_ident` (`summarize.rs:167`) into a shared `strip_bidi_controls` (single-sourced policy: all 12/12 bidi controls + separators) and apply it to the recipe with the recipe-appropriate `MAX_RECIPE_LEN`. Over-cap recipes are rejected at *grant* (§4.2), never truncated at synth — so the trusted instruction is exactly what the user signed. The recipe is the *only* instruction.
- **Untrusted data** = each source file body, fenced via `crate::extract::push_fenced_source` (`extract.rs:185`) — the exact fence M6b uses; embedded `<<<SOURCE_*>>>` markers neutralized with ZWSP. **Bounds (Rev 2 / finding E):** the count of in-scope sources is capped (mirror `MAX_ENTITIES_PER_MEMORY`, `extract.rs:61`) so a directory-bomb under `source_scope` can't blow up gather/hash/synth each tick. If the combined fenced sources would exceed `MAX_INPUT_TEXT_BYTES=16_384` (`extract.rs:95`), the engine does **NOT silently truncate** — it **elides** (emits nothing, stays retryable, surfaces a "mandate scope too large" status). Silent truncation under an autonomous proposer both desyncs the cache key from the fed bytes and proposes a confidently-wrong file from partial input.
- **Output schema** `recipe_schema()` requires exactly one string `synced_content`, `additionalProperties:false` (mirrors `rewrite_schema`, `reconcile.rs:127`). `expected_bytes = synced_content.into_bytes()`. **Degenerate cases (Rev 2 / findings G, I4):** an **empty in-scope source set** → **elide** (no LLM call, no proposal — never synthesize content from nothing). An **empty `synced_content`** → `write_rejected` ("empty synthesis"), mirroring M6b's empty-rewrite reject — never auto-truncate the target to zero bytes.

The model reads fenced data + a trusted recipe and emits the whole derived file — identical trust posture to M6b's whole-file rewrite, then gated + human-confirmed.

**Deterministic + cached synthesis (load-bearing for convergence).** An LLM is not bit-exact across calls, so re-synthesizing every tick would make `expected` drift and the proposer would churn even with the file already in sync (a self-loop). The engine therefore **synthesizes once per source-state and caches the bytes**, keyed on `(mandate_id, sources_hash)` in an encrypted `mandate_synthesis_cache` table (§8); later ticks reuse the cached bytes for the compare and skip the LLM entirely until the sources change. Synthesis also requests deterministic decoding (temperature 0 + fixed seed) where the `Reasoner` seam supports it. The cache is a **convergence/efficiency aid, never an authorization source** — confirm re-gates the bytes through M6a (`get_proposal_bytes_checked`, `log.rs:2093`), exactly as for `proposal_bytes`.

**The cache row also stores the synth-time lineage (Rev 2 / finding B — CRITICAL taint fix).** A cache hit reuses bytes that were synthesized from the sources present *at synth time*; if it then re-derived the lineage from the *current* in-scope sources, a tainted source that left scope (moved/deleted/superseded) between synth and the cache hit would drop out of the lineage while its influence stays baked into the cached bytes → the proposal would be stamped **clean** (no `origin:"external"`, no loud modal) = laundered. So the cache row carries `source_event_ids_at_synth` (the exact `file_event_id`s read during synthesis), and on a cache hit the proposal's lineage (§5.3) is **`source_event_ids_at_synth ∪ current in-scope ids`**. Taint is monotone, so the union can only *add* an external source, never drop one — the bytes and the provenance that produced them can never travel separately. **Eviction (finding F):** on cache-write, delete the mandate's prior rows (`DELETE … WHERE mandate_grant_id=?1 AND sources_hash<>?2` — only the current source-state's bytes are ever needed); on `revoke_mandate`, purge all of that mandate's cache rows. So the cache cannot grow unboundedly under a live watcher.

### 5.3 Lineage (engine-gathered D8 — the anti-laundering rule)

New pure-ish helper `mandate_lineage(mandate_id: &str, source_ids: &[String]) -> Result<Vec<String>, BossclawError>`, mirroring `reconciliation_lineage` (`log.rs:3412`): `union({mandate_id}, source_ids)` then `sort` + `dedup`. **The model's citations are never consulted.** Callers MUST propagate `Err` (never `unwrap_or_default`) — swallowing would launder taint to an empty lineage. `source_ids` are engine-gathered `file_event_id`s — not the over-reaching entity graph, not the model's word — and specifically: **on a fresh synth**, the ids the engine read in §5.1 step 1; **on a cache hit**, `source_event_ids_at_synth ∪ current in-scope ids` (§5.2 finding B), so the lineage always covers every source whose bytes shaped `expected`.

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
- **Per-mandate per-tick cap (Rev 2 / finding C — replaces the wall-clock daily cap):** a mandate emits at most `MAX_PROPOSALS_PER_MANDATE_PER_TICK` (default 1) proposals per tick; the rest elide (`report.proposals_elided_cap += 1`). This is **deterministic and hermetically testable** (no clock seam), unlike a "trailing 24h" window — `ts` is wall-clock `Utc::now()` at the chokepoint (`log.rs:575`) with no injection point, so a timed cap could not be unit-tested (the exact "claimed-but-untested safety control" the review exists to stop). Long-run volume is already bounded structurally by **idempotency** (one proposal per distinct `sources_hash`, §5.4) + **decline-stickiness**, so a timed cap is redundant; cut per YAGNI. When the per-mandate cap query *does* need to attribute proposals to a mandate, it counts `write_proposal` events where **`content.inducing_key.mandate == M.mandate_id`** (via `events_of_types(&[WRITE_PROPOSAL])`, `log.rs:4383`) — there is no top-level `mandate_id` field; the producer string alone cannot separate mandates (all M6c proposals share one producer).
- **Off-switch (new, D8):** `mandates_enabled` — a sticky, fail-closed, default-open `config` event mirroring `proposals_enabled` (`set/get` at `log.rs:4291`/`:4327`). Independent of M6b's `proposals_enabled`. **Re-read per-mandate** within the phase (a cheap projection read, security-finding M1) so flipping the kill switch bounds a runaway to one mandate, not a whole tick — the widest-surface proposer must be killable fast.

### 5.6 Producer + events reused

New producer const `M6C_PROPOSER_PRODUCER = "m6c-mandate-proposer"` (`graph.rs`, beside `M6B_PROPOSER_PRODUCER` at `graph.rs:91`). **Producer generalization (Rev 2 / finding C):** rename the shared builder `build_m6b_event` (`log.rs:2127`) → `build_proposer_event(producer: &str, …)` and thread `producer` through **all three** record helpers that route through it — `append_write_proposal` (`log.rs:1966`), `append_write_rejected` (`log.rs:1982`), **and `decline_write_proposal` (`log.rs:1992`)** (the critic flagged that the third was missed; it currently stamps a *declined-M6c-proposal* event as `m6b-reconciler`). M6b call sites pass `M6B_PROPOSER_PRODUCER`; M6c passes `M6C_PROPOSER_PRODUCER`. Event shapes are unchanged. The per-mandate-cap test (§7 proof 6) MUST assert the *recorded* `model_meta.model_id == "m6c-mandate-proposer"` **and** that the cap elides — else the test passes while the cap counts zero. M6c **reuses unchanged**: `put_proposal_bytes`/`get_proposal_bytes_checked` (`log.rs:2063`/`:2093`), `propose_write` (`log.rs:2368`), `execute_write_resolving` (`log.rs:2598`), `file_written` (`graph.rs:72`). Best-effort isolation: any per-mandate failure is `log::warn!`-ed and the loop continues — never unwinds committed graph/summarize work (mirrors M6b, `log.rs:5048-5057`).

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

The watcher thread **never opens a second writer**. It holds an **`Arc<EventLog>` clone of the one existing instance** (Rev 2 / security I4) — it must NOT construct or clone a second `Store`/SQLCipher connection over the same DB file (that would be a second writer and break the single-connection assumption). It calls the existing `EventLog` methods, which serialize on the inner mutex (`log.rs:133`) and `rename_lock` (`log.rs:2642`). One consumer, one writer — exactly the daemon "one-puller" discipline from the messaging stack. Add an assert/test that the watcher path shares the single handle.

### 6.4 The three new dangers (created by pulling the Watcher in)

1. **Self-write → watcher feedback loop** 🌀 (HIGH; A6/§8). `execute_write` mutates the target but does **not** re-ingest; a live watcher could see the engine's own write as a change and re-fire forever. **Three guards, layered:**
   - *Primary (convergence):* the proposer compares the **cached** expected (synthesized once per source-state, §5.2) vs the target's **on-disk bytes** (§5.1 step 3). After a confirmed write, `actual == cached expected` → the next tick proposes nothing. Because expected is cached (not re-synthesized while sources are unchanged), the fixpoint holds **even if the model is nondeterministic** — bounded to one extra no-op tick.
   - *Structural (now MANDATORY, Rev 2 / finding A):* the target **must live outside every active read-grant root**, *enforced at `add_mandate`* (§4.2) — not a "should." The grant-time check resolves an existing target's **leaf symlink** before the segment-aware `starts_with` scan, so a symlinked target that really lives in a read root is rejected at grant (a tight early-reject). Convergence is then a **layered** guarantee: the grant-time guard rejects the footgun config up front, and the *ultimate* enforcers are execute-time `O_NOFOLLOW` (the engine can never write *through* a leaf symlink into a read root) + canonical-root-anchored ingest (only files reached by descending the canonical grant root are walked as sources). So the engine's own confirmed write can never be re-ingested as a source, and the recipe can never fold its output into its input. This demotes the Belt guard below to true belt-and-suspenders. *(Security review, Task 2: the grant-time `starts_with` alone is defense-in-depth, not the sole enforcer.)*
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
1. ★ **Cannot widen grant — propose-time AND execute-time:** (a) grant mandate → revoke target's write-grant → tick → gate rejects, **no `file_written`**, `write_rejected` recorded; (b) revoke the write-grant **between propose and confirm** → `execute_write_resolving`'s re-check rejects, no write (the load-bearing boundary is execute-time, `log.rs:2683`).
2. ★ **Cannot shed taint:** mandate with an external source → emitted `write_proposal` carries the source `file_ingested` id, is stamped `origin:"external"`, verdict `taint=Untrusted` + `requires_loud_modal`.
3. ★ **Lineage is engine-gathered:** a scripted reasoner that *tries to cite* a forbidden id → that id is **absent** from `source_event_ids`; the set == `{mandate_id} ∪ {actual sources read}`.
3b. ★ **Cache-hit lineage covers synth-time taint (finding B):** synth with an external source in scope (caching its bytes + `source_event_ids_at_synth`), then move/delete that source out of scope and force a **cache hit** → the emitted proposal STILL carries the departed source's `file_ingested` id and is `origin:"external"` + loud. (This test fails against the pre-Rev-2 design — it is the regression guard for the taint-laundering Critical.)
4. **Convergence (no self-loop), incl. a nondeterministic model:** after a confirmed write with unchanged sources, the next tick proposes nothing. Use a reasoner that returns **different bytes each call** to prove convergence comes from the per-source-state cache (§5.2), not from any LLM-determinism assumption.
5. **Decline-stickiness:** decline a sync proposal → next tick (unchanged sources) proposes nothing for that `sources_hash`; then change sources → proposes the new version.
6. **Caps (hermetic, no clock):** per-mandate **per-tick** + global per-tick elide (emit nothing, stay retryable — no permanent suppress); assert the emitted proposal's recorded `model_meta.model_id == "m6c-mandate-proposer"` so a dead cap can't pass silently.
7. **Off-switch:** `mandates_enabled=false` → mandate phase emits nothing; sticky + fail-closed; flipping it mid-phase stops further mandates (per-mandate re-read).
8. **Event-storm bounded:** a burst of N watcher events coalesces to a bounded number of ticks/proposals.
9. **Structural self-loop guard (finding A):** `add_mandate` **rejects** a target under any active read-grant root; and a mandate whose target is legitimately outside the roots does not re-ingest its own confirmed write as a source.
10. **Elide/reject degenerate cases (findings E, G):** empty in-scope source set → elide; over-cap combined sources → elide (retryable, *not* `write_rejected`); empty `synced_content` → `write_rejected`; segment-aware `source_scope` (`/a/b` never matches `/a/bc`).
11. **Live-Ollama oracle** (`#[ignore]`): real `qwen2.5:7b` drives a real sync end-to-end (a source changes → a grounded, in-sync proposal), like M4a/M6b.

---

## 8. Data model (new + reused)

**New events** (all additive — `Event` is a generic struct, *no* enum exhaustiveness; A10/§8): `mandate_grant`, `mandate_revoke` (ground-truth, `model_meta:None`); a `mandates_enabled` `config` control event.
**New projection:** `mandates` table + `Mandate` read type (mirror `write_grants`/`WriteGrant`).
**New side-table:** `mandate_synthesis_cache (mandate_grant_id, sources_hash) → (expected_hash, expected_bytes BLOB, source_event_ids_at_synth BLOB, created_at)` — encrypted in the `Store`; the convergence/efficiency cache (§5.2), re-gated at confirm, **never an authorization source**. `source_event_ids_at_synth` is the synth-time lineage (Rev 2 / finding B) the cache hit unions into the proposal lineage. **Eviction (finding F):** delete a mandate's prior rows on cache-write (keep only the current `sources_hash`) and purge all of a mandate's rows on `revoke_mandate` — bounded under a live watcher.
**New consts:** `MANDATE_GRANT_EVENT_TYPE`, `MANDATE_REVOKE_EVENT_TYPE`, `M6C_PROPOSER_PRODUCER`, `MANDATES_ENABLED_KEY` (`graph.rs`/`log.rs`).
**Files to touch to add the event types (A10):** `graph.rs` (consts + `Mandate` type), `log.rs` (writers near `:2213`; `CREATE TABLE` near `:335-413`; fold in `rebuild_graph` near `:3692`; `active_mandates`/`is_mandate_proposal_suppressed`/per-mandate-cap queries; generalize `append_write_proposal`/`append_write_rejected` to take a producer, `:1966`/`:1982`), `lib.rs` (re-export `Mandate`), `evolve.rs` (`EvolveReport` already has `proposals_*` counts — reuse), plus the new `mandate.rs`/`watch.rs` modules. **No change** to `event.rs` (generic struct), no signing/serialization match.
**Reused unchanged:** `write_proposal` / `write_rejected` / `write_declined` / `file_written`; `propose_write` / `execute_write_resolving` / `undo_write`; `proposal_bytes` side-table; `push_fenced_source`; `sanitize_ident`; `is_write_allowed`; the append chokepoint + `reject_empty_tier_b`.
**New dependency:** `notify` (FSEvents/inotify). Its internal `unsafe` is in a separate crate — **does not** violate this crate's `#![forbid(unsafe_code)]`. Pre-merge (Rev 2 / dep audit): pin the version, run `cargo audit` (no CRITICAL/HIGH CVEs), and verify no feature pulls `unsafe`/extra weight into bossclaw-core. The watcher is `#[cfg(unix)]`; ensure no `pub` item is left undocumented or dead on the non-unix build (clippy `-D warnings` across both feature sets, §10). Every new `pub` item needs a doc comment (`#![deny(missing_docs)]`).

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

- **Hermetic** (`tempfile::tempdir()` + in-memory `EventLog` + `MockEmbedder` + a scripted-delegating `Reasoner` that answers the synthesis turn, the M6b `tests/reconcile.rs` pattern): all proofs 1–10 (§7), plus mandate grant/revoke, Create-vs-Edit, segment-aware source filtering + target exclusion, **over-cap elision** (not truncation), recipe-over-cap grant-rejection, fenced-marker breakout attempt, and the cache-laundering regression guard (3b). A deliberately **nondeterministic** scripted reasoner backs proof 4.
- **Live-Ollama** (`#[ignore]`, `tests/live_ollama.rs`): proof 9 — real model, real file synced end-to-end + idempotent + supersede-on-source-change.
- **Gates:** `cargo test -p bossclaw-core`; `clippy -D warnings` (default + `ollama`); `cargo build` (proves `deny(missing_docs)`); taint chokepoint **byte-unchanged**; `forbid(unsafe)` intact; `notify` is the only new dep.

---

## 11. Non-goals (v1)

- **Managed-section sync** inside a hand-edited file (D2: whole-file only).
- **Free-form goal pursuit** (Approach B) — deferred as a possible future brick.
- **Mandate types beyond "keep a file in sync"** (polish, living-record, etc.).
- **Sources spanning multiple read-grant roots** (Rev 2 / critic): `source_scope` is a **single** canonical subtree; a mandate deriving from two disjoint folders is out of scope for v1.
- **Timed expiry of a mandate** (Rev 2 / finding C): needs a testable clock seam; revoke (instant + sticky) covers v1. Same reason the per-mandate cap is per-tick, not per-day.
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

---

## 14. Rev 2 changelog (critic + security review, 2026-06-21)

Both independent Opus reviewers returned **SHIP-WITH-FIXES**; the seam-map verified with zero stale refs. Folded:

- **A (Critical, both reviewers) — self-loop / recipe-eats-output:** the convergence guard "target outside watched roots" was a *should*. Now `add_mandate` **MUST reject** a target under any active read-grant root (§4.2, §6.4 #1, proof 9). Structurally guarantees the engine's own write never re-enters as a source.
- **B (Critical, security C1) — cache-hit taint laundering:** a cache hit re-derived lineage from *current* sources, dropping a tainted source that left scope while its bytes stayed baked in. Now the cache row stores `source_event_ids_at_synth` and the proposal lineage is `stored ∪ current` (taint monotone) (§5.2, §5.3, §8, proof 3b).
- **C (Important, both) — untestable cap + dead producer:** the wall-clock "per-day" cap had no hermetic clock seam; the producer was hardcoded to `m6b-reconciler`. Now a **per-tick** per-mandate cap (no clock), counted on `inducing_key.mandate`; `build_m6b_event`→`build_proposer_event(producer)` threaded through all 3 helpers; timed expiry deferred (§4.1, §5.5, §5.6, proof 6).
- **D (Important, security I2) — recipe silently truncated** by the 200-byte `sanitize_ident`. Now its own sanitize path + `MAX_RECIPE_LEN`, rejected at grant if over-cap (§4.2, §5.2).
- **E (Important, security I3 + critic I4) — silent source truncation / directory-bomb.** Over-cap combined sources now **elide** (retryable); in-scope source *count* capped (§5.2).
- **F (Important, critic I3) — unbounded cache.** Keep only the current `sources_hash` per mandate; purge on revoke (§5.2, §8).
- **G (minors)** — empty-source/empty-output → elide/reject (§5.2, proof 10); mandate phase placed after summarize (§5.1); proof-5 `sources_hash` typo; single-writer `Arc<EventLog>` assert (§6.3); `source_scope` canonicalized + segment-aware (§4.2, §5.1); execute-time grant-revoke proof (proof 1b); `notify` CVE/unsafe audit pre-merge (§8); non-unix `missing_docs` (§8).

Deferred to explicit non-goals (§11): multi-read-grant-spanning sources; timed expiry.
