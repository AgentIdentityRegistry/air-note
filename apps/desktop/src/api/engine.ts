import { invoke } from "@tauri-apps/api/core";

export type GrantDto = { canonical_root: string; granted_at: string; revoked: boolean };
export type FileRecordDto = { canonical_path: string; file_event_id: string; content_hash: string; grant_root: string; writable: boolean };
export type SkipDto = { path: string; reason: string };
export type IngestReportDto = {
  ingested: number;
  superseded: number;
  deduped: number;
  skipped: SkipDto[];
  failed: SkipDto[];
};
export type RecallSource = "vector" | "keyword";
export type HitDto = { event_id: string; score: number; kind: string; sources: RecallSource[]; text: string };
export type EvolveStatusDto = {
  enabled: boolean;
  queue_depth: number;
  last_tick_ms: number | null;
  error_count: number;
  last_error: string | null;
  last_tainted_snippets: number | null;
};
export type EvolveReportDto = {
  entities_minted: number;
  links_emitted: number;
  invalidates_emitted: number;
  pages_emitted: number;
  memories_processed: number;
};
export type OllamaStatusDto = { reachable: boolean; model_present: boolean; model_tag: string };

/** Opens the native folder picker; resolves to the chosen path, or null if the user cancels. */
export const pickFolder = (): Promise<string | null> => invoke<string | null>("engine_pick_folder");
/** Opens the native file picker; resolves to the chosen file path, or null if the user cancels. */
export const pickFile = (): Promise<string | null> => invoke<string | null>("engine_pick_file");
export const addGrant = (path: string): Promise<void> => invoke<void>("engine_add_grant", { path });
export const revokeGrant = (path: string): Promise<void> => invoke<void>("engine_revoke_grant", { path });
export const listGrants = (): Promise<GrantDto[]> => invoke<GrantDto[]>("engine_list_grants");
export const runIngest = (): Promise<IngestReportDto> => invoke<IngestReportDto>("engine_run_ingest");
export const listFiles = (): Promise<FileRecordDto[]> => invoke<FileRecordDto[]>("engine_list_files");
export const recall = (query: string, k: number): Promise<HitDto[]> =>
  invoke<HitDto[]>("engine_recall", { query, k });
export const evolveStatus = (): Promise<EvolveStatusDto> => invoke<EvolveStatusDto>("engine_evolve_status");
export const setEvolveEnabled = (enabled: boolean): Promise<void> =>
  invoke<void>("engine_set_evolve_enabled", { enabled });
export const evolveNow = (): Promise<EvolveReportDto> => invoke<EvolveReportDto>("engine_evolve_now");
export const ollamaStatus = (): Promise<OllamaStatusDto> => invoke<OllamaStatusDto>("engine_ollama_status");

export type ReasonerMode = "local" | "cloud";
export type CloudProvider = "anthropic" | "openai-compat" | "gemini";
export type ReasonerConfigDto = {
  mode: ReasonerMode;
  provider: CloudProvider;
  model: string;
  base_url: string | null;
  ready: boolean;
};
/** Write payload — snake_case value keys (Tauri does not rename inner object keys). No `ready` (output-only). */
export type ReasonerConfigInput = {
  mode: ReasonerMode;
  provider: CloudProvider;
  model: string;
  base_url: string | null;
};

export const getReasonerConfig = (): Promise<ReasonerConfigDto> =>
  invoke<ReasonerConfigDto>("engine_get_reasoner_config");
/** Persists config (no consent, never makes cloud ready). Used to switch back to Local. */
export const setReasonerConfig = (config: ReasonerConfigInput): Promise<void> =>
  invoke<void>("engine_set_reasoner_config", { config });
/** The R5 enable flow: test-key probe → signs consent → activates cloud. The only path that makes cloud ready. */
export const enableCloudReasoner = (config: ReasonerConfigInput): Promise<void> =>
  invoke<void>("engine_enable_cloud_reasoner", { config });

// ---- Language pack (rung 2 multilingual, U7) ----
/**
 * The multilingual pack's model id — the folder name the daemon serves once the pack is active. The
 * pinned mirror of the Rust downloader's `MULTILINGUAL_MODEL_ID`; the card compares it against
 * `ModelStatusDto.active_model_id` to detect "multilingual active" and passes it to `setActiveModel`
 * on a Failed-migration retry (the files are already on disk).
 */
export const MULTILINGUAL_MODEL_ID = "minishlab/potion-multilingual-128M";
/**
 * The multilingual pack's pinned weights sha256 — the mirror of the Rust downloader's pinned safetensors
 * hash (`PACK_FILES[0].sha256`). Only used to re-enable already-downloaded files on a Failed retry; the
 * daemon re-verifies it against the on-disk bytes (fail-closed), so a drifted mirror fails loud, never
 * corrupts. Keep in sync with `apps/desktop/src-tauri/src/engine/language_pack.rs`.
 */
export const MULTILINGUAL_SAFETENSORS_SHA =
  "14b5eb39cb4ce5666da8ad1f3dc6be4346e9b2d601c073302fa0a31bf7943397";

/**
 * Loaded-vs-intended model state the Settings card polls (payload-encoded, mirrors the daemon's
 * `engine_model_status`). `state` is `"ok"` when the intended model is serving; `"missing"` and
 * `"mismatch"` are the fail-loud re-download states; `"failed"` means a background re-embed
 * migration errored while the old model keeps serving, with the message in `reason`. `active_model_id`
 * is the model id currently SERVED (`MULTILINGUAL_MODEL_ID` once the pack is live, else the English
 * base id), which the card uses to show "Multilingual active" and hide the redundant Enable button.
 * Every `Option<_>` field arrives as `null` when absent — serde emits all fields.
 */
