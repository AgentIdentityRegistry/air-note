# bossclaw-core M5a (Ingest Pipeline) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Read-only ingest of user-granted folders into the signed log as recallable, externally-tainted `file_ingested` events — the complete safe pipeline with native UTF-8 text/markdown parsing in-process (no subprocess), kernel-enforced containment, dedup/version-supersede, and the taint root.

**Architecture:** A new `ingest` module orchestrates a no-symlink-follow `openat`-fd-chain walk over a granted folder, opens each file via a per-OS *careful open* (Linux `openat2(RESOLVE_BENEATH|RESOLVE_NO_SYMLINKS)`, macOS `openat`+`O_NOFOLLOW` chain, Windows canonicalize-contain + reparse-reject), reads its bytes once (the orchestrator owns identity hashing), hands the bytes to a `Parser` seam (native UTF-8 impl + mock), then makes a per-`canonical_path` dedup/supersede decision and emits a ground-truth (`model_meta: None`) `file_ingested` event carrying `origin:"external"` inside its signed content. Two new deterministic projections — `grants` (active folders) and `files` (current `file_ingested` per path) — fold from events exactly like the existing `pages` projection and rebuild on open. Recall gains a new exclusion arm that keeps a `file_ingested` hit only if it is the current version for its path AND its grant is still active.

**Tech Stack:** Rust (edition 2021), `rustix` (safe syscall bindings — keeps `#![forbid(unsafe_code)]`), `rusqlite` + `bundled-sqlcipher`, `sha2`, `serde_json`, existing `EventLog`/`Embedder`/`graph` fold infrastructure. Tests are hermetic (temp homes + `MockEmbedder` + a mock parser), inline `#[cfg(test)] mod tests`.

> **Plan revision — folds a second opinion (2026-06-18).** An independent **critic + security** review of *this plan* (both SHIP-WITH-FIXES, converged) verified all six load-bearing code claims TRUE (no Rev-1-style false reuse) but caught one genuine spec-level over-claim: **taint laundering via the evolve loop's recall _context_** — cursor-exclusion alone is NOT "no laundering" once files are recallable. Folded throughout: a new `RecallOptions.exclude_files` flag set in the evolve recall (Tasks 8–9); never-touch **case-insensitivity** for macOS/APFS + more secret patterns (Task 6); a Windows **re-fstat-after-open** + a **CI-both-OS** containment requirement + a macOS-strength reconciliation (Tasks 5/11); and added cross-fold, mtime-no-supersede, and context-laundering tests (Tasks 3/7/9). The matching spec correction is **Rev 3** (§4, §6, §10).

---

## Pre-flight (read before Task 1)

**Branch:** `bossclaw-core-m5-ingest` (already checked out; spec Rev 2 committed at `8c51692`). All work commits here; PR opens after Task 11.

**Spec:** `docs/superpowers/specs/2026-06-18-bossclaw-core-m5a-ingest-pipeline-design.md` (Rev 2). This plan implements it.

**Verified code anchors (read these once):**
- `EMBEDDABLE_EVENT_TYPES` — `crates/bossclaw-core/src/log.rs:119` = `&[MEMORY_EVENT_TYPE, PAGE_EVENT_TYPE]`. Adding `file_ingested` here makes it embed+FTS+recallable with **zero new embed code** (the embed text extractor `embeddable_text` at `log.rs:3286` reads `content["text"]` for any type in this set).
- `append` / `append_pair` / `reject_empty_tier_b` — `log.rs:300/315/330`. `reject_empty_tier_b` fires **only** when `model_meta` is `Some`. Ground-truth events (`model_meta: None`) pass both `append` and `append_pair` freely → a ground-truth `supersede`+`file_ingested` pair is atomic with no orphan.
- `page` / `supersede` / `emit_page` — `log.rs:1599/1633/1665` — the **template** this plan mirrors for `file_ingested` (but ground-truth, not Tier-B).
- `rebuild_graph` — `log.rs:1807` folds edges/entities/pages and writes the projection tables inside one tx (`DELETE FROM …` then `INSERT`). M5a extends it to also fold+write `grants` and `files`.
- `open_with_recall` — `log.rs:520` calls `rebuild_indexes` then `rebuild_graph`, so new projections rebuild on open for free.
- Recall exclusion arm + gating — `log.rs:982` (gated `current_pages()` read) and `log.rs:1096` (the `hits.retain` page arm). M5a adds a parallel `file_ingested` arm.
- Evolve laundering has **TWO doors — both must be shut** (second-opinion finding; spec Rev 3 §4). (1) The evolve **cursor** (`unprocessed_memories_since`, `log.rs:2443`) filters `event_type = MEMORY_EVENT_TYPE`, so files are never extraction *subjects* (its doc comment already says "`file_ingested` extraction is deferred"). (2) The evolve loop's internal **recall context** (`evolve_once` → `recall(.., &RecallOptions { exclude_pages: true, .. })`, `log.rs:2951-2964`) — because Task 1 makes `file_ingested` recallable, an unfiltered context recall would feed external file text into `extract::propose` and the derived link/entity would **not** be marked external (laundering). **Task 8 adds `RecallOptions.exclude_files`; Task 9 sets it `true` in the evolve recall**, mirroring the existing `exclude_pages` (F3) one-way rule. Cursor exclusion alone is NOT "no laundering."
- `#![forbid(unsafe_code)]` — `lib.rs:16`. Use `rustix` (safe wrappers), never hand-rolled `unsafe`.
- Test harness (mirror exactly) — `log.rs:3378`: `const DEK: [u8;32] = [42u8;32];`, `const KEY_BYTES: [u8;32] = [7u8;32];`, `fn open_log(dir) { EventLog::open(&dir.join("m.db"), &DEK, SigningKey::from_bytes(&KEY_BYTES)).unwrap() }`, `fn mk_memory(text) -> Event {…}`, `tempfile::tempdir()`.

**Build / verify gates (run from `~/air-note`):**
- Build: `cargo build -p bossclaw-core`
- Tests: `cargo test -p bossclaw-core 2>&1 | tail -50`
- Lint (must stay clean, default + each feature): `cargo clippy -p bossclaw-core --all-targets` and `cargo clippy -p bossclaw-core --all-targets --features ollama`
- The crate must KEEP `#![forbid(unsafe_code)]` (do not remove it).

**File structure (created/modified across the plan):**
- **Create** `crates/bossclaw-core/src/ingest.rs` — the orchestrator: `IngestReport`, `IngestError`, `PathHint`, `ContainedFile`, `Parser` trait + `NativeTextParser` + `MockParser`, the careful-open (per-OS), the safe walk + never-touch filter + caps, `is_external`, and `impl EventLog { ingest_grant, ingest_all }`.
- **Modify** `crates/bossclaw-core/src/graph.rs` — new consts (`FILE_INGESTED_EVENT_TYPE`, `GRANT_EVENT_TYPE`, `REVOKE_EVENT_TYPE`, `EXTERNAL_ORIGIN`), `Grant` + `FileRecord` structs, `fold_grants` + `fold_files` + parse helpers.
- **Modify** `crates/bossclaw-core/src/log.rs` — add `file_ingested` to `EMBEDDABLE_EVENT_TYPES`; create the `grants` + `files` tables in `open`; extend `rebuild_graph` to fold+write them; add `add_grant`/`revoke_grant`/`grants`/`current_files`/`current_file_for_path`/`current_files_active`; add the recall `file_ingested` exclusion arm.
- **Modify** `crates/bossclaw-core/src/lib.rs` — `pub mod ingest;` + re-exports.
- **Modify** `crates/bossclaw-core/Cargo.toml` — add `rustix`.

**Cross-task type contract (defined in Task 1/4, used everywhere — names are fixed):**
- `graph::FILE_INGESTED_EVENT_TYPE = "file_ingested"`, `graph::GRANT_EVENT_TYPE = "grant"`, `graph::REVOKE_EVENT_TYPE = "revoke"`, `graph::EXTERNAL_ORIGIN = "external"`.
- `graph::Grant { canonical_root: String, granted_at: String, revoked: bool }`.
- `graph::FileRecord { canonical_path: String, file_event_id: String, content_hash: String, grant_root: String }`.
- `ingest::IngestReport { ingested: usize, superseded: usize, deduped: usize, skipped: Vec<(PathBuf, String)>, failed: Vec<(PathBuf, String)> }`.
- `ingest::Parser::convert(&self, raw: &[u8], hint: &PathHint) -> Result<String, IngestError>` and `Parser::parser_id(&self) -> &str`.
- `ingest::ContainedFile` — proof-of-containment handle returned by the careful open; exposes `identity() -> FileIdentity`, `size() -> u64`, `read_all_capped(cap: usize) -> Result<Vec<u8>, IngestError>`.

