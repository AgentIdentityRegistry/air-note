import { useEffect, useState, type CSSProperties } from "react";
import { Button } from "../components/Button";
import {
  integrationsStatus, connectClaudeCode, disconnectClaudeCode,
  backfillCount as fetchBackfillCount, setCaptureEnabled, captureEnabled, isConnected,
  type ClaudeCodeStatus,
} from "../api/integrations";

/** Shared style for the inline checkbox labels (the consent box + the capture toggle). */
const checkboxLabelStyle: CSSProperties = {
  display: "flex", alignItems: "center", gap: 8, fontSize: 13, cursor: "pointer",
};

/**
 * Settings ▸ Integrations — the SP3 consent model for the Claude Code memory link. Four states off
 * the detect enum (spec §6): NotFound (install hint + disabled action), NotConnected (the consent
 * Connect flow), Connected{capture:true} (a full SP3 link — confirmation + the forward-only capture
 * toggle), and Connected{capture:false} (an SP2-era install — "Re-connect to enable session capture"
 * + the Connect flow again). Prop-less; mounted as <IntegrationsPanel/> in AirSettings. Tokens only
 * (no hardcoded colors) per the shell-redesign gate; Tauri rejects Result<_,String> with a BARE
 * STRING, so every catch is String(e), never `.message`.
 *
 * Two distinct "capture" signals, never conflated: `Connected { capture }` is HOOK PRESENCE in the
 * config (did Connect wire the SessionEnd hook?), while `captureEnabled()` is the engine's LIVE
 * on/off flag (the toggle POSITION). They can legitimately differ — hook present but flag off is the
 * normal toggled-off state.
 */
