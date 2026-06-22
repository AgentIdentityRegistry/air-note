# Desktop Engine Spine (SP1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire the `bossclaw-core` memory engine into the AIR Agent desktop app: open one encrypted `EventLog` held in `AppState`, unlocked from the keychain with a separate brain key + DEK, proven via an `engine_status` command, and torn down on identity reset.

**Architecture:** A new `engine` module with two focused files — `keystore.rs` (mint/load/delete the brain key + DEK via the per-key `SecretsVault`, returning `Zeroizing` material; enforces the partial-state matrix) and `mod.rs` (`EngineHandle`: an async-safe `tokio::sync::OnceCell<Arc<EventLog>>` whose `get_or_open` is gated on onboarding and serializes first-open). A thin `commands/engine.rs` exposes `engine_status`. `reset_identity` is extended to tear the engine down.

**Tech Stack:** Rust, Tauri 2, `bossclaw_core::EventLog` (SQLCipher), `ed25519_dalek::SigningKey`, `zeroize::Zeroizing`, `tokio::sync::OnceCell`.

Spec: `docs/superpowers/specs/2026-06-22-desktop-engine-spine-design.md` (Rev 2).

---

## File Structure

- **Modify** `apps/desktop/src-tauri/Cargo.toml` — add `bossclaw-core` + `zeroize` deps.
- **Create** `apps/desktop/src-tauri/src/engine/mod.rs` — `EngineHandle`, `EngineError`, `EngineState`, `EngineStatus`; the lazy cell, gated `get_or_open`, `status`, `teardown`. Owns engine tests.
- **Create** `apps/desktop/src-tauri/src/engine/keystore.rs` — `EngineKeystore`, `EngineKeys`; mint/load/delete keys; partial-state matrix. Owns keystore tests.
- **Create** `apps/desktop/src-tauri/src/commands/engine.rs` — the `engine_status` `#[tauri::command]`.
- **Modify** `apps/desktop/src-tauri/src/main.rs` — declare `mod engine;`, build `EngineHandle` in `setup`, add it to `AppState`, register `engine_status`.
- **Modify** `apps/desktop/src-tauri/src/commands/identity.rs` — add `engine` to `AppState`; extend `reset_identity` to tear the engine down.
- **Modify** `apps/desktop/src-tauri/src/commands/mod.rs` — `pub mod engine;`.

---

## Task 1: Add the engine dependencies

**Files:**
- Modify: `apps/desktop/src-tauri/Cargo.toml`

- [ ] **Step 1: Add the dependencies**

In `apps/desktop/src-tauri/Cargo.toml`, under `[dependencies]`, add (the desktop already depends on `air-rs`, `ed25519-dalek`, `hex`, `tokio`, `tauri`):

```toml
bossclaw-core = { path = "../../../crates/bossclaw-core" }
zeroize = "1"
```

Do NOT enable any `bossclaw-core` features (no `fastembed`, `ollama`, `markitdown`) — the default build is the bare encrypted log (model2vec pure-Rust + bundled SQLCipher; no ONNX).

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p air_agent_desktop`
Expected: builds clean (first build pulls SQLCipher; slow but succeeds).

- [ ] **Step 3: Commit**

```bash
git add apps/desktop/src-tauri/Cargo.toml apps/desktop/src-tauri/Cargo.lock
git commit -m "build(desktop): add bossclaw-core + zeroize deps for the engine spine"
```

---

## Task 2: EngineKeystore — mint/load/delete keys (partial-state matrix)

**Files:**
- Create: `apps/desktop/src-tauri/src/engine/keystore.rs`
- Create: `apps/desktop/src-tauri/src/engine/mod.rs` (minimal, to host the module + `EngineError`)

- [ ] **Step 1: Create the module with `EngineError` and a stub keystore**

Create `apps/desktop/src-tauri/src/engine/mod.rs`:

```rust
//! The engine spine (SP1): a single live, encrypted `EventLog` wired into the desktop.
//! See docs/superpowers/specs/2026-06-22-desktop-engine-spine-design.md.
pub mod keystore;

use std::fmt;

