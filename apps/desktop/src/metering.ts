import { loadJson, saveJson } from "./localStore";
import type { UsageEvent } from "./models";

type PricePoint = {
  promptPer1k: number;
  completionPer1k: number;
};

const DEFAULT_PRICE_POINT: PricePoint = {
  promptPer1k: 0.01,
  completionPer1k: 0.03
};

const PRICE_TABLE: Record<string, PricePoint> = {
  "openai:gpt-4.1": { promptPer1k: 0.01, completionPer1k: 0.03 },
  "openai:gpt-4.1-mini": { promptPer1k: 0.0005, completionPer1k: 0.0015 },
  "anthropic:claude-3-5-sonnet": { promptPer1k: 0.003, completionPer1k: 0.015 },
  "google:gemini-1.5-pro": { promptPer1k: 0.00125, completionPer1k: 0.005 },
  "google_gemini:default": { promptPer1k: 0, completionPer1k: 0 },
  "anthropic_claude:default": { promptPer1k: 0, completionPer1k: 0 },
  "brave:search": { promptPer1k: 0, completionPer1k: 0 },
  "tavily:search": { promptPer1k: 0, completionPer1k: 0 },
  "air-agent:default": { promptPer1k: 0, completionPer1k: 0 }
};

function normalizeModel(model: string | null): string {
  if (!model || model.trim().length === 0) {
    return "default";
  }
  return model.trim().toLowerCase();
}

function roundUsd(value: number): number {
  return Math.round(value * 1_000_000) / 1_000_000;
}

export function estimateCost(
  provider: string,
  model: string | null,
  promptTokens: number | null,
  completionTokens: number | null
): number | null {
  const hasPrompt = typeof promptTokens === "number" && Number.isFinite(promptTokens);
  const hasCompletion =
    typeof completionTokens === "number" && Number.isFinite(completionTokens);

  if (!hasPrompt && !hasCompletion) {
    return null;
  }

  const normalizedProvider = provider.trim().toLowerCase();
  const normalizedModel = normalizeModel(model);
  const rates =
    PRICE_TABLE[`${normalizedProvider}:${normalizedModel}`] ??
    PRICE_TABLE[`${normalizedProvider}:default`] ??
    DEFAULT_PRICE_POINT;

  const promptCost = ((hasPrompt ? (promptTokens as number) : 0) / 1000) * rates.promptPer1k;
  const completionCost =
    ((hasCompletion ? (completionTokens as number) : 0) / 1000) * rates.completionPer1k;

  return roundUsd(promptCost + completionCost);
}

export async function recordUsage(event: UsageEvent): Promise<void> {
  const existingEvents = await loadJson<UsageEvent[]>("usage_events.json", []);
  await saveJson("usage_events.json", [event, ...existingEvents]);
}
