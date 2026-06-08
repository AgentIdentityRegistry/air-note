import { loadJson, saveJson, type StoreFilename } from "./localStore";
import type {
  Agent,
  AgentProvider,
  AuditEntry,
  ConfigChangeProposal,
  ConfigObjectKind,
  DiffChange,
  FilePolicy,
  LoggingMode,
  Mission,
  ObjectDiff,
  SkillInstallConfig,
  UndoToken,
  Versioned,
  WebPolicy,
  Workspace
} from "./models";

const SECRET_KEY_PATTERN = /(key|token|secret|password|api_key|authorization|cookie)/i;
const MAX_DIFF_ITEMS = 50;

type ConfigRecordByKind = {
  agent: Agent;
  mission: Mission;
  workspace: Workspace;
  skill_install: SkillInstallConfig;
  web_policy: WebPolicy;
  file_policy: FilePolicy;
};

const STORE_FILE_BY_KIND: Record<ConfigObjectKind, StoreFilename> = {
  agent: "agents.json",
  mission: "missions.json",
  workspace: "workspaces.json",
  skill_install: "skill_installs.json",
  web_policy: "web_policies.json",
  file_policy: "file_policies.json"
};

export async function loadList<T>(filename: StoreFilename, defaultValue: T[]): Promise<T[]> {
  return loadJson<T[]>(filename, defaultValue);
}

export async function saveList<T>(filename: StoreFilename, data: T[]): Promise<void> {
  await saveJson(filename, data);
}

function objectEntries(value: unknown): Array<[string, unknown]> {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return [];
  }

  return Object.entries(value as Record<string, unknown>);
}

function redactObject(value: unknown, parentKey?: string): unknown {
  if (parentKey && SECRET_KEY_PATTERN.test(parentKey)) {
    return "***REDACTED***";
  }

  if (Array.isArray(value)) {
    return value.map((entry) => redactObject(entry));
  }

  if (value && typeof value === "object") {
    const redacted: Record<string, unknown> = {};
    for (const [key, child] of objectEntries(value)) {
      redacted[key] = redactObject(child, key);
    }
    return redacted;
  }

  return value;
}

function toSummary(value: unknown): string {
  if (typeof value === "string") {
    return value.length > 120 ? `${value.slice(0, 117)}...` : value;
  }

  if (typeof value === "number" || typeof value === "boolean" || value === null) {
    return String(value);
  }

  if (Array.isArray(value)) {
    return `[Array(${value.length})]`;
  }

  if (value && typeof value === "object") {
    const keys = Object.keys(value as Record<string, unknown>);
    return keys.length ? `{${keys.slice(0, 4).join(",")}${keys.length > 4 ? ",..." : ""}}` : "{}";
  }

  return "";
}

function flattenForDiff(value: unknown, path: string, output: Map<string, string>): void {
  if (Array.isArray(value)) {
    if (value.length === 0) {
      output.set(path || "$", "[]");
      return;
    }

    value.forEach((entry, index) => {
      flattenForDiff(entry, `${path}[${index}]`, output);
    });
    return;
  }

  if (value && typeof value === "object") {
    const entries = objectEntries(value).sort(([left], [right]) => left.localeCompare(right));
    if (!entries.length) {
      output.set(path || "$", "{}");
      return;
    }

    for (const [key, child] of entries) {
      const nextPath = path ? `${path}.${key}` : key;
      flattenForDiff(child, nextPath, output);
    }
    return;
  }

  output.set(path || "$", toSummary(value));
}

function capDiff(diff: ObjectDiff): ObjectDiff {
  const sequence: Array<{ kind: "added" | "removed" | "changed"; value: string | DiffChange }> = [];

  for (const value of diff.added) {
    sequence.push({ kind: "added", value });
  }
  for (const value of diff.removed) {
    sequence.push({ kind: "removed", value });
  }
  for (const value of diff.changed) {
    sequence.push({ kind: "changed", value });
  }

  if (sequence.length <= MAX_DIFF_ITEMS) {
    return diff;
  }

  const hiddenCount = sequence.length - (MAX_DIFF_ITEMS - 1);
  const visible = sequence.slice(0, MAX_DIFF_ITEMS - 1);
  const capped: ObjectDiff = { added: [], removed: [], changed: [] };

  for (const item of visible) {
    if (item.kind === "added") {
      capped.added.push(item.value as string);
    } else if (item.kind === "removed") {
      capped.removed.push(item.value as string);
    } else {
      capped.changed.push(item.value as DiffChange);
    }
  }

  capped.added.push(`...and ${hiddenCount} more changes`);
  return capped;
}

