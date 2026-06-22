//! The engine spine (SP1): a single live, encrypted `EventLog` wired into the desktop.
//! See docs/superpowers/specs/2026-06-22-desktop-engine-spine-design.md.

pub mod embed;
pub mod keystore;
pub mod reason;

use crate::engine::keystore::EngineKeystore;
use crate::secrets::SecretsVault;
use bossclaw_core::{Embedder, EventLog};
use serde::Serialize;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task::spawn_blocking;

/// Errors from opening / accessing the engine. Mapped to `EngineState` for the UI.
#[derive(Debug)]
pub enum EngineError {
    /// No identity yet — the brain is not created before onboarding.
    NotOnboarded,
    /// Exactly one of (brain key, DEK) is present — never re-mint (would orphan the DB).
    KeystoreInconsistent,
    /// The DB could not be opened with the stored DEK (wrong key or unopenable).
    KeystoreDbMismatch(String),
    /// A keychain or other I/O error.
    Vault(String),
    /// A background task failed to join.
    Join(String),
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EngineError::NotOnboarded => write!(f, "not onboarded"),
            EngineError::KeystoreInconsistent => write!(f, "engine keystore inconsistent"),
            EngineError::KeystoreDbMismatch(e) => write!(f, "engine keystore/DB mismatch: {e}"),
            EngineError::Vault(e) => write!(f, "engine keychain error: {e}"),
            EngineError::Join(e) => write!(f, "engine task error: {e}"),
        }
    }
}

/// Errors from the SP2 operational commands (grant/ingest/list). Wraps the
/// SP1 open/gate path so SP1's `EngineError`/`map_err_state`/`EngineState`
/// stay a status-only concern (untouched).
#[derive(Debug)]
pub enum EngineOpError {
    Open(EngineError),
    Core(String),
    Embedder(String),
    /// Reasoner BUILD failure — part of the `ReasonerProvider` seam's error surface. In SP3
    /// the only provider is `OllamaReasonerProvider`, whose `OllamaReasoner::new` is
    /// infallible (loopback is verified per-call inside `complete_json`, surfaced through
    /// `evolve_once` as `Core`), so nothing constructs this variant YET. It is load-bearing
    /// for the future fallible (cloud BYO-key) provider that drops in behind the same seam
    /// (spec §"Future hooks"); the `?` on `reasoner()` in `evolve_once` already routes to it.
    #[allow(dead_code)]
    Reasoner(String),
    /// A serialized op is already in flight; the `&'static str` names it ("ingest" | "evolve").
    Busy(&'static str),
    Join(String),
}

impl std::fmt::Display for EngineOpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineOpError::Open(e) => write!(f, "{e}"),
            EngineOpError::Core(m) => write!(f, "engine error: {m}"),
            EngineOpError::Embedder(m) => write!(f, "memory model unavailable: {m}"),
            EngineOpError::Reasoner(m) => write!(f, "reasoner unavailable: {m}"),
            EngineOpError::Busy(op) => write!(f, "an {op} is already running"),
            EngineOpError::Join(m) => write!(f, "engine task error: {m}"),
        }
    }
}

/// The coarse engine state surfaced to the UI (distinguishes setup states from faults).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineState {
    NotOnboarded,
    Ready,
    KeystoreInconsistent,
    KeystoreDbMismatch,
    ChainFailed,
}

#[derive(Debug, Clone, Serialize)]
pub struct EngineStatus {
    pub state: EngineState,
    pub event_count: i64,
    pub chain_ok: bool,
}

/// A recall `Hit` paired with its hydrated snippet text (`recall.rs::Hit` carries no text).
/// The command layer maps this to `HitDto`; the snippet is best-effort (missing → empty).
#[derive(Debug)]
pub struct HitWithText {
    pub hit: bossclaw_core::Hit,
    pub text: String,
}

/// Session-scoped evolve telemetry (the engine's own `EvolveStatus` stubs these fields to
/// `None/0/None`, so SP3 owns the real values). Reset on app restart; persistence is M7.
/// Written by `record_tick` + read by `evolve_status`/`EvolveStatusDto` (SP3 Tasks 7–8).
#[derive(Default, Clone)]
pub struct EvolveTelemetry {
    pub last_tick_ms: Option<u128>,
    pub error_count: usize,
    pub last_error: Option<String>,
}

