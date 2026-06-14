use bossclaw_core::event::{canonical_bytes, compute_hash, Event};

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