> **Design note carried from spec verification (surface, don't hide — the M4b lesson):**
> 1. **The supersede event for files reuses `SUPERSEDE_EVENT_TYPE` but is GROUND-TRUTH** (`model_meta: None`), emitted via `append_pair` (which tolerates ground-truth). It is **not** the Tier-B page `supersede()` method. Cross-safety with `fold_pages` holds because a `supersede{supersedes: X}` retires exactly the one event id `X`, and file-event ids and page ids never collide (unique ULIDs) — so `fold_pages` never retires a file and `fold_files` never retires a page.
> 2. **The `Parser` seam takes `&[u8]` (already-read bytes), not the file handle.** The spec sketched `convert(&ContainedFile, …)`; this plan refines it so the **orchestrator** performs the single contained read and owns identity hashing (a parser can never re-resolve a path or lie about `content_hash`). The spec's intent — "`PathHint` carries no resolvable path; the path is never re-resolved downstream" — is preserved and strengthened. M5b's subprocess parser writes these bytes to the sandboxed child's stdin (still no path to the child).
> 3. **Containment is the `openat`-fd-chain walk with `O_NOFOLLOW` on every descent + the per-OS careful final open** (Linux `openat2 RESOLVE_BENEATH|RESOLVE_NO_SYMLINKS`, macOS `openat`+`NOFOLLOW`, Windows canonicalize+contain+reparse-reject), exactly per spec D3. `..` never appears (readdir yields child names only; `.`/`..` are skipped), so there is no `..`-escape path to resolve.

---

### Task 1: Constants, `rustix` dependency, and the recall/embed seam

**Files:**
- Modify: `crates/bossclaw-core/Cargo.toml`
- Modify: `crates/bossclaw-core/src/graph.rs` (consts near the existing `*_EVENT_TYPE` block, ~line 30–43)
- Modify: `crates/bossclaw-core/src/log.rs:119-120` (`EMBEDDABLE_EVENT_TYPES`)
- Test: inline in `crates/bossclaw-core/src/graph.rs`

- [ ] **Step 1: Add `rustix` to `Cargo.toml`**

Under `[dependencies]` (after `hnsw_rs = "0.3.4"`):

```toml
# Safe, I/O-safe syscall bindings — used by the ingest careful-open + walk so the
# crate keeps `#![forbid(unsafe_code)]` (no hand-rolled libc/unsafe). `fs` enables
# openat/openat2/statat/readdir; `process` is not needed in M5a.
rustix = { version = "0.38", features = ["fs"] }
```

- [ ] **Step 2: Add the new single-sourced consts to `graph.rs`**

After `pub const EXTERNAL_NODE_KIND: &str = "external";` (graph.rs:43), add:

```rust
/// The `event_type` discriminator for an ingested-file event (M5a). Ground-truth
/// (plain `append`, `model_meta: None`); added to `EMBEDDABLE_EVENT_TYPES` so its
/// `content["text"]` is embedded + FTS-indexed + recallable. Single-sourced so the
/// stamp site, the fold filter, and the recall arm reference the same string.
pub const FILE_INGESTED_EVENT_TYPE: &str = "file_ingested";
/// The `event_type` discriminator for a folder-grant event (M5a). Ground-truth.
pub const GRANT_EVENT_TYPE: &str = "grant";
/// The `event_type` discriminator for a folder-revoke event (M5a). Ground-truth.
pub const REVOKE_EVENT_TYPE: &str = "revoke";
/// The taint stamp written at `content["origin"]` of every `file_ingested` event
/// (M5a, D4). Distinct from the `edges.origin` column (`"manual"`/`"machine"`):
/// this marks external-origin content so the M6 lineage walk can fail closed.
/// Single-sourced so the stamp site and the `is_external` classifier cannot drift.
pub const EXTERNAL_ORIGIN: &str = "external";
```

- [ ] **Step 3: Add `file_ingested` to the embeddable/recallable set in `log.rs:119`**

Change:

```rust
const EMBEDDABLE_EVENT_TYPES: &[&str] =
    &[crate::graph::MEMORY_EVENT_TYPE, crate::graph::PAGE_EVENT_TYPE];
```

to:

```rust
const EMBEDDABLE_EVENT_TYPES: &[&str] = &[
    crate::graph::MEMORY_EVENT_TYPE,
    crate::graph::PAGE_EVENT_TYPE,
    crate::graph::FILE_INGESTED_EVENT_TYPE,
];
```

- [ ] **Step 4: Write the failing test (consts + embeddable membership)**

Add to `graph.rs`'s `#[cfg(test)] mod tests`:

```rust
#[test]
fn m5a_event_type_consts_are_distinct_and_stable() {
    // Stable wire strings (these land in signed content + the byte-identical rebuild).
    assert_eq!(FILE_INGESTED_EVENT_TYPE, "file_ingested");
    assert_eq!(GRANT_EVENT_TYPE, "grant");
    assert_eq!(REVOKE_EVENT_TYPE, "revoke");
    assert_eq!(EXTERNAL_ORIGIN, "external");
    // Must not collide with existing discriminators.
    for other in [MEMORY_EVENT_TYPE, PAGE_EVENT_TYPE, SUPERSEDE_EVENT_TYPE, ENTITY_EVENT_TYPE, CONFIG_EVENT_TYPE] {
        assert_ne!(FILE_INGESTED_EVENT_TYPE, other);
        assert_ne!(GRANT_EVENT_TYPE, other);
        assert_ne!(REVOKE_EVENT_TYPE, other);
    }
}
```

- [ ] **Step 5: Run the test (expect FAIL → then PASS after Step 2 lands)**

Run: `cargo test -p bossclaw-core m5a_event_type_consts -- --nocolor`
Expected before Step 2: FAIL (`cannot find value FILE_INGESTED_EVENT_TYPE`). After Steps 2–3: PASS.

- [ ] **Step 6: Build to confirm `rustix` resolves and nothing else broke**

Run: `cargo build -p bossclaw-core`
Expected: builds clean (an unused-dependency warning for `rustix` is fine until Task 5).

- [ ] **Step 7: Commit**

```bash
git add crates/bossclaw-core/Cargo.toml crates/bossclaw-core/src/graph.rs crates/bossclaw-core/src/log.rs
git commit -m "feat(bossclaw-core): M5a Task 1 — ingest event-type consts + rustix dep + embeddable seam"
```

---

### Task 2: `grants` projection — schema, fold, API, rebuild wiring

**Files:**
- Modify: `crates/bossclaw-core/src/graph.rs` (add `Grant`, `fold_grants`, `parse_grant_content`)
- Modify: `crates/bossclaw-core/src/log.rs` (create table in `open`; `add_grant`/`revoke_grant`/`grants`; fold+write in `rebuild_graph`)
- Test: inline in both

- [ ] **Step 1: Write the failing fold test in `graph.rs` tests**

```rust
#[test]
fn fold_grants_is_last_writer_wins_per_root() {
    // grant A, grant B, revoke A, grant A again → A active (re-granted), B active.
    let mk = |etype: &str, root: &str, ts: &str| Event {
        id: String::new(), ts: ts.to_string(), valid_time: None,
        event_type: etype.to_string(),
        content: serde_json::json!({ "canonical_root": root }),
        model_meta: None, prev_hash: String::new(), hash: None,
        signed_by_did: "did:wba:AIR-TEST".to_string(), signature: None,
    };
    let events = vec![
        mk(GRANT_EVENT_TYPE, "/a", "2026-06-18T00:00:00Z"),
        mk(GRANT_EVENT_TYPE, "/b", "2026-06-18T00:00:01Z"),
        mk(REVOKE_EVENT_TYPE, "/a", "2026-06-18T00:00:02Z"),
        mk(GRANT_EVENT_TYPE, "/a", "2026-06-18T00:00:03Z"),
    ];
    let mut grants = fold_grants(&events);
    grants.sort_by(|x, y| x.canonical_root.cmp(&y.canonical_root));
    assert_eq!(grants.len(), 2);
    assert_eq!(grants[0].canonical_root, "/a");
    assert!(!grants[0].revoked, "/a was re-granted after revoke → active");
    assert_eq!(grants[0].granted_at, "2026-06-18T00:00:03Z", "granted_at = latest grant ts");
    assert!(!grants[1].revoked, "/b never revoked");
}

#[test]
fn fold_grants_revoke_without_grant_is_ignored() {
    let ev = Event {
        id: String::new(), ts: "2026-06-18T00:00:00Z".to_string(), valid_time: None,
        event_type: REVOKE_EVENT_TYPE.to_string(),
        content: serde_json::json!({ "canonical_root": "/never-granted" }),
        model_meta: None, prev_hash: String::new(), hash: None,
        signed_by_did: "did:wba:AIR-TEST".to_string(), signature: None,
    };
    assert!(fold_grants(&[ev]).is_empty(), "a revoke with no prior grant yields no row");
}
```

- [ ] **Step 2: Run → expect FAIL**

Run: `cargo test -p bossclaw-core fold_grants -- --nocolor`
Expected: FAIL (`cannot find function fold_grants`).

- [ ] **Step 3: Implement `Grant`, `parse_grant_content`, `fold_grants` in `graph.rs`**

Add near `fold_pages` (after the `Page` block, ~line 300):

```rust
/// A folded folder grant (M5a): the CURRENT state of one granted root. A
/// deterministic fold over `grant`/`revoke` events; rebuilt by `rebuild_graph`.
/// `revoked` files are kept in the log forever but excluded from recall.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    /// The canonicalized absolute folder path (the grant's identity key).
    pub canonical_root: String,
    /// RFC 3339 ts of the latest `grant` event for this root (provenance).
    pub granted_at: String,
    /// True iff the latest event for this root is a `revoke`.
    pub revoked: bool,
}

/// Extract `canonical_root` from a `grant`/`revoke` event's content, or `None`.
fn parse_grant_content(content: &serde_json::Value) -> Option<String> {
    Some(content.get("canonical_root")?.as_str()?.to_string())
}

/// Fold `grant`/`revoke` events (MUST be in `seq` order) into the current grant
/// per root (last-writer-wins). A `grant` (re)activates and stamps `granted_at`;
/// a `revoke` marks an EXISTING root revoked. A `revoke` with no prior grant is
/// ignored. Deterministic → byte-identical rebuild.
pub fn fold_grants(events: &[Event]) -> Vec<Grant> {
    use std::collections::BTreeMap;
    let mut by_root: BTreeMap<String, Grant> = BTreeMap::new();
    for ev in events {
        let root = match parse_grant_content(&ev.content) {
            Some(r) => r,
            None => continue,
        };
        if ev.event_type == GRANT_EVENT_TYPE {
            by_root.insert(root.clone(), Grant { canonical_root: root, granted_at: ev.ts.clone(), revoked: false });
        } else if ev.event_type == REVOKE_EVENT_TYPE {
            if let Some(g) = by_root.get_mut(&root) {
                g.revoked = true;
            }
        }
    }
    by_root.into_values().collect()
}
```

- [ ] **Step 4: Create the `grants` table in `EventLog::open`**

In `log.rs`, after the `pages` table block (ends ~line 237), add:

```rust
// Folder-grant projection (Tier-A; M5a). One row per granted root; a
// deterministic fold over `grant`/`revoke` events, rebuilt by `rebuild_graph`.
// `ingest_all` iterates active (revoked = 0) grants only.
store.exec(
    "CREATE TABLE IF NOT EXISTS grants (
        canonical_root TEXT PRIMARY KEY,
        granted_at     TEXT NOT NULL,
        revoked        INTEGER NOT NULL DEFAULT 0
    )",
)?;
```

- [ ] **Step 5: Extend `rebuild_graph` to fold + write `grants`**

In `rebuild_graph` (log.rs:1807): after the `let pages = crate::graph::fold_pages(&page_events);` line (1836), add:

```rust
// Fold grant/revoke events → current grants projection (M5a).
let grant_events = self.grant_events_ordered()?;
let grants = crate::graph::fold_grants(&grant_events);
```

Inside the write tx, after `tx.execute("DELETE FROM pages", [])?;` (1865), add `tx.execute("DELETE FROM grants", [])?;`. After the `for p in &pages { … }` insert loop (ends ~1904), add:

```rust
for g in &grants {
    tx.execute(
        "INSERT INTO grants (canonical_root, granted_at, revoked) VALUES (?1, ?2, ?3)",
        rusqlite::params![g.canonical_root, g.granted_at, g.revoked as i64],
    )?;
}
```

Add the reader helper near `page_and_supersede_events_ordered` (mirror its shape):

```rust
/// All `grant`/`revoke` events, payload-parsed, in chain (`seq ASC`) order.
fn grant_events_ordered(&self) -> Result<Vec<Event>, BossclawError> {
    let store = self.inner.lock().expect(POISON);
    let conn = store.conn();
    let mut stmt = conn.prepare(
        "SELECT payload FROM events WHERE event_type IN ('grant', 'revoke') ORDER BY seq ASC",
    )?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(serde_json::from_str(&row?)?);
    }
    Ok(out)
}
```

- [ ] **Step 6: Add `add_grant` / `revoke_grant` / `grants` to `EventLog`**

Add a new `impl EventLog` section in `log.rs` (or near `current_pages`). `add_grant`/`revoke_grant` append a ground-truth event then `rebuild_graph()` so the projection is immediately current:

```rust
/// Grant read-access to a folder (M5a). Canonicalizes `path`, appends a
/// ground-truth `grant` event, and refreshes the grants projection. Returns the
/// event id. Canonicalization fails closed if the path does not exist.
pub fn add_grant(&self, path: &std::path::Path) -> Result<String, BossclawError> {
    let canonical = std::fs::canonicalize(path)
        .map_err(|e| BossclawError::InvalidInput(format!("grant path not resolvable: {e}")))?;
    let root = canonical.to_string_lossy().to_string();
    let id = self.append(Event {
        id: String::new(), ts: String::new(), valid_time: None,
        event_type: crate::graph::GRANT_EVENT_TYPE.to_string(),
        content: serde_json::json!({ "canonical_root": root }),
        model_meta: None, prev_hash: String::new(), hash: None,
        signed_by_did: self.signer_did(), signature: None,
    })?;
    self.rebuild_graph()?;
    Ok(id)
}

/// Revoke a previously-granted folder (M5a). Canonicalizes `path`, appends a
/// ground-truth `revoke` event, and refreshes the grants projection. Ingested
/// files under a revoked root stay in the log but are excluded from recall.
pub fn revoke_grant(&self, path: &std::path::Path) -> Result<String, BossclawError> {
    let canonical = std::fs::canonicalize(path)
        .map_err(|e| BossclawError::InvalidInput(format!("revoke path not resolvable: {e}")))?;
    let root = canonical.to_string_lossy().to_string();
    let id = self.append(Event {
        id: String::new(), ts: String::new(), valid_time: None,
        event_type: crate::graph::REVOKE_EVENT_TYPE.to_string(),
        content: serde_json::json!({ "canonical_root": root }),
        model_meta: None, prev_hash: String::new(), hash: None,
        signed_by_did: self.signer_did(), signature: None,
    })?;
    self.rebuild_graph()?;
    Ok(id)
}

/// Every grant (active and revoked), `ORDER BY canonical_root ASC`. Tier-A read.
pub fn grants(&self) -> Result<Vec<crate::graph::Grant>, BossclawError> {
    let store = self.inner.lock().expect(POISON);
    let conn = store.conn();
    let mut stmt = conn.prepare(
        "SELECT canonical_root, granted_at, revoked FROM grants ORDER BY canonical_root ASC",
    )?;
    let rows = stmt.query_map([], |r| Ok(crate::graph::Grant {
        canonical_root: r.get(0)?, granted_at: r.get(1)?, revoked: r.get::<_, i64>(2)? != 0,
    }))?;
    let mut out = Vec::new();
    for row in rows { out.push(row?); }
    Ok(out)
}
```

- [ ] **Step 7: Write the failing log-level test (persist + survive reopen + revoke)**

Add to `log.rs` tests:

```rust
#[test]
fn grants_persist_revoke_and_survive_reopen() {
    let dir = tempfile::tempdir().unwrap();
    // A real folder to canonicalize.
    let folder = dir.path().join("notes");
    std::fs::create_dir(&folder).unwrap();
    {
        let log = open_log(dir.path());
        log.add_grant(&folder).unwrap();
        let g = log.grants().unwrap();
        assert_eq!(g.len(), 1);
        assert!(!g[0].revoked);
        log.revoke_grant(&folder).unwrap();
        assert!(log.grants().unwrap()[0].revoked, "revoke marks the row");
    }
    // Reopen: grants are a fold over events, so they rebuild from the log.
    let log2 = open_log(dir.path());
    log2.rebuild_graph().unwrap();
    let g = log2.grants().unwrap();
    assert_eq!(g.len(), 1, "grant survives reopen via replay");
    assert!(g[0].revoked, "revoked state survives reopen");
}
```

- [ ] **Step 8: Run → expect PASS; run the fold tests too**

Run: `cargo test -p bossclaw-core fold_grants grants_persist -- --nocolor`
Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add crates/bossclaw-core/src/graph.rs crates/bossclaw-core/src/log.rs
git commit -m "feat(bossclaw-core): M5a Task 2 — grants projection (fold + schema + add/revoke API)"
```

---

### Task 3: `files` projection — `FileRecord`, `fold_files`, rebuild wiring, readers

**Files:**
- Modify: `crates/bossclaw-core/src/graph.rs` (`FileRecord`, `parse_file_ingested_content`, `fold_files`)
- Modify: `crates/bossclaw-core/src/log.rs` (`files` table; fold+write in `rebuild_graph`; `current_files`/`current_file_for_path`/`current_files_active`)
- Test: inline in both

- [ ] **Step 1: Write the failing `fold_files` test in `graph.rs` tests**

```rust
fn mk_file_ingested(path: &str, content_hash: &str, grant_root: &str) -> Event {
    Event {
        id: String::new(), ts: String::new(), valid_time: None,
        event_type: FILE_INGESTED_EVENT_TYPE.to_string(),
        content: serde_json::json!({
            "text": "body", "origin": EXTERNAL_ORIGIN,
            "provenance": { "canonical_path": path, "content_hash": content_hash, "grant_root": grant_root }
        }),
        model_meta: None, prev_hash: String::new(), hash: None,
        signed_by_did: "did:wba:AIR-TEST".to_string(), signature: None,
    }
}
fn mk_file_supersede(prior_id: &str) -> Event {
    Event {
        id: String::new(), ts: String::new(), valid_time: None,
        event_type: SUPERSEDE_EVENT_TYPE.to_string(),
        content: serde_json::json!({ "supersedes": prior_id }),
        model_meta: None, prev_hash: String::new(), hash: None,
        signed_by_did: "did:wba:AIR-TEST".to_string(), signature: None,
    }
}

#[test]
fn fold_files_keeps_latest_unsuperseded_per_path() {
    // /a v1 (id "f1"), then supersede f1 + /a v2 ("f2"); /b once ("f3").
    let mut v1 = mk_file_ingested("/a", "hashA1", "/root"); v1.id = "f1".to_string();
    let sup = mk_file_supersede("f1");
    let mut v2 = mk_file_ingested("/a", "hashA2", "/root"); v2.id = "f2".to_string();
    let mut b = mk_file_ingested("/b", "hashB", "/root"); b.id = "f3".to_string();
    let mut files = fold_files(&[v1, sup, v2, b]);
    files.sort_by(|x, y| x.canonical_path.cmp(&y.canonical_path));
    assert_eq!(files.len(), 2, "two distinct paths");
    assert_eq!(files[0].canonical_path, "/a");
    assert_eq!(files[0].file_event_id, "f2", "v2 is current; v1 superseded");
    assert_eq!(files[0].content_hash, "hashA2");
    assert_eq!(files[1].file_event_id, "f3");
}

#[test]
fn fold_files_cross_path_identical_bytes_both_current() {
    let mut a = mk_file_ingested("/a", "same", "/root"); a.id = "fa".to_string();
    let mut b = mk_file_ingested("/b", "same", "/root"); b.id = "fb".to_string();
    let files = fold_files(&[a, b]);
    assert_eq!(files.len(), 2, "identical bytes at two paths → both kept (dedup is per-path)");
}

// Proves the ground-truth-supersede REUSE is safe: `SUPERSEDE_EVENT_TYPE` is
// shared by pages and files, but a supersede targets exactly one id, and page
// ids never collide with file ids — so neither fold cross-retires the other's.
#[test]
fn supersede_does_not_cross_retire_between_pages_and_files() {
    let mut page = Event {
        id: "P1".to_string(), ts: String::new(), valid_time: None,
        event_type: PAGE_EVENT_TYPE.to_string(),
        content: serde_json::json!({ "topic_id": "t", "title": "T", "text": "p" }),
        model_meta: None, prev_hash: String::new(), hash: None,
        signed_by_did: "did:wba:AIR-TEST".to_string(), signature: None,
    };
    let mut file = mk_file_ingested("/a", "h", "/root"); file.id = "F1".to_string();

    // A supersede targeting the FILE id retires the file, NOT the page.
    let sup_file = mk_file_supersede("F1");
    assert_eq!(fold_pages(&[page.clone(), file.clone(), sup_file.clone()]).len(), 1,
        "a file-supersede must not retire a page");
    assert!(fold_files(&[page.clone(), file.clone(), sup_file]).is_empty(),
        "the file-supersede retired its own file");

    // A supersede targeting the PAGE id retires the page, NOT the file.
    let sup_page = mk_file_supersede("P1");
    assert_eq!(fold_files(&[page.clone(), file.clone(), sup_page.clone()]).len(), 1,
        "a page-supersede must not retire a file");
    assert!(fold_pages(&[page, file, sup_page]).is_empty(),
        "the page-supersede retired its own page");
}
```

- [ ] **Step 2: Run → expect FAIL**

Run: `cargo test -p bossclaw-core fold_files -- --nocolor`
Expected: FAIL (`cannot find function fold_files`).

- [ ] **Step 3: Implement `FileRecord`, `parse_file_ingested_content`, `fold_files` in `graph.rs`**

```rust
/// A folded ingested-file record (M5a): the CURRENT (un-superseded) `file_ingested`
/// event for one `canonical_path`. A deterministic fold over `file_ingested` +
/// `supersede` events; rebuilt by `rebuild_graph`. Keyed on path, NOT on bytes:
/// identical bytes at two paths yield two records (dedup is per-path).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRecord {
    /// The canonicalized absolute file path (the record's identity key).
    pub canonical_path: String,
    /// The current `file_ingested` event's ULID.
    pub file_event_id: String,
    /// sha256 (hex) of the file BYTES — the dedup identity key.
    pub content_hash: String,
    /// The grant root this file was ingested under (for revoke-aware recall).
    pub grant_root: String,
}