export function diffObjects(before: unknown, after: unknown): ObjectDiff {
  const beforeMap = new Map<string, string>();
  const afterMap = new Map<string, string>();

  flattenForDiff(redactObject(before), "", beforeMap);
  flattenForDiff(redactObject(after), "", afterMap);

  const keys = new Set<string>([...beforeMap.keys(), ...afterMap.keys()]);
  const sortedKeys = [...keys].sort((left, right) => left.localeCompare(right));

  const added: string[] = [];
  const removed: string[] = [];
  const changed: DiffChange[] = [];

  for (const key of sortedKeys) {
    const beforeValue = beforeMap.get(key);
    const afterValue = afterMap.get(key);

    if (beforeValue === undefined && afterValue !== undefined) {
      added.push(`${key}: ${afterValue}`);
      continue;
    }

    if (beforeValue !== undefined && afterValue === undefined) {
      removed.push(`${key}: ${beforeValue}`);
      continue;
    }

    if (beforeValue !== afterValue && beforeValue !== undefined && afterValue !== undefined) {
      changed.push({
        path: key,
        from: beforeValue,
        to: afterValue
      });
    }
  }

  return capDiff({ added, removed, changed });
}

function normalizeLoggingMode(value: unknown): LoggingMode {
  return value === "verbose" || value === "detailed" ? "detailed" : "simple";
}

function normalizeAgentProvider(value: unknown): AgentProvider {
  if (value === "google_gemini" || value === "anthropic_claude" || value === "openai_compat") {
    return value;
  }
  return "openai_compat";
}

function normalizeAgent(input: Record<string, unknown>): Agent {
  const policy = (input.policy ?? {}) as Record<string, unknown>;
  const toolsEnabled = Array.isArray(policy.toolsEnabled)
    ? policy.toolsEnabled.filter((value): value is string => typeof value === "string")
    : [];
  const preferredName =
    typeof input.preferredName === "string" && input.preferredName.trim().length
      ? input.preferredName.trim()
      : undefined;
  const tone =
    input.tone === "concise" || input.tone === "detailed"
      ? input.tone
      : undefined;
  const hasAskedName =
    typeof input.hasAskedName === "boolean" ? input.hasAskedName : false;
  const hasAskedTone =
    typeof input.hasAskedTone === "boolean" ? input.hasAskedTone : false;
  const hasAskedAgentName =
    typeof input.hasAskedAgentName === "boolean" ? input.hasAskedAgentName : false;
  const rawModelId =
    typeof input.modelId === "string"
      ? input.modelId
      : typeof input.openaiCompatModelOverride === "string"
        ? input.openaiCompatModelOverride
        : "";
  const modelId = rawModelId.trim() ? rawModelId.trim() : undefined;

  return {
    id: typeof input.id === "string" ? input.id : crypto.randomUUID(),
    name: typeof input.name === "string" ? input.name : "Unnamed Agent",
    purpose: typeof input.purpose === "string" ? input.purpose : "",
    provider: normalizeAgentProvider(input.provider),
    modelId,
    openaiCompatBaseUrlOverride:
      typeof input.openaiCompatBaseUrlOverride === "string" && input.openaiCompatBaseUrlOverride.trim()
        ? input.openaiCompatBaseUrlOverride
        : null,
    openaiCompatModelOverride:
      typeof input.openaiCompatModelOverride === "string" && input.openaiCompatModelOverride.trim()
        ? input.openaiCompatModelOverride
        : null,
    preferredName,
    tone,
    hasAskedName,
    hasAskedTone,
    hasAskedAgentName,
    archived: Boolean(input.archived),
    policy: {
      memoryMode: policy.memoryMode === "shared" ? "shared" : "isolated",
      loggingMode: normalizeLoggingMode(policy.loggingMode),
      toolsEnabled
    },
    createdAt: typeof input.createdAt === "string" ? input.createdAt : new Date().toISOString()
  };
}

