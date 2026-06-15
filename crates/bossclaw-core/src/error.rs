//! Error types for the `bossclaw-core` crate.

use thiserror::Error;

/// Errors that can occur within the bossclaw-core memory engine.
#[derive(Debug, Error)]
pub enum BossclawError {
    /// Canonical JSON (JCS) serialisation failed.
    #[error("canonical JSON error: {0}")]
    Canonical(String),

    /// A cryptographic signature operation failed (sign or verify).
    #[error("signature error: {0}")]
    Signature(String),

    /// A multibase encode/decode operation failed.
    #[error("multibase error: {0}")]
    Multibase(String),

    /// An event's hash did not match its recomputed canonical hash, or the
    /// chain link to the previous event was broken.
    #[error("chain integrity error: {0}")]
    Chain(String),

    /// The signed high-water-mark indicates the log was truncated or rolled back.
    #[error("truncation/rollback detected: {0}")]
    Truncation(String),

    /// A storage / SQLite error.
    #[error("store error: {0}")]
    Store(String),

    /// JSON (de)serialisation failed.
    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),

    /// An IO failure (high-water file, etc.).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl From<rusqlite::Error> for BossclawError {
    fn from(e: rusqlite::Error) -> Self {
        BossclawError::Store(e.to_string())
    }
}