/// Extract `(canonical_path, content_hash, grant_root)` from a `file_ingested`
/// event's content, or `None` if malformed.
fn parse_file_ingested_content(content: &serde_json::Value) -> Option<(String, String, String)> {
    let p = content.get("provenance")?;
    Some((
        p.get("canonical_path")?.as_str()?.to_string(),
        p.get("content_hash")?.as_str()?.to_string(),
        p.get("grant_root")?.as_str()?.to_string(),
    ))
}

/// Fold `file_ingested` + `supersede` events (MUST be in `seq` order) into the
/// current file per path. A `supersede{supersedes: F}` retires `file_ingested` F;
/// walking in seq order, the last non-superseded `file_ingested` per
/// `canonical_path` wins. Mirrors [`fold_pages`] but keyed on path. A `supersede`
/// whose target is a page id is harmless here (no `file_ingested` has that id).
pub fn fold_files(events: &[Event]) -> Vec<FileRecord> {
    use std::collections::{BTreeMap, HashSet};
    let mut superseded: HashSet<String> = HashSet::new();
    for ev in events {
        if ev.event_type == SUPERSEDE_EVENT_TYPE {
            if let Some(s) = ev.content.get("supersedes").and_then(|v| v.as_str()) {
                superseded.insert(s.to_string());
            }
        }
    }
    let mut by_path: BTreeMap<String, FileRecord> = BTreeMap::new();
    for ev in events {
        if ev.event_type != FILE_INGESTED_EVENT_TYPE || superseded.contains(&ev.id) {
            continue;
        }
        if let Some((canonical_path, content_hash, grant_root)) = parse_file_ingested_content(&ev.content) {
            by_path.insert(canonical_path.clone(), FileRecord {
                canonical_path, file_event_id: ev.id.clone(), content_hash, grant_root,
            });
        }
    }
    by_path.into_values().collect()
}
```

- [ ] **Step 4: Run the fold tests → expect PASS**

Run: `cargo test -p bossclaw-core fold_files -- --nocolor`
Expected: PASS.

- [ ] **Step 5: Create the `files` table in `EventLog::open`**

After the `grants` table block (Task 2 Step 4), add:

```rust
// Ingested-file projection (Tier-A; M5a). At most one CURRENT file_ingested per
// canonical_path; a deterministic fold over file_ingested/supersede events,
// rebuilt by `rebuild_graph`. `content_hash` is the dedup key; `grant_root` lets
// recall exclude files under a now-revoked grant.
store.exec(
    "CREATE TABLE IF NOT EXISTS files (
        canonical_path TEXT PRIMARY KEY,
        file_event_id  TEXT NOT NULL,
        content_hash   TEXT NOT NULL,
        grant_root     TEXT NOT NULL
    )",
)?;
```

- [ ] **Step 6: Extend `rebuild_graph` to fold + write `files`**

After the grants fold (Task 2 Step 5), add:

```rust
// Fold file_ingested/supersede events → current file per path (M5a).
let file_events = self.file_and_supersede_events_ordered()?;
let files = crate::graph::fold_files(&file_events);
```

In the write tx, add `tx.execute("DELETE FROM files", [])?;` (next to the grants delete), and after the grants insert loop:

```rust
for f in &files {
    tx.execute(
        "INSERT INTO files (canonical_path, file_event_id, content_hash, grant_root)
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![f.canonical_path, f.file_event_id, f.content_hash, f.grant_root],
    )?;
}
```

Add the reader (mirrors `grant_events_ordered`; includes `supersede` so the fold sees retirements):

```rust
/// All `file_ingested`/`supersede` events, payload-parsed, in chain (`seq ASC`)
/// order. (Page supersedes are included but harmless — `fold_files` only retires
/// `file_ingested` ids.)
fn file_and_supersede_events_ordered(&self) -> Result<Vec<Event>, BossclawError> {
    let store = self.inner.lock().expect(POISON);
    let conn = store.conn();
    let mut stmt = conn.prepare(
        "SELECT payload FROM events WHERE event_type IN ('file_ingested', 'supersede') ORDER BY seq ASC",
    )?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    let mut out = Vec::new();
    for row in rows { out.push(serde_json::from_str(&row?)?); }
    Ok(out)
}
```

- [ ] **Step 7: Add the `files` readers to `EventLog`**

```rust
/// Every CURRENT file (one per path), `ORDER BY canonical_path ASC`. Tier-A read.
pub fn current_files(&self) -> Result<Vec<crate::graph::FileRecord>, BossclawError> {
    let store = self.inner.lock().expect(POISON);
    let conn = store.conn();
    let mut stmt = conn.prepare(
        "SELECT canonical_path, file_event_id, content_hash, grant_root FROM files ORDER BY canonical_path ASC",
    )?;
    let rows = stmt.query_map([], |r| Ok(crate::graph::FileRecord {
        canonical_path: r.get(0)?, file_event_id: r.get(1)?, content_hash: r.get(2)?, grant_root: r.get(3)?,
    }))?;
    let mut out = Vec::new();
    for row in rows { out.push(row?); }
    Ok(out)
}

/// The CURRENT file record for `canonical_path`, or `None`. The dedup-decision
/// lookup used by ingest.
pub(crate) fn current_file_for_path(&self, canonical_path: &str) -> Result<Option<crate::graph::FileRecord>, BossclawError> {
    let store = self.inner.lock().expect(POISON);
    let conn = store.conn();
    let row = conn.query_row(
        "SELECT canonical_path, file_event_id, content_hash, grant_root FROM files WHERE canonical_path = ?1",
        rusqlite::params![canonical_path],
        |r| Ok(crate::graph::FileRecord {
            canonical_path: r.get(0)?, file_event_id: r.get(1)?, content_hash: r.get(2)?, grant_root: r.get(3)?,
        }),
    ).optional()?;
    Ok(row)
}

/// Event ids of CURRENT files whose grant root is still ACTIVE (revoked = 0).
/// Used by recall to drop stale-version AND revoked-grant file hits.
pub(crate) fn current_files_active(&self) -> Result<std::collections::HashSet<String>, BossclawError> {
    let store = self.inner.lock().expect(POISON);
    let conn = store.conn();
    let mut stmt = conn.prepare(
        "SELECT f.file_event_id FROM files f \
         JOIN grants g ON g.canonical_root = f.grant_root \
         WHERE g.revoked = 0",
    )?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    let mut out = std::collections::HashSet::new();
    for row in rows { out.insert(row?); }
    Ok(out)
}
```

> Note: `optional()` comes from `rusqlite::OptionalExtension` (already imported in `log.rs` — `verify_chain_since` uses it). If a compile error says otherwise, add `use rusqlite::OptionalExtension;` at the top of the file.

- [ ] **Step 8: Write the failing log test (projection rebuilds + path lookup)**

```rust
#[test]
fn files_projection_rebuilds_and_path_lookup_works() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    // Append two file_ingested events by hand (ingest_grant lands in Task 7).
    let mut v1 = Event {
        id: String::new(), ts: String::new(), valid_time: None,
        event_type: crate::graph::FILE_INGESTED_EVENT_TYPE.to_string(),
        content: serde_json::json!({
            "text": "hello", "origin": crate::graph::EXTERNAL_ORIGIN,
            "provenance": { "canonical_path": "/x/a.md", "content_hash": "h1", "grant_root": "/x" }
        }),
        model_meta: None, prev_hash: String::new(), hash: None,
        signed_by_did: ENGINE_SIGNER_DID.to_string(), signature: None,
    };
    let id1 = log.append(v1.clone()).unwrap();
    log.rebuild_graph().unwrap();
    let rec = log.current_file_for_path("/x/a.md").unwrap().expect("present");
    assert_eq!(rec.file_event_id, id1);
    assert_eq!(rec.content_hash, "h1");
    assert!(log.current_file_for_path("/x/missing.md").unwrap().is_none());
}
```

- [ ] **Step 9: Run → expect PASS**

Run: `cargo test -p bossclaw-core files_projection -- --nocolor`
Expected: PASS.

- [ ] **Step 10: Commit**

```bash
git add crates/bossclaw-core/src/graph.rs crates/bossclaw-core/src/log.rs
git commit -m "feat(bossclaw-core): M5a Task 3 — files projection (fold + schema + path/active readers)"
```

---

### Task 4: `Parser` seam — `PathHint`, `IngestError`, `NativeTextParser`, `MockParser`

**Files:**
- Create: `crates/bossclaw-core/src/ingest.rs`
- Modify: `crates/bossclaw-core/src/lib.rs` (`pub mod ingest;` + re-exports)
- Test: inline in `ingest.rs`

- [ ] **Step 1: Create `ingest.rs` with the seam types (parser half only)**

```rust
//! M5a ingest pipeline: read-only ingest of user-granted folders into the signed
//! log as recallable, externally-tainted `file_ingested` events.
//!
//! Safety model (spec §6): kernel-enforced containment via an `openat`-fd-chain
//! walk with `O_NOFOLLOW` on every descent + a per-OS careful final open; a
//! never-touch hazard-reduction filter; per-path dedup/version-supersede; and the
//! taint root (`origin: "external"` inside signed content). NO subprocess, NO
//! `unsafe` (rustix encapsulates the syscalls); rich formats (PDF/docx) are M5b.

use std::path::PathBuf;

/// A sanitized type hint for parser dispatch (spec §4). Carries the lowercased
/// file extension ONLY — never a resolvable path — so a `Parser` can never
/// re-resolve or escape the contained read the orchestrator already performed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PathHint {
    /// Lowercased extension without the dot (e.g. `"md"`), or `None` if absent.
    pub ext: Option<String>,
}

