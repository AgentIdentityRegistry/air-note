import { Button } from "../components/Button";
import { Card } from "../components/Card";
import { useOnboarding } from "../state/onboarding";

export function Welcome() {
  const { dispatch } = useOnboarding();
  return (
    <Card>
      <h1 style={{ margin: 0 }}>Welcome to BossClaw</h1>
      <p style={{ color: "#666", marginTop: 4 }}>
        Your AI agent, with verifiable identity.
      </p>
      <p style={{ marginTop: "1.5rem", lineHeight: 1.5 }}>
        BossClaw is an open-source AI agent that acts on your behalf. To start,
        we'll create a cryptographic identity for your agent and register it with{" "}
        <a href="https://agentidentityregistry.org" target="_blank">AIR</a> (the
        Agent Identity Registry).
      </p>
      <p style={{ marginTop: 12, color: "#666", fontSize: 13 }}>
        Your agent's private key stays on this device. We never see it.
      </p>
      <div style={{ marginTop: 24 }}>
        <Button onClick={() => dispatch({ type: "next" })}>Continue</Button>
      </div>
    </Card>
  );
}
