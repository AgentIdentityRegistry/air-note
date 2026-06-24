import { useEffect } from "react";
import { Card } from "../components/Card";
import { Loading } from "../components/Loading";
import { useOnboarding } from "../state/onboarding";
import { useIdentity } from "../state/identity";
import { createIdentity } from "../api/tauri";

export function GenerateAndRegister() {
  const { state, dispatch } = useOnboarding();
  const { refresh } = useIdentity();

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        await createIdentity(state.name, state.domain);
        if (cancelled) return;
        await refresh();
        if (cancelled) return;
        dispatch({ type: "next" });
      } catch (e) {
        if (cancelled) return;
        dispatch({
          type: "error",
          message: typeof e === "string" ? e : (e as Error).message,
        });
      }
    })();
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <Card>
      <h1 style={{ margin: 0 }}>Setting up your agent</h1>
      <div style={{ marginTop: 24 }}>
        <Loading
          label={
            state.step === "generating"
              ? "Generating your agent's cryptographic identity..."
              : "Registering with AIR..."
          }
        />
      </div>
      {state.error && (
        <div
          style={{
            marginTop: 16,
            padding: 12,
            background: "color-mix(in srgb, var(--error) 18%, var(--surface))",
            borderRadius: 6,
            color: "var(--error)",
          }}
        >
          {state.error}
        </div>
      )}
    </Card>
  );
}
