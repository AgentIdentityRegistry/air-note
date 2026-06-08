import { Button } from "../components/Button";
import { Card } from "../components/Card";
import { useIdentity } from "../state/identity";

export function Done({ onFinish }: { onFinish: () => void }) {
  const { identity, trustScore } = useIdentity();

  if (!identity) {
    return <Card>Loading identity...</Card>;
  }

  return (
    <Card>
      <h1 style={{ margin: 0 }}>Your agent is live</h1>
      <p style={{ color: "#666", marginTop: 4 }}>
        Registered in AIR with a verifiable identity.
      </p>

      <div style={{ marginTop: 24 }}>
        <div style={{ fontSize: 13, color: "#666" }}>Name</div>
        <div style={{ fontSize: 16, marginTop: 2 }}>{identity.name}</div>
      </div>

      <div style={{ marginTop: 16 }}>
        <div style={{ fontSize: 13, color: "#666" }}>DID</div>
        <div
          style={{
            fontSize: 12,
            fontFamily: "monospace",
            marginTop: 2,
            wordBreak: "break-all",
          }}
        >
          {identity.did}
        </div>
      </div>

      <div style={{ marginTop: 16 }}>
        <div style={{ fontSize: 13, color: "#666" }}>Trust score</div>
        <div style={{ fontSize: 16, marginTop: 2 }}>
          {trustScore ?? "—"}
        </div>
      </div>

      <div style={{ marginTop: 24 }}>
        <Button onClick={onFinish}>Open BossClaw</Button>
      </div>
    </Card>
  );
}