/// Why one file could not be ingested. Per-file and best-effort: these become a
/// `(path, reason)` entry in [`IngestReport`], never a hard failure of the run.
#[derive(Debug)]
pub enum IngestError {
    /// The bytes are not valid UTF-8 (M5a parses text/markdown only; rich
    /// formats wait for M5b's sandboxed parser).
    NonUtf8,
    /// The careful open refused the file (symlink / escape / TOCTOU swap), or a
    /// containment invariant failed. The file is dropped (fail closed).
    Containment(String),
    /// The file exceeded the byte cap (skipped, not truncated).
    TooLarge,
    /// A parser-internal conversion error (reserved for M5b).
    Parse(String),
    /// An OS error while reading the contained handle.
    Io(String),
}

impl std::fmt::Display for IngestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IngestError::NonUtf8 => write!(f, "not valid UTF-8 (rich formats are M5b)"),
            IngestError::Containment(m) => write!(f, "containment refused: {m}"),
            IngestError::TooLarge => write!(f, "exceeds byte cap"),
            IngestError::Parse(m) => write!(f, "parse error: {m}"),
            IngestError::Io(m) => write!(f, "io error: {m}"),
        }
    }
}

/// The pluggable converter (spec §4 / D2). Takes already-read, contained bytes +
/// a sanitized hint and returns text. M5a ships [`NativeTextParser`] (UTF-8) and
/// [`MockParser`]; M5b adds a sandboxed-`markitdown` impl behind a feature.
pub trait Parser: Send + Sync {
    /// Convert `raw` bytes to text, or a per-file [`IngestError`].
    fn convert(&self, raw: &[u8], hint: &PathHint) -> Result<String, IngestError>;
    /// Stable id stamped into `file_ingested` provenance (`parser_id`).
    fn parser_id(&self) -> &str;
}

/// The M5a native parser: in-process strict UTF-8 decode. Non-UTF-8 bytes (most
/// binary formats) → [`IngestError::NonUtf8`] (skipped). The `hint` is unused in
/// M5a (any valid-UTF-8 file is text); M5b's parser dispatches on it.
pub struct NativeTextParser;

impl Parser for NativeTextParser {
    fn convert(&self, raw: &[u8], _hint: &PathHint) -> Result<String, IngestError> {
        std::str::from_utf8(raw).map(|s| s.to_string()).map_err(|_| IngestError::NonUtf8)
    }
    fn parser_id(&self) -> &str { "native-text-v1" }
}

/// A test double that returns a fixed string regardless of input.
#[cfg(test)]
pub struct MockParser {
    /// The text every `convert` call returns.
    pub output: String,
}

#[cfg(test)]
impl Parser for MockParser {
    fn convert(&self, _raw: &[u8], _hint: &PathHint) -> Result<String, IngestError> {
        Ok(self.output.clone())
    }
    fn parser_id(&self) -> &str { "mock" }
}

/// Best-effort accounting of one ingest run (spec §4). LOUD by design: callers
/// surface `skipped`/`failed` to the user (e.g. "N files matched the never-touch
/// filter"). `superseded` counts files whose content changed since last ingest.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct IngestReport {
    /// New files appended this run.
    pub ingested: usize,
    /// Changed files whose prior version was superseded this run.
    pub superseded: usize,
    /// Unchanged files (same path + same content hash) — no-op.
    pub deduped: usize,
    /// Files intentionally not ingested, with reason (never-touch, non-UTF-8,
    /// oversize, wall-clock budget, …).
    pub skipped: Vec<(PathBuf, String)>,
    /// Files dropped due to a safety/IO error, with reason (containment, io, …).
    pub failed: Vec<(PathBuf, String)>,
}
```

- [ ] **Step 2: Wire the module into `lib.rs`**

Add `pub mod ingest;` (alphabetically, after `pub mod index;`) and the re-export near the others (after the `recall` re-export):

```rust
pub use ingest::{IngestReport, NativeTextParser, Parser, PathHint};
```

- [ ] **Step 3: Write the failing parser tests (append to `ingest.rs`)**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_parser_reads_utf8_text() {
        let p = NativeTextParser;
        let out = p.convert("# Title\nbody".as_bytes(), &PathHint { ext: Some("md".into()) }).unwrap();
        assert_eq!(out, "# Title\nbody");
        assert_eq!(p.parser_id(), "native-text-v1");
    }

    #[test]
    fn native_parser_rejects_non_utf8_as_nonutf8() {
        let p = NativeTextParser;
        // 0xFF 0xFE is not valid UTF-8.
        let err = p.convert(&[0xFF, 0xFE, 0x00], &PathHint::default()).unwrap_err();
        assert!(matches!(err, IngestError::NonUtf8));
    }

    #[test]
    fn path_hint_carries_no_path() {
        // Compile-time guarantee: PathHint has exactly one field, the extension.
        let h = PathHint { ext: Some("txt".into()) };
        assert_eq!(h.ext.as_deref(), Some("txt"));
    }
}
```

- [ ] **Step 4: Run → expect PASS; build**

Run: `cargo test -p bossclaw-core ingest::tests -- --nocolor && cargo build -p bossclaw-core`
Expected: PASS + clean build.

- [ ] **Step 5: Commit**

```bash
git add crates/bossclaw-core/src/ingest.rs crates/bossclaw-core/src/lib.rs
git commit -m "feat(bossclaw-core): M5a Task 4 — Parser seam (PathHint, IngestError, NativeTextParser, MockParser)"
```

---

### Task 5: Careful open — per-OS containment + `ContainedFile` + the TOCTOU swap test

