# bossclaw-core — Milestone 4a (Clever Linker) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax. Build TDD: failing test first, then implementation.

**Goal:** Turn the M3 graph from a hand-linked skeleton into an **auto-populating curator**. A local LLM (`qwen2.5:7b-instruct` via Ollama) reads each new memory, extracts entities + typed relationships, resolves entities against the existing graph, retires contradicted facts, and appends signed Tier-B `entity`/`link`/`invalidate` events through the existing single-writer `EventLog::append` — which feeds the M3 fold, which makes the next recall smarter. Plus the always-on evolve-loop runtime, the edge-trust gate on the recall boost, and a live-model behavioral gate.

**Architecture:** A thin backend-agnostic `Reasoner` seam (`reason.rs`) with a deterministic `ScriptedReasoner` (hermetic tests) and a feature-gated `OllamaReasoner` (`ollama.rs`, real default). Pure extraction logic (`extract.rs`): retrieval-augmented prompt construction, response parsing, the 2-pass reflexion state machine, entity-resolution thresholds. The SQL/IO loop runtime (`evolve.rs`): a persistent `evolve_cursor`, `evolve_once()` tick orchestration, scheduler/debounce, hard off-switch, resource policy, observability. New `entity` Tier-B events fold into an `entities` projection + `nodes(kind="entity")`; the M3 `edges` fold gains `origin`/`confidence` columns (pure functions of event fields → byte-identical rebuild still holds). The closed loop — recall (M2) → extract (LLM) → resolve → graph (M3) → richer recall — is the whole point: a 7b punches above its weight only because M2+M3 are its working memory. The reasoner's output is **data, never authority**: invalidate-not-delete, confidence/trust-gating, every emit through the serialized writer.

**Tech Stack:** Rust 2021 · `rusqlite` (`bundled-sqlcipher`) · `chrono` · `ulid` · `serde_json` · `sha2` (scripted-reasoner key hash, already a dep). New: `ureq` (minimal blocking HTTP, behind the `ollama` feature only — default build stays pure, no network dep). Builds on M1's `EventLog`/`append`/`ModelMeta`, M2's `Embedder`/`recall`/`HnswIndex`, M3's `link`/`invalidate`/`rebuild_graph`/`neighbors`/`append_graph_event` (F2 producer gate) and the recall graph-proximity boost.

**Spec:** `docs/superpowers/specs/2026-06-16-bossclaw-core-m4a-clever-linker-design.md` (addendum to `...-bossclaw-core-design.md` §5.8 reasoner + §5.9 evolve + §12.4 milestone 4). Implements M4a §3 closed loop, §4 events/data model, §5 reasoner seam, §6 cleverness mechanics, §7 confidence/trust-gate/reinforcement, §8 evolve runtime, §11 constants, §12 testing, §13 build sequence.

> ⚠️ **Rev 2 (2026-06-16):** two independent reviewers (critic → SHIP-WITH-FIXES; security → NO-SHIP-until-3-criticals → then SHIP-WITH-FIXES) reviewed this plan. Read **"Rev 2 — folded second-opinion review fixes"** below FIRST — its snippets SUPERSEDE the inline task snippets where they overlap. Peter's decision on the one reviewer fork (Pass B): **model-driven critique + pure fail-closed floor + lineage invariant** (F1).

---

## Design decisions (locked in the spec; do not re-derive)
1. **Real backend, first-class; hermetic via fixtures.** Default `Reasoner` is `OllamaReasoner` (`qwen2.5:7b-instruct`, loopback `127.0.0.1:11434`, `format`=schema, `temperature 0`, digest-pinned, no egress). The deterministic `ScriptedReasoner` is the ONLY way to test the byte-identical Tier-A layer (a live LLM has no byte-identity); the real model is proven by a separate `#[ignore]` live gate (the M4a analogue of M2's `recall@3`). CI stays hermetic. (spec §2.1, §2.2)
2. **The closed loop IS the architecture.** Before extracting from memory `M`: recall (M2) semantically-similar memories + pull the M3 graph neighborhood of the entities → the model's "cheat sheet". recall → extract → graph → better recall. Only possible because M2+M3 exist. (spec §2.3, §3)
3. **Provenance falls out of the cleverness.** The cheat-sheet inputs (source memory + recalled + graph-neighbor events the model actually saw) **are** the `source_event_ids` on every emitted event — non-empty, the real read-set, never the manual `[src,dst]` default. The cleverness mechanism and M3's F2 taint-lineage contract are the same list. (spec §2.4, §16)
4. **Entities are first-class, signed, resolvable.** A new `entity` Tier-B event mints a stable `entity:<ulid>` node carrying `{label, aliases, entity_type}`; it folds into `nodes(kind="entity")` + an `entities` projection and is embedded for resolution. The **label is a property, never the id** (names collide and change; the id does not). Re-seeing "Kenny" resolves to the existing node (no duplicate Kennys). (spec §4, §5)
5. **`confidence` lives in the signed `link` CONTENT, never in `ModelMeta`.** Changing the frozen `ModelMeta` struct would alter canonicalization/signing for ALL Tier-B events. `link.content` extends to `{src, relation, dst, confidence?}`; the M3 `parse_link_content` path gains an optional confidence read (absent ⇒ `NULL`; keeps byte-identical rebuild). (spec §4, §7, §17.4, locked constraint)
6. **New Tier-A folds stay byte-identical on rebuild.** `entities` and the new `edges.origin`/`edges.confidence` columns are pure functions of event fields, added to the fold's `CREATE TABLE` + populated by the fold itself — no `ALTER`/migration. `evolve_cursor` is persistent processing progress, **NOT a fold** (losing it only re-processes events, idempotently). (spec §4, §12)
7. **The evolve loop is NOT a privileged writer.** Every emit (`entity`/`invalidate`/`link`) goes through `EventLog::append` (the single serialized writer). Non-manual producers MUST pass explicit non-empty `source_event_ids` — an empty non-manual source set is rejected (the M3 F2 guard, extended to `entity`/machine `link`/`invalidate`). (spec §3.6, §8, §16, locked constraint)
8. **recall EXCLUDES `entity`-kind events; resolution searches ONLY `entity`-kind.** `entity` events are embedded for resolution but must never surface in `recall` (kind filter on the vector/keyword arms' candidate set); the resolution vector search is a dedicated entity index (or kind-filtered). (spec §3.1, §4, §6, locked constraint)
9. **Trust gate is now active.** M3's graph-proximity recall boost gains a predicate: only `origin='manual'` OR (`origin='machine'` AND `confidence ≥ TRUST_MIN`) edges contribute the boost. Low-confidence machine edges are still recorded (never-forget) and queryable, but do not tilt recall. (spec §7, §16)
10. **Intra-result reinforcement** (the deferred M3 §11 item): proximity seeds expand from top-1 to candidates that neighbor *other* strong fused hits. Const-gated; additive to M3's boost math. (spec §7)
11. **Degrade, never break.** Any reasoner/graph error makes the tick a no-op that retries; recall + storage are never broken by the evolve loop (mirrors M2's keyword-only degrade). First-run / no-Ollama: ingest + recall + manual links keep working; the loop queues + surfaces "waiting for local model". (spec §2.7, §10)
12. **Maximal cleverness, bounded by named constants.** 2-pass reflexion (`MAX_REFLECT`), conservative resolution thresholds (`RESOLVE_HIGH`/`RESOLVE_LOW`), a confidence trust-gate (`TRUST_MIN`), `GRAPH_CONTEXT_K`/`EVOLVE_BATCH`/`EVOLVE_DEBOUNCE` — all const-sourced, no magic numbers (spec §11), tunable in dogfooding.
13. **`#![forbid(unsafe_code)]` + `#![deny(missing_docs)]`** crate-wide (already in `lib.rs`): every `pub` item needs a doc comment; zero `unsafe`.

---

## Named constants (no magic numbers; spec §11)
All live in `src/extract.rs` (resolution/reflexion) or `src/evolve.rs` (loop), re-exported where tests need them. Each carries a sourced-comment rationale (the doc comment shown in the implementing step).

| Const | Value | Module | Rationale |
|---|---|---|---|
| `RESOLVE_HIGH` | `0.92` | `extract` | Cosine auto-merge floor. High — a wrong entity merge is expensive to undo. |
| `RESOLVE_LOW` | `0.75` | `extract` | Below ⇒ mint a new entity. Between LOW/HIGH ⇒ the model adjudicates. |
| `MAX_REFLECT` | `2` | `extract` | Reflexion passes (propose + 1 critique). Bounds model calls per memory. |
| `TRUST_MIN` | `0.6` | `extract` | Min machine-edge confidence to contribute the recall boost / be actuator-eligible. |
| `GRAPH_CONTEXT_K` | `8` | `extract` | Recalled neighbors fed as the Pass-A cheat sheet. |
| `EVOLVE_BATCH` | `16` | `evolve` | Max memories processed per tick (bounds tick latency). |
| `EVOLVE_DEBOUNCE` | `2000` (ms) | `evolve` | Debounce after an append before a tick (coalesce bursts). |

---

## Rev 2 — folded second-opinion review fixes (2026-06-16)

Two independent adversarial reviewers (critic + security) reviewed the pre-review plan (`3674d37`). Apply ALL of the following; where a fix overlaps an inline task snippet below, **the Rev 2 snippet supersedes it.** Severity tags: 🔴 Critical (blocks execution), 🟠 Major/Important, 🟡 Minor.

### Code fixes

**F1 🔴 (critic C1 — Pass B is real reflexion + a pure floor) [Task 5 + Task 7].** As drafted, Pass B is a substring check (`source.contains(&supported_by)`) and `PASS_B_SYSTEM`/`MAX_REFLECT` are dead code — the "maximal clever" headline is hollow. Peter's decision: **model-driven Pass B over a pure fail-closed floor.** Keep the pure span-verify (it can only DROP), then run ONE model critique turn and INTERSECT: the model may remove or down-confidence a proposal but can NEVER add an edge the floor didn't already support.
```rust
// extract.rs — pure: the model's critique may only subtract from the floor-verified set.
/// Keep a relation iff BOTH the pure floor verified it AND the model's critique
/// returned it (by (src,relation,dst) identity). The model can down-confidence
/// (take the min) but never introduce an unsupported edge. Same for retractions.
pub fn intersect_keep_floor(floor: &Extraction, critique: &Extraction) -> Extraction { /* … */ }

// evolve_once — after `verified` (pure floor), before resolution→emit:
let critique = self.reasoner.complete_json(
    extract::PASS_B_SYSTEM,
    &extract::build_pass_b_prompt(&mem_text, &verified, &neighborhood),
    &extract::extraction_schema(),
)?;
let refined = extract::intersect_keep_floor(&verified, &extract::parse_extraction(&critique)?);
```
`MAX_REFLECT` now bounds the propose↔critique turns (v1 = 1 propose + 1 critique). Tests (Task 5): a `ScriptedReasoner` keyed on the Pass-B `(system,prompt)` — (a) a model DROP is honored; (b) a model-INVENTED relation is rejected by the floor (never emitted). No dead code remains.

**F2 🔴 (security C1 — the off-switch + active-model are a privilege, not data) [Task 7].** `evolve_enabled` and `active_model` live in `config` events that the generic `append` accepts from anyone, and `evolve_enabled()` is default-open + last-writer-wins → a flag-less or forged newer `config` silently re-arms the autonomous loop or swaps the model (parent §15 recall-integrity attack).
- **(a) Sticky / fail-closed off-switch.** Once an explicit `evolve_enabled=false` exists with no *later explicit* `true`, stay disabled — a flag-less newer config must NOT flip it. Default-open only when the flag was never set.
- **(b) Typed-setter-only control config.** Add `EventLog::set_evolve_enabled(bool)` as the ONLY writer of that key (alongside the existing `set_active_model`). Document that control config must not go through a generic append in v1.
- **(c) Tolerant `active_model()`.** A `config` carrying only `evolve_enabled` must not make `active_model()` error (it currently `serde_from_value`s the whole content as `ActiveModel`, `log.rs:463`) — make the off-switch its own key-scan and make `active_model()` ignore configs lacking its fields.
- **(d) Carried code-DoD (M7):** `evolve_enabled()`/`active_model()` MUST reject a control `config` whose signer DID ≠ the resolved user owner (today `signed_by_did` is unverified — M3 §12.1). Surface enable/model changes in `EvolveStatus`.

**F3 🔴 (critic C3 + M4 — integer-milli confidence, never a raw f32) [Tasks 5/6/7].** Signing a raw `f32` into JCS content is a cross-version determinism hazard (`json!(f32)`→f64→JCS shortest-round-trip): a future `serde_jcs` could change the canonical bytes and break `verify_chain` on an append-only signed store. Store confidence as an integer 0–1000 in the signed content:
```rust
// link_machine content: integer milli — ONE canonical JCS form, no f32/f64 ambiguity.
content["confidence_milli"] = json!((conf.clamp(0.0, 1.0) * 1000.0).round() as i64);
```
The fold reads it to `edges.confidence_milli INTEGER` (NULL for manual). The trust gate compares integers with the threshold **bound as a parameter** (never `format!`-ed into SQL): `WHERE origin='manual' OR (origin='machine' AND confidence_milli >= ?)` with `(TRUST_MIN*1000.0) as i64` = `600`. `TRUST_MIN` stays a documented `f32` used only to derive that integer. (Resolves M4's float-into-SQL fragility too.)

**F4 🔴 (critic M2+M3 — resolve mention→id BEFORE any graph-key comparison) [Tasks 5/7].** The drafted `confirm_retractions` compares the model's raw mention strings against `active_edge_keys()` (resolved `entity:<ulid>` ids) → they never match → **no `invalidate` ever fires and the live gate fails.** Build `mention_to_id` over EVERY distinct `src`/`dst` in `relations` AND `retractions` (not just `entities[]`), resolve each through the SAME resolver, and remap every relation/retraction endpoint to its resolved id BEFORE `confirm_retractions` and before `link_machine`. (Test T-D.)

**F5 🟠 (critic M1 / security #9 — within-tick idempotency) [Task 7].** `active` (the dedup set) is snapshotted once before the batch loop and never updated, so two memories in one tick asserting the same edge both emit. Seed an in-loop `HashSet` from `active` and `insert` each emitted key; dedup against it. (Test T-C.)

**F6 🟠 (security #5 — in-crate resource fail-safes) [Task 7 + const].** A huge/booby-trapped memory must not flood the loop. Add `MAX_ENTITIES_PER_MEMORY = 32` (cap entities accepted from one memory), cap the input text length fed to the reasoner (truncate a >N-byte memory), and call `rebuild_entity_index` ONCE after the batch (or incremental `add`) — not per-memory (it's currently O(memories×entities) inside the loop). The *running scheduler / battery-thermal throttle* stays M7; these caps ship in M4a.

**F7 🟠 (critic C2 — fix the M3 recall test the reinforcement change breaks) [Task 6].** Widening auto-seed top-1→`GRAPH_REINFORCE_TOPK=3` makes `recall_graph_proximity_explicit_seeds_boost_over_autoseed` (`recall.rs`) fail: with a 3-memory corpus all three become seeds, so the explicit-seed-over-autoseed premise collapses. Enlarge that test's corpus to ≥6 memories so the explicit seed is OUTSIDE the top-`GRAPH_REINFORCE_TOPK` fused hits — preserve the contract, don't weaken the assertion.

**F8 🟠 (security #10/#11 — loopback + digest hardening) [Task 1].** (a) Drop bare `"localhost"` from the loopback allowlist (hosts-file/DNS-rebind risk) — require a numeric loopback IP or resolve-and-assert `.is_loopback()`. (b) Surface the resolved model **digest** into `model_id()` (provenance records which blob produced each event; makes the 7b→14b "non-destructive upgrade" honest) and document the `qwen2.5:7b-instruct@sha256:…` production form. (c) Add a WIRED refusal test (`with_url("http://10.0.0.5:11434")` + `complete_json` → `Err(Reasoner)`) — the pure `is_loopback_url` unit test does not prove the guard is on the request path.

**F9 🟡 (critic m2/m3/m4 — prompt fidelity + scope) [Task 4].** (a) Actually include the few-shot exemplars in `build_pass_a_prompt` (the spec promises them; they materially help a 7b) and assert their presence in the prompt test. (b) Teach the prompt when to use `works_at` vs single-valued `works_at_primary` (fold cardinality into the relation explanations) or contradictions rarely trigger. (c) State explicitly that M4a processes `memory` events only; `file_ingested` extraction is deferred.

### Additional required tests (hermetic unless noted)

**T-A 🔴 (security C2 — injection / confused-deputy containment) [Task 7, `tests/evolve.rs`].** The first LLM-over-untrusted-content path MUST prove containment (parent §8.4/§11). A `ScriptedReasoner` simulates a model that "obeyed" an injected instruction in the memory text; assert the emitted edge is (1) `origin='machine'` (never manual), (2) its `source_event_ids` lineage REACHES the malicious memory (visible to the §5.11 walk), (3) it does NOT contribute the recall boost unless ≥`TRUST_MIN`, and (4) NO `config`/control event was emitted (no privilege escalation).

**T-B 🔴 (security C3 — lineage invariant) [Task 7].** For every emitted `entity`/`link`/`invalidate`, assert every `source_event_ids` entry resolves to a real `events` row AND none start with `entity:` (event ids, never node ids). Carried code-DoD (M6): the §5.11 taint walk fails CLOSED (untrusted-origin) on an unresolvable lineage id — never skips it.

**T-C 🟠 (within-tick idempotency, F5) [Task 7].** Two memories asserting the same edge in one `evolve_once` → exactly one edge.

**T-D 🔴 (resolved-id retraction, F4) [Task 7].** Seed an active machine edge on resolved ids; script a retraction whose mentions resolve to those ids → exactly one `invalidate` fires. (Guards the contradiction-retirement the live gate asserts.)

**T-E 🟠 (security #7 — confidence is signed) [Task 6].** Append a machine `link` (`confidence_milli=900`); directly UPDATE the stored payload's confidence to `100`; assert `verify_chain()` → `Err`.

**T-F 🟠 (security #6 — SQLi regression on the new label paths) [Tasks 2/6].** A malicious entity label + alias (`Robert"); DROP TABLE entities; --`) and a malicious machine relation label round-trip as inert literal data (mirrors M3's T-E for `entities` + the machine-`link` path).

**T-G 🟠 (security #8 — recall entity-exclusion e2e) [Tasks 3/8].** Mint an `entity` whose label is a verbatim copy of a query; rebuild the recall + entity indexes; assert `recall(query)` returns the memory and NEVER the `entity:<ulid>`. **Also fix the plan prose:** entity-exclusion is **by construction** (entity events are non-embeddable per `EMBEDDABLE_EVENT_TYPES`, verified in `log.rs:105`) — NOT a new post-hoc filter. Don't claim a filter that isn't added; prove the by-construction property instead.

**T-H 🟠 (security #4 — trust-gate ZERO contribution) [Task 6].** Strengthen the trust-gate test: assert a low-confidence machine edge's neighbor scores EXACTLY its no-edge baseline (retire the edge → compare equal), proving zero contribution — not merely "less than `s_high*1.2`".

### Provenance / integrity contracts (recorded here + spec §16; carried to M4b/M6/M7)
- **Node ids ≠ event ids.** `source_event_ids` are always EVENT ids; an `entity:<ulid>` node id must never enter lineage (T-B). A later `EventId`/`NodeId` newtype would make this a compile error (carried).
- **`config` is a privilege.** The evolve on-switch + active model live in `config`; v1 = typed-setter-only; M7 MUST verify the control config's signer DID == the resolved user owner before honoring it (forged/replayed config = recall-integrity attack, parent §15).
- **Trust = `origin` + `confidence_milli` + lineage, NEVER the literal `model_id` string** (continues M3 §12.1). "machine ⇒ untrusted-until-gated."
- **Pass B can only subtract** (F1): the model critique drops/down-confidences; it never fabricates an edge the pure floor didn't support. Reflexion improves precision, never invents.
- **`signed_by_did` still UNVERIFIED** (carried): M4a adds no user-facing ownership claim; M7 resolves DID→pubkey.

### DoD honesty adjustments
- `EvolveStatus`: `queue_depth` + `enabled` are live in M4a; `last_tick_ms`/`error_count`/`last_error` are wired by M7's loop driver (the spec §8 resource-policy *enforcement* — running scheduler, idle/charging/thermal throttle — is M7; M4a ships the batch cap, the pure `debounce_due` decision, the off-switch, and the in-crate resource fail-safes F6).
- New const for §11: `MAX_ENTITIES_PER_MEMORY = 32` (`extract`).

---

## File structure
| File | Responsibility |
|---|---|
| `crates/bossclaw-core/src/reason.rs` (**new**) | The `Reasoner` trait (`complete_json` + `model_id`); the deterministic `ScriptedReasoner` test double (canned JSON keyed by SHA-256 of `(system, prompt)`); the extraction + adjudication JSON-schema builders (`extraction_schema()`, `adjudication_schema()`). Pure types + a deterministic double — no I/O. |
| `crates/bossclaw-core/src/ollama.rs` (**new**, feature `ollama`) | `OllamaReasoner` — the only I/O (loopback HTTP POST to `/api/chat`, `format`=schema, `options.temperature=0`, digest-pinned model tag, non-loopback host refused). Behind the feature so the default build stays pure (no network dep). |
| `crates/bossclaw-core/src/extract.rs` (**new**, PURE) | Resolution thresholds + reflexion/loop consts; entity-resolution decision (`resolve_decision`); Pass-A prompt construction (cheat sheet + relation vocabulary + few-shot) + response parsing → `Proposals`; Pass-B critique/self-verify against current edges; the `RELATION_VOCAB` seed set + `RELATION_CARDINALITY` single-valued table. Takes recall/graph results as inputs, calls the `Reasoner` trait → unit-testable with `ScriptedReasoner`. |
| `crates/bossclaw-core/src/evolve.rs` (**new**) | `evolve_cursor` table helpers; `evolve_once()` end-to-end tick (recall → Pass A → resolve → augment → Pass B → emit via `append` → advance cursor) + idempotency; scheduler/debounce, idle tick, `evolve_enabled` off-switch, resource policy, `EvolveStatus` observability. |
| `crates/bossclaw-core/src/lib.rs` (modify) | `pub mod reason; pub mod extract; pub mod evolve;` + `#[cfg(feature="ollama")] pub mod ollama;` + re-exports `pub use reason::{Reasoner, ScriptedReasoner}; pub use evolve::EvolveStatus;` + M4a crate-doc line. |
| `crates/bossclaw-core/src/error.rs` (modify) | New `Reasoner(String)` variant. |
| `crates/bossclaw-core/src/graph.rs` (modify) | `entity` content parser (`parse_entity_content`); `Entity` type + `entities` fold (`fold_entities`); `parse_link_content` → add an optional `confidence` read returning `(src, relation, dst, Option<f32>)`; `MANUAL_LINK_PRODUCER` reused; `origin_of` helper (`'manual'` iff `model_id == MANUAL_LINK_PRODUCER`). |
| `crates/bossclaw-core/src/log.rs` (modify) | `entities`/`evolve_cursor` DDL in `open`; `EventLog::entity(...)` append helper (non-manual producer → F2-gated); `entity` vector derivation + a dedicated entity vector read; extend `rebuild_graph` to fold entities + the `edges` origin/confidence columns + mark entity-node kinds; `entity_search` (kind-filtered resolution search); recall kind-exclusion of entity events; the trust-gate predicate + intra-result reinforcement in the recall boost; `evolve_cursor` read/write. |
| `crates/bossclaw-core/src/recall.rs` (modify) | `GRAPH_REINFORCE_TOPK` const (intra-result reinforcement seed count). |
| `crates/bossclaw-core/tests/reason.rs` (**new**) | `ScriptedReasoner` determinism + schema-builder shape. |
| `crates/bossclaw-core/tests/extract.rs` (**new**, pure) | Pass-A parse, Pass-B critique, cardinality contradiction, `MAX_REFLECT` cap, resolution thresholds. |
| `crates/bossclaw-core/tests/entity_resolution.rs` (**new**) | Embed + threshold + scripted adjudication on the `entities` projection. |
| `crates/bossclaw-core/tests/evolve.rs` (**new**, hermetic) | `evolve_once` end-to-end, idempotency, cursor persistence, byte-identical rebuild with entities, off-switch, provenance, contradiction, trust gate. |
| `crates/bossclaw-core/tests/graph.rs` (modify) | Entity fold + byte-identical-rebuild-with-entities; `edges.origin`/`edges.confidence` columns. |
| `crates/bossclaw-core/tests/recall.rs` (modify) | Entity-kind excluded from recall; trust-gate boost; intra-result reinforcement. |
| `crates/bossclaw-core/tests/live_ollama.rs` (**new**, `#[ignore]`) | Property assertions against the real model. |
| `crates/bossclaw-core/Cargo.toml` (modify) | `ollama` feature + `ureq` (optional, gated). |
| `crates/bossclaw-core/CHANGELOG.md` (modify) | M4a entry. |

The reasoner seam, the entity-resolution thresholds, the reflexion state machine, the `edges` origin/confidence fold + trust gate, and the `evolve_once` orchestration are the load-bearing pieces — everything else is wiring.

---

## Task 1: Reasoner seam (`reason.rs` + `ollama.rs` + the `ollama` feature)

**Files:**
- Create: `crates/bossclaw-core/src/reason.rs`, `crates/bossclaw-core/src/ollama.rs`
- Modify: `crates/bossclaw-core/src/lib.rs`, `crates/bossclaw-core/src/error.rs`, `crates/bossclaw-core/Cargo.toml`
- Test: `crates/bossclaw-core/tests/reason.rs` (new)

- [ ] **Step 1 — write the failing test** (`tests/reason.rs`):

```rust
//! Tests for the M4a reasoner seam: the deterministic `ScriptedReasoner` double
//! and the JSON-schema builders. The live backend is proven separately by the
//! `#[ignore]` gate in `tests/live_ollama.rs`.

use bossclaw_core::reason::{
    adjudication_schema, extraction_schema, Reasoner, ScriptedReasoner,
};
use serde_json::json;

#[test]
fn scripted_reasoner_returns_canned_json_keyed_by_system_and_prompt() {
    let canned = json!({ "entities": [], "relations": [], "retractions": [] });
    let reasoner = ScriptedReasoner::new("test-scripted-v1")
        .with_response("SYS", "PROMPT-A", canned.clone());

    // Exact (system, prompt) match → the canned value, byte-for-byte.
    let got = reasoner
        .complete_json("SYS", "PROMPT-A", &extraction_schema())
        .unwrap();
    assert_eq!(got, canned);

    // Deterministic: a second identical call yields the identical value.
    let again = reasoner
        .complete_json("SYS", "PROMPT-A", &extraction_schema())
        .unwrap();
    assert_eq!(again, canned);

    // model_id is the configured stamp.
    assert_eq!(reasoner.model_id(), "test-scripted-v1");
}

#[test]
fn scripted_reasoner_errors_on_unknown_prompt() {
    let reasoner = ScriptedReasoner::new("test-scripted-v1");
    let err = reasoner
        .complete_json("SYS", "UNSEEN", &extraction_schema())
        .expect_err("an unscripted (system,prompt) must error, not hang or panic");
    assert!(
        matches!(err, bossclaw_core::BossclawError::Reasoner(_)),
        "unknown prompt must surface as BossclawError::Reasoner, got {err:?}"
    );
}

#[test]
fn extraction_schema_constrains_the_three_proposal_arrays() {
    let schema = extraction_schema();
    // Object schema with the three top-level proposal arrays the prompt asks for.
    assert_eq!(schema["type"], json!("object"));
    let props = &schema["properties"];
    for key in ["entities", "relations", "retractions"] {
        assert_eq!(props[key]["type"], json!("array"), "{key} must be an array");
    }
    // A relation item carries the supported_by span + confidence the parser reads.
    let rel_item = &props["relations"]["items"]["properties"];
    assert!(rel_item.get("src").is_some());
    assert!(rel_item.get("relation").is_some());
    assert!(rel_item.get("dst").is_some());
    assert!(rel_item.get("confidence").is_some());
    assert!(rel_item.get("supported_by").is_some());
}

#[test]
fn adjudication_schema_constrains_a_single_choice() {
    let schema = adjudication_schema();
    assert_eq!(schema["type"], json!("object"));
    // The adjudicator answers "which candidate (or none)" → a string match id.
    assert!(schema["properties"].get("match").is_some());
}
```

- [ ] **Step 2 — run, verify it fails**

Run: `cargo test -p bossclaw-core --test reason -- scripted_reasoner extraction_schema adjudication_schema`
Expected: FAIL — `unresolved import bossclaw_core::reason` / `no variant Reasoner` (compile error: the module and the error variant don't exist yet).

- [ ] **Step 3 — add the `Reasoner` error variant** to `src/error.rs`. Insert after the `Embed` variant (keep the `enum` ordering: it goes last):

```rust
    /// A reasoner (LLM) backend failed: transport error, a non-loopback host was
    /// refused, malformed/un-decodable JSON, or (for the scripted test double) an
    /// unscripted `(system, prompt)`. The reasoner's output is data, never
    /// authority — this error makes the evolve tick a retryable no-op (spec §10),
    /// never corrupting the log.
    #[error("reasoner error: {0}")]
    Reasoner(String),
```

- [ ] **Step 4 — create `src/reason.rs`** (the trait + the scripted double + the schema builders, all pure):

```rust
//! The reasoner seam (spec §5): a thin, backend-agnostic interface whose output
//! is DATA, never authority. The default real backend is [`crate::ollama`]'s
//! `OllamaReasoner` (feature-gated); this module holds the trait, the
//! deterministic [`ScriptedReasoner`] test double, and the JSON-schema builders
//! that constrain the model's structured output.
//!
//! PURE: no network, no `Store`, no SQL. The only I/O reasoner lives behind the
//! `ollama` feature in [`crate::ollama`], mirroring M2's `fastembed`-behind-a-
//! feature precedent so the default build stays dependency-light and the live
//! gate can live in-crate.

use std::collections::HashMap;

use sha2::{Digest, Sha256};

use crate::error::BossclawError;

/// The thin, backend-agnostic seam (spec §5). An implementation performs a
/// schema-constrained structured completion; the engine parses the result as
/// *proposals*, never as commands (the untrusted-content fence, parent §8.4).
pub trait Reasoner: Send + Sync {
    /// Schema-constrained structured completion. `schema` constrains the JSON the
    /// model may emit; the implementation is responsible for honoring it (Ollama
    /// passes it as the `format` field). `system` is the instruction channel and
    /// `prompt` is the (fenced) data channel. Returns the parsed JSON value or
    /// [`BossclawError::Reasoner`] on transport/decoding failure.
    fn complete_json(
        &self,
        system: &str,
        prompt: &str,
        schema: &serde_json::Value,
    ) -> Result<serde_json::Value, BossclawError>;

    /// The model id stamped into every emitted event's `model_meta.model_id`
    /// (provenance, not trust — parent §16 / M3 §12.1). A 7b→14b upgrade is
    /// non-destructive: new events carry the better id; old ones stay tagged.
    fn model_id(&self) -> &str;
}

/// Deterministic, dependency-free reasoner double for the hermetic suite
/// (spec §2.2). Returns canned JSON keyed by a SHA-256 of `(system, prompt)`,
/// so a given prompt always yields the same value across toolchains — the only
/// way to test the byte-identical Tier-A layer (a live LLM has no byte-identity).
///
/// NOT a production path. Real intelligence is proven by the `#[ignore]` live
/// gate against the actual model.
pub struct ScriptedReasoner {
    model_id: String,
    /// SHA-256 hex of `system \u{1f} prompt` → the canned response.
    responses: HashMap<String, serde_json::Value>,
}

impl ScriptedReasoner {
    /// Create a scripted reasoner stamping `model_id`, with no responses yet.
    pub fn new(model_id: &str) -> Self {
        Self { model_id: model_id.to_string(), responses: HashMap::new() }
    }

    /// Register the canned `response` for an exact `(system, prompt)` pair.
    /// Builder-style so a test can chain several scripted turns.
    pub fn with_response(
        mut self,
        system: &str,
        prompt: &str,
        response: serde_json::Value,
    ) -> Self {
        self.responses.insert(Self::key(system, prompt), response);
        self
    }

    /// SHA-256 hex of `system`, a unit separator, and `prompt`. The separator
    /// (`U+001F`) cannot appear in normal text, so distinct `(system, prompt)`
    /// pairs can never collide by concatenation (`"a"+"bc"` vs `"ab"+"c"`).
    fn key(system: &str, prompt: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(system.as_bytes());
        hasher.update([0x1f]);
        hasher.update(prompt.as_bytes());
        hex::encode(hasher.finalize())
    }
}

impl Reasoner for ScriptedReasoner {
    fn complete_json(
        &self,
        system: &str,
        prompt: &str,
        _schema: &serde_json::Value,
    ) -> Result<serde_json::Value, BossclawError> {
        // The schema is intentionally ignored by the double — it exercises the
        // SAME parse path the real backend feeds, so a scripted value that the
        // parser rejects fails the test exactly as a bad real completion would.
        self.responses
            .get(&Self::key(system, prompt))
            .cloned()
            .ok_or_else(|| {
                BossclawError::Reasoner(format!(
                    "ScriptedReasoner: no canned response for this (system, prompt) \
                     [key={}]",
                    Self::key(system, prompt)
                ))
            })
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }
}

/// JSON Schema constraining the Pass-A / Pass-B extraction output (spec §6). The
/// model may emit ONLY `{entities[], relations[], retractions[]}`, each item
/// carrying the `confidence` + `supported_by` the parser reads. Passed verbatim
/// to the backend as the structured-output constraint.
pub fn extraction_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "entities": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "mention": { "type": "string" },
                        "entity_type": { "type": "string" },
                        "confidence": { "type": "number" }
                    },
                    "required": ["mention", "entity_type", "confidence"]
                }
            },
            "relations": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "src": { "type": "string" },
                        "relation": { "type": "string" },
                        "dst": { "type": "string" },
                        "confidence": { "type": "number" },
                        "supported_by": { "type": "string" }
                    },
                    "required": ["src", "relation", "dst", "confidence", "supported_by"]
                }
            },
            "retractions": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "src": { "type": "string" },
                        "relation": { "type": "string" },
                        "dst": { "type": "string" },
                        "reason": { "type": "string" },
                        "confidence": { "type": "number" }
                    },
                    "required": ["src", "relation", "dst", "reason", "confidence"]
                }
            }
        },
        "required": ["entities", "relations", "retractions"]
    })
}

