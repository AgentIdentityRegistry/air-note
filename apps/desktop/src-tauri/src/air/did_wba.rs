use super::types::Did;
use ed25519_dalek::{SigningKey, VerifyingKey};
use rand_core::OsRng;
use serde_json::{json, Value};

pub struct AgentKeypair {
    signing_key: SigningKey,
}

impl AgentKeypair {
    /// Re-hydrate a keypair from saved 32-byte secret. Used in Phase 3 (attestation signing)
    /// when the signing key is loaded from the SecretsVault on app start.
    pub fn from_secret_bytes(bytes: &[u8; 32]) -> Self {
        Self {
            signing_key: SigningKey::from_bytes(bytes),
        }
    }

    pub fn public_key_bytes(&self) -> [u8; 32] {
        self.signing_key.verifying_key().to_bytes()
    }

    pub fn secret_key_bytes(&self) -> [u8; 32] {
        self.signing_key.to_bytes()
    }

    /// Used in Phase 3 (attestation signing) for DID document publishing + signature verification.
    pub fn verifying_key(&self) -> VerifyingKey {
        self.signing_key.verifying_key()
    }

    /// Used in Phase 3 (attestation signing) to produce Ed25519 signatures over A2A payloads.
    pub fn sign(&self, msg: &[u8]) -> Vec<u8> {
        use ed25519_dalek::Signer;
        self.signing_key.sign(msg).to_bytes().to_vec()
    }
}

pub fn generate_keypair() -> AgentKeypair {
    let signing_key = SigningKey::generate(&mut OsRng);
    AgentKeypair { signing_key }
}

/// Build a did:wba identifier from a keypair and domain.
/// Format: did:wba:<domain>[:<path-segments>]
///
/// Per did:wba spec, the path identifies the agent within the domain.
/// If `path` is None, we derive a short fingerprint from the public key.
pub fn build_did(kp: &AgentKeypair, domain: &str, path: Option<&str>) -> Did {
    let path = match path {
        Some(p) => p.to_string(),
        None => {
            let pk = kp.public_key_bytes();
            // Short fingerprint: first 8 bytes of sha256(pk), hex
            use sha2::{Digest, Sha256};
            let mut h = Sha256::new();
            h.update(pk);
            let digest = h.finalize();
            hex::encode(&digest[..8])
        }
    };
    Did(format!("did:wba:{}:{}", domain, path))
}

/// Build a W3C DID Document for this DID.
/// Encodes the Ed25519 public key as multibase per did-core spec.
/// Exercised by tests; used by Phase 3 (DID document publishing) from binary.
pub fn build_did_document(did: &Did, kp: &AgentKeypair) -> Value {
    let pk_bytes = kp.public_key_bytes();
    let pk_multibase = encode_ed25519_public_multibase(&pk_bytes);
    let key_id = format!("{}#key-1", did.0);

    json!({
        "@context": [
            "https://www.w3.org/ns/did/v1",
            "https://w3id.org/security/suites/ed25519-2020/v1"
        ],
        "id": did.0,
        "verificationMethod": [{
            "id": key_id.clone(),
            "type": "Ed25519VerificationKey2020",
            "controller": did.0,
            "publicKeyMultibase": pk_multibase
        }],
        "authentication": [key_id.clone()],
        "assertionMethod": [key_id]
    })
}

/// Multibase-encode a 32-byte Ed25519 public key per multikey spec.
/// Multicodec prefix for ed25519-pub is 0xed01 (varint).
/// Called transitively from build_did_document; same Phase 3 reach.
fn encode_ed25519_public_multibase(pk: &[u8; 32]) -> String {
    let mut buf = Vec::with_capacity(34);
    // multicodec prefix for ed25519-pub: 0xed01
    buf.extend_from_slice(&[0xed, 0x01]);
    buf.extend_from_slice(pk);
    multibase::encode(multibase::Base::Base58Btc, &buf)
}