This is the security core. The `openat`-fd-chain (Task 6's walk) holds a real parent-directory fd reached via `O_NOFOLLOW` descents; the careful **final** open opens the file from that fd, refusing any symlink swap.

**Files:**
- Modify: `crates/bossclaw-core/src/ingest.rs`
- Test: inline in `ingest.rs`

- [ ] **Step 1: Add `ContainedFile` + `FileIdentity` + the careful open functions**

```rust
use std::io::Read;

/// A per-run identity for hardlink/inode dedup. On Unix this is `(dev, ino)`; on
/// Windows (where rustix has no `openat`) it falls back to the canonical path —
/// a documented weaker guarantee (hardlinks are not deduped on Windows).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum FileIdentity {
    /// Unix `(st_dev, st_ino)`.
    DevIno(u64, u64),
    /// Windows canonical-path fallback.
    Path(PathBuf),
}

/// A file the careful open proved is contained beneath the grant root with no
/// symlink traversal. The orchestrator reads its bytes ONCE (owning identity
/// hashing); the `Parser` never sees this handle or any path.
pub(crate) struct ContainedFile {
    file: std::fs::File,
    identity: FileIdentity,
    size: u64,
}

impl ContainedFile {
    pub(crate) fn identity(&self) -> &FileIdentity { &self.identity }
    pub(crate) fn size(&self) -> u64 { self.size }

    /// Read up to `cap` bytes. Returns [`IngestError::TooLarge`] if the file has
    /// more than `cap` bytes (read `cap + 1` and check) — never a truncated body.
    pub(crate) fn read_all_capped(mut self, cap: usize) -> Result<Vec<u8>, IngestError> {
        let mut buf = Vec::with_capacity(self.size.min(cap as u64) as usize);
        let read = (&mut self.file)
            .take(cap as u64 + 1)
            .read_to_end(&mut buf)
            .map_err(|e| IngestError::Io(e.to_string()))?;
        if read > cap {
            return Err(IngestError::TooLarge);
        }
        Ok(buf)
    }
}

// ── Unix: open the final file from a parent dir fd with O_NOFOLLOW. The fd-chain
//    walk (Task 6) reached `dir_fd` via O_NOFOLLOW descents, so a NOFOLLOW open
//    here refuses a final-component symlink AND a dir swapped to a symlink after
//    readdir named it (TOCTOU). On Linux we additionally use openat2 with
//    RESOLVE_BENEATH|RESOLVE_NO_SYMLINKS (spec D3) when the kernel supports it. ──
#[cfg(unix)]
pub(crate) fn careful_open_file(
    dir_fd: &std::os::fd::OwnedFd,
    name: &std::ffi::OsStr,
) -> Result<ContainedFile, IngestError> {
    use rustix::fs::{Mode, OFlags};
    use std::os::unix::ffi::OsStrExt;

    #[cfg(target_os = "linux")]
    let owned = {
        use rustix::fs::{openat2, ResolveFlags};
        match openat2(
            dir_fd,
            name.as_bytes(),
            OFlags::RDONLY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
        ) {
            Ok(fd) => fd,
            // Pre-5.6 kernels lack openat2 → fall back to the NOFOLLOW open, which
            // still refuses a final-component symlink (the chain gave containment).
            Err(rustix::io::Errno::NOSYS) => rustix::fs::openat(
                dir_fd, name.as_bytes(), OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty(),
            ).map_err(|e| IngestError::Containment(e.to_string()))?,
            Err(e) => return Err(IngestError::Containment(e.to_string())),
        }
    };
    #[cfg(not(target_os = "linux"))]
    let owned = rustix::fs::openat(
        dir_fd, name.as_bytes(), OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty(),
    ).map_err(|e| IngestError::Containment(e.to_string()))?;

    let st = rustix::fs::fstat(&owned).map_err(|e| IngestError::Io(e.to_string()))?;
    // Reject anything that is not a regular file (fifos, devices, dirs).
    if rustix::fs::FileType::from_raw_mode(st.st_mode) != rustix::fs::FileType::RegularFile {
        return Err(IngestError::Containment("not a regular file".into()));
    }
    Ok(ContainedFile {
        file: std::fs::File::from(owned),
        identity: FileIdentity::DevIno(st.st_dev as u64, st.st_ino as u64),
        size: st.st_size as u64,
    })
}

// ── Windows: no openat. Canonicalize, assert containment under the grant root,
//    and reject reparse points (symlinks/junctions). Final-component-strong;
//    the intermediate-dir swap race is a documented residual (spec §6.1, D3). ──
#[cfg(windows)]
pub(crate) fn careful_open_windows(
    grant_root: &std::path::Path,
    candidate: &std::path::Path,
) -> Result<ContainedFile, IngestError> {
    let meta = std::fs::symlink_metadata(candidate).map_err(|e| IngestError::Io(e.to_string()))?;
    if meta.file_type().is_symlink() {
        return Err(IngestError::Containment("reparse point / symlink rejected".into()));
    }
    let canonical = std::fs::canonicalize(candidate).map_err(|e| IngestError::Io(e.to_string()))?;
    let root_canonical = std::fs::canonicalize(grant_root).map_err(|e| IngestError::Io(e.to_string()))?;
    if !canonical.starts_with(&root_canonical) {
        return Err(IngestError::Containment("escapes grant root".into()));
    }
    // Open first, then re-check type on the OPENED handle (NOT the pre-open
    // `symlink_metadata`) so a file→dir swap between canonicalize and open cannot
    // slip a non-regular target through.
    let file = std::fs::File::open(&canonical).map_err(|e| IngestError::Io(e.to_string()))?;
    let opened = file.metadata().map_err(|e| IngestError::Io(e.to_string()))?;
    if !opened.file_type().is_file() {
        return Err(IngestError::Containment("not a regular file".into()));
    }
    Ok(ContainedFile { file, identity: FileIdentity::Path(canonical), size: opened.len() })
}
```

> **`openat2`/`ResolveFlags`/`FileType::from_raw_mode` API check:** these are rustix 0.38 `fs` APIs. If a signature differs at compile time, fix to match `cargo doc -p rustix` — the TDD loop surfaces it immediately. The required behavior (kernel refuses symlink traversal + escape) is the contract; the exact call is mechanical.

> **Containment strength + CI matrix (second-opinion reconciliation):** the Unix walk holds a *real directory fd* for each level, reached via per-descent `O_NOFOLLOW`; an already-open ancestor fd is immune to a later name swap, and the final careful open's `NOFOLLOW`/`openat2 RESOLVE_NO_SYMLINKS` refuses a swapped-in symlink. So **macOS is as symlink-swap-tight as Linux here** — the spec's earlier "macOS intermediate-dir residual" was over-conservative (Rev 3 §10 corrects it). The genuine residuals are **Windows** (non-atomic `canonicalize`→`open`) and **hardlink-into-grant** (Deferred/Risks). Because the two careful-open `cfg` branches are *different code*, the containment tests MUST run on **both macOS and Linux** in CI (Task 11), and the Linux swap test SHOULD assert the refusal is `ELOOP`-class so it's `RESOLVE_NO_SYMLINKS` that fired, not an accidental `ENOENT` (add `#[cfg(target_os = "linux")]` checking the `Containment(msg)` string contains the errno).

- [ ] **Step 2: Write the failing containment tests (Unix; the TOCTOU swap is the key one)**

```rust
#[cfg(unix)]
mod containment_tests {
    use super::*;
    use rustix::fs::{Mode, OFlags};
    use std::os::unix::ffi::OsStrExt;

    // Open a directory as a NOFOLLOW dir fd (what the walk does).
    fn open_dir(path: &std::path::Path) -> std::os::fd::OwnedFd {
        rustix::fs::openat(
            rustix::fs::CWD, path.as_os_str().as_bytes(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty(),
        ).unwrap()
    }

    #[test]
    fn careful_open_reads_a_contained_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"hello").unwrap();
        let dfd = open_dir(dir.path());
        let cf = careful_open_file(&dfd, std::ffi::OsStr::new("a.txt")).unwrap();
        assert_eq!(cf.read_all_capped(1024).unwrap(), b"hello");
    }

    #[test]
    fn careful_open_refuses_a_symlink_final_component() {
        let dir = tempfile::tempdir().unwrap();
        let secret = dir.path().join("secret");
        std::fs::write(&secret, b"TOP SECRET").unwrap();
        std::os::unix::fs::symlink(&secret, dir.path().join("link")).unwrap();
        let dfd = open_dir(dir.path());
        let err = careful_open_file(&dfd, std::ffi::OsStr::new("link")).unwrap_err();
        assert!(matches!(err, IngestError::Containment(_)), "a symlink must be refused, got {err:?}");
    }

    // The TOCTOU swap: a name resolves to a real file at readdir time, then is
    // swapped to a symlink BEFORE the open. NOFOLLOW (and openat2 RESOLVE_NO_SYMLINKS)
    // must refuse — proving the open is hardened, which a static-symlink test does not.
    #[test]
    fn careful_open_refuses_a_post_readdir_symlink_swap() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real.txt");
        std::fs::write(&target, b"ok").unwrap();
        let outside = dir.path().join("outside_secret");
        std::fs::write(&outside, b"SECRET").unwrap();
        let dfd = open_dir(dir.path());
        // Simulate the race: between "the walk saw real.txt" and the open, an
        // attacker replaces real.txt with a symlink pointing outside.
        std::fs::remove_file(&target).unwrap();
        std::os::unix::fs::symlink(&outside, &target).unwrap();
        let err = careful_open_file(&dfd, std::ffi::OsStr::new("real.txt")).unwrap_err();
        assert!(matches!(err, IngestError::Containment(_)), "swapped-in symlink must be refused, got {err:?}");
    }

    #[test]
    fn read_all_capped_rejects_oversize() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("big.txt"), vec![b'x'; 100]).unwrap();
        let dfd = open_dir(dir.path());
        let cf = careful_open_file(&dfd, std::ffi::OsStr::new("big.txt")).unwrap();
        assert!(matches!(cf.read_all_capped(10), Err(IngestError::TooLarge)));
    }
}
```

- [ ] **Step 3: Run → expect PASS on macOS (dev machine) and Linux (CI)**

Run: `cargo test -p bossclaw-core containment_tests -- --nocolor`
Expected: PASS (4 tests). The swap test proves `O_NOFOLLOW`/`openat2` hardening, not just static-symlink rejection.

- [ ] **Step 4: Commit**

```bash
git add crates/bossclaw-core/src/ingest.rs
git commit -m "feat(bossclaw-core): M5a Task 5 — per-OS careful open + ContainedFile + TOCTOU swap test"
```

---

### Task 6: Safe walk — never-touch filter, `openat`-fd-chain, inode-seen, caps

**Files:**
- Modify: `crates/bossclaw-core/src/ingest.rs`
- Test: inline in `ingest.rs`

- [ ] **Step 1: Add the caps + never-touch consts**

```rust
use std::time::{Duration, Instant};

/// Max bytes read per file. Files larger than this are skipped (recorded), not
/// truncated — a partial body would corrupt content_hash + recall. 10 MiB covers
/// notes/markdown/code; rich/large formats wait for M5b.
const MAX_FILE_BYTES: usize = 10 * 1024 * 1024;
/// Whole-run wall-clock budget (spec §6.2). The walk stops cleanly past this and
/// records a budget skip, so a pathological tree never hangs the engine.
const INGEST_WALL_CLOCK: Duration = Duration::from_secs(300);
/// Max directory nesting depth (defense against pathological/looping trees; the
/// inode-seen set also breaks hardlink loops).
const MAX_WALK_DEPTH: usize = 64;

/// Directory names never descended into (hazard reduction, NOT a containment
/// boundary — the boundary is the grant + informed consent; spec §6.3). Matched
/// **case-insensitively**: the primary platform (macOS/APFS) is case-insensitive,
/// so a case-sensitive filter would let `.SSH` bypass it. Keep these LOWERCASE.
const NEVER_TOUCH_DIRS: &[&str] =
    &[".ssh", ".aws", ".azure", ".gnupg", ".git", ".kube", ".docker", "gcloud"];
/// Exact file names never ingested (LOWERCASE; matched case-insensitively).
const NEVER_TOUCH_FILES: &[&str] = &[
    ".env", ".netrc", ".pgpass", ".git-credentials", "wallet.dat",
    ".npmrc", ".pypirc", ".dockercfg", "known_hosts",
];
/// Glob patterns never ingested. Only two shapes: `*.ext` (suffix) and `prefix*`
/// (prefix). LOWERCASE; matched case-insensitively. Single-sourced + tested.
const NEVER_TOUCH_GLOBS: &[&str] = &[
    "*.key", "*.pem", "*.p12", "*.pfx", "*.gpg", "*.asc", "id_*",
    "*.keychain", "*.kdbx", "*.jks", "*.ppk", "*.mobileconfig", "*.ovpn",
];

/// True if `name_lc` (already lowercased by the caller) matches a `*.ext` (suffix)
/// or `prefix*` (prefix) glob. Patterns are already lowercase.
fn matches_glob(name_lc: &str, pattern: &str) -> bool {
    if let Some(suffix) = pattern.strip_prefix('*') {
        name_lc.ends_with(suffix)
    } else if let Some(prefix) = pattern.strip_suffix('*') {
        name_lc.starts_with(prefix)
    } else {
        name_lc == pattern
    }
}

/// Whether a directory component must never be descended (hazard reduction).
/// Case-insensitive. Also catches the `.config/gh` pair via `rel_dir`.
fn is_never_touch_dir(name: &str, rel_dir: &str) -> bool {
    let name_lc = name.to_lowercase();
    NEVER_TOUCH_DIRS.contains(&name_lc.as_str()) || rel_dir.to_lowercase().ends_with(".config/gh")
}

/// Whether a file component must never be ingested. Case-insensitive.
fn is_never_touch_file(name: &str) -> bool {
    let name_lc = name.to_lowercase();
    NEVER_TOUCH_FILES.contains(&name_lc.as_str()) || NEVER_TOUCH_GLOBS.iter().any(|g| matches_glob(&name_lc, g))
}
```

- [ ] **Step 2: Write the failing filter unit tests**

```rust
#[cfg(test)]
mod filter_tests {
    use super::*;

    #[test]
    fn never_touch_files_and_globs() {
        assert!(is_never_touch_file(".env"));
        assert!(is_never_touch_file("server.key"));
        assert!(is_never_touch_file("id_rsa"));
        assert!(is_never_touch_file("vault.kdbx"));
        assert!(is_never_touch_file("cert.p12"));
        assert!(is_never_touch_file("known_hosts"));
        // Case-insensitive (macOS/APFS): uppercase variants must also match.
        assert!(is_never_touch_file(".ENV"));
        assert!(is_never_touch_file("Server.PEM"));
        assert!(is_never_touch_file("ID_RSA"));
        assert!(!is_never_touch_file("notes.md"));
        assert!(!is_never_touch_file("readme.txt"));
    }

    #[test]
    fn never_touch_dirs_including_config_gh() {
        assert!(is_never_touch_dir(".ssh", "project/.ssh"));
        assert!(is_never_touch_dir(".SSH", "project/.SSH")); // case-insensitive (macOS)
        assert!(is_never_touch_dir(".git", "project/.git"));
        assert!(is_never_touch_dir("gh", "home/.config/gh"));
        assert!(!is_never_touch_dir("src", "project/src"));
    }

    #[test]
    fn glob_shapes() {
        assert!(matches_glob("a.pem", "*.pem"));
        assert!(matches_glob("id_ed25519", "id_*"));
        assert!(!matches_glob("pem.txt", "*.pem"));
    }
}
```

- [ ] **Step 3: Run filter tests → expect PASS** (they only need the consts/helpers)

Run: `cargo test -p bossclaw-core filter_tests -- --nocolor`
Expected: PASS.

- [ ] **Step 4: Implement the walk (Unix `openat`-fd-chain; Windows `read_dir`)**

The walk yields `(ContainedFile, PathBuf canonical_path, PathHint)` for each ingestable regular file, while updating an `inode-seen` set and the report's `skipped` list. It does NOT touch the log.

```rust
/// A file the walk surfaced for ingest: the contained handle + its path + a
/// sanitized hint. `canonical_path` is `grant_root` (already canonicalized) joined
/// with the walk-relative components — safe to treat as canonical because the walk
/// admitted no symlink and no `..`, so it equals `realpath` WITHOUT re-resolving.
pub(crate) struct WalkedFile {
    pub(crate) file: ContainedFile,
    pub(crate) canonical_path: PathBuf,
    pub(crate) hint: PathHint,
}

/// Recursively walk `grant_root` (already canonicalized), invoking `sink` for each
/// ingestable regular file. No-symlink-follow, never-touch-filtered, depth- and
/// wall-clock-bounded, inode-deduped within the run. `report.skipped` accumulates
/// never-touch / oversize / budget skips. Returns early (Ok) when the wall-clock
/// budget is hit (a `<budget>` skip is recorded).
#[cfg(unix)]
pub(crate) fn walk_grant(
    grant_root: &std::path::Path,
    started: Instant,
    seen: &mut std::collections::HashSet<FileIdentity>,
    report: &mut IngestReport,
    mut sink: impl FnMut(WalkedFile) -> Result<(), crate::error::BossclawError>,
) -> Result<(), crate::error::BossclawError> {
    use rustix::fs::{Mode, OFlags};
    use std::os::unix::ffi::OsStrExt;

    let root_fd = rustix::fs::openat(
        rustix::fs::CWD, grant_root.as_os_str().as_bytes(),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty(),
    ).map_err(|e| crate::error::BossclawError::Io(std::io::Error::other(e.to_string())))?;

    // Explicit stack of (dir_fd, rel_dir, depth) so recursion can't blow the
    // native stack; each dir_fd was opened from its parent with O_NOFOLLOW.
    let mut stack: Vec<(std::os::fd::OwnedFd, String, usize)> = vec![(root_fd, String::new(), 0)];

    while let Some((dir_fd, rel_dir, depth)) = stack.pop() {
        if started.elapsed() > INGEST_WALL_CLOCK {
            report.skipped.push((grant_root.join(&rel_dir), "wall-clock budget exceeded".into()));
            return Ok(());
        }
        // Read entries from the dir fd. `Dir` borrows the fd; collect names first.
        let dir = rustix::fs::Dir::read_from(&dir_fd)
            .map_err(|e| crate::error::BossclawError::Io(std::io::Error::other(e.to_string())))?;
        let mut entries: Vec<std::ffi::OsString> = Vec::new();
        for entry in dir {
            let entry = entry.map_err(|e| crate::error::BossclawError::Io(std::io::Error::other(e.to_string())))?;
            let name_bytes = entry.file_name().to_bytes();
            if name_bytes == b"." || name_bytes == b".." {
                continue;
            }
            entries.push(std::ffi::OsStr::from_bytes(name_bytes).to_os_string());
        }

        for name_os in entries {
            let name = name_os.to_string_lossy().to_string();
            let rel_child = if rel_dir.is_empty() { name.clone() } else { format!("{rel_dir}/{name}") };

            // statat with SYMLINK_NOFOLLOW: classify WITHOUT following symlinks.
            let st = match rustix::fs::statat(&dir_fd, name_os.as_bytes(), rustix::fs::AtFlags::SYMLINK_NOFOLLOW) {
                Ok(st) => st,
                Err(e) => { report.skipped.push((grant_root.join(&rel_child), format!("stat failed: {e}"))); continue; }
            };
            let ftype = rustix::fs::FileType::from_raw_mode(st.st_mode);

            if ftype == rustix::fs::FileType::Symlink {
                // No-symlink-follow: silently skip (not an error; expected).
                continue;
            }
            if ftype == rustix::fs::FileType::Directory {
                if is_never_touch_dir(&name, &rel_child) {
                    report.skipped.push((grant_root.join(&rel_child), "never-touch dir".into()));
                    continue;
                }
                if depth + 1 > MAX_WALK_DEPTH {
                    report.skipped.push((grant_root.join(&rel_child), "max depth exceeded".into()));
                    continue;
                }
                let child_fd = match rustix::fs::openat(
                    &dir_fd, name_os.as_bytes(),
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty(),
                ) {
                    Ok(fd) => fd,
                    Err(e) => { report.skipped.push((grant_root.join(&rel_child), format!("dir open refused: {e}"))); continue; }
                };
                stack.push((child_fd, rel_child, depth + 1));
                continue;
            }
            if ftype != rustix::fs::FileType::RegularFile {
                continue; // fifo, socket, device — skip silently
            }

            // Regular file.
            if is_never_touch_file(&name) {
                report.skipped.push((grant_root.join(&rel_child), "never-touch file".into()));
                continue;
            }
            let cf = match careful_open_file(&dir_fd, &name_os) {
                Ok(cf) => cf,
                Err(IngestError::TooLarge) => { report.skipped.push((grant_root.join(&rel_child), "oversize".into())); continue; }
                Err(e) => { report.failed.push((grant_root.join(&rel_child), e.to_string())); continue; }
            };
            if cf.size() > MAX_FILE_BYTES as u64 {
                report.skipped.push((grant_root.join(&rel_child), "oversize".into()));
                continue;
            }
            if !seen.insert(cf.identity().clone()) {
                continue; // same inode already ingested this run (hardlink / overlap)
            }
            let hint = PathHint {
                ext: std::path::Path::new(&name).extension().map(|e| e.to_string_lossy().to_lowercase()),
            };
            sink(WalkedFile { file: cf, canonical_path: grant_root.join(&rel_child), hint })?;
        }
    }
    Ok(())
}
```

> **rustix `Dir::read_from` / `statat` / `AtFlags::SYMLINK_NOFOLLOW` API check:** rustix 0.38 `fs`. `Dir::read_from(fd)` yields `io::Result<DirEntry>`; `DirEntry::file_name()` returns `&CStr` (hence `.to_bytes()`). If a name differs, align to `cargo doc -p rustix`. The contract — enumerate a dir fd, classify entries without following symlinks — is fixed; the spelling is mechanical. `std::io::Error::other` requires Rust ≥ 1.74 (the workspace toolchain; if older, use `std::io::Error::new(std::io::ErrorKind::Other, …)`).

- [ ] **Step 5: Write the failing walk tests**

```rust
#[cfg(all(test, unix))]
mod walk_tests {
    use super::*;

    fn collect(root: &std::path::Path) -> (Vec<String>, IngestReport) {
        let mut report = IngestReport::default();
        let mut seen = std::collections::HashSet::new();
        let mut names = Vec::new();
        walk_grant(root, Instant::now(), &mut seen, &mut report, |wf| {
            names.push(wf.canonical_path.file_name().unwrap().to_string_lossy().to_string());
            Ok(())
        }).unwrap();
        names.sort();
        (names, report)
    }

    #[test]
    fn walk_skips_never_touch_and_symlinks_finds_regular_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("a.md"), b"a").unwrap();
        std::fs::write(root.join(".env"), b"SECRET=1").unwrap();
        std::fs::write(root.join("k.pem"), b"key").unwrap();
        std::fs::create_dir(root.join(".ssh")).unwrap();
        std::fs::write(root.join(".ssh").join("id_rsa"), b"key").unwrap();
        std::os::unix::fs::symlink(root.join("a.md"), root.join("link.md")).unwrap();

        let (names, report) = collect(root);
        assert_eq!(names, vec!["a.md".to_string()], "only the plain file is surfaced");
        // .env + k.pem + the .ssh dir are never-touch skips (the symlink is silent).
        assert!(report.skipped.iter().any(|(_, r)| r == "never-touch file"));
        assert!(report.skipped.iter().any(|(_, r)| r == "never-touch dir"));
    }

    #[test]
    fn walk_dedups_hardlinks_within_a_run() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("orig.txt"), b"data").unwrap();
        std::fs::hard_link(root.join("orig.txt"), root.join("dup.txt")).unwrap();
        let (names, _r) = collect(root);
        assert_eq!(names.len(), 1, "a hardlinked inode is surfaced once per run");
    }
}
```

- [ ] **Step 6: Run → expect PASS**

Run: `cargo test -p bossclaw-core walk_tests filter_tests -- --nocolor`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/bossclaw-core/src/ingest.rs
git commit -m "feat(bossclaw-core): M5a Task 6 — safe openat walk (never-touch, no-symlink, inode-seen, caps)"
```

---

### Task 7: Orchestrator core — dedup/supersede decision, `file_ingested` content, append

**Files:**
- Modify: `crates/bossclaw-core/src/ingest.rs` (`impl EventLog { ingest_grant }` + content builder + `is_external`)
- Test: inline in `ingest.rs`

- [ ] **Step 1: Add the content builder + the taint classifier**

```rust
use crate::event::{Event, ModelMeta};
use crate::log::EventLog;
use sha2::{Digest, Sha256};

/// Build the signed content of a `file_ingested` event (D4). `text` is top-level
/// so `embeddable_text` finds it; `origin` is the taint stamp; everything is
/// inside the signed bytes (JCS canonical + byte-identical rebuild).
fn file_ingested_content(
    text: &str,
    canonical_path: &str,
    raw: &[u8],
    grant_root: &str,
    parser_id: &str,
    modified_at: &str,
) -> serde_json::Value {
    let content_hash = hex::encode(Sha256::digest(raw));
    let text_hash = hex::encode(Sha256::digest(text.as_bytes()));
    serde_json::json!({
        "text": text,
        "origin": crate::graph::EXTERNAL_ORIGIN,
        "provenance": {
            "canonical_path": canonical_path,
            "content_hash": content_hash,
            "text_hash": text_hash,
            "size_bytes": raw.len(),
            "modified_at": modified_at,
            "parser_id": parser_id,
            "grant_root": grant_root,
        }
    })
}

/// True iff `event` is externally-tainted (M5a, D5). The classifier the M6
/// actuator's fail-closed lineage walk will consume; here it is the taint root +
/// a tested predicate. Reads the single-sourced `EXTERNAL_ORIGIN` stamp.
pub fn is_external(event: &Event) -> bool {
    event.content.get("origin").and_then(|v| v.as_str()) == Some(crate::graph::EXTERNAL_ORIGIN)
}
```

- [ ] **Step 2: Add `ingest_grant` (the per-grant orchestrator) in `impl EventLog`**

```rust
impl EventLog {
    /// Ingest one already-granted, canonicalized folder `grant_root`. Walks it
    /// safely, parses each file, and applies the per-path dedup/supersede
    /// decision, appending ground-truth `file_ingested` events (D4). Best-effort:
    /// per-file problems land in the returned [`IngestReport`], not as errors.
    ///
    /// `seen` is the run-wide inode-dedup set (shared across grants by
    /// `ingest_all`). Re-checks the grant is still active before EVERY append so
    /// a concurrent `revoke_grant` stops further writes (spec §7).
    pub(crate) fn ingest_grant_inner(
        &self,
        grant_root: &std::path::Path,
        parser: &dyn Parser,
        embedder: &dyn crate::embed::Embedder,
        started: Instant,
        seen: &mut std::collections::HashSet<FileIdentity>,
        report: &mut IngestReport,
    ) -> Result<(), crate::error::BossclawError> {
        let grant_root_str = grant_root.to_string_lossy().to_string();
        // Collect walked files first (the walk borrows dir fds; appends happen after).
        let mut walked: Vec<WalkedFile> = Vec::new();
        walk_grant(grant_root, started, seen, report, |wf| { walked.push(wf); Ok(()) })?;

        for wf in walked {
            if started.elapsed() > INGEST_WALL_CLOCK {
                report.skipped.push((wf.canonical_path, "wall-clock budget exceeded".into()));
                continue;
            }
            // Re-check the grant is active before doing work (revoke mid-ingest).
            let still_active = self.grants()?.iter().any(|g| g.canonical_root == grant_root_str && !g.revoked);
            if !still_active {
                report.skipped.push((wf.canonical_path, "grant revoked mid-ingest".into()));
                continue;
            }

            let canonical_path = wf.canonical_path.to_string_lossy().to_string();
            let modified_at = file_mtime_rfc3339(&wf.file);
            let raw = match wf.file.read_all_capped(MAX_FILE_BYTES) {
                Ok(b) => b,
                Err(IngestError::TooLarge) => { report.skipped.push((wf.canonical_path, "oversize".into())); continue; }
                Err(e) => { report.failed.push((wf.canonical_path, e.to_string())); continue; }
            };
            let text = match parser.convert(&raw, &wf.hint) {
                Ok(t) => t,
                Err(e @ IngestError::NonUtf8) => { report.skipped.push((wf.canonical_path, e.to_string())); continue; }
                Err(e) => { report.failed.push((wf.canonical_path, e.to_string())); continue; }
            };
            let content = file_ingested_content(&text, &canonical_path, &raw, &grant_root_str, parser.parser_id(), &modified_at);
            let new_hash = content["provenance"]["content_hash"].as_str().unwrap().to_string();

            // ── Dedup / supersede decision (spec §4 table), keyed on canonical_path ──
            match self.current_file_for_path(&canonical_path)? {
                Some(prev) if prev.content_hash == new_hash => {
                    report.deduped += 1; // same path + same bytes → no-op
                }
                Some(prev) => {
                    // Changed bytes → atomic ground-truth supersede + new file_ingested.
                    let supersede_ev = ground_truth_supersede(&prev.file_event_id, self.signer_did());
                    let file_ev = ground_truth_file_ingested(content, self.signer_did());
                    let (_s, new_id) = self.append_pair(supersede_ev, file_ev)?;
                    self.derive_vector_for(embedder, &new_id)?;
                    report.superseded += 1;
                }
                None => {
                    let file_ev = ground_truth_file_ingested(content, self.signer_did());
                    let new_id = self.append(file_ev)?;
                    self.derive_vector_for(embedder, &new_id)?;
                    report.ingested += 1;
                }
            }
        }
        Ok(())
    }
}

/// A ground-truth `file_ingested` Event (model_meta: None → plain append/append_pair).
fn ground_truth_file_ingested(content: serde_json::Value, signer_did: String) -> Event {
    Event {
        id: String::new(), ts: String::new(), valid_time: None,
        event_type: crate::graph::FILE_INGESTED_EVENT_TYPE.to_string(),
        content, model_meta: None, prev_hash: String::new(), hash: None,
        signed_by_did: signer_did, signature: None,
    }
}

/// A ground-truth `supersede` Event retiring `prior_id` (reuses SUPERSEDE_EVENT_TYPE
/// but with model_meta: None — see the plan's design note on cross-fold safety).
fn ground_truth_supersede(prior_id: &str, signer_did: String) -> Event {
    Event {
        id: String::new(), ts: String::new(), valid_time: None,
        event_type: crate::graph::SUPERSEDE_EVENT_TYPE.to_string(),
        content: serde_json::json!({ "supersedes": prior_id }),
        model_meta: None, prev_hash: String::new(), hash: None,
        signed_by_did: signer_did, signature: None,
    }
}

/// File mtime as RFC 3339 (provenance only; NEVER a dedup/identity key).
fn file_mtime_rfc3339(cf: &ContainedFile) -> String {
    cf.modified_at_rfc3339()
}
```

- [ ] **Step 3: Expose the two tiny `EventLog` helpers used above**

`self.signer_did()` exists (private, `log.rs:1792`) — `ingest.rs` is in the same crate but `signer_did` is a private method; promote it to `pub(crate)`. Change `fn signer_did(&self)` → `pub(crate) fn signer_did(&self)` in `log.rs:1792`.

Add `derive_vector_for` to `EventLog` in `log.rs` (loads the event by id, then reuses `derive_vector`):

```rust
/// Derive + persist the vector for a just-appended event id (M5a ingest convenience).
/// Best-effort: a non-embeddable or text-less event is a no-op.
pub(crate) fn derive_vector_for(&self, embedder: &dyn Embedder, event_id: &str) -> Result<(), BossclawError> {
    let payload: Option<String> = {
        let store = self.inner.lock().expect(POISON);
        store.conn().query_row(
            "SELECT payload FROM events WHERE id = ?1", rusqlite::params![event_id],
            |r| r.get::<_, String>(0),
        ).optional()?
    };
    if let Some(p) = payload {
        let ev: Event = serde_json::from_str(&p)?;
        self.derive_vector(embedder, &ev)?;
    }
    Ok(())
}
```

Add `modified_at_rfc3339` to `ContainedFile` in `ingest.rs` (uses the std metadata on the open file; falls back to the epoch if the platform lacks mtime):

```rust
impl ContainedFile {
    /// File mtime as RFC 3339 UTC (provenance only).
    pub(crate) fn modified_at_rfc3339(&self) -> String {
        use chrono::{DateTime, Utc};
        self.file.metadata().ok()
            .and_then(|m| m.modified().ok())
            .map(|t| DateTime::<Utc>::from(t).to_rfc3339())
            .unwrap_or_else(|| "1970-01-01T00:00:00+00:00".to_string())
    }
}
```

> **Contract notes (second-opinion clarifications):**
> - `ingest_grant_inner` populates the `vectors` table per file (`derive_vector_for`) but does **NOT** rebuild the in-memory ANN index or the FTS index — so it alone leaves files keyword-unsearchable. Callers MUST run `rebuild_indexes(embedder)` + `rebuild_graph()` before recall; `ingest_all` (Task 11) does this, and any test that recalls must do it explicitly.
> - The byte cap is checked twice on purpose: the walk's `cf.size() > MAX_FILE_BYTES` (fstat — a cheap early-out) and `read_all_capped`'s `cap + 1` read (**authoritative** — catches a file that grows after fstat). Treat `read_all_capped` as the source of truth.
> - Capture `modified_at` BEFORE `read_all_capped` (it consumes the `ContainedFile`); the plan's ordering already does this — keep a comment so a refactor can't reorder it.

- [ ] **Step 4: Write the failing orchestrator test (fresh / dedup / supersede + mtime-only no-op)**

```rust
#[cfg(all(test, unix))]
mod orchestrator_tests {
    use super::*;
    use crate::embed::MockEmbedder;
    use ed25519_dalek::SigningKey;

    const DEK: [u8; 32] = [42u8; 32];
    const KEY_BYTES: [u8; 32] = [7u8; 32];

    fn run_ingest(log: &EventLog, root: &std::path::Path, parser: &dyn Parser, emb: &MockEmbedder) -> IngestReport {
        let mut report = IngestReport::default();
        let mut seen = std::collections::HashSet::new();
        log.ingest_grant_inner(root, parser, emb, Instant::now(), &mut seen, &mut report).unwrap();
        report
    }

    #[test]
    fn fresh_then_dedup_then_supersede() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("m.db");
        let folder = dir.path().join("notes");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("a.md"), b"# v1").unwrap();

        let emb = MockEmbedder::new(16);
        let log = EventLog::open(&db, &DEK, SigningKey::from_bytes(&KEY_BYTES)).unwrap();
        log.add_grant(&folder).unwrap();
        let canonical_folder = std::fs::canonicalize(&folder).unwrap();

        // Fresh ingest.
        let r1 = run_ingest(&log, &canonical_folder, &NativeTextParser, &emb);
        assert_eq!((r1.ingested, r1.deduped, r1.superseded), (1, 0, 0));

        // Re-ingest unchanged → dedup no-op.
        let r2 = run_ingest(&log, &canonical_folder, &NativeTextParser, &emb);
        assert_eq!((r2.ingested, r2.deduped, r2.superseded), (0, 1, 0));

        // Change the file → supersede.
        std::fs::write(folder.join("a.md"), b"# v2 changed").unwrap();
        let r3 = run_ingest(&log, &canonical_folder, &NativeTextParser, &emb);
        assert_eq!((r3.ingested, r3.deduped, r3.superseded), (0, 0, 1));

        // The current file record reflects v2.
        let canonical_file = canonical_folder.join("a.md").to_string_lossy().to_string();
        let rec = log.current_file_for_path(&canonical_file).unwrap().unwrap();
        let ev = log.stream_all().unwrap().into_iter().find(|e| e.id == rec.file_event_id).unwrap();
        assert_eq!(ev.content["text"], "# v2 changed");
        assert!(is_external(&ev), "file_ingested is externally tainted");
    }

    #[test]
    fn mtime_change_without_byte_change_does_not_supersede() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("m.db");
        let folder = dir.path().join("notes");
        std::fs::create_dir(&folder).unwrap();
        let f = folder.join("a.md");
        std::fs::write(&f, b"identical bytes").unwrap();
        let emb = MockEmbedder::new(16);
        let log = EventLog::open(&db, &DEK, SigningKey::from_bytes(&KEY_BYTES)).unwrap();
        log.add_grant(&folder).unwrap();
        let canonical = std::fs::canonicalize(&folder).unwrap();
        assert_eq!(run_ingest(&log, &canonical, &NativeTextParser, &emb).ingested, 1);

        // Rewrite IDENTICAL bytes (bumps mtime, content_hash unchanged).
        std::fs::write(&f, b"identical bytes").unwrap();
        let r = run_ingest(&log, &canonical, &NativeTextParser, &emb);
        assert_eq!((r.ingested, r.superseded, r.deduped), (0, 0, 1),
            "mtime is provenance-only; identical bytes → dedup, NEVER supersede");
    }
}
```

> `MockEmbedder::new(16)` — confirm the constructor against `embed.rs` (the recall tests construct it the same way). If the signature differs, match it; the dimension value is arbitrary for the mock.

- [ ] **Step 5: Run → expect PASS**

Run: `cargo test -p bossclaw-core orchestrator_tests -- --nocolor`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/bossclaw-core/src/ingest.rs crates/bossclaw-core/src/log.rs
git commit -m "feat(bossclaw-core): M5a Task 7 — ingest orchestrator (dedup/supersede decision + ground-truth file_ingested)"
```

