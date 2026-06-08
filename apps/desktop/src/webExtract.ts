import type { WebPolicy } from "./models";
import { extractReadableDocument } from "./engine/web/readability";

export type WebExtractLevel = "public" | "auth" | "browser";

export type WebExtractSchema = {
  fields: Array<{
    name: string;
    description: string;
  }>;
};

export type WebExtractInput = {
  url: string;
  level?: WebExtractLevel;
  selectorHints?: string[];
  extractSchema?: WebExtractSchema;
};

export type WebExtractResult = {
  title?: string;
  text: string;
  markdown?: string;
  meta: {
    url: string;
    host: string;
    level: WebExtractLevel;
    fetchedAt: string;
  };
};

export type ParsedWebExtractInput =
  | {
      ok: true;
      input: WebExtractInput;
      host: string;
      pathname: string;
      url: URL;
    }
  | {
      ok: false;
      error: string;
    };

function toMarkdown(title: string | undefined, text: string): string | undefined {
  if (!text) {
    return undefined;
  }

  if (!title) {
    return text;
  }

  return `# ${title}\n\n${text}`;
}

export function normalizeWebLevel(input: unknown): WebExtractLevel | null {
  if (typeof input !== "string") {
    return null;
  }

  const normalized = input.trim().toLowerCase();
  if (normalized === "public" || normalized === "standard") {
    return "public";
  }
  if (normalized === "auth" || normalized === "signed-in" || normalized === "signed_in") {
    return "auth";
  }
  if (normalized === "browser" || normalized === "interactive") {
    return "browser";
  }
  return null;
}

function parseJsonLikeInput(value: string): Partial<WebExtractInput> | null {
  if (!value.startsWith("{")) {
    return null;
  }

  try {
    const parsed = JSON.parse(value) as unknown;
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
      return null;
    }
    return parsed as Partial<WebExtractInput>;
  } catch {
    return null;
  }
}

function firstUrlFromText(value: string): string | null {
  const match = value.match(/https?:\/\/[^\s)]+/i);
  return match ? match[0] : null;
}

export function parseWebExtractInput(input: {
  rawInput?: string | null;
  rawInstruction?: string | null;
}): ParsedWebExtractInput {
  const rawInput = input.rawInput?.trim() || "";
  const rawInstruction = input.rawInstruction?.trim() || "";

  let parsed: Partial<WebExtractInput> | null = null;
  if (rawInput) {
    parsed = parseJsonLikeInput(rawInput);
  }
  if (!parsed && rawInstruction) {
    parsed = parseJsonLikeInput(rawInstruction);
  }

  const candidateUrl =
    parsed?.url?.trim() || firstUrlFromText(rawInput) || firstUrlFromText(rawInstruction) || "";

  if (!candidateUrl) {
    return {
      ok: false,
      error: "web.extract requires a valid URL in step input or instruction."
    };
  }

  let url: URL;
  try {
    url = new URL(candidateUrl);
  } catch {
    return {
      ok: false,
      error: "web.extract URL is invalid."
    };
  }

  if (url.protocol !== "http:" && url.protocol !== "https:") {
    return {
      ok: false,
      error: "web.extract supports only http/https URLs."
    };
  }

  const host = url.host.toLowerCase();
  const level =
    normalizeWebLevel(parsed?.level) ??
    normalizeWebLevel(rawInput.includes("interactive") ? "interactive" : null) ??
    undefined;
  const pathname = url.pathname || "/";

  const selectorHints = Array.isArray(parsed?.selectorHints)
    ? parsed?.selectorHints.filter((value): value is string => typeof value === "string")
    : undefined;

  const extractSchema =
    parsed?.extractSchema &&
    typeof parsed.extractSchema === "object" &&
    Array.isArray((parsed.extractSchema as WebExtractSchema).fields)
      ? (parsed.extractSchema as WebExtractSchema)
      : undefined;

  return {
    ok: true,
    host,
    pathname,
    url,
    input: {
      url: url.toString(),
      level,
      selectorHints,
      extractSchema
    }
  };
}

export function isPolicyPathAllowed(policy: WebPolicy, pathname: string): boolean {
  const configured = policy.allowPaths ?? [];
  if (!configured.length) {
    return true;
  }

  const normalizedPathname = pathname.startsWith("/") ? pathname : `/${pathname}`;
  return configured.some((allowed) => {
    const normalizedAllowed = allowed.startsWith("/") ? allowed : `/${allowed}`;
    return normalizedPathname.startsWith(normalizedAllowed);
  });
}

export function getEffectiveWebLevel(
  preferred: WebExtractLevel | undefined,
  policy: WebPolicy
): WebExtractLevel {
  if (!preferred) {
    return policy.level;
  }

  const rank: Record<WebExtractLevel, number> = {
    public: 1,
    auth: 2,
    browser: 3
  };

  return rank[preferred] > rank[policy.level] ? preferred : policy.level;
}

export function usageTagLevel(level: WebExtractLevel): "standard" | "signed_in" | "interactive" {
  if (level === "auth") {
    return "signed_in";
  }
  if (level === "browser") {
    return "interactive";
  }
  return "standard";
}

export function extractWebDocument(payload: {
  html: string;
  url: string;
  host: string;
  level: WebExtractLevel;
  fetchedAt?: string;
}): WebExtractResult {
  const readable = extractReadableDocument(payload.html);
  const title = readable.title ?? undefined;
  const text = readable.text;

  return {
    title,
    text,
    markdown: toMarkdown(title, text),
    meta: {
      url: payload.url,
      host: payload.host,
      level: payload.level,
      fetchedAt: payload.fetchedAt ?? new Date().toISOString()
    }
  };
}
