import { useCallback, useEffect, useState } from "react";
import { reflectEnabled, setReflectEnabled } from "../api/integrations";

/** Rung-4 R4-A (§2.5): the single reflection toggle. Deliberately NOT inside the Claude-Code-connected
 * gate — reflection is a brain-local loop, unrelated to connection state. Fails closed: any read/write
 * error renders the toggle OFF (the daemon's flag is the sole truth; the sweeper self-gates). */
export default function ReflectPanel() {
  const [enabled, setEnabled] = useState(false);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    try {
      setEnabled(await reflectEnabled());
    } catch {
      setEnabled(false); // fail-closed display: an unreadable flag shows OFF
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const onToggle = useCallback(async () => {
    const next = !enabled;
    setBusy(true);
    try {
      await setReflectEnabled(next);
      setEnabled(next);
    } catch {
      await refresh(); // write failed → re-read the daemon's truth, never assume
    } finally {
      setBusy(false);
    }
  }, [enabled, refresh]);

  return (
    <section aria-label="Reflection">
      <h3 style={{ margin: "0 0 4px" }}>Reflection</h3>
      <label style={{ display: "flex", alignItems: "center", gap: 8 }}>
        <input
          type="checkbox"
          role="checkbox"
          aria-label="Reflect on recently-missed topics"
          checked={enabled}
          disabled={busy}
          onChange={() => void onToggle()}
        />
        <span>Reflect on recently-missed topics</span>
      </label>
      <p style={{ color: "var(--text-tertiary)", fontSize: 12, margin: "4px 0 0" }}>
        When your machine is idle, AIR quietly refreshes dossiers for topics you recently searched and
        couldn’t find. Uses your configured reasoner; with a cloud reasoner enabled, gathered material may
        be sent under your existing cloud consent.
      </p>
    </section>
  );
}