function normalizeMissionGoalForFingerprint(goal: string): string {
  return goal.trim().toLowerCase().replace(/\s+/g, " ");
}

function missionScheduleSignatureForFingerprint(inputSchedule: {
  kind: Mission["schedule"]["kind"];
  time?: string;
  weekday?: number;
  intervalMinutes?: number;
}): string {
  return [
    inputSchedule.kind,
    inputSchedule.time ?? "",
    String(inputSchedule.weekday ?? ""),
    String(inputSchedule.intervalMinutes ?? "")
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
  inputSchedule: {
    kind: Mission["schedule"]["kind"];
    time?: string;
    weekday?: number;
    intervalMinutes?: number;
  },
  goal: string
): string {
  const signature = missionScheduleSignatureForFingerprint(inputSchedule);
  const normalizedGoal = normalizeMissionGoalForFingerprint(goal);
  return `msn_${hashMissionFingerprint(`${agentId}|${signature}|${normalizedGoal}`)}`;
}

function normalizeMission(input: Record<string, unknown>): Mission {
  const normalizeTime = (value: unknown): string | undefined => {
    if (typeof value !== "string") {
      return undefined;
    }
    const match = value.trim().match(/^([01]?\d|2[0-3]):([0-5]\d)$/);
    if (!match) {
      return undefined;
    }
    const hour = String(Number(match[1])).padStart(2, "0");
    const minute = String(Number(match[2])).padStart(2, "0");
    return `${hour}:${minute}`;
  };

  const normalizeWeekday = (value: unknown): number | undefined => {
    if (typeof value !== "number" || !Number.isInteger(value)) {
      return undefined;
    }
    if (value < 0 || value > 6) {
      return undefined;
    }
    return value;
  };

  const normalizeIntervalMinutes = (value: unknown): number | undefined => {
    if (typeof value !== "number" || !Number.isFinite(value)) {
      return undefined;
    }
    const rounded = Math.trunc(value);
    if (rounded < 1) {
      return undefined;
    }
    return Math.min(rounded, 1_440);
  };

  const buildCron = (inputSchedule: {
    kind: Mission["schedule"]["kind"];
    time?: string;
    weekday?: number;
    intervalMinutes?: number;
    cron?: string;
  }): string => {
    const explicitCron = inputSchedule.cron?.trim();
    if (explicitCron) {
      return explicitCron;
    }

    if (inputSchedule.kind === "every_minutes") {
      const interval = inputSchedule.intervalMinutes ?? 15;
      return `*/${interval} * * * *`;
    }

    const time = inputSchedule.time ?? "09:00";
    const [hour, minute] = time.split(":");
    const minutePart = Number(minute);
    const hourPart = Number(hour);
    if (inputSchedule.kind === "weekdays") {
      return `${minutePart} ${hourPart} * * 1-5`;
    }
    if (inputSchedule.kind === "weekly") {
      return `${minutePart} ${hourPart} * * ${inputSchedule.weekday ?? 1}`;
    }
    if (inputSchedule.kind === "custom") {
      return "0 9 * * *";
    }
    return `${minutePart} ${hourPart} * * *`;
  };

  const parseLegacyCron = (cronRaw: string): Mission["schedule"] => {
    const cron = cronRaw.trim();
    const everyMinutes = cron.match(/^\*\/(\d+)\s+\*\s+\*\s+\*\s+\*$/);
    if (everyMinutes) {
      const interval = Math.max(1, Math.min(1_440, Number(everyMinutes[1]) || 15));
      return {
        kind: "every_minutes",
        intervalMinutes: interval,
        cron: `*/${interval} * * * *`
      };
    }

    const weekdays = cron.match(/^(\d{1,2})\s+(\d{1,2})\s+\*\s+\*\s+1-5$/);
    if (weekdays) {
      const minute = String(Number(weekdays[1])).padStart(2, "0");
      const hour = String(Number(weekdays[2])).padStart(2, "0");
      const time = `${hour}:${minute}`;
      return {
        kind: "weekdays",
        time,
        cron: `${Number(minute)} ${Number(hour)} * * 1-5`
      };
    }

    const weekly = cron.match(/^(\d{1,2})\s+(\d{1,2})\s+\*\s+\*\s+([0-6])$/);
    if (weekly) {
      const minute = String(Number(weekly[1])).padStart(2, "0");
      const hour = String(Number(weekly[2])).padStart(2, "0");
      const weekday = Number(weekly[3]);
      const time = `${hour}:${minute}`;
      return {
        kind: "weekly",
        time,
        weekday,
        cron: `${Number(minute)} ${Number(hour)} * * ${weekday}`
      };
    }

    const daily = cron.match(/^(\d{1,2})\s+(\d{1,2})\s+\*\s+\*\s+\*$/);
    if (daily) {
      const minute = String(Number(daily[1])).padStart(2, "0");
      const hour = String(Number(daily[2])).padStart(2, "0");
      const time = `${hour}:${minute}`;
      return {
        kind: "daily",
        time,
        cron: `${Number(minute)} ${Number(hour)} * * *`
      };
    }

    return {
      kind: "custom",
      cron: cron || "0 9 * * *"
    };
  };

  const normalizeSchedule = (): Mission["schedule"] => {
    if (typeof input.schedule === "string") {
      return parseLegacyCron(input.schedule);
    }

    if (input.schedule && typeof input.schedule === "object" && !Array.isArray(input.schedule)) {
      const raw = input.schedule as Record<string, unknown>;
      const kind =
        raw.kind === "daily" ||
        raw.kind === "weekdays" ||
        raw.kind === "every_minutes" ||
        raw.kind === "weekly" ||
        raw.kind === "custom"
          ? raw.kind
          : "daily";
      const time = normalizeTime(raw.time);
      const weekday = normalizeWeekday(raw.weekday);
      const intervalMinutes = normalizeIntervalMinutes(raw.intervalMinutes);
      const schedule: Mission["schedule"] =
        kind === "every_minutes"
          ? {
              kind,
              intervalMinutes: intervalMinutes ?? 15,
              cron: buildCron({
                kind,
                intervalMinutes: intervalMinutes ?? 15,
                cron: typeof raw.cron === "string" ? raw.cron : undefined
              })
            }
          : kind === "weekly"
            ? {
                kind,
                time: time ?? "09:00",
                weekday: weekday ?? 1,
                cron: buildCron({
                  kind,
                  time: time ?? "09:00",
                  weekday: weekday ?? 1,
                  cron: typeof raw.cron === "string" ? raw.cron : undefined
                })
              }
            : kind === "custom"
              ? {
                  kind,
                  cron: buildCron({
                    kind,
                    cron: typeof raw.cron === "string" ? raw.cron : undefined
                  })
                }
              : {
                  kind,
                  time: time ?? "09:00",
                  cron: buildCron({
                    kind,
                    time: time ?? "09:00",
                    cron: typeof raw.cron === "string" ? raw.cron : undefined
                  })
                };
      return schedule;
    }

    return {
      kind: "daily",
      time: "09:00",
      cron: "0 9 * * *"
    };
  };

  const normalizeIso = (value: unknown): string | undefined => {
    if (typeof value !== "string") {
      return undefined;
    }
    const parsed = new Date(value);
    if (Number.isNaN(parsed.getTime())) {
      return undefined;
    }
    return parsed.toISOString();
  };

  const nowIso = new Date().toISOString();
  const schedule = normalizeSchedule();
  const createdAt = normalizeIso(input.createdAt) ?? nowIso;
  const updatedAt = normalizeIso(input.updatedAt) ?? createdAt;
  const legacySchedule = typeof input.schedule === "string" ? input.schedule.trim() : "";
  const enabled =
    typeof input.enabled === "boolean"
      ? input.enabled
      : typeof input.schedule === "object" && input.schedule !== null
        ? true
        : legacySchedule.length > 0;

  return {
    id: typeof input.id === "string" ? input.id : crypto.randomUUID(),
    agentId:
      typeof input.agentId === "string" && input.agentId.trim().length > 0
        ? input.agentId
        : "__unassigned__",
    fingerprint:
      typeof input.fingerprint === "string" && input.fingerprint.trim().length > 0
        ? input.fingerprint.trim()
        : buildMissionFingerprint(
            typeof input.agentId === "string" && input.agentId.trim().length > 0
              ? input.agentId
              : "__unassigned__",
            schedule,
            typeof input.goal === "string"
              ? input.goal
              : typeof input.notes === "string"
                ? input.notes
                : ""
          ),
    title:
      typeof input.title === "string" && input.title.trim().length > 0
        ? input.title
        : typeof input.name === "string" && input.name.trim().length > 0
          ? input.name
          : "Mission",
    goal:
      typeof input.goal === "string"
        ? input.goal
        : typeof input.notes === "string"
          ? input.notes
          : "",
    enabled,
    chatPosting:
      input.chatPosting === "off" || input.chatPosting === "summary" || input.chatPosting === "verbose"
        ? input.chatPosting
        : "summary",
    collapseRepeats:
      typeof input.collapseRepeats === "boolean" ? input.collapseRepeats : true,
    schedule,
    nextRunAt: normalizeIso(input.nextRunAt) ?? createdAt,
    lastRunAt: normalizeIso(input.lastRunAt),
    archived: Boolean(input.archived),
    createdAt,
    updatedAt
  };
}

function normalizeWorkspace(input: Record<string, unknown>): Workspace {
  return {
    id: typeof input.id === "string" ? input.id : crypto.randomUUID(),
    name: typeof input.name === "string" ? input.name : "Default Workspace",
    path: typeof input.path === "string" ? input.path : null,
    archived: Boolean(input.archived),
    createdAt: typeof input.createdAt === "string" ? input.createdAt : new Date().toISOString()
  };
}

function normalizeSkillInstall(input: Record<string, unknown>): SkillInstallConfig {
  return {
    id: typeof input.id === "string" ? input.id : crypto.randomUUID(),
    version: typeof input.version === "string" ? input.version : "0.0.0",
    channel: typeof input.channel === "string" ? input.channel : "verified",
    installDir: typeof input.installDir === "string" ? input.installDir : "",
    installedAt: typeof input.installedAt === "string" ? input.installedAt : new Date().toISOString(),
    archived: Boolean(input.archived)
  };
}

function normalizeWebPolicy(input: Record<string, unknown>): WebPolicy {
  const level =
    input.level === "auth" || input.level === "browser" || input.level === "public"
      ? input.level
      : "public";

  return {
    host:
      typeof input.host === "string"
        ? input.host.toLowerCase()
        : typeof input.id === "string"
          ? input.id.toLowerCase()
          : "",
    level,
    allowPaths: Array.isArray(input.allowPaths)
      ? input.allowPaths.filter((value): value is string => typeof value === "string")
      : undefined,
    approvedAt:
      typeof input.approvedAt === "string" ? input.approvedAt : new Date().toISOString(),
    approvedBy: input.approvedBy === "agent" ? "agent" : "user",
    notes: typeof input.notes === "string" ? input.notes : undefined,
    archived: Boolean(input.archived)
  };
}

function normalizeFilePolicy(input: Record<string, unknown>): FilePolicy {
  const mode = input.mode === "read_write" ? "read_write" : "read";

  const rawPath =
    typeof input.path === "string"
      ? input.path
      : typeof input.id === "string"
        ? input.id
        : "";
  const normalizedPath = rawPath.replace(/\\/g, "/").replace(/\/+$/, "") || "/";

  return {
    path: normalizedPath,
    mode,
    approvedAt:
      typeof input.approvedAt === "string" ? input.approvedAt : new Date().toISOString(),
    approvedBy: input.approvedBy === "agent" ? "agent" : "user",
    archived: Boolean(input.archived)
  };
}

function normalizeByKind<K extends ConfigObjectKind>(kind: K, input: Record<string, unknown>): ConfigRecordByKind[K] {
  if (kind === "agent") {
    return normalizeAgent(input) as ConfigRecordByKind[K];
  }
  if (kind === "mission") {
    return normalizeMission(input) as ConfigRecordByKind[K];
  }
  if (kind === "workspace") {
    return normalizeWorkspace(input) as ConfigRecordByKind[K];
  }
  if (kind === "web_policy") {
    return normalizeWebPolicy(input) as ConfigRecordByKind[K];
  }
  if (kind === "file_policy") {
    return normalizeFilePolicy(input) as ConfigRecordByKind[K];
  }
  return normalizeSkillInstall(input) as ConfigRecordByKind[K];
}

function isVersionedRecord<T>(value: unknown): value is Versioned<T> {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return false;
  }

  const candidate = value as Record<string, unknown>;
  return (
    typeof candidate.id === "string" &&
    typeof candidate.version === "number" &&
    typeof candidate.updatedAt === "string" &&
    candidate.data !== undefined
  );
}

