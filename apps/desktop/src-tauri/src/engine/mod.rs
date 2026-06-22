//! The engine spine (SP1): a single live, encrypted `EventLog` wired into the desktop.
//! See docs/superpowers/specs/2026-06-22-desktop-engine-spine-design.md.

pub mod embed;
pub mod keystore;

use crate::engine::keystore::EngineKeystore;
use crate::secrets::SecretsVault;
use bossclaw_core::EventLog;
use serde::Serialize;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

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

/// The single chokepoint for engine access. Holds one lazily-opened `Arc<EventLog>`
/// behind an async mutex; `get_or_open` serializes first-open and gates on onboarding.
pub struct EngineHandle {
    cell: Mutex<Option<Arc<EventLog>>>,
    keystore: EngineKeystore,
    db_path: PathBuf,
    #[allow(dead_code)] // wired in Task 6 (ingest reads the provider + in-flight lock)
    embedder_provider: Arc<dyn crate::engine::embed::EmbedderProvider>,
    #[allow(dead_code)] // wired in Task 6
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
            EventLog::open(&db_path, &keys.dek, keys.signing_key)
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
        let h = EngineHandle::new(vault.clone(), dir.path().to_path_buf(), std::sync::Arc::new(embed::MockEmbedderProvider::new(8)));
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
        let h = EngineHandle::new(vault, dir.path().to_path_buf(), std::sync::Arc::new(embed::MockEmbedderProvider::new(8)));
        let st = h.status(true).await;
        assert!(matches!(st.state, EngineState::Ready), "state was {:?}", st.state);
        assert_eq!(st.event_count, 0);
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
        let h1 = EngineHandle::new(vault.clone(), dir.path().to_path_buf(), std::sync::Arc::new(embed::MockEmbedderProvider::new(8)));
        h1.get_or_open(true).await.unwrap();
        // Now corrupt the stored DEK and open with a FRESH handle (empty cell).
        vault.set("air-agent.engine.dek", &hex::encode([0u8; 32])).unwrap();
        let h2 = EngineHandle::new(vault, dir.path().to_path_buf(), std::sync::Arc::new(embed::MockEmbedderProvider::new(8)));
        let st = h2.status(true).await;
        assert!(matches!(st.state, EngineState::KeystoreDbMismatch));
    }

    #[tokio::test]
    async fn teardown_removes_keys_db_and_resets_cell() {
        let dir = tempfile::tempdir().unwrap();
        let vault = TestVault::new();
        let h = EngineHandle::new(vault.clone(), dir.path().to_path_buf(), std::sync::Arc::new(embed::MockEmbedderProvider::new(8)));
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
}
