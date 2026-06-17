use crate::secrets::SecretsVault;
use keyring::{Entry, Error as KeyringError};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, RwLock};

pub const SERVICE_NAME: &str = "ai.air-agent.desktop";
/// Legacy SecretsVault service (pre-rename); also the legacy Tauri bundle id, so it
/// doubles as the old app-data dir name for `migrate_identity_metadata`.
pub const LEGACY_IDENTITY_SERVICE: &str = "ai.bossclaw.desktop";

pub fn vault_for_service(service: &str) -> Arc<dyn SecretsVault> {
    #[cfg(target_os = "macos")]
    return Arc::new(crate::secrets::macos::MacosVault::new(service));

    #[cfg(target_os = "windows")]
    return Arc::new(crate::secrets::windows::WindowsVault::new(service));

    #[cfg(target_os = "linux")]
    return Arc::new(crate::secrets::linux::LinuxVault::new(service));

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    compile_error!("Unsupported platform — only macOS, Windows, Linux supported");
}

pub fn default_vault() -> Arc<dyn SecretsVault> {
    vault_for_service(SERVICE_NAME)
}

// ---------------------------------------------------------------------------
// Legacy blob-based helpers — used by llm_stream and web_access modules.
// These will be migrated to SecretsVault in Phase 2.
// ---------------------------------------------------------------------------

const BLOB_SERVICE: &str = "AIR Agent";
const BLOB_KEY: &str = "air_agent_vault_blob";
// Legacy blob + individual-key location (pre-rename) — read-only migration source.
const LEGACY_BLOB_SERVICE: &str = "BossClaw";
const LEGACY_BLOB_KEY: &str = "bossclaw_vault_blob";
const LEGACY_KEYS: [&str; 7] = [
    "session_jwt",
    "openai_compat_api_key",
    "openai_api_key",
    "anthropic_api_key",
    "google_api_key",
    "brave_api_key",
    "tavily_api_key",
];

static SECRET_CACHE: LazyLock<RwLock<Option<HashMap<String, String>>>> =
    LazyLock::new(|| RwLock::new(None));

fn blob_entry() -> Result<Entry, String> {
    Entry::new(BLOB_SERVICE, BLOB_KEY)
        .map_err(|_| "Unable to access secure vault".to_string())
}

fn legacy_entry(key: &str) -> Result<Entry, String> {
    Entry::new(LEGACY_BLOB_SERVICE, key).map_err(|_| "Unable to access secure vault".to_string())
}

fn load_blob_from_keychain() -> Result<Option<HashMap<String, String>>, String> {
    let entry = blob_entry()?;

    #[cfg(debug_assertions)]
    eprintln!("vault_get miss -> keychain read: {}", BLOB_KEY);

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

fn save_blob_to_keychain(values: &HashMap<String, String>) -> Result<(), String> {
    let serialized = serde_json::to_string(values)
        .map_err(|_| "Unable to serialize secure vault data".to_string())?;
    let entry = blob_entry()?;
    entry
        .set_password(&serialized)
        .map_err(|_| "Unable to save to secure vault".to_string())
}

fn read_legacy_key(key: &str) -> Result<Option<String>, String> {
    let entry = legacy_entry(key)?;
    #[cfg(debug_assertions)]
    eprintln!("vault_get miss -> keychain read: {}", key);
    match entry.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(_) => Err("Unable to read from secure vault".to_string()),
    }
}

