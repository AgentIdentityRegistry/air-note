//! HTTP transport for the AIR A2A relay (Phase 3 Stage 3b / W3-2).
//!
//! [`RelayClient`] is the client-side counterpart to the Cloudflare Worker
//! at `relay.agentidentityregistry.org`. It speaks four operations:
//!
//! | Operation | Method                   | Use |
//! |-----------|--------------------------|-----|
//! | Send      | [`RelayClient::send`]    | POST envelope to recipient's inbox |
//! | Pull      | [`RelayClient::pull`]    | Fetch un-acked messages (batch polling) |
//! | Stream    | [`RelayClient::stream`]  | Open SSE for real-time push (sub-second latency) |
//! | Ack       | [`RelayClient::ack`]     | Confirm receipt so the relay can drop the message |
//!
//! Per A2A spec Principle 3, the relay does NOT verify envelope signatures.
//! Recipients verify on pull via [`crate::verify_envelope`]. The relay
//! returns envelope bytes byte-identical to what the sender posted so the
//! wax-seal (signature) check on the recipient side works against the
//! exact JCS-canonical bytes the sender signed.
//!
//! ## Send paths
//!
//! - [`RelayClient::send`] — serialises the envelope via `serde_json`.
//!   Convenient but the bytes may not be JCS-canonical. Use only if the
//!   envelope's signature was computed against `serde_json::to_vec` output
//!   (which it currently isn't — [`crate::sign_envelope`] uses `serde_jcs`).
//! - [`RelayClient::send_raw`] — sends pre-canonicalised raw bytes. The
//!   correct path for signed envelopes: get bytes from `sign_envelope`'s
//!   companion canonical-serialise path, post them here, and the recipient
//!   verifies against the same bytes.
//!
//! ## Module gating
//!
//! Behind the `transport` feature (default on). The core envelope/signing
//! API has no network dependencies, so consumers that only need to sign or
//! verify can opt out with `default-features = false`.

use crate::envelope::Envelope;
use crate::error::A2AError;
use base64::Engine;
use eventsource_stream::Eventsource;
use futures::Stream;
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::pin::Pin;

/// Boxed, pinned, `Send` stream of pulled messages — used as the return
/// type of [`RelayClient::stream`] so callers can use `.next().await`
/// without manual `pin!`/`Box::pin` ceremony.
pub type PullStream =
    Pin<Box<dyn Stream<Item = Result<PulledMessage, A2AError>> + Send>>;

/// Default relay URL — the public AIR-operated relay at
/// `relay.agentidentityregistry.org`. Federated; alternative relays may
/// be substituted by passing the URL to [`RelayClient::new`].
pub const DEFAULT_RELAY_URL: &str = "https://relay.agentidentityregistry.org";

/// HTTP client for the AIR A2A relay.
///
/// Cheap to clone — wraps a `reqwest::Client` whose internal connection
/// pool is `Arc`-shared. Construct one per agent process and reuse.
#[derive(Clone, Debug)]
pub struct RelayClient {
    client: Client,
    base_url: String,
}

impl Default for RelayClient {
    fn default() -> Self {
        Self::new(DEFAULT_RELAY_URL)
    }
}

impl RelayClient {
    /// Construct a client pointing at the given relay base URL
    /// (e.g. `"https://relay.agentidentityregistry.org"`). Strip any
    /// trailing slash for cleanest URL composition.
    pub fn new(base_url: impl Into<String>) -> Self {
        let mut url = base_url.into();
        if url.ends_with('/') {
            url.pop();
        }
        Self {
            client: Client::new(),
            base_url: url,
        }
    }

    /// POST an [`Envelope`] to its recipient's inbox via the relay.
    ///
    /// Re-serialises via `serde_json::to_vec`. Use [`RelayClient::send_raw`]
    /// when you already have the JCS-canonical signed bytes — the relay
    /// will store and return whatever bytes you POST, so for the recipient
    /// to verify the signature the bytes here must match the bytes that
    /// were signed.
    pub async fn send(&self, envelope: &Envelope) -> Result<SendReceipt, A2AError> {
        let body = serde_json::to_vec(envelope)?;
        self.send_raw(&envelope.to, body).await
    }