/// JSON Schema constraining the entity-resolution adjudication (spec §6). When a
/// mention's cosine similarity lands in the mid-band, the model picks which
/// candidate id it matches, or the sentinel for "none". `match` is a string so
/// the result is a single, parseable choice.
pub fn adjudication_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "match": {
                "type": "string",
                "description": "the chosen candidate entity id, or \"none\""
            }
        },
        "required": ["match"]
    })
}
```

- [ ] **Step 5 — create `src/ollama.rs`** (the real backend, behind the `ollama` feature):

```rust
//! The real local reasoner backend (spec §5): `OllamaReasoner` POSTs to a
//! loopback Ollama server, schema-constrained, `temperature 0`, digest-pinned,
//! refusing any non-loopback host (parent §8.5, no egress). Behind the `ollama`
//! cargo feature so the default build is pure (no network dependency); the app
//! may still inject its own [`Reasoner`].

use crate::error::BossclawError;
use crate::reason::Reasoner;

/// Default Ollama endpoint — loopback only (parent §8.5: no network egress).
const OLLAMA_LOOPBACK_URL: &str = "http://127.0.0.1:11434/api/chat";

/// HTTP request timeout. Generous: a cold 7b on CPU can take tens of seconds for
/// the first token. A timeout (rather than an unbounded wait) keeps a wedged
/// server from hanging the evolve tick forever — the tick degrades to a no-op
/// and retries (spec §10).
const OLLAMA_TIMEOUT_SECS: u64 = 120;

/// The real reasoner: a schema-constrained, deterministic (`temperature 0`),
/// digest-pinned local model over loopback HTTP. Holds the resolved model tag
/// (e.g. `qwen2.5:7b-instruct`) stamped into every emitted event.
pub struct OllamaReasoner {
    model_tag: String,
    url: String,
    agent: ureq::Agent,
}

impl OllamaReasoner {
    /// Create a reasoner for `model_tag` (e.g. `"qwen2.5:7b-instruct"`) against
    /// the loopback Ollama server. Use a digest-pinned tag in production so the
    /// model cannot silently change under you (spec §2.1).
    pub fn new(model_tag: &str) -> Self {
        Self::with_url(model_tag, OLLAMA_LOOPBACK_URL)
    }

    /// Create a reasoner pointed at an explicit `url`. **Refuses any non-loopback
    /// host at call time** (see [`is_loopback_url`]) so a misconfiguration can
    /// never cause egress. Primarily for tests that run a loopback stub on a
    /// chosen port; production should use [`OllamaReasoner::new`].
    pub fn with_url(model_tag: &str, url: &str) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(OLLAMA_TIMEOUT_SECS))
            .build();
        Self { model_tag: model_tag.to_string(), url: url.to_string(), agent }
    }
}

impl Reasoner for OllamaReasoner {
    fn complete_json(
        &self,
        system: &str,
        prompt: &str,
        schema: &serde_json::Value,
    ) -> Result<serde_json::Value, BossclawError> {
        // Fail-closed egress guard: refuse anything that is not a loopback host
        // BEFORE any socket is opened (parent §8.5).
        if !is_loopback_url(&self.url) {
            return Err(BossclawError::Reasoner(format!(
                "refusing non-loopback reasoner host: {} (loopback-only, no egress)",
                self.url
            )));
        }
        // Ollama /api/chat with structured output: `format` = the schema,
        // `options.temperature = 0` for determinism, `stream = false` so we get
        // one complete JSON body.
        let body = serde_json::json!({
            "model": self.model_tag,
            "stream": false,
            "format": schema,
            "options": { "temperature": 0 },
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": prompt }
            ]
        });
        let resp = self
            .agent
            .post(&self.url)
            .send_json(body)
            .map_err(|e| BossclawError::Reasoner(format!("ollama transport: {e}")))?;
        let envelope: serde_json::Value = resp
            .into_json()
            .map_err(|e| BossclawError::Reasoner(format!("ollama response not JSON: {e}")))?;
        // The structured content is a JSON STRING in message.content; parse it to
        // the value the caller's schema described.
        let content = envelope
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .ok_or_else(|| {
                BossclawError::Reasoner("ollama response missing message.content".into())
            })?;
        serde_json::from_str(content)
            .map_err(|e| BossclawError::Reasoner(format!("ollama content not valid JSON: {e}")))
    }

    fn model_id(&self) -> &str {
        &self.model_tag
    }
}

