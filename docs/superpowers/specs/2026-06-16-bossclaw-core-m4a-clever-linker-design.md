# bossclaw-core — Milestone 4a (Clever Linker) Design

**Status:** Approved (brainstorm 2026-06-16, superpowers:brainstorming) · addendum to the parent design
**Author:** Peter + Claude
**Repo:** `~/air-note` (canonical) · crate `crates/bossclaw-core`
**Addendum to:** `docs/superpowers/specs/2026-06-15-bossclaw-core-design.md` §5.8 (reasoner) + §5.9 (evolve) + §12.4 (milestone 4). The parent remains the canonical overall design; this file records the M4a-specific decisions.

## Revision log
- **Rev 1 (2026-06-16):** initial M4a design from the brainstorm. Milestone 4 is **split** into **M4a (Clever Linker — this spec)** and **M4b (Summarizer — a later spec)**. Primary forks resolved by Peter: scope = **full minimal curator** (split a/b); LLM wiring = **real backend, first-class** (Ollama `qwen2.5:7b-instruct`); cleverness = **maximal** (retrieval-augmented extraction + embedding entity-resolution + multi-pass reflexion + typed relation ontology + intra-result reinforcement + confidence/trust-gate).
- **Rev 2 (2026-06-16):** folded an independent two-reviewer pass (critic → SHIP-WITH-FIXES; security → NO-SHIP-until-3-criticals). The detailed fixes live in the **plan's "Rev 2 — folded second-opinion review fixes"** section; the contract-level deltas are in **"Rev 2 contract updates"** below and SUPERSEDE the inline text where they overlap. Peter's decision on the Pass B fork: **model-driven critique + pure fail-closed floor + lineage invariant**.

## Rev 2 contract updates (folded review — supersede inline text where they overlap)
- **Confidence is an integer `confidence_milli` (0–1000) in the signed `link` content, NOT a raw `f32`** (§4/§7/§11). Integers have one canonical JCS form — a raw `f32` risks `verify_chain` breaking across `serde_jcs` versions on an append-only signed store. The trust gate compares integers (`confidence_milli >= 600`) with the threshold **bound as a SQL parameter**, never string-formatted. `TRUST_MIN` (0.6) only derives that integer.
- **`config` is a privilege, not data** (§8/§16). The `evolve_enabled` off-switch is **sticky / fail-closed** (an explicit `false` latches until a *later explicit* authorized `true`; a flag-less newer config does not flip it) and is written ONLY via a typed setter. M7 MUST verify a control config's signer DID == the resolved user owner before honoring an enable or active-model change (forged/replayed config = recall-integrity attack, parent §15).
- **Pass B is a model critique over a pure fail-closed floor** (§3 step 5 / §6): the pure span-verify runs first; the model critique may only DROP or down-confidence — it can never add an edge the floor did not support. `MAX_REFLECT` bounds the propose↔critique turns.
- **`source_event_ids` are EVENT ids, never `entity:<ulid>` node ids** (§16) — enforced by a lineage-invariant test; the §5.11 taint walk fails CLOSED on an unresolvable id (M6 DoD).
- **In-crate resource fail-safes ship in M4a** (§8/§11): `MAX_ENTITIES_PER_MEMORY = 32` + an input-size cap fed to the reasoner + entity-index rebuild once per batch. The *running scheduler / idle-charging-thermal throttle enforcement* is **M7** (M4a ships `evolve_once()` + the pure `debounce_due` + the off-switch + the batch cap).
- **Mandatory new tests** (§12): injection/confused-deputy containment; the lineage invariant; SQLi regression on the new entity-label/alias + machine-relation-label paths; confidence-is-signed (`verify_chain` breaks on tamper); within-tick idempotency; resolved-id retraction → `invalidate`; recall entity-exclusion e2e; trust-gate zero-contribution.

---

## 1. Scope & the M4 split

**M4 is large** (≈2× M2/M3): a reasoner backend + the full "maximal clever" evolve pass + summarization. To keep each piece shippable, demoable, and independently reviewed (the proven one-spec-per-milestone rhythm), M4 is split:

