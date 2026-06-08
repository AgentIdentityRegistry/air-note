use super::client_trait::*;
use super::types::*;
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Mutex;

struct StoredRecord {
    record: AgentRecord,
    secret_hash: String,
}

pub struct MockAirClient {
    store: Mutex<HashMap<String, StoredRecord>>,
}

impl MockAirClient {
    pub fn new() -> Self {
        Self {
            store: Mutex::new(HashMap::new()),
        }
    }

    fn now_iso() -> String {
        // Cheap placeholder; replace with chrono if needed elsewhere
        use std::time::{SystemTime, UNIX_EPOCH};
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        format!("1970-01-01T00:00:{}Z", secs)
    }

    fn hash_secret(secret: &str) -> String {
        let mut h = Sha256::new();
        h.update(secret.as_bytes());
        hex::encode(h.finalize())
    }

    fn random_secret() -> String {
        use rand_core::{OsRng, RngCore};
        let mut buf = [0u8; 32];
        OsRng.fill_bytes(&mut buf);
        hex::encode(buf)
    }
}

impl Default for MockAirClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AirClient for MockAirClient {
    async fn register(
        &self,
        did: &Did,
        manifest: &AgentManifest,
    ) -> Result<RegistrationResponse, AirError> {
        let secret = Self::random_secret();
        let now = Self::now_iso();
        let record = AgentRecord {
            did: did.clone(),
            name: manifest.name.clone(),
            manifest_url: None,
            trust_score: TrustScore::initial(),
            created_at: now.clone(),
            updated_at: now,
        };
        let stored = StoredRecord {
            record: record.clone(),
            secret_hash: Self::hash_secret(&secret),
        };
        self.store
            .lock()
            .unwrap()
            .insert(did.0.clone(), stored);
        Ok(RegistrationResponse {
            did: did.clone(),
            agent_secret: secret,
            record,
        })
    }

    async fn lookup(&self, did: &Did) -> Result<AgentRecord, AirError> {
        self.store
            .lock()
            .unwrap()
            .get(&did.0)
            .map(|s| s.record.clone())
            .ok_or_else(|| AirError::NotFound(did.0.clone()))
    }

    async fn update(
        &self,
        did: &Did,
        agent_secret: &str,
        manifest: &AgentManifest,
    ) -> Result<AgentRecord, AirError> {
        let mut store = self.store.lock().unwrap();
        let stored = store
            .get_mut(&did.0)
            .ok_or_else(|| AirError::NotFound(did.0.clone()))?;
        let provided = Self::hash_secret(agent_secret);
        if provided != stored.secret_hash {
            return Err(AirError::Unauthorized);
        }
        stored.record.name = manifest.name.clone();
        stored.record.updated_at = Self::now_iso();
        Ok(stored.record.clone())
    }

    async fn trust_score(&self, did: &Did) -> Result<TrustScore, AirError> {
        let store = self.store.lock().unwrap();
        store
            .get(&did.0)
            .map(|s| s.record.trust_score)
            .ok_or_else(|| AirError::NotFound(did.0.clone()))
    }

    async fn health(&self) -> Result<(), AirError> {
        Ok(())
    }
}