/// True iff `url`'s host is a loopback address (`127.0.0.0/8`, `::1`, or the
/// literal `localhost`). The egress guard: a non-loopback host is refused before
/// any connection (parent §8.5). Best-effort parse — an unparseable URL is NOT
/// loopback (fail-closed).
fn is_loopback_url(url: &str) -> bool {
    // Strip scheme, then take the authority up to the first '/'.
    let after_scheme = url.split("://").nth(1).unwrap_or(url);
    let authority = after_scheme.split('/').next().unwrap_or("");
    // Drop an optional port (host:port). IPv6 literals are bracketed.
    let host = if let Some(rest) = authority.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        authority.split(':').next().unwrap_or("")
    };
    if host == "localhost" {
        return true;
    }
    match host.parse::<std::net::IpAddr>() {
        Ok(ip) => ip.is_loopback(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::is_loopback_url;

    #[test]
    fn loopback_hosts_are_accepted_and_others_refused() {
        assert!(is_loopback_url("http://127.0.0.1:11434/api/chat"));
        assert!(is_loopback_url("http://localhost:11434/api/chat"));
        assert!(is_loopback_url("http://[::1]:11434/api/chat"));
        assert!(!is_loopback_url("http://10.0.0.5:11434/api/chat"));
        assert!(!is_loopback_url("http://evil.example.com/api/chat"));
        assert!(!is_loopback_url("not a url"));
    }
}
```

- [ ] **Step 6 — register the modules + re-exports** in `src/lib.rs`. Add to the crate-doc comment (after the M3 line): `//! Milestone 4a (Clever Linker): the Reasoner seam + LLM auto-linker (entity/link/invalidate from memories), the evolve-loop runtime, the edge-trust gate.`. Then add the module declarations keeping alphabetical order — `pub mod evolve;` after `pub mod event;`, `pub mod extract;` after it, `#[cfg(feature = "ollama")] pub mod ollama;` after `pub mod model2vec;`, and `pub mod reason;` after `pub mod recall;`. Add to the re-export block:

```rust
pub use evolve::EvolveStatus;
pub use reason::{Reasoner, ScriptedReasoner};
```

(`OllamaReasoner` is re-exported only under the feature:)

```rust
#[cfg(feature = "ollama")]
pub use ollama::OllamaReasoner;
```

- [ ] **Step 7 — add the `ollama` feature + `ureq`** to `Cargo.toml`. In `[features]` add `ollama = ["dep:ureq"]`; add an optional dep block:

```toml
[dependencies.ureq]
version = "2"
optional = true
default-features = false
features = ["json", "tls"]
```

> NOTE: `default-features = false` + only `json`/`tls` keeps the (feature-gated) dependency minimal. The default build (no `ollama`) pulls in **zero** of this — the pure-crate invariant holds.

- [ ] **Step 8 — run, verify pass.** Run the default-feature test (proves `reason.rs` compiles + the scripted double works) AND the feature build (proves `ollama.rs` compiles + its loopback unit test passes):

Run: `cargo test -p bossclaw-core --test reason` then `cargo test -p bossclaw-core --features ollama --lib ollama::tests`
Expected: PASS (4 reason tests; the `loopback_hosts_are_accepted_and_others_refused` unit test).

- [ ] **Step 9 — commit**

```bash
git add crates/bossclaw-core/src/reason.rs crates/bossclaw-core/src/ollama.rs crates/bossclaw-core/src/lib.rs crates/bossclaw-core/src/error.rs crates/bossclaw-core/Cargo.toml crates/bossclaw-core/tests/reason.rs
git status -s
git commit -m "feat(bossclaw-core): Reasoner seam + ScriptedReasoner + feature-gated OllamaReasoner (M4a T1)"
```

---

## Task 2: `entity` event + `entities` projection + entity-node fold

**Files:**
- Modify: `crates/bossclaw-core/src/graph.rs` (`Entity` type, `parse_entity_content`, `fold_entities`), `crates/bossclaw-core/src/log.rs` (`entities` DDL, `EventLog::entity(...)`, extend `rebuild_graph` to fold entities + mark `kind="entity"`, `all_entities` read), `crates/bossclaw-core/src/lib.rs` (re-export `Entity`)
- Test: `crates/bossclaw-core/tests/graph.rs`

- [ ] **Step 1 — write the failing tests** (`tests/graph.rs`, append). Reuses the file's existing `open_log`, `mk_memory`, `DID`:

```rust
use bossclaw_core::graph::Entity;

#[test]
fn entity_appends_tier_b_event_with_explicit_sources() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let m = log.append(mk_memory("kenny works at acme")).unwrap();

    // entity() is a NON-manual producer → MUST pass explicit non-empty sources.
    let entity_id = log
        .entity("Kenny", &["Ken".to_string()], "person", "m4-reasoner", &[m.clone()])
        .unwrap();

    assert!(entity_id.starts_with("entity:"), "node id is namespaced: {entity_id}");
    let ev = log.stream_all().unwrap().into_iter()
        .find(|e| format!("entity:{}", e.id) == entity_id).unwrap();
    assert_eq!(ev.event_type, "entity");
    assert_eq!(ev.content["label"], json!("Kenny"));
    assert_eq!(ev.content["aliases"], json!(["Ken"]));
    assert_eq!(ev.content["entity_type"], json!("person"));
    let meta = ev.model_meta.expect("entity is Tier-B");
    assert_eq!(meta.model_id, "m4-reasoner");
    assert_eq!(meta.source_event_ids, vec![m]);
}

#[test]
fn entity_rejects_non_manual_producer_with_empty_sources() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    // F2 guard extended to entity: a non-manual producer with empty sources is
    // rejected (an empty default would erase the inducing memory from lineage).
    let err = log
        .entity("Kenny", &[], "person", "m4-reasoner", &[])
        .expect_err("non-manual entity with empty sources must be rejected");
    assert!(matches!(err, bossclaw_core::BossclawError::InvalidInput(_)));
}

#[test]
fn rebuild_graph_folds_entities_and_marks_entity_node_kind() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let m = log.append(mk_memory("kenny")).unwrap();
    let kenny = log
        .entity("Kenny", &["Ken".to_string()], "person", "m4-reasoner", &[m.clone()])
        .unwrap();
    // A link from the memory to the entity node.
    log.link(&m, "mentions", &kenny, None, &[m.clone()]).unwrap();
    log.rebuild_graph().unwrap();

    // entities projection populated from the entity event.
    let entities = log.all_entities().unwrap();
    assert_eq!(entities.len(), 1);
    assert_eq!(entities[0].entity_id, kenny);
    assert_eq!(entities[0].label, "Kenny");
    assert_eq!(entities[0].aliases, vec!["Ken".to_string()]);
    assert_eq!(entities[0].entity_type, "person");

    // The entity endpoint is marked kind="entity" (NOT "external"), the memory
    // endpoint stays "memory".
    let nodes = log.all_nodes().unwrap();
    let kind_of = |id: &str| nodes.iter().find(|n| n.node_id == id).map(|n| n.kind.clone());
    assert_eq!(kind_of(&kenny).as_deref(), Some("entity"));
    assert_eq!(kind_of(&m).as_deref(), Some("memory"));
}

#[test]
fn rebuild_graph_is_byte_identical_with_entities() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let m = log.append(mk_memory("kenny at acme")).unwrap();
    let kenny = log.entity("Kenny", &[], "person", "m4-reasoner", &[m.clone()]).unwrap();
    let acme = log.entity("Acme", &[], "org", "m4-reasoner", &[m.clone()]).unwrap();
    log.link(&kenny, "works_at", &acme, None, &[m.clone()]).unwrap();

    log.rebuild_graph().unwrap();
    let (e1, n1, ent1) = (log.all_edges().unwrap(), log.all_nodes().unwrap(), log.all_entities().unwrap());
    log.rebuild_graph().unwrap();
    let (e2, n2, ent2) = (log.all_edges().unwrap(), log.all_nodes().unwrap(), log.all_entities().unwrap());

    assert_eq!(e1, e2, "edges fold byte-identical with entities present");
    assert_eq!(n1, n2, "nodes fold byte-identical with entities present");
    assert_eq!(ent1, ent2, "entities fold byte-identical across rebuilds");
    assert_eq!(ent1.len(), 2, "two entity events → two entities rows");
}
```

- [ ] **Step 2 — run, verify fail**

Run: `cargo test -p bossclaw-core --test graph -- entity_appends entity_rejects rebuild_graph_folds_entities rebuild_graph_is_byte_identical_with_entities`
Expected: FAIL — `no method named entity` / `no method named all_entities` / `no type Entity` (compile error).

- [ ] **Step 3 — add the `Entity` type + entity parser + fold** to `src/graph.rs`. Append after `fold_edges` (and before the `#[cfg(test)]` module):

```rust
/// A folded entity record: one `entity` Tier-B event projected into the
/// `entities` table (spec §4). The id is `entity:<the entity event's ULID>` —
/// stable, mint-once; the `label` is a property, never the id (names collide and
/// change, the id does not).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entity {
    /// The stable namespaced node id, `entity:<ulid>`.
    pub entity_id: String,
    /// Human-readable label (display name).
    pub label: String,
    /// Known aliases (other surface forms that resolve to this entity).
    pub aliases: Vec<String>,
    /// Coarse type discriminator (e.g. `"person"`, `"org"`).
    pub entity_type: String,
}

/// Extract `(label, aliases, entity_type)` from an `entity` event's content, or
/// `None` if `label`/`entity_type` are missing or non-string (malformed —
/// skipped by the fold rather than failing it). `aliases` defaults to empty when
/// absent or non-array; non-string alias items are dropped.
pub fn parse_entity_content(content: &serde_json::Value) -> Option<(String, Vec<String>, String)> {
    let label = content.get("label")?.as_str()?.to_string();
    let entity_type = content.get("entity_type")?.as_str()?.to_string();
    let aliases = content
        .get("aliases")
        .and_then(|a| a.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    Some((label, aliases, entity_type))
}

/// Fold `entity` events (which MUST already be in `seq` order) into the entity
/// set. Deterministic: one [`Entity`] per well-formed `entity` event, in event
/// order, id = `entity:<event id>`. Malformed events (no label/entity_type) are
/// skipped. Mint-once: an entity event id is unique, so there is no merge here —
/// resolution (spec §6) decides reuse-vs-mint BEFORE an event is appended.
pub fn fold_entities(events: &[Event]) -> Vec<Entity> {
    let mut out = Vec::new();
    for ev in events {
        if ev.event_type != "entity" {
            continue;
        }
        if let Some((label, aliases, entity_type)) = parse_entity_content(&ev.content) {
            out.push(Entity { entity_id: format!("entity:{}", ev.id), label, aliases, entity_type });
        }
    }
    out
}
```

- [ ] **Step 4 — add the `entities` DDL** in `EventLog::open` (`src/log.rs`), right after the `nodes` `CREATE TABLE` block and before the `PRAGMA temp_store` line:

```rust
        // Entity projection (Tier-A; spec §4). One row per `entity` event,
        // id = "entity:<event ulid>". A deterministic fold over entity events,
        // rebuilt by `rebuild_graph`. The label is a property, never the id.
        store.exec(
            "CREATE TABLE IF NOT EXISTS entities (
                entity_id   TEXT PRIMARY KEY,
                label       TEXT NOT NULL,
                aliases     TEXT NOT NULL,
                entity_type TEXT NOT NULL
            )",
        )?;
```

- [ ] **Step 5 — add the `EventLog::entity(...)` append helper + `all_entities` read** to `impl EventLog` (`src/log.rs`). Place `entity` right after `invalidate`. It is a NON-manual producer, so it routes through `append_graph_event`'s sibling logic — but entity content differs from link content, so it has its own builder that REUSES the F2 producer gate. Add it directly:

```rust
    /// Append a signed Tier-B `entity` event minting a stable `entity:<ulid>`
    /// node carrying `{label, aliases, entity_type}` (spec §4). Returns the
    /// namespaced node id `entity:<event id>` (NOT the bare event id) — the form
    /// links reference.
    ///
    /// `entity` is a NON-MANUAL producer: `source_event_ids` MUST be non-empty
    /// (the memory/-ies that introduced the entity). An empty source set is
    /// rejected (the M3 F2 taint guard, parent §5.11) — defaulting here would
    /// erase the inducing memory from the lineage the actuator walks fail-closed.
    ///
    /// The `entities` table is NOT updated here — call [`EventLog::rebuild_graph`]
    /// to refresh it (same append→rebuild lifecycle as [`EventLog::link`]).
    pub fn entity(
        &self,
        label: &str,
        aliases: &[String],
        entity_type: &str,
        producer: &str,
        source_event_ids: &[String],
    ) -> Result<String, BossclawError> {
        if source_event_ids.is_empty() {
            // entity is never the manual producer; an empty source set is always
            // a taint-laundering reject (mirrors `append_graph_event`'s F2 arm).
            return Err(BossclawError::InvalidInput(
                "entity event requires explicit non-empty source_event_ids (the inducing \
                 memory) — an empty default would erase it from the §5.11 lineage walk".into(),
            ));
        }
        let event_id = self.append(Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: "entity".to_string(),
            content: serde_json::json!({
                "label": label,
                "aliases": aliases,
                "entity_type": entity_type,
            }),
            model_meta: Some(ModelMeta {
                model_id: producer.to_string(),
                prompt_hash: String::new(),
                source_event_ids: source_event_ids.to_vec(),
            }),
            prev_hash: String::new(),
            hash: None,
            signed_by_did: self.signer_did(),
            signature: None,
        })?;
        Ok(format!("entity:{event_id}"))
    }

    /// Every entity, `ORDER BY entity_id ASC` (deterministic). Tier-A read.
    pub fn all_entities(&self) -> Result<Vec<crate::graph::Entity>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn
            .prepare("SELECT entity_id, label, aliases, entity_type FROM entities ORDER BY entity_id ASC")?;
        let rows = stmt.query_map([], |r| {
            let aliases_json: String = r.get(2)?;
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, aliases_json, r.get::<_, String>(3)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (entity_id, label, aliases_json, entity_type) = row?;
            // aliases is stored as a JSON array string; a malformed value degrades
            // to empty rather than failing the read (best-effort, matches the fold).
            let aliases: Vec<String> = serde_json::from_str(&aliases_json).unwrap_or_default();
            out.push(crate::graph::Entity { entity_id, label, aliases, entity_type });
        }
        Ok(out)
    }
```

- [ ] **Step 6 — extend `rebuild_graph`** (`src/log.rs`) to also fold entities and mark entity-node kinds. Three edits inside the existing `rebuild_graph`:

(a) After `let memory_ids = self.memory_page_ids()?;` add the entity fold inputs:

```rust
        let entity_events = self.entity_events_ordered()?;
        let entities = crate::graph::fold_entities(&entity_events);
        // Set of entity node ids → used to mark node kind "entity" (overrides the
        // "external" default for ids the edges reference).
        let entity_ids: HashSet<String> =
            entities.iter().map(|e| e.entity_id.clone()).collect();
```

(b) In the node-kind loop, the `kind` decision gains an entity branch (replace the existing `or_insert_with` closure body):

```rust
                node_kinds.entry(endpoint.clone()).or_insert_with(|| {
                    if entity_ids.contains(endpoint) {
                        "entity".to_string()
                    } else if memory_ids.contains(endpoint) {
                        "memory".to_string()
                    } else {
                        "external".to_string()
                    }
                });
```

(c) Inside the transaction (after the `nodes` insert loop, before `tx.commit()`), wipe + refill `entities`:

```rust
        tx.execute("DELETE FROM entities", [])?;
        for e in &entities {
            tx.execute(
                "INSERT INTO entities (entity_id, label, aliases, entity_type)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    e.entity_id,
                    e.label,
                    serde_json::to_string(&e.aliases)?, // JSON array string (deterministic)
                    e.entity_type
                ],
            )?;
        }
```

(d) Add the private collector (next to `graph_events_ordered`):

```rust
    /// All `entity` events, payload-parsed, in chain (`seq ASC`) order.
    fn entity_events_ordered(&self) -> Result<Vec<Event>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn
            .prepare("SELECT payload FROM events WHERE event_type = 'entity' ORDER BY seq ASC")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(serde_json::from_str(&row?)?);
        }
        Ok(out)
    }
```

> NOTE on determinism: `serde_json::to_string(&Vec<String>)` is deterministic for a `Vec` (array order preserved), so the stored `aliases` string is byte-stable across rebuilds — the byte-identical-rebuild gate holds.

- [ ] **Step 7 — re-export `Entity`** in `src/lib.rs`: change `pub use graph::{AsOf, Edge, Node};` to `pub use graph::{AsOf, Edge, Entity, Node};`.

- [ ] **Step 8 — run, verify pass**

Run: `cargo test -p bossclaw-core --test graph`
Expected: PASS (all existing M3 graph tests + the four new entity tests).

- [ ] **Step 9 — commit**

```bash
git add crates/bossclaw-core/src/graph.rs crates/bossclaw-core/src/log.rs crates/bossclaw-core/src/lib.rs crates/bossclaw-core/tests/graph.rs
git status -s
git commit -m "feat(bossclaw-core): entity Tier-B event + entities projection + entity-node fold (M4a T2)"
```

---

## Task 3: Entity resolution (embed → threshold → scripted adjudication)

**Files:**
- Modify: `crates/bossclaw-core/src/extract.rs` (**new this task** — resolution consts + the pure `resolve_decision`), `crates/bossclaw-core/src/log.rs` (entity vector derivation + `entity_search` kind-filtered search + the `EventLog` resolution glue `resolve_mention`), `crates/bossclaw-core/src/lib.rs` (`pub mod extract;` already added in T1 Step 6 — verify present)
- Test: `crates/bossclaw-core/tests/entity_resolution.rs` (new), `crates/bossclaw-core/tests/extract.rs` (new, the pure-decision unit tests)

- [ ] **Step 1 — write the failing pure-decision test** (`tests/extract.rs`, new file):

```rust
//! Pure unit tests for `extract.rs`: the resolution decision, the relation
//! vocabulary/cardinality tables, and (later tasks) the reflexion parse/critique.
//! No DB, no model — only the pure functions, driven by fixed inputs.

use bossclaw_core::extract::{resolve_decision, ResolveDecision, RESOLVE_HIGH, RESOLVE_LOW};

#[test]
fn resolve_decision_auto_merges_above_high() {
    // Best candidate's cosine similarity ≥ RESOLVE_HIGH → auto-merge to it.
    let d = resolve_decision(&[("entity:ken".to_string(), RESOLVE_HIGH + 0.01)]);
    assert_eq!(d, ResolveDecision::Merge("entity:ken".to_string()));
}

#[test]
fn resolve_decision_mints_below_low() {
    // Best candidate ≤ RESOLVE_LOW (or no candidates) → mint a fresh entity.
    let d = resolve_decision(&[("entity:ken".to_string(), RESOLVE_LOW - 0.01)]);
    assert_eq!(d, ResolveDecision::Mint);
    assert_eq!(resolve_decision(&[]), ResolveDecision::Mint, "no candidates → mint");
}

#[test]
fn resolve_decision_routes_midband_to_adjudication_with_candidate_list() {
    // Strictly between LOW and HIGH → adjudicate among the candidates (sorted
    // best-first), so the model picks "same as one of these, or none".
    let cands = vec![
        ("entity:ken".to_string(), 0.80),
        ("entity:kenji".to_string(), 0.78),
    ];
    match resolve_decision(&cands) {
        ResolveDecision::Adjudicate(ids) => {
            assert_eq!(ids, vec!["entity:ken".to_string(), "entity:kenji".to_string()]);
        }
        other => panic!("mid-band must adjudicate, got {other:?}"),
    }
}
```

- [ ] **Step 2 — write the failing integration test** (`tests/entity_resolution.rs`, new file). Drives the full `EventLog` resolution path with `MockEmbedder` + `ScriptedReasoner`:

```rust
//! Entity resolution against the live `entities` projection: embed the mention,
//! search existing entity vectors (kind-filtered), apply RESOLVE_HIGH/LOW, route
//! the mid-band to the scripted adjudicator. Hermetic: MockEmbedder + ScriptedReasoner.

use bossclaw_core::embed::MockEmbedder;
use bossclaw_core::event::Event;
use bossclaw_core::extract::ResolveDecision;
use bossclaw_core::log::EventLog;
use bossclaw_core::reason::ScriptedReasoner;
use ed25519_dalek::SigningKey;
use serde_json::json;

const DEK: [u8; 32] = [42u8; 32];
const KEY_BYTES: [u8; 32] = [7u8; 32];
const MID_DIM: usize = 64;

fn open_log(dir: &std::path::Path) -> EventLog {
    let key = SigningKey::from_bytes(&KEY_BYTES);
    EventLog::open(&dir.join("m.db"), &DEK, key).unwrap()
}
fn mk_memory(text: &str) -> Event {
    Event {
        id: String::new(), ts: String::new(), valid_time: None,
        event_type: "memory".to_string(), content: json!({ "text": text }),
        model_meta: None, prev_hash: String::new(), hash: None,
        signed_by_did: "did:wba:AIR-TEST".to_string(), signature: None,
    }
}

