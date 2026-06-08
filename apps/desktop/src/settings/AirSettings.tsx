import { Card } from "../components/Card";
import { Button } from "../components/Button";
import { resetIdentity } from "../api/tauri";
import { useIdentity } from "../state/identity";

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

      <p style={{ marginTop: 16, color: "#666", lineHeight: 1.5 }}>
        AIR endpoint is configured via the <code>BOSSCLAW_USE_REAL_AIR</code>{" "}
        environment variable at launch. (Settings UI for this comes in v1.1.)
      </p>

      <div style={{ marginTop: 24, paddingTop: 16, borderTop: "1px solid #eee" }}>
        <div style={{ fontWeight: 600, marginBottom: 4 }}>Danger zone</div>
        <p style={{ color: "#666", fontSize: 13 }}>
          Reset will delete your agent's identity and require re-onboarding.
        </p>
        <Button variant="secondary" onClick={handleReset} style={{ color: "#b00" }}>
          Reset agent
        </Button>
      </div>
    </Card>
  );
}