export type ModelStatusDto = {
  state: "ok" | "missing" | "mismatch" | "failed";
  expected: string | null;
  loaded: string | null;
  reason: string | null;
  reindex_done: number | null;
  reindex_total: number | null;
  active_model_id: string | null;
};

/** Download + verify + install the multilingual pack, then enable it (starts the re-index). */
export const downloadLanguagePack = (): Promise<void> =>
  invoke<void>("engine_download_language_pack");
/** Enable an already-downloaded pack (retry / re-download recovery). */
export const setActiveModel = (modelId: string, safetensorsSha: string): Promise<void> =>
  invoke<void>("engine_set_active_model", { modelId, safetensorsSha });
/** Poll the language-pack state + re-index progress. Throws only on a daemon-transport failure. */
export const modelStatus = (): Promise<ModelStatusDto> =>
  invoke<ModelStatusDto>("engine_model_status");

export type ProposalDto = {
  id: string;
  target: string;
  op: string;
  new_content_hash: string;
  rationale: string;
  requires_loud_modal: boolean;
  producer: string;
};
export type PreviewDto = {
  path: string;
  folder: string;
  rationale: string;
  op: string;
  old_text: string;
  new_text: string;
  requires_loud_modal: boolean;
  taint: string;
};
export type ApplyResultDto = { file_written_id: string };

export const setFolderWritable = (path: string, on: boolean): Promise<void> =>
  invoke<void>("engine_set_folder_writable", { path, on });
export const listWritable = (): Promise<string[]> => invoke<string[]>("engine_list_writable");
export const setProposalsEnabled = (enabled: boolean): Promise<void> =>
  invoke<void>("engine_set_proposals_enabled", { enabled });
export const listProposals = (): Promise<ProposalDto[]> => invoke<ProposalDto[]>("engine_list_proposals");
export const proposalPreview = (id: string): Promise<PreviewDto> =>
  invoke<PreviewDto>("engine_proposal_preview", { id });
export const applyProposal = (id: string, acknowledgedLoud: boolean): Promise<ApplyResultDto> =>
  invoke<ApplyResultDto>("engine_apply_proposal", { id, acknowledgedLoud });
export const declineProposal = (id: string, reason: string): Promise<void> =>
  invoke<void>("engine_decline_proposal", { id, reason });
export const undoApply = (fileWrittenId: string): Promise<void> =>
  invoke<void>("engine_undo_apply", { fileWrittenId });

export type MandateDto = {
  mandate_grant_id: string;
  target: string;
  source_scope: string;
  recipe: string;
  granted_at: string;
  revoked: boolean;
};
export type MandateWriteDto = {
  file_written_id: string;
  target: string;
  written_at: string;
  undone: boolean;
};

export const setMandatesEnabled = (enabled: boolean): Promise<void> =>
  invoke<void>("engine_set_mandates_enabled", { enabled });
export const mandatesEnabled = (): Promise<boolean> => invoke<boolean>("engine_mandates_enabled");
export const addMandate = (target: string, sourceScope: string, recipe: string): Promise<MandateDto> =>
  invoke<MandateDto>("engine_add_mandate", { target, sourceScope, recipe });
export const revokeMandate = (mandateGrantId: string): Promise<void> =>
  invoke<void>("engine_revoke_mandate", { mandateGrantId });
export const listMandates = (): Promise<MandateDto[]> => invoke<MandateDto[]>("engine_list_mandates");
export const mandateWrites = (): Promise<MandateWriteDto[]> =>
  invoke<MandateWriteDto[]>("engine_mandate_writes");

// ---- Library (SP3, Plan C — sessions / notes / forget / recall stats) ----
// snake_case field names mirror C1's serde DTOs (the wire contract). i64/u64 timestamps + byte
// counts arrive as `number`; every `Option<_>` arrives as `null` when absent (serde emits all fields).

/** One captured-session Library row. */
export type SessionSummaryDto = {
  session_id: string;
  title: string;
  project: string;
  tool: string;
  started_at: number;
  ended_at: number;
  approx_bytes: number;
};
/** One captured session's detail: its summary + the daemon-rendered Markdown body. */
export type SessionDetailDto = { summary: SessionSummaryDto; markdown: string };
/** One `remember` note. `superseded_by` is the superseding event id once the note has been edited. */
export type NoteDto = { event_id: string; text: string; created_at: number; superseded_by: string | null };
/** One recorded recall miss (a recall that found nothing) for the tuning UI. */
export type RecallMissDto = { query: string; at: number };
/** Recall telemetry: lifetime `total` recalls, how many were `misses`, and the recent misses. */
export type RecallStatsDto = { total: number; misses: number; recent_misses: RecallMissDto[] };

export const listSessions = (): Promise<SessionSummaryDto[]> =>
  invoke<SessionSummaryDto[]>("engine_list_sessions");
/**
 * Read one session's summary + Markdown. An unknown/deleted id rejects with the daemon's bare
 * "session not found or deleted" string — deliberately propagated so the UI can show "already deleted".
 */
export const getSession = (sessionId: string): Promise<SessionDetailDto> =>
  invoke<SessionDetailDto>("engine_get_session", { sessionId });
/** Tombstone a captured session (durable forget). Idempotent — deleting an unknown id resolves Ok. */
export const deleteSession = (sessionId: string): Promise<void> =>
  invoke<void>("engine_delete_session", { sessionId });
export const listNotes = (): Promise<NoteDto[]> => invoke<NoteDto[]>("engine_list_notes");
/** Supersede a note with new text; resolves to the new (superseding) event id. */
export const supersedeNote = (eventId: string, text: string): Promise<string> =>
  invoke<string>("engine_supersede_note", { eventId, text });
export const recallStats = (): Promise<RecallStatsDto> => invoke<RecallStatsDto>("engine_recall_stats");
