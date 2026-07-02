// Copied from apps/desktop/src-tauri/src/secrets/mod.rs (M1a Task 4); the in-app original is removed in Task 6.

mod trait_def;
pub use trait_def::SecretsVault;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(test)]
mod tests;