function toRedactedSnapshot(value: unknown): Record<string, unknown> | undefined {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return undefined;
  }

  return redactObject(value) as Record<string, unknown>;
}

async function saveVersionHistory<K extends ConfigObjectKind>(
  kind: K,
  items: Versioned<ConfigRecordByKind[K]>[]
): Promise<void> {
  await saveJson(STORE_FILE_BY_KIND[kind], items);
}

async function loadVersionHistory<K extends ConfigObjectKind>(
  kind: K
): Promise<Versioned<ConfigRecordByKind[K]>[]> {
  const file = STORE_FILE_BY_KIND[kind];
  const raw = await loadJson<unknown[]>(file, []);

  if (!Array.isArray(raw) || !raw.length) {
    return [];
  }

  if (raw.every((entry) => isVersionedRecord<ConfigRecordByKind[K]>(entry))) {
    return raw.map((entry) => {
      const versioned = entry as Versioned<Record<string, unknown>>;
      return {
        id: versioned.id,
        version: versioned.version,
        updatedAt: versioned.updatedAt,
        data: normalizeByKind(kind, versioned.data)
      } as Versioned<ConfigRecordByKind[K]>;
    });
  }

  const now = new Date().toISOString();
  const converted = raw
    .filter((entry): entry is Record<string, unknown> => Boolean(entry) && typeof entry === "object")
    .map((entry) => ({
      id: typeof entry.id === "string" ? entry.id : crypto.randomUUID(),
      version: 1,
      updatedAt: now,
      data: normalizeByKind(kind, entry)
    }));

  await saveVersionHistory(kind, converted);
  return converted;
}

