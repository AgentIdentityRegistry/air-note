import type { ErrorObject } from "ajv";
import Ajv2020 from "ajv/dist/2020";
import planSchema from "./schema/plan.v1.schema.json";

export type PlannerMode = "autopilot" | "fsd";

export type PlanStep = {
  id?: string;
  title: string;
  tool: string;
  instruction?: string;
  input?: string;
  expectedOutput?: string;
  safe?: boolean;
  retry?: {
    maxAttempts?: number;
    backoffMs?: number;
  };
};

export type PlanConfigProposal = {
  object: {
    kind: "workspace" | "agent" | "mission" | "skill_install";
    id: string;
  };
  summary: string;
  applyMode?: PlannerMode;
  after: Record<string, unknown>;
};

export type PlanMissionProposal = {
  type: "create_mission";
  title: string;
  goal: string;
  schedule: {
    kind: "daily" | "weekdays" | "every_minutes" | "weekly";
    time?: string;
    intervalMinutes?: number;
    weekday?: number;
  };
  autonomy: PlannerMode;
};

export type BossClawPlanV1 = {
  schema: "bossclaw.plan.v1";
  goal: string;
  mode: PlannerMode;
  summary?: string;
  estimatedCostUsd?: number;
  permissionExpansions?: string[];
  steps: PlanStep[];
  configProposal?: PlanConfigProposal;
  missionProposal?: PlanMissionProposal;
};

const ajv = new Ajv2020({
  allErrors: true,
  strict: false
});

const validate = ajv.compile(planSchema);

function formatError(error: ErrorObject): string {
  const fieldPath = error.instancePath ? error.instancePath.slice(1).replace(/\//g, ".") : "root";
  const message = error.message ?? "Invalid value";
  return `${fieldPath}: ${message}`;
}

function extractJsonObject(raw: string): string {
  const trimmed = raw.trim();
  if (!trimmed) {
    throw new Error("Planner returned an empty response.");
  }

  if (trimmed.startsWith("{")) {
    return trimmed;
  }

  const codeFenceMatch = trimmed.match(/```(?:json)?\s*([\s\S]*?)```/i);
  if (codeFenceMatch?.[1]) {
    return codeFenceMatch[1].trim();
  }

  const start = trimmed.indexOf("{");
  const end = trimmed.lastIndexOf("}");
  if (start >= 0 && end > start) {
    return trimmed.slice(start, end + 1);
  }

  throw new Error("Planner response did not contain a JSON object.");
}

export function validatePlan(
  payload: unknown
): { ok: true; data: BossClawPlanV1 } | { ok: false; errors: string[] } {
  const isValid = validate(payload);

  if (isValid) {
    return {
      ok: true,
      data: payload as BossClawPlanV1
    };
  }

  const errors = (validate.errors ?? []).map(formatError);
  return {
    ok: false,
    errors: errors.length ? errors : ["Plan validation failed"]
  };
}

export function parseAndValidatePlanText(
  raw: string
): { ok: true; data: BossClawPlanV1; normalizedText: string } | { ok: false; errors: string[] } {
  try {
    const jsonText = extractJsonObject(raw);
    const parsed = JSON.parse(jsonText) as unknown;
    const validation = validatePlan(parsed);

    if (!validation.ok) {
      return validation;
    }

    return {
      ok: true,
      data: validation.data,
      normalizedText: jsonText
    };
  } catch (error) {
    return {
      ok: false,
      errors: [error instanceof Error ? error.message : "Unable to parse planner response"]
    };
  }
}
