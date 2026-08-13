// Tests added per-feature in subsequent tasks.

use super::client_trait::{AirClient, AirError};
use super::did_wba::{build_did, build_did_document, generate_keypair};
use super::identity::*;
use super::mock_client::MockAirClient;
use super::types::*;

// Compile-time check that the trait can be used as a trait object
fn _trait_object_compiles(_client: &dyn AirClient) {}

#[tokio::test]
async fn mock_register_then_lookup() {
    let client = MockAirClient::new();
    let did = Did("did:wba:example.com:test-agent".to_string());
    let manifest = AgentManifest {
        name: "Test Agent".to_string(),
        description: "for unit tests".to_string(),
        capabilities: vec!["a2a-negotiate-marketplace".to_string()],
        owner_hint: Some("human-controlled".to_string()),
    };

    let resp = client.register(&did, &manifest).await.unwrap();
    assert_eq!(resp.did, did);
    assert!(!resp.agent_secret.is_empty());

    let record = client.lookup(&did).await.unwrap();
    assert_eq!(record.did, did);
    assert_eq!(record.name, "Test Agent");
}

#[tokio::test]
async fn mock_update_requires_secret() {
    let client = MockAirClient::new();
    let did = Did("did:wba:example.com:test-update".to_string());
    let manifest = AgentManifest {
        name: "v1".to_string(),
        description: "".to_string(),
        capabilities: vec![],
        owner_hint: None,
    };
    let resp = client.register(&did, &manifest).await.unwrap();

    let updated = AgentManifest {
        name: "v2".to_string(),
        description: "".to_string(),
        capabilities: vec![],
        owner_hint: None,
    };

    // Wrong secret
    let bad = client.update(&did, "wrong-secret", &updated).await;
    assert!(matches!(bad, Err(AirError::Unauthorized)));

    // Right secret
    let good = client.update(&did, &resp.agent_secret, &updated).await;
    assert!(good.is_ok());
    let record = client.lookup(&did).await.unwrap();
    assert_eq!(record.name, "v2");
}

#[tokio::test]
async fn mock_claim_username_secret_and_conflict() {
    let client = MockAirClient::new();
    let did_a = Did("did:wba:example.com:alice-agent".to_string());
    let manifest = AgentManifest {
        name: "Alice".to_string(),
        description: String::new(),
        capabilities: vec![],
        owner_hint: None,
    };
    let resp_a = client.register(&did_a, &manifest).await.unwrap();

    // Wrong secret is rejected before any handle is recorded.
    let bad = client.claim_username(&did_a, "wrong-secret", "alice").await;
    assert!(matches!(bad, Err(AirError::Unauthorized)));

    // Right secret claims the handle.
    let good = client.claim_username(&did_a, &resp_a.agent_secret, "alice").await;
    assert!(good.is_ok());

    // A different DID claiming the same handle conflicts.
    let did_b = Did("did:wba:example.com:bob-agent".to_string());
    let resp_b = client.register(&did_b, &manifest).await.unwrap();
    let taken = client.claim_username(&did_b, &resp_b.agent_secret, "alice").await;
    assert!(matches!(taken, Err(AirError::Conflict(_))));
}

#[tokio::test]
async fn mock_check_username_reflects_claims() {
    let client = MockAirClient::new();
    let did = Did("did:wba:example.com:carol-agent".to_string());
    let manifest = AgentManifest {
        name: "Carol".to_string(),
        description: String::new(),
        capabilities: vec![],
        owner_hint: None,
    };
    let resp = client.register(&did, &manifest).await.unwrap();

    // Unclaimed, valid handle → available.
    let before = client.check_username("carol").await.unwrap();
    assert!(before.valid);
    assert!(before.available);

    client.claim_username(&did, &resp.agent_secret, "carol").await.unwrap();

    // Same handle after claim → no longer available.
    let after = client.check_username("carol").await.unwrap();
    assert!(after.valid);
    assert!(!after.available);

    // An invalid handle is reported invalid (and never available).
    let invalid = client.check_username("ab").await.unwrap();
    assert!(!invalid.valid);
    assert!(!invalid.available);
}

#[tokio::test]
async fn mock_lookup_missing_returns_not_found() {
    let client = MockAirClient::new();
    let did = Did("did:wba:example.com:does-not-exist".to_string());
    let r = client.lookup(&did).await;
    assert!(matches!(r, Err(AirError::NotFound(_))));
}

#[test]
fn keypair_round_trip() {
    let kp = generate_keypair();
    assert_eq!(kp.public_key_bytes().len(), 32);
    assert_eq!(kp.secret_key_bytes().len(), 32);
}

#[test]
fn did_is_did_wba_format() {
    let kp = generate_keypair();
    let did = build_did(&kp, "bossclaw.ai", Some("agent-1"));
    assert!(did.0.starts_with("did:wba:bossclaw.ai:"));
}

#[test]
fn did_document_has_expected_shape() {
    let kp = generate_keypair();
    let did = build_did(&kp, "bossclaw.ai", Some("agent-1"));
    let doc = build_did_document(&did, &kp);

    assert_eq!(doc["id"], did.0.clone());
    assert!(doc["verificationMethod"].is_array());
    assert!(doc["authentication"].is_array());
}

#[tokio::test]
#[ignore] // Run manually with: cargo test air::tests::live_health_check -- --ignored
async fn live_health_check() {
    use super::http_client::HttpAirClient;
    let client = HttpAirClient::production();
    let r = client.health().await;
    // Just confirms we can reach AIR. May fail if AIR is down — that's expected info.
    println!("AIR health: {:?}", r);
}

#[test]
fn identity_serde_round_trip() {
    let id = IdentityMetadata {
        did: Did("did:wba:bossclaw.ai:abc123".to_string()),
        name: "My Agent".to_string(),
        username: None,
        created_at: "2026-05-18T12:00:00Z".to_string(),
    };
    let json = serde_json::to_string(&id).unwrap();
    let back: IdentityMetadata = serde_json::from_str(&json).unwrap();
    assert_eq!(back.did, id.did);
    assert_eq!(back.name, id.name);
    assert_eq!(back.username, id.username);
}

/// The identity card's `created_at` must be a real RFC3339 UTC instant, pinned to the
/// house shape used everywhere else in this codebase (`2026-05-18T12:00:00Z`).
///
/// Regression guard: an early placeholder formatted the Unix epoch into the SECONDS
/// field of a fixed 1970 date (`1970-01-01T00:00:1782190630Z`), which parses as
/// neither a timestamp nor an integer. Parsing it back and re-formatting proves the
/// emitted string is genuinely a date, not merely a string that resembles one.
#[tokio::test]
async fn mock_registration_stamps_a_parseable_utc_timestamp() {
    let client = MockAirClient::new();
    let did = Did("did:wba:example.com:stamp-agent".to_string());
    let manifest = AgentManifest {
        name: "Stamp Agent".to_string(),
        description: "pins the created_at format".to_string(),
        capabilities: vec![],
        owner_hint: None,
    };

    let stamped = client.register(&did, &manifest).await.unwrap().record.created_at;

    let parsed = chrono::DateTime::parse_from_rfc3339(&stamped)
        .unwrap_or_else(|e| panic!("created_at {stamped:?} is not RFC3339: {e}"));
    assert_eq!(
        parsed.with_timezone(&chrono::Utc).format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        stamped,
        "created_at must round-trip in the house shape YYYY-MM-DDTHH:MM:SSZ"
    );
    assert!(
        parsed.timestamp() > 1_700_000_000,
        "created_at {stamped:?} must be the real mint time, not the 1970 placeholder"
    );
}
