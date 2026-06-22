# Desktop Engine — Recall + Evolve Loop (SP3) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make SP2's persisted memory searchable (a "Memory" tab over hybrid recall) and self-organizing (a local-Ollama evolve loop, OFF by default), all through SP1's onboarding-gated `EngineHandle`.

**Architecture:** Mirror SP2's seams exactly — a lazy+cached `ReasonerProvider` (like `EmbedderProvider`) yields the engine's `OllamaReasoner`; new `EngineHandle` methods (`recall`, `evolve_once`, `evolve_status`, …) gate → `spawn_blocking` → engine call; a `tauri::async_runtime::spawn` scheduler ticks the loop; a React "Memory" tab drives it. Autonomous writes are forced off at first open. **Zero `bossclaw-core` code change** — SP3 consumes the engine's recall/evolve/reasoner/off-switch surface as-is. Design source of truth: [the Rev 2 spec](../specs/2026-06-23-sp3-recall-evolve-design.md) — read it alongside this plan.

**Tech Stack:** Rust (Tauri 2, tokio, `bossclaw-core` with the `ollama` feature), TypeScript/React (vitest), `reqwest` (desktop, Ollama probe), `ureq` (engine, loopback LLM — feature-gated).

**Branch:** `desktop-engine-recall-evolve` (already created off `main` `bbf1f03`). All new Rust + the Memory tab's engine calls are `#[cfg(unix)]`.

**Engine API reference (verified on `bbf1f03`, do not re-derive):** `recall(&dyn Embedder, &str, usize, &RecallOptions) -> Result<Vec<Hit>>` (log.rs:1321); `Hit { event_id:String, score:f32, sources:Vec<RecallSource>, kind:String }` (recall.rs:35); `RecallSource::{Vector,Keyword}` (recall.rs:21); `RecallOptions::default()` (recall.rs:75); `event_by_id(&str) -> Result<Option<Event>>` (log.rs:739); `evolve_once(&dyn Embedder, &dyn Reasoner) -> Result<EvolveReport>` (log.rs:5463); `EvolveReport{entities_minted,links_emitted,invalidates_emitted,pages_emitted,pages_superseded,memories_processed,proposals_emitted,proposals_rejected,proposals_elided_cap,skipped_disabled}` (evolve.rs:32); `evolve_status() -> Result<EvolveStatus>` (log.rs:6393); `EvolveStatus{queue_depth,last_tick_ms,error_count,last_error,enabled}` (evolve.rs:66 — only `queue_depth`+`enabled` live); `evolve_enabled()`/`set_evolve_enabled(bool)` (4819/4786); `proposals_enabled()`/`set_proposals_enabled(bool)` (4879/4843); `mandates_enabled()`/`set_mandates_enabled(bool)` (4949/~4909); `rebuild_indexes(&dyn Embedder)`/`rebuild_graph()` (1111/—); `Reasoner` trait `complete_json(&str,&str,&Value)->Result<Value>`+`model_id()->&str` (reason.rs:29); `OllamaReasoner::new(&str)` (ollama.rs:51, loopback-fail-closed); `ScriptedReasoner::new(&str).with_response(sys,prompt,Value)` (reason.rs:56). Public re-exports at `crates/bossclaw-core/src/lib.rs:49-79`.

---

## File Structure

**Create:**
- `apps/desktop/src-tauri/src/engine/reason.rs` — `ReasonerProvider` trait, `OllamaReasonerProvider`, `REASONER_MODEL_ID`, `#[cfg(test)] MockReasonerProvider`.
- `apps/desktop/src-tauri/src/engine/ollama_probe.rs` — desktop reqwest probe of `127.0.0.1:11434/api/tags` → `OllamaStatus`.
- `apps/desktop/src-tauri/src/engine/scheduler.rs` — `EvolveScheduler` (the `tauri::async_runtime::spawn` loop).
- `apps/desktop/src/memory/MemoryPanel.tsx` — the Memory tab.
- `apps/desktop/src/memory/recallView.ts` + `recallView.test.ts` — pure hit→row mapper + vitest.
- `apps/desktop/src/memory/evolveStatus.ts` + `evolveStatus.test.ts` — pure status formatter + vitest.

**Modify:**
- `apps/desktop/src-tauri/src/engine/mod.rs` — new fields/methods on `EngineHandle`; `EngineOpError` (`Reasoner`, `Busy(&'static str)`); `prime_switches` in `get_or_open`; `EvolveTelemetry`.
- `apps/desktop/src-tauri/src/engine/embed.rs` — (unchanged API; referenced).
- `apps/desktop/src-tauri/src/commands/engine.rs` — new commands + DTOs (`HitDto`, `EvolveStatusDto`, `EvolveReportDto`, `OllamaStatusDto`).
- `apps/desktop/src-tauri/src/air/identity.rs` — `#[derive(Clone)]` on `IdentityStore`.
- `apps/desktop/src-tauri/src/main.rs` — construct `OllamaReasonerProvider`, pass to `EngineHandle::new`, spawn the scheduler, register the 5 new commands.
- `apps/desktop/src-tauri/Cargo.toml` — `bossclaw-core` `features = ["ollama"]` (unix).
- `apps/desktop/src/App.tsx` — `View` += `"memory"`, nav button, body branch.
- `apps/desktop/src/api/engine.ts` — TS DTOs + `invoke` wrappers.
- `.github/workflows/build.yml` — two-graph network guard (idiom-fixed) + ollama clippy step.

---

## Task 1: `ReasonerProvider` seam + `EngineOpError::Reasoner`

**Files:**
- Create: `apps/desktop/src-tauri/src/engine/reason.rs`
- Modify: `apps/desktop/src-tauri/src/engine/mod.rs` (declare `pub mod reason;`, add `EngineOpError::Reasoner`, generalize `Busy`)
- Modify: `apps/desktop/src-tauri/Cargo.toml` (add the `ollama` feature — needed for `bossclaw_core::OllamaReasoner` to exist)

- [ ] **Step 1: Enable the engine `ollama` feature (so `OllamaReasoner` compiles)**

In `apps/desktop/src-tauri/Cargo.toml`, the `cfg(unix)` dependency:
```toml
[target.'cfg(unix)'.dependencies]
bossclaw-core = { path = "../../../crates/bossclaw-core", features = ["ollama"] }
zeroize = "1"
```

