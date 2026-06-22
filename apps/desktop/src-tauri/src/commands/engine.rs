use crate::commands::identity::AppState;
use crate::engine::EngineStatus;
use tauri::State;

/// Reports the brain's status: opens-or-gets the engine (gated on onboarding), verifies
/// its chain, and counts entries. Never errors — failures are encoded in `status.state`.
#[tauri::command]
pub async fn engine_status(state: State<'_, AppState>) -> Result<EngineStatus, String> {
    let onboarded = state.identity_store.is_onboarded();
    Ok(state.engine.status(onboarded).await)
}
