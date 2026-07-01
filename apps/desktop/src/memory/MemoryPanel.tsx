import { useEffect, useRef, useState } from "react";
import { Card } from "../components/Card";
import { Button } from "../components/Button";
import {
  recall, evolveStatus, setEvolveEnabled, evolveNow, ollamaStatus,
  getReasonerConfig, setReasonerConfig, enableCloudReasoner,
  type HitDto, type EvolveStatusDto, type OllamaStatusDto, type ReasonerConfigDto,
} from "../api/engine";
import { vaultSet, vaultHas } from "../vault";
import { ReasonerConfigPanel } from "./ReasonerConfigPanel";
import { CloudEgressBanner } from "./CloudEgressBanner";
import { searchBlurb, modeBlurb } from "./reasonerView";
import { toRow } from "./recallView";
import { formatEvolve } from "./evolveStatus";

/** How many recall hits to request per search. */
const RECALL_K = 10;
/** How often the evolve card refreshes its status while the tab is open. */
const POLL_MS = 5000;

export function MemoryPanel() {
  // Search state.
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<HitDto[]>([]);
  const [searched, setSearched] = useState(false);
  const [searching, setSearching] = useState(false);
  const [searchError, setSearchError] = useState<string | null>(null);

  // Evolve state.
  const [status, setStatus] = useState<EvolveStatusDto | null>(null);
  const [ollama, setOllama] = useState<OllamaStatusDto | null>(null);
  const [evolving, setEvolving] = useState(false);
  const [toggling, setToggling] = useState(false);
  const [evolveError, setEvolveError] = useState<string | null>(null);
  const [unavailable, setUnavailable] = useState(false);
  const [reasonerCfg, setReasonerCfg] = useState<ReasonerConfigDto | null>(null);

  const refreshStatus = async () => {
    try {
      const [s, o, r] = await Promise.all([evolveStatus(), ollamaStatus(), getReasonerConfig()]);
      setStatus(s);
      setOllama(o);
      setReasonerCfg(r);
      setUnavailable(false);
    } catch {
      setUnavailable(true);
    }
  };

  // Poll on mount and every POLL_MS; clear the timer on unmount.
  const refreshRef = useRef(refreshStatus);
  refreshRef.current = refreshStatus;
  useEffect(() => {
    void refreshRef.current();
    const id = setInterval(() => void refreshRef.current(), POLL_MS);
    return () => clearInterval(id);
  }, []);

  if (unavailable) {
    return (
      <Card>
        <h2 style={{ margin: 0 }}>Brain</h2>
        <p style={{ color: "var(--text-secondary)" }}>Couldn’t reach your agent’s memory.</p>
      </Card>
    );
  }

  const onSearch = async () => {
    const q = query.trim();
    if (!q) return;
    setSearching(true);
    setSearchError(null);
    try {
      setHits(await recall(q, RECALL_K));
      setSearched(true);
    } catch (e) {
      setSearchError(String(e));
    } finally {
      setSearching(false);
    }
  };

  const onToggleEvolve = async () => {
    if (!status) return;
    setToggling(true);
    setEvolveError(null);
    try {
      await setEvolveEnabled(!status.enabled);
      await refreshStatus();
    } catch (e) {
      setEvolveError(String(e));
    } finally {
      setToggling(false);
    }
  };

  const onEvolveNow = async () => {
    setEvolving(true);
    setEvolveError(null);
    try {
      await evolveNow();
      await refreshStatus();
    } catch (e) {
      setEvolveError(String(e));
    } finally {
      setEvolving(false);
    }
  };

  const ollamaReady = !!ollama?.reachable && !!ollama?.model_present;
  const isCloud = reasonerCfg?.mode === "cloud";
  // Evolve readiness is mode-aware (spec §2b): Local trusts the Ollama probe; Cloud trusts the
  // signed-consent gate (cfg.ready is false for Local by design). Gating on cfg.ready alone left
  // the default Local mode's Evolve controls permanently disabled.
  const reasonerReady = isCloud ? !!reasonerCfg?.ready : ollamaReady;

  return (
    <Card>
      <h2 style={{ margin: 0 }}>Brain</h2>
      <p style={{ color: "var(--text-secondary)", fontSize: 13 }}>
        {searchBlurb(reasonerCfg?.mode ?? "local", reasonerCfg?.provider ?? "anthropic")}
      </p>
      <CloudEgressBanner cfg={reasonerCfg} />

      <div style={{ display: "flex", gap: 8, margin: "12px 0" }}>
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => { if (e.key === "Enter") void onSearch(); }}
          placeholder="Search your memory…"
          style={{
            flex: 1, padding: "8px 12px", borderRadius: 6,
            fontFamily: "inherit", fontSize: 14,
          }}
        />
        <Button variant="primary" onClick={onSearch} disabled={searching || query.trim() === ""}>
          {searching ? "Searching…" : "Search"}
        </Button>
      </div>

      {searchError ? <p style={{ fontSize: 13, color: "var(--error)" }}>{searchError}</p> : null}

      {searched && hits.length === 0 && !searchError ? (
        <p style={{ color: "var(--text-secondary)", fontSize: 13 }}>Nothing found yet — try a different search.</p>
      ) : null}

      <ul style={{ listStyle: "none", padding: 0, margin: 0 }}>
        {hits.map((h) => {
          const row = toRow(h);
          return (
            <li key={row.id} style={{ padding: "10px 0", borderBottom: "1px solid var(--border-soft)" }}>
              <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 4 }}>
                <span style={{
                  fontSize: 11, fontWeight: 600, color: "var(--text-secondary)",
                  background: "var(--surface-soft)", borderRadius: 4, padding: "2px 6px",
                }}>
                  {row.kindLabel}
                </span>
                <span style={{ fontSize: 12, color: "var(--text-tertiary)" }}>
                  {row.sourcesLabel}{row.sourcesLabel ? " · " : ""}score {row.score}
                </span>
              </div>
              <div style={{ fontSize: 14, lineHeight: 1.4 }}>{row.text}</div>
            </li>
          );
        })}
      </ul>

      <div style={{ marginTop: 24, paddingTop: 16, borderTop: "1px solid var(--border-soft)" }}>
        <div style={{ fontWeight: 600, marginBottom: 4 }}>Evolve</div>
        <p style={{ color: "var(--text-secondary)", fontSize: 13 }}>
          {modeBlurb(reasonerCfg?.mode ?? "local", reasonerCfg?.provider ?? "anthropic")}
        </p>

        {reasonerCfg ? (
          <ReasonerConfigPanel
            cfg={reasonerCfg}
            onSetConfig={setReasonerConfig}
            onEnableCloud={enableCloudReasoner}
            onVaultSet={vaultSet}
            onVaultHas={vaultHas}
            onChanged={refreshStatus}
          />
        ) : null}

        <p style={{ fontSize: 13, margin: "8px 0" }}>
          {status ? formatEvolve(status) : "Loading…"}
        </p>

        {!isCloud && (
          <p style={{ fontSize: 13, color: "var(--text-secondary)" }}>
            Local model:{" "}
            {ollama == null
              ? "checking…"
              : ollamaReady
              ? `ready (${ollama.model_tag})`
              : ollama.reachable
              ? `Ollama running, but ${ollama.model_tag} not installed`
              : "Ollama not detected"}
          </p>
        )}

        {!isCloud && !ollamaReady && ollama != null ? (
          <p style={{ fontSize: 13, color: "var(--text-secondary)" }}>
            To enable, install Ollama and run <code>ollama pull qwen2.5:7b-instruct</code>.
          </p>
        ) : null}

        <div style={{ display: "flex", gap: 8, margin: "8px 0" }}>
          <Button variant="secondary" onClick={onToggleEvolve} disabled={!status || toggling || !reasonerReady}>
            {status?.enabled ? "Turn Evolve Off" : "Turn Evolve On"}
          </Button>
          <Button variant="primary" onClick={onEvolveNow} disabled={evolving || !reasonerReady || !status?.enabled}>
            {evolving ? "Evolving…" : "Evolve Now"}
          </Button>
        </div>

        {evolveError ? <p style={{ fontSize: 13, color: "var(--error)" }}>{evolveError}</p> : null}
      </div>
    </Card>
  );
}
