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

/// Max length of the LOCAL display name, in characters (Unicode scalar values).
/// The display name is unsigned metadata shown in the UI/chat — it is NOT the
/// published unique @handle and NOT part of the DID, so this cap is purely a
/// sanity bound (keeps the UI readable, the JSON small) rather than a protocol rule.
const MAX_DISPLAY_NAME_LEN: usize = 80;

/// Validate + normalize a new local display name. Pure + deterministic so it can be unit-tested
/// without constructing `AppState`; the `rename_identity` command is the only caller. Returns the
/// trimmed name on success, or a human-readable error to surface inline.
fn validate_display_name(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Name can’t be empty.".to_string());
    }
    if trimmed.chars().count() > MAX_DISPLAY_NAME_LEN {
        return Err(format!("Name is too long (max {MAX_DISPLAY_NAME_LEN} characters)."));
    }
    Ok(trimmed.to_string())
}

/// The pure validate-+-mutate core of a rename: validates `raw` and, on success, sets only
/// `meta.name`. No filesystem, no keys. Factored out (instead of inlined in the command) so the
/// real command and the unit tests exercise the SAME logic — for a security command the invariant
/// "only `name` changes; `did`/`created_at` are untouched" must be asserted against the actual
/// path, not a copy that could drift. On error, `meta` is left unmodified.
fn apply_rename(meta: &mut IdentityMetadata, raw: &str) -> Result<(), String> {
    meta.name = validate_display_name(raw)?;
    Ok(())
}

/// Rename the agent's LOCAL display name only. Loads the current metadata, validates
/// the new name (trimmed, non-empty, within `MAX_DISPLAY_NAME_LEN`), swaps just the
/// `name`, and persists. The DID, the keypair, and `created_at` are left untouched —
/// the name is unsigned metadata, not derived from or bound to the key (see `air/identity.rs`).
#[tauri::command]
pub async fn rename_identity(
    state: State<'_, AppState>,
    new_name: String,
) -> Result<IdentityMetadata, String> {
    let mut meta = state
        .identity_store
        .load_metadata()?
        .ok_or_else(|| "No identity yet. Set up your agent first.".to_string())?;

    // Change ONLY the display name; did + created_at + the keypair are deliberately preserved.
    apply_rename(&mut meta, &new_name)?;
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

// These tests drive the SAME `apply_rename` core the command uses (no mirrored copy), so the
// security invariant is asserted against the real path. `apply_rename` is the validate-+-mutate
// step; the surrounding load/save is thin store I/O covered by `air/identity.rs`. Pure → runs on
// every platform.
#[cfg(test)]
mod rename_tests {
    use super::*;
    use crate::air::types::Did;

    fn sample_meta() -> IdentityMetadata {
        IdentityMetadata {
            did: Did("did:wba:example.com:agent".to_string()),
            name: "Old Name".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn apply_rename_changes_only_the_name() {
        let mut meta = sample_meta();
        let did_before = meta.did.clone();
        let created_before = meta.created_at.clone();

        apply_rename(&mut meta, "  New Name  ").unwrap();
        assert_eq!(meta.name, "New Name", "name updates (and is trimmed)");
        assert_eq!(meta.did, did_before, "did untouched");
        assert_eq!(meta.created_at, created_before, "created_at untouched");
    }

    #[test]
    fn empty_or_whitespace_name_is_rejected_and_meta_unchanged() {
        for bad in ["", "   "] {
            let mut meta = sample_meta();
            let before = meta.clone();

            assert!(apply_rename(&mut meta, bad).is_err(), "{bad:?} rejected");
            // A rejected rename leaves every field exactly as it was.
            assert_eq!(meta.name, before.name);
            assert_eq!(meta.did, before.did);
            assert_eq!(meta.created_at, before.created_at);
        }
    }

    #[test]
    fn over_length_name_is_rejected_and_meta_unchanged() {
        let mut meta = sample_meta();
        let before = meta.clone();

        let long = "x".repeat(MAX_DISPLAY_NAME_LEN + 1);
        assert!(apply_rename(&mut meta, &long).is_err());
        assert_eq!(meta.name, before.name, "rejected over-length leaves name intact");
        assert_eq!(meta.did, before.did);
        assert_eq!(meta.created_at, before.created_at);

        // The boundary length is accepted.
        let at_cap = "x".repeat(MAX_DISPLAY_NAME_LEN);
        apply_rename(&mut meta, &at_cap).unwrap();
        assert_eq!(meta.name, at_cap);
    }
}

// The engine half of reset is Unix-only, so this test (which exercises it) is too.
#[cfg(all(test, unix))]
mod tests {
    use crate::engine::{embed, EngineHandle, EngineState};
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
        let engine = Arc::new(EngineHandle::new(vault.clone(), dir.path().to_path_buf(), Arc::new(embed::MockEmbedderProvider::new(8)), Arc::new(crate::engine::reason::MockReasonerProvider::new("m"))));
        engine.get_or_open(true).await.unwrap();
        // Simulate the engine half of reset_identity:
        engine.teardown().await.unwrap();
        assert!(vault.get("air-agent.engine.dek").unwrap().is_none());
        assert!(!dir.path().join("brain.db").exists());
        assert!(matches!(engine.status(false).await.state, EngineState::NotOnboarded));
    }
}
