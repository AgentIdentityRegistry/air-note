# bossclaw-core — Milestone 3 (Graph) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax. Build TDD: failing test first, then implementation.

**Goal:** Give the M1/M2 memory engine a **connect-the-dots layer** — `link`/`invalidate` events, a deterministic bi-temporal `nodes`/`edges` fold over them, `neighbors`/`as_of` queries, and a **live graph-proximity boost** wired into hybrid recall.

**Architecture:** `link`/`invalidate` are **Tier-B signed events** appended through the existing single-writer `EventLog::append`. The `edges`/`nodes` tables are **persisted Tier-A projections** (like `vectors`/`fts`), repopulated by `rebuild_graph` as a deterministic fold over the events `ORDER BY seq ASC` — byte-identical on rebuild. Queries (`neighbors`, `as_of`) read the `edges` table directly. The recall boost reads the current edges to nudge memories that are graph-near the top hit. All timestamps are normalized to fixed-width UTC so SQL `TEXT` comparison equals chronological order.

**Tech Stack:** Rust 2021 · `rusqlite` (`bundled-sqlcipher`) · `chrono` · `ulid` · `serde_json`. Builds on M1's `EventLog`/`Event`/`append` and M2's `recall`/`RecallOptions`.

**Spec:** `docs/superpowers/specs/2026-06-15-bossclaw-core-m3-graph-design.md` (addendum to `...-bossclaw-core-design.md` §5.6/§12.3). Implements M3 §3 events, §4 fold, §5 queries, §6 boost, §9 tests.

---

## Design decisions (locked in the spec; do not re-derive)
1. **General nodes:** `node_id` is an opaque string; v1 links connect memory event-ids. `kind = "memory"` if the id resolves to a `memory`/`page` event, else `"external"`.
2. **`link`/`invalidate` are Tier-B:** `model_meta = { model_id: "manual", prompt_hash: "", source_event_ids }`. `append` only requires `source_event_ids` non-empty (confirmed in `log.rs::append`), so empty `prompt_hash` is valid.
3. **Edge identity = the `link` event's ULID** (`Event.id`). `invalidate` targets the edge-key `(src, relation, dst)`, closing **all currently-active** assertions for that key.
4. **`nodes`/`edges` are persisted Tier-A**, repopulated by `rebuild_graph` (wipe + refold `ORDER BY seq ASC`). Byte-identical on rebuild. **Lifecycle:** after appending `link`/`invalidate` events, call `rebuild_graph()` to refresh queries — the same "rebuild after append" lifecycle M2 documents for `rebuild_indexes`.
5. **Two-axis `as_of`** via `AsOf { valid_time, known_as_of }`; both `None` == current (== `neighbors`).
6. **Live recall boost:** one more multiplier in the recency/pin family; **auto-seeded** from the top fused hit (explicit `graph_seeds` override); **1 hop** (`GRAPH_MAX_HOPS`), **current edges only**.

## File structure
| File | Responsibility |
|---|---|
| `crates/bossclaw-core/src/graph.rs` (**new**) | Pure types `Edge`/`Node`/`AsOf` + pure `fold_edges`, `parse_link_content`, `normalize_ts`; `MANUAL_LINK_PRODUCER` const. No SQL, no `Store`. |
| `crates/bossclaw-core/src/lib.rs` (modify) | `pub mod graph;` + `pub use graph::{AsOf, Edge, Node};` + M3 line in the crate doc. |
| `crates/bossclaw-core/src/log.rs` (modify) | `edges`/`nodes` DDL in `open`; `link`/`invalidate` append helpers; `rebuild_graph`; `all_edges`/`all_nodes`; `neighbors`; `as_of`; `current_neighbors_with_hops`; the recall boost wiring. |
| `crates/bossclaw-core/src/recall.rs` (modify) | Four named graph consts; `RecallOptions.graph_seeds`. |
| `crates/bossclaw-core/tests/graph.rs` (**new**) | Fold determinism, invalidate-not-delete, re-link, `neighbors`/backlinks, `as_of` both clocks. |
| `crates/bossclaw-core/tests/recall.rs` (modify) | Graph-proximity boost tests; fix the existing `RecallOptions` literal. |
| `crates/bossclaw-core/CHANGELOG.md` (modify) | M3 entry. |

`as_of` valid_time / known_as_of clock filters, the fold, and the boost are the load-bearing pieces — everything else is wiring.

---

## Task 1: `link`/`invalidate` events + `edges`/`nodes` schema

**Files:**
- Create: `crates/bossclaw-core/src/graph.rs` (types + `MANUAL_LINK_PRODUCER` only this task)
- Modify: `crates/bossclaw-core/src/lib.rs`, `crates/bossclaw-core/src/log.rs`
- Test: `crates/bossclaw-core/tests/graph.rs` (new)

- [ ] **Step 1 — write the failing test** (`tests/graph.rs`):

```rust
use bossclaw_core::event::Event;
use bossclaw_core::log::EventLog;
use ed25519_dalek::SigningKey;
use serde_json::json;

const DEK: [u8; 32] = [42u8; 32];
const KEY_BYTES: [u8; 32] = [7u8; 32];
const DID: &str = "did:wba:AIR-TEST";

fn open_log(dir: &std::path::Path) -> EventLog {
    let key = SigningKey::from_bytes(&KEY_BYTES);
    EventLog::open(&dir.join("m.db"), &DEK, key).unwrap()
}

fn mk_memory(text: &str) -> Event {
    Event {
        id: String::new(), ts: String::new(), valid_time: None,
        event_type: "memory".to_string(), content: json!({ "text": text }),
        model_meta: None, prev_hash: String::new(), hash: None,
        signed_by_did: DID.to_string(), signature: None,
    }
}

#[test]
fn link_appends_tier_b_event_with_source_ids() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let a = log.append(mk_memory("kenny")).unwrap();
    let b = log.append(mk_memory("acme")).unwrap();

    // Empty source_event_ids → helper defaults to [src, dst] (non-empty, so append accepts it).
    let edge_event_id = log.link(&a, "works_at", &b, None, &[]).unwrap();

    let ev = log.stream_all().unwrap().into_iter().find(|e| e.id == edge_event_id).unwrap();
    assert_eq!(ev.event_type, "link");
    assert_eq!(ev.content["src"], json!(a));
    assert_eq!(ev.content["relation"], json!("works_at"));
    assert_eq!(ev.content["dst"], json!(b));
    let meta = ev.model_meta.expect("link is Tier-B");
    assert_eq!(meta.model_id, "manual");
    assert_eq!(meta.source_event_ids, vec![a.clone(), b.clone()]);
}

#[test]
fn invalidate_appends_event_with_edge_key() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let a = log.append(mk_memory("kenny")).unwrap();
    let b = log.append(mk_memory("acme")).unwrap();
    log.link(&a, "works_at", &b, None, &[]).unwrap();

    let inv_id = log.invalidate(&a, "works_at", &b, None, &[a.clone()]).unwrap();
    let ev = log.stream_all().unwrap().into_iter().find(|e| e.id == inv_id).unwrap();
    assert_eq!(ev.event_type, "invalidate");
    assert_eq!(ev.content["src"], json!(a));
    assert_eq!(ev.model_meta.unwrap().source_event_ids, vec![a]);
}
```