#[test]
fn resolving_an_identical_mention_reuses_the_existing_entity() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let embedder = MockEmbedder::new(MID_DIM);
    let m = log.append(mk_memory("kenny ferris rustacean")).unwrap();
    // Mint Kenny, derive its entity vector, rebuild so it is searchable.
    let kenny = log.entity("kenny ferris rustacean", &[], "person", "m4-reasoner", &[m.clone()]).unwrap();
    log.derive_entity_vector(&embedder, &kenny, "kenny ferris rustacean").unwrap();
    log.rebuild_entity_index(&embedder).unwrap();

    // The SAME surface text re-embeds to an identical vector (cosine 1.0 ≥ HIGH)
    // → resolve must MERGE to the existing node, not mint a second.
    let reasoner = ScriptedReasoner::new("m4-reasoner"); // adjudicator unused at cosine 1.0
    let decision = log
        .resolve_mention(&embedder, &reasoner, "kenny ferris rustacean")
        .unwrap();
    assert_eq!(decision, ResolveDecision::Merge(kenny));
}

#[test]
fn resolving_a_disjoint_mention_mints_a_new_entity() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let embedder = MockEmbedder::new(MID_DIM);
    let m = log.append(mk_memory("kenny")).unwrap();
    let kenny = log.entity("kenny ferris rustacean", &[], "person", "m4-reasoner", &[m.clone()]).unwrap();
    log.derive_entity_vector(&embedder, &kenny, "kenny ferris rustacean").unwrap();
    log.rebuild_entity_index(&embedder).unwrap();

    // A totally disjoint mention shares no tokens (cosine 0.0 ≤ LOW) → mint.
    let reasoner = ScriptedReasoner::new("m4-reasoner");
    let decision = log
        .resolve_mention(&embedder, &reasoner, "completely unrelated quantum lecture")
        .unwrap();
    assert_eq!(decision, ResolveDecision::Mint);
}

#[test]
fn entity_vectors_are_not_returned_by_recall() {
    // The locked constraint: entity events are embedded for resolution but recall
    // must EXCLUDE entity-kind. (Full recall-exclusion is wired in T8; here we
    // assert the dedicated entity index is separate from the recall index.)
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let embedder = MockEmbedder::new(MID_DIM);
    let m = log.append(mk_memory("kenny")).unwrap();
    let kenny = log.entity("kenny ferris", &[], "person", "m4-reasoner", &[m]).unwrap();
    log.derive_entity_vector(&embedder, &kenny, "kenny ferris").unwrap();
    log.rebuild_entity_index(&embedder).unwrap();
    // entity_search finds the entity…
    let hits = log.entity_search(&embedder, "kenny ferris", 5).unwrap();
    assert!(hits.iter().any(|(id, _)| id == &kenny), "entity index finds the entity node");
}
```

- [ ] **Step 3 — run, verify fail**

Run: `cargo test -p bossclaw-core --test extract` then `cargo test -p bossclaw-core --test entity_resolution`
Expected: FAIL — `unresolved import bossclaw_core::extract` / `no method named resolve_mention` / `derive_entity_vector` / `rebuild_entity_index` / `entity_search`.

- [ ] **Step 4 — create `src/extract.rs`** with the resolution consts + the pure decision (the prompt/parse functions land in T4/T5; this task seeds the module):

```rust
//! Pure extraction + resolution logic (spec §6): the retrieval-augmented prompt
//! construction, response parsing, the reflexion state machine, and the
//! entity-resolution decision. PURE — no SQL, no I/O, no `Store`. Takes recall +
//! graph results as inputs and calls the [`crate::reason::Reasoner`] trait, so it
//! is unit-testable with [`crate::reason::ScriptedReasoner`]. Mirrors the pure
//! split in [`crate::recall`] / [`crate::graph`].

/// Cosine auto-merge floor (spec §11). At or above this similarity a mention is
/// merged into the best candidate entity WITHOUT asking the model. High on
/// purpose — a wrong entity merge is expensive to undo. Tunable in dogfooding.
pub const RESOLVE_HIGH: f32 = 0.92;

/// Mint-new ceiling (spec §11). At or below this similarity a mention mints a
/// fresh entity. Between [`RESOLVE_LOW`] and [`RESOLVE_HIGH`] the model
/// adjudicates ("same as one of these, or none"). Tunable in dogfooding.
pub const RESOLVE_LOW: f32 = 0.75;

/// Total reflexion passes per memory (spec §11): propose (Pass A) + one critique
/// (Pass B). Bounds the model calls per memory so a tick's latency is bounded.
pub const MAX_REFLECT: u32 = 2;

/// Minimum machine-edge confidence to contribute the recall boost / be actuator-
/// eligible (spec §7, §11). Below this a machine edge is still recorded
/// (never-forget) and queryable, but does not tilt recall. Tunable.
pub const TRUST_MIN: f32 = 0.6;

/// How many recalled neighbors are fed as the Pass-A cheat sheet (spec §11). The
/// retrieval-augmented context that lets a 7b reconcile against KNOWN facts
/// rather than recall everything. Tunable.
pub const GRAPH_CONTEXT_K: usize = 8;

/// The outcome of resolving one entity mention against the existing entity nodes
/// (spec §6). The embedding-first, model-adjudicated mid-band decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveDecision {
    /// Cosine ≥ [`RESOLVE_HIGH`]: auto-merge into this existing entity id.
    Merge(String),
    /// Cosine ≤ [`RESOLVE_LOW`] (or no candidates): mint a fresh `entity` event.
    Mint,
    /// Mid-band: ask the reasoner to pick among these candidate ids (best-first)
    /// or answer "none" (→ mint). The caller runs the adjudication step.
    Adjudicate(Vec<String>),
}

/// Decide how to resolve a mention given its `candidates` as `(entity_id,
/// cosine_similarity)` pairs (NOT necessarily sorted). Pure: compares the BEST
/// candidate's similarity to the thresholds. `Merge` above HIGH, `Mint` at/below
/// LOW or when empty, else `Adjudicate` with all candidates sorted best-first.
///
/// Cosine similarity here is `1 - distance` (the vector index returns distance);
/// the caller converts before calling so this function speaks one scale.
pub fn resolve_decision(candidates: &[(String, f32)]) -> ResolveDecision {
    let mut sorted: Vec<&(String, f32)> = candidates.iter().collect();
    // Best (highest similarity) first; id as a deterministic tie-break.
    sorted.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let best = match sorted.first() {
        Some(c) => c.1,
        None => return ResolveDecision::Mint,
    };
    if best >= RESOLVE_HIGH {
        ResolveDecision::Merge(sorted[0].0.clone())
    } else if best <= RESOLVE_LOW {
        ResolveDecision::Mint
    } else {
        ResolveDecision::Adjudicate(sorted.into_iter().map(|(id, _)| id.clone()).collect())
    }
}
```

- [ ] **Step 5 — add the entity vector path + resolution glue** to `impl EventLog` (`src/log.rs`). The entity index is a SECOND `HnswIndex` held alongside `vector_index`, so the resolution search is physically separate from recall (locked constraint: recall excludes entity-kind, resolution searches only entity-kind). First, add the field to the struct (after `vector_index`):

```rust
    /// In-memory ANN index over `entity`-event vectors ONLY, for entity
    /// resolution (spec §6). Physically separate from `vector_index` so recall
    /// can never surface an entity node and resolution can never match a memory.
    /// `None` until [`EventLog::rebuild_entity_index`]; rebuilt from the encrypted
    /// log on open (zero plaintext index on disk, like the recall index).
    entity_index: Mutex<Option<Box<dyn VectorIndex>>>,
```

…and initialize it in `open` (in the `Ok(Self { … })` literal): `entity_index: Mutex::new(None),`.

Then the methods:

```rust
    /// Derive + store the resolution vector for an `entity` node under
    /// `(entity_id, model_id)` in a dedicated `entity_vectors` table. Separate
    /// from `vectors` (which feeds recall) so the two indexes never bleed. The
    /// `text` is the entity's label (+ optionally aliases) — what future mentions
    /// are matched against. Idempotent upsert.
    pub fn derive_entity_vector(
        &self,
        embedder: &dyn Embedder,
        entity_id: &str,
        text: &str,
    ) -> Result<(), BossclawError> {
        let embedding = embed_one(embedder, text)?;
        let blob = vec_to_blob(&embedding);
        let store = self.inner.lock().expect(POISON);
        store.conn().execute(
            "INSERT OR REPLACE INTO entity_vectors (entity_id, model_id, dim, embedding)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![entity_id, embedder.model_id(), embedder.dim() as i64, blob],
        )?;
        Ok(())
    }

    /// Rebuild the in-memory entity-resolution index from `entity_vectors` for
    /// the active model (zero plaintext index on disk; rebuilt on open — same
    /// mechanism as [`EventLog::rebuild_indexes`]). Serial insertion over
    /// `entity_id ASC` for reproducibility.
    pub fn rebuild_entity_index(&self, embedder: &dyn Embedder) -> Result<(), BossclawError> {
        let rows = self.entity_vectors_for_model(embedder.model_id())?;
        let mut index = HnswIndex::with_capacity(rows.len());
        for (entity_id, vec) in rows {
            index.add(&entity_id, &vec);
        }
        let boxed: Box<dyn VectorIndex> = Box::new(index);
        *self.entity_index.lock().expect(POISON) = Some(boxed);
        Ok(())
    }

    /// All entity vectors for `model_id` as `(entity_id, vector)` pairs, ordered
    /// `entity_id ASC` (deterministic rebuild order). Mirrors
    /// [`EventLog::vectors_for_model`] but over the `entity_vectors` table.
    fn entity_vectors_for_model(
        &self,
        model_id: &str,
    ) -> Result<Vec<(String, Vec<f32>)>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT entity_id, embedding FROM entity_vectors WHERE model_id = ?1 \
             ORDER BY entity_id ASC",
        )?;
        let rows = stmt.query_map([model_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, blob) = row?;
            out.push((id, blob_to_vec(&blob)?));
        }
        Ok(out)
    }

    /// Search the entity-resolution index for the `k` nearest `(entity_id,
    /// distance)` pairs to `mention`'s embedding. ONLY entity nodes are searched
    /// (the index holds only `entity_vectors`). Returns [`BossclawError::InvalidInput`]
    /// if the entity index was never built.
    pub fn entity_search(
        &self,
        embedder: &dyn Embedder,
        mention: &str,
        k: usize,
    ) -> Result<Vec<(String, f32)>, BossclawError> {
        let query = embed_one(embedder, mention)?;
        let guard = self.entity_index.lock().expect(POISON);
        match guard.as_ref() {
            Some(index) => Ok(index.search(&query, k)),
            None => Err(BossclawError::InvalidInput(
                "entity index not built — call rebuild_entity_index".into(),
            )),
        }
    }

    /// Resolve one entity `mention` against the existing entity nodes (spec §6):
    /// embed → search the entity index → convert distance to cosine similarity →
    /// [`crate::extract::resolve_decision`]; for the mid-band, ask `reasoner` to
    /// adjudicate and collapse its answer to a final [`ResolveDecision::Merge`]
    /// (a chosen candidate) or [`ResolveDecision::Mint`] (`"none"` / unknown id).
    ///
    /// The adjudication call is the ONLY model use here; merge/mint short-circuit
    /// without a model call (cheap + deterministic at the thresholds).
    pub fn resolve_mention(
        &self,
        embedder: &dyn Embedder,
        reasoner: &dyn crate::reason::Reasoner,
        mention: &str,
    ) -> Result<crate::extract::ResolveDecision, BossclawError> {
        use crate::extract::ResolveDecision;
        // DistCosine returns distance in [0, 2]; similarity = 1 - distance.
        let candidates: Vec<(String, f32)> = self
            .entity_search(embedder, mention, crate::extract::GRAPH_CONTEXT_K)?
            .into_iter()
            .map(|(id, dist)| (id, 1.0 - dist))
            .collect();
        match crate::extract::resolve_decision(&candidates) {
            ResolveDecision::Adjudicate(ids) => {
                let decided = self.adjudicate_entity(reasoner, mention, &ids)?;
                match decided {
                    Some(id) => Ok(ResolveDecision::Merge(id)),
                    None => Ok(ResolveDecision::Mint),
                }
            }
            other => Ok(other),
        }
    }

    /// Ask `reasoner` which of `candidate_ids` (if any) the `mention` refers to.
    /// Returns `Some(id)` for a chosen candidate that is actually in the list,
    /// `None` for `"none"` OR any id the model invented (defensive: a hallucinated
    /// id must not become a merge target). Uses the adjudication schema.
    fn adjudicate_entity(
        &self,
        reasoner: &dyn crate::reason::Reasoner,
        mention: &str,
        candidate_ids: &[String],
    ) -> Result<Option<String>, BossclawError> {
        let system = "You resolve entity coreference. Answer ONLY with the JSON the schema \
                      describes: the id of the candidate the mention refers to, or \"none\".";
        let prompt = crate::extract::build_adjudication_prompt(mention, candidate_ids);
        let answer = reasoner.complete_json(system, &prompt, &crate::reason::adjudication_schema())?;
        let chosen = answer.get("match").and_then(|m| m.as_str()).unwrap_or("none");
        if chosen == "none" {
            return Ok(None);
        }
        // Fail-closed: only accept an id the model was actually offered.
        Ok(candidate_ids.iter().find(|id| id.as_str() == chosen).cloned())
    }
```

- [ ] **Step 6 — add `build_adjudication_prompt` to `src/extract.rs`** (pure prompt builder, used by `adjudicate_entity`):

```rust
/// Build the entity-resolution adjudication prompt (spec §6): the mention plus a
/// short candidate list (best-first). The model answers with one candidate id or
/// `"none"`. Pure string construction so it is unit-testable.
pub fn build_adjudication_prompt(mention: &str, candidate_ids: &[String]) -> String {
    let mut s = String::new();
    s.push_str("Mention to resolve:\n");
    s.push_str(mention);
    s.push_str("\n\nCandidate entities (choose the one the mention refers to, or \"none\"):\n");
    for id in candidate_ids {
        s.push_str("- ");
        s.push_str(id);
        s.push('\n');
    }
    s
}
```

- [ ] **Step 7 — add the `entity_vectors` DDL** in `EventLog::open` (`src/log.rs`), right after the `entities` `CREATE TABLE` (added in T2 Step 4):

```rust
        // Entity-resolution vectors (Tier-A derived; spec §6). Separate from
        // `vectors` so the resolution index NEVER mixes with the recall index —
        // recall must exclude entity-kind, resolution searches only entity-kind.
        store.exec(
            "CREATE TABLE IF NOT EXISTS entity_vectors (
                entity_id TEXT NOT NULL,
                model_id  TEXT NOT NULL,
                dim       INTEGER NOT NULL,
                embedding BLOB NOT NULL,
                PRIMARY KEY(entity_id, model_id)
            )",
        )?;
```

- [ ] **Step 8 — run, verify pass**

Run: `cargo test -p bossclaw-core --test extract` then `cargo test -p bossclaw-core --test entity_resolution`
Expected: PASS (3 pure decision tests + 3 resolution integration tests).

- [ ] **Step 9 — commit**

```bash
git add crates/bossclaw-core/src/extract.rs crates/bossclaw-core/src/log.rs crates/bossclaw-core/tests/extract.rs crates/bossclaw-core/tests/entity_resolution.rs
git status -s
git commit -m "feat(bossclaw-core): embedding entity resolution (thresholds + scripted adjudication) (M4a T3)"
```

---

## Task 4: `extract.rs` Pass A (propose)

**Files:**
- Modify: `crates/bossclaw-core/src/extract.rs` (the relation vocabulary + cardinality tables, `Proposals`/`ProposedEntity`/`ProposedRelation`/`ProposedRetraction` types, `build_pass_a_prompt`, `parse_proposals`, `propose`)
- Test: `crates/bossclaw-core/tests/extract.rs`

- [ ] **Step 1 — write the failing tests** (`tests/extract.rs`, append):

```rust
use bossclaw_core::extract::{
    build_pass_a_prompt, parse_proposals, propose, Proposals, RELATION_VOCAB,
};
use bossclaw_core::reason::{extraction_schema, ScriptedReasoner};
use serde_json::json;

#[test]
fn pass_a_prompt_carries_source_recalled_and_vocabulary() {
    let prompt = build_pass_a_prompt(
        "Kenny started at Acme last week.",
        &["Kenny used to be at Globex.".to_string()],
    );
    assert!(prompt.contains("Kenny started at Acme"), "source memory present");
    assert!(prompt.contains("Globex"), "recalled neighbor present");
    // The seed relation vocabulary is handed to the model so it reuses labels.
    assert!(prompt.contains("works_at"), "relation vocabulary present");
    assert!(RELATION_VOCAB.contains(&"works_at"), "vocab seed includes works_at");
}

#[test]
fn parse_proposals_reads_entities_relations_retractions_with_fields() {
    let raw = json!({
        "entities": [{ "mention": "Kenny", "entity_type": "person", "confidence": 0.9 }],
        "relations": [{
            "src": "Kenny", "relation": "works_at", "dst": "Acme",
            "confidence": 0.8, "supported_by": "Kenny started at Acme last week."
        }],
        "retractions": [{
            "src": "Kenny", "relation": "works_at", "dst": "Globex",
            "reason": "moved to Acme", "confidence": 0.7
        }]
    });
    let p: Proposals = parse_proposals(&raw).unwrap();
    assert_eq!(p.entities.len(), 1);
    assert_eq!(p.entities[0].mention, "Kenny");
    assert_eq!(p.relations.len(), 1);
    assert_eq!(p.relations[0].relation, "works_at");
    assert_eq!(p.relations[0].supported_by, "Kenny started at Acme last week.");
    assert!((p.relations[0].confidence - 0.8).abs() < 1e-6);
    assert_eq!(p.retractions.len(), 1);
    assert_eq!(p.retractions[0].dst, "Globex");
}

#[test]
fn parse_proposals_rejects_a_relation_missing_supported_by() {
    // supported_by is mandatory — a relation without a source span is unverifiable
    // and must be dropped (Pass B would drop it anyway; the parser is the first gate).
    let raw = json!({
        "entities": [],
        "relations": [{ "src": "A", "relation": "knows", "dst": "B", "confidence": 0.9 }],
        "retractions": []
    });
    let p = parse_proposals(&raw).unwrap();
    assert!(p.relations.is_empty(), "a relation with no supported_by span is dropped");
}

#[test]
fn propose_runs_pass_a_through_the_reasoner() {
    let source = "Kenny started at Acme last week.";
    let recalled = vec!["Kenny used to be at Globex.".to_string()];
    let system = "extract"; // the propose() system string is fixed; see impl
    let prompt = build_pass_a_prompt(source, &recalled);
    let canned = json!({
        "entities": [{ "mention": "Kenny", "entity_type": "person", "confidence": 0.95 }],
        "relations": [{
            "src": "Kenny", "relation": "works_at", "dst": "Acme",
            "confidence": 0.9, "supported_by": "Kenny started at Acme last week."
        }],
        "retractions": []
    });
    // Key the scripted reasoner on propose()'s actual (system, prompt).
    let reasoner = ScriptedReasoner::new("m4-reasoner")
        .with_response(bossclaw_core::extract::PASS_A_SYSTEM, &prompt, canned);
    let _ = system; // documents that propose() owns the system string
    let p = propose(&reasoner, source, &recalled).unwrap();
    assert_eq!(p.entities[0].mention, "Kenny");
    assert_eq!(p.relations[0].dst, "Acme");
    // Sanity: the schema builder is the one propose() passes.
    assert_eq!(extraction_schema()["type"], json!("object"));
}
```

- [ ] **Step 2 — run, verify fail**

Run: `cargo test -p bossclaw-core --test extract -- pass_a parse_proposals propose`
Expected: FAIL — `no function build_pass_a_prompt` / `parse_proposals` / `propose` / `no const RELATION_VOCAB`.

- [ ] **Step 3 — add the vocabulary, types, prompt, parser, and `propose`** to `src/extract.rs`:

```rust
use crate::error::BossclawError;
use crate::reason::{extraction_schema, Reasoner};

