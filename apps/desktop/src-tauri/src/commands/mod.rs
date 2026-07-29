pub mod a2a;
// Engine command surface is Unix-only until M7 (bossclaw-core doesn't build on Windows yet).
#[cfg(unix)]
pub mod engine;
// Rung-5 SP-V1 export — engine-backed, so Unix-only alongside `engine`.
#[cfg(unix)]
pub mod export;
pub mod identity;
pub mod inbox;
#[cfg(unix)]
pub mod integrations;
