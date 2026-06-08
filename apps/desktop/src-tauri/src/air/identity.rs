use super::types::Did;
use crate::secrets::SecretsVault;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityMetadata {
    pub did: Did,
    pub name: String,
    pub created_at: String,
}

/// Storage layout:
///   - Private key bytes → SecretsVault under key "bossclaw.agent.signing_key"
///   - Agent secret (returned by AIR) → SecretsVault under "bossclaw.agent.air_secret"
///   - Public metadata → JSON at <app_data_dir>/identity.json
pub struct IdentityStore {
    vault: Arc<dyn SecretsVault>,
    data_dir: PathBuf,
}

impl IdentityStore {
    pub fn new(vault: Arc<dyn SecretsVault>, data_dir: PathBuf) -> Self {
        Self { vault, data_dir }
    }

    const SIGNING_KEY: &'static str = "bossclaw.agent.signing_key";
    const AIR_SECRET: &'static str = "bossclaw.agent.air_secret";
    const METADATA_FILE: &'static str = "identity.json";

    pub fn save_signing_key(&self, bytes: &[u8; 32]) -> Result<(), String> {
        self.vault.set(Self::SIGNING_KEY, &hex::encode(bytes))
    }

    /// Used in Phase 3 (attestation signing): loads the private key from SecretsVault
    /// so AgentKeypair::from_secret_bytes can re-hydrate it on app start.
    pub fn load_signing_key(&self) -> Result<Option<[u8; 32]>, String> {
        match self.vault.get(Self::SIGNING_KEY)? {
            Some(hex_str) => {
                let bytes = hex::decode(&hex_str).map_err(|e| e.to_string())?;
                if bytes.len() != 32 {
                    return Err(format!("expected 32 bytes, got {}", bytes.len()));
                }
                let mut out = [0u8; 32];
                out.copy_from_slice(&bytes);
                Ok(Some(out))
            }
            None => Ok(None),
        }
    }

    pub fn save_air_secret(&self, secret: &str) -> Result<(), String> {
        self.vault.set(Self::AIR_SECRET, secret)
    }

    /// Used in Phase 3: loads the AIR per-agent secret so update() calls can authenticate.
    pub fn load_air_secret(&self) -> Result<Option<String>, String> {
        self.vault.get(Self::AIR_SECRET)
    }

    pub fn save_metadata(&self, meta: &IdentityMetadata) -> Result<(), String> {
        std::fs::create_dir_all(&self.data_dir).map_err(|e| e.to_string())?;
        let path = self.data_dir.join(Self::METADATA_FILE);
        let json = serde_json::to_string_pretty(meta).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| e.to_string())
    }

    pub fn load_metadata(&self) -> Result<Option<IdentityMetadata>, String> {
        let path = self.data_dir.join(Self::METADATA_FILE);
        if !path.exists() {
            return Ok(None);
        }
        let json = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let meta: IdentityMetadata = serde_json::from_str(&json).map_err(|e| e.to_string())?;
        Ok(Some(meta))
    }

    pub fn clear(&self) -> Result<(), String> {
        self.vault.delete(Self::SIGNING_KEY)?;
        self.vault.delete(Self::AIR_SECRET)?;
        let path = self.data_dir.join(Self::METADATA_FILE);
        if path.exists() {
            std::fs::remove_file(path).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub fn is_onboarded(&self) -> bool {
        self.load_metadata().map(|m| m.is_some()).unwrap_or(false)
    }
}
