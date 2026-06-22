import { invoke } from "@tauri-apps/api/core";

export type GrantDto = { canonical_root: string; granted_at: string; revoked: boolean };
export type FileRecordDto = { canonical_path: string; file_event_id: string; content_hash: string; grant_root: string };
export type SkipDto = { path: string; reason: string };
export type IngestReportDto = {
  ingested: number;
  superseded: number;
  deduped: number;
  skipped: SkipDto[];
  failed: SkipDto[];
};

export const pickFolder = (): Promise<string | null> => invoke<string | null>("engine_pick_folder");
export const addGrant = (path: string): Promise<void> => invoke<void>("engine_add_grant", { path });
export const revokeGrant = (path: string): Promise<void> => invoke<void>("engine_revoke_grant", { path });
export const listGrants = (): Promise<GrantDto[]> => invoke<GrantDto[]>("engine_list_grants");
export const runIngest = (): Promise<IngestReportDto> => invoke<IngestReportDto>("engine_run_ingest");
export const listFiles = (): Promise<FileRecordDto[]> => invoke<FileRecordDto[]>("engine_list_files");
