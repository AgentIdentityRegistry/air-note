import { useEffect, useState } from "react";
import { Button } from "../components/Button";
import { CloudConsentModal } from "./CloudConsentModal";
import { buildConfigInput, defaultModelFor, vaultKeyFor, providerLabel } from "./reasonerView";
import type { ReasonerConfigDto, ReasonerConfigInput, CloudProvider } from "../api/engine";
import type { ProviderVaultKey } from "../vault";

type Props = {
  cfg: ReasonerConfigDto;
  onSetConfig: (input: ReasonerConfigInput) => Promise<void>;
  onEnableCloud: (input: ReasonerConfigInput) => Promise<void>;
  onVaultSet: (key: ProviderVaultKey, value: string) => Promise<void>;
  onVaultHas: (key: ProviderVaultKey) => Promise<boolean>;
  onChanged: () => Promise<void>; // awaited so the banner/gate reflect the new cfg immediately
};

export function ReasonerConfigPanel(props: Props) {
  const { cfg, onSetConfig, onEnableCloud, onVaultSet, onVaultHas, onChanged } = props;

  const [selectedMode, setSelectedMode] = useState<"local" | "cloud">(cfg.mode);
  const [provider, setProvider] = useState<CloudProvider>(cfg.provider);
  const [model, setModel] = useState(cfg.model || defaultModelFor(cfg.provider));
  const [baseUrl, setBaseUrl] = useState(cfg.base_url ?? "");
  const [keyInput, setKeyInput] = useState("");
  const [keySaved, setKeySaved] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showConsent, setShowConsent] = useState(false);

  // Reflect whether a key is stored for the CURRENT provider (re-runs on provider change).
  useEffect(() => {
    let alive = true;
    void onVaultHas(vaultKeyFor(provider)).then((has) => { if (alive) setKeySaved(has); });
    return () => { alive = false; };
  }, [provider, onVaultHas]);

  const onSelectLocal = async () => {
    setSelectedMode("local");
    if (cfg.mode === "cloud") {
      setBusy(true); setError(null);
      try {
        await onSetConfig({ mode: "local", provider, model: model.trim(), base_url: null });
        await onChanged();
      } catch (e) { setError(String(e)); } finally { setBusy(false); }
    }
  };

  const onChangeProvider = (p: CloudProvider) => {
    setProvider(p);
    setModel(defaultModelFor(p)); // note: overwrites a hand-typed model on provider switch (acceptable)
  };

  const onSaveKey = async () => {
    setBusy(true); setError(null);
    try {
      await onVaultSet(vaultKeyFor(provider), keyInput);
      setKeyInput("");
      setKeySaved(await onVaultHas(vaultKeyFor(provider)));
    } catch (e) { setError(String(e)); } finally { setBusy(false); }
  };

  const formInput = (): ReasonerConfigInput =>
    buildConfigInput({ mode: "cloud", provider, model, baseUrl });

  // Client gate is COSMETIC. The real SSRF/HTTPS enforcement is the backend
  // validate_reasoner_config + the connect-time pinned resolver, which also surface
  // the precise rejection through the consent modal's error.
  const canEnable = keySaved && model.trim() !== "" &&
    (provider !== "openai-compat" || baseUrl.trim() !== "");

  // Low-2: if cloud is enabled and the form diverges from the CONSENTED provider/host,
  // the backend will fail-close (consent binding mismatch). Tell the user to re-consent.
  const consentedBaseUrl = cfg.base_url ?? null;
  const formBaseUrl = formInput().base_url;
  const needsReconsent = cfg.mode === "cloud" && cfg.ready &&
    (provider !== cfg.provider || formBaseUrl !== consentedBaseUrl);

  const onConfirmConsent = async () => {
    await onEnableCloud(formInput());
    await onChanged();          // settle cfg.ready BEFORE closing so the banner shows immediately
    setShowConsent(false);
  };

  return (
    <div style={{ marginTop: 12 }}>
      <div style={{ display: "flex", gap: 8 }}>
        <Button variant={selectedMode === "local" ? "primary" : "secondary"} disabled={busy} onClick={onSelectLocal}>
          Local
        </Button>
        <Button variant={selectedMode === "cloud" ? "primary" : "secondary"} disabled={busy} onClick={() => setSelectedMode("cloud")}>
          Cloud
        </Button>
      </div>

      {selectedMode === "cloud" ? (
        <div style={{ marginTop: 8, display: "flex", flexDirection: "column", gap: 8 }}>
          <label style={{ fontSize: 13 }}>
            Provider
            <select value={provider} onChange={(e) => onChangeProvider(e.target.value as CloudProvider)} style={{ marginLeft: 8 }}>
              <option value="anthropic">Anthropic</option>
              <option value="openai-compat">OpenAI-compatible</option>
            </select>
          </label>

          <label style={{ fontSize: 13 }}>
            Model
            <input value={model} onChange={(e) => setModel(e.target.value)} style={{ marginLeft: 8 }} />
          </label>

          {provider === "openai-compat" ? (
            <label style={{ fontSize: 13 }}>
              Base URL (HTTPS)
              <input value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)} placeholder="https://…" style={{ marginLeft: 8 }} />
            </label>
          ) : null}

          <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
            <label style={{ fontSize: 13 }}>
              API key
              <input type="password" value={keyInput} onChange={(e) => setKeyInput(e.target.value)} style={{ marginLeft: 8 }} />
            </label>
            <Button variant="secondary" disabled={busy || keyInput.trim() === ""} onClick={onSaveKey}>Save key</Button>
            {keySaved ? <span style={{ fontSize: 13, color: "var(--text-secondary)" }}>key saved ✓</span> : null}
          </div>

          {needsReconsent ? (
            <p style={{ fontSize: 13, color: "var(--error)" }}>
              You changed the provider or base URL — click Enable Cloud to re-consent before cloud resumes.
            </p>
          ) : null}

          <div>
            <Button variant="primary" disabled={busy || !canEnable} onClick={() => setShowConsent(true)}>
              Enable Cloud ({providerLabel(provider)})
            </Button>
          </div>

          {error ? <p style={{ fontSize: 13, color: "var(--error)" }}>{error}</p> : null}
        </div>
      ) : null}

      {showConsent ? (
        <CloudConsentModal provider={provider} onConfirm={onConfirmConsent} onCancel={() => setShowConsent(false)} />
      ) : null}
    </div>
  );
}