/// Seed relation vocabulary (spec §6): a small, EXTENSIBLE set of relation labels
/// handed to the model so the graph does not sprout five synonyms for one
/// relation. An unknown relation the model proposes is allowed but flagged
/// (lower trust) — the vocabulary grows by curation, not silently. This is the
/// memory-graph relation vocabulary, NOT the AIR agent-capability ontology.
pub const RELATION_VOCAB: &[&str] = &[
    "works_at",
    "knows",
    "located_in",
    "part_of",
    "caused_by",
    "owns",
    "member_of",
    "works_at_primary",
];

/// Relations that are single-valued FOR A SUBJECT (spec §6): a new such fact
/// about the same `src` implies the prior is retired. Used by Pass B's
/// contradiction confirmation together with model judgment (neither alone).
pub const RELATION_CARDINALITY_SINGLE: &[&str] = &["works_at_primary", "located_in"];

/// One proposed entity mention from extraction.
#[derive(Debug, Clone, PartialEq)]
pub struct ProposedEntity {
    /// The surface mention as it appeared (e.g. `"Kenny"`).
    pub mention: String,
    /// Coarse type the model assigned (e.g. `"person"`).
    pub entity_type: String,
    /// Model confidence in `[0, 1]`.
    pub confidence: f32,
}

/// One proposed relation, carrying its mandatory `supported_by` source span.
#[derive(Debug, Clone, PartialEq)]
pub struct ProposedRelation {
    /// Source mention/id.
    pub src: String,
    /// Relation label (ideally from [`RELATION_VOCAB`]).
    pub relation: String,
    /// Destination mention/id.
    pub dst: String,
    /// Model confidence in `[0, 1]`.
    pub confidence: f32,
    /// Verbatim span from the source memory that supports this relation. MANDATORY
    /// — a relation with no supporting span is dropped at parse (unverifiable).
    pub supported_by: String,
}

/// One proposed retraction (a fact the new memory contradicts).
#[derive(Debug, Clone, PartialEq)]
pub struct ProposedRetraction {
    /// Source of the contradicted edge.
    pub src: String,
    /// Relation of the contradicted edge.
    pub relation: String,
    /// Destination of the contradicted edge.
    pub dst: String,
    /// Why the model believes it is contradicted.
    pub reason: String,
    /// Model confidence in `[0, 1]`.
    pub confidence: f32,
}

/// The parsed Pass-A / Pass-B proposal set (spec §3 step 2/5).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Proposals {
    /// Proposed entity mentions.
    pub entities: Vec<ProposedEntity>,
    /// Proposed relations (each with a `supported_by` span).
    pub relations: Vec<ProposedRelation>,
    /// Proposed retractions of contradicted facts.
    pub retractions: Vec<ProposedRetraction>,
}

/// The fixed system instruction for Pass A (propose). Public so the hermetic
/// tests can key the [`crate::reason::ScriptedReasoner`] on the exact pair
/// `propose` uses.
pub const PASS_A_SYSTEM: &str =
    "You are a careful knowledge extractor. Read the SOURCE memory, reconcile it \
     against the KNOWN facts, and emit ONLY the JSON the schema describes. Reuse a \
     relation label from the vocabulary when one fits. Every relation MUST include a \
     verbatim supported_by span copied from the SOURCE. Do not invent facts.";

/// Build the Pass-A prompt (spec §6): the source memory, the recalled neighbors
/// (the cheat sheet), and the seed relation vocabulary. The text is presented as
/// DATA to be reconciled (the untrusted-content fence, parent §8.4) — the model's
/// job is extraction; its output is parsed as proposals, never executed.
pub fn build_pass_a_prompt(source: &str, recalled: &[String]) -> String {
    let mut s = String::new();
    s.push_str("SOURCE memory (extract facts ONLY from this):\n");
    s.push_str(source);
    s.push_str("\n\nKNOWN facts (recalled context — reconcile against these, do not re-extract them):\n");
    if recalled.is_empty() {
        s.push_str("(none)\n");
    } else {
        for r in recalled {
            s.push_str("- ");
            s.push_str(r);
            s.push('\n');
        }
    }
    s.push_str("\nRelation vocabulary (prefer these labels): ");
    s.push_str(&RELATION_VOCAB.join(", "));
    s.push('\n');
    s
}

/// Parse a reasoner JSON value into [`Proposals`] (spec §6). Tolerant of missing
/// arrays (treated as empty). A relation with no non-empty `supported_by` span is
/// DROPPED (unverifiable). Numeric confidences default to `0.0` when absent.
pub fn parse_proposals(raw: &serde_json::Value) -> Result<Proposals, BossclawError> {
    let arr = |key: &str| raw.get(key).and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let f = |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
    let s = |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_str()).map(String::from);

    let mut entities = Vec::new();
    for e in arr("entities") {
        if let (Some(mention), Some(entity_type)) = (s(&e, "mention"), s(&e, "entity_type")) {
            entities.push(ProposedEntity { mention, entity_type, confidence: f(&e, "confidence") });
        }
    }
    let mut relations = Vec::new();
    for r in arr("relations") {
        let supported_by = s(&r, "supported_by").unwrap_or_default();
        if supported_by.trim().is_empty() {
            continue; // drop: no source span ⇒ unverifiable
        }
        if let (Some(src), Some(relation), Some(dst)) = (s(&r, "src"), s(&r, "relation"), s(&r, "dst")) {
            relations.push(ProposedRelation {
                src, relation, dst, confidence: f(&r, "confidence"), supported_by,
            });
        }
    }
    let mut retractions = Vec::new();
    for r in arr("retractions") {
        if let (Some(src), Some(relation), Some(dst)) = (s(&r, "src"), s(&r, "relation"), s(&r, "dst")) {
            retractions.push(ProposedRetraction {
                src, relation, dst,
                reason: s(&r, "reason").unwrap_or_default(),
                confidence: f(&r, "confidence"),
            });
        }
    }
    Ok(Proposals { entities, relations, retractions })
}

/// Pass A (propose, spec §3 step 2): build the retrieval-augmented prompt, call
/// the reasoner schema-constrained, and parse the result into [`Proposals`].
/// Pure w.r.t. storage — `recalled` is supplied by the caller (the evolve loop
/// fetches it via M2 recall). Unit-testable with [`crate::reason::ScriptedReasoner`].
pub fn propose(
    reasoner: &dyn Reasoner,
    source: &str,
    recalled: &[String],
) -> Result<Proposals, BossclawError> {
    let prompt = build_pass_a_prompt(source, recalled);
    let raw = reasoner.complete_json(PASS_A_SYSTEM, &prompt, &extraction_schema())?;
    parse_proposals(&raw)
}
```

- [ ] **Step 4 — run, verify pass**

Run: `cargo test -p bossclaw-core --test extract`
Expected: PASS (T3 decision tests + the four new Pass-A tests).

- [ ] **Step 5 — commit**

```bash
git add crates/bossclaw-core/src/extract.rs crates/bossclaw-core/tests/extract.rs
git status -s
git commit -m "feat(bossclaw-core): extract Pass A — retrieval-augmented propose + parse (M4a T4)"
```

---

## Task 5: `extract.rs` Pass B (critique / self-verify)

**Files:**
- Modify: `crates/bossclaw-core/src/extract.rs` (`build_pass_b_prompt`, `critique`, `confirm_retractions`, `is_single_valued`)
- Test: `crates/bossclaw-core/tests/extract.rs`

- [ ] **Step 1 — write the failing tests** (`tests/extract.rs`, append):

```rust
use bossclaw_core::extract::{
    confirm_retractions, critique, is_single_valued, ProposedRelation, ProposedRetraction,
};

#[test]
fn single_valued_relations_are_recognised() {
    assert!(is_single_valued("works_at_primary"));
    assert!(is_single_valued("located_in"));
    assert!(!is_single_valued("knows"), "knows is many-valued");
}

#[test]
fn confirm_retractions_only_fires_on_an_active_edge() {
    // A retraction is confirmed ONLY when its (src, relation, dst) is a still-
    // active edge in the current graph. A retraction of a non-existent edge is
    // dropped (it cannot contradict what was never asserted).
    let retractions = vec![
        ProposedRetraction { src: "Kenny".into(), relation: "works_at_primary".into(),
            dst: "Globex".into(), reason: "moved".into(), confidence: 0.9 },
        ProposedRetraction { src: "Kenny".into(), relation: "works_at_primary".into(),
            dst: "NeverCorp".into(), reason: "x".into(), confidence: 0.9 },
    ];
    // Current active edge-keys (src, relation, dst) the loop passes in.
    let active = vec![("Kenny".to_string(), "works_at_primary".to_string(), "Globex".to_string())];
    let confirmed = confirm_retractions(&retractions, &active);
    assert_eq!(confirmed.len(), 1, "only the retraction matching an active edge survives");
    assert_eq!(confirmed[0].dst, "Globex");
}

#[test]
fn critique_drops_relations_whose_span_is_absent_from_source() {
    // Pass B drops any relation whose supported_by span is NOT a substring of the
    // source memory (the model hallucinated support). Finalize confidence too.
    let source = "Kenny started at Acme last week.";
    let proposals = bossclaw_core::extract::Proposals {
        entities: vec![],
        relations: vec![
            ProposedRelation { src: "Kenny".into(), relation: "works_at".into(), dst: "Acme".into(),
                confidence: 0.9, supported_by: "Kenny started at Acme last week.".into() },
            ProposedRelation { src: "Kenny".into(), relation: "knows".into(), dst: "Zoe".into(),
                confidence: 0.9, supported_by: "Kenny and Zoe are close friends.".into() }, // not in source
        ],
        retractions: vec![],
    };
    let verified = critique(&proposals, source);
    assert_eq!(verified.relations.len(), 1, "the unsupported relation is dropped");
    assert_eq!(verified.relations[0].dst, "Acme");
}
```

- [ ] **Step 2 — run, verify fail**

Run: `cargo test -p bossclaw-core --test extract -- single_valued confirm_retractions critique`
Expected: FAIL — `no function confirm_retractions` / `critique` / `is_single_valued`.

- [ ] **Step 3 — add Pass B to `src/extract.rs`:**

```rust
/// True iff `relation` is single-valued for a subject (a member of
/// [`RELATION_CARDINALITY_SINGLE`]) — a new such fact about the same `src`
/// implies the prior is retired (spec §6). The cardinality HINT; model judgment
/// confirms (neither alone fires an `invalidate`).
pub fn is_single_valued(relation: &str) -> bool {
    RELATION_CARDINALITY_SINGLE.contains(&relation)
}

/// The fixed system instruction for Pass B (critique). Public so the hermetic
/// tests can key the scripted reasoner on the exact pair `critique_with_reasoner`
/// uses (the model-driven Pass B variant, T7); the pure [`critique`] needs no model.
pub const PASS_B_SYSTEM: &str =
    "You are a strict verifier. For each proposed relation, confirm its supported_by \
     span is justified by the SOURCE and the KNOWN neighborhood; drop unsupported \
     relations; confirm or deny each contradiction against the CURRENT edges. Emit \
     ONLY the JSON the schema describes.";

/// Build the Pass-B critique prompt (spec §6): the source text, the Pass-A
/// proposals (re-serialized), and the resolved-entity graph neighborhood (current
/// edges as `src -relation-> dst` lines). Pure string construction.
pub fn build_pass_b_prompt(source: &str, proposals: &Proposals, neighborhood: &[String]) -> String {
    let mut s = String::new();
    s.push_str("SOURCE memory:\n");
    s.push_str(source);
    s.push_str("\n\nPROPOSED relations to verify:\n");
    for r in &proposals.relations {
        s.push_str(&format!("- {} {} {} (span: {})\n", r.src, r.relation, r.dst, r.supported_by));
    }
    s.push_str("\nCURRENT edges in the neighborhood (confirm contradictions against these):\n");
    if neighborhood.is_empty() {
        s.push_str("(none)\n");
    } else {
        for n in neighborhood {
            s.push_str("- ");
            s.push_str(n);
            s.push('\n');
        }
    }
    s
}

/// Pass B (pure self-verify, spec §3 step 5a): drop every relation whose
/// `supported_by` span is NOT a verbatim substring of `source` (the model
/// hallucinated support). Entities and retractions pass through unchanged here;
/// retraction confirmation against active edges is [`confirm_retractions`], and
/// the optional model-driven Pass B (re-scoring confidence) is wired in the
/// evolve loop (T7) bounded by [`MAX_REFLECT`].
pub fn critique(proposals: &Proposals, source: &str) -> Proposals {
    let relations = proposals
        .relations
        .iter()
        .filter(|r| source.contains(&r.supported_by))
        .cloned()
        .collect();
    Proposals {
        entities: proposals.entities.clone(),
        relations,
        retractions: proposals.retractions.clone(),
    }
}

/// Confirm retractions against the CURRENT graph (spec §6): keep only those whose
/// `(src, relation, dst)` is a still-active edge-key in `active_edges`. A
/// retraction of an edge that was never asserted (or already retired) cannot
/// contradict anything, so it is dropped — an `invalidate` only ever fires on a
/// real, still-active edge. Pure: the caller supplies the active edge-keys.
pub fn confirm_retractions(
    retractions: &[ProposedRetraction],
    active_edges: &[(String, String, String)],
) -> Vec<ProposedRetraction> {
    retractions
        .iter()
        .filter(|r| {
            active_edges
                .iter()
                .any(|(s, rel, d)| *s == r.src && *rel == r.relation && *d == r.dst)
        })
        .cloned()
        .collect()
}
```

- [ ] **Step 4 — run, verify pass**

Run: `cargo test -p bossclaw-core --test extract`
Expected: PASS (T3 + T4 + the three new Pass-B tests).

- [ ] **Step 5 — commit**

```bash
git add crates/bossclaw-core/src/extract.rs crates/bossclaw-core/tests/extract.rs
git status -s
git commit -m "feat(bossclaw-core): extract Pass B — self-verify spans + cardinality-gated retractions (M4a T5)"
```

---

## Task 6: `edges` origin/confidence + trust-gated boost + intra-result reinforcement

**Files:**
- Modify: `crates/bossclaw-core/src/graph.rs` (`parse_link_content` → optional confidence; `origin_of` helper; `Edge` gains `origin`/`confidence`; `fold_edges` populates them), `crates/bossclaw-core/src/log.rs` (`link` content gains optional `confidence`; `edges` DDL + fold INSERT/SELECT gain the two columns; the trust-gate predicate in `current_adjacent`; intra-result reinforcement seeding in `recall`), `crates/bossclaw-core/src/recall.rs` (`GRAPH_REINFORCE_TOPK` const)
- Test: `crates/bossclaw-core/tests/graph.rs`, `crates/bossclaw-core/tests/recall.rs`

> ⚠️ **Byte-identical-rebuild contract:** the two new `edges` columns are PURE functions of event fields (`origin` from `model_id`, `confidence` from `link.content`), so the M3/M4a rebuild gate (`rebuild_graph_is_byte_identical*`) MUST still pass — verify it in Step 6.

- [ ] **Step 1 — write the failing tests** (`tests/graph.rs`, append):

```rust
#[test]
fn edges_carry_origin_and_confidence_from_the_producing_link() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let m = log.append(mk_memory("kenny at acme")).unwrap();
    let kenny = log.entity("Kenny", &[], "person", "m4-reasoner", &[m.clone()]).unwrap();
    let acme = log.entity("Acme", &[], "org", "m4-reasoner", &[m.clone()]).unwrap();

    // A machine link WITH confidence (the M4a reasoner producer).
    log.link_machine(&kenny, "works_at", &acme, Some(0.83), "m4-reasoner", &[m.clone()]).unwrap();
    // A manual link (no confidence).
    let other = log.append(mk_memory("note")).unwrap();
    log.link(&m, "relates_to", &other, None, &[m.clone()]).unwrap();
    log.rebuild_graph().unwrap();

    let edges = log.all_edges().unwrap();
    let machine = edges.iter().find(|e| e.relation == "works_at").unwrap();
    assert_eq!(machine.origin, "machine");
    assert!((machine.confidence.unwrap() - 0.83).abs() < 1e-6);
    let manual = edges.iter().find(|e| e.relation == "relates_to").unwrap();
    assert_eq!(manual.origin, "manual");
    assert_eq!(manual.confidence, None, "manual links carry NULL confidence");
}

#[test]
fn edges_origin_confidence_rebuild_is_byte_identical() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let m = log.append(mk_memory("kenny at acme")).unwrap();
    let kenny = log.entity("Kenny", &[], "person", "m4-reasoner", &[m.clone()]).unwrap();
    let acme = log.entity("Acme", &[], "org", "m4-reasoner", &[m.clone()]).unwrap();
    log.link_machine(&kenny, "works_at", &acme, Some(0.42), "m4-reasoner", &[m.clone()]).unwrap();
    log.rebuild_graph().unwrap();
    let e1 = log.all_edges().unwrap();
    log.rebuild_graph().unwrap();
    let e2 = log.all_edges().unwrap();
    assert_eq!(e1, e2, "origin/confidence columns are pure → byte-identical rebuild holds");
}
```

And the trust-gate + reinforcement tests (`tests/recall.rs`, append; reuses `seeded_log`, `find_hit`, `MID_DIM`, `RECALL_TOP_K`):

```rust
/// Trust gate (spec §7): a LOW-confidence machine edge (< TRUST_MIN) must NOT
/// contribute the recall boost, while a manual or ≥TRUST_MIN machine edge does.
/// Score-based: the neighbor's score is unboosted when its only seed-edge is a
/// low-confidence machine edge.
#[test]
fn recall_trust_gate_excludes_low_confidence_machine_edges_from_boost() {
    let (log, _dir, ids) = seeded_log(&[
        "rustacean memory engine ferris",
        "completely unrelated tokens here",
    ]);
    // Low-confidence machine edge from the query-matching hit to the neighbor.
    log.link_machine(&ids[0], "relates_to", &ids[1], Some(0.10), "m4-reasoner", &[ids[0].clone()]).unwrap();
    log.rebuild_graph().unwrap();
    let embedder = MockEmbedder::new(MID_DIM);
    let query = "rustacean memory engine ferris";
    let low = log.recall(&embedder, query, RECALL_TOP_K, &RecallOptions::default()).unwrap();
    let s_low = find_hit(&low, &ids[1]).expect("present").score;

    // Replace with a HIGH-confidence machine edge (re-link supersedes; rebuild).
    log.invalidate(&ids[0], "relates_to", &ids[1], None, &[ids[0].clone()]).unwrap();
    log.link_machine(&ids[0], "relates_to", &ids[1], Some(0.95), "m4-reasoner", &[ids[0].clone()]).unwrap();
    log.rebuild_graph().unwrap();
    let high = log.recall(&embedder, query, RECALL_TOP_K, &RecallOptions::default()).unwrap();
    let s_high = find_hit(&high, &ids[1]).expect("present").score;

    assert!(
        s_high > s_low * 1.2,
        "only the trusted (≥TRUST_MIN) edge boosts: high={s_high}, low={s_low}"
    );
}

