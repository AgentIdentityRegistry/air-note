//! Sealed-box body encryption for A2A (spec §14): ephemeral-static X25519 →
//! HKDF-SHA256 → ChaCha20-Poly1305. Byte-compatible with the JS reference in
//! `agent-bridge-mcp/src/crypto.mjs`. NOT forward-secret (MVP; see design §9).

use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    ChaCha20Poly1305, Key, Nonce,
};
use curve25519_dalek::edwards::CompressedEdwardsY;
use hkdf::Hkdf;
use serde_json::Value;
use sha2::{Digest, Sha256, Sha512};
use unicode_normalization::UnicodeNormalization;
use x25519_dalek::{PublicKey, StaticSecret};

use crate::error::A2AError;

const E2E_ALG: &str = "x25519-hkdf-sha256-chacha20poly1305";
const E2E_INFO: &[u8] = b"air-msg/e2e/v1";
const X25519_MULTICODEC: [u8; 2] = [0xec, 0x01];

/// Recipient padlock: Ed25519 public (32) → X25519 public (32).
pub fn ed25519_pub_to_x25519(ed_pub: &[u8; 32]) -> Result<[u8; 32], A2AError> {
    let point = CompressedEdwardsY(*ed_pub)
        .decompress()
        .ok_or_else(|| A2AError::InvalidEncoding("invalid ed25519 public key".into()))?;
    Ok(point.to_montgomery().to_bytes())
}

/// Own key-agreement scalar: Ed25519 seed (32) → X25519 StaticSecret
/// (libsodium crypto_sign_ed25519_sk_to_curve25519 semantics).
pub fn ed25519_seed_to_x25519(seed: &[u8; 32]) -> StaticSecret {
    let hash = Sha512::digest(seed);
    let mut scalar = [0u8; 32];
    scalar.copy_from_slice(&hash[..32]);
    scalar[0] &= 248;
    scalar[31] &= 127;
    scalar[31] |= 64;
    StaticSecret::from(scalar)
}

fn derive_key(shared: &[u8; 32], eph_pub: &[u8; 32]) -> [u8; 32] {
    let mut info = Vec::with_capacity(E2E_INFO.len() + 32);
    info.extend_from_slice(E2E_INFO);
    info.extend_from_slice(eph_pub);
    let hk = Hkdf::<Sha256>::new(None, shared); // None salt == HashLen zeros == JS empty salt
    let mut okm = [0u8; 32];
    hk.expand(&info, &mut okm).expect("32 is a valid HKDF length");
    okm
}

/// NFC-normalize + RFC 8785 JCS the body, matching the JS plaintext bytes.
fn canonical_body_bytes(body: &Value) -> Result<Vec<u8>, A2AError> {
    let mut v = body.clone();
    nfc_normalize(&mut v);
    serde_jcs::to_vec(&v).map_err(|e| A2AError::Canonical(format!("serde_jcs: {e}")))
}

fn nfc_normalize(value: &mut Value) {
    match value {
        Value::String(s) => *s = s.nfc().collect(),
        Value::Object(map) => map.values_mut().for_each(nfc_normalize),
        Value::Array(arr) => arr.iter_mut().for_each(nfc_normalize),
        _ => {}
    }
}

fn x25519_pub_multibase(raw: &[u8; 32]) -> String {
    // multibase::encode(Base58Btc, ..) already prepends the 'z' base prefix,
    // matching JS `"z" + bs58.encode(..)`.
    let mut prefixed = Vec::with_capacity(34);
    prefixed.extend_from_slice(&X25519_MULTICODEC);
    prefixed.extend_from_slice(raw);
    multibase::encode(multibase::Base::Base58Btc, prefixed)
}

fn x25519_pub_from_multibase(mb: &str) -> Result<[u8; 32], A2AError> {
    let (_base, bytes) =
        multibase::decode(mb).map_err(|e| A2AError::Multibase(format!("decode epk: {e}")))?;
    if bytes.len() != 34 || bytes[0] != 0xec || bytes[1] != 0x01 {
        return Err(A2AError::InvalidEncoding("not an X25519 multicodec key".into()));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes[2..]);
    Ok(out)
}

/// Seal a body with a caller-supplied ephemeral secret + nonce (deterministic; for vectors).
pub fn seal_body_with_ephemeral(
    body: &Value,
    recipient_ed_pub: &[u8; 32],
    aad: &[u8],
    eph_secret: [u8; 32],
    nonce: [u8; 12],
) -> Result<Value, A2AError> {
    let recipient_x = PublicKey::from(ed25519_pub_to_x25519(recipient_ed_pub)?);
    let eph = StaticSecret::from(eph_secret);
    let eph_pub = PublicKey::from(&eph).to_bytes();
    let shared = eph.diffie_hellman(&recipient_x).to_bytes();
    let key = derive_key(&shared, &eph_pub);

    let plaintext = canonical_body_bytes(body)?;
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key));
    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: &plaintext, aad })
        .map_err(|e| A2AError::InvalidEncoding(format!("aead encrypt: {e}")))?;

    Ok(serde_json::json!({
        "type": "encrypted",
        "alg": E2E_ALG,
        "v": 1,
        "epk": x25519_pub_multibase(&eph_pub),
        "nonce": b64url(&nonce),
        "ct": b64url(&ct),
    }))
}

/// Open an encrypted body with my Ed25519 seed. Returns the decrypted body Value.
pub fn open_body(enc: &Value, my_ed_seed: &[u8; 32], aad: &[u8]) -> Result<Value, A2AError> {
    if enc["alg"] != E2E_ALG {
        return Err(A2AError::InvalidEncoding(format!("unsupported alg: {}", enc["alg"])));
    }
    let eph_pub = x25519_pub_from_multibase(enc["epk"].as_str().ok_or_else(|| A2AError::InvalidEncoding("epk".into()))?)?;
    let nonce_vec = b64url_decode(enc["nonce"].as_str().ok_or_else(|| A2AError::InvalidEncoding("nonce".into()))?)?;
    let nonce: [u8; 12] = nonce_vec
        .as_slice()
        .try_into()
        .map_err(|_| A2AError::InvalidEncoding(format!("nonce must be 12 bytes, got {}", nonce_vec.len())))?;
    let ct = b64url_decode(enc["ct"].as_str().ok_or_else(|| A2AError::InvalidEncoding("ct".into()))?)?;
    if ct.len() < 16 {
        return Err(A2AError::InvalidEncoding(format!("ciphertext too short: {} bytes", ct.len())));
    }

    let my_x = ed25519_seed_to_x25519(my_ed_seed);
    let shared = my_x.diffie_hellman(&PublicKey::from(eph_pub)).to_bytes();
    let key = derive_key(&shared, &eph_pub);

    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key));
    let pt = cipher
        .decrypt(Nonce::from_slice(&nonce), Payload { msg: &ct, aad })
        .map_err(|e| A2AError::Signature(format!("aead decrypt: {e}")))?;
    serde_json::from_slice(&pt).map_err(|e| A2AError::Canonical(format!("plaintext json: {e}")))
}

fn b64url(bytes: &[u8]) -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    URL_SAFE_NO_PAD.encode(bytes)
}

fn b64url_decode(s: &str) -> Result<Vec<u8>, A2AError> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    URL_SAFE_NO_PAD.decode(s).map_err(|e| A2AError::InvalidEncoding(format!("base64url: {e}")))
}
