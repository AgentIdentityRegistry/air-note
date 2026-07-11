import { invoke } from "@tauri-apps/api/core";

/**
 * Mirrors the Rust `ClaudeCodeStatus` — an externally-tagged, snake_case serde enum. The two bare
 * strings are unit variants; `Connected` is an object carrying `capture` (hook presence in the
 * config), so the wire shapes are `"not_found"`, `"not_connected"`, `{"connected":{"capture":true}}`.
 * Source of truth: `src-tauri/src/integrations/mod.rs`.
 */
export type ClaudeCodeStatus =
  | "not_found"
  | "not_connected"
  | { connected: { capture: boolean } };

/**
 * Type guard for the object-form `Connected` variant, so callers discriminate the union without ever
 * string-comparing an object (the bare-string variants are primitives — a plain `"connected" in s`
 * would throw on them, hence the `typeof` short-circuit).
 */
export function isConnected(s: ClaudeCodeStatus): s is { connected: { capture: boolean } } {
  return typeof s === "object" && s !== null && "connected" in s;
}

export type IntegrationsStatusDto = { claude_code: ClaudeCodeStatus };

export const integrationsStatus = (): Promise<IntegrationsStatusDto> =>
  invoke<IntegrationsStatusDto>("integrations_status");

/**
 * Connect Claude Code. `captureConsent` sets BOTH engine capture flags (ongoing capture + the
 * one-time ~30-day backfill) — spec §6a. Tauri renames the camelCase key to the Rust `capture_consent`
 * param, exactly like `engine_get_session({ sessionId })`.
 */
export const connectClaudeCode = (captureConsent: boolean): Promise<IntegrationsStatusDto> =>
  invoke<IntegrationsStatusDto>("integrations_connect_claude_code", { captureConsent });

export const disconnectClaudeCode = (): Promise<IntegrationsStatusDto> =>
  invoke<IntegrationsStatusDto>("integrations_disconnect_claude_code");

/** App-side count of `~/.claude/projects` transcripts for the Connect disclosure. Missing dir = 0. */
export const backfillCount = (): Promise<number> =>
  invoke<number>("integrations_backfill_count");

/**
 * The Integrations capture toggle — FORWARD-ONLY (never backfills). Returns the refreshed detect
 * status. Distinct from `captureEnabled()`: this WRITES the engine flag, that READS it.
 */
export const setCaptureEnabled = (enabled: boolean): Promise<IntegrationsStatusDto> =>
  invoke<IntegrationsStatusDto>("integrations_set_capture_enabled", { enabled });

/**
 * The engine's live capture flag — the toggle POSITION (distinct from `Connected { capture }`, which
 * is hook presence in the config). Fails closed to `false` on the Rust side.
 */
export const captureEnabled = (): Promise<boolean> =>
  invoke<boolean>("integrations_capture_enabled");
