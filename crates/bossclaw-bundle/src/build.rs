//! Assemble a sealed `.airmem` from gathered inputs. Pure: no engine, no I/O, no clock (the caller
//! passes `created_at`). The brain `SigningKey` seals the canonical manifest.
use bossclaw_canon::sign::{sign_bytes, SigningKey};

use crate::binding::binding_hash;
use crate::format::{Airmem, AirmemItem, Binding, ItemClass, Manifest, FORMAT_VERSION};
use crate::merkle;

/// One gathered memory to disclose.
pub enum ItemInput {
    /// External note: canonical event bytes + original write-time signature + derived origin token.
    Stamped {
        /// The original event's canonical JSON text (UTF-8), verbatim from the write-time record.
        event_bytes: String,
        /// The original write-time multibase signature over the event hash.
        signature: String,
        /// The derived origin token (`"external"`/`"unattested"`), cross-checked by verify.
        carried_origin: String,
    },
    /// Session/ingest/dossier: kind + content + optional safe display metadata.
    ///
    /// INVARIANT — `display` object keys MUST be ASCII. `serde_jcs 0.1.0` sorts keys by UTF-8 bytes
    /// while RFC-8785 §3.2.3 mandates UTF-16 code-unit order; the two diverge for any non-BMP key
    /// (every emoji is non-BMP), which would make a FOREIGN verifier compute a different leaf hash.
    /// Self-consistent for us (build + native verify + the WASM verifier are one crate), so this is a
    /// conformance constraint, not a security hole. Authored by the caller (core export); verify
    /// rejects non-ASCII `display` keys on the read side.
    ///
    /// `display` must also carry NO paths, session ids, grant roots, or lineage (spec A-N1) — that
    /// is likewise the caller's contract, not enforced here.
    SealVouched {
        /// Display kind: `"session"` | `"ingest"` | `"dossier"`.
        kind: String,
        /// The disclosed content text (a session ships its FULL transcript).
        content: String,
        /// Optional safe display metadata, subject to the two constraints above.
        display: Option<serde_json::Value>,
    },
}

/// Everything build needs that isn't derivable here.
pub struct BuildInput<'a> {
    /// Export time (RFC3339) — caller-supplied (build is clock-free).
    pub created_at: String,
    /// THE identity claim.
    pub did: String,
    /// Multibase brain verifying key.
    pub brain_verifying_key: String,
    /// Free-text selection description.
    pub selection_description: String,
    /// The gathered items, in leaf order (leaf order = item order).
    pub items: Vec<ItemInput>,
    /// The latest stored binding (verbatim).
    pub binding: Binding,
    /// The brain signing key (seals the manifest).
    pub brain_key: &'a SigningKey,
}

