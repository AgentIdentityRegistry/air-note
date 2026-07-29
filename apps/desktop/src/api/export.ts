import { invoke } from "@tauri-apps/api/core";

/**
 * Export the selected memories as a signed `.airmem` (Rung-5 SP-V1). The backend mints the identity
 * binding on the first export, asks the daemon to build + seal the bundle, and saves it where the
 * owner chooses. Resolves to the saved path, or `null` if the owner cancelled the save dialog.
 *
 * Rejects with the daemon's own refusal text (empty selection, a note that is no longer current, a
 * bundle over the size limit, …) — surface it as-is, never soften it.
 */
export const exportBundle = (
  noteEventIds: string[],
  sessionEventIds: string[],
  ingestEventIds: string[],
  description: string,
): Promise<string | null> =>
  invoke<string | null>("export_bundle", {
    noteEventIds,
    sessionEventIds,
    ingestEventIds,
    description,
  });