function latestById<K extends ConfigObjectKind>(
  records: Versioned<ConfigRecordByKind[K]>[]
): Map<string, Versioned<ConfigRecordByKind[K]>> {
  const map = new Map<string, Versioned<ConfigRecordByKind[K]>>();

  for (const record of records) {
    const current = map.get(record.id);
    if (!current || record.version > current.version) {
      map.set(record.id, record);
    }
  }

  return map;
}

export async function loadLatestConfig<K extends ConfigObjectKind>(
  kind: K
): Promise<ConfigRecordByKind[K][]> {
  const history = await loadVersionHistory(kind);
  return [...latestById(history).values()]
    .filter((record) => !record.data.archived)
    .sort((left, right) => right.updatedAt.localeCompare(left.updatedAt))
    .map((record) => record.data);
}

export async function loadConfigVersions<K extends ConfigObjectKind>(
  kind: K,
  id: string
): Promise<Versioned<ConfigRecordByKind[K]>[]> {
  const history = await loadVersionHistory(kind);
  return history
    .filter((record) => record.id === id)
    .sort((left, right) => right.version - left.version);
}

export async function listVersions<K extends ConfigObjectKind>(
  kind: K,
  id: string
): Promise<Versioned<ConfigRecordByKind[K]>[]> {
  return loadConfigVersions(kind, id);
}