/// The single chokepoint for engine access. Holds one lazily-opened `Arc<EventLog>`
/// behind an async mutex; `get_or_open` serializes first-open and gates on onboarding.
pub struct EngineHandle {
    cell: Mutex<Option<Arc<EventLog>>>,
    keystore: EngineKeystore,
    db_path: PathBuf,
    embedder_provider: Arc<dyn crate::engine::embed::EmbedderProvider>,
    /// Read by `evolve_once` (SP3 Task 7) to lazily build the reasoner on first evolve.
    reasoner_provider: Arc<dyn crate::engine::reason::ReasonerProvider>,
    ingest_lock: Mutex<()>,
    /// Serializes manual + scheduled evolve ticks (`try_lock` → `Busy("evolve")`).
    evolve_lock: Mutex<()>,
    /// `true` once the in-memory recall index reflects persisted vectors this session.
    /// Set ONLY after a successful rebuild (a failure stays retryable). See `ensure_indexed`.
    indexed: Mutex<bool>,
    /// The evolve status read path (a `std::sync::Mutex`, poison-recovered on read).
    /// Written by `record_tick` + read by `evolve_status` (SP3 Task 7).
    evolve_tel: std::sync::Mutex<EvolveTelemetry>,
}

impl EngineHandle {
    pub fn new(
        vault: Arc<dyn SecretsVault>,
        data_dir: PathBuf,
        embedder_provider: Arc<dyn crate::engine::embed::EmbedderProvider>,
        reasoner_provider: Arc<dyn crate::engine::reason::ReasonerProvider>,
    ) -> Self {
        Self {
            cell: Mutex::new(None),
            keystore: EngineKeystore::new(vault),
            db_path: data_dir.join("brain.db"),
            embedder_provider,
            reasoner_provider,
            ingest_lock: Mutex::new(()),
            evolve_lock: Mutex::new(()),
            indexed: Mutex::new(false),
            evolve_tel: std::sync::Mutex::new(EvolveTelemetry::default()),
        }
    }

    /// Open-or-get the single `EventLog`. The onboarding gate is enforced HERE (returns
    /// `NotOnboarded` if `!onboarded`) so no caller — including future SP3/SP4 commands —
    /// can bypass it. Holding the async-mutex guard across the open serializes concurrent
    /// first-opens, so exactly one `EventLog` is ever minted/constructed.
    pub async fn get_or_open(&self, onboarded: bool) -> Result<Arc<EventLog>, EngineError> {
        if !onboarded {
            return Err(EngineError::NotOnboarded);
        }
        let mut guard = self.cell.lock().await;
        if let Some(log) = guard.as_ref() {
            return Ok(log.clone());
        }
        let keystore = self.keystore.clone();
        let db_path = self.db_path.clone();
        let log = tokio::task::spawn_blocking(move || {
            let keys = keystore.load_or_mint()?;
            // Open bare, then force the three autonomy switches off. Any open/prime failure
            // maps to the existing open-failure path (KeystoreDbMismatch) — no new variant.
            EventLog::open(&db_path, &keys.dek, keys.signing_key)
                .and_then(|log| {
                    Self::prime_switches(&log)?;
                    Ok(log)
                })
                .map(Arc::new)
                .map_err(|e| EngineError::KeystoreDbMismatch(e.to_string()))
        })
        .await
        .map_err(|e| EngineError::Join(e.to_string()))??;
        // The `guard` held across the spawn_blocking open above is LOAD-BEARING: it
        // serializes first-open so exactly one EventLog is ever constructed. Do NOT
        // release it before storing here.
        *guard = Some(log.clone());
        Ok(log)
    }

    /// Get-or-open + verify the chain + count, mapped to a never-erroring `EngineStatus`.
    /// Open-failure (wrong DEK / unopenable) maps to `KeystoreDbMismatch`; an opened-but-
    /// tampered log maps to `ChainFailed` — the two are kept distinct (spec failure matrix).
    pub async fn status(&self, onboarded: bool) -> EngineStatus {
        let log = match self.get_or_open(onboarded).await {
            Ok(log) => log,
            Err(e) => return EngineStatus { state: map_err_state(&e), event_count: 0, chain_ok: false },
        };
        let probe = tokio::task::spawn_blocking(move || {
            let chain_ok = log.verify_chain().is_ok();
            let count = log.count().unwrap_or(0);
            (chain_ok, count)
        })
        .await;
        match probe {
            Ok((true, count)) => EngineStatus { state: EngineState::Ready, event_count: count, chain_ok: true },
            Ok((false, count)) => EngineStatus { state: EngineState::ChainFailed, event_count: count, chain_ok: false },
            Err(_) => EngineStatus { state: EngineState::KeystoreDbMismatch, event_count: 0, chain_ok: false },
        }
    }

