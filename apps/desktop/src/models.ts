export type MemoryMode = "isolated" | "shared";
export type LoggingMode = "simple" | "detailed";
export type AgentProvider = "openai_compat" | "google_gemini" | "anthropic_claude";

export type AgentPolicy = {
  memoryMode: MemoryMode;
  loggingMode: LoggingMode;
  toolsEnabled: string[];
};

export type Agent = {
  id: string;
  name: string;
  purpose: string;
  provider: AgentProvider;
  modelId?: string;
  openaiCompatBaseUrlOverride: string | null;
  openaiCompatModelOverride: string | null;
  preferredName?: string;
  tone?: "concise" | "detailed";
  hasAskedName?: boolean;
  hasAskedTone?: boolean;
  hasAskedAgentName?: boolean;
  archived?: boolean;
  policy: AgentPolicy;
  createdAt: string;
};

export type Mission = {
  id: string;
  agentId: string;
  fingerprint: string;
  title: string;
  goal: string;
  enabled: boolean;
  chatPosting: "off" | "summary" | "verbose";
  collapseRepeats: boolean;
  schedule: MissionSchedule;
  nextRunAt: string;
  lastRunAt?: string;
  archived?: boolean;
  createdAt: string;
  updatedAt: string;
};

export type MissionScheduleKind =
  | "daily"
  | "weekdays"
  | "every_minutes"
  | "weekly"
  | "custom";

export type MissionSchedule = {
  kind: MissionScheduleKind;
  time?: string;
  weekday?: number;
  intervalMinutes?: number;
  cron: string;
};

export type Workspace = {
  id: string;
  name: string;
  path: string | null;
  archived?: boolean;
  createdAt: string;
};

export type SkillInstallConfig = {
  id: string;
  version: string;
  channel: string;
  installDir: string;
  installedAt: string;
  archived?: boolean;
};

export type WebPolicy = {
  host: string;
  level: "public" | "auth" | "browser";
  allowPaths?: string[];
  approvedAt: string;
  approvedBy: "user" | "agent";
  notes?: string;
  archived?: boolean;
};

export type FilePolicy = {
  path: string;
  mode: "read" | "read_write";
  approvedAt: string;
  approvedBy: "user" | "agent";
  archived?: boolean;
};

export type RunStatus =
  | "planning"
  | "planned"
  | "executing"
  | "waiting_for_approval"
  | "completed"
  | "failed"
  | "cancelled";

export type Run = {
  id: string;
  agentId: string;
  missionId?: string;
  title: string;
  status: RunStatus;
  startedAt: string;
  finishedAt: string | null;
  summary: string;
  logs: string[];
};

export type ApprovalStatus = "pending" | "approved" | "denied";

export type ApprovalItem = {
  id: string;
  agentId: string;
  createdAt: string;
  kind: string;
  message: string;
  status: ApprovalStatus;
};

export type UsageKind = "llm" | "search" | "web" | "file" | "other";

export type UsageEvent = {
  id: string;
  ts: string;
  agentId: string | null;
  runId: string | null;
  provider: string;
  model: string | null;
  kind: UsageKind;
  promptTokens: number | null;
  completionTokens: number | null;
  totalTokens: number | null;
  inputChars: number;
  outputChars: number;
  latencyMs: number;
  estimatedCostUsd: number | null;
  tags: {
    cacheHit?: boolean;
    usedSummary?: boolean;
    usedEmbeddings?: boolean;
    level?: "standard" | "signed_in" | "interactive";
    bytes?: number;
    op?: "read" | "write";
  };
};

export type ConfigObjectKind =
  | "workspace"
  | "agent"
  | "mission"
  | "skill_install"
  | "web_policy"
  | "file_policy";

export type Versioned<T> = {
  id: string;
  version: number;
  updatedAt: string;
  data: T;
};

export type DiffChange = {
  path: string;
  from: string;
  to: string;
};

export type ObjectDiff = {
  added: string[];
  removed: string[];
  changed: DiffChange[];
};

export type AuditEntry = {
  id: string;
  ts: string;
  actor: { type: "user" | "agent" | "system"; id?: string };
  object: { kind: ConfigObjectKind; id: string };
  action: "create" | "update" | "delete" | "rollback";
  beforeVersion?: number;
  afterVersion?: number;
  summary: string;
  diff: ObjectDiff;
  snapshot: {
    before?: Record<string, unknown>;
    after?: Record<string, unknown>;
  };
};

export type ConfigChangeProposal = {
  id: string;
  ts: string;
  object: { kind: ConfigObjectKind; id: string };
  summary: string;
  diff: ObjectDiff;
  applyMode: "autopilot" | "fsd";
  requiresConfirm: boolean;
  proposedBy: { type: "agent" | "user"; id?: string };
  patch: {
    after: Record<string, unknown>;
  };
};

export type UndoToken = {
  kind: ConfigObjectKind;
  id: string;
  previousVersion: number | null;
  summary: string;
};