/// Build + seal. `items` MUST be non-empty (caller enforces `EmptySelection`).
///
/// # Panics
/// Panics if `items` is empty — `merkle::root` asserts a non-empty leaf slice. This is a
/// caller-honored precondition on the owner-side export path (Task 7 rejects an empty selection
/// before calling here); it is NOT attacker-reachable. The verify side must never reach the same
/// assert with a received bundle — it rejects `items: []` as `Malformed` first.
pub fn build_bundle(input: BuildInput<'_>) -> Airmem {
    let mut items: Vec<AirmemItem> = input
        .items
        .into_iter()
        .map(|ii| match ii {
            ItemInput::Stamped { event_bytes, signature, carried_origin } => AirmemItem {
                leaf: String::new(), class: ItemClass::Stamped, kind: "note".into(),
                event_bytes: Some(event_bytes), signature: Some(signature),
                carried_origin: Some(carried_origin), content: None, display: None,
            },
            ItemInput::SealVouched { kind, content, display } => AirmemItem {
                leaf: String::new(), class: ItemClass::SealVouched, kind,
                event_bytes: None, signature: None, carried_origin: None,
                content: Some(content), display,
            },
        })
        .collect();
    let leaves: Vec<[u8; 32]> = items.iter().map(merkle::leaf_hash).collect();
    for (item, leaf) in items.iter_mut().zip(&leaves) {
        item.leaf = hex::encode(leaf);
    }
    let manifest = Manifest {
        format_version: FORMAT_VERSION.into(), created_at: input.created_at, did: input.did,
        brain_verifying_key: input.brain_verifying_key,
        selection_description: input.selection_description,
        item_count: items.len() as u64,
        merkle_root: hex::encode(merkle::root(&leaves)),
        binding_hash: hex::encode(binding_hash(&input.binding)),
    };
    let seal = sign_bytes(
        &crate::format::canonical_json(&manifest).expect("manifest serializable"),
        input.brain_key,
    );
    Airmem { manifest, items, binding: input.binding, seal }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bossclaw_canon::sign::{sign_bytes, verify_bytes, SigningKey};
    use crate::binding::binding_signing_bytes;
    use crate::format::BindingPayload;

    fn binding(brain_mb: &str) -> Binding {
        let idk = SigningKey::from_bytes(&[9u8; 32]);
        let idvk = multibase::encode(multibase::Base::Base58Btc, idk.verifying_key().to_bytes());
        let payload = BindingPayload {
            brain_verifying_key: brain_mb.into(), identity_verifying_key: idvk,
            did: "did:wba:example.com:me".into(), purpose: "memory-signing".into(),
            epoch: 1, created_at: "2026-07-21T00:00:00Z".into(),
        };
        let sig = sign_bytes(&binding_signing_bytes(&payload), &idk);
        Binding { payload, identity_signature: sig }
    }

    #[test]
    fn build_seals_a_verifiable_manifest() {
        let brain = SigningKey::from_bytes(&[1u8; 32]);
        let brain_mb = multibase::encode(multibase::Base::Base58Btc, brain.verifying_key().to_bytes());
        let a = build_bundle(BuildInput {
            created_at: "2026-07-21T00:00:00Z".into(), did: "did:wba:example.com:me".into(),
            brain_verifying_key: brain_mb.clone(), selection_description: "1 note + 1 session".into(),
            items: vec![
                ItemInput::Stamped { event_bytes: "{}".into(), signature: "zSig".into(), carried_origin: "external".into() },
                ItemInput::SealVouched { kind: "session".into(), content: "body".into(), display: Some(serde_json::json!({"title":"S"})) },
            ],
            binding: binding(&brain_mb), brain_key: &brain,
        });
        assert_eq!(a.manifest.item_count, 2);
        assert!(a.items.iter().all(|i| i.leaf.len() == 64));
        let manifest_bytes = crate::format::canonical_json(&a.manifest).unwrap();
        verify_bytes(&manifest_bytes, &a.seal, &brain.verifying_key()).expect("seal verifies");
    }

    #[test]
    fn each_item_leaf_matches_its_recomputed_leaf_hash() {
        // The stored `leaf` must equal leaf_hash(item) AFTER the leaf field was filled — proves the
        // fill step didn't self-invalidate (leaf_hash blanks `leaf` before hashing).
        let brain = SigningKey::from_bytes(&[1u8; 32]);
        let brain_mb = multibase::encode(multibase::Base::Base58Btc, brain.verifying_key().to_bytes());
        let a = build_bundle(BuildInput {
            created_at: "2026-07-21T00:00:00Z".into(), did: "did:wba:example.com:me".into(),
            brain_verifying_key: brain_mb.clone(), selection_description: "3 items".into(),
            items: vec![
                ItemInput::Stamped { event_bytes: "{\"a\":1}".into(), signature: "zS1".into(), carried_origin: "external".into() },
                ItemInput::SealVouched { kind: "session".into(), content: "b1".into(), display: None },
                ItemInput::SealVouched { kind: "ingest".into(), content: "b2".into(), display: None },
            ],
            binding: binding(&brain_mb), brain_key: &brain,
        });
        // THIS loop — not the root assert below — is what pins leaf↔item pairing. The root assert
        // recomputes leaves from the items, so it is blind to any permutation of the stored `leaf`
        // strings. Mutation-verified: reversing or rotating the zip kills only this assert.
        for item in &a.items {
            assert_eq!(item.leaf, hex::encode(merkle::leaf_hash(item)), "stored leaf must be self-consistent");
        }
        let leaves: Vec<[u8; 32]> = a.items.iter().map(merkle::leaf_hash).collect();
        assert_eq!(a.manifest.merkle_root, hex::encode(merkle::root(&leaves)), "root must cover the stored leaves");
    }

    #[test]
    fn manifest_binding_hash_matches_the_embedded_binding() {
        // The seal covers binding_hash, so this is what makes the card un-swappable (design Blocker C1).
        let brain = SigningKey::from_bytes(&[2u8; 32]);
        let brain_mb = multibase::encode(multibase::Base::Base58Btc, brain.verifying_key().to_bytes());
        let b = binding(&brain_mb);
        let a = build_bundle(BuildInput {
            created_at: "2026-07-21T00:00:00Z".into(), did: "did:wba:example.com:me".into(),
            brain_verifying_key: brain_mb, selection_description: "1 item".into(),
            items: vec![ItemInput::SealVouched { kind: "session".into(), content: "x".into(), display: None }],
            binding: b.clone(), brain_key: &brain,
        });
        assert_eq!(a.manifest.binding_hash, hex::encode(binding_hash(&a.binding)));
        assert_eq!(a.binding, b, "the binding is embedded verbatim");
    }
}
