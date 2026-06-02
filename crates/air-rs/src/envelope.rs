//! Typed message envelopes for the AIR A2A protocol.
//!
//! An [`Envelope`] wraps a [`MessageBody`] and carries routing metadata
//! (sender DID, recipient DID, thread ID, nonce) plus an optional EdDSA
//! signature over the canonical-JSON representation of the envelope body
//! (see `signing` module and spec §5).
//!
//! **Spec reference:** <https://agentidentityregistry.org/specs/air/draft-1> §4

use serde::{Deserialize, Serialize};

/// A signed A2A message envelope carrying a [`MessageBody`] between
/// two AIR-registered agents.
///
/// All fields are required except `in_reply_to` (present only for replies /
/// continuations of an existing thread) and `signature` (absent before signing,
/// present after [`crate::signing::sign_envelope`] is called).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Envelope {
    /// Globally unique envelope identifier (UUID v4, lowercase, no braces).
    pub id: String,

    /// Sender DID (`did:wba:…`) — MUST match the signing key resolved from AIR.
    pub from: String,

    /// Recipient DID (`did:wba:…`) — used by relay for routing.
    pub to: String,

    /// ISO 8601 timestamp at which the envelope was created (UTC, second precision).
    ///
    /// Recipients MUST reject envelopes whose timestamp is more than 5 minutes
    /// in the past or more than 1 minute in the future (spec §4.5 clock-skew).
    pub timestamp: String,

    /// If this envelope is a reply, the `id` of the envelope being replied to.
    pub in_reply_to: Option<String>,

    /// Thread identifier shared across all envelopes in one negotiation.
    ///
    /// Together with `from` and `nonce` forms the replay-protection triple
    /// `(sender_did, thread_id, nonce)` maintained in an LRU at the recipient
    /// (spec §8).
    pub thread_id: String,

    /// Cryptographic nonce (UUID v4) — unique per envelope within a thread.
    pub nonce: String,

    /// The typed payload of this message.
    pub body: MessageBody,

    /// EdDSA signature (multibase `z`-prefixed base58btc) over the
    /// canonical-JSON representation of this envelope with `signature` set to
    /// `null`, per spec §5. Absent before signing.
    pub signature: Option<String>,
}

/// Discriminated union of all A2A message types.
///
/// Serialised with a `"type"` tag using snake_case variants (e.g. `"offer"`,
/// `"counter"`, `"accept"`, `"decline"`, `"withdraw"`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessageBody {
    /// Initial offer from buyer/seller to counterpart.
    Offer(Offer),
    /// Counter-proposal in response to an offer.
    Counter(Counter),
    /// Unconditional acceptance of the most recent offer or counter.
    Accept(Accept),
    /// Rejection of the most recent offer or counter.
    Decline(Decline),
    /// Cancellation of the thread by the initiating party.
    Withdraw(Withdraw),
}

/// Payload for an [`MessageBody::Offer`] message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Offer {
    /// Identifier of the item being negotiated (opaque string, catalog-scoped).
    pub item_id: String,

    /// The value offered for the item.
    pub offered_value: Value,

    /// Optional free-text note accompanying the offer.
    pub note: Option<String>,
}

/// Payload for a [`MessageBody::Counter`] message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Counter {
    /// Identifier of the item being negotiated.
    pub item_id: String,

    /// The counter-proposed value.
    pub counter_value: Value,

    /// Optional free-text note.
    pub note: Option<String>,
}

/// Payload for an [`MessageBody::Accept`] message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Accept {
    /// Identifier of the item being accepted.
    pub item_id: String,

    /// Optional free-text note.
    pub note: Option<String>,
}

/// Payload for a [`MessageBody::Decline`] message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Decline {
    /// Identifier of the item being declined.
    pub item_id: String,

    /// Optional reason for declining.
    pub reason: Option<String>,
}

/// Payload for a [`MessageBody::Withdraw`] message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Withdraw {
    /// Identifier of the item for which the thread is being withdrawn.
    pub item_id: String,

    /// Optional reason for withdrawing.
    pub reason: Option<String>,
}

/// Monetary or item-exchange value used in offers and counters.
///
/// **Spec §5 hard restriction — NO FLOATS.** All monetary amounts are
/// expressed as integer cent values plus an ISO 4217 currency code.
/// Item quantities are unsigned 32-bit integers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Value {
    /// A monetary value: `amount_cents` minor units of `currency`.
    ///
    /// Example: `{ "type": "cash", "amount_cents": 1000, "currency": "USD" }`
    /// represents USD 10.00.
    Cash {
        /// Amount in the smallest denomination of the currency (e.g. cents for
        /// USD, pence for GBP). MUST be a non-negative integer — never a float.
        amount_cents: u64,

        /// ISO 4217 three-letter currency code (e.g. `"USD"`, `"KRW"`, `"CAD"`).
        currency: String,
    },

    /// An item-for-item (barter) value.
    Item {
        /// Catalog item identifier offered in exchange.
        item_id: String,

        /// Quantity of the item. MUST be a positive integer — never a float.
        quantity: u32,
    },
}