export async function getLatestConfigVersion<K extends ConfigObjectKind>(
  kind: K,
  id: string
): Promise<Versioned<ConfigRecordByKind[K]> | null> {
  const versions = await loadConfigVersions(kind, id);
  return versions[0] ?? null;
}

export async function getLatest<K extends ConfigObjectKind>(
  kind: K,
  id: string
): Promise<Versioned<ConfigRecordByKind[K]> | null> {
  return getLatestConfigVersion(kind, id);
}

export async function appendAudit(entry: AuditEntry): Promise<void> {
  const existing = await loadJson<AuditEntry[]>("audit_log.json", []);
  await saveJson("audit_log.json", existing.concat(entry));
}

export async function loadAuditLog(): Promise<AuditEntry[]> {
  const entries = await loadJson<AuditEntry[]>("audit_log.json", []);
  return [...entries].sort((left, right) => right.ts.localeCompare(left.ts));
}

function buildAuditAction(
  before: { archived?: boolean } | null,
  after: { archived?: boolean }
): "create" | "update" | "delete" {
  if (!before) {
    return after.archived ? "delete" : "create";
  }

  if (!before.archived && after.archived) {
    return "delete";
  }

  return "update";
}

function asRecord(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return {};
  }

  return value as Record<string, unknown>;
}

