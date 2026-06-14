//! Raw-hash Ed25519 signing for events. NOTE: this is intentionally NOT
//! `air-rs::sign_envelope` (that is coupled to the `Envelope` struct). We reuse
//! the `ed25519-dalek` primitive + the multibase encoding discipline only.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use multibase::{decode as mb_decode, encode as mb_encode, Base};

use crate::error::BossclawError;

const ED25519_SIGNATURE_LEN: usize = 64;

/// Sign the 32-byte event hash, returning a multibase base58btc (`z`) string.
pub fn sign_hash(hash: &[u8; 32], key: &SigningKey) -> String {
    let sig: Signature = key.sign(hash);
    mb_encode(Base::Base58Btc, sig.to_bytes())
}

/// Verify a multibase signature over the 32-byte event hash.
///
/// # Errors
/// * [`BossclawError::Multibase`] if the signature is not valid multibase.
/// * [`BossclawError::Signature`] on wrong length or a verification mismatch.
pub fn verify_hash(
    hash: &[u8; 32],
    signature_mb: &str,
    key: &VerifyingKey,
) -> Result<(), BossclawError> {
    let (_b, raw) =
        mb_decode(signature_mb).map_err(|e| BossclawError::Multibase(format!("decode: {e}")))?;
    let bytes: [u8; ED25519_SIGNATURE_LEN] = raw
        .as_slice()
        .try_into()
        .map_err(|_| BossclawError::Signature(format!("sig must be 64 bytes, got {}", raw.len())))?;
    let sig = Signature::from_bytes(&bytes);
    key.verify(hash, &sig)
        .map_err(|e| BossclawError::Signature(e.to_string()))
}
