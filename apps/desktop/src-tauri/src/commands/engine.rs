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
    /// True iff this file's `grant_root` is under an active write-grant (Lock 1).
    pub writable: bool,
}
impl FileRecordDto {
    /// Build from a record + the set of active writable roots (a file is writable iff its
    /// ingest grant_root is one of them — same root identity the write-grant uses).
    pub fn from_parts(f: bossclaw_core::graph::FileRecord, writable_roots: &std::collections::HashSet<String>) -> Self {
        let writable = writable_roots.contains(&f.grant_root);
        Self {
            canonical_path: f.canonical_path,
            file_event_id: f.file_event_id,
            content_hash: f.content_hash,
            grant_root: f.grant_root,
            writable,
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

/// A recall hit for the webview: the engine's `Hit` plus the hydrated snippet text.
/// `sources` is snake_case (`"vector"` / `"keyword"`) to match the TS `RecallSource` union.
#[derive(Serialize)]
pub struct HitDto {
    pub event_id: String,
    pub score: f32,
    pub kind: String,
    pub sources: Vec<String>,
    pub text: String,
}
impl From<crate::engine::HitWithText> for HitDto {
    fn from(h: crate::engine::HitWithText) -> Self {
        HitDto {
            event_id: h.hit.event_id,
            score: h.hit.score,
            kind: h.hit.kind,
            text: h.text,
            sources: h
                .hit
                .sources
                .iter()
                .map(|s| match s {
                    bossclaw_core::RecallSource::Vector => "vector".to_string(),
                    bossclaw_core::RecallSource::Keyword => "keyword".to_string(),
                })
                .collect(),
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
pub async fn engine_set_folder_writable(path: String, on: bool, state: State<'_, AppState>) -> Result<(), String> {
    let onboarded = state.identity_store.is_onboarded();
    state.engine.set_folder_writable(onboarded, std::path::PathBuf::from(path), on).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn engine_list_writable(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let onboarded = state.identity_store.is_onboarded();
    state.engine.list_writable(onboarded).await.map_err(|e| e.to_string())
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
    let writable_roots: std::collections::HashSet<String> =
        state.engine.list_writable(onboarded).await.map_err(|e| e.to_string())?.into_iter().collect();
    Ok(files.into_iter().map(|f| FileRecordDto::from_parts(f, &writable_roots)).collect())
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

/// Picks a single file (the mandate TARGET — a file, not a folder). Mirrors
/// `engine_pick_folder` exactly but calls `pick_file`; a cancelled dialog (or a dropped sender
/// when the window closes) yields `None`.
#[tauri::command]
pub async fn engine_pick_file(app: tauri::AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_file(move |p| {
        let _ = tx.send(p);
    });
    let picked = rx.await.ok().flatten();
    Ok(picked.and_then(|p| p.into_path().ok()).map(|pb| pb.to_string_lossy().into_owned()))
}

/// Hybrid recall over the user's memory. `k` is clamped server-side to a sane range so a
/// webview bug can't request an unbounded result set.
#[tauri::command]
pub async fn engine_recall(
    query: String,
    k: usize,
    state: State<'_, AppState>,
) -> Result<Vec<HitDto>, String> {
    let k = k.clamp(1, 50);
    let onboarded = state.identity_store.is_onboarded();
    state
        .engine
        .recall(onboarded, query, k)
        .await
        .map(|v| v.into_iter().map(HitDto::from).collect())
        .map_err(|e| e.to_string())
}

/// The evolve loop's status for the webview: the engine's live `{enabled, queue_depth}`
/// merged with the handle's session telemetry `{last_tick_ms, error_count, last_error}`
/// (the engine stubs those three to `None/0/None`, so telemetry WINS for them).
#[derive(Serialize)]
pub struct EvolveStatusDto {
    pub enabled: bool,
    pub queue_depth: usize,
    pub last_tick_ms: Option<u128>,
    pub error_count: usize,
    pub last_error: Option<String>,
    /// File-derived (`is_external`) snippet count the most recent CLOUD tick sent off-box, or
    /// `null` until a cloud tick runs this session. Drives the egress banner's disclosure (R4).
    pub last_tainted_snippets: Option<usize>,
}
impl EvolveStatusDto {
    /// Merge the engine status with the handle telemetry: `enabled`/`queue_depth` come from
    /// the engine; `last_tick_ms`/`error_count`/`last_error` come from telemetry (which holds
    /// the real values the engine only stubs).
    pub fn from_parts(s: &bossclaw_core::EvolveStatus, t: &crate::engine::EvolveTelemetry) -> Self {
        EvolveStatusDto {
            enabled: s.enabled,
            queue_depth: s.queue_depth,
            last_tick_ms: t.last_tick_ms,
            error_count: t.error_count,
            last_error: t.last_error.clone(),
            last_tainted_snippets: t.last_tainted_snippets,
        }
    }
}

/// What one evolve tick did (the subset the Memory tab renders). Mirrors the TS twin.
#[derive(Serialize)]
pub struct EvolveReportDto {
    pub entities_minted: usize,
    pub links_emitted: usize,
    pub invalidates_emitted: usize,
    pub pages_emitted: usize,
    pub memories_processed: usize,
}
impl From<bossclaw_core::EvolveReport> for EvolveReportDto {
    fn from(r: bossclaw_core::EvolveReport) -> Self {
        EvolveReportDto {
            entities_minted: r.entities_minted,
            links_emitted: r.links_emitted,
            invalidates_emitted: r.invalidates_emitted,
            pages_emitted: r.pages_emitted,
            memories_processed: r.memories_processed,
        }
    }
}

#[derive(Serialize)]
pub struct ProposalDto {
    pub id: String,
    pub target: String,
    pub op: String,
    pub new_content_hash: String,
    pub rationale: String,
    pub requires_loud_modal: bool,
    pub producer: String,
}
impl From<crate::engine::ProposalSummary> for ProposalDto {
    fn from(p: crate::engine::ProposalSummary) -> Self {
        Self {
            id: p.id, target: p.target, op: p.op,
            new_content_hash: p.new_content_hash, rationale: p.rationale,
            requires_loud_modal: p.requires_loud_modal, producer: p.producer,
        }
    }
}

#[tauri::command]
pub async fn engine_list_proposals(state: State<'_, AppState>) -> Result<Vec<ProposalDto>, String> {
    let onboarded = state.identity_store.is_onboarded();
    let proposals = state.engine.list_proposals(onboarded).await.map_err(|e| e.to_string())?;
    Ok(proposals.into_iter().map(ProposalDto::from).collect())
}

#[derive(Serialize)]
pub struct PreviewDto {
    pub path: String,
    pub folder: String,
    pub rationale: String,
    pub op: String,
    pub old_text: String,
    pub new_text: String,
    pub requires_loud_modal: bool,
    pub taint: String,
}
impl From<crate::engine::PreviewData> for PreviewDto {
    fn from(p: crate::engine::PreviewData) -> Self {
        Self {
            path: p.path, folder: p.folder, rationale: p.rationale, op: p.op,
            old_text: p.old_text, new_text: p.new_text,
            requires_loud_modal: p.requires_loud_modal, taint: p.taint,
        }
    }
}

#[tauri::command]
pub async fn engine_proposal_preview(id: String, state: State<'_, AppState>) -> Result<PreviewDto, String> {
    let onboarded = state.identity_store.is_onboarded();
    let preview = state.engine.proposal_preview(onboarded, id).await.map_err(|e| e.to_string())?;
    Ok(PreviewDto::from(preview))
}

#[derive(Serialize)]
pub struct ApplyResultDto {
    pub file_written_id: String,
}
impl From<crate::engine::ApplyResult> for ApplyResultDto {
    fn from(r: crate::engine::ApplyResult) -> Self {
        Self { file_written_id: r.file_written_id }
    }
}

#[tauri::command]
pub async fn engine_apply_proposal(id: String, acknowledged_loud: bool, state: State<'_, AppState>) -> Result<ApplyResultDto, String> {
    let onboarded = state.identity_store.is_onboarded();
    let result = state.engine.apply_proposal(onboarded, id, acknowledged_loud).await.map_err(|e| e.to_string())?;
    Ok(ApplyResultDto::from(result))
}

#[tauri::command]
pub async fn engine_decline_proposal(id: String, reason: String, state: State<'_, AppState>) -> Result<(), String> {
    let onboarded = state.identity_store.is_onboarded();
    state.engine.decline_proposal(onboarded, id, reason).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn engine_undo_apply(file_written_id: String, state: State<'_, AppState>) -> Result<(), String> {
    let onboarded = state.identity_store.is_onboarded();
    state.engine.undo_apply(onboarded, file_written_id).await.map_err(|e| e.to_string())
}

/// The evolve loop's status. NEVER errors: if the engine isn't open yet (or any op error),
/// report a disabled/empty status — payload-encoded exactly like `engine_status`, so a
/// status poll from the Memory tab can never throw.
#[tauri::command]
pub async fn engine_evolve_status(state: State<'_, AppState>) -> Result<EvolveStatusDto, String> {
    let onboarded = state.identity_store.is_onboarded();
    match state.engine.evolve_status(onboarded).await {
        Ok((s, t)) => Ok(EvolveStatusDto::from_parts(&s, &t)),
        Err(_) => Ok(EvolveStatusDto {
            enabled: false,
            queue_depth: 0,
            last_tick_ms: None,
            error_count: 0,
            last_error: None,
            last_tainted_snippets: None,
        }),
    }
}

/// Flip the sticky evolve off-switch (the Memory tab toggle).
#[tauri::command]
pub async fn engine_set_evolve_enabled(
    enabled: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let onboarded = state.identity_store.is_onboarded();
    state.engine.set_evolve_enabled(onboarded, enabled).await.map_err(|e| e.to_string())
}

/// Flip the sticky proposals off-switch (invoked under the hood on first folder-enable).
#[tauri::command]
pub async fn engine_set_proposals_enabled(enabled: bool, state: State<'_, AppState>) -> Result<(), String> {
    let onboarded = state.identity_store.is_onboarded();
    state.engine.set_proposals_enabled(onboarded, enabled).await.map_err(|e| e.to_string())
}

/// Flip the sticky mandates off-switch (SP5 global Mandates on/off). Off by default.
#[tauri::command]
pub async fn engine_set_mandates_enabled(enabled: bool, state: State<'_, AppState>) -> Result<(), String> {
    let onboarded = state.identity_store.is_onboarded();
    state.engine.set_mandates_enabled(onboarded, enabled).await.map_err(|e| e.to_string())
}

/// Read the sticky mandates flag (SF5 — the UI toggle reads this on mount to reflect persisted state).
#[tauri::command]
pub async fn engine_mandates_enabled(state: State<'_, AppState>) -> Result<bool, String> {
    let onboarded = state.identity_store.is_onboarded();
    state.engine.mandates_enabled(onboarded).await.map_err(|e| e.to_string())
}

#[derive(Serialize)]
pub struct MandateDto {
    pub mandate_grant_id: String,
    pub target: String,
    pub source_scope: String,
    pub recipe: String,
    pub granted_at: String,
    pub revoked: bool,
}
impl From<crate::engine::MandateSummary> for MandateDto {
    fn from(m: crate::engine::MandateSummary) -> Self {
        Self {
            mandate_grant_id: m.mandate_grant_id, target: m.target, source_scope: m.source_scope,
            recipe: m.recipe, granted_at: m.granted_at, revoked: m.revoked,
        }
    }
}

#[tauri::command]
pub async fn engine_add_mandate(target: String, source_scope: String, recipe: String, state: State<'_, AppState>) -> Result<MandateDto, String> {
    let onboarded = state.identity_store.is_onboarded();
    let m = state.engine.add_mandate(onboarded, std::path::PathBuf::from(target),
        std::path::PathBuf::from(source_scope), recipe).await.map_err(|e| e.to_string())?;
    Ok(MandateDto::from(m))
}

#[tauri::command]
pub async fn engine_revoke_mandate(mandate_grant_id: String, state: State<'_, AppState>) -> Result<(), String> {
    let onboarded = state.identity_store.is_onboarded();
    state.engine.revoke_mandate(onboarded, mandate_grant_id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn engine_list_mandates(state: State<'_, AppState>) -> Result<Vec<MandateDto>, String> {
    let onboarded = state.identity_store.is_onboarded();
    let mandates = state.engine.list_mandates(onboarded).await.map_err(|e| e.to_string())?;
    Ok(mandates.into_iter().map(MandateDto::from).collect())
}

#[derive(Serialize)]
pub struct MandateWriteDto {
    pub file_written_id: String,
    pub target: String,
    pub written_at: String,
    pub undone: bool,
}
impl From<crate::engine::MandateWriteSummary> for MandateWriteDto {
    fn from(r: crate::engine::MandateWriteSummary) -> Self {
        Self { file_written_id: r.file_written_id, target: r.target, written_at: r.written_at, undone: r.undone }
    }
}

#[tauri::command]
pub async fn engine_mandate_writes(state: State<'_, AppState>) -> Result<Vec<MandateWriteDto>, String> {
    let onboarded = state.identity_store.is_onboarded();
    let writes = state.engine.mandate_writes(onboarded).await.map_err(|e| e.to_string())?;
    Ok(writes.into_iter().map(MandateWriteDto::from).collect())
}

/// Run one evolve tick now ("Evolve now"). Returns the tick report.
#[tauri::command]
pub async fn engine_evolve_now(state: State<'_, AppState>) -> Result<EvolveReportDto, String> {
    let onboarded = state.identity_store.is_onboarded();
    state
        .engine
        .evolve_once(onboarded)
        .await
        .map(EvolveReportDto::from)
        .map_err(|e| e.to_string())
}

/// Whether the LOCAL Ollama is reachable and the evolve model is pulled, plus the model tag
/// the Memory tab shows in its "install Ollama and `ollama pull …`" hint. `model_tag` is
/// always `REASONER_MODEL_ID` (the single source of truth), independent of the probe result.
#[derive(Serialize)]
pub struct OllamaStatusDto {
    pub reachable: bool,
    pub model_present: bool,
    pub model_tag: String,
}

/// Probe the local Ollama for the evolve loop. NEVER errors (payload-encoded exactly like
/// `engine_status`): the probe maps any connect/timeout/parse failure to `reachable:false`,
/// so the Memory tab can poll this without a throw. No `AppState` / no onboarding gate — the
/// probe touches a fixed loopback host, never the engine.
#[tauri::command]
pub async fn engine_ollama_status() -> Result<OllamaStatusDto, String> {
    let model_tag = crate::engine::reason::REASONER_MODEL_ID;
    let status = crate::engine::ollama_probe::probe(model_tag).await;
    Ok(OllamaStatusDto {
        reachable: status.reachable,
        model_present: status.model_present,
        model_tag: model_tag.to_string(),
    })
}

// ---- Cloud reasoner config (Milestone D Phase 2a, spec §3.5 / R2 / R5) ----

/// Cheap, NETWORK-FREE pre-validation run at config-set time. The authoritative IP screening
/// happens at connect time inside the hardened client's `PinnedResolver` (spec R2), so this does
/// NO DNS lookup — it only rejects a config that can never be safe regardless of resolution:
/// a non-HTTPS or literal-IP `base_url`. Returns a short human message on any violation.
/// - Local mode (or any non-"cloud" mode): always Ok (local needs no URL).
/// - Cloud + `anthropic`: base_url is ignored (host is pinned), so nothing to validate.
/// - Cloud + `openai-compat`: base_url REQUIRED, must be HTTPS (via `normalize_https_base`),
///   and must NOT be a literal IP host (spec R2: "reject literal-IP base_urls outright").
pub(crate) fn validate_reasoner_config(config: &serde_json::Value) -> Result<(), String> {
    // Anything that isn't an explicit "cloud" mode is Local — no validation needed.
    if config.get("mode").and_then(|v| v.as_str()) != Some("cloud") {
        return Ok(());
    }
    // Only openai-compat carries a user-supplied base_url; anthropic's host is pinned.
    if config.get("provider").and_then(|v| v.as_str()) != Some("openai-compat") {
        return Ok(());
    }
    let base_url = config
        .get("base_url")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| "Cloud openai-compat reasoner requires a base URL.".to_string())?;
    // Enforce HTTPS + normalize via the SAME helper the streaming path uses (single source of
    // truth for the HTTPS rule). A non-HTTPS or unparseable URL fails here.
    let normalized = crate::llm_stream::normalize_https_base(base_url)?;
    // Reject a literal-IP host outright (no DNS): a base_url like `https://10.0.0.5` would let
    // the user point the reasoner straight at an internal address, bypassing the intent of the
    // connect-time resolver guard. `host_str()` returns `10.0.0.5` for IPv4 and `[::1]` (WITH
    // brackets) for an IPv6 literal, so strip any surrounding brackets before the `IpAddr` parse —
    // that catches both forms; a real hostname never parses as an `IpAddr`.
    if let Ok(url) = reqwest::Url::parse(&normalized) {
        if let Some(host) = url.host_str() {
            let bare = host.trim_start_matches('[').trim_end_matches(']');
            if bare.parse::<std::net::IpAddr>().is_ok() {
                return Err("Provider base URL must be a hostname, not a literal IP.".to_string());
            }
        }
    }
    Ok(())
}

/// The reasoner config the Settings UI reads/writes: mode + provider + model + optional base_url,
/// plus the live cloud-readiness flag. Serialized to the webview as-is — snake_case keys
/// (`base_url`, not `baseUrl`); the TS `ReasonerConfigDto` twin mirrors these literally (no
/// `#[serde(rename_all)]`). `ready` is `reasoner_ready_or_false` — for a Local config it is the
/// CLOUD gate's `false` (local readiness is the scheduler's Ollama probe, surfaced separately).
#[derive(serde::Serialize)]
pub struct ReasonerConfigDto {
    pub mode: String,        // "local" | "cloud"
    pub provider: String,    // "anthropic" | "openai-compat"
    pub model: String,
    pub base_url: Option<String>,
    pub ready: bool,
}

/// Read the persisted reasoner config + live cloud-readiness for the Settings UI. Fail-SAFE:
/// the engine read defaults to Local on any error, and readiness is fail-closed to false.
#[tauri::command]
pub async fn engine_get_reasoner_config(
    state: State<'_, AppState>,
) -> Result<ReasonerConfigDto, String> {
    let onboarded = state.identity_store.is_onboarded();
    let config = state.engine.reasoner_config_or_default(onboarded).await;
    let ready = state.engine.reasoner_ready_or_false(onboarded).await;
    Ok(ReasonerConfigDto {
        mode: match config.mode {
            crate::engine::reason::ReasonerMode::Local => "local",
            _ => "cloud",
        }
        .into(),
        provider: crate::engine::reason::provider_str(config.provider).into(),
        model: config.model,
        base_url: config.base_url,
        ready,
    })
}

/// Persist the NON-security reasoner config (mode/provider/model/base_url) WITHOUT granting cloud
/// consent — flipping to cloud still requires `engine_enable_cloud_reasoner`'s tested opt-in (R1).
/// Validates network-free first, then writes the signed log, then refreshes the in-memory cell the
/// provider reads so a mode change is picked up without a restart.
#[tauri::command]
pub async fn engine_set_reasoner_config(
    config: serde_json::Value,
    state: State<'_, AppState>,
) -> Result<(), String> {
    validate_reasoner_config(&config)?;
    let onboarded = state.identity_store.is_onboarded();
    // The daemon owns the reasoner-config cell now, so persisting through the client is the whole
    // job — the daemon refreshes the cell its provider closure reads. (The former app-side
    // write-through cache went away with `AppState.reasoner_cfg` in the M1a split.)
    state.engine.set_reasoner_config(onboarded, config).await.map_err(|e| e.to_string())
}

/// The R5 "test-key-on-enable" opt-in to cloud: validate network-free, then the engine proves the
/// provider key works with ONE trivial probe BEFORE signing config + consent (a probe failure
/// writes NOTHING). On success, refresh the in-memory cell so the evolve loop egresses to cloud.
#[tauri::command]
pub async fn engine_enable_cloud_reasoner(
    config: serde_json::Value,
    state: State<'_, AppState>,
) -> Result<(), String> {
    validate_reasoner_config(&config)?;
    let onboarded = state.identity_store.is_onboarded();
    // As with `set_reasoner_config`, the daemon owns the cell the evolve loop reads, so enabling
    // through the client is the whole job (no app-side write-through cell to refresh anymore).
    state.engine.enable_cloud_reasoner(onboarded, config).await.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hit_dto_maps_sources_to_snake_case() {
        let h = bossclaw_core::Hit {
            event_id: "e1".into(),
            score: 0.5,
            sources: vec![bossclaw_core::RecallSource::Vector, bossclaw_core::RecallSource::Keyword],
            kind: "memory".into(),
        };
        let dto = HitDto::from(crate::engine::HitWithText { hit: h, text: "hello".into() });
        assert_eq!(dto.sources, vec!["vector".to_string(), "keyword".to_string()]);
        assert_eq!(dto.text, "hello");
        assert_eq!(dto.kind, "memory");
        assert_eq!(dto.event_id, "e1");
        assert_eq!(dto.score, 0.5);
    }

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

    #[test]
    fn evolve_status_dto_merges_engine_and_telemetry() {
        // The engine's EvolveStatus stubs last_tick_ms/error_count/last_error; the handle's
        // telemetry holds the real values and MUST win. enabled + queue_depth come from the engine.
        let st = bossclaw_core::EvolveStatus {
            queue_depth: 4,
            last_tick_ms: None,
            error_count: 0,
            last_error: None,
            enabled: true,
        };
        let tel = crate::engine::EvolveTelemetry {
            last_tick_ms: Some(120),
            error_count: 2,
            last_error: Some("boom".into()),
            last_tainted_snippets: Some(2),
        };
        let dto = EvolveStatusDto::from_parts(&st, &tel);
        assert_eq!(dto.queue_depth, 4);
        assert!(dto.enabled);
        assert_eq!(dto.last_tick_ms, Some(120), "telemetry wins over the engine stub");
        assert_eq!(dto.error_count, 2);
        assert_eq!(dto.last_error.as_deref(), Some("boom"));
        assert_eq!(dto.last_tainted_snippets, Some(2), "the file-derived egress count passes through");
    }

    #[test]
    fn evolve_report_maps_to_dto() {
        let r = bossclaw_core::EvolveReport {
            entities_minted: 3,
            links_emitted: 2,
            invalidates_emitted: 1,
            pages_emitted: 4,
            memories_processed: 5,
            ..Default::default()
        };
        let dto = EvolveReportDto::from(r);
        assert_eq!(dto.entities_minted, 3);
        assert_eq!(dto.links_emitted, 2);
        assert_eq!(dto.invalidates_emitted, 1);
        assert_eq!(dto.pages_emitted, 4);
        assert_eq!(dto.memories_processed, 5);
    }

    use crate::commands::identity::AppState;
    use crate::secrets::SecretsVault;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use tauri::Manager;

    struct CmdTestVault { store: Mutex<HashMap<String, String>> }
    impl CmdTestVault { fn new() -> Arc<Self> { Arc::new(Self { store: Mutex::new(HashMap::new()) }) } }
    impl SecretsVault for CmdTestVault {
        fn set(&self, k: &str, v: &str) -> Result<(), String> { self.store.lock().unwrap().insert(k.into(), v.into()); Ok(()) }
        fn get(&self, k: &str) -> Result<Option<String>, String> { Ok(self.store.lock().unwrap().get(k).cloned()) }
        fn delete(&self, k: &str) -> Result<(), String> { self.store.lock().unwrap().remove(k); Ok(()) }
    }

    #[test]
    fn engine_undo_apply_binds_camelcase_arg_over_ipc() {
        use crate::air::identity::{IdentityMetadata, IdentityStore};
        use crate::air::types::Did;
        let dir = tempfile::tempdir().unwrap();
        let vault = CmdTestVault::new();
        // Make is_onboarded() true: save a signing key + metadata (the engine then opens).
        // IdentityMetadata has no Default — build it explicitly (Did is a pub-field tuple struct).
        let identity_store = IdentityStore::new(vault.clone(), dir.path().to_path_buf());
        identity_store.save_signing_key(&[7u8; 32]).unwrap();
        identity_store.save_metadata(&IdentityMetadata {
            did: Did("did:wba:AIR-TEST:cmd".to_string()),
            name: "Test".to_string(),
            username: None,
            created_at: "2026-06-23T00:00:00Z".to_string(),
        }).unwrap();
        // AppState carries air_client + inbox too (the desktop's real wiring, main.rs); use the
        // in-repo doubles so the managed state matches the real struct shape. `engine` is now the
        // daemon-backed FACADE over an in-process `DuplexTransport` (its own hermetic engine home) —
        // the command's assertions are unchanged; only how the engine is built is (Task 6).
        let state = AppState {
            air_client: Arc::new(crate::air::MockAirClient::new()),
            identity_store,
            inbox: Arc::new(crate::inbox::manager::InboxManager::new()),
            engine: duplex_engine(&dir),
        };

        // Grant the `main` window permission to invoke `engine_undo_apply`, so the request ROUTES
        // PAST Tauri's ACL (authority.rs) instead of being rejected with "engine_undo_apply not
        // allowed. Plugin not found" BEFORE arg deserialization. Without this, any failure (incl.
        // the routing one) would vacuously pass a negative assertion — the test must actually reach
        // `undo_write` to prove the camelCase→snake_case binding. The invoke URL below
        // (`http://tauri.localhost`) is classified as a Remote origin, so the grant must name that
        // exact origin (a Local grant would not match — Origin::matches in authority.rs).
        let origin: tauri::utils::acl::RemoteUrlPattern =
            "http://tauri.localhost".parse().expect("valid remote url pattern");
        let mut context = tauri::test::mock_context(tauri::test::noop_assets());
        context.runtime_authority_mut().__allow_command(
            "engine_undo_apply".to_string(),
            tauri::utils::acl::ExecutionContext::Remote { url: origin },
        );
        let app = tauri::test::mock_builder()
            .invoke_handler(tauri::generate_handler![engine_undo_apply])
            .build(context)
            .expect("build mock app");
        app.manage(state);
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default()).build().unwrap();

        // Invoke with the camelCase key the TS twin sends.
        let res = tauri::test::get_ipc_response(
            &webview,
            tauri::webview::InvokeRequest {
                cmd: "engine_undo_apply".into(),
                callback: tauri::ipc::CallbackFn(0),
                error: tauri::ipc::CallbackFn(1),
                url: "http://tauri.localhost".parse().unwrap(),
                body: tauri::ipc::InvokeBody::Json(serde_json::json!({ "fileWrittenId": "no-such-id" })),
                headers: Default::default(),
                invoke_key: tauri::test::INVOKE_KEY.to_string(),
            },
        );
        // POSITIVE op-ran assertion: the camelCase `fileWrittenId` bound to the snake_case
        // `file_written_id` param AND `undo_write` actually ran — proven by the fail-closed
        // SIGNATURE it returns for an unknown id (`undo_write fail-closed: no undo record …`,
        // bossclaw-core log.rs). That string can ONLY appear past the arg binding + the op; a
        // routing reject, a missing-key deserialize error, or a wrong key could never produce it.
        let err = res.expect_err("unknown id makes undo_write fail");
        let msg = err.to_string();
        assert!(msg.contains("undo_write fail-closed") && msg.contains("no undo record"),
            "expected the undo_write fail-closed signature (arg bound + op ran), got: {msg}");
    }

    #[test]
    fn engine_add_mandate_binds_camelcase_arg_over_ipc() {
        use crate::air::identity::{IdentityMetadata, IdentityStore};
        use crate::air::types::Did;
        let dir = tempfile::tempdir().unwrap();
        let vault = CmdTestVault::new();
        let identity_store = IdentityStore::new(vault.clone(), dir.path().to_path_buf());
        identity_store.save_signing_key(&[7u8; 32]).unwrap();
        identity_store.save_metadata(&IdentityMetadata {
            did: Did("did:wba:AIR-TEST:cmd".to_string()),
            name: "Test".to_string(),
            username: None,
            created_at: "2026-06-23T00:00:00Z".to_string(),
        }).unwrap();
        let state = AppState {
            air_client: Arc::new(crate::air::MockAirClient::new()),
            identity_store,
            inbox: Arc::new(crate::inbox::manager::InboxManager::new()),
            engine: duplex_engine(&dir),
        };

        // ACL: a mock app has NO capability grant, so without this the request is rejected with
        // "engine_add_mandate not allowed. Plugin not found" BEFORE arg deserialization — and a
        // negative assertion would vacuously pass. The invoke URL `http://tauri.localhost` is a
        // Remote origin, so the grant must name that exact origin.
        let origin: tauri::utils::acl::RemoteUrlPattern =
            "http://tauri.localhost".parse().expect("valid remote url pattern");
        let mut context = tauri::test::mock_context(tauri::test::noop_assets());
        context.runtime_authority_mut().__allow_command(
            "engine_add_mandate".to_string(),
            tauri::utils::acl::ExecutionContext::Remote { url: origin },
        );
        let app = tauri::test::mock_builder()
            .invoke_handler(tauri::generate_handler![engine_add_mandate])
            .build(context)
            .expect("build mock app");
        app.manage(state);
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default()).build().unwrap();

        // A real (existing) but NOT write-granted target dir + a scope dir, so the op REACHES the
        // engine grant guard and rejects with a deterministic signature (proving the op ran).
        let target_dir = tempfile::tempdir().unwrap();
        let scope_dir = tempfile::tempdir().unwrap();
        let target = target_dir.path().join("synced.md");
        std::fs::write(&target, b"x\n").unwrap();

        let res = tauri::test::get_ipc_response(
            &webview,
            tauri::webview::InvokeRequest {
                cmd: "engine_add_mandate".into(),
                callback: tauri::ipc::CallbackFn(0),
                error: tauri::ipc::CallbackFn(1),
                url: "http://tauri.localhost".parse().unwrap(),
                // camelCase `sourceScope` — must bind to the snake_case `source_scope` param.
                body: tauri::ipc::InvokeBody::Json(serde_json::json!({
                    "target": target.to_string_lossy(),
                    "sourceScope": scope_dir.path().to_string_lossy(),
                    "recipe": "r",
                })),
                headers: Default::default(),
                invoke_key: tauri::test::INVOKE_KEY.to_string(),
            },
        );
        // POSITIVE op-ran assertion: the engine grant guard's "target not write-granted" signature
        // can ONLY appear past the arg binding + the op. A routing reject, a missing-key
        // deserialize error, or a wrong key (`source_scope` never bound) could never produce it.
        let err = res.expect_err("an un-write-granted target makes add_mandate reject");
        let msg = err.to_string();
        assert!(msg.contains("not write-granted"),
            "expected the engine grant-guard signature (sourceScope bound + op ran), got: {msg}");
    }

    // ---- Cloud reasoner config commands (Milestone D Phase 2a, Task 12b) ----
    //
    // NETWORK SAFETY (the #1 constraint): every test below stays NETWORK-FREE and never reads a
    // real provider key. Tests 1 & 3 use a LOCAL config (no validation, no vault, no egress). The
    // reject tests feed a CLOUD openai-compat config whose base_url fails `validate_reasoner_config`
    // (non-HTTPS / literal-IP) BEFORE the command reaches the engine — so the vault and network are
    // never touched. NONE invokes `engine_enable_cloud_reasoner` with a config that could reach
    // `CloudReasoner::complete_json` (the R5 no-key path is unit-covered by
    // `cloud_reasoner_missing_key_fails_closed`). A multi-second runtime would mean a real network
    // call leaked in — these all complete near-instantly.

    /// An onboarded test `AppState` whose engine is the daemon-backed facade over an in-process
    /// `DuplexTransport` (hermetic `test_engine`). The app-side `IdentityStore` (in-memory vault)
    /// makes `is_onboarded()` true; the daemon opens its own fresh (default-Local) brain on demand.
    /// Returns the temp dir so the caller keeps it alive (it roots both the identity + daemon homes).
    fn onboarded_reasoner_state() -> (AppState, tempfile::TempDir) {
        use crate::air::identity::{IdentityMetadata, IdentityStore};
        use crate::air::types::Did;
        let dir = tempfile::tempdir().unwrap();
        let vault = CmdTestVault::new();
        let identity_store = IdentityStore::new(vault.clone(), dir.path().to_path_buf());
        identity_store.save_signing_key(&[7u8; 32]).unwrap();
        identity_store.save_metadata(&IdentityMetadata {
            did: Did("did:wba:AIR-TEST:cmd".to_string()),
            name: "Test".to_string(),
            username: None,
            created_at: "2026-06-23T00:00:00Z".to_string(),
        }).unwrap();
        let engine = duplex_engine(&dir);
        let state = AppState {
            air_client: Arc::new(crate::air::MockAirClient::new()),
            identity_store,
            inbox: Arc::new(crate::inbox::manager::InboxManager::new()),
            engine,
        };
        (state, dir)
    }

    /// Build the daemon-backed [`crate::engine::Engine`] facade over an in-process `DuplexTransport`
    /// (Task 6). The transport spins the REAL daemon dispatch on a hermetic `test_engine` rooted at a
    /// fresh subdir of `dir` (kept separate from the app's identity slots there), so command tests
    /// exercise the true request → daemon → response path with UNCHANGED assertions — no socket, no
    /// keychain. `dir` must outlive the returned engine (the caller keeps its `TempDir`).
    fn duplex_engine(dir: &tempfile::TempDir) -> Arc<crate::engine::Engine> {
        use crate::engine::transport::{DuplexTransport, Transport};
        let home = dir.path().join("bossclawd-home");
        std::fs::create_dir_all(&home).expect("daemon home dir");
        let transport: Arc<dyn Transport> = Arc::new(DuplexTransport::new(home));
        Arc::new(crate::engine::Engine::new(transport))
    }

    /// Build a mock app whose `main` window may invoke each of `cmds` (Remote-origin ACL grant, as
    /// the real webview's `http://tauri.localhost` invokes need), managing `state`. Mirrors the
    /// single-command IPC tests above but allows a small set so a test can set-then-get.
    fn reasoner_mock_app(
        state: AppState,
        cmds: &[&str],
    ) -> tauri::App<tauri::test::MockRuntime> {
        let origin: tauri::utils::acl::RemoteUrlPattern =
            "http://tauri.localhost".parse().expect("valid remote url pattern");
        let mut context = tauri::test::mock_context(tauri::test::noop_assets());
        for cmd in cmds {
            context.runtime_authority_mut().__allow_command(
                (*cmd).to_string(),
                tauri::utils::acl::ExecutionContext::Remote { url: origin.clone() },
            );
        }
        let app = tauri::test::mock_builder()
            .invoke_handler(tauri::generate_handler![
                engine_get_reasoner_config,
                engine_set_reasoner_config,
                engine_enable_cloud_reasoner,
            ])
            .build(context)
            .expect("build mock app");
        app.manage(state);
        app
    }

    /// Invoke `cmd` on `webview` with a camelCase JSON `body`, returning the IPC result. The Err
    /// arm is a `serde_json::Value` (Display-able), matching `get_ipc_response`'s signature.
    fn invoke_reasoner(
        webview: &tauri::WebviewWindow<tauri::test::MockRuntime>,
        cmd: &str,
        body: serde_json::Value,
    ) -> Result<tauri::ipc::InvokeResponseBody, serde_json::Value> {
        tauri::test::get_ipc_response(
            webview,
            tauri::webview::InvokeRequest {
                cmd: cmd.into(),
                callback: tauri::ipc::CallbackFn(0),
                error: tauri::ipc::CallbackFn(1),
                url: "http://tauri.localhost".parse().unwrap(),
                body: tauri::ipc::InvokeBody::Json(body),
                headers: Default::default(),
                invoke_key: tauri::test::INVOKE_KEY.to_string(),
            },
        )
    }

    #[test]
    fn engine_set_reasoner_config_binds_camelcase_and_persists() {
        // A LOCAL config: zero network/vault/validation risk. Proves the command binds the
        // `config` arg, writes the signed log, AND `engine_get_reasoner_config` reads it back.
        let (state, _dir) = onboarded_reasoner_state();
        let app = reasoner_mock_app(
            state,
            &["engine_set_reasoner_config", "engine_get_reasoner_config"],
        );
        let webview =
            tauri::WebviewWindowBuilder::new(&app, "main", Default::default()).build().unwrap();

        // Invoke set with the `config` arg (the TS twin's payload key). The inner object uses the
        // snake_case keys of the OPAQUE persisted config schema (`base_url`) — that value is not a
        // Tauri command arg, so it is NOT camelCase-renamed; only the `config` arg name binds.
        let set = invoke_reasoner(
            &webview,
            "engine_set_reasoner_config",
            serde_json::json!({ "config": {
                "mode": "local", "provider": "anthropic", "model": "x", "base_url": null
            }}),
        );
        set.expect("set_reasoner_config (local) succeeds over IPC");

        // get must read the just-written config back as Local.
        let got = invoke_reasoner(&webview, "engine_get_reasoner_config", serde_json::json!({}))
            .expect("get_reasoner_config succeeds over IPC");
        let json = ipc_json(got);
        assert_eq!(json.get("mode").and_then(|v| v.as_str()), Some("local"),
            "the written local config reads back as mode=local, got: {json}");
    }

    #[test]
    fn engine_set_reasoner_config_rejects_invalid_cloud() {
        // A CLOUD openai-compat config with a LITERAL-IP base_url: `validate_reasoner_config`
        // rejects it NETWORK-FREE (no DNS, no vault) before the engine is ever called.
        let (state, _dir) = onboarded_reasoner_state();
        let app = reasoner_mock_app(state, &["engine_set_reasoner_config"]);
        let webview =
            tauri::WebviewWindowBuilder::new(&app, "main", Default::default()).build().unwrap();

        let res = invoke_reasoner(
            &webview,
            "engine_set_reasoner_config",
            serde_json::json!({ "config": {
                "mode": "cloud", "provider": "openai-compat",
                "model": "m", "base_url": "https://10.0.0.5"
            }}),
        );
        let err = res.expect_err("a literal-IP cloud base_url is rejected at validation");
        let msg = err.to_string();
        assert!(msg.contains("literal IP"),
            "expected the literal-IP validation message, got: {msg}");
    }

    #[test]
    fn engine_get_reasoner_config_defaults_local_not_ready() {
        // A fresh engine (no config written): get reports the fail-SAFE Local default. `ready` is
        // the CLOUD gate's false here (local readiness is the scheduler's Ollama probe, surfaced
        // elsewhere), so we only assert mode=local — the durable, network-free contract.
        let (state, _dir) = onboarded_reasoner_state();
        let app = reasoner_mock_app(state, &["engine_get_reasoner_config"]);
        let webview =
            tauri::WebviewWindowBuilder::new(&app, "main", Default::default()).build().unwrap();

        let got = invoke_reasoner(&webview, "engine_get_reasoner_config", serde_json::json!({}))
            .expect("get_reasoner_config succeeds over IPC");
        let json = ipc_json(got);
        assert_eq!(json.get("mode").and_then(|v| v.as_str()), Some("local"),
            "a fresh engine defaults to mode=local, got: {json}");
        assert_eq!(json.get("ready").and_then(|v| v.as_bool()), Some(false),
            "a fresh (un-consented) config is not cloud-ready, got: {json}");
    }

    #[test]
    fn engine_enable_cloud_reasoner_rejects_invalid_config() {
        // Enable with a NON-HTTPS openai-compat base_url: `validate_reasoner_config` rejects it
        // BEFORE the engine's R5 probe, so this NEVER reaches the vault/network. A follow-up get
        // must still report Local (nothing was enabled). This is the only enable IPC test, and it
        // deliberately fails at validation — it can never reach `complete_json`.
        let (state, _dir) = onboarded_reasoner_state();
        let app = reasoner_mock_app(
            state,
            &["engine_enable_cloud_reasoner", "engine_get_reasoner_config"],
        );
        let webview =
            tauri::WebviewWindowBuilder::new(&app, "main", Default::default()).build().unwrap();

        let res = invoke_reasoner(
            &webview,
            "engine_enable_cloud_reasoner",
            serde_json::json!({ "config": {
                "mode": "cloud", "provider": "openai-compat",
                "model": "m", "base_url": "http://x.example"
            }}),
        );
        let err = res.expect_err("a non-HTTPS cloud base_url is rejected before the engine probe");
        assert!(err.to_string().contains("HTTPS"),
            "expected the HTTPS validation message, got: {err}");

        // Nothing was enabled: the config is still the Local default.
        let got = invoke_reasoner(&webview, "engine_get_reasoner_config", serde_json::json!({}))
            .expect("get_reasoner_config succeeds over IPC");
        let json = ipc_json(got);
        assert_eq!(json.get("mode").and_then(|v| v.as_str()), Some("local"),
            "a rejected enable leaves the config Local, got: {json}");
    }

    /// Decode a successful IPC response body to a `serde_json::Value` (the DTO is JSON-serialized).
    fn ipc_json(body: tauri::ipc::InvokeResponseBody) -> serde_json::Value {
        match body {
            tauri::ipc::InvokeResponseBody::Json(s) => {
                serde_json::from_str(&s).expect("IPC body is valid JSON")
            }
            other => panic!("expected a JSON IPC body, got: {other:?}"),
        }
    }

    // ---- validate_reasoner_config unit coverage (pure, NETWORK-FREE) ----

    #[test]
    fn validate_reasoner_config_passes_local_and_anthropic() {
        // Local needs no URL; anthropic's host is pinned so base_url is irrelevant.
        assert!(validate_reasoner_config(&serde_json::json!({ "mode": "local" })).is_ok());
        assert!(validate_reasoner_config(&serde_json::json!({
            "mode": "cloud", "provider": "anthropic", "model": "m"
        })).is_ok());
        // A valid HTTPS hostname for openai-compat passes.
        assert!(validate_reasoner_config(&serde_json::json!({
            "mode": "cloud", "provider": "openai-compat",
            "model": "m", "base_url": "https://api.example.com/v1"
        })).is_ok());
    }

    #[test]
    fn validate_reasoner_config_rejects_bad_openai_compat() {
        // Missing base_url.
        assert!(validate_reasoner_config(&serde_json::json!({
            "mode": "cloud", "provider": "openai-compat", "model": "m"
        })).is_err());
        // Non-HTTPS.
        assert!(validate_reasoner_config(&serde_json::json!({
            "mode": "cloud", "provider": "openai-compat",
            "model": "m", "base_url": "http://api.example.com"
        })).is_err());
        // Literal IPv4 host.
        assert!(validate_reasoner_config(&serde_json::json!({
            "mode": "cloud", "provider": "openai-compat",
            "model": "m", "base_url": "https://10.0.0.5"
        })).is_err());
        // Literal IPv6 host (bracketed).
        assert!(validate_reasoner_config(&serde_json::json!({
            "mode": "cloud", "provider": "openai-compat",
            "model": "m", "base_url": "https://[::1]"
        })).is_err());
    }
}
