// The `air` module is an incremental, forward-looking API surface built across
// Phases 2-4. Phase 2 wires in registration + identity-store; Phase 3 will reach
// trait methods (update, health), key re-hydration (from_secret_bytes, sign), and
// DID document publishing (build_did_document). Allow dead_code and unused_imports
// at the module level so future-API items + re-exports don't need scattered
// per-item annotations.
#![allow(dead_code, unused_imports)]

pub mod types;
pub mod client_trait;
pub mod mock_client;
pub mod http_client;
pub mod did_wba;
pub mod identity;

pub use client_trait::{AirClient, AirError};
pub use mock_client::MockAirClient;
pub use http_client::HttpAirClient;
pub use did_wba::{generate_keypair, build_did, build_did_document, AgentKeypair};
pub use identity::{IdentityStore, IdentityMetadata};
pub use types::*;

#[cfg(test)]
mod tests;
