//! The `.airmem` document shapes + the canonical-JSON serializer. Standards-shaped (spec §2.1):
//! liftable into C2PA/VC/SCITT later without re-signing semantics.
use serde::{Deserialize, Serialize};

/// The `.airmem` version. Verifier refuses a newer MAJOR (spec §2.2 `FormatTooNew`).
pub const FORMAT_VERSION: &str = "1.0.0";

/// The whole document: `{ manifest, items, binding, seal }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Airmem {
    /// The sealed sub-document (commits to items via `merkle_root`, to the card via `binding_hash`).
    pub manifest: Manifest,
    /// The disclosed memories, in leaf order.
    pub items: Vec<AirmemItem>,
    /// The ID card (hash-committed by `manifest.binding_hash`).
    pub binding: Binding,
    /// One master Ed25519 signature (multibase) by the brain key over `jcs(manifest)`.
    pub seal: String,
}

/// The seal's message.
///
/// `deny_unknown_fields` is load-bearing, not hygiene: verify re-serializes THIS PARSED STRUCT via
/// [`canonical_json`] rather than trusting the received byte stream, so without the attribute an
/// injected extra key would be silently dropped at parse time and the seal would still verify —
/// letting the attacker smuggle a field that every consumer downstream of verify might render.
/// Rejecting at the parse boundary is what makes canonicalize-from-struct safe (Task-2 review #2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Semver; verifier refuses a newer major.
    pub format_version: String,
    /// Export time, RFC3339.
    pub created_at: String,
    /// THE authoritative identity claim (spec §2.3). Resolution always starts here, never binding.
    pub did: String,
    /// The brain verifying key (multibase). Every item stamp is checked against THIS only (A6).
    pub brain_verifying_key: String,
    /// Free-text selection description shown to the receiver.
    pub selection_description: String,
    /// Number of items (integer; canonical-JSON no-float discipline).
    pub item_count: u64,
    /// Hex of the Merkle root over `items`.
    pub merkle_root: String,
    /// Hex of `H(jcs(binding))` — the seal thereby atomically covers the ID card (C1).
    pub binding_hash: String,
}

/// Which verification class an item belongs to (spec §2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemClass {
    /// External `remember()` notes: disclosed event canonical bytes + original write-time signature.
    Stamped,
    /// Sessions/ingests/dossiers: content + display metadata ONLY (export-time vouching, H1).
    SealVouched,
}

/// One disclosed memory. `leaf` and `carried_origin` are EXCLUDED from the leaf hash (finding A7 +
/// critic C-origin: `carried_origin` is a display token whose integrity comes from the verifier's
/// cross-check against the stamp-covered event bytes, so a flip yields `OriginMismatch`, not a hash break).
///
/// The type is deliberately FLAT (one struct, two classes), so it permits combinations the format
/// forbids. `deny_unknown_fields` closes the injected-key channel; the class↔field-set invariant
/// (`verify::check_item_shape`) closes the illegal-combination channel. Neither is optional.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AirmemItem {
    /// Hex of this item's Merkle leaf hash. Excluded when computing the leaf (see merkle.rs).
    pub leaf: String,
    /// The verification class.
    pub class: ItemClass,
    /// Display kind: `"note"` | `"session"` | `"ingest"` | `"dossier"`. Drives the verifier's label.
    pub kind: String,
    /// STAMPED only: the original event's canonical JSON text (UTF-8; hash/sig excluded, per canon).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_bytes: Option<String>,
    /// STAMPED only: the original write-time multibase signature over the event hash.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    /// STAMPED only: the carried origin token (`"external"`/`"unattested"`), cross-checked by verify
    /// against the recomputed origin (A5 → `OriginMismatch`). Merkle-excluded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub carried_origin: Option<String>,
    /// SEAL_VOUCHED only: the disclosed content text (a session ships its FULL transcript).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// SEAL_VOUCHED only: safe display metadata (NEVER paths/session_id/grant_root/lineage — A-N1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display: Option<serde_json::Value>,
}

/// The ID card payload (canonical) + its identity signature.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    /// The signed fields (spec §2.3).
    pub payload: BindingPayload,
    /// Multibase base58btc signature over `jcs(payload)` by the identity key (`did_wba.rs:33` wrapped).
    pub identity_signature: String,
}

