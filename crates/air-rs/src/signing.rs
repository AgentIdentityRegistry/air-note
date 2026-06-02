//! EdDSA signing and verification for A2A envelopes.
//!
//! ## Procedure (spec §5)
//!
//! 1. Set `envelope.signature = None`.
//! 2. Serialize via NFC-normalized RFC 8785 JCS (canonical JSON) using
//!    [`serde_jcs`] — no floats, NFC string normalization, deterministic key
//!    ordering.
//! 3. Sign the canonical bytes with the sender's Ed25519 private key via
//!    [`ed25519_dalek`].
//! 4. Encode the raw 64-byte signature as multibase `z`-prefix base58btc and
//!    store in `envelope.signature`.
//!
//! Verification reverses the process: strip `signature`, canonicalize the
//! remaining envelope, multibase-decode the signature, then call
//! [`VerifyingKey::verify`].
//!
//! **Spec reference:** <https://agentidentityregistry.org/specs/air/draft-1> §5

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use multibase::{Base, decode as multibase_decode, encode as multibase_encode};

use crate::{envelope::Envelope, error::A2AError};

/// Length in bytes of a raw EdDSA signature.
const ED25519_SIGNATURE_LEN: usize = 64;

/// Sign an [`Envelope`] using the provided Ed25519 signing key.
///
/// Takes the envelope by value and returns a new envelope whose `signature`
/// field is set to the multibase `z`-prefixed base58btc encoding of the raw
/// 64-byte EdDSA signature over the canonical-JSON representation of the
/// envelope with `signature` absent.
///
/// # Errors
///
/// * [`A2AError::InvalidEncoding`] if the input envelope already carries a
///   signature — callers MUST clear it first to make double-signing explicit.
/// * [`A2AError::Canonical`] if canonical JSON serialisation fails.
pub fn sign_envelope(envelope: Envelope, key: &SigningKey) -> Result<Envelope, A2AError> {
    if envelope.signature.is_some() {
        return Err(A2AError::InvalidEncoding(
            "envelope already signed; clear `signature` before re-signing".to_string(),
        ));
    }

    let canonical = canonical_bytes(&envelope)?;
    let signature: Signature = key.sign(&canonical);
    let multibase_sig = multibase_encode(Base::Base58Btc, signature.to_bytes());

    Ok(Envelope {
        signature: Some(multibase_sig),
        ..envelope
    })
}

/// Verify the `signature` field of an [`Envelope`] against the provided
/// Ed25519 verifying key.
///
/// Strips `signature`, canonicalizes the envelope to JCS bytes, decodes the
/// multibase signature, then runs EdDSA verification.
///
/// # Errors
///
/// * [`A2AError::InvalidEncoding`] if the signature field is absent or
///   decoded to anything other than 64 bytes.
/// * [`A2AError::Multibase`] if the signature string is not valid multibase.
/// * [`A2AError::Canonical`] if canonical JSON serialisation fails.
/// * [`A2AError::Signature`] if the signature does not match.
pub fn verify_envelope(envelope: &Envelope, key: &VerifyingKey) -> Result<(), A2AError> {
    let sig_mb = envelope
        .signature
        .as_deref()
        .ok_or_else(|| A2AError::InvalidEncoding("envelope has no signature".to_string()))?;

    let (_base, raw) = multibase_decode(sig_mb)
        .map_err(|e| A2AError::Multibase(format!("decode signature: {e}")))?;

    let sig_bytes: [u8; ED25519_SIGNATURE_LEN] = raw.as_slice().try_into().map_err(|_| {
        A2AError::InvalidEncoding(format!(
            "signature must be {ED25519_SIGNATURE_LEN} bytes, got {}",
            raw.len()
        ))
    })?;
    let signature = Signature::from_bytes(&sig_bytes);

    // Canonicalize the envelope with the signature field cleared.
    let unsigned = Envelope {
        signature: None,
        ..envelope.clone()
    };
    let canonical = canonical_bytes(&unsigned)?;

    key.verify(&canonical, &signature)
        .map_err(|e| A2AError::Signature(e.to_string()))
}

/// Produce the canonical JSON byte representation of an envelope per spec §5:
/// serialize to a `serde_json::Value`, NFC-normalize every string, strip the
/// `signature` field if present, then apply RFC 8785 JCS via `serde_jcs`.
///
/// Callers SHOULD pass an envelope with `signature` already cleared; the strip
/// is defensive and matches the conformance harness behaviour.
fn canonical_bytes(envelope: &Envelope) -> Result<Vec<u8>, A2AError> {
    let mut value = serde_json::to_value(envelope)
        .map_err(|e| A2AError::Canonical(format!("envelope to_value: {e}")))?;
    if let serde_json::Value::Object(ref mut map) = value {
        map.remove("signature");
    }
    nfc_normalize(&mut value);
    serde_jcs::to_vec(&value).map_err(|e| A2AError::Canonical(format!("serde_jcs: {e}")))
}

/// Recursively NFC-normalize every string value in a `serde_json::Value`.
///
/// Mirrors the conformance harness so that envelopes constructed in Rust
/// produce byte-identical canonical output to envelopes round-tripped via
/// the reference test vectors.
fn nfc_normalize(value: &mut serde_json::Value) {
    use unicode_normalization::UnicodeNormalization;
    match value {
        serde_json::Value::String(s) => {
            let normalized: String = s.nfc().collect();
            *s = normalized;
        }
        serde_json::Value::Object(map) => {
            for v in map.values_mut() {
                nfc_normalize(v);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr.iter_mut() {
                nfc_normalize(v);
            }
        }
        _ => {}
    }
}
