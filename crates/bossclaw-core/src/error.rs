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

    /// A caller supplied an invalid argument (e.g. `dim = 0`).
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// A model load or encode operation failed (e.g. corrupt weights file).
    #[error("embed error: {0}")]
    Embed(String),

    /// A reasoner (LLM) backend failed: transport error, a non-loopback host was
    /// refused, malformed/un-decodable JSON, or (for the scripted test double) an
    /// unscripted `(system, prompt)`. The reasoner's output is data, never
    /// authority — this error makes the evolve tick a retryable no-op (spec §10),
    /// never corrupting the log.
    #[error("reasoner error: {0}")]
    Reasoner(String),
}

impl From<rusqlite::Error> for BossclawError {
    fn from(e: rusqlite::Error) -> Self {
        BossclawError::Store(e.to_string())
    }
}

impl From<bossclaw_canon::CanonError> for BossclawError {
    fn from(e: bossclaw_canon::CanonError) -> Self {
        use bossclaw_canon::CanonError as C;
        match e {
            C::Canonical(s) => BossclawError::Canonical(s),
            C::Signature(s) => BossclawError::Signature(s),
            C::Multibase(s) => BossclawError::Multibase(s),
            C::Chain(s) => BossclawError::Chain(s),
        }
    }
}
