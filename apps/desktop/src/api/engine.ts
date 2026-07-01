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
