// LEGACY — Mar 3 2026 paid-SaaS App.tsx. Reference codex only.
// Do NOT import this from new code.
// New BossClaw v1 frontend starts fresh in App.tsx.

import { FormEvent, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { subscriptionSummarySchema, type SubscriptionSummary } from "@superclaw/shared";
import { loadJson, saveJson } from "./localStore";
import { estimateCost, recordUsage } from "./metering";
import type {
  Agent,
  AgentProvider,
  ApprovalItem,
  AuditEntry,
  ConfigChangeProposal,
  ConfigObjectKind,
  FilePolicy,
  LoggingMode,
  MemoryMode,
  Mission,
  Run,
  RunStatus,
  SkillInstallConfig,
  UndoToken,
  UsageEvent,
  WebPolicy,
  Workspace
} from "./models";
import { renderSimpleMarkdown } from "./skills/markdown";
import {
  installVerifiedSkill,
  loadVerifiedSkills,
  type InstalledSkillRecord,
  type VerifiedSkillItem
} from "./skills/registry";
import { TOOL_REGISTRY } from "./toolRegistry";
import { ChangeCard } from "./components/ChangeCard";
import { Surface } from "./components/ui/Surface";
import { GlowRing } from "./components/ui/GlowRing";
import { StatusBadge } from "./components/ui/StatusBadge";
import { ToggleSwitch } from "./components/ui/ToggleSwitch";
import { FloatingPrimaryButton } from "./components/ui/FloatingPrimaryButton";
import { SettingsSectionCard } from "./components/ui/SettingsSectionCard";
import {
  parseAndValidatePlanText,
  type BossClawPlanV1,
  type PlanStep as PlannerStep
} from "./engine/validatePlan";
import {
  applyConfigChange,
  diffObjects,
  loadAuditLog,
  loadConfigVersions,
  loadLatestConfig,
  rollbackToVersion,
  undoChange
} from "./configVersioning";
import {
  PROVIDER_VAULT_KEYS,
  type ProviderVaultKey,
  vaultDelete,
  vaultGet,
  vaultLock,
  vaultSet,
  vaultWarmCache
} from "./vault";
import {
  extractWebDocument,
  getEffectiveWebLevel,
  isPolicyPathAllowed,
  parseWebExtractInput,
  usageTagLevel,
  type WebExtractLevel
} from "./webExtract";
import { webExtract } from "./engine/tools/webExtract";
import {
  fileReadTool,
  fileWriteTool,
  normalizeFolderPath,
  parseReadCommand,
  parseWriteCommand
} from "./engine/tools/fileTools";
import {
  DESKTOP_THEMES,
  isDesktopThemeKey,
  type DesktopThemeKey
} from "./design/themes";
import {
  buildMissionSchedule,
  computeNextRunAt,
  formatMissionNextRun,
  formatMissionSchedule,
  isMissionDue,
  needsMissionNextRunRepair,
  normalizeMissionInterval,
  normalizeMissionTime,
  normalizeMissionWeekday,
  WEEKDAY_OPTIONS
} from "./missions";

const DEFAULT_API_BASE = "https://bossclaw-api.onrender.com";

function normalizeApiBase(value: unknown): string {
  if (typeof value !== "string") {
    return DEFAULT_API_BASE;
  }

  const trimmed = value.trim();
  if (!trimmed) {
    return DEFAULT_API_BASE;
  }

  try {
    const parsed = new URL(trimmed);
    if (parsed.protocol !== "https:") {
      return DEFAULT_API_BASE;
    }
    return `${parsed.protocol}//${parsed.host}`;
  } catch {
    return DEFAULT_API_BASE;
  }
}

const API_BASE = normalizeApiBase(import.meta.env.VITE_API_URL);
const WEB_URL = import.meta.env.VITE_WEB_URL ?? "https://bossclaw.ai";
const IS_PRODUCTION = import.meta.env.PROD;
const APP_VERSION = import.meta.env.VITE_APP_VERSION ?? "0.1.0";
const APP_GIT_SHA = import.meta.env.VITE_GIT_SHA ?? "local";

type AppRoute = "/login" | "/app" | "/locked";
type LoginStep = "request" | "verify";
type TabId = "missionControl" | "agents" | "skills" | "settings";
type AgentPanelTab = "chat" | "activity" | "approvals" | "agentSettings";
type SettingsSection = "appearance" | "keys" | "usage" | "changes" | "advanced" | "about";

type SessionCheckResult = {
  ok: boolean;
  active: boolean;
  email: string | null;
  subscription: SubscriptionSummary | null;
};

type UsageSummary = {
  eventCount: number;
  totalTokens: number;
  totalCostUsd: number;
};

type MarkItDownStatus = "not_installed" | "installing" | "ready" | "error";

type MdDetectResponse = {
  pythonFound: boolean;
  markitdownFound: boolean;
  venvPath: string | null;
};

type MdInstallResponse = {
  logs: string;
  venvPath: string;
};

type MdConvertResponse = {
  markdown: string;
  inputBytes: number;
};

type WebFetchResponse = {
  finalUrl: string;
  status: number;
  contentType: string | null;
  html: string;
};

type FileReadResponse = {
  path: string;
  text: string;
  bytes: number;
};

type FileWriteResponse = {
  path: string;
  bytesWritten: number;
};

type PwDetectResponse = {
  nodeFound: boolean;
  nodeVersion: string | null;
  helperInstalled: boolean;
  helperPath: string | null;
};

type PwInstallResponse = {
  logs: string;
  helperPath: string;
};

type PwFetchRenderedResponse = {
  html: string;
};

type AppSettings = {
  openaiCompatBaseUrl: string;
  openaiCompatModelMode: "tier" | "all" | "custom";
  openaiCompatTier: "fast" | "balanced" | "advanced";
  openaiCompatModelId: string;
  openaiCompatModel: string;
  openaiModelMode: "tier" | "all" | "custom";
  openaiTier: "fast" | "balanced" | "advanced";
  openaiModelId: string;
  openaiModel: string;
  anthropicModelMode: "tier" | "all" | "custom";
  anthropicTier: "fast" | "balanced" | "advanced";
  anthropicModelId: string;
  anthropicModel: string;
  googleModelMode: "tier" | "all" | "custom";
  googleTier: "fast" | "balanced" | "advanced";
  googleModelId: string;
  googleModel: string;
  appearance: "system" | "light" | "dark";
  skin: DesktopThemeKey;
  lastActiveAgentId: string | null;
  lastActiveWorkspaceId: string | null;
  missionsPaused: boolean;
};

type ModelProvider = "openai_compat" | "openai" | "anthropic" | "google";
type ModelTier = "fast" | "balanced" | "advanced";
type ModelMode = "tier" | "all" | "custom";

type ChatRole = "user" | "assistant";
type HandshakeStep = "name" | "tone" | "agent_name";
type MissionPresetKind = "daily" | "weekdays" | "every_minutes" | "weekly";
type ChatMessageKind =
  | "user"
  | "assistant"
  | "handshake_name"
  | "handshake_tone"
  | "handshake_agent_name"
  | "handshake_complete"
  | "mission_update";

type ChatMessage = {
  id: string;
  runId: string;
  agentId: string;
  role: ChatRole;
  content: string;
  createdAt: string;
  kind?: ChatMessageKind;
  missionId?: string;
  missionRunIds?: string[];
  count?: number;
  lastRunAt?: string;
  lastSnippet?: string;
};

type MissionRunChatPosting = "off" | "summary" | "verbose";

type MissionRunContext = {
  missionId: string;
  missionTitle: string;
  chatPosting: MissionRunChatPosting;
  collapseRepeats: boolean;
};

type LlmUsage = {
  prompt_tokens?: number;
  completion_tokens?: number;
  total_tokens?: number;
};

type LlmStreamChunkPayload = {
  runId: string;
  delta: string;
};

type LlmStreamDonePayload = {
  runId: string;
  cancelled?: boolean;
  usage?: LlmUsage;
  model?: string;
};

type LlmStreamErrorPayload = {
  runId: string;
  message: string;
};

type LlmStreamNoticePayload = {
  runId: string;
  message: string;
  detail?: string;
};

type PlanStepExecutionStatus =
  | "pending"
  | "running"
  | "completed"
  | "failed"
  | "not_implemented";

type PlanStepExecution = {
  index: number;
  title: string;
  tool: string;
  status: PlanStepExecutionStatus;
  note?: string;
};

type PlannedRunState = {
  runId: string;
  agentId: string;
  prompt: string;
  rawPlanText: string;
  plan: BossClawPlanV1 | null;
  planningError: string | null;
  plannerAttempts: number;
  plannerErrors: string[];
  status:
    | "planning"
    | "planned"
    | "executing"
    | "executing_direct"
    | "waiting_for_approval"
    | "completed"
    | "failed"
    | "cancelled";
  stepStates: PlanStepExecution[];
  configProposals: ConfigChangeProposal[];
  autoRunEligible: boolean;
  runRequested: boolean;
};

type StreamWaiterResult = {
  cancelled: boolean;
  usage?: LlmUsage;
  model?: string;
};

type PendingWebApproval = {
  runId: string;
  stepIndex: number;
  proposalId: string;
};

type PendingQuickExtractApproval = {
  runId: string;
  agentId: string;
  url: string;
  proposal: ConfigChangeProposal;
};

type QuickFileOperation =
  | {
      kind: "read";
      path: string;
    }
  | {
      kind: "write";
      path: string;
      text: string;
      createIfMissing?: boolean;
      allowOverwrite?: boolean;
    };

type PendingQuickFileApproval = {
  runId: string;
  agentId: string;
  operation: QuickFileOperation;
  proposal: ConfigChangeProposal;
};

type ExecuteWebExtractResult =
  | {
      ok: true;
      host: string;
      level: WebExtractLevel;
      title?: string;
      text: string;
      markdown?: string;
      statusNote: string;
    }
  | {
      ok: false;
      reason: "missing_policy" | "invalid_input" | "fetch_failed" | "path_blocked";
      host?: string;
      requestedLevel?: WebExtractLevel;
      message: string;
    };

const RAIL_NAV_ITEMS: Array<{ id: TabId; label: string }> = [
  { id: "agents", label: "Chat" },
  { id: "skills", label: "Skills" },
  { id: "settings", label: "Settings" }
];

const DEFAULT_APP_SETTINGS: AppSettings = {
  openaiCompatBaseUrl: "https://api.openai.com",
  openaiCompatModelMode: "tier",
  openaiCompatTier: "balanced",
  openaiCompatModelId: "gpt-5-mini",
  openaiCompatModel: "gpt-5-mini",
  openaiModelMode: "tier",
  openaiTier: "balanced",
  openaiModelId: "gpt-5-mini",
  openaiModel: "gpt-5-mini",
  anthropicModelMode: "tier",
  anthropicTier: "balanced",
  anthropicModelId: "claude-sonnet-4-6",
  anthropicModel: "claude-sonnet-4-6",
  googleModelMode: "tier",
  googleTier: "balanced",
  googleModelId: "gemini-2.5-flash",
  googleModel: "gemini-2.5-flash",
  appearance: "system",
  skin: "instrument_light",
  lastActiveAgentId: null,
  lastActiveWorkspaceId: null,
  missionsPaused: false
};

const MODEL_TIER_DEFAULTS: Record<ModelProvider, Record<ModelTier, string>> = {
  openai_compat: {
    fast: "gpt-5-nano",
    balanced: "gpt-5-mini",
    advanced: "gpt-5.2"
  },
  openai: {
    fast: "gpt-5-nano",
    balanced: "gpt-5-mini",
    advanced: "gpt-5.2"
  },
  anthropic: {
    fast: "claude-haiku-4-5",
    balanced: "claude-sonnet-4-6",
    advanced: "claude-opus-4-6"
  },
  google: {
    fast: "gemini-2.5-flash-lite",
    balanced: "gemini-2.5-flash",
    advanced: "gemini-2.5-pro"
  }
};

const OPENAI_COMPAT_ADVANCED_DEFAULT_MODELS = ["gpt-5.2", "gpt-5-mini", "gpt-5-nano", "gpt-5.2-pro"];
const OPENAI_ADVANCED_DEFAULT_MODELS = ["gpt-5.2", "gpt-5-mini", "gpt-5-nano", "gpt-5.2-pro"];
const ANTHROPIC_ADVANCED_DEFAULT_MODELS = [
  "claude-opus-4-6",
  "claude-sonnet-4-6",
  "claude-haiku-4-5"
];
const GOOGLE_ADVANCED_DEFAULT_MODELS = [
  "gemini-2.5-pro",
  "gemini-2.5-flash",
  "gemini-2.5-flash-lite"
];

const MODEL_TIER_OPTIONS: Array<{ value: ModelTier; label: string }> = [
  { value: "fast", label: "Fast" },
  { value: "balanced", label: "Balanced" },
  { value: "advanced", label: "Advanced" }
];
const CUSTOM_MODEL_OPTION = "__custom__";

const WEB_LEVEL_LABELS: Record<WebExtractLevel, string> = {
  public: "Standard",
  auth: "Signed-in",
  browser: "Interactive"
};
const HANDSHAKE_TASK_PATTERN =
  /\b(do|please|scan|summarize|organize|plan|build|fix|create|run|check|analyze|review|generate|draft|investigate|compare)\b/i;
const AGENT_RENAME_PATTERNS: RegExp[] = [
  /^your name is now (.+)$/i,
  /^your name is (.+)$/i,
  /^call yourself (.+)$/i,
  /^rename yourself to (.+)$/i,
  /^from now on you(?:'|’)re (.+)$/i,
  /^from now on you are (.+)$/i
];
const USER_ADDRESS_PATTERNS: RegExp[] = [
  /^call me (.+)$/i,
  /^you can call me (.+)$/i,
  /^address me as (.+)$/i,
  /^my name is (.+)$/i
];
const SAFE_MAX_STEP_RETRIES = 3;
const DEFAULT_RETRY_BACKOFF_MS = 350;
const MAX_RETRY_BACKOFF_MS = 3_000;
const MISSION_SCHEDULER_INTERVAL_MS = 30_000;

const PROVIDER_LABELS: Record<ProviderVaultKey, string> = {
  openai_compat_api_key: "OpenAI-Compatible Key",
  openai_api_key: "OpenAI",
  anthropic_api_key: "Anthropic",
  google_api_key: "Google",
  brave_api_key: "Brave Search",
  tavily_api_key: "Tavily Search"
};

const AGENT_PROVIDER_OPTIONS: Array<{
  value: AgentProvider;
  label: string;
}> = [
  { value: "openai_compat", label: "OpenAI-compatible (streaming)" },
  { value: "google_gemini", label: "Google Gemini" },
  { value: "anthropic_claude", label: "Anthropic Claude" }
];

const AGENT_PROVIDER_LABELS: Record<AgentProvider, string> = {
  openai_compat: "OpenAI-compatible (streaming)",
  google_gemini: "Google Gemini",
  anthropic_claude: "Anthropic Claude"
};

const AGENT_PROVIDER_HEADER_LABELS: Record<AgentProvider, string> = {
  openai_compat: "OpenAI",
  google_gemini: "Google Gemini",
  anthropic_claude: "Claude"
};

const EMPTY_VAULT_INPUT: Record<ProviderVaultKey, string> = {
  openai_compat_api_key: "",
  openai_api_key: "",
  anthropic_api_key: "",
  google_api_key: "",
  brave_api_key: "",
  tavily_api_key: ""
};

const EMPTY_VAULT_STATUS: Record<ProviderVaultKey, boolean> = {
  openai_compat_api_key: false,
  openai_api_key: false,
  anthropic_api_key: false,
  google_api_key: false,
  brave_api_key: false,
  tavily_api_key: false
};

const DEFAULT_SKILL_PERMISSIONS = {
  network: {
    default: "deny",
    allowUserAddDomains: false,
    approval: "always_confirm"
  },
  files: {
    default: "deny",
    approval: "once_then_remember"
  },
  clipboard: {
    default: "deny",
    approval: "always_confirm"
  },
  notifications: {
    default: "deny"
  }
} as const;

function normalizeRoute(pathname: string): AppRoute {
  if (pathname === "/login" || pathname === "/app" || pathname === "/locked") {
    return pathname;
  }
  return "/login";
}

function normalizeHostInput(value: string): string | null {
  const trimmed = value.trim().toLowerCase();
  if (!trimmed) {
    return null;
  }

  try {
    const fromUrl = new URL(trimmed.startsWith("http") ? trimmed : `https://${trimmed}`);
    if (!fromUrl.host) {
      return null;
    }
    return fromUrl.host.toLowerCase();
  } catch {
    return null;
  }
}

function apiUrl(path: string): string {
  const base = API_BASE.replace(/\/+$/, "");
  const p = path.startsWith("/") ? path : `/${path}`;
  return `${base}${p}`;
}

type AuthResponseResult =
  | {
      ok: true;
      devCode?: string;
      token?: string;
      email?: string;
    }
  | {
      ok: false;
      error: string;
      status?: number;
      requestUrl: string;
      cannotReach?: boolean;
    };

let rustApiBaseConfigured = false;

function extractStatus(message: string): number | undefined {
  const match = message.match(/HTTP\s+(\d{3})/i);
  if (!match) {
    return undefined;
  }

  const parsed = Number(match[1]);
  return Number.isInteger(parsed) ? parsed : undefined;
}

function invokeErrorMessage(error: unknown, fallback: string): string {
  if (typeof error === "string" && error.trim()) {
    return error;
  }

  if (error instanceof Error && error.message.trim()) {
    return error.message;
  }

  return fallback;
}

async function ensureRustApiBase(): Promise<void> {
  if (rustApiBaseConfigured) {
    return;
  }

  await invoke("api_set_base_url", { baseUrl: API_BASE });
  rustApiBaseConfigured = true;
}

async function callAuthStart(email: string): Promise<AuthResponseResult> {
  const requestUrl = apiUrl("/auth/start");

  try {
    await ensureRustApiBase();
    const payload = (await invoke("api_auth_start", {
      email
    })) as {
      dev_code?: unknown;
    };

    return {
      ok: true,
      devCode: typeof payload.dev_code === "string" ? payload.dev_code : undefined
    };
  } catch (error) {
    const message = invokeErrorMessage(error, `Cannot reach server: ${requestUrl}`);
    const status = extractStatus(message);
    const cannotReach = message.includes("Cannot reach API endpoint");
    return {
      ok: false,
      cannotReach,
      status,
      requestUrl,
      error: cannotReach ? `Cannot reach server: ${requestUrl}` : message
    };
  }
}

async function callAuthVerify(email: string, code: string): Promise<AuthResponseResult> {
  const requestUrl = apiUrl("/auth/verify");

  try {
    await ensureRustApiBase();
    const payload = (await invoke("api_auth_verify", {
      email,
      code
    })) as {
      token?: unknown;
      user?: {
        email?: unknown;
      };
    };

    if (typeof payload.token !== "string") {
      return {
        ok: false,
        requestUrl,
        error: "Invalid email or code."
      };
    }

    return {
      ok: true,
      token: payload.token as string,
      email: typeof payload.user?.email === "string" ? payload.user.email : undefined
    };
  } catch (error) {
    const message = invokeErrorMessage(error, `Cannot reach server: ${requestUrl}`);
    const status = extractStatus(message);
    const cannotReach = message.includes("Cannot reach API endpoint");
    return {
      ok: false,
      cannotReach,
      status,
      requestUrl,
      error: cannotReach ? `Cannot reach server: ${requestUrl}` : message
    };
  }
}

async function checkSession(token: string): Promise<SessionCheckResult> {
  try {
    await ensureRustApiBase();
    const subscriptionPayload = (await invoke("api_me_subscription", {
      token
    })) as unknown;
    const parsedSubscription = subscriptionSummarySchema.safeParse(subscriptionPayload);
    if (!parsedSubscription.success) {
      return { ok: false, active: false, email: null, subscription: null };
    }

    return {
      ok: true,
      active: parsedSubscription.data.active,
      email: null,
      subscription: parsedSubscription.data
    };
  } catch {
    return { ok: false, active: false, email: null, subscription: null };
  }
}

function eventTokenTotal(event: UsageEvent): number {
  if (typeof event.totalTokens === "number") {
    return event.totalTokens;
  }
  return (event.promptTokens ?? 0) + (event.completionTokens ?? 0);
}

function eventCostValue(event: UsageEvent): number {
  return event.estimatedCostUsd ?? 0;
}

function formatUsd(value: number): string {
  return `$${value.toFixed(4)}`;
}

function maskCronListLine(line: string): string {
  if (!/schedule\.cron/i.test(line)) {
    return line;
  }

  const separatorIndex = line.indexOf(":");
  if (separatorIndex < 0) {
    return "schedule.cron: [internal]";
  }
  return `${line.slice(0, separatorIndex + 1)} [internal]`;
}

function maskCronDiffValue(path: string, value: string): string {
  if (path === "schedule.cron") {
    return "[internal]";
  }
  return value;
}

function buildPermissionsDiff(skill: VerifiedSkillItem): string[] {
  if (!skill.manifest) {
    return [];
  }

  const diff: string[] = [];
  const { permissions } = skill.manifest;

  if (permissions.network.default !== DEFAULT_SKILL_PERMISSIONS.network.default) {
    diff.push(
      `Network default: ${DEFAULT_SKILL_PERMISSIONS.network.default} -> ${permissions.network.default}`
    );
  }

  if (permissions.network.allowUserAddDomains !== DEFAULT_SKILL_PERMISSIONS.network.allowUserAddDomains) {
    diff.push(
      `Network domain controls: allowUserAddDomains ${String(DEFAULT_SKILL_PERMISSIONS.network.allowUserAddDomains)} -> ${String(permissions.network.allowUserAddDomains)}`
    );
  }

  if (permissions.network.approval !== DEFAULT_SKILL_PERMISSIONS.network.approval) {
    diff.push(
      `Network approval: ${DEFAULT_SKILL_PERMISSIONS.network.approval} -> ${permissions.network.approval}`
    );
  }

  if (permissions.files.default !== DEFAULT_SKILL_PERMISSIONS.files.default) {
    diff.push(`Files default: ${DEFAULT_SKILL_PERMISSIONS.files.default} -> ${permissions.files.default}`);
  }

  if (permissions.files.approval !== DEFAULT_SKILL_PERMISSIONS.files.approval) {
    diff.push(
      `Files approval: ${DEFAULT_SKILL_PERMISSIONS.files.approval} -> ${permissions.files.approval}`
    );
  }

  if (permissions.clipboard.default !== DEFAULT_SKILL_PERMISSIONS.clipboard.default) {
    diff.push(
      `Clipboard default: ${DEFAULT_SKILL_PERMISSIONS.clipboard.default} -> ${permissions.clipboard.default}`
    );
  }

  if (permissions.clipboard.approval !== DEFAULT_SKILL_PERMISSIONS.clipboard.approval) {
    diff.push(
      `Clipboard approval: ${DEFAULT_SKILL_PERMISSIONS.clipboard.approval} -> ${permissions.clipboard.approval}`
    );
  }

  if (permissions.notifications.default !== DEFAULT_SKILL_PERMISSIONS.notifications.default) {
    diff.push(
      `Notifications default: ${DEFAULT_SKILL_PERMISSIONS.notifications.default} -> ${permissions.notifications.default}`
    );
  }

  return diff;
}

function normalizeAgentProvider(provider: unknown): AgentProvider {
  if (
    provider === "openai_compat" ||
    provider === "google_gemini" ||
    provider === "anthropic_claude"
  ) {
    return provider;
  }
  return "openai_compat";
}

function normalizeLoadedAgent(input: Agent): Agent {
  const loggingModeValue = input.policy.loggingMode as string;
  const normalizedLoggingMode =
    loggingModeValue === "detailed" || loggingModeValue === "verbose" ? "detailed" : "simple";
  const preferredName =
    typeof input.preferredName === "string" && input.preferredName.trim().length
      ? input.preferredName.trim()
      : undefined;
  const tone = input.tone === "concise" || input.tone === "detailed" ? input.tone : undefined;
  const normalizedProvider = normalizeAgentProvider(input.provider);
  const normalizedModelId =
    typeof input.modelId === "string" && input.modelId.trim()
      ? input.modelId.trim()
      : typeof input.openaiCompatModelOverride === "string" && input.openaiCompatModelOverride.trim()
        ? input.openaiCompatModelOverride.trim()
        : undefined;

  return {
    ...input,
    provider: normalizedProvider,
    modelId: normalizedModelId,
    openaiCompatBaseUrlOverride: input.openaiCompatBaseUrlOverride ?? null,
    openaiCompatModelOverride: input.openaiCompatModelOverride ?? null,
    preferredName,
    tone,
    hasAskedName: input.hasAskedName ?? false,
    hasAskedTone: input.hasAskedTone ?? false,
    hasAskedAgentName: input.hasAskedAgentName ?? false,
    policy: {
      ...input.policy,
      loggingMode: normalizedLoggingMode
    }
  };
}

function modelProviderForAgent(provider: AgentProvider): ModelProvider {
  if (provider === "google_gemini") {
    return "google";
  }
  if (provider === "anthropic_claude") {
    return "anthropic";
  }
  return "openai_compat";
}

function normalizedOpenAiCompatBase(value: string | null | undefined): string | null {
  if (!value || value.trim().length === 0) {
    return null;
  }

  try {
    const parsed = new URL(value.trim());
    if (parsed.protocol !== "https:") {
      return null;
    }
    return parsed.toString().replace(/\/+$/, "");
  } catch {
    return null;
  }
}

function normalizeModelTier(value: unknown): ModelTier {
  return value === "fast" || value === "balanced" || value === "advanced" ? value : "balanced";
}

function normalizeModelMode(value: unknown): ModelMode {
  return value === "tier" || value === "all" || value === "custom" ? value : "tier";
}

function resolveModel(provider: ModelProvider, settings: AppSettings): string {
  if (provider === "openai_compat") {
    if (settings.openaiCompatModelMode === "tier") {
      return MODEL_TIER_DEFAULTS.openai_compat[settings.openaiCompatTier];
    }
    const chosen = settings.openaiCompatModelId.trim();
    return chosen || MODEL_TIER_DEFAULTS.openai_compat.balanced;
  }

  if (provider === "openai") {
    if (settings.openaiModelMode === "tier") {
      return MODEL_TIER_DEFAULTS.openai[settings.openaiTier];
    }
    const chosen = settings.openaiModelId.trim();
    return chosen || MODEL_TIER_DEFAULTS.openai.balanced;
  }

  if (provider === "anthropic") {
    if (settings.anthropicModelMode === "tier") {
      return MODEL_TIER_DEFAULTS.anthropic[settings.anthropicTier];
    }
    const chosen = settings.anthropicModelId.trim();
    return chosen || MODEL_TIER_DEFAULTS.anthropic.balanced;
  }

  if (settings.googleModelMode === "tier") {
    return MODEL_TIER_DEFAULTS.google[settings.googleTier];
  }
  const chosen = settings.googleModelId.trim();
  return chosen || MODEL_TIER_DEFAULTS.google.balanced;
}

function effectiveOpenAiCompatModel(agent: Agent | null, settings: AppSettings): string {
  const modelId = agent?.modelId?.trim();
  if (modelId) {
    return modelId;
  }
  const legacyOverride = agent?.openaiCompatModelOverride?.trim();
  if (legacyOverride) {
    return legacyOverride;
  }
  return resolveModel("openai_compat", settings);
}

function providerVaultKeyForAgent(provider: AgentProvider): ProviderVaultKey {
  if (provider === "google_gemini") {
    return "google_api_key";
  }
  if (provider === "anthropic_claude") {
    return "anthropic_api_key";
  }
  return "openai_compat_api_key";
}

function providerMissingKeyMessage(
  provider: AgentProvider,
  keyStatus: Record<ProviderVaultKey, boolean>
): string | null {
  if (provider === "google_gemini" && !keyStatus.google_api_key) {
    return "Google API key not set. Add it in Settings → Keys.";
  }
  if (provider === "anthropic_claude" && !keyStatus.anthropic_api_key) {
    return "Anthropic API key not set. Add it in Settings → Keys.";
  }
  if (provider === "openai_compat" && !keyStatus.openai_compat_api_key) {
    return "OpenAI-compatible API key not set. Add it in Settings → Keys.";
  }
  return null;
}

function isSafePlannerStep(step: PlannerStep): boolean {
  return step.tool === "llm.generate";
}

function estimateStepRisk(step: PlannerStep): "low" | "medium" | "high" {
  if (step.tool === "llm.generate") {
    return "low";
  }

  if (step.tool === "web.extract" || step.tool.includes("file") || step.tool.includes("search")) {
    return "medium";
  }

  return "high";
}

function asRecord(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return {};
  }
  return value as Record<string, unknown>;
}

function agentGlowStateFromStatus(
  status: Run["status"] | null,
  finishedAt: string | null,
  hasPendingApproval: boolean
): "idle" | "planning" | "executing" | "approval" | "error" | "completed" {
  if (status === "failed") {
    return "error";
  }
  if (hasPendingApproval || status === "waiting_for_approval") {
    return "approval";
  }
  if (status === "planning") {
    return "planning";
  }
  if (status === "executing") {
    return "executing";
  }
  if (status === "completed" && finishedAt) {
    const deltaMs = Date.now() - new Date(finishedAt).getTime();
    if (!Number.isNaN(deltaMs) && deltaMs <= 8_000) {
      return "completed";
    }
  }
  return "idle";
}

function normalizeRecoveredRuns(loadedRuns: Run[]): Run[] {
  const nowIso = new Date().toISOString();
  return loadedRuns.map((run) => {
    if (run.status === "executing" || run.status === ("running" as RunStatus)) {
      return {
        ...run,
        status: "failed",
        finishedAt: run.finishedAt ?? nowIso,
        summary: "app_closed_during_execution",
        logs: run.logs.concat("Run marked failed: app_closed_during_execution")
      };
    }

    if (run.status === ("waiting_approval" as RunStatus)) {
      return {
        ...run,
        status: "waiting_for_approval"
      };
    }

    return run;
  });
}

function getStepRetryConfig(step: PlannerStep): { retries: number; backoffMs: number } {
  const requested = Number.isFinite(step.retry?.maxAttempts)
    ? Math.max(0, Math.floor(step.retry?.maxAttempts ?? 0))
    : 0;
  const retries = Math.min(requested, SAFE_MAX_STEP_RETRIES);
  const requestedBackoff = Number.isFinite(step.retry?.backoffMs)
    ? Math.max(100, Math.floor(step.retry?.backoffMs ?? DEFAULT_RETRY_BACKOFF_MS))
    : DEFAULT_RETRY_BACKOFF_MS;
  return {
    retries,
    backoffMs: Math.min(requestedBackoff, MAX_RETRY_BACKOFF_MS)
  };
}

async function sleep(ms: number): Promise<void> {
  await new Promise((resolve) => {
    window.setTimeout(resolve, ms);
  });
}

function looksLikeTaskMessage(value: string): boolean {
  const trimmed = value.trim();
  if (!trimmed) {
    return false;
  }

  if (trimmed.length > 40 || trimmed.includes("\n")) {
    return true;
  }

  return HANDSHAKE_TASK_PATTERN.test(trimmed);
}

function looksLikeNameInput(value: string): boolean {
  const trimmed = value.trim();
  if (!trimmed || trimmed.length > 40 || trimmed.includes("\n")) {
    return false;
  }
  return !looksLikeTaskMessage(trimmed);
}

function normalizeAgentNameCandidate(value: string): string | null {
  const withoutNewLines = value.replace(/\r?\n/g, " ").trim();
  if (!withoutNewLines) {
    return null;
  }

  const strippedQuotes = withoutNewLines.replace(/^["'`]+|["'`]+$/g, "").trim();
  const withoutTrailingNow = strippedQuotes.replace(/\s+now$/i, "").trim();
  const normalizedSpaces = withoutTrailingNow.replace(/\s+/g, " ");
  if (!normalizedSpaces || normalizedSpaces.length > 40) {
    return null;
  }

  return normalizedSpaces;
}

function parseAgentRenameInstruction(value: string): string | null {
  const trimmed = value.trim();
  if (!trimmed) {
    return null;
  }

  for (const pattern of AGENT_RENAME_PATTERNS) {
    const match = trimmed.match(pattern);
    if (!match || typeof match[1] !== "string") {
      continue;
    }

    return normalizeAgentNameCandidate(match[1]);
  }

  return null;
}

function parsePreferredNameInstruction(value: string): string | null {
  const trimmed = value.trim();
  if (!trimmed) {
    return null;
  }

  for (const pattern of USER_ADDRESS_PATTERNS) {
    const match = trimmed.match(pattern);
    if (!match || typeof match[1] !== "string") {
      continue;
    }
    const normalized = normalizeAgentNameCandidate(match[1]);
    if (normalized) {
      return normalized;
    }
  }

  return null;
}

function looksLikeAgentNameInput(value: string): boolean {
  const trimmed = value.trim();
  if (!trimmed || trimmed.length > 40 || trimmed.includes("\n") || trimmed.startsWith("/")) {
    return false;
  }

  return !looksLikeTaskMessage(trimmed);
}

function detectRecurringIntent(value: string): { detected: boolean; strongSignal: boolean } {
  const trimmed = value.trim();
  if (!trimmed) {
    return { detected: false, strongSignal: false };
  }

  const strongSignal = /^\s*\[mission\]/i.test(trimmed);
  const detected =
    strongSignal ||
    /\bevery\b/i.test(trimmed) ||
    /\bdaily\b/i.test(trimmed) ||
    /\bweekly\b/i.test(trimmed) ||
    /\beach minute\b/i.test(trimmed);

  return { detected, strongSignal };
}

function missionScheduleFromProposal(proposal: BossClawPlanV1["missionProposal"]): Mission["schedule"] | null {
  if (!proposal) {
    return null;
  }

  const schedule = proposal.schedule;
  if (schedule.kind === "every_minutes") {
    return buildMissionSchedule({
      kind: "every_minutes",
      intervalMinutes: normalizeMissionInterval(schedule.intervalMinutes)
    });
  }

  if (schedule.kind === "weekly") {
    return buildMissionSchedule({
      kind: "weekly",
      weekday: normalizeMissionWeekday(schedule.weekday),
      time: normalizeMissionTime(schedule.time)
    });
  }

  if (schedule.kind === "weekdays") {
    return buildMissionSchedule({
      kind: "weekdays",
      time: normalizeMissionTime(schedule.time)
    });
  }

  if (schedule.kind === "daily") {
    return buildMissionSchedule({
      kind: "daily",
      time: normalizeMissionTime(schedule.time)
    });
  }

  return null;
}

function parseTimeFromPrompt(prompt: string): string | null {
  const lower = prompt.toLowerCase();
  const amPmMatch = lower.match(/\b(\d{1,2})(?::(\d{2}))?\s*(am|pm)\b/);
  if (amPmMatch) {
    const rawHour = Number(amPmMatch[1]);
    const minute = Number(amPmMatch[2] ?? "0");
    if (rawHour >= 1 && rawHour <= 12 && minute >= 0 && minute <= 59) {
      const convertedHour =
        amPmMatch[3] === "pm" ? (rawHour % 12) + 12 : rawHour % 12;
      return `${String(convertedHour).padStart(2, "0")}:${String(minute).padStart(2, "0")}`;
    }
  }

  const twentyFourHourMatch = lower.match(/\b([01]?\d|2[0-3]):([0-5]\d)\b/);
  if (twentyFourHourMatch) {
    return `${String(Number(twentyFourHourMatch[1])).padStart(2, "0")}:${twentyFourHourMatch[2]}`;
  }

  return null;
}

function inferMissionScheduleFromPrompt(prompt: string): Mission["schedule"] {
  const lower = prompt.toLowerCase();
  const parsedTime = parseTimeFromPrompt(prompt) ?? "09:00";

  const minuteMatch = lower.match(/\bevery\s+(\d+)\s+minutes?\b/);
  if (minuteMatch) {
    return buildMissionSchedule({
      kind: "every_minutes",
      intervalMinutes: normalizeMissionInterval(Number(minuteMatch[1]))
    });
  }

  if (/\beach minute\b|\bevery minute\b/.test(lower)) {
    return buildMissionSchedule({
      kind: "every_minutes",
      intervalMinutes: 1
    });
  }

  if (/\bweekday|weekdays\b/.test(lower)) {
    return buildMissionSchedule({
      kind: "weekdays",
      time: parsedTime
    });
  }

  if (/\bweekly|every week\b/.test(lower)) {
    const weekdayMap: Array<{ pattern: RegExp; value: number }> = [
      { pattern: /\bsunday\b/, value: 0 },
      { pattern: /\bmonday\b/, value: 1 },
      { pattern: /\btuesday\b/, value: 2 },
      { pattern: /\bwednesday\b/, value: 3 },
      { pattern: /\bthursday\b/, value: 4 },
      { pattern: /\bfriday\b/, value: 5 },
      { pattern: /\bsaturday\b/, value: 6 }
    ];
    const weekday = weekdayMap.find((entry) => entry.pattern.test(lower))?.value ?? 1;
    return buildMissionSchedule({
      kind: "weekly",
      weekday,
      time: parsedTime
    });
  }

  return buildMissionSchedule({
    kind: "daily",
    time: parsedTime
  });
}

function inferMissionTitleFromPrompt(prompt: string): string {
  const cleaned = prompt
    .replace(/^\s*\[mission\]\s*/i, "")
    .replace(/\bevery\b[\s\S]*$/i, "")
    .replace(/\bdaily\b[\s\S]*$/i, "")
    .replace(/\bweekly\b[\s\S]*$/i, "")
    .trim();
  if (cleaned.length >= 3) {
    return cleaned.slice(0, 60);
  }
  return "Recurring Mission";
}

function buildMissionSnippet(value: string, maxLength = 80): string {
  const normalized = value.replace(/\s+/g, " ").trim();
  if (!normalized) {
    return "Completed";
  }
  if (normalized.length <= maxLength) {
    return normalized;
  }
  return `${normalized.slice(0, maxLength - 1)}…`;
}

function normalizeMissionGoalForFingerprint(goal: string): string {
  return goal.trim().toLowerCase().replace(/\s+/g, " ");
}

function missionScheduleSignatureForFingerprint(schedule: Mission["schedule"]): string {
  return [
    schedule.kind,
    schedule.time ?? "",
    String(schedule.weekday ?? ""),
    String(schedule.intervalMinutes ?? "")
  ].join("|");
}

function hashMissionFingerprint(raw: string): string {
  let hash = 0x811c9dc5;
  for (let index = 0; index < raw.length; index += 1) {
    hash ^= raw.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193);
  }
  return (hash >>> 0).toString(16).padStart(8, "0");
}

function buildMissionFingerprint(
  agentId: string,
  schedule: Mission["schedule"],
  goal: string
): string {
  const scheduleSignature = missionScheduleSignatureForFingerprint(schedule);
  const normalizedGoal = normalizeMissionGoalForFingerprint(goal);
  return `msn_${hashMissionFingerprint(`${agentId}|${scheduleSignature}|${normalizedGoal}`)}`;
}

function hasMatchingMissionFingerprint(
  mission: Mission,
  agentId: string,
  schedule: Mission["schedule"],
  goal: string
): boolean {
  if (mission.agentId !== agentId || mission.archived) {
    return false;
  }
  const nextFingerprint = buildMissionFingerprint(agentId, schedule, goal);
  if (mission.fingerprint === nextFingerprint) {
    return true;
  }
  const legacyFingerprint = buildMissionFingerprint(
    mission.agentId,
    mission.schedule,
    mission.goal
  );
  return legacyFingerprint === nextFingerprint;
}

function isMissionScheduleEqual(left: Mission["schedule"], right: Mission["schedule"]): boolean {
  return (
    left.kind === right.kind &&
    (left.time ?? "") === (right.time ?? "") &&
    (left.weekday ?? null) === (right.weekday ?? null) &&
    (left.intervalMinutes ?? null) === (right.intervalMinutes ?? null) &&
    left.cron === right.cron
  );
}

function computeAgentStatus(
  agent: Agent | null,
  runs: Run[],
  missions: Mission[]
): "idle" | "running" | "error" {
  if (!agent) {
    return "idle";
  }

  const nowMs = Date.now();
  const running = runs.some(
    (run) =>
      run.agentId === agent.id &&
      (run.status === "planning" ||
        run.status === "executing" ||
        run.status === "waiting_for_approval")
  );
  if (running) {
    return "running";
  }

  const missionLikelyRunning = missions.some((mission) => {
    if (mission.agentId !== agent.id || !mission.enabled || mission.archived) {
      return false;
    }

    const nextRunMs = new Date(mission.nextRunAt).getTime();
    const lastRunMs = mission.lastRunAt ? new Date(mission.lastRunAt).getTime() : Number.NaN;
    const dueSoon = Number.isFinite(nextRunMs) && nextRunMs - nowMs <= 60_000 && nextRunMs >= nowMs - 120_000;
    const justRan = Number.isFinite(lastRunMs) && nowMs - lastRunMs <= 120_000;
    return dueSoon || justRan;
  });
  if (missionLikelyRunning) {
    return "running";
  }

  const latestRun = runs
    .filter((run) => run.agentId === agent.id)
    .sort((left, right) => new Date(right.startedAt).getTime() - new Date(left.startedAt).getTime())[0];
  if (latestRun?.status === "failed") {
    return "error";
  }

  return "idle";
}

function parseExtractCommand(value: string): string | null {
  const trimmed = value.trim();
  if (!trimmed.toLowerCase().startsWith("/extract")) {
    return null;
  }

  const target = trimmed.slice("/extract".length).trim();
  return target || null;
}

export default function App() {
  const [route, setRoute] = useState<AppRoute>(() => normalizeRoute(window.location.pathname));
  const [tab, setTab] = useState<TabId>("agents");
  const [isRailCollapsed, setIsRailCollapsed] = useState(false);

  const [isBootstrapping, setIsBootstrapping] = useState(true);
  const [isBusy, setIsBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [loginStep, setLoginStep] = useState<LoginStep>("request");
  const [emailInput, setEmailInput] = useState("");
  const [verifyEmail, setVerifyEmail] = useState("");
  const [codeInput, setCodeInput] = useState("");
  const [devCode, setDevCode] = useState<string | null>(null);
  const [showDiagnostics, setShowDiagnostics] = useState(false);
  const [diagnosticsLoading, setDiagnosticsLoading] = useState(false);
  const [diagnosticsResult, setDiagnosticsResult] = useState<string | null>(null);

  const [sessionToken, setSessionToken] = useState<string | null>(null);
  const [sessionEmail, setSessionEmail] = useState<string | null>(null);
  const [subscription, setSubscription] = useState<SubscriptionSummary | null>(null);

  const [agents, setAgents] = useState<Agent[]>([]);
  const [missions, setMissions] = useState<Mission[]>([]);
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [skillInstalls, setSkillInstalls] = useState<SkillInstallConfig[]>([]);
  const [runs, setRuns] = useState<Run[]>([]);
  const [approvals, setApprovals] = useState<ApprovalItem[]>([]);
  const [auditEntries, setAuditEntries] = useState<AuditEntry[]>([]);
  const [usageEvents, setUsageEvents] = useState<UsageEvent[]>([]);
  const [localDataLoaded, setLocalDataLoaded] = useState(false);

  const [selectedAgentId, setSelectedAgentId] = useState<string | null>(null);
  const [selectedRunId, setSelectedRunId] = useState<string | null>(null);
  const [selectedWorkspaceId, setSelectedWorkspaceId] = useState<string | null>(null);
  const [agentPanelAgentId, setAgentPanelAgentId] = useState<string | null>(null);
  const [agentPanelTab, setAgentPanelTab] = useState<AgentPanelTab>("chat");
  const [showMissionApprovals, setShowMissionApprovals] = useState(false);
  const [settingsSection, setSettingsSection] = useState<SettingsSection>("appearance");

  const [showCreateAgentModal, setShowCreateAgentModal] = useState(false);
  const [newAgentName, setNewAgentName] = useState("");
  const [newAgentPurpose, setNewAgentPurpose] = useState("");
  const [newAgentProvider, setNewAgentProvider] = useState<AgentProvider>("openai_compat");
  const [newAgentBaseOverride, setNewAgentBaseOverride] = useState("");
  const [newAgentModelOverride, setNewAgentModelOverride] = useState("");
  const [newAgentMemoryMode, setNewAgentMemoryMode] = useState<MemoryMode>("isolated");
  const [newAgentLoggingMode, setNewAgentLoggingMode] = useState<LoggingMode>("simple");
  const [newAgentTools, setNewAgentTools] = useState<string[]>([]);
  const [showCreateMissionModal, setShowCreateMissionModal] = useState(false);
  const [newMissionTitle, setNewMissionTitle] = useState("");
  const [newMissionGoal, setNewMissionGoal] = useState("");
  const [newMissionPresetKind, setNewMissionPresetKind] = useState<MissionPresetKind>("daily");
  const [newMissionTime, setNewMissionTime] = useState("09:00");
  const [newMissionWeekday, setNewMissionWeekday] = useState<number>(1);
  const [newMissionIntervalMinutes, setNewMissionIntervalMinutes] = useState<number>(60);
  const [openMissionMenuId, setOpenMissionMenuId] = useState<string | null>(null);
  const [showHeaderMissionMenu, setShowHeaderMissionMenu] = useState(false);
  const [missionPendingDelete, setMissionPendingDelete] = useState<Mission | null>(null);

  const [vaultInputs, setVaultInputs] =
    useState<Record<ProviderVaultKey, string>>(EMPTY_VAULT_INPUT);
  const [vaultStatus, setVaultStatus] =
    useState<Record<ProviderVaultKey, boolean>>(EMPTY_VAULT_STATUS);
  const [vaultMessage, setVaultMessage] = useState<string | null>(null);
  const [mdStatus, setMdStatus] = useState<MarkItDownStatus>("not_installed");
  const [mdVenvPath, setMdVenvPath] = useState<string | null>(null);
  const [mdLogs, setMdLogs] = useState("");
  const [mdPreview, setMdPreview] = useState("");
  const [mdSelectedFile, setMdSelectedFile] = useState<string | null>(null);
  const [mdError, setMdError] = useState<string | null>(null);
  const [appSettings, setAppSettings] = useState<AppSettings>(DEFAULT_APP_SETTINGS);
  const [webPolicies, setWebPolicies] = useState<WebPolicy[]>([]);
  const [filePolicies, setFilePolicies] = useState<FilePolicy[]>([]);
  const [fileAccessMessage, setFileAccessMessage] = useState<string | null>(null);
  const [fileAccessError, setFileAccessError] = useState<string | null>(null);
  const [webPolicyHostInput, setWebPolicyHostInput] = useState("");
  const [webPolicyLevelInput, setWebPolicyLevelInput] = useState<WebExtractLevel>("public");
  const [webPolicyPathInput, setWebPolicyPathInput] = useState("");
  const [webAuthInputs, setWebAuthInputs] = useState<Record<string, string>>({});
  const [webAccessMessage, setWebAccessMessage] = useState<string | null>(null);
  const [webAccessError, setWebAccessError] = useState<string | null>(null);
  const [webTestUrl, setWebTestUrl] = useState("https://example.com");
  const [webTestLoading, setWebTestLoading] = useState(false);
  const [webTestResult, setWebTestResult] = useState<string | null>(null);
  const [pwStatus, setPwStatus] = useState<PwDetectResponse | null>(null);
  const [pwLoading, setPwLoading] = useState(false);
  const [pwLogs, setPwLogs] = useState("");
  const [pwTestUrl, setPwTestUrl] = useState("https://example.com");
  const [pwTestResult, setPwTestResult] = useState<string | null>(null);
  const [pendingWebApprovals, setPendingWebApprovals] = useState<PendingWebApproval[]>([]);
  const [pendingQuickExtractApproval, setPendingQuickExtractApproval] =
    useState<PendingQuickExtractApproval | null>(null);
  const [pendingQuickFileApproval, setPendingQuickFileApproval] =
    useState<PendingQuickFileApproval | null>(null);
  const [systemPrefersDark, setSystemPrefersDark] = useState<boolean>(() =>
    window.matchMedia("(prefers-color-scheme: dark)").matches
  );
  const [autonomyMode, setAutonomyMode] = useState<"autopilot" | "fsd">("autopilot");
  const [settingsMessage, setSettingsMessage] = useState<string | null>(null);
  const [lockToastMessage, setLockToastMessage] = useState<string | null>(null);
  const [openAiCompatModelOptions, setOpenAiCompatModelOptions] = useState<string[]>(
    OPENAI_COMPAT_ADVANCED_DEFAULT_MODELS
  );
  const [isRefreshingOpenAiCompatModels, setIsRefreshingOpenAiCompatModels] = useState(false);
  const [openAiCompatModelRefreshError, setOpenAiCompatModelRefreshError] = useState<string | null>(null);
  const [openAiModelOptions, setOpenAiModelOptions] = useState<string[]>(
    OPENAI_ADVANCED_DEFAULT_MODELS
  );
  const [isRefreshingOpenAiModels, setIsRefreshingOpenAiModels] = useState(false);
  const [openAiModelRefreshError, setOpenAiModelRefreshError] = useState<string | null>(null);
  const [skillsChannel, setSkillsChannel] = useState("verified");
  const [verifiedSkills, setVerifiedSkills] = useState<VerifiedSkillItem[]>([]);
  const [installedSkills, setInstalledSkills] = useState<InstalledSkillRecord[]>([]);
  const [skillsSearch, setSkillsSearch] = useState("");
  const [skillsMessage, setSkillsMessage] = useState<string | null>(null);
  const [skillsError, setSkillsError] = useState<string | null>(null);
  const [skillsLoading, setSkillsLoading] = useState(false);
  const [selectedSkillId, setSelectedSkillId] = useState<string | null>(null);
  const [pendingInstallSkillId, setPendingInstallSkillId] = useState<string | null>(null);
  const [isInstallingSkill, setIsInstallingSkill] = useState(false);
  const [historyKindFilter, setHistoryKindFilter] = useState<"all" | ConfigObjectKind>("all");
  const [historyObjectFilter, setHistoryObjectFilter] = useState<string>("all");
  const [expandedAuditEntryId, setExpandedAuditEntryId] = useState<string | null>(null);
  const [chatInput, setChatInput] = useState("");
  const [chatMessages, setChatMessages] = useState<ChatMessage[]>([]);
  const [showJumpToLatest, setShowJumpToLatest] = useState(false);
  const [showPlanDetails, setShowPlanDetails] = useState(false);
  const [isEditingAgentName, setIsEditingAgentName] = useState(false);
  const [agentNameDraft, setAgentNameDraft] = useState("");
  const [pendingHandshakeByAgent, setPendingHandshakeByAgent] = useState<Record<string, HandshakeStep>>({});
  const [agentNameConfirmationByAgent, setAgentNameConfirmationByAgent] = useState<
    Record<string, { name: string; expiresAt: number }>
  >({});
  const [activeChatRunId, setActiveChatRunId] = useState<string | null>(null);
  const [activeNonStreamingRunId, setActiveNonStreamingRunId] = useState<string | null>(null);
  const [chatNotice, setChatNotice] = useState<string | null>(null);
  const [chatError, setChatError] = useState<string | null>(null);
  const [plannedRuns, setPlannedRuns] = useState<Record<string, PlannedRunState>>({});
  const [activePlanRunId, setActivePlanRunId] = useState<string | null>(null);
  const [undoState, setUndoState] = useState<{
    token: UndoToken;
    message: string;
    expiresAt: number;
  } | null>(null);
  const chatMessagesRef = useRef<ChatMessage[]>([]);
  const chatThreadRef = useRef<HTMLDivElement | null>(null);
  const chatBottomAnchorRef = useRef<HTMLDivElement | null>(null);
  const chatNearBottomRef = useRef(true);
  const chatLastMessageSignatureRef = useRef<string>("");
  const streamPinToBottomRef = useRef<Record<string, boolean>>({});
  const chatInputRef = useRef<HTMLInputElement | null>(null);
  const agentNameConfirmationTimeoutsRef = useRef<Record<string, number>>({});
  const plannedRunsRef = useRef<Record<string, PlannedRunState>>({});
  const streamWaitersRef = useRef<
    Record<
      string,
      {
        resolve: (result: StreamWaiterResult) => void;
        reject: (message: string) => void;
      }
    >
  >({});
  const chatRunMetaRef = useRef<
    Record<
      string,
      {
        startedAt: number;
        prompt: string;
        model: string;
        agentId: string;
      }
    >
  >({});
  const runningMissionIdsRef = useRef<Set<string>>(new Set());
  const missionSchedulerBusyRef = useRef(false);
  const runsRef = useRef<Run[]>([]);
  const missionRunContextRef = useRef<Record<string, MissionRunContext>>({});
  const runOutputBufferRef = useRef<Record<string, string>>({});

  const navigate = useCallback((nextRoute: AppRoute, replace = false) => {
    const currentPath = window.location.pathname;
    if (currentPath !== nextRoute) {
      if (replace) {
        window.history.replaceState({}, "", nextRoute);
      } else {
        window.history.pushState({}, "", nextRoute);
      }
    }
    setRoute(nextRoute);
  }, []);

  const shouldShowMissionLiveOutput = useCallback((runId: string): boolean => {
    const missionContext = missionRunContextRef.current[runId];
    if (!missionContext) {
      return true;
    }
    return false;
  }, []);

  const isChatNearBottom = useCallback(() => {
    const container = chatThreadRef.current;
    if (!container) {
      return true;
    }
    const remaining = container.scrollHeight - container.scrollTop - container.clientHeight;
    return remaining <= 120;
  }, []);

  const scrollChatToBottom = useCallback((behavior: ScrollBehavior = "auto") => {
    const container = chatThreadRef.current;
    if (!container) {
      return;
    }
    const anchor = chatBottomAnchorRef.current;
    if (anchor) {
      anchor.scrollIntoView({ behavior, block: "end" });
    } else {
      container.scrollTo({
        top: container.scrollHeight,
        behavior
      });
    }
    chatNearBottomRef.current = true;
    setShowJumpToLatest(false);
  }, []);

  const handleChatThreadScroll = useCallback(() => {
    const nearBottom = isChatNearBottom();
    chatNearBottomRef.current = nearBottom;
    if (nearBottom) {
      setShowJumpToLatest(false);
    }
  }, [isChatNearBottom]);

  const clearChatInputAndFocus = useCallback(() => {
    setChatInput("");
    window.requestAnimationFrame(() => {
      chatInputRef.current?.focus();
    });
  }, []);

  const clearLocalAppState = useCallback(() => {
    setSessionToken(null);
    setSessionEmail(null);
    setSubscription(null);
    setAgents([]);
    setMissions([]);
    setWorkspaces([]);
    setSkillInstalls([]);
    setRuns([]);
    setApprovals([]);
    setAuditEntries([]);
    setUsageEvents([]);
    setSelectedAgentId(null);
    setSelectedRunId(null);
    setSelectedWorkspaceId(null);
    setAgentPanelAgentId(null);
    setAgentPanelTab("chat");
    setShowMissionApprovals(false);
    setSettingsSection("appearance");
    setLocalDataLoaded(false);
    setShowCreateMissionModal(false);
    setNewMissionTitle("");
    setNewMissionGoal("");
    setNewMissionPresetKind("daily");
    setNewMissionTime("09:00");
    setNewMissionWeekday(1);
    setNewMissionIntervalMinutes(60);
    setTab("agents");
    setIsRailCollapsed(false);
    setVaultMessage(null);
    setMdStatus("not_installed");
    setMdVenvPath(null);
    setMdLogs("");
    setMdPreview("");
    setMdSelectedFile(null);
    setMdError(null);
    setAppSettings(DEFAULT_APP_SETTINGS);
    setWebPolicies([]);
    setFilePolicies([]);
    setFileAccessMessage(null);
    setFileAccessError(null);
    setWebPolicyHostInput("");
    setWebPolicyLevelInput("public");
    setWebPolicyPathInput("");
    setWebAuthInputs({});
    setWebAccessMessage(null);
    setWebAccessError(null);
    setWebTestUrl("https://example.com");
    setWebTestLoading(false);
    setWebTestResult(null);
    setPwStatus(null);
    setPwLoading(false);
    setPwLogs("");
    setPwTestUrl("https://example.com");
    setPwTestResult(null);
    setPendingWebApprovals([]);
    setPendingQuickExtractApproval(null);
    setPendingQuickFileApproval(null);
    setAutonomyMode("autopilot");
    setSettingsMessage(null);
    setOpenAiCompatModelOptions(OPENAI_COMPAT_ADVANCED_DEFAULT_MODELS);
    setIsRefreshingOpenAiCompatModels(false);
    setOpenAiCompatModelRefreshError(null);
    setOpenAiModelOptions(OPENAI_ADVANCED_DEFAULT_MODELS);
    setIsRefreshingOpenAiModels(false);
    setOpenAiModelRefreshError(null);
    setSkillsChannel("verified");
    setVerifiedSkills([]);
    setInstalledSkills([]);
    setSkillsSearch("");
    setSkillsMessage(null);
    setSkillsError(null);
    setSkillsLoading(false);
    setSelectedSkillId(null);
    setPendingInstallSkillId(null);
    setIsInstallingSkill(false);
    setHistoryKindFilter("all");
    setHistoryObjectFilter("all");
    setExpandedAuditEntryId(null);
    setChatInput("");
    setChatMessages([]);
    setShowJumpToLatest(false);
    setShowPlanDetails(false);
    setIsEditingAgentName(false);
    setAgentNameDraft("");
    setPendingHandshakeByAgent({});
    setAgentNameConfirmationByAgent({});
    setActiveChatRunId(null);
    setActiveNonStreamingRunId(null);
    setChatNotice(null);
    setChatError(null);
    setPlannedRuns({});
    setActivePlanRunId(null);
    setUndoState(null);
    chatMessagesRef.current = [];
    chatNearBottomRef.current = true;
    chatLastMessageSignatureRef.current = "";
    streamPinToBottomRef.current = {};
    Object.values(agentNameConfirmationTimeoutsRef.current).forEach((timeoutId) =>
      window.clearTimeout(timeoutId)
    );
    agentNameConfirmationTimeoutsRef.current = {};
    plannedRunsRef.current = {};
    streamWaitersRef.current = {};
    chatRunMetaRef.current = {};
    runsRef.current = [];
    missionRunContextRef.current = {};
    runOutputBufferRef.current = {};
    runningMissionIdsRef.current.clear();
    missionSchedulerBusyRef.current = false;
    setShowDiagnostics(false);
    setDiagnosticsLoading(false);
    setDiagnosticsResult(null);
  }, []);

  const refreshVaultStatus = useCallback(async () => {
    const nextStatus: Record<ProviderVaultKey, boolean> = { ...EMPTY_VAULT_STATUS };

    await Promise.all(
      PROVIDER_VAULT_KEYS.map(async (key) => {
        const providerKey = key as ProviderVaultKey;
        const value = await vaultGet(providerKey).catch(() => null);
        nextStatus[providerKey] = Boolean(value);
      })
    );

    setVaultStatus(nextStatus);
  }, []);

  const warmVaultCache = useCallback(async (provider?: AgentProvider | null): Promise<void> => {
    const warmedKeys = await vaultWarmCache(provider ?? undefined).catch(
      () => [] as string[]
    );
    const normalizedProvider = provider ?? null;

    setVaultStatus((previous) => {
      const next = { ...previous };

      for (const key of warmedKeys) {
        if (
          key === "openai_compat_api_key" ||
          key === "openai_api_key" ||
          key === "anthropic_api_key" ||
          key === "google_api_key" ||
          key === "brave_api_key" ||
          key === "tavily_api_key"
        ) {
          next[key] = true;
        }
      }

      if (normalizedProvider) {
        const providerKey = providerVaultKeyForAgent(normalizedProvider);
        next[providerKey] = warmedKeys.includes(providerKey);
      }

      return next;
    });
  }, []);

  const ensureProviderKeyAvailable = useCallback(
    async (provider: AgentProvider): Promise<boolean> => {
      const providerKey = providerVaultKeyForAgent(provider);
      if (vaultStatus[providerKey]) {
        return true;
      }

      const value = await vaultGet(providerKey).catch(() => null);
      const hasValue = Boolean(value && value.trim().length > 0);
      setVaultStatus((previous) => ({
        ...previous,
        [providerKey]: hasValue
      }));
      return hasValue;
    },
    [vaultStatus]
  );

  const loadLocalData = useCallback(async () => {
    const [
      loadedAgents,
      loadedMissions,
      loadedWorkspaces,
      loadedSkillInstalls,
      loadedWebPolicies,
      loadedFilePolicies,
      loadedAuditEntries,
      loadedRuns,
      loadedApprovals,
      loadedUsageEvents,
      loadedSettings
    ] = await Promise.all([
      loadLatestConfig("agent"),
      loadLatestConfig("mission"),
      loadLatestConfig("workspace"),
      loadLatestConfig("skill_install"),
      loadLatestConfig("web_policy"),
      loadLatestConfig("file_policy"),
      loadAuditLog(),
      loadJson<Run[]>("runs.json", []),
      loadJson<ApprovalItem[]>("approvals.json", []),
      loadJson<UsageEvent[]>("usage_events.json", []),
      loadJson<Record<string, unknown>>(
        "settings.json",
        DEFAULT_APP_SETTINGS as unknown as Record<string, unknown>
      )
    ]);

    const resolvedMissions = loadedMissions;
    let resolvedWorkspaces = loadedWorkspaces;
    let resolvedAuditEntries = loadedAuditEntries;

    if (!resolvedWorkspaces.length) {
      const defaultWorkspace: Workspace = {
        id: crypto.randomUUID(),
        name: "Default Workspace",
        path: null,
        createdAt: new Date().toISOString()
      };

      await applyConfigChange(
        {
          id: crypto.randomUUID(),
          ts: new Date().toISOString(),
          object: { kind: "workspace", id: defaultWorkspace.id },
          summary: "Initialize default workspace scaffold",
          diff: diffObjects(null, defaultWorkspace),
          applyMode: "autopilot",
          requiresConfirm: true,
          proposedBy: { type: "user", id: "system" },
          patch: {
            after: defaultWorkspace as unknown as Record<string, unknown>
          }
        },
        { type: "system", id: "bootstrap" }
      );
      resolvedWorkspaces = await loadLatestConfig("workspace");
      resolvedAuditEntries = await loadAuditLog();
    }

    setAgents(loadedAgents.map(normalizeLoadedAgent));
    setMissions(resolvedMissions);
    setWorkspaces(resolvedWorkspaces);
    setSkillInstalls(loadedSkillInstalls);
    setWebPolicies(loadedWebPolicies);
    setFilePolicies(loadedFilePolicies);
    setAuditEntries(resolvedAuditEntries);
    setRuns(normalizeRecoveredRuns(loadedRuns));
    setApprovals(loadedApprovals);
    setUsageEvents(loadedUsageEvents);
    const loadedSettingsRecord = loadedSettings as Record<string, unknown>;
    const persistedLastActiveAgentId =
      typeof loadedSettingsRecord.lastActiveAgentId === "string"
        ? loadedSettingsRecord.lastActiveAgentId
        : null;
    const persistedLastActiveWorkspaceId =
      typeof loadedSettingsRecord.lastActiveWorkspaceId === "string"
        ? loadedSettingsRecord.lastActiveWorkspaceId
        : null;
    const initialAgentId =
      (persistedLastActiveAgentId &&
      loadedAgents.some((agent) => agent.id === persistedLastActiveAgentId)
        ? persistedLastActiveAgentId
        : null) ??
      loadedAgents[0]?.id ??
      null;
    const initialWorkspaceId =
      (persistedLastActiveWorkspaceId &&
      resolvedWorkspaces.some((workspace) => workspace.id === persistedLastActiveWorkspaceId)
        ? persistedLastActiveWorkspaceId
        : null) ??
      resolvedWorkspaces[0]?.id ??
      null;
    const loadedOpenAiCompatBase =
      (typeof loadedSettingsRecord.openaiCompatBaseUrl === "string"
        ? loadedSettingsRecord.openaiCompatBaseUrl
        : typeof loadedSettingsRecord.openai_compat_base_url === "string"
          ? loadedSettingsRecord.openai_compat_base_url
          : undefined) ?? DEFAULT_APP_SETTINGS.openaiCompatBaseUrl;
    const loadedOpenAiCompatModel =
      (typeof loadedSettingsRecord.openaiCompatModel === "string"
        ? loadedSettingsRecord.openaiCompatModel
        : typeof loadedSettingsRecord.openai_compat_model === "string"
          ? loadedSettingsRecord.openai_compat_model
          : undefined) ?? DEFAULT_APP_SETTINGS.openaiCompatModel;
    const loadedOpenAiModel =
      (typeof loadedSettingsRecord.openaiModel === "string"
        ? loadedSettingsRecord.openaiModel
        : typeof loadedSettingsRecord.openai_model === "string"
          ? loadedSettingsRecord.openai_model
          : undefined) ?? DEFAULT_APP_SETTINGS.openaiModel;
    const loadedAnthropicModel =
      (typeof loadedSettingsRecord.anthropicModel === "string"
        ? loadedSettingsRecord.anthropicModel
        : typeof loadedSettingsRecord.anthropic_model === "string"
          ? loadedSettingsRecord.anthropic_model
          : undefined) ?? DEFAULT_APP_SETTINGS.anthropicModel;
    const loadedGoogleModel =
      (typeof loadedSettingsRecord.googleModel === "string"
        ? loadedSettingsRecord.googleModel
        : typeof loadedSettingsRecord.google_model === "string"
          ? loadedSettingsRecord.google_model
          : undefined) ?? DEFAULT_APP_SETTINGS.googleModel;

    const hasOpenAiCompatModelMode =
      typeof loadedSettingsRecord.openaiCompatModelMode === "string" ||
      typeof loadedSettingsRecord.openai_compat_model_mode === "string";
    const loadedOpenAiCompatModelMode = hasOpenAiCompatModelMode
      ? normalizeModelMode(
          (loadedSettingsRecord.openaiCompatModelMode ??
            loadedSettingsRecord.openai_compat_model_mode) as unknown
        )
      : loadedOpenAiCompatModel.trim() &&
          !Object.values(MODEL_TIER_DEFAULTS.openai_compat).includes(loadedOpenAiCompatModel.trim())
        ? "custom"
        : "tier";
    const loadedOpenAiCompatTier = normalizeModelTier(
      (loadedSettingsRecord.openaiCompatTier ?? loadedSettingsRecord.openai_compat_tier) as unknown
    );
    const loadedOpenAiCompatModelId =
      (typeof loadedSettingsRecord.openaiCompatModelId === "string"
        ? loadedSettingsRecord.openaiCompatModelId
        : typeof loadedSettingsRecord.openai_compat_model_id === "string"
          ? loadedSettingsRecord.openai_compat_model_id
          : loadedOpenAiCompatModel) ?? DEFAULT_APP_SETTINGS.openaiCompatModelId;

    const hasOpenAiModelMode =
      typeof loadedSettingsRecord.openaiModelMode === "string" ||
      typeof loadedSettingsRecord.openai_model_mode === "string";
    const loadedOpenAiModelMode = hasOpenAiModelMode
      ? normalizeModelMode(
          (loadedSettingsRecord.openaiModelMode ?? loadedSettingsRecord.openai_model_mode) as unknown
        )
      : loadedOpenAiModel.trim() &&
          !Object.values(MODEL_TIER_DEFAULTS.openai).includes(loadedOpenAiModel.trim())
        ? "custom"
        : "tier";
    const loadedOpenAiTier = normalizeModelTier(
      (loadedSettingsRecord.openaiTier ?? loadedSettingsRecord.openai_tier) as unknown
    );
    const loadedOpenAiModelId =
      (typeof loadedSettingsRecord.openaiModelId === "string"
        ? loadedSettingsRecord.openaiModelId
        : typeof loadedSettingsRecord.openai_model_id === "string"
          ? loadedSettingsRecord.openai_model_id
          : loadedOpenAiModel) ?? DEFAULT_APP_SETTINGS.openaiModelId;

    const hasAnthropicModelMode =
      typeof loadedSettingsRecord.anthropicModelMode === "string" ||
      typeof loadedSettingsRecord.anthropic_model_mode === "string";
    const loadedAnthropicModelMode = hasAnthropicModelMode
      ? normalizeModelMode(
          (loadedSettingsRecord.anthropicModelMode ??
            loadedSettingsRecord.anthropic_model_mode) as unknown
        )
      : loadedAnthropicModel.trim() &&
          !Object.values(MODEL_TIER_DEFAULTS.anthropic).includes(loadedAnthropicModel.trim())
        ? "custom"
        : "tier";
    const loadedAnthropicTier = normalizeModelTier(
      (loadedSettingsRecord.anthropicTier ?? loadedSettingsRecord.anthropic_tier) as unknown
    );
    const loadedAnthropicModelId =
      (typeof loadedSettingsRecord.anthropicModelId === "string"
        ? loadedSettingsRecord.anthropicModelId
        : typeof loadedSettingsRecord.anthropic_model_id === "string"
          ? loadedSettingsRecord.anthropic_model_id
          : loadedAnthropicModel) ?? DEFAULT_APP_SETTINGS.anthropicModelId;

    const hasGoogleModelMode =
      typeof loadedSettingsRecord.googleModelMode === "string" ||
      typeof loadedSettingsRecord.google_model_mode === "string";
    const loadedGoogleModelMode = hasGoogleModelMode
      ? normalizeModelMode(
          (loadedSettingsRecord.googleModelMode ?? loadedSettingsRecord.google_model_mode) as unknown
        )
      : loadedGoogleModel.trim() &&
          !Object.values(MODEL_TIER_DEFAULTS.google).includes(loadedGoogleModel.trim())
        ? "custom"
        : "tier";
    const loadedGoogleTier = normalizeModelTier(
      (loadedSettingsRecord.googleTier ?? loadedSettingsRecord.google_tier) as unknown
    );
    const loadedGoogleModelId =
      (typeof loadedSettingsRecord.googleModelId === "string"
        ? loadedSettingsRecord.googleModelId
        : typeof loadedSettingsRecord.google_model_id === "string"
          ? loadedSettingsRecord.google_model_id
          : loadedGoogleModel) ?? DEFAULT_APP_SETTINGS.googleModelId;
    const resolvedSettings: AppSettings = {
      openaiCompatBaseUrl:
        normalizedOpenAiCompatBase(loadedOpenAiCompatBase) ??
        DEFAULT_APP_SETTINGS.openaiCompatBaseUrl,
      openaiCompatModelMode: loadedOpenAiCompatModelMode,
      openaiCompatTier: loadedOpenAiCompatTier,
      openaiCompatModelId:
        loadedOpenAiCompatModelId.trim() || DEFAULT_APP_SETTINGS.openaiCompatModelId,
      openaiCompatModel: DEFAULT_APP_SETTINGS.openaiCompatModel,
      openaiModelMode: loadedOpenAiModelMode,
      openaiTier: loadedOpenAiTier,
      openaiModelId: loadedOpenAiModelId.trim() || DEFAULT_APP_SETTINGS.openaiModelId,
      openaiModel: DEFAULT_APP_SETTINGS.openaiModel,
      anthropicModelMode: loadedAnthropicModelMode,
      anthropicTier: loadedAnthropicTier,
      anthropicModelId:
        loadedAnthropicModelId.trim() || DEFAULT_APP_SETTINGS.anthropicModelId,
      anthropicModel: DEFAULT_APP_SETTINGS.anthropicModel,
      googleModelMode: loadedGoogleModelMode,
      googleTier: loadedGoogleTier,
      googleModelId: loadedGoogleModelId.trim() || DEFAULT_APP_SETTINGS.googleModelId,
      googleModel: DEFAULT_APP_SETTINGS.googleModel,
      appearance:
        loadedSettingsRecord.appearance === "light" ||
        loadedSettingsRecord.appearance === "dark" ||
        loadedSettingsRecord.appearance === "system"
          ? loadedSettingsRecord.appearance
          : DEFAULT_APP_SETTINGS.appearance,
      skin: isDesktopThemeKey(loadedSettingsRecord.skin)
        ? loadedSettingsRecord.skin
        : DEFAULT_APP_SETTINGS.skin,
      lastActiveAgentId: initialAgentId,
      lastActiveWorkspaceId: initialWorkspaceId,
      missionsPaused:
        typeof loadedSettingsRecord.missionsPaused === "boolean"
          ? loadedSettingsRecord.missionsPaused
          : typeof loadedSettingsRecord.missions_paused === "boolean"
            ? loadedSettingsRecord.missions_paused
            : DEFAULT_APP_SETTINGS.missionsPaused
    };

    resolvedSettings.openaiCompatModel = resolveModel("openai_compat", resolvedSettings);
    resolvedSettings.openaiModel = resolveModel("openai", resolvedSettings);
    resolvedSettings.anthropicModel = resolveModel("anthropic", resolvedSettings);
    resolvedSettings.googleModel = resolveModel("google", resolvedSettings);

    setAppSettings(resolvedSettings);
    setSelectedAgentId(initialAgentId);
    setSelectedWorkspaceId(initialWorkspaceId);
    setAgentPanelAgentId(initialAgentId);
    setAgentPanelTab("chat");
    setTab("agents");
    setSelectedRunId(loadedRuns[0]?.id ?? null);
    setLocalDataLoaded(true);
  }, []);

  const refreshConfigState = useCallback(async () => {
    const [
      latestAgents,
      latestMissions,
      latestWorkspaces,
      latestSkillInstalls,
      latestWebPolicies,
      latestFilePolicies
    ] = await Promise.all([
      loadLatestConfig("agent"),
      loadLatestConfig("mission"),
      loadLatestConfig("workspace"),
      loadLatestConfig("skill_install"),
      loadLatestConfig("web_policy"),
      loadLatestConfig("file_policy")
    ]);

    setAgents(latestAgents.map(normalizeLoadedAgent));
    setMissions(latestMissions);
    setWorkspaces(latestWorkspaces);
    setSkillInstalls(latestSkillInstalls);
    setWebPolicies(latestWebPolicies);
    setFilePolicies(latestFilePolicies);
  }, []);

  const refreshAuditState = useCallback(async () => {
    const latestAudit = await loadAuditLog();
    setAuditEntries(latestAudit);
  }, []);

  const applyProposal = useCallback(
    async (proposal: ConfigChangeProposal, actor: AuditEntry["actor"], undoMessage: string) => {
      const result = await applyConfigChange(proposal, actor);
      await refreshConfigState();
      await refreshAuditState();
      setUndoState({
        token: result.undoToken,
        message: undoMessage,
        expiresAt: Date.now() + 30_000
      });
      return result;
    },
    [refreshAuditState, refreshConfigState]
  );

  const rollbackObjectVersion = useCallback(
    async (kind: ConfigObjectKind, id: string, version: number, summary?: string) => {
      await rollbackToVersion(kind, id, version, { type: "user", id: sessionEmail ?? undefined }, summary);
      await refreshConfigState();
      await refreshAuditState();
    },
    [refreshAuditState, refreshConfigState, sessionEmail]
  );

  const undoRecentChange = useCallback(async () => {
    if (!undoState) {
      return;
    }

    await undoChange(undoState.token, { type: "user", id: sessionEmail ?? undefined });
    await refreshConfigState();
    await refreshAuditState();
    setUndoState(null);
  }, [refreshAuditState, refreshConfigState, sessionEmail, undoState]);

  const buildUsageEvent = useCallback(
    (input: {
      agentId: string | null;
      runId: string | null;
      provider: string;
      model: string | null;
      kind: UsageEvent["kind"];
      promptTokens?: number | null;
      completionTokens?: number | null;
      totalTokens?: number | null;
      inputChars?: number;
      outputChars?: number;
      latencyMs: number;
      tags?: UsageEvent["tags"];
    }): UsageEvent => {
      const promptTokens = input.promptTokens ?? null;
      const completionTokens = input.completionTokens ?? null;
      const totalTokens =
        input.totalTokens ??
        (promptTokens !== null || completionTokens !== null
          ? (promptTokens ?? 0) + (completionTokens ?? 0)
          : null);

      return {
        id: crypto.randomUUID(),
        ts: new Date().toISOString(),
        agentId: input.agentId,
        runId: input.runId,
        provider: input.provider,
        model: input.model,
        kind: input.kind,
        promptTokens,
        completionTokens,
        totalTokens,
        inputChars: input.inputChars ?? 0,
        outputChars: input.outputChars ?? 0,
        latencyMs: Math.max(0, Math.round(input.latencyMs)),
        estimatedCostUsd: estimateCost(input.provider, input.model, promptTokens, completionTokens),
        tags: input.tags ?? {}
      };
    },
    []
  );

  const logUsageEvent = useCallback(async (event: UsageEvent): Promise<void> => {
    try {
      await recordUsage(event);
      setUsageEvents((previous) => [event, ...previous]);
    } catch {
      // Ignore metering storage failures to avoid breaking product flows.
    }
  }, []);

  const detectMarkItDown = useCallback(async () => {
    setMdError(null);

    try {
      const result = await invoke<MdDetectResponse>("md_detect");
      setMdVenvPath(result.venvPath);

      if (!result.pythonFound) {
        setMdStatus("error");
        setMdError("python3 not found. Install Python 3 to use MarkItDown.");
        return;
      }

      if (result.markitdownFound) {
        setMdStatus("ready");
      } else {
        setMdStatus("not_installed");
      }
    } catch {
      setMdStatus("error");
      setMdError("Unable to detect MarkItDown environment.");
    }
  }, []);

  const installMarkItDown = useCallback(async () => {
    setMdError(null);
    setMdStatus("installing");
    const startedAt = performance.now();

    try {
      const result = await invoke<MdInstallResponse>("md_install");
      setMdLogs(result.logs);
      setMdVenvPath(result.venvPath);
      setMdStatus("ready");

      await logUsageEvent(
        buildUsageEvent({
          agentId: null,
          runId: null,
          provider: "markitdown",
          model: null,
          kind: "file",
          inputChars: 0,
          outputChars: result.logs.length,
          latencyMs: performance.now() - startedAt
        })
      );
    } catch (error) {
      const message = error instanceof Error ? error.message : "MarkItDown install failed.";
      setMdStatus("error");
      setMdError(message);

      await logUsageEvent(
        buildUsageEvent({
          agentId: null,
          runId: null,
          provider: "markitdown",
          model: null,
          kind: "file",
          inputChars: 0,
          outputChars: message.length,
          latencyMs: performance.now() - startedAt
        })
      );
    }
  }, [buildUsageEvent, logUsageEvent]);

  const testConvertWithMarkItDown = useCallback(async () => {
    setMdError(null);
    setMdPreview("");

    try {
      const selected = await invoke<string | string[] | null>("plugin:dialog|open", {
        options: {
          title: "Select a document to convert",
          multiple: false,
          directory: false
        }
      });

      const selectedFile = Array.isArray(selected) ? selected[0] : selected;
      if (!selectedFile) {
        return;
      }

      setMdSelectedFile(selectedFile);
      const startedAt = performance.now();
      const result = await invoke<MdConvertResponse>("md_convert", { filePath: selectedFile });
      const preview = result.markdown.slice(0, 2000);
      setMdPreview(preview);

      await logUsageEvent(
        buildUsageEvent({
          agentId: null,
          runId: null,
          provider: "markitdown",
          model: null,
          kind: "file",
          inputChars: Math.round(result.inputBytes),
          outputChars: result.markdown.length,
          latencyMs: performance.now() - startedAt
        })
      );
    } catch (error) {
      setMdError(error instanceof Error ? error.message : "MarkItDown convert failed.");
    }
  }, [buildUsageEvent, logUsageEvent]);

  const applySession = useCallback(
    async (token: string, fallbackEmail?: string): Promise<boolean> => {
      const startedAt = performance.now();
      const sessionResult = await checkSession(token);
      const subscriptionCheckEvent = buildUsageEvent({
        agentId: null,
        runId: null,
        provider: "bossclaw",
        model: null,
        kind: "other",
        inputChars: 0,
        outputChars: sessionResult.subscription ? JSON.stringify(sessionResult.subscription).length : 0,
        latencyMs: performance.now() - startedAt
      });
      await logUsageEvent(subscriptionCheckEvent);

      if (!sessionResult.ok || !sessionResult.subscription) {
        return false;
      }

      setSessionToken(token);
      setSessionEmail(sessionResult.email ?? fallbackEmail ?? null);
      setSubscription(sessionResult.subscription);

      if (sessionResult.active) {
        navigate("/app", true);
      } else {
        navigate("/locked", true);
      }

      return true;
    },
    [buildUsageEvent, logUsageEvent, navigate]
  );

  const logout = useCallback(async () => {
    await vaultDelete("session_jwt").catch(() => undefined);
    await vaultLock().catch(() => undefined);
    clearLocalAppState();
    setLoginStep("request");
    setCodeInput("");
    setDevCode(null);
    navigate("/login", true);
  }, [clearLocalAppState, navigate]);

  useEffect(() => {
    const handlePopState = () => {
      setRoute(normalizeRoute(window.location.pathname));
    };
    window.addEventListener("popstate", handlePopState);
    return () => window.removeEventListener("popstate", handlePopState);
  }, []);

  useEffect(() => {
    const bootstrap = async () => {
      setIsBootstrapping(true);
      setError(null);

      await warmVaultCache();
      const existingToken = await vaultGet("session_jwt").catch(() => null);
      if (!existingToken) {
        navigate("/login", true);
        setIsBootstrapping(false);
        return;
      }

      const applied = await applySession(existingToken);
      if (!applied) {
        await logout();
        setError("Session expired. Please sign in again.");
      }

      setIsBootstrapping(false);
    };

    void bootstrap();
  }, [applySession, logout, navigate, warmVaultCache]);

  useEffect(() => {
    if (route !== "/app" || !subscription?.active) {
      return;
    }

    void detectMarkItDown();
  }, [detectMarkItDown, route, subscription?.active]);

  useEffect(() => {
    if (route !== "/app" || !subscription?.active || localDataLoaded) {
      return;
    }
    void loadLocalData();
  }, [loadLocalData, localDataLoaded, route, subscription?.active]);

  useEffect(() => {
    if (!localDataLoaded) {
      return;
    }
    void saveJson("runs.json", runs);
  }, [localDataLoaded, runs]);

  useEffect(() => {
    if (!localDataLoaded) {
      return;
    }
    void saveJson("approvals.json", approvals);
  }, [approvals, localDataLoaded]);

  useEffect(() => {
    if (!localDataLoaded) {
      return;
    }
    void saveJson("settings.json", appSettings);
  }, [appSettings, localDataLoaded]);

  useEffect(() => {
    chatMessagesRef.current = chatMessages;
  }, [chatMessages]);

  useEffect(() => {
    runsRef.current = runs;
  }, [runs]);

  useEffect(() => {
    plannedRunsRef.current = plannedRuns;
  }, [plannedRuns]);

  useEffect(() => {
    setShowPlanDetails(false);
  }, [activePlanRunId]);

  useEffect(() => {
    setIsEditingAgentName(false);
    setAgentNameDraft(agentPanelAgentId ? agents.find((agent) => agent.id === agentPanelAgentId)?.name ?? "" : "");
    setChatNotice(null);
    setOpenMissionMenuId(null);
    setShowHeaderMissionMenu(false);
  }, [agentPanelAgentId, agents]);

  useEffect(() => {
    const handlePointerDown = (event: PointerEvent) => {
      const target = event.target as HTMLElement | null;
      if (!target?.closest("[data-mission-menu]")) {
        setOpenMissionMenuId(null);
      }
      if (!target?.closest("[data-header-mission-menu]")) {
        setShowHeaderMissionMenu(false);
      }
    };

    window.addEventListener("pointerdown", handlePointerDown);
    return () => {
      window.removeEventListener("pointerdown", handlePointerDown);
    };
  }, []);

  useEffect(
    () => () => {
      Object.values(agentNameConfirmationTimeoutsRef.current).forEach((timeoutId) =>
        window.clearTimeout(timeoutId)
      );
      agentNameConfirmationTimeoutsRef.current = {};
    },
    []
  );

  useEffect(() => {
    if (!undoState) {
      return;
    }

    const timeoutId = window.setTimeout(() => {
      setUndoState((current) => {
        if (!current || current.expiresAt !== undoState.expiresAt) {
          return current;
        }
        return null;
      });
    }, Math.max(0, undoState.expiresAt - Date.now()));

    return () => window.clearTimeout(timeoutId);
  }, [undoState]);

  useEffect(() => {
    if (!lockToastMessage) {
      return;
    }

    const timeoutId = window.setTimeout(() => {
      setLockToastMessage((current) => (current === lockToastMessage ? null : current));
    }, 3_200);

    return () => window.clearTimeout(timeoutId);
  }, [lockToastMessage]);

  useEffect(() => {
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const handleChange = (event: MediaQueryListEvent) => setSystemPrefersDark(event.matches);
    setSystemPrefersDark(media.matches);
    media.addEventListener("change", handleChange);
    return () => media.removeEventListener("change", handleChange);
  }, []);

  const resolvedAppearance = useMemo<"light" | "dark">(() => {
    if (appSettings.appearance === "system") {
      return systemPrefersDark ? "dark" : "light";
    }
    return appSettings.appearance;
  }, [appSettings.appearance, systemPrefersDark]);

  useEffect(() => {
    document.documentElement.setAttribute("data-theme", resolvedAppearance);
  }, [resolvedAppearance]);

  useEffect(() => {
    document.documentElement.setAttribute("data-skin", appSettings.skin);
  }, [appSettings.skin]);

  useEffect(() => {
    if (!activePlanRunId) {
      return;
    }

    const mode = plannedRuns[activePlanRunId]?.plan?.mode;
    if (mode) {
      setAutonomyMode(mode);
    }
  }, [activePlanRunId, plannedRuns]);

  useEffect(() => {
    if (!agents.length) {
      setSelectedAgentId(null);
      return;
    }

    if (!selectedAgentId || !agents.some((agent) => agent.id === selectedAgentId)) {
      setSelectedAgentId(agents[0].id);
    }
  }, [agents, selectedAgentId]);

  useEffect(() => {
    if (!selectedAgentId) {
      setAgentPanelAgentId(null);
      return;
    }
    if (agentPanelAgentId !== selectedAgentId) {
      setAgentPanelAgentId(selectedAgentId);
    }
  }, [agentPanelAgentId, selectedAgentId]);

  useEffect(() => {
    if (!workspaces.length) {
      setSelectedWorkspaceId(null);
      return;
    }

    if (!selectedWorkspaceId || !workspaces.some((workspace) => workspace.id === selectedWorkspaceId)) {
      setSelectedWorkspaceId(workspaces[0].id);
    }
  }, [selectedWorkspaceId, workspaces]);

  useEffect(() => {
    if (!runs.length) {
      setSelectedRunId(null);
      return;
    }

    if (!selectedRunId || !runs.some((run) => run.id === selectedRunId)) {
      setSelectedRunId(runs[0].id);
    }
  }, [runs, selectedRunId]);

  useEffect(() => {
    if (!selectedAgentId) {
      return;
    }
    setAppSettings((previous) =>
      previous.lastActiveAgentId === selectedAgentId
        ? previous
        : { ...previous, lastActiveAgentId: selectedAgentId }
    );
  }, [selectedAgentId]);

  useEffect(() => {
    if (!selectedWorkspaceId) {
      return;
    }
    setAppSettings((previous) =>
      previous.lastActiveWorkspaceId === selectedWorkspaceId
        ? previous
        : { ...previous, lastActiveWorkspaceId: selectedWorkspaceId }
    );
  }, [selectedWorkspaceId]);

  useEffect(() => {
    if (isBootstrapping) {
      return;
    }

    if (!sessionToken && route !== "/login") {
      navigate("/login", true);
      return;
    }

    if (route === "/app" && subscription && !subscription.active) {
      navigate("/locked", true);
    }
  }, [isBootstrapping, navigate, route, sessionToken, subscription]);

  useEffect(() => {
    if (route !== "/app") {
      return;
    }
    if (!agentPanelAgentId || agentPanelTab !== "chat") {
      return;
    }
    if (tab !== "agents") {
      return;
    }

    const active = document.activeElement as HTMLElement | null;
    const typingElsewhere =
      Boolean(active) &&
      active !== chatInputRef.current &&
      (active?.tagName === "INPUT" ||
        active?.tagName === "TEXTAREA" ||
        active?.isContentEditable === true);
    if (typingElsewhere) {
      return;
    }

    const frame = window.requestAnimationFrame(() => {
      chatInputRef.current?.focus();
    });
    return () => window.cancelAnimationFrame(frame);
  }, [agentPanelAgentId, agentPanelTab, route, tab]);

  useEffect(() => {
    if (route !== "/app" || tab !== "agents" || agentPanelTab !== "chat") {
      return;
    }

    const frame = window.requestAnimationFrame(() => {
      scrollChatToBottom("auto");
    });
    return () => window.cancelAnimationFrame(frame);
  }, [agentPanelAgentId, agentPanelTab, route, scrollChatToBottom, tab]);

  const selectedRun = useMemo(
    () => runs.find((run) => run.id === selectedRunId) ?? null,
    [runs, selectedRunId]
  );
  const selectedAgent = useMemo(
    () => agents.find((agent) => agent.id === selectedAgentId) ?? null,
    [agents, selectedAgentId]
  );
  useEffect(() => {
    if (route !== "/app" || !subscription?.active || !selectedAgent) {
      return;
    }

    void warmVaultCache(selectedAgent.provider);
  }, [route, selectedAgent, subscription?.active, warmVaultCache]);

  const activePlannedRun = useMemo(
    () => (activePlanRunId ? plannedRuns[activePlanRunId] ?? null : null),
    [activePlanRunId, plannedRuns]
  );
  const agentPanelAgent = useMemo(
    () => agents.find((agent) => agent.id === agentPanelAgentId) ?? null,
    [agentPanelAgentId, agents]
  );
  const agentPanelProviderWarning = useMemo(() => {
    if (!agentPanelAgent) {
      return null;
    }
    return providerMissingKeyMessage(agentPanelAgent.provider, vaultStatus);
  }, [agentPanelAgent, vaultStatus]);
  const agentPanelProviderLabel = useMemo(() => {
    if (!agentPanelAgent) {
      return "";
    }
    return AGENT_PROVIDER_HEADER_LABELS[agentPanelAgent.provider];
  }, [agentPanelAgent]);
  const agentPanelMessages = useMemo(
    () => chatMessages.filter((message) => message.agentId === agentPanelAgentId),
    [agentPanelAgentId, chatMessages]
  );
  const agentPanelRuns = useMemo(
    () => runs.filter((run) => run.agentId === agentPanelAgentId),
    [agentPanelAgentId, runs]
  );
  const agentPanelPendingApprovals = useMemo(
    () =>
      approvals.filter(
        (approval) => approval.status === "pending" && approval.agentId === agentPanelAgentId
      ),
    [agentPanelAgentId, approvals]
  );
  const agentPanelMissions = useMemo(
    () =>
      missions.filter(
        (mission) => !mission.archived && mission.agentId === agentPanelAgentId
      ),
    [agentPanelAgentId, missions]
  );
  const missionCountsByAgent = useMemo(() => {
    const counts = new Map<string, { total: number; enabled: number }>();
    for (const mission of missions) {
      if (mission.archived) {
        continue;
      }
      const current = counts.get(mission.agentId) ?? { total: 0, enabled: 0 };
      current.total += 1;
      if (mission.enabled) {
        current.enabled += 1;
      }
      counts.set(mission.agentId, current);
    }
    return counts;
  }, [missions]);
  const agentEnabledMissionCount = useMemo(
    () => agentPanelMissions.filter((mission) => mission.enabled).length,
    [agentPanelMissions]
  );
  const assistantDisplayName = useMemo(() => {
    const trimmed = agentPanelAgent?.name?.trim() || selectedAgent?.name?.trim();
    return trimmed ? trimmed : "Assistant";
  }, [agentPanelAgent?.name, selectedAgent?.name]);
  const agentPresence = useMemo(() => computeAgentStatus(agentPanelAgent, runs, missions), [
    agentPanelAgent,
    missions,
    runs
  ]);
  const agentPresenceLabel = useMemo(() => {
    if (agentPresence === "running") {
      return "Running";
    }
    if (agentPresence === "error") {
      return "Error";
    }
    return "Online";
  }, [agentPresence]);
  const activeHandshakeStep = useMemo<HandshakeStep | null>(() => {
    if (!agentPanelAgent) {
      return null;
    }
    return pendingHandshakeByAgent[agentPanelAgent.id] ?? null;
  }, [agentPanelAgent, pendingHandshakeByAgent]);
  const activeAgentNameConfirmation = useMemo(() => {
    if (!agentPanelAgent) {
      return null;
    }

    return agentNameConfirmationByAgent[agentPanelAgent.id] ?? null;
  }, [agentNameConfirmationByAgent, agentPanelAgent]);
  const canEditAgentNameConfirmation = useMemo(
    () =>
      Boolean(activeAgentNameConfirmation && activeAgentNameConfirmation.expiresAt > Date.now()),
    [activeAgentNameConfirmation]
  );
  const missingAgentProviderKeyMessage = useMemo(() => {
    if (!agentPanelAgent) {
      return null;
    }
    return providerMissingKeyMessage(agentPanelAgent.provider, vaultStatus);
  }, [agentPanelAgent, vaultStatus]);

  useEffect(() => {
    if (route !== "/app" || tab !== "agents" || agentPanelTab !== "chat" || !agentPanelAgentId) {
      return;
    }

    const lastMessage = agentPanelMessages[agentPanelMessages.length - 1] ?? null;
    const lastSignature = lastMessage
      ? `${lastMessage.id}:${lastMessage.content.length}:${agentPanelMessages.length}`
      : "";
    const hasChanged = lastSignature !== chatLastMessageSignatureRef.current;
    chatLastMessageSignatureRef.current = lastSignature;

    if (!hasChanged || !lastMessage) {
      if (!lastMessage) {
        setShowJumpToLatest(false);
      }
      return;
    }

    const nearBottom = isChatNearBottom();
    chatNearBottomRef.current = nearBottom;
    if (nearBottom || lastMessage.role === "user") {
      const frame = window.requestAnimationFrame(() => {
        scrollChatToBottom("auto");
      });
      return () => window.cancelAnimationFrame(frame);
    }

    setShowJumpToLatest(true);
  }, [
    agentPanelAgentId,
    agentPanelMessages,
    agentPanelTab,
    isChatNearBottom,
    route,
    scrollChatToBottom,
    tab
  ]);

  const isChatActionBusy = useMemo(
    () =>
      Boolean(activeChatRunId) ||
      Boolean(activeNonStreamingRunId) ||
      activePlannedRun?.status === "planning" ||
      activePlannedRun?.status === "executing" ||
      activePlannedRun?.status === "executing_direct",
    [activeChatRunId, activeNonStreamingRunId, activePlannedRun]
  );
  const pendingApprovals = useMemo(
    () => approvals.filter((approval) => approval.status === "pending"),
    [approvals]
  );
  const usageSummaries = useMemo(() => {
    const now = Date.now();
    const todayStart = new Date();
    todayStart.setHours(0, 0, 0, 0);
    const todayStartMs = todayStart.getTime();
    const last7DaysMs = now - 7 * 24 * 60 * 60 * 1000;
    const last30DaysMs = now - 30 * 24 * 60 * 60 * 1000;

    const summarize = (predicate: (eventTimeMs: number) => boolean): UsageSummary => {
      let eventCount = 0;
      let totalTokens = 0;
      let totalCostUsd = 0;

      for (const event of usageEvents) {
        const eventTimeMs = new Date(event.ts).getTime();
        if (Number.isNaN(eventTimeMs) || !predicate(eventTimeMs)) {
          continue;
        }

        eventCount += 1;
        totalTokens += eventTokenTotal(event);
        totalCostUsd += eventCostValue(event);
      }

      return { eventCount, totalTokens, totalCostUsd };
    };

    return {
      today: summarize((eventTimeMs) => eventTimeMs >= todayStartMs),
      sevenDays: summarize((eventTimeMs) => eventTimeMs >= last7DaysMs),
      thirtyDays: summarize((eventTimeMs) => eventTimeMs >= last30DaysMs)
    };
  }, [usageEvents]);

  const usageByProvider = useMemo(() => {
    const map = new Map<string, { count: number; tokens: number; cost: number }>();
    for (const event of usageEvents) {
      const key = event.provider || "unknown";
      const current = map.get(key) ?? { count: 0, tokens: 0, cost: 0 };
      current.count += 1;
      current.tokens += eventTokenTotal(event);
      current.cost += eventCostValue(event);
      map.set(key, current);
    }

    return Array.from(map.entries())
      .map(([provider, values]) => ({ provider, ...values }))
      .sort((a, b) => b.cost - a.cost || b.tokens - a.tokens || b.count - a.count);
  }, [usageEvents]);

  const usageByAgent = useMemo(() => {
    const agentNameById = new Map(agents.map((agent) => [agent.id, agent.name]));
    const map = new Map<string, { count: number; tokens: number; cost: number }>();

    for (const event of usageEvents) {
      const key = event.agentId ? agentNameById.get(event.agentId) ?? event.agentId : "Unassigned";
      const current = map.get(key) ?? { count: 0, tokens: 0, cost: 0 };
      current.count += 1;
      current.tokens += eventTokenTotal(event);
      current.cost += eventCostValue(event);
      map.set(key, current);
    }

    return Array.from(map.entries())
      .map(([agent, values]) => ({ agent, ...values }))
      .sort((a, b) => b.cost - a.cost || b.tokens - a.tokens || b.count - a.count);
  }, [agents, usageEvents]);

  const mostExpensiveEvents = useMemo(() => {
    const sorted = [...usageEvents].sort((left, right) => {
      if (left.estimatedCostUsd !== null && right.estimatedCostUsd !== null) {
        return right.estimatedCostUsd - left.estimatedCostUsd;
      }

      if (left.estimatedCostUsd !== null) {
        return -1;
      }

      if (right.estimatedCostUsd !== null) {
        return 1;
      }

      return eventTokenTotal(right) - eventTokenTotal(left);
    });

    return sorted.slice(0, 10);
  }, [usageEvents]);

  const mdStatusLabel = useMemo(() => {
    if (mdStatus === "ready") {
      return "Ready";
    }
    if (mdStatus === "installing") {
      return "Installing";
    }
    if (mdStatus === "error") {
      return "Error";
    }
    return "Not installed";
  }, [mdStatus]);

  const openAiCompatModelChoices = useMemo(
    () => Array.from(new Set([...OPENAI_COMPAT_ADVANCED_DEFAULT_MODELS, ...openAiCompatModelOptions])),
    [openAiCompatModelOptions]
  );
  const openAiModelChoices = useMemo(
    () => Array.from(new Set([...OPENAI_ADVANCED_DEFAULT_MODELS, ...openAiModelOptions])),
    [openAiModelOptions]
  );
  const anthropicModelChoices = useMemo(
    () => Array.from(new Set(ANTHROPIC_ADVANCED_DEFAULT_MODELS)),
    []
  );
  const googleModelChoices = useMemo(
    () => Array.from(new Set(GOOGLE_ADVANCED_DEFAULT_MODELS)),
    []
  );
  const isOpenAiCompatCustomModel = useMemo(
    () =>
      appSettings.openaiCompatModelMode === "custom" ||
      (appSettings.openaiCompatModelMode !== "tier" &&
        appSettings.openaiCompatModelId.trim().length > 0 &&
        !openAiCompatModelChoices.includes(appSettings.openaiCompatModelId.trim())),
    [appSettings.openaiCompatModelId, appSettings.openaiCompatModelMode, openAiCompatModelChoices]
  );
  const isOpenAiCustomModel = useMemo(
    () =>
      appSettings.openaiModelMode === "custom" ||
      (appSettings.openaiModelMode !== "tier" &&
        appSettings.openaiModelId.trim().length > 0 &&
        !openAiModelChoices.includes(appSettings.openaiModelId.trim())),
    [appSettings.openaiModelId, appSettings.openaiModelMode, openAiModelChoices]
  );
  const isAnthropicCustomModel = useMemo(
    () =>
      appSettings.anthropicModelMode === "custom" ||
      (appSettings.anthropicModelMode !== "tier" &&
        appSettings.anthropicModelId.trim().length > 0 &&
        !anthropicModelChoices.includes(appSettings.anthropicModelId.trim())),
    [anthropicModelChoices, appSettings.anthropicModelId, appSettings.anthropicModelMode]
  );
  const isGoogleCustomModel = useMemo(
    () =>
      appSettings.googleModelMode === "custom" ||
      (appSettings.googleModelMode !== "tier" &&
        appSettings.googleModelId.trim().length > 0 &&
        !googleModelChoices.includes(appSettings.googleModelId.trim())),
    [appSettings.googleModelId, appSettings.googleModelMode, googleModelChoices]
  );

  const filteredSkills = useMemo(() => {
    const query = skillsSearch.trim().toLowerCase();
    if (!query) {
      return verifiedSkills;
    }

    return verifiedSkills.filter((skill) => {
      const name = skill.manifest?.name.toLowerCase() ?? "";
      const description = skill.manifest?.description.toLowerCase() ?? "";
      const tags = (skill.manifest?.tags ?? []).join(" ").toLowerCase();
      const id = skill.id.toLowerCase();
      return (
        name.includes(query) ||
        description.includes(query) ||
        tags.includes(query) ||
        id.includes(query)
      );
    });
  }, [skillsSearch, verifiedSkills]);

  const selectedSkill = useMemo(
    () => verifiedSkills.find((skill) => skill.id === selectedSkillId) ?? null,
    [selectedSkillId, verifiedSkills]
  );

  const selectedSkillPermissionsDiff = useMemo(
    () => (selectedSkill ? buildPermissionsDiff(selectedSkill) : []),
    [selectedSkill]
  );

  const selectedSkillInstalled = useMemo(() => {
    if (!selectedSkill?.manifest) {
      return false;
    }
    return installedSkills.some(
      (item) => item.id === selectedSkill.id && item.version === selectedSkill.manifest?.version
    );
  }, [installedSkills, selectedSkill]);
  const historyFilterOptions = useMemo(() => {
    const options: Array<{ kind: ConfigObjectKind; id: string; label: string; value: string }> = [];

    for (const agent of agents) {
      options.push({
        kind: "agent",
        id: agent.id,
        label: `Agent · ${agent.name}`,
        value: `agent:${agent.id}`
      });
    }
    for (const mission of missions) {
      options.push({
        kind: "mission",
        id: mission.id,
        label: `Mission · ${mission.title}`,
        value: `mission:${mission.id}`
      });
    }
    for (const workspace of workspaces) {
      options.push({
        kind: "workspace",
        id: workspace.id,
        label: `Workspace · ${workspace.name}`,
        value: `workspace:${workspace.id}`
      });
    }
    for (const install of skillInstalls) {
      options.push({
        kind: "skill_install",
        id: install.id,
        label: `Skill Install · ${install.id} (${install.version})`,
        value: `skill_install:${install.id}`
      });
    }
    for (const policy of webPolicies) {
      options.push({
        kind: "web_policy",
        id: policy.host,
        label: `Web Policy · ${policy.host}`,
        value: `web_policy:${policy.host}`
      });
    }
    for (const policy of filePolicies) {
      options.push({
        kind: "file_policy",
        id: policy.path,
        label: `File Policy · ${policy.path}`,
        value: `file_policy:${policy.path}`
      });
    }

    return options;
  }, [agents, filePolicies, missions, skillInstalls, webPolicies, workspaces]);

  const filteredAuditEntries = useMemo(() => {
    return auditEntries.filter((entry) => {
      if (historyKindFilter !== "all" && entry.object.kind !== historyKindFilter) {
        return false;
      }
      if (historyObjectFilter !== "all") {
        const [kind, id] = historyObjectFilter.split(":", 2);
        if (entry.object.kind !== kind || entry.object.id !== id) {
          return false;
        }
      }
      return true;
    });
  }, [auditEntries, historyKindFilter, historyObjectFilter]);
  const activeWebPolicies = useMemo(
    () => webPolicies.filter((policy) => !policy.archived),
    [webPolicies]
  );
  const activeFilePolicies = useMemo(
    () => filePolicies.filter((policy) => !policy.archived),
    [filePolicies]
  );
  const webPolicyByHost = useMemo(() => {
    const map = new Map<string, WebPolicy>();
    for (const policy of activeWebPolicies) {
      map.set(policy.host.toLowerCase(), policy);
    }
    return map;
  }, [activeWebPolicies]);
  const filePolicyByPath = useMemo(() => {
    const map = new Map<string, FilePolicy>();
    for (const policy of activeFilePolicies) {
      map.set(normalizeFolderPath(policy.path), policy);
    }
    return map;
  }, [activeFilePolicies]);
  const healthCheckUrl = useMemo(() => apiUrl("/health"), []);

  useEffect(() => {
    if (!filteredSkills.length) {
      setSelectedSkillId(null);
      return;
    }

    if (!selectedSkillId || !filteredSkills.some((skill) => skill.id === selectedSkillId)) {
      setSelectedSkillId(filteredSkills[0].id);
    }
  }, [filteredSkills, selectedSkillId]);

  useEffect(() => {
    if (historyObjectFilter === "all") {
      return;
    }

    const stillValid = historyFilterOptions.some((option) => option.value === historyObjectFilter);
    if (!stillValid) {
      setHistoryObjectFilter("all");
    }
  }, [historyFilterOptions, historyObjectFilter]);

  useEffect(() => {
    let unlistenChunk: UnlistenFn | undefined;
    let unlistenDone: UnlistenFn | undefined;
    let unlistenError: UnlistenFn | undefined;
    let unlistenNotice: UnlistenFn | undefined;

    const attach = async () => {
      unlistenChunk = await listen<LlmStreamChunkPayload>("llm_stream_chunk", (event) => {
        const { runId, delta } = event.payload;
        if (!runId || !delta) {
          return;
        }

        if (!(runId in streamPinToBottomRef.current)) {
          streamPinToBottomRef.current[runId] = isChatNearBottom();
        }
        const shouldPinToBottom = streamPinToBottomRef.current[runId] === true;

        runOutputBufferRef.current[runId] = `${runOutputBufferRef.current[runId] ?? ""}${delta}`;
        if (!shouldShowMissionLiveOutput(runId)) {
          return;
        }

        const meta = chatRunMetaRef.current[runId];
        setChatMessages((previous) => {
          const messageIndex = previous.findIndex(
            (entry) => entry.runId === runId && entry.role === "assistant"
          );

          if (messageIndex < 0) {
            return previous.concat({
              id: crypto.randomUUID(),
              runId,
              agentId: meta?.agentId ?? selectedAgentId ?? "",
              role: "assistant",
              content: delta,
              createdAt: new Date().toISOString()
            });
          }

          const next = [...previous];
          const existing = next[messageIndex];
          next[messageIndex] = {
            ...existing,
            content: `${existing.content}${delta}`
          };
          return next;
        });

        if (shouldPinToBottom) {
          window.requestAnimationFrame(() => {
            scrollChatToBottom("auto");
          });
        }
      });

      unlistenDone = await listen<LlmStreamDonePayload>("llm_stream_done", (event) => {
        const { runId, cancelled = false, usage, model } = event.payload;
        if (!runId) {
          return;
        }

        const meta = chatRunMetaRef.current[runId];
        const missionContext = missionRunContextRef.current[runId] ?? null;
        const bufferedOutput = runOutputBufferRef.current[runId] ?? "";
        const assistantContent =
          chatMessagesRef.current.find(
            (entry) => entry.runId === runId && entry.role === "assistant"
          )?.content ?? "";
        const finalOutput = bufferedOutput || assistantContent;
        const latencyMs = meta ? performance.now() - meta.startedAt : 0;

        setRuns((previous) =>
          previous.map((run) =>
            run.id === runId
              ? {
                  ...run,
                  status: cancelled ? "cancelled" : "completed",
                  finishedAt: new Date().toISOString(),
                  summary: cancelled
                    ? "Stream cancelled by user."
                    : "Streaming response completed.",
                  logs: run.logs.concat([
                    cancelled
                      ? "Streaming cancelled."
                      : "Streaming completed.",
                    usage
                      ? `Usage: prompt=${usage.prompt_tokens ?? 0}, completion=${usage.completion_tokens ?? 0}, total=${usage.total_tokens ?? 0}`
                      : "Usage: unavailable",
                    ...(missionContext ? [`Output:\n${finalOutput || "(empty)"}`] : [])
                  ])
                }
              : run
          )
        );

        if (usage && meta) {
          void logUsageEvent(
            buildUsageEvent({
              agentId: meta.agentId,
              runId,
              provider: "openai_compat",
              model: model ?? meta.model,
              kind: "llm",
              promptTokens:
                typeof usage.prompt_tokens === "number" ? usage.prompt_tokens : null,
              completionTokens:
                typeof usage.completion_tokens === "number"
                  ? usage.completion_tokens
                  : null,
              totalTokens:
                typeof usage.total_tokens === "number" ? usage.total_tokens : null,
              inputChars: meta.prompt.length,
              outputChars: assistantContent.length,
              latencyMs
            })
          );
        }

        const waiter = streamWaitersRef.current[runId];
        if (waiter) {
          delete streamWaitersRef.current[runId];
          waiter.resolve({
            cancelled,
            usage,
            model
          });
          setActiveChatRunId((current) => (current === runId ? null : current));
          if (cancelled) {
            setChatError("Generation cancelled.");
          }
          delete streamPinToBottomRef.current[runId];
          delete chatRunMetaRef.current[runId];
          if (!missionContext) {
            delete runOutputBufferRef.current[runId];
          }
          return;
        }

        if (cancelled) {
          setChatError("Generation cancelled.");
        }

        setActiveChatRunId((current) => (current === runId ? null : current));
        delete streamPinToBottomRef.current[runId];
        delete chatRunMetaRef.current[runId];
        if (!missionContext) {
          delete runOutputBufferRef.current[runId];
        }
      });

      unlistenError = await listen<LlmStreamErrorPayload>("llm_stream_error", (event) => {
        const { runId, message } = event.payload;
        if (!runId) {
          return;
        }

        const missionContext = missionRunContextRef.current[runId] ?? null;
        const waiter = streamWaitersRef.current[runId];
        if (waiter) {
          delete streamWaitersRef.current[runId];
          waiter.reject(message || "Streaming failed.");
          setActiveChatRunId((current) => (current === runId ? null : current));
          setChatError(message || "Streaming failed.");
          delete streamPinToBottomRef.current[runId];
          delete chatRunMetaRef.current[runId];
          if (!missionContext) {
            delete runOutputBufferRef.current[runId];
          }
          return;
        }

        setRuns((previous) =>
          previous.map((run) =>
            run.id === runId
              ? {
                  ...run,
                  status: "failed",
                  finishedAt: new Date().toISOString(),
                  summary: message || "Streaming failed.",
                  logs: run.logs.concat(message || "Streaming failed.")
                }
              : run
          )
        );

        setChatError(message || "Streaming failed.");
        setActiveChatRunId((current) => (current === runId ? null : current));
        delete streamPinToBottomRef.current[runId];
        delete chatRunMetaRef.current[runId];
        if (!missionContext) {
          delete runOutputBufferRef.current[runId];
        }
      });

      unlistenNotice = await listen<LlmStreamNoticePayload>("llm_stream_notice", (event) => {
        const { runId, message, detail } = event.payload;
        if (!runId || !message) {
          return;
        }

        setChatNotice(message);
        setRuns((previous) =>
          previous.map((run) => {
            if (run.id !== runId) {
              return run;
            }
            const runAgent = agents.find((agent) => agent.id === run.agentId);
            if (runAgent?.policy.loggingMode !== "detailed" || !detail) {
              return run;
            }
            return {
              ...run,
              logs: run.logs.concat(detail)
            };
          })
        );
      });
    };

    void attach();

    return () => {
      if (unlistenChunk) {
        unlistenChunk();
      }
      if (unlistenDone) {
        unlistenDone();
      }
      if (unlistenError) {
        unlistenError();
      }
      if (unlistenNotice) {
        unlistenNotice();
      }
    };
  }, [
    agents,
    buildUsageEvent,
    isChatNearBottom,
    logUsageEvent,
    scrollChatToBottom,
    selectedAgentId,
    shouldShowMissionLiveOutput
  ]);

  const handleStartAuth = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();

    const email = emailInput.trim().toLowerCase();
    if (!email) {
      setError("Enter a valid email.");
      return;
    }

    setIsBusy(true);
    setError(null);
    setDevCode(null);

    try {
      const result = await callAuthStart(email);
      if (!result.ok) {
        setError(result.error);
        return;
      }

      setVerifyEmail(email);
      setLoginStep("verify");
      setCodeInput("");
      if (!IS_PRODUCTION && result.devCode) {
        setDevCode(result.devCode);
      }
    } catch {
      setError(`Cannot reach server: ${apiUrl("/auth/start")}`);
    } finally {
      setIsBusy(false);
    }
  };

  const handleVerifyAuth = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setIsBusy(true);
    setError(null);
    const startedAt = performance.now();

    try {
      const result = await callAuthVerify(verifyEmail, codeInput);
      if (!result.ok) {
        setError(result.error);
        return;
      }

      if (!result.token) {
        setError("Server response missing token.");
        return;
      }

      await vaultSet("session_jwt", result.token);
      const applied = await applySession(result.token, result.email ?? verifyEmail);
      if (!applied) {
        await logout();
        setError("Unable to verify subscription for this account.");
        return;
      }

      const loginEvent = buildUsageEvent({
        agentId: null,
        runId: null,
        provider: "bossclaw",
        model: null,
        kind: "other",
        inputChars: verifyEmail.length + codeInput.length,
        outputChars: (result.email ?? verifyEmail).length,
        latencyMs: performance.now() - startedAt
      });
      await logUsageEvent(loginEvent);
    } catch {
      setError(`Cannot reach server: ${apiUrl("/auth/verify")}`);
    } finally {
      setIsBusy(false);
    }
  };

  const pingHealth = useCallback(async () => {
    setDiagnosticsLoading(true);
    setDiagnosticsResult(null);

    try {
      await ensureRustApiBase();
      const payload = (await invoke("api_health")) as unknown;
      setDiagnosticsResult(`Health check OK: ${JSON.stringify(payload)}`);
    } catch (error) {
      const message = invokeErrorMessage(error, `Cannot reach server: ${healthCheckUrl}`);
      if (message.includes("Cannot reach API endpoint")) {
        setDiagnosticsResult(`Cannot reach server: ${healthCheckUrl}`);
      } else {
        setDiagnosticsResult(message);
      }
    } finally {
      setDiagnosticsLoading(false);
    }
  }, [healthCheckUrl]);

  const buildWebPolicyProposal = useCallback(
    (input: {
      host: string;
      level: WebExtractLevel;
      allowPaths?: string[];
      approvedBy: "user" | "agent";
      notes?: string;
      applyMode: "autopilot" | "fsd";
      proposedBy: { type: "user" | "agent"; id?: string };
      summary: string;
    }): ConfigChangeProposal => {
      const existing = webPolicyByHost.get(input.host);
      const nextPolicy: WebPolicy = {
        host: input.host,
        level: input.level,
        allowPaths: input.allowPaths?.length ? input.allowPaths : existing?.allowPaths,
        approvedAt: new Date().toISOString(),
        approvedBy: input.approvedBy,
        notes: input.notes ?? existing?.notes
      };

      return {
        id: crypto.randomUUID(),
        ts: new Date().toISOString(),
        object: { kind: "web_policy", id: input.host },
        summary: input.summary,
        diff: diffObjects(existing ?? null, nextPolicy),
        applyMode: input.applyMode,
        requiresConfirm: true,
        proposedBy: input.proposedBy,
        patch: {
          after: nextPolicy as unknown as Record<string, unknown>
        }
      };
    },
    [webPolicyByHost]
  );

  const saveWebPolicy = useCallback(
    async (input: {
      host: string;
      level: WebExtractLevel;
      allowPaths?: string[];
      approvedBy: "user" | "agent";
      notes?: string;
      applyMode?: "autopilot" | "fsd";
      proposedBy?: { type: "user" | "agent"; id?: string };
      summary?: string;
      undoMessage?: string;
    }) => {
      const proposal = buildWebPolicyProposal({
        host: input.host,
        level: input.level,
        allowPaths: input.allowPaths,
        approvedBy: input.approvedBy,
        notes: input.notes,
        applyMode: input.applyMode ?? "autopilot",
        proposedBy: input.proposedBy ?? { type: "user", id: sessionEmail ?? undefined },
        summary:
          input.summary ??
          `Allow web access to ${input.host} at ${WEB_LEVEL_LABELS[input.level]} level`
      });

      await applyProposal(
        proposal,
        input.proposedBy ?? { type: "user", id: sessionEmail ?? undefined },
        input.undoMessage ?? proposal.summary
      );
    },
    [applyProposal, buildWebPolicyProposal, sessionEmail]
  );

  const addWebPolicyFromInput = useCallback(async () => {
    setWebAccessError(null);
    setWebAccessMessage(null);

    const host = normalizeHostInput(webPolicyHostInput);
    if (!host) {
      setWebAccessError("Enter a valid host (example.com).");
      return;
    }

    try {
      const allowPaths = webPolicyPathInput
        .split(",")
        .map((value) => value.trim())
        .filter(Boolean);

      await saveWebPolicy({
        host,
        level: webPolicyLevelInput,
        allowPaths: allowPaths.length ? allowPaths : undefined,
        approvedBy: "user",
        summary: `Allow web access to ${host} at ${WEB_LEVEL_LABELS[webPolicyLevelInput]} level`,
        undoMessage: `Updated web access policy for ${host}`
      });
      setWebPolicyHostInput("");
      setWebPolicyPathInput("");
      setWebAccessMessage(`Web access approved for ${host}.`);
    } catch {
      setWebAccessError("Unable to save web access policy.");
    }
  }, [saveWebPolicy, webPolicyHostInput, webPolicyLevelInput, webPolicyPathInput]);

  const updateWebPolicyLevel = useCallback(
    async (policy: WebPolicy, level: WebExtractLevel) => {
      setWebAccessError(null);
      setWebAccessMessage(null);
      try {
        await saveWebPolicy({
          host: policy.host,
          level,
          allowPaths: policy.allowPaths,
          approvedBy: "user",
          notes: policy.notes,
          summary: `Update ${policy.host} to ${WEB_LEVEL_LABELS[level]} level`,
          undoMessage: `Updated ${policy.host} level`
        });
        setWebAccessMessage(`Updated ${policy.host}.`);
      } catch {
        setWebAccessError("Unable to update web policy.");
      }
    },
    [saveWebPolicy]
  );

  const archiveWebPolicy = useCallback(
    async (policy: WebPolicy) => {
      setWebAccessError(null);
      setWebAccessMessage(null);
      try {
        const proposal: ConfigChangeProposal = {
          id: crypto.randomUUID(),
          ts: new Date().toISOString(),
          object: { kind: "web_policy", id: policy.host },
          summary: `Remove web access for ${policy.host}`,
          diff: diffObjects(policy, { ...policy, archived: true }),
          applyMode: "autopilot",
          requiresConfirm: true,
          proposedBy: { type: "user", id: sessionEmail ?? undefined },
          patch: {
            after: { ...(policy as unknown as Record<string, unknown>), archived: true }
          }
        };
        await applyProposal(
          proposal,
          { type: "user", id: sessionEmail ?? undefined },
          `Removed web access for ${policy.host}`
        );
        await invoke("web_auth_delete", { host: policy.host }).catch(() => undefined);
        setWebAccessMessage(`Removed ${policy.host} from approved hosts.`);
      } catch {
        setWebAccessError("Unable to remove web policy.");
      }
    },
    [applyProposal, sessionEmail]
  );

  const buildFilePolicyProposal = useCallback(
    (input: {
      path: string;
      mode: "read" | "read_write";
      approvedBy: "user" | "agent";
      applyMode: "autopilot" | "fsd";
      proposedBy: { type: "user" | "agent"; id?: string };
      summary: string;
    }): ConfigChangeProposal => {
      const normalizedPath = normalizeFolderPath(input.path);
      const existing = filePolicyByPath.get(normalizedPath);
      const nextPolicy: FilePolicy = {
        path: normalizedPath,
        mode: input.mode,
        approvedAt: new Date().toISOString(),
        approvedBy: input.approvedBy
      };

      return {
        id: crypto.randomUUID(),
        ts: new Date().toISOString(),
        object: { kind: "file_policy", id: normalizedPath },
        summary: input.summary,
        diff: diffObjects(existing ?? null, nextPolicy),
        applyMode: input.applyMode,
        requiresConfirm: true,
        proposedBy: input.proposedBy,
        patch: {
          after: nextPolicy as unknown as Record<string, unknown>
        }
      };
    },
    [filePolicyByPath]
  );

  const saveFilePolicy = useCallback(
    async (input: {
      path: string;
      mode: "read" | "read_write";
      approvedBy: "user" | "agent";
      applyMode?: "autopilot" | "fsd";
      proposedBy?: { type: "user" | "agent"; id?: string };
      summary?: string;
      undoMessage?: string;
    }) => {
      const normalizedPath = normalizeFolderPath(input.path);
      const proposal = buildFilePolicyProposal({
        path: normalizedPath,
        mode: input.mode,
        approvedBy: input.approvedBy,
        applyMode: input.applyMode ?? "autopilot",
        proposedBy: input.proposedBy ?? { type: "user", id: sessionEmail ?? undefined },
        summary: input.summary ?? `Allow file access to ${normalizedPath}`
      });

      await applyProposal(
        proposal,
        input.proposedBy ?? { type: "user", id: sessionEmail ?? undefined },
        input.undoMessage ?? proposal.summary
      );
    },
    [applyProposal, buildFilePolicyProposal, sessionEmail]
  );

  const addFilePolicyFromPicker = useCallback(async () => {
    setFileAccessError(null);
    setFileAccessMessage(null);

    try {
      const selected = await invoke<string | string[] | null>("plugin:dialog|open", {
        options: {
          title: "Choose a folder",
          directory: true,
          multiple: false
        }
      });
      const rawPath = Array.isArray(selected) ? selected[0] : selected;
      if (!rawPath) {
        return;
      }
      const normalizedPath = normalizeFolderPath(rawPath);
      await saveFilePolicy({
        path: normalizedPath,
        mode: "read",
        approvedBy: "user",
        summary: `Allow read access to ${normalizedPath}`,
        undoMessage: `Updated file access for ${normalizedPath}`
      });
      setFileAccessMessage(`Added ${normalizedPath}.`);
    } catch {
      setFileAccessError("Unable to add folder access.");
    }
  }, [saveFilePolicy]);

  const updateFilePolicyMode = useCallback(
    async (policy: FilePolicy, mode: "read" | "read_write") => {
      setFileAccessError(null);
      setFileAccessMessage(null);
      try {
        await saveFilePolicy({
          path: policy.path,
          mode,
          approvedBy: "user",
          summary: `Update file access mode for ${policy.path}`,
          undoMessage: `Updated file access mode for ${policy.path}`
        });
        setFileAccessMessage(`Updated ${policy.path}.`);
      } catch {
        setFileAccessError("Unable to update folder access.");
      }
    },
    [saveFilePolicy]
  );

  const archiveFilePolicy = useCallback(
    async (policy: FilePolicy) => {
      setFileAccessError(null);
      setFileAccessMessage(null);
      try {
        const proposal: ConfigChangeProposal = {
          id: crypto.randomUUID(),
          ts: new Date().toISOString(),
          object: { kind: "file_policy", id: policy.path },
          summary: `Remove file access for ${policy.path}`,
          diff: diffObjects(policy, { ...policy, archived: true }),
          applyMode: "autopilot",
          requiresConfirm: true,
          proposedBy: { type: "user", id: sessionEmail ?? undefined },
          patch: {
            after: { ...(policy as unknown as Record<string, unknown>), archived: true }
          }
        };
        await applyProposal(
          proposal,
          { type: "user", id: sessionEmail ?? undefined },
          `Removed file access for ${policy.path}`
        );
        setFileAccessMessage(`Removed ${policy.path}.`);
      } catch {
        setFileAccessError("Unable to remove folder access.");
      }
    },
    [applyProposal, sessionEmail]
  );

  const saveWebAuthToken = useCallback(async (host: string) => {
    const token = webAuthInputs[host]?.trim() ?? "";
    setWebAccessError(null);
    setWebAccessMessage(null);

    if (!token) {
      setWebAccessError("Provide an auth token with cookie:, bearer:, or basic: prefix.");
      return;
    }

    try {
      await invoke("web_auth_set", { host, value: token });
      setWebAuthInputs((previous) => ({ ...previous, [host]: "" }));
      setWebAccessMessage(`Saved auth token for ${host}.`);
    } catch (error) {
      setWebAccessError(invokeErrorMessage(error, "Unable to save auth token."));
    }
  }, [webAuthInputs]);

  const clearWebAuthToken = useCallback(async (host: string) => {
    setWebAccessError(null);
    setWebAccessMessage(null);
    try {
      await invoke("web_auth_delete", { host });
      setWebAuthInputs((previous) => ({ ...previous, [host]: "" }));
      setWebAccessMessage(`Cleared auth token for ${host}.`);
    } catch (error) {
      setWebAccessError(invokeErrorMessage(error, "Unable to clear auth token."));
    }
  }, []);

  const testWebAccessFetch = useCallback(
    async (host: string, level: WebExtractLevel) => {
      setWebAccessError(null);
      setWebTestResult(null);
      setWebTestLoading(true);
      const startedAt = performance.now();

      try {
        const candidate = webTestUrl.trim();
        const parsed = new URL(candidate);
        if (parsed.host.toLowerCase() !== host.toLowerCase()) {
          throw new Error(`Test URL host must match ${host}.`);
        }

        let preview = "";
        if (level === "browser") {
          const rendered = await invoke<PwFetchRenderedResponse>("pw_fetch_rendered", {
            url: parsed.toString()
          });
          const extracted = extractWebDocument({
            html: rendered.html,
            url: parsed.toString(),
            host,
            level
          });
          preview = `Interactive fetch OK · ${extracted.title ?? "Untitled"} · ${extracted.text.slice(0, 160)}`;
          setWebTestResult(preview);
        } else {
          const response =
            level === "auth"
              ? await invoke<WebFetchResponse>("web_fetch_auth", {
                  url: parsed.toString(),
                  host
                })
              : await invoke<WebFetchResponse>("web_fetch_public", {
                  url: parsed.toString()
                });
          const extracted = extractWebDocument({
            html: response.html,
            url: parsed.toString(),
            host,
            level
          });
          preview = `HTTP ${response.status} · ${extracted.title ?? "Untitled"} · ${extracted.text.slice(0, 160)}`;
          setWebTestResult(preview);
        }

        await logUsageEvent(
          buildUsageEvent({
            agentId: null,
            runId: null,
            provider: "web",
            model: null,
            kind: "web",
            inputChars: webTestUrl.length,
            outputChars: preview.length,
            latencyMs: performance.now() - startedAt,
            tags: { level: usageTagLevel(level) }
          })
        );
      } catch (error) {
        setWebAccessError(invokeErrorMessage(error, "Web access test failed."));
      } finally {
        setWebTestLoading(false);
      }
    },
    [buildUsageEvent, logUsageEvent, webTestUrl]
  );

  const refreshPlaywrightHelper = useCallback(async () => {
    try {
      const status = await invoke<PwDetectResponse>("pw_detect");
      setPwStatus(status);
    } catch {
      setPwStatus(null);
    }
  }, []);

  const installPlaywrightHelper = useCallback(async () => {
    setPwLoading(true);
    setWebAccessError(null);
    setPwLogs("");

    try {
      const result = await invoke<PwInstallResponse>("pw_install");
      setPwLogs(result.logs);
      setWebAccessMessage("Browser helper installed.");
      await refreshPlaywrightHelper();
    } catch (error) {
      setWebAccessError(invokeErrorMessage(error, "Unable to install browser helper."));
    } finally {
      setPwLoading(false);
    }
  }, [refreshPlaywrightHelper]);

  const testPlaywrightFetch = useCallback(async () => {
    setPwTestResult(null);
    setWebAccessError(null);

    try {
      const parsed = new URL(pwTestUrl.trim());
      const response = await invoke<PwFetchRenderedResponse>("pw_fetch_rendered", {
        url: parsed.toString()
      });
      const extracted = extractWebDocument({
        html: response.html,
        url: parsed.toString(),
        host: parsed.host.toLowerCase(),
        level: "browser"
      });
      setPwTestResult(`${extracted.title ?? "Untitled"} · ${extracted.text.slice(0, 160)}`);
    } catch (error) {
      setWebAccessError(invokeErrorMessage(error, "Browser Mode fetch failed."));
    }
  }, [pwTestUrl]);

  useEffect(() => {
    if (route !== "/app" || !subscription?.active || tab !== "settings") {
      return;
    }

    void refreshPlaywrightHelper();
  }, [refreshPlaywrightHelper, route, subscription?.active, tab]);

  const updateModelSettings = useCallback((updater: (previous: AppSettings) => AppSettings) => {
    setAppSettings((previous) => {
      const next = updater(previous);
      return {
        ...next,
        openaiCompatModel: resolveModel("openai_compat", next),
        openaiModel: resolveModel("openai", next),
        anthropicModel: resolveModel("anthropic", next),
        googleModel: resolveModel("google", next)
      };
    });
  }, []);

  const refreshOpenAiCompatModels = useCallback(async () => {
    setOpenAiCompatModelRefreshError(null);
    setIsRefreshingOpenAiCompatModels(true);
    try {
      const response = await invoke<string[]>("llm_list_models", {
        provider: "openai_compat",
        baseUrl: appSettings.openaiCompatBaseUrl
      });
      const normalized = response
        .map((model) => model.trim())
        .filter((model) => model.length > 0);
      if (normalized.length) {
        setOpenAiCompatModelOptions(
          Array.from(new Set([...OPENAI_COMPAT_ADVANCED_DEFAULT_MODELS, ...normalized]))
        );
      }
    } catch {
      setOpenAiCompatModelRefreshError("Couldn't refresh models.");
    } finally {
      setIsRefreshingOpenAiCompatModels(false);
    }
  }, [appSettings.openaiCompatBaseUrl]);

  const refreshOpenAiModels = useCallback(async () => {
    setOpenAiModelRefreshError(null);
    setIsRefreshingOpenAiModels(true);
    try {
      const response = await invoke<string[]>("llm_list_models", {
        provider: "openai",
        baseUrl: "https://api.openai.com"
      });
      const normalized = response
        .map((model) => model.trim())
        .filter((model) => model.length > 0);
      if (normalized.length) {
        setOpenAiModelOptions(Array.from(new Set([...OPENAI_ADVANCED_DEFAULT_MODELS, ...normalized])));
      }
    } catch {
      setOpenAiModelRefreshError("Couldn't refresh models.");
    } finally {
      setIsRefreshingOpenAiModels(false);
    }
  }, []);

  const updatePlannedRun = useCallback(
    (runId: string, updater: (current: PlannedRunState) => PlannedRunState) => {
      setPlannedRuns((previous) => {
        const current = previous[runId];
        if (!current) {
          return previous;
        }
        return {
          ...previous,
          [runId]: updater(current)
        };
      });
    },
    []
  );

  const buildPlannerContextSummary = useCallback(
    (agent: Agent, prompt: string): string => {
      const workspaceSummary = workspaces
        .slice(0, 2)
        .map((workspace) => `${workspace.name}${workspace.path ? ` (${workspace.path})` : ""}`)
        .join(", ");
      return [
        `agent_name=${agent.name}`,
        `agent_purpose=${agent.purpose}`,
        `agent_tools=${agent.policy.toolsEnabled.join(",") || "none"}`,
        `activity_detail=${agent.policy.loggingMode}`,
        `workspace_count=${workspaces.length}`,
        `web_policy_count=${webPolicies.length}`,
        `mission_count=${missions.length}`,
        `pending_approvals=${pendingApprovals.length}`,
        `recent_runs=${runs.slice(0, 3).map((run) => run.title).join(" | ") || "none"}`,
        `known_workspaces=${workspaceSummary || "none"}`,
        `prompt_length=${prompt.length}`
      ].join("\n");
    },
    [missions.length, pendingApprovals.length, runs, webPolicies.length, workspaces]
  );

  const buildConfigProposalsFromPlan = useCallback(
    (plan: BossClawPlanV1, agent: Agent): ConfigChangeProposal[] => {
      const proposals: ConfigChangeProposal[] = [];
      const byKind: Record<ConfigObjectKind, Array<Record<string, unknown>>> = {
        agent: agents as unknown as Array<Record<string, unknown>>,
        mission: missions as unknown as Array<Record<string, unknown>>,
        workspace: workspaces as unknown as Array<Record<string, unknown>>,
        skill_install: skillInstalls as unknown as Array<Record<string, unknown>>,
        web_policy: webPolicies as unknown as Array<Record<string, unknown>>,
        file_policy: filePolicies as unknown as Array<Record<string, unknown>>
      };

      if (plan.configProposal) {
        const base = byKind[plan.configProposal.object.kind].find(
          (entry) => entry.id === plan.configProposal?.object.id
        );
        const after = {
          ...asRecord(base),
          ...asRecord(plan.configProposal.after),
          id: plan.configProposal.object.id
        };
        proposals.push({
          id: crypto.randomUUID(),
          ts: new Date().toISOString(),
          object: {
            kind: plan.configProposal.object.kind,
            id: plan.configProposal.object.id
          },
          summary: plan.configProposal.summary,
          diff: diffObjects(base ?? null, after),
          applyMode: plan.configProposal.applyMode ?? plan.mode,
          requiresConfirm: true,
          proposedBy: { type: "agent", id: agent.id },
          patch: {
            after
          }
        });
      }

      const existingPolicyHosts = new Set(
        webPolicies.filter((policy) => !policy.archived).map((policy) => policy.host.toLowerCase())
      );
      const proposedPolicyHosts = new Set<string>();

      for (const step of plan.steps) {
        if (step.tool !== "web.extract") {
          continue;
        }

        const parsed = parseWebExtractInput({
          rawInput: step.input,
          rawInstruction: step.instruction
        });
        if (!parsed.ok) {
          continue;
        }

        const host = parsed.host.toLowerCase();
        if (existingPolicyHosts.has(host) || proposedPolicyHosts.has(host)) {
          continue;
        }

        const requestedLevel = parsed.input.level ?? "public";
        const policyRecord: WebPolicy = {
          host,
          level: requestedLevel,
          approvedAt: new Date().toISOString(),
          approvedBy: "agent",
          notes: `Planned via ${agent.name}`
        };

        proposals.push({
          id: crypto.randomUUID(),
          ts: new Date().toISOString(),
          object: { kind: "web_policy", id: host },
          summary: `Allow web access to ${host} at ${WEB_LEVEL_LABELS[requestedLevel]} level`,
          diff: diffObjects(null, policyRecord),
          applyMode: plan.mode,
          requiresConfirm: true,
          proposedBy: { type: "agent", id: agent.id },
          patch: {
            after: policyRecord as unknown as Record<string, unknown>
          }
        });
        proposedPolicyHosts.add(host);
      }

      return proposals;
    },
    [agents, filePolicies, missions, skillInstalls, webPolicies, workspaces]
  );

  const buildExecutorPrompt = useCallback((agent: Agent, prompt: string): string => {
    const styleInstruction =
      agent.tone === "detailed"
        ? "Reply in a detailed style with clear context and explanation."
        : "Reply in a concise style with short, clear points.";
    const addressingInstruction = agent.preferredName
      ? `Address the user as ${agent.preferredName}.`
      : "";

    return [prompt, "", `Response style: ${styleInstruction}`, addressingInstruction]
      .filter(Boolean)
      .join("\n");
  }, []);

  const streamPlanStep = useCallback(
    async (runId: string, agentId: string, prompt: string, model: string): Promise<StreamWaiterResult> =>
      new Promise((resolve, reject) => {
        chatRunMetaRef.current[runId] = {
          startedAt: performance.now(),
          prompt,
          model,
          agentId
        };

        streamWaitersRef.current[runId] = { resolve, reject };
        setActiveChatRunId(runId);

        void invoke("llm_stream_start", {
          runId,
          agentId,
          prompt
        }).catch((error) => {
          delete streamWaitersRef.current[runId];
          setActiveChatRunId((current) => (current === runId ? null : current));
          reject(invokeErrorMessage(error, "Unable to start streaming step."));
        });
      }),
    []
  );

  const runDirectAnswer = useCallback(
    async (input: {
      runId: string;
      agent: Agent;
      prompt: string;
      rawPlanText: string;
      plannerAttempts: number;
      plannerErrors: string[];
    }) => {
      const model = effectiveOpenAiCompatModel(input.agent, appSettings);
      const styledPrompt = buildExecutorPrompt(input.agent, input.prompt);

      updatePlannedRun(input.runId, (current) => ({
        ...current,
        status: "executing_direct",
        rawPlanText: input.rawPlanText,
        planningError: input.plannerErrors.join(" | ") || null,
        plannerAttempts: input.plannerAttempts,
        plannerErrors: input.plannerErrors,
        plan: null,
        stepStates: [],
        configProposals: [],
        autoRunEligible: true,
        runRequested: true
      }));

      setRuns((previous) =>
        previous.map((run) =>
          run.id === input.runId
            ? {
                ...run,
                status: "executing",
                summary: "Answering directly.",
                logs: run.logs.concat("Planner fallback: answering directly.")
              }
            : run
        )
      );

      try {
        const result = await streamPlanStep(input.runId, input.agent.id, styledPrompt, model);
        if (result.cancelled) {
          updatePlannedRun(input.runId, (current) => ({
            ...current,
            status: "cancelled"
          }));
          setRuns((previous) =>
            previous.map((run) =>
              run.id === input.runId
                ? {
                    ...run,
                    status: "cancelled",
                    finishedAt: new Date().toISOString(),
                    summary: "Run cancelled by user."
                  }
                : run
            )
          );
          return;
        }

        updatePlannedRun(input.runId, (current) => ({
          ...current,
          status: "completed"
        }));
        setRuns((previous) =>
          previous.map((run) =>
            run.id === input.runId
              ? {
                  ...run,
                  status: "completed",
                  finishedAt: new Date().toISOString(),
                  summary: "Completed (direct).",
                  logs: run.logs.concat("Direct response completed.")
                }
              : run
          )
        );
      } catch (error) {
        const message = invokeErrorMessage(error, "Unable to respond right now.");
        updatePlannedRun(input.runId, (current) => ({
          ...current,
          status: "failed"
        }));
        setRuns((previous) =>
          previous.map((run) =>
            run.id === input.runId
              ? {
                  ...run,
                  status: "failed",
                  finishedAt: new Date().toISOString(),
                  summary: message,
                  logs: run.logs.concat(message)
                }
              : run
          )
        );
        setChatError(message);
      }
    },
    [appSettings, buildExecutorPrompt, streamPlanStep, updatePlannedRun]
  );

  const runNonStreamingProviderReply = useCallback(
    async (input: { runId: string; agent: Agent; prompt: string }): Promise<void> => {
      if (input.agent.provider === "openai_compat") {
        throw new Error("Non-streaming provider runner only supports Gemini and Claude.");
      }

      const startedAt = performance.now();
      const resolvedModel = resolveModel(modelProviderForAgent(input.agent.provider), appSettings);
      const styledPrompt = buildExecutorPrompt(input.agent, input.prompt);
      const commandName =
        input.agent.provider === "google_gemini" ? "gemini_generate" : "claude_generate";
      setActiveNonStreamingRunId(input.runId);

      try {
        const responseText = await invoke<string>(commandName, {
          prompt: styledPrompt,
          model: resolvedModel
        });

        runOutputBufferRef.current[input.runId] = responseText;
        const missionContext = missionRunContextRef.current[input.runId] ?? null;
        if (shouldShowMissionLiveOutput(input.runId)) {
          setChatMessages((previous) =>
            previous.concat({
              id: crypto.randomUUID(),
              runId: input.runId,
              agentId: input.agent.id,
              role: "assistant",
              content: responseText,
              createdAt: new Date().toISOString()
            })
          );
        }

        updatePlannedRun(input.runId, (current) => ({
          ...current,
          status: "completed"
        }));
        setRuns((previous) =>
          previous.map((run) =>
            run.id === input.runId
              ? {
                  ...run,
                  status: "completed",
                  finishedAt: new Date().toISOString(),
                  summary: `Reply completed via ${AGENT_PROVIDER_LABELS[input.agent.provider]}.`,
                  logs: run.logs.concat(
                    `Completed via ${AGENT_PROVIDER_LABELS[input.agent.provider]}.`,
                    ...(missionContext ? [`Output:\n${responseText || "(empty)"}`] : [])
                  )
                }
              : run
          )
        );

        await logUsageEvent(
          buildUsageEvent({
            agentId: input.agent.id,
            runId: input.runId,
            provider: input.agent.provider,
            model: resolvedModel,
            kind: "llm",
            inputChars: styledPrompt.length,
            outputChars: responseText.length,
            latencyMs: performance.now() - startedAt
          })
        );
      } catch (error) {
        const message = invokeErrorMessage(error, "Unable to generate response.");
        updatePlannedRun(input.runId, (current) => ({
          ...current,
          status: "failed",
          planningError: message
        }));
        setRuns((previous) =>
          previous.map((run) =>
            run.id === input.runId
              ? {
                  ...run,
                  status: "failed",
                  finishedAt: new Date().toISOString(),
                  summary: message,
                  logs: run.logs.concat(message)
                }
              : run
          )
        );
        setChatError(message);
      } finally {
        setActiveNonStreamingRunId((current) => (current === input.runId ? null : current));
        if (!missionRunContextRef.current[input.runId]) {
          delete runOutputBufferRef.current[input.runId];
        }
      }
    },
    [
      appSettings,
      buildExecutorPrompt,
      buildUsageEvent,
      logUsageEvent,
      shouldShowMissionLiveOutput,
      updatePlannedRun
    ]
  );

  const executeWebExtractStep = useCallback(
    async (input: {
      runId: string;
      agentId: string;
      step: PlannerStep;
    }): Promise<ExecuteWebExtractResult> => {
      const parsed = parseWebExtractInput({
        rawInput: input.step.input,
        rawInstruction: input.step.instruction
      });
      if (!parsed.ok) {
        return {
          ok: false,
          reason: "invalid_input",
          message: parsed.error
        };
      }

      const policy = webPolicyByHost.get(parsed.host);
      const requestedLevel = parsed.input.level ?? "public";
      if (!policy) {
        return {
          ok: false,
          reason: "missing_policy",
          host: parsed.host,
          requestedLevel,
          message: `Web access to ${parsed.host} is not approved yet.`
        };
      }

      if (!isPolicyPathAllowed(policy, parsed.pathname)) {
        return {
          ok: false,
          reason: "path_blocked",
          host: parsed.host,
          requestedLevel,
          message: `Path ${parsed.pathname} is outside approved scope for ${parsed.host}.`
        };
      }

      const level = getEffectiveWebLevel(parsed.input.level, policy);
      const startedAt = performance.now();

      try {
        let html = "";
        let statusNote = "OK";

        if (level === "browser") {
          const response = await invoke<PwFetchRenderedResponse>("pw_fetch_rendered", {
            url: parsed.url.toString()
          });
          html = response.html;
          statusNote = "Browser rendered";
        } else if (level === "auth") {
          const response = await invoke<WebFetchResponse>("web_fetch_auth", {
            url: parsed.url.toString(),
            host: parsed.host
          });
          html = response.html;
          statusNote = `HTTP ${response.status}`;
        } else {
          const response = await invoke<WebFetchResponse>("web_fetch_public", {
            url: parsed.url.toString()
          });
          html = response.html;
          statusNote = `HTTP ${response.status}`;
        }

        const extracted = extractWebDocument({
          html,
          url: parsed.url.toString(),
          host: parsed.host,
          level
        });
        const htmlBytes = new TextEncoder().encode(html).length;

        await logUsageEvent(
          buildUsageEvent({
            agentId: input.agentId,
            runId: input.runId,
            provider: "web",
            model: null,
            kind: "web",
            inputChars: parsed.url.toString().length,
            outputChars: extracted.text.length,
            latencyMs: performance.now() - startedAt,
            tags: {
              level: usageTagLevel(level),
              bytes: htmlBytes
            }
          })
        );

        return {
          ok: true,
          host: parsed.host,
          level,
          title: extracted.title,
          text: extracted.text,
          markdown: extracted.markdown,
          statusNote
        };
      } catch (error) {
        return {
          ok: false,
          reason: "fetch_failed",
          host: parsed.host,
          requestedLevel: level,
          message: invokeErrorMessage(error, "Web access request failed.")
        };
      }
    },
    [buildUsageEvent, logUsageEvent, webPolicyByHost]
  );

  const executeQuickWebExtract = useCallback(
    async (input: { runId: string; agent: Agent; url: string }): Promise<void> => {
      setActiveChatRunId(input.runId);
      try {
        const result = await webExtract(input.url, input.agent.id, {
          policyByHost: webPolicyByHost,
          buildApprovalProposal: (host) =>
            buildWebPolicyProposal({
              host,
              level: "public",
              approvedBy: "agent",
              notes: `Requested by ${input.agent.name}`,
              applyMode: autonomyMode,
              proposedBy: { type: "agent", id: input.agent.id },
              summary: `Allow web access to ${host} at ${WEB_LEVEL_LABELS.public} level`
            }),
          invokeFetchPublic: (url) =>
            invoke<WebFetchResponse>("web_fetch_public", {
              url
            }),
          onUsage: async (usage) => {
            await logUsageEvent(
              buildUsageEvent({
                agentId: input.agent.id,
                runId: input.runId,
                provider: "web",
                model: null,
                kind: "web",
                inputChars: usage.inputChars,
                outputChars: usage.outputChars,
                latencyMs: usage.latencyMs,
                tags: {
                  level: "standard",
                  bytes: usage.bytes
                }
              })
            );
          }
        });

        if (!result.ok) {
          if (result.reason === "requires_approval") {
            setPendingQuickExtractApproval({
              runId: input.runId,
              agentId: input.agent.id,
              url: input.url,
              proposal: result.proposal
            });
            setRuns((previous) =>
              previous.map((run) =>
                run.id === input.runId
                  ? {
                      ...run,
                      status: "waiting_for_approval",
                      summary: result.message,
                      logs: run.logs.concat(result.message)
                    }
                  : run
              )
            );
            setChatMessages((previous) =>
              previous.concat({
                id: crypto.randomUUID(),
                runId: input.runId,
                agentId: input.agent.id,
                role: "assistant",
                content: `Approval required: allow ${result.host} (${WEB_LEVEL_LABELS.public}).`,
                createdAt: new Date().toISOString()
              })
            );
            return;
          }

          setChatError(result.message);
          setRuns((previous) =>
            previous.map((run) =>
              run.id === input.runId
                ? {
                    ...run,
                    status: "failed",
                    finishedAt: new Date().toISOString(),
                    summary: result.message,
                    logs: run.logs.concat(result.message)
                  }
                : run
            )
          );
          return;
        }

        setPendingQuickExtractApproval((current) =>
          current?.runId === input.runId ? null : current
        );
        setRuns((previous) =>
          previous.map((run) =>
            run.id === input.runId
              ? {
                  ...run,
                  status: "completed",
                  finishedAt: new Date().toISOString(),
                  summary: `Extracted ${result.meta.host}`,
                  logs: run.logs.concat(
                    `Fetched ${result.meta.url} (${result.meta.status}, ${result.meta.bytes} bytes) in ${result.usage.latencyMs}ms.`
                  )
                }
              : run
          )
        );

        const previewText =
          result.text.length > 1600 ? `${result.text.slice(0, 1600)}...` : result.text;
        const contentLines = [
          `Web Access (Standard): ${result.meta.host}`,
          result.title ? `Title: ${result.title}` : null,
          `Result: ${previewText}`
        ].filter(Boolean);
        setChatMessages((previous) =>
          previous.concat({
            id: crypto.randomUUID(),
            runId: input.runId,
            agentId: input.agent.id,
            role: "assistant",
            content: contentLines.join("\n"),
            createdAt: new Date().toISOString()
          })
        );
      } finally {
        setActiveChatRunId((current) => (current === input.runId ? null : current));
      }
    },
    [autonomyMode, buildUsageEvent, buildWebPolicyProposal, logUsageEvent, webPolicyByHost]
  );

  const applyQuickExtractProposal = useCallback(async () => {
    if (!pendingQuickExtractApproval) {
      return;
    }

    const pending = pendingQuickExtractApproval;
    try {
      await applyProposal(
        pending.proposal,
        { type: "user", id: sessionEmail ?? undefined },
        pending.proposal.summary
      );
      setPendingQuickExtractApproval(null);

      const agent = agents.find((item) => item.id === pending.agentId);
      if (!agent) {
        setRuns((previous) =>
          previous.map((run) =>
            run.id === pending.runId
              ? {
                  ...run,
                  status: "failed",
                  finishedAt: new Date().toISOString(),
                  summary: "Agent is no longer available.",
                  logs: run.logs.concat("Web extract failed: agent not found after approval.")
                }
              : run
          )
        );
        return;
      }

      setRuns((previous) =>
        previous.map((run) =>
          run.id === pending.runId
            ? {
                ...run,
                status: "executing",
                summary: "Approval granted. Extracting now...",
                logs: run.logs.concat("Web access approved. Continuing extraction.")
              }
            : run
        )
      );

      await executeQuickWebExtract({
        runId: pending.runId,
        agent,
        url: pending.url
      });
    } catch {
      setChatError("Unable to apply web access approval.");
    }
  }, [agents, applyProposal, executeQuickWebExtract, pendingQuickExtractApproval, sessionEmail]);

  const cancelQuickExtractProposal = useCallback(() => {
    if (!pendingQuickExtractApproval) {
      return;
    }

    const pending = pendingQuickExtractApproval;
    setPendingQuickExtractApproval(null);
    setRuns((previous) =>
      previous.map((run) =>
        run.id === pending.runId
          ? {
              ...run,
              status: "cancelled",
              finishedAt: new Date().toISOString(),
              summary: "Web access approval was cancelled.",
              logs: run.logs.concat("Web access approval cancelled by user.")
            }
          : run
      )
    );
  }, [pendingQuickExtractApproval]);

  const executeQuickFileOperation = useCallback(
    async (input: { runId: string; agent: Agent; operation: QuickFileOperation }): Promise<void> => {
      setActiveChatRunId(input.runId);
      try {
        if (input.operation.kind === "read") {
          const result = await fileReadTool(input.operation.path, {
            policies: activeFilePolicies,
            invokeRead: (path, maxBytes) =>
              invoke<FileReadResponse>("file_read", {
                path,
                maxBytes
              }),
            buildApprovalProposal: (folderPath, mode) =>
              buildFilePolicyProposal({
                path: folderPath,
                mode,
                approvedBy: "agent",
                applyMode: autonomyMode,
                proposedBy: { type: "agent", id: input.agent.id },
                summary: `Allow file access to ${folderPath}`
              })
          });

          if (!result.ok) {
            if (result.reason === "requires_approval") {
              setPendingQuickFileApproval({
                runId: input.runId,
                agentId: input.agent.id,
                operation: input.operation,
                proposal: result.proposal
              });
              setRuns((previous) =>
                previous.map((run) =>
                  run.id === input.runId
                    ? {
                        ...run,
                        status: "waiting_for_approval",
                        summary: result.message,
                        logs: run.logs.concat(result.message)
                      }
                    : run
                )
              );
              setChatMessages((previous) =>
                previous.concat({
                  id: crypto.randomUUID(),
                  runId: input.runId,
                  agentId: input.agent.id,
                  role: "assistant",
                  content: `Approval required: allow folder ${result.folderPath}.`,
                  createdAt: new Date().toISOString()
                })
              );
              return;
            }

            setChatError(result.message);
            setRuns((previous) =>
              previous.map((run) =>
                run.id === input.runId
                  ? {
                      ...run,
                      status: "failed",
                      finishedAt: new Date().toISOString(),
                      summary: result.message,
                      logs: run.logs.concat(result.message)
                    }
                  : run
              )
            );
            return;
          }

          await logUsageEvent(
            buildUsageEvent({
              agentId: input.agent.id,
              runId: input.runId,
              provider: "filesystem",
              model: null,
              kind: "file",
              inputChars: input.operation.path.length,
              outputChars: result.bytes,
              latencyMs: result.latencyMs,
              tags: {
                op: "read",
                bytes: result.bytes
              }
            })
          );

          const preview = result.text.length > 1600 ? `${result.text.slice(0, 1600)}...` : result.text;
          setChatMessages((previous) =>
            previous.concat({
              id: crypto.randomUUID(),
              runId: input.runId,
              agentId: input.agent.id,
              role: "assistant",
              content: [`Read: ${result.path}`, `Result: ${preview}`].join("\n"),
              createdAt: new Date().toISOString()
            })
          );
          setRuns((previous) =>
            previous.map((run) =>
              run.id === input.runId
                ? {
                    ...run,
                    status: "completed",
                    finishedAt: new Date().toISOString(),
                    summary: `Read ${result.path}`,
                    logs: run.logs.concat(`Read ${result.path} (${result.bytes} bytes).`)
                  }
                : run
            )
          );
          return;
        }

        const writeResult = await fileWriteTool(
          {
            path: input.operation.path,
            text: input.operation.text,
            createIfMissing: input.operation.createIfMissing ?? true
          },
          {
            policies: activeFilePolicies,
            invokeExists: (path) => invoke<boolean>("file_exists", { path }),
            invokeWrite: (path, text, createIfMissing, maxBytes) =>
              invoke<FileWriteResponse>("file_write", {
                path,
                text,
                createIfMissing,
                maxBytes
              }),
            buildApprovalProposal: (folderPath, mode) =>
              buildFilePolicyProposal({
                path: folderPath,
                mode,
                approvedBy: "agent",
                applyMode: autonomyMode,
                proposedBy: { type: "agent", id: input.agent.id },
                summary: `Allow file access to ${folderPath}`
              }),
            requireOverwriteApproval: autonomyMode === "autopilot",
            allowOverwrite: Boolean(input.operation.allowOverwrite)
          }
        );

        if (!writeResult.ok) {
          if (writeResult.reason === "requires_approval") {
            setPendingQuickFileApproval({
              runId: input.runId,
              agentId: input.agent.id,
              operation: input.operation,
              proposal: writeResult.proposal
            });
            setRuns((previous) =>
              previous.map((run) =>
                run.id === input.runId
                  ? {
                      ...run,
                      status: "waiting_for_approval",
                      summary: writeResult.message,
                      logs: run.logs.concat(writeResult.message)
                    }
                  : run
              )
            );
            setChatMessages((previous) =>
              previous.concat({
                id: crypto.randomUUID(),
                runId: input.runId,
                agentId: input.agent.id,
                role: "assistant",
                content: `Approval required: allow folder ${writeResult.folderPath}.`,
                createdAt: new Date().toISOString()
              })
            );
            return;
          }

          if (writeResult.reason === "overwrite_requires_approval") {
            const confirmed = window.confirm("This file already exists. Overwrite it?");
            if (confirmed) {
              const forcedWrite = await fileWriteTool(
                {
                  path: input.operation.path,
                  text: input.operation.text,
                  createIfMissing: input.operation.createIfMissing ?? true
                },
                {
                  policies: activeFilePolicies,
                  invokeExists: (path) => invoke<boolean>("file_exists", { path }),
                  invokeWrite: (path, text, createIfMissing, maxBytes) =>
                    invoke<FileWriteResponse>("file_write", {
                      path,
                      text,
                      createIfMissing,
                      maxBytes
                    }),
                  buildApprovalProposal: (folderPath, mode) =>
                    buildFilePolicyProposal({
                      path: folderPath,
                      mode,
                      approvedBy: "agent",
                      applyMode: autonomyMode,
                      proposedBy: { type: "agent", id: input.agent.id },
                      summary: `Allow file access to ${folderPath}`
                    }),
                  requireOverwriteApproval: false,
                  allowOverwrite: true
                }
              );

              if (!forcedWrite.ok) {
                setChatError(forcedWrite.message);
                setRuns((previous) =>
                  previous.map((run) =>
                    run.id === input.runId
                      ? {
                          ...run,
                          status: "failed",
                          finishedAt: new Date().toISOString(),
                          summary: forcedWrite.message,
                          logs: run.logs.concat(forcedWrite.message)
                        }
                      : run
                  )
                );
                return;
              }

              await logUsageEvent(
                buildUsageEvent({
                  agentId: input.agent.id,
                  runId: input.runId,
                  provider: "filesystem",
                  model: null,
                  kind: "file",
                  inputChars: input.operation.path.length,
                  outputChars: forcedWrite.bytesWritten,
                  latencyMs: forcedWrite.latencyMs,
                  tags: {
                    op: "write",
                    bytes: forcedWrite.bytesWritten
                  }
                })
              );
              setChatMessages((previous) =>
                previous.concat({
                  id: crypto.randomUUID(),
                  runId: input.runId,
                  agentId: input.agent.id,
                  role: "assistant",
                  content: `Wrote ${forcedWrite.bytesWritten} bytes to ${forcedWrite.path}.`,
                  createdAt: new Date().toISOString()
                })
              );
              setRuns((previous) =>
                previous.map((run) =>
                  run.id === input.runId
                    ? {
                        ...run,
                        status: "completed",
                        finishedAt: new Date().toISOString(),
                        summary: `Wrote ${forcedWrite.path}`,
                        logs: run.logs.concat(
                          `Wrote ${forcedWrite.bytesWritten} bytes to ${forcedWrite.path}.`
                        )
                      }
                    : run
                )
              );
            } else {
              setRuns((previous) =>
                previous.map((run) =>
                  run.id === input.runId
                    ? {
                        ...run,
                        status: "cancelled",
                        finishedAt: new Date().toISOString(),
                        summary: "Write cancelled.",
                        logs: run.logs.concat("Write cancelled by user.")
                      }
                    : run
                )
              );
            }
            return;
          }

          setChatError(writeResult.message);
          setRuns((previous) =>
            previous.map((run) =>
              run.id === input.runId
                ? {
                    ...run,
                    status: "failed",
                    finishedAt: new Date().toISOString(),
                    summary: writeResult.message,
                    logs: run.logs.concat(writeResult.message)
                  }
                : run
            )
          );
          return;
        }

        await logUsageEvent(
          buildUsageEvent({
            agentId: input.agent.id,
            runId: input.runId,
            provider: "filesystem",
            model: null,
            kind: "file",
            inputChars: input.operation.path.length,
            outputChars: writeResult.bytesWritten,
            latencyMs: writeResult.latencyMs,
            tags: {
              op: "write",
              bytes: writeResult.bytesWritten
            }
          })
        );
        setChatMessages((previous) =>
          previous.concat({
            id: crypto.randomUUID(),
            runId: input.runId,
            agentId: input.agent.id,
            role: "assistant",
            content: `Wrote ${writeResult.bytesWritten} bytes to ${writeResult.path}.`,
            createdAt: new Date().toISOString()
          })
        );
        setRuns((previous) =>
          previous.map((run) =>
            run.id === input.runId
              ? {
                  ...run,
                  status: "completed",
                  finishedAt: new Date().toISOString(),
                  summary: `Wrote ${writeResult.path}`,
                  logs: run.logs.concat(
                    `Wrote ${writeResult.bytesWritten} bytes to ${writeResult.path}.`
                  )
                }
              : run
          )
        );
      } finally {
        setActiveChatRunId((current) => (current === input.runId ? null : current));
      }
    },
    [
      activeFilePolicies,
      autonomyMode,
      buildFilePolicyProposal,
      buildUsageEvent,
      logUsageEvent
    ]
  );

  const applyQuickFileProposal = useCallback(async () => {
    if (!pendingQuickFileApproval) {
      return;
    }

    const pending = pendingQuickFileApproval;
    try {
      await applyProposal(
        pending.proposal,
        { type: "user", id: sessionEmail ?? undefined },
        pending.proposal.summary
      );
      setPendingQuickFileApproval(null);

      const agent = agents.find((item) => item.id === pending.agentId);
      if (!agent) {
        setRuns((previous) =>
          previous.map((run) =>
            run.id === pending.runId
              ? {
                  ...run,
                  status: "failed",
                  finishedAt: new Date().toISOString(),
                  summary: "Agent is no longer available.",
                  logs: run.logs.concat("File operation failed: agent not found after approval.")
                }
              : run
          )
        );
        return;
      }

      setRuns((previous) =>
        previous.map((run) =>
          run.id === pending.runId
            ? {
                ...run,
                status: "executing",
                summary: "Approval granted. Continuing...",
                logs: run.logs.concat("Folder access approved. Continuing operation.")
              }
            : run
        )
      );

      await executeQuickFileOperation({
        runId: pending.runId,
        agent,
        operation: pending.operation
      });
    } catch {
      setChatError("Unable to apply file access approval.");
    }
  }, [agents, applyProposal, executeQuickFileOperation, pendingQuickFileApproval, sessionEmail]);

  const cancelQuickFileProposal = useCallback(() => {
    if (!pendingQuickFileApproval) {
      return;
    }
    const pending = pendingQuickFileApproval;
    setPendingQuickFileApproval(null);
    setRuns((previous) =>
      previous.map((run) =>
        run.id === pending.runId
          ? {
              ...run,
              status: "cancelled",
              finishedAt: new Date().toISOString(),
              summary: "Folder approval cancelled.",
              logs: run.logs.concat("Folder access approval cancelled by user.")
            }
          : run
      )
    );
  }, [pendingQuickFileApproval]);

  const executePlannedRun = useCallback(
    async (runId: string): Promise<void> => {
      const planned = plannedRunsRef.current[runId];
      if (!planned?.plan) {
        return;
      }

      const agent = agents.find((item) => item.id === planned.agentId);
      if (!agent) {
        setChatError("Agent for planned run was not found.");
        return;
      }

      const model = effectiveOpenAiCompatModel(agent, appSettings);
      const allowLiveOutput = shouldShowMissionLiveOutput(runId);
      updatePlannedRun(runId, (current) => ({
        ...current,
        status: "executing",
        runRequested: true
      }));

      setRuns((previous) =>
        previous.map((run) =>
          run.id === runId
            ? {
                ...run,
                status: "executing",
                summary: "Executing plan steps.",
                logs: run.logs.concat("Plan execution started.")
              }
            : run
        )
      );

      if (allowLiveOutput) {
        setChatMessages((previous) => {
          const hasAssistant = previous.some(
            (entry) => entry.runId === runId && entry.role === "assistant"
          );
          if (hasAssistant) {
            return previous;
          }
          return previous.concat({
            id: crypto.randomUUID(),
            runId,
            agentId: planned.agentId,
            role: "assistant",
            content: "",
            createdAt: new Date().toISOString()
          });
        });
      }

      for (const [index, step] of planned.plan.steps.entries()) {
        const retryConfig = getStepRetryConfig(step);
        const totalAttempts = retryConfig.retries + 1;
        let completed = false;
        let safeFailureHandled = false;

        for (let attempt = 1; attempt <= totalAttempts; attempt += 1) {
          const isLastAttempt = attempt === totalAttempts;
          updatePlannedRun(runId, (current) => ({
            ...current,
            stepStates: current.stepStates.map((state) =>
              state.index === index
                ? {
                    ...state,
                    status: "running",
                    note: `Attempt ${attempt}/${totalAttempts}`
                  }
                : state
            )
          }));

          setRuns((previous) =>
            previous.map((run) =>
              run.id === runId
                ? {
                    ...run,
                    logs: run.logs.concat(`Step ${index + 1} attempt ${attempt}/${totalAttempts}`)
                  }
                : run
            )
          );

          if (step.tool !== "llm.generate" && step.tool !== "web.extract") {
            const message = `${step.tool} is not implemented.`;
            updatePlannedRun(runId, (current) => ({
              ...current,
              stepStates: current.stepStates.map((state) =>
                state.index === index
                  ? {
                      ...state,
                      status: "failed",
                      note: message
                    }
                  : state
              )
            }));

            if (step.safe === true) {
              safeFailureHandled = true;
              setRuns((previous) =>
                previous.map((run) =>
                  run.id === runId
                    ? {
                        ...run,
                        logs: run.logs.concat(`Step ${index + 1} failed safely: ${message}`)
                      }
                    : run
                )
              );
              break;
            }

            updatePlannedRun(runId, (current) => ({ ...current, status: "failed" }));
            setRuns((previous) =>
              previous.map((run) =>
                run.id === runId
                  ? {
                      ...run,
                      status: "failed",
                      finishedAt: new Date().toISOString(),
                      summary: message,
                      logs: run.logs.concat(message)
                    }
                  : run
              )
            );
            setChatError(message);
            return;
          }

          if (step.tool === "web.extract") {
            const webResult = await executeWebExtractStep({
              runId,
              agentId: planned.agentId,
              step
            });

            if (!webResult.ok) {
              if (
                webResult.reason === "missing_policy" &&
                webResult.host &&
                webResult.requestedLevel
              ) {
                const requestedLevel = webResult.requestedLevel;
                const existingProposal = plannedRunsRef.current[runId]?.configProposals.find(
                  (entry) => entry.object.kind === "web_policy" && entry.object.id === webResult.host
                );
                const proposal =
                  existingProposal ??
                  buildWebPolicyProposal({
                    host: webResult.host,
                    level: requestedLevel,
                    approvedBy: "agent",
                    notes: `Requested by ${agent.name}`,
                    applyMode: planned.plan.mode,
                    proposedBy: { type: "agent", id: agent.id },
                    summary: `Allow web access to ${webResult.host} at ${WEB_LEVEL_LABELS[requestedLevel]} level`
                  });

                updatePlannedRun(runId, (current) => ({
                  ...current,
                  status: "waiting_for_approval",
                  runRequested: false,
                  configProposals: current.configProposals.some(
                    (entry) => entry.object.kind === "web_policy" && entry.object.id === webResult.host
                  )
                    ? current.configProposals
                    : current.configProposals.concat(proposal),
                  stepStates: current.stepStates.map((state) =>
                    state.index === index
                      ? {
                          ...state,
                          status: "failed",
                          note: `Approval required for ${webResult.host}`
                        }
                      : state
                  )
                }));
                setPendingWebApprovals((previous) =>
                  previous.some(
                    (entry) =>
                      entry.runId === runId &&
                      entry.stepIndex === index &&
                      entry.proposalId === proposal.id
                  )
                    ? previous
                    : previous.concat({ runId, stepIndex: index, proposalId: proposal.id })
                );
                setRuns((previous) =>
                  previous.map((run) =>
                    run.id === runId
                      ? {
                          ...run,
                          status: "waiting_for_approval",
                          summary: `Approval required for ${webResult.host}`,
                          logs: run.logs.concat(
                            `Step ${index + 1} requires approval for ${webResult.host}.`
                          )
                        }
                      : run
                  )
                );
                if (allowLiveOutput) {
                  setChatMessages((previous) =>
                    previous.concat({
                      id: crypto.randomUUID(),
                      runId,
                      agentId: planned.agentId,
                      role: "assistant",
                      content: `Approval required: allow ${webResult.host} (${WEB_LEVEL_LABELS[requestedLevel]}).`,
                      createdAt: new Date().toISOString()
                    })
                  );
                }
                return;
              }

              const message = webResult.message;
              if (!isLastAttempt && webResult.reason === "fetch_failed") {
                const delay = retryConfig.backoffMs * 2 ** (attempt - 1);
                setRuns((previous) =>
                  previous.map((run) =>
                    run.id === runId
                      ? {
                          ...run,
                          logs: run.logs.concat(
                            `Step ${index + 1} failed: ${message}. Retrying in ${delay}ms.`
                          )
                        }
                      : run
                  )
                );
                await sleep(delay);
                continue;
              }

              updatePlannedRun(runId, (current) => ({
                ...current,
                status: "failed",
                stepStates: current.stepStates.map((state) =>
                  state.index === index
                    ? {
                        ...state,
                        status: "failed",
                        note: message
                      }
                    : state
                )
              }));
              setRuns((previous) =>
                previous.map((run) =>
                  run.id === runId
                    ? {
                        ...run,
                        status: "failed",
                        finishedAt: new Date().toISOString(),
                        summary: message,
                        logs: run.logs.concat(message)
                      }
                    : run
                )
              );
              setChatError(message);
              return;
            }

            const excerpt = webResult.text.slice(0, 900);
            runOutputBufferRef.current[runId] =
              `${runOutputBufferRef.current[runId] ?? ""}\n${webResult.text}`.trim();
            if (allowLiveOutput) {
              setChatMessages((previous) => {
                const existingAssistantIndex = previous.findIndex(
                  (entry) => entry.runId === runId && entry.role === "assistant"
                );
                const block = [
                  `Web Access (${WEB_LEVEL_LABELS[webResult.level]}): ${webResult.host}`,
                  webResult.title ? `Title: ${webResult.title}` : null,
                  `Result: ${excerpt}${webResult.text.length > excerpt.length ? "..." : ""}`
                ]
                  .filter(Boolean)
                  .join("\n");

                if (existingAssistantIndex < 0) {
                  return previous.concat({
                    id: crypto.randomUUID(),
                    runId,
                    agentId: planned.agentId,
                    role: "assistant",
                    content: block,
                    createdAt: new Date().toISOString()
                  });
                }

                const next = [...previous];
                const existing = next[existingAssistantIndex];
                next[existingAssistantIndex] = {
                  ...existing,
                  content: existing.content ? `${existing.content}\n\n${block}` : block
                };
                return next;
              });
            }

            updatePlannedRun(runId, (current) => ({
              ...current,
              stepStates: current.stepStates.map((state) =>
                state.index === index
                  ? {
                      ...state,
                      status: "completed",
                      note: webResult.statusNote
                    }
                  : state
              )
            }));
            setRuns((previous) =>
              previous.map((run) =>
                run.id === runId
                  ? {
                      ...run,
                      logs: run.logs.concat(
                        `Step ${index + 1} completed via web.extract (${webResult.host}, ${WEB_LEVEL_LABELS[webResult.level]}).`
                      )
                    }
                  : run
              )
            );
            completed = true;
            break;
          }

          const stepPrompt = step.input?.trim() || step.instruction?.trim() || planned.prompt;
          const styledStepPrompt = buildExecutorPrompt(agent, stepPrompt);

          try {
            const streamResult = await streamPlanStep(
              runId,
              planned.agentId,
              styledStepPrompt,
              model
            );

            updatePlannedRun(runId, (current) => ({
              ...current,
              stepStates: current.stepStates.map((state) =>
                state.index === index
                  ? {
                      ...state,
                      status: streamResult.cancelled ? "failed" : "completed",
                      note: streamResult.cancelled ? "Cancelled" : undefined
                    }
                  : state
              )
            }));

            setRuns((previous) =>
              previous.map((run) =>
                run.id === runId
                  ? {
                      ...run,
                      logs: run.logs.concat(
                        streamResult.cancelled
                          ? `Step ${index + 1} cancelled.`
                          : `Step ${index + 1} completed.`
                      )
                    }
                  : run
              )
            );

            if (streamResult.cancelled) {
              updatePlannedRun(runId, (current) => ({ ...current, status: "cancelled" }));
              setRuns((previous) =>
                previous.map((run) =>
                  run.id === runId
                    ? {
                        ...run,
                        status: "cancelled",
                        finishedAt: new Date().toISOString(),
                        summary: "Run cancelled by user."
                      }
                    : run
                )
              );
              return;
            }

            completed = true;
            break;
          } catch (error) {
            const message = invokeErrorMessage(error, "Plan step execution failed.");
            if (!isLastAttempt) {
              const delay = retryConfig.backoffMs * 2 ** (attempt - 1);
              setRuns((previous) =>
                previous.map((run) =>
                  run.id === runId
                    ? {
                        ...run,
                        logs: run.logs.concat(
                          `Step ${index + 1} failed: ${message}. Retrying in ${delay}ms.`
                        )
                      }
                    : run
                )
              );
              await sleep(delay);
              continue;
            }

            updatePlannedRun(runId, (current) => ({
              ...current,
              status: "failed",
              stepStates: current.stepStates.map((state) =>
                state.index === index
                  ? {
                      ...state,
                      status: "failed",
                      note: message
                    }
                  : state
              )
            }));
            setRuns((previous) =>
              previous.map((run) =>
                run.id === runId
                  ? {
                      ...run,
                      status: "failed",
                      finishedAt: new Date().toISOString(),
                      summary: message,
                      logs: run.logs.concat(message)
                    }
                  : run
              )
            );
            setChatError(message);
            return;
          }
        }

        if (!completed && !safeFailureHandled) {
          updatePlannedRun(runId, (current) => ({ ...current, status: "failed" }));
          setRuns((previous) =>
            previous.map((run) =>
              run.id === runId
                ? {
                    ...run,
                    status: "failed",
                    finishedAt: new Date().toISOString(),
                    summary: `Step ${index + 1} failed.`,
                    logs: run.logs.concat(`Step ${index + 1} failed.`)
                  }
                : run
            )
          );
          setChatError("Execution failed.");
          return;
        }
      }

      const finalState = plannedRunsRef.current[runId];
      const failedSteps = finalState?.stepStates.filter((state) => state.status === "failed").length ?? 0;
      const missionContext = missionRunContextRef.current[runId] ?? null;
      const bufferedOutput = runOutputBufferRef.current[runId] ?? "";

      updatePlannedRun(runId, (current) => ({ ...current, status: "completed" }));
      setRuns((previous) =>
        previous.map((run) =>
          run.id === runId
            ? {
                ...run,
                status: "completed",
                finishedAt: new Date().toISOString(),
                summary:
                  failedSteps > 0
                    ? `Completed with ${failedSteps} safe failure(s).`
                    : "Completed all planned steps.",
                logs: run.logs.concat([
                  "Plan execution finished.",
                  ...(missionContext &&
                  bufferedOutput &&
                  !run.logs.some((entry) => entry.startsWith("Output:\n"))
                    ? [`Output:\n${bufferedOutput}`]
                    : [])
                ])
              }
            : run
        )
      );
      setPendingWebApprovals((previous) => previous.filter((item) => item.runId !== runId));
    },
    [
      agents,
      appSettings,
      buildExecutorPrompt,
      buildWebPolicyProposal,
      executeWebExtractStep,
      shouldShowMissionLiveOutput,
      streamPlanStep,
      updatePlannedRun
    ]
  );

  const createMissionFromPlannerProposal = useCallback(
    async (agent: Agent, plan: BossClawPlanV1): Promise<Mission | null> => {
      const missionProposal = plan.missionProposal;
      if (!missionProposal || missionProposal.type !== "create_mission") {
        return null;
      }

      const schedule = missionScheduleFromProposal(missionProposal);
      if (!schedule) {
        return null;
      }

      const nowIso = new Date().toISOString();
      const title = missionProposal.title.trim() || "Recurring Mission";
      const goal = missionProposal.goal.trim() || plan.goal;
      const fingerprint = buildMissionFingerprint(agent.id, schedule, goal);
      const existingMission =
        missions.find((mission) => hasMatchingMissionFingerprint(mission, agent.id, schedule, goal)) ?? null;
      if (existingMission) {
        const nextMission: Mission = {
          ...existingMission,
          title,
          goal,
          enabled: true,
          schedule,
          fingerprint,
          nextRunAt: computeNextRunAt(schedule),
          updatedAt: nowIso
        };
        const hasChanges =
          existingMission.title !== nextMission.title ||
          existingMission.goal !== nextMission.goal ||
          existingMission.enabled !== nextMission.enabled ||
          existingMission.fingerprint !== nextMission.fingerprint ||
          existingMission.nextRunAt !== nextMission.nextRunAt ||
          !isMissionScheduleEqual(existingMission.schedule, nextMission.schedule);
        if (hasChanges) {
          const proposal: ConfigChangeProposal = {
            id: crypto.randomUUID(),
            ts: nowIso,
            object: { kind: "mission", id: existingMission.id },
            summary: `Update recurring mission ${nextMission.title}`,
            diff: diffObjects(existingMission, nextMission),
            applyMode: missionProposal.autonomy,
            requiresConfirm: true,
            proposedBy: { type: "agent", id: agent.id },
            patch: {
              after: nextMission as unknown as Record<string, unknown>
            }
          };

          await applyProposal(
            proposal,
            { type: "agent", id: agent.id },
            `Updated recurring mission ${nextMission.title}`
          );
        }
        return nextMission;
      }

      const mission: Mission = {
        id: crypto.randomUUID(),
        agentId: agent.id,
        fingerprint,
        title,
        goal,
        enabled: true,
        chatPosting: "summary",
        collapseRepeats: true,
        schedule,
        nextRunAt: computeNextRunAt(schedule),
        createdAt: nowIso,
        updatedAt: nowIso
      };

      const proposal: ConfigChangeProposal = {
        id: crypto.randomUUID(),
        ts: nowIso,
        object: { kind: "mission", id: mission.id },
        summary: `Create recurring mission ${mission.title}`,
        diff: diffObjects(null, mission),
        applyMode: missionProposal.autonomy,
        requiresConfirm: true,
        proposedBy: { type: "agent", id: agent.id },
        patch: {
          after: mission as unknown as Record<string, unknown>
        }
      };

      await applyProposal(
        proposal,
        { type: "agent", id: agent.id },
        `Created recurring mission ${mission.title}`
      );

      return mission;
    },
    [applyProposal, missions]
  );

  const createMissionFromRecurringPrompt = useCallback(
    async (agent: Agent, prompt: string, mode: "autopilot" | "fsd"): Promise<Mission> => {
      const schedule = inferMissionScheduleFromPrompt(prompt);
      const title = inferMissionTitleFromPrompt(prompt);
      const nowIso = new Date().toISOString();
      const goal = prompt.trim();
      const fingerprint = buildMissionFingerprint(agent.id, schedule, goal);
      const existingMission =
        missions.find((mission) => hasMatchingMissionFingerprint(mission, agent.id, schedule, goal)) ?? null;
      if (existingMission) {
        const nextMission: Mission = {
          ...existingMission,
          title,
          goal,
          enabled: true,
          schedule,
          fingerprint,
          nextRunAt: computeNextRunAt(schedule),
          updatedAt: nowIso
        };
        const hasChanges =
          existingMission.title !== nextMission.title ||
          existingMission.goal !== nextMission.goal ||
          existingMission.enabled !== nextMission.enabled ||
          existingMission.fingerprint !== nextMission.fingerprint ||
          existingMission.nextRunAt !== nextMission.nextRunAt ||
          !isMissionScheduleEqual(existingMission.schedule, nextMission.schedule);
        if (hasChanges) {
          const proposal: ConfigChangeProposal = {
            id: crypto.randomUUID(),
            ts: nowIso,
            object: { kind: "mission", id: existingMission.id },
            summary: `Update recurring mission ${nextMission.title}`,
            diff: diffObjects(existingMission, nextMission),
            applyMode: mode,
            requiresConfirm: true,
            proposedBy: { type: "agent", id: agent.id },
            patch: {
              after: nextMission as unknown as Record<string, unknown>
            }
          };

          await applyProposal(
            proposal,
            { type: "agent", id: agent.id },
            `Updated recurring mission ${nextMission.title}`
          );
        }
        return nextMission;
      }

      const mission: Mission = {
        id: crypto.randomUUID(),
        agentId: agent.id,
        fingerprint,
        title,
        goal,
        enabled: true,
        chatPosting: "summary",
        collapseRepeats: true,
        schedule,
        nextRunAt: computeNextRunAt(schedule),
        createdAt: nowIso,
        updatedAt: nowIso
      };

      const proposal: ConfigChangeProposal = {
        id: crypto.randomUUID(),
        ts: nowIso,
        object: { kind: "mission", id: mission.id },
        summary: `Create recurring mission ${mission.title}`,
        diff: diffObjects(null, mission),
        applyMode: mode,
        requiresConfirm: true,
        proposedBy: { type: "agent", id: agent.id },
        patch: {
          after: mission as unknown as Record<string, unknown>
        }
      };

      await applyProposal(
        proposal,
        { type: "agent", id: agent.id },
        `Created recurring mission ${mission.title}`
      );

      return mission;
    },
    [applyProposal, missions]
  );

  const runPlannerForRun = useCallback(
    async (runId: string, agent: Agent, prompt: string): Promise<void> => {
      const isScheduledMissionRun = Boolean(missionRunContextRef.current[runId]);
      const recurringIntent = isScheduledMissionRun
        ? { detected: false, strongSignal: false }
        : detectRecurringIntent(prompt);
      setPendingWebApprovals((previous) => previous.filter((item) => item.runId !== runId));
      updatePlannedRun(runId, (current) => ({
        ...current,
        status: "planning",
        planningError: null,
        plannerAttempts: 0,
        plannerErrors: [],
        rawPlanText: "",
        plan: null,
        stepStates: [],
        configProposals: [],
        autoRunEligible: false,
        runRequested: false
      }));

      setRuns((previous) =>
        previous.map((run) =>
          run.id === runId
            ? {
                ...run,
                status: "planning",
                summary: "Planning steps from your request...",
                logs: run.logs.concat("Planner started.")
              }
            : run
        )
      );

      const contextSummary = [
        buildPlannerContextSummary(agent, prompt),
        `recurring_intent=${recurringIntent.detected ? "true" : "false"}`,
        `mission_prefix_signal=${recurringIntent.strongSignal ? "strong" : "none"}`
      ].join("\n");
      let rawPlanText = "";
      let plannerAttemptCount = 0;
      const plannerErrors: string[] = [];
      let parsedResult:
        | ReturnType<typeof parseAndValidatePlanText>
        | null = null;

      for (let plannerAttempt = 1; plannerAttempt <= 2; plannerAttempt += 1) {
        plannerAttemptCount = plannerAttempt;
        try {
          rawPlanText = await invoke<string>("llm_plan", {
            agentId: agent.id,
            userMessage: prompt,
            contextSummary
          });
        } catch (plannerInvokeError) {
          const plannerError = invokeErrorMessage(plannerInvokeError, "Planner unavailable.");
          plannerErrors.push(plannerError);
          updatePlannedRun(runId, (current) => ({
            ...current,
            plannerAttempts: plannerAttempt,
            plannerErrors: [...plannerErrors],
            planningError: plannerError
          }));
          setRuns((previous) =>
            previous.map((run) =>
              run.id === runId
                ? {
                    ...run,
                    logs: run.logs.concat(
                      `Planner attempt ${plannerAttempt} failed: ${plannerError}`
                    )
                  }
                : run
            )
          );
          continue;
        }

        const parsed = parseAndValidatePlanText(rawPlanText);
        if (parsed.ok) {
          parsedResult = parsed;
          break;
        }
        plannerErrors.push(parsed.errors.join(" | "));
        updatePlannedRun(runId, (current) => ({
          ...current,
          plannerAttempts: plannerAttempt,
          plannerErrors: [...plannerErrors],
          planningError: plannerErrors[plannerErrors.length - 1] ?? null,
          rawPlanText
        }));

        setRuns((previous) =>
          previous.map((run) =>
            run.id === runId
              ? {
                  ...run,
                  logs: run.logs.concat(
                    `Planner output invalid on attempt ${plannerAttempt}: ${parsed.errors.join(" | ")}`
                  )
                }
              : run
          )
        );
      }

      if (!parsedResult || !parsedResult.ok) {
        if (recurringIntent.detected) {
          try {
            const fallbackMission = await createMissionFromRecurringPrompt(
              agent,
              prompt,
              "autopilot"
            );
            updatePlannedRun(runId, (current) => ({
              ...current,
              status: "completed",
              planningError: null,
              plannerAttempts: plannerAttemptCount,
              plannerErrors,
              rawPlanText
            }));
            setRuns((previous) =>
              previous.map((run) =>
                run.id === runId
                  ? {
                      ...run,
                      status: "completed",
                      finishedAt: new Date().toISOString(),
                      summary: `Mission created: ${fallbackMission.title}`,
                      logs: run.logs.concat("Recurring intent routed to mission scheduler.")
                    }
                  : run
              )
            );
            setChatMessages((previous) =>
              previous.concat({
                id: crypto.randomUUID(),
                runId,
                agentId: agent.id,
                role: "assistant",
                content: `Mission created: ${fallbackMission.title} (${formatMissionSchedule(fallbackMission.schedule)}).`,
                createdAt: new Date().toISOString()
              })
            );
            return;
          } catch {
            const message = "Unable to create a recurring mission right now.";
            updatePlannedRun(runId, (current) => ({
              ...current,
              status: "failed",
              planningError: message,
              plannerAttempts: plannerAttemptCount,
              plannerErrors: plannerErrors.concat(message),
              rawPlanText
            }));
            setRuns((previous) =>
              previous.map((run) =>
                run.id === runId
                  ? {
                      ...run,
                      status: "failed",
                      finishedAt: new Date().toISOString(),
                      summary: message,
                      logs: run.logs.concat(message)
                    }
                  : run
              )
            );
            setChatError(message);
            return;
          }
        }
        await runDirectAnswer({
          runId,
          agent,
          prompt,
          rawPlanText,
          plannerAttempts: plannerAttemptCount,
          plannerErrors
        });
        return;
      }

      const plan = parsedResult.data;
      let createdMission: Mission | null = null;
      if (!isScheduledMissionRun && plan.missionProposal) {
        try {
          createdMission = await createMissionFromPlannerProposal(agent, plan);
        } catch (missionError) {
          plannerErrors.push(
            invokeErrorMessage(missionError, "Unable to create mission from planner proposal.")
          );
        }
      }
      if (!isScheduledMissionRun && !createdMission && recurringIntent.detected) {
        try {
          createdMission = await createMissionFromRecurringPrompt(agent, prompt, plan.mode);
        } catch (fallbackMissionError) {
          plannerErrors.push(
            invokeErrorMessage(
              fallbackMissionError,
              "Unable to create mission from recurring intent."
            )
          );
        }
      }
      if (!isScheduledMissionRun && recurringIntent.detected && !createdMission) {
        const message = "Unable to create a recurring mission right now.";
        updatePlannedRun(runId, (current) => ({
          ...current,
          status: "failed",
          planningError: message,
          plannerAttempts: plannerAttemptCount,
          plannerErrors: plannerErrors.concat(message),
          rawPlanText: parsedResult.normalizedText,
          plan,
          runRequested: false
        }));
        setRuns((previous) =>
          previous.map((run) =>
            run.id === runId
              ? {
                  ...run,
                  status: "failed",
                  finishedAt: new Date().toISOString(),
                  summary: message,
                  logs: run.logs.concat(message)
                }
              : run
          )
        );
        setChatError(message);
        return;
      }
      if (!isScheduledMissionRun && createdMission) {
        updatePlannedRun(runId, (current) => ({
          ...current,
          status: "completed",
          planningError: null,
          plannerAttempts: plannerAttemptCount,
          plannerErrors,
          rawPlanText: parsedResult.normalizedText,
          plan,
          stepStates: [],
          configProposals: [],
          autoRunEligible: false,
          runRequested: false
        }));
        setRuns((previous) =>
          previous.map((run) =>
            run.id === runId
              ? {
                  ...run,
                  status: "completed",
                  finishedAt: new Date().toISOString(),
                  summary: `Mission created: ${createdMission.title}`,
                  logs: run.logs.concat(
                    `Mission routed to scheduler (${formatMissionSchedule(createdMission.schedule)}).`
                  )
                }
              : run
          )
        );
        setChatMessages((previous) =>
          previous.concat({
            id: crypto.randomUUID(),
            runId,
            agentId: agent.id,
            role: "assistant",
            content: `Mission created: ${createdMission.title} (${formatMissionSchedule(createdMission.schedule)}).`,
            createdAt: new Date().toISOString()
          })
        );
        return;
      }

      const configProposals = buildConfigProposalsFromPlan(plan, agent);
      const stepStates: PlanStepExecution[] = plan.steps.map((step, index) => ({
        index,
        title: step.title,
        tool: step.tool,
        status: "pending"
      }));
      const safeForAutoRun =
        plan.mode === "fsd" &&
        configProposals.length === 0 &&
        (plan.permissionExpansions?.length ?? 0) === 0 &&
        plan.steps.every((step) => isSafePlannerStep(step));

      updatePlannedRun(runId, (current) => ({
        ...current,
        status: "executing",
        planningError: null,
        plannerAttempts: plannerAttemptCount,
        plannerErrors,
        rawPlanText: parsedResult.normalizedText,
        plan,
        stepStates,
        configProposals,
        autoRunEligible: safeForAutoRun,
        runRequested: true
      }));

      setRuns((previous) =>
        previous.map((run) =>
          run.id === runId
            ? {
                ...run,
                status: "executing",
                summary: "Working...",
                logs: run.logs.concat(
                  `Planner produced ${plan.steps.length} step(s).`,
                  configProposals.length
                    ? `Planner included ${configProposals.length} config proposal(s).`
                    : "Planner returned no config proposals."
                )
              }
            : run
        )
      );

      await executePlannedRun(runId);
    },
    [
      buildConfigProposalsFromPlan,
      buildPlannerContextSummary,
      createMissionFromPlannerProposal,
      createMissionFromRecurringPrompt,
      executePlannedRun,
      runDirectAnswer,
      updatePlannedRun
    ]
  );

  const sendChatPrompt = async () => {
    const prompt = chatInput.trim();
    if (!prompt) {
      setChatError("Enter a prompt before sending.");
      return;
    }

    if (!selectedAgent) {
      setChatError("Select an agent first.");
      return;
    }

    const pendingHandshake = pendingHandshakeByAgent[selectedAgent.id] ?? null;
    const pendingHandshakeForFlow: HandshakeStep | null = pendingHandshake;
    const isHandshakeStepMissing = (agent: Agent | undefined, step: HandshakeStep): boolean => {
      if (!agent) {
        return false;
      }
      if (step === "name") {
        return !agent.preferredName;
      }
      if (step === "tone") {
        return agent.hasAskedTone !== true;
      }
      return agent.hasAskedName !== true;
    };
    const renameFromInstruction = parseAgentRenameInstruction(prompt);
    if (renameFromInstruction) {
      setUndoState(null);
      setChatError(null);
      const createdAt = new Date().toISOString();

      setChatMessages((previous) =>
        previous.concat({
          id: crypto.randomUUID(),
          runId: `handshake-${selectedAgent.id}-${Date.now()}`,
          agentId: selectedAgent.id,
          role: "user",
          content: prompt,
          createdAt
        })
      );
      clearChatInputAndFocus();

      try {
        await saveAgentUpdates(
          selectedAgent.id,
          {
            name: renameFromInstruction,
            hasAskedName: true,
            hasAskedAgentName: true
          },
          `Rename agent to ${renameFromInstruction}`
        );
        clearAgentNameConfirmation(selectedAgent.id);
        clearPendingHandshake(selectedAgent.id);
        appendAssistantChatMessage(selectedAgent.id, `Renamed to ${renameFromInstruction}.`);
        setUndoState((current) =>
          current ? { ...current, message: `Renamed to ${renameFromInstruction}` } : current
        );
      } catch {
        setChatError("Unable to rename this agent.");
      }
      return;
    }

    const preferredNameFromInstruction = parsePreferredNameInstruction(prompt);
    if (preferredNameFromInstruction) {
      setUndoState(null);
      setChatError(null);
      const createdAt = new Date().toISOString();

      setChatMessages((previous) =>
        previous.concat({
          id: crypto.randomUUID(),
          runId: `handshake-${selectedAgent.id}-${Date.now()}`,
          agentId: selectedAgent.id,
          role: "user",
          content: prompt,
          createdAt
        })
      );
      clearChatInputAndFocus();

      try {
        await saveAgentUpdates(
          selectedAgent.id,
          {
            preferredName: preferredNameFromInstruction
          },
          `Set preferred name for ${selectedAgent.name}`
        );
        clearPendingHandshake(selectedAgent.id);
        appendAssistantChatMessage(
          selectedAgent.id,
          `Got it — I’ll call you ${preferredNameFromInstruction}.`
        );
      } catch {
        setChatError("Unable to save how to address you.");
      }
      return;
    }

    if (pendingHandshake === "name" && looksLikeNameInput(prompt)) {
      setUndoState(null);
      setChatError(null);
      const createdAt = new Date().toISOString();

      setChatMessages((previous) =>
        previous.concat({
          id: crypto.randomUUID(),
          runId: `handshake-${selectedAgent.id}-${Date.now()}`,
          agentId: selectedAgent.id,
          role: "user",
          content: prompt,
          createdAt
        })
      );
      clearChatInputAndFocus();

      try {
        await saveAgentUpdates(
          selectedAgent.id,
          {
            preferredName: prompt.trim()
          },
          `Set preferred name for ${selectedAgent.name}`
        );
        clearPendingHandshake(selectedAgent.id);
      } catch {
        setChatError("Unable to save name preference.");
      }
      return;
    }

    if (pendingHandshake === "agent_name" && looksLikeAgentNameInput(prompt)) {
      const nextName = normalizeAgentNameCandidate(prompt);
      if (nextName) {
        setUndoState(null);
        setChatError(null);
        const createdAt = new Date().toISOString();
        setChatMessages((previous) =>
          previous.concat({
            id: crypto.randomUUID(),
            runId: `handshake-${selectedAgent.id}-${Date.now()}`,
            agentId: selectedAgent.id,
            role: "user",
            content: prompt,
            createdAt
          })
        );
        clearChatInputAndFocus();

        try {
          await saveAgentUpdates(
            selectedAgent.id,
            {
              name: nextName,
              hasAskedName: true,
              hasAskedAgentName: true
            },
            `Rename agent to ${nextName}`
          );
          setAgentNameConfirmation(selectedAgent.id, nextName);
          clearPendingHandshake(selectedAgent.id);
          appendAssistantChatMessage(
            selectedAgent.id,
            `Got it — I'll go by ${nextName}.`,
            "handshake_complete"
          );
          setUndoState((current) => (current ? { ...current, message: `Renamed to ${nextName}` } : current));
        } catch {
          setChatError("Unable to rename this agent.");
        }
        return;
      }
    }

    const deferredHandshakeStep: HandshakeStep | null =
      pendingHandshakeForFlow && looksLikeTaskMessage(prompt) ? pendingHandshakeForFlow : null;
    const extractCommandUrl = !IS_PRODUCTION ? parseExtractCommand(prompt) : null;
    const readCommandPath = !IS_PRODUCTION ? parseReadCommand(prompt) : null;
    const writeCommand = !IS_PRODUCTION ? parseWriteCommand(prompt) : null;

    if (isChatActionBusy) {
      setChatError("A planning or execution run is already in progress.");
      return;
    }

    setUndoState(null);
    setChatNotice(null);
    setChatError(null);

    if (extractCommandUrl) {
      const runId = crypto.randomUUID();
      const startedAtIso = new Date().toISOString();

      const run: Run = {
        id: runId,
        agentId: selectedAgent.id,
        title: `Web Extract: ${extractCommandUrl}`,
        status: "executing",
        startedAt: startedAtIso,
        finishedAt: null,
        summary: "Extracting webpage...",
        logs: ["Web extract queued."]
      };

      setRuns((previous) => [run, ...previous]);
      setSelectedRunId(runId);
      setChatMessages((previous) =>
        previous.concat({
          id: crypto.randomUUID(),
          runId,
          agentId: selectedAgent.id,
          role: "user",
          content: prompt,
          createdAt: startedAtIso
        })
      );
      clearChatInputAndFocus();

      try {
        await executeQuickWebExtract({
          runId,
          agent: selectedAgent,
          url: extractCommandUrl
        });
      } finally {
        if (deferredHandshakeStep) {
          const latestAgent = agents.find((agent) => agent.id === selectedAgent.id);
          if (isHandshakeStepMissing(latestAgent, deferredHandshakeStep)) {
            queueHandshakePrompt(selectedAgent.id, deferredHandshakeStep, true);
          }
        }
      }
      return;
    }

    if (readCommandPath || writeCommand) {
      const runId = crypto.randomUUID();
      const startedAtIso = new Date().toISOString();
      const operation: QuickFileOperation = readCommandPath
        ? {
            kind: "read",
            path: readCommandPath
          }
        : {
            kind: "write",
            path: writeCommand!.path,
            text: writeCommand!.text,
            createIfMissing: true
          };

      const run: Run = {
        id: runId,
        agentId: selectedAgent.id,
        title:
          operation.kind === "read"
            ? `File Read: ${operation.path}`
            : `File Write: ${operation.path}`,
        status: "executing",
        startedAt: startedAtIso,
        finishedAt: null,
        summary:
          operation.kind === "read" ? "Reading file..." : "Writing file...",
        logs: [
          operation.kind === "read"
            ? "File read queued."
            : "File write queued."
        ]
      };

      setRuns((previous) => [run, ...previous]);
      setSelectedRunId(runId);
      setChatMessages((previous) =>
        previous.concat({
          id: crypto.randomUUID(),
          runId,
          agentId: selectedAgent.id,
          role: "user",
          content: prompt,
          createdAt: startedAtIso
        })
      );
      clearChatInputAndFocus();

      try {
        await executeQuickFileOperation({
          runId,
          agent: selectedAgent,
          operation
        });
      } finally {
        if (deferredHandshakeStep) {
          const latestAgent = agents.find((agent) => agent.id === selectedAgent.id);
          if (isHandshakeStepMissing(latestAgent, deferredHandshakeStep)) {
            queueHandshakePrompt(selectedAgent.id, deferredHandshakeStep, true);
          }
        }
      }
      return;
    }

    const hasProviderKey = await ensureProviderKeyAvailable(selectedAgent.provider);
    if (!hasProviderKey) {
      setChatError(
        providerMissingKeyMessage(selectedAgent.provider, {
          ...vaultStatus,
          [providerVaultKeyForAgent(selectedAgent.provider)]: false
        }) ?? "API key not set. Add it in Settings → Keys."
      );
      return;
    }

    if (selectedAgent.provider !== "openai_compat") {
      const runId = crypto.randomUUID();
      const startedAtIso = new Date().toISOString();
      const run: Run = {
        id: runId,
        agentId: selectedAgent.id,
        title: `Reply: ${prompt.slice(0, 48)}${prompt.length > 48 ? "..." : ""}`,
        status: "executing",
        startedAt: startedAtIso,
        finishedAt: null,
        summary: "Generating reply...",
        logs: [`Non-streaming ${AGENT_PROVIDER_LABELS[selectedAgent.provider]} request queued.`]
      };

      setRuns((previous) => [run, ...previous]);
      setSelectedRunId(runId);
      setChatMessages((previous) =>
        previous.concat({
          id: crypto.randomUUID(),
          runId,
          agentId: selectedAgent.id,
          role: "user",
          content: prompt,
          createdAt: startedAtIso
        })
      );
      clearChatInputAndFocus();

      try {
        await runNonStreamingProviderReply({
          runId,
          agent: selectedAgent,
          prompt
        });
      } finally {
        if (deferredHandshakeStep) {
          const latestAgent = agents.find((agent) => agent.id === selectedAgent.id);
          if (isHandshakeStepMissing(latestAgent, deferredHandshakeStep)) {
            queueHandshakePrompt(selectedAgent.id, deferredHandshakeStep, true);
          }
        }
      }

      return;
    }

    const runId = crypto.randomUUID();
    const startedAtIso = new Date().toISOString();
    const isStreamingProvider = selectedAgent.provider === "openai_compat";

    const run: Run = {
      id: runId,
      agentId: selectedAgent.id,
      title: `Goal: ${prompt.slice(0, 48)}${prompt.length > 48 ? "..." : ""}`,
      status: isStreamingProvider ? "planning" : "executing",
      startedAt: startedAtIso,
      finishedAt: null,
      summary: isStreamingProvider ? "Planning request..." : "Generating response...",
      logs: [isStreamingProvider ? "Planner request queued." : "Non-streaming response queued."]
    };

    setRuns((previous) => [run, ...previous]);
    setSelectedRunId(runId);
    setActivePlanRunId(runId);
    setPlannedRuns((previous) => ({
      ...previous,
      [runId]: {
        runId,
        agentId: selectedAgent.id,
        prompt,
        rawPlanText: "",
        plan: null,
        planningError: null,
        plannerAttempts: 0,
        plannerErrors: [],
        status: isStreamingProvider ? "planning" : "executing_direct",
        stepStates: [],
        configProposals: [],
        autoRunEligible: false,
        runRequested: !isStreamingProvider
      }
    }));

    setChatMessages((previous) =>
      previous.concat({
        id: crypto.randomUUID(),
        runId,
        agentId: selectedAgent.id,
        role: "user",
        content: prompt,
        createdAt: startedAtIso
      })
    );
    clearChatInputAndFocus();

    try {
      if (isStreamingProvider) {
        await runPlannerForRun(runId, selectedAgent, prompt);
      } else {
        await runNonStreamingProviderReply({
          runId,
          agent: selectedAgent,
          prompt
        });
      }
    } catch (planningError) {
      const message = invokeErrorMessage(planningError, "Unable to run provider response.");
      setChatError(message);
      updatePlannedRun(runId, (current) => ({
        ...current,
        status: "failed",
        planningError: message
      }));
      setRuns((previous) =>
        previous.map((entry) =>
          entry.id === runId
            ? {
                ...entry,
                status: "failed",
                finishedAt: new Date().toISOString(),
                summary: message,
                logs: entry.logs.concat(message)
              }
            : entry
        )
      );
    } finally {
      if (deferredHandshakeStep) {
        const latestAgent = agents.find((agent) => agent.id === selectedAgent.id);
        if (isHandshakeStepMissing(latestAgent, deferredHandshakeStep)) {
          queueHandshakePrompt(selectedAgent.id, deferredHandshakeStep, true);
        }
      }
    }
  };

  const retryPlanning = async () => {
    if (!activePlanRunId) {
      return;
    }
    const planned = plannedRunsRef.current[activePlanRunId];
    if (!planned) {
      return;
    }

    const agent = agents.find((item) => item.id === planned.agentId);
    if (!agent) {
      setChatError("Agent for this plan was not found.");
      return;
    }

    try {
      await runPlannerForRun(planned.runId, agent, planned.prompt);
    } catch (planningError) {
      setChatError(invokeErrorMessage(planningError, "Unable to rerun planner."));
    }
  };

  const runPlannedExecution = async () => {
    if (!activePlanRunId) {
      return;
    }

    try {
      await executePlannedRun(activePlanRunId);
    } catch (executionError) {
      setChatError(invokeErrorMessage(executionError, "Unable to execute planned run."));
    }
  };

  const applyPlanConfigProposal = async (proposalId: string) => {
    if (!activePlanRunId) {
      return;
    }

    const planned = plannedRunsRef.current[activePlanRunId];
    if (!planned) {
      return;
    }

    const proposal = planned.configProposals.find((item) => item.id === proposalId);
    if (!proposal) {
      return;
    }

    try {
      await applyProposal(
        proposal,
        { type: "user", id: sessionEmail ?? undefined },
        proposal.summary
      );
      const relatedPendingApproval =
        proposal.object.kind === "web_policy"
          ? pendingWebApprovals.find(
              (item) => item.runId === activePlanRunId && item.proposalId === proposal.id
            )
          : undefined;
      updatePlannedRun(activePlanRunId, (current) => ({
        ...current,
        status: relatedPendingApproval ? "executing" : current.status,
        configProposals: current.configProposals.filter((item) => item.id !== proposalId),
        stepStates: relatedPendingApproval
          ? current.stepStates.map((state) =>
              state.index === relatedPendingApproval.stepIndex
                ? {
                    ...state,
                    status: "pending",
                    note: "Approval granted."
                  }
                : state
            )
          : current.stepStates
      }));
      if (relatedPendingApproval) {
        setPendingWebApprovals((previous) =>
          previous.filter((item) => item.proposalId !== proposal.id)
        );
        setRuns((previous) =>
          previous.map((run) =>
            run.id === activePlanRunId
              ? {
                  ...run,
                  status: "executing",
                  summary: "Working...",
                  logs: run.logs.concat(
                    `Web access approved for ${proposal.object.id}. Continuing execution.`
                  )
                }
              : run
          )
        );
        void executePlannedRun(activePlanRunId).catch(() => {
          setChatError("Unable to continue after approval.");
        });
      }
      setChatMessages((previous) =>
        previous.concat({
          id: crypto.randomUUID(),
          runId: activePlanRunId,
          agentId: planned.agentId,
          role: "assistant",
          content: `Applied config change: ${proposal.summary}`,
          createdAt: new Date().toISOString()
        })
      );
    } catch {
      setChatError("Unable to apply configuration change.");
    }
  };

  const cancelChatStream = async () => {
    if (!activeChatRunId) {
      return;
    }

    const runId = activeChatRunId;
    try {
      await invoke("llm_stream_cancel", {
        runId
      });
      const waiter = streamWaitersRef.current[runId];
      if (waiter) {
        delete streamWaitersRef.current[runId];
        waiter.resolve({ cancelled: true });
      }
      delete chatRunMetaRef.current[runId];

      updatePlannedRun(runId, (current) => ({ ...current, status: "cancelled" }));
      setRuns((previous) =>
        previous.map((run) =>
          run.id === runId
            ? {
                ...run,
                status: "cancelled",
                finishedAt: new Date().toISOString(),
                summary: "Run cancelled by user.",
                logs: run.logs.concat("Run cancelled by user.")
              }
            : run
        )
      );
      setActiveChatRunId((current) => (current === runId ? null : current));
      setChatError("Generation cancelled.");
    } catch (cancelError) {
      const message = invokeErrorMessage(cancelError, "Unable to cancel streaming run.");
      setChatError(message);
    }
  };

  const handleSaveVaultKey = useCallback(async (key: ProviderVaultKey): Promise<boolean> => {
    setVaultMessage(null);
    const nextValue = vaultInputs[key].trim();

    try {
      if (nextValue.length > 0) {
        await vaultSet(key, nextValue);
      } else {
        await vaultDelete(key);
      }

      setVaultInputs((previous) => ({ ...previous, [key]: "" }));
      await refreshVaultStatus();
      setVaultMessage(`${PROVIDER_LABELS[key]} key updated.`);
      return true;
    } catch {
      setVaultMessage("Unable to update vault key.");
      return false;
    }
  }, [refreshVaultStatus, vaultInputs]);

  const handleLockBossClaw = useCallback(async () => {
    try {
      await vaultLock();
      const message = "BossClaw locked. You may be prompted again when you use keys.";
      setSettingsMessage(message);
      setLockToastMessage(message);
    } catch {
      setSettingsMessage("Unable to lock BossClaw right now.");
    }
  }, []);

  const saveOpenAiCompatProvider = useCallback(async () => {
    const normalizedBase =
      normalizedOpenAiCompatBase(appSettings.openaiCompatBaseUrl) ??
      DEFAULT_APP_SETTINGS.openaiCompatBaseUrl;
    updateModelSettings((previous) => ({
      ...previous,
      openaiCompatBaseUrl: normalizedBase
    }));

    const ok = await handleSaveVaultKey("openai_compat_api_key");
    if (ok) {
      setSettingsMessage("OpenAI-compatible model and key saved.");
    }
  }, [appSettings.openaiCompatBaseUrl, handleSaveVaultKey, updateModelSettings]);

  const saveOpenAiProvider = useCallback(async () => {
    updateModelSettings((previous) => ({
      ...previous
    }));
    const ok = await handleSaveVaultKey("openai_api_key");
    if (ok) {
      setSettingsMessage("OpenAI model and key saved.");
    }
  }, [handleSaveVaultKey, updateModelSettings]);

  const saveAnthropicProvider = useCallback(async () => {
    updateModelSettings((previous) => ({
      ...previous
    }));
    const ok = await handleSaveVaultKey("anthropic_api_key");
    if (ok) {
      setSettingsMessage("Anthropic model and key saved.");
    }
  }, [handleSaveVaultKey, updateModelSettings]);

  const saveGoogleProvider = useCallback(async () => {
    updateModelSettings((previous) => ({
      ...previous
    }));
    const ok = await handleSaveVaultKey("google_api_key");
    if (ok) {
      setSettingsMessage("Google model and key saved.");
    }
  }, [handleSaveVaultKey, updateModelSettings]);

  const toggleNewAgentTool = (toolId: string) => {
    setNewAgentTools((previous) =>
      previous.includes(toolId)
        ? previous.filter((enabledToolId) => enabledToolId !== toolId)
        : previous.concat(toolId)
    );
  };

  const handleCreateAgent = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();

    const name = newAgentName.trim();
    const purpose = newAgentPurpose.trim();
    if (!name || !purpose) {
      setError("Agent name and purpose are required.");
      return;
    }

    const createdAgent: Agent = {
      id: crypto.randomUUID(),
      name,
      purpose,
      provider: newAgentProvider,
      modelId: newAgentModelOverride.trim() || undefined,
      openaiCompatBaseUrlOverride: normalizedOpenAiCompatBase(newAgentBaseOverride),
      openaiCompatModelOverride:
        newAgentProvider === "openai_compat" ? newAgentModelOverride.trim() || null : null,
      hasAskedName: false,
      hasAskedTone: false,
      hasAskedAgentName: false,
      policy: {
        memoryMode: newAgentMemoryMode,
        loggingMode: newAgentLoggingMode,
        toolsEnabled: newAgentTools
      },
      createdAt: new Date().toISOString()
    };

    const proposal: ConfigChangeProposal = {
      id: crypto.randomUUID(),
      ts: new Date().toISOString(),
      object: { kind: "agent", id: createdAgent.id },
      summary: `Create agent ${createdAgent.name}`,
      diff: diffObjects(null, createdAgent),
      applyMode: "autopilot",
      requiresConfirm: true,
      proposedBy: { type: "user", id: sessionEmail ?? undefined },
      patch: {
        after: createdAgent as unknown as Record<string, unknown>
      }
    };

    try {
      await applyProposal(proposal, { type: "user", id: sessionEmail ?? undefined }, `Created ${createdAgent.name}`);
    } catch {
      setError("Unable to create agent.");
      return;
    }

    openAgentPanel(createdAgent.id, "chat");
    setShowCreateAgentModal(false);
    setNewAgentName("");
    setNewAgentPurpose("");
    setNewAgentProvider("openai_compat");
    setNewAgentBaseOverride("");
    setNewAgentModelOverride("");
    setNewAgentMemoryMode("isolated");
    setNewAgentLoggingMode("simple");
    setNewAgentTools([]);
    setError(null);
  };

  const saveAgentUpdates = useCallback(
    async (
      agentId: string,
      updates: Partial<{
        name: string;
        memoryMode: MemoryMode;
        loggingMode: LoggingMode;
        toolsEnabled: string[];
        provider: AgentProvider;
        modelId: string | null;
        openaiCompatBaseUrlOverride: string | null;
        openaiCompatModelOverride: string | null;
        preferredName: string | null;
        tone: "concise" | "detailed" | null;
        hasAskedName: boolean;
        hasAskedTone: boolean;
        hasAskedAgentName: boolean;
      }>,
      summary?: string
    ) => {
      const currentAgent = agents.find((agent) => agent.id === agentId);
      if (!currentAgent) {
        throw new Error("Agent was not found.");
      }

      const preferredName =
        "preferredName" in updates
          ? updates.preferredName?.trim() || undefined
          : currentAgent.preferredName;
      const tone =
        "tone" in updates
          ? updates.tone === "concise" || updates.tone === "detailed"
            ? updates.tone
            : undefined
          : currentAgent.tone;
      const resolvedProvider = normalizeAgentProvider(updates.provider ?? currentAgent.provider);
      const resolvedModelId =
        "modelId" in updates
          ? updates.modelId?.trim() || undefined
          : currentAgent.modelId?.trim() || undefined;

      const nextAgent: Agent = {
        ...currentAgent,
        name:
          "name" in updates && typeof updates.name === "string"
            ? updates.name.trim() || currentAgent.name
            : currentAgent.name,
        provider: resolvedProvider,
        modelId: resolvedModelId,
        openaiCompatBaseUrlOverride:
          "openaiCompatBaseUrlOverride" in updates
            ? updates.openaiCompatBaseUrlOverride ?? null
            : currentAgent.openaiCompatBaseUrlOverride,
        openaiCompatModelOverride:
          "openaiCompatModelOverride" in updates
            ? updates.openaiCompatModelOverride ?? null
            : resolvedProvider === "openai_compat"
              ? resolvedModelId ?? currentAgent.openaiCompatModelOverride
              : currentAgent.openaiCompatModelOverride,
        preferredName,
        tone,
        hasAskedName:
          "hasAskedName" in updates
            ? Boolean(updates.hasAskedName)
            : currentAgent.hasAskedName ?? false,
        hasAskedTone:
          "hasAskedTone" in updates
            ? Boolean(updates.hasAskedTone)
            : currentAgent.hasAskedTone ?? false,
        hasAskedAgentName:
          "hasAskedAgentName" in updates
            ? Boolean(updates.hasAskedAgentName)
            : currentAgent.hasAskedAgentName ?? false,
        policy: {
          memoryMode: updates.memoryMode ?? currentAgent.policy.memoryMode,
          loggingMode: updates.loggingMode ?? currentAgent.policy.loggingMode,
          toolsEnabled: updates.toolsEnabled ?? currentAgent.policy.toolsEnabled
        }
      };

      const proposal: ConfigChangeProposal = {
        id: crypto.randomUUID(),
        ts: new Date().toISOString(),
        object: { kind: "agent", id: agentId },
        summary: summary ?? `Update agent ${currentAgent.name}`,
        diff: diffObjects(currentAgent, nextAgent),
        applyMode: "autopilot",
        requiresConfirm: true,
        proposedBy: { type: "user", id: sessionEmail ?? undefined },
        patch: {
          after: nextAgent as unknown as Record<string, unknown>
        }
      };

      await applyProposal(
        proposal,
        { type: "user", id: sessionEmail ?? undefined },
        summary ?? `Updated ${currentAgent.name}`
      );
    },
    [agents, applyProposal, sessionEmail]
  );

  const updateAgentPolicy = (
    agentId: string,
    updates: Partial<{
      name: string;
      memoryMode: MemoryMode;
      loggingMode: LoggingMode;
      toolsEnabled: string[];
      provider: AgentProvider;
      modelId: string | null;
      openaiCompatBaseUrlOverride: string | null;
      openaiCompatModelOverride: string | null;
      preferredName: string | null;
      tone: "concise" | "detailed" | null;
      hasAskedName: boolean;
      hasAskedTone: boolean;
      hasAskedAgentName: boolean;
    }>
  ) => {
    void saveAgentUpdates(agentId, updates).catch(() => {
      setError("Unable to update agent configuration.");
    });
  };

  const saveMissionUpdates = useCallback(
    async (
      missionId: string,
      updates: Partial<{
        agentId: string;
        title: string;
        goal: string;
        enabled: boolean;
        archived: boolean;
        chatPosting: MissionRunChatPosting;
        collapseRepeats: boolean;
        schedule: Mission["schedule"];
        nextRunAt: string;
        lastRunAt: string | null;
      }>,
      summary?: string
    ): Promise<Mission> => {
      const currentMission = missions.find((mission) => mission.id === missionId);
      if (!currentMission) {
        throw new Error("Mission not found.");
      }

      const nextAgentId =
        "agentId" in updates && typeof updates.agentId === "string" && updates.agentId.trim().length > 0
          ? updates.agentId
          : currentMission.agentId;
      const nextGoal = "goal" in updates && typeof updates.goal === "string" ? updates.goal : currentMission.goal;
      const nextSchedule = updates.schedule ?? currentMission.schedule;
      const nextMission: Mission = {
        ...currentMission,
        agentId: nextAgentId,
        title:
          "title" in updates && typeof updates.title === "string" && updates.title.trim().length > 0
            ? updates.title.trim()
            : currentMission.title,
        goal: nextGoal,
        enabled: "enabled" in updates ? Boolean(updates.enabled) : currentMission.enabled,
        archived: "archived" in updates ? Boolean(updates.archived) : currentMission.archived,
        chatPosting:
          "chatPosting" in updates &&
          (updates.chatPosting === "off" ||
            updates.chatPosting === "summary" ||
            updates.chatPosting === "verbose")
            ? updates.chatPosting
            : currentMission.chatPosting,
        collapseRepeats:
          "collapseRepeats" in updates
            ? Boolean(updates.collapseRepeats)
            : currentMission.collapseRepeats,
        schedule: nextSchedule,
        nextRunAt: updates.nextRunAt ?? currentMission.nextRunAt,
        lastRunAt:
          "lastRunAt" in updates
            ? updates.lastRunAt ?? undefined
            : currentMission.lastRunAt,
        fingerprint: buildMissionFingerprint(nextAgentId, nextSchedule, nextGoal),
        updatedAt: new Date().toISOString()
      };

      const proposal: ConfigChangeProposal = {
        id: crypto.randomUUID(),
        ts: new Date().toISOString(),
        object: { kind: "mission", id: missionId },
        summary: summary ?? `Update mission ${currentMission.title}`,
        diff: diffObjects(currentMission, nextMission),
        applyMode: "autopilot",
        requiresConfirm: true,
        proposedBy: { type: "user", id: sessionEmail ?? undefined },
        patch: {
          after: nextMission as unknown as Record<string, unknown>
        }
      };

      await applyProposal(
        proposal,
        { type: "user", id: sessionEmail ?? undefined },
        summary ?? `Updated mission ${nextMission.title}`
      );

      return nextMission;
    },
    [applyProposal, missions, sessionEmail]
  );

  const saveMissionUpdatesBySystem = useCallback(
    async (
      missionId: string,
      updates: Partial<{
        agentId: string;
        title: string;
        goal: string;
        enabled: boolean;
        archived: boolean;
        chatPosting: MissionRunChatPosting;
        collapseRepeats: boolean;
        schedule: Mission["schedule"];
        nextRunAt: string;
        lastRunAt: string | null;
      }>,
      summary: string
    ): Promise<Mission> => {
      const currentMission = missions.find((mission) => mission.id === missionId);
      if (!currentMission) {
        throw new Error("Mission not found.");
      }

      const nextAgentId =
        "agentId" in updates && typeof updates.agentId === "string" && updates.agentId.trim().length > 0
          ? updates.agentId
          : currentMission.agentId;
      const nextGoal = "goal" in updates && typeof updates.goal === "string" ? updates.goal : currentMission.goal;
      const nextSchedule = updates.schedule ?? currentMission.schedule;
      const nextMission: Mission = {
        ...currentMission,
        agentId: nextAgentId,
        title:
          "title" in updates && typeof updates.title === "string" && updates.title.trim().length > 0
            ? updates.title.trim()
            : currentMission.title,
        goal: nextGoal,
        enabled: "enabled" in updates ? Boolean(updates.enabled) : currentMission.enabled,
        archived: "archived" in updates ? Boolean(updates.archived) : currentMission.archived,
        chatPosting:
          "chatPosting" in updates &&
          (updates.chatPosting === "off" ||
            updates.chatPosting === "summary" ||
            updates.chatPosting === "verbose")
            ? updates.chatPosting
            : currentMission.chatPosting,
        collapseRepeats:
          "collapseRepeats" in updates
            ? Boolean(updates.collapseRepeats)
            : currentMission.collapseRepeats,
        schedule: nextSchedule,
        nextRunAt: updates.nextRunAt ?? currentMission.nextRunAt,
        lastRunAt:
          "lastRunAt" in updates
            ? updates.lastRunAt ?? undefined
            : currentMission.lastRunAt,
        fingerprint: buildMissionFingerprint(nextAgentId, nextSchedule, nextGoal),
        updatedAt: new Date().toISOString()
      };

      const proposal: ConfigChangeProposal = {
        id: crypto.randomUUID(),
        ts: new Date().toISOString(),
        object: { kind: "mission", id: missionId },
        summary,
        diff: diffObjects(currentMission, nextMission),
        applyMode: "autopilot",
        requiresConfirm: true,
        proposedBy: { type: "user", id: "mission_scheduler" },
        patch: {
          after: nextMission as unknown as Record<string, unknown>
        }
      };

      await applyConfigChange(proposal, { type: "system", id: "mission_scheduler" });
      await refreshConfigState();
      await refreshAuditState();

      return nextMission;
    },
    [missions, refreshAuditState, refreshConfigState]
  );

  const openCreateMissionModal = useCallback(() => {
    if (!agentPanelAgent) {
      setError("Select an agent first.");
      return;
    }
    setNewMissionTitle(`Recurring task for ${agentPanelAgent.name}`);
    setNewMissionGoal("");
    setNewMissionPresetKind("daily");
    setNewMissionTime("09:00");
    setNewMissionWeekday(1);
    setNewMissionIntervalMinutes(60);
    setShowCreateMissionModal(true);
  }, [agentPanelAgent]);

  const closeCreateMissionModal = useCallback(() => {
    setShowCreateMissionModal(false);
  }, []);

  const handleCreateMission = useCallback(
    async (event: FormEvent<HTMLFormElement>) => {
      event.preventDefault();
      if (!agentPanelAgent) {
        setError("Select an agent first.");
        return;
      }

      const title = newMissionTitle.trim();
      const goal = newMissionGoal.trim();
      if (!title || !goal) {
        setError("Mission title and goal are required.");
        return;
      }

      const normalizedTime = normalizeMissionTime(newMissionTime);
      const normalizedWeekday = normalizeMissionWeekday(newMissionWeekday);
      const normalizedInterval = normalizeMissionInterval(newMissionIntervalMinutes);
      const schedule = buildMissionSchedule(
        newMissionPresetKind === "every_minutes"
          ? {
              kind: "every_minutes",
              intervalMinutes: normalizedInterval
            }
          : newMissionPresetKind === "weekly"
            ? {
                kind: "weekly",
                weekday: normalizedWeekday,
                time: normalizedTime
              }
            : newMissionPresetKind === "weekdays"
              ? {
                  kind: "weekdays",
                  time: normalizedTime
                }
              : {
                  kind: "daily",
                  time: normalizedTime
                }
      );

      const nowIso = new Date().toISOString();
      const fingerprint = buildMissionFingerprint(agentPanelAgent.id, schedule, goal);
      const existingMission =
        missions.find((mission) =>
          hasMatchingMissionFingerprint(mission, agentPanelAgent.id, schedule, goal)
        ) ?? null;
      if (existingMission) {
        const nextMission: Mission = {
          ...existingMission,
          title,
          goal,
          enabled: true,
          schedule,
          fingerprint,
          nextRunAt: computeNextRunAt(schedule),
          updatedAt: nowIso
        };
        const hasChanges =
          existingMission.title !== nextMission.title ||
          existingMission.goal !== nextMission.goal ||
          existingMission.enabled !== nextMission.enabled ||
          existingMission.fingerprint !== nextMission.fingerprint ||
          existingMission.nextRunAt !== nextMission.nextRunAt ||
          !isMissionScheduleEqual(existingMission.schedule, nextMission.schedule);

        try {
          if (hasChanges) {
            const updateProposal: ConfigChangeProposal = {
              id: crypto.randomUUID(),
              ts: nowIso,
              object: { kind: "mission", id: existingMission.id },
              summary: `Update mission ${nextMission.title}`,
              diff: diffObjects(existingMission, nextMission),
              applyMode: "autopilot",
              requiresConfirm: true,
              proposedBy: { type: "user", id: sessionEmail ?? undefined },
              patch: {
                after: nextMission as unknown as Record<string, unknown>
              }
            };

            await applyProposal(
              updateProposal,
              { type: "user", id: sessionEmail ?? undefined },
              `Updated mission ${nextMission.title}`
            );
          }
          setShowCreateMissionModal(false);
          setError(null);
        } catch {
          setError("Unable to create mission.");
        }
        return;
      }

      const mission: Mission = {
        id: crypto.randomUUID(),
        agentId: agentPanelAgent.id,
        fingerprint,
        title,
        goal,
        enabled: true,
        chatPosting: "summary",
        collapseRepeats: true,
        schedule,
        nextRunAt: computeNextRunAt(schedule),
        createdAt: nowIso,
        updatedAt: nowIso
      };

      const proposal: ConfigChangeProposal = {
        id: crypto.randomUUID(),
        ts: nowIso,
        object: { kind: "mission", id: mission.id },
        summary: `Create mission ${mission.title}`,
        diff: diffObjects(null, mission),
        applyMode: "autopilot",
        requiresConfirm: true,
        proposedBy: { type: "user", id: sessionEmail ?? undefined },
        patch: {
          after: mission as unknown as Record<string, unknown>
        }
      };

      try {
        await applyProposal(
          proposal,
          { type: "user", id: sessionEmail ?? undefined },
          `Created mission ${mission.title}`
        );
        setShowCreateMissionModal(false);
        setError(null);
      } catch {
        setError("Unable to create mission.");
      }
    },
    [
      agentPanelAgent,
      applyProposal,
      newMissionGoal,
      newMissionIntervalMinutes,
      newMissionPresetKind,
      newMissionTime,
      newMissionTitle,
      newMissionWeekday,
      missions,
      sessionEmail
    ]
  );

  const toggleMissionEnabled = useCallback(
    async (mission: Mission) => {
      const nextEnabled = !mission.enabled;
      const nextRunAt = nextEnabled
        ? computeNextRunAt(mission.schedule)
        : mission.nextRunAt;
      const summary = nextEnabled
        ? `Enable mission ${mission.title}`
        : `Disable mission ${mission.title}`;

      try {
        await saveMissionUpdates(
          mission.id,
          {
            enabled: nextEnabled,
            nextRunAt
          },
          summary
        );
        setError(null);
      } catch {
        setError("Unable to update mission.");
      }
    },
    [saveMissionUpdates]
  );

  const toggleMissionsPaused = useCallback(() => {
    setAppSettings((previous) => ({
      ...previous,
      missionsPaused: !previous.missionsPaused
    }));
    setError(null);
  }, []);

  const deleteMission = useCallback(
    async (mission: Mission) => {
      try {
        await saveMissionUpdates(
          mission.id,
          {
            enabled: false,
            archived: true
          },
          `Delete mission ${mission.title}`
        );
        setError(null);
      } catch {
        setError("Unable to delete mission.");
      }
    },
    [saveMissionUpdates]
  );

  const requestMissionDelete = useCallback((mission: Mission) => {
    setOpenMissionMenuId(null);
    setMissionPendingDelete(mission);
  }, []);

  const closeMissionDeleteModal = useCallback(() => {
    setMissionPendingDelete(null);
  }, []);

  const confirmMissionDelete = useCallback(async () => {
    if (!missionPendingDelete) {
      return;
    }
    const mission = missionPendingDelete;
    setMissionPendingDelete(null);
    await deleteMission(mission);
  }, [deleteMission, missionPendingDelete]);

  const showRecurringPlaceholder = useCallback(() => {
    setChatNotice("“Make this recurring” from chat is coming soon. Use New Mission for now.");
  }, []);

  const postMissionRunChatUpdate = useCallback(
    (input: {
      mission: Mission;
      runId: string;
      runStatus: RunStatus;
      runSummary: string;
      output: string;
      finishedAt: string;
    }) => {
      const mission = input.mission;
      const postingMode = mission.chatPosting;
      if (postingMode === "off") {
        return;
      }

      const fallbackSummary = input.runStatus === "failed" ? "Failed" : "Completed";
      const snippet = buildMissionSnippet(input.output || input.runSummary || fallbackSummary);
      const lastTimeLabel = new Date(input.finishedAt).toLocaleTimeString([], {
        hour: "numeric",
        minute: "2-digit"
      });
      const runHistoryCount =
        runsRef.current.filter((run) => run.missionId === mission.id).length +
        (runsRef.current.some((run) => run.id === input.runId) ? 0 : 1);
      const runHistoryIds = runsRef.current
        .filter((run) => run.missionId === mission.id)
        .map((run) => run.id);
      const runIdsWithCurrent = Array.from(new Set([...runHistoryIds, input.runId]));
      const verboseResult = (input.output || "").trim();
      const nonCollapsedResult =
        postingMode === "verbose" ? verboseResult || snippet : snippet;
      const collapsedResult = snippet;

      const collapseMode = mission.collapseRepeats;
      if (!collapseMode) {
        setChatMessages((previous) =>
          previous.concat({
            id: crypto.randomUUID(),
            runId: input.runId,
            agentId: mission.agentId,
            role: "assistant",
            kind: "mission_update",
            missionId: mission.id,
            missionRunIds: [input.runId],
            count: 1,
            lastRunAt: input.finishedAt,
            lastSnippet: snippet,
            content: `🕒 Scheduled Update — ${mission.title} (${lastTimeLabel})\nResult: ${nonCollapsedResult || "Completed"}`,
            createdAt: new Date().toISOString()
          })
        );
        return;
      }

      setChatMessages((previous) => {
        let existingIndex = -1;
        for (let index = previous.length - 1; index >= 0; index -= 1) {
          const message = previous[index];
          if (
            message.kind === "mission_update" &&
            message.missionId === mission.id &&
            message.role === "assistant"
          ) {
            existingIndex = index;
            break;
          }
        }

        if (existingIndex < 0) {
          return previous.concat({
            id: crypto.randomUUID(),
            runId: input.runId,
            agentId: mission.agentId,
            role: "assistant",
            kind: "mission_update",
            missionId: mission.id,
            missionRunIds: runIdsWithCurrent,
            count: Math.max(1, runHistoryCount),
            lastRunAt: input.finishedAt,
            lastSnippet: snippet,
            content:
              `🕒 Scheduled Update — ${mission.title} ran ${Math.max(1, runHistoryCount)} times (last ${lastTimeLabel})\nResult: ${collapsedResult || "Completed"}`,
            createdAt: new Date().toISOString()
          });
        }

        const next = [...previous];
        const existing = next[existingIndex];
        const nextRunIds = Array.from(new Set([...(existing.missionRunIds ?? []), input.runId]));
        const nextCount = Math.max(existing.count ?? 1, runHistoryCount, nextRunIds.length);
        next[existingIndex] = {
          ...existing,
          runId: input.runId,
          missionRunIds: nextRunIds,
          count: nextCount,
          lastRunAt: input.finishedAt,
          lastSnippet: snippet,
          content:
            `🕒 Scheduled Update — ${mission.title} ran ${nextCount} times (last ${lastTimeLabel})\nResult: ${collapsedResult || "Completed"}`
        };
        return next;
      });
    },
    []
  );

  const executeScheduledMissionRun = useCallback(
    async (mission: Mission, agent: Agent): Promise<{ runId: string }> => {
      const runId = crypto.randomUUID();
      const startedAtIso = new Date().toISOString();
      const isStreamingProvider = agent.provider === "openai_compat";
      missionRunContextRef.current[runId] = {
        missionId: mission.id,
        missionTitle: mission.title,
        chatPosting: mission.chatPosting,
        collapseRepeats: mission.collapseRepeats
      };
      runOutputBufferRef.current[runId] = "";
      const run: Run = {
        id: runId,
        agentId: agent.id,
        missionId: mission.id,
        title: `Mission: ${mission.title}`,
        status: isStreamingProvider ? "planning" : "executing",
        startedAt: startedAtIso,
        finishedAt: null,
        summary: isStreamingProvider ? "Mission planning request..." : "Mission generating response...",
        logs: [`Scheduled mission triggered: ${mission.title}`]
      };

      setRuns((previous) => [run, ...previous]);
      setPlannedRuns((previous) => ({
        ...previous,
        [runId]: {
          runId,
          agentId: agent.id,
          prompt: mission.goal,
          rawPlanText: "",
          plan: null,
          planningError: null,
          plannerAttempts: 0,
          plannerErrors: [],
          status: isStreamingProvider ? "planning" : "executing_direct",
          stepStates: [],
          configProposals: [],
          autoRunEligible: false,
          runRequested: !isStreamingProvider
        }
      }));

      if (shouldShowMissionLiveOutput(runId)) {
        setChatMessages((previous) =>
          previous.concat({
            id: crypto.randomUUID(),
            runId,
            agentId: agent.id,
            role: "user",
            content: `[Mission] ${mission.goal}`,
            createdAt: startedAtIso
          })
        );
      }

      if (isStreamingProvider) {
        await runPlannerForRun(runId, agent, mission.goal);
      } else {
        await runNonStreamingProviderReply({
          runId,
          agent,
          prompt: mission.goal
        });
      }
      return { runId };
    },
    [runNonStreamingProviderReply, runPlannerForRun, shouldShowMissionLiveOutput]
  );

  const runMissionSchedulerTick = useCallback(async (): Promise<void> => {
    if (appSettings.missionsPaused) {
      return;
    }
    if (missionSchedulerBusyRef.current) {
      return;
    }

    missionSchedulerBusyRef.current = true;
    try {
      const now = new Date();
      for (const mission of missions) {
        if (!isMissionDue(mission, now)) {
          continue;
        }

        if (runningMissionIdsRef.current.has(mission.id)) {
          continue;
        }

        const agent = agents.find((entry) => entry.id === mission.agentId && !entry.archived);
        const completedAt = new Date();
        const nextRunAt = computeNextRunAt(mission.schedule, completedAt);

        if (!agent) {
          await saveMissionUpdatesBySystem(
            mission.id,
            {
              lastRunAt: completedAt.toISOString(),
              nextRunAt
            },
            `Reschedule mission ${mission.title}`
          ).catch(() => undefined);
          continue;
        }

        runningMissionIdsRef.current.add(mission.id);
        let scheduledRunId: string | null = null;
        try {
          const hasProviderKey = await ensureProviderKeyAvailable(agent.provider);
          if (!hasProviderKey) {
            const failedAt = completedAt.toISOString();
            const keyMessage =
              providerMissingKeyMessage(agent.provider, EMPTY_VAULT_STATUS) ??
              "API key not set. Add it in Settings → Keys.";
            const failedRunId = crypto.randomUUID();
            scheduledRunId = failedRunId;
            setRuns((previous) => [
              {
                id: failedRunId,
                agentId: agent.id,
                missionId: mission.id,
                title: `Mission: ${mission.title}`,
                status: "failed",
                startedAt: failedAt,
                finishedAt: failedAt,
                summary: keyMessage,
                logs: [keyMessage]
              },
              ...previous
            ]);
          } else {
            const scheduled = await executeScheduledMissionRun(mission, agent);
            scheduledRunId = scheduled.runId;
          }
        } catch (error) {
          const failedAt = new Date().toISOString();
          const message = invokeErrorMessage(error, `Mission failed: ${mission.title}`);
          const failedRunId = crypto.randomUUID();
          scheduledRunId = failedRunId;
          setRuns((previous) => [
            {
              id: failedRunId,
              agentId: mission.agentId,
              missionId: mission.id,
              title: `Mission: ${mission.title}`,
              status: "failed",
              startedAt: failedAt,
              finishedAt: failedAt,
              summary: message,
              logs: [message]
            },
            ...previous
          ]);
        } finally {
          await saveMissionUpdatesBySystem(
            mission.id,
            {
              lastRunAt: completedAt.toISOString(),
              nextRunAt
            },
            `Mission run completed: ${mission.title}`
          ).catch(() => undefined);
          if (scheduledRunId) {
            const runRecord = runsRef.current.find((run) => run.id === scheduledRunId);
            const finishedAt = runRecord?.finishedAt ?? completedAt.toISOString();
            const outputFromLogs =
              runRecord?.logs
                .slice()
                .reverse()
                .find((entry) => entry.startsWith("Output:\n"))
                ?.slice("Output:\n".length) ?? "";
            const output = runOutputBufferRef.current[scheduledRunId] ?? outputFromLogs;
            postMissionRunChatUpdate({
              mission,
              runId: scheduledRunId,
              runStatus: runRecord?.status ?? "completed",
              runSummary: runRecord?.summary ?? "Completed",
              output,
              finishedAt
            });
            delete missionRunContextRef.current[scheduledRunId];
            delete runOutputBufferRef.current[scheduledRunId];
          }
          runningMissionIdsRef.current.delete(mission.id);
        }
      }
    } finally {
      missionSchedulerBusyRef.current = false;
    }
  }, [
    agents,
    appSettings.missionsPaused,
    ensureProviderKeyAvailable,
    executeScheduledMissionRun,
    missions,
    postMissionRunChatUpdate,
    saveMissionUpdatesBySystem
  ]);

  useEffect(() => {
    if (route !== "/app" || !subscription?.active || !localDataLoaded || !agents.length) {
      return;
    }

    let cancelled = false;
    const firstAgentId = agents[0]?.id ?? null;

    const repairMissions = async () => {
      for (const mission of missions) {
        if (cancelled || mission.archived) {
          return;
        }

        const missingAgent = !agents.some((agent) => agent.id === mission.agentId);
        const needsNextRunRepair = needsMissionNextRunRepair(mission);
        if (!missingAgent && !needsNextRunRepair) {
          continue;
        }
        if (missingAgent && !firstAgentId) {
          continue;
        }

        const patch: Partial<{
          agentId: string;
          nextRunAt: string;
        }> = {};

        if (missingAgent && firstAgentId) {
          patch.agentId = firstAgentId;
        }
        if (needsNextRunRepair) {
          patch.nextRunAt = computeNextRunAt(mission.schedule);
        }

        await saveMissionUpdatesBySystem(
          mission.id,
          patch,
          `Repair mission schedule: ${mission.title}`
        ).catch(() => undefined);
      }
    };

    void repairMissions();
    return () => {
      cancelled = true;
    };
  }, [agents, localDataLoaded, missions, route, saveMissionUpdatesBySystem, subscription?.active]);

  useEffect(() => {
    if (route !== "/app" || !subscription?.active || !localDataLoaded) {
      return;
    }

    void runMissionSchedulerTick();
    const intervalId = window.setInterval(() => {
      void runMissionSchedulerTick();
    }, MISSION_SCHEDULER_INTERVAL_MS);

    return () => window.clearInterval(intervalId);
  }, [localDataLoaded, route, runMissionSchedulerTick, subscription?.active]);

  const appendAssistantChatMessage = useCallback(
    (agentId: string, content: string, kind?: ChatMessageKind) => {
      setChatMessages((previous) =>
        previous.concat({
          id: crypto.randomUUID(),
          runId: `handshake-${agentId}-${Date.now()}`,
          agentId,
          role: "assistant",
          content,
          createdAt: new Date().toISOString(),
          kind
        })
      );
    },
    []
  );

  const queueHandshakePrompt = useCallback(
    (agentId: string, step: HandshakeStep, force = false) => {
      let kind: ChatMessageKind = "handshake_name";
      let content = "How should I address you?";
      if (step === "tone") {
        kind = "handshake_tone";
        content = "How would you like me to communicate: concise or detailed?";
      }
      if (step === "agent_name") {
        kind = "handshake_agent_name";
        content = "What would you like to call me?";
      }

      setPendingHandshakeByAgent((previous) => {
        if (!force && previous[agentId] === step) {
          return previous;
        }
        return {
          ...previous,
          [agentId]: step
        };
      });

      appendAssistantChatMessage(agentId, content, kind);
    },
    [appendAssistantChatMessage]
  );

  const clearPendingHandshake = useCallback((agentId: string) => {
    setPendingHandshakeByAgent((previous) => {
      if (!(agentId in previous)) {
        return previous;
      }

      const next = { ...previous };
      delete next[agentId];
      return next;
    });
  }, []);

  const setAgentNameConfirmation = useCallback((agentId: string, name: string) => {
    const expiresAt = Date.now() + 10_000;
    setAgentNameConfirmationByAgent((previous) => ({
      ...previous,
      [agentId]: { name, expiresAt }
    }));

    const existingTimeout = agentNameConfirmationTimeoutsRef.current[agentId];
    if (existingTimeout) {
      window.clearTimeout(existingTimeout);
    }

    const timeoutId = window.setTimeout(() => {
      setAgentNameConfirmationByAgent((previous) => {
        const current = previous[agentId];
        if (!current || current.expiresAt !== expiresAt) {
          return previous;
        }
        return {
          ...previous,
          [agentId]: {
            ...current,
            expiresAt: 0
          }
        };
      });
      delete agentNameConfirmationTimeoutsRef.current[agentId];
    }, 10_000);

    agentNameConfirmationTimeoutsRef.current[agentId] = timeoutId;
  }, []);

  const clearAgentNameConfirmation = useCallback((agentId: string) => {
    const timeoutId = agentNameConfirmationTimeoutsRef.current[agentId];
    if (timeoutId) {
      window.clearTimeout(timeoutId);
      delete agentNameConfirmationTimeoutsRef.current[agentId];
    }

    setAgentNameConfirmationByAgent((previous) => {
      if (!(agentId in previous)) {
        return previous;
      }
      const next = { ...previous };
      delete next[agentId];
      return next;
    });
  }, []);

  const editAgentNameHandshake = useCallback((agentId: string) => {
    clearAgentNameConfirmation(agentId);
    setPendingHandshakeByAgent((previous) => ({
      ...previous,
      [agentId]: "agent_name"
    }));
    clearChatInputAndFocus();
  }, [clearAgentNameConfirmation, clearChatInputAndFocus]);

  useEffect(() => {
    if (agentPanelTab !== "chat" || !agentPanelAgentId) {
      return;
    }

    const activeAgent = agents.find((agent) => agent.id === agentPanelAgentId);
    if (!activeAgent) {
      return;
    }

    const nextStep: HandshakeStep | null =
      activeAgent.hasAskedName === true
        ? activeAgent.preferredName && activeAgent.preferredName.trim().length > 0
          ? null
          : "name"
        : "agent_name";

    if (!nextStep) {
      clearAgentNameConfirmation(activeAgent.id);
      clearPendingHandshake(activeAgent.id);
      return;
    }

    if (pendingHandshakeByAgent[activeAgent.id] === nextStep) {
      return;
    }

    queueHandshakePrompt(activeAgent.id, nextStep);
  }, [
    agentPanelAgentId,
    agentPanelTab,
    agents,
    clearAgentNameConfirmation,
    clearPendingHandshake,
    pendingHandshakeByAgent,
    queueHandshakePrompt
  ]);

  const applyToneChoice = useCallback(
    async (agent: Agent, tone: "concise" | "detailed") => {
      setUndoState(null);
      setChatError(null);
      try {
        await saveAgentUpdates(
          agent.id,
          {
            tone,
            hasAskedTone: true
          },
          `Set communication style for ${agent.name}`
        );
        queueHandshakePrompt(agent.id, "agent_name", true);
      } catch {
        setChatError("Unable to save communication style.");
      }
    },
    [queueHandshakePrompt, saveAgentUpdates]
  );

  const skipNameHandshake = useCallback(async () => {
    if (!agentPanelAgent) {
      return;
    }

    setUndoState(null);
    setChatError(null);
    try {
      await saveAgentUpdates(
        agentPanelAgent.id,
        {
          preferredName: null,
          hasAskedName: true
        },
        `Skipped preferred name for ${agentPanelAgent.name}`
      );
      clearPendingHandshake(agentPanelAgent.id);
    } catch {
      setChatError("Unable to update handshake preferences.");
    }
  }, [agentPanelAgent, clearPendingHandshake, saveAgentUpdates]);

  const skipToneHandshake = useCallback(async () => {
    if (!agentPanelAgent) {
      return;
    }

    setUndoState(null);
    setChatError(null);
    try {
      await saveAgentUpdates(
        agentPanelAgent.id,
        {
          tone: "concise",
          hasAskedTone: true
        },
        `Set default communication style for ${agentPanelAgent.name}`
      );
      queueHandshakePrompt(agentPanelAgent.id, "agent_name", true);
    } catch {
      setChatError("Unable to update handshake preferences.");
    }
  }, [agentPanelAgent, queueHandshakePrompt, saveAgentUpdates]);

  const skipAgentNameHandshake = useCallback(async () => {
    if (!agentPanelAgent) {
      return;
    }

    setUndoState(null);
    setChatError(null);
    try {
      await saveAgentUpdates(
        agentPanelAgent.id,
        {
          hasAskedName: true,
          hasAskedAgentName: true
        },
        `Skipped agent name for ${agentPanelAgent.name}`
      );
      clearAgentNameConfirmation(agentPanelAgent.id);
      if (agentPanelAgent.preferredName && agentPanelAgent.preferredName.trim().length > 0) {
        clearPendingHandshake(agentPanelAgent.id);
      } else {
        queueHandshakePrompt(agentPanelAgent.id, "name", true);
      }
      appendAssistantChatMessage(agentPanelAgent.id, "Understood. Ready when you are.", "handshake_complete");
    } catch {
      setChatError("Unable to update handshake preferences.");
    }
  }, [
    agentPanelAgent,
    appendAssistantChatMessage,
    clearAgentNameConfirmation,
    clearPendingHandshake,
    queueHandshakePrompt,
    saveAgentUpdates
  ]);

  const startAgentRename = useCallback(() => {
    if (!agentPanelAgent) {
      return;
    }
    setAgentNameDraft(agentPanelAgent.name);
    setIsEditingAgentName(true);
  }, [agentPanelAgent]);

  const cancelAgentRename = useCallback(() => {
    setIsEditingAgentName(false);
    setAgentNameDraft(agentPanelAgent?.name ?? "");
  }, [agentPanelAgent]);

  const commitAgentRename = useCallback(async () => {
    if (!agentPanelAgent) {
      return;
    }

    const nextName = agentNameDraft.trim();
    if (!nextName) {
      setChatError("Agent name cannot be empty.");
      return;
    }

    if (nextName === agentPanelAgent.name) {
      setIsEditingAgentName(false);
      return;
    }

    setChatError(null);
    setUndoState(null);
    try {
      await saveAgentUpdates(
        agentPanelAgent.id,
        { name: nextName },
        `Rename agent to ${nextName}`
      );
      setUndoState((current) => (current ? { ...current, message: `Renamed to ${nextName}` } : current));
      setIsEditingAgentName(false);
    } catch {
      setChatError("Unable to rename this agent.");
    }
  }, [agentNameDraft, agentPanelAgent, saveAgentUpdates]);

  const deleteAgent = (agentId: string) => {
    const currentAgent = agents.find((agent) => agent.id === agentId);
    if (!currentAgent) {
      return;
    }

    const archivedAgent: Agent = {
      ...currentAgent,
      archived: true
    };

    const proposal: ConfigChangeProposal = {
      id: crypto.randomUUID(),
      ts: new Date().toISOString(),
      object: { kind: "agent", id: agentId },
      summary: `Delete agent ${currentAgent.name}`,
      diff: diffObjects(currentAgent, archivedAgent),
      applyMode: "autopilot",
      requiresConfirm: true,
      proposedBy: { type: "user", id: sessionEmail ?? undefined },
      patch: {
        after: archivedAgent as unknown as Record<string, unknown>
      }
    };

    void applyProposal(proposal, { type: "user", id: sessionEmail ?? undefined }, `Deleted ${currentAgent.name}`).catch(
      () => {
        setError("Unable to delete agent.");
      }
    );

    setRuns((previous) => previous.filter((run) => run.agentId !== agentId));
    setApprovals((previous) => previous.filter((approval) => approval.agentId !== agentId));
  };

  const createDummyRun = () => {
    const meteringStartedAt = performance.now();
    const fallbackAgentId = selectedAgentId ?? agents[0]?.id ?? null;
    if (!fallbackAgentId) {
      setError("Create an agent first.");
      return;
    }

    const agent = agents.find((entry) => entry.id === fallbackAgentId);
    const runStartedAt = new Date().toISOString();
    const run: Run = {
      id: crypto.randomUUID(),
      agentId: fallbackAgentId,
      title: `Run for ${agent?.name ?? "Agent"}`,
      status: "completed",
      startedAt: runStartedAt,
      finishedAt: runStartedAt,
      summary: "Skeleton run completed. No provider execution is wired yet.",
      logs: [
        "Run initialized from desktop skeleton.",
        "No external tool calls were executed.",
        "Run completed in local stub mode."
      ]
    };

    setRuns((previous) => [run, ...previous]);
    setSelectedRunId(run.id);
    void logUsageEvent(
      buildUsageEvent({
        agentId: run.agentId,
        runId: run.id,
        provider: "bossclaw",
        model: null,
        kind: "other",
        inputChars: run.title.length,
        outputChars: run.summary.length,
        latencyMs: performance.now() - meteringStartedAt
      })
    );
    setError(null);
  };

  const createTestApproval = () => {
    const fallbackAgentId = selectedAgentId ?? agents[0]?.id;
    if (!fallbackAgentId) {
      setError("Create an agent first to test approvals.");
      return;
    }

    const item: ApprovalItem = {
      id: crypto.randomUUID(),
      agentId: fallbackAgentId,
      createdAt: new Date().toISOString(),
      kind: "network",
      message: "Agent requests permission to call an external endpoint.",
      status: "pending"
    };

    setApprovals((previous) => [item, ...previous]);
    setError(null);
  };

  const updateWorkspaceScaffold = () => {
    const workspace = workspaces[0];
    if (!workspace) {
      return;
    }

    const nextWorkspace: Workspace = {
      ...workspace,
      path: workspace.path ? null : "/Users/ahnkwangwook/SuperClaw"
    };

    const proposal: ConfigChangeProposal = {
      id: crypto.randomUUID(),
      ts: new Date().toISOString(),
      object: { kind: "workspace", id: workspace.id },
      summary: workspace.path ? "Clear default workspace path" : "Set default workspace path",
      diff: diffObjects(workspace, nextWorkspace),
      applyMode: "autopilot",
      requiresConfirm: true,
      proposedBy: { type: "user", id: sessionEmail ?? undefined },
      patch: {
        after: nextWorkspace as unknown as Record<string, unknown>
      }
    };

    void applyProposal(
      proposal,
      { type: "user", id: sessionEmail ?? undefined },
      proposal.summary
    ).catch(() => setError("Unable to update workspace scaffold."));
  };

  const openPricing = async () => {
    await openUrl(`${WEB_URL}/pricing`);
  };

  const openAgentPanel = useCallback(
    (agentId: string, initialTab: AgentPanelTab = "chat") => {
      setSelectedAgentId(agentId);
      setAgentPanelAgentId(agentId);
      setAgentPanelTab(initialTab);
      setAppSettings((previous) =>
        previous.lastActiveAgentId === agentId
          ? previous
          : { ...previous, lastActiveAgentId: agentId }
      );
      if (tab !== "agents") {
        setTab("agents");
      }
    },
    [tab]
  );

  const openRunDetails = useCallback(
    (agentId: string, runId: string) => {
      setSelectedRunId(runId);
      openAgentPanel(agentId, "activity");
    },
    [openAgentPanel]
  );

  const refreshVerifiedSkills = useCallback(async () => {
    setSkillsLoading(true);
    setSkillsError(null);
    setSkillsMessage(null);

    try {
      const state = await loadVerifiedSkills();
      setSkillsChannel(state.channel);
      setVerifiedSkills(state.skills);
      setInstalledSkills(state.installed);

      setSelectedSkillId((current) => {
        if (current && state.skills.some((skill) => skill.id === current)) {
          return current;
        }
        return state.skills[0]?.id ?? null;
      });
    } catch (loadError) {
      const message =
        loadError instanceof Error ? loadError.message : "Unable to load local verified skill pack.";
      setSkillsError(message);
    } finally {
      setSkillsLoading(false);
    }
  }, []);

  const confirmInstallSkill = useCallback(async () => {
    if (!pendingInstallSkillId) {
      return;
    }

    setIsInstallingSkill(true);
    setSkillsError(null);
    setSkillsMessage(null);

    try {
      const installed = await installVerifiedSkill(pendingInstallSkillId);
      const installRecord: SkillInstallConfig = {
        id: installed.id,
        version: installed.version,
        channel: installed.channel,
        installDir: installed.installDir,
        installedAt: installed.installedAt
      };

      const proposal: ConfigChangeProposal = {
        id: crypto.randomUUID(),
        ts: new Date().toISOString(),
        object: { kind: "skill_install", id: installRecord.id },
        summary: `Install skill ${installRecord.id} v${installRecord.version}`,
        diff: diffObjects(
          skillInstalls.find((item) => item.id === installRecord.id) ?? null,
          installRecord
        ),
        applyMode: "autopilot",
        requiresConfirm: true,
        proposedBy: { type: "user", id: sessionEmail ?? undefined },
        patch: {
          after: installRecord as unknown as Record<string, unknown>
        }
      };

      await applyProposal(
        proposal,
        { type: "user", id: sessionEmail ?? undefined },
        `Installed ${installRecord.id} v${installRecord.version}`
      );

      setInstalledSkills((previous) => {
        const index = previous.findIndex(
          (item) => item.id === installed.id && item.version === installed.version
        );

        if (index >= 0) {
          const next = [...previous];
          next[index] = installed;
          return next;
        }

        return [installed, ...previous];
      });

      setSkillsMessage(`Installed ${installed.id} v${installed.version}.`);
      setPendingInstallSkillId(null);
    } catch (installError) {
      const message =
        installError instanceof Error ? installError.message : "Unable to install selected skill.";
      setSkillsError(message);
    } finally {
      setIsInstallingSkill(false);
    }
  }, [applyProposal, pendingInstallSkillId, sessionEmail, skillInstalls]);

  useEffect(() => {
    if (route !== "/app" || !subscription?.active || tab !== "skills") {
      return;
    }

    void refreshVerifiedSkills();
  }, [refreshVerifiedSkills, route, subscription?.active, tab]);

  const downloadUsageJson = () => {
    const json = JSON.stringify(usageEvents, null, 2);
    const blob = new Blob([json], { type: "application/json" });
    const objectUrl = URL.createObjectURL(blob);

    const anchor = document.createElement("a");
    anchor.href = objectUrl;
    anchor.download = `bossclaw-usage-${new Date().toISOString().slice(0, 10)}.json`;
    anchor.click();

    URL.revokeObjectURL(objectUrl);
  };

  if (isBootstrapping) {
    return (
      <main className="login-wrap">
        <div className="login-card">
          <h1>BossClaw Desktop</h1>
          <p>Loading account and subscription status...</p>
        </div>
      </main>
    );
  }

  if (route === "/login") {
    return (
      <main className="login-wrap">
        <form className="login-card" onSubmit={loginStep === "request" ? handleStartAuth : handleVerifyAuth}>
          <h1>BossClaw Desktop</h1>
          <p>Sign in to access your subscribed desktop workspace.</p>

          {error ? <p className="error-banner">{error}</p> : null}

          {loginStep === "request" ? (
            <>
              <label htmlFor="email">Email</label>
              <input
                id="email"
                type="email"
                value={emailInput}
                onChange={(event) => setEmailInput(event.target.value)}
                placeholder="you@company.com"
                disabled={isBusy}
                required
              />
              <button type="submit" disabled={isBusy}>
                {isBusy ? "Sending..." : "Send code"}
              </button>
            </>
          ) : (
            <>
              <p className="footnote">Code sent to {verifyEmail}</p>
              <label htmlFor="code">6-digit code</label>
              <input
                id="code"
                inputMode="numeric"
                pattern="[0-9]{6}"
                maxLength={6}
                value={codeInput}
                onChange={(event) => setCodeInput(event.target.value)}
                placeholder="123456"
                disabled={isBusy}
                required
              />
              <button type="submit" disabled={isBusy}>
                {isBusy ? "Verifying..." : "Verify code"}
              </button>
              <button
                type="button"
                className="secondary-btn"
                disabled={isBusy}
                onClick={() => {
                  setLoginStep("request");
                  setCodeInput("");
                  setDevCode(null);
                }}
              >
                Use different email
              </button>
              {!IS_PRODUCTION && devCode ? (
                <p className="footnote">
                  Development code: <strong>{devCode}</strong>
                </p>
              ) : null}
            </>
          )}

          <button
            type="button"
            className="link-btn"
            onClick={() => {
              setShowDiagnostics((previous) => !previous);
              setDiagnosticsResult(null);
            }}
          >
            {showDiagnostics ? "Hide diagnostics" : "Diagnostics"}
          </button>

          {showDiagnostics ? (
            <div className="diagnostics-box">
              <p className="muted">API base: {API_BASE}</p>
              <p className="muted">Ping URL: {healthCheckUrl}</p>
              <button type="button" className="secondary-btn" onClick={() => void pingHealth()} disabled={diagnosticsLoading}>
                {diagnosticsLoading ? "Pinging..." : "Ping /health"}
              </button>
              {diagnosticsResult ? <p className="footnote">{diagnosticsResult}</p> : null}
            </div>
          ) : null}
        </form>
      </main>
    );
  }

  if (route === "/locked") {
    return (
      <main className="login-wrap">
        <div className="login-card">
          <h1>Subscription required</h1>
          <p>Current status: {subscription?.status ?? "free"}</p>
          <button type="button" onClick={() => void openPricing()}>
            Open Subscription Page
          </button>
          <button type="button" className="secondary-btn" onClick={() => void logout()}>
            Logout
          </button>
          {error ? <p className="error-text">{error}</p> : null}
        </div>
      </main>
    );
  }

  return (
    <div className={isRailCollapsed ? "app-shell rail-collapsed" : "app-shell"}>
      <aside className={isRailCollapsed ? "sidebar agent-rail collapsed" : "sidebar agent-rail"}>
        <div className="rail-top">
          {!isRailCollapsed ? (
            <div className="brand">
              <h1>BossClaw</h1>
            </div>
          ) : null}
          <button
            type="button"
            className="secondary-btn rail-toggle-btn"
            onClick={() => setIsRailCollapsed((current) => !current)}
            aria-label={isRailCollapsed ? "Expand sidebar" : "Collapse sidebar"}
            title={isRailCollapsed ? "Expand sidebar" : "Collapse sidebar"}
          >
            {isRailCollapsed ? "›" : "‹"}
          </button>
        </div>

        <nav className="tab-list rail-primary-nav" aria-label="Primary">
          {RAIL_NAV_ITEMS.map((entry) => (
            <button
              key={entry.id}
              type="button"
              className={tab === entry.id ? "tab-btn active" : "tab-btn"}
              onClick={() => setTab(entry.id)}
            >
              {isRailCollapsed ? entry.label.slice(0, 1) : entry.label}
            </button>
          ))}
        </nav>

        <div className="rail-agent-list">
          <div className="rail-agent-list-head">
            {!isRailCollapsed ? <span>Agents</span> : null}
            <button
              type="button"
              className="secondary-btn"
              onClick={() => setShowCreateAgentModal(true)}
              aria-label="Create agent"
              title="Create agent"
            >
              +
            </button>
          </div>
          {agents.length ? (
            agents.map((agent) => {
              const latestRun = runs.find((run) => run.agentId === agent.id) ?? null;
              const pendingCount = pendingApprovals.filter((item) => item.agentId === agent.id).length;
              const missionCounts = missionCountsByAgent.get(agent.id) ?? { total: 0, enabled: 0 };
              const hasEnabledMissions = missionCounts.enabled > 0 && !appSettings.missionsPaused;
              const allMissionsPaused =
                missionCounts.total > 0 && (missionCounts.enabled === 0 || appSettings.missionsPaused);
              const statusClass =
                latestRun?.status === "failed"
                  ? "error"
                  : latestRun?.status === "executing" || latestRun?.status === "planning"
                    ? "working"
                    : pendingCount > 0
                      ? "attention"
                      : "ready";
              const isActiveAgent = agent.id === (selectedAgentId ?? agentPanelAgentId);
              return (
                <button
                  key={agent.id}
                  type="button"
                  className={isActiveAgent ? "rail-agent-btn active" : "rail-agent-btn"}
                  onClick={() => openAgentPanel(agent.id, "chat")}
                  title={agent.name}
                >
                  <span className={`status-dot ${statusClass}`} aria-hidden="true" />
                  {isRailCollapsed ? (
                    <span className="rail-agent-label">{agent.name.slice(0, 1).toUpperCase()}</span>
                  ) : (
                    <span className="rail-agent-meta">
                      <span className="rail-agent-label">{agent.name}</span>
                      {hasEnabledMissions ? (
                        <span className="rail-agent-substatus">
                          <span className="mission-state-dot running" aria-hidden="true" />
                          Running
                        </span>
                      ) : allMissionsPaused ? (
                        <span className="rail-agent-substatus">
                          <span className="mission-state-dot paused" aria-hidden="true" />
                          Paused
                        </span>
                      ) : null}
                    </span>
                  )}
                </button>
              );
            })
          ) : (
            !isRailCollapsed ? <p className="muted">No agents yet.</p> : null
          )}
        </div>

        <div className="sidebar-footer">
          {!isRailCollapsed ? <p>{sessionEmail ?? "Signed in"}</p> : null}
          <button type="button" className="secondary-btn" onClick={() => void logout()}>
            {isRailCollapsed ? "⎋" : "Logout"}
          </button>
        </div>
      </aside>

      <main className="main-area">
        {tab === "missionControl" ? (
          <section className="mission-stage">
            <div className="mission-toolbar">
              {workspaces.length ? (
                <label className="compact-field">
                  <span>Workspace</span>
                  <select
                    value={selectedWorkspaceId ?? ""}
                    onChange={(event) => setSelectedWorkspaceId(event.target.value || null)}
                  >
                    {workspaces.map((workspace) => (
                      <option key={workspace.id} value={workspace.id}>
                        {workspace.name}
                      </option>
                    ))}
                  </select>
                </label>
              ) : (
                <span />
              )}
              <div className="mission-indicators">
                <button
                  type="button"
                  className="secondary-btn"
                  onClick={() => setShowMissionApprovals((previous) => !previous)}
                >
                  Approvals ({pendingApprovals.length})
                </button>
                <span className="pill active">
                  Usage Today: {usageSummaries.today.totalTokens.toLocaleString()} tokens
                </span>
              </div>
            </div>

            {showMissionApprovals ? (
              <article className="entity-card">
                <div className="entity-head">
                  <h3>Pending Approvals</h3>
                </div>
                {pendingApprovals.length ? (
                  <ul className="simple-list">
                    {pendingApprovals.map((item) => (
                      <li key={item.id}>
                        <strong>{item.kind}</strong>: {item.message}
                      </li>
                    ))}
                  </ul>
                ) : (
                  <p className="muted">No pending approvals.</p>
                )}
              </article>
            ) : null}

            {agents.length ? (
              <div className="card-list mission-agent-canvas">
                {agents.map((agent) => {
                  const latestAgentRun = runs.find((run) => run.agentId === agent.id) ?? null;
                  const pendingCount = pendingApprovals.filter((item) => item.agentId === agent.id).length;
                  const latestStatus = latestAgentRun?.status ?? "planned";
                  const statusTone =
                    latestStatus === "failed"
                      ? "error"
                      : latestStatus === "cancelled"
                        ? "neutral"
                      : pendingCount > 0
                        ? "warning"
                        : latestStatus === "executing" || latestStatus === "planning"
                          ? "primary"
                          : "accent";
                  return (
                    <article
                      key={agent.id}
                      className="entity-card mission-agent-card"
                      role="button"
                      tabIndex={0}
                      onClick={() => openAgentPanel(agent.id, "chat")}
                      onKeyDown={(event) => {
                        if (event.key === "Enter" || event.key === " ") {
                          event.preventDefault();
                          openAgentPanel(agent.id, "chat");
                        }
                      }}
                    >
                      <div className="entity-head">
                        <div className="agent-headline">
                          <GlowRing
                            state={agentGlowStateFromStatus(
                              latestAgentRun?.status ?? null,
                              latestAgentRun?.finishedAt ?? null,
                              pendingCount > 0
                            )}
                          >
                            <span className="agent-avatar-label">{agent.name.slice(0, 1).toUpperCase()}</span>
                          </GlowRing>
                          <div>
                            <h3>{agent.name}</h3>
                            {latestAgentRun?.summary ? (
                              <p className="muted single-line">{latestAgentRun.summary}</p>
                            ) : null}
                          </div>
                        </div>
                        <StatusBadge tone={statusTone}>
                          {pendingCount > 0 ? `${pendingCount} approvals` : latestStatus}
                        </StatusBadge>
                      </div>
                      <p className="muted">
                        Last activity:{" "}
                        {latestAgentRun ? new Date(latestAgentRun.startedAt).toLocaleString() : "—"}
                      </p>
                    </article>
                  );
                })}
              </div>
            ) : (
              <div className="mission-empty">
                <p className="muted">No agents yet. Create one in Agents.</p>
              </div>
            )}
          </section>
        ) : null}

        {tab === "agents" ? (
          <Surface as="section" className="panel chat-panel main-chat-panel" elevation="level2">
            {agentPanelAgent ? (
              <>
                <div className="section-header chat-header-row">
                  <div className="chat-header-title">
                    {isEditingAgentName ? (
                      <div className="chat-rename-wrap">
                        <input
                          value={agentNameDraft}
                          onChange={(event) => setAgentNameDraft(event.target.value)}
                          onKeyDown={(event) => {
                            if (event.key === "Enter") {
                              event.preventDefault();
                              void commitAgentRename();
                            }
                            if (event.key === "Escape") {
                              event.preventDefault();
                              cancelAgentRename();
                            }
                          }}
                          autoFocus
                        />
                        <button type="button" className="secondary-btn" onClick={() => void commitAgentRename()}>
                          Save
                        </button>
                        <button type="button" className="secondary-btn" onClick={cancelAgentRename}>
                          Cancel
                        </button>
                      </div>
                    ) : (
                      <>
                        <div className="chat-title-row">
                          <h2>{agentPanelAgent.name}</h2>
                          <button
                            type="button"
                            className="secondary-btn chat-icon-btn"
                            aria-label="Rename agent"
                            title="Rename agent"
                            onClick={startAgentRename}
                          >
                            ✎
                          </button>
                        </div>
                        <div className="chat-header-meta">
                          <span className={`agent-presence agent-presence-${agentPresence}`}>
                            <span className="agent-presence-dot" />
                            <span>{agentPresenceLabel}</span>
                          </span>
                          <span className="muted">{agentPanelProviderLabel}</span>
                        </div>
                      </>
                    )}
                  </div>
                  <div className="run-toolbar chat-header-actions">
                    <span className="muted chat-header-stat">
                      Approvals {agentPanelPendingApprovals.length}
                    </span>
                    <span className="muted chat-header-stat">
                      Today {usageSummaries.today.totalTokens.toLocaleString()} tokens
                    </span>
                    {activePlannedRun ? (
                      <button
                        type="button"
                        className="link-btn"
                        onClick={() => setShowPlanDetails((current) => !current)}
                      >
                        {showPlanDetails ? "Hide Details" : "Details"}
                      </button>
                    ) : null}
                    {activeChatRunId ? (
                      <button type="button" className="secondary-btn" onClick={() => void cancelChatStream()}>
                        Stop
                      </button>
                    ) : null}
                    {agentEnabledMissionCount > 0 ? (
                      <div className="header-mission-control" data-header-mission-menu>
                        <button
                          type="button"
                          className="mission-indicator-btn"
                          onClick={() => setShowHeaderMissionMenu((current) => !current)}
                        >
                          Missions Active • {agentEnabledMissionCount}
                        </button>
                        {showHeaderMissionMenu ? (
                          <div className="mission-header-menu">
                            <button
                              type="button"
                              className="mission-overflow-item"
                              onClick={() => {
                                toggleMissionsPaused();
                                setShowHeaderMissionMenu(false);
                              }}
                            >
                              {appSettings.missionsPaused ? "Resume missions" : "Pause all missions"}
                            </button>
                            <button
                              type="button"
                              className="mission-overflow-item"
                              onClick={() => {
                                setAgentPanelTab("agentSettings");
                                setShowHeaderMissionMenu(false);
                              }}
                            >
                              View in Agent Settings
                            </button>
                          </div>
                        ) : null}
                      </div>
                    ) : null}
                    <ToggleSwitch
                      checked={autonomyMode === "fsd"}
                      onChange={(next) => setAutonomyMode(next ? "fsd" : "autopilot")}
                      label="Autonomy"
                      offLabel="Autopilot"
                      onLabel="FSD"
                    />
                    <button
                      type="button"
                      className="secondary-btn chat-delete-btn"
                      onClick={() => deleteAgent(agentPanelAgent.id)}
                    >
                      Delete
                    </button>
                  </div>
                </div>

                <div className="chat-subtabs">
                  <button
                    type="button"
                    className={agentPanelTab === "chat" ? "tab-btn active" : "tab-btn"}
                    onClick={() => setAgentPanelTab("chat")}
                  >
                    Chat
                  </button>
                  <button
                    type="button"
                    className={agentPanelTab === "activity" ? "tab-btn active" : "tab-btn"}
                    onClick={() => setAgentPanelTab("activity")}
                  >
                    Activity
                  </button>
                  <button
                    type="button"
                    className={agentPanelTab === "approvals" ? "tab-btn active" : "tab-btn"}
                    onClick={() => setAgentPanelTab("approvals")}
                  >
                    Approvals{agentPanelPendingApprovals.length ? ` (${agentPanelPendingApprovals.length})` : ""}
                  </button>
                  <button
                    type="button"
                    className={agentPanelTab === "agentSettings" ? "tab-btn active" : "tab-btn"}
                    onClick={() => setAgentPanelTab("agentSettings")}
                  >
                    Agent Settings
                  </button>
                </div>

                {agentPanelTab === "chat" && chatError ? <p className="error-banner">{chatError}</p> : null}
                {agentPanelTab === "chat" && missingAgentProviderKeyMessage ? (
                  <p className="muted">{missingAgentProviderKeyMessage}</p>
                ) : null}
                {agentPanelTab === "chat" && chatNotice ? <p className="muted">{chatNotice}</p> : null}
                {agentPanelTab === "chat" && undoState ? (
                  <div className="diagnostics-box">
                    <p className="muted">{undoState.message}</p>
                    <button type="button" className="secondary-btn" onClick={() => void undoRecentChange()}>
                      Undo
                    </button>
                  </div>
                ) : null}

                {agentPanelTab === "chat" && activePlannedRun && showPlanDetails ? (
                  <article className="entity-card">
                    <div className="entity-head">
                      <h3>Details</h3>
                      <StatusBadge tone={activePlannedRun.status === "failed" ? "error" : "primary"}>
                        {activePlannedRun.status}
                      </StatusBadge>
                    </div>
                    <p className="muted">Planner attempts: {activePlannedRun.plannerAttempts}</p>

                    {activePlannedRun.plan ? (
                      <ul className="simple-list">
                        {activePlannedRun.plan.steps.map((step, index) => {
                          const risk = estimateStepRisk(step);
                          const stepState = activePlannedRun.stepStates[index];
                          return (
                            <li key={`${activePlannedRun.runId}-step-${index}`}>
                              <strong>{step.title}</strong> · {step.tool}
                              {" · "}
                              <span
                                className={
                                  risk === "low" ? "pill active" : risk === "medium" ? "pill medium" : "pill high"
                                }
                              >
                                {risk}
                              </span>
                              {" · "}
                              <span className="muted">{stepState?.status ?? "pending"}</span>
                              {stepState?.note ? <span className="muted"> ({stepState.note})</span> : null}
                            </li>
                          );
                        })}
                      </ul>
                    ) : null}

                    {agentPanelAgent?.policy.loggingMode === "detailed" &&
                    activePlannedRun.plannerErrors.length ? (
                      <details>
                        <summary>Planner debug</summary>
                        <ul className="simple-list">
                          {activePlannedRun.plannerErrors.map((entry, index) => (
                            <li key={`${activePlannedRun.runId}-planner-error-${index}`}>{entry}</li>
                          ))}
                        </ul>
                      </details>
                    ) : null}

                    <div className="row-actions">
                      {activePlannedRun.status === "planned" ? (
                        <FloatingPrimaryButton type="button" onClick={() => void runPlannedExecution()}>
                          Continue
                        </FloatingPrimaryButton>
                      ) : null}
                      {activePlannedRun.status === "failed" ? (
                        <button type="button" className="secondary-btn" onClick={() => void retryPlanning()}>
                          Retry
                        </button>
                      ) : null}
                    </div>
                  </article>
                ) : null}

                {agentPanelTab === "chat" && activePlannedRun?.configProposals.map((proposal) => (
                  <ChangeCard
                    key={proposal.id}
                    proposal={proposal}
                    onApply={() => void applyPlanConfigProposal(proposal.id)}
                    onCancel={() => {
                      updatePlannedRun(activePlannedRun.runId, (current) => ({
                        ...current,
                        configProposals: current.configProposals.filter((item) => item.id !== proposal.id)
                      }));
                      setPendingWebApprovals((previous) =>
                        previous.filter((item) => item.proposalId !== proposal.id)
                      );
                    }}
                  />
                ))}

                {agentPanelTab === "chat" && pendingQuickExtractApproval ? (
                  <ChangeCard
                    proposal={pendingQuickExtractApproval.proposal}
                    onApply={() => void applyQuickExtractProposal()}
                    onCancel={cancelQuickExtractProposal}
                  />
                ) : null}

                {agentPanelTab === "chat" && pendingQuickFileApproval ? (
                  <ChangeCard
                    proposal={pendingQuickFileApproval.proposal}
                    onApply={() => void applyQuickFileProposal()}
                    onCancel={cancelQuickFileProposal}
                  />
                ) : null}

                {agentPanelTab === "chat" ? (
                  <div className="chat-shell">
                    <div
                      ref={chatThreadRef}
                      className="chat-scroll-area"
                      onScroll={handleChatThreadScroll}
                    >
                      <div className="chat-scroll-inner chat-column">
                        {agentPanelAgent && activeHandshakeStep === "name" ? (
                          <div className="diagnostics-box">
                            <p className="muted">
                              How should I address you?
                            </p>
                            <button type="button" className="link-btn" onClick={() => void skipNameHandshake()}>
                              Skip
                            </button>
                          </div>
                        ) : null}

                        {agentPanelAgent && activeHandshakeStep === "tone" ? (
                          <div className="diagnostics-box">
                            <p className="muted">Choose how this agent should communicate with you.</p>
                            <div className="row-actions">
                              <button
                                type="button"
                                className="secondary-btn"
                                onClick={() => void applyToneChoice(agentPanelAgent, "concise")}
                              >
                                Concise
                              </button>
                              <button
                                type="button"
                                className="secondary-btn"
                                onClick={() => void applyToneChoice(agentPanelAgent, "detailed")}
                              >
                                Detailed
                              </button>
                              <button type="button" className="link-btn" onClick={() => void skipToneHandshake()}>
                                Skip
                              </button>
                            </div>
                          </div>
                        ) : null}

                        {agentPanelAgent && activeHandshakeStep === "agent_name" ? (
                          <div className="diagnostics-box">
                            {activeAgentNameConfirmation ? (
                              <>
                                <p className="muted">
                                  Got it — I&apos;ll go by {activeAgentNameConfirmation.name}.
                                </p>
                                <div className="row-actions">
                                  {canEditAgentNameConfirmation ? (
                                    <button
                                      type="button"
                                      className="link-btn"
                                      onClick={() => editAgentNameHandshake(agentPanelAgent.id)}
                                    >
                                      Edit
                                    </button>
                                  ) : null}
                                  <button type="button" className="link-btn" onClick={() => void skipAgentNameHandshake()}>
                                    Skip
                                  </button>
                                </div>
                              </>
                            ) : (
                              <>
                                <p className="muted">What would you like to call me?</p>
                                <button type="button" className="link-btn" onClick={() => void skipAgentNameHandshake()}>
                                  Skip
                                </button>
                              </>
                            )}
                          </div>
                        ) : null}

                        <div className="chat-thread">
                          {agentPanelMessages.length ? (
                            agentPanelMessages.map((message) => {
                              const isMissionUpdate = message.kind === "mission_update";
                              const showMissionDetailsButton =
                                isMissionUpdate &&
                                message.role === "assistant" &&
                                typeof message.runId === "string" &&
                                message.runId.length > 0;
                              return (
                                <article
                                  key={message.id}
                                  className={
                                    isMissionUpdate
                                      ? "chat-message mission-update"
                                      : message.role === "assistant"
                                        ? "chat-message assistant"
                                        : "chat-message user"
                                  }
                                >
                                  {isMissionUpdate ? (
                                    <span className="chat-message-badge">🕒 Scheduled</span>
                                  ) : (
                                    <strong>{message.role === "assistant" ? assistantDisplayName : "You"}</strong>
                                  )}
                                  <p className={isMissionUpdate ? "chat-message-structured" : undefined}>
                                    {message.content ||
                                      (message.role === "assistant" && activeChatRunId === message.runId ? "..." : "")}
                                  </p>
                                  {showMissionDetailsButton ? (
                                    <button
                                      type="button"
                                      className="link-btn"
                                      onClick={() => {
                                        if (!message.runId) {
                                          return;
                                        }
                                        openRunDetails(message.agentId, message.runId);
                                      }}
                                    >
                                      View details
                                    </button>
                                  ) : null}
                                </article>
                              );
                            })
                          ) : (
                            <p className="muted chat-empty">No messages yet.</p>
                          )}
                        </div>
                        <div ref={chatBottomAnchorRef} className="chat-bottom-anchor" aria-hidden="true" />
                      </div>
                    </div>

                    {showJumpToLatest ? (
                      <div className="chat-jump-row">
                        <div className="chat-column">
                          <button
                            type="button"
                            className="secondary-btn chat-jump-btn"
                            onClick={() => scrollChatToBottom("smooth")}
                          >
                            Jump to latest
                          </button>
                        </div>
                      </div>
                    ) : null}

                    <form
                      className="chat-input-bar"
                      onSubmit={(event) => {
                        event.preventDefault();
                        void sendChatPrompt();
                      }}
                    >
                      <div className="chat-composer-inner chat-column">
                        <input
                          ref={chatInputRef}
                          id="chat-prompt"
                          value={chatInput}
                          onChange={(event) => setChatInput(event.target.value)}
                          placeholder="Tell this agent what to do..."
                          disabled={isChatActionBusy}
                        />
                        <div className="row-actions">
                          <button
                            type="submit"
                            disabled={!agentPanelAgent || !chatInput.trim() || isChatActionBusy}
                          >
                            {activePlannedRun?.status === "planning"
                              ? "Planning..."
                              : activePlannedRun?.status === "executing" ||
                                  activePlannedRun?.status === "executing_direct"
                                ? "Running..."
                                : "Send"}
                          </button>
                          {activeChatRunId ? (
                            <button type="button" className="secondary-btn" onClick={() => void cancelChatStream()}>
                              Stop
                            </button>
                          ) : null}
                        </div>
                      </div>
                    </form>
                  </div>
                ) : null}

                {agentPanelTab === "activity" ? (
                  <article className="entity-card">
                    <h3>Activity</h3>
                    {agentPanelRuns.length ? (
                      <ul className="simple-list">
                        {agentPanelRuns.map((run) => (
                          <li key={run.id}>
                            <strong>{run.title}</strong> · {run.status}
                            <span className="muted"> ({new Date(run.startedAt).toLocaleString()})</span>
                          </li>
                        ))}
                      </ul>
                    ) : (
                      <p className="muted">No activity yet for this agent.</p>
                    )}
                  </article>
                ) : null}

                {agentPanelTab === "approvals" ? (
                  <article className="entity-card">
                    <div className="entity-head">
                      <h3>Approvals</h3>
                      {!IS_PRODUCTION ? (
                        <button type="button" className="secondary-btn" onClick={createTestApproval}>
                          Create Test Approval
                        </button>
                      ) : null}
                    </div>
                    {agentPanelPendingApprovals.length ? (
                      <ul className="simple-list">
                        {agentPanelPendingApprovals.map((item) => (
                          <li key={item.id}>
                            <strong>{item.kind}</strong>: {item.message}
                          </li>
                        ))}
                      </ul>
                    ) : (
                      <p className="muted">No approvals for this agent.</p>
                    )}
                  </article>
                ) : null}

                {agentPanelTab === "agentSettings" && agentPanelAgent ? (
                  <>
                    <article className="entity-card">
                      <h3>Agent Settings</h3>
                      <div className="policy-grid">
                        <label>
                          Provider
                          <select
                            value={agentPanelAgent.provider}
                            onChange={(event) =>
                              updateAgentPolicy(agentPanelAgent.id, {
                                provider: event.target.value as AgentProvider
                              })
                            }
                          >
                            {AGENT_PROVIDER_OPTIONS.map((option) => (
                              <option key={option.value} value={option.value}>
                                {option.label}
                              </option>
                            ))}
                          </select>
                        </label>
                        <label>
                          How Should This Agent Address You?
                          <input
                            value={agentPanelAgent.preferredName ?? ""}
                            placeholder="Name or preferred address"
                            onChange={(event) =>
                              updateAgentPolicy(agentPanelAgent.id, {
                                preferredName: event.target.value.trim() || null,
                                hasAskedName: true
                              })
                            }
                          />
                        </label>
                        <label>
                          Communication Style
                          <select
                            value={agentPanelAgent.tone ?? "concise"}
                            onChange={(event) =>
                              updateAgentPolicy(agentPanelAgent.id, {
                                tone: event.target.value as "concise" | "detailed",
                                hasAskedTone: true
                              })
                            }
                          >
                            <option value="concise">Concise</option>
                            <option value="detailed">Detailed</option>
                          </select>
                        </label>
                        <label>
                          Activity Detail
                          <select
                            value={agentPanelAgent.policy.loggingMode}
                            onChange={(event) =>
                              updateAgentPolicy(agentPanelAgent.id, {
                                loggingMode: event.target.value as LoggingMode
                              })
                            }
                          >
                            <option value="simple">Simple</option>
                            <option value="detailed">Detailed</option>
                          </select>
                        </label>
                        <label>
                          Memory Mode
                          <select
                            value={agentPanelAgent.policy.memoryMode}
                            onChange={(event) =>
                              updateAgentPolicy(agentPanelAgent.id, {
                                memoryMode: event.target.value as MemoryMode
                              })
                            }
                          >
                            <option value="isolated">Isolated</option>
                            <option value="shared">Shared</option>
                          </select>
                        </label>
                      </div>
                      {agentPanelProviderWarning ? (
                        <p className="error-banner">{agentPanelProviderWarning}</p>
                      ) : null}
                      <ToggleSwitch
                        checked={autonomyMode === "fsd"}
                        onChange={(next) => setAutonomyMode(next ? "fsd" : "autopilot")}
                        label="Autonomy"
                        offLabel="Autopilot"
                        onLabel="FSD"
                      />
                    </article>

                    <article className="entity-card">
                      <div className="entity-head">
                        <h3>Missions</h3>
                        <button type="button" onClick={openCreateMissionModal}>
                          New Mission
                        </button>
                      </div>
                      <p className="muted">Schedule recurring goals for this agent.</p>
                      <div className="row-actions">
                        <button type="button" className="secondary-btn" onClick={showRecurringPlaceholder}>
                          Make this recurring
                        </button>
                      </div>

                      {agentPanelMissions.length ? (
                        <div className="mission-settings-list">
                          {agentPanelMissions.map((mission) => (
                            <article key={mission.id} className="entity-card mission-settings-item">
                              <div className="entity-head">
                                <div>
                                  <h4>{mission.title}</h4>
                                  <p className="muted">{formatMissionSchedule(mission.schedule)}</p>
                                </div>
                                <div className="mission-card-controls">
                                  <button
                                    type="button"
                                    className="mission-control-btn"
                                    onClick={() => void toggleMissionEnabled(mission)}
                                  >
                                    {mission.enabled ? "Pause" : "Resume"}
                                  </button>
                                  <div className="mission-control-overflow" data-mission-menu>
                                    <button
                                      type="button"
                                      className="mission-overflow-trigger"
                                      aria-label={`Open actions for ${mission.title}`}
                                      title="Mission actions"
                                      onClick={() =>
                                        setOpenMissionMenuId((current) =>
                                          current === mission.id ? null : mission.id
                                        )
                                      }
                                    >
                                      ...
                                    </button>
                                    {openMissionMenuId === mission.id ? (
                                      <div className="mission-overflow-menu">
                                        <button type="button" className="mission-overflow-item" disabled>
                                          Edit
                                        </button>
                                        <button type="button" className="mission-overflow-item" disabled>
                                          Duplicate
                                        </button>
                                        <button
                                          type="button"
                                          className="mission-overflow-item"
                                          onClick={() => requestMissionDelete(mission)}
                                        >
                                          Delete
                                        </button>
                                      </div>
                                    ) : null}
                                  </div>
                                </div>
                              </div>
                              <p className="muted">Goal: {mission.goal}</p>
                              <div className="mission-chat-settings">
                                <label>
                                  Post updates to chat
                                  <select
                                    value={mission.chatPosting}
                                    onChange={(event) => {
                                      const nextValue = event.target.value as MissionRunChatPosting;
                                      void saveMissionUpdates(
                                        mission.id,
                                        { chatPosting: nextValue },
                                        `Set mission chat updates to ${nextValue} for ${mission.title}`
                                      ).catch(() => {
                                        setError("Unable to update mission chat settings.");
                                      });
                                    }}
                                  >
                                    <option value="off">Off</option>
                                    <option value="summary">Summary (recommended)</option>
                                    <option value="verbose">Every run (verbose)</option>
                                  </select>
                                </label>
                                <label className="check-row">
                                  <input
                                    type="checkbox"
                                    checked={mission.collapseRepeats}
                                    onChange={(event) => {
                                      void saveMissionUpdates(
                                        mission.id,
                                        { collapseRepeats: event.target.checked },
                                        `Set collapse repeats for ${mission.title}`
                                      ).catch(() => {
                                        setError("Unable to update mission chat settings.");
                                      });
                                    }}
                                  />
                                  <span>Collapse repeats</span>
                                </label>
                                {mission.chatPosting === "verbose" ? (
                                  <p className="muted">Verbose mode can be noisy.</p>
                                ) : null}
                              </div>
                              <p className="muted">
                                Next run: {mission.enabled ? formatMissionNextRun(mission.nextRunAt) : "Disabled"}
                              </p>
                              {mission.lastRunAt ? (
                                <p className="muted">Last run: {new Date(mission.lastRunAt).toLocaleString()}</p>
                              ) : null}
                            </article>
                          ))}
                        </div>
                      ) : (
                        <p className="muted">No missions yet for this agent.</p>
                      )}
                    </article>
                  </>
                ) : null}
              </>
            ) : (
              <div className="mission-empty">
                <p className="muted">Create your first agent.</p>
                <FloatingPrimaryButton type="button" onClick={() => setShowCreateAgentModal(true)}>
                  Create your first agent
                </FloatingPrimaryButton>
              </div>
            )}
          </Surface>
        ) : null}

        {tab === "settings" ? (
          <section className="panel settings-panel settings-shell">
            <div className="settings-tabs">
              <button
                type="button"
                className={settingsSection === "appearance" ? "settings-tab active" : "settings-tab"}
                onClick={() => setSettingsSection("appearance")}
              >
                Appearance
              </button>
              <button
                type="button"
                className={settingsSection === "keys" ? "settings-tab active" : "settings-tab"}
                onClick={() => setSettingsSection("keys")}
              >
                Keys
              </button>
              <button
                type="button"
                className={settingsSection === "usage" ? "settings-tab active" : "settings-tab"}
                onClick={() => setSettingsSection("usage")}
              >
                Usage
              </button>
              <button
                type="button"
                className={settingsSection === "changes" ? "settings-tab active" : "settings-tab"}
                onClick={() => setSettingsSection("changes")}
              >
                Changes
              </button>
              <button
                type="button"
                className={settingsSection === "advanced" ? "settings-tab active" : "settings-tab"}
                onClick={() => setSettingsSection("advanced")}
              >
                Advanced
              </button>
              <button
                type="button"
                className={settingsSection === "about" ? "settings-tab active" : "settings-tab"}
                onClick={() => setSettingsSection("about")}
              >
                About
              </button>
            </div>
          </section>
        ) : null}

        {tab === "settings" && settingsSection === "appearance" ? (
          <section className="panel settings-panel">
            <h2>Appearance</h2>
            <div className="settings-stack">
              <SettingsSectionCard title="Color Mode" description="Choose how the app follows light and dark mode.">
                <div className="appearance-grid">
                  <button
                    type="button"
                    className={appSettings.appearance === "light" ? "appearance-option active" : "appearance-option"}
                    onClick={() =>
                      setAppSettings((previous) => ({
                        ...previous,
                        appearance: "light"
                      }))
                    }
                  >
                    Light
                  </button>
                  <button
                    type="button"
                    className={appSettings.appearance === "dark" ? "appearance-option active" : "appearance-option"}
                    onClick={() =>
                      setAppSettings((previous) => ({
                        ...previous,
                        appearance: "dark"
                      }))
                    }
                  >
                    Dark
                  </button>
                  <button
                    type="button"
                    className={appSettings.appearance === "system" ? "appearance-option active" : "appearance-option"}
                    onClick={() =>
                      setAppSettings((previous) => ({
                        ...previous,
                        appearance: "system"
                      }))
                    }
                  >
                    Follow System
                  </button>
                </div>
                <p className="muted">Current mode: {resolvedAppearance}</p>
              </SettingsSectionCard>

              <SettingsSectionCard title="Theme" description="Choose a visual skin for the workspace.">
                <div className="appearance-grid">
                  {DESKTOP_THEMES.map((theme) => (
                    <button
                      key={theme.key}
                      type="button"
                      className={appSettings.skin === theme.key ? "appearance-option active" : "appearance-option"}
                      onClick={() =>
                        setAppSettings((previous) => ({
                          ...previous,
                          skin: theme.key
                        }))
                      }
                    >
                      {theme.label}
                    </button>
                  ))}
                </div>
                <p className="muted">
                  {DESKTOP_THEMES.find((theme) => theme.key === appSettings.skin)?.description}
                </p>
              </SettingsSectionCard>
            </div>
          </section>
        ) : null}

        {tab === "settings" && settingsSection === "keys" ? (
          <section className="panel settings-panel">
            <h2>Keys</h2>
            <p className="muted">Secrets are stored in macOS Keychain.</p>
            <div className="settings-stack">
              <SettingsSectionCard
                title="OpenAI-Compatible"
                description="Choose a model, set a base URL, and save your key."
                actions={
                  <span className={vaultStatus.openai_compat_api_key ? "pill active" : "pill inactive"}>
                    {vaultStatus.openai_compat_api_key ? "Set" : "Not set"}
                  </span>
                }
              >
                <label>
                  Base URL (HTTPS)
                  <input
                    value={appSettings.openaiCompatBaseUrl}
                    onChange={(event) =>
                      setAppSettings((previous) => ({
                        ...previous,
                        openaiCompatBaseUrl: event.target.value
                      }))
                    }
                    placeholder="https://api.openai.com"
                  />
                </label>
                <label>
                  Model Tier
                  <select
                    value={appSettings.openaiCompatTier}
                    onChange={(event) =>
                      updateModelSettings((previous) => ({
                        ...previous,
                        openaiCompatTier: event.target.value as ModelTier,
                        openaiCompatModelMode: "tier",
                        openaiCompatModelId: MODEL_TIER_DEFAULTS.openai_compat[event.target.value as ModelTier]
                      }))
                    }
                  >
                    {MODEL_TIER_OPTIONS.map((option) => (
                      <option key={option.value} value={option.value}>
                        {option.label}
                      </option>
                    ))}
                  </select>
                </label>
                {appSettings.openaiCompatModelMode === "tier" ? (
                  <button
                    type="button"
                    className="link-btn"
                    onClick={() =>
                      updateModelSettings((previous) => ({
                        ...previous,
                        openaiCompatModelMode: "all",
                        openaiCompatModelId: resolveModel("openai_compat", previous)
                      }))
                    }
                  >
                    Show all models
                  </button>
                ) : (
                  <>
                    <label>
                      All Models
                      <select
                        value={isOpenAiCompatCustomModel ? CUSTOM_MODEL_OPTION : appSettings.openaiCompatModelId.trim()}
                        onChange={(event) =>
                          updateModelSettings((previous) => ({
                            ...previous,
                            openaiCompatModelMode:
                              event.target.value === CUSTOM_MODEL_OPTION ? "custom" : "all",
                            openaiCompatModelId:
                              event.target.value === CUSTOM_MODEL_OPTION ? "" : event.target.value
                          }))
                        }
                      >
                        {openAiCompatModelChoices.map((model) => (
                          <option key={model} value={model}>
                            {model}
                          </option>
                        ))}
                        <option value={CUSTOM_MODEL_OPTION}>Custom...</option>
                      </select>
                    </label>
                    {isOpenAiCompatCustomModel ? (
                      <label>
                        Custom Model
                        <input
                          value={appSettings.openaiCompatModelId}
                          onChange={(event) =>
                            updateModelSettings((previous) => ({
                              ...previous,
                              openaiCompatModelMode: "custom",
                              openaiCompatModelId: event.target.value
                            }))
                          }
                          placeholder={MODEL_TIER_DEFAULTS.openai_compat.balanced}
                        />
                      </label>
                    ) : null}
                    <button
                      type="button"
                      className="link-btn"
                      onClick={() =>
                        updateModelSettings((previous) => ({
                          ...previous,
                          openaiCompatModelMode: "tier",
                          openaiCompatModelId: MODEL_TIER_DEFAULTS.openai_compat[previous.openaiCompatTier]
                        }))
                      }
                    >
                      Use tier defaults
                    </button>
                  </>
                )}
                <p className="muted">Current model: {resolveModel("openai_compat", appSettings)}</p>
                <label>
                  API Key
                  <input
                    type="password"
                    value={vaultInputs.openai_compat_api_key}
                    placeholder="Paste key and click Save"
                    onChange={(event) =>
                      setVaultInputs((previous) => ({
                        ...previous,
                        openai_compat_api_key: event.target.value
                      }))
                    }
                  />
                </label>
                <div className="row-actions">
                  {appSettings.openaiCompatModelMode !== "tier" ? (
                    <button
                      type="button"
                      className="secondary-btn"
                      onClick={() => void refreshOpenAiCompatModels()}
                      disabled={isRefreshingOpenAiCompatModels}
                    >
                      {isRefreshingOpenAiCompatModels ? "Refreshing..." : "Refresh Models"}
                    </button>
                  ) : null}
                  <button
                    type="button"
                    onClick={() => void saveOpenAiCompatProvider()}
                  >
                    Save
                  </button>
                </div>
                {openAiCompatModelRefreshError ? (
                  <p className="muted">{openAiCompatModelRefreshError}</p>
                ) : null}
                {settingsMessage ? <p className="muted">{settingsMessage}</p> : null}
              </SettingsSectionCard>

              <SettingsSectionCard
                title="OpenAI"
                actions={
                  <span className={vaultStatus.openai_api_key ? "pill active" : "pill inactive"}>
                    {vaultStatus.openai_api_key ? "Set" : "Not set"}
                  </span>
                }
              >
                <label>
                  Model Tier
                  <select
                    value={appSettings.openaiTier}
                    onChange={(event) =>
                      updateModelSettings((previous) => ({
                        ...previous,
                        openaiTier: event.target.value as ModelTier,
                        openaiModelMode: "tier",
                        openaiModelId: MODEL_TIER_DEFAULTS.openai[event.target.value as ModelTier]
                      }))
                    }
                  >
                    {MODEL_TIER_OPTIONS.map((option) => (
                      <option key={option.value} value={option.value}>
                        {option.label}
                      </option>
                    ))}
                  </select>
                </label>
                {appSettings.openaiModelMode === "tier" ? (
                  <button
                    type="button"
                    className="link-btn"
                    onClick={() =>
                      updateModelSettings((previous) => ({
                        ...previous,
                        openaiModelMode: "all",
                        openaiModelId: resolveModel("openai", previous)
                      }))
                    }
                  >
                    Show all models
                  </button>
                ) : (
                  <>
                    <label>
                      All Models
                      <select
                        value={isOpenAiCustomModel ? CUSTOM_MODEL_OPTION : appSettings.openaiModelId.trim()}
                        onChange={(event) =>
                          updateModelSettings((previous) => ({
                            ...previous,
                            openaiModelMode: event.target.value === CUSTOM_MODEL_OPTION ? "custom" : "all",
                            openaiModelId: event.target.value === CUSTOM_MODEL_OPTION ? "" : event.target.value
                          }))
                        }
                      >
                        {openAiModelChoices.map((model) => (
                          <option key={model} value={model}>
                            {model}
                          </option>
                        ))}
                        <option value={CUSTOM_MODEL_OPTION}>Custom...</option>
                      </select>
                    </label>
                    {isOpenAiCustomModel ? (
                      <label>
                        Custom Model
                        <input
                          value={appSettings.openaiModelId}
                          onChange={(event) =>
                            updateModelSettings((previous) => ({
                              ...previous,
                              openaiModelMode: "custom",
                              openaiModelId: event.target.value
                            }))
                          }
                          placeholder={MODEL_TIER_DEFAULTS.openai.balanced}
                        />
                      </label>
                    ) : null}
                    <button
                      type="button"
                      className="link-btn"
                      onClick={() =>
                        updateModelSettings((previous) => ({
                          ...previous,
                          openaiModelMode: "tier",
                          openaiModelId: MODEL_TIER_DEFAULTS.openai[previous.openaiTier]
                        }))
                      }
                    >
                      Use tier defaults
                    </button>
                  </>
                )}
                <p className="muted">Current model: {resolveModel("openai", appSettings)}</p>
                <label>
                  API Key
                  <input
                    type="password"
                    value={vaultInputs.openai_api_key}
                    placeholder="Paste key and click Save"
                    onChange={(event) =>
                      setVaultInputs((previous) => ({
                        ...previous,
                        openai_api_key: event.target.value
                      }))
                    }
                  />
                </label>
                <div className="row-actions">
                  {appSettings.openaiModelMode !== "tier" ? (
                    <button
                      type="button"
                      className="secondary-btn"
                      onClick={() => void refreshOpenAiModels()}
                      disabled={isRefreshingOpenAiModels}
                    >
                      {isRefreshingOpenAiModels ? "Refreshing..." : "Refresh Models"}
                    </button>
                  ) : null}
                  <button type="button" onClick={() => void saveOpenAiProvider()}>
                    Save
                  </button>
                </div>
                {openAiModelRefreshError ? <p className="muted">{openAiModelRefreshError}</p> : null}
              </SettingsSectionCard>

              <SettingsSectionCard
                title="Anthropic"
                actions={
                  <span className={vaultStatus.anthropic_api_key ? "pill active" : "pill inactive"}>
                    {vaultStatus.anthropic_api_key ? "Set" : "Not set"}
                  </span>
                }
              >
                <label>
                  Model Tier
                  <select
                    value={appSettings.anthropicTier}
                    onChange={(event) =>
                      updateModelSettings((previous) => ({
                        ...previous,
                        anthropicTier: event.target.value as ModelTier,
                        anthropicModelMode: "tier",
                        anthropicModelId: MODEL_TIER_DEFAULTS.anthropic[event.target.value as ModelTier]
                      }))
                    }
                  >
                    {MODEL_TIER_OPTIONS.map((option) => (
                      <option key={option.value} value={option.value}>
                        {option.label}
                      </option>
                    ))}
                  </select>
                </label>
                {appSettings.anthropicModelMode === "tier" ? (
                  <button
                    type="button"
                    className="link-btn"
                    onClick={() =>
                      updateModelSettings((previous) => ({
                        ...previous,
                        anthropicModelMode: "all",
                        anthropicModelId: resolveModel("anthropic", previous)
                      }))
                    }
                  >
                    Show all models
                  </button>
                ) : (
                  <>
                    <label>
                      All Models
                      <select
                        value={isAnthropicCustomModel ? CUSTOM_MODEL_OPTION : appSettings.anthropicModelId.trim()}
                        onChange={(event) =>
                          updateModelSettings((previous) => ({
                            ...previous,
                            anthropicModelMode: event.target.value === CUSTOM_MODEL_OPTION ? "custom" : "all",
                            anthropicModelId:
                              event.target.value === CUSTOM_MODEL_OPTION ? "" : event.target.value
                          }))
                        }
                      >
                        {anthropicModelChoices.map((model) => (
                          <option key={model} value={model}>
                            {model}
                          </option>
                        ))}
                        <option value={CUSTOM_MODEL_OPTION}>Custom...</option>
                      </select>
                    </label>
                    {isAnthropicCustomModel ? (
                      <label>
                        Custom Model
                        <input
                          value={appSettings.anthropicModelId}
                          onChange={(event) =>
                            updateModelSettings((previous) => ({
                              ...previous,
                              anthropicModelMode: "custom",
                              anthropicModelId: event.target.value
                            }))
                          }
                          placeholder={MODEL_TIER_DEFAULTS.anthropic.balanced}
                        />
                      </label>
                    ) : null}
                    <button
                      type="button"
                      className="link-btn"
                      onClick={() =>
                        updateModelSettings((previous) => ({
                          ...previous,
                          anthropicModelMode: "tier",
                          anthropicModelId: MODEL_TIER_DEFAULTS.anthropic[previous.anthropicTier]
                        }))
                      }
                    >
                      Use tier defaults
                    </button>
                  </>
                )}
                <p className="muted">Current model: {resolveModel("anthropic", appSettings)}</p>
                <label>
                  API Key
                  <input
                    type="password"
                    value={vaultInputs.anthropic_api_key}
                    placeholder="Paste key and click Save"
                    onChange={(event) =>
                      setVaultInputs((previous) => ({
                        ...previous,
                        anthropic_api_key: event.target.value
                      }))
                    }
                  />
                </label>
                <div className="row-actions">
                  <button type="button" onClick={() => void saveAnthropicProvider()}>
                    Save
                  </button>
                </div>
              </SettingsSectionCard>

              <SettingsSectionCard
                title="Google"
                actions={
                  <span className={vaultStatus.google_api_key ? "pill active" : "pill inactive"}>
                    {vaultStatus.google_api_key ? "Set" : "Not set"}
                  </span>
                }
              >
                <label>
                  Model Tier
                  <select
                    value={appSettings.googleTier}
                    onChange={(event) =>
                      updateModelSettings((previous) => ({
                        ...previous,
                        googleTier: event.target.value as ModelTier,
                        googleModelMode: "tier",
                        googleModelId: MODEL_TIER_DEFAULTS.google[event.target.value as ModelTier]
                      }))
                    }
                  >
                    {MODEL_TIER_OPTIONS.map((option) => (
                      <option key={option.value} value={option.value}>
                        {option.label}
                      </option>
                    ))}
                  </select>
                </label>
                {appSettings.googleModelMode === "tier" ? (
                  <button
                    type="button"
                    className="link-btn"
                    onClick={() =>
                      updateModelSettings((previous) => ({
                        ...previous,
                        googleModelMode: "all",
                        googleModelId: resolveModel("google", previous)
                      }))
                    }
                  >
                    Show all models
                  </button>
                ) : (
                  <>
                    <label>
                      All Models
                      <select
                        value={isGoogleCustomModel ? CUSTOM_MODEL_OPTION : appSettings.googleModelId.trim()}
                        onChange={(event) =>
                          updateModelSettings((previous) => ({
                            ...previous,
                            googleModelMode: event.target.value === CUSTOM_MODEL_OPTION ? "custom" : "all",
                            googleModelId: event.target.value === CUSTOM_MODEL_OPTION ? "" : event.target.value
                          }))
                        }
                      >
                        {googleModelChoices.map((model) => (
                          <option key={model} value={model}>
                            {model}
                          </option>
                        ))}
                        <option value={CUSTOM_MODEL_OPTION}>Custom...</option>
                      </select>
                    </label>
                    {isGoogleCustomModel ? (
                      <label>
                        Custom Model
                        <input
                          value={appSettings.googleModelId}
                          onChange={(event) =>
                            updateModelSettings((previous) => ({
                              ...previous,
                              googleModelMode: "custom",
                              googleModelId: event.target.value
                            }))
                          }
                          placeholder={MODEL_TIER_DEFAULTS.google.balanced}
                        />
                      </label>
                    ) : null}
                    <button
                      type="button"
                      className="link-btn"
                      onClick={() =>
                        updateModelSettings((previous) => ({
                          ...previous,
                          googleModelMode: "tier",
                          googleModelId: MODEL_TIER_DEFAULTS.google[previous.googleTier]
                        }))
                      }
                    >
                      Use tier defaults
                    </button>
                  </>
                )}
                <p className="muted">Current model: {resolveModel("google", appSettings)}</p>
                <label>
                  API Key
                  <input
                    type="password"
                    value={vaultInputs.google_api_key}
                    placeholder="Paste key and click Save"
                    onChange={(event) =>
                      setVaultInputs((previous) => ({
                        ...previous,
                        google_api_key: event.target.value
                      }))
                    }
                  />
                </label>
                <div className="row-actions">
                  <button type="button" onClick={() => void saveGoogleProvider()}>
                    Save
                  </button>
                </div>
              </SettingsSectionCard>

              <SettingsSectionCard title="Search Providers">
                {(["brave_api_key", "tavily_api_key"] as const).map((key) => (
                  <div key={key} className="card-list">
                    <label>
                      {PROVIDER_LABELS[key]}
                      <input
                        type="password"
                        value={vaultInputs[key]}
                        placeholder="Paste key and click Save"
                        onChange={(event) =>
                          setVaultInputs((previous) => ({ ...previous, [key]: event.target.value }))
                        }
                      />
                    </label>
                    <div className="row-actions">
                      <button type="button" onClick={() => void handleSaveVaultKey(key)}>
                        Save
                      </button>
                    </div>
                  </div>
                ))}
              </SettingsSectionCard>

              <SettingsSectionCard title="Session">
                <p>{sessionToken ? "Signed in" : "Signed out"}</p>
                <div className="row-actions">
                  <button type="button" className="secondary-btn" onClick={() => void handleLockBossClaw()}>
                    Lock BossClaw
                  </button>
                  <button type="button" className="secondary-btn" onClick={() => void logout()}>
                    Logout
                  </button>
                </div>
                {settingsMessage ? <p className="muted">{settingsMessage}</p> : null}
              </SettingsSectionCard>

              <SettingsSectionCard
                title="Document Converter (MarkItDown)"
                actions={
                  <span
                    className={
                      mdStatus === "ready"
                        ? "pill active"
                        : mdStatus === "installing"
                          ? "pill active"
                          : "pill inactive"
                    }
                  >
                    {mdStatusLabel}
                  </span>
                }
              >
                <p className="muted">
                  Convert documents to markdown using a BossClaw-managed Python venv.
                </p>
                <p className="muted">Venv: {mdVenvPath ?? "Not created yet"}</p>
                <div className="row-actions">
                  <button type="button" className="secondary-btn" onClick={() => void detectMarkItDown()}>
                    Detect
                  </button>
                  <button
                    type="button"
                    onClick={() => void installMarkItDown()}
                    disabled={mdStatus === "installing"}
                  >
                    {mdStatus === "installing" ? "Installing..." : "Install"}
                  </button>
                  <button
                    type="button"
                    onClick={() => void testConvertWithMarkItDown()}
                    disabled={mdStatus !== "ready"}
                  >
                    Test Convert
                  </button>
                </div>
                {mdSelectedFile ? <p className="muted">Selected: {mdSelectedFile}</p> : null}
                {mdError ? <p className="error-text">{mdError}</p> : null}
                {mdPreview ? (
                  <div>
                    <h4>Preview (first 2,000 chars)</h4>
                    <pre className="code-box">{mdPreview}</pre>
                  </div>
                ) : null}
                {mdLogs ? (
                  <details>
                    <summary>Install logs</summary>
                    <pre className="code-box">{mdLogs}</pre>
                  </details>
                ) : null}
              </SettingsSectionCard>

              {vaultMessage ? <p className="muted">{vaultMessage}</p> : null}
            </div>
          </section>
        ) : null}

        {tab === "skills" ? (
          <section className="panel">
            <h2>Skills</h2>

            <div className="section-header">
              <p className="muted">Channel: {skillsChannel}</p>
              <button type="button" className="secondary-btn" onClick={() => void refreshVerifiedSkills()}>
                Refresh
              </button>
            </div>

            {skillsLoading ? <p className="muted">Loading local verified skill pack...</p> : null}
            {skillsError ? <p className="error-text">{skillsError}</p> : null}
            {skillsMessage ? <p className="muted">{skillsMessage}</p> : null}

            <div className="skills-layout">
              <div className="skills-list-pane">
                <label htmlFor="skills-search">
                  Search
                  <input
                    id="skills-search"
                    type="search"
                    placeholder="Search by name, tag, or id"
                    value={skillsSearch}
                    onChange={(event) => setSkillsSearch(event.target.value)}
                  />
                </label>

                <div className="skills-list">
                  {filteredSkills.length ? (
                    filteredSkills.map((skill) => {
                      const installedVersion = skill.manifest
                        ? installedSkills.find(
                            (item) => item.id === skill.id && item.version === skill.manifest?.version
                          )?.version
                        : null;

                      return (
                        <button
                          key={skill.id}
                          type="button"
                          className={skill.id === selectedSkillId ? "skill-item selected" : "skill-item"}
                          onClick={() => {
                            setSelectedSkillId(skill.id);
                            setPendingInstallSkillId(null);
                          }}
                        >
                          <div className="skill-item-head">
                            <strong>{skill.manifest?.name ?? skill.id}</strong>
                            {installedVersion ? <span className="pill active">Installed</span> : null}
                          </div>
                          <span className="muted">{skill.manifest?.description ?? "Manifest unavailable"}</span>
                          <span className="muted">
                            {(skill.manifest?.tags ?? []).length
                              ? (skill.manifest?.tags ?? []).join(", ")
                              : skill.id}
                          </span>
                        </button>
                      );
                    })
                  ) : (
                    <p className="muted">No skills matched your search.</p>
                  )}
                </div>
              </div>

              <div className="skills-detail-pane">
                {selectedSkill ? (
                  <article className="entity-card">
                    <div className="entity-head">
                      <div>
                        <h3>{selectedSkill.manifest?.name ?? selectedSkill.id}</h3>
                        <p className="muted">{selectedSkill.manifest?.description ?? "No manifest summary."}</p>
                      </div>
                      {selectedSkillInstalled ? (
                        <span className="pill active">Installed</span>
                      ) : (
                        <span className="pill inactive">Not installed</span>
                      )}
                    </div>

                    <p className="muted">
                      ID: {selectedSkill.id}
                      {selectedSkill.manifest ? ` · Version: ${selectedSkill.manifest.version}` : ""}
                    </p>

                    {selectedSkill.loadError ? (
                      <p className="error-text">{selectedSkill.loadError}</p>
                    ) : null}

                    {selectedSkill.manifestValidationErrors.length ? (
                      <div>
                        <h4>Manifest validation errors</h4>
                        <ul className="simple-list">
                          {selectedSkill.manifestValidationErrors.map((item, index) => (
                            <li key={`${selectedSkill.id}-error-${index}`}>{item}</li>
                          ))}
                        </ul>
                      </div>
                    ) : null}

                    {selectedSkill.manifest ? (
                      <div className="skills-subsection">
                        <h4>Manifest summary</h4>
                        <ul className="simple-list">
                          <li>Runtime: {selectedSkill.manifest.runtime.type}</li>
                          <li>
                            Tool:{" "}
                            {selectedSkill.manifest.runtime.type === "native"
                              ? selectedSkill.manifest.runtime.toolId
                              : selectedSkill.manifest.runtime.package.name}
                          </li>
                          <li>
                            Automation:
                            {selectedSkill.manifest.automation?.supportsSchedule
                              ? " supports schedule"
                              : " manual only"}
                          </li>
                        </ul>
                      </div>
                    ) : null}

                    {selectedSkill.manifest ? (
                      <div className="skills-subsection">
                        <h4>Permissions diff before install</h4>
                        {selectedSkillPermissionsDiff.length ? (
                          <ul className="simple-list">
                            {selectedSkillPermissionsDiff.map((line, index) => (
                              <li key={`${selectedSkill.id}-diff-${index}`}>{line}</li>
                            ))}
                          </ul>
                        ) : (
                          <p className="muted">No permission changes from app defaults.</p>
                        )}
                      </div>
                    ) : null}

                    <div className="skills-subsection">
                      <h4>SKILL.md preview</h4>
                      {selectedSkill.skillMd ? (
                        <div
                          className="markdown-preview"
                          dangerouslySetInnerHTML={{
                            __html: renderSimpleMarkdown(selectedSkill.skillMd)
                          }}
                        />
                      ) : (
                        <p className="muted">SKILL.md not found.</p>
                      )}
                    </div>

                    <details>
                      <summary>Manifest JSON</summary>
                      <pre className="code-box">
                        {selectedSkill.manifestRaw
                          ? JSON.stringify(selectedSkill.manifestRaw, null, 2)
                          : "Manifest JSON unavailable"}
                      </pre>
                    </details>

                    <details>
                      <summary>PROMPT.md</summary>
                      <pre className="code-box">
                        {selectedSkill.promptMd || "PROMPT.md unavailable"}
                      </pre>
                    </details>

                    <div className="row-actions">
                      <button
                        type="button"
                        className="secondary-btn"
                        onClick={() => setPendingInstallSkillId(null)}
                        disabled={!pendingInstallSkillId}
                      >
                        Cancel
                      </button>
                      {pendingInstallSkillId === selectedSkill.id ? (
                        <button
                          type="button"
                          onClick={() => void confirmInstallSkill()}
                          disabled={
                            isInstallingSkill ||
                            selectedSkillInstalled ||
                            !selectedSkill.manifest ||
                            selectedSkill.manifestValidationErrors.length > 0
                          }
                        >
                          {isInstallingSkill ? "Installing..." : "Confirm install"}
                        </button>
                      ) : (
                        <button
                          type="button"
                          onClick={() => setPendingInstallSkillId(selectedSkill.id)}
                          disabled={
                            selectedSkillInstalled ||
                            !selectedSkill.manifest ||
                            selectedSkill.manifestValidationErrors.length > 0
                          }
                        >
                          Install
                        </button>
                      )}
                    </div>
                    <p className="muted">
                      Install is file-copy only. Skill code is not executed during install.
                    </p>
                  </article>
                ) : (
                  <p className="muted">Select a skill to preview and install.</p>
                )}
              </div>
            </div>
          </section>
        ) : null}

        {tab === "settings" && settingsSection === "advanced" ? (
          <section className="panel">
            <SettingsSectionCard
              title="Activity"
              actions={
                <div className="run-toolbar">
                  <select
                    value={selectedAgentId ?? ""}
                    onChange={(event) => setSelectedAgentId(event.target.value || null)}
                  >
                    {agents.length ? null : <option value="">No agents</option>}
                    {agents.map((agent) => (
                      <option key={agent.id} value={agent.id}>
                        {agent.name}
                      </option>
                    ))}
                  </select>
                  <button type="button" onClick={createDummyRun} disabled={!agents.length}>
                    New Activity
                  </button>
                </div>
              }
            >
              <div className="runs-layout">
                <div className="runs-list">
                  {runs.length ? (
                    runs.map((run) => (
                      <button
                        key={run.id}
                        type="button"
                        className={run.id === selectedRunId ? "run-item selected" : "run-item"}
                        onClick={() => setSelectedRunId(run.id)}
                      >
                        <strong>{run.title}</strong>
                        <span className="muted">{run.status}</span>
                      </button>
                    ))
                  ) : (
                    <p className="muted">No activity yet.</p>
                  )}
                </div>

                <div className="run-details">
                  {selectedRun ? (
                    <>
                      <h3>{selectedRun.title}</h3>
                      <p className="muted">Status: {selectedRun.status}</p>
                      <p className="muted">
                        Started: {new Date(selectedRun.startedAt).toLocaleString()}
                      </p>
                      <p>{selectedRun.summary}</p>
                      <h4>Logs</h4>
                      <ul className="simple-list">
                        {selectedRun.logs.map((entry, index) => (
                          <li key={`${selectedRun.id}-${index}`}>{entry}</li>
                        ))}
                      </ul>
                    </>
                  ) : (
                    <p className="muted">Select an activity item to view details.</p>
                  )}
                </div>
              </div>
            </SettingsSectionCard>
          </section>
        ) : null}

        {tab === "settings" && settingsSection === "usage" ? (
          <section className="panel settings-panel">
            <h2>Usage</h2>
            <SettingsSectionCard
              title="Usage Summary"
              description="Cost values are estimates based on local placeholder pricing."
              actions={
                <button type="button" onClick={downloadUsageJson} disabled={!usageEvents.length}>
                  Download Usage as JSON
                </button>
              }
            >
              <div className="stats-grid">
              <article className="stat-card">
                <h3>Today</h3>
                <p className="big-number">{usageSummaries.today.eventCount}</p>
                <p className="muted">
                  {usageSummaries.today.totalTokens} tokens ·{" "}
                  {formatUsd(usageSummaries.today.totalCostUsd)}
                </p>
              </article>
              <article className="stat-card">
                <h3>Last 7 days</h3>
                <p className="big-number">{usageSummaries.sevenDays.eventCount}</p>
                <p className="muted">
                  {usageSummaries.sevenDays.totalTokens} tokens ·{" "}
                  {formatUsd(usageSummaries.sevenDays.totalCostUsd)}
                </p>
              </article>
              <article className="stat-card">
                <h3>Last 30 days</h3>
                <p className="big-number">{usageSummaries.thirtyDays.eventCount}</p>
                <p className="muted">
                  {usageSummaries.thirtyDays.totalTokens} tokens ·{" "}
                  {formatUsd(usageSummaries.thirtyDays.totalCostUsd)}
                </p>
              </article>
              </div>

              <h3 className="section-title">By provider</h3>
              {usageByProvider.length ? (
                <table className="usage-table">
                  <thead>
                    <tr>
                      <th>Provider</th>
                      <th>Events</th>
                      <th>Tokens</th>
                      <th>Estimated cost</th>
                    </tr>
                  </thead>
                  <tbody>
                    {usageByProvider.map((entry) => (
                      <tr key={entry.provider}>
                        <td>{entry.provider}</td>
                        <td>{entry.count}</td>
                        <td>{entry.tokens}</td>
                        <td>{formatUsd(entry.cost)}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              ) : (
                <p className="muted">No usage events recorded yet.</p>
              )}

              <h3 className="section-title">By agent</h3>
              {usageByAgent.length ? (
                <table className="usage-table">
                  <thead>
                    <tr>
                      <th>Agent</th>
                      <th>Events</th>
                      <th>Tokens</th>
                      <th>Estimated cost</th>
                    </tr>
                  </thead>
                  <tbody>
                    {usageByAgent.map((entry) => (
                      <tr key={entry.agent}>
                        <td>{entry.agent}</td>
                        <td>{entry.count}</td>
                        <td>{entry.tokens}</td>
                        <td>{formatUsd(entry.cost)}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              ) : (
                <p className="muted">No agent usage recorded yet.</p>
              )}

              <h3 className="section-title">Most expensive events</h3>
              {mostExpensiveEvents.length ? (
                <table className="usage-table">
                  <thead>
                    <tr>
                      <th>Time</th>
                      <th>Provider</th>
                      <th>Kind</th>
                      <th>Agent</th>
                      <th>Tokens</th>
                      <th>Estimated cost</th>
                    </tr>
                  </thead>
                  <tbody>
                    {mostExpensiveEvents.map((event) => (
                      <tr key={event.id}>
                        <td>{new Date(event.ts).toLocaleString()}</td>
                        <td>{event.provider}</td>
                        <td>{event.kind}</td>
                        <td>{event.agentId ?? "Unassigned"}</td>
                        <td>{eventTokenTotal(event)}</td>
                        <td>
                          {event.estimatedCostUsd !== null
                            ? formatUsd(event.estimatedCostUsd)
                            : "n/a"}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              ) : (
                <p className="muted">No metered events yet.</p>
              )}
            </SettingsSectionCard>
          </section>
        ) : null}

        {tab === "settings" && settingsSection === "changes" ? (
          <section className="panel settings-panel">
            <h2>Changes</h2>
            <SettingsSectionCard title="History" description="Review, inspect, and roll back configuration changes.">
              <div className="run-toolbar">
                <select
                  value={historyKindFilter}
                  onChange={(event) =>
                    setHistoryKindFilter(event.target.value as "all" | ConfigObjectKind)
                  }
                >
                  <option value="all">All kinds</option>
                  <option value="agent">Agents</option>
                  <option value="mission">Missions</option>
                  <option value="workspace">Workspaces</option>
                  <option value="skill_install">Skill installs</option>
                  <option value="web_policy">Web policies</option>
                  <option value="file_policy">File policies</option>
                </select>

                <select
                  value={historyObjectFilter}
                  onChange={(event) => setHistoryObjectFilter(event.target.value)}
                >
                  <option value="all">All objects</option>
                  {historyFilterOptions
                    .filter((option) => historyKindFilter === "all" || option.kind === historyKindFilter)
                    .map((option) => (
                      <option key={`${option.kind}-${option.id}`} value={option.value}>
                        {option.label}
                      </option>
                    ))}
                </select>
              </div>

              {filteredAuditEntries.length ? (
                <div className="card-list">
                {filteredAuditEntries.map((entry) => {
                  const expanded = expandedAuditEntryId === entry.id;
                  return (
                    <article key={entry.id} className="entity-card">
                      <div className="entity-head">
                        <div>
                          <h3>{entry.summary}</h3>
                          <p className="muted">
                            {new Date(entry.ts).toLocaleString()} · {entry.object.kind} · {entry.action}
                          </p>
                        </div>
                        <button
                          type="button"
                          className="secondary-btn"
                          onClick={() =>
                            setExpandedAuditEntryId((current) => (current === entry.id ? null : entry.id))
                          }
                        >
                          {expanded ? "Hide" : "View"}
                        </button>
                      </div>

                      {expanded ? (
                        <>
                          <p className="muted">
                            Version: {entry.beforeVersion ?? "none"} -&gt; {entry.afterVersion ?? "none"}
                          </p>

                          {entry.diff.added.length ? (
                            <div>
                              <h4>Added</h4>
                              <ul className="simple-list">
                                {entry.diff.added.map((item, index) => (
                                  <li key={`${entry.id}-added-${index}`}>{maskCronListLine(item)}</li>
                                ))}
                              </ul>
                            </div>
                          ) : null}

                          {entry.diff.removed.length ? (
                            <div>
                              <h4>Removed</h4>
                              <ul className="simple-list">
                                {entry.diff.removed.map((item, index) => (
                                  <li key={`${entry.id}-removed-${index}`}>{maskCronListLine(item)}</li>
                                ))}
                              </ul>
                            </div>
                          ) : null}

                          {entry.diff.changed.length ? (
                            <div>
                              <h4>Changed</h4>
                              <ul className="simple-list">
                                {entry.diff.changed.map((item) => (
                                  <li key={`${entry.id}-changed-${item.path}`}>
                                    <strong>{item.path}</strong>: {maskCronDiffValue(item.path, item.from)} -&gt;{" "}
                                    {maskCronDiffValue(item.path, item.to)}
                                  </li>
                                ))}
                              </ul>
                            </div>
                          ) : null}

                          {entry.action === "update" && typeof entry.beforeVersion === "number" ? (
                            <div className="row-actions">
                              <button
                                type="button"
                                onClick={() => {
                                  const rollbackVersion = entry.beforeVersion;
                                  if (typeof rollbackVersion !== "number") {
                                    return;
                                  }
                                  const confirmed = window.confirm(
                                    `Rollback ${entry.object.kind} ${entry.object.id} to version ${rollbackVersion}?`
                                  );
                                  if (!confirmed) {
                                    return;
                                  }
                                  void loadConfigVersions(entry.object.kind, entry.object.id)
                                    .then((versions) => {
                                      const targetExists = versions.some(
                                        (versionEntry) => versionEntry.version === rollbackVersion
                                      );
                                      if (!targetExists) {
                                        throw new Error("Target version not found.");
                                      }

                                      return rollbackObjectVersion(
                                        entry.object.kind,
                                        entry.object.id,
                                        rollbackVersion,
                                        `Rollback ${entry.object.kind} to version ${rollbackVersion}`
                                      );
                                    })
                                    .catch(() => {
                                      setError("Unable to rollback selected version.");
                                    });
                                }}
                              >
                                Rollback to version {entry.beforeVersion}
                              </button>
                            </div>
                          ) : null}
                        </>
                      ) : null}
                    </article>
                  );
                })}
                </div>
              ) : (
                <p className="muted">No audit entries found.</p>
              )}
            </SettingsSectionCard>
          </section>
        ) : null}

        {tab === "settings" && settingsSection === "advanced" ? (
          <section className="panel settings-panel">
            <h2>Advanced</h2>
            <SettingsSectionCard title="Tools and Integrations" description="Manage advanced local tooling and web access.">
              <div className="card-list">
              <article className="entity-card">
                <h3>Environment</h3>
                <p className="muted">API base URL: {API_BASE}</p>
                <p className="muted">Web base URL: {WEB_URL}</p>
              </article>
              <article className="entity-card">
                <h3>File Access</h3>
                <p className="muted">BossClaw can only access folders you approve.</p>
                {fileAccessError ? <p className="error-text">{fileAccessError}</p> : null}
                {fileAccessMessage ? <p className="muted">{fileAccessMessage}</p> : null}
                <div className="row-actions">
                  <button type="button" onClick={() => void addFilePolicyFromPicker()}>
                    Add folder
                  </button>
                </div>

                {activeFilePolicies.length ? (
                  <div className="card-list">
                    {activeFilePolicies.map((policy) => (
                      <article className="entity-card" key={policy.path}>
                        <div className="entity-head">
                          <div>
                            <h4 className="single-line">{policy.path}</h4>
                            <p className="muted">
                              Approved {new Date(policy.approvedAt).toLocaleString()} by {policy.approvedBy}
                            </p>
                          </div>
                          <StatusBadge tone="primary">
                            {policy.mode === "read_write" ? "Read & Write" : "Read-only"}
                          </StatusBadge>
                        </div>
                        <div className="policy-grid">
                          <label>
                            Permission
                            <select
                              value={policy.mode}
                              onChange={(event) =>
                                void updateFilePolicyMode(
                                  policy,
                                  event.target.value as "read" | "read_write"
                                )
                              }
                            >
                              <option value="read">Read-only</option>
                              <option value="read_write">Read & Write</option>
                            </select>
                          </label>
                        </div>
                        <div className="row-actions">
                          <button
                            type="button"
                            className="danger-btn"
                            onClick={() => void archiveFilePolicy(policy)}
                          >
                            Remove folder
                          </button>
                        </div>
                      </article>
                    ))}
                  </div>
                ) : (
                  <p className="muted">No approved folders yet.</p>
                )}
              </article>
              <article className="entity-card">
                <h3>Web Access</h3>
                <p className="muted">
                  Approve domains once, then agents can use Standard, Signed-in, or Interactive web access.
                </p>
                {webAccessError ? <p className="error-text">{webAccessError}</p> : null}
                {webAccessMessage ? <p className="muted">{webAccessMessage}</p> : null}

                <div className="policy-grid">
                  <label>
                    Host
                    <input
                      value={webPolicyHostInput}
                      onChange={(event) => setWebPolicyHostInput(event.target.value)}
                      placeholder="example.com"
                    />
                  </label>
                  <label>
                    Level
                    <select
                      value={webPolicyLevelInput}
                      onChange={(event) => setWebPolicyLevelInput(event.target.value as WebExtractLevel)}
                    >
                      <option value="public">Standard</option>
                      <option value="auth">Signed-in</option>
                      <option value="browser">Interactive</option>
                    </select>
                  </label>
                </div>
                <label>
                  Allowed paths (optional, comma-separated)
                  <input
                    value={webPolicyPathInput}
                    onChange={(event) => setWebPolicyPathInput(event.target.value)}
                    placeholder="/docs,/pricing"
                  />
                </label>
                <div className="row-actions">
                  <button type="button" onClick={() => void addWebPolicyFromInput()}>
                    Approve host
                  </button>
                </div>

                <label>
                  Test URL
                  <input
                    value={webTestUrl}
                    onChange={(event) => setWebTestUrl(event.target.value)}
                    placeholder="https://example.com"
                  />
                </label>
                {webTestResult ? <p className="muted">{webTestResult}</p> : null}

                {activeWebPolicies.length ? (
                  <div className="card-list">
                    {activeWebPolicies.map((policy) => (
                      <article className="entity-card" key={policy.host}>
                        <div className="entity-head">
                          <div>
                            <h4>{policy.host}</h4>
                            <p className="muted">
                              Approved {new Date(policy.approvedAt).toLocaleString()} by {policy.approvedBy}
                            </p>
                          </div>
                          <StatusBadge tone="primary">
                            {WEB_LEVEL_LABELS[policy.level]}
                          </StatusBadge>
                        </div>

                        {policy.allowPaths?.length ? (
                          <p className="muted">Allowed paths: {policy.allowPaths.join(", ")}</p>
                        ) : (
                          <p className="muted">Allowed paths: all paths</p>
                        )}

                        <div className="policy-grid">
                          <label>
                            Access level
                            <select
                              value={policy.level}
                              onChange={(event) =>
                                void updateWebPolicyLevel(policy, event.target.value as WebExtractLevel)
                              }
                            >
                              <option value="public">Standard</option>
                              <option value="auth">Signed-in</option>
                              <option value="browser">Interactive</option>
                            </select>
                          </label>
                          <label>
                            Auth token (cookie:/bearer:/basic:)
                            <input
                              type="password"
                              value={webAuthInputs[policy.host] ?? ""}
                              onChange={(event) =>
                                setWebAuthInputs((previous) => ({
                                  ...previous,
                                  [policy.host]: event.target.value
                                }))
                              }
                              placeholder="bearer: your-token"
                            />
                          </label>
                        </div>
                        <div className="row-actions">
                          <button
                            type="button"
                            className="secondary-btn"
                            onClick={() => void testWebAccessFetch(policy.host, policy.level)}
                            disabled={webTestLoading}
                          >
                            {webTestLoading ? "Testing..." : "Test Fetch"}
                          </button>
                          <button
                            type="button"
                            className="secondary-btn"
                            onClick={() => void clearWebAuthToken(policy.host)}
                          >
                            Clear token
                          </button>
                          <button type="button" onClick={() => void saveWebAuthToken(policy.host)}>
                            Set token
                          </button>
                          <button
                            type="button"
                            className="danger-btn"
                            onClick={() => void archiveWebPolicy(policy)}
                          >
                            Remove host
                          </button>
                        </div>
                      </article>
                    ))}
                  </div>
                ) : (
                  <p className="muted">No approved hosts yet.</p>
                )}
              </article>
              <article className="entity-card">
                <h3>Browser Mode Helper</h3>
                <p className="muted">
                  Optional Playwright helper for Interactive mode (JS-rendered pages).
                </p>
                <p className="muted">
                  Status:{" "}
                  {pwStatus?.helperInstalled
                    ? "Ready"
                    : pwStatus?.nodeFound
                      ? "Not installed"
                      : "Node.js missing"}
                  {pwStatus?.nodeVersion ? ` · ${pwStatus.nodeVersion}` : ""}
                </p>
                <p className="muted">Helper path: {pwStatus?.helperPath ?? "Unavailable"}</p>
                <div className="row-actions">
                  <button type="button" className="secondary-btn" onClick={() => void refreshPlaywrightHelper()}>
                    Detect
                  </button>
                  <button
                    type="button"
                    onClick={() => void installPlaywrightHelper()}
                    disabled={pwLoading}
                  >
                    {pwLoading ? "Installing..." : "Install"}
                  </button>
                </div>
                <label>
                  Interactive test URL
                  <input
                    value={pwTestUrl}
                    onChange={(event) => setPwTestUrl(event.target.value)}
                    placeholder="https://example.com"
                  />
                </label>
                <div className="row-actions">
                  <button
                    type="button"
                    className="secondary-btn"
                    onClick={() => void testPlaywrightFetch()}
                    disabled={!pwStatus?.helperInstalled}
                  >
                    Test Interactive Fetch
                  </button>
                </div>
                {pwTestResult ? <p className="muted">{pwTestResult}</p> : null}
                {pwLogs ? (
                  <details>
                    <summary>Install logs</summary>
                    <pre className="code-box">{pwLogs}</pre>
                  </details>
                ) : null}
              </article>
              <article className="entity-card">
                <h3>Missions</h3>
                <p className="muted">Mission records: {missions.length}</p>
              </article>
              <article className="entity-card">
                <h3>Workspace</h3>
                <p className="muted">
                  Workspace records: {workspaces.length}
                  {workspaces[0]?.path ? ` · ${workspaces[0].path}` : " · No path configured"}
                </p>
                <button type="button" className="secondary-btn" onClick={updateWorkspaceScaffold}>
                  Toggle workspace path
                </button>
              </article>
              <article className="entity-card">
                <h3>Skill Install Records</h3>
                <p className="muted">Versioned install records: {skillInstalls.length}</p>
              </article>
              <article className="entity-card">
                <h3>Billing</h3>
                <p className="muted">Manage subscription on the website pricing page.</p>
                <button type="button" onClick={() => void openPricing()}>
                  Open Pricing page
                </button>
              </article>
              </div>
            </SettingsSectionCard>
          </section>
        ) : null}

        {tab === "settings" && settingsSection === "about" ? (
          <section className="panel settings-panel">
            <h2>About</h2>
            <SettingsSectionCard title="BossClaw Desktop" description="Product and release details.">
              <p className="muted">Version: {APP_VERSION}</p>
              <p className="muted">Build: {APP_GIT_SHA}</p>
            </SettingsSectionCard>
          </section>
        ) : null}

        {error ? <p className="error-text app-error">{error}</p> : null}
      </main>

      {showCreateAgentModal ? (
        <div className="modal-backdrop" role="presentation">
          <form className="modal-card" onSubmit={handleCreateAgent}>
            <h3>Create Agent</h3>
            <label>
              Name
              <input
                value={newAgentName}
                onChange={(event) => setNewAgentName(event.target.value)}
                placeholder="Market Scout"
                required
              />
            </label>
            <label>
              Purpose
              <input
                value={newAgentPurpose}
                onChange={(event) => setNewAgentPurpose(event.target.value)}
                placeholder="Find high-signal opportunities"
                required
              />
            </label>
            <div className="policy-grid">
              <label>
                Provider
                <select
                  value={newAgentProvider}
                  onChange={(event) => setNewAgentProvider(event.target.value as AgentProvider)}
                >
                  <option value="openai_compat">OpenAI-compatible (streaming)</option>
                  <option value="google_gemini">Google Gemini</option>
                  <option value="anthropic_claude">Anthropic Claude</option>
                </select>
              </label>
              <label>
                Model override
                <input
                  value={newAgentModelOverride}
                  onChange={(event) => setNewAgentModelOverride(event.target.value)}
                  placeholder={`Default: ${
                    newAgentProvider === "google_gemini"
                      ? resolveModel("google", appSettings)
                      : newAgentProvider === "anthropic_claude"
                        ? resolveModel("anthropic", appSettings)
                        : appSettings.openaiCompatModel
                  }`}
                />
              </label>
            </div>
            <label>
              Base URL override (HTTPS)
              <input
                value={newAgentBaseOverride}
                onChange={(event) => setNewAgentBaseOverride(event.target.value)}
                placeholder={`Default: ${appSettings.openaiCompatBaseUrl}`}
              />
            </label>
            <div className="policy-grid">
              <label>
                Memory Mode
                <select
                  value={newAgentMemoryMode}
                  onChange={(event) => setNewAgentMemoryMode(event.target.value as MemoryMode)}
                >
                  <option value="isolated">isolated</option>
                  <option value="shared">shared</option>
                </select>
              </label>
              <label>
                Activity Detail
                <select
                  value={newAgentLoggingMode}
                  onChange={(event) => setNewAgentLoggingMode(event.target.value as LoggingMode)}
                >
                  <option value="simple">Simple</option>
                  <option value="detailed">Detailed</option>
                </select>
              </label>
            </div>
            <div className="tool-grid">
              {TOOL_REGISTRY.map((tool) => (
                <label key={tool.id} className="check-row">
                  <input
                    type="checkbox"
                    checked={newAgentTools.includes(tool.id)}
                    onChange={() => toggleNewAgentTool(tool.id)}
                  />
                  <span>{tool.label}</span>
                </label>
              ))}
            </div>
            <div className="row-actions">
              <button type="button" className="secondary-btn" onClick={() => setShowCreateAgentModal(false)}>
                Cancel
              </button>
              <button type="submit">Create Agent</button>
            </div>
          </form>
        </div>
      ) : null}

      {showCreateMissionModal ? (
        <div className="modal-backdrop" role="presentation">
          <form className="modal-card" onSubmit={handleCreateMission}>
            <h3>New Mission</h3>
            <label>
              Title
              <input
                value={newMissionTitle}
                onChange={(event) => setNewMissionTitle(event.target.value)}
                placeholder="Morning market pulse"
                required
              />
            </label>
            <label>
              Goal
              <textarea
                value={newMissionGoal}
                onChange={(event) => setNewMissionGoal(event.target.value)}
                placeholder="Check today’s headlines and summarize top 3 items."
                rows={4}
                required
              />
            </label>
            <label>
              Schedule
              <select
                value={newMissionPresetKind}
                onChange={(event) => setNewMissionPresetKind(event.target.value as MissionPresetKind)}
              >
                <option value="daily">Daily at time</option>
                <option value="weekdays">Weekdays at time</option>
                <option value="every_minutes">Every N minutes</option>
                <option value="weekly">Weekly</option>
              </select>
            </label>
            {(newMissionPresetKind === "daily" ||
              newMissionPresetKind === "weekdays" ||
              newMissionPresetKind === "weekly") ? (
              <label>
                Time
                <input
                  type="time"
                  value={newMissionTime}
                  onChange={(event) => setNewMissionTime(event.target.value)}
                />
              </label>
            ) : null}
            {newMissionPresetKind === "weekly" ? (
              <label>
                Weekday
                <select
                  value={newMissionWeekday}
                  onChange={(event) => setNewMissionWeekday(Number(event.target.value))}
                >
                  {WEEKDAY_OPTIONS.map((option) => (
                    <option key={option.value} value={option.value}>
                      {option.label}
                    </option>
                  ))}
                </select>
              </label>
            ) : null}
            {newMissionPresetKind === "every_minutes" ? (
              <label>
                Interval minutes
                <input
                  type="number"
                  min={1}
                  max={1440}
                  value={newMissionIntervalMinutes}
                  onChange={(event) => setNewMissionIntervalMinutes(Number(event.target.value))}
                />
              </label>
            ) : null}
            <div className="row-actions">
              <button type="button" className="secondary-btn" onClick={closeCreateMissionModal}>
                Cancel
              </button>
              <button type="submit">Create</button>
            </div>
          </form>
        </div>
      ) : null}

      {missionPendingDelete ? (
        <div className="modal-backdrop" role="presentation">
          <div className="modal-card mission-delete-modal">
            <h3>Delete Mission</h3>
            <p className="muted">Delete “{missionPendingDelete.title}”?</p>
            <p className="muted">This stops future runs for this mission.</p>
            <div className="row-actions">
              <button type="button" className="secondary-btn" onClick={closeMissionDeleteModal}>
                Cancel
              </button>
              <button type="button" className="mission-control-btn" onClick={() => void confirmMissionDelete()}>
                Delete
              </button>
            </div>
          </div>
        </div>
      ) : null}

      {lockToastMessage ? (
        <div className="app-toast" role="status" aria-live="polite">
          {lockToastMessage}
        </div>
      ) : null}
    </div>
  );
}
