import type { ErrorObject } from "ajv";
import Ajv2020 from "ajv/dist/2020";
import manifestSchema from "./schema/manifest.v1.schema.json";

export type PermissionDefault = "deny" | "allow";

export type Manifest = {
  id: string;
  name: string;
  version: string;
  description: string;
  author: string;
  minAirAgentVersion: string;
  category?: string[];
  tags?: string[];
  runtime:
    | {
        type: "mcp";
        transport: "stdio" | "http";
        package: {
          kind: string;
          name: string;
          version: string;
        };
      }
    | {
        type: "native";
        toolId: string;
      };
  configSchema?: {
    fields: Array<{
      key: string;
      type: "string" | "string[]" | "number" | "boolean" | "cron" | "enum";
      label: string;
      required?: boolean;
      default?: unknown;
      options?: string[];
    }>;
  };
  credentials?: Array<{
    id: string;
    kind: "llm" | "search" | "connector";
    providers: string[];
    required: boolean;
  }>;
  permissions: {
    network: {
      default: PermissionDefault;
      allowDomains: string[];
      allowUserAddDomains: boolean;
      approval: "always_confirm" | "once_then_remember";
    };
    files: {
      default: PermissionDefault;
      allowPaths: string[];
      approval: "always_confirm_destructive" | "once_then_remember";
    };
    clipboard: {
      default: PermissionDefault;
      approval: "always_confirm" | "once_then_remember";
    };
    notifications: {
      default: PermissionDefault;
    };
  };
  automation?: {
    supportsSchedule: boolean;
    supportsTriggers: string[];
  };
  budget?: {
    defaultMonthlyUsd: number;
    actionOnExceed: "pause_and_notify" | "notify_only" | "hard_stop";
  };
  ui?: {
    icon?: string;
    entrypoints?: Array<{
      id: string;
      label: string;
      type: "command" | "settings";
    }>;
    showInStore?: boolean;
  };
  trust: {
    channel: "verified" | "community" | "local";
    signed: boolean;
    source: {
      type: "registry" | "local";
      url?: string;
    };
  };
};

const ajv = new Ajv2020({
  allErrors: true,
  strict: false
});

const validate = ajv.compile(manifestSchema);

function formatError(error: ErrorObject): string {
  const fieldPath = error.instancePath ? error.instancePath.slice(1).replace(/\//g, ".") : "root";
  const message = error.message ?? "Invalid value";
  return `${fieldPath}: ${message}`;
}

export function validateManifest(
  manifest: unknown
): { ok: true; data: Manifest } | { ok: false; errors: string[] } {
  const isValid = validate(manifest);

  if (isValid) {
    return {
      ok: true,
      data: manifest as Manifest
    };
  }

  const errors = (validate.errors ?? []).map(formatError);
  return {
    ok: false,
    errors: errors.length ? errors : ["Manifest validation failed"]
  };
}
