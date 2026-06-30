import { useState } from "react";
import { Card } from "../components/Card";
import { Button } from "../components/Button";
import { consentBody, providerLabel } from "./reasonerView";
import type { CloudProvider } from "../api/engine";

/**
 * One-time, blunt consent gate before cloud egress is enabled. `onConfirm` performs
 * the test-key probe + signs the consent record (engine_enable_cloud_reasoner). On
 * failure it surfaces the already-classified error and does NOT enable / close.
 */
export function CloudConsentModal({
  provider, onConfirm, onCancel,
}: {
  provider: CloudProvider;
  onConfirm: () => Promise<void>;
  onCancel: () => void;
}) {
  const [acknowledged, setAcknowledged] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const onEnable = async () => {
    setBusy(true);
    setError(null);
    try {
      await onConfirm();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Card>
      <div style={{ fontWeight: 600, color: "var(--error)" }}>
        Enable Cloud Reasoner — your memory leaves this device
      </div>
      <p style={{ fontSize: 13, color: "var(--text-secondary)" }}>{consentBody(provider)}</p>
      <label style={{ display: "flex", gap: 6, alignItems: "center", fontSize: 13 }}>
        <input type="checkbox" checked={acknowledged} onChange={(e) => setAcknowledged(e.target.checked)} />
        I understand my memory will be sent to {providerLabel(provider)}
      </label>
      <div style={{ display: "flex", gap: 8, marginTop: 8 }}>
        <Button variant="primary" disabled={!acknowledged || busy} onClick={onEnable}>
          {busy ? "Enabling…" : "Enable Cloud Reasoner"}
        </Button>
        <Button variant="secondary" disabled={busy} onClick={onCancel}>Cancel</Button>
      </div>
      {error ? <p style={{ fontSize: 13, color: "var(--error)" }}>{error}</p> : null}
    </Card>
  );
}