---

### Task 8: Recall integration — the `file_ingested` exclusion arm

**Files:**
- Modify: `crates/bossclaw-core/src/log.rs` (the `recall` method)
- Test: inline in `log.rs` (or `ingest.rs`)

- [ ] **Step 1: Add `exclude_files` to `RecallOptions`, then the gated current-file set + retain arm in `recall`**

First, in `crates/bossclaw-core/src/recall.rs`, add a field to `RecallOptions` (right after `exclude_pages`, ~line 87) — the file analogue of the F3 one-way rule:

```rust
    /// When true, drop ALL `file_ingested`-kind hits — the one-way rule for the
    /// evolve loop's internal recall (Task 9), so external file text never enters
    /// the reasoner's extraction context. User-facing recall leaves it false.
    pub exclude_files: bool,
```

Then in `recall` (log.rs:944), alongside the `current_page_ids` block (982–987), add:

```rust
// Current file ids whose grant is still active (for the file-version +
// revoked-grant exclusion). Gated: skipped entirely unless a file is in the
// fusion candidate set (mirrors the page gate).
let current_file_ids: std::collections::HashSet<String> =
    if kinds.values().any(|k| k == crate::graph::FILE_INGESTED_EVENT_TYPE) {
        self.current_files_active()?
    } else {
        std::collections::HashSet::new()
    };
```

