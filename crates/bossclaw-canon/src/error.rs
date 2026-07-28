//! Error type for the extracted canonical/signing primitives.
use thiserror::Error;

/// Errors from canonical-bytes production and Ed25519 hash-signing. Display strings are
/// byte-identical to the pre-extraction `BossclawError` variants so no user-facing string moves.
#[derive(Debug, Error)]
pub enum CanonError {
    /// Canonical JSON (JCS) serialisation failed.
    #[error("canonical JSON error: {0}")]
    Canonical(String),
    /// A cryptographic signature operation failed (sign or verify).
    #[error("signature error: {0}")]
    Signature(String),
    /// A multibase encode/decode operation failed.
    #[error("multibase error: {0}")]
    Multibase(String),
    /// An event's hash did not recompute, or a chain-adjacent decode failed.
    #[error("chain integrity error: {0}")]
    Chain(String),
}
