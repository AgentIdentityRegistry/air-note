// Copied from apps/desktop/src-tauri/src/secrets/macos.rs (M1a Task 4); the in-app original is removed in Task 6.

use super::SecretsVault;
use keyring::Entry;

pub struct MacosVault {
    #[allow(dead_code)] // used in keyring Entry calls via self.service
    service: String,
}

impl MacosVault {
    pub fn new(service: &str) -> Self {
        Self { service: service.to_string() }
    }
}

impl SecretsVault for MacosVault {
    fn set(&self, key: &str, value: &str) -> Result<(), String> {
        Entry::new(&self.service, key)
            .map_err(|e| e.to_string())?
            .set_password(value)
            .map_err(|e| e.to_string())
    }

    fn get(&self, key: &str) -> Result<Option<String>, String> {
        let entry = Entry::new(&self.service, key).map_err(|e| e.to_string())?;
        match entry.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        let entry = Entry::new(&self.service, key).map_err(|e| e.to_string())?;
        match entry.delete_password() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}
