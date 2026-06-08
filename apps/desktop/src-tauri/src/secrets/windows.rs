use super::SecretsVault;
use keyring::Entry;

pub struct WindowsVault {
    service: String,
}

impl WindowsVault {
    pub fn new(service: &str) -> Self {
        Self { service: service.to_string() }
    }
}

impl SecretsVault for WindowsVault {
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