- [ ] **Step 2: Write the failing test** (`engine/reason.rs` `#[cfg(test)]` module)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mock_provider_yields_a_scripted_reasoner() {
        let p = MockReasonerProvider::new("test-model");
        let r = p.reasoner().expect("reasoner builds");
        assert_eq!(r.model_id(), "test-model");
    }
    #[test]
    fn ollama_provider_caches_one_instance() {
        let p = OllamaReasonerProvider::new();
        let a = p.reasoner().expect("a");
        let b = p.reasoner().expect("b");
        assert!(std::sync::Arc::ptr_eq(&a, &b), "second call returns the cached Arc");
    }
}
```

- [ ] **Step 3: Run it to verify it fails**

Run: `cargo test -p air_agent_desktop reason:: 2>&1 | tail -20`
Expected: FAIL (module/types not defined).

- [ ] **Step 4: Implement `reason.rs`**

```rust
//! The reasoner seam (mirrors engine/embed.rs): lazy, cached construction of the
//! local Ollama reasoner the evolve loop drives. A cloud reasoner can later drop in
//! behind this same trait with zero rework.
use std::sync::{Arc, Mutex};
use bossclaw_core::Reasoner;
use super::EngineOpError;

/// Single source of truth for the evolve reasoner's Ollama model tag (mirrors
/// embed::MODEL_ID). Unpinned for SP3 (the user pulls it via `ollama pull`).
pub const REASONER_MODEL_ID: &str = "qwen2.5:7b-instruct";

pub trait ReasonerProvider: Send + Sync {
    /// Build (and cache) the reasoner. Called on first evolve, never at startup.
    fn reasoner(&self) -> Result<Arc<dyn Reasoner>, EngineOpError>;
}

/// Production: yields `bossclaw_core::OllamaReasoner` (loopback-fail-closed), cached
/// for the process lifetime.
pub struct OllamaReasonerProvider {
    cell: Mutex<Option<Arc<dyn Reasoner>>>,
}
impl OllamaReasonerProvider {
    pub fn new() -> Self { Self { cell: Mutex::new(None) } }
}
impl Default for OllamaReasonerProvider {
    fn default() -> Self { Self::new() }
}
impl ReasonerProvider for OllamaReasonerProvider {
    fn reasoner(&self) -> Result<Arc<dyn Reasoner>, EngineOpError> {
        let mut g = self.cell.lock().expect("reasoner cell poisoned");
        if let Some(r) = g.as_ref() {
            return Ok(r.clone());
        }
        let r: Arc<dyn Reasoner> = Arc::new(bossclaw_core::OllamaReasoner::new(REASONER_MODEL_ID));
        *g = Some(r.clone());
        Ok(r)
    }
}

