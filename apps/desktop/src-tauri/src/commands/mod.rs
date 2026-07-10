pub mod a2a;
// Engine command surface is Unix-only until M7 (bossclaw-core doesn't build on Windows yet).
#[cfg(unix)]
pub mod engine;
pub mod identity;
pub mod inbox;
#[cfg(unix)]
pub mod integrations;
