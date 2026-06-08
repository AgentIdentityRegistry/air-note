import type { ConfigChangeProposal, FilePolicy } from "../../models";

const READ_MAX_BYTES = 2 * 1024 * 1024;
const WRITE_MAX_BYTES = 1024 * 1024;

const TEXT_EXTENSIONS = new Set([".txt", ".md", ".json", ".csv", ".log"]);

type BaseFailure = {
  ok: false;
  message: string;
};

type RequiresApproval = BaseFailure & {
  reason: "requires_approval";
  folderPath: string;
  proposal: ConfigChangeProposal;
};

type InvalidPathFailure = BaseFailure & {
  reason: "invalid_path" | "unsupported_type" | "scope_blocked" | "io_error";
};

type OverwriteApprovalFailure = BaseFailure & {
  reason: "overwrite_requires_approval";
};

export type FileReadResult =
  | {
      ok: true;
      path: string;
      text: string;
      bytes: number;
      readAt: string;
      latencyMs: number;
    }
  | RequiresApproval
  | InvalidPathFailure;

export type FileWriteResult =
  | {
      ok: true;
      path: string;
      bytesWritten: number;
      wroteAt: string;
      latencyMs: number;
    }
  | RequiresApproval
  | InvalidPathFailure
  | OverwriteApprovalFailure;

type FileReadResponse = {
  path: string;
  text: string;
  bytes: number;
};

type FileWriteResponse = {
  path: string;
  bytesWritten: number;
};

type FileReadDeps = {
  policies: FilePolicy[];
  invokeRead: (path: string, maxBytes: number) => Promise<FileReadResponse>;
  buildApprovalProposal: (folderPath: string, mode: "read" | "read_write") => ConfigChangeProposal;
};

type FileWriteDeps = {
  policies: FilePolicy[];
  invokeExists: (path: string) => Promise<boolean>;
  invokeWrite: (
    path: string,
    text: string,
    createIfMissing: boolean,
    maxBytes: number
  ) => Promise<FileWriteResponse>;
  buildApprovalProposal: (folderPath: string, mode: "read" | "read_write") => ConfigChangeProposal;
  requireOverwriteApproval: boolean;
  allowOverwrite: boolean;
};

function normalizeFsPath(value: string): string {
  const raw = value.trim().replace(/\\/g, "/");
  if (!raw.startsWith("/")) {
    return raw.replace(/\/+$/, "");
  }

  const segments = raw.split("/");
  const stack: string[] = [];
  for (const segment of segments) {
    if (!segment || segment === ".") {
      continue;
    }
    if (segment === "..") {
      stack.pop();
      continue;
    }
    stack.push(segment);
  }

  const normalized = `/${stack.join("/")}`.replace(/\/+$/, "");
  return normalized || "/";
}

function isAbsolutePath(value: string): boolean {
  return value.startsWith("/");
}

function getFolderPath(filePath: string): string {
  const normalized = normalizeFsPath(filePath);
  const index = normalized.lastIndexOf("/");
  if (index <= 0) {
    return "/";
  }
  return normalized.slice(0, index) || "/";
}

function isPathInsideFolder(path: string, folderPath: string): boolean {
  const normalizedPath = normalizeFsPath(path);
  const normalizedFolder = normalizeFsPath(folderPath);
  if (normalizedFolder === "/") {
    return true;
  }
  return normalizedPath === normalizedFolder || normalizedPath.startsWith(`${normalizedFolder}/`);
}

function hasTextExtension(path: string): boolean {
  const normalizedPath = normalizeFsPath(path).toLowerCase();
  const index = normalizedPath.lastIndexOf(".");
  if (index < 0) {
    return false;
  }
  const extension = normalizedPath.slice(index);
  return TEXT_EXTENSIONS.has(extension);
}

function hasPermission(
  policies: FilePolicy[],
  path: string,
  mode: "read" | "read_write"
): boolean {
  return policies
    .filter((policy) => !policy.archived)
    .sort((left, right) => right.path.length - left.path.length)
    .some((policy) => {
      if (!isPathInsideFolder(path, policy.path)) {
        return false;
      }
      if (mode === "read") {
        return policy.mode === "read" || policy.mode === "read_write";
      }
      return policy.mode === "read_write";
    });
}

function parseCommandPath(value: string): string {
  const trimmed = value.trim();
  if (
    (trimmed.startsWith('"') && trimmed.endsWith('"')) ||
    (trimmed.startsWith("'") && trimmed.endsWith("'"))
  ) {
    return trimmed.slice(1, -1);
  }
  return trimmed;
}

