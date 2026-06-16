# bossclaw-core — Milestone 3 (Graph) Design

**Status:** Approved (brainstorm 2026-06-15, superpowers:brainstorming) · addendum to the parent design
**Author:** Peter + Claude
**Repo:** `~/air-note` (canonical) · crate `crates/bossclaw-core`
**Addendum to:** `docs/superpowers/specs/2026-06-15-bossclaw-core-design.md` §5.6 (bi-temporal graph) + §12.3 (milestone 3). This file records the M3-specific decisions the parent left open; the parent remains the canonical overall design.

## Revision log
- **Rev 2 (2026-06-15):** folded an independent two-reviewer pass (critic + security, both SHIP-WITH-FIXES). Added the provenance/integrity contracts in §12 (manual≠user-authored; `signed_by_did` is currently unverified; the `[src,dst]` `source_event_ids` default is gated to the manual producer; the boost has no edge-trust gate yet). No design-direction change — these harden the M3→M4 seam. The plan carries the matching code/test fixes (F1–F4, T-A–T-E).
- **Rev 1 (2026-06-15):** initial M3 design from the brainstorm. Primary fork resolved by Peter — **Option 1: live graph-proximity boost over a general node graph.** Secondary forks (edge identity, `invalidate` targeting, `as_of` axes, boost mechanics, manual-link Tier-B handling) decided here and approved.

---

## 1. Scope & the resolved fork

M3 is the **connect-the-dots layer**: draw typed connections (`link`) between memories, retire a connection when it stops being true (`invalidate`) **without deleting it**, remember **when** each connection was true (bi-temporal), and feed those connections back into recall so related memories rank higher.

**Resolved fork — Option 1 (live boost, general nodes):**
- Nodes are **opaque string ids**. In v1 a `link` connects one **memory event-id** to another, so the graph-proximity recall boost is **live and tested on real links in M3** — not dormant.
- M4's entity-extractor later adds **entity** nodes (e.g. `entity:kenny`) and episodic↔entity links to the **same `nodes`/`edges` tables — no schema change.

**In scope (M3):** `link`/`invalidate` event conventions + append helpers; the `nodes`/`edges` Tier-A fold; bi-temporal `neighbors`/`as_of`/backlinks; the live graph-proximity boost wired into `EventLog::recall`; hermetic tests + gates.

**NOT in M3 (parent §5.6/§5.9):** LLM **extraction** that auto-creates links (that is non-deterministic → the evolve loop, M4). M3 builds the graph *machinery* and proves it with hand/test-asserted links.

---

## 2. Decisions locked in this brainstorm

