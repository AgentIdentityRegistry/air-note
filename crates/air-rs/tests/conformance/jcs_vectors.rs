//! Cross-language JCS + Ed25519 conformance harness.
//!
//! Reads the canonical test-vectors file at
//! `~/air-site/specs/air/draft-1/test-vectors.json` and asserts that the Rust
//! `serde_jcs` + `ed25519_dalek` implementation reproduces byte-identical
//! canonical JSON (verified via SHA-256) and byte-identical signatures for
//! every vector.
//!
//! **Gate:** all 20 vectors must pass before `draft-1` is promoted to
//! `/specs/air/v1`. See Phase 3 Stage 3a.5 of the RALPLAN-DR plan.
//!
//! Run with:
//! ```sh
//! cargo test --features conformance -p air-rs
//! ```
//!
//! The path to the vectors file is resolved relative to the workspace root via
//! the `CARGO_MANIFEST_DIR` environment variable (set by Cargo at test time).

#![cfg(feature = "conformance")]

use ed25519_dalek::SigningKey;
use sha2::{Digest, Sha256};

// ---------------------------------------------------------------------------
// Vector file deserialization types
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
struct VectorsFile {
    vectors: Vec<Vector>,
}

#[derive(Debug, serde::Deserialize)]
struct Vector {
    id: String,
    #[allow(dead_code)]
    description: String,
    /// Raw JSON object — we round-trip through serde_json::Value to avoid
    /// prescribing field order, then re-serialize via serde_jcs.
    input_envelope: serde_json::Value,
    expected_canonical_bytes_sha256_hex: String,
    signing_key_seed_hex: String,
    signing_key_public_multibase: String,
    expected_signature_multibase: String,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolve the vectors file path.
///
/// Tries `VECTORS_PATH` env var first (CI override), then falls back to a
/// path relative to this crate's manifest directory.
fn vectors_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("VECTORS_PATH") {
        return std::path::PathBuf::from(p);
    }
    // CARGO_MANIFEST_DIR = .../SuperClaw/crates/air-rs
    // The vectors live in .../air-site/specs/air/draft-1/test-vectors.json
    // Going up: .. -> crates/, ../.. -> SuperClaw/, ../../.. -> ~ (parent of SuperClaw).
    // From ~, air-site/specs/... is the target. Three `..` segments, not four.
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    std::path::Path::new(manifest_dir)
        .join("../../../air-site/specs/air/draft-1/test-vectors.json")
}

/// Canonicalize a JSON value via RFC 8785 JCS (serde_jcs).
///
/// Steps matching spec §5:
/// 1. Remove the `signature` field if present.
/// 2. NFC-normalize all string values.
/// 3. Apply serde_jcs::to_vec.
fn canonicalize(mut value: serde_json::Value) -> Vec<u8> {
    // Step 1: strip signature field
    if let serde_json::Value::Object(ref mut map) = value {
        map.remove("signature");
    }
    // Step 2: NFC-normalize all strings
    nfc_normalize(&mut value);
    // Step 3: JCS
    serde_jcs::to_vec(&value).expect("serde_jcs canonicalization failed")
}

