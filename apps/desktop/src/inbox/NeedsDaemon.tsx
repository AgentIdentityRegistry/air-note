import { Card } from "../components/Card";

export function NeedsDaemon() {
  return (
    <Card>
      <h2 style={{ margin: 0 }}>Connect AIR Note</h2>
      <p style={{ marginTop: 12, color: "#666", lineHeight: 1.5 }}>
        No local AIR Note agent found. Install the CLI and start the daemon, then reopen this tab:
      </p>
      <pre style={{ background: "#F3F4F6", padding: 12, borderRadius: 8, fontSize: 12, overflowX: "auto" }}>
        air-msg daemon install
      </pre>
    </Card>
  );
}
