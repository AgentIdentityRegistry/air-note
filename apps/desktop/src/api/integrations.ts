import { invoke } from "@tauri-apps/api/core";

/** Mirrors the Rust `ClaudeCodeStatus` (serde snake_case). */
export type ClaudeCodeStatus = "not_found" | "not_connected" | "connected";
export type IntegrationsStatusDto = { claude_code: ClaudeCodeStatus };

export const integrationsStatus = (): Promise<IntegrationsStatusDto> =>
  invoke<IntegrationsStatusDto>("integrations_status");
export const connectClaudeCode = (): Promise<IntegrationsStatusDto> =>
  invoke<IntegrationsStatusDto>("integrations_connect_claude_code");
export const disconnectClaudeCode = (): Promise<IntegrationsStatusDto> =>
  invoke<IntegrationsStatusDto>("integrations_disconnect_claude_code");