export async function applyConfigChange<K extends ConfigObjectKind>(
  proposal: ConfigChangeProposal,
  actor: AuditEntry["actor"]
): Promise<{
  undoToken: UndoToken;
  latest: Versioned<ConfigRecordByKind[K]>;
  audit: AuditEntry;
}> {
  const kind = proposal.object.kind as K;
  const id = proposal.object.id;
  const history = await loadVersionHistory(kind);
  const latestMap = latestById(history);
  const previous = latestMap.get(id) ?? null;
  const beforeData = previous?.data ?? null;

  const normalizedAfter = normalizeByKind(kind, {
    ...asRecord(proposal.patch.after),
    id
  });

  const nextVersion = (previous?.version ?? 0) + 1;
  const now = new Date().toISOString();
  const nextRecord: Versioned<ConfigRecordByKind[K]> = {
    id,
    version: nextVersion,
    updatedAt: now,
    data: normalizedAfter
  };

  history.push(nextRecord);
  await saveVersionHistory(kind, history);

  const audit: AuditEntry = {
    id: crypto.randomUUID(),
    ts: now,
    actor,
    object: {
      kind,
      id
    },
    action: buildAuditAction(beforeData, normalizedAfter),
    beforeVersion: previous?.version,
    afterVersion: nextVersion,
    summary: proposal.summary,
    diff: diffObjects(beforeData, normalizedAfter),
    snapshot: {
      before: toRedactedSnapshot(beforeData),
      after: toRedactedSnapshot(normalizedAfter)
    }
  };

  await appendAudit(audit);

  return {
    undoToken: {
      kind,
      id,
      previousVersion: previous?.version ?? null,
      summary: proposal.summary
    },
    latest: nextRecord,
    audit
  };
}

export async function rollbackToVersion<K extends ConfigObjectKind>(
  kind: K,
  id: string,
  version: number,
  actor: AuditEntry["actor"],
  summary?: string
): Promise<Versioned<ConfigRecordByKind[K]>> {
  const history = await loadVersionHistory(kind);
  const allForObject = history.filter((record) => record.id === id);
  const previous = allForObject.sort((left, right) => right.version - left.version)[0];
  const target = allForObject.find((record) => record.version === version);

  if (!previous || !target) {
    throw new Error("Unable to rollback because target version was not found.");
  }

  const now = new Date().toISOString();
  const nextRecord: Versioned<ConfigRecordByKind[K]> = {
    id,
    version: previous.version + 1,
    updatedAt: now,
    data: normalizeByKind(kind, asRecord(target.data))
  };

  history.push(nextRecord);
  await saveVersionHistory(kind, history);

  const audit: AuditEntry = {
    id: crypto.randomUUID(),
    ts: now,
    actor,
    object: { kind, id },
    action: "rollback",
    beforeVersion: previous.version,
    afterVersion: nextRecord.version,
    summary: summary ?? `Rollback to version ${version}`,
    diff: diffObjects(previous.data, nextRecord.data),
    snapshot: {
      before: toRedactedSnapshot(previous.data),
      after: toRedactedSnapshot(nextRecord.data)
    }
  };

  await appendAudit(audit);
  return nextRecord;
}

export async function undoChange(
  token: UndoToken,
  actor: AuditEntry["actor"]
): Promise<void> {
  if (typeof token.previousVersion === "number") {
    await rollbackToVersion(
      token.kind,
      token.id,
      token.previousVersion,
      actor,
      `Undo ${token.summary}`
    );
    return;
  }

  const latest = await getLatestConfigVersion(token.kind, token.id);
  if (!latest) {
    return;
  }

  const archivedData = {
    ...asRecord(latest.data),
    archived: true
  };

  const proposal: ConfigChangeProposal = {
    id: crypto.randomUUID(),
    ts: new Date().toISOString(),
    object: {
      kind: token.kind,
      id: token.id
    },
    summary: `Undo ${token.summary}`,
    diff: diffObjects(latest.data, archivedData),
    applyMode: "autopilot",
    requiresConfirm: true,
    proposedBy: {
      type: "user",
      id: "system"
    },
    patch: {
      after: archivedData
    }
  };

  await applyConfigChange(proposal, actor);
}
