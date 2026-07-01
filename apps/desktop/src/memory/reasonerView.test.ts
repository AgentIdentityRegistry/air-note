import { describe, it, expect } from "vitest";
import {
  providerLabel, defaultModelFor, vaultKeyFor, cloudActive,
  bannerText, modeBlurb, searchBlurb, consentBody, buildConfigInput, taintedNotice,
} from "./reasonerView";
import type { ReasonerConfigDto } from "../api/engine";

const cloudReady: ReasonerConfigDto =
  { mode: "cloud", provider: "anthropic", model: "claude-sonnet-4-6", base_url: null, ready: true };

describe("reasonerView", () => {
  it("labels providers", () => {
    expect(providerLabel("anthropic")).toBe("Anthropic");
    expect(providerLabel("openai-compat")).toBe("OpenAI-compatible");
    expect(providerLabel("gemini")).toBe("Gemini");
  });
  it("supplies a default model per provider", () => {
    expect(defaultModelFor("anthropic")).toBe("claude-sonnet-4-6");
    expect(defaultModelFor("openai-compat")).toBe("gpt-5-mini");
    expect(defaultModelFor("gemini")).toBe("gemini-2.5-flash");
  });
  it("maps a provider to its vault key", () => {
    expect(vaultKeyFor("anthropic")).toBe("anthropic_api_key");
    expect(vaultKeyFor("openai-compat")).toBe("openai_compat_api_key");
    expect(vaultKeyFor("gemini")).toBe("google_api_key");
  });
  it("cloudActive only when mode is cloud AND ready", () => {
    expect(cloudActive(cloudReady)).toBe(true);
    expect(cloudActive({ ...cloudReady, ready: false })).toBe(false);
    expect(cloudActive({ ...cloudReady, mode: "local" })).toBe(false);
  });
  it("banner names the provider and warns about egress", () => {
    expect(bannerText(cloudReady)).toBe("Brain model: Cloud · Anthropic — context leaves this device");
  });
  it("mode-aware blurbs", () => {
    expect(modeBlurb("local", "anthropic")).toContain("runs only on your machine");
    expect(modeBlurb("cloud", "anthropic")).toContain("leaves this device");
    expect(searchBlurb("local", "anthropic")).toContain("Everything stays on your machine");
    expect(searchBlurb("cloud", "openai-compat")).toContain("OpenAI-compatible");
  });
  it("consent body is blunt about file contents", () => {
    const body = consentBody("anthropic");
    expect(body).toContain("passwords, keys, or personal data");
    expect(body).toContain("Anthropic");
  });
  it("buildConfigInput emits snake_case base_url, null unless openai-compat with a value", () => {
    expect(buildConfigInput({ mode: "cloud", provider: "anthropic", model: " m ", baseUrl: "x" }))
      .toEqual({ mode: "cloud", provider: "anthropic", model: "m", base_url: null });
    expect(buildConfigInput({ mode: "cloud", provider: "openai-compat", model: "m", baseUrl: " https://h " }))
      .toEqual({ mode: "cloud", provider: "openai-compat", model: "m", base_url: "https://h" });
  });
  it("taintedNotice discloses the file-derived egress count, omitting until a cloud tick runs", () => {
    expect(taintedNotice(null)).toBeNull();
    expect(taintedNotice(0)).toBe("Last sync included no content from your ingested files.");
    expect(taintedNotice(1)).toBe("Last sync included 1 snippet from your ingested files.");
    expect(taintedNotice(3)).toBe("Last sync included 3 snippets from your ingested files.");
  });
});
