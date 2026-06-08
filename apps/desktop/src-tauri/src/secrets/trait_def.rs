#[allow(dead_code)] // methods used via trait impls; Phase 2 will wire Tauri commands
pub trait SecretsVault: Send + Sync {
    fn set(&self, key: &str, value: &str) -> Result<(), String>;
    fn get(&self, key: &str) -> Result<Option<String>, String>;
    fn delete(&self, key: &str) -> Result<(), String>;
}
