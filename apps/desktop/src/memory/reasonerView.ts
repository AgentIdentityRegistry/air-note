import type { ReasonerConfigDto, ReasonerConfigInput, ReasonerMode, CloudProvider } from "../api/engine";
import type { ProviderVaultKey } from "../vault";

export const providerLabel = (p: CloudProvider): string =>
  p === "anthropic" ? "Anthropic" : "OpenAI-compatible";

export const defaultModelFor = (p: CloudProvider): string =>
  p === "anthropic" ? "claude-sonnet-4-6" : "gpt-5-mini";

export const vaultKeyFor = (p: CloudProvider): ProviderVaultKey =>
  p === "anthropic" ? "anthropic_api_key" : "openai_compat_api_key";

/** Cloud is actively egressing only when the saved mode is cloud AND the backend reports ready. */
export const cloudActive = (cfg: ReasonerConfigDto): boolean => cfg.mode === "cloud" && cfg.ready;

export const bannerText = (cfg: ReasonerConfigDto): string =>
  `Brain model: Cloud · ${providerLabel(cfg.provider)} — context leaves this device`;

/** Egress disclosure for the banner: how many file-derived snippets the last CLOUD tick sent
 *  off-box. `null` (no cloud tick yet this session) omits the line so the banner never guesses. */
export const taintedNotice = (count: number | null): string | null => {
  if (count === null) return null;
  if (count === 0) return "Last sync included no content from your ingested files.";
  return `Last sync included ${count} snippet${count === 1 ? "" : "s"} from your ingested files.`;
};

export const modeBlurb = (mode: ReasonerMode, provider: CloudProvider): string =>
  mode === "cloud"
    ? `Cloud mode sends your brain's working context — built from your memories and ingested files — to ${providerLabel(provider)}. Your memory leaves this device.`
    : "A local model can organize memories into dossiers in the background. Off by default; runs only on your machine.";

/** Search/recall is always local; only evolve egresses. Keep that distinction honest. */
export const searchBlurb = (mode: ReasonerMode, provider: CloudProvider): string =>
  mode === "cloud"
    ? `Search everything the agent has read and learned. Search stays on your machine; in Cloud mode, evolve sends context to ${providerLabel(provider)}.`
    : "Search everything the agent has read and learned. Everything stays on your machine.";

/** Blunt, no-euphemism consent body (spec R4). */
export const consentBody = (provider: CloudProvider): string =>
  `Cloud mode sends your brain's working context to ${providerLabel(provider)} on every evolve tick. ` +
  `This can include the full text of files you've ingested — including any passwords, keys, or personal data inside them. ` +
  `Your memory leaves this device. You can switch back to Local at any time.`;

/** Build the snake_case write payload (base_url only for openai-compat with a non-empty value). */
export const buildConfigInput = (form: {
  mode: ReasonerMode; provider: CloudProvider; model: string; baseUrl: string;
}): ReasonerConfigInput => ({
  mode: form.mode,
  provider: form.provider,
  model: form.model.trim(),
  base_url: form.provider === "openai-compat" && form.baseUrl.trim() !== "" ? form.baseUrl.trim() : null,
});
