//! Error types for the `air-rs` crate.

use thiserror::Error;

/// Errors that can occur within the A2A protocol implementation.
#[derive(Debug, Error)]
pub enum A2AError {
    /// A cryptographic signature operation failed.
    ///
    /// Contains a human-readable description of the failure (e.g. invalid key
    /// encoding, signature mismatch).
    #[error("signature error: {0}")]
    Signature(String),

    /// A multibase encoding or decoding operation failed.
    ///
    /// Multibase is used to encode/decode Ed25519 public keys and signatures
    /// with the `z`-prefix (base58btc) per spec §5.
    #[error("multibase error: {0}")]
    Multibase(String),

    /// Canonical JSON serialisation failed.
    ///
    /// All envelopes are signed over their RFC 8785 JCS (canonical JSON)
    /// representation; this error is returned when that serialisation fails.
    #[error("canonical JSON error: {0}")]
    Canonical(String),

    /// JSON serialisation or deserialisation of an envelope failed.
    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),

    /// The envelope's `signature` field had an unexpected shape — either present
    /// when it should be absent (during signing) or absent / wrong length when
    /// it should be a valid 64-byte EdDSA signature (during verification).
    #[error("invalid encoding: {0}")]
    InvalidEncoding(String),

    /// A network IO or HTTP transport-level failure (DNS, TCP, TLS, timeout,
    /// SSE stream error). Distinct from `RelayError` which is a well-formed
    /// non-2xx response from the relay.
    #[error("network error: {0}")]
    Network(String),

    /// The relay returned a non-2xx HTTP status. `status` is the response
    /// code; `message` is the response body (typically a JSON error object
    /// from the relay describing what went wrong).
    #[error("relay returned {status}: {message}")]
    RelayError {
        /// HTTP status code returned by the relay.
        status: u16,
        /// Response body (typically a JSON error envelope from the relay).
        message: String,
    },
}
