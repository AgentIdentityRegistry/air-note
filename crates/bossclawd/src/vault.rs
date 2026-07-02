// Copied (read path only) from apps/desktop/src-tauri/src/vault.rs (M1a Task 4);
// the in-app original is removed in Task 6 (its webview `vault_set`/command surface
// stays app-side — the webview writes provider keys; the daemon only READS them).
//
// The daemon shares ONE provider-key blob with the app so a cloud reasoner enabled
// in the app is readable here without re-entry. This mirrors the app's `secret_get_cached`
// EXACTLY: same blob location (`service="AIR Agent"`, key `air_agent_vault_blob`), same
// legacy-blob migration source, same in-process cache. `cloud_reasoner::read_key` +
// `engine::current_key_fingerprint` consume it.

use keyring::{Entry, Error as KeyringError};
use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

/// The keychain service that holds the DEK + signing key (the vault-key seam: MUST match
/// the app's `vault::SERVICE_NAME` so the daemon and app open the same engine keystore).
pub const SERVICE_NAME: &str = "ai.air-agent.desktop";

// ---------------------------------------------------------------------------
// Provider-key blob (shared with the app). Read path only — the daemon never
// writes provider keys (the webview does, via the app's `vault_set`).
// ---------------------------------------------------------------------------

const BLOB_SERVICE: &str = "AIR Agent";
const BLOB_KEY: &str = "air_agent_vault_blob";
// Legacy blob location (pre-rename) — read-only migration source, matching the app.
const LEGACY_BLOB_SERVICE: &str = "BossClaw";
const LEGACY_BLOB_KEY: &str = "bossclaw_vault_blob";

static SECRET_CACHE: LazyLock<RwLock<Option<HashMap<String, String>>>> =
    LazyLock::new(|| RwLock::new(None));

fn blob_entry() -> Result<Entry, String> {
    Entry::new(BLOB_SERVICE, BLOB_KEY).map_err(|_| "Unable to access secure vault".to_string())
}

fn load_blob_from_keychain() -> Result<Option<HashMap<String, String>>, String> {
    let entry = blob_entry()?;
    match entry.get_password() {
        Ok(serialized) => {
            let parsed = serde_json::from_str::<HashMap<String, String>>(&serialized)
                .map_err(|_| "Unable to parse secure vault data".to_string())?;
            Ok(Some(parsed))
        }
        Err(KeyringError::NoEntry) => Ok(None),
        Err(_) => Err("Unable to read from secure vault".to_string()),
    }
}

/// Best-effort read of the pre-rename blob (the app migrates it on write; the daemon,
/// being read-only, just falls back to it if the current blob is absent). Never writes.
fn load_legacy_blob_from_keychain() -> Option<HashMap<String, String>> {
    let entry = Entry::new(LEGACY_BLOB_SERVICE, LEGACY_BLOB_KEY).ok()?;
    match entry.get_password() {
        Ok(serialized) => serde_json::from_str::<HashMap<String, String>>(&serialized).ok(),
        _ => None,
    }
}

fn write_cache(values: HashMap<String, String>) -> Result<(), String> {
    let mut cache = SECRET_CACHE
        .write()
        .map_err(|_| "Unable to access secure vault cache".to_string())?;
    *cache = Some(values);
    Ok(())
}

fn ensure_loaded_blob() -> Result<HashMap<String, String>, String> {
    {
        let cache = SECRET_CACHE
            .read()
            .map_err(|_| "Unable to access secure vault cache".to_string())?;
        if let Some(values) = cache.as_ref() {
            return Ok(values.clone());
        }
    }
    if let Some(values) = load_blob_from_keychain()? {
        write_cache(values.clone())?;
        return Ok(values);
    }
    // Current blob absent — fall back to the legacy one read-only (do NOT rewrite it;
    // the app owns migration on its next `vault_set`).
    let values = load_legacy_blob_from_keychain().unwrap_or_default();
    write_cache(values.clone())?;
    Ok(values)
}

/// Read a provider key from the shared blob (cached). Returns `None` when absent.
/// Consumed by `cloud_reasoner::read_key` + `engine::current_key_fingerprint`.
pub fn secret_get_cached(key: &str) -> Result<Option<String>, String> {
    let values = ensure_loaded_blob()?;
    Ok(values.get(key).cloned())
}

/// Clear the in-process cache so the next read re-hits the keychain. The app flips a
/// provider key without notifying the daemon, so the reasoner re-reads at CALL time and
/// this lets a long-lived daemon pick up a rotated key on the next boot-reseed / restart.
/// (Kept as a hook; the reasoner reads per-call so staleness is naturally bounded.)
#[allow(dead_code)]
pub fn secret_cache_clear() -> Result<(), String> {
    let mut cache = SECRET_CACHE
        .write()
        .map_err(|_| "Unable to access secure vault cache".to_string())?;
    *cache = None;
    Ok(())
}

/// Build the daemon's engine keystore vault over the SHARED service name
/// (`ai.air-agent.desktop`) — the same slot the app's `EngineKeystore` uses, so both
/// read the identical DEK/signing key (proven silent by the Task 0.5 keychain gate when
/// the daemon is co-signed with the app's identifier).
pub fn engine_vault() -> std::sync::Arc<dyn crate::secrets::SecretsVault> {
    #[cfg(target_os = "macos")]
    {
        std::sync::Arc::new(crate::secrets::macos::MacosVault::new(SERVICE_NAME))
    }
    #[cfg(target_os = "linux")]
    {
        std::sync::Arc::new(crate::secrets::linux::LinuxVault::new(SERVICE_NAME))
    }
    #[cfg(target_os = "windows")]
    {
        std::sync::Arc::new(crate::secrets::windows::WindowsVault::new(SERVICE_NAME))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        compile_error!("Unsupported platform — only macOS, Windows, Linux supported")
    }
}
