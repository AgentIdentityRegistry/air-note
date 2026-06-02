//! End-to-end integration tests for `air_rs::transport::RelayClient`
//! against the live production relay at `relay.agentidentityregistry.org`.
//!
//! These tests are network-bound by design — they're how we prove the
//! Rust transport actually talks to the deployed Cloudflare Worker. Skip
//! by setting `A2A_SKIP_LIVE_TESTS=1` (used by air-gapped CI runs).
//!
//! Naming convention: each test generates fresh DIDs containing the test
//! name + a random suffix so concurrent runs don't collide. Each test
//! also cleans up after itself (acks any messages it queued).

#![cfg(feature = "transport")]

use air_rs::transport::RelayClient;
use futures::StreamExt;
use std::time::Duration;
use uuid::Uuid;

/// Helper: skip a test if the env var is set (for air-gapped CI).
fn live_tests_skipped() -> bool {
    std::env::var("A2A_SKIP_LIVE_TESTS").is_ok()
}

/// Helper: build a test envelope as a JSON value the relay will accept.
///
/// We construct the JSON directly (rather than going through
/// `Envelope::serialize`) because for these transport-level tests we
/// don't need a real signature — we're verifying the byte-pipe semantics
/// of the relay round-trip, not signing/verification (which has its own
/// test suite in `signing_tests.rs`).
fn test_envelope_json(envelope_id: &str, from: &str, to: &str, body_text: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "id":        envelope_id,
        "from":      from,
        "to":        to,
        "timestamp": "2026-05-28T14:00:00.000Z",
        "thread_id": Uuid::new_v4().to_string(),
        "nonce":     Uuid::new_v4().to_string(),
        "body":      { "type": "offer", "item_id": "test-item", "note": body_text },
        // Relay does NOT verify the signature, so a placeholder string is fine
        // for these transport-only round-trip tests.
        "signature": "z3TestSignaturePlaceholder",
    }))
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn health_endpoint_returns_ok() {
    if live_tests_skipped() {
        eprintln!("skipped: A2A_SKIP_LIVE_TESTS set");
        return;
    }
    let client = RelayClient::default();
    let report = client.health().await.expect("health check failed");
    assert_eq!(report.status, "ok");
    assert_eq!(report.service, "air-relay");
    assert!(
        report.queue_stats.is_object() || report.queue_stats.is_null(),
        "queue_stats should be an object or null, got {:?}",
        report.queue_stats
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_pull_ack_round_trip() {
    if live_tests_skipped() {
        return;
    }
    // Fresh DID per run — avoids interference across parallel test executions.
    let recipient = format!("did:wba:test:rs-recipient-{}", Uuid::new_v4());
    let sender = format!("did:wba:test:rs-sender-{}", Uuid::new_v4());
    let envelope_id = format!("test-env-{}", Uuid::new_v4());

    let client = RelayClient::default();

    // 1. Send the envelope
    let body = test_envelope_json(&envelope_id, &sender, &recipient, "hello from rust");
    let receipt = client
        .send_raw(&recipient, body.clone())
        .await
        .expect("send_raw failed");
    assert_eq!(receipt.status, "queued");
    assert_eq!(receipt.envelope_id, envelope_id);
    assert!(receipt.seq > 0);

    // 2. Pull it back
    let batch = client
        .pull(&recipient, 0, None)
        .await
        .expect("pull failed");
    assert_eq!(batch.recipient_did, recipient);
    assert_eq!(batch.messages.len(), 1);
    assert_eq!(batch.messages[0].envelope_id, envelope_id);
    assert_eq!(batch.messages[0].sender_did, sender);

    // 3. Verify byte-perfect round-trip — the bytes we get back MUST be
    //    byte-identical to what we POSTed. This is the contract that lets
    //    recipient-side signature verification work.
    let returned_bytes = batch.messages[0]
        .decoded_bytes()
        .expect("decoded_bytes failed");
    assert_eq!(
        returned_bytes, body,
        "relay must return byte-identical envelope bytes (wax-seal property)"
    );

    // 4. Ack the message
    let ack = client
        .ack(&recipient, std::slice::from_ref(&envelope_id))
        .await
        .expect("ack failed");
    assert_eq!(ack.status, "acked");
    assert_eq!(ack.acked_count, 1);
    assert_eq!(ack.requested, 1);

    // 5. Subsequent pull should now be empty (the relay filters acked).
    let batch_after = client
        .pull(&recipient, 0, None)
        .await
        .expect("post-ack pull failed");
    assert!(
        batch_after.messages.is_empty(),
        "acked message must not appear in subsequent pulls; got {} messages",
        batch_after.messages.len()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duplicate_envelope_id_returns_409() {
    if live_tests_skipped() {
        return;
    }
    let recipient = format!("did:wba:test:rs-dup-{}", Uuid::new_v4());
    let sender = format!("did:wba:test:rs-sender-{}", Uuid::new_v4());
    let envelope_id = format!("test-dup-{}", Uuid::new_v4());

    let client = RelayClient::default();
    let body = test_envelope_json(&envelope_id, &sender, &recipient, "first");

    // First post — should succeed.
    client
        .send_raw(&recipient, body.clone())
        .await
        .expect("first send must succeed");

    // Second post with same envelope_id — should fail with 409.
    let result = client.send_raw(&recipient, body).await;
    match result {
        Err(air_rs::A2AError::RelayError { status, .. }) => assert_eq!(status, 409),
        other => panic!("expected RelayError 409, got {:?}", other),
    }

    // Clean up.
    let _ = client.ack(&recipient, std::slice::from_ref(&envelope_id)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sse_streaming_delivers_within_seconds() {
    if live_tests_skipped() {
        return;
    }
    // Use a unique recipient so concurrent test runs don't push messages
    // into each other's streams.
    let recipient = format!("did:wba:test:rs-sse-{}", Uuid::new_v4());
    let sender = format!("did:wba:test:rs-sender-{}", Uuid::new_v4());
    let envelope_id = format!("test-sse-{}", Uuid::new_v4());

    let client = RelayClient::default();

    // Open the SSE stream FIRST so we're listening when the message arrives.
    let mut stream = client.stream(&recipient, 0).await.expect("stream open failed");

    // Send the envelope on a separate task so we keep polling the stream.
    let sender_client = client.clone();
    let recipient_clone = recipient.clone();
    let envelope_id_clone = envelope_id.clone();
    let body = test_envelope_json(&envelope_id, &sender, &recipient, "sse round-trip");
    let send_task = tokio::spawn(async move {
        // Small delay so we're definitely listening on the stream before
        // the message lands in D1.
        tokio::time::sleep(Duration::from_millis(200)).await;
        sender_client
            .send_raw(&recipient_clone, body.clone())
            .await
            .expect("send during SSE test");
        body
    });

    // Wait up to 5 seconds for the message to arrive via SSE. The relay
    // polls D1 every 1s inside the SSE handler, so we expect delivery
    // within ~1.5 seconds. 5s gives generous slack for network jitter.
    let pulled = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("SSE delivery timed out after 5s")
        .expect("SSE stream ended unexpectedly")
        .expect("SSE event was an error");
    assert_eq!(pulled.envelope_id, envelope_id_clone);
    assert_eq!(pulled.sender_did, sender);

    let original_body = send_task.await.expect("send task panicked");
    let returned_bytes = pulled.decoded_bytes().expect("decoded_bytes failed");
    assert_eq!(
        returned_bytes, original_body,
        "SSE-delivered bytes must be byte-identical to sent bytes"
    );

    // Clean up.
    let _ = client.ack(&recipient, &[envelope_id]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pull_paginates_via_cursor() {
    if live_tests_skipped() {
        return;
    }
    let recipient = format!("did:wba:test:rs-page-{}", Uuid::new_v4());
    let sender = format!("did:wba:test:rs-sender-{}", Uuid::new_v4());
    let client = RelayClient::default();

    // Send 5 envelopes; pull them in two pages of size 3 + 2.
    let mut ids = Vec::new();
    for i in 0..5 {
        let envelope_id = format!("test-page-{}-{}", i, Uuid::new_v4());
        let body = test_envelope_json(&envelope_id, &sender, &recipient, &format!("msg {i}"));
        client
            .send_raw(&recipient, body)
            .await
            .expect("send during pagination test");
        ids.push(envelope_id);
    }

    let page1 = client
        .pull(&recipient, 0, Some(3))
        .await
        .expect("page1 pull failed");
    assert_eq!(page1.messages.len(), 3);
    assert!(page1.has_more);

    let page2 = client
        .pull(&recipient, page1.cursor, Some(3))
        .await
        .expect("page2 pull failed");
    assert_eq!(page2.messages.len(), 2);
    assert!(!page2.has_more);

    // Combined: all 5 envelope_ids accounted for in order.
    let all: Vec<_> = page1
        .messages
        .iter()
        .chain(page2.messages.iter())
        .map(|m| m.envelope_id.clone())
        .collect();
    assert_eq!(all, ids);

    // Clean up.
    let _ = client.ack(&recipient, &ids).await;
}
