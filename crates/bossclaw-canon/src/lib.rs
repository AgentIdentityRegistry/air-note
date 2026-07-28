//! Canonical event bytes + Ed25519 hash-signing, extracted from bossclaw-core (zero behavior
//! change) so the Rung-5 verifier reproduces the engine's exact bytes on wasm32.
#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod error;
pub mod event;
pub mod sign;

pub use error::CanonError;
pub use event::{canonical_bytes, compute_hash, is_external, Event, ModelMeta, EXTERNAL_ORIGIN};
pub use sign::{sign_bytes, sign_hash, verify_bytes, verify_hash, SigningKey, VerifyingKey};
