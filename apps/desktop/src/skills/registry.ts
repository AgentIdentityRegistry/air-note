import { invoke } from "@tauri-apps/api/core";
import type { Manifest } from "./validateManifest";
import { validateManifest } from "./validateManifest";

export type InstalledSkillRecord = {
  id: string;
  version: string;
  installedAt: string;
  channel: string;
  installDir: string;
};

type SkillsListResponse = {
  channel: string;
  skills: Array<{
    id: string;
    path: string;
    featured: boolean;
    manifestJson: string | null;
    skillMd: string | null;
    promptMd: string | null;
    loadError: string | null;
  }>;
  installed: InstalledSkillRecord[];
};

export type VerifiedSkillItem = {
  id: string;
  path: string;
  featured: boolean;
  manifest: Manifest | null;
  manifestRaw: unknown;
  manifestValidationErrors: string[];
  skillMd: string;
  promptMd: string;
  loadError: string | null;
};

export type VerifiedSkillsState = {
  channel: string;
  skills: VerifiedSkillItem[];
  installed: InstalledSkillRecord[];
};

export async function loadVerifiedSkills(): Promise<VerifiedSkillsState> {
  const response = await invoke<SkillsListResponse>("skills_list_verified");

  const skills = response.skills.map<VerifiedSkillItem>((entry) => {
    let manifestRaw: unknown = null;
    let manifest: Manifest | null = null;
    let manifestValidationErrors: string[] = [];

    if (entry.manifestJson) {
      try {
        manifestRaw = JSON.parse(entry.manifestJson) as unknown;
        const validationResult = validateManifest(manifestRaw);
        if (validationResult.ok) {
          manifest = validationResult.data;
        } else {
          manifestValidationErrors = validationResult.errors;
        }
      } catch {
        manifestValidationErrors = ["manifest.json: invalid JSON"];
      }
    } else {
      manifestValidationErrors = ["manifest.json is missing"];
    }

    return {
      id: entry.id,
      path: entry.path,
      featured: entry.featured,
      manifest,
      manifestRaw,
      manifestValidationErrors,
      skillMd: entry.skillMd ?? "",
      promptMd: entry.promptMd ?? "",
      loadError: entry.loadError
    };
  });

  return {
    channel: response.channel,
    skills,
    installed: response.installed
  };
}

export async function installVerifiedSkill(skillId: string): Promise<InstalledSkillRecord> {
  return invoke<InstalledSkillRecord>("skills_install_verified", { skillId });
}
