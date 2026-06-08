//! Integration tests for the `a2a_demo_round_trip` Tauri command.
//!
//! These tests call the underlying command function directly (not via the Tauri
//! IPC bridge) to validate the sign→verify round-trip without spinning up a
//! full Tauri runtime.

// The function under test lives in the binary crate; make it accessible via
// `#[path]` so the integration test can import it without a lib target.
#[path = "../src/commands/a2a.rs"]
mod a2a;

#[tokio::test]
async fn a2a_demo_round_trip_returns_verified_true() {
    let result = a2a::a2a_demo_round_trip()
        .await
        .expect("a2a_demo_round_trip should succeed");

    // The top-level `verified` field must be true.
    assert_eq!(
        result["verified"],
        serde_json::Value::Bool(true),
        "expected verified: true, got: {result:?}"
    );
}

#[tokio::test]
async fn a2a_demo_round_trip_envelope_has_signature() {
    let result = a2a::a2a_demo_round_trip()
        .await
        .expect("a2a_demo_round_trip should succeed");

    let sig = result["envelope"]["signature"].as_str();
    assert!(
        sig.is_some_and(|s| s.starts_with('z')),
        "expected multibase z-prefix signature, got: {sig:?}"
    );
}

#[tokio::test]
async fn a2a_demo_round_trip_envelope_routing_fields() {
    let result = a2a::a2a_demo_round_trip()
        .await
        .expect("a2a_demo_round_trip should succeed");

    let env = &result["envelope"];
    assert_eq!(
        env["from"].as_str().unwrap(),
        "did:wba:bossclaw.ai:test-sender"
    );
    assert_eq!(
        env["to"].as_str().unwrap(),
        "did:wba:bossclaw.ai:test-recipient"
    );
    // id, thread_id, nonce should look like UUIDs (8-4-4-4-12).
    for field in ["id", "thread_id", "nonce"] {
        let val = env[field].as_str().unwrap_or("");
        let parts: Vec<&str> = val.split('-').collect();
        assert_eq!(
            parts.len(),
            5,
            "field `{field}` should be UUID-shaped, got: {val:?}"
        );
    }
}

#[tokio::test]
async fn a2a_demo_round_trip_body_is_offer() {
    let result = a2a::a2a_demo_round_trip()
        .await
        .expect("a2a_demo_round_trip should succeed");

    let body = &result["envelope"]["body"];
    assert_eq!(
        body["type"].as_str().unwrap(),
        "offer",
        "expected body.type == offer, got: {body:?}"
    );
    assert_eq!(
        body["offered_value"]["type"].as_str().unwrap(),
        "cash"
    );
    assert_eq!(body["offered_value"]["amount_cents"].as_u64().unwrap(), 1000);
    assert_eq!(
        body["offered_value"]["currency"].as_str().unwrap(),
        "USD"
    );
}