- **M4a — The Clever Linker (THIS spec):** the LLM auto-linker that *populates* the graph M3 built. Reads memories → extracts entities + relationships → resolves entities against the existing graph → retires contradicted facts → appends signed Tier-B events → feeds the graph fold → which makes the next recall smarter. Plus the always-on evolve-loop runtime, the edge-trust gate, and the live-model gate.
- **M4b — The Summarizer (LATER spec):** `page` summary Tier-B events + the `pages` projection + `supersede`, reusing M4a's `evolve.rs` runtime. (Parent §5.9 "write summary pages" + §7 page-supersede.)

**In scope (M4a):** the `Reasoner` seam + the real `OllamaReasoner` backend (feature-gated) + a deterministic test double; the retrieval-augmented extraction pipeline (`extract.rs`); entity nodes + the `entity` event + embedding entity-resolution; multi-pass reflexion; contradiction → `invalidate`; the typed relation vocabulary; confidence + the edge-trust gate on the recall boost; intra-result reinforcement; the evolve-loop runtime (cursor, scheduler, off-switch, resource policy, observability); hermetic determinism tests + the live-Ollama behavioral gate.

**NOT in M4a:** summary `page`s + the `pages` projection (M4b); the full AIR capability ontology (strategic #7 — distinct from M4a's *relation* vocabulary); cloud-frontier escalation (§5.8 `CloudReasoner`); proactive surfacing; the file actuator's use of the trust gate (M6).

---

## 2. Decisions locked in this brainstorm

1. **Real backend, first-class.** The default `Reasoner` is the real local model, **`qwen2.5:7b-instruct` via Ollama** over loopback HTTP, **schema-constrained** (Ollama `format` = a JSON Schema), `temperature 0`, **digest-pinned**, **loopback-only, no egress** (parent §8.5). Rationale: a mock extractor proves plumbing, not intelligence; Peter is the daily dogfooder. Model is swappable behind the trait; every emitted event records its real `model_id`, so a 7b→14b upgrade later is **non-destructive** (new events use the better model; old ones stay tagged + can be re-extracted).
2. **Hermetic tests survive via fixtures, not a live model.** Determinism tests feed **recorded extraction outputs** through a deterministic `ScriptedReasoner` (the only way to test the deterministic Tier-A layer — a live LLM has no byte-identity). The real model is proven by a separate **live-Ollama behavioral gate** (`#[ignore]`, local must-run; the M4a analogue of M2's `recall@3`). CI stays hermetic.
3. **The closed loop is the architecture.** Before extracting from a new memory, the loop **recalls** semantically-similar memories (M2) and pulls the **graph neighborhood** (M3) of the entities involved, and feeds them as the model's working context ("the cheat sheet"). recall → extract → graph → better recall. This is what lets a 7b punch above its weight, and it is **only possible because M2 + M3 already exist.**
4. **Provenance falls out of the cleverness.** The cheat-sheet inputs (the source memory + the recalled memories + the graph-neighbor events the model actually saw) **are** the `source_event_ids` on every emitted event. The cleverness mechanism and M3's F2 taint-lineage contract are the *same list* — no defaulting, always non-empty, the real read-set (parent §16 / M3 §12.1).
5. **Entities are first-class, signed, resolvable.** A new **`entity` Tier-B event** mints a stable `entity:<ulid>` node carrying `{label, aliases, entity_type}`; it folds into `nodes(kind="entity")` + an `entities` projection and is embedded for resolution. Re-seeing "Kenny" **resolves** to the existing node (no duplicate Kennys).
6. **Maximal cleverness, bounded by named constants.** 2-pass reflexion (`MAX_REFLECT`), conservative resolution thresholds (`RESOLVE_HIGH`/`RESOLVE_LOW`), a confidence trust-gate (`TRUST_MIN`) — all const-sourced, no magic numbers (§11), all tunable in dogfooding.
7. **Degrade, never break.** Any reasoner/graph error makes the tick a no-op that retries — recall + storage are never broken by the evolve loop (mirrors M2's keyword-only degrade).

---

## 3. The closed loop — one evolve tick, end to end

For each unprocessed `memory` (or `file_ingested`) event `M` since the cursor:

1. **Recall context.** `recall(embedder, M.text, k=GRAPH_CONTEXT_K, opts)` → the top semantically-related memories. (Filtered to `memory`/`page` kinds; entity events are excluded from recall, §4.)
2. **Pass A — propose (reflexion pass 1).** Prompt the reasoner with `M.text` + the recalled memories + the seed **relation vocabulary** + few-shot exemplars, schema-constrained → `{entities[], relations[], retractions[]}`, each with a `confidence` and a `supported_by` span.
3. **Resolve entities.** For each proposed entity mention: embed it → vector-search existing `entity` nodes → **auto-merge** above `RESOLVE_HIGH`, **mint a new `entity` event** below `RESOLVE_LOW`, **ask the reasoner to adjudicate** the mid-band ("is this the same as any of: […]?"). Yields a stable `entity:<ulid>` per mention.
4. **Augment with the graph neighborhood.** `neighbors()` of the resolved entities (current edges) → the second half of the cheat sheet.
5. **Pass B — critique (reflexion pass 2).** Prompt with `M.text` + the proposals + the resolved-entity neighborhood → (a) **drop** relations not supported by `M.text`, (b) **confirm/deny contradictions** against the now-known current edges (helped by the relation-cardinality table, §6), (c) finalize `confidence`. Capped at `MAX_REFLECT` passes total.
6. **Emit signed Tier-B events** via the **existing serialized writer** (`EventLog::append`; the loop is *not* a privileged writer, parent §4):
   - `entity` events for newly-minted entities,
   - `invalidate` events for confirmed contradictions (emitted **before** their replacement so the fold closes the old interval first),
   - `link` events for confirmed relations,
   - every one carrying `model_meta = { model_id: "qwen2.5:7b-instruct", prompt_hash, source_event_ids: [M.id, …recalled, …neighbors] }` and a `confidence` in content.
   - **Idempotency:** skip emitting a `link` whose `(src, relation, dst)` is already an active edge; reuse resolved entities. Re-running a tick adds nothing.
7. **Advance the cursor** to `M.seq` after the batch commits.

The graph fold (M3, live-on-open) absorbs the new events → the next tick's recall + neighborhood are richer.

---

## 4. New events & data model (all additive; Tier-A folds stay byte-identical-on-rebuild)

### The `entity` event (new Tier-B type)
- `event_type = "entity"`; `content = { "label": String, "aliases": [String], "entity_type": String }`.
- `model_meta.source_event_ids` non-empty (the memory/-ies that introduced it) — enforced at append.
- Node id = `entity:<the entity event's ULID>` (stable, mint-once). The **label is a property, never the id** (names collide and change; the id does not).
- Embedded into `vectors` (keyed `(entity_event_id, model_id)`) for resolution search.

### `entities` projection (Tier-A fold of `entity` events)
```sql
CREATE TABLE IF NOT EXISTS entities (
  entity_id   TEXT PRIMARY KEY,   -- "entity:<ulid>"
  label       TEXT NOT NULL,
  aliases     TEXT NOT NULL,      -- JSON array
  entity_type TEXT NOT NULL
);
```

### `edges` (M3) gains two fold-derived columns
`edges` is a derived projection **recreated by `rebuild_graph`** (not a migrated, persisted table), so these are added to the fold's `CREATE TABLE edges` and populated by the fold itself — **no `ALTER`/migration**:
```sql
-- added to the M3 edges projection (populated by the deterministic fold):
origin     TEXT NOT NULL DEFAULT 'manual',  -- 'manual' | 'machine'
confidence REAL                             -- NULL for manual, [0,1] for machine
```
- **`origin`** = `'manual'` iff the producing `link`'s `model_meta.model_id == MANUAL_LINK_PRODUCER` ("manual"), else `'machine'`.
- **`confidence`** = the `link` content's optional `confidence` (machine links carry it; manual links leave it `NULL`).
- Both are **pure functions of event fields** → the M3 byte-identical-rebuild gate still holds. `link.content` extends to `{ src, relation, dst, confidence? }`; the M3 fold (`parse_link_content`) gains an optional `confidence` read (back-compatible: absent ⇒ `NULL`).
- **`confidence` stays out of `ModelMeta`** (changing that frozen struct would alter canonicalization/signing for *all* Tier-B events) — it lives in the signed `content` instead.

### `evolve_cursor` (re-derivable progress state — NOT a Tier-A fold)
Unlike `nodes`/`edges`/`entities` (folds of the log), this is the loop's processing high-water-mark — persistent, not rebuilt from events:
```sql
CREATE TABLE IF NOT EXISTS evolve_cursor ( id INTEGER PRIMARY KEY CHECK (id = 0), last_seq INTEGER NOT NULL );
```
Single row; advanced after each committed batch. Losing it only re-processes events (idempotent via §3 step 6), never corrupts.

---

## 5. The reasoner seam (`reason.rs` + `ollama.rs`)

```rust
/// The thin, backend-agnostic seam. Output is DATA, never authority (parent §5.8).
pub trait Reasoner: Send + Sync {
    /// Schema-constrained structured completion. `schema` constrains the JSON the
    /// model may emit; the impl is responsible for honoring it (Ollama: `format`).
    fn complete_json(&self, system: &str, prompt: &str, schema: &serde_json::Value)
        -> Result<serde_json::Value, BossclawError>;
    /// The model id stamped into every emitted event's `model_meta.model_id`.
    fn model_id(&self) -> &str;
}
```
- **`OllamaReasoner`** (in `ollama.rs`, behind the **`ollama` cargo feature**): POSTs `/api/chat` to `127.0.0.1:11434` with `format` = the schema, `options.temperature = 0`, the **digest-pinned** model tag; refuses any non-loopback host; no other network egress (parent §8.5). The real default backend.
- **`ScriptedReasoner`** (always compiled, test/util): returns canned JSON keyed by a hash of `(system, prompt)`; the deterministic driver for the hermetic suite (§12). Not a production path.
- **Untrusted-content fence (parent §8.4):** the memory/recalled text is fenced as data (the `channel.mjs` pattern) — the model's job is extraction, and its output is parsed as proposals, never executed. The fence "raises the bar against direct injection; it does not by itself stop a confused-deputy proposal" — which is why machine edges are trust-gated (§7) and writes stay human-gated (M6).
- **§4-diagram note:** the parent diagram draws backends in the app layer; M4a places `OllamaReasoner` *in-core behind a feature* (matching M2's `fastembed`-behind-a-feature precedent) so the live gate lives in-crate. The default build is pure (no `ollama` feature, no network dep); the app may still inject its own `Reasoner`.

---

## 6. The cleverness mechanics

### Retrieval-augmented extraction (the heart)
The prompt for both passes carries: the source memory text, the recalled neighbors (Pass A) / resolved-entity graph neighborhood (Pass B), the seed relation vocabulary, and few-shot exemplars. This turns "extract everything you know" (which a 7b fails) into "reconcile THIS note against THESE specific known facts" (which a 7b does well).

### Extraction schema (Ollama-constrained)
```json
{
  "entities":    [{ "mention": "Kenny", "entity_type": "person", "confidence": 0.0 }],
  "relations":   [{ "src": "Kenny", "relation": "works_at", "dst": "Acme",
                    "confidence": 0.0, "supported_by": "verbatim span from the source" }],
  "retractions": [{ "src": "Kenny", "relation": "works_at", "dst": "Globex",
                    "reason": "...", "confidence": 0.0 }]
}
```

### Entity resolution (no duplicate Kennys)
Embedding-first, model-adjudicated mid-band: cosine ≥ `RESOLVE_HIGH` → auto-merge; ≤ `RESOLVE_LOW` → mint new; in between → the reasoner picks from a short candidate list (or "none"). Conservative because a wrong merge is expensive to undo; thresholds tuned in dogfooding. Reuses M2's `Embedder` + `vectors` (filtered to `kind="entity"`).

### Typed relation vocabulary
A small, **extensible** seed set of relation labels (`works_at`, `knows`, `located_in`, `part_of`, `caused_by`, `owns`, `member_of`, …) handed to the model so the graph doesn't sprout five synonyms for one relation. An unknown relation the model proposes is allowed but flagged (lower trust) — the vocabulary grows by curation, not silently. *(This is the memory-graph relation vocabulary — NOT the AIR registry's agent-capability ontology, strategic #7, which stays separate.)*

### Contradiction → `invalidate`
A small **relation-cardinality table** marks certain relations single-valued *for a subject* (e.g. `works_at_primary`, `located_in`): a new such fact about the same subject implies the prior is retired. Pass B confirms each candidate retraction against the **current** graph edges (it only fires on a real, still-active edge) before an `invalidate` is emitted. Model judgment + cardinality hint together; neither alone.

---

## 7. Confidence, the edge-trust gate, and intra-result reinforcement

- **Confidence** rides in each machine `link`'s signed content; the fold projects it to `edges.confidence`; `origin` is derived from `model_id`.
- **The trust gate** (the M3 §12.1 carry): M3's graph-proximity recall boost gains a predicate — **only `origin='manual'` OR (`origin='machine'` AND `confidence ≥ TRUST_MIN`) edges contribute the boost.** Low-confidence machine edges are still *recorded* (never-forget) and queryable, but they do **not** tilt recall, and (M6) the file actuator must never reason over an untrusted-origin edge. A retired edge already contributes no boost (M3).
- **Intra-result reinforcement** (the deferred M3 §11 item): proximity seeds expand from top-1 to candidates that neighbor *other* strong fused hits — so a memory linked to several of the result set's strong hits gets the tilt, not only neighbors of the single top hit. Const-gated; additive to M3's boost math.

---

## 8. The evolve-loop runtime (`evolve.rs`)

- **Trigger:** debounced-on-append (a new memory schedules a tick after `EVOLVE_DEBOUNCE`) + an idle tick + a manual `evolve_once()` for tests/CLI. The loop processes up to `EVOLVE_BATCH` memories per tick.
- **Off-switch:** a `config` event (`evolve_enabled=false`) hard-disables the loop; honored before any model call (parent §5.9).
- **Resource policy:** idle/charging-aware throttle + per-tick rate-limit (parent §5.9, open-Q #4). v1 thresholds are conservative consts (§11), surfaced for tuning.
- **Observability:** `last_tick`, queue depth (events behind the cursor), error counts, last error — exposed for the desktop (parent §15). A surfaced "waiting for local model" state covers first-run (§10).
- **Single-writer respect:** all emits go through `EventLog::append` (serialized). The loop holds an `Embedder` + a `Reasoner` + a read handle for recall/graph; it never opens a second writer.
- **Depends on:** `events` (serialized append), `recall` (+`embed`, +`index`, +`keyword`), `graph`, `reason`, a scheduler. *(Parent §5.9 listed events/graph/reason/scheduler; M4a adds `recall`+`embed` — the retrieval-augmented strengthening.)*

---

## 9. Components & file layout
- **`src/reason.rs` (new):** the `Reasoner` trait + `ScriptedReasoner` + the extraction/adjudication JSON schemas (pure types + a deterministic double).
- **`src/ollama.rs` (new, feature `ollama`):** `OllamaReasoner` — the only I/O (loopback HTTP). Behind the feature so the default build stays pure.
- **`src/extract.rs` (new, PURE):** prompt construction (cheat sheet + vocab + few-shot), response parsing, the reflexion state machine, self-verify. Takes recall/graph results as inputs, calls the `Reasoner` trait → unit-testable with `ScriptedReasoner`. Mirrors the pure split in `recall.rs`/`graph.rs`.
- **`src/evolve.rs` (new):** the SQL/IO loop runtime (cursor, tick orchestration, scheduler, off-switch, throttle, observability). Same ownership pattern `log.rs` uses.
- **`src/graph.rs` / `src/log.rs` (extend):** the `entity` event helper + `entities` projection + the `edges` origin/confidence fold columns + `neighbors`-by-entity + the trust-gated boost + intra-result reinforcement.
- **`src/recall.rs` (extend):** the new named consts; the trust-gate predicate + reinforcement seeding in the boost.
- **Tests:** `tests/evolve.rs` (new, hermetic, `ScriptedReasoner`); `tests/extract.rs` (new, pure); `tests/entity_resolution.rs` (new); extend `tests/graph.rs` + `tests/recall.rs`; `tests/live_ollama.rs` (new, `#[ignore]`).

---

## 10. Error handling
- Typed `BossclawError` (`thiserror`); no panics in library code.
- **Reasoner error / malformed JSON** → drop that proposal (or the whole tick), log, retry next tick. Schema-constrained decoding + a bounded re-ask makes this rare.
- **First-run / no Ollama installed** (parent §10): ingest + recall + manual links keep working; the loop **queues** and surfaces "waiting for local model" — never blocks, never silently drops.
- **Graph/recall error mid-tick** → the tick is a no-op (best-effort), recall + storage unaffected.
- **Append error** (signing/chain/Tier-B empty-source rejection) propagates and aborts that tick's batch; the cursor only advances on a clean commit.

---

## 11. Named constants (no magic numbers; all dogfooding-tunable)
| Const | Value | Rationale |
|---|---|---|
| `RESOLVE_HIGH` | `0.92` | Cosine auto-merge floor. High — a wrong entity merge is expensive to undo. |
| `RESOLVE_LOW` | `0.75` | Below ⇒ mint a new entity. Between LOW/HIGH ⇒ the model adjudicates. |
| `MAX_REFLECT` | `2` | Reflexion passes (propose + 1 critique). Bounds model calls per memory. |
| `TRUST_MIN` | `0.6` | Min machine-edge confidence to contribute the recall boost / be actuator-eligible. |
| `GRAPH_CONTEXT_K` | `8` | Recalled neighbors fed as the Pass-A cheat sheet. |
| `EVOLVE_BATCH` | `16` | Max memories processed per tick (bounds tick latency). |
| `EVOLVE_DEBOUNCE` | `2000 ms` | Debounce after an append before a tick (coalesce bursts). |

---

## 12. Testing strategy (both truths coexist — the project invariant)
**Hermetic determinism suite (CI, `ScriptedReasoner`):**
- **Tier-A byte-identical rebuild** still holds with `entity` events + the new `edges` columns + `entities`/`evolve_cursor` (snapshot → `rebuild_graph` → assert identical; the M3 §9 standard).
- **Idempotency:** running a tick twice over the same memories emits nothing new (entity resolution reuses; active edge-keys are skipped).
- **Entity resolution** with fixed (mock) embeddings: above-`HIGH` merges, below-`LOW` mints, mid-band routes to the (scripted) adjudicator.
- **Contradiction:** a scripted retraction against an active edge emits exactly one `invalidate` that closes it; recall stops boosting it.
- **Trust gate:** a low-confidence machine edge is recorded but yields no boost; a manual or ≥`TRUST_MIN` edge does.
- **Provenance:** every emitted event's `source_event_ids` equals the actual cheat-sheet read-set, non-empty (F2).
- Hermetic temp homes, `clippy --all-targets -D warnings`, zero `unsafe`.

**Live-Ollama behavioral gate (`#[ignore]`, local must-run; the M4a `recall@3`):**
- Runs the real `qwen2.5:7b-instruct` against a small fixture corpus and asserts *properties, not bytes*: a memory naming a person yields ≥1 `entity`; a stated relationship yields a `link` with a `supported_by` span; a contradiction across two memories yields an `invalidate`; re-running is idempotent. We run it live this session to dogfood.

---

## 13. Build sequence (TDD milestones; each demoable)
1. **`Reasoner` seam:** the trait + `ScriptedReasoner` + the JSON schemas; the `OllamaReasoner` behind the `ollama` feature (talks to live Ollama, schema-constrained, digest-pinned, loopback-guarded).
2. **`entity` event + `entities` projection + entity-node fold** (`kind="entity"`) + the byte-identical-rebuild test.
3. **Entity resolution** (embed → top-K → thresholds → scripted adjudication) on the `entities` projection.
4. **`extract.rs` Pass A** (propose: cheat-sheet prompt + vocab + few-shot + parse) against `ScriptedReasoner`.
5. **`extract.rs` Pass B** (critique/self-verify + contradiction confirmation against current edges) — `MAX_REFLECT`.
6. **`edges` origin/confidence columns + the trust-gated recall boost + intra-result reinforcement** (extend the M3 fold + `recall`).
7. **`evolve.rs` runtime:** cursor + `evolve_once()` end-to-end (recall → extract → resolve → emit → fold) + idempotency; then the scheduler/debounce + off-switch + resource policy + observability.
8. **The live-Ollama gate** + CHANGELOG + final gates (hermetic suite green, clippy clean, zero `unsafe`); **dogfood live** against Peter's real memories.

---

## 14. Deferred / carried
- **M4b — Summarizer:** `page` Tier-B events + `pages` projection + `supersede`, reusing `evolve.rs`. **OKF forward note (2026-06-16):** shape the M4b `page` frontmatter to be **Open Knowledge Format-compatible** (`type/title/description/tags/timestamp` + markdown body + cross-links) so pages are export-ready at ~zero cost. OKF (Google, v0.1, 2026-06-12) is the "LLM-wiki" markdown-bundle interchange standard; it is an **export/interchange target, never the store** (it is plaintext on disk — the opposite of our encrypted signed log). See **§15 export** below + the GBrain landscape note.
- **OKF as an export format** for the parent §15 signed-portable-export (a user-initiated decrypt-and-emit; preserves the encrypted-at-rest moat). Candidate, decided at the export milestone.
- **AIR capability ontology** (strategic #7) — distinct from M4a's relation vocabulary.
- **Cloud-frontier escalation** (`CloudReasoner`, §5.8) for rare hard synthesis.
- **Proactive surfacing** ("you may want to revisit X").
- **14b/quality upgrade:** non-destructive (model_id-tagged); re-extraction over old events on upgrade is a later option.
- **2-hop neighborhood** in the cheat sheet (v1 = 1-hop, like M3).

## 15. Honesty lines
- **A 7b is Haiku-class, not frontier.** M4a is clever *because of the system around the model* (recall + graph as working memory, reflexion, resolution, the trust gate), not because the model is smart. The design treats the model as **fallible by construction**: invalidate-not-delete, confidence/trust-gating, human-gated writes (M6). A wrong link is contained, not catastrophic.
- **Extraction is non-deterministic; only the machinery is tested for determinism.** The live behavioral gate proves *properties*, never byte-identity (parent §4/§11). Tier-B events are replayed, never recomputed.
- **The graph's value at scale is empirical.** M3 proved the mechanism; M4a gives it a hit-rate. Whether the linked corpus *feels* smart is a dogfooding question answered with the M7 desktop, not a promised number.

## 16. Provenance / integrity contracts (continues M3 §12.1 / parent §16)
- **The non-manual producer passes its real read-set.** `OllamaReasoner`-produced `entity`/`link`/`invalidate` events set `source_event_ids` to the actual cheat-sheet inputs (source + recalled + neighbors), non-empty — never the manual `[src,dst]` default (which is manual-only). This keeps the §5.11 fail-closed taint-lineage walk honest: a machine link's lineage always reaches the inducing memory.
- **`model_id` is provenance, not trust.** Trust derives from `source_event_ids` lineage + (future) the user-DID signer + the confidence/origin gate — **never** from the literal `model_id` string. "machine ⇒ untrusted-until-gated"; "manual ⇒ engine/test-asserted, not user-authored" (M3 §12.1).
- **`signed_by_did` is still UNVERIFIED** (carried from M3): `verify_chain` checks only the engine key. Before any user-facing ownership claim (M7), verify MUST resolve DID→pubkey. M4a adds no new ownership claim.
- **The trust gate is now active, not deferred.** M3 noted "the boost has no edge-trust gate yet"; M4a adds it (§7) because links are now machine-derived.

## 17. Parent-design deviations flagged (all additive strengthenings)
1. **New `entity` Tier-B event type** (beyond parent §5.2's list) — first-class, signed, provenance-bearing entity records. Consistent with Tier-B discipline; does not widen the `events` row schema (parent §7 "derive richer structures").
2. **`OllamaReasoner` in-core behind the `ollama` feature** (the §4 diagram drew backends app-side) — matches M2's `fastembed`-behind-a-feature; lets the live gate live in-crate; default build stays pure.
3. **`evolve` depends on `recall`+`embed`** (§5.9 listed events/graph/reason/scheduler) — the retrieval-augmented cleverness; a strengthening, not a conflict.
4. **`confidence` in `link` content** (not in `ModelMeta`) — keeps the frozen `ModelMeta`/canonicalization untouched while making edges trust-gatable.