Then extend the `hits.retain(...)` closure (1096–1104) so file hits are filtered too:

```rust
hits.retain(|h| {
    if h.kind == crate::graph::PAGE_EVENT_TYPE {
        if opts.exclude_pages {
            return false; // one-way rule (F3)
        }
        return current_page_ids.contains(&h.event_id); // only the CURRENT page
    }
    if h.kind == crate::graph::FILE_INGESTED_EVENT_TYPE {
        if opts.exclude_files {
            return false; // one-way rule for files — keeps external text out of evolve context (Task 9)
        }
        // Keep only the CURRENT version for its path AND only if the grant is
        // still active (never-forget storage ≠ must-surface).
        return current_file_ids.contains(&h.event_id);
    }
    true // every other kind always survives
});
```

(Delete the old standalone page `retain` body that this replaces — there is one `retain`; only its closure changes.)

- [ ] **Step 2: Write the failing recall test (current-only + revoked-excluded)**

Add to `ingest.rs` `orchestrator_tests` (it already has the harness):

```rust
#[test]
fn recall_returns_only_current_version_and_drops_revoked() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("m.db");
    let folder = dir.path().join("notes");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("topic.md"), b"alpha unique-token-v1").unwrap();

    let emb = MockEmbedder::new(16);
    let log = EventLog::open_with_recall(&db, &DEK, SigningKey::from_bytes(&KEY_BYTES), &emb).unwrap();
    log.add_grant(&folder).unwrap();
    let canonical_folder = std::fs::canonicalize(&folder).unwrap();
    run_ingest(&log, &canonical_folder, &NativeTextParser, &emb);

    // Change the file, re-ingest → v1 superseded by v2.
    std::fs::write(folder.join("topic.md"), b"alpha unique-token-v2").unwrap();
    run_ingest(&log, &canonical_folder, &NativeTextParser, &emb);
    log.rebuild_indexes(&emb).unwrap();
    log.rebuild_graph().unwrap();

    // Recall: only the CURRENT (v2) file id survives the new arm.
    let hits = log.recall(&emb, "alpha", 10, &Default::default()).unwrap();
    let file_hits: Vec<_> = hits.iter().filter(|h| h.kind == crate::graph::FILE_INGESTED_EVENT_TYPE).collect();
    assert_eq!(file_hits.len(), 1, "only the current version surfaces, never both");
    let canonical_file = canonical_folder.join("topic.md").to_string_lossy().to_string();
    let cur = log.current_file_for_path(&canonical_file).unwrap().unwrap();
    assert_eq!(file_hits[0].event_id, cur.file_event_id);
    assert!(file_hits[0].sources.contains(&crate::recall::RecallSource::Keyword),
        "the keyword (FTS) arm surfaces the file — proves ingest + rebuild_indexes populated FTS, not only vectors");

    // Revoke the grant → the file is excluded from recall (still in the log).
    log.revoke_grant(&canonical_folder).unwrap();
    let hits2 = log.recall(&emb, "alpha", 10, &Default::default()).unwrap();
    assert!(hits2.iter().all(|h| h.kind != crate::graph::FILE_INGESTED_EVENT_TYPE),
        "a revoked grant's files do not surface in recall");
}
```

- [ ] **Step 3: Run → expect PASS**

Run: `cargo test -p bossclaw-core recall_returns_only_current -- --nocolor`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/bossclaw-core/src/log.rs crates/bossclaw-core/src/ingest.rs
git commit -m "feat(bossclaw-core): M5a Task 8 — recall file_ingested exclusion arm (current-only + revoke-aware)"
```

---

### Task 9: Taint root — classifier, evolve cursor + CONTEXT exclusion, ground-truth invariants

**Files:**
- Modify: `crates/bossclaw-core/src/log.rs` (set `exclude_files: true` in `evolve_once`'s internal recall — closes evolve door (2))
- Modify: `crates/bossclaw-core/src/ingest.rs` (re-export `is_external`)
- Modify: `crates/bossclaw-core/src/lib.rs` (`pub use ingest::is_external;`)
- Test: inline in `ingest.rs`

- [ ] **Step 1: Re-export the classifier**

In `lib.rs`, extend the ingest re-export:

```rust
pub use ingest::{is_external, IngestReport, NativeTextParser, Parser, PathHint};
```

- [ ] **Step 2: Close evolve door (2) — exclude files from the reasoner's recall context**

In `evolve_once` (`log.rs:2951-2957`) the internal context recall passes `exclude_pages: true` only. Add `exclude_files: true`:

```rust
            let recalled: Vec<String> = self
                .recall(
                    embedder,
                    &text,
                    crate::extract::GRAPH_CONTEXT_K,
                    &RecallOptions { exclude_pages: true, exclude_files: true, ..Default::default() },
                )
```

This is the ONLY behavioral change to the evolve loop. Then grep every other internal `recall(` call site in the evolve/summarize paths that feeds a model and add `exclude_files: true` there too; verify `gather_fact_set`/`fact_texts_for_ids` cannot pull a `file_ingested` id into reasoner context (they already drop pages — extend the same way if any path can).

- [ ] **Step 3: Write the taint + cursor-exclusion + CONTEXT-laundering + ground-truth tests**

```rust
#[test]
fn taint_classifier_and_extraction_exclusion() {
    // A file_ingested event is classified external; a memory is not.
    let file_ev = Event {
        id: "f".into(), ts: String::new(), valid_time: None,
        event_type: crate::graph::FILE_INGESTED_EVENT_TYPE.into(),
        content: serde_json::json!({ "text": "x", "origin": crate::graph::EXTERNAL_ORIGIN }),
        model_meta: None, prev_hash: String::new(), hash: None,
        signed_by_did: "did:wba:AIR-TEST".into(), signature: None,
    };
    assert!(is_external(&file_ev));
    let mem = Event {
        id: "m".into(), ts: String::new(), valid_time: None,
        event_type: crate::graph::MEMORY_EVENT_TYPE.into(),
        content: serde_json::json!({ "text": "x" }),
        model_meta: None, prev_hash: String::new(), hash: None,
        signed_by_did: "did:wba:AIR-TEST".into(), signature: None,
    };
    assert!(!is_external(&mem), "a memory carries no external taint");
}

#[cfg(unix)]
#[test]
fn ingested_files_are_excluded_from_the_evolve_cursor() {
    use crate::embed::MockEmbedder;
    use ed25519_dalek::SigningKey;
    const DEK: [u8; 32] = [42u8; 32];
    const KEY_BYTES: [u8; 32] = [7u8; 32];

    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("notes");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("a.md"), b"some note").unwrap();
    let emb = MockEmbedder::new(16);
    let log = EventLog::open(&dir.path().join("m.db"), &DEK, SigningKey::from_bytes(&KEY_BYTES)).unwrap();
    log.add_grant(&folder).unwrap();
    let canonical = std::fs::canonicalize(&folder).unwrap();
    let mut report = IngestReport::default();
    let mut seen = std::collections::HashSet::new();
    log.ingest_grant_inner(&canonical, &NativeTextParser, &emb, Instant::now(), &mut seen, &mut report).unwrap();
    assert_eq!(report.ingested, 1);

    // The evolve queue depth counts ONLY memory events; the file_ingested must
    // not appear (verified against the memory-only cursor at log.rs:2443).
    // NOTE: this proves files are not extraction SUBJECTS (door 1); the
    // context-laundering test below proves they are not extraction CONTEXT (door 2).
    let depth = log.evolve_status().unwrap();
    assert_eq!(depth.queue_depth, 0, "file_ingested events are never an evolve work-unit (cursor door)");
}

// Door (2): the evolve loop's internal recall must NOT surface file text as
// context. Assert the EXACT RecallOptions the loop uses (exclude_pages +
// exclude_files) returns ZERO file hits, while user-facing recall (defaults)
// DOES return the file — proving the knob, not an empty corpus, is what hides it.
#[cfg(unix)]
#[test]
fn evolve_context_recall_excludes_file_text() {
    use crate::embed::MockEmbedder;
    use crate::recall::RecallOptions;
    use ed25519_dalek::SigningKey;
    const DEK: [u8; 32] = [42u8; 32];
    const KEY_BYTES: [u8; 32] = [7u8; 32];

    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("notes");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("a.md"), b"zztoken external poison").unwrap();
    let emb = MockEmbedder::new(16);
    let log = EventLog::open_with_recall(&dir.path().join("m.db"), &DEK, SigningKey::from_bytes(&KEY_BYTES), &emb).unwrap();
    log.add_grant(&folder).unwrap();
    log.ingest_all(&NativeTextParser, &emb).unwrap();

    // User-facing recall surfaces the file…
    let user = log.recall(&emb, "zztoken", 10, &Default::default()).unwrap();
    assert!(user.iter().any(|h| h.kind == crate::graph::FILE_INGESTED_EVENT_TYPE), "default recall returns the file");
    // …the evolve-context recall (the loop's exact options) does NOT.
    let ctx = log.recall(&emb, "zztoken", 10, &RecallOptions { exclude_pages: true, exclude_files: true, ..Default::default() }).unwrap();
    assert!(ctx.iter().all(|h| h.kind != crate::graph::FILE_INGESTED_EVENT_TYPE),
        "exclude_files drops file text from the reasoner's extraction context (no laundering)");
}

