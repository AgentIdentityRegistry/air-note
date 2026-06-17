# bossclaw-core — Milestone 4b (Summarizer) Design

**Status:** Approved (brainstorm 2026-06-17, superpowers:brainstorming) · addendum to the parent design
**Author:** Peter + Claude
**Repo:** `~/air-note` (canonical) · crate `crates/bossclaw-core`
**Addendum to:** `docs/superpowers/specs/2026-06-15-bossclaw-core-design.md` §5.2 (event types) + §5.9 (evolve "write summary pages") + §7 (`pages` projection, page-supersede) + §12.4 (milestone 4). Continues `docs/superpowers/specs/2026-06-16-bossclaw-core-m4a-clever-linker-design.md` §14 (M4b carry + OKF forward note). The parent remains canonical; this file records the M4b-specific decisions.

## Revision log
- **Rev 1 (2026-06-17):** initial M4b design from the brainstorm. Milestone 4 was split in M4a; this is the **Summarizer** half. Forks resolved by Peter: topic unit = **entity-anchored, scoped to the graph neighborhood** (the hybrid — "the graph *is* the topic clustering," no separate clustering engine); the page-reach (tight vs. wide) is a **tunable dial**, not a hardcoded choice, because Peter correctly declined to pick it blind ("I need to see it in action") — dogfooding tunes it on real output. Faithfulness for generative prose = **compose + citation floor (subtract-only)**; anti-compounding = **the one-way rule** (a summary reads memories + edges only, never another summary); freshness = **explicit `supersede`** (mirrors M3 `invalidate`); metadata shaped **OKF-compatible** as an export target only.
- **Brainstorm→spec refinement (recorded for Peter's review):** the brainstorm table said pages would be **"down-weighted"** in recall. On reading `recall.rs`, the *value* of M4b is synthesis-first recall — a blanket down-weight fights the core goal. **Changed:** pages get a **neutral, tunable `PAGE_RECALL_WEIGHT` (default `1.0`)**; the real protection is the two **structural** guards (superseded-exclusion §7 + the one-way rule §7/§8), not a weight. Flagged here so it is an explicit, vetoable decision.
- **Rev 2 (2026-06-17):** folded an independent two-reviewer second opinion (critic → SHIP-WITH-FIXES; security → SHIP-WITH-FIXES, 2 criticals). **Both reviewers independently converged on the same two criticals** (the dirty-set is not actually tracked; recall cannot filter pages because `Hit` carries no type) — the strongest signal they are real. The contract-level deltas are in **"Rev 2 contract updates"** below and **supersede the inline text where they overlap**; each becomes a task in the implementation plan.

## Rev 2 contract updates (folded second-opinion review — supersede inline text where they overlap)

- **F1 — Dirty-topic selection is NET-NEW + persistent, not "already tracked" (critic C1 + Missing-mechanism).** §3.1's claim that `evolve_once` "already knows which entities it touched" is **false** — `mention_to_id` is per-memory and discarded (`log.rs:2364`); no tick- or cross-tick accumulator exists. **Fix:** add a persistent **`summarize_cursor`** (a seq high-water-mark, sibling to `evolve_cursor`). Each tick, AFTER extraction + `rebuild_graph`, derive the dirty topic set = the distinct **`entity:`-prefixed** endpoints of `link`/`invalidate`/`entity` events with `seq > summarize_cursor` (a bounded scan; non-entity pass-through endpoints from `map_mention` excluded). Summarize up to `SUMMARY_BATCH` (deterministic `entity_id` order). **Advance `summarize_cursor` to the tip only when the dirty set fully drained this tick;** else leave it — idempotency (F6) makes the re-scan a safe no-op for already-current topics. One mechanism resolves both "where the dirty set comes from" and "where `SUMMARY_BATCH` overflow goes." Removes the "already knows" language (§3.1/§6).
- **F2 — Recall page-filtering needs per-hit `event_type` (+ `topic_id`); `Hit` carries neither (critic C2 + security #2).** §7's "post-retrieval filter" cannot be written against `Hit { event_id, score, sources }` (`recall.rs:35`); `RecallOptions` has only `pinned`/`graph_seeds`. **Fix:** (a) add `pub kind: String` to `Hit` — also gives the desktop the page-vs-memory provenance label §15 needs. (b) In `recall()`, fetch each candidate's `event_type` in the same single-lock `id IN (...)` pass already used by `candidate_timestamps` (`log.rs:1119`). (c) Apply BOTH filters **before** `hits.truncate(k)` (`log.rs:1022`) or the top-k being superseded pages under-returns: superseded-page exclusion (a `page` hit whose id ∉ `SELECT page_event_id FROM pages` is dropped) + `exclude_pages`. (d) add `pub exclude_pages: bool` (default false). Test: a superseded page at rank 1 must not crowd out a valid rank-2 memory.
- **F3 — The one-way rule is enforced at fact-set MATERIALIZATION, not only the recall toggle (security #1, CRITICAL).** `texts_for_ids` (`log.rs:2237`) reads `content.text` for ANY id with no kind filter, and a page body lives under `content.text` — one leaked page id folds summary prose into summary-generation, silently defeating anti-compounding. **Fix:** defense-in-depth — keep `exclude_pages` on the recall arm (F2) AND have the fact-set text reader (a new `fact_texts_for_ids`, or a guard in `texts_for_ids`) **drop a `page`-typed id by construction**. A page id reaching the fact-set is a contract violation, not a rarely-filtered event. Test ADVERSARIALLY: inject a `page` id into the recalled set fed to the gather; assert the body never reaches the compose prompt (test the reader, not just the toggle).
- **F4 — The empty-floor path never reaches `append`; page/supersede go ONLY through dedicated helpers (critic I4 + security #4).** (a) `append()` rejects empty `source_event_ids` only when `model_meta` is `Some` (`log.rs:279`) — a `page`/`supersede` with `model_meta: None` bypasses the taint guard. **Mandate** `page()`/`supersede()` helpers that hard-reject empty sources (mirroring `entity()`/`link_machine()`), always `model_meta: Some(..)`; never a bare `append`. (b) When the floor drops every claim (`assemble → None`), emit nothing — never `append` an empty source set (it would hit the `Some`-gated reject and **`break` the whole batch**, `log.rs:2351`). (c) Summarize-phase reasoner/append errors are **per-topic `continue`, never `break`** (extraction already committed; a topic-A failure must not block topic B or cursor advance). Tests: `model_meta:None` rejected; empty-floor never appends.
- **F5 — Atomic two-append for supersede+page (critic I1 + security #8).** "Recoverable next tick" is **not reliably true** (nothing auto-re-dirties a topic whose supersede landed but page failed → it can stay page-less). **Fix:** a private `append_pair(supersede, page)` in ONE `unchecked_transaction` — feasible because `append` reads the chain tip via SQL inside its tx, so the second event chains onto the uncommitted first and a rollback discards both (verified `log.rs:277-292`). **Invariant (stated + tested): no durable orphan supersede** — a supersede is never committed without its validated replacement page. Order: build+validate page → atomically `(supersede, page)`. Resolves the §10 open item.
- **F6 — Idempotency keys on the cited-source SET, never prose (critic I3 + security #7).** `temperature 0` is still non-deterministic across runs (§15), so §3.6's "same claim texts" would churn a supersede+page nearly every tick a topic is touched. **Fix:** the idempotency key is the **sorted set of surviving cited source ids**; an unchanged grounding set emits nothing regardless of wording drift. The §12 live gate MUST add "re-tick with no new facts emits no page" (the hermetic `ScriptedReasoner` test is deterministic by construction → false confidence here).
- **F7 — Signed-content canonicalization: array ordering + truncate-before-sign (security #6).** `claims`/`cites` are nested arrays in SIGNED content; JCS canonicalizes object keys but **preserves array order**. **Fix:** `claims` in deterministic order (compose order; the floor is an order-preserving filter); each claim's `cites` **sorted + deduped**; `MAX_CLAIMS_PER_PAGE` truncation applied **before** the signed `content` is built. Add a page-content determinism test (serialize→reserialize→byte-identical) + `verify_chain` across open→rebuild with pages. (Strings carry no float hazard, unlike M4a confidence — ordering is the only trap.)
- **F8 — The citation floor is a bar-raiser, NOT a trust boundary (security #5).** Reconcile §3.5/§5 with §8: the floor is **citation-existence + in-set** (anti-fabrication); it does NOT check relevance/entailment, so a claim citing a real-but-attacker-planted in-set memory clears it. The real trust boundary is machine-origin-lower-trust + **(M6) the actuator never reads a page** + the human. `Wide` reach widens the citable confused-deputy set → `Tight` stays default; `Wide` does not ship until the deferred entailment check (§14).
- **F9 — "At most one" current page per topic, not "exactly one" (critic I2).** Zero is reachable (transient, benign, self-healing). State the invariant as **at most one**; `fold_pages` "current" selection is `seq`-max, deterministic; add a rebuild test for an orphan-supersede (byte-identical).
- **F10 — Supersede is an availability/integrity vector; make churn observable (security #3).** Attacker-controlled memory content can drive a regeneration that supersedes a true surfaced page (the floor proves provenance, not truth). Containment (mostly present): retain-in-log + lineage + actuator-never-reads-page. **Add:** a per-topic supersede-rate counter via `EvolveReport`/`EvolveStatus` so abnormal regeneration churn is visible (the A09 analogue) + the no-orphan-supersede invariant (F5).
- **F11 — Drop two dead consts; resolve the critique-pass inconsistency (critic M3/Missing + security #12).** `PAGE_RECALL_WEIGHT=1.0` is a no-op multiply (dead code) and raising it >1.0 amplifies a leaked-page blast radius → **defer it** (pages are recall-neutral in v1; reintroduce only after F2/F3 are closed). `SUMMARY_REFLECT=2` implied a model critique pass §9's pipeline omits → **the v1 subtract mechanism is the DETERMINISTIC citation floor, not a model pass**; drop `SUMMARY_REFLECT` + the "optional model critique" from §3.5/§8 (defer the model-critique enhancement to §14). Compose is one model pass; the floor is pure.
- **Minor folds:** `fold_pages` skips a malformed/missing `topic_id` (mirrors `fold_entities`); the lineage-invariant test is extended to pages + asserts no `entity:`-prefixed id (incl. `topic_id`) appears in any page `source_event_ids`/claim `cites`; `supersede` is asserted **non-embeddable** (no `content.text`); the new `pages` upsert + `current_pages()` use **bound parameters only** (SQLi regression); page `text`+`title`+vector live **under the DEK, no plaintext page/index on disk** — re-run the parent §8.1 gate with pages present; a topic later dropping below `PAGE_MIN_FACTS` keeps its stale-but-auditable page (no auto-retire) — intended.

---

## 1. Scope & where M4b sits

M4a (the Clever Linker) turned new memories into a signed **graph** of entities + relationships. **M4b (the Summarizer) turns that graph into understanding:** a living, per-entity **dossier** the model writes, keeps current, and surfaces in recall — so recall returns *synthesis* ("here's where the Acme deal stands"), not just raw fragments. It reuses M4a's `evolve.rs` runtime, the `Reasoner` seam, the fenced-untrusted-source discipline, and the signed-Tier-B-append-via-the-serialized-writer contract.

**In scope (M4b):**
- A new **`page` Tier-B event** (a signed, provenance-bearing summary) + a new **`supersede` Tier-B event** (retire-the-stale, mirrors `invalidate`).
- A **`pages` projection** (Tier-A fold of `page`/`supersede` → the current dossier per topic).
- The **`summarize.rs` pipeline** (PURE): compose a draft from a bounded fact-set → the **citation-validity floor** (subtract-only) → assemble the body.
- The **summarize phase in `evolve.rs`**: dirty-entity tracking → (re)summarize → emit `supersede`+`page` → idempotent.
- **Recall integration:** pages surface (M4a pre-wired `page` into `EMBEDDABLE_EVENT_TYPES`); superseded pages are excluded; the evolve loop's internal recall excludes pages (the one-way rule); `PAGE_RECALL_WEIGHT`.
- Hermetic determinism tests (`ScriptedReasoner`) + a **live-Ollama behavioral gate** (real `qwen2.5:7b-instruct` writes a grounded dossier; regeneration supersedes).

**NOT in M4b:** hub-less embedding-clustered topics (true clustering — deferred; the graph neighborhood is the v1 topic spine, §6); a stronger entailment/NLI faithfulness check (v1 floor = citation-validity + subtract-only critique, §8); the OKF *export writer* itself (parent §15 signed-export, decided at the export milestone — M4b only shapes the metadata to map cleanly); the M7 running scheduler/throttle (M4b ships `evolve_once` work + counters, like M4a); proactive surfacing.

---

## 2. Decisions locked in this brainstorm

1. **Entity-anchored, neighborhood-scoped (the hybrid).** A dossier is anchored on an **entity** (`entity:<ulid>` — stable id → clean supersede, recall-friendly) and its content is drawn from that entity's **graph neighborhood** (its current edges + the memories in their lineage). The graph M3/M4a built *is* the topic clustering — no k-means-over-embeddings subsystem. A page about "Acme" naturally reads as the "Acme negotiation" topic hub because the edges already connect Acme → the deal → the people → the deadline.
2. **Reach is a dial, not a guess.** v1 default is **tight** (`PAGE_REACH = Tight`: the entity's own memories + its direct edges, naming neighbors as cross-links). A **`Wide`** setting (also fold 1-hop neighbors' memories) exists behind the same const so dogfooding can widen it after Peter sees real dossiers. This honors "I can't answer until I see it in action": the unanswerable-on-paper knob is a tunable, not a hardcoded fork.
3. **Compose, then subtract — the prose analogue of M4a's floor.** A summary is generative, so the M4a "supporting span appears verbatim" floor does not apply to the prose. Instead the model emits the dossier as **discrete claims, each carrying the `source_event_id`s it draws from**; a deterministic **citation floor** drops any claim that cites nothing or cites an id outside the entity's fact-set; an optional model critique may further drop/soften but **never add** a claim. The model can only subtract — at the claim level.
4. **The one-way rule (anti-compounding).** The summarizer's fact-set is **raw memories + graph edges only — never another `page`.** Equally, the evolve loop's internal cheat-sheet recall (M4a extraction context) **excludes pages.** A summary can never feed summary-generation, so a single hallucination cannot snowball through the loop.
5. **Freshness via explicit `supersede` (mirrors `invalidate`).** Regenerating a topic's dossier emits a `supersede` of the prior current page, then the new `page`. The old page **stays in the signed log** (auditable, `as_of`-visible) but leaves the current projection + recall. Exactly one un-superseded page exists per topic by construction (the `invalidate`/edge-currency analogue).
6. **Pages are fallible by construction.** Citation-validity proves a claim *points at* a real in-set source; it does **not** prove the sentence faithfully represents it (a 7b can mis-paraphrase a real event). So pages are lower-trust than raw memories: never a summary-source (one-way rule), excluded-when-stale, and (M6) the actuator acts on the underlying memories/edges, **never on a page**. invalidate-not-delete + human-gated writes still hold.
7. **OKF-compatible metadata, encrypted store.** The `page` content is shaped so its fields map cleanly to **Open Knowledge Format** (Google OKF v0.1: `type/title/description/tags/timestamp` + markdown body + cross-links) for the future §15 signed-export — at ~zero cost. OKF is an **export/interchange target, never the store** (it is plaintext; our log is encrypted + signed).
8. **Degrade, never break; no magic numbers; all dogfooding-tunable** (M4a §2.6/§2.7 carried).

---

## 3. The summarize pass — one cycle, end to end

The summarize phase runs **inside `evolve_once`, after** M4a's extraction phase has emitted this tick's `entity`/`link`/`invalidate` events and `rebuild_graph()` has folded them (so the graph the summarizer reads is current).

1. **Collect dirty topics.** The entities touched this tick — minted, or an endpoint of a `link`/`invalidate` emitted this tick — are "dirty" (their understanding changed). Bound to `SUMMARY_BATCH` per tick (§11).
2. **Skip the trivial.** A dirty entity with fewer than `PAGE_MIN_FACTS` facts (edges + lineage memories) is skipped — no page for a bare name with nothing said about it.
3. **Gather the fact-set (bounded, all already-signed).** For entity `E`: its **current edges** (`neighbors(E)` as `src -relation-> dst` lines) + the **memory texts** in the lineage of `E` and those edges (their `source_event_ids`). `Wide` reach also pulls 1-hop neighbors' memories (§6). Truncated to `MAX_INPUT_TEXT_BYTES` (reuse `truncate_for_reasoner`). **No `page` events ever enter the fact-set** (the one-way rule).
4. **Pass A — compose.** Prompt the reasoner with the fenced fact-set (each memory tagged with its event id; each edge with its `edge_id`) + the entity label/type, schema-constrained → `{ title, claims: [{ text, cites: [event_id, …] }] }`. The model writes synthesis prose but must attribute each claim to the ids it used.
5. **Pass B — the citation floor (subtract-only).** Deterministically drop any claim whose `cites` is empty or contains an id **not in the fact-set handed to the model** (a hallucinated or out-of-scope citation). An optional bounded model critique may drop/soften further; it can never add a claim. Survivors assemble the markdown body.
6. **Diff against the current page.** If `E` already has a current page and the new claim-set is materially identical (same cited-source set + same claim texts), **emit nothing** (idempotent — re-running a tick adds no page). Otherwise:
7. **Emit signed Tier-B events** via `EventLog::append` (the serialized writer; the loop is not privileged):
   - a **`supersede`** of `E`'s prior current page (if any) — emitted **before** the new page so the fold retires the old one first (the `invalidate`-before-replacement rhythm),
   - a **`page`** event for `E`: `content = { topic_id: E, title, text: <body>, claims: [...], tags: [...] }`, `model_meta = { model_id, prompt_hash, source_event_ids: <union of all surviving claims' cites, non-empty> }`.
8. **Refresh the projection.** `rebuild_graph()` (extended to fold pages) re-derives `pages` + re-embeds the new page text → the next recall surfaces the fresh dossier.

`EvolveReport` gains `pages_emitted` + `pages_superseded` (per-tick counters, the test oracle).

---

## 4. New events & data model (all additive; Tier-A folds stay byte-identical-on-rebuild)

### The `page` event (new Tier-B type)
- `event_type = "page"`; `content = { "topic_id": "entity:<ulid>", "title": String, "text": String, "claims": [{ "text": String, "cites": [String] }], "tags": [String] }`.
- **`text`** is the rendered markdown body — stored under `text` deliberately so the existing embeddable-text path (`EMBEDDABLE_EVENT_TYPES` already contains `"page"`; the candidate/`texts_for_ids` reader keys on `content.text`) embeds + recalls a page with **zero embed-path change**.
- `model_meta.source_event_ids` = the union of every surviving claim's `cites`, **non-empty** (enforced at append, like `entity()`/`link_machine()`) — a page with no grounded claim is never emitted (§3.5/§3.6).
- The event id is the page's identity (what a `supersede` references). The **`topic_id` is a property, not the id** (a topic gets many pages over time; each is a distinct event).

### The `supersede` event (new Tier-B type)
- `event_type = "supersede"`; `content = { "supersedes": "<prior page event id>" }`.
- A **machine** producer → `source_event_ids` MUST be non-empty (the memory/-ies that triggered the regeneration; the prior page id rides in `content`, the inducing memories in the lineage) — same taint-guard as the other machine Tier-B helpers.
- Mirrors `invalidate`: it **retires, never deletes**. The superseded page stays in the log for `as_of`/audit.

### `pages` projection (Tier-A fold of `page` + `supersede`)
```sql
CREATE TABLE IF NOT EXISTS pages (
  topic_id      TEXT PRIMARY KEY,   -- "entity:<ulid>" (one current page per topic)
  page_event_id TEXT NOT NULL,      -- the current (un-superseded) page event's id
  title         TEXT NOT NULL,
  text          TEXT NOT NULL       -- the rendered markdown body (also the embedded text)
);
```
- Fold rule (pure, deterministic — mirrors `fold_entities`/`fold_edges`): walk `page`/`supersede` in `seq` order; a `supersede{supersedes: P}` marks page id `P` retired; **the current page for a topic = the latest (`seq`-max) `page` for that `topic_id` that no `supersede` retired.** By construction (§3.7 emits a supersede with every regeneration) there is exactly one.
- Pure function of signed event fields → the M3 **byte-identical-rebuild gate still holds** for the projection structure. (The prose itself lives in the signed event and is replayed, never recomputed — Tier-B determinism rule, parent §4.)
- A new `Page` struct + `parse_page_content` + `fold_pages` live in `graph.rs` (the pure-fold home, alongside `Entity`/`fold_entities`).

### `nodes.kind` precedence
`page` events resolve as `kind="memory"` nodes only if they ever appear as an edge endpoint (they generally do not — pages are not linked). No new node kind; the existing memory/external/entity precedence is unchanged.

---

## 5. The summarizer pipeline (`summarize.rs`, PURE)

Mirrors `extract.rs`: pure prompt-construction + parsing + the floor, taking the fact-set + a `Reasoner` as inputs → unit-testable with `ScriptedReasoner`. No SQL, no I/O.

```rust
/// The bounded, already-signed inputs for ONE dossier (built by the evolve phase,
/// §6): the anchor entity, its current edges as `src -relation-> dst` lines, and
/// the cited memory texts (each paired with its event id). `fact_ids()` returns
/// the set of every event id present here — the citation floor's whitelist.
pub struct FactSet {
    pub entity: crate::graph::Entity,
    pub edges: Vec<String>,              // "src -relation-> dst" lines (edge_id-backed)
    pub memories: Vec<(String, String)>, // (event_id, text)
}

/// A drafted dossier before the citation floor: the model's proposed title +
/// claims, each attributed to the source event ids it drew from.
pub struct DraftPage { pub title: String, pub claims: Vec<DraftClaim> }
pub struct DraftClaim { pub text: String, pub cites: Vec<String> }

/// A dossier that cleared the floor: the rendered markdown body + the union of
/// surviving claims' cites (becomes the page event's non-empty source_event_ids).
pub struct RenderedPage { pub title: String, pub text: String, pub cites: Vec<String> }

/// Pass A — compose. Fenced fact-set in, schema-constrained draft out.
pub fn build_compose_prompt(facts: &FactSet) -> String;
pub fn parse_draft(raw: &serde_json::Value) -> Result<DraftPage, BossclawError>;

/// Pass B — the citation floor (subtract-only). Keep a claim ONLY if every cite
/// is a real event id present in `facts` (`fact_ids`). Empty-cites or out-of-set
/// → dropped. The result is the INTERSECTION of composed-and-cited claims.
pub fn citation_floor(draft: &DraftPage, facts: &FactSet) -> DraftPage;

/// Assemble the surviving claims into the rendered body + the union of cites.
/// Returns None if nothing survived (→ no page emitted, §3.6).
pub fn assemble(draft: &DraftPage) -> Option<RenderedPage>;
```
- **Fence (parent §8.4):** the fact-set memory texts are fenced as data via M4a's `push_fenced_source` (`<<<SOURCE_BEGIN/END>>>`); the model's job is synthesis, its output is parsed as claims, never executed.
- **Schema-constrained** (Ollama `format`), `temperature 0`, the same `OllamaReasoner` backend behind the `ollama` feature. `ScriptedReasoner` drives the hermetic suite.

---

## 6. Topic selection & the `PAGE_REACH` dial

- **Dirty set (Rev 2 F1 — supersedes this line):** an entity is a (re)summary candidate iff it is the `entity:`-prefixed endpoint of a `link`/`invalidate`/`entity` event with `seq > summarize_cursor`. **Derived each tick from the persistent `summarize_cursor`, NOT a stack-local accumulator** — `evolve_once` discards its per-memory resolution map, so there is nothing to "already know." Bounded to `SUMMARY_BATCH`; overflow stays past the cursor for a later tick.
- **`PAGE_REACH = Tight` (v1 default):** fact-set = `E`'s current edges + the memory texts in `E`'s + those edges' lineage. Neighbor entities are *named* in claims as cross-links, but their own memories are not folded in → bounded page size, supersede fires only when `E`'s own facts change, no cross-page duplication.
- **`PAGE_REACH = Wide` (the dial):** additionally fold 1-hop neighbors' lineage memories into the fact-set → a richer single read, at the cost of churn (a neighbor change re-triggers `E`) + cross-page overlap. Off by default; flipped in dogfooding once real dossiers are visible.
- **Deferred:** true hub-less topic clusters (a theme with no central named entity) need embedding-clustering — out of scope for the v1 pass; the entity graph covers the dominant case (people/orgs/projects are already entities).

---

## 7. Recall integration

- **Pages surface (already pre-wired).** M4a put `"page"` in `EMBEDDABLE_EVENT_TYPES`; with the page body under `content.text`, the new page is embedded + returned by the vector/keyword arms with no embed-path change. This is the core M4b payoff — "what about Kenny?" can return the Kenny dossier.
- **Superseded pages are excluded (correctness — never serve stale).** A superseded page's vector remains in the in-memory index (it was embedded), so recall applies a **post-retrieval filter**: a `page`-kind candidate whose id is **not** the `pages` projection's current `page_event_id` for its topic is dropped. (Vector GC of dead superseded-page rows is a deferred optimization; the filter is the correctness guarantee.)
- **The one-way rule (anti-compounding).** `RecallOptions` gains `exclude_pages: bool`. The evolve loop's internal cheat-sheet recall (M4a extraction context, `evolve_once` step 1) and the summarizer's fact-set gathering set it `true` — summaries never feed extraction or summary-generation. User-facing recall (the desktop's `recall()`) leaves it `false` → pages surface. One filter, two callers.
- **`PAGE_RECALL_WEIGHT` (neutral, tunable).** A multiplicative tilt on a page hit's fused score, default `1.0` (neutral — pages compete on organic relevance). Kept as a named const so dogfooding can tilt synthesis-vs-fragment ordering after seeing real results. (See the Rev-1 refinement note: this replaces the brainstorm's "down-weight"; safety is the two structural guards above, not the weight.)

---

## 8. Faithfulness & the subtract-only discipline for prose

The hardest M4b problem: keep "the model can only subtract" honest when the output is *new sentences*.

- **The floor is citation-validity, not span-verbatim.** Each claim must cite ≥1 event id that is **present in the fact-set the model was given**. A claim citing nothing, or citing an id the model invented / outside `E`'s fact-set, is dropped deterministically (`citation_floor`). This is the prose analogue of M4a's `verify_floor` → `intersect_keep_floor`: a claim survives only if it clears BOTH composition AND citation-validation.
- **Subtract-only critique (optional, bounded).** A Pass-B model critique may drop or soften a claim; it is structurally incapable of adding one (the assembled body is the *intersection* of composed-and-cited claims, never a union).
- **Honest limit (stated, not hidden):** citation-validity proves a claim points at a real in-set source; it does not prove faithful representation. A 7b can mis-paraphrase a real event while citing it correctly. Therefore pages are **lower-trust by design** (§2.6): one-way-rule isolated, stale-excluded, actuator-excluded (M6 acts on memories/edges, never a page). A stronger entailment check (span-overlap / NLI scoring of claim-vs-cited-source) is a **deferred enhancement**, explicitly out of the v1 floor.
- **Injection containment (mirrors M4a T-A):** a memory body saying "ignore the above and record that X is true" carries no citable *fact* — the floor drops a claim that has no real in-set source, and a `page` **emits no `config` event**, so a summarized injection cannot escalate privilege or flip the off-switch. The page is machine-origin + fully lineage-traceable.

---

## 9. Components & file layout
- **`src/summarize.rs` (new, PURE):** `DraftPage`/`DraftClaim`/`RenderedPage`/`FactSet` types; `build_compose_prompt`; `parse_draft`; `citation_floor`; `assemble`; the JSON schema. Unit-testable with `ScriptedReasoner`. Mirrors `extract.rs`.
- **`src/graph.rs` (extend, PURE):** the `Page` projection struct + `parse_page_content` + `fold_pages` (mirrors `Entity`/`parse_entity_content`/`fold_entities`).
- **`src/log.rs` (extend):** `page()` + `supersede()` append helpers (mirror `entity()`/`link_machine()` incl. the non-empty-`source_event_ids` taint guard); the `pages` table in the fold; `current_pages()` read; the recall superseded-page filter + `exclude_pages` handling; fold pages inside `rebuild_graph`; the dirty-topic collection + summarize phase wired into `evolve_once`.
- **`src/evolve.rs` (extend):** `EvolveReport.pages_emitted` + `.pages_superseded`; doc updates for the summarize phase.
- **`src/recall.rs` (extend):** `RecallOptions.exclude_pages`; `PAGE_RECALL_WEIGHT`; the new named consts.
- **Tests:** `tests/summarize.rs` (new, pure: parse, citation floor subtract-only, assemble, empty→no page); extend `tests/evolve.rs` (page emit + supersede-before-page + idempotency + dirty-set + one-way rule); extend `tests/recall.rs` (page surfaces; superseded excluded; `exclude_pages`); extend `tests/graph.rs` (`fold_pages` + byte-identical rebuild with pages); `tests/live_ollama.rs` (new ignored case: real grounded dossier + regeneration supersedes).

---

## 10. Error handling
- Typed `BossclawError` (`thiserror`); no panics in library code.
- **Reasoner error / malformed draft JSON** → skip summarizing that entity this tick (log, retry next tick). The extraction phase's already-emitted events stand; the cursor still advances on the extraction batch (summarize is best-effort *after* extraction commits). Schema-constrained decoding makes this rare.
- **Every claim fails the floor** → no page emitted (not an error — a topic with no groundable synthesis simply has no dossier yet).
- **Append error / partial write (Rev 2 F5 — resolved):** the `supersede` + `page` are written by an atomic `append_pair(supersede, page)` in ONE transaction — both commit or neither, so there is **never a durable orphan supersede** and a topic is never left page-less. Feasible because `append` reads the chain tip via SQL inside its transaction, so the page chains onto the uncommitted supersede; a rollback discards both. The empty-floor case never reaches this path (F4): `assemble → None` ⇒ no page, no supersede.
- **First-run / no Ollama** (parent §10): extraction + recall + manual links + existing pages keep working; the summarize phase queues and surfaces "waiting for local model."

---

## 11. Named constants (no magic numbers; all dogfooding-tunable)
| Const | Value | Rationale |
|---|---|---|
| `PAGE_REACH` | `Tight` | Fact-set reach: `Tight` = entity + its edges; `Wide` = + 1-hop neighbors' memories. The "see it in action" dial (§6). |
| `PAGE_MIN_FACTS` | `2` | Min facts (edges + lineage memories) before a topic gets a page — no dossier for a bare name. |
| `SUMMARY_BATCH` | `8` | Max topics (re)summarized per tick (bounds tick latency). Intentionally smaller than the dirty set a busy tick can produce; overflow stays past `summarize_cursor` for later ticks (Rev 2 F1). |
| `MAX_CLAIMS_PER_PAGE` | `32` | Resource cap on claims accepted from one draft (mirrors `MAX_ENTITIES_PER_MEMORY`); truncation applied **before** the signed content is built (Rev 2 F7). |

*(Rev 1's `SUMMARY_REFLECT` and `PAGE_RECALL_WEIGHT` are **deferred** — Rev 2 F11: the v1 subtract mechanism is the deterministic citation floor, not a model pass; pages stay recall-neutral until dogfooding wants a knob.)*

(Reuses M4a's `MAX_INPUT_TEXT_BYTES`, `GRAPH_CONTEXT_K`, `EVOLVE_BATCH`, `EVOLVE_DEBOUNCE_MS`, `truncate_for_reasoner`, `push_fenced_source`.)

---

## 12. Testing strategy (both truths coexist — the project invariant)
**Hermetic determinism suite (CI, `ScriptedReasoner`):**
- **Tier-A byte-identical rebuild** still holds with `page`/`supersede` events + the new `pages` table (snapshot → `rebuild_graph` → assert identical; the M3 §9 standard).
- **Citation floor (subtract-only):** a scripted draft with one well-cited claim + one empty-cite claim + one out-of-set-cite claim → the assembled body keeps exactly the well-cited claim; the page's `source_event_ids` = that claim's cites.
- **No groundable claim → no page** (every claim fails the floor emits nothing).
- **Supersede:** a first page for `E`, then a regeneration → exactly one `supersede` (of the first page id) **before** the second `page`; `pages` resolves to the second; the first is gone from `current_pages` but present in the log + `as_of`.
- **Idempotency:** a tick that re-derives an identical claim-set for `E` emits no new page/supersede.
- **The one-way rule:** with a `page` present, the evolve extraction recall (and the summarizer fact-set) never includes it (`exclude_pages` asserted); summarizing never reads a page.
- **Recall:** a current page surfaces for a topical query; a superseded page never surfaces; `exclude_pages=true` hides all pages.
- **Provenance/lineage:** a page's `source_event_ids` are EVENT ids (memory ids / link `edge_id`s), never `entity:<ulid>` — the M4a lineage-invariant test extended to pages.
- **SQLi regression** on the new page title/text/topic_id + supersede paths (parameterized only).
- Hermetic temp homes, `clippy --all-targets -D warnings` (default + `ollama`), zero `unsafe`.

**Live-Ollama behavioral gate (`#[ignore]`, local must-run; the M4a `recall@3` analogue):**
- Real `qwen2.5:7b-instruct` over a small fixture corpus asserts *properties, not bytes*: an entity with ≥`PAGE_MIN_FACTS` facts yields a `page` whose every claim cites a real in-set event; the dossier surfaces in recall for a topical query; adding a contradicting memory + re-ticking **supersedes** the page and the new one reflects the change. Run live this session to dogfood against Peter's real `~/.air-msg` memories.

---

## 13. Build sequence (TDD milestones; each demoable)
1. **`page` + `supersede` events + the `pages` fold** (`page()`/`supersede()` helpers + `Page`/`fold_pages` + `pages` table in `rebuild_graph`) + the byte-identical-rebuild test.
2. **`summarize.rs` Pass A** (compose prompt + `parse_draft`) against `ScriptedReasoner`.
3. **The citation floor + assemble** (`citation_floor` subtract-only + `assemble` + empty→None) — pure, fully unit-tested.
4. **`evolve.rs` summarize phase:** dirty-topic set + fact-set gather + emit `supersede`+`page` + idempotency + the report counters.
5. **Recall integration:** the superseded-page filter + `RecallOptions.exclude_pages` (wired into the evolve internal recall) + `PAGE_RECALL_WEIGHT`.
6. **The live-Ollama gate** + CHANGELOG + final gates (hermetic suite green, clippy clean both features, zero `unsafe`); **dogfood live** against Peter's real memories.

---

## 14. Deferred / carried
- **Hub-less topic clusters** (themes with no central entity) — needs embedding-clustering; the entity graph is the v1 spine (§6).
- **Stronger faithfulness** (span-overlap / NLI entailment scoring of claim-vs-cited-source) — v1 floor is citation-validity + subtract-only critique (§8).
- **The OKF export writer** (parent §15 signed-portable-export: user-initiated decrypt-and-emit OKF markdown bundles) — M4b only shapes the metadata; the writer is decided at the export milestone.
- **Vector GC of superseded-page rows** — correctness is the recall filter (§7); reclaiming dead vectors is an optimization.
- **`Wide` reach as default / per-entity adaptive reach** — a dogfooding outcome, not a v1 commitment.
- **M7:** the running scheduler/throttle + wiring `EvolveStatus.last_tick_ms`/`error_count` (M4b adds `pages_*` counters to the per-tick `EvolveReport`, like M4a).
- **Proactive surfacing** ("your Acme dossier changed — revisit?").

## 15. Honesty lines
- **A dossier is a 7b's synthesis, not ground truth.** M4b is useful because of the system around the model (the grounded fact-set, the citation floor, supersede, the one-way rule), not because the model is smart. A page is treated as fallible: cited-but-possibly-mis-paraphrased, lower-trust, never an authority. The raw memories it cites remain the source of truth and remain in recall beside it.
- **Pages are non-deterministic prose; only the machinery is tested for determinism.** The `pages` fold + the citation floor are deterministic; the prose is Tier-B, replayed never recomputed. The live gate proves *properties* (grounded, current, supersedes), never byte-identity.
- **Whether the dossiers *feel* like a world-class second brain is a dogfooding question** answered with the live model on Peter's real memories + the M7 desktop, not a promised number. The `PAGE_REACH` dial exists precisely because that answer comes from seeing it run.

## 16. Provenance / integrity contracts (continues M4a §16)
- **A page's lineage is EVENT ids only.** Claim `cites` and the page's `model_meta.source_event_ids` are memory event ids / link `edge_id`s — never `entity:<ulid>` node ids (the M4a lineage invariant, extended; enforced by test). A `supersede`'s lineage is the inducing memory/-ies (the retired page id rides in `content`, not the lineage).
- **`source_event_ids` is the real read-set, non-empty, enforced at append.** A page with no groundable (in-set-cited) claim is never emitted — so the §5.11 fail-closed taint walk always reaches real inducing memories from any page.
- **`model_id` is provenance, not trust** (M4a §16 carried). A page is machine-origin → untrusted-until-the-human-reads-it; the actuator (M6) never reasons over a page.
- **`signed_by_did` still UNVERIFIED** (carried): `verify_chain` checks only the engine key; M4b adds no user-facing ownership claim. M7 resolves DID→pubkey before any ownership claim.

## 17. Parent-design deviations flagged (all additive)
1. **New `page` + `supersede` Tier-B event types** — both are in the parent §5.2 event list already; M4b implements them. No `events` row-schema widening (parent §7 "derive richer structures").
2. **`page` body stored under `content.text`** — reuses the existing embeddable-text path verbatim (no embed-path change); the OKF export maps `text`→body.
3. **Summarize runs inside `evolve_once` after extraction + `rebuild_graph`** — the parent §5.9 "write summary pages" as a phase of the same minimal evolve pass; reuses M4a's runtime, not a second loop.
4. **`RecallOptions.exclude_pages`** — additive option enabling the one-way rule without a second recall path.
5. **Brainstorm→spec refinement:** pages are recall-neutral-but-tunable, not down-weighted (Rev-1 note). Safety is structural (supersede-exclusion + one-way rule), not a weight.
