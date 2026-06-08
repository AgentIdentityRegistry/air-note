import { invoke } from "@tauri-apps/api/core";

export type StoreFilename =
  | "agents.json"
  | "missions.json"
  | "workspaces.json"
  | "skill_installs.json"
  | "web_policies.json"
  | "file_policies.json"
  | "audit_log.json"
  | "runs.json"
  | "approvals.json"
  | "usage_events.json"
  | "settings.json";

export async function loadJson<T>(filename: StoreFilename, defaultValue: T): Promise<T> {
  const raw = await invoke<string | null>("store_read", { filename });
  if (!raw) {
    return defaultValue;
  }

  try {
    return JSON.parse(raw) as T;
  } catch {
    return defaultValue;
  }
}

export async function saveJson<T>(filename: StoreFilename, data: T): Promise<void> {
  await invoke("store_write", {
    filename,
    contents: JSON.stringify(data, null, 2)
  });
}
