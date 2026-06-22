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
    state.identity_store.clear()
}