/// Intra-result reinforcement (spec §7): a memory that neighbors a NON-top strong
/// fused hit still gets the proximity tilt (M3 boosted only neighbors of the
/// single top-1 hit). With reinforcement, seeding expands to the top
/// GRAPH_REINFORCE_TOPK fused hits.
#[test]
fn recall_intra_result_reinforcement_boosts_neighbor_of_a_non_top_hit() {
    // Query matches event 0 strongest; event 1 also matches (a strong, non-top
    // hit); event 2 neighbors event 1 only. M3 (top-1) would not boost event 2;
    // reinforcement does.
    let (log, _dir, ids) = seeded_log(&[
        "rustacean memory engine ferris crab",   // 0: strongest
        "rustacean memory ferris",                // 1: strong but #2
        "completely unrelated tokens here",       // 2: neighbor of 1
    ]);
    log.link(&ids[1], "relates_to", &ids[2], None, &[ids[1].clone()]).unwrap();
    log.rebuild_graph().unwrap();
    let embedder = MockEmbedder::new(MID_DIM);
    let query = "rustacean memory engine ferris crab";

    let hits = log.recall(&embedder, query, RECALL_TOP_K, &RecallOptions::default()).unwrap();
    let s2 = find_hit(&hits, &ids[2]).expect("neighbor present").score;
    // Retire the edge → event 2 loses the reinforcement boost (baseline).
    log.invalidate(&ids[1], "relates_to", &ids[2], None, &[ids[1].clone()]).unwrap();
    log.rebuild_graph().unwrap();
    let base = log.recall(&embedder, query, RECALL_TOP_K, &RecallOptions::default()).unwrap();
    let s2_base = find_hit(&base, &ids[2]).expect("present").score;
    assert!(
        s2 > s2_base * 1.2,
        "neighbor of a non-top strong hit must be reinforced: boosted={s2}, base={s2_base}"
    );
}
```

- [ ] **Step 2 — run, verify fail**

Run: `cargo test -p bossclaw-core --test graph -- edges_carry_origin edges_origin_confidence_rebuild` then `cargo test -p bossclaw-core --test recall -- recall_trust_gate recall_intra_result`
Expected: FAIL — `no method named link_machine`; `no field origin`/`confidence` on `Edge`; reinforcement not applied.

- [ ] **Step 3 — extend `src/graph.rs`.** (a) Add the two `Edge` fields (after `invalidated_by`):

```rust
    /// Edge origin: `"manual"` iff the producing `link`'s `model_id ==
    /// MANUAL_LINK_PRODUCER`, else `"machine"` (spec §4). A pure function of the
    /// event field → the byte-identical-rebuild gate holds.
    pub origin: String,
    /// Machine-link confidence in `[0, 1]` from the `link` content; `None` for a
    /// manual link (spec §4). Used by the recall trust gate (spec §7).
    pub confidence: Option<f32>,
```

(b) Replace `parse_link_content` to also read the optional confidence, returning a 4-tuple:

```rust
/// Extract `(src, relation, dst, confidence?)` from a `link`/`invalidate` event's
/// content, or `None` if `src`/`relation`/`dst` are missing or non-string
/// (malformed — skipped by the fold). `confidence` is OPTIONAL: absent or
/// non-numeric ⇒ `None` (back-compatible — M3 links have no confidence and keep
/// byte-identical rebuilds). Spec §4: `confidence` lives in the signed content,
/// never in `ModelMeta`.
pub fn parse_link_content(
    content: &serde_json::Value,
) -> Option<(String, String, String, Option<f32>)> {
    let src = content.get("src")?.as_str()?.to_string();
    let relation = content.get("relation")?.as_str()?.to_string();
    let dst = content.get("dst")?.as_str()?.to_string();
    let confidence = content.get("confidence").and_then(|c| c.as_f64()).map(|c| c as f32);
    Some((src, relation, dst, confidence))
}

/// Edge origin from a producing link's `model_id` (spec §4): `"manual"` iff it
/// equals [`MANUAL_LINK_PRODUCER`], else `"machine"`. Single-sourced so the
/// derivation is identical everywhere.
pub fn origin_of(model_id: &str) -> String {
    if model_id == MANUAL_LINK_PRODUCER { "manual".to_string() } else { "machine".to_string() }
}
```

(c) Update `fold_edges` to populate `origin`/`confidence`. The fold needs the producing event's `model_id`, which lives in `ev.model_meta`. Update the destructure + the `link` arm:

```rust
        let (src, relation, dst, confidence) = match parse_link_content(&ev.content) {
            Some(t) => t,
            None => continue,
        };
        let key = (src.clone(), relation.clone(), dst.clone());
        match ev.event_type.as_str() {
            "link" => {
                let valid_from = normalize_ts(ev.valid_time.as_deref().unwrap_or(&ev.ts));
                let ingested_at = normalize_ts(&ev.ts);
                // origin from the signed model_id; a link with no model_meta is
                // treated as manual (M3 hand-links always had model_meta, so this
                // is a defensive default, not a normal path).
                let origin = ev
                    .model_meta
                    .as_ref()
                    .map(|m| origin_of(&m.model_id))
                    .unwrap_or_else(|| "manual".to_string());
                // Manual links carry NULL confidence even if a stray value appears.
                let confidence = if origin == "manual" { None } else { confidence };
                edges.push(Edge {
                    edge_id: ev.id.clone(),
                    src, relation, dst,
                    valid_from, valid_to: None,
                    ingested_at, invalidated_at: None, invalidated_by: None,
                    origin, confidence,
                });
                active.entry(key).or_default().push(edges.len() - 1);
            }
```

> The `invalidate` arm is unchanged (it closes by key; it never reads confidence). Update the existing graph-module unit tests / any `Edge { … }` literals to include the two new fields (search `tests/graph.rs` + `graph.rs` for `Edge {`).

- [ ] **Step 4 — extend `src/log.rs`.** (a) `edges` DDL — add the two columns (after `invalidated_by`):

```rust
                invalidated_by TEXT,
                origin         TEXT NOT NULL DEFAULT 'manual',
                confidence     REAL
```

(b) The `rebuild_graph` INSERT — add the columns + params:

```rust
                "INSERT INTO edges
                   (edge_id, src, relation, dst, valid_from, valid_to,
                    ingested_at, invalidated_at, invalidated_by, origin, confidence)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                rusqlite::params![
                    e.edge_id, e.src, e.relation, e.dst, e.valid_from, e.valid_to,
                    e.ingested_at, e.invalidated_at, e.invalidated_by, e.origin, e.confidence
                ],
```

(c) `query_edges` mapping + every edge `SELECT` column list (in `all_edges`, `neighbors`, `as_of`) — append `origin, confidence` to the column lists and to the row mapper:

```rust
                invalidated_by: r.get(8)?,
                origin: r.get(9)?,
                confidence: r.get(10)?,
```

(The three SELECT strings change from `… invalidated_at, invalidated_by FROM edges …` to `… invalidated_at, invalidated_by, origin, confidence FROM edges …`.)

(d) The F4 malformed-count filter in `rebuild_graph` uses `parse_link_content(...).is_none()` — still valid (the 4-tuple still returns `None` on malformed). No change needed.

(e) Add the `link_machine` public helper (next to `link`):

```rust
    /// Append a signed Tier-B machine `link` carrying an optional `confidence` in
    /// its CONTENT (spec §4/§7 — never in `ModelMeta`). For the M4a reasoner: a
    /// NON-MANUAL producer, so `source_event_ids` MUST be non-empty (the F2 taint
    /// guard rejects an empty set). `confidence` projects to `edges.confidence`
    /// and gates the recall boost (spec §7). Returns the edge's event id.
    pub fn link_machine(
        &self,
        src: &str,
        relation: &str,
        dst: &str,
        confidence: Option<f32>,
        producer: &str,
        source_event_ids: &[String],
    ) -> Result<String, BossclawError> {
        if source_event_ids.is_empty() {
            return Err(BossclawError::InvalidInput(
                "machine link requires explicit non-empty source_event_ids (the cheat-sheet \
                 read-set) — an empty default would launder taint past the §5.11 lineage walk".into(),
            ));
        }
        let mut content = serde_json::json!({ "src": src, "relation": relation, "dst": dst });
        if let Some(c) = confidence {
            content["confidence"] = serde_json::json!(c);
        }
        self.append(Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: "link".to_string(),
            content,
            model_meta: Some(ModelMeta {
                model_id: producer.to_string(),
                prompt_hash: String::new(),
                source_event_ids: source_event_ids.to_vec(),
            }),
            prev_hash: String::new(),
            hash: None,
            signed_by_did: self.signer_did(),
            signature: None,
        })
    }
```

(f) The trust gate — `current_adjacent` must only traverse edges that pass the gate. Replace its two `WHERE invalidated_at IS NULL` clauses with a shared trust predicate:

```rust
        // Trust gate (spec §7): only manual edges OR machine edges with
        // confidence ≥ TRUST_MIN contribute the proximity boost. Low-confidence
        // machine edges are still recorded + queryable, but do not tilt recall.
        let trust = format!(
            "(origin = 'manual' OR (origin = 'machine' AND confidence >= {}))",
            crate::extract::TRUST_MIN
        );
        let sql = format!(
            "SELECT dst AS other FROM edges \
               WHERE invalidated_at IS NULL AND {trust} AND src IN ({placeholders}) \
             UNION \
             SELECT src AS other FROM edges \
               WHERE invalidated_at IS NULL AND {trust} AND dst IN ({placeholders})"
        );
```

> `TRUST_MIN` is an `f32` const formatted into the SQL as a numeric literal (not user input → no injection). `confidence` is `NULL` for manual edges, so the `origin='manual'` arm matches them regardless (NULL never satisfies `>= TRUST_MIN`, which is why the OR is structured this way).

(g) Intra-result reinforcement — in `recall`, expand the auto-seed from top-1 to the top `GRAPH_REINFORCE_TOPK` fused hits. Replace the auto-seed `take(GRAPH_AUTO_SEED_TOPK)` with `take(GRAPH_REINFORCE_TOPK)` and update the import + comment:

```rust
            // Auto-seed expands to the top GRAPH_REINFORCE_TOPK fused hits
            // (intra-result reinforcement, spec §7): a memory linked to ANY strong
            // hit gets the tilt, not only neighbors of the single top hit.
            by_score.into_iter().take(GRAPH_REINFORCE_TOPK).map(|(id, _)| id.clone()).collect()
```

Add `GRAPH_REINFORCE_TOPK` to the `use crate::recall::{…}` import group in `log.rs`.

- [ ] **Step 5 — add `GRAPH_REINFORCE_TOPK` to `src/recall.rs`** (after `GRAPH_AUTO_SEED_TOPK`):

```rust
/// Intra-result reinforcement seed count (spec §7): auto-seed proximity from the
/// top N fused hits, not just the single top-1 (the M3 `GRAPH_AUTO_SEED_TOPK`).
/// A memory linked to several of the result set's strong hits gets the tilt. 3 is
/// conservative — enough to catch a cluster, small enough that the boost stays a
/// tilt (a deep hit is unlikely to seed). Tunable in dogfooding.
pub const GRAPH_REINFORCE_TOPK: usize = 3;
```

- [ ] **Step 6 — run, verify pass (incl. the byte-identical gate)**

Run: `cargo test -p bossclaw-core --test graph` then `cargo test -p bossclaw-core --test recall`
Expected: PASS — new origin/confidence + trust-gate + reinforcement tests AND the pre-existing `rebuild_graph_is_byte_identical*` / M3 boost tests (the columns are pure → rebuild stays byte-identical; auto-seed widening must not break the existing top-1 boost tests — verify both).

- [ ] **Step 7 — commit**

```bash
git add crates/bossclaw-core/src/graph.rs crates/bossclaw-core/src/log.rs crates/bossclaw-core/src/recall.rs crates/bossclaw-core/tests/graph.rs crates/bossclaw-core/tests/recall.rs
git status -s
git commit -m "feat(bossclaw-core): edges origin/confidence + trust-gated boost + intra-result reinforcement (M4a T6)"
```

---

## Task 7: `evolve.rs` runtime (cursor + `evolve_once` + scheduler + off-switch + observability)

**Files:**
- Create: `crates/bossclaw-core/src/evolve.rs`
- Modify: `crates/bossclaw-core/src/log.rs` (`evolve_cursor` DDL + `evolve_cursor`/`set_evolve_cursor` read/write; `unprocessed_memories_since`; `evolve_enabled`; the public `evolve_once`/`evolve_status` entry points delegating to `evolve.rs`), `crates/bossclaw-core/src/lib.rs` (`EvolveStatus` re-export — added in T1)
- Test: `crates/bossclaw-core/tests/evolve.rs` (new, hermetic)

- [ ] **Step 1 — write the failing tests** (`tests/evolve.rs`, new file). Hermetic: `MockEmbedder` + `ScriptedReasoner`. The reasoner is scripted on the EXACT `(system, prompt)` `evolve_once` produces, so the loop is deterministic:

```rust
//! Hermetic end-to-end tests for the evolve loop: one full tick (recall → Pass A
//! → resolve → augment → Pass B → emit → advance cursor), idempotency, cursor
//! persistence, byte-identical rebuild with evolve output, and the off-switch.
//! Driven by MockEmbedder + ScriptedReasoner — no live model.

use bossclaw_core::embed::MockEmbedder;
use bossclaw_core::event::Event;
use bossclaw_core::extract::{build_pass_a_prompt, PASS_A_SYSTEM};
use bossclaw_core::log::EventLog;
use bossclaw_core::reason::ScriptedReasoner;
use ed25519_dalek::SigningKey;
use serde_json::json;

const DEK: [u8; 32] = [42u8; 32];
const KEY_BYTES: [u8; 32] = [7u8; 32];
const MID_DIM: usize = 64;

fn open_log(dir: &std::path::Path) -> EventLog {
    let key = SigningKey::from_bytes(&KEY_BYTES);
    EventLog::open(&dir.join("m.db"), &DEK, key).unwrap()
}
fn mk_memory(text: &str) -> Event {
    Event {
        id: String::new(), ts: String::new(), valid_time: None,
        event_type: "memory".to_string(), content: json!({ "text": text }),
        model_meta: None, prev_hash: String::new(), hash: None,
        signed_by_did: "did:wba:AIR-TEST".to_string(), signature: None,
    }
}

/// Script the reasoner for the single memory's Pass-A prompt (recall is empty on
/// a one-memory store, so the prompt is deterministic). Pass B here is the PURE
/// `critique` (no model call) — the loop only calls the model for Pass A +
/// mid-band adjudication, neither of which we hit beyond Pass A in this fixture.
fn scripted_for(source: &str) -> ScriptedReasoner {
    let prompt = build_pass_a_prompt(source, &[]);
    let canned = json!({
        "entities": [
            { "mention": "Kenny", "entity_type": "person", "confidence": 0.95 },
            { "mention": "Acme",  "entity_type": "org",    "confidence": 0.95 }
        ],
        "relations": [{
            "src": "Kenny", "relation": "works_at", "dst": "Acme",
            "confidence": 0.9, "supported_by": "Kenny works at Acme."
        }],
        "retractions": []
    });
    ScriptedReasoner::new("scripted-evolve-v1").with_response(PASS_A_SYSTEM, &prompt, canned)
}

#[test]
fn evolve_once_emits_entities_and_a_link_then_advances_the_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let embedder = MockEmbedder::new(MID_DIM);
    let source = "Kenny works at Acme.";
    let m = log.append(mk_memory(source)).unwrap();
    log.rederive_pending(&embedder).unwrap();
    log.rebuild_indexes(&embedder).unwrap();
    log.rebuild_graph().unwrap();
    log.rebuild_entity_index(&embedder).unwrap();

    let reasoner = scripted_for(source);
    let report = log.evolve_once(&embedder, &reasoner).unwrap();
    assert!(report.entities_minted >= 1, "at least one entity minted");
    assert!(report.links_emitted >= 1, "the works_at link emitted");

    // Cursor advanced past the processed memory's seq.
    log.rebuild_graph().unwrap();
    assert!(log.all_entities().unwrap().len() >= 2, "Kenny + Acme entities folded");
    let edges = log.all_edges().unwrap();
    assert!(edges.iter().any(|e| e.relation == "works_at" && e.origin == "machine"));
    // Every emitted event's source_event_ids includes the inducing memory (F2).
    let link_ev = log.stream_all().unwrap().into_iter()
        .find(|e| e.event_type == "link").unwrap();
    assert!(link_ev.model_meta.unwrap().source_event_ids.contains(&m),
        "machine link lineage reaches the inducing memory (provenance, spec §16)");
}

#[test]
fn evolve_once_is_idempotent_on_a_second_run() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let embedder = MockEmbedder::new(MID_DIM);
    let source = "Kenny works at Acme.";
    log.append(mk_memory(source)).unwrap();
    log.rederive_pending(&embedder).unwrap();
    log.rebuild_indexes(&embedder).unwrap();
    log.rebuild_graph().unwrap();
    log.rebuild_entity_index(&embedder).unwrap();
    let reasoner = scripted_for(source);

    log.evolve_once(&embedder, &reasoner).unwrap();
    let count_after_first = log.count().unwrap();
    // Second run: cursor is past the only memory → nothing to process → no new events.
    let report2 = log.evolve_once(&embedder, &reasoner).unwrap();
    assert_eq!(report2.entities_minted, 0);
    assert_eq!(report2.links_emitted, 0);
    assert_eq!(log.count().unwrap(), count_after_first, "re-running emits nothing new");
}

#[test]
fn evolve_cursor_persists_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    {
        let log = open_log(dir.path());
        log.set_evolve_cursor(7).unwrap();
        assert_eq!(log.evolve_cursor().unwrap(), 7);
    }
    // Reopen: the cursor is persistent progress state, NOT a fold — it survives.
    let log = open_log(dir.path());
    assert_eq!(log.evolve_cursor().unwrap(), 7, "cursor persists (not rebuilt from events)");
}

#[test]
fn evolve_once_is_a_noop_when_disabled_by_config() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let embedder = MockEmbedder::new(MID_DIM);
    let source = "Kenny works at Acme.";
    log.append(mk_memory(source)).unwrap();
    log.rederive_pending(&embedder).unwrap();
    log.rebuild_indexes(&embedder).unwrap();
    log.rebuild_graph().unwrap();
    log.rebuild_entity_index(&embedder).unwrap();
    // Hard off-switch: a config event with evolve_enabled=false.
    log.append(Event {
        id: String::new(), ts: String::new(), valid_time: None,
        event_type: "config".to_string(),
        content: json!({ "evolve_enabled": false }),
        model_meta: None, prev_hash: String::new(), hash: None,
        signed_by_did: "did:wba:AIR-TEST".to_string(), signature: None,
    }).unwrap();
    let reasoner = scripted_for(source);
    let report = log.evolve_once(&embedder, &reasoner).unwrap();
    assert_eq!(report.entities_minted, 0, "disabled loop is a no-op");
    assert_eq!(report.links_emitted, 0);
    assert!(report.skipped_disabled, "the report flags the off-switch");
}

#[test]
fn evolve_status_reports_queue_depth_and_last_tick() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let embedder = MockEmbedder::new(MID_DIM);
    log.append(mk_memory("a")).unwrap();
    log.append(mk_memory("b")).unwrap();
    // Cursor at 0 → both memories are behind it → queue depth 2.
    let status = log.evolve_status().unwrap();
    assert_eq!(status.queue_depth, 2, "two unprocessed memories behind the cursor");
    assert_eq!(status.last_tick_ms, None, "no tick run yet");
}
```

- [ ] **Step 2 — run, verify fail**

Run: `cargo test -p bossclaw-core --test evolve`
Expected: FAIL — `no method named evolve_once` / `set_evolve_cursor` / `evolve_cursor` / `evolve_status`; `no type EvolveStatus`.

- [ ] **Step 3 — add the `evolve_cursor` DDL** in `EventLog::open` (`src/log.rs`), right after the `entity_vectors` `CREATE TABLE` (T3 Step 7):

```rust
        // Evolve-loop progress (re-derivable progress state — NOT a Tier-A fold,
        // spec §4). Single row, advanced after each committed batch. Losing it
        // only re-processes events (idempotent via §3 step 6), never corrupts.
        store.exec(
            "CREATE TABLE IF NOT EXISTS evolve_cursor (
                id       INTEGER PRIMARY KEY CHECK (id = 0),
                last_seq INTEGER NOT NULL
            )",
        )?;