- [ ] **Step 2 — run, verify it fails**

Run: `cd ~/air-note && cargo test -p bossclaw-core --test graph -- link_appends invalidate_appends`
Expected: FAIL — `no method named link` / `no method named invalidate`.

- [ ] **Step 3 — create `src/graph.rs`** (types + producer const only; fold helpers land in Task 2):

```rust
//! Pure bi-temporal graph types and fold helpers (spec §5.6 / M3 §4-5).
//!
//! This module is deliberately PURE — no SQL, no I/O, no `Store`. It mirrors the
//! split used by [`crate::recall`] and [`crate::keyword`]: the database work
//! (folding events into tables, running graph queries) lives on
//! [`crate::log::EventLog`]; everything here is data types and pure helpers.

/// `model_id` recorded on a hand/test-asserted (non-LLM) `link`/`invalidate`
/// event. M4's reasoner replaces this with its real model id. Kept as a named
/// constant so the convention is single-sourced (no magic string).
pub const MANUAL_LINK_PRODUCER: &str = "manual";

/// A folded graph edge: one assertion from a `link` event, possibly closed by a
/// later `invalidate`. Two clocks (spec §5): `valid_*` is world-time ("true in
/// the world"); `ingested_at`/`invalidated_at` is learned-time ("when we knew").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    /// The originating `link` event's ULID — the edge's stable identity.
    pub edge_id: String,
    /// Source node id.
    pub src: String,
    /// Relation label (e.g. `"works_at"`).
    pub relation: String,
    /// Destination node id.
    pub dst: String,
    /// World-clock start: the `link`'s `valid_time`, else its ingestion `ts`.
    pub valid_from: String,
    /// World-clock end: `None` until invalidated.
    pub valid_to: Option<String>,
    /// Learned-clock start: the `link` event's `ts`.
    pub ingested_at: String,
    /// Learned-clock end: `None` until invalidated.
    pub invalidated_at: Option<String>,
    /// The `invalidate` event id that closed this edge, if any.
    pub invalidated_by: Option<String>,
}

impl Edge {
    /// True when this edge has not been invalidated (part of the current graph).
    pub fn is_current(&self) -> bool {
        self.invalidated_at.is_none()
    }
}

/// A graph node = a distinct edge endpoint. `kind` is `"memory"` if the id
/// resolves to a `memory`/`page` event, else `"external"` (M4 adds `"entity"`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// The node's opaque string id (a memory event id in v1).
    pub node_id: String,
    /// Node kind: `"memory"` or `"external"` in v1.
    pub kind: String,
}

/// Two-axis bi-temporal point for [`crate::log::EventLog::as_of`]. Both optional;
/// an all-`None` `AsOf` means "current" (identical to `neighbors`). (spec §5)
#[derive(Debug, Clone, Default)]
pub struct AsOf {
    /// World-clock: keep edges valid in the world at this RFC 3339 time
    /// (`valid_from <= t` AND (`valid_to` is null OR `t < valid_to`)).
    pub valid_time: Option<String>,
    /// Learned-clock: keep edges we knew at this RFC 3339 time
    /// (`ingested_at <= t` AND (`invalidated_at` is null OR `t < invalidated_at`)).
    pub known_as_of: Option<String>,
}

/// Normalize an RFC 3339 timestamp to fixed-width UTC microseconds + `Z`, so
/// lexicographic (SQL `TEXT`) comparison equals chronological comparison
/// regardless of the source offset or sub-second precision. Returns the input
/// unchanged if it cannot be parsed (best-effort — an unparseable world-clock
/// value degrades to raw-string compare rather than failing the fold).
pub fn normalize_ts(ts: &str) -> String {
    match chrono::DateTime::parse_from_rfc3339(ts) {
        Ok(dt) => dt
            .with_timezone(&chrono::Utc)
            .format("%Y-%m-%dT%H:%M:%S%.6fZ")
            .to_string(),
        Err(_) => ts.to_string(),
    }
}
```

- [ ] **Step 4 — register the module** in `src/lib.rs`. After `pub mod event;` add `pub mod graph;` (keep alphabetical: it goes between `event` and `highwater`). Add to the re-export block: `pub use graph::{AsOf, Edge, Node};`. Extend the crate doc comment with: `//! Milestone 3 (Graph): bi-temporal link/invalidate fold + graph-proximity recall boost.`

- [ ] **Step 5 — add the `edges`/`nodes` DDL** in `EventLog::open` (`src/log.rs`), right after the `fts_map` `CREATE TABLE` block and before the `PRAGMA temp_store` line:

```rust
        // Bi-temporal graph projection (Tier-A; spec §5.6). One `edges` row per
        // `link` event (PK = the link's ULID); `invalidate` closes rows by
        // setting valid_to/invalidated_at. `nodes` = distinct endpoints. Both are
        // a deterministic fold over link/invalidate events, rebuilt by
        // `rebuild_graph`. Timestamps are stored normalized (fixed-width UTC) so
        // SQL TEXT comparison equals chronological comparison.
        store.exec(
            "CREATE TABLE IF NOT EXISTS edges (
                edge_id        TEXT PRIMARY KEY,
                src            TEXT NOT NULL,
                relation       TEXT NOT NULL,
                dst            TEXT NOT NULL,
                valid_from     TEXT NOT NULL,
                valid_to       TEXT,
                ingested_at    TEXT NOT NULL,
                invalidated_at TEXT,
                invalidated_by TEXT
            )",
        )?;
        store.exec(
            "CREATE TABLE IF NOT EXISTS nodes (
                node_id TEXT PRIMARY KEY,
                kind    TEXT NOT NULL
            )",
        )?;
```

- [ ] **Step 6 — add the `link`/`invalidate` helpers** to `impl EventLog` (`src/log.rs`). First extend the import: change `use crate::event::{compute_hash, Event};` to `use crate::event::{compute_hash, Event, ModelMeta};`, and add `use crate::graph::MANUAL_LINK_PRODUCER;` to the `use crate::graph::...` group (add the group if absent). Then:

```rust
    /// Append a signed Tier-B `link` event connecting `src` —`relation`→ `dst`.
    ///
    /// `valid_time` (optional, RFC 3339) is the world-clock start; absent means
    /// "valid from when we learned it" (the event's ingestion `ts`). If
    /// `source_event_ids` is empty it defaults to `[src, dst]` so the Tier-B
    /// non-empty-provenance rule is satisfied honestly (the two endpoints justify
    /// the link). Returns the new event id (which is also the edge's identity).
    ///
    /// The `edges` table is NOT updated here — call [`EventLog::rebuild_graph`]
    /// to refresh `neighbors`/`as_of`/the recall boost (same "rebuild after
    /// append" lifecycle as [`EventLog::rebuild_indexes`]).
    pub fn link(
        &self,
        src: &str,
        relation: &str,
        dst: &str,
        valid_time: Option<&str>,
        source_event_ids: &[String],
    ) -> Result<String, BossclawError> {
        self.append_graph_event("link", src, relation, dst, valid_time, source_event_ids)
    }

    /// Append a signed Tier-B `invalidate` event retiring the edge-key
    /// `(src, relation, dst)`. `valid_time` (optional) is when the fact stopped
    /// being true in the world. Same `source_event_ids` defaulting and lifecycle
    /// as [`EventLog::link`].
    pub fn invalidate(
        &self,
        src: &str,
        relation: &str,
        dst: &str,
        valid_time: Option<&str>,
        source_event_ids: &[String],
    ) -> Result<String, BossclawError> {
        self.append_graph_event("invalidate", src, relation, dst, valid_time, source_event_ids)
    }

    /// Shared builder for `link`/`invalidate`: constructs the Tier-B event
    /// (`model_id = "manual"`, empty prompt, non-empty `source_event_ids`) and
    /// routes it through the single-writer [`EventLog::append`].
    fn append_graph_event(
        &self,
        event_type: &str,
        src: &str,
        relation: &str,
        dst: &str,
        valid_time: Option<&str>,
        source_event_ids: &[String],
    ) -> Result<String, BossclawError> {
        let sources = if source_event_ids.is_empty() {
            vec![src.to_string(), dst.to_string()]
        } else {
            source_event_ids.to_vec()
        };
        self.append(Event {
            id: String::new(),
            ts: String::new(),
            valid_time: valid_time.map(String::from),
            event_type: event_type.to_string(),
            content: serde_json::json!({ "src": src, "relation": relation, "dst": dst }),
            model_meta: Some(ModelMeta {
                model_id: MANUAL_LINK_PRODUCER.to_string(),
                prompt_hash: String::new(),
                source_event_ids: sources,
            }),
            prev_hash: String::new(),
            hash: None,
            signed_by_did: self.signer_did(),
            signature: None,
        })
    }

    /// The DID stamped on engine-authored events (`link`/`invalidate`). v1 uses a
    /// fixed engine identity; M4/M7 will thread the user's real DID through here.
    fn signer_did(&self) -> String {
        "did:wba:bossclaw-engine".to_string()
    }
```

> NOTE: `signed_by_did` is informational here (not verified against `key` at append). A fixed engine DID keeps the M3 surface small; threading the user DID is M4/M7 (carried).

- [ ] **Step 7 — run, verify pass**

Run: `cd ~/air-note && cargo test -p bossclaw-core --test graph`
Expected: PASS (both Task-1 tests).

- [ ] **Step 8 — commit**

```bash
cd ~/air-note
git add crates/bossclaw-core/src/graph.rs crates/bossclaw-core/src/lib.rs crates/bossclaw-core/src/log.rs crates/bossclaw-core/tests/graph.rs
git status -s
git commit -m "feat(bossclaw-core): link/invalidate Tier-B events + edges/nodes schema (M3 T1)"
```

---

## Task 2: `rebuild_graph` deterministic fold + byte-identical rebuild test

**Files:**
- Modify: `crates/bossclaw-core/src/graph.rs` (add `fold_edges`, `parse_link_content`), `crates/bossclaw-core/src/log.rs` (`rebuild_graph`, `all_edges`, `all_nodes`, two private collectors)
- Test: `crates/bossclaw-core/tests/graph.rs`

- [ ] **Step 1 — write the failing tests** (`tests/graph.rs`, append):

```rust
use bossclaw_core::graph::Edge;

#[test]
fn rebuild_graph_is_byte_identical_across_rebuilds() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let a = log.append(mk_memory("kenny")).unwrap();
    let b = log.append(mk_memory("acme")).unwrap();
    let c = log.append(mk_memory("beta corp")).unwrap();
    log.link(&a, "works_at", &b, None, &[]).unwrap();
    log.link(&a, "knows", &c, None, &[]).unwrap();
    log.invalidate(&a, "works_at", &b, None, &[a.clone()]).unwrap();

    log.rebuild_graph().unwrap();
    let edges1 = log.all_edges().unwrap();
    let nodes1 = log.all_nodes().unwrap();
    log.rebuild_graph().unwrap();
    let edges2 = log.all_edges().unwrap();
    let nodes2 = log.all_nodes().unwrap();

    assert_eq!(edges1, edges2, "edges fold must be byte-identical across rebuilds");
    assert_eq!(nodes1, nodes2, "nodes fold must be byte-identical across rebuilds");
    assert_eq!(edges1.len(), 2, "two link events → two edge rows");
    assert_eq!(nodes1.len(), 3, "three distinct endpoints → three nodes");
}

#[test]
fn invalidate_closes_not_deletes_and_relink_opens_new_interval() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let a = log.append(mk_memory("kenny")).unwrap();
    let b = log.append(mk_memory("acme")).unwrap();

    log.link(&a, "works_at", &b, None, &[]).unwrap();
    log.invalidate(&a, "works_at", &b, None, &[a.clone()]).unwrap();
    log.link(&a, "works_at", &b, None, &[]).unwrap(); // re-link → new interval
    log.rebuild_graph().unwrap();

    let edges = log.all_edges().unwrap();
    assert_eq!(edges.len(), 2, "invalidate closes (not deletes); re-link adds a 2nd row");
    let closed: Vec<&Edge> = edges.iter().filter(|e| e.invalidated_at.is_some()).collect();
    let open: Vec<&Edge> = edges.iter().filter(|e| e.is_current()).collect();
    assert_eq!(closed.len(), 1, "exactly one closed assertion");
    assert_eq!(open.len(), 1, "exactly one re-opened current assertion");
    assert!(closed[0].valid_to.is_some(), "closed edge carries a world-clock end");
    assert!(closed[0].invalidated_by.is_some(), "closed edge records its invalidator");
}

#[test]
fn nodes_kind_is_memory_for_memory_endpoints() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let a = log.append(mk_memory("kenny")).unwrap();
    // dst is an id that does NOT exist as a memory event → "external".
    log.link(&a, "mentions", "entity:acme", None, &[a.clone()]).unwrap();
    log.rebuild_graph().unwrap();

    let nodes = log.all_nodes().unwrap();
    let kind_of = |id: &str| nodes.iter().find(|n| n.node_id == id).map(|n| n.kind.clone());
    assert_eq!(kind_of(&a).as_deref(), Some("memory"));
    assert_eq!(kind_of("entity:acme").as_deref(), Some("external"));
}
```

