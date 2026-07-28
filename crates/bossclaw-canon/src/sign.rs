//! Raw-hash Ed25519 signing for events. NOTE: this is intentionally NOT
//! `air-rs::sign_envelope` (that is coupled to the `Envelope` struct). We reuse
//! the `ed25519-dalek` primitive + the multibase encoding discipline only.

// `SigningKey`/`VerifyingKey` are the public signing surface (lifted to crate root in lib.rs),
// so they are `pub use` re-exported. `Signature` (internal wire type) and the `Signer`/`Verifier`
// traits (needed only in scope for `.sign()`/`.verify()`) stay private imports.
use ed25519_dalek::{Signature, Signer, Verifier};
pub use ed25519_dalek::{SigningKey, VerifyingKey};
use multibase::{decode as mb_decode, encode as mb_encode, Base};

use crate::error::CanonError;

const ED25519_SIGNATURE_LEN: usize = 64;

/// Sign the 32-byte event hash, returning a multibase base58btc (`z`) string.
pub fn sign_hash(hash: &[u8; 32], key: &SigningKey) -> String {
    let sig: Signature = key.sign(hash);
    mb_encode(Base::Base58Btc, sig.to_bytes())
}

/// Verify a multibase signature over the 32-byte event hash.
///
/// # Errors
/// * [`CanonError::Multibase`] if the signature is not valid multibase.
/// * [`CanonError::Signature`] on wrong length or a verification mismatch.
pub fn verify_hash(
    hash: &[u8; 32],
    signature_mb: &str,
    key: &VerifyingKey,
) -> Result<(), CanonError> {
    let (_b, raw) =
        mb_decode(signature_mb).map_err(|e| CanonError::Multibase(format!("decode: {e}")))?;
    let bytes: [u8; ED25519_SIGNATURE_LEN] = raw
        .as_slice()
        .try_into()
        .map_err(|_| CanonError::Signature(format!("sig must be 64 bytes, got {}", raw.len())))?;
    let sig = Signature::from_bytes(&bytes);
    key.verify(hash, &sig)
        .map_err(|e| CanonError::Signature(e.to_string()))
}

/// Sign an arbitrary message, returning a multibase base58btc (`z`) string. Ed25519 signs any
/// length natively (no pre-hash), so the master seal + binding signature use this, not `sign_hash`.
pub fn sign_bytes(msg: &[u8], key: &SigningKey) -> String {
    let sig: Signature = key.sign(msg);
    mb_encode(Base::Base58Btc, sig.to_bytes())
}

/// Verify a multibase base58btc signature over an arbitrary message.
///
/// # Errors
/// * [`CanonError::Multibase`] if the signature is not valid multibase.
/// * [`CanonError::Signature`] on wrong length or a verification mismatch.
pub fn verify_bytes(msg: &[u8], signature_mb: &str, key: &VerifyingKey) -> Result<(), CanonError> {
    let (_b, raw) =
        mb_decode(signature_mb).map_err(|e| CanonError::Multibase(format!("decode: {e}")))?;
    let bytes: [u8; ED25519_SIGNATURE_LEN] = raw
        .as_slice()
        .try_into()
        .map_err(|_| CanonError::Signature(format!("sig must be 64 bytes, got {}", raw.len())))?;
    key.verify(msg, &Signature::from_bytes(&bytes))
        .map_err(|e| CanonError::Signature(e.to_string()))
}