```

- [ ] **Step 4 — create `src/evolve.rs`** (the loop runtime types + the pure scheduler/debounce + the orchestration delegated from `EventLog`). The SQL lives on `EventLog`; `evolve.rs` owns the orchestration + the observability/scheduler types:

```rust
//! The evolve-loop runtime (spec §8): the always-on curator that turns new
//! memories into signed `entity`/`link`/`invalidate` events. It is NOT a
//! privileged writer — every emit goes through [`crate::log::EventLog::append`]
//! (the single serialized writer). The loop holds an `Embedder` + a `Reasoner` +
//! a read handle for recall/graph; it never opens a second writer.
//!
//! `evolve_once` (one tick) lives as a method on [`crate::log::EventLog`] (it is
//! SQL/IO-heavy); this module holds the report/status/scheduler types and the
//! PURE debounce decision so they are unit-testable without a DB.

/// What one [`crate::log::EventLog::evolve_once`] tick did (spec §8 observability
/// + the test oracle). Counts are per-tick, not cumulative.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EvolveReport {
    /// New `entity` events minted this tick (resolution chose Mint).
    pub entities_minted: usize,
    /// Machine `link` events emitted this tick (new active edge-keys only).
    pub links_emitted: usize,
    /// `invalidate` events emitted this tick (confirmed contradictions).
    pub invalidates_emitted: usize,
    /// Memories processed this tick (≤ `EVOLVE_BATCH`).
    pub memories_processed: usize,
    /// True iff the tick short-circuited because the off-switch is engaged.
    pub skipped_disabled: bool,
}

/// A snapshot of loop health for the desktop (spec §8 observability surface).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvolveStatus {
    /// Events behind the cursor (unprocessed `memory` events) right now.
    pub queue_depth: usize,
    /// Wall-clock duration of the most recent tick in ms, or `None` if no tick
    /// has run since open.
    pub last_tick_ms: Option<u128>,
    /// Total reasoner/tick errors observed since open (each made the tick a
    /// retryable no-op — spec §10).
    pub error_count: usize,
    /// The most recent error message, if any.
    pub last_error: Option<String>,
    /// Whether the loop is currently enabled (the latest `config`
    /// `evolve_enabled`, defaulting to enabled when unset).
    pub enabled: bool,
}

/// Decide whether a debounced tick is due (spec §8 scheduler): a tick fires once
/// at least `debounce_ms` have elapsed since the last append that scheduled it.
/// PURE — the caller supplies the clock readings, so this is unit-testable.
/// `now_ms` and `last_append_ms` are monotonic millisecond readings.
pub fn debounce_due(now_ms: u128, last_append_ms: u128, debounce_ms: u128) -> bool {
    now_ms.saturating_sub(last_append_ms) >= debounce_ms
}

#[cfg(test)]
mod tests {
    use super::debounce_due;

    #[test]
    fn debounce_fires_only_after_the_window() {
        assert!(!debounce_due(1500, 1000, 2000), "0.5s < 2s window → not due");
        assert!(debounce_due(3000, 1000, 2000), "2s elapsed → due");
        assert!(debounce_due(3001, 1000, 2000), "past the window → due");
    }
}
```

- [ ] **Step 5 — add the `EventLog` evolve glue** (`src/log.rs`, `impl EventLog`). Import the consts + types at the top: extend with `use crate::evolve::{EvolveReport, EvolveStatus};` and `use crate::extract::{EVOLVE_BATCH, MAX_REFLECT};` (add `EVOLVE_BATCH` to `extract` — see note). The methods:

```rust
    /// Read the evolve cursor (the last processed `seq`); `0` if never set (the
    /// table is empty on a fresh store → no memory has been processed).
    pub fn evolve_cursor(&self) -> Result<i64, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let seq = conn
            .query_row("SELECT last_seq FROM evolve_cursor WHERE id = 0", [], |r| r.get(0))
            .optional()?
            .unwrap_or(0);
        Ok(seq)
    }

    /// Set the evolve cursor to `last_seq` (idempotent upsert of the single row).
    /// Persistent progress state — NOT rebuilt from events (spec §4).
    pub fn set_evolve_cursor(&self, last_seq: i64) -> Result<(), BossclawError> {
        let store = self.inner.lock().expect(POISON);
        store.conn().execute(
            "INSERT INTO evolve_cursor (id, last_seq) VALUES (0, ?1)
             ON CONFLICT(id) DO UPDATE SET last_seq = ?1",
            rusqlite::params![last_seq],
        )?;
        Ok(())
    }

    /// Whether the evolve loop is enabled: the latest `config` event's
    /// `evolve_enabled` flag, defaulting to `true` when no config sets it (spec
    /// §8 off-switch). Honored BEFORE any model call.
    pub fn evolve_enabled(&self) -> Result<bool, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        // Scan configs newest-first; the first one that carries the flag wins.
        let mut stmt = conn.prepare(
            "SELECT payload FROM events WHERE event_type = 'config' ORDER BY seq DESC",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        for row in rows {
            let ev: Event = serde_json::from_str(&row?)?;
            if let Some(flag) = ev.content.get("evolve_enabled").and_then(|v| v.as_bool()) {
                return Ok(flag);
            }
        }
        Ok(true) // default enabled
    }

    /// The `(seq, id, text)` of each unprocessed `memory` event strictly after the
    /// cursor, in `seq ASC` order, capped at `limit` (the per-tick batch). Only
    /// `memory` events are processed (the evolve unit of work). Returns owned data
    /// so the store lock is released before any model/embedder call.
    fn unprocessed_memories_since(
        &self,
        cursor: i64,
        limit: usize,
    ) -> Result<Vec<(i64, String, String)>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT seq, id, payload FROM events
             WHERE event_type = 'memory' AND seq > ?1 ORDER BY seq ASC LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![cursor, limit as i64], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (seq, id, payload) = row?;
            let ev: Event = serde_json::from_str(&payload)?;
            if let Some(text) = ev.content.get("text").and_then(|t| t.as_str()) {
                out.push((seq, id, text.to_string()));
            }
        }
        Ok(out)
    }

    /// The set of CURRENT active edge-keys `(src, relation, dst)` — used by Pass B
    /// to confirm a retraction fires only on a still-active edge (spec §6).
    fn active_edge_keys(&self) -> Result<Vec<(String, String, String)>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT src, relation, dst FROM edges WHERE invalidated_at IS NULL",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Run ONE evolve tick (spec §3, §8): for each unprocessed `memory` (≤
    /// [`EVOLVE_BATCH`]): recall context → Pass A propose → resolve each entity
    /// mention → augment with the resolved-entity neighborhood → Pass B critique
    /// (pure span-verify + cardinality-gated retraction confirmation) → emit
    /// `entity`/`invalidate`/`link` events through [`EventLog::append`] → advance
    /// the cursor after the batch commits. Idempotent: an active edge-key is
    /// skipped, a resolved entity is reused. The loop is NOT a privileged writer.
    ///
    /// Degrade-never-break (spec §10): the off-switch short-circuits to a no-op;
    /// a reasoner error on a memory logs + skips that memory (the cursor does not
    /// advance past an unprocessed memory) rather than failing the tick.
    pub fn evolve_once(
        &self,
        embedder: &dyn Embedder,
        reasoner: &dyn crate::reason::Reasoner,
    ) -> Result<EvolveReport, BossclawError> {
        let mut report = EvolveReport::default();
        if !self.evolve_enabled()? {
            report.skipped_disabled = true;
            return Ok(report);
        }
        let cursor = self.evolve_cursor()?;
        let batch = self.unprocessed_memories_since(cursor, EVOLVE_BATCH)?;
        let active = self.active_edge_keys()?;
        let mut last_committed_seq = cursor;

        for (seq, mem_id, text) in batch {
            // ── 1. recall context (M2). entity-kind is excluded from recall by
            //    construction (separate index), so neighbors are memories/pages. ──
            let recalled: Vec<String> = self
                .recall(embedder, &text, crate::extract::GRAPH_CONTEXT_K, &RecallOptions::default())
                .map(|hits| {
                    hits.into_iter()
                        .filter(|h| h.event_id != mem_id) // never feed the source back as context
                        .map(|h| h.event_id)
                        .collect()
                })
                .unwrap_or_default();
            let recalled_texts = self.texts_for_ids(&recalled)?;

            // ── 2. Pass A — propose. A reasoner error makes THIS memory a no-op
            //    (skip; cursor does not advance past it) — spec §10. ──
            let proposals = match crate::extract::propose(reasoner, &text, &recalled_texts) {
                Ok(p) => p,
                Err(e) => {
                    log::warn!("evolve: Pass A failed for memory {mem_id}, skipping tick: {e}");
                    break; // stop the batch; cursor stays at last_committed_seq
                }
            };

            // ── 3. resolve each entity mention → a stable entity:<ulid>. Mint via
            //    a signed entity event when resolution says Mint; reuse otherwise.
            //    The read-set (source + recalled + neighbors) is the provenance. ──
            let read_set: Vec<String> = {
                let mut v = vec![mem_id.clone()];
                v.extend(recalled.iter().cloned());
                v
            };
            let mut mention_to_id: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();
            for ent in &proposals.entities {
                let decision = self.resolve_mention(embedder, reasoner, &ent.mention)?;
                let id = match decision {
                    crate::extract::ResolveDecision::Merge(id) => id,
                    crate::extract::ResolveDecision::Mint
                    | crate::extract::ResolveDecision::Adjudicate(_) => {
                        // Adjudicate is collapsed inside resolve_mention; only
                        // Merge/Mint reach here. Mint a fresh signed entity.
                        let new_id = self.entity(
                            &ent.mention, &[], &ent.entity_type, reasoner.model_id(), &read_set,
                        )?;
                        self.derive_entity_vector(embedder, &new_id, &ent.mention)?;
                        report.entities_minted += 1;
                        new_id
                    }
                };
                mention_to_id.insert(ent.mention.clone(), id);
            }
            // Keep the entity index fresh so within-batch mints are resolvable.
            self.rebuild_entity_index(embedder)?;

            // ── 4 + 5. Pass B — pure self-verify (spans) + cardinality-gated
            //    retraction confirmation against the CURRENT active edges. Bounded
            //    by MAX_REFLECT total passes (Pass A + this critique = 2). ──
            debug_assert!(MAX_REFLECT >= 2, "Pass A + one critique");
            let verified = crate::extract::critique(&proposals, &text);
            let confirmed = crate::extract::confirm_retractions(&verified.retractions, &active);

            // ── 6a. invalidate confirmed contradictions FIRST (so the fold closes
            //    the old interval before the replacement opens). ──
            for r in &confirmed {
                let (s, d) = (self.map_mention(&mention_to_id, &r.src), self.map_mention(&mention_to_id, &r.dst));
                self.invalidate(&s, &r.relation, &d, None, &read_set)?;
                report.invalidates_emitted += 1;
            }

            // ── 6b. emit confirmed relations as machine links, skipping any whose
            //    (src, relation, dst) is ALREADY an active edge (idempotency). ──
            for rel in &verified.relations {
                let s = self.map_mention(&mention_to_id, &rel.src);
                let d = self.map_mention(&mention_to_id, &rel.dst);
                let key = (s.clone(), rel.relation.clone(), d.clone());
                if active.iter().any(|k| *k == key) {
                    continue; // already asserted → emit nothing (idempotent)
                }
                self.link_machine(
                    &s, &rel.relation, &d, Some(rel.confidence), reasoner.model_id(), &read_set,
                )?;
                report.links_emitted += 1;
            }

            report.memories_processed += 1;
            last_committed_seq = seq;
        }

        // ── 7. advance the cursor to the last fully-processed memory's seq. ──
        if last_committed_seq > cursor {
            self.set_evolve_cursor(last_committed_seq)?;
        }
        Ok(report)
    }

    /// Map a proposed mention to its resolved `entity:<ulid>` if known, else pass
    /// the raw string through (a relation endpoint the model named but did not
    /// list as an entity — kept as an opaque node id, never silently dropped).
    fn map_mention(
        &self,
        mention_to_id: &std::collections::HashMap<String, String>,
        mention: &str,
    ) -> String {
        mention_to_id.get(mention).cloned().unwrap_or_else(|| mention.to_string())
    }

    /// Fetch the `content["text"]` of each id in `ids` (memory/page events), in
    /// the given order, skipping ids with no text. Used to turn recalled ids into
    /// the Pass-A cheat-sheet text.
    fn texts_for_ids(&self, ids: &[String]) -> Result<Vec<String>, BossclawError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders: String =
            (0..ids.len()).map(|i| format!("?{}", i + 1)).collect::<Vec<_>>().join(",");
        let sql = format!("SELECT id, payload FROM events WHERE id IN ({placeholders})");
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(params.as_slice(), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        // Preserve the caller's id order (recall rank), not SQL row order.
        let mut by_id: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        for row in rows {
            let (id, payload) = row?;
            let ev: Event = serde_json::from_str(&payload)?;
            if let Some(t) = ev.content.get("text").and_then(|t| t.as_str()) {
                by_id.insert(id, t.to_string());
            }
        }
        Ok(ids.iter().filter_map(|id| by_id.get(id).cloned()).collect())
    }

    /// A snapshot of evolve-loop health (spec §8). `queue_depth` = unprocessed
    /// `memory` events behind the cursor; `last_tick_ms`/`error_count`/`last_error`
    /// are process-local (reset on open — they are observability, not persisted
    /// state); `enabled` reflects the off-switch.
    ///
    /// v1 returns a fresh status computed from the log + the cursor; the running
    /// last_tick/error counters are surfaced by the desktop's loop driver (M7),
    /// which owns the long-lived scheduler. Here `last_tick_ms`/`error_count` are
    /// `None`/`0` (no persisted tick history) so the method is pure-read + testable.
    pub fn evolve_status(&self) -> Result<EvolveStatus, BossclawError> {
        let cursor = self.evolve_cursor()?;
        let queue_depth = {
            let store = self.inner.lock().expect(POISON);
            let conn = store.conn();
            conn.query_row(
                "SELECT count(*) FROM events WHERE event_type = 'memory' AND seq > ?1",
                rusqlite::params![cursor],
                |r| r.get::<_, i64>(0),
            )? as usize
        };
        Ok(EvolveStatus {
            queue_depth,
            last_tick_ms: None,
            error_count: 0,
            last_error: None,
            enabled: self.evolve_enabled()?,
        })
    }
```

> NOTE — `EVOLVE_BATCH`/`EVOLVE_DEBOUNCE` placement: the file-structure table puts the loop consts in `evolve.rs`, but `evolve_once` (on `EventLog`) needs `EVOLVE_BATCH`, and `extract.rs` already holds `GRAPH_CONTEXT_K`/`MAX_REFLECT`. To keep one import path and avoid a cycle, define `EVOLVE_BATCH` and `EVOLVE_DEBOUNCE` in `extract.rs` alongside the other tunables (it is the pure-consts module), and re-export them from `evolve.rs` (`pub use crate::extract::{EVOLVE_BATCH, EVOLVE_DEBOUNCE};`) so the spec's "evolve consts" grouping still reads naturally. Add to `extract.rs`:
>
> ```rust
> /// Max memories processed per evolve tick (spec §11): bounds tick latency.
> /// Tunable in dogfooding.
> pub const EVOLVE_BATCH: usize = 16;
>
> /// Debounce after an append before an evolve tick fires, in milliseconds (spec
> /// §11): coalesces a burst of appends into one tick. Tunable.
> pub const EVOLVE_DEBOUNCE_MS: u128 = 2000;
> ```
>
> …and in `evolve.rs`: `pub use crate::extract::{EVOLVE_BATCH, EVOLVE_DEBOUNCE_MS};` (single-sourced; no magic numbers, no cycle — `evolve` already depends on `extract`).

- [ ] **Step 6 — run, verify pass**

Run: `cargo test -p bossclaw-core --test evolve`
Expected: PASS (all five evolve tests). Then the pure debounce unit test: `cargo test -p bossclaw-core --lib evolve::tests`.

- [ ] **Step 7 — commit**

```bash
git add crates/bossclaw-core/src/evolve.rs crates/bossclaw-core/src/extract.rs crates/bossclaw-core/src/log.rs crates/bossclaw-core/tests/evolve.rs
git status -s
git commit -m "feat(bossclaw-core): evolve runtime — cursor + evolve_once + off-switch + observability (M4a T7)"
```

---

## Task 8: Live-Ollama gate + CHANGELOG + final gates

**Files:**
- Create: `crates/bossclaw-core/tests/live_ollama.rs` (`#[ignore]`)
- Modify: `crates/bossclaw-core/CHANGELOG.md`

- [ ] **Step 1 — write the live behavioral gate** (`tests/live_ollama.rs`, new file). `#[ignore]` so CI stays hermetic; a local must-run that asserts *properties, not bytes* against the real `qwen2.5:7b-instruct` (the M4a analogue of M2's `recall@3`). Gated behind the `ollama` feature so it only compiles when the real backend is built:

```rust
//! Live behavioral gate for the M4a clever linker (spec §2.2, §12). `#[ignore]`:
//! it requires a running local Ollama with `qwen2.5:7b-instruct` and is NOT part
//! of the hermetic CI suite. It asserts PROPERTIES, never byte-identity (a live
//! LLM is non-deterministic). Run locally with:
//!   `cargo test -p bossclaw-core --features ollama --test live_ollama -- --ignored`
#![cfg(feature = "ollama")]

use bossclaw_core::embed::MockEmbedder;
use bossclaw_core::event::Event;
use bossclaw_core::log::EventLog;
use bossclaw_core::ollama::OllamaReasoner;
use ed25519_dalek::SigningKey;
use serde_json::json;

const DEK: [u8; 32] = [42u8; 32];
const KEY_BYTES: [u8; 32] = [7u8; 32];
const MID_DIM: usize = 64;
const MODEL: &str = "qwen2.5:7b-instruct";

fn open_log(dir: &std::path::Path) -> EventLog {
    let key = SigningKey::from_bytes(&KEY_BYTES);
    EventLog::open(&dir.join("m.db"), &DEK, key).unwrap()
}
fn mk_memory(text: &str) -> Event {
    Event {
        id: String::new(), ts: String::new(), valid_time: None,
        event_type: "memory".to_string(), content: json!({ "text": text }),
        model_meta: None, prev_hash: String::new(), hash: None,
        signed_by_did: "did:wba:AIR-TEST".to_string(), signature: None,
    }
}

/// Append `text`, refresh all indexes, run one evolve tick against the real model.
fn ingest_and_evolve(log: &EventLog, embedder: &MockEmbedder, reasoner: &OllamaReasoner, text: &str) {
    log.append(mk_memory(text)).unwrap();
    log.rederive_pending(embedder).unwrap();
    log.rebuild_indexes(embedder).unwrap();
    log.rebuild_graph().unwrap();
    log.rebuild_entity_index(embedder).unwrap();
    log.evolve_once(embedder, reasoner).unwrap();
}

#[test]
#[ignore = "requires a local Ollama running qwen2.5:7b-instruct"]
fn live_a_memory_naming_a_person_yields_at_least_one_entity() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let embedder = MockEmbedder::new(MID_DIM);
    let reasoner = OllamaReasoner::new(MODEL);
    ingest_and_evolve(&log, &embedder, &reasoner, "Kenny is a software engineer at Acme.");
    log.rebuild_graph().unwrap();
    let entities = log.all_entities().unwrap();
    assert!(!entities.is_empty(), "a memory naming a person must yield ≥1 entity");
}

#[test]
#[ignore = "requires a local Ollama running qwen2.5:7b-instruct"]
fn live_a_stated_relationship_yields_a_link_with_a_supported_by_span() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let embedder = MockEmbedder::new(MID_DIM);
    let reasoner = OllamaReasoner::new(MODEL);
    ingest_and_evolve(&log, &embedder, &reasoner, "Kenny works at Acme.");
    log.rebuild_graph().unwrap();
    let edges = log.all_edges().unwrap();
    assert!(
        edges.iter().any(|e| e.origin == "machine"),
        "a stated relationship must yield at least one machine link"
    );
    // The link's source span provenance: at least one emitted link event carries
    // a non-empty supported_by-derived confidence (machine origin ⇒ confidence set).
    assert!(
        edges.iter().any(|e| e.origin == "machine" && e.confidence.is_some()),
        "machine links carry a confidence (the supported_by span drove extraction)"
    );
}