/// Errors from opening / accessing the engine. Mapped to `EngineState` for the UI.
#[derive(Debug)]
pub enum EngineError {
    /// No identity yet — the brain is not created before onboarding.
    NotOnboarded,
    /// Exactly one of (brain key, DEK) is present — never re-mint (would orphan the DB).
    KeystoreInconsistent,
    /// The DB could not be opened with the stored DEK (wrong key or unopenable).
    KeystoreDbMismatch(String),
    /// The DB opened but its hash chain failed verification (tamper/truncation).
    ChainFailed,
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
            EngineError::ChainFailed => write!(f, "engine chain verification failed"),
            EngineError::Vault(e) => write!(f, "engine keychain error: {e}"),
            EngineError::Join(e) => write!(f, "engine task error: {e}"),
        }
    }
}
```

Create `apps/desktop/src-tauri/src/engine/keystore.rs`:

```rust
//! Mints / loads / deletes the engine's two secrets (brain Ed25519 key + DEK) via the
//! per-key `SecretsVault` — the same backend `IdentityStore` uses. Returns key material
//! in `Zeroizing` so it is wiped from memory on drop.

use crate::engine::EngineError;
use crate::secrets::SecretsVault;
use ed25519_dalek::SigningKey;
use rand_core::{OsRng, RngCore};
use std::sync::Arc;
use zeroize::Zeroizing;

/// Keychain slot for the engine's Ed25519 signing key (distinct from the identity key).
const SIGNING_KEY_SLOT: &str = "air-agent.engine.signing_key";
/// Keychain slot for the 32-byte SQLCipher data-encryption key.
const DEK_SLOT: &str = "air-agent.engine.dek";

/// The unlocked engine key material. DEK is zeroized on drop.
pub struct EngineKeys {
    pub dek: Zeroizing<[u8; 32]>,
    pub signing_key: SigningKey,
}

#[derive(Clone)]
pub struct EngineKeystore {
    vault: Arc<dyn SecretsVault>,
}

impl EngineKeystore {
    pub fn new(vault: Arc<dyn SecretsVault>) -> Self {
        Self { vault }
    }

    /// Load both secrets, or mint+persist both on first run. Errors `KeystoreInconsistent`
    /// if exactly one is present (never silently re-mints — that would orphan the DB).
    pub fn load_or_mint(&self) -> Result<EngineKeys, EngineError> {
        let sk = self.vault.get(SIGNING_KEY_SLOT).map_err(EngineError::Vault)?;
        let dek = self.vault.get(DEK_SLOT).map_err(EngineError::Vault)?;
        match (sk, dek) {
            (Some(sk_hex), Some(dek_hex)) => Ok(EngineKeys {
                signing_key: decode_signing_key(&sk_hex)?,
                dek: decode_dek(&dek_hex)?,
            }),
            (None, None) => self.mint(),
            _ => Err(EngineError::KeystoreInconsistent),
        }
    }

    fn mint(&self) -> Result<EngineKeys, EngineError> {
        let signing_key = SigningKey::generate(&mut OsRng);
        let mut dek = Zeroizing::new([0u8; 32]);
        OsRng.fill_bytes(dek.as_mut());
        // Persist BOTH before returning (so a half-mint never reaches open).
        self.vault
            .set(SIGNING_KEY_SLOT, &hex::encode(signing_key.to_bytes()))
            .map_err(EngineError::Vault)?;
        self.vault
            .set(DEK_SLOT, &hex::encode(dek.as_ref()))
            .map_err(EngineError::Vault)?;
        Ok(EngineKeys { dek, signing_key })
    }

    /// Delete both slots (identity-reset teardown). Best-effort over both.
    pub fn delete(&self) -> Result<(), EngineError> {
        let a = self.vault.delete(SIGNING_KEY_SLOT);
        let b = self.vault.delete(DEK_SLOT);
        a.and(b).map_err(EngineError::Vault)
    }
}