fn delete_legacy_key_best_effort(key: &str) {
    let Ok(entry) = legacy_entry(key) else {
        return;
    };

    match entry.delete_password() {
        Ok(()) | Err(KeyringError::NoEntry) => {}
        Err(_) => {}
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

    let mut migrated_values = HashMap::new();
    for key in LEGACY_KEYS {
        if let Some(value) = read_legacy_key(key)? {
            if !value.is_empty() {
                migrated_values.insert(key.to_string(), value);
            }
        }
    }

    save_blob_to_keychain(&migrated_values)?;
    for key in LEGACY_KEYS {
        delete_legacy_key_best_effort(key);
    }

    write_cache(migrated_values.clone())?;
    Ok(migrated_values)
}

pub fn secret_set_cached(key: &str, value: &str) -> Result<(), String> {
    let mut values = ensure_loaded_blob()?;
    values.insert(key.to_string(), value.to_string());
    save_blob_to_keychain(&values)?;
    write_cache(values)
}

pub fn secret_get_cached(key: &str) -> Result<Option<String>, String> {
    let mut values = ensure_loaded_blob()?;
    if let Some(value) = values.get(key) {
        return Ok(Some(value.clone()));
    }

    if key.starts_with("web_auth::") {
        if let Some(value) = read_legacy_key(key)? {
            values.insert(key.to_string(), value.clone());
            save_blob_to_keychain(&values)?;
            write_cache(values)?;
            delete_legacy_key_best_effort(key);
            return Ok(Some(value));
        }
    }

    Ok(None)
}

pub fn secret_delete_cached(key: &str) -> Result<(), String> {
    let mut values = ensure_loaded_blob()?;
    values.remove(key);
    save_blob_to_keychain(&values)?;
    write_cache(values)?;
    delete_legacy_key_best_effort(key);
    Ok(())
}

#[allow(dead_code)] // Phase 2 migration helper; kept for upcoming vault refactor
pub fn secret_cache_clear() -> Result<(), String> {
    let mut cache = SECRET_CACHE
        .write()
        .map_err(|_| "Unable to access secure vault cache".to_string())?;
    *cache = None;
    Ok(())
}

/// Copy a keychain blob from one (service, key) to another if the destination is empty.
/// Idempotent (destination wins); best-effort delete of the source.
fn migrate_blob_between(
    old_service: &str,
    old_key: &str,
    new_service: &str,
    new_key: &str,
) -> Result<(), String> {
    let new_entry = Entry::new(new_service, new_key).map_err(|_| "vault".to_string())?;
    match new_entry.get_password() {
        Ok(_) => return Ok(()),                  // destination populated — never clobber
        Err(KeyringError::NoEntry) => {}         // empty — proceed with migration
        Err(_) => return Ok(()),                 // keychain error — best-effort, skip
    }
    let old_entry = Entry::new(old_service, old_key).map_err(|_| "vault".to_string())?;
    match old_entry.get_password() {
        Ok(serialized) => {
            new_entry.set_password(&serialized).map_err(|_| "vault".to_string())?;
            let _ = old_entry.delete_password();
            Ok(())
        }
        Err(KeyringError::NoEntry) => Ok(()),
        Err(_) => Ok(()),
    }
}

/// One-time rename migration: adopt the pre-rename API-key blob into the renamed location.
/// MUST be called at startup BEFORE the first `ensure_loaded_blob`/`secret_get_cached`, so the
/// in-process SECRET_CACHE is populated from the migrated (new) blob, not the empty new slot.
pub fn migrate_legacy_blob_once() {
    let _ = migrate_blob_between(LEGACY_BLOB_SERVICE, LEGACY_BLOB_KEY, BLOB_SERVICE, BLOB_KEY);
}

#[cfg(all(test, target_os = "macos"))]
mod blob_migrate_tests {
    use super::*;

    #[test]
    fn migrates_blob_then_idempotent() {
        let old_s = "ai.air-agent.test.legacy";
        let new_s = "ai.air-agent.test.current";
        let key = "blob";
        let payload = "{\"openai_api_key\":\"sk-x\"}";

        Entry::new(old_s, key).unwrap().set_password(payload).unwrap();
        let _ = Entry::new(new_s, key).unwrap().delete_password();

        migrate_blob_between(old_s, key, new_s, key).unwrap();
        assert_eq!(Entry::new(new_s, key).unwrap().get_password().unwrap(), payload);
        assert!(matches!(
            Entry::new(old_s, key).unwrap().get_password(),
            Err(KeyringError::NoEntry)
        ));

        migrate_blob_between(old_s, key, new_s, key).unwrap();
        assert_eq!(Entry::new(new_s, key).unwrap().get_password().unwrap(), payload);

        let _ = Entry::new(new_s, key).unwrap().delete_password();
    }
}