export function IntegrationsPanel() {
  const [status, setStatus] = useState<ClaudeCodeStatus | null>(null);
  // Best-effort disclosure count (~30 days of transcripts). A read failure zeroes it, never blanks
  // the panel — the consent copy stays honest with "0 found".
  const [backfillCount, setBackfillCount] = useState<number>(0);
  // The engine's live capture flag = the toggle position. Fail-closed to false (matches the Rust).
  const [capture, setCapture] = useState(false);
  // The Connect-flow consent checkbox — pre-checked (opt-out: one gesture connects AND captures).
  const [consentChecked, setConsentChecked] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refreshStatus = async () => {
    try {
      setStatus((await integrationsStatus()).claude_code);
    } catch (e) {
      setError(String(e));
    }
  };
  // Secondary fetches degrade QUIETLY — neither may blank the panel (spec §6): a backfill failure
  // zeroes N, a capture-flag failure shows the safe "off".
  const refreshCapture = async () => {
    try {
      setCapture(await captureEnabled());
    } catch {
      setCapture(false);
    }
  };
  const refreshBackfillCount = async () => {
    try {
      setBackfillCount(await fetchBackfillCount());
    } catch {
      setBackfillCount(0);
    }
  };

  useEffect(() => {
    void refreshStatus();
    void refreshCapture();
    void refreshBackfillCount();
  }, []);

  // Run a status-returning command: reflect the returned DTO, then re-read the live capture flag (a
  // connect/disconnect/toggle each move it). A failure surfaces on the error line, never crashes.
  const runStatusCmd = async (fn: () => Promise<{ claude_code: ClaudeCodeStatus }>) => {
    setBusy(true);
    setError(null);
    try {
      setStatus((await fn()).claude_code);
      await refreshCapture();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const onConnect = () => runStatusCmd(() => connectClaudeCode(consentChecked));
  const onDisconnect = () => runStatusCmd(disconnectClaudeCode);
  const onToggleCapture = () => runStatusCmd(() => setCaptureEnabled(!capture));

  const notFound = status === "not_found";
  const connectedWithCapture = status !== null && isConnected(status) && status.connected.capture;
  const needsReconnect = status !== null && isConnected(status) && !status.connected.capture;
  const showConnectFlow = status === "not_connected" || needsReconnect;

  return (
    <div style={{ marginTop: 24, paddingTop: 16, borderTop: "1px solid var(--border-soft)" }}>
      <div style={{ fontWeight: 600, marginBottom: 4 }}>Integrations</div>
      <p style={{ color: "var(--text-secondary)", fontSize: 13 }}>
        Connect your coding tools to your agent’s memory. Claude Code will be able to recall your
        notes and remember new ones — in every project. For best results, quit Claude Code before
        connecting (a running session may overwrite the change).
      </p>

      {/* State 4 (SP2-era install): the re-connect prompt sits ABOVE the Connect flow. */}
      {needsReconnect ? (
        <p style={{ fontSize: 13, color: "var(--text-secondary)", margin: "8px 0 0" }}>
          Re-connect to enable session capture.
        </p>
      ) : null}

      {/* States 2 & 4: the consent Connect flow. The disclosure + checkbox live in their OWN block
          (L11: visually separable from the Connect button, which sits below the block). */}
      {showConnectFlow ? (
        <div style={{ margin: "12px 0" }}>
          <div style={{ padding: 12, borderRadius: 8, border: "1px solid var(--border-soft)", marginBottom: 12 }}>
            <p style={{ fontSize: 13, color: "var(--text-secondary)", lineHeight: 1.5, margin: "0 0 8px" }}>
              Keep a local, private copy of your Claude Code sessions in AIR memory — including your
              recent sessions (~30 days, {backfillCount} found). They’re processed on this Mac and stored
              unencrypted on this Mac — readable Markdown you own, so protect the disk with FileVault.
            </p>
            <label style={checkboxLabelStyle}>
              <input
                type="checkbox"
                checked={consentChecked}
                onChange={(e) => setConsentChecked(e.target.checked)}
              />
              <span>Keep a local copy of my Claude Code sessions</span>
            </label>
          </div>
          <Button variant="primary" disabled={busy} onClick={onConnect}>
            {busy ? "Connecting…" : needsReconnect ? "Re-connect Claude Code" : "Connect Claude Code"}
          </Button>
        </div>
      ) : null}

      {/* State 3 (full SP3 link): confirmation + the forward-only capture toggle. */}
      {connectedWithCapture ? (
        <div style={{ margin: "12px 0" }}>
          <p style={{ fontSize: 12, color: "var(--text-tertiary)", margin: "0 0 12px" }}>
            Connected. Takes effect the next time you start Claude Code. Disconnect here before moving
            or uninstalling the app so it can clean up the config.
          </p>
          <label style={{ display: "flex", alignItems: "center", gap: 8, fontSize: 13, cursor: "pointer" }}>
            <input type="checkbox" checked={capture} disabled={busy} onChange={onToggleCapture} />
            <span>Capture new Claude Code sessions</span>
          </label>
          <p style={{ fontSize: 12, color: "var(--text-tertiary)", margin: "4px 0 0" }}>
            This applies going forward — history import is only offered at Connect.
          </p>
        </div>
      ) : null}

      {/* Disconnect is available in BOTH connected states (full link and SP2-era). */}
      {connectedWithCapture || needsReconnect ? (
        <div style={{ margin: "12px 0 0" }}>
          <Button variant="secondary" disabled={busy} onClick={onDisconnect}>
            {busy ? "Working…" : "Disconnect"}
          </Button>
        </div>
      ) : null}

      {/* State 1: not installed — the install hint + a disabled Connect action. */}
      {notFound ? (
        <div style={{ margin: "12px 0" }}>
          <p style={{ fontSize: 13, color: "var(--text-secondary)", margin: "0 0 8px" }}>
            Claude Code isn’t installed yet — install Claude Code, then reopen this page.
          </p>
          <Button variant="primary" disabled>
            Connect Claude Code
          </Button>
        </div>
      ) : null}

      {error ? <p style={{ fontSize: 13, color: "var(--error)" }}>{error}</p> : null}
    </div>
  );
}