/// Recursively NFC-normalize all string values in a JSON value.
fn nfc_normalize(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(s) => {
            use unicode_normalization::UnicodeNormalization;
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

/// Decode a 32-byte hex string into an Ed25519 signing key.
fn signing_key_from_hex(hex_str: &str) -> SigningKey {
    let bytes = hex::decode(hex_str).expect("invalid hex in signing_key_seed_hex");
    let seed: [u8; 32] = bytes.try_into().expect("seed must be exactly 32 bytes");
    SigningKey::from_bytes(&seed)
}

/// Encode raw bytes as multibase z + base58btc.
///
/// Used for both public keys (with multicodec prefix) and signatures (raw bytes).
fn multibase_z_base58(bytes: &[u8]) -> String {
    use multibase::{encode, Base};
    encode(Base::Base58Btc, bytes)
}

/// Encode an Ed25519 public key as `multicodec 0xed01 + base58btc + 'z' prefix`.
fn public_key_multibase(key: &ed25519_dalek::VerifyingKey) -> String {
    let raw = key.as_bytes();
    let mut prefixed = vec![0xed, 0x01];
    prefixed.extend_from_slice(raw);
    multibase_z_base58(&prefixed)
}

/// Encode a 64-byte signature as multibase z + base58btc (no multicodec prefix).
fn signature_multibase(sig: &ed25519_dalek::Signature) -> String {
    multibase_z_base58(sig.to_bytes().as_ref())
}

// ---------------------------------------------------------------------------
// Core assertion function — runs one vector, panics with context on failure
// ---------------------------------------------------------------------------

fn assert_vector(v: &Vector) {
    // 1. Canonicalize
    let canon_bytes = canonicalize(v.input_envelope.clone());

    // 2. Assert SHA-256 of canonical bytes
    let sha256_hex = {
        let mut hasher = Sha256::new();
        hasher.update(&canon_bytes);
        hex::encode(hasher.finalize())
    };
    assert_eq!(
        sha256_hex,
        v.expected_canonical_bytes_sha256_hex,
        "Vector {}: canonical bytes SHA-256 mismatch\n  got:      {}\n  expected: {}",
        v.id,
        sha256_hex,
        v.expected_canonical_bytes_sha256_hex,
    );

    // 3. Derive keypair from seed
    let signing_key = signing_key_from_hex(&v.signing_key_seed_hex);
    let verifying_key = signing_key.verifying_key();

    // 4. Assert public key encoding matches
    let pub_mb = public_key_multibase(&verifying_key);
    assert_eq!(
        pub_mb,
        v.signing_key_public_multibase,
        "Vector {}: public key multibase mismatch\n  got:      {}\n  expected: {}",
        v.id,
        pub_mb,
        v.signing_key_public_multibase,
    );

    // 5. Sign and assert signature matches
    use ed25519_dalek::Signer;
    let sig = signing_key.sign(&canon_bytes);
    let sig_mb = signature_multibase(&sig);
    assert_eq!(
        sig_mb,
        v.expected_signature_multibase,
        "Vector {}: signature multibase mismatch\n  got:      {}\n  expected: {}",
        v.id,
        sig_mb,
        v.expected_signature_multibase,
    );

    // 6. Verify round-trip
    use ed25519_dalek::Verifier;
    verifying_key
        .verify(&canon_bytes, &sig)
        .unwrap_or_else(|e| panic!("Vector {}: signature verification failed: {}", v.id, e));
}

// ---------------------------------------------------------------------------
// Load vectors once at test startup
// ---------------------------------------------------------------------------

fn load_vectors() -> Vec<Vector> {
    let path = vectors_path();
    let content = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "Cannot read test-vectors.json at {}: {}\n\
             Set VECTORS_PATH env var or ensure air-site is checked out at \
             ../../../../air-site relative to the crates/air-rs manifest.",
            path.display(),
            e
        )
    });
    let file: VectorsFile = serde_json::from_str(&content)
        .unwrap_or_else(|e| panic!("Failed to parse test-vectors.json: {}", e));
    assert_eq!(
        file.vectors.len(),
        20,
        "Expected 20 vectors, got {}",
        file.vectors.len()
    );
    file.vectors
}

// ---------------------------------------------------------------------------
// One test per vector — named cross_lang_vector_<id>
// ---------------------------------------------------------------------------
// We use a macro to generate individual test functions so failures show
// the vector ID clearly in cargo test output.

macro_rules! conformance_test {
    ($fn_name:ident, $vector_id:expr) => {
        #[test]
        fn $fn_name() {
            let vectors = load_vectors();
            let v = vectors
                .iter()
                .find(|v| v.id == $vector_id)
                .unwrap_or_else(|| panic!("Vector '{}' not found in test-vectors.json", $vector_id));
            assert_vector(v);
        }
    };
}

conformance_test!(cross_lang_vector_v01_ascii_offer,                  "v01-ascii-offer");
conformance_test!(cross_lang_vector_v02_ascii_counter,                "v02-ascii-counter");
conformance_test!(cross_lang_vector_v03_ascii_accept,                 "v03-ascii-accept");
conformance_test!(cross_lang_vector_v04_ascii_decline,                "v04-ascii-decline");
conformance_test!(cross_lang_vector_v05_ascii_withdraw,               "v05-ascii-withdraw");
conformance_test!(cross_lang_vector_v06_nonascii_korean,              "v06-nonascii-korean");
conformance_test!(cross_lang_vector_v07_nonascii_japanese,            "v07-nonascii-japanese");
conformance_test!(cross_lang_vector_v08_nonascii_arabic,              "v08-nonascii-arabic");
conformance_test!(cross_lang_vector_v09_nfc_precomposed,              "v09-nfc-precomposed");
conformance_test!(cross_lang_vector_v10_nfc_decomposed,               "v10-nfc-decomposed");
conformance_test!(cross_lang_vector_v11_int_boundary_2pow53_minus1,   "v11-int-boundary-2pow53-minus1");
conformance_test!(cross_lang_vector_v12_int_boundary_2pow53_plus1,    "v12-int-boundary-2pow53-plus1");
conformance_test!(cross_lang_vector_v13_empty_options_offer,          "v13-empty-options-offer");
conformance_test!(cross_lang_vector_v14_empty_options_decline,        "v14-empty-options-decline");
conformance_test!(cross_lang_vector_v15_thread_with_reply,            "v15-thread-with-reply");
conformance_test!(cross_lang_vector_v16_thread_no_reply,              "v16-thread-no-reply");
conformance_test!(cross_lang_vector_v17_currency_usd,                 "v17-currency-usd");
conformance_test!(cross_lang_vector_v18_currency_krw,                 "v18-currency-krw");
conformance_test!(cross_lang_vector_v19_same_input_key_seed_a,        "v19-same-input-key-seed-a");
conformance_test!(cross_lang_vector_v20_same_input_key_seed_b,        "v20-same-input-key-seed-b");
