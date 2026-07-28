//! The signed event: the single authoritative record. JCS-canonicalized exactly
//! like `air-rs/signing.rs` so bytes are deterministic and cross-language stable.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::CanonError;

/// Provenance for a model-derived (Tier-B) event. `source_event_ids` MUST be
/// non-empty for Tier-B events (enforced at append, see `EventLog::append`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelMeta {
    /// The model that produced this event (e.g. the local reasoner id).
    pub model_id: String,
    /// Hash of the prompt used (provenance, not reproducibility).
    pub prompt_hash: String,
    /// The source event ids this conclusion was derived from. Non-empty.
    pub source_event_ids: Vec<String>,
}

/// A single signed, hash-chained event. `hash` and `signature` are excluded
/// from the canonical bytes (they are computed *over* the canonical bytes).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Event {
    /// ULID, monotonic-ish, lexicographically sortable.
    pub id: String,
    /// Ingestion time, RFC 3339.
    pub ts: String,
    /// Optional valid-time (bi-temporal), RFC 3339.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_time: Option<String>,
    /// Event type discriminator. Serialized as `"type"`.
    #[serde(rename = "type")]
    pub event_type: String,
    /// Opaque content payload.
    pub content: serde_json::Value,
    /// Provenance for Tier-B (model-derived) events; `None` for Tier-A.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_meta: Option<ModelMeta>,
    /// Hex of the previous event's 32-byte hash; genesis = 64 zeros.
    pub prev_hash: String,
    /// Hex of this event's 32-byte hash. Excluded from canon; set by `append`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    /// DID of the signer.
    pub signed_by_did: String,
    /// Multibase signature over `hash`. Excluded from canon; set by `append`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

/// Produce the canonical JSON bytes of an event per the spec §5.2 recipe:
/// serialize → strip `hash` + `signature` → NFC-normalize every string →
/// RFC 8785 JCS via `serde_jcs`. Mirrors `air-rs/signing.rs::canonical_bytes`.
pub fn canonical_bytes(event: &Event) -> Result<Vec<u8>, CanonError> {
    let mut value = serde_json::to_value(event)
        .map_err(|e| CanonError::Canonical(format!("event to_value: {e}")))?;
    if let serde_json::Value::Object(ref mut map) = value {
        map.remove("hash");
        map.remove("signature");
    }
    nfc_normalize(&mut value);
    serde_jcs::to_vec(&value).map_err(|e| CanonError::Canonical(format!("serde_jcs: {e}")))
}

/// Compute the 32-byte chain hash: `SHA256(prev_hash_bytes ‖ canonical_bytes)`.
pub fn compute_hash(event: &Event) -> Result<[u8; 32], CanonError> {
    let prev = hex::decode(&event.prev_hash)
        .map_err(|e| CanonError::Chain(format!("prev_hash not hex: {e}")))?;
    if prev.len() != 32 {
        return Err(CanonError::Chain(format!(
            "prev_hash must be 32 bytes, got {}",
            prev.len()
        )));
    }
    let canon = canonical_bytes(event)?;
    let mut hasher = Sha256::new();
    hasher.update(&prev);
    hasher.update(&canon);
    Ok(hasher.finalize().into())
}

fn nfc_normalize(value: &mut serde_json::Value) {
    use unicode_normalization::UnicodeNormalization;
    match value {
        serde_json::Value::String(s) => *s = s.nfc().collect(),
        serde_json::Value::Object(map) => map.values_mut().for_each(nfc_normalize),
        serde_json::Value::Array(arr) => arr.iter_mut().for_each(nfc_normalize),
        _ => {}
    }
}

/// The taint stamp written at `content["origin"]` of every externally-sourced event (remember()
/// notes, captured sessions, file ingests). Single-sourced so the stamp site and the `is_external`
/// classifier can never drift. (Moved from graph.rs:75, zero value change.)
pub const EXTERNAL_ORIGIN: &str = "external";

/// True iff `event` is externally-tainted — reads the single-sourced `EXTERNAL_ORIGIN` stamp.
/// (Moved from ingest.rs:716, zero behavior change.)
pub fn is_external(event: &Event) -> bool {
    event.content.get("origin").and_then(|v| v.as_str()) == Some(EXTERNAL_ORIGIN)
}