- [ ] **Step 2 — run, verify fail**

Run: `cd ~/air-note && cargo test -p bossclaw-core --test graph -- rebuild_graph invalidate_closes nodes_kind`
Expected: FAIL — `no method named rebuild_graph` / `all_edges` / `all_nodes`.

- [ ] **Step 3 — add the pure fold to `src/graph.rs`:**

```rust
use std::collections::HashMap;

use crate::event::Event;

/// Extract `(src, relation, dst)` from a `link`/`invalidate` event's content,
/// or `None` if any field is missing or non-string (malformed — skipped by the
/// fold rather than failing it).
pub fn parse_link_content(content: &serde_json::Value) -> Option<(String, String, String)> {
    let src = content.get("src")?.as_str()?.to_string();
    let relation = content.get("relation")?.as_str()?.to_string();
    let dst = content.get("dst")?.as_str()?.to_string();
    Some((src, relation, dst))
}

/// Fold `link`/`invalidate` events (which MUST already be in `seq` order) into
/// the current edge set. Deterministic: edges are produced in link-event order;
/// each `link` opens an assertion (`edge_id` = the link event id), each
/// `invalidate` closes ALL currently-active assertions for its `(src, relation,
/// dst)` key. Re-linking after an invalidate opens a fresh assertion (a new
/// validity interval). Malformed events (no src/relation/dst) are skipped.
pub fn fold_edges(events: &[Event]) -> Vec<Edge> {
    let mut edges: Vec<Edge> = Vec::new();
    // edge-key → indices into `edges` that are still open for that key.
    let mut active: HashMap<(String, String, String), Vec<usize>> = HashMap::new();
    for ev in events {
        let (src, relation, dst) = match parse_link_content(&ev.content) {
            Some(t) => t,
            None => continue,
        };
        let key = (src.clone(), relation.clone(), dst.clone());
        match ev.event_type.as_str() {
            "link" => {
                let valid_from = normalize_ts(ev.valid_time.as_deref().unwrap_or(&ev.ts));
                let ingested_at = normalize_ts(&ev.ts);
                edges.push(Edge {
                    edge_id: ev.id.clone(),
                    src,
                    relation,
                    dst,
                    valid_from,
                    valid_to: None,
                    ingested_at,
                    invalidated_at: None,
                    invalidated_by: None,
                });
                active.entry(key).or_default().push(edges.len() - 1);
            }
            "invalidate" => {
                if let Some(indices) = active.remove(&key) {
                    let valid_to = normalize_ts(ev.valid_time.as_deref().unwrap_or(&ev.ts));
                    let invalidated_at = normalize_ts(&ev.ts);
                    for i in indices {
                        edges[i].valid_to = Some(valid_to.clone());
                        edges[i].invalidated_at = Some(invalidated_at.clone());
                        edges[i].invalidated_by = Some(ev.id.clone());
                    }
                }
            }
            _ => {}
        }
    }
    edges
}
```

- [ ] **Step 4 — add `rebuild_graph` + reads to `src/log.rs`** (`impl EventLog`). Add `use std::collections::{BTreeMap, HashSet};` to the existing imports as needed (note `HashMap` is already imported):

```rust
    /// Rebuild the persisted `edges`/`nodes` tables as a deterministic fold over
    /// every `link`/`invalidate` event (`ORDER BY seq ASC`). Tier-A: byte-
    /// identical across rebuilds (spec §4/§9). Wipes both tables and re-inserts
    /// under one transaction. Cheap (graph events are few). Call after appending
    /// `link`/`invalidate` events to refresh `neighbors`/`as_of`/the recall boost.
    pub fn rebuild_graph(&self) -> Result<(), BossclawError> {
        let started = Instant::now();
        let events = self.graph_events_ordered()?;
        let edges = crate::graph::fold_edges(&events);
        let memory_ids = self.memory_page_ids()?;

        // Distinct endpoints → nodes (BTreeMap = deterministic node order).
        let mut node_kinds: BTreeMap<String, String> = BTreeMap::new();
        for e in &edges {
            for endpoint in [&e.src, &e.dst] {
                node_kinds.entry(endpoint.clone()).or_insert_with(|| {
                    if memory_ids.contains(endpoint) {
                        "memory".to_string()
                    } else {
                        "external".to_string()
                    }
                });
            }
        }

        let edge_count = edges.len();
        let node_count = node_kinds.len();
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let tx = conn.unchecked_transaction()?;
        tx.execute("DELETE FROM edges", [])?;
        tx.execute("DELETE FROM nodes", [])?;
        for e in &edges {
            tx.execute(
                "INSERT INTO edges
                   (edge_id, src, relation, dst, valid_from, valid_to,
                    ingested_at, invalidated_at, invalidated_by)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![
                    e.edge_id, e.src, e.relation, e.dst, e.valid_from, e.valid_to,
                    e.ingested_at, e.invalidated_at, e.invalidated_by
                ],
            )?;
        }
        for (node_id, kind) in &node_kinds {
            tx.execute(
                "INSERT INTO nodes (node_id, kind) VALUES (?1, ?2)",
                rusqlite::params![node_id, kind],
            )?;
        }
        tx.commit()?;
        log::info!(
            "rebuilt graph: {edge_count} edges, {node_count} nodes in {}ms",
            started.elapsed().as_millis()
        );
        Ok(())
    }

    /// All `link`/`invalidate` events, payload-parsed, in chain (`seq ASC`) order.
    fn graph_events_ordered(&self) -> Result<Vec<Event>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT payload FROM events
             WHERE event_type IN ('link', 'invalidate') ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(serde_json::from_str(&row?)?);
        }
        Ok(out)
    }

    /// Set of event ids whose type is `memory`/`page` — used to label node kinds.
    fn memory_page_ids(&self) -> Result<HashSet<String>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt =
            conn.prepare("SELECT id FROM events WHERE event_type IN ('memory', 'page')")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = HashSet::new();
        for row in rows {
            out.insert(row?);
        }
        Ok(out)
    }

    /// Every edge, `ORDER BY edge_id ASC` (deterministic). Tier-A read.
    pub fn all_edges(&self) -> Result<Vec<crate::graph::Edge>, BossclawError> {
        self.query_edges("SELECT edge_id, src, relation, dst, valid_from, valid_to, \
            ingested_at, invalidated_at, invalidated_by FROM edges ORDER BY edge_id ASC", &[])
    }

    /// Every node, `ORDER BY node_id ASC`.
    pub fn all_nodes(&self) -> Result<Vec<crate::graph::Node>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare("SELECT node_id, kind FROM nodes ORDER BY node_id ASC")?;
        let rows = stmt.query_map([], |r| {
            Ok(crate::graph::Node { node_id: r.get(0)?, kind: r.get(1)? })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Run a SELECT that returns the full edge column list (in the fixed order
    /// used by [`EventLog::all_edges`]) and map rows to [`crate::graph::Edge`].
    /// Shared by `all_edges`, `neighbors`, and `as_of` so the column→field
    /// mapping is single-sourced.
    fn query_edges(
        &self,
        sql: &str,
        params: &[&dyn rusqlite::ToSql],
    ) -> Result<Vec<crate::graph::Edge>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map(params, |r| {
            Ok(crate::graph::Edge {
                edge_id: r.get(0)?,
                src: r.get(1)?,
                relation: r.get(2)?,
                dst: r.get(3)?,
                valid_from: r.get(4)?,
                valid_to: r.get(5)?,
                ingested_at: r.get(6)?,
                invalidated_at: r.get(7)?,
                invalidated_by: r.get(8)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }
```

- [ ] **Step 5 — run, verify pass**

Run: `cd ~/air-note && cargo test -p bossclaw-core --test graph`
Expected: PASS (all Task-1 + Task-2 tests).

- [ ] **Step 6 — commit**

```bash
cd ~/air-note
git add crates/bossclaw-core/src/graph.rs crates/bossclaw-core/src/log.rs crates/bossclaw-core/tests/graph.rs
git status -s
git commit -m "feat(bossclaw-core): deterministic bi-temporal edges/nodes fold (M3 T2)"
```

---

## Task 3: `neighbors` + backlinks

**Files:** Modify `crates/bossclaw-core/src/log.rs`; test in `crates/bossclaw-core/tests/graph.rs`.

- [ ] **Step 1 — failing test:**

```rust
#[test]
fn neighbors_returns_current_edges_both_directions_backlinks_filterable() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let a = log.append(mk_memory("kenny")).unwrap();
    let b = log.append(mk_memory("acme")).unwrap();
    let c = log.append(mk_memory("carol")).unwrap();
    log.link(&a, "works_at", &b, None, &[]).unwrap(); // a → b (outgoing from a)
    log.link(&c, "manages", &a, None, &[]).unwrap();   // c → a (incoming to a = backlink)
    let stale = log.link(&a, "old", &b, None, &[]).unwrap();
    log.invalidate(&a, "old", &b, None, &[a.clone()]).unwrap(); // closed → excluded
    log.rebuild_graph().unwrap();

    let n = log.neighbors(&a).unwrap();
    assert_eq!(n.len(), 2, "two current edges touch a (the invalidated 'old' edge is excluded)");
    assert!(n.iter().all(|e| e.edge_id != stale), "invalidated edge must not appear");

    // Backlinks = the subset whose dst == a (incoming).
    let backlinks: Vec<&_> = n.iter().filter(|e| e.dst == a).collect();
    assert_eq!(backlinks.len(), 1);
    assert_eq!(backlinks[0].src, c);
    assert_eq!(backlinks[0].relation, "manages");
}
```

- [ ] **Step 2 — run, verify fail**

Run: `cd ~/air-note && cargo test -p bossclaw-core --test graph -- neighbors_returns`
Expected: FAIL — `no method named neighbors`.

- [ ] **Step 3 — implement `neighbors`** (`src/log.rs`, `impl EventLog`):

```rust
    /// Current edges touching `node` in either direction (`invalidated_at IS
    /// NULL`). Backlinks are the subset whose `dst == node`. `ORDER BY edge_id
    /// ASC` for deterministic output.
    pub fn neighbors(&self, node: &str) -> Result<Vec<crate::graph::Edge>, BossclawError> {
        self.query_edges(
            "SELECT edge_id, src, relation, dst, valid_from, valid_to, \
                ingested_at, invalidated_at, invalidated_by \
             FROM edges \
             WHERE (src = ?1 OR dst = ?1) AND invalidated_at IS NULL \
             ORDER BY edge_id ASC",
            &[&node as &dyn rusqlite::ToSql],
        )
    }
```

- [ ] **Step 4 — run, verify pass**

Run: `cd ~/air-note && cargo test -p bossclaw-core --test graph`
Expected: PASS.

- [ ] **Step 5 — commit**

```bash
cd ~/air-note
git add crates/bossclaw-core/src/log.rs crates/bossclaw-core/tests/graph.rs
git status -s
git commit -m "feat(bossclaw-core): neighbors + backlinks over current edges (M3 T3)"
```

---

## Task 4: bi-temporal `as_of` (both clocks)

**Files:** Modify `crates/bossclaw-core/src/log.rs`; test in `crates/bossclaw-core/tests/graph.rs`.

- [ ] **Step 1 — failing test.** Uses explicit `valid_time` to make the two clocks distinguishable from the auto-assigned ingestion `ts`:

```rust
use bossclaw_core::graph::AsOf;

#[test]
fn as_of_valid_time_shows_world_truth_then_known_as_of_shows_belief_then() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let a = log.append(mk_memory("kenny")).unwrap();
    let b = log.append(mk_memory("acme")).unwrap();

    // Kenny worked at Acme in the world from 2020 until 2022.
    log.link(&a, "works_at", &b, Some("2020-01-01T00:00:00Z"), &[]).unwrap();
    log.invalidate(&a, "works_at", &b, Some("2022-01-01T00:00:00Z"), &[a.clone()]).unwrap();
    log.rebuild_graph().unwrap();

    // all-None == current: the edge is invalidated, so nothing current.
    assert!(log.as_of(&a, &AsOf::default()).unwrap().is_empty(), "edge is retired → not current");

    // valid_time inside [2020, 2022): the fact WAS true in the world.
    let mid = AsOf { valid_time: Some("2021-06-01T00:00:00Z".into()), known_as_of: None };
    assert_eq!(log.as_of(&a, &mid).unwrap().len(), 1, "true in the world in 2021");

    // valid_time after 2022: no longer true.
    let after = AsOf { valid_time: Some("2023-01-01T00:00:00Z".into()), known_as_of: None };
    assert!(log.as_of(&a, &after).unwrap().is_empty(), "not true in the world in 2023");

    // known_as_of in the far future: we learned (and then un-learned) it — the
    // invalidate's ingested time is "now", so a far-future known_as_of sees it as
    // already-retracted; a known_as_of BEFORE the invalidate ingested would see it
    // as still-believed. Both ingestion times are ~now, so assert the retracted side.
    let known_future = AsOf { valid_time: None, known_as_of: Some("2999-01-01T00:00:00Z".into()) };
    assert!(log.as_of(&a, &known_future).unwrap().is_empty(), "by 2999 we had retracted it");
}
```

- [ ] **Step 2 — run, verify fail**

Run: `cd ~/air-note && cargo test -p bossclaw-core --test graph -- as_of_valid_time`
Expected: FAIL — `no method named as_of`.

- [ ] **Step 3 — implement `as_of`** (`src/log.rs`, `impl EventLog`). Builds the WHERE clause dynamically and normalizes the query timestamps so they compare correctly against the normalized stored values:

```rust
    /// Bi-temporal edge query for `node` (spec §5). Both `AsOf` axes are optional
    /// `WHERE` filters layered on the persisted edges:
    /// - `valid_time` t → `valid_from <= t AND (valid_to IS NULL OR t < valid_to)`
    ///   ("true in the world at t").
    /// - `known_as_of` t → `ingested_at <= t AND (invalidated_at IS NULL OR
    ///   t < invalidated_at)` ("known at t").
    ///
    /// When BOTH axes are `None`, returns the current graph (`invalidated_at IS
    /// NULL`), identical to [`EventLog::neighbors`]. Query timestamps are
    /// normalized so TEXT comparison is chronological. `ORDER BY edge_id ASC`.
    pub fn as_of(
        &self,
        node: &str,
        as_of: &crate::graph::AsOf,
    ) -> Result<Vec<crate::graph::Edge>, BossclawError> {
        let mut sql = String::from(
            "SELECT edge_id, src, relation, dst, valid_from, valid_to, \
                ingested_at, invalidated_at, invalidated_by \
             FROM edges WHERE (src = ?1 OR dst = ?1)",
        );
        // Owned, normalized params kept alive for the bind slice below.
        let mut owned: Vec<String> = Vec::new();
        let valid = as_of.valid_time.as_ref().map(|t| crate::graph::normalize_ts(t));
        let known = as_of.known_as_of.as_ref().map(|t| crate::graph::normalize_ts(t));

        match (&valid, &known) {
            (None, None) => sql.push_str(" AND invalidated_at IS NULL"),
            _ => {
                if let Some(t) = &valid {
                    let i = owned.len() + 2; // ?1 is node
                    sql.push_str(&format!(
                        " AND valid_from <= ?{i} AND (valid_to IS NULL OR ?{i} < valid_to)"
                    ));
                    owned.push(t.clone());
                }
                if let Some(t) = &known {
                    let i = owned.len() + 2;
                    sql.push_str(&format!(
                        " AND ingested_at <= ?{i} AND (invalidated_at IS NULL OR ?{i} < invalidated_at)"
                    ));
                    owned.push(t.clone());
                }
            }
        }
        sql.push_str(" ORDER BY edge_id ASC");

        let mut params: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(1 + owned.len());
        params.push(&node as &dyn rusqlite::ToSql);
        for t in &owned {
            params.push(t as &dyn rusqlite::ToSql);
        }
        self.query_edges(&sql, &params)
    }
```

- [ ] **Step 4 — run, verify pass**

Run: `cd ~/air-note && cargo test -p bossclaw-core --test graph`
Expected: PASS.

- [ ] **Step 5 — commit**

```bash
cd ~/air-note
git add crates/bossclaw-core/src/log.rs crates/bossclaw-core/tests/graph.rs
git status -s
git commit -m "feat(bossclaw-core): bi-temporal as_of (valid-time + known-as-of) (M3 T4)"
```

---

## Task 5: live graph-proximity recall boost

**Files:** Modify `crates/bossclaw-core/src/recall.rs` (consts + `graph_seeds`), `crates/bossclaw-core/src/log.rs` (BFS helper + recall wiring); test in `crates/bossclaw-core/tests/recall.rs` (+ fix the existing `RecallOptions` literal).

- [ ] **Step 1 — add the four named consts to `src/recall.rs`** (after `PIN_MULTIPLIER`):

```rust
/// Max multiplicative graph-proximity boost at 1 hop, as a fraction of the fused
/// score: a direct neighbour of a seed is boosted by `1 + GRAPH_WEIGHT`. Kept
/// just below [`RECENCY_WEIGHT`] (0.5) and far below [`PIN_MULTIPLIER`] (2.0) so
/// graph-relatedness is a *tilt* on ranking, not an override.
pub const GRAPH_WEIGHT: f32 = 0.4;

/// Per-hop decay of the graph boost: each extra hop multiplies the boost factor
/// by this. With [`GRAPH_MAX_HOPS`] = 1 only direct neighbours are boosted, but
/// the term is applied as `GRAPH_HOP_DECAY^(hops-1)` so raising the hop cap later
/// needs no formula change.
pub const GRAPH_HOP_DECAY: f32 = 0.5;

/// How many hops out from a seed the proximity boost reaches. v1 = 1 (direct
/// neighbours only); the BFS + decay term already support a larger cap.
pub const GRAPH_MAX_HOPS: u32 = 1;

/// When no explicit `graph_seeds` are supplied, auto-seed proximity from the top
/// N fused candidates (their own node ids). 1 = "boost what's linked to the
/// single strongest hit", which fires with zero caller input.
pub const GRAPH_AUTO_SEED_TOPK: usize = 1;
```

- [ ] **Step 2 — add `graph_seeds` to `RecallOptions`** (`src/recall.rs`). The struct still `#[derive(Default)]` (a `Vec` defaults to empty):

```rust
#[derive(Default)]
pub struct RecallOptions {
    /// Event ids to boost by [`PIN_MULTIPLIER`] regardless of organic rank.
    pub pinned: Vec<String>,
    /// Explicit graph-proximity seed node ids. When non-empty, recall boosts
    /// candidates within [`GRAPH_MAX_HOPS`] of these (current edges only). When
    /// empty, recall auto-seeds from the top [`GRAPH_AUTO_SEED_TOPK`] fused hits.
    pub graph_seeds: Vec<String>,
}
```

- [ ] **Step 3 — fix the existing `RecallOptions` literal** in `crates/bossclaw-core/tests/recall.rs`. Find `let opts = RecallOptions { pinned: vec![older.clone()] };` and change it to:

```rust
    let opts = RecallOptions { pinned: vec![older.clone()], ..Default::default() };
```

- [ ] **Step 4 — write the failing boost tests** (`tests/recall.rs`, append). Reuse the file's existing `seeded_log`, `find_hit`, `MID_DIM`, `RECALL_TOP_K`, `MockEmbedder`. The tests are **score-based, not rank-based**: they toggle an edge current→invalidated and assert the *same node's* `Hit.score` changes by the boost factor, which is deterministic (a rank assertion across query-irrelevant candidates would flake on MockEmbedder's arbitrary base order):

```rust
/// Auto-seed boost fires on a CURRENT edge and disappears when that edge is
/// invalidated. Score-based (robust to the arbitrary base order of
/// query-irrelevant candidates): the linked neighbour's score carries the ~1.4×
/// graph multiplier while the edge is current, and reverts to its unboosted base
/// once the edge is retired (current-edges-only gating, spec §6).
#[test]
fn recall_graph_proximity_auto_seed_boosts_only_current_edges() {
    let (log, _dir, ids) = seeded_log(&[
        "rustacean memory engine ferris",   // 0: query matches this → auto-seed
        "completely unrelated tokens here", // 1: neighbour of the seed
        "another disjoint vocabulary set",  // 2: unlinked bystander
    ]);
    log.link(&ids[0], "relates_to", &ids[1], None, &[]).unwrap();
    log.rebuild_graph().unwrap();

    let embedder = MockEmbedder::new(MID_DIM);
    let query = "rustacean memory engine ferris";

    let boosted = log.recall(&embedder, query, RECALL_TOP_K, &RecallOptions::default()).unwrap();
    let s_boosted = find_hit(&boosted, &ids[1]).expect("neighbor present").score;

    // Retire the edge → the neighbour must lose the boost.
    log.invalidate(&ids[0], "relates_to", &ids[1], None, &[ids[0].clone()]).unwrap();
    log.rebuild_graph().unwrap();
    let retired = log.recall(&embedder, query, RECALL_TOP_K, &RecallOptions::default()).unwrap();
    let s_retired = find_hit(&retired, &ids[1]).expect("neighbor present").score;

    assert!(
        s_boosted > s_retired * 1.2,
        "current-edge neighbor must be boosted (~1.4x) vs the retired-edge baseline: \
         boosted={s_boosted}, retired={s_retired}"
    );
}

/// Explicit `graph_seeds` boost a chosen node's neighbour that auto-seeding would
/// NOT reach. The link is seed(2) ↔ neighbour(1), but the query matches event 0,
/// so auto-seed (top hit = 0) never touches event 1. Passing `graph_seeds=[2]`
/// boosts event 1's score above its auto-seed (unboosted) score.
#[test]
fn recall_graph_proximity_explicit_seeds_boost_over_autoseed() {
    let (log, _dir, ids) = seeded_log(&[
        "rustacean memory engine ferris",
        "completely unrelated tokens here",
        "another disjoint vocabulary set",
    ]);
    log.link(&ids[2], "relates_to", &ids[1], None, &[]).unwrap();
    log.rebuild_graph().unwrap();

    let embedder = MockEmbedder::new(MID_DIM);
    let query = "rustacean memory engine ferris";

    let auto = log.recall(&embedder, query, RECALL_TOP_K, &RecallOptions::default()).unwrap();
    let s_auto = find_hit(&auto, &ids[1]).expect("present").score;

    let opts = RecallOptions { graph_seeds: vec![ids[2].clone()], ..Default::default() };
    let seeded = log.recall(&embedder, query, RECALL_TOP_K, &opts).unwrap();
    let s_seeded = find_hit(&seeded, &ids[1]).expect("present").score;

    assert!(
        s_seeded > s_auto * 1.2,
        "explicit seed must boost its neighbor over the auto-seed baseline: \
         seeded={s_seeded}, auto={s_auto}"
    );
}
```

- [ ] **Step 5 — run, verify fail**

Run: `cd ~/air-note && cargo test -p bossclaw-core --test recall -- recall_graph_proximity`
Expected: FAIL — `no field graph_seeds` / boost not applied.

- [ ] **Step 6 — add the BFS neighbor helper to `src/log.rs`** (`impl EventLog`). Returns `id → hop-distance` for current-edge neighbors within `max_hops`, excluding the seeds themselves:

```rust
    /// Map every node within `max_hops` of any `seed` (over CURRENT edges,
    /// treated as undirected for relatedness) to its shortest hop distance
    /// (1..=max_hops). Seeds themselves are excluded. Used by the recall
    /// graph-proximity boost. A seed with no current edges contributes nothing.
    fn current_neighbors_with_hops(
        &self,
        seeds: &[String],
        max_hops: u32,
    ) -> Result<HashMap<String, u32>, BossclawError> {
        let mut hops: HashMap<String, u32> = HashMap::new();
        let mut frontier: HashSet<String> = seeds.iter().cloned().collect();
        let mut visited: HashSet<String> = seeds.iter().cloned().collect();
        for hop in 1..=max_hops {
            if frontier.is_empty() {
                break;
            }
            let next = self.current_adjacent(&frontier)?;
            let mut new_frontier: HashSet<String> = HashSet::new();
            for id in next {
                if visited.insert(id.clone()) {
                    hops.insert(id.clone(), hop);
                    new_frontier.insert(id);
                }
            }
            frontier = new_frontier;
        }
        Ok(hops)
    }

    /// Distinct opposite endpoints of CURRENT edges incident to any id in
    /// `frontier` (undirected: returns both `dst` where `src ∈ frontier` and
    /// `src` where `dst ∈ frontier`). Empty `frontier` → empty set.
    fn current_adjacent(&self, frontier: &HashSet<String>) -> Result<HashSet<String>, BossclawError> {
        if frontier.is_empty() {
            return Ok(HashSet::new());
        }
        let ids: Vec<&String> = frontier.iter().collect();
        let placeholders: String =
            (0..ids.len()).map(|i| format!("?{}", i + 1)).collect::<Vec<_>>().join(",");
        // dst where src ∈ frontier  UNION  src where dst ∈ frontier (current only).
        let sql = format!(
            "SELECT dst AS other FROM edges WHERE invalidated_at IS NULL AND src IN ({placeholders}) \
             UNION \
             SELECT src AS other FROM edges WHERE invalidated_at IS NULL AND dst IN ({placeholders})"
        );
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(&sql)?;
        // Both IN clauses reference the SAME ?1..?n placeholders, so bind the id
        // list ONCE (n params, not 2n — binding 2n would exceed the statement's
        // parameter count and error at query time).
        let params: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|id| *id as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(params.as_slice(), |r| r.get::<_, String>(0))?;
        let mut out = HashSet::new();
        for row in rows {
            out.insert(row?);
        }
        Ok(out)
    }
```

- [ ] **Step 7 — wire the boost into `EventLog::recall`** (`src/log.rs`). Extend the `use crate::recall::{...}` import to add `GRAPH_AUTO_SEED_TOPK, GRAPH_HOP_DECAY, GRAPH_MAX_HOPS, GRAPH_WEIGHT`. Then, in `recall`, AFTER `fused` is computed and BEFORE the `scored` map, insert the seed + neighbor computation:

```rust
        // ── Graph-proximity seeds: explicit, else auto-seed from the top fused
        //    base score(s). Then BFS current-edge neighbors (best-effort: a graph
        //    error degrades to no boost, never failing recall — spec §6/§10). ──
        let seeds: Vec<String> = if !opts.graph_seeds.is_empty() {
            opts.graph_seeds.clone()
        } else {
            let mut by_score: Vec<(&String, &f32)> = fused.iter().collect();
            by_score.sort_by(|a, b| b.1.total_cmp(a.1).then_with(|| b.0.cmp(a.0)));
            by_score.into_iter().take(GRAPH_AUTO_SEED_TOPK).map(|(id, _)| id.clone()).collect()
        };
        let graph_hops = self
            .current_neighbors_with_hops(&seeds, GRAPH_MAX_HOPS)
            .unwrap_or_else(|e| {
                log::warn!("recall: graph-proximity boost skipped: {e}");
                HashMap::new()
            });
```

Then inside the `scored` map closure, AFTER the pin block and BEFORE building `sources`, add the graph multiplier:

```rust
                // Graph-proximity tilt: a current-edge neighbour of a seed is
                // boosted by 1 + GRAPH_WEIGHT * GRAPH_HOP_DECAY^(hops-1).
                if let Some(&hop) = graph_hops.get(&id) {
                    let decay = (GRAPH_HOP_DECAY as f64).powi(hop as i32 - 1);
                    score_f64 *= 1.0 + GRAPH_WEIGHT as f64 * decay;
                }
```

- [ ] **Step 8 — run, verify pass**

Run: `cd ~/air-note && cargo test -p bossclaw-core --test recall`
Expected: PASS (existing recall tests + the three new boost tests).

- [ ] **Step 9 — commit**

```bash
cd ~/air-note
git add crates/bossclaw-core/src/recall.rs crates/bossclaw-core/src/log.rs crates/bossclaw-core/tests/recall.rs
git status -s
git commit -m "feat(bossclaw-core): live graph-proximity recall boost (auto-seed, 1-hop, current edges) (M3 T5)"
```

---

## Task 6: CHANGELOG + final gates

**Files:** Modify `crates/bossclaw-core/CHANGELOG.md`.

- [ ] **Step 1 — add the M3 CHANGELOG entry.** Open `crates/bossclaw-core/CHANGELOG.md`, read the existing M2 entry's heading style, and add a matching M3 section at the top of the unreleased/most-recent area:

```markdown
### Milestone 3 — Graph
- `link`/`invalidate` Tier-B events; deterministic bi-temporal `nodes`/`edges`
  fold (`rebuild_graph`) — byte-identical on rebuild, timestamps normalized to
  fixed-width UTC.
- `neighbors` + backlinks (current edges); two-axis `as_of` (valid-time +
  known-as-of); `all_edges`/`all_nodes` reads.
- Live graph-proximity recall boost: auto-seeded from the top hit (explicit
  `graph_seeds` override), 1-hop, current edges only, multiplicative
  (`GRAPH_WEIGHT 0.4`) below recency/pin.
```

- [ ] **Step 2 — full hermetic suite green**

Run: `cd ~/air-note && cargo test -p bossclaw-core`
Expected: PASS, 0 failures (all `#[ignore]` real-model tests excluded). Confirm the new `graph` test file and the extended `recall` file both run.

- [ ] **Step 3 — clippy + unsafe gate**

Run: `cd ~/air-note && cargo clippy -p bossclaw-core --all-targets -- -D warnings`
Expected: clean (no warnings). `#![forbid(unsafe_code)]` already guarantees zero `unsafe`.

- [ ] **Step 4 — commit**

```bash
cd ~/air-note
git add crates/bossclaw-core/CHANGELOG.md
git status -s
git commit -m "docs(bossclaw-core): CHANGELOG M3 (Graph) + final gates (M3 T6)"
```

---

## Milestone 3 — Definition of Done
- [ ] `link`/`invalidate` append signed Tier-B events (`model_id="manual"`, non-empty `source_event_ids` defaulting to `[src,dst]`).
- [ ] `edges`/`nodes` are persisted Tier-A; `rebuild_graph` is a deterministic fold (`ORDER BY seq ASC`) that is **byte-identical across rebuilds** (proven by test).
- [ ] `invalidate` **closes, never deletes**; re-link opens a new validity interval (proven by test).
- [ ] `neighbors` returns current edges both directions; backlinks filterable by `dst`.
- [ ] `as_of` filters both clocks independently; all-`None` == current; timestamps normalized so SQL comparison is chronological.
- [ ] Live graph-proximity boost in `recall`: auto-seed + explicit seeds, 1-hop, **current edges only** (retired edges give no boost — proven), multiplicative, below recency/pin; degrades to no-boost on graph error.
- [ ] Existing `RecallOptions` literal updated; whole `bossclaw-core` suite green (hermetic, temp homes only); `clippy -D warnings` clean; zero `unsafe`.

## Carried into later milestones
- **LLM extraction** that auto-creates `link`/`invalidate` events → M4 (evolve); reuses these tables.
- **Entity nodes** (`kind="entity"`, namespaced ids) → M4, same schema.
- **Incremental single-edge fold** (`index_link`, so appends don't need a full `rebuild_graph`) → M7 (mirrors the deferred incremental `index_event` for vectors).
- **2-hop proximity** (BFS + decay already support it; `GRAPH_MAX_HOPS` gates it at 1) and **intra-result reinforcement seeding** → post-v1.
- **User DID threading** into `signer_did()` (v1 stamps a fixed engine DID) → M4/M7.
- **Desktop "what's connected to this" graph view** → M7.

---

## Self-Review
**Spec coverage (M3 design):** §3 link/invalidate events ✓(T1) · §4 edges/nodes fold + byte-identical rebuild ✓(T2) · §5 neighbors/backlinks ✓(T3) + bi-temporal as_of ✓(T4) · §6 live boost (auto-seed, explicit seeds, current-edges, multiplicative consts) ✓(T5) · §9 tests: determinism ✓, invalidate-not-delete ✓, re-link ✓, as_of both clocks ✓, boost + retired-edge ✓, hermetic ✓(T1-T6).
**Placeholder scan:** no TBD/TODO; every code step shows complete code; the one cross-task reference (`query_edges`) is defined in T2 and reused in T3/T4. `GRAPH_HOP_DECAY`/`GRAPH_MAX_HOPS` are used by the BFS/boost (no dead consts → no clippy warning).
**Type consistency:** `Edge`/`Node`/`AsOf` fields are identical across `graph.rs`, the `edges` DDL, `query_edges`, and every test. `link`/`invalidate`/`append_graph_event` signatures match their call sites. `RecallOptions` gains `graph_seeds` and the one existing literal is fixed (T5 S3). `current_neighbors_with_hops`/`current_adjacent` return types match the recall wiring (`HashMap<String,u32>`).
**No magic numbers:** `GRAPH_WEIGHT`, `GRAPH_HOP_DECAY`, `GRAPH_MAX_HOPS`, `GRAPH_AUTO_SEED_TOPK` are named, sourced consts; `MANUAL_LINK_PRODUCER` names the producer string.
**Hermeticity:** all tests use `MockEmbedder` + `tempfile` temp homes; no network, no real model. The real-model test stays `#[ignore]`.