// A ground-truth supersede the orchestrator emits is NOT externally tainted —
// `origin` is the single source of truth for `is_external`, and supersedes carry none.
#[test]
fn ground_truth_supersede_is_not_external() {
    let sup = Event {
        id: "s".into(), ts: String::new(), valid_time: None,
        event_type: crate::graph::SUPERSEDE_EVENT_TYPE.into(),
        content: serde_json::json!({ "supersedes": "f1" }),
        model_meta: None, prev_hash: String::new(), hash: None,
        signed_by_did: "did:wba:AIR-TEST".into(), signature: None,
    };
    assert!(!is_external(&sup), "a file supersede is ground-truth control, not external content");
}
```

> `evolve_status()` returns `EvolveStatus`; confirm the queue-depth field name (`queue_depth`) against `evolve.rs`/`log.rs:3242`. If it differs, assert via the `unprocessed`-equivalent. The invariant: ingesting files adds **zero** evolve work AND contributes **zero** extraction context.

- [ ] **Step 4: Run → expect PASS**

Run: `cargo test -p bossclaw-core taint_classifier ingested_files_are_excluded evolve_context_recall_excludes ground_truth_supersede_is_not_external -- --nocolor`
Expected: PASS (4 tests — both evolve doors proven shut).

- [ ] **Step 5: Commit**

```bash
git add crates/bossclaw-core/src/log.rs crates/bossclaw-core/src/ingest.rs crates/bossclaw-core/src/lib.rs
git commit -m "feat(bossclaw-core): M5a Task 9 — taint root: classifier + BOTH evolve doors (cursor + context exclude_files)"
```

---

### Task 10: Tier-A — byte-identical rebuild + frozen canonicalization vector

**Files:**
- Modify: `crates/bossclaw-core/src/ingest.rs` or `crates/bossclaw-core/src/event.rs` tests
- Test: inline

- [ ] **Step 1: Locate the existing byte-identical-rebuild + frozen-canon tests**

Run: `grep -rn "byte-identical\|canonical_bytes\|frozen\|rebuild" crates/bossclaw-core/src/event.rs crates/bossclaw-core/src/log.rs | grep -i test`
Read the closest existing test (e.g. an `event.rs` canonicalization test). The new tests MIRROR its structure.

- [ ] **Step 2: Write the byte-identical-rebuild test for `file_ingested`**

Add to `ingest.rs` `orchestrator_tests`:

```rust
#[test]
fn file_ingested_survives_byte_identical_rebuild() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("m.db");
    let folder = dir.path().join("notes");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("a.md"), b"stable bytes").unwrap();
    let emb = MockEmbedder::new(16);

    let recorded_hash = {
        let log = EventLog::open(&db, &DEK, SigningKey::from_bytes(&KEY_BYTES)).unwrap();
        log.add_grant(&folder).unwrap();
        let canonical = std::fs::canonicalize(&folder).unwrap();
        run_ingest(&log, &canonical, &NativeTextParser, &emb);
        let ev = log.stream_all().unwrap().into_iter()
            .find(|e| e.event_type == crate::graph::FILE_INGESTED_EVENT_TYPE).unwrap();
        ev.hash.clone().unwrap()
    };
    // Reopen: the chain re-verifies and the file_ingested hash is unchanged.
    let log2 = EventLog::open_with_recall(&db, &DEK, SigningKey::from_bytes(&KEY_BYTES), &emb).unwrap();
    log2.verify_chain().unwrap();
    let ev2 = log2.stream_all().unwrap().into_iter()
        .find(|e| e.event_type == crate::graph::FILE_INGESTED_EVENT_TYPE).unwrap();
    assert_eq!(ev2.hash.unwrap(), recorded_hash, "file_ingested is byte-identical across reopen");
}
```

- [ ] **Step 3: Write the frozen canonicalization vector for the `file_ingested` content shape**

Add to `event.rs` tests (mirror the existing frozen-vector test there). A fully-fixed event (no append-assigned fields) pins the canonical bytes so an accidental shape/ordering change is caught:

```rust
#[test]
fn file_ingested_canonicalization_is_frozen() {
    let ev = Event {
        id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string(),
        ts: "2026-06-18T00:00:00+00:00".to_string(),
        valid_time: None,
        event_type: "file_ingested".to_string(),
        content: serde_json::json!({
            "text": "hello",
            "origin": "external",
            "provenance": {
                "canonical_path": "/x/a.md",
                "content_hash": "aaa",
                "text_hash": "bbb",
                "size_bytes": 5,
                "modified_at": "2026-06-18T00:00:00+00:00",
                "parser_id": "native-text-v1",
                "grant_root": "/x"
            }
        }),
        model_meta: None,
        prev_hash: "0".repeat(64),
        hash: None,
        signed_by_did: "did:wba:bossclaw-engine".to_string(),
        signature: None,
    };
    let canon = canonical_bytes(&ev).unwrap();
    let got = hex::encode(sha2::Sha256::digest(&canon));
    // FREEZE STEP: run once; the assert below fails and prints `got`. Paste that
    // hex here, then re-run → PASS. This pins the file_ingested canonical shape.
    let expected = "<<paste the printed hex from the first run here>>";
    assert_eq!(got, expected, "file_ingested canonicalization changed (got {got}); update only if intentional");
}
```

> The freeze step is the standard way a golden vector is created (run → copy the computed value → pin). It is **not** a placeholder: after the first run the constant is a fixed, meaningful value that guards the wire shape forever.

- [ ] **Step 4: Run, freeze, re-run → expect PASS; confirm `recall@k` fixture unaffected**

Run: `cargo test -p bossclaw-core file_ingested_canonicalization_is_frozen -- --nocolor` (copy the printed hex into `expected`, re-run → PASS).
Run: `cargo test -p bossclaw-core file_ingested_survives_byte_identical_rebuild -- --nocolor` → PASS.
Run the recall fixture (find it: `grep -rn "recall@\|recall_at\|fn recall" crates/bossclaw-core/src/*.rs | grep -i test`) and confirm it still passes unchanged.

- [ ] **Step 5: Commit**

```bash
git add crates/bossclaw-core/src/ingest.rs crates/bossclaw-core/src/event.rs
git commit -m "feat(bossclaw-core): M5a Task 10 — file_ingested byte-identical rebuild + frozen canon vector"
```

---

### Task 11: `ingest_all`, grant lifecycle, mid-ingest revoke, end-to-end + gates

**Files:**
- Modify: `crates/bossclaw-core/src/ingest.rs` (`ingest_all`)
- Test: inline in `ingest.rs`

- [ ] **Step 1: Add the public `ingest_all` entry point**

```rust
impl EventLog {
    /// Ingest every ACTIVE granted folder (spec §4). Runs the safe pipeline per
    /// grant, sharing one inode-seen set + one wall-clock budget across the whole
    /// run, then refreshes the recall index + projections so new files are
    /// immediately recallable. Returns the aggregate [`IngestReport`].
    pub fn ingest_all(
        &self,
        parser: &dyn Parser,
        embedder: &dyn crate::embed::Embedder,
    ) -> Result<IngestReport, crate::error::BossclawError> {
        let started = Instant::now();
        let mut report = IngestReport::default();
        let mut seen = std::collections::HashSet::new();
        let active: Vec<String> = self.grants()?.into_iter().filter(|g| !g.revoked).map(|g| g.canonical_root).collect();
        for root in active {
            self.ingest_grant_inner(std::path::Path::new(&root), parser, embedder, started, &mut seen, &mut report)?;
        }
        // Make the newly-appended files recallable + refresh files/grants projections.
        self.rebuild_indexes(embedder)?;
        self.rebuild_graph()?;
        Ok(report)
    }
}
```

- [ ] **Step 2: Write the failing end-to-end + mid-ingest-revoke tests**

```rust
#[test]
fn ingest_all_iterates_active_grants_and_is_recallable() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("m.db");
    let g1 = dir.path().join("notes1");
    let g2 = dir.path().join("notes2");
    std::fs::create_dir(&g1).unwrap();
    std::fs::create_dir(&g2).unwrap();
    std::fs::write(g1.join("x.md"), b"gamma unique-x").unwrap();
    std::fs::write(g2.join("y.md"), b"gamma unique-y").unwrap();

    let emb = MockEmbedder::new(16);
    let log = EventLog::open_with_recall(&db, &DEK, SigningKey::from_bytes(&KEY_BYTES), &emb).unwrap();
    log.add_grant(&g1).unwrap();
    log.add_grant(&g2).unwrap();

    let report = log.ingest_all(&NativeTextParser, &emb).unwrap();
    assert_eq!(report.ingested, 2, "both granted folders' files ingested");
    let hits = log.recall(&emb, "gamma", 10, &Default::default()).unwrap();
    assert_eq!(hits.iter().filter(|h| h.kind == crate::graph::FILE_INGESTED_EVENT_TYPE).count(), 2);
}

#[test]
fn revoked_grant_is_not_ingested_by_ingest_all() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("m.db");
    let folder = dir.path().join("notes");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("a.md"), b"delta").unwrap();
    let emb = MockEmbedder::new(16);
    let log = EventLog::open_with_recall(&db, &DEK, SigningKey::from_bytes(&KEY_BYTES), &emb).unwrap();
    log.add_grant(&folder).unwrap();
    log.revoke_grant(&std::fs::canonicalize(&folder).unwrap()).unwrap();
    let report = log.ingest_all(&NativeTextParser, &emb).unwrap();
    assert_eq!(report.ingested, 0, "ingest_all skips revoked grants");
}
```

- [ ] **Step 3: Run → expect PASS**

Run: `cargo test -p bossclaw-core ingest_all -- --nocolor`
Expected: PASS.

- [ ] **Step 4: Full suite + clippy + `forbid(unsafe_code)` confirmation**

```bash
cargo test -p bossclaw-core 2>&1 | tail -50
cargo clippy -p bossclaw-core --all-targets 2>&1 | tail -20
cargo clippy -p bossclaw-core --all-targets --features ollama 2>&1 | tail -20
grep -n 'forbid(unsafe_code)' crates/bossclaw-core/src/lib.rs   # must still be present
grep -rn 'unsafe' crates/bossclaw-core/src/ingest.rs            # expect: NONE
```

Expected: all tests pass; clippy clean (default + ollama); `forbid(unsafe_code)` present; zero `unsafe` in `ingest.rs`.

> **CI matrix (second-opinion):** the careful open has distinct Linux (`openat2`) vs macOS (`openat`+`NOFOLLOW`) `cfg` branches, and the containment/walk tests are `#[cfg(unix)]`. Ensure CI runs `cargo test -p bossclaw-core` on **both macOS and Linux** (the M1–M4 matrix already does) so neither branch is exercised only on a dev laptop; if Windows is in the matrix, confirm `careful_open_windows` + its tests are gated and run there too.

- [ ] **Step 5: Commit**

```bash
git add crates/bossclaw-core/src/ingest.rs
git commit -m "feat(bossclaw-core): M5a Task 11 — ingest_all + grant lifecycle + end-to-end (clippy clean, no unsafe)"
```

---

## Deferred (NOT in M5a — do not implement)

- **M5b (own spec + security review):** the sandboxed `markitdown` subprocess parser (per-OS sandbox, process-group kill, env-scrub, fd→stdin streaming, scratch cwd, output cap, pinned version + `pip-audit`). Rich formats (PDF/docx) become ingestable then.
- **Extraction** of entities/links from `file_ingested` content (evolve over files) — a later milestone; the M5a taint root makes it safe.
- **Actuator / writes (M6):** the fail-closed lineage WALK that *consumes* `is_external`, plus the confused-deputy write defenses. M5a plants the root and ships the classifier but claims **no** end-to-end write gate.
- **Content high-entropy-line skip** (secret-in-granted-folder mitigation) — documented fast-follow.
- **Hardlink-into-grant escape (residual, not defended in M5a).** A hardlink inside a granted folder pointing at an inode whose other name is *outside* the grant is caught by neither `NOFOLLOW`/`openat2 RESOLVE_BENEATH` (a hardlink has no parent pointer — the name genuinely is beneath the root) nor the name-based never-touch filter. The inode-seen set is **dedup, NOT containment** (never sell it as containment). The boundary is the grant + informed consent (an attacker who can plant hardlinks in your folder can equally copy the bytes in). Optional hardening (own decision, with a real tradeoff): skip-or-flag `st_nlink > 1` files loudly in the report — but that also skips *legitimate* within-grant hardlinks the inode-seen set would otherwise ingest once. Documented in spec Rev 3 §10.
- **Linux `openat2` as the sole final-open path on every Unix** — M5a uses the `openat`+`NOFOLLOW` fd-chain uniformly (with `openat2` layered on Linux); broadening is optional hardening.

## Self-review (completed by the plan author)

- **Spec coverage:** §1 goal → all tasks; §2 D1–D5 → Tasks 4/5 (D2/D3), 7 (D4 ground-truth), 9 (D5 taint root + classifier); §3 new event types + projections → Tasks 1/2/3; §4 components → Tasks 4–8; §5 data flow → Task 7/11; §6 safety/DoD → Tasks 5 (containment + TOCTOU), 6 (no-symlink/inode/caps/never-touch), 7 (dedup/supersede integrity), 8 (revoke-aware recall), 9 (taint + fence honesty doc), 10 (at-rest via existing SQLCipher); §7 error handling → Task 7 (best-effort report, fail-closed safety, append-per-file, revoke recheck); §8 tests → every task's tests, the TOCTOU swap (Task 5) and the new-arm recall test (Task 8) called out specifically; §8 Tier-A + frozen vector → Task 10; clippy + `forbid(unsafe_code)` → Task 11.
- **Placeholder scan:** the only "fill-in" is the frozen-vector hex in Task 10, which is the standard run-once-to-freeze step (documented as such), not a vague TODO.
- **Type consistency:** `FileRecord`/`Grant` field names, `IngestReport` fields, `Parser::convert(&[u8], &PathHint)`, `ContainedFile` methods, and the `FILE_INGESTED_EVENT_TYPE`/`EXTERNAL_ORIGIN`/`GRANT_EVENT_TYPE`/`REVOKE_EVENT_TYPE` consts are defined once (Tasks 1/3/4) and referenced identically thereafter. `signer_did`/`derive_vector_for`/`current_file_for_path`/`current_files_active` visibility promotions are listed where first needed.
- **Known API-confirm points (TDD surfaces instantly):** rustix `openat2`/`ResolveFlags`/`Dir::read_from`/`statat`/`FileType::from_raw_mode` spellings; `MockEmbedder::new` signature; `EvolveStatus` queue-depth field name; `OptionalExtension` import. Each is flagged inline at its task.