fn decode_dek(hex_str: &str) -> Result<Zeroizing<[u8; 32]>, EngineError> {
    let raw = Zeroizing::new(
        hex::decode(hex_str).map_err(|e| EngineError::KeystoreDbMismatch(e.to_string()))?,
    );
    if raw.len() != 32 {
        return Err(EngineError::KeystoreInconsistent);
    }
    let mut dek = Zeroizing::new([0u8; 32]);
    dek.copy_from_slice(&raw);
    Ok(dek)
}

fn decode_signing_key(hex_str: &str) -> Result<SigningKey, EngineError> {
    let raw = hex::decode(hex_str).map_err(|_| EngineError::KeystoreInconsistent)?;
    let bytes: [u8; 32] = raw
        .try_into()
        .map_err(|_| EngineError::KeystoreInconsistent)?;
    Ok(SigningKey::from_bytes(&bytes))
}
```

- [ ] **Step 2: Add `mod engine;` to main.rs so it compiles**

In `apps/desktop/src-tauri/src/main.rs`, near the other `mod` declarations, add:

```rust
mod engine;
```

Run: `cargo build -p air_agent_desktop` → compiles (the keystore is implemented; the engine module exists; unused-warning on `delete`/`EngineKeys` until Task 3 is fine).

- [ ] **Step 3: Write the tests**

Append to `apps/desktop/src-tauri/src/engine/keystore.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// Minimal in-memory vault for hermetic tests (mirrors secrets::tests::MockVault).
    struct TestVault {
        store: Mutex<HashMap<String, String>>,
    }
    impl TestVault {
        fn new() -> Arc<Self> {
            Arc::new(Self { store: Mutex::new(HashMap::new()) })
        }
    }
    impl SecretsVault for TestVault {
        fn set(&self, k: &str, v: &str) -> Result<(), String> {
            self.store.lock().unwrap().insert(k.into(), v.into());
            Ok(())
        }
        fn get(&self, k: &str) -> Result<Option<String>, String> {
            Ok(self.store.lock().unwrap().get(k).cloned())
        }
        fn delete(&self, k: &str) -> Result<(), String> {
            self.store.lock().unwrap().remove(k);
            Ok(())
        }
    }

    #[test]
    fn first_run_mints_both_slots_and_is_stable() {
        let vault = TestVault::new();
        let ks = EngineKeystore::new(vault.clone());
        let k1 = ks.load_or_mint().expect("mint");
        // Both slots now populated.
        assert!(vault.get(SIGNING_KEY_SLOT).unwrap().is_some());
        assert!(vault.get(DEK_SLOT).unwrap().is_some());
        // Second load returns the SAME bytes (no re-mint).
        let k2 = ks.load_or_mint().expect("load");
        assert_eq!(*k1.dek, *k2.dek);
        assert_eq!(k1.signing_key.to_bytes(), k2.signing_key.to_bytes());
    }

    #[test]
    fn partial_state_is_a_hard_error() {
        let vault = TestVault::new();
        vault.set(SIGNING_KEY_SLOT, &hex::encode([7u8; 32])).unwrap(); // only the key
        let ks = EngineKeystore::new(vault);
        assert!(matches!(ks.load_or_mint(), Err(EngineError::KeystoreInconsistent)));
    }

    #[test]
    fn delete_removes_both_slots() {
        let vault = TestVault::new();
        let ks = EngineKeystore::new(vault.clone());
        ks.load_or_mint().unwrap();
        ks.delete().unwrap();
        assert!(vault.get(SIGNING_KEY_SLOT).unwrap().is_none());
        assert!(vault.get(DEK_SLOT).unwrap().is_none());
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p air_agent_desktop engine::keystore 2>&1 | tail -20`
Expected: PASS (3 tests: first-run-mints-and-stable, partial-state error, delete removes both).
(For strict TDD, write the test module before the `impl EngineKeystore` body and watch it fail to compile first — the executor running subagent-driven-development does this per-task.)

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src-tauri/src/engine/mod.rs apps/desktop/src-tauri/src/engine/keystore.rs apps/desktop/src-tauri/src/main.rs
git commit -m "feat(desktop): engine keystore — mint/load/delete brain key + DEK (partial-state matrix)"
```

---

## Task 3: EngineHandle — lazy cell, gated open, status, teardown

**Files:**
- Modify: `apps/desktop/src-tauri/src/engine/mod.rs`

- [ ] **Step 1: Write the failing tests**

Append a `#[cfg(test)] mod tests` to `apps/desktop/src-tauri/src/engine/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
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

    #[tokio::test]
    async fn not_onboarded_does_not_open_or_mint() {
        let dir = tempfile::tempdir().unwrap();
        let vault = TestVault::new();
        let h = EngineHandle::new(vault.clone(), dir.path().to_path_buf());
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
        let h = EngineHandle::new(vault, dir.path().to_path_buf());
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
        let h1 = EngineHandle::new(vault.clone(), dir.path().to_path_buf());
        h1.get_or_open(true).await.unwrap();
        // Now corrupt the stored DEK and open with a FRESH handle (empty cell).
        vault.set("air-agent.engine.dek", &hex::encode([0u8; 32])).unwrap();
        let h2 = EngineHandle::new(vault, dir.path().to_path_buf());
        let st = h2.status(true).await;
        assert!(matches!(st.state, EngineState::KeystoreDbMismatch));
    }

    #[tokio::test]
    async fn teardown_removes_keys_db_and_resets_cell() {
        let dir = tempfile::tempdir().unwrap();
        let vault = TestVault::new();
        let h = EngineHandle::new(vault.clone(), dir.path().to_path_buf());
        h.get_or_open(true).await.unwrap();
        assert!(dir.path().join("brain.db").exists());
        h.teardown().await.unwrap();
        assert!(vault.get("air-agent.engine.signing_key").unwrap().is_none());
        assert!(!dir.path().join("brain.db").exists());
        // Cell is empty again: a not-onboarded call after teardown stays NotOnboarded.
        assert!(matches!(h.status(false).await.state, EngineState::NotOnboarded));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p air_agent_desktop engine:: 2>&1 | tail -20`
Expected: compile error — `EngineHandle`, `EngineState`, `status`, `get_or_open`, `teardown` not defined.

- [ ] **Step 3: Implement `EngineHandle`**

Add to `apps/desktop/src-tauri/src/engine/mod.rs` (after the `EngineError` block). The cell is a `tokio::sync::Mutex<Option<Arc<EventLog>>>` — an async mutex whose guard may be held across the `spawn_blocking` await, so it BOTH serializes concurrent first-opens (mint-once, exactly one `EventLog`) AND lets `teardown` reset the cell to `None` through `&self`:

```rust
use crate::engine::keystore::EngineKeystore;
use crate::secrets::SecretsVault;
use bossclaw_core::EventLog;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

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
}

impl EngineHandle {
    pub fn new(vault: Arc<dyn SecretsVault>, data_dir: PathBuf) -> Self {
        Self {
            cell: Mutex::new(None),
            keystore: EngineKeystore::new(vault),
            db_path: data_dir.join("brain.db"),
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
    match e {
        EngineError::NotOnboarded => EngineState::NotOnboarded,
        EngineError::KeystoreInconsistent => EngineState::KeystoreInconsistent,
        EngineError::ChainFailed => EngineState::ChainFailed,
        _ => EngineState::KeystoreDbMismatch,
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p air_agent_desktop engine:: 2>&1 | tail -20`
Expected: PASS (keystore 3 + engine 4 = 7 tests). `tempfile` is already a dev-dependency via the workspace; if `cargo test` reports it missing for this crate, add `tempfile` under `[dev-dependencies]` in the desktop `Cargo.toml`.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src-tauri/src/engine/mod.rs apps/desktop/src-tauri/Cargo.toml
git commit -m "feat(desktop): EngineHandle — gated lazy open, status probe, reset teardown"
```

---

## Task 4: engine_status command + AppState wiring

**Files:**
- Create: `apps/desktop/src-tauri/src/commands/engine.rs`
- Modify: `apps/desktop/src-tauri/src/commands/mod.rs`
- Modify: `apps/desktop/src-tauri/src/commands/identity.rs` (AppState)
- Modify: `apps/desktop/src-tauri/src/main.rs` (setup + register)

- [ ] **Step 1: Create the command**

Create `apps/desktop/src-tauri/src/commands/engine.rs`:

```rust
use crate::commands::identity::AppState;
use crate::engine::EngineStatus;
use tauri::State;

/// Reports the brain's status: opens-or-gets the engine (gated on onboarding), verifies
/// its chain, and counts entries. Never errors — failures are encoded in `status.state`.
#[tauri::command]
pub async fn engine_status(state: State<'_, AppState>) -> Result<EngineStatus, String> {
    let onboarded = state.identity_store.is_onboarded();
    Ok(state.engine.status(onboarded).await)
}
```

- [ ] **Step 2: Register the module**

In `apps/desktop/src-tauri/src/commands/mod.rs`, add `pub mod engine;`.

- [ ] **Step 3: Add `engine` to `AppState`**

In `apps/desktop/src-tauri/src/commands/identity.rs`, modify the `AppState` struct:

```rust
pub struct AppState {
    pub air_client: Arc<dyn AirClient>,
    pub identity_store: IdentityStore,
    pub inbox: std::sync::Arc<crate::inbox::manager::InboxManager>,
    pub engine: std::sync::Arc<crate::engine::EngineHandle>,
}
```

- [ ] **Step 4: Build the handle in `setup` + register the command**

In `apps/desktop/src-tauri/src/main.rs` `setup` closure, the `vault` is moved into `IdentityStore::new`. Clone the `Arc` first, and clone `data_dir` for the engine:

```rust
let identity_store = IdentityStore::new(vault.clone(), data_dir.clone());
let engine = std::sync::Arc::new(crate::engine::EngineHandle::new(vault, data_dir));
// ... air_client unchanged ...
app.manage(AppState {
    air_client,
    identity_store,
    inbox: std::sync::Arc::new(crate::inbox::manager::InboxManager::new()),
    engine,
});
```

In the `invoke_handler` macro list, add: `commands::engine::engine_status,`.

- [ ] **Step 5: Verify build + tests**

Run: `cargo build -p air_agent_desktop && cargo test -p air_agent_desktop engine:: 2>&1 | tail -8`
Expected: builds; engine tests still PASS.

- [ ] **Step 6: Commit**

```bash
git add apps/desktop/src-tauri/src/commands/engine.rs apps/desktop/src-tauri/src/commands/mod.rs apps/desktop/src-tauri/src/commands/identity.rs apps/desktop/src-tauri/src/main.rs
git commit -m "feat(desktop): engine_status command + wire EngineHandle into AppState"
```

---

## Task 5: Tear the engine down on identity reset

**Files:**
- Modify: `apps/desktop/src-tauri/src/commands/identity.rs`

- [ ] **Step 1: Write the failing test**

Append to `apps/desktop/src-tauri/src/commands/identity.rs` a test that drives the engine-teardown half of reset directly (the `reset_identity` command needs full Tauri `State`, which is awkward to fabricate; test the `EngineHandle::teardown` contract that reset will call, plus assert reset calls it via a thin helper):

```rust
#[cfg(test)]
mod tests {
    use crate::engine::{EngineHandle, EngineState};
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

    #[tokio::test]
    async fn reset_tears_down_the_engine() {
        let dir = tempfile::tempdir().unwrap();
        let vault = TestVault::new();
        let engine = Arc::new(EngineHandle::new(vault.clone(), dir.path().to_path_buf()));
        engine.get_or_open(true).await.unwrap();
        // Simulate the engine half of reset_identity:
        engine.teardown().await.unwrap();
        assert!(vault.get("air-agent.engine.dek").unwrap().is_none());
        assert!(!dir.path().join("brain.db").exists());
        assert!(matches!(engine.status(false).await.state, EngineState::NotOnboarded));
    }
}
```

- [ ] **Step 2: Run it**

Run: `cargo test -p air_agent_desktop commands::identity::tests 2>&1 | tail -12`
Expected: PASS. This pins the `EngineHandle::teardown` contract (implemented in Task 3) that `reset_identity` will call. (The `reset_identity` command itself takes a full Tauri `State`, awkward to fabricate in a unit test — so we pin the teardown contract here and wire it into the command in Step 3; the one-line wiring is covered by the build + the manual reset check in Task 7.)

- [ ] **Step 3: Wire teardown into `reset_identity`**

Modify `reset_identity` in `apps/desktop/src-tauri/src/commands/identity.rs`:

```rust
#[tauri::command]
pub async fn reset_identity(state: State<'_, AppState>) -> Result<(), String> {
    // Clear identity first, then tear the engine down so a re-onboard starts on a clean
    // brain (otherwise the OLD identity's memories silently re-attach — see spec Rev 2).
    state.identity_store.clear()?;
    state.engine.teardown().await.map_err(|e| e.to_string())?;
    Ok(())
}
```

- [ ] **Step 4: Verify build + tests**

Run: `cargo build -p air_agent_desktop && cargo test -p air_agent_desktop 2>&1 | tail -8`
Expected: builds; all desktop tests PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src-tauri/src/commands/identity.rs
git commit -m "fix(desktop): reset_identity tears down the engine (no orphaned brain across reset)"
```

---

## Task 6 (Optional): Settings "🧠 Brain ready" status line

**Files:**
- Create: `apps/desktop/src/api/engine.ts`
- Modify: `apps/desktop/src/<the Settings panel>.tsx` (the `AirSettings` view)

- [ ] **Step 1: Add the API wrapper** (mirrors `api/inbox.ts` style)

Create `apps/desktop/src/api/engine.ts`:

```ts
import { invoke } from "@tauri-apps/api/core";

export type EngineState =
  | "not_onboarded" | "ready" | "keystore_inconsistent"
  | "keystore_db_mismatch" | "chain_failed";

export interface EngineStatus {
  state: EngineState;
  event_count: number;
  chain_ok: boolean;
}

export const engineStatus = () => invoke<EngineStatus>("engine_status");
```

- [ ] **Step 2: Render it in Settings**

In the Settings panel, add a `useEffect` that calls `engineStatus()` on mount and renders one line, e.g.:
- `ready` → `🧠 Brain ready · {event_count} memories`
- `not_onboarded` → `🧠 Brain: set up an identity first`
- anything else → `⚠️ Brain: {state}`

- [ ] **Step 3: Typecheck**

Run: `npm run typecheck --workspace @air-agent/desktop`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add apps/desktop/src/api/engine.ts apps/desktop/src/*Settings*.tsx
git commit -m "feat(desktop): Settings shows brain status (engine_status)"
```

---

## Task 7: Full gate + PR

- [ ] **Step 1: Run all gates**

```bash
cargo build -p air_agent_desktop
cargo test -p air_agent_desktop 2>&1 | tail -8
cargo clippy -p air_agent_desktop --all-targets -- -D warnings 2>&1 | tail -4
npm run typecheck --workspace @air-agent/desktop   # only if Task 6 done
```
Expected: all green.

- [ ] **Step 2: Open the PR**

```bash
git push -u origin desktop-engine-spine
gh pr create --base main --title "feat(desktop): engine spine (SP1) — bossclaw-core EventLog wired into the app" --body "<summary + spec link + the dual-review note>"
```

---

## Notes for the implementer

- **Keys never logged.** Do not add `Debug`/`println!` of `EngineKeys`, the DEK, or the signing key. `EngineError::KeystoreDbMismatch(String)` carries the engine's error string, which (verified) does NOT echo key bytes.
- **`is_onboarded` proxy.** The gate uses `IdentityStore::is_onboarded()`. The engine mints its OWN brain key regardless of identity — the gate is policy (no brain before onboarding), not a key dependency.
- **`tempfile` dev-dep.** If the desktop crate lacks it under `[dev-dependencies]`, add `tempfile = "3"`.
- **Recall deferred:** bare `EventLog::open` only. SP3 swaps in `open_with_recall`.
- **`rand_core`/`OsRng`:** already used in `air/did_wba.rs`. If `rand_core` isn't a direct dep, import `OsRng` via the same path `did_wba.rs` uses.