#[test]
#[ignore = "requires a local Ollama running qwen2.5:7b-instruct"]
fn live_a_contradiction_across_two_memories_yields_an_invalidate() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let embedder = MockEmbedder::new(MID_DIM);
    let reasoner = OllamaReasoner::new(MODEL);
    // Establish, then contradict (a single-valued relation: primary employer).
    ingest_and_evolve(&log, &embedder, &reasoner, "Kenny's primary job is at Globex.");
    ingest_and_evolve(&log, &embedder, &reasoner, "Kenny left Globex; his primary job is now at Acme.");
    log.rebuild_graph().unwrap();
    let invalidated = log
        .stream_all().unwrap().into_iter()
        .any(|e| e.event_type == "invalidate");
    assert!(invalidated, "a contradiction across two memories must yield an invalidate");
}

#[test]
#[ignore = "requires a local Ollama running qwen2.5:7b-instruct"]
fn live_re_running_a_tick_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let embedder = MockEmbedder::new(MID_DIM);
    let reasoner = OllamaReasoner::new(MODEL);
    ingest_and_evolve(&log, &embedder, &reasoner, "Kenny works at Acme.");
    let count_after_first = log.count().unwrap();
    // A second tick with no new memories must emit nothing (cursor is past it).
    let report = log.evolve_once(&embedder, &reasoner).unwrap();
    assert_eq!(report.memories_processed, 0, "no unprocessed memories on re-run");
    assert_eq!(log.count().unwrap(), count_after_first, "re-running adds no events");
}
```

- [ ] **Step 2 — run the hermetic gate (live tests excluded by default)**

Run: `cargo test -p bossclaw-core`
Expected: PASS, 0 failures — every `#[ignore]` live test is skipped; the whole hermetic suite (reason, extract, entity_resolution, evolve, graph, recall, chain, no_plaintext, vectors) is green. Confirm the new test files all run.

- [ ] **Step 3 — run the live gate locally (dogfood; NOT in CI)**

Run: `cargo test -p bossclaw-core --features ollama --test live_ollama -- --ignored`
Expected: PASS against the real `qwen2.5:7b-instruct` (Ollama running). This is the dogfood step — if a property fails, tune the prompts/consts (not the test's properties) and re-run. Record the outcome in the session handoff.

- [ ] **Step 4 — add the M4a CHANGELOG entry.** Open `crates/bossclaw-core/CHANGELOG.md`, match the M3 section's heading/sub-section style (`#### Added`, `#### Tests`), add a matching M4a section at the top of `[Unreleased]` (above the M3 section):

```markdown
### M4a — Clever Linker (2026-06-16)

The LLM auto-linker that *populates* the M3 graph. A local model
(`qwen2.5:7b-instruct` via Ollama) reads each new memory, extracts entities +
typed relationships, resolves entities against the existing graph, retires
contradicted facts, and appends signed Tier-B `entity`/`link`/`invalidate`
events through the single-writer `append` — feeding the M3 fold, which makes
the next recall smarter. The closed loop (recall → extract → graph → recall) is
the architecture; the model's output is data, never authority
(invalidate-not-delete, confidence/trust-gating, every emit serialized).

#### Added

- **Reasoner seam** (`src/reason.rs`) — the `Reasoner` trait
  (`complete_json` + `model_id`); a deterministic `ScriptedReasoner` test double
  (canned JSON keyed by SHA-256 of `(system, prompt)`); the extraction +
  adjudication JSON-schema builders. Pure — no I/O.
- **`OllamaReasoner`** (`src/ollama.rs`, feature `ollama`) — POSTs `/api/chat`
  to loopback `127.0.0.1:11434`, `format` = the schema, `options.temperature =
  0`, a digest-pinnable model tag, refusing any non-loopback host (no egress).
  Behind the feature so the default build stays pure (no network dep).
- **`entity` Tier-B event + `entities` projection** (`src/graph.rs`,
  `src/log.rs`) — `EventLog::entity(label, aliases, entity_type, producer,
  source_event_ids)` mints a stable `entity:<ulid>` node; a NON-manual producer
  → `source_event_ids` MUST be non-empty (the F2 taint guard, extended).
  `rebuild_graph` folds entities into an `entities` table + marks those node ids
  `kind="entity"`. Byte-identical on rebuild.
- **Embedding entity resolution** (`src/extract.rs`, `src/log.rs`) — embed the
  mention, search a dedicated entity vector index (`entity_vectors`,
  kind-isolated from recall), apply `RESOLVE_HIGH`/`RESOLVE_LOW`, route the
  mid-band to the reasoner to adjudicate. No duplicate entities.
- **Retrieval-augmented extraction** (`src/extract.rs`, PURE) — Pass A (propose:
  cheat-sheet prompt + seed relation vocabulary + few-shot → parse to
  `{entities, relations, retractions}` each with confidence + a mandatory
  `supported_by` span); Pass B (critique: drop relations whose span is absent
  from the source, confirm contradictions against current edges via a relation-
  cardinality table). Bounded by `MAX_REFLECT`.
- **`edges` origin/confidence + trust-gated boost + intra-result reinforcement**
  (`src/graph.rs`, `src/log.rs`, `src/recall.rs`) — the M3 `edges` fold gains
  `origin` (`'manual'` iff `model_id == MANUAL_LINK_PRODUCER`, else `'machine'`)
  + `confidence` (from the `link` content, NULL for manual). `link.content`
  extends to `{src, relation, dst, confidence?}`; `confidence` lives in the
  signed content, NEVER in `ModelMeta`. The recall proximity boost now gates on
  `origin='manual' OR confidence ≥ TRUST_MIN`, and auto-seeds from the top
  `GRAPH_REINFORCE_TOPK` fused hits (not just top-1).
- **Evolve runtime** (`src/evolve.rs`, `src/log.rs`) — a persistent
  `evolve_cursor` (progress state, NOT a fold); `evolve_once()` runs the full
  tick (recall → Pass A → resolve → augment → Pass B → emit via `append` →
  advance cursor), idempotent (skip active edge-keys, reuse resolved entities);
  a hard off-switch (`config` `evolve_enabled=false`, honored before any model
  call); a pure `debounce_due` scheduler decision; an `EvolveStatus`
  observability surface (queue depth, last tick, error counts, enabled).

#### Tests

- **Hermetic suite (CI, `ScriptedReasoner` + `MockEmbedder`)** — reasoner
  determinism + schema shape (`tests/reason.rs`); resolution thresholds + Pass A
  parse + Pass B critique/cardinality (`tests/extract.rs`); entity resolution
  merge/mint/adjudicate (`tests/entity_resolution.rs`); evolve `evolve_once`
  end-to-end + idempotency + cursor persistence + off-switch + provenance
  (`tests/evolve.rs`); entity fold + byte-identical-rebuild-with-entities +
  `edges` origin/confidence (`tests/graph.rs`); trust-gate boost + intra-result
  reinforcement (`tests/recall.rs`).
- **Live-Ollama behavioral gate** (`tests/live_ollama.rs`, `#[ignore]`, feature
  `ollama`) — asserts properties not bytes against the real model: a person →
  ≥1 entity; a relationship → a machine link; a contradiction → an invalidate;
  re-run is idempotent.
```

- [ ] **Step 5 — final gates: full hermetic suite + clippy + unsafe**

Run: `cargo test -p bossclaw-core` (hermetic green, all `#[ignore]` excluded)
Run: `cargo clippy -p bossclaw-core --all-targets -- -D warnings` (clean)
Run: `cargo clippy -p bossclaw-core --all-targets --features ollama -- -D warnings` (clean — the feature-gated `ollama.rs` also passes)
Expected: all green; `#![forbid(unsafe_code)]` already guarantees zero `unsafe`.

- [ ] **Step 6 — commit**

```bash
git add crates/bossclaw-core/tests/live_ollama.rs crates/bossclaw-core/CHANGELOG.md
git status -s
git commit -m "feat(bossclaw-core): live-Ollama behavioral gate + CHANGELOG + final gates (M4a T8)"
```

---

## Milestone 4a — Definition of Done
- [ ] `Reasoner` trait + `ScriptedReasoner` (deterministic, keyed by hash of `(system, prompt)`) + extraction/adjudication JSON schemas; `OllamaReasoner` behind the `ollama` feature (loopback-only, `format`=schema, `temperature 0`, non-loopback refused); `BossclawError::Reasoner` added; default build is pure (no network dep).
- [ ] `entity` Tier-B event + `entities` projection + `nodes(kind="entity")` fold; `EventLog::entity(...)` is a non-manual producer (empty `source_event_ids` rejected — F2 guard extended); byte-identical rebuild WITH entities (proven).
- [ ] Entity resolution: embed → dedicated entity index → `RESOLVE_HIGH`/`RESOLVE_LOW` → scripted mid-band adjudication; resolution searches ONLY entity-kind (separate index from recall).
- [ ] `extract.rs` Pass A (retrieval-augmented propose + parse, mandatory `supported_by`) + Pass B (span self-verify + cardinality-gated retraction confirmation against current edges); bounded by `MAX_REFLECT`; pure, tested with `ScriptedReasoner`.
- [ ] `edges.origin`/`edges.confidence` columns are PURE fold outputs (byte-identical rebuild holds); `confidence` in signed `link` content, never `ModelMeta`; the recall boost is trust-gated (`origin='manual' OR confidence ≥ TRUST_MIN`); auto-seed widened to intra-result reinforcement (`GRAPH_REINFORCE_TOPK`).
- [ ] `evolve_cursor` persistent progress (NOT a fold; survives reopen); `evolve_once()` end-to-end (recall → Pass A → resolve → augment → Pass B → emit via `append` → advance cursor); idempotent (active edge-keys skipped, resolved entities reused); off-switch (`config` `evolve_enabled=false`) honored before any model call; `EvolveStatus` observability.
- [ ] The evolve loop is NOT a privileged writer — every `entity`/`invalidate`/`link` goes through `EventLog::append`; each emitted event's `source_event_ids` = the cheat-sheet read-set (source + recalled), non-empty (F2 provenance, spec §16).
- [ ] recall EXCLUDES entity-kind (separate index, source never fed back as its own context); never-forget holds (a memory stays recallable after its edge is retired — carried M3 T-D still green).
- [ ] Live-Ollama gate (`#[ignore]`, feature `ollama`) asserts properties (person→entity, relationship→link, contradiction→invalidate, idempotent re-run); CHANGELOG M4a entry; whole hermetic suite green (temp homes only); `clippy --all-targets -D warnings` clean (default AND `ollama` feature); zero `unsafe`.

## Carried into later milestones
- **M4b — Summarizer:** `page` summary Tier-B events + the `pages` projection + `supersede`, reusing `evolve.rs`. Shape the `page` frontmatter Open-Knowledge-Format-compatible (spec §14).
- **Cloud-frontier escalation** (`CloudReasoner`, parent §5.8) for rare hard synthesis.
- **Proactive surfacing** ("you may want to revisit X").
- **14b/quality upgrade:** non-destructive (model_id-tagged); re-extraction over old events on upgrade is a later option.
- **2-hop neighborhood** in the cheat sheet (v1 = 1-hop, like M3).
- **The long-lived scheduler/driver** (running `last_tick`/error counters, idle-tick + resource-policy enforcement) → the desktop (M7); `evolve_status` v1 returns a pure-read snapshot.
- **The file actuator's use of the trust gate** (M6) — never reason over an untrusted-origin edge.
- **User DID threading** into `signer_did()` (v1 stamps a fixed engine DID) → M4b/M7.
- **AIR capability ontology** (strategic #7) — distinct from M4a's relation vocabulary.

---

## Self-Review

**Spec coverage (M4a design → task):**
- §3 closed-loop tick (recall → Pass A → resolve → augment → Pass B → emit → advance cursor) → T7 `evolve_once` ✓
- §4 `entity` event + `entities` projection ✓(T2); `edges` origin/confidence columns ✓(T6); `evolve_cursor` (NOT a fold) ✓(T7); byte-identical rebuild with all new folds ✓(T2 + T6 gate)
- §5 `Reasoner` trait + `ScriptedReasoner` + schemas ✓(T1); `OllamaReasoner` feature-gated loopback ✓(T1); untrusted-content fence (data channel framing in prompts) ✓(T4 `build_pass_a_prompt` doc)
- §6 retrieval-augmented extraction ✓(T4); extraction schema ✓(T1); entity resolution (embed/threshold/adjudicate) ✓(T3); typed relation vocabulary ✓(T4 `RELATION_VOCAB`); contradiction→invalidate via cardinality table ✓(T5 `RELATION_CARDINALITY_SINGLE`/`confirm_retractions`)
- §7 confidence in content + edge-trust gate ✓(T6); intra-result reinforcement ✓(T6 `GRAPH_REINFORCE_TOPK`)
- §8 evolve runtime: cursor ✓, `evolve_once` ✓, off-switch ✓, observability (`EvolveStatus`) ✓, single-writer respect ✓, debounce (pure `debounce_due`) ✓ — all T7. (Resource-policy ENFORCEMENT + the running scheduler are explicitly carried to M7; v1 ships the off-switch + the pure debounce decision + the status surface — flagged as a partial below.)
- §10 error handling: reasoner-error → skip/no-op ✓(T7 `evolve_once` Pass-A break); off-switch no-op ✓(T7); first-run no-Ollama degrade ✓ (the loop queues; `evolve_enabled`/`evolve_status` surface it — the app injects the reasoner, so "no Ollama" = the app simply doesn't run a tick)
- §11 named constants: `RESOLVE_HIGH`/`RESOLVE_LOW`/`MAX_REFLECT`/`TRUST_MIN`/`GRAPH_CONTEXT_K` ✓(T3/T4 `extract.rs`), `EVOLVE_BATCH`/`EVOLVE_DEBOUNCE_MS` ✓(T7 `extract.rs`, re-exported from `evolve.rs`), `GRAPH_REINFORCE_TOPK` ✓(T6 `recall.rs`) — each with a sourced-comment rationale; no magic numbers
- §12 testing: byte-identical rebuild with entities ✓(T2), idempotency ✓(T7), entity resolution thresholds ✓(T3), contradiction→one invalidate ✓ (T5 pure + T7 emit + T8 live), trust gate ✓(T6), provenance read-set ✓(T7), hermetic temp homes + clippy + zero unsafe ✓(T8); live gate properties ✓(T8)
- §13 build sequence (8 tasks) → T1..T8 one-to-one ✓
- §16 provenance contracts: non-manual producer passes real read-set, non-empty ✓(T2/T6 `entity`/`link_machine` reject empty; T7 builds the read-set); `model_id` is provenance not trust (origin derives from it, but the TRUST gate uses confidence/origin not the literal string) ✓
- §17 deviations (new `entity` type; `OllamaReasoner` in-core behind a feature; `evolve` depends on recall+embed; `confidence` in content) — all realized as designed ✓

**Gaps / partials (honest):**
1. **Resource policy (§8 "idle/charging-aware throttle + per-tick rate-limit") is NOT enforced in T7** — only the off-switch, the pure `debounce_due`, the `EVOLVE_BATCH` cap, and the `EvolveStatus` surface ship. The running scheduler/driver (idle-tick, throttle enforcement, live `last_tick`/error counters) is explicitly carried to M7's desktop loop driver. The spec lists the resource policy under M4a §8; this plan ships the *mechanism hooks* (batch cap, debounce decision, status) and defers the *policy enforcement* to the long-lived driver. **Flag for review:** is the deferral acceptable, or must T7 ship an in-crate throttle?
2. **Pass B is PURE-only in T5/T7** (span-substring verify + cardinality-gated retraction confirmation). The spec §3 step 5 also describes Pass B *re-scoring confidence* via a second model call ("finalize confidence"). This plan keeps the confidence from Pass A and does the *verification* deterministically; the model-driven re-score is implied by `MAX_REFLECT=2` + `PASS_B_SYSTEM` (defined, reserved) but not wired into `evolve_once` (which calls the pure `critique`). **Flag for review:** does M4a require the second model call for confidence re-scoring, or is deterministic Pass-B verification (the safer, testable choice) sufficient for v1? `PASS_B_SYSTEM` exists so wiring it is a small follow-up if required.
3. **`MAX_REFLECT` is asserted (`debug_assert!`) but not a live loop bound** — because Pass A is one call and Pass B is pure, the per-memory model-call count is 1 (+ mid-band adjudications), well under `MAX_REFLECT=2`. If the §3 model-driven Pass B (gap #2) is required, `MAX_REFLECT` becomes the real cap on the propose↔critique cycle.

**Placeholder scan:** no `TODO`/`TBD`/`unimplemented!`; every code step shows complete code (new functions in full; edits as targeted snippets, as the M3 plan's Rev-2 fixes do). Cross-task references are all defined before use: `extract.rs` consts/types (T3/T4/T5) before `evolve_once` (T7); `entity`/`derive_entity_vector`/`entity_search`/`resolve_mention` (T2/T3) before T7; `link_machine` + origin/confidence (T6) before T7 emits them; `ScriptedReasoner`/schemas (T1) before every reasoner test.

**Type-consistency check:**
- `Edge` gains `origin: String` + `confidence: Option<f32>` consistently across `graph.rs` (struct + `fold_edges`), the `edges` DDL (`origin TEXT NOT NULL DEFAULT 'manual'`, `confidence REAL`), `query_edges` row mapper (`r.get(9)`/`r.get(10)`), and all three edge `SELECT` column lists (`all_edges`/`neighbors`/`as_of`) — the M3 plan's single-sourced `query_edges` mapping is preserved. **All existing `Edge { … }` literals (in `tests/graph.rs` + `graph.rs` unit tests) must add the two fields** — called out in T6 Step 3.
- `parse_link_content` returns a 4-tuple `(String,String,String,Option<f32>)` everywhere; `fold_edges`'s destructure updated (T6); the F4 malformed filter still type-checks (`.is_none()`).
- `ResolveDecision` (T3) is the single return type of `resolve_decision` (pure) and `resolve_mention` (glue collapses `Adjudicate`→`Merge`/`Mint`); `Proposals`/`Proposed*` (T4) are shared by `propose`/`critique`/`confirm_retractions` (T4/T5) and consumed by `evolve_once` (T7).
- `Reasoner` is `&dyn` everywhere (`complete_json(&self, &str, &str, &Value) -> Result<Value, BossclawError>` + `model_id(&self)->&str`), matching the spec §5 signature exactly; `ScriptedReasoner`/`OllamaReasoner` both impl it.
- `EvolveReport`/`EvolveStatus` (T7) fields match every test assertion (`entities_minted`, `links_emitted`, `invalidates_emitted`, `memories_processed`, `skipped_disabled`; `queue_depth`, `last_tick_ms`, `error_count`, `last_error`, `enabled`).
- `entity()` returns `entity:<id>` (namespaced) — every caller (T2/T3/T7 tests + `evolve_once`) treats the return as a node id, never the bare event id.
- New consts are all `pub` with the exact names the spec §11 lists (`EVOLVE_DEBOUNCE` is spec-named in ms; realized as `EVOLVE_DEBOUNCE_MS: u128` — a naming refinement flagged here for the reviewer; value `2000` unchanged).

**No magic numbers:** every threshold/weight/cap is a named, sourced `pub const` (`RESOLVE_HIGH`, `RESOLVE_LOW`, `MAX_REFLECT`, `TRUST_MIN`, `GRAPH_CONTEXT_K`, `EVOLVE_BATCH`, `EVOLVE_DEBOUNCE_MS`, `GRAPH_REINFORCE_TOPK`); `MANUAL_LINK_PRODUCER` (M3) names the producer string; `OLLAMA_LOOPBACK_URL`/`OLLAMA_TIMEOUT_SECS` name the I/O literals.

**Hermeticity:** every CI test uses `MockEmbedder` + `ScriptedReasoner` + `tempfile` temp homes; no network, no real model. The real-model test is `#[cfg(feature="ollama")]` + `#[ignore]` — excluded from `cargo test -p bossclaw-core` twice over (feature off in CI; ignored even when on).
