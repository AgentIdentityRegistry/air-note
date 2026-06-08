use super::SecretsVault;
use std::collections::HashMap;
use std::sync::Mutex;

struct MockVault {
    store: Mutex<HashMap<String, String>>,
}

impl MockVault {
    fn new() -> Self {
        Self { store: Mutex::new(HashMap::new()) }
    }
}

impl SecretsVault for MockVault {
    fn set(&self, key: &str, value: &str) -> Result<(), String> {
        self.store.lock().unwrap().insert(key.to_string(), value.to_string());
        Ok(())
    }

    fn get(&self, key: &str) -> Result<Option<String>, String> {
        Ok(self.store.lock().unwrap().get(key).cloned())
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        self.store.lock().unwrap().remove(key);
        Ok(())
    }
}

#[test]
fn round_trip_works() {
    let v = MockVault::new();
    v.set("k1", "v1").unwrap();
    assert_eq!(v.get("k1").unwrap(), Some("v1".to_string()));
    v.delete("k1").unwrap();
    assert_eq!(v.get("k1").unwrap(), None);
}

#[cfg(target_os = "macos")]
#[test]
fn macos_keychain_round_trip() {
    use super::macos::MacosVault;
    let v = MacosVault::new("ai.bossclaw.test");
    v.set("test_key", "test_value").unwrap();
    let got = v.get("test_key").unwrap();
    assert_eq!(got, Some("test_value".to_string()));
    v.delete("test_key").unwrap();
    assert_eq!(v.get("test_key").unwrap(), None);
}
