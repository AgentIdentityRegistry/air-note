# Desktop Engine Ingest (SP2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire the engine's M5a ingest pipeline into the AIR Agent desktop app — grant folders, run ingest (native text, real bundled-model vectors), list ingested files — behind a minimal Sources panel, all through SP1's `EngineHandle`.

**Architecture:** Bottom-up. A new engine `set_active_model` records the embedding model; a desktop `EmbedderProvider` seam loads a bundled `model2vec` model (mock in tests); five `EngineHandle` methods route grant/ingest/list through SP1's onboarding-gated chokepoint; thin Tauri commands + serde DTOs expose them; a React Sources panel drives them. The model ships as a hash-pinned Tauri resource. Everything desktop-side is `#[cfg(unix)]` like SP1.

**Tech Stack:** Rust (Tauri 2, tokio, `bossclaw-core`, `model2vec-rs`, `tauri-plugin-dialog`), TypeScript/React (vitest), cargo-deny.

**Spec:** `docs/superpowers/specs/2026-06-22-desktop-engine-ingest-design.md` (Rev 2).

---

## Executor notes (read before starting)

- **Dead-code lint lifecycle (SP1 lesson).** The desktop CI runs `cargo clippy -- -D warnings` **without** `--all-targets`, so an item used *only* by `#[cfg(test)]` tests is flagged dead in the lib/bin build. New `EngineHandle` methods + `EngineOpError` are unreachable from `main` until the commands are registered (Task 6). Each such item carries `#[allow(dead_code)]` with a `// wired in Task 6` comment when introduced, and **Task 6 removes every one of them** once the commands make them reachable. Do not delete the methods to satisfy clippy — add the allow, and remove it at Task 6.
- **Unix-gating.** All desktop engine code is `#[cfg(unix)]` (the `engine` module, the `commands::engine` module, the `AppState.engine` field, and each `generate_handler!` entry — see main.rs:7, commands/mod.rs:3, main.rs:115). The engine method `set_active_model` (Task 1) is plain (the engine crate already compiles cross-platform; it's only *called* from Unix code).
- **Model prerequisite for manual launch.** `tauri dev`/`tauri build` need the real model on disk. Run `scripts/fetch-model.sh` (Task 8) first. `cargo test`/CI never need it — tests use `MockEmbedderProvider`.
- **Commit discipline.** Stage explicit paths (never `git add .`); verify `git status -s` between staging and commit. Each task ends with a green commit on branch `desktop-engine-ingest`.

## File structure

**Engine (`crates/bossclaw-core/`)**
- Modify `src/log.rs` — add `EventLog::set_active_model(model_id, dim)` (+ test).

**Desktop backend (`apps/desktop/src-tauri/`, all `#[cfg(unix)]`)**
- Create `src/engine/embed.rs` — `MODEL_ID`, `EmbedderProvider` trait, `ResourceModel2Vec`, `MockEmbedderProvider` (test).
- Modify `src/engine/mod.rs` — `EngineOpError`; `EngineHandle` fields (`embedder_provider`, `ingest_lock`) + new `new()`; five methods; update SP1 tests.
- Modify `src/commands/engine.rs` — DTOs + six commands.
- Modify `src/main.rs` — construct `ResourceModel2Vec`, pass to `EngineHandle::new`; register six commands.
- Modify `tauri.conf.json` — `bundle.resources` for the model.

**Desktop frontend (`apps/desktop/src/`)**
- Create `src/api/engine.ts` — typed `invoke` wrappers + DTO types.
- Create `src/sources/ingestSummary.ts` (+ `.test.ts`) — report → summary string.
- Create `src/sources/grants.ts` (+ `.test.ts`) — active-grant filter.
- Create `src/sources/SourcesPanel.tsx` — the panel.
- Modify `src/settings/AirSettings.tsx` — mount `<SourcesPanel/>`.

**Build / security (repo root)**
- Create `scripts/fetch-model.sh` — hash-pinned model download.
- Modify `.gitignore` — ignore the model files; create `apps/desktop/src-tauri/resources/models/.gitkeep`.
- Modify `.github/workflows/build.yml` — an "engine network-free" guard step (scoped `cargo tree` on `bossclaw-core`).

---

## Task 1: Engine — `EventLog::set_active_model`

**Files:**
- Modify: `crates/bossclaw-core/src/log.rs` (add a method on `impl EventLog`, near `reembed_migration` ~line 1719; add a test in the file's `#[cfg(test)] mod tests`)

Records the active embedding model as a signed `config` event (so `active_model()` is truthful and SP3's recall can discover the model). Mirrors `reembed_migration`'s config write (log.rs:1732-1747) but signed by the log's own DID and without the re-embed/GC.

- [ ] **Step 1: Write the failing test**

Add to `crates/bossclaw-core/src/log.rs` `mod tests` (use the test harness already in that module — `open_temp()` or the existing pattern that builds an `EventLog` with a fixed DEK/key; match the surrounding tests' constructor):

```rust
#[test]
fn set_active_model_writes_discoverable_config() {
    let tmp = tempfile::tempdir().unwrap();
    let log = open_log(tmp.path()); // the existing test helper in this module (log.rs:6542)
    assert!(log.active_model().unwrap().is_none());

    log.set_active_model("minishlab/potion-base-8M", 256).unwrap();

    let m = log.active_model().unwrap().expect("config now present");
    assert_eq!(m.active_model_id, "minishlab/potion-base-8M");
    assert_eq!(m.dim, 256);
    assert_eq!(m.schema_version, SCHEMA_VERSION);

    // Idempotent re-set with the same model keeps it discoverable.
    log.set_active_model("minishlab/potion-base-8M", 256).unwrap();
    assert_eq!(log.active_model().unwrap().unwrap().active_model_id, "minishlab/potion-base-8M");
}
```

> `open_log(dir: &Path) -> EventLog` already exists in that test module (log.rs:6542): it opens `dir.join("m.db")` with the fixed `DEK` + `KEY_BYTES` `SigningKey`. `tempfile::tempdir()` is available (the module's other tests use it).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bossclaw-core set_active_model_writes_discoverable_config`
Expected: FAIL — `no method named set_active_model`.

- [ ] **Step 3: Write minimal implementation**

Add to `impl EventLog` in `crates/bossclaw-core/src/log.rs` (alongside `reembed_migration`):

```rust
/// Record the active embedding model as a signed `config` event so
/// [`active_model`](Self::active_model) becomes truthful. Mirrors the config
/// write inside [`reembed_migration`] but signed by this log's own DID (not the
/// migration DID) and without the re-embed/GC — callers that have just embedded
/// under `model_id` use this to stamp the model at vector-birth. Reuses the
/// existing `schema_version` if a config already exists. Returns the event id.
pub fn set_active_model(&self, model_id: &str, dim: u32) -> Result<String, BossclawError> {
    let schema_version = self
        .active_model()?
        .map(|m| m.schema_version)
        .unwrap_or(SCHEMA_VERSION);
    self.append(Event {
        id: String::new(),
        ts: String::new(),
        valid_time: None,
        event_type: CONFIG_EVENT_TYPE.to_string(),
        content: serde_json::json!({
            "active_model_id": model_id,
            "dim": dim,
            "schema_version": schema_version,
        }),
        model_meta: None,
        prev_hash: String::new(),
        hash: None,
        signed_by_did: self.signer_did(),
        signature: None,
    })
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p bossclaw-core set_active_model_writes_discoverable_config`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bossclaw-core/src/log.rs
git status -s
git commit -m "feat(bossclaw-core): EventLog::set_active_model — stamp the active model at vector-birth"
```

---

## Task 2: Desktop — embedder seam (`engine/embed.rs`) + `EngineOpError` + handle wiring

**Files:**
- Create: `apps/desktop/src-tauri/src/engine/embed.rs`
- Modify: `apps/desktop/src-tauri/src/engine/mod.rs` (add `pub mod embed;`, `EngineOpError`, two `EngineHandle` fields, new `new()` signature; update the existing tests)
- Modify: `apps/desktop/src-tauri/src/main.rs` (construct `ResourceModel2Vec`, pass to `new`)

- [ ] **Step 1: Write the failing test**

Add to `apps/desktop/src-tauri/src/engine/mod.rs` `mod tests` (it already has a `TestVault`):

```rust
#[test]
fn mock_embedder_provider_yields_a_working_embedder() {
    use crate::engine::embed::{EmbedderProvider, MockEmbedderProvider};
    let p = MockEmbedderProvider::new(8);
    let e = p.embedder().expect("mock embedder builds");
    let v = e.embed(&["hello".to_string()]).unwrap();
    assert_eq!(v[0].len(), 8);
    assert_eq!(e.model_id(), "mock-v1");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run (from `apps/desktop/src-tauri`): `cargo test mock_embedder_provider_yields_a_working_embedder`
Expected: FAIL — `unresolved module ... embed`.

- [ ] **Step 3: Write minimal implementation**

Create `apps/desktop/src-tauri/src/engine/embed.rs`:

```rust
//! The SP2 embedder seam: a provider that yields the real `Model2Vec` (loaded
//! from the bundled model resource, lazily + cached) in production and a
//! `MockEmbedder` in tests. See docs/superpowers/specs/2026-06-22-desktop-engine-ingest-design.md.

use crate::engine::EngineOpError;
use bossclaw_core::{Embedder, Model2Vec};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// The single source of truth for the active embedding model id. Both this
/// crate's ingest (SP2) and the future recall-open (SP3) construct `Model2Vec`
/// with THIS id, so the vectors SP2 writes and the index SP3 rebuilds match.
pub const MODEL_ID: &str = "minishlab/potion-base-8M";

/// Builds (and caches) the embedder. Called on first ingest, never at startup.
pub trait EmbedderProvider: Send + Sync {
    fn embedder(&self) -> Result<Arc<dyn Embedder>, EngineOpError>;
}

/// Production provider: loads `Model2Vec` from the bundled model directory on
/// first use and caches it for the process lifetime.
pub struct ResourceModel2Vec {
    model_dir: PathBuf,
    cell: Mutex<Option<Arc<dyn Embedder>>>,
}

impl ResourceModel2Vec {
    pub fn new(model_dir: PathBuf) -> Self {
        Self { model_dir, cell: Mutex::new(None) }
    }
}

impl EmbedderProvider for ResourceModel2Vec {
    fn embedder(&self) -> Result<Arc<dyn Embedder>, EngineOpError> {
        let mut guard = self.cell.lock().expect("embedder cell poisoned");
        if let Some(e) = guard.as_ref() {
            return Ok(e.clone());
        }
        let model = Model2Vec::from_pretrained(&self.model_dir, MODEL_ID)
            .map_err(|e| EngineOpError::Embedder(e.to_string()))?;
        let arc: Arc<dyn Embedder> = Arc::new(model);
        *guard = Some(arc.clone());
        Ok(arc)
    }
}

#[cfg(test)]
pub struct MockEmbedderProvider {
    dim: usize,
}

#[cfg(test)]
impl MockEmbedderProvider {
    pub fn new(dim: usize) -> Self {
        Self { dim }
    }
}

#[cfg(test)]
impl EmbedderProvider for MockEmbedderProvider {
    fn embedder(&self) -> Result<Arc<dyn Embedder>, EngineOpError> {
        Ok(Arc::new(bossclaw_core::MockEmbedder::new(self.dim)))
    }
}
```

In `apps/desktop/src-tauri/src/engine/mod.rs`, add the module declaration near the top (under the existing `pub mod keystore;`):

```rust
pub mod embed;
```

Add the operational error type (below `EngineError`'s `impl Display`). The `#[allow(dead_code)]` comes off in Task 6:

```rust
/// Errors from the SP2 operational commands (grant/ingest/list). Wraps the
/// SP1 open/gate path so SP1's `EngineError`/`map_err_state`/`EngineState`
/// stay a status-only concern (untouched).
#[allow(dead_code)] // wired in Task 6 (commands map this to String)
#[derive(Debug)]
pub enum EngineOpError {
    Open(EngineError),
    Core(String),
    Embedder(String),
    Busy,
    Join(String),
}

#[allow(dead_code)] // wired in Task 6
impl std::fmt::Display for EngineOpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineOpError::Open(e) => write!(f, "{e}"),
            EngineOpError::Core(m) => write!(f, "engine error: {m}"),
            EngineOpError::Embedder(m) => write!(f, "memory model unavailable: {m}"),
            EngineOpError::Busy => write!(f, "an ingest is already running"),
            EngineOpError::Join(m) => write!(f, "engine task error: {m}"),
        }
    }
}
```

Change the `EngineHandle` struct + `new` to hold the provider and the in-flight lock:

```rust
pub struct EngineHandle {
    cell: Mutex<Option<Arc<EventLog>>>,
    keystore: EngineKeystore,
    db_path: PathBuf,
    embedder_provider: Arc<dyn crate::engine::embed::EmbedderProvider>,
    ingest_lock: Mutex<()>,
}

impl EngineHandle {
    pub fn new(
        vault: Arc<dyn SecretsVault>,
        data_dir: PathBuf,
        embedder_provider: Arc<dyn crate::engine::embed::EmbedderProvider>,
    ) -> Self {
        Self {
            cell: Mutex::new(None),
            keystore: EngineKeystore::new(vault),
            db_path: data_dir.join("brain.db"),
            embedder_provider,
            ingest_lock: Mutex::new(()),
        }
    }
    // ... existing get_or_open / status / teardown unchanged ...
}
```

> `Mutex` here is the `tokio::sync::Mutex` already imported in mod.rs (SP1 uses it for `cell`). `ingest_lock`'s `try_lock` (Task 4) returns immediately when held.

Update the **existing SP1 tests** in mod.rs `mod tests`: every `EngineHandle::new(vault, dir)` becomes `EngineHandle::new(vault, dir, std::sync::Arc::new(embed::MockEmbedderProvider::new(8)))`. Add `use crate::engine::embed;` to the test module. (There are four such call sites: `not_onboarded_does_not_open_or_mint`, `onboarded_opens_fresh_brain_and_memoizes`, `wrong_dek_reports_keystore_db_mismatch`, `teardown_removes_keys_db_and_resets_cell`.)

In `apps/desktop/src-tauri/src/main.rs`, replace the engine construction (line ~65-66) inside the `#[cfg(unix)]` block:

```rust
#[cfg(unix)]
let engine = {
    let resource_dir = app.path().resource_dir().expect("resource dir");
    let model_dir = resource_dir.join("models/potion-base-8M");
    let provider = std::sync::Arc::new(crate::engine::embed::ResourceModel2Vec::new(model_dir));
    std::sync::Arc::new(crate::engine::EngineHandle::new(vault, data_dir, provider))
};
```

- [ ] **Step 4: Run test + gates to verify green**

Run (from `apps/desktop/src-tauri`):
```
cargo test mock_embedder_provider_yields_a_working_embedder
cargo clippy -- -D warnings
```
Expected: the new test PASSES; the four updated SP1 tests still PASS; clippy clean (the `#[allow(dead_code)]` on `EngineOpError` keeps it green — it has no caller yet).

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src-tauri/src/engine/embed.rs apps/desktop/src-tauri/src/engine/mod.rs apps/desktop/src-tauri/src/main.rs
git status -s
git commit -m "feat(desktop): SP2 embedder seam (EmbedderProvider + ResourceModel2Vec) + EngineOpError"
```

---

## Task 3: Desktop — `EngineHandle` grant methods (add/revoke/list grants)

**Files:**
- Modify: `apps/desktop/src-tauri/src/engine/mod.rs` (three methods on `impl EngineHandle` + tests)

- [ ] **Step 1: Write the failing test**

Add to mod.rs `mod tests`:

```rust
#[tokio::test]
async fn grant_then_list_then_revoke() {
    let app_dir = tempfile::tempdir().unwrap();
    let src_dir = tempfile::tempdir().unwrap();
    let vault = TestVault::new();
    let h = EngineHandle::new(
        vault, app_dir.path().to_path_buf(),
        std::sync::Arc::new(embed::MockEmbedderProvider::new(8)),
    );

    h.add_grant(true, src_dir.path().to_path_buf()).await.unwrap();
    let grants = h.list_grants(true).await.unwrap();
    assert_eq!(grants.len(), 1);
    assert!(!grants[0].revoked);

    h.revoke_grant(true, src_dir.path().to_path_buf()).await.unwrap();
    let grants = h.list_grants(true).await.unwrap();
    assert_eq!(grants.len(), 1);
    assert!(grants[0].revoked);

    // Gate: not-onboarded refuses.
    assert!(matches!(h.add_grant(false, src_dir.path().to_path_buf()).await, Err(EngineOpError::Open(EngineError::NotOnboarded))));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test grant_then_list_then_revoke`
Expected: FAIL — `no method named add_grant`.

- [ ] **Step 3: Write minimal implementation**

Add to `impl EngineHandle` in mod.rs (each `#[allow(dead_code)]` removed in Task 6):

```rust
/// Grant read-access to `path` (canonicalized + appended by the engine). Gated.
#[allow(dead_code)] // wired in Task 6
pub async fn add_grant(&self, onboarded: bool, path: PathBuf) -> Result<(), EngineOpError> {
    let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
    tokio::task::spawn_blocking(move || {
        log.add_grant(&path).map(|_| ()).map_err(|e| EngineOpError::Core(e.to_string()))
    })
    .await
    .map_err(|e| EngineOpError::Join(e.to_string()))?
}

/// Revoke a previously-granted folder. Gated.
#[allow(dead_code)] // wired in Task 6
pub async fn revoke_grant(&self, onboarded: bool, path: PathBuf) -> Result<(), EngineOpError> {
    let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
    tokio::task::spawn_blocking(move || {
        log.revoke_grant(&path).map(|_| ()).map_err(|e| EngineOpError::Core(e.to_string()))
    })
    .await
    .map_err(|e| EngineOpError::Join(e.to_string()))?
}

/// Every grant (active + revoked); the UI filters to active. Gated.
#[allow(dead_code)] // wired in Task 6
pub async fn list_grants(&self, onboarded: bool) -> Result<Vec<bossclaw_core::Grant>, EngineOpError> {
    let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
    tokio::task::spawn_blocking(move || {
        log.grants().map_err(|e| EngineOpError::Core(e.to_string()))
    })
    .await
    .map_err(|e| EngineOpError::Join(e.to_string()))?
}
```

- [ ] **Step 4: Run test + clippy**

Run: `cargo test grant_then_list_then_revoke && cargo clippy -- -D warnings`
Expected: PASS; clippy clean (allows in place).

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src-tauri/src/engine/mod.rs
git status -s
git commit -m "feat(desktop): EngineHandle add_grant/revoke_grant/list_grants (gated, spawn_blocking)"
```

---

## Task 4: Desktop — `EngineHandle::run_ingest` + `list_files`

**Files:**
- Modify: `apps/desktop/src-tauri/src/engine/mod.rs` (two methods + a test)

- [ ] **Step 1: Write the failing test**

Add to mod.rs `mod tests`:

```rust
#[tokio::test]
async fn ingest_indexes_files_and_records_model() {
    use std::fs;
    let app_dir = tempfile::tempdir().unwrap();
    let src_dir = tempfile::tempdir().unwrap();
    fs::write(src_dir.path().join("a.txt"), "the quick brown fox").unwrap();
    fs::write(src_dir.path().join("b.md"), "# notes\nhello world").unwrap();

    let vault = TestVault::new();
    let h = EngineHandle::new(
        vault, app_dir.path().to_path_buf(),
        std::sync::Arc::new(embed::MockEmbedderProvider::new(8)),
    );
    h.add_grant(true, src_dir.path().to_path_buf()).await.unwrap();

    let report = h.run_ingest(true).await.unwrap();
    assert_eq!(report.ingested, 2);
    assert_eq!(report.failed.len(), 0);

    let files = h.list_files(true).await.unwrap();
    assert_eq!(files.len(), 2);

    // The active model is now recorded (mock id in tests).
    // (Re-ingest is a no-op: everything deduped, model already recorded.)
    let again = h.run_ingest(true).await.unwrap();
    assert_eq!(again.ingested, 0);
    assert_eq!(again.deduped, 2);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test ingest_indexes_files_and_records_model`
Expected: FAIL — `no method named run_ingest`.

- [ ] **Step 3: Write minimal implementation**

Add to `impl EngineHandle` in mod.rs:

```rust
/// Ingest every active granted folder (native text only), then record the
/// active model once (so SP3 recall can discover it). Gated; serialized by an
/// in-flight guard (a concurrent call returns `Busy`).
#[allow(dead_code)] // wired in Task 6
pub async fn run_ingest(&self, onboarded: bool) -> Result<bossclaw_core::IngestReport, EngineOpError> {
    let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
    let _guard = self.ingest_lock.try_lock().map_err(|_| EngineOpError::Busy)?;
    let provider = self.embedder_provider.clone();
    let report = tokio::task::spawn_blocking(move || -> Result<bossclaw_core::IngestReport, EngineOpError> {
        let embedder = provider.embedder()?; // lazy, cached — built BEFORE the walk
        let router = bossclaw_core::ingest::ParserRouter::native_only();
        let report = log.ingest_all(&router, &*embedder).map_err(|e| EngineOpError::Core(e.to_string()))?;
        // Record the active model at vector-birth (idempotent: only when absent or changed).
        let needs = match log.active_model().map_err(|e| EngineOpError::Core(e.to_string()))? {
            Some(m) => m.active_model_id != embedder.model_id(),
            None => true,
        };
        if needs {
            log.set_active_model(embedder.model_id(), embedder.dim() as u32)
                .map_err(|e| EngineOpError::Core(e.to_string()))?;
        }
        Ok(report)
    })
    .await
    .map_err(|e| EngineOpError::Join(e.to_string()))??;
    Ok(report)
}

/// Every current ingested file (one per path). Gated.
#[allow(dead_code)] // wired in Task 6
pub async fn list_files(&self, onboarded: bool) -> Result<Vec<bossclaw_core::graph::FileRecord>, EngineOpError> {
    let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
    tokio::task::spawn_blocking(move || {
        log.current_files().map_err(|e| EngineOpError::Core(e.to_string()))
    })
    .await
    .map_err(|e| EngineOpError::Join(e.to_string()))?
}
```

- [ ] **Step 4: Run test + clippy**

Run: `cargo test ingest_indexes_files_and_records_model && cargo clippy -- -D warnings`
Expected: PASS; clippy clean.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src-tauri/src/engine/mod.rs
git status -s
git commit -m "feat(desktop): EngineHandle run_ingest (native-only, records active model) + list_files"
```

---

## Task 5: Desktop — DTOs (`commands/engine.rs`)

**Files:**
- Modify: `apps/desktop/src-tauri/src/commands/engine.rs` (DTOs + `From` mappings + a test)

- [ ] **Step 1: Write the failing test**

Add to `apps/desktop/src-tauri/src/commands/engine.rs` a `#[cfg(test)] mod tests`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ingest_report_maps_to_dto() {
        let mut r = bossclaw_core::IngestReport::default();
        r.ingested = 2;
        r.skipped.push((std::path::PathBuf::from("/x/a.bin"), "not valid UTF-8".into()));
        let dto = IngestReportDto::from(r);
        assert_eq!(dto.ingested, 2);
        assert_eq!(dto.skipped.len(), 1);
        assert_eq!(dto.skipped[0].path, "/x/a.bin");
        assert_eq!(dto.skipped[0].reason, "not valid UTF-8");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml ingest_report_maps_to_dto`
Expected: FAIL — `cannot find type IngestReportDto`.

- [ ] **Step 3: Write minimal implementation**

At the top of `apps/desktop/src-tauri/src/commands/engine.rs` add (the module is already `#[cfg(unix)]` via commands/mod.rs):

```rust
use serde::Serialize;

#[derive(Serialize)]
pub struct GrantDto {
    pub canonical_root: String,
    pub granted_at: String,
    pub revoked: bool,
}
impl From<bossclaw_core::Grant> for GrantDto {
    fn from(g: bossclaw_core::Grant) -> Self {
        Self { canonical_root: g.canonical_root, granted_at: g.granted_at, revoked: g.revoked }
    }
}

#[derive(Serialize)]
pub struct FileRecordDto {
    pub canonical_path: String,
    pub file_event_id: String,
    pub content_hash: String,
    pub grant_root: String,
}
impl From<bossclaw_core::graph::FileRecord> for FileRecordDto {
    fn from(f: bossclaw_core::graph::FileRecord) -> Self {
        Self {
            canonical_path: f.canonical_path,
            file_event_id: f.file_event_id,
            content_hash: f.content_hash,
            grant_root: f.grant_root,
        }
    }
}

#[derive(Serialize)]
pub struct SkipDto {
    pub path: String,
    pub reason: String,
}

#[derive(Serialize)]
pub struct IngestReportDto {
    pub ingested: usize,
    pub superseded: usize,
    pub deduped: usize,
    pub skipped: Vec<SkipDto>,
    pub failed: Vec<SkipDto>,
}
impl From<bossclaw_core::IngestReport> for IngestReportDto {
    fn from(r: bossclaw_core::IngestReport) -> Self {
        let map = |v: Vec<(std::path::PathBuf, String)>| {
            v.into_iter()
                .map(|(p, reason)| SkipDto { path: p.to_string_lossy().into_owned(), reason })
                .collect()
        };
        Self {
            ingested: r.ingested,
            superseded: r.superseded,
            deduped: r.deduped,
            skipped: map(r.skipped),
            failed: map(r.failed),
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml ingest_report_maps_to_dto`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src-tauri/src/commands/engine.rs
git status -s
git commit -m "feat(desktop): SP2 ingest DTOs (Grant/FileRecord/IngestReport) + mappings"
```

---

## Task 6: Desktop — commands + register in `main.rs` + remove dead-code allows

**Files:**
- Modify: `apps/desktop/src-tauri/src/commands/engine.rs` (six commands)
- Modify: `apps/desktop/src-tauri/src/main.rs` (register six entries)
- Modify: `apps/desktop/src-tauri/src/engine/mod.rs` (remove `#[allow(dead_code)]` from `EngineOpError` + its `Display` + the five methods)

- [ ] **Step 1: Write the implementation (commands)**

Append to `apps/desktop/src-tauri/src/commands/engine.rs` (it already has `use crate::commands::identity::AppState;` and `use tauri::State;` from SP1):

```rust
#[tauri::command]
pub async fn engine_add_grant(path: String, state: State<'_, AppState>) -> Result<(), String> {
    let onboarded = state.identity_store.is_onboarded();
    state.engine.add_grant(onboarded, std::path::PathBuf::from(path)).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn engine_revoke_grant(path: String, state: State<'_, AppState>) -> Result<(), String> {
    let onboarded = state.identity_store.is_onboarded();
    state.engine.revoke_grant(onboarded, std::path::PathBuf::from(path)).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn engine_list_grants(state: State<'_, AppState>) -> Result<Vec<GrantDto>, String> {
    let onboarded = state.identity_store.is_onboarded();
    let grants = state.engine.list_grants(onboarded).await.map_err(|e| e.to_string())?;
    Ok(grants.into_iter().map(GrantDto::from).collect())
}

#[tauri::command]
pub async fn engine_run_ingest(state: State<'_, AppState>) -> Result<IngestReportDto, String> {
    let onboarded = state.identity_store.is_onboarded();
    let report = state.engine.run_ingest(onboarded).await.map_err(|e| e.to_string())?;
    Ok(IngestReportDto::from(report))
}

#[tauri::command]
pub async fn engine_list_files(state: State<'_, AppState>) -> Result<Vec<FileRecordDto>, String> {
    let onboarded = state.identity_store.is_onboarded();
    let files = state.engine.list_files(onboarded).await.map_err(|e| e.to_string())?;
    Ok(files.into_iter().map(FileRecordDto::from).collect())
}

#[tauri::command]
pub async fn engine_pick_folder(app: tauri::AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_folder(move |p| {
        let _ = tx.send(p);
    });
    // A cancelled dialog yields None; a dropped sender (window closed) also → None.
    let picked = rx.await.ok().flatten();
    Ok(picked.and_then(|p| p.into_path().ok()).map(|pb| pb.to_string_lossy().into_owned()))
}
```

- [ ] **Step 2: Register in `main.rs` + remove the allows**

In `apps/desktop/src-tauri/src/main.rs` `generate_handler!`, after the existing `#[cfg(unix)] commands::engine::engine_status,` (line 115-116), add:

```rust
            #[cfg(unix)]
            commands::engine::engine_add_grant,
            #[cfg(unix)]
            commands::engine::engine_revoke_grant,
            #[cfg(unix)]
            commands::engine::engine_list_grants,
            #[cfg(unix)]
            commands::engine::engine_run_ingest,
            #[cfg(unix)]
            commands::engine::engine_list_files,
            #[cfg(unix)]
            commands::engine::engine_pick_folder,
```

In `apps/desktop/src-tauri/src/engine/mod.rs`, delete every `#[allow(dead_code)] // wired in Task 6` line added in Tasks 2-4 (on `EngineOpError`, its `Display` impl, and the five methods). They are now reachable from `main` via the commands.

- [ ] **Step 3: Run the gates to verify green**

Run (from `apps/desktop/src-tauri`):
```
cargo test
cargo clippy -- -D warnings
cargo check
```
Expected: all PASS, **with no `#[allow(dead_code)]` remaining** in `engine/mod.rs` — every method is now reachable, so clippy stays green without the allows. (If clippy reports a method as still dead, it isn't registered correctly — fix the `generate_handler!` entry, don't re-add the allow.)

- [ ] **Step 4: Verify the frontend dist exists then full Rust check**

Run (from repo root):
```
mkdir -p apps/desktop/dist
cargo check --manifest-path apps/desktop/src-tauri/Cargo.toml
```
Expected: PASS (matches the CI step at build.yml:65-70).

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src-tauri/src/commands/engine.rs apps/desktop/src-tauri/src/main.rs apps/desktop/src-tauri/src/engine/mod.rs
git status -s
git commit -m "feat(desktop): wire SP2 engine commands (grant/ingest/list/pick) + drop dead-code allows"
```

---

## Task 7: Frontend — API wrappers + pure helpers (TDD with vitest)

**Files:**
- Create: `apps/desktop/src/api/engine.ts`
- Create: `apps/desktop/src/sources/ingestSummary.ts` + `apps/desktop/src/sources/ingestSummary.test.ts`
- Create: `apps/desktop/src/sources/grants.ts` + `apps/desktop/src/sources/grants.test.ts`

- [ ] **Step 1: Write the failing tests**

`apps/desktop/src/sources/ingestSummary.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { ingestSummary } from "./ingestSummary";
import type { IngestReportDto } from "../api/engine";

describe("ingestSummary", () => {
  it("renders counts in a compact line", () => {
    const r: IngestReportDto = { ingested: 3, superseded: 1, deduped: 12, skipped: [{ path: "/x/a.bin", reason: "not valid UTF-8" }], failed: [] };
    expect(ingestSummary(r)).toBe("3 added · 1 updated · 12 unchanged · 1 skipped · 0 failed");
  });
});
```

`apps/desktop/src/sources/grants.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { activeGrants } from "./grants";
import type { GrantDto } from "../api/engine";

describe("activeGrants", () => {
  it("drops revoked grants", () => {
    const all: GrantDto[] = [
      { canonical_root: "/a", granted_at: "t1", revoked: false },
      { canonical_root: "/b", granted_at: "t2", revoked: true },
    ];
    expect(activeGrants(all).map((g) => g.canonical_root)).toEqual(["/a"]);
  });
});
```

- [ ] **Step 2: Run tests to verify they fail**

Run (from repo root): `npm run test --workspace @air-agent/desktop`
Expected: FAIL — modules not found.

- [ ] **Step 3: Write the implementations**

`apps/desktop/src/api/engine.ts`:

```ts
import { invoke } from "@tauri-apps/api/core";

export type GrantDto = { canonical_root: string; granted_at: string; revoked: boolean };
export type FileRecordDto = { canonical_path: string; file_event_id: string; content_hash: string; grant_root: string };
export type SkipDto = { path: string; reason: string };
export type IngestReportDto = {
  ingested: number;
  superseded: number;
  deduped: number;
  skipped: SkipDto[];
  failed: SkipDto[];
};

export const pickFolder = (): Promise<string | null> => invoke<string | null>("engine_pick_folder");
export const addGrant = (path: string): Promise<void> => invoke<void>("engine_add_grant", { path });
export const revokeGrant = (path: string): Promise<void> => invoke<void>("engine_revoke_grant", { path });
export const listGrants = (): Promise<GrantDto[]> => invoke<GrantDto[]>("engine_list_grants");
export const runIngest = (): Promise<IngestReportDto> => invoke<IngestReportDto>("engine_run_ingest");
export const listFiles = (): Promise<FileRecordDto[]> => invoke<FileRecordDto[]>("engine_list_files");
```

`apps/desktop/src/sources/ingestSummary.ts`:

```ts
import type { IngestReportDto } from "../api/engine";

export function ingestSummary(r: IngestReportDto): string {
  return [
    `${r.ingested} added`,
    `${r.superseded} updated`,
    `${r.deduped} unchanged`,
    `${r.skipped.length} skipped`,
    `${r.failed.length} failed`,
  ].join(" · ");
}
```

`apps/desktop/src/sources/grants.ts`:

```ts
import type { GrantDto } from "../api/engine";

export const activeGrants = (all: GrantDto[]): GrantDto[] => all.filter((g) => !g.revoked);
```

- [ ] **Step 4: Run tests + typecheck**

Run (from repo root):
```
npm run test --workspace @air-agent/desktop
npm run typecheck --workspace @air-agent/desktop
```
Expected: both PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src/api/engine.ts apps/desktop/src/sources/ingestSummary.ts apps/desktop/src/sources/ingestSummary.test.ts apps/desktop/src/sources/grants.ts apps/desktop/src/sources/grants.test.ts
git status -s
git commit -m "feat(desktop): SP2 engine API wrappers + ingest-summary/active-grants helpers (vitest)"
```

---

## Task 8: Frontend — the Sources panel + mount it

**Files:**
- Create: `apps/desktop/src/sources/SourcesPanel.tsx`
- Modify: `apps/desktop/src/settings/AirSettings.tsx` (mount `<SourcesPanel/>`)

This is UI wiring over Task 7's tested helpers; verified by typecheck + manual launch (Task 9).

- [ ] **Step 1: Write `SourcesPanel.tsx`**

```tsx
import { useEffect, useState } from "react";
import { Button } from "../components/Button";
import {
  pickFolder, addGrant, revokeGrant, listGrants, runIngest, listFiles,
  type GrantDto, type FileRecordDto, type IngestReportDto,
} from "../api/engine";
import { activeGrants } from "./grants";
import { ingestSummary } from "./ingestSummary";

export function SourcesPanel() {
  const [grants, setGrants] = useState<GrantDto[]>([]);
  const [files, setFiles] = useState<FileRecordDto[]>([]);
  const [busy, setBusy] = useState(false);
  const [summary, setSummary] = useState<IngestReportDto | null>(null);
  const [unavailable, setUnavailable] = useState(false);

  const refresh = async () => {
    try {
      setGrants(await listGrants());
      setFiles(await listFiles());
    } catch {
      setUnavailable(true); // e.g. not available on this platform (Windows pre-M7)
    }
  };

  useEffect(() => { void refresh(); }, []);

  if (unavailable) {
    return <p style={{ color: "#666" }}>The memory engine isn’t available on this platform yet.</p>;
  }

  const onAdd = async () => {
    const path = await pickFolder();
    if (!path) return;
    await addGrant(path);
    await refresh();
  };
  const onRevoke = async (path: string) => { await revokeGrant(path); await refresh(); };
  const onIngest = async () => {
    setBusy(true);
    try { setSummary(await runIngest()); await refresh(); }
    finally { setBusy(false); }
  };

  const active = activeGrants(grants);

  return (
    <div style={{ marginTop: 24, paddingTop: 16, borderTop: "1px solid #eee" }}>
      <div style={{ fontWeight: 600, marginBottom: 4 }}>Sources</div>
      <p style={{ color: "#666", fontSize: 13 }}>
        Folders the agent may read into its memory. Files are read locally and never leave your machine.
      </p>

      <div style={{ display: "flex", gap: 8, margin: "8px 0" }}>
        <Button variant="secondary" onClick={onAdd}>Add folder</Button>
        <Button variant="primary" onClick={onIngest} disabled={busy || active.length === 0}>
          {busy ? "Ingesting…" : "Ingest now"}
        </Button>
      </div>

      {summary ? <p style={{ fontSize: 13 }}>{ingestSummary(summary)}</p> : null}

      <ul style={{ paddingLeft: 18, fontSize: 13 }}>
        {active.map((g) => (
          <li key={g.canonical_root} style={{ marginBottom: 4 }}>
            <code>{g.canonical_root}</code>{" "}
            <button onClick={() => onRevoke(g.canonical_root)} style={{ marginLeft: 8 }}>Revoke</button>
          </li>
        ))}
        {active.length === 0 ? <li style={{ color: "#666", listStyle: "none" }}>No folders yet.</li> : null}
      </ul>

      {files.length > 0 ? (
        <details style={{ fontSize: 13 }}>
          <summary>{files.length} ingested file{files.length === 1 ? "" : "s"}</summary>
          <ul style={{ paddingLeft: 18 }}>
            {files.map((f) => <li key={f.file_event_id}><code>{f.canonical_path}</code></li>)}
          </ul>
        </details>
      ) : null}
    </div>
  );
}
```

- [ ] **Step 2: Mount it in `AirSettings.tsx`**

Add the import and render `<SourcesPanel/>` inside the `<Card>`, before the "Danger zone" block:

```tsx
import { SourcesPanel } from "../sources/SourcesPanel";
// ... inside the <Card>, before the Danger zone <div>:
        <SourcesPanel />
```

- [ ] **Step 3: Typecheck + build**

Run (from repo root):
```
npm run typecheck --workspace @air-agent/desktop
npm run build --workspace @air-agent/desktop
```
Expected: both PASS.

- [ ] **Step 4: (no separate test — covered by Task 7 helpers + Task 9 manual)**

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src/sources/SourcesPanel.tsx apps/desktop/src/settings/AirSettings.tsx
git status -s
git commit -m "feat(desktop): Sources panel (add/revoke folder, run ingest, list files)"
```

---

## Task 9: Build/security — model resource, fetch script, gitignore, cargo-deny

**Files:**
- Modify: `apps/desktop/src-tauri/tauri.conf.json` (`bundle.resources`)
- Create: `apps/desktop/src-tauri/resources/models/.gitkeep`
- Modify: `.gitignore`
- Create: `scripts/fetch-model.sh`
- Modify: `.github/workflows/build.yml` (engine network-free guard step)

> **Mechanism note (refines the spec).** The spec said "cargo-deny ban on hf-hub/ureq/reqwest." A *workspace-wide* cargo-deny ban is wrong here: the desktop crate depends on `reqwest` (Cargo.toml:20) and `bossclaw-core` itself carries `ureq`/`hf-hub` as *feature-gated* deps — a global ban would false-positive and red CI. The precise enforcement of the spec's intent ("the embedder stays network-free, so it needs no sandbox") is a **`bossclaw-core`-scoped `cargo tree` guard** on the default-feature graph, added below.

- [ ] **Step 1: Declare the model resource + keep the dir, ignore the blob**

In `apps/desktop/src-tauri/tauri.conf.json`, add to the `bundle` object:

```json
"resources": ["resources/models/potion-base-8M/*"]
```

Create `apps/desktop/src-tauri/resources/models/.gitkeep` (empty file, so the dir exists in CI even though the model is ignored).

Append to `.gitignore`:

```
# SP2: the bundled embedding model is fetched (hash-pinned), never committed.
/apps/desktop/src-tauri/resources/models/potion-base-8M/
```

- [ ] **Step 2: Verify `cargo check` tolerates the resource entry (no model present)**

Run (from repo root):
```
mkdir -p apps/desktop/dist
cargo check --manifest-path apps/desktop/src-tauri/Cargo.toml
```
Expected: PASS (a `bundle.resources` glob is only materialized at `tauri build`; `generate_context!` does not require the files at check time). If this FAILS on the missing resource, fall back to documenting the resource in `tauri.conf.json` only under a release profile — but it should pass.

- [ ] **Step 3: Write `scripts/fetch-model.sh`**

```bash
#!/usr/bin/env bash
# Fetch the potion-base-8M model2vec model into the Tauri resource dir, verifying
# committed sha256 pins. Run before `tauri dev`/`tauri build`. Fails closed on any
# mismatch. The model is MIT-licensed (minishlab) and never committed to git.
set -euo pipefail

DEST="apps/desktop/src-tauri/resources/models/potion-base-8M"
BASE="https://huggingface.co/minishlab/potion-base-8M/resolve/main"

# Pinned sha256 of each file (fill in on first run by inspecting the downloaded
# files, then commit these constants — DO NOT read them from the same response).
declare -A SHA=(
  ["model.safetensors"]="REPLACE_WITH_PINNED_SHA256"
  ["tokenizer.json"]="REPLACE_WITH_PINNED_SHA256"
  ["config.json"]="REPLACE_WITH_PINNED_SHA256"
)

mkdir -p "$DEST"
for f in "${!SHA[@]}"; do
  echo "fetching $f"
  curl -fsSL "$BASE/$f" -o "$DEST/$f"
  got=$(shasum -a 256 "$DEST/$f" | awk '{print $1}')
  want="${SHA[$f]}"
  if [ "$want" != "REPLACE_WITH_PINNED_SHA256" ] && [ "$got" != "$want" ]; then
    echo "ERROR: sha256 mismatch for $f (got $got, want $want)" >&2
    rm -f "$DEST/$f"
    exit 1
  fi
  echo "$f: $got"
done
echo "Model ready at $DEST"
```

Make it executable: `chmod +x scripts/fetch-model.sh`.

> First-run procedure (do once, then commit the pins): run the script, copy each printed `sha256` into the `SHA` map, re-run to confirm it verifies.

- [ ] **Step 4: Add the "engine network-free" guard to CI**

In `.github/workflows/build.yml`, add a step to the existing `bossclaw-core` job (after the clippy step, ~line 109):

```yaml
      - name: Engine network-free guard (embedder runs un-sandboxed only if its graph has no HTTP client)
        run: |
          if cargo tree -p bossclaw-core -e normal --prefix none | grep -qE '^(hf-hub|ureq|reqwest)( |$)'; then
            echo "FORBIDDEN: a network crate is in the default bossclaw-core graph"; exit 1
          fi
          echo "engine graph is network-free"
```

This fails the build if `model2vec-rs`'s `default-features = false` is ever reverted (pulling `hf-hub`), or if `ollama`/`fastembed` become default features (pulling `ureq`/`hf-hub`) — the exact regression that would invalidate the no-sandbox decision. It is scoped to `bossclaw-core`'s default graph, so it never touches the desktop's own `reqwest`.

- [ ] **Step 5: Verify the guard locally**

Run (from repo root):
```
cargo tree -p bossclaw-core -e normal --prefix none | grep -E '^(hf-hub|ureq|reqwest)( |$)' && echo "UNEXPECTED — investigate" || echo "engine graph is network-free (expected)"
```
Expected: prints `engine graph is network-free (expected)` (the grep matches nothing in the default graph).

- [ ] **Step 6: Commit**

```bash
git add apps/desktop/src-tauri/tauri.conf.json apps/desktop/src-tauri/resources/models/.gitkeep .gitignore scripts/fetch-model.sh .github/workflows/build.yml
git status -s
git commit -m "build(desktop): bundle potion-base-8M (hash-pinned fetch) + engine network-free CI guard"
```

---

## Task 10: Full gate + manual launch verification

**Files:** none (verification only; commit only if a fix is needed).

- [ ] **Step 1: Run every automated gate**

Run (from repo root):
```
mkdir -p apps/desktop/dist
cargo test -p bossclaw-core
cargo clippy -p bossclaw-core --all-targets -- -D warnings
( cd apps/desktop/src-tauri && cargo test && cargo clippy -- -D warnings && cargo check )
npm run typecheck --workspace @air-agent/desktop
npm run test --workspace @air-agent/desktop
cargo tree -p bossclaw-core -e normal --prefix none | grep -qE '^(hf-hub|ureq|reqwest)( |$)' && (echo "FORBIDDEN net crate in engine graph"; exit 1) || echo "engine network-free OK"
```
Expected: all green. Fix anything red before proceeding (TDD — write/adjust a test for any bug found).

- [ ] **Step 2: Fetch the model + manual launch**

```
./scripts/fetch-model.sh        # first run: fill in + commit the sha256 pins
npm run tauri dev --workspace @air-agent/desktop   # or the repo's dev launch command
```
Manual checklist (onboarded identity required):
- Settings → **Sources** panel renders.
- **Add folder** → native picker → choose a folder with a couple of `.txt`/`.md` files → it appears in the list.
- **Ingest now** → button shows "Ingesting…", then a summary like `2 added · 0 updated · 0 unchanged · 0 skipped · 0 failed`; the ingested files appear under the disclosure.
- **Ingest now** again → `0 added · … · 2 unchanged …` (dedup).
- **Revoke** a folder → it leaves the active list.
- Add a folder containing a binary/PDF file → it shows as **skipped** with a reason (native-only).

- [ ] **Step 3: Final status check**

```bash
git status -sb
git log --oneline main..HEAD
```
Expected: clean tree; the SP2 commits on `desktop-engine-ingest`. Ready to open a PR.

---

## Spec coverage check

- Embedder seam + bundled model + `MODEL_ID` single-source → Tasks 2, 9.
- Real vectors during ingest + record active model (closes SP3 trap) → Tasks 1, 4.
- Grant / revoke / list grants / run ingest / list files through the gated `EngineHandle` → Tasks 3, 4.
- DTOs + six commands + native folder picker → Tasks 5, 6.
- Sources panel (add/revoke/ingest/list, "Ingesting…", summary, skip reasons) → Tasks 7, 8.
- Native-only parsing → Task 4 (`ParserRouter::native_only()`); skipped non-text verified in Task 10.
- `EngineOpError` (zero SP1 churn) + in-flight `Busy` guard → Tasks 2, 4.
- Unix-gating → all desktop tasks (cfg(unix) module/handler entries).
- Engine network-free CI guard (scoped `cargo tree`) + model-blob hygiene (.gitignore same commit, committed sha256 pin, fail-closed) → Task 9.
- Hermetic tests (MockEmbedderProvider) + manual real-model launch → Tasks 2-7, 10.
