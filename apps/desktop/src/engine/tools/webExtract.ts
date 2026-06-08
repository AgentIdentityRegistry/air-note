import type { ConfigChangeProposal, WebPolicy } from "../../models";
import { isPolicyPathAllowed } from "../../webExtract";
import { extractReadableDocument } from "../web/readability";

type WebFetchPublicResponse = {
  finalUrl: string;
  status: number;
  contentType: string | null;
  html: string;
};

type WebExtractUsageInput = {
  agentId: string;
  url: string;
  host: string;
  inputChars: number;
  outputChars: number;
  latencyMs: number;
  bytes: number;
};

export type WebExtractToolResult =
  | {
      ok: true;
      title?: string;
      text: string;
      meta: {
        url: string;
        host: string;
        level: "public";
        fetchedAt: string;
        status: number;
        contentType: string | null;
        bytes: number;
      };
      usage: {
        inputChars: number;
        outputChars: number;
        latencyMs: number;
        bytes: number;
      };
    }
  | {
      ok: false;
      reason: "requires_approval";
      host: string;
      message: string;
      proposal: ConfigChangeProposal;
    }
  | {
      ok: false;
      reason: "invalid_url" | "path_blocked" | "fetch_failed";
      message: string;
      host?: string;
    };

type WebExtractDependencies = {
  policyByHost: Map<string, WebPolicy>;
  buildApprovalProposal: (host: string) => ConfigChangeProposal;
  invokeFetchPublic: (url: string) => Promise<WebFetchPublicResponse>;
  onUsage?: (input: WebExtractUsageInput) => Promise<void> | void;
};

export async function webExtract(
  url: string,
  agentId: string,
  deps: WebExtractDependencies
): Promise<WebExtractToolResult> {
  let parsedUrl: URL;
  try {
    parsedUrl = new URL(url.trim());
  } catch {
    return {
      ok: false,
      reason: "invalid_url",
      message: "Enter a valid URL."
    };
  }

  if (parsedUrl.protocol !== "http:" && parsedUrl.protocol !== "https:") {
    return {
      ok: false,
      reason: "invalid_url",
      message: "Only http and https URLs are supported."
    };
  }

  const host = parsedUrl.host.toLowerCase();
  const pathname = parsedUrl.pathname || "/";
  const policy = deps.policyByHost.get(host);
  if (!policy) {
    return {
      ok: false,
      reason: "requires_approval",
      host,
      message: `Web access to ${host} is not approved yet.`,
      proposal: deps.buildApprovalProposal(host)
    };
  }

  if (!isPolicyPathAllowed(policy, pathname)) {
    return {
      ok: false,
      reason: "path_blocked",
      host,
      message: `Path ${pathname} is outside approved scope for ${host}.`
    };
  }

  const startedAt = performance.now();
  try {
    const response = await deps.invokeFetchPublic(parsedUrl.toString());
    const { title, text } = extractReadableDocument(response.html);
    const extractedText = text.trim();
    if (!extractedText) {
      return {
        ok: false,
        reason: "fetch_failed",
        host,
        message: "No readable text was found at this URL."
      };
    }

    const bytes = new TextEncoder().encode(response.html).length;
    const latencyMs = Math.max(0, Math.round(performance.now() - startedAt));
    await deps.onUsage?.({
      agentId,
      url: parsedUrl.toString(),
      host,
      inputChars: parsedUrl.toString().length,
      outputChars: extractedText.length,
      latencyMs,
      bytes
    });

    return {
      ok: true,
      title: title ?? undefined,
      text: extractedText,
      meta: {
        url: response.finalUrl || parsedUrl.toString(),
        host,
        level: "public",
        fetchedAt: new Date().toISOString(),
        status: response.status,
        contentType: response.contentType,
        bytes
      },
      usage: {
        inputChars: parsedUrl.toString().length,
        outputChars: extractedText.length,
        latencyMs,
        bytes
      }
    };
  } catch {
    return {
      ok: false,
      reason: "fetch_failed",
      host,
      message: "Unable to fetch this URL."
    };
  }
}