1. **General node model.** `node_id` is an opaque string; v1 endpoints are memory/page event-ids (`kind = "memory"`). Entity nodes (`kind = "entity"`) are an M4 additive, same tables.
2. **`link`/`invalidate` are Tier-B** (parent §4/§5.2) — they carry mandatory non-empty `source_event_ids`, preserving the §5.11 taint-lineage walk. A hand-asserted M3 link uses `model_meta = { model_id: "manual", prompt_hash: "", source_event_ids: [...] }` — per `event.rs`, append mandates only `source_event_ids` non-empty, so an empty `prompt_hash` is valid for a promptless manual link (Task 1 confirms append accepts it); M4's reasoner swaps `"manual"` for its real id. Memory↔memory links self-justify with the two memory ids.
3. **Edge identity = the `link` event's ULID** — unique and **rebuild-stable**. (Note: `Ulid::new()` is monotonic *across* milliseconds but random-tailed *within* one, so `ORDER BY edge_id` is NOT guaranteed creation-order for two links in the same millisecond. The byte-identical-**on-rebuild** guarantee holds regardless, because rebuild reads the same stored ULIDs; deterministic *creation* order is the `seq`-ordered fold's job, not `edge_id`'s.)
4. **`invalidate` targets the edge-key `(src, relation, dst)`** (Graphiti "this fact is no longer true"), closing **all currently-active** assertions for that key. Re-linking opens a fresh assertion row → full validity history.
5. **`nodes`/`edges` are Tier-A**, a deterministic fold over `link`/`invalidate` events `ORDER BY seq ASC`, **byte-identical on rebuild** (unlike M2's relaxed in-memory hnsw — these are persisted projections, like `vectors`).
6. **Bi-temporal `as_of` exposes both clocks** via one struct (`valid_time` = true-in-world; `known_as_of` = learned-by) — both optional `WHERE`-clause filters, cheap because the columns already exist.
7. **Live recall boost** = one more multiplier in the existing `recency`/`pin` family, **auto-seeded** from the top fused hit (explicit seeds optional), **1 hop** (const allows 2), **current edges only**, capped **below pin and ≈ recency**.

---

## 3. The two new events

Both are **Tier-B** events appended through the existing **single-writer signed `EventLog::append`** — no new write path, no new signing path.

### `link`
- `event_type = "link"`.
- `content = { "src": String, "relation": String, "dst": String }`.
- Optional world-time via the existing `Event.valid_time` (RFC 3339).
- `model_meta.source_event_ids` non-empty (enforced at append). For memory↔memory links the default is `[src, dst]`.

### `invalidate`
- `event_type = "invalidate"`.
- `content = { "src": String, "relation": String, "dst": String }` — the **edge-key** to retire.
- `model_meta.source_event_ids` non-empty (what justified the retraction).
- Optional `valid_time` = when the fact **stopped** being true in the world.

### Append helpers (on `EventLog`)
```rust
/// Append a signed Tier-B `link` event (single-writer). `source_event_ids`
/// defaults to [src, dst] when both are memory/page event-ids and the caller
/// passes none; otherwise the caller supplies them (non-empty, enforced).
pub fn link(&self, src: &str, relation: &str, dst: &str,
            valid_time: Option<&str>, source_event_ids: &[String]) -> Result<String, BossclawError>;

/// Append a signed Tier-B `invalidate` event closing the active assertion(s)
/// for (src, relation, dst).
pub fn invalidate(&self, src: &str, relation: &str, dst: &str,
                  valid_time: Option<&str>, source_event_ids: &[String]) -> Result<String, BossclawError>;
```

---

## 4. The fold → `nodes` / `edges` (Tier-A, byte-identical)

Replay every `link`/`invalidate` event in `seq` order → today's graph. Lose the tables → rebuild exactly from the encrypted log. Folded in the same open/rebuild path M2 already runs (`rebuild_graph` joins `rebuild_indexes`).

### `edges` — one row per `link` event
```sql
CREATE TABLE IF NOT EXISTS edges (
  edge_id        TEXT PRIMARY KEY,   -- the link event's ULID (deterministic)
  src            TEXT NOT NULL,
  relation       TEXT NOT NULL,
  dst            TEXT NOT NULL,
  valid_from     TEXT NOT NULL,      -- link.valid_time, else link.ts (world-clock start)
  valid_to       TEXT,               -- NULL until invalidated (world-clock end)
  ingested_at    TEXT NOT NULL,      -- link.ts (learned-clock)
  invalidated_at TEXT,               -- NULL until invalidated (learned-clock end)
  invalidated_by TEXT                -- NULL until invalidated (the invalidate event id)
);
```
- **Fold rule:** apply events `ORDER BY seq ASC`. A `link` inserts a row. An `invalidate` sets `valid_to` (= invalidate.valid_time, else invalidate.ts), `invalidated_at` (= invalidate.ts), and `invalidated_by` on **every active** row matching `(src, relation, dst)` where `invalidated_at IS NULL`.
- **Current graph** = `invalidated_at IS NULL`.
- **Determinism:** all values are pure functions of event fields; PK is the link ULID; query/output ordering is `ORDER BY edge_id ASC`. Rebuild is byte-identical (the M3 §9 gate).

### `nodes` — distinct edge endpoints
```sql
CREATE TABLE IF NOT EXISTS nodes (
  node_id TEXT PRIMARY KEY,
  kind    TEXT NOT NULL              -- "memory" if node_id resolves to a memory/page event, else "external"
);
```
A memory with no links is simply not a node yet — correct. M4 adds `kind = "entity"`.

---

## 5. Bi-temporal queries

```rust
pub struct Edge { pub edge_id: String, pub src: String, pub relation: String, pub dst: String,
                  pub valid_from: String, pub valid_to: Option<String>,
                  pub ingested_at: String, pub invalidated_at: Option<String> }

/// Two-axis time-travel. Both optional; empty == "current".
pub struct AsOf {
    pub valid_time:  Option<String>, // what was TRUE in the world at t: valid_from <= t < (valid_to or +inf)
    pub known_as_of: Option<String>, // what we had LEARNED by t: ingested_at <= t AND (invalidated_at IS NULL OR invalidated_at > t)
}

impl EventLog {
    /// Current edges touching `node` (either direction). Backlinks = the subset where dst == node.
    pub fn neighbors(&self, node: &str) -> Result<Vec<Edge>, BossclawError>;
    /// Time-travelled edges touching `node` under `as_of` (filters layered on the same projection).
    pub fn as_of(&self, node: &str, as_of: &AsOf) -> Result<Vec<Edge>, BossclawError>;
}
```
- `neighbors` returns edges in **both** directions (each `Edge` carries `src`/`relation`/`dst`, so direction is explicit); **backlinks** = `dst == node`.
- `as_of` with `valid_time` answers "who did Kenny work for in 2021"; with `known_as_of` answers "what did I believe last Tuesday"; both set = full bi-temporal point. Output `ORDER BY edge_id ASC`.

---

## 6. The live recall boost

Slots into `EventLog::recall` as **one more multiplier** beside `recency`/`pin`. Adjacency for proximity is the **undirected** union of current edges (a connection means "related" both ways), even though edges are stored directed.

```rust
// RecallOptions gains:
pub graph_seeds: Vec<String>, // explicit proximity seeds; empty => auto-seed (below)
```

- **Seeds:** if `graph_seeds` non-empty, use them; **else auto-seed** from the top `GRAPH_AUTO_SEED_TOPK` fused hits' own node-ids (one neighbor lookup → the boost fires with zero caller input).
- **Distance:** `hops(candidate)` = shortest hop (≤ `GRAPH_MAX_HOPS`) to any seed over **current** edges (`invalidated_at IS NULL`). Out-of-range → ×1 (no boost). Retired facts never boost recall.
- **Boost:** `fused *= 1.0 + GRAPH_WEIGHT * GRAPH_HOP_DECAY.powi(hops as i32 - 1)`. Multiplicative, same family as recency/pin.
- **Degrade:** any graph lookup error → skip the boost (recall is never broken by the graph), mirroring M2's keyword-only degrade.

### Named constants (in `recall.rs`, sourced comments — no magic numbers)
| Const | Value | Rationale |
|---|---|---|
| `GRAPH_WEIGHT` | `0.4` | Max 1-hop boost +40% — a *tilt*, below recency's +50% and far below pin ×2. |
| `GRAPH_HOP_DECAY` | `0.5` | Each extra hop halves the boost. |
| `GRAPH_MAX_HOPS` | `1` | v1 = direct neighbors; the formula supports 2 (const-gated, carried). |
| `GRAPH_AUTO_SEED_TOPK` | `1` | Auto-seed from the single strongest fused hit. |

---

## 7. Components & file layout
- **`src/graph.rs` (new):** pure types (`Edge`, `Node`, `AsOf`) + pure fold/adjacency/proximity helpers (mirrors the pure split in `recall.rs`/`keyword.rs`). No SQL, no `Store`.
- **`src/log.rs` (extend):** the SQL-touching `EventLog` methods — `link`, `invalidate`, `rebuild_graph`, `neighbors`, `as_of`, and the proximity-boost wiring inside `recall`. Same ownership pattern keyword/recall already use (the `pub(crate)` conn stays inside the crate).
- **`src/recall.rs` (extend):** the four named graph consts + `RecallOptions.graph_seeds`.
- **`tests/graph.rs` (new):** fold determinism, `as_of`, invalidate-not-delete, re-link, neighbors/backlinks. **`tests/recall.rs` (extend):** the proximity boost.

---

## 8. Error handling
- Typed `BossclawError` (reuse `thiserror`); no panics in library code.
- `link`/`invalidate` propagate append errors (signing/chain/Tier-B-empty-source rejection).
- `rebuild_graph` failure surfaces as an error on open (the graph is a derived projection; the log remains authoritative and re-foldable).
- Recall proximity-boost failure **degrades to no-boost** (best-effort), never failing the recall call (§6).

---

## 9. Testing strategy (the gates)
- **Tier-A byte-identical rebuild:** fold N `link`/`invalidate` events, snapshot `nodes`/`edges`, `rebuild_graph`, assert identical (the persisted-Tier-A standard, parent §4/§11 — stronger than M2's hnsw relaxation).
- **Invalidate-closes-not-deletes:** after `invalidate`, the row persists with `invalidated_at` set; `neighbors` excludes it; `as_of(valid_time within the active window)` includes it.
- **Re-link opens a new interval:** link → invalidate → link again ⇒ two rows; history queryable; current = the latest.
- **Bi-temporal `as_of`:** both clocks independently and together.
- **neighbors / backlinks** correctness (direction).
- **Proximity boost:** a 1-hop neighbor of the seed outranks an otherwise-equal unlinked memory; a **retired** edge yields no boost; **auto-seed** fires from the top hit with no explicit seeds.
- **Hermetic:** temp homes, `MockEmbedder`; `clippy -D warnings`; zero `unsafe`.

---

## 10. Build sequence (TDD milestones; each demoable)
1. `link`/`invalidate` event conventions + content schema + `EventLog::link`/`invalidate` helpers (Tier-B, source_event_ids default).
2. `edges` projection + the deterministic fold (`rebuild_graph`) + invalidate-closes semantics + the byte-identical rebuild test.
3. `nodes` projection + `neighbors` + backlinks.
4. Bi-temporal `as_of` (both axes).
5. The live graph-proximity boost in `recall` (auto-seed + explicit seeds; 1-hop; current-edges; consts).
6. CHANGELOG + final gates (hermetic suite green, clippy clean, zero unsafe).

---

## 11. Deferred / carried
- **Entity nodes + LLM extraction** → M4 (evolve), same tables.
- **Intra-result reinforcement seeding** (boost candidates that are neighbors of *other* strong candidates, not just the top hit) — additive, no schema impact.
- **2-hop proximity** — formula + const ready; v1 caps at 1.
- **Persisted graph sidecar** — rebuild-on-open is fine (the graph is tiny vs vectors); persist only if startup cost ever shows.
- **Desktop graph view / "what's connected to this"** → M7.
- **Directed-relation semantics in the boost** (v1 treats adjacency as undirected for relatedness).

## 12. Honesty lines
- The **end-user-visible** "feel it" dogfooding lands with the M7 desktop. The boost is nonetheless **functional and tested on real links in M3** — the live path, not dormant. **But auto-seed fires on the top-1 hit only:** if the single strongest hit is unlinked, no boost fires (intra-result reinforcement is deferred, §11). M3 proves the *mechanism*; a meaningful hit-rate on an organically-linked corpus arrives with M4's auto-linking.
- M3 links are **hand/test-asserted**; automatic, intelligent linking is M4. The graph's *correctness* (fold, bi-temporality, retraction) is fully proven in M3; its *population at scale* is M4's job.

### 12.1 Provenance / integrity contracts (Rev 2 — second-opinion review)
- **`model_id="manual"` is engine/test-asserted, NOT user-authored.** The future taint walk (§5.11) derives trust from `source_event_ids` lineage + the (future) user-DID signer, **never** from the literal string `"manual"`. No milestone may establish "manual ⇒ clean."
- **The `[src,dst]` `source_event_ids` default is manual-only.** A non-manual producer (M4's reasoner) MUST pass its real read-set; the helper rejects a non-manual empty source set. Defaulting there would launder taint past the §5.11 fail-closed lineage walk.
- **`signed_by_did` is currently UNVERIFIED.** `verify_chain` checks only the engine signing key; the parent design §5.2's "resolve pubkey from `signed_by_did`" step is **aspirational, not implemented**. M3 stamps a fixed `did:wba:bossclaw-engine` = engine-asserted, NOT user-owned. Before any user-facing ownership claim (M7), verify MUST resolve DID→pubkey and reject mismatches. *(Parent §5.2 wording to be reconciled separately.)*
- **The proximity boost has no edge-trust gate.** Fine for M3 (hand-asserted links). Once links can be machine/ingest-derived (M4), an untrusted-origin edge must not boost a candidate into the actuator's reasoning set (§5.11 taint work).
