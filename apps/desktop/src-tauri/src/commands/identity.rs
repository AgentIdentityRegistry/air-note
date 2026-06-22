use crate::air::{
    build_did, generate_keypair, AgentManifest, AirClient, IdentityMetadata, IdentityStore,
};
use std::sync::Arc;
use tauri::State;

/// App-level state holding the active AIR client + identity store.
pub struct AppState {
    pub air_client: Arc<dyn AirClient>,
    pub identity_store: IdentityStore,
    pub inbox: std::sync::Arc<crate::inbox::manager::InboxManager>,
    // Engine spine is Unix-only until M7 (bossclaw-core doesn't build on Windows yet).
    #[cfg(unix)]
    pub engine: std::sync::Arc<crate::engine::EngineHandle>,
}

#[tauri::command]
pub async fn is_onboarded(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(state.identity_store.is_onboarded())
}

#[tauri::command]
pub async fn get_identity(
    state: State<'_, AppState>,
) -> Result<Option<IdentityMetadata>, String> {
    state.identity_store.load_metadata()
}

#[tauri::command]
pub async fn get_trust_score(
    state: State<'_, AppState>,
) -> Result<Option<u8>, String> {
    let meta = state.identity_store.load_metadata()?;
    let did = match meta {
        Some(m) => m.did,
        None => return Ok(None),
    };
    match state.air_client.trust_score(&did).await {
        Ok(score) => Ok(Some(score.0)),
        Err(_) => Ok(None),
    }
}

#[tauri::command]
pub async fn create_identity(
    state: State<'_, AppState>,
    name: String,
    domain: String,
) -> Result<IdentityMetadata, String> {
    // 1. Generate keypair
    let kp = generate_keypair();

    // 2. Build DID
    let did = build_did(&kp, &domain, None);

    // 3. Build manifest
    let manifest = AgentManifest {
        name: name.clone(),
        description: "AIR Agent owned by user (v1)".to_string(),
        capabilities: vec![
            "a2a-negotiate-marketplace".to_string(),
            "sign-attestation".to_string(),
        ],
        owner_hint: Some("human-controlled".to_string()),
    };

    // 4. Register with AIR
    let resp = state
        .air_client
        .register(&did, &manifest)
        .await
        .map_err(|e| e.to_string())?;

    // 5. Persist
    state.identity_store.save_signing_key(&kp.secret_key_bytes())?;
    state.identity_store.save_air_secret(&resp.agent_secret)?;

    let meta = IdentityMetadata {
        did: resp.did.clone(),
        name,
        created_at: resp.record.created_at,
    };
    state.identity_store.save_metadata(&meta)?;

    Ok(meta)
}

#[tauri::command]
pub async fn reset_identity(state: State<'_, AppState>) -> Result<(), String> {
    // Clear identity first, then tear the engine down so a re-onboard starts on a clean
    // brain (otherwise the OLD identity's memories silently re-attach — see spec Rev 2).
    // Both halves are idempotent, so a partial failure is retry-safe: the surfaced error
    // prompts a retry that completes the rest, and a half-done reset never leaves a brain
    // reachable by a NEW identity (the onboarding gate + fresh-key mint prevent that).
    state.identity_store.clear()?;
    // On Windows there is no engine, so reset only clears the identity slots above.
    #[cfg(unix)]
    state.engine.teardown().await.map_err(|e| e.to_string())?;
    Ok(())
}

// The engine half of reset is Unix-only, so this test (which exercises it) is too.
#[cfg(all(test, unix))]
mod tests {
    use crate::engine::{EngineHandle, EngineState};
    use crate::secrets::SecretsVault;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    struct TestVault { store: Mutex<HashMap<String, String>> }
    impl TestVault { fn new() -> Arc<Self> { Arc::new(Self { store: Mutex::new(HashMap::new()) }) } }
    impl SecretsVault for TestVault {
        fn set(&self, k: &str, v: &str) -> Result<(), String> { self.store.lock().unwrap().insert(k.into(), v.into()); Ok(()) }
        fn get(&self, k: &str) -> Result<Option<String>, String> { Ok(self.store.lock().unwrap().get(k).cloned()) }
        fn delete(&self, k: &str) -> Result<(), String> { self.store.lock().unwrap().remove(k); Ok(()) }
    }

    #[tokio::test]
    async fn reset_tears_down_the_engine() {
        let dir = tempfile::tempdir().unwrap();
        let vault = TestVault::new();
        let engine = Arc::new(EngineHandle::new(vault.clone(), dir.path().to_path_buf()));
        engine.get_or_open(true).await.unwrap();
        // Simulate the engine half of reset_identity:
        engine.teardown().await.unwrap();
        assert!(vault.get("air-agent.engine.dek").unwrap().is_none());
        assert!(!dir.path().join("brain.db").exists());
        assert!(matches!(engine.status(false).await.state, EngineState::NotOnboarded));
    }
}
