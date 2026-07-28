use bossclaw_canon::event::{canonical_bytes, compute_hash, is_external, Event, EXTERNAL_ORIGIN};
use bossclaw_canon::sign::{sign_hash, verify_hash};
use bossclaw_canon::SigningKey;

fn fixture() -> Event {
    Event {
        id: "01J0000000000000000000000A".into(), ts: "2026-06-15T00:00:00Z".into(),
        valid_time: None, event_type: "memory".into(),
        content: serde_json::json!({ "text": "hello" }), model_meta: None,
        prev_hash: "00".repeat(32), hash: None,
        signed_by_did: "did:wba:AIR-2JE0-EM7W-JNBK".into(), signature: None,
    }
}

#[test]
fn canonical_bytes_frozen() {
    let expected = r#"{"content":{"text":"hello"},"id":"01J0000000000000000000000A","prev_hash":"0000000000000000000000000000000000000000000000000000000000000000","signed_by_did":"did:wba:AIR-2JE0-EM7W-JNBK","ts":"2026-06-15T00:00:00Z","type":"memory"}"#;
    assert_eq!(String::from_utf8(canonical_bytes(&fixture()).unwrap()).unwrap(), expected);
}

#[test]
fn genesis_hash_frozen() {
    assert_eq!(hex::encode(compute_hash(&fixture()).unwrap()),
        "9089b0bd99a3f72e37653c2e8da756aeeb737085c0faa9a1ae5d0defc35dbde9",
        "a dep bump changed canonical bytes — DO NOT rebase the pins to fix this");
}

#[test]
fn sign_verify_and_origin() {
    let key = SigningKey::from_bytes(&[7u8; 32]);
    let h = compute_hash(&fixture()).unwrap();
    let sig = sign_hash(&h, &key);
    assert!(sig.starts_with('z'));
    verify_hash(&h, &sig, &key.verifying_key()).unwrap();
    let mut ext = fixture();
    ext.content = serde_json::json!({ "text": "x", "origin": EXTERNAL_ORIGIN });
    assert!(is_external(&ext));
    assert!(!is_external(&fixture()));
}
