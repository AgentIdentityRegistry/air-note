use air_rs::seal::{open_body, seal_body_with_ephemeral, ed25519_pub_to_x25519, ed25519_seed_to_x25519};
use serde_json::json;
use x25519_dalek::PublicKey;

// A fixed Ed25519 seed + matching public key (RFC 8032 test vector).
fn recipient() -> ([u8; 32], [u8; 32]) {
    let seed: [u8; 32] = [
        0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec, 0x2c,
        0xc4, 0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03, 0x1c, 0xae,
        0x7f, 0x60,
    ];
    let signing = ed25519_dalek::SigningKey::from_bytes(&seed);
    (seed, signing.verifying_key().to_bytes())
}

#[test]
fn seal_then_open_round_trips() {
    let (seed, ed_pub) = recipient();
    let aad = b"id\0from\0to\0thread";
    let body = json!({ "type": "text", "text": "hello \u{1f510}" });

    let eph_secret = [7u8; 32];
    let nonce = [0u8; 12];
    let enc = seal_body_with_ephemeral(&body, &ed_pub, aad, eph_secret, nonce).unwrap();
    assert_eq!(enc["type"], "encrypted");
    assert_eq!(enc["alg"], "x25519-hkdf-sha256-chacha20poly1305");

    let opened = open_body(&enc, &seed, aad).unwrap();
    assert_eq!(opened, body);
}

#[test]
fn open_fails_with_wrong_aad() {
    let (seed, ed_pub) = recipient();
    let enc = seal_body_with_ephemeral(&json!({"type":"text","text":"x"}), &ed_pub, b"aad-a", [7u8;32], [0u8;12]).unwrap();
    assert!(open_body(&enc, &seed, b"aad-b").is_err());
}

#[test]
fn open_fails_on_malformed_nonce_without_panicking() {
    let (seed, ed_pub) = recipient();
    let aad = b"id\0from\0to\0thread";
    let mut enc = seal_body_with_ephemeral(&json!({"type":"text","text":"x"}), &ed_pub, aad, [7u8; 32], [0u8; 12]).unwrap();
    enc["nonce"] = serde_json::json!("AAAA"); // decodes to 3 bytes, not 12 — must Err, not panic
    assert!(open_body(&enc, &seed, aad).is_err());
}

#[test]
fn derived_keys_agree() {
    let (seed, ed_pub) = recipient();
    let x_pub = ed25519_pub_to_x25519(&ed_pub).unwrap();
    let x_priv = ed25519_seed_to_x25519(&seed);
    assert_eq!(PublicKey::from(&x_priv).to_bytes(), x_pub);
}

#[test]
fn interop_vectors_match_js() {
    let raw = include_str!("e2e_interop_vectors.json");
    let doc: serde_json::Value = serde_json::from_str(raw).unwrap();
    let vectors = doc["vectors"].as_array().unwrap();
    assert!(!vectors.is_empty(), "interop vectors file must not be empty");
    for v in vectors {
        let seed = hex32(v["recipient_seed_hex"].as_str().unwrap());
        let ed_pub = ed25519_dalek::SigningKey::from_bytes(&seed).verifying_key().to_bytes();
        let aad = build_aad(&v["env"]);
        let eph = hex32(v["eph_secret_hex"].as_str().unwrap());
        let nonce: [u8; 12] = hex_n(v["nonce_hex"].as_str().unwrap());

        // (a) Rust reproduces the frozen sealed body byte-for-byte.
        let enc = seal_body_with_ephemeral(&v["body"], &ed_pub, &aad, eph, nonce).unwrap();
        assert_eq!(enc, v["expected"], "Rust sealed body must match the frozen vector");

        // (b) Rust opens the JS-produced expected body.
        let opened = open_body(&v["expected"], &seed, &aad).unwrap();
        assert_eq!(opened, v["body"]);
    }
}

fn build_aad(env: &serde_json::Value) -> Vec<u8> {
    let mut out = Vec::new();
    for (i, k) in ["id", "from", "to", "thread_id"].iter().enumerate() {
        if i > 0 { out.push(0); }
        out.extend_from_slice(env[k].as_str().unwrap().as_bytes());
    }
    out
}

fn hex32(s: &str) -> [u8; 32] { hex_n(s) }
fn hex_n<const N: usize>(s: &str) -> [u8; N] {
    let bytes: Vec<u8> = (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect();
    assert_eq!(bytes.len(), N, "hex_n: expected {N} bytes, got {}", bytes.len());
    let mut out = [0u8; N];
    out.copy_from_slice(&bytes);
    out
}
