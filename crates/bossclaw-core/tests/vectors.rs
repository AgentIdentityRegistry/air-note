use bossclaw_core::event::{canonical_bytes, compute_hash, Event, ModelMeta};
use bossclaw_core::sign::{sign_hash, verify_hash};
use ed25519_dalek::SigningKey;
use sha2::{Digest, Sha256};

fn fixture_event() -> Event {
    Event {
        id: "01J0000000000000000000000A".to_string(),
        ts: "2026-06-15T00:00:00Z".to_string(),
        valid_time: None,
        event_type: "memory".to_string(),
        content: serde_json::json!({ "text": "hello" }),
        model_meta: None,
        prev_hash: "00".repeat(32),
        hash: None,
        signed_by_did: "did:wba:AIR-2JE0-EM7W-JNBK".to_string(),
        signature: None,
    }
}

#[test]
fn canonical_bytes_are_stable_and_exclude_hash_and_signature() {
    let mut e = fixture_event();
    let base = canonical_bytes(&e).unwrap();
    e.hash = Some("ff".repeat(32));
    e.signature = Some("zSomeSignature".to_string());
    let with_fields = canonical_bytes(&e).unwrap();
    assert_eq!(base, with_fields, "hash/signature must be excluded from canon");

    let expected = r#"{"content":{"text":"hello"},"id":"01J0000000000000000000000A","prev_hash":"0000000000000000000000000000000000000000000000000000000000000000","signed_by_did":"did:wba:AIR-2JE0-EM7W-JNBK","ts":"2026-06-15T00:00:00Z","type":"memory"}"#;
    assert_eq!(String::from_utf8(base).unwrap(), expected);
}

#[test]
fn genesis_hash_is_frozen() {
    let e = fixture_event(); // prev_hash = 64 zeros
    let h = compute_hash(&e).unwrap();
    let hex_h = hex::encode(h);
    assert_eq!(hex_h, "9089b0bd99a3f72e37653c2e8da756aeeb737085c0faa9a1ae5d0defc35dbde9");
}

#[test]
fn second_event_links_to_first() {
    let first = fixture_event();
    let h1 = compute_hash(&first).unwrap();

    let mut second = fixture_event();
    second.id = "01J0000000000000000000000B".to_string();
    second.prev_hash = hex::encode(h1);
    let h2 = compute_hash(&second).unwrap();

    assert_ne!(h1, h2, "different events hash differently");
}

fn fixture_key() -> SigningKey {
    SigningKey::from_bytes(&[7u8; 32]) // deterministic test key
}

#[test]
fn sign_then_verify_roundtrips() {
    let key = fixture_key();
    let hash = compute_hash(&fixture_event()).unwrap();
    let sig = sign_hash(&hash, &key);
    assert!(sig.starts_with('z'), "multibase base58btc 'z' prefix");
    verify_hash(&hash, &sig, &key.verifying_key()).expect("valid signature verifies");
}

#[test]
fn tampered_hash_fails_verification() {
    let key = fixture_key();
    let hash = compute_hash(&fixture_event()).unwrap();
    let sig = sign_hash(&hash, &key);
    let mut bad = hash;
    bad[0] ^= 0xFF;
    assert!(verify_hash(&bad, &sig, &key.verifying_key()).is_err());
}

#[test]
fn file_ingested_canonicalization_is_frozen() {
    let ev = Event {
        id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string(),
        ts: "2026-06-18T00:00:00+00:00".to_string(),
        valid_time: None,
        event_type: "file_ingested".to_string(),
        content: serde_json::json!({
            "text": "hello",
            "origin": "external",
            "provenance": {
                "canonical_path": "/x/a.md",
                "content_hash": "aaa",
                "text_hash": "bbb",
                "size_bytes": 5,
                "modified_at": "2026-06-18T00:00:00+00:00",
                "parser_id": "native-text-v1",
                "grant_root": "/x"
            }
        }),
        model_meta: None,
        prev_hash: "0".repeat(64),
        hash: None,
        signed_by_did: "did:wba:bossclaw-engine".to_string(),
        signature: None,
    };
    let canon = canonical_bytes(&ev).unwrap();
    let got = hex::encode(Sha256::digest(&canon));
    // FREEZE: run once; copy the printed hex into `expected`; re-run → PASS.
    let expected = "3754a3d258b562b999f4e4df214c89db39d5027b4c6c902096193a617b0b83e0";
    assert_eq!(got, expected, "file_ingested canonicalization changed (got {got}); update only if intentional");
}

// FREEZE: a tainted `link` event — verifies that "origin": "external" is INSIDE
// the canonicalized signed bytes for the link path.
// Run once with a dummy expected to print the actual hex, paste it, re-run → PASS.
#[test]
fn tainted_link_canonicalization_is_frozen() {
    let ev = Event {
        id: "01ARZ3NDEKTSV4RRFFQ69G5FB0".to_string(),
        ts: "2026-06-18T00:00:00+00:00".to_string(),
        valid_time: None,
        event_type: "link".to_string(),
        content: serde_json::json!({
            "src": "entity:01AAA",
            "relation": "knows",
            "dst": "entity:01BBB",
            "confidence_milli": 900,
            "origin": "external",
        }),
        model_meta: Some(ModelMeta {
            model_id: "scripted".to_string(),
            prompt_hash: String::new(),
            source_event_ids: vec!["01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string()],
        }),
        prev_hash: "0".repeat(64),
        hash: None,
        signed_by_did: "did:wba:bossclaw-engine".to_string(),
        signature: None,
    };
    let canon = canonical_bytes(&ev).unwrap();
    let got = hex::encode(Sha256::digest(&canon));
    // FREEZE: run once; copy the printed hex into `expected`; re-run → PASS.
    let expected = "c7c32f2c007464c84cd1afd3d20138d9fb5bd599c6bbb773b3783ea24076cfe7";
    assert_eq!(got, expected, "tainted link canonicalization changed (got {got}); update only if intentional");
}

// FREEZE: a tainted `page` (dossier) event — verifies that "origin": "external"
// is INSIDE the canonicalized signed bytes for the page/dossier path.
#[test]
fn tainted_page_canonicalization_is_frozen() {
    let ev = Event {
        id: "01ARZ3NDEKTSV4RRFFQ69G5FC0".to_string(),
        ts: "2026-06-18T00:00:00+00:00".to_string(),
        valid_time: None,
        event_type: "page".to_string(),
        content: serde_json::json!({
            "topic_id": "entity:01AAA",
            "title": "Acme",
            "text": "Acme shipped widget X.",
            "claims": [{ "text": "Acme shipped widget X.", "cites": ["01ARZ3NDEKTSV4RRFFQ69G5FAV"] }],
            "tags": [],
            "origin": "external",
        }),
        model_meta: Some(ModelMeta {
            model_id: "scripted".to_string(),
            prompt_hash: String::new(),
            source_event_ids: vec!["01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string()],
        }),
        prev_hash: "0".repeat(64),
        hash: None,
        signed_by_did: "did:wba:bossclaw-engine".to_string(),
        signature: None,
    };
    let canon = canonical_bytes(&ev).unwrap();
    let got = hex::encode(Sha256::digest(&canon));
    // FREEZE: run once; copy the printed hex into `expected`; re-run → PASS.
    let expected = "09ce409a1d0f82d97356ffb43cb3b6fda8f2985283eb4c3b5ce071ccad26e816";
    assert_eq!(got, expected, "tainted page canonicalization changed (got {got}); update only if intentional");
}
