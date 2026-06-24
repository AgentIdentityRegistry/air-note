import { Button } from "../components/Button";
import { Input } from "../components/Input";
import { Card } from "../components/Card";
import { useOnboarding } from "../state/onboarding";

export function NameAgent() {
  const { state, dispatch } = useOnboarding();
  const canContinue = state.name.trim().length >= 2;

  return (
    <Card>
      <h1 style={{ margin: 0 }}>Name your agent</h1>
      <p style={{ color: "var(--text-secondary)", marginTop: 4 }}>
        Give it a name. This will be visible to other agents you transact with.
      </p>
      <div style={{ marginTop: 24 }}>
        <label style={{ display: "block", fontSize: 13, marginBottom: 6 }}>
          Agent name
        </label>
        <Input
          autoFocus
          placeholder="e.g. Peter's AIR Agent"
          value={state.name}
          onChange={(e) => dispatch({ type: "set_name", name: e.target.value })}
        />
      </div>
      <div style={{ marginTop: 16 }}>
        <label style={{ display: "block", fontSize: 13, marginBottom: 6 }}>
          Domain (advanced)
        </label>
        <Input
          value={state.domain}
          onChange={(e) => dispatch({ type: "set_domain", domain: e.target.value })}
        />
        <p style={{ fontSize: 12, color: "var(--text-tertiary)", marginTop: 4 }}>
          Your did:wba identifier will be derived from this domain.
        </p>
      </div>
      <div style={{ marginTop: 24, display: "flex", gap: 8 }}>
        <Button variant="secondary" onClick={() => dispatch({ type: "back" })}>
          Back
        </Button>
        <Button
          disabled={!canContinue}
          onClick={() => dispatch({ type: "next" })}
        >
          Continue
        </Button>
      </div>
    </Card>
  );
}