/// The binding payload — hash-committed by `manifest.binding_hash`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingPayload {
    /// The brain verifying key (multibase). MUST equal `manifest.brain_verifying_key` (C1).
    pub brain_verifying_key: String,
    /// The identity verifying key (multibase) — the offline-checkable key (A3/C3).
    pub identity_verifying_key: String,
    /// The AIR did. MUST equal `manifest.did` (C1).
    pub did: String,
    /// Fixed `"memory-signing"`.
    pub purpose: String,
    /// Monotonic integer reserved for future rotation semantics (A8/C9). First-write-wins per epoch.
    pub epoch: u64,
    /// Mint time, RFC3339.
    pub created_at: String,
}

/// Canonical JSON bytes (JCS RFC-8785) of any serializable value — the single canonicalizer.
pub fn canonical_json(value: &impl Serialize) -> Result<Vec<u8>, serde_json::Error> {
    serde_jcs::to_vec(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> Airmem {
        Airmem {
            manifest: Manifest {
                format_version: FORMAT_VERSION.into(), created_at: "2026-07-21T00:00:00Z".into(),
                did: "did:wba:example.com:me".into(), brain_verifying_key: "zBrain".into(),
                selection_description: "2 items".into(), item_count: 1,
                merkle_root: "ab".repeat(32), binding_hash: "cd".repeat(32),
            },
            items: vec![AirmemItem {
                leaf: "ef".repeat(32), class: ItemClass::Stamped, kind: "note".into(),
                event_bytes: Some("{}".into()), signature: Some("zSig".into()),
                carried_origin: Some("external".into()), content: None, display: None,
            }],
            binding: Binding {
                payload: BindingPayload {
                    brain_verifying_key: "zBrain".into(), identity_verifying_key: "zId".into(),
                    did: "did:wba:example.com:me".into(), purpose: "memory-signing".into(),
                    epoch: 1, created_at: "2026-07-21T00:00:00Z".into(),
                },
                identity_signature: "zIdSig".into(),
            },
            seal: "zSeal".into(),
        }
    }
    #[test]
    fn airmem_round_trips_through_canonical_json() {
        let a = sample();
        let back: Airmem = serde_json::from_slice(&canonical_json(&a).unwrap()).unwrap();
        assert_eq!(a, back);
    }
    #[test]
    fn canonical_json_is_key_sorted_and_deterministic() {
        let a = sample();
        assert_eq!(canonical_json(&a).unwrap(), canonical_json(&a).unwrap());
        let s = String::from_utf8(canonical_json(&a.manifest).unwrap()).unwrap();
        assert!(s.find("binding_hash").unwrap() < s.find("brain_verifying_key").unwrap(), "JCS sorts keys");
    }
    /// Re-serialize `a`, splice `extra` into the object at `pointer`, and try to parse it back.
    fn parse_with_injected_field(a: &Airmem, pointer: &str, extra: &str) -> Result<Airmem, serde_json::Error> {
        let mut v: serde_json::Value = serde_json::to_value(a).unwrap();
        let target = if pointer.is_empty() { &mut v } else { v.pointer_mut(pointer).expect("pointer resolves") };
        target.as_object_mut().unwrap().insert(extra.into(), serde_json::json!("smuggled"));
        serde_json::from_value(v)
    }

    #[test]
    fn unknown_fields_are_rejected_at_the_parse_boundary() {
        // Verify canonicalizes FROM THE PARSED STRUCT, so a silently-dropped extra key would ride
        // under a still-valid seal. These four types are the ones that feed hashes/signatures.
        let a = sample();
        for (pointer, field) in [
            ("/manifest", "evil"),
            ("/items/0", "evil"),
            ("/binding", "evil"),
            ("/binding/payload", "evil"),
        ] {
            assert!(
                parse_with_injected_field(&a, pointer, field).is_err(),
                "an injected `{field}` at `{pointer}` must fail to parse, never be dropped"
            );
        }
    }

    #[test]
    fn known_fields_still_parse() {
        // Regression guard: deny_unknown_fields must not have broken the honest round-trip.
        let a = sample();
        let back: Airmem = serde_json::from_value(serde_json::to_value(&a).unwrap()).unwrap();
        assert_eq!(a, back);
    }

    #[test]
    fn seal_vouched_item_round_trips() {
        let mut a = sample();
        a.items = vec![AirmemItem {
            leaf: "12".repeat(32), class: ItemClass::SealVouched, kind: "session".into(),
            event_bytes: None, signature: None, carried_origin: None,
            content: Some("full transcript text".into()),
            display: Some(serde_json::json!({"when": "2026-07-20", "source": "session"})),
        }];
        let back: Airmem = serde_json::from_slice(&canonical_json(&a).unwrap()).unwrap();
        assert_eq!(a, back);
    }
}
