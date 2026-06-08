use crate::secrets::SecretsVault;
use keyring::{Entry, Error as KeyringError};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, RwLock};

pub const SERVICE_NAME: &str = "ai.bossclaw.desktop";

pub fn default_vault() -> Arc<dyn SecretsVault> {
    #[cfg(target_os = "macos")]
    return Arc::new(crate::secrets::macos::MacosVault::new(SERVICE_NAME));

    #[cfg(target_os = "windows")]
    return Arc::new(crate::secrets::windows::WindowsVault::new(SERVICE_NAME));

    #[cfg(target_os = "linux")]
    return Arc::new(crate::secrets::linux::LinuxVault::new(SERVICE_NAME));

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    compile_error!("Unsupported platform — only macOS, Windows, Linux supported");
}

// ---------------------------------------------------------------------------
// Legacy blob-based helpers — used by llm_stream and web_access modules.
// These will be migrated to SecretsVault in Phase 2.
// ---------------------------------------------------------------------------

const VAULT_SERVICE_NAME: &str = "BossClaw";
const VAULT_BLOB_KEY: &str = "bossclaw_vault_blob";
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
    Entry::new(VAULT_SERVICE_NAME, VAULT_BLOB_KEY)
        .map_err(|_| "Unable to access secure vault".to_string())
}

fn legacy_entry(key: &str) -> Result<Entry, String> {
    Entry::new(VAULT_SERVICE_NAME, key).map_err(|_| "Unable to access secure vault".to_string())
}

fn load_blob_from_keychain() -> Result<Option<HashMap<String, String>>, String> {
    let entry = blob_entry()?;

    #[cfg(debug_assertions)]
    eprintln!("vault_get miss -> keychain read: {}", VAULT_BLOB_KEY);

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