#[cfg(test)]
pub struct MockReasonerProvider {
    reasoner: Arc<dyn Reasoner>,
}
#[cfg(test)]
impl MockReasonerProvider {
    /// A reasoner with NO canned responses (only `model_id` is exercised). Tests that
    /// drive `evolve_once` build a `ScriptedReasoner` with `.with_response(...)` turns
    /// and wrap it via `from_scripted`.
    pub fn new(model_id: &str) -> Self {
        Self { reasoner: Arc::new(bossclaw_core::reason::ScriptedReasoner::new(model_id)) }
    }
    pub fn from_scripted(s: bossclaw_core::reason::ScriptedReasoner) -> Self {
        Self { reasoner: Arc::new(s) }
    }
}
#[cfg(test)]
impl ReasonerProvider for MockReasonerProvider {
    fn reasoner(&self) -> Result<Arc<dyn Reasoner>, EngineOpError> { Ok(self.reasoner.clone()) }
}
```

In `engine/mod.rs`: add `pub mod reason;`, and update `EngineOpError` (see §Task 3 for the full enum — this task only adds the `Reasoner` variant + generalizes `Busy`; if Task 3 hasn't run yet, add them now):
```rust
pub enum EngineOpError {
    Open(EngineError),
    Core(String),
    Embedder(String),
    Reasoner(String),       // NEW: reasoner build/transport failure
    Busy(&'static str),     // CHANGED from `Busy`: names the op ("ingest" | "evolve")
    Join(String),
}
// Display: add
//   EngineOpError::Reasoner(m) => write!(f, "reasoner unavailable: {m}"),
//   EngineOpError::Busy(op)    => write!(f, "an {op} is already running"),
```
Update the existing SP2 ingest call site (`run_ingest`): `.map_err(|_| EngineOpError::Busy("ingest"))`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p air_agent_desktop reason:: 2>&1 | tail -20` → PASS.
Run: `cargo build -p air_agent_desktop 2>&1 | tail -5` (confirms the `ollama` feature + `Busy` change compile; fix any other `Busy` match arms the compiler flags).

- [ ] **Step 6: Commit**

```bash
git add apps/desktop/src-tauri/src/engine/reason.rs apps/desktop/src-tauri/src/engine/mod.rs apps/desktop/src-tauri/Cargo.toml
git commit -m "feat(desktop): SP3 ReasonerProvider seam + ollama feature + EngineOpError::Reasoner"
```

**Review focus:** the cache cell pattern matches `embed.rs`; `Busy(&'static str)` updates ALL call sites; `ollama` feature doesn't break the default build.

---

## Task 2: Two-graph network-posture guard + ollama clippy (CI)

**Files:**
- Modify: `.github/workflows/build.yml` (the `bossclaw-core` job)

- [ ] **Step 1: Replace the SP2 single-graph guard with the two-graph guard**

Find the `Engine network-free guard` step and replace it; add the ollama clippy step right after `Clippy bossclaw-core (default features)`:
```yaml
      - name: Engine network-posture guard (two-graph)
        run: |
          if cargo tree -p bossclaw-core -e normal --prefix none \
               | grep -qE '^(hf-hub|ureq|reqwest)( |$)'; then
            echo "FORBIDDEN: a network crate is in the DEFAULT bossclaw-core graph"; exit 1; fi
          if cargo tree -p bossclaw-core -e normal --features ollama --prefix none \
               | grep -qE '^(hf-hub|reqwest)( |$)'; then
            echo "FORBIDDEN: hf-hub/reqwest in the ollama graph (only ureq allowed)"; exit 1; fi
          echo "network posture OK: default=zero-client, ollama=ureq-only"
      - name: Clippy bossclaw-core (ollama feature)
        run: cargo clippy -p bossclaw-core --features ollama --all-targets -- -D warnings
```

- [ ] **Step 2: Verify the guard locally (both graphs)**

Run:
```bash
cargo tree -p bossclaw-core -e normal --prefix none | grep -qE '^(hf-hub|ureq|reqwest)( |$)'; echo "default exit=$? (want 1)"
cargo tree -p bossclaw-core -e normal --features ollama --prefix none | grep -qE '^(hf-hub|reqwest)( |$)'; echo "ollama exit=$? (want 1)"
cargo clippy -p bossclaw-core --features ollama --all-targets -- -D warnings 2>&1 | tail -5
```
Expected: both `exit=1` (forbidden crates ABSENT → guard passes); clippy clean.

- [ ] **Step 3: Validate YAML**

Run: `ruby -ryaml -e 'YAML.load_file(".github/workflows/build.yml"); puts "ok"'` → `ok`.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/build.yml
git commit -m "build(ci): two-graph engine network guard (allow loopback ureq) + ollama clippy"
```

**Review focus:** the `if … then exit 1; fi` idiom (NOT inline `&& exit 1`, which reds CI under `pipefail`); `ureq` allowed only under `ollama`; embedder still `hf-hub`-free.

---

## Task 3: `EngineHandle` plumbing — fields, providers, `IdentityStore: Clone`

**Files:**
- Modify: `apps/desktop/src-tauri/src/engine/mod.rs` (struct fields, `new` signature, all call sites)
- Modify: `apps/desktop/src-tauri/src/air/identity.rs` (`#[derive(Clone)]`)
- Modify: `apps/desktop/src-tauri/src/main.rs` + `apps/desktop/src-tauri/src/air/identity.rs` test sites (4th `new` arg)

- [ ] **Step 1: Write the failing test** (extend `engine/mod.rs` tests)

```rust
#[tokio::test]
async fn handle_constructs_with_both_providers() {
    let (vault, dir) = test_vault_and_dir();          // existing SP2 test helper
    let handle = EngineHandle::new(
        vault,
        dir,
        std::sync::Arc::new(crate::engine::embed::MockEmbedderProvider::new(8)),
        std::sync::Arc::new(crate::engine::reason::MockReasonerProvider::new("m")),
    );
    // Not onboarded → recall gates closed (proves the handle is wired, gate intact).
    let err = handle.recall(false, "q".into(), 3).await.unwrap_err();
    assert!(matches!(err, EngineOpError::Open(EngineError::NotOnboarded)));
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p air_agent_desktop handle_constructs_with_both_providers 2>&1 | tail -20`
Expected: FAIL (`new` takes 3 args; `recall` undefined).

- [ ] **Step 3: Add fields + constructor arg**

In `engine/mod.rs`:
```rust
pub struct EngineHandle {
    cell: Mutex<Option<Arc<EventLog>>>,
    keystore: EngineKeystore,
    db_path: PathBuf,
    embedder_provider: Arc<dyn crate::engine::embed::EmbedderProvider>,
    reasoner_provider: Arc<dyn crate::engine::reason::ReasonerProvider>,  // NEW
    ingest_lock: Mutex<()>,
    evolve_lock: Mutex<()>,                       // NEW (tokio::sync::Mutex)
    indexed: Mutex<bool>,                         // NEW (tokio::sync::Mutex<bool>, false)
    evolve_tel: std::sync::Mutex<EvolveTelemetry>,// NEW (std mutex; status read path)
}

#[derive(Default, Clone)]
pub struct EvolveTelemetry {
    pub last_tick_ms: Option<u128>,
    pub error_count: usize,
    pub last_error: Option<String>,
}

impl EngineHandle {
    pub fn new(
        vault: Arc<dyn SecretsVault>,
        data_dir: PathBuf,
        embedder_provider: Arc<dyn crate::engine::embed::EmbedderProvider>,
        reasoner_provider: Arc<dyn crate::engine::reason::ReasonerProvider>,  // NEW
    ) -> Self {
        let db_path = data_dir.join("brain.db");
        Self {
            cell: Mutex::new(None),
            keystore: EngineKeystore::new(vault),
            db_path,
            embedder_provider,
            reasoner_provider,
            ingest_lock: Mutex::new(()),
            evolve_lock: Mutex::new(()),
            indexed: Mutex::new(false),
            evolve_tel: std::sync::Mutex::new(EvolveTelemetry::default()),
        }
    }
}
```

- [ ] **Step 4: `IdentityStore: Clone` + update all `EngineHandle::new` call sites**

In `air/identity.rs`, add `Clone` to the derive on `IdentityStore` (both fields are `Arc`/`PathBuf`):
```rust
#[derive(Clone)]
pub struct IdentityStore { /* unchanged fields */ }
```
Update every `EngineHandle::new(...)` call (run `grep -rn "EngineHandle::new" apps/desktop/src-tauri/src` — `main.rs:70`, `air/identity.rs:~122`, and ~8 sites in `engine/mod.rs` tests) to pass `Arc::new(OllamaReasonerProvider::new())` (prod) or `Arc::new(MockReasonerProvider::new("m"))` (tests) as the 4th arg.

- [ ] **Step 5: Run to verify it passes** (recall is added in Task 6; for now stub `recall` to make THIS test compile, or sequence Task 6 before re-running. Recommended: implement Task 6's `recall` now since this test calls it.)

> **Sequencing note:** Task 3's test calls `recall`; either implement Task 6 immediately after Step 3/4 here, or temporarily assert construction only. The subagent should do Task 3 + Task 6 together if needed to keep RED→GREEN tight.

Run: `cargo test -p air_agent_desktop -- engine:: 2>&1 | tail -20` → PASS after Task 6.

- [ ] **Step 6: Commit**

```bash
git add apps/desktop/src-tauri/src/engine/mod.rs apps/desktop/src-tauri/src/air/identity.rs apps/desktop/src-tauri/src/main.rs
git commit -m "feat(desktop): SP3 EngineHandle plumbing (reasoner provider, locks, telemetry) + IdentityStore Clone"
```

**Review focus:** `evolve_lock`/`indexed` are `tokio::Mutex`, `evolve_tel` is `std::sync::Mutex`; all `new` call sites updated; no field left uninitialized.

---

## Task 4: `prime_switches` — force evolve/proposals/mandates OFF at first open

**Files:**
- Modify: `apps/desktop/src-tauri/src/engine/mod.rs` (`get_or_open` first-open block + `prime_switches`)

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn first_open_forces_all_autonomy_switches_off() {
    let (vault, dir) = test_vault_and_dir();
    let handle = new_test_handle(vault, dir);          // helper from Task 3
    let log = handle.get_or_open(true).await.expect("opens");
    assert!(!log.evolve_enabled().unwrap(), "evolve off");
    assert!(!log.proposals_enabled().unwrap(), "proposals off");
    assert!(!log.mandates_enabled().unwrap(), "mandates off");
    // Idempotent: a second open writes no new config events.
    let n1 = log.count().unwrap();
    drop(log);
    *handle.cell.lock().await = None;                  // force re-open
    let log2 = handle.get_or_open(true).await.expect("re-opens");
    assert_eq!(log2.count().unwrap(), n1, "no duplicate config events on re-open");
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p air_agent_desktop first_open_forces_all_autonomy_switches_off 2>&1 | tail -20`
Expected: FAIL (`evolve_enabled()` returns true — default).

- [ ] **Step 3: Add `prime_switches` + call it in `get_or_open`'s first-open closure**

```rust
/// Force the three autonomy switches OFF (the engine defaults them ON when never set).
/// Sticky setters → writes at most once per flag; idempotent across opens.
fn prime_switches(log: &EventLog) -> Result<(), bossclaw_core::BossclawError> {
    if log.evolve_enabled()?    { log.set_evolve_enabled(false)?; }
    if log.proposals_enabled()? { log.set_proposals_enabled(false)?; }
    if log.mandates_enabled()?  { log.set_mandates_enabled(false)?; }
    Ok(())
}
```
In `get_or_open`'s `spawn_blocking` open closure, after `EventLog::open(...)`, chain priming and map any failure to the existing open-failure path (no new `EngineError` variant):
```rust
let log = EventLog::open(&db_path, &keys.dek, keys.signing_key)
    .and_then(|log| { Self::prime_switches(&log)?; Ok(log) })
    .map_err(|e| EngineError::KeystoreDbMismatch(e.to_string()))?;
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p air_agent_desktop first_open_forces_all_autonomy_switches_off 2>&1 | tail -20` → PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src-tauri/src/engine/mod.rs
git commit -m "feat(desktop): SP3 prime_switches — force evolve/proposals/mandates off at first open"
```

**Review focus:** all THREE flags (M6c mandate path needs `mandates_enabled`, not just `proposals_enabled`); idempotent (no event churn on re-open); failure reuses `KeystoreDbMismatch`.

---

## Task 5: `ensure_indexed` — lazy rebuild, flag set only on success

**Files:**
- Modify: `apps/desktop/src-tauri/src/engine/mod.rs` (`ensure_indexed`; `run_ingest` sets `*indexed=true`)

- [ ] **Step 1: Write the failing test** (recall round-trips through `ensure_indexed`; full recall asserts come in Task 6, so test the flag here)

```rust
#[tokio::test]
async fn ensure_indexed_builds_once_then_recall_finds_ingested_text() {
    let (vault, dir) = test_vault_and_dir();
    let handle = new_test_handle(vault, dir);
    let src = tempfile::tempdir().unwrap();
    std::fs::write(src.path().join("a.txt"), "ferris the crab loves rust").unwrap();
    handle.add_grant(true, src.path().to_path_buf()).await.unwrap();
    handle.run_ingest(true).await.unwrap();            // sets indexed=true after its rebuild
    let hits = handle.recall(true, "ferris crab".into(), 5).await.unwrap();
    assert!(hits.iter().any(|h| h.text.contains("ferris")), "recall finds the ingested text");
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p air_agent_desktop ensure_indexed_builds_once 2>&1 | tail -20`
Expected: FAIL (`ensure_indexed`/`recall` not defined).

- [ ] **Step 3: Implement `ensure_indexed` + set the flag in `run_ingest`**

```rust
/// Build the in-memory recall index from persisted vectors the first time it's needed.
/// Sets the flag ONLY after a successful rebuild, so a rebuild error stays retryable
/// (no silent-empty-recall trap). Returns the (cached) embedder for the caller.
async fn ensure_indexed(&self, log: &Arc<EventLog>) -> Result<Arc<dyn Embedder>, EngineOpError> {
    let embedder = self.embedder_provider.embedder()?;
    let mut done = self.indexed.lock().await;
    if !*done {
        let (log2, emb2) = (log.clone(), embedder.clone());
        spawn_blocking(move || -> Result<(), EngineOpError> {
            log2.rebuild_indexes(&*emb2).map_err(|e| EngineOpError::Core(e.to_string()))?;
            log2.rebuild_graph().map_err(|e| EngineOpError::Core(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))??;
        *done = true;
    }
    Ok(embedder)
}
```
In `run_ingest`, after a successful `ingest_all` (which rebuilds internally), mark the index current: `*self.indexed.lock().await = true;` (place it after the `spawn_blocking` returns Ok, outside the blocking closure).

- [ ] **Step 4: Run to verify it passes** (needs Task 6's `recall`)

Run: `cargo test -p air_agent_desktop ensure_indexed_builds_once 2>&1 | tail -20` → PASS after Task 6.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src-tauri/src/engine/mod.rs
git commit -m "feat(desktop): SP3 ensure_indexed (lazy rebuild, flag-on-success) + run_ingest marks index current"
```

**Review focus:** flag set AFTER success only (the Major review finding); `tokio::Mutex<bool>` serializes (no double rebuild); `run_ingest` keeps the flag truthful.

---

## Task 6: `EngineHandle::recall` + snippet hydration → `engine_recall` command + DTO

**Files:**
- Modify: `apps/desktop/src-tauri/src/engine/mod.rs` (`recall`, `HitWithText`)
- Modify: `apps/desktop/src-tauri/src/commands/engine.rs` (`engine_recall`, `HitDto`)
- Modify: `apps/desktop/src/api/engine.ts` (TS `HitDto` + `recall` wrapper)
- Modify: `apps/desktop/src-tauri/src/main.rs` (register `engine_recall`)

- [ ] **Step 1: Write the failing test** (DTO mapping is pure → unit test it; the handle path is covered by Task 5's test)

```rust
#[test]
fn hit_dto_maps_sources_to_snake_case() {
    let h = bossclaw_core::Hit {
        event_id: "e1".into(), score: 0.5,
        sources: vec![bossclaw_core::RecallSource::Vector, bossclaw_core::RecallSource::Keyword],
        kind: "memory".into(),
    };
    let dto = HitDto::from(HitWithText { hit: h, text: "hello".into() });
    assert_eq!(dto.sources, vec!["vector".to_string(), "keyword".to_string()]);
    assert_eq!(dto.text, "hello");
    assert_eq!(dto.kind, "memory");
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p air_agent_desktop hit_dto_maps_sources 2>&1 | tail -20` → FAIL.

- [ ] **Step 3: Implement `recall` (handle) + `HitWithText` + `HitDto` (command)**

`engine/mod.rs`:
```rust
pub struct HitWithText { pub hit: bossclaw_core::Hit, pub text: String }

async fn recall(&self, onboarded: bool, query: String, k: usize)
    -> Result<Vec<HitWithText>, EngineOpError>
{
    let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
    let embedder = self.ensure_indexed(&log).await?;
    spawn_blocking(move || -> Result<Vec<HitWithText>, EngineOpError> {
        let hits = log.recall(&*embedder, &query, k, &bossclaw_core::RecallOptions::default())
            .map_err(|e| EngineOpError::Core(e.to_string()))?;
        Ok(hits.into_iter().map(|h| {
            let text = log.event_by_id(&h.event_id).ok().flatten()
                .and_then(|e| e.content.get("text").and_then(|t| t.as_str()).map(str::to_owned))
                .unwrap_or_default();
            HitWithText { hit: h, text }
        }).collect())
    })
    .await
    .map_err(|e| EngineOpError::Join(e.to_string()))?
}
```
`commands/engine.rs`:
```rust
#[derive(serde::Serialize)]
pub struct HitDto { pub event_id: String, pub score: f32, pub kind: String,
    pub sources: Vec<String>, pub text: String }
impl From<crate::engine::HitWithText> for HitDto {
    fn from(h: crate::engine::HitWithText) -> Self {
        HitDto {
            event_id: h.hit.event_id, score: h.hit.score, kind: h.hit.kind, text: h.text,
            sources: h.hit.sources.iter().map(|s| match s {
                bossclaw_core::RecallSource::Vector => "vector".to_string(),
                bossclaw_core::RecallSource::Keyword => "keyword".to_string(),
            }).collect(),
        }
    }
}
#[tauri::command]
pub async fn engine_recall(query: String, k: usize, state: tauri::State<'_, AppState>)
    -> Result<Vec<HitDto>, String>
{
    let k = k.clamp(1, 50);
    let onboarded = state.identity_store.is_onboarded();
    state.engine.recall(onboarded, query, k).await
        .map(|v| v.into_iter().map(HitDto::from).collect())
        .map_err(|e| e.to_string())
}
```
`api/engine.ts`:
```ts
export type RecallSource = "vector" | "keyword";
export type HitDto = { event_id: string; score: number; kind: string; sources: RecallSource[]; text: string };
export const recall = (query: string, k: number): Promise<HitDto[]> =>
  invoke<HitDto[]>("engine_recall", { query, k });
```
`main.rs`: add `engine_recall` to `generate_handler!` (with `#[cfg(unix)]`).

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p air_agent_desktop -- engine:: 2>&1 | tail -20` (Tasks 3+5+6 tests now PASS) and `npm run typecheck --workspace @air-agent/desktop`.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src-tauri/src/engine/mod.rs apps/desktop/src-tauri/src/commands/engine.rs apps/desktop/src/api/engine.ts apps/desktop/src-tauri/src/main.rs
git commit -m "feat(desktop): SP3 recall — EngineHandle.recall + snippet hydration + engine_recall command"
```

**Review focus:** snippet hydration is best-effort (missing → empty, never errors); `k` clamped; sources snake_case matches the TS union; `RecallOptions::default()` (no page/file exclusion).

---

## Task 7: `EngineHandle::evolve_once` + telemetry + `evolve_status`

**Files:**
- Modify: `apps/desktop/src-tauri/src/engine/mod.rs` (`evolve_once`, `record_tick`, `evolve_status`)

- [ ] **Step 1: Write the failing test** (scripted reasoner; assert entities minted AND zero proposals with a mandate registered)

```rust
#[tokio::test]
async fn evolve_once_extracts_and_never_proposes_even_with_a_mandate() {
    use bossclaw_core::reason::ScriptedReasoner;
    let (vault, dir) = test_vault_and_dir();
    // Build a scripted reasoner priming Pass A + Pass B (+ adjudication/summarize) turns —
    // mirror crates/bossclaw-core/tests/evolve.rs::scripted_both_passes for the exact
    // (system,prompt)->response keys; reuse the engine's prompt builders.
    let scripted: ScriptedReasoner = build_scripted_for_one_memory();   // test helper
    let handle = new_test_handle_with_reasoner(vault, dir, scripted);
    // Seed a memory + a mandate, then enable evolve for THIS test (prime_switches set it off).
    let log = handle.get_or_open(true).await.unwrap();
    seed_one_memory(&log, "Kenny works at Acme");
    register_dummy_mandate(&log);                       // proves mandate path stays gated off
    log.set_evolve_enabled(true).unwrap();
    drop(log);
    let report = handle.evolve_once(true).await.unwrap();
    assert!(report.entities_minted >= 1, "extracted at least one entity");
    assert_eq!(report.proposals_emitted, 0, "no write proposals in SP3");
    // and no write_proposal events landed:
    let log = handle.get_or_open(true).await.unwrap();
    assert_eq!(count_events_of_type(&log, "write_proposal"), 0);
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p air_agent_desktop evolve_once_extracts_and_never_proposes 2>&1 | tail -20` → FAIL.

- [ ] **Step 3: Implement `evolve_once` + `record_tick` + `evolve_status`**

```rust
async fn evolve_once(&self, onboarded: bool) -> Result<EvolveReport, EngineOpError> {
    let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
    let _guard = self.evolve_lock.try_lock().map_err(|_| EngineOpError::Busy("evolve"))?;
    let embedder = self.ensure_indexed(&log).await?;
    let reasoner = self.reasoner_provider.reasoner()?;
    let t0 = std::time::Instant::now();
    let result = spawn_blocking({
        let log = log.clone(); let emb = embedder.clone();
        move || -> Result<EvolveReport, EngineOpError> {
            let r = log.evolve_once(&*emb, &*reasoner).map_err(|e| EngineOpError::Core(e.to_string()))?;
            log.rebuild_indexes(&*emb).map_err(|e| EngineOpError::Core(e.to_string()))?;
            log.rebuild_graph().map_err(|e| EngineOpError::Core(e.to_string()))?;
            Ok(r)
        }
    }).await.map_err(|e| EngineOpError::Join(e.to_string()))?;
    self.record_tick(t0.elapsed().as_millis(), &result);
    result
}

fn record_tick(&self, ms: u128, result: &Result<EvolveReport, EngineOpError>) {
    let mut tel = self.evolve_tel.lock().unwrap_or_else(|p| p.into_inner());  // poison-recover
    tel.last_tick_ms = Some(ms);
    if let Err(e) = result {
        tel.error_count += 1;
        let mut s = e.to_string(); s.truncate(512);     // cap (flows to the webview DTO)
        tel.last_error = Some(s);
    }
}

async fn evolve_status(&self, onboarded: bool)
    -> Result<(bossclaw_core::EvolveStatus, EvolveTelemetry), EngineOpError>
{
    let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
    let status = spawn_blocking(move || log.evolve_status().map_err(|e| EngineOpError::Core(e.to_string())))
        .await.map_err(|e| EngineOpError::Join(e.to_string()))??;
    let tel = self.evolve_tel.lock().unwrap_or_else(|p| p.into_inner()).clone();
    Ok((status, tel))
}
```
Also add `set_evolve_enabled` (gate → `spawn_blocking(log.set_evolve_enabled)`).

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p air_agent_desktop evolve_once_extracts_and_never_proposes 2>&1 | tail -20` → PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src-tauri/src/engine/mod.rs
git commit -m "feat(desktop): SP3 evolve_once (gated, locked, rebuild-after, telemetry) + evolve_status"
```

**Review focus:** `evolve_lock` `try_lock` → `Busy("evolve")`; rebuild after the tick (recall sees new dossiers); telemetry poison-recovers + caps `last_error`; the test is NON-vacuous (mandate registered, still zero proposals).

---

## Task 8: Evolve commands + DTOs (`evolve_status`, `set_evolve_enabled`, `evolve_now`)

**Files:**
- Modify: `apps/desktop/src-tauri/src/commands/engine.rs` (commands + `EvolveStatusDto`, `EvolveReportDto`)
- Modify: `apps/desktop/src/api/engine.ts` (TS DTOs + wrappers)
- Modify: `apps/desktop/src-tauri/src/main.rs` (register 3 commands)

- [ ] **Step 1: Write the failing test** (DTO merge is pure)

```rust
#[test]
fn evolve_status_dto_merges_engine_and_telemetry() {
    let st = bossclaw_core::EvolveStatus { queue_depth: 4, last_tick_ms: None, error_count: 0,
        last_error: None, enabled: true };
    let tel = crate::engine::EvolveTelemetry { last_tick_ms: Some(120), error_count: 2,
        last_error: Some("boom".into()) };
    let dto = EvolveStatusDto::from_parts(&st, &tel);
    assert_eq!(dto.queue_depth, 4);
    assert!(dto.enabled);
    assert_eq!(dto.last_tick_ms, Some(120));   // telemetry wins over the engine stub
    assert_eq!(dto.error_count, 2);
    assert_eq!(dto.last_error.as_deref(), Some("boom"));
}
```

- [ ] **Step 2: Run to verify it fails** → FAIL.

- [ ] **Step 3: Implement DTOs + commands**

```rust
#[derive(serde::Serialize)]
pub struct EvolveStatusDto {
    pub enabled: bool, pub queue_depth: usize,
    pub last_tick_ms: Option<u128>, pub error_count: usize, pub last_error: Option<String>,
}
impl EvolveStatusDto {
    pub fn from_parts(s: &bossclaw_core::EvolveStatus, t: &crate::engine::EvolveTelemetry) -> Self {
        EvolveStatusDto {
            enabled: s.enabled, queue_depth: s.queue_depth,
            last_tick_ms: t.last_tick_ms, error_count: t.error_count, last_error: t.last_error.clone(),
        }
    }
}
#[derive(serde::Serialize)]
pub struct EvolveReportDto { pub entities_minted: usize, pub links_emitted: usize,
    pub invalidates_emitted: usize, pub pages_emitted: usize, pub memories_processed: usize }
impl From<bossclaw_core::EvolveReport> for EvolveReportDto { /* field copy */ }

#[tauri::command]
pub async fn engine_evolve_status(state: tauri::State<'_, AppState>) -> Result<EvolveStatusDto, String> {
    let onboarded = state.identity_store.is_onboarded();
    match state.engine.evolve_status(onboarded).await {
        Ok((s, t)) => Ok(EvolveStatusDto::from_parts(&s, &t)),
        // never errors: report a disabled/empty status if the engine isn't open yet
        Err(_) => Ok(EvolveStatusDto { enabled: false, queue_depth: 0, last_tick_ms: None,
            error_count: 0, last_error: None }),
    }
}
#[tauri::command]
pub async fn engine_set_evolve_enabled(enabled: bool, state: tauri::State<'_, AppState>)
    -> Result<(), String> {
    let onboarded = state.identity_store.is_onboarded();
    state.engine.set_evolve_enabled(onboarded, enabled).await.map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn engine_evolve_now(state: tauri::State<'_, AppState>) -> Result<EvolveReportDto, String> {
    let onboarded = state.identity_store.is_onboarded();
    state.engine.evolve_once(onboarded).await.map(EvolveReportDto::from).map_err(|e| e.to_string())
}
```
`api/engine.ts`:
```ts
export type EvolveStatusDto = { enabled: boolean; queue_depth: number;
  last_tick_ms: number | null; error_count: number; last_error: string | null };
export type EvolveReportDto = { entities_minted: number; links_emitted: number;
  invalidates_emitted: number; pages_emitted: number; memories_processed: number };
export const evolveStatus = (): Promise<EvolveStatusDto> => invoke<EvolveStatusDto>("engine_evolve_status");
export const setEvolveEnabled = (enabled: boolean): Promise<void> => invoke<void>("engine_set_evolve_enabled", { enabled });
export const evolveNow = (): Promise<EvolveReportDto> => invoke<EvolveReportDto>("engine_evolve_now");
```
`main.rs`: register the 3 commands (`#[cfg(unix)]`).

- [ ] **Step 4: Run to verify it passes** → PASS; `npm run typecheck`.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src-tauri/src/commands/engine.rs apps/desktop/src/api/engine.ts apps/desktop/src-tauri/src/main.rs
git commit -m "feat(desktop): SP3 evolve commands (status/toggle/now) + DTOs"
```

**Review focus:** `engine_evolve_status` never errors (payload-encoded); telemetry overrides the engine stubs; DTO field names match TS twins.

---

## Task 9: Ollama detection — probe + `engine_ollama_status`

**Files:**
- Create: `apps/desktop/src-tauri/src/engine/ollama_probe.rs`
- Modify: `apps/desktop/src-tauri/src/commands/engine.rs` (`engine_ollama_status` + `OllamaStatusDto`)
- Modify: `apps/desktop/src/api/engine.ts`; `apps/desktop/src-tauri/src/main.rs`

- [ ] **Step 1: Write the failing test** (parse logic is pure)

```rust
#[test]
fn parses_model_present_from_tags_json() {
    let body = serde_json::json!({ "models": [ {"name":"qwen2.5:7b-instruct"}, {"name":"llama3"} ] });
    assert!(model_present_in_tags(&body, "qwen2.5:7b-instruct"));
    assert!(!model_present_in_tags(&body, "absent:1b"));
}
```

- [ ] **Step 2: Run to verify it fails** → FAIL.

- [ ] **Step 3: Implement the probe + command**

`engine/ollama_probe.rs`:
```rust
use serde_json::Value;
pub struct OllamaStatus { pub reachable: bool, pub model_present: bool }

pub fn model_present_in_tags(body: &Value, tag: &str) -> bool {
    body.get("models").and_then(|m| m.as_array())
        .map(|arr| arr.iter().any(|m| m.get("name").and_then(|n| n.as_str()) == Some(tag)))
        .unwrap_or(false)
}

/// Probe the LOCAL Ollama (hardcoded loopback host). Any error → not reachable.
pub async fn probe(model_tag: &str) -> OllamaStatus {
    let url = "http://127.0.0.1:11434/api/tags";
    match reqwest::Client::new().get(url)
        .timeout(std::time::Duration::from_secs(2)).send().await
    {
        Ok(resp) if resp.status().is_success() => match resp.json::<Value>().await {
            Ok(body) => OllamaStatus { reachable: true, model_present: model_present_in_tags(&body, model_tag) },
            Err(_) => OllamaStatus { reachable: true, model_present: false },
        },
        _ => OllamaStatus { reachable: false, model_present: false },
    }
}
```
`commands/engine.rs`:
```rust
#[derive(serde::Serialize)]
pub struct OllamaStatusDto { pub reachable: bool, pub model_present: bool, pub model_tag: String }
#[tauri::command]
pub async fn engine_ollama_status() -> Result<OllamaStatusDto, String> {
    let tag = crate::engine::reason::REASONER_MODEL_ID;
    let s = crate::engine::ollama_probe::probe(tag).await;
    Ok(OllamaStatusDto { reachable: s.reachable, model_present: s.model_present, model_tag: tag.to_string() })
}
```
`api/engine.ts`:
```ts
export type OllamaStatusDto = { reachable: boolean; model_present: boolean; model_tag: string };
export const ollamaStatus = (): Promise<OllamaStatusDto> => invoke<OllamaStatusDto>("engine_ollama_status");
```
`engine/mod.rs`: `pub mod ollama_probe;`. `main.rs`: register `engine_ollama_status` (`#[cfg(unix)]`).

- [ ] **Step 4: Run to verify it passes** → PASS; `npm run typecheck`.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src-tauri/src/engine/ollama_probe.rs apps/desktop/src-tauri/src/engine/mod.rs apps/desktop/src-tauri/src/commands/engine.rs apps/desktop/src/api/engine.ts apps/desktop/src-tauri/src/main.rs
git commit -m "feat(desktop): SP3 Ollama detection probe + engine_ollama_status"
```

**Review focus:** hardcoded loopback host (no user input); reqwest already a desktop dep (no engine-graph impact); errors → not-reachable (never throws).

---

## Task 10: `EvolveScheduler` + `main.rs` wiring

**Files:**
- Create: `apps/desktop/src-tauri/src/engine/scheduler.rs`
- Modify: `apps/desktop/src-tauri/src/main.rs` (construct reasoner provider, spawn scheduler)

- [ ] **Step 1: Write the failing test** (the tick *decision* is pure — extract + test it)

```rust
#[test]
fn tick_decision_gates_correctly() {
    use TickGate::*;
    assert_eq!(decide_tick(false, true, true, 5), Skip);   // not onboarded
    assert_eq!(decide_tick(true, false, true, 5), Skip);   // evolve disabled
    assert_eq!(decide_tick(true, true, false, 5), Skip);   // ollama unavailable
    assert_eq!(decide_tick(true, true, true, 0), Skip);    // empty queue
    assert_eq!(decide_tick(true, true, true, 5), Run);     // all conditions met
}
```

- [ ] **Step 2: Run to verify it fails** → FAIL.

- [ ] **Step 3: Implement the scheduler**

```rust
//! The background evolve driver. OFF by default (gates on the engine's evolve_enabled).
use std::sync::Arc;
use std::time::Duration;
use crate::engine::{EngineHandle, ollama_probe};
use crate::air::identity::IdentityStore;

pub const EVOLVE_INTERVAL: Duration = Duration::from_secs(300);   // ~5 min

#[derive(Debug, PartialEq, Eq)]
pub enum TickGate { Run, Skip }

/// Pure gating decision (unit-tested).
pub fn decide_tick(onboarded: bool, evolve_enabled: bool, ollama_ready: bool, queue_depth: usize) -> TickGate {
    if onboarded && evolve_enabled && ollama_ready && queue_depth > 0 { TickGate::Run } else { TickGate::Skip }
}

/// Spawn the loop via tauri::async_runtime::spawn (NOT tokio::spawn — setup() has no reactor).
pub fn spawn(engine: Arc<EngineHandle>, identity: IdentityStore) {
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(EVOLVE_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let onboarded = identity.is_onboarded();
            let evolve_enabled = engine.evolve_enabled_or_false(onboarded).await;
            let oll = ollama_probe::probe(crate::engine::reason::REASONER_MODEL_ID).await;
            let queue = engine.queue_depth_or_zero(onboarded).await;
            if decide_tick(onboarded, evolve_enabled, oll.reachable && oll.model_present, queue) == TickGate::Run {
                let _ = engine.evolve_once(onboarded).await;   // records telemetry; Busy → skip
            }
        }
    });
}
```
Add two tiny read helpers on `EngineHandle` (`evolve_enabled_or_false`, `queue_depth_or_zero`) that gate + return a safe default on error. In `main.rs` `setup`, after the engine is constructed (`#[cfg(unix)]`): build `OllamaReasonerProvider`, pass it as the 4th `EngineHandle::new` arg, then `crate::engine::scheduler::spawn(engine.clone(), identity_store.clone());`.

- [ ] **Step 4: Run to verify it passes** → PASS; `cargo build -p air_agent_desktop`.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src-tauri/src/engine/scheduler.rs apps/desktop/src-tauri/src/engine/mod.rs apps/desktop/src-tauri/src/main.rs
git commit -m "feat(desktop): SP3 EvolveScheduler (async_runtime::spawn, MissedTickBehavior::Skip, gated)"
```

**Review focus:** `tauri::async_runtime::spawn` (not `tokio::spawn`); `MissedTickBehavior::Skip`; gate is pure + tested; scheduler holds a cloned `IdentityStore` + `Arc<EngineHandle>`.

---

## Task 11: Frontend Memory tab

**Files:**
- Create: `apps/desktop/src/memory/MemoryPanel.tsx`, `recallView.ts`(+`.test.ts`), `evolveStatus.ts`(+`.test.ts`)
- Modify: `apps/desktop/src/App.tsx`

- [ ] **Step 1: Write the failing vitest** (`recallView.test.ts` + `evolveStatus.test.ts`)

```ts
// recallView.test.ts
import { describe, it, expect } from "vitest";
import { toRow } from "./recallView";
describe("toRow", () => {
  it("labels the kind and joins sources", () => {
    const r = toRow({ event_id: "e", score: 0.42, kind: "page", sources: ["vector","keyword"], text: "hi" });
    expect(r.kindLabel).toBe("Dossier");
    expect(r.sourcesLabel).toBe("vector + keyword");
    expect(r.score).toBe("0.42");
  });
});
// evolveStatus.test.ts
import { describe, it, expect } from "vitest";
import { formatEvolve } from "./evolveStatus";
describe("formatEvolve", () => {
  it("summarizes an enabled loop", () => {
    expect(formatEvolve({ enabled: true, queue_depth: 3, last_tick_ms: 120, error_count: 0, last_error: null }))
      .toBe("On · 3 queued · last tick 120ms · 0 errors");
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `npm run test --workspace @air-agent/desktop 2>&1 | tail -20` → FAIL (modules missing).

- [ ] **Step 3: Implement the pure helpers + the panel + nav**

`recallView.ts`:
```ts
import type { HitDto } from "../api/engine";
const KIND_LABEL: Record<string, string> = { memory: "Memory", page: "Dossier", file_ingested: "File" };
export type Row = { id: string; kindLabel: string; sourcesLabel: string; score: string; text: string };
export const toRow = (h: HitDto): Row => ({
  id: h.event_id,
  kindLabel: KIND_LABEL[h.kind] ?? h.kind,
  sourcesLabel: h.sources.join(" + "),
  score: h.score.toFixed(2),
  text: h.text,
});
```
`evolveStatus.ts`:
```ts
import type { EvolveStatusDto } from "../api/engine";
export const formatEvolve = (s: EvolveStatusDto): string =>
  `${s.enabled ? "On" : "Off"} · ${s.queue_depth} queued · ` +
  `last tick ${s.last_tick_ms == null ? "—" : `${s.last_tick_ms}ms`} · ${s.error_count} errors`;
```
`MemoryPanel.tsx`: a `SettingsSectionCard`-styled panel with a search `<input>` + a button → `recall(query, k)` → maps `toRow` into a list; an evolve card polling `evolveStatus()` + `ollamaStatus()`, a toggle (`setEvolveEnabled`) disabled until `reachable && model_present` (with the install hint), and an "Evolve now" button (`evolveNow`) → re-`refresh`. Follow `SourcesPanel.tsx` conventions (inline styles, `Button`, error via `String(e)` in a red `<p>`, `unavailable` early-return).
`App.tsx`: `type View = "identity" | "inbox" | "memory" | "settings";`; add a nav `<Button>` for `"memory"`; extend the body ternary with `: view === "memory" ? <MemoryPanel /> :` before the settings else.

- [ ] **Step 4: Run to verify it passes**

Run: `npm run test --workspace @air-agent/desktop 2>&1 | tail -20` → PASS; `npm run typecheck --workspace @air-agent/desktop`.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src/memory apps/desktop/src/App.tsx
git commit -m "feat(desktop): SP3 Memory tab — recall search + evolve status/controls"
```

**Review focus:** pure helpers have vitest; toggle gated on Ollama presence; mirrors SourcesPanel; ternary edit correct.

---

## Task 12: Whole-impl review, full gates, live-Ollama test, manual launch

**Files:** none new — verification + one `#[ignore]` test.

- [ ] **Step 1: Add one `#[ignore]` live-Ollama evolve test** (`engine/mod.rs` tests, mirroring `crates/bossclaw-core/tests/live_ollama.rs`): real `OllamaReasonerProvider`, enable evolve, ingest a small text, `evolve_once`, assert `entities_minted >= 1` and a follow-up `recall` surfaces a dossier. Gated `#[ignore]` so the default suite stays offline.

- [ ] **Step 2: Run every gate**

```bash
cargo build -p air_agent_desktop 2>&1 | tail -5
cargo test -p air_agent_desktop 2>&1 | tail -15
cargo clippy -p air_agent_desktop --all-targets -- -D warnings 2>&1 | tail -5
cargo test -p bossclaw-core 2>&1 | grep -E "test result:|FAILED" | tail -25
cargo clippy -p bossclaw-core --features ollama --all-targets -- -D warnings 2>&1 | tail -5
npm run typecheck --workspace @air-agent/desktop
npm run test --workspace @air-agent/desktop 2>&1 | tail -15
# two-graph network guard (both want exit 1 = forbidden ABSENT):
cargo tree -p bossclaw-core -e normal --prefix none | grep -qE '^(hf-hub|ureq|reqwest)( |$)'; echo "default=$?"
cargo tree -p bossclaw-core -e normal --features ollama --prefix none | grep -qE '^(hf-hub|reqwest)( |$)'; echo "ollama=$?"
```
Expected: all green; both guard checks print `1`.

- [ ] **Step 3: Whole-impl Opus review** — dispatch a reviewer over the full diff (`git diff main...HEAD`) against the Rev 2 spec's security invariants: loopback-only reasoner, all-three-switches-off (non-vacuous test), embedder still network-free, taint preserved, no second writer, `ensure_indexed` flag-on-success, scheduler spawn API. Fold any Critical/Important.

- [ ] **Step 4: Manual launch** (`npm run dev --workspace @air-agent/desktop` or the built app): onboard → ingest a folder → search in the Memory tab → (with Ollama up) flip Evolve on → "Evolve now" → watch queue drain + status update → search again, confirm a dossier appears. Capture a screenshot.

- [ ] **Step 5: Open the PR**

```bash
git push
gh pr create --base main --head desktop-engine-recall-evolve \
  --title "feat(desktop): SP3 recall + evolve loop — Memory tab, local Ollama, off by default" \
  --body "<summary from the spec + the per-task review trail>"
```

**Review focus:** every spec §"Security invariants" bullet has a corresponding test or verified gate; no `bossclaw-core` code changed; all 7+ CI checks green.

---

## Self-Review (against the Rev 2 spec)

- **Spec coverage:** §A ensure_indexed → T5; §B recall + Memory tab → T6/T11; §C ReasonerProvider → T1; §D Ollama detection → T9; §E scheduler + evolve_once → T7/T10; §F status/telemetry → T7/T8; §"Autonomous writes forced OFF" → T4 (+ non-vacuous test in T7); §"Network-posture guard" → T2; §"Error type" → T1/T3; commands+DTOs → T6/T8/T9; security invariants → T12. **All covered.**
- **Placeholder scan:** test-helper names (`build_scripted_for_one_memory`, `register_dummy_mandate`, `new_test_handle`, `test_vault_and_dir`) are SP2 test-harness patterns the implementer reuses/extends — each is named with its purpose; the scripted-reasoner helper explicitly mirrors `crates/bossclaw-core/tests/evolve.rs::scripted_both_passes`. Not placeholders for production code.
- **Type consistency:** `HitWithText` (T6) used in T6 DTO; `EvolveTelemetry` (T3) used in T7/T8; `EvolveStatusDto::from_parts` (T8) matches the merge in §F; `decide_tick`/`TickGate` (T10) self-contained; TS DTOs (`HitDto`/`EvolveStatusDto`/`EvolveReportDto`/`OllamaStatusDto`) match their Rust serde twins field-for-field.
- **Sequencing:** T3's construction test references `recall` (T6) — the implementer does T3+T6 together (noted in T3 Step 5). T5's test needs T6's `recall` (noted). Otherwise linear: T1→T2→T3→T4→T5→T6→T7→T8→T9→T10→T11→T12.
