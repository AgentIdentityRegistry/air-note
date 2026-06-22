use crate::commands::identity::AppState;
use crate::engine::EngineStatus;
use serde::Serialize;
use tauri::State;

#[derive(Serialize)]
pub struct GrantDto {
    pub canonical_root: String,
    pub granted_at: String,
    pub revoked: bool,
}
impl From<bossclaw_core::Grant> for GrantDto {
    fn from(g: bossclaw_core::Grant) -> Self {
        Self { canonical_root: g.canonical_root, granted_at: g.granted_at, revoked: g.revoked }
    }
}

#[derive(Serialize)]
pub struct FileRecordDto {
    pub canonical_path: String,
    pub file_event_id: String,
    pub content_hash: String,
    pub grant_root: String,
}
impl From<bossclaw_core::graph::FileRecord> for FileRecordDto {
    fn from(f: bossclaw_core::graph::FileRecord) -> Self {
        Self {
            canonical_path: f.canonical_path,
            file_event_id: f.file_event_id,
            content_hash: f.content_hash,
            grant_root: f.grant_root,
        }
    }
}

#[derive(Serialize)]
pub struct SkipDto {
    pub path: String,
    pub reason: String,
}

#[derive(Serialize)]
pub struct IngestReportDto {
    pub ingested: usize,
    pub superseded: usize,
    pub deduped: usize,
    pub skipped: Vec<SkipDto>,
    pub failed: Vec<SkipDto>,
}
impl From<bossclaw_core::IngestReport> for IngestReportDto {
    fn from(r: bossclaw_core::IngestReport) -> Self {
        let map = |v: Vec<(std::path::PathBuf, String)>| {
            v.into_iter()
                .map(|(p, reason)| SkipDto { path: p.to_string_lossy().into_owned(), reason })
                .collect()
        };
        Self {
            ingested: r.ingested,
            superseded: r.superseded,
            deduped: r.deduped,
            skipped: map(r.skipped),
            failed: map(r.failed),
        }
    }
}

/// Reports the brain's status: opens-or-gets the engine (gated on onboarding), verifies
/// its chain, and counts entries. Never errors — failures are encoded in `status.state`.
#[tauri::command]
pub async fn engine_status(state: State<'_, AppState>) -> Result<EngineStatus, String> {
    let onboarded = state.identity_store.is_onboarded();
    Ok(state.engine.status(onboarded).await)
}

#[tauri::command]
pub async fn engine_add_grant(path: String, state: State<'_, AppState>) -> Result<(), String> {
    let onboarded = state.identity_store.is_onboarded();
    state.engine.add_grant(onboarded, std::path::PathBuf::from(path)).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn engine_revoke_grant(path: String, state: State<'_, AppState>) -> Result<(), String> {
    let onboarded = state.identity_store.is_onboarded();
    state.engine.revoke_grant(onboarded, std::path::PathBuf::from(path)).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn engine_list_grants(state: State<'_, AppState>) -> Result<Vec<GrantDto>, String> {
    let onboarded = state.identity_store.is_onboarded();
    let grants = state.engine.list_grants(onboarded).await.map_err(|e| e.to_string())?;
    Ok(grants.into_iter().map(GrantDto::from).collect())
}

#[tauri::command]
pub async fn engine_run_ingest(state: State<'_, AppState>) -> Result<IngestReportDto, String> {
    let onboarded = state.identity_store.is_onboarded();
    let report = state.engine.run_ingest(onboarded).await.map_err(|e| e.to_string())?;
    Ok(IngestReportDto::from(report))
}

#[tauri::command]
pub async fn engine_list_files(state: State<'_, AppState>) -> Result<Vec<FileRecordDto>, String> {
    let onboarded = state.identity_store.is_onboarded();
    let files = state.engine.list_files(onboarded).await.map_err(|e| e.to_string())?;
    Ok(files.into_iter().map(FileRecordDto::from).collect())
}

#[tauri::command]
pub async fn engine_pick_folder(app: tauri::AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_folder(move |p| {
        let _ = tx.send(p);
    });
    // A cancelled dialog yields None; a dropped sender (window closed) also -> None.
    let picked = rx.await.ok().flatten();
    Ok(picked.and_then(|p| p.into_path().ok()).map(|pb| pb.to_string_lossy().into_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ingest_report_maps_to_dto() {
        let r = bossclaw_core::IngestReport {
            ingested: 2,
            skipped: vec![(std::path::PathBuf::from("/x/a.bin"), "not valid UTF-8".into())],
            ..Default::default()
        };
        let dto = IngestReportDto::from(r);
        assert_eq!(dto.ingested, 2);
        assert_eq!(dto.skipped.len(), 1);
        assert_eq!(dto.skipped[0].path, "/x/a.bin");
        assert_eq!(dto.skipped[0].reason, "not valid UTF-8");
    }
}
