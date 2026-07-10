import { useEffect, useState } from "react";
import { Button } from "../components/Button";
import {
  integrationsStatus, connectClaudeCode, disconnectClaudeCode,
  type ClaudeCodeStatus,
} from "../api/integrations";

export function IntegrationsPanel() {
  const [status, setStatus] = useState<ClaudeCodeStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = async () => {
    try {
      setStatus((await integrationsStatus()).claude_code);
    } catch (e) {
      setError(String(e));
    }
  };
  useEffect(() => { void refresh(); }, []);

  const run = async (fn: () => Promise<{ claude_code: ClaudeCodeStatus }>) => {
    setBusy(true);
    setError(null);
    try {
      setStatus((await fn()).claude_code);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const notFound = status === "not_found";
  const connected = status === "connected";

  return (
    <div style={{ marginTop: 24, paddingTop: 16, borderTop: "1px solid var(--border-soft)" }}>
      <div style={{ fontWeight: 600, marginBottom: 4 }}>Integrations</div>
      <p style={{ color: "var(--text-secondary)", fontSize: 13 }}>
        Connect your coding tools to your agent’s memory. Claude Code will be able to recall your
        notes and remember new ones — in every project. For best results, quit Claude Code before
        connecting (a running session may overwrite the change).
      </p>

      <div style={{ display: "flex", alignItems: "center", gap: 8, margin: "8px 0" }}>
        <span style={{ fontSize: 13 }}>Claude Code</span>
        {connected ? (
          <Button variant="secondary" disabled={busy} onClick={() => void run(disconnectClaudeCode)}>
            {busy ? "Working…" : "Disconnect"}
          </Button>
        ) : (
          <Button
            variant="primary"
            disabled={busy || notFound}
            onClick={() => void run(connectClaudeCode)}
          >
            {busy ? "Connecting…" : "Connect Claude Code"}
          </Button>
        )}
      </div>

      {connected ? (
        <p style={{ fontSize: 12, color: "var(--text-tertiary)" }}>
          Connected. Takes effect the next time you start Claude Code. Disconnect here before moving
          or uninstalling the app so it can clean up the config.
        </p>
      ) : null}
      {notFound ? (
        <p style={{ fontSize: 13, color: "var(--text-secondary)" }}>
          Claude Code isn’t installed yet — install Claude Code, then reopen this page.
        </p>
      ) : null}
      {error ? <p style={{ fontSize: 13, color: "var(--error)" }}>{error}</p> : null}
    </div>
  );
}