    /// Grant read-access to `path` (canonicalized + appended by the engine). Gated.
    pub async fn add_grant(&self, onboarded: bool, path: PathBuf) -> Result<(), EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        tokio::task::spawn_blocking(move || {
            log.add_grant(&path).map(|_| ()).map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// Revoke a previously-granted folder. Gated.
    pub async fn revoke_grant(&self, onboarded: bool, path: PathBuf) -> Result<(), EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        tokio::task::spawn_blocking(move || {
            log.revoke_grant(&path).map(|_| ()).map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// Every grant (active + revoked); the UI filters to active. Gated.
    pub async fn list_grants(&self, onboarded: bool) -> Result<Vec<bossclaw_core::Grant>, EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        tokio::task::spawn_blocking(move || {
            log.grants().map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// Ingest every active granted folder (native text only), then record the
    /// active model once (so SP3 recall can discover it). Gated; serialized by an
    /// in-flight guard (a concurrent call returns `Busy`).
    pub async fn run_ingest(&self, onboarded: bool) -> Result<bossclaw_core::IngestReport, EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        let _guard = self.ingest_lock.try_lock().map_err(|_| EngineOpError::Busy("ingest"))?;
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
        // `ingest_all` rebuilds the in-memory index internally, so the index now reflects
        // persisted vectors — mark it current to skip a redundant first-recall rebuild.
        *self.indexed.lock().await = true;
        Ok(report)
    }

    /// Force the three autonomy switches OFF (the engine defaults them ON when never set).
    /// Each setter is sticky, so this writes at most once per flag and is idempotent across
    /// opens. Runs inside `get_or_open`'s first-open closure; failure reuses the open path.
    fn prime_switches(log: &EventLog) -> Result<(), bossclaw_core::BossclawError> {
        if log.evolve_enabled()? {
            log.set_evolve_enabled(false)?;
        }
        if log.proposals_enabled()? {
            log.set_proposals_enabled(false)?;
        }
        if log.mandates_enabled()? {
            log.set_mandates_enabled(false)?;
        }
        Ok(())
    }

    /// Build the in-memory recall index from persisted vectors the first time it's needed.
    /// The flag is set ONLY after a successful rebuild, so a rebuild error leaves it `false`
    /// and the next call retries (no silent-empty-recall trap). The `tokio::Mutex<bool>`
    /// serializes concurrent first-recalls (no double rebuild) and makes "set true only on
    /// success" race-free. Returns the (cached) embedder for the caller.
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

    /// Hybrid recall over persisted memory (semantic + keyword), with best-effort snippet
    /// hydration. Gated → `ensure_indexed` → `spawn_blocking(log.recall)`. A missing snippet
    /// becomes an empty string (never errors): `event_by_id` is best-effort per hit.
    pub async fn recall(
        &self,
        onboarded: bool,
        query: String,
        k: usize,
    ) -> Result<Vec<HitWithText>, EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        let embedder = self.ensure_indexed(&log).await?;
        spawn_blocking(move || -> Result<Vec<HitWithText>, EngineOpError> {
            let hits = log
                .recall(&*embedder, &query, k, &bossclaw_core::RecallOptions::default())
                .map_err(|e| EngineOpError::Core(e.to_string()))?;
            Ok(hits
                .into_iter()
                .map(|h| {
                    let text = log
                        .event_by_id(&h.event_id)
                        .ok()
                        .flatten()
                        .and_then(|e| e.content.get("text").and_then(|t| t.as_str()).map(str::to_owned))
                        .unwrap_or_default();
                    HitWithText { hit: h, text }
                })
                .collect())
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// Run ONE evolve tick (gated, serialized). Gate → `evolve_lock.try_lock()`
    /// (`Busy("evolve")` if a manual + scheduled tick overlap) → `ensure_indexed` (yields
    /// the embedder) → build the reasoner → `spawn_blocking`: `evolve_once` THEN
    /// `rebuild_indexes` THEN `rebuild_graph` (so recall sees the new entities/links/dossiers)
    /// → record telemetry. The post-tick rebuild is why a follow-up `recall` surfaces what
    /// this tick minted.
    pub async fn evolve_once(&self, onboarded: bool) -> Result<bossclaw_core::EvolveReport, EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        // Serialize manual + scheduled ticks: a second overlapping tick is Busy, not queued.
        let _guard = self.evolve_lock.try_lock().map_err(|_| EngineOpError::Busy("evolve"))?;
        let embedder = self.ensure_indexed(&log).await?;
        let reasoner = self.reasoner_provider.reasoner()?;
        let t0 = std::time::Instant::now();
        let result = spawn_blocking({
            let log = log.clone();
            let emb = embedder.clone();
            move || -> Result<bossclaw_core::EvolveReport, EngineOpError> {
                // Precondition: `evolve_once` resolves mentions via the entity-resolution
                // index, which `entity_search` requires be built (it errors otherwise). It is
                // SEPARATE from the recall index (`ensure_indexed` builds that) and is NOT
                // built by `open`/`ingest_all` — the engine only rebuilds it AT THE END of a
                // tick, so the FIRST tick on a freshly-opened log must build it here. Cheap:
                // it reads persisted entity_vectors (empty on a fresh store → empty index,
                // which is valid — every mention then mints). Mirrors the engine's own test
                // seed lifecycle (`tests/evolve.rs::seed_memory`).
                log.rebuild_entity_index(&*emb).map_err(|e| EngineOpError::Core(e.to_string()))?;
                let report = log
                    .evolve_once(&*emb, &*reasoner)
                    .map_err(|e| EngineOpError::Core(e.to_string()))?;
                // Fold the new vectors + edges so recall reflects this tick's curation.
                log.rebuild_indexes(&*emb).map_err(|e| EngineOpError::Core(e.to_string()))?;
                log.rebuild_graph().map_err(|e| EngineOpError::Core(e.to_string()))?;
                Ok(report)
            }
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?;
        self.record_tick(t0.elapsed().as_millis(), &result);
        result
    }

    /// Record one tick's telemetry. The lock is poison-RECOVERED (a panicked tick must not
    /// wedge the status read path), `last_tick_ms` is always set, and on error `error_count`
    /// is bumped and `last_error` stored TRUNCATED to ~512 bytes — engine error strings can
    /// embed paths / reasoner output and flow to the webview DTO, so the cap is a
    /// security-relevant bound (the Group A review flagged it).
    fn record_tick(&self, ms: u128, result: &Result<bossclaw_core::EvolveReport, EngineOpError>) {
        let mut tel = self.evolve_tel.lock().unwrap_or_else(|p| p.into_inner());
        tel.last_tick_ms = Some(ms);
        if let Err(e) = result {
            tel.error_count += 1;
            let mut s = e.to_string();
            truncate_on_char_boundary(&mut s, 512);
            tel.last_error = Some(s);
        }
    }

    /// Evolve status: the engine's live `{queue_depth, enabled}` plus a clone of the
    /// session telemetry (`{last_tick_ms, error_count, last_error}` — the engine stubs those).
    /// Gated → `spawn_blocking(log.evolve_status())` → poison-recovered telemetry clone.
    pub async fn evolve_status(
        &self,
        onboarded: bool,
    ) -> Result<(bossclaw_core::EvolveStatus, EvolveTelemetry), EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        let status = spawn_blocking(move || {
            log.evolve_status().map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))??;
        let tel = self.evolve_tel.lock().unwrap_or_else(|p| p.into_inner()).clone();
        Ok((status, tel))
    }

    /// Flip the sticky engine evolve off-switch (the toggle behind the Memory tab). Gated.
    pub async fn set_evolve_enabled(&self, onboarded: bool, enabled: bool) -> Result<(), EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        spawn_blocking(move || {
            log.set_evolve_enabled(enabled).map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// Every current ingested file (one per path). Gated.
    pub async fn list_files(&self, onboarded: bool) -> Result<Vec<bossclaw_core::graph::FileRecord>, EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        tokio::task::spawn_blocking(move || {
            log.current_files().map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// Identity-reset teardown: delete both engine slots, drop the memoized log (reset the
    /// cell to `None`), then delete brain.db.
    pub async fn teardown(&self) -> Result<(), EngineError> {
        self.keystore.delete()?;
        *self.cell.lock().await = None;
        if self.db_path.exists() {
            std::fs::remove_file(&self.db_path).map_err(|e| EngineError::Vault(e.to_string()))?;
        }
        Ok(())
    }
}

/// Truncate `s` in place to at most `max` bytes WITHOUT splitting a UTF-8 char (plain
/// `String::truncate` panics on a non-char-boundary). Walks back to the nearest boundary at
/// or below `max`. Used to cap `last_error` before it flows to the webview DTO.
fn truncate_on_char_boundary(s: &mut String, max: usize) {
    if s.len() <= max {
        return;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
}

fn map_err_state(e: &EngineError) -> EngineState {
    // Exhaustive on purpose (no `_` wildcard): a new EngineError variant must force a
    // deliberate state mapping here at compile time rather than silently defaulting.
    match e {
        EngineError::NotOnboarded => EngineState::NotOnboarded,
        EngineError::KeystoreInconsistent => EngineState::KeystoreInconsistent,
        EngineError::KeystoreDbMismatch(_) | EngineError::Vault(_) | EngineError::Join(_) => {
            EngineState::KeystoreDbMismatch
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::embed;
    use crate::secrets::SecretsVault;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    struct TestVault { store: Mutex<HashMap<String, String>> }
    impl TestVault { fn new() -> Arc<Self> { Arc::new(Self { store: Mutex::new(HashMap::new()) }) } }
    impl SecretsVault for TestVault {
        fn set(&self, k: &str, v: &str) -> Result<(), String> { self.store.lock().unwrap().insert(k.into(), v.into()); Ok(()) }
        fn get(&self, k: &str) -> Result<Option<String>, String> { Ok(self.store.lock().unwrap().get(k).cloned()) }
        fn delete(&self, k: &str) -> Result<(), String> { self.store.lock().unwrap().remove(k); Ok(()) }
    }

    /// A fresh `TestVault` + tempdir (the dir guard must outlive the handle). Mirrors the
    /// inline setup the SP2 tests use; centralised here for the SP3 recall/evolve tests.
    fn test_vault_and_dir() -> (Arc<TestVault>, tempfile::TempDir) {
        (TestVault::new(), tempfile::tempdir().unwrap())
    }

    /// A handle wired with the mock embedder + mock reasoner (the common SP3 test shape).
    fn new_test_handle(vault: Arc<TestVault>, dir: &tempfile::TempDir) -> EngineHandle {
        EngineHandle::new(
            vault,
            dir.path().to_path_buf(),
            Arc::new(embed::MockEmbedderProvider::new(8)),
            Arc::new(crate::engine::reason::MockReasonerProvider::new("m")),
        )
    }

    #[test]
    fn mock_embedder_provider_yields_a_working_embedder() {
        use crate::engine::embed::{EmbedderProvider, MockEmbedderProvider};
        let p = MockEmbedderProvider::new(8);
        let e = p.embedder().expect("mock embedder builds");
        let v = e.embed(&["hello".to_string()]).unwrap();
        assert_eq!(v[0].len(), 8);
        assert_eq!(e.model_id(), "mock-v1");
    }

    #[tokio::test]
    async fn not_onboarded_does_not_open_or_mint() {
        let dir = tempfile::tempdir().unwrap();
        let vault = TestVault::new();
        let h = EngineHandle::new(vault.clone(), dir.path().to_path_buf(), std::sync::Arc::new(embed::MockEmbedderProvider::new(8)), std::sync::Arc::new(crate::engine::reason::MockReasonerProvider::new("m")));
        let st = h.status(false).await;
        assert!(matches!(st.state, EngineState::NotOnboarded));
        // No keys minted, no DB created.
        assert!(vault.get("air-agent.engine.dek").unwrap().is_none());
        assert!(!dir.path().join("brain.db").exists());
    }

    #[tokio::test]
    async fn onboarded_opens_fresh_brain_and_memoizes() {
        let dir = tempfile::tempdir().unwrap();
        let vault = TestVault::new();
        let h = EngineHandle::new(vault, dir.path().to_path_buf(), std::sync::Arc::new(embed::MockEmbedderProvider::new(8)), std::sync::Arc::new(crate::engine::reason::MockReasonerProvider::new("m")));
        let st = h.status(true).await;
        assert!(matches!(st.state, EngineState::Ready), "state was {:?}", st.state);
        // First open primes the three autonomy switches OFF (SP3 `prime_switches`), so a
        // fresh brain holds exactly those 3 sticky `config` events — not zero.
        assert_eq!(st.event_count, 3);
        assert!(st.chain_ok);
        // Second call reuses the same instance (Arc ptr identical).
        let a = h.get_or_open(true).await.unwrap();
        let b = h.get_or_open(true).await.unwrap();
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[tokio::test]
    async fn wrong_dek_reports_keystore_db_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let vault = TestVault::new();
        // Open once to create the DB under the real minted DEK.
        let h1 = EngineHandle::new(vault.clone(), dir.path().to_path_buf(), std::sync::Arc::new(embed::MockEmbedderProvider::new(8)), std::sync::Arc::new(crate::engine::reason::MockReasonerProvider::new("m")));
        h1.get_or_open(true).await.unwrap();
        // Now corrupt the stored DEK and open with a FRESH handle (empty cell).
        vault.set("air-agent.engine.dek", &hex::encode([0u8; 32])).unwrap();
        let h2 = EngineHandle::new(vault, dir.path().to_path_buf(), std::sync::Arc::new(embed::MockEmbedderProvider::new(8)), std::sync::Arc::new(crate::engine::reason::MockReasonerProvider::new("m")));
        let st = h2.status(true).await;
        assert!(matches!(st.state, EngineState::KeystoreDbMismatch));
    }

    #[tokio::test]
    async fn teardown_removes_keys_db_and_resets_cell() {
        let dir = tempfile::tempdir().unwrap();
        let vault = TestVault::new();
        let h = EngineHandle::new(vault.clone(), dir.path().to_path_buf(), std::sync::Arc::new(embed::MockEmbedderProvider::new(8)), std::sync::Arc::new(crate::engine::reason::MockReasonerProvider::new("m")));
        h.get_or_open(true).await.unwrap();
        assert!(dir.path().join("brain.db").exists());
        h.teardown().await.unwrap();
        assert!(vault.get("air-agent.engine.signing_key").unwrap().is_none());
        assert!(!dir.path().join("brain.db").exists());
        // Cell is empty again: a not-onboarded call after teardown stays NotOnboarded.
        assert!(matches!(h.status(false).await.state, EngineState::NotOnboarded));
    }

    #[tokio::test]
    async fn grant_then_list_then_revoke() {
        let app_dir = tempfile::tempdir().unwrap();
        let src_dir = tempfile::tempdir().unwrap();
        let vault = TestVault::new();
        let h = EngineHandle::new(
            vault, app_dir.path().to_path_buf(),
            std::sync::Arc::new(embed::MockEmbedderProvider::new(8)),
            std::sync::Arc::new(crate::engine::reason::MockReasonerProvider::new("m")),
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
            std::sync::Arc::new(crate::engine::reason::MockReasonerProvider::new("m")),
        );
        h.add_grant(true, src_dir.path().to_path_buf()).await.unwrap();

        let report = h.run_ingest(true).await.unwrap();
        assert_eq!(report.ingested, 2);
        assert_eq!(report.failed.len(), 0);

        let files = h.list_files(true).await.unwrap();
        assert_eq!(files.len(), 2);

        // Re-ingest is a no-op: everything deduped, model already recorded.
        let again = h.run_ingest(true).await.unwrap();
        assert_eq!(again.ingested, 0);
        assert_eq!(again.deduped, 2);
    }

    // ---- SP3: recall core (Tasks 3/4/5/6) ----

    /// Task 3: the handle constructs with BOTH providers, and the recall gate is intact
    /// (not onboarded → `Open(NotOnboarded)`), proving the new field is wired without bypass.
    #[tokio::test]
    async fn handle_constructs_with_both_providers() {
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        let err = handle.recall(false, "q".into(), 3).await.unwrap_err();
        assert!(matches!(err, EngineOpError::Open(EngineError::NotOnboarded)));
    }

    /// Task 4: the first open forces all THREE autonomy switches off (the engine defaults
    /// them on), and priming is idempotent across re-opens (no duplicate config events).
    #[tokio::test]
    async fn first_open_forces_all_autonomy_switches_off() {
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        let log = handle.get_or_open(true).await.expect("opens");
        assert!(!log.evolve_enabled().unwrap(), "evolve off");
        assert!(!log.proposals_enabled().unwrap(), "proposals off");
        assert!(!log.mandates_enabled().unwrap(), "mandates off");
        // Idempotent: a second open (forced via clearing the cell) writes no new config events.
        let n1 = log.count().unwrap();
        drop(log);
        *handle.cell.lock().await = None;
        let log2 = handle.get_or_open(true).await.expect("re-opens");
        assert_eq!(log2.count().unwrap(), n1, "no duplicate config events on re-open");
    }

    /// Tasks 5 + 6: `run_ingest` marks the index current, then `recall` round-trips through
    /// `ensure_indexed` and hydrates the ingested snippet text.
    #[tokio::test]
    async fn ensure_indexed_builds_once_then_recall_finds_ingested_text() {
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        let src = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("a.txt"), "ferris the crab loves rust").unwrap();
        handle.add_grant(true, src.path().to_path_buf()).await.unwrap();
        handle.run_ingest(true).await.unwrap(); // sets indexed=true after its rebuild
        let hits = handle.recall(true, "ferris crab".into(), 5).await.unwrap();
        assert!(hits.iter().any(|h| h.text.contains("ferris")), "recall finds the ingested text");
    }

    // ---- SP3: evolve loop core (Task 7) ----

    /// A prompt-agnostic `Reasoner` test double for the evolve loop. The engine's
    /// `ScriptedReasoner` is SHA-256-keyed on the exact `(system, prompt)`, but
    /// `evolve_once` computes the recall/neighborhood context internally, so reproducing
    /// those keys at the desktop level is fragile. This stub instead inspects the `schema`
    /// arg and returns a schema-valid response:
    /// - extraction schema (Pass A / Pass B): one entity, no relations/retractions — so a
    ///   fresh store deterministically MINTS exactly one entity (no relation ⇒ empty floor
    ///   ⇒ Pass B keeps nothing ⇒ no proposal/link path is ever reached);
    /// - adjudication schema (mid-band entity resolution): `{"match":"none"}` (mint a fresh
    ///   entity). On a fresh store there are no candidates, so this arm is defensive.
    ///
    /// It mints WITHOUT prompt-prediction, which is exactly what the task's "recommended
    /// test approach" calls for.
    struct StubReasoner {
        model_id: String,
    }
    impl StubReasoner {
        fn new(model_id: &str) -> Self {
            Self { model_id: model_id.to_string() }
        }
    }
    impl bossclaw_core::Reasoner for StubReasoner {
        fn complete_json(
            &self,
            _system: &str,
            _prompt: &str,
            schema: &serde_json::Value,
        ) -> Result<serde_json::Value, bossclaw_core::BossclawError> {
            // Dispatch on the schema shape (NOT the prompt) so the response is always
            // valid for the call site regardless of the internally-computed context.
            if schema == &bossclaw_core::reason::adjudication_schema() {
                return Ok(serde_json::json!({ "match": "none" }));
            }
            // Default: the extraction schema (Pass A and Pass B both use it). One entity,
            // no relations ⇒ a fresh mention mints exactly one entity and nothing else.
            Ok(serde_json::json!({
                "entities": [
                    { "mention": "Kenny", "entity_type": "person", "confidence": 0.95 }
                ],
                "relations": [],
                "retractions": []
            }))
        }
        fn model_id(&self) -> &str {
            &self.model_id
        }
    }

    /// A handle wired with the mock embedder + an arbitrary reasoner (the evolve tests
    /// inject the prompt-agnostic `StubReasoner` here).
    fn new_test_handle_with_reasoner(
        vault: Arc<TestVault>,
        dir: &tempfile::TempDir,
        reasoner: Arc<dyn bossclaw_core::Reasoner>,
    ) -> EngineHandle {
        EngineHandle::new(
            vault,
            dir.path().to_path_buf(),
            Arc::new(embed::MockEmbedderProvider::new(8)),
            Arc::new(crate::engine::reason::MockReasonerProvider::from_reasoner(reasoner)),
        )
    }

    /// Seed one `memory` event directly (the evolve queue consumes `memory` events). The
    /// derive/rebuild lifecycle runs through `ensure_indexed`/`evolve_once`, so this only
    /// needs to land the event.
    fn seed_one_memory(log: &EventLog, text: &str) {
        let ev = bossclaw_core::Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: "memory".to_string(),
            content: serde_json::json!({ "text": text }),
            model_meta: None,
            prev_hash: String::new(),
            hash: None,
            signed_by_did: "did:wba:AIR-TEST".to_string(),
            signature: None,
        };
        log.append(ev).unwrap();
    }

    /// Count events of a given `event_type` in the log (used to prove zero `write_proposal`s).
    fn count_events_of_type(log: &EventLog, event_type: &str) -> usize {
        log.stream_all().unwrap().into_iter().filter(|e| e.event_type == event_type).count()
    }

    /// Task 7 (mint): with the stub returning one entity for the extraction schema, an
    /// enabled `evolve_once` over a seeded memory mints ≥1 entity, and a follow-up recall
    /// surfaces the new entity dossier-adjacent content (here: the entity itself is folded,
    /// so recall over the brain still succeeds without error post-evolve).
    #[tokio::test]
    async fn evolve_once_mints_at_least_one_entity() {
        let (vault, dir) = test_vault_and_dir();
        let handle =
            new_test_handle_with_reasoner(vault, &dir, Arc::new(StubReasoner::new("stub-v1")));
        let log = handle.get_or_open(true).await.unwrap();
        seed_one_memory(&log, "Kenny works at Acme");
        log.set_evolve_enabled(true).unwrap();
        drop(log);

        let report = handle.evolve_once(true).await.unwrap();
        assert!(report.entities_minted >= 1, "extracted at least one entity");
        // The post-evolve rebuild kept recall healthy (the new vectors fold in cleanly).
        let hits = handle.recall(true, "Kenny".into(), 5).await.unwrap();
        assert!(hits.iter().any(|h| h.text.contains("Kenny")), "recall surfaces the seeded memory");
    }

    /// Task 7 (proposals stay off — NON-VACUOUS): a dummy mandate is registered (so the M6c
    /// mandate path is reachable in principle), evolve is enabled, and a tick runs. The
    /// report MUST show zero proposals AND zero `write_proposal` events landed — proving
    /// BOTH the M6b `proposals_enabled` and M6c `mandates_enabled` gates hold (prime_switches).
    #[tokio::test]
    async fn evolve_once_emits_no_proposals_even_with_a_mandate() {
        let (vault, dir) = test_vault_and_dir();
        let handle =
            new_test_handle_with_reasoner(vault, &dir, Arc::new(StubReasoner::new("stub-v1")));
        let log = handle.get_or_open(true).await.unwrap();

        // Register a real mandate so active_mandates() is non-empty (non-vacuous). The
        // target must be write-granted AND outside every read-grant root (Finding A).
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        log.add_grant(src.path()).unwrap();
        log.add_write_grant(out.path()).unwrap();
        log.add_mandate(&out.path().join("index.md"), src.path(), "an index of titles").unwrap();
        assert_eq!(log.active_mandates().unwrap().len(), 1, "mandate is registered (non-vacuous)");

        seed_one_memory(&log, "Kenny works at Acme");
        log.set_evolve_enabled(true).unwrap();
        drop(log);

        let report = handle.evolve_once(true).await.unwrap();
        assert_eq!(report.proposals_emitted, 0, "no write proposals in SP3 (both gates off)");

        let log = handle.get_or_open(true).await.unwrap();
        assert_eq!(
            count_events_of_type(&log, "write_proposal"),
            0,
            "zero write_proposal events landed (proposals + mandates gated off)"
        );
        // The gates themselves are still off after the tick (prime_switches at open).
        assert!(!log.proposals_enabled().unwrap(), "proposals gate stays off");
        assert!(!log.mandates_enabled().unwrap(), "mandates gate stays off");
    }

    /// Task 7 (evolve_lock): a second concurrent `evolve_once` returns `Busy("evolve")`.
    /// The first tick holds `evolve_lock` across its `spawn_blocking`; the second `try_lock`
    /// fails. We force overlap by holding the lock guard directly while issuing a call.
    #[tokio::test]
    async fn second_concurrent_evolve_is_busy() {
        let (vault, dir) = test_vault_and_dir();
        let handle =
            new_test_handle_with_reasoner(vault, &dir, Arc::new(StubReasoner::new("stub-v1")));
        let log = handle.get_or_open(true).await.unwrap();
        seed_one_memory(&log, "Kenny works at Acme");
        log.set_evolve_enabled(true).unwrap();
        drop(log);

        // Hold the evolve lock to simulate an in-flight tick, then a real call must be Busy.
        let guard = handle.evolve_lock.try_lock().expect("first lock acquired");
        let err = handle.evolve_once(true).await.unwrap_err();
        assert!(matches!(err, EngineOpError::Busy("evolve")), "second tick is Busy(\"evolve\")");
        drop(guard);
        // After release, a tick succeeds again.
        handle.evolve_once(true).await.unwrap();
    }

    /// Task 7 (telemetry): after a successful tick, `evolve_status` reports `last_tick_ms`
    /// is set; a forced error path bumps `error_count` and caps `last_error` length.
    #[tokio::test]
    async fn evolve_status_reflects_telemetry_and_error_path() {
        let (vault, dir) = test_vault_and_dir();
        let handle =
            new_test_handle_with_reasoner(vault, &dir, Arc::new(StubReasoner::new("stub-v1")));
        let log = handle.get_or_open(true).await.unwrap();
        seed_one_memory(&log, "Kenny works at Acme");
        log.set_evolve_enabled(true).unwrap();
        drop(log);

        handle.evolve_once(true).await.unwrap();
        let (status, tel) = handle.evolve_status(true).await.unwrap();
        // The handle telemetry holds the REAL tick time; the engine's own EvolveStatus
        // honestly stubs last_tick_ms to None (it's the DTO merge that makes telemetry win —
        // covered by `evolve_status_dto_merges_engine_and_telemetry`).
        assert!(tel.last_tick_ms.is_some(), "a tick recorded its duration in telemetry");
        assert_eq!(status.last_tick_ms, None, "the engine status still stubs last_tick_ms");
        assert_eq!(tel.error_count, 0, "the successful tick recorded no error");

        // Force an error through record_tick and assert the bump + length cap directly
        // (the engine error path is exercised end-to-end by the live-Ollama test).
        let huge = "x".repeat(2000);
        handle.record_tick(7, &Err(EngineOpError::Core(huge)));
        let (_s, tel2) = handle.evolve_status(true).await.unwrap();
        assert_eq!(tel2.error_count, 1, "the forced error bumped error_count");
        let last = tel2.last_error.expect("last_error stored");
        assert!(last.len() <= 512, "last_error is capped to ~512 bytes (was {})", last.len());
    }

    /// Task 7 (toggle): `set_evolve_enabled` flips the sticky engine flag through the gate.
    #[tokio::test]
    async fn set_evolve_enabled_toggles_the_engine_flag() {
        let (vault, dir) = test_vault_and_dir();
        let handle =
            new_test_handle_with_reasoner(vault, &dir, Arc::new(StubReasoner::new("stub-v1")));
        // prime_switches forces it off at open.
        let (status0, _t) = handle.evolve_status(true).await.unwrap();
        assert!(!status0.enabled, "evolve starts disabled (prime_switches)");
        handle.set_evolve_enabled(true, true).await.unwrap();
        let (status1, _t) = handle.evolve_status(true).await.unwrap();
        assert!(status1.enabled, "toggle on takes effect");
    }
}
