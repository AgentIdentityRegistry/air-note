//! Wire frames for the daemon socket (PROTOCOL.md §2–§6). The fixture is the source of truth.
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A delivered message record (PROTOCOL §5 `message.message`). Conditional fields are omitted
/// when falsy on the wire (optional-when-falsy rule) — `Option` + skip_serializing_if reproduces
/// that, `#[serde(default)]` tolerates their absence on decode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// Monotonic sequence number assigned by the daemon relay.
    pub seq: i64,
    /// Sequence number from the upstream relay.
    pub relay_seq: i64,
    /// UUID v4 identifying this envelope end-to-end.
    pub envelope_id: String,
    /// Sender DID (`did:wba:…`).
    pub from: String,
    /// Whether the daemon verified the sender's signature.
    pub verified: bool,
    /// Whether the body was sealed (encrypted) before delivery.
    pub encrypted: bool,
    /// ISO 8601 UTC timestamp at which the daemon received the envelope.
    pub received_at: String,
    /// Short display name for the sender, present only when the sender is a known contact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact: Option<String>,
    /// Present-and-true only when the sender's signing key changed since last contact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_changed: Option<bool>,
    /// Thread identifier shared across all envelopes in one conversation thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    /// Room identifier for group messaging contexts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room_id: Option<String>,
    /// Decoded (or raw, if encrypted) message body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
}

impl Message {
    /// `key_changed` is truthy only when present-and-true (omitted means "no change").
    pub fn key_changed(&self) -> bool {
        matches!(self.key_changed, Some(true))
    }
}

/// One entry in `status.clients[]`. Field names are camelCase ON THE WIRE (PROTOCOL §5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientSnapshot {
    /// Role this client is connected as (`"viewer"` or `"channel"`).
    pub role: String,
    /// Last sequence number acknowledged by this client, or `null` if none yet.
    #[serde(rename = "lastSeq")]
    pub last_seq: Option<i64>,
    /// Number of frames dropped for this client due to backpressure.
    pub dropped: i64,
}

/// Frames the daemon sends to the client. **Decode-only in practice** — the client never serializes
/// a `ServerFrame`. Unknown `type`s decode to `Unknown` (PROTOCOL §8); note `Unknown` would serialize
/// to `{"type":"Unknown"}`, so never round-trip a `ServerFrame` back onto the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerFrame {
    /// Handshake acknowledgement sent after a valid `hello` frame.
    #[serde(rename = "hello-ok")]
    HelloOk {
        /// OS process ID of the running daemon.
        pid: i64,
        /// ISO 8601 UTC timestamp at which the daemon process started.
        start_time: String,
        /// DID of the local identity the daemon is serving.
        did: String,
    },
    /// A delivered message pushed to the client.
    #[serde(rename = "message")]
    Message {
        /// The delivered message record.
        message: Message,
    },
    /// Sequence gap notification: the client missed messages after this seq.
    #[serde(rename = "gap")]
    Gap {
        /// The last known good sequence number; messages after this were lost.
        after_seq: i64,
    },
    /// Response to a `ping` frame.
    #[serde(rename = "pong")]
    Pong,
    /// Response to a `status` request frame.
    #[serde(rename = "status")]
    Status {
        /// Filesystem path of the daemon's Unix socket.
        socket: String,
        /// Highest sequence number the daemon has stored, or `null` if the inbox is empty.
        last_seq: Option<i64>,
        /// Snapshot of all currently connected clients.
        clients: Vec<ClientSnapshot>,
        /// Active notification sinks (e.g. `"banner"`, `"socket"`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sinks: Option<Vec<String>>,
    },
    /// Acknowledgement that an outbound send was accepted and relayed.
    #[serde(rename = "send-ok")]
    SendOk {
        /// Client-supplied correlation ID from the `send` frame.
        id: String,
        /// UUID v4 assigned to the envelope by the relay.
        envelope_id: String,
        /// Whether the relay encrypted the envelope before forwarding.
        encrypted: bool,
    },
    /// Notification that an outbound send failed.
    #[serde(rename = "send-err")]
    SendErr {
        /// Client-supplied correlation ID from the `send` frame.
        id: String,
        /// Whether the client may safely retry this send.
        retryable: bool,
        /// Human-readable reason for the failure.
        reason: String,
    },
    /// Fatal protocol error; the daemon will close the connection after sending this.
    #[serde(rename = "error")]
    Error {
        /// Human-readable description of the protocol violation.
        reason: String,
    },
    /// Placeholder for any `type` tag the client does not recognise (forward-compat).
    #[serde(other)]
    Unknown,
}

/// Frames the client sends to the daemon.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientFrame {
    /// First frame after connecting; establishes the client role and optional replay point.
    #[serde(rename = "hello")]
    Hello {
        /// Client role: `"viewer"` (UI) or `"channel"` (AI agent).
        role: String,
        /// If set, the daemon replays all stored messages with `seq > since_seq`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        since_seq: Option<i64>,
    },
    /// Keep-alive probe; the daemon responds with `pong`.
    #[serde(rename = "ping")]
    Ping,
    /// Request a `status` snapshot from the daemon.
    #[serde(rename = "status")]
    Status,
    /// Request to send a message to another agent.
    #[serde(rename = "send")]
    Send {
        /// Client-chosen correlation ID (UUID v4); echoed in `send-ok` / `send-err`.
        id: String,
        /// Recipient DID (`did:wba:…`).
        to: String,
        /// Message body payload (arbitrary JSON object).
        body: Value,
        /// If `true`, the relay will NOT encrypt the envelope before forwarding.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plaintext: Option<bool>,
        /// Thread identifier to attach to this envelope.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_id: Option<String>,
        /// Envelope ID this message is replying to.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        in_reply_to: Option<String>,
    },
}