export function parseReadCommand(input: string): string | null {
  const match = input.trim().match(/^\/read\s+(.+)$/i);
  if (!match) {
    return null;
  }
  const parsedPath = parseCommandPath(match[1] ?? "");
  return parsedPath.trim() || null;
}

export function parseWriteCommand(
  input: string
): {
  path: string;
  text: string;
} | null {
  const trimmed = input.trim();
  const match = trimmed.match(/^\/write\s+(".*?"|'.*?'|\S+)\s+([\s\S]+)$/i);
  if (!match) {
    return null;
  }

  const path = parseCommandPath(match[1] ?? "");
  const text = (match[2] ?? "").trim();
  if (!path.trim() || !text) {
    return null;
  }

  return {
    path,
    text
  };
}

export function normalizeFolderPath(path: string): string {
  return normalizeFsPath(path);
}

export async function fileReadTool(path: string, deps: FileReadDeps): Promise<FileReadResult> {
  const normalizedPath = normalizeFsPath(path);
  if (!normalizedPath || !isAbsolutePath(normalizedPath)) {
    return {
      ok: false,
      reason: "invalid_path",
      message: "Use an absolute file path."
    };
  }
  if (!hasTextExtension(normalizedPath)) {
    return {
      ok: false,
      reason: "unsupported_type",
      message: "Only text files are supported (.txt, .md, .json, .csv, .log)."
    };
  }

  if (!hasPermission(deps.policies, normalizedPath, "read")) {
    const folderPath = getFolderPath(normalizedPath);
    return {
      ok: false,
      reason: "requires_approval",
      folderPath,
      message: `File access to ${folderPath} is not approved yet.`,
      proposal: deps.buildApprovalProposal(folderPath, "read")
    };
  }

  const startedAt = performance.now();
  try {
    const result = await deps.invokeRead(normalizedPath, READ_MAX_BYTES);
    return {
      ok: true,
      path: result.path,
      text: result.text,
      bytes: result.bytes,
      readAt: new Date().toISOString(),
      latencyMs: Math.max(0, Math.round(performance.now() - startedAt))
    };
  } catch {
    return {
      ok: false,
      reason: "io_error",
      message: "Unable to read this file."
    };
  }
}

export async function fileWriteTool(
  input: {
    path: string;
    text: string;
    createIfMissing?: boolean;
  },
  deps: FileWriteDeps
): Promise<FileWriteResult> {
  const normalizedPath = normalizeFsPath(input.path);
  if (!normalizedPath || !isAbsolutePath(normalizedPath)) {
    return {
      ok: false,
      reason: "invalid_path",
      message: "Use an absolute file path."
    };
  }
  if (!hasTextExtension(normalizedPath)) {
    return {
      ok: false,
      reason: "unsupported_type",
      message: "Only text files are supported (.txt, .md, .json, .csv, .log)."
    };
  }

  const bytes = new TextEncoder().encode(input.text).length;
  if (bytes > WRITE_MAX_BYTES) {
    return {
      ok: false,
      reason: "scope_blocked",
      message: "Text is too large to write safely."
    };
  }

  if (!hasPermission(deps.policies, normalizedPath, "read_write")) {
    const folderPath = getFolderPath(normalizedPath);
    return {
      ok: false,
      reason: "requires_approval",
      folderPath,
      message: `Write access to ${folderPath} is not approved yet.`,
      proposal: deps.buildApprovalProposal(folderPath, "read_write")
    };
  }

  const exists = await deps.invokeExists(normalizedPath).catch(() => false);
  if (exists && deps.requireOverwriteApproval && !deps.allowOverwrite) {
    return {
      ok: false,
      reason: "overwrite_requires_approval",
      message: "This file already exists. Confirm overwrite to continue."
    };
  }

  const startedAt = performance.now();
  try {
    const result = await deps.invokeWrite(
      normalizedPath,
      input.text,
      Boolean(input.createIfMissing),
      WRITE_MAX_BYTES
    );
    return {
      ok: true,
      path: result.path,
      bytesWritten: result.bytesWritten,
      wroteAt: new Date().toISOString(),
      latencyMs: Math.max(0, Math.round(performance.now() - startedAt))
    };
  } catch {
    return {
      ok: false,
      reason: "io_error",
      message: "Unable to write this file."
    };
  }
}
