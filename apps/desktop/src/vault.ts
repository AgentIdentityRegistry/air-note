import { invoke } from "@tauri-apps/api/core";

export type VaultKey =
  | "session_jwt"
  | "openai_compat_api_key"
  | "openai_api_key"
  | "anthropic_api_key"
  | "google_api_key"
  | "brave_api_key"
  | "tavily_api_key";

export type ProviderVaultKey = Exclude<VaultKey, "session_jwt">;
export type VaultWarmProvider = "openai_compat" | "google_gemini" | "anthropic_claude";

export const PROVIDER_VAULT_KEYS: ProviderVaultKey[] = [
  "openai_compat_api_key",
  "openai_api_key",
  "anthropic_api_key",
  "google_api_key",
  "brave_api_key",
  "tavily_api_key"
];

export async function vaultSet(key: VaultKey, value: string): Promise<void> {
  await invoke("vault_set", { key, value });
}

export async function vaultGet(key: VaultKey): Promise<string | null> {
  return invoke<string | null>("vault_get", { key });
}

export async function vaultDelete(key: VaultKey): Promise<void> {
  await invoke("vault_delete", { key });
}

export async function vaultLock(): Promise<void> {
  await invoke("vault_lock");
}

export async function vaultWarmCache(provider?: VaultWarmProvider): Promise<VaultKey[]> {
  return invoke<VaultKey[]>("vault_warm_cache", { provider: provider ?? null });
}
