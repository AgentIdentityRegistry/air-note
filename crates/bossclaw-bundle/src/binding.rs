//! Binding (ID card) canonical form: the bytes the identity key signs, the hash the seal commits to,
//! the L1 internal-consistency check, and an encoding-agnostic Ed25519 key decode. Identity
//! resolution NEVER starts here (spec §2.3) — the sealed `manifest.did` is authoritative.
use sha2::{Digest, Sha256};
use bossclaw_canon::sign::{verify_bytes, VerifyingKey};
use crate::format::{Binding, BindingPayload};

/// The exact bytes the identity key signs (and re-verifies): `jcs(payload)`.
pub fn binding_signing_bytes(payload: &BindingPayload) -> Vec<u8> {
    crate::format::canonical_json(payload).expect("binding payload is always serializable")
}

/// `H(jcs(binding))` — the value `manifest.binding_hash` must equal (C-NEW-2). Hashes the WHOLE
/// binding so a tampered signature also trips `BindingHashMismatch`.
pub fn binding_hash(binding: &Binding) -> [u8; 32] {
    let canon = crate::format::canonical_json(binding).expect("binding is always serializable");
    Sha256::new().chain_update(&canon).finalize().into()
}

/// L1 internal consistency: the identity signature verifies over `jcs(payload)` against the EMBEDDED
/// `identity_verifying_key`. ZERO identity assurance (the key is attacker-choosable, C3).
pub fn verify_binding_internal(binding: &Binding) -> bool {
    let Some(raw) = decode_ed25519_key(&binding.payload.identity_verifying_key) else { return false };
    let Ok(vk) = VerifyingKey::from_bytes(&raw) else { return false };
    verify_bytes(&binding_signing_bytes(&binding.payload), &binding.identity_signature, &vk).is_ok()
}

/// Decode a multibase Ed25519 public key to its raw 32 bytes, accepting BOTH the bare-32 form and
/// the `0xed01`-prefixed multikey form (finding #6: the registry publishes multikey; the binding
/// stores bare — L2 must compare the RAW KEY BYTES, never the encoded strings).
pub fn decode_ed25519_key(mb: &str) -> Option<[u8; 32]> {
    let (_b, raw) = multibase::decode(mb).ok()?;
    let bytes: &[u8] = if raw.len() == 34 && raw[0] == 0xed && raw[1] == 0x01 { &raw[2..] } else { &raw };
    bytes.try_into().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bossclaw_canon::sign::{sign_bytes, SigningKey};
    fn signed_binding(id_key: &SigningKey, epoch: u64) -> Binding {
        let idvk = multibase::encode(multibase::Base::Base58Btc, id_key.verifying_key().to_bytes());
        let payload = BindingPayload {
            brain_verifying_key: "zBrain".into(), identity_verifying_key: idvk,
            did: "did:wba:example.com:me".into(), purpose: "memory-signing".into(),
            epoch, created_at: "2026-07-21T00:00:00Z".into(),
        };
        let sig = sign_bytes(&binding_signing_bytes(&payload), id_key);
        Binding { payload, identity_signature: sig }
    }
    #[test]
    fn internal_verify_passes_for_a_correctly_signed_card() {
        assert!(verify_binding_internal(&signed_binding(&SigningKey::from_bytes(&[3u8; 32]), 1)));
    }
    #[test]
    fn internal_verify_fails_if_payload_tampered() {
        let mut b = signed_binding(&SigningKey::from_bytes(&[3u8; 32]), 1);
        b.payload.did = "did:wba:evil.com:attacker".into();
        assert!(!verify_binding_internal(&b));
    }
    #[test]
    fn binding_hash_covers_signature() {
        let b = signed_binding(&SigningKey::from_bytes(&[3u8; 32]), 1);
        let h = binding_hash(&b);
        assert_eq!(h, binding_hash(&b));
        let mut t = b.clone(); t.identity_signature = "zDIFFERENT".into();
        assert_ne!(h, binding_hash(&t));
    }
    #[test]
    fn decode_accepts_bare_and_multikey_forms() {
        let k = SigningKey::from_bytes(&[5u8; 32]);
        let bytes = k.verifying_key().to_bytes();
        let bare = multibase::encode(multibase::Base::Base58Btc, bytes);
        let mut multikey = vec![0xed, 0x01]; multikey.extend_from_slice(&bytes);
        let mk = multibase::encode(multibase::Base::Base58Btc, &multikey);
        assert_eq!(decode_ed25519_key(&bare).unwrap(), bytes);
        assert_eq!(decode_ed25519_key(&mk).unwrap(), bytes, "multikey and bare decode to the SAME 32 bytes");
    }
    #[test]
    fn decode_rejects_garbage_without_panicking() {
        assert!(decode_ed25519_key("not-multibase-at-all").is_none());
        assert!(decode_ed25519_key("").is_none());
        // right prefix, wrong length
        let short = multibase::encode(multibase::Base::Base58Btc, [0xed, 0x01, 0x00]);
        assert!(decode_ed25519_key(&short).is_none());
        // no prefix, wrong length
        let long = multibase::encode(multibase::Base::Base58Btc, [7u8; 40]);
        assert!(decode_ed25519_key(&long).is_none());
    }
}
