import { invoke } from "@tauri-apps/api/core";

/** Provider API keys the user can set from the app (matches the Rust allow-list). */
export type ProviderVaultKey =
  | "openai_compat_api_key"
  | "openai_api_key"
  | "anthropic_api_key"
  | "google_api_key"
  | "brave_api_key"
  | "tavily_api_key";

export const PROVIDER_VAULT_KEYS: ProviderVaultKey[] = [
  "openai_compat_api_key",
  "openai_api_key",
  "anthropic_api_key",
  "google_api_key",
  "brave_api_key",
  "tavily_api_key",
];

/** Save a provider API key into the OS keychain. */
export async function vaultSet(key: ProviderVaultKey, value: string): Promise<void> {
  await invoke("vault_set", { key, value });
}

/** Delete a stored provider API key. */
export async function vaultDelete(key: ProviderVaultKey): Promise<void> {
  await invoke("vault_delete", { key });
}

/**
 * Whether a non-empty value is stored for this key. Returns only a boolean —
 * the secret itself is never read back into the webview.
 */
export async function vaultHas(key: ProviderVaultKey): Promise<boolean> {
  return invoke<boolean>("vault_has", { key });
}
