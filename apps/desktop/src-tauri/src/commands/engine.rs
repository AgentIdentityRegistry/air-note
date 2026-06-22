use crate::commands::identity::AppState;
use crate::engine::EngineStatus;
use serde::Serialize;
use tauri::State;

#[derive(Serialize)]
#[allow(dead_code)] // wired in Task 6
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
#[allow(dead_code)] // wired in Task 6
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
#[allow(dead_code)] // wired in Task 6
pub struct SkipDto {
    pub path: String,
    pub reason: String,
}

#[derive(Serialize)]
#[allow(dead_code)] // wired in Task 6
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ingest_report_maps_to_dto() {
        let mut r = bossclaw_core::IngestReport::default();
        r.ingested = 2;
        r.skipped.push((std::path::PathBuf::from("/x/a.bin"), "not valid UTF-8".into()));
        let dto = IngestReportDto::from(r);
        assert_eq!(dto.ingested, 2);
        assert_eq!(dto.skipped.len(), 1);
        assert_eq!(dto.skipped[0].path, "/x/a.bin");
        assert_eq!(dto.skipped[0].reason, "not valid UTF-8");
    }
}
