//! # air-rs
//!
//! Reference Rust implementation of the AIR Agent-to-Agent (A2A) protocol.
//!
//! **Spec:** <https://agentidentityregistry.org/specs/air/draft-1> (promoted to `/specs/air/v1` in Phase 3 Week 5 once cross-language conformance passes — see RALPLAN-DR plan)
//!
//! ## Status
//!
//! Version `0.0.x` — API subject to change until `v1` spec freeze. Do NOT use in production yet.
//!
//! ## Modules
//!
//! - [`envelope`] — typed message envelopes (Offer, Counter, Accept, Decline, Withdraw)
//! - [`signing`] — EdDSA + canonical JSON via `serde_jcs`
//! - [`transport`] — HTTP client for the AIR A2A relay (gated by `transport` feature, default on)
//! - `discovery` — resolve recipient `service.endpoint` via AIR DID document (planned)
//!
//! ## Design principles (per the plan)
//!
//! - Spec-first; this crate is one implementation of an open protocol
//! - All keys are Ed25519, encoded as `multicodec 0xed01 + base58btc + 'z' prefix` per W3C did-core
//! - Envelopes signed via JCS (RFC 8785) — no floats, NFC string normalization, see spec §5
//! - 20 cross-language test vectors at `/specs/air/draft-1/test-vectors.json` gate v1 promotion
//! - Relays are byte pipes (Principle 3) — no signature verification at the relay; recipient verifies

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod envelope;
pub mod error;
pub mod seal;
pub mod signing;

#[cfg(feature = "transport")]
pub mod transport;

pub use envelope::{
    Accept, Counter, Decline, Envelope, MessageBody, Offer, Value, Withdraw,
};
pub use error::A2AError;
pub use signing::{sign_envelope, verify_envelope};

#[cfg(feature = "transport")]
pub use transport::{
    AckResult, HealthReport, PullBatch, PulledMessage, RelayClient, SendReceipt,
    DEFAULT_RELAY_URL,
};