    /// POST raw envelope bytes to the recipient's inbox. The correct send
    /// path for signed envelopes: post the same bytes that were JCS-
    /// canonicalised and signed.
    pub async fn send_raw(
        &self,
        recipient_did: &str,
        body: Vec<u8>,
    ) -> Result<SendReceipt, A2AError> {
        let url = format!("{}/inbox/{}", self.base_url, recipient_did);
        let resp = self
            .client
            .post(&url)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .map_err(|e| A2AError::Network(format!("POST {url}: {e}")))?;
        decode_or_relay_error(resp).await
    }

    /// Poll the relay for messages addressed to `recipient_did` newer than
    /// the `since` cursor.
    ///
    /// On first call pass `since = 0`. For subsequent calls pass
    /// [`PullBatch::cursor`] from the previous response.
    ///
    /// `limit` caps the page size (relay default is 50, max 200). Passing
    /// `None` uses the relay default.
    pub async fn pull(
        &self,
        recipient_did: &str,
        since: i64,
        limit: Option<u32>,
    ) -> Result<PullBatch, A2AError> {
        let url = match limit {
            Some(n) => format!(
                "{}/pull/{}?since={}&limit={}",
                self.base_url, recipient_did, since, n
            ),
            None => format!("{}/pull/{}?since={}", self.base_url, recipient_did, since),
        };
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| A2AError::Network(format!("GET {url}: {e}")))?;
        decode_or_relay_error(resp).await
    }

    /// Open a Server-Sent Events stream that pushes messages addressed to
    /// `recipient_did` newer than `since` as the relay receives them.
    ///
    /// Returns a stream that yields [`PulledMessage`] items. The stream
    /// closes when the recipient drops the connection or the relay closes
    /// it. Implements the "WhatsApp-like real-time" delivery target — POST
    /// to a recipient's inbox is observed by the recipient's stream
    /// consumer within ~1 second.
    ///
    /// ## Example
    ///
    /// ```no_run
    /// use air_rs::transport::RelayClient;
    /// use futures::StreamExt;
    ///
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = RelayClient::default();
    /// let mut stream = client.stream("did:wba:...", 0).await?;
    /// while let Some(item) = stream.next().await {
    ///     let msg = item?;
    ///     println!("got envelope {}", msg.envelope_id);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn stream(&self, recipient_did: &str, since: i64) -> Result<PullStream, A2AError> {
        let url = format!(
            "{}/pull/{}?stream=1&since={}",
            self.base_url, recipient_did, since
        );
        let resp = self
            .client
            .get(&url)
            .header("accept", "text/event-stream")
            .send()
            .await
            .map_err(|e| A2AError::Network(format!("GET {url} (SSE): {e}")))?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let message = resp.text().await.unwrap_or_default();
            return Err(A2AError::RelayError { status, message });
        }
        let event_stream = resp.bytes_stream().eventsource();
        let mapped = event_stream.filter_map(|ev| async move {
            match ev {
                Ok(event) if event.event == "envelope" => {
                    match serde_json::from_str::<PulledMessage>(&event.data) {
                        Ok(msg) => Some(Ok(msg)),
                        Err(e) => Some(Err(A2AError::Serde(e))),
                    }
                }
                // Heartbeats (comment frames) and other event types are
                // surfaced to the eventsource parser as events with empty
                // `event` field — filter them out silently.
                Ok(_) => None,
                Err(e) => Some(Err(A2AError::Network(format!("SSE: {e}")))),
            }
        });
        Ok(Box::pin(mapped))
    }

    /// Acknowledge receipt of one or more envelopes. After ack, the relay
    /// filters these out of subsequent pulls/streams and they become
    /// eligible for garbage collection 24 hours later.
    ///
    /// Bulk operation — pass up to 500 envelope ids per call. Re-acking an
    /// already-acked message is a no-op (relay increments
    /// `requested` but not `acked_count`).
    pub async fn ack(
        &self,
        recipient_did: &str,
        envelope_ids: &[String],
    ) -> Result<AckResult, A2AError> {
        let url = format!("{}/ack/{}", self.base_url, recipient_did);
        let body = serde_json::json!({ "envelope_ids": envelope_ids });
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| A2AError::Network(format!("POST {url}: {e}")))?;
        decode_or_relay_error(resp).await
    }

    /// Liveness probe against `/health`. Returns the relay's reported
    /// status, version, binding-status, and live queue stats.
    pub async fn health(&self) -> Result<HealthReport, A2AError> {
        let url = format!("{}/health", self.base_url);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| A2AError::Network(format!("GET {url}: {e}")))?;
        decode_or_relay_error(resp).await
    }
}

