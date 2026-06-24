import { Card } from "../components/Card";
import { Button } from "../components/Button";
import { resetIdentity } from "../api/tauri";
import { useIdentity } from "../state/identity";
import { SourcesPanel } from "../sources/SourcesPanel";

export function AirSettings() {
  const { refresh } = useIdentity();

  const handleReset = async () => {
    if (!confirm("Delete this agent identity? This cannot be undone.")) return;
    await resetIdentity();
    await refresh();
  };

  return (
    <Card>
      <h2 style={{ margin: 0 }}>Settings</h2>

      <SourcesPanel />

      <div style={{ marginTop: 24, paddingTop: 16, borderTop: "1px solid var(--border-soft)" }}>
        <div style={{ fontWeight: 600, marginBottom: 4 }}>Danger Zone</div>
        <p style={{ color: "var(--text-secondary)", fontSize: 13 }}>
          Reset will delete your agent&apos;s identity and require re-onboarding.
        </p>
        <Button variant="secondary" className="danger-btn" onClick={handleReset}>
          Reset agent
        </Button>
      </div>
    </Card>
  );
}
