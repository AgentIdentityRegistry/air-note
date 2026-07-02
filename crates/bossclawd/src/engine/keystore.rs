// Copied from apps/desktop/src-tauri/src/engine/keystore.rs (M1a Task 4); the in-app original is removed in Task 6.

//! Mints / loads / deletes the engine's two secrets (brain Ed25519 key + DEK) via the
//! per-key `SecretsVault` — the same backend `IdentityStore` uses. Returns key material
//! in `Zeroizing` so it is wiped from memory on drop.

use crate::engine::EngineError;
use crate::secrets::SecretsVault;
use ed25519_dalek::SigningKey;
use rand_core::{OsRng, RngCore};
use std::sync::Arc;
use zeroize::Zeroizing;

/// Keychain slot for the engine's Ed25519 signing key (distinct from the identity key).
/// MUST match the app's `keystore.rs` value EXACTLY (the vault-key seam): the app and the
/// daemon read/write the SAME slot.
const SIGNING_KEY_SLOT: &str = "air-agent.engine.signing_key";
/// Keychain slot for the 32-byte SQLCipher data-encryption key. MUST match the app's
/// `keystore.rs` value EXACTLY (the vault-key seam).
const DEK_SLOT: &str = "air-agent.engine.dek";

/// The unlocked engine key material. DEK is zeroized on drop.
pub struct EngineKeys {
    pub dek: Zeroizing<[u8; 32]>,
    pub signing_key: SigningKey,
}

#[derive(Clone)]
pub struct EngineKeystore {
    vault: Arc<dyn SecretsVault>,
}

impl EngineKeystore {
    pub fn new(vault: Arc<dyn SecretsVault>) -> Self {
        Self { vault }
    }

    /// Load both secrets, or mint+persist both on first run. Errors `KeystoreInconsistent`
    /// if exactly one is present (never silently re-mints — that would orphan the DB).
    pub fn load_or_mint(&self) -> Result<EngineKeys, EngineError> {
        let sk = self.vault.get(SIGNING_KEY_SLOT).map_err(EngineError::Vault)?;
        let dek = self.vault.get(DEK_SLOT).map_err(EngineError::Vault)?;
        match (sk, dek) {
            (Some(sk_hex), Some(dek_hex)) => Ok(EngineKeys {
                signing_key: decode_signing_key(&sk_hex)?,
                dek: decode_dek(&dek_hex)?,
            }),
            (None, None) => self.mint(),
            _ => Err(EngineError::KeystoreInconsistent),
        }
    }

    fn mint(&self) -> Result<EngineKeys, EngineError> {
        let signing_key = SigningKey::generate(&mut OsRng);
        let mut dek = Zeroizing::new([0u8; 32]);
        OsRng.fill_bytes(dek.as_mut());
        // Persist BOTH before returning (so a half-mint never reaches open).
        self.vault
            .set(SIGNING_KEY_SLOT, &hex::encode(signing_key.to_bytes()))
            .map_err(EngineError::Vault)?;
        self.vault
            .set(DEK_SLOT, &hex::encode(dek.as_ref()))
            .map_err(EngineError::Vault)?;
        Ok(EngineKeys { dek, signing_key })
    }

    /// Delete both slots (identity-reset teardown). Attempts both deletes, then reports
    /// the first failure.
    pub fn delete(&self) -> Result<(), EngineError> {
        let a = self.vault.delete(SIGNING_KEY_SLOT);
        let b = self.vault.delete(DEK_SLOT);
        a.and(b).map_err(EngineError::Vault)
    }
}

fn decode_dek(hex_str: &str) -> Result<Zeroizing<[u8; 32]>, EngineError> {
    // Both slots are always self-written as hex of a fixed-size array, so ANY decode
    // failure (bad hex or wrong length) means the stored material is corrupt =
    // KeystoreInconsistent. KeystoreDbMismatch is reserved for EventLog::open failures.
    let raw = Zeroizing::new(hex::decode(hex_str).map_err(|_| EngineError::KeystoreInconsistent)?);
    if raw.len() != 32 {
        return Err(EngineError::KeystoreInconsistent);
    }
    let mut dek = Zeroizing::new([0u8; 32]);
    dek.copy_from_slice(&raw);
    Ok(dek)
}

fn decode_signing_key(hex_str: &str) -> Result<SigningKey, EngineError> {
    let raw = hex::decode(hex_str).map_err(|_| EngineError::KeystoreInconsistent)?;
    let bytes: [u8; 32] = raw
        .try_into()
        .map_err(|_| EngineError::KeystoreInconsistent)?;
    Ok(SigningKey::from_bytes(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// Minimal in-memory vault for hermetic tests (mirrors secrets::tests::MockVault).
    struct TestVault {
        store: Mutex<HashMap<String, String>>,
    }
    impl TestVault {
        fn new() -> Arc<Self> {
            Arc::new(Self { store: Mutex::new(HashMap::new()) })
        }
    }
    impl SecretsVault for TestVault {
        fn set(&self, k: &str, v: &str) -> Result<(), String> {
            self.store.lock().unwrap().insert(k.into(), v.into());
            Ok(())
        }
        fn get(&self, k: &str) -> Result<Option<String>, String> {
            Ok(self.store.lock().unwrap().get(k).cloned())
        }
        fn delete(&self, k: &str) -> Result<(), String> {
            self.store.lock().unwrap().remove(k);
            Ok(())
        }
    }

    #[test]
    fn first_run_mints_both_slots_and_is_stable() {
        let vault = TestVault::new();
        let ks = EngineKeystore::new(vault.clone());
        let k1 = ks.load_or_mint().expect("mint");
        // Both slots now populated.
        assert!(vault.get(SIGNING_KEY_SLOT).unwrap().is_some());
        assert!(vault.get(DEK_SLOT).unwrap().is_some());
        // Second load returns the SAME bytes (no re-mint).
        let k2 = ks.load_or_mint().expect("load");
        assert_eq!(*k1.dek, *k2.dek);
        assert_eq!(k1.signing_key.to_bytes(), k2.signing_key.to_bytes());
    }

    #[test]
    fn partial_state_is_a_hard_error() {
        let vault = TestVault::new();
        vault.set(SIGNING_KEY_SLOT, &hex::encode([7u8; 32])).unwrap(); // only the key
        let ks = EngineKeystore::new(vault);
        assert!(matches!(ks.load_or_mint(), Err(EngineError::KeystoreInconsistent)));
    }

    #[test]
    fn delete_removes_both_slots() {
        let vault = TestVault::new();
        let ks = EngineKeystore::new(vault.clone());
        ks.load_or_mint().unwrap();
        ks.delete().unwrap();
        assert!(vault.get(SIGNING_KEY_SLOT).unwrap().is_none());
        assert!(vault.get(DEK_SLOT).unwrap().is_none());
    }
}