// -----------------------------------------------------------------------------
// Response types
// -----------------------------------------------------------------------------

/// Returned by [`RelayClient::send`] and [`RelayClient::send_raw`] when the
/// relay accepts an envelope (HTTP 202).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendReceipt {
    /// Always `"queued"` on success.
    pub status: String,
    /// The envelope's own `id` (echoed back so the sender has a confirmation).
    pub envelope_id: String,
    /// Relay-internal monotonic sequence number assigned at queue time.
    pub seq: i64,
}

/// One queued message returned by [`RelayClient::pull`] or yielded from
/// [`RelayClient::stream`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PulledMessage {
    /// Relay-assigned monotonic cursor — use to drive subsequent `since`
    /// values when polling.
    pub seq: i64,
    /// Sender's DID, as declared in the envelope's `from` field. NOT
    /// cryptographically verified by the relay; recipient verifies via
    /// [`crate::verify_envelope`].
    pub sender_did: String,
    /// The envelope's own `id` field — use this in [`RelayClient::ack`].
    pub envelope_id: String,
    /// Base64 (standard, NOT URL-safe) of the raw JCS-canonical envelope
    /// bytes. Decode via [`PulledMessage::decoded_bytes`] and verify
    /// against these bytes — NEVER re-serialise before verification.
    pub envelope_b64: String,
    /// Unix epoch seconds at which the relay queued this message.
    pub queued_at: i64,
}

impl PulledMessage {
    /// Decode `envelope_b64` to the raw JCS-canonical bytes the sender
    /// signed. These are what [`crate::verify_envelope`] must check
    /// against — DO NOT re-serialise before verification (would break
    /// the JCS guarantee).
    pub fn decoded_bytes(&self) -> Result<Vec<u8>, A2AError> {
        base64::engine::general_purpose::STANDARD
            .decode(&self.envelope_b64)
            .map_err(|e| A2AError::InvalidEncoding(format!("envelope base64: {e}")))
    }

    /// Parse `envelope_b64` into an [`Envelope`] (does NOT verify the
    /// signature — caller must follow up with [`crate::verify_envelope`]
    /// against [`PulledMessage::decoded_bytes`]).
    pub fn to_envelope(&self) -> Result<Envelope, A2AError> {
        let bytes = self.decoded_bytes()?;
        serde_json::from_slice(&bytes).map_err(A2AError::Serde)
    }
}

/// Returned by [`RelayClient::pull`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullBatch {
    /// Recipient DID this batch was pulled for (echoed for caller sanity).
    pub recipient_did: String,
    /// Messages in `seq` order (oldest first).
    pub messages: Vec<PulledMessage>,
    /// Cursor to pass as `since` on the next pull. Equals the seq of the
    /// last message in this batch, or the input `since` when no messages.
    pub cursor: i64,
    /// `true` if the batch hit the limit — caller should pull again to
    /// drain the queue. `false` if the relay returned fewer than the
    /// limit (i.e. queue is currently empty for this recipient).
    pub has_more: bool,
}

/// Returned by [`RelayClient::ack`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AckResult {
    /// Always `"acked"` on success.
    pub status: String,
    /// Number of envelopes actually flipped from un-acked to acked. Will
    /// be less than `requested` if some were already acked (idempotent).
    pub acked_count: i64,
    /// Number of envelope_ids the caller submitted.
    pub requested: i64,
}

/// Returned by [`RelayClient::health`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthReport {
    /// Always `"ok"` when the relay is live.
    pub status: String,
    /// Relay name (always `"air-relay"` for the canonical AIR relay).
    pub service: String,
    /// Relay implementation version.
    pub version: String,
    /// Live queue depth across all recipients on this relay instance.
    pub queue_stats: serde_json::Value,
}

// -----------------------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------------------

/// Decode a JSON response into `T`, or convert the response into
/// [`A2AError::RelayError`] if the HTTP status is non-2xx.
async fn decode_or_relay_error<T: for<'de> Deserialize<'de>>(
    resp: reqwest::Response,
) -> Result<T, A2AError> {
    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let message = resp.text().await.unwrap_or_default();
        return Err(A2AError::RelayError { status, message });
    }
    resp.json::<T>()
        .await
        .map_err(|e| A2AError::Network(format!("decode response: {e}")))
}
