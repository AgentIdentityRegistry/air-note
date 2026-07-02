// Copied from apps/desktop/src-tauri/src/secrets/trait_def.rs (M1a Task 4); the in-app original is removed in Task 6.

#[allow(dead_code)] // methods used via trait impls; the daemon reads the DEK/blob via them.
pub trait SecretsVault: Send + Sync {
    fn set(&self, key: &str, value: &str) -> Result<(), String>;
    fn get(&self, key: &str) -> Result<Option<String>, String>;
    fn delete(&self, key: &str) -> Result<(), String>;
}
