import { Card } from "../components/Card";
import { Button } from "../components/Button";
import { useIdentity } from "../state/identity";

export function IdentityPanel() {
  const { identity, trustScore, loading, refresh } = useIdentity();

  if (loading) return <Card>Loading...</Card>;
  if (!identity) return <Card>No identity yet.</Card>;

  return (
    <Card>
      <h2 style={{ margin: 0 }}>Your agent</h2>

      <div style={{ marginTop: 16 }}>
        <div style={{ fontSize: 13, color: "#666" }}>Name</div>
        <div style={{ fontSize: 16 }}>{identity.name}</div>
      </div>

      <div style={{ marginTop: 12 }}>
        <div style={{ fontSize: 13, color: "#666" }}>DID</div>
        <div
          style={{
            fontSize: 12,
            fontFamily: "monospace",
            wordBreak: "break-all",
          }}
        >
          {identity.did}
        </div>
      </div>

      <div style={{ marginTop: 12 }}>
        <div style={{ fontSize: 13, color: "#666" }}>Trust score</div>
        <div style={{ fontSize: 16 }}>{trustScore ?? "—"}</div>
      </div>

      <div style={{ marginTop: 16 }}>
        <Button variant="secondary" onClick={refresh}>
          Refresh
        </Button>
      </div>
    </Card>
  );
}
