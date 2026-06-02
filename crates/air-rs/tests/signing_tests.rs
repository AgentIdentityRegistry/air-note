//! Integration tests for `sign_envelope` + `verify_envelope`.
//!
//! Covers the five required cases for Phase 3 Stage 3a:
//! 1. round-trip (sign then verify with the matching public key)
//! 2. double-sign rejection (signing an already-signed envelope is an error)
//! 3. wrong-key rejection (verification fails against a different key)
//! 4. body tampering (changing the payload after signing breaks verification)
//! 5. `from`-field tampering (changing the sender DID after signing breaks
//!    verification)
//!
//! These run on the default feature set (`cargo test -p air-rs`) — they are
//! intentionally NOT gated behind the `conformance` feature so they always
//! catch regressions.

use air_rs::{
    Accept, A2AError, Envelope, MessageBody, Offer, Value, sign_envelope, verify_envelope,
};
use ed25519_dalek::SigningKey;

/// Build a minimal unsigned envelope carrying a cash Offer.
fn make_offer_envelope() -> Envelope {
    Envelope {
        id: "envelope-test-001".to_string(),
        from: "did:wba:alice".to_string(),
        to: "did:wba:bob".to_string(),
        timestamp: "2026-05-28T12:00:00Z".to_string(),
        in_reply_to: None,
        thread_id: "thread-test-001".to_string(),
        nonce: "nonce-test-001".to_string(),
        body: MessageBody::Offer(Offer {
            item_id: "widget-7".to_string(),
            offered_value: Value::Cash {
                amount_cents: 12_500,
                currency: "USD".to_string(),
            },
            note: Some("first offer".to_string()),
        }),
        signature: None,
    }
}

/// Deterministic 32-byte seed for the primary signing key in these tests.
fn primary_signing_key() -> SigningKey {
    SigningKey::from_bytes(&[7u8; 32])
}

/// Deterministic 32-byte seed for a *different* signing key (wrong-key cases).
fn other_signing_key() -> SigningKey {
    SigningKey::from_bytes(&[42u8; 32])
}

// ---------------------------------------------------------------------------
// 1. Round-trip
// ---------------------------------------------------------------------------

#[test]
fn round_trip_sign_then_verify_succeeds() {
    let key = primary_signing_key();
    let unsigned = make_offer_envelope();

    let signed =
        sign_envelope(unsigned.clone(), &key).expect("sign_envelope should succeed on unsigned");

    // Signature field is now populated with a multibase z-prefixed string.
    let sig = signed.signature.as_ref().expect("signature must be set");
    assert!(sig.starts_with('z'), "signature must use multibase z prefix");

    // All other fields are unchanged.
    assert_eq!(signed.id, unsigned.id);
    assert_eq!(signed.from, unsigned.from);
    assert_eq!(signed.to, unsigned.to);
    assert_eq!(signed.body, unsigned.body);

    // Verify with the matching public key.
    verify_envelope(&signed, &key.verifying_key())
        .expect("verify_envelope should succeed for a freshly signed envelope");
}

// ---------------------------------------------------------------------------
// 2. Double-sign rejection
// ---------------------------------------------------------------------------

#[test]
fn signing_already_signed_envelope_is_rejected() {
    let key = primary_signing_key();
    let signed =
        sign_envelope(make_offer_envelope(), &key).expect("initial sign should succeed");

    let err = sign_envelope(signed, &key)
        .expect_err("sign_envelope must refuse to re-sign an envelope whose signature is set");

    match err {
        A2AError::InvalidEncoding(msg) => {
            assert!(
                msg.contains("already signed"),
                "error message should mention double-sign: got {msg}"
            );
        }
        other => panic!("expected A2AError::InvalidEncoding, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 3. Wrong-key rejection
// ---------------------------------------------------------------------------

#[test]
fn verify_with_wrong_key_returns_signature_error() {
    let key = primary_signing_key();
    let wrong_key = other_signing_key();

    let signed = sign_envelope(make_offer_envelope(), &key).expect("sign should succeed");

    let err = verify_envelope(&signed, &wrong_key.verifying_key())
        .expect_err("verify must fail against a different public key");

    assert!(
        matches!(err, A2AError::Signature(_)),
        "wrong-key failure must surface as A2AError::Signature, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// 4. Body tampering
// ---------------------------------------------------------------------------

#[test]
fn tampering_with_body_after_signing_invalidates_signature() {
    let key = primary_signing_key();
    let signed = sign_envelope(make_offer_envelope(), &key).expect("sign should succeed");

    // Swap the Offer body for an Accept — payload changes, signature does not.
    let mut tampered = signed.clone();
    tampered.body = MessageBody::Accept(Accept {
        item_id: "widget-7".to_string(),
        note: None,
    });

    let err = verify_envelope(&tampered, &key.verifying_key())
        .expect_err("verify must fail when the body has been altered post-signing");

    assert!(
        matches!(err, A2AError::Signature(_)),
        "body-tamper failure must surface as A2AError::Signature, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// 5. `from`-field tampering
// ---------------------------------------------------------------------------

#[test]
fn tampering_with_from_after_signing_invalidates_signature() {
    let key = primary_signing_key();
    let signed = sign_envelope(make_offer_envelope(), &key).expect("sign should succeed");

    let mut tampered = signed.clone();
    tampered.from = "did:wba:eve".to_string();

    let err = verify_envelope(&tampered, &key.verifying_key())
        .expect_err("verify must fail when the sender DID has been altered post-signing");

    assert!(
        matches!(err, A2AError::Signature(_)),
        "from-tamper failure must surface as A2AError::Signature, got {err:?}"
    );
}
