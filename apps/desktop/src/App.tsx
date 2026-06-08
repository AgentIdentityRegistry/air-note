import { useState } from "react";
import { IdentityProvider, useIdentity } from "./state/identity";
import { OnboardingProvider, useOnboarding } from "./state/onboarding";
import { Welcome } from "./onboarding/Welcome";
import { NameAgent } from "./onboarding/NameAgent";
import { GenerateAndRegister } from "./onboarding/GenerateAndRegister";
import { Done } from "./onboarding/Done";
import { IdentityPanel } from "./identity/IdentityPanel";
import { AirSettings } from "./settings/AirSettings";
import { Button } from "./components/Button";

export default function App() {
  return (
    <IdentityProvider>
      <OnboardingProvider>
        <Shell />
      </OnboardingProvider>
    </IdentityProvider>
  );
}

function Shell() {
  const { identity, loading } = useIdentity();
  const [view, setView] = useState<"identity" | "settings">("identity");
  const [onboardingDone, setOnboardingDone] = useState(false);

  if (loading) {
    return (
      <div style={{ padding: "2rem" }}>Loading...</div>
    );
  }

  if (!identity && !onboardingDone) {
    return (
      <div style={{ padding: "2rem", maxWidth: 600 }}>
        <OnboardingFlow onDone={() => setOnboardingDone(true)} />
      </div>
    );
  }

  return (
    <div style={{ padding: "2rem", maxWidth: 600, fontFamily: "system-ui" }}>
      <nav style={{ display: "flex", gap: 8, marginBottom: 16 }}>
        <Button
          variant={view === "identity" ? "primary" : "secondary"}
          onClick={() => setView("identity")}
        >
          Identity
        </Button>
        <Button
          variant={view === "settings" ? "primary" : "secondary"}
          onClick={() => setView("settings")}
        >
          Settings
        </Button>
      </nav>

      {view === "identity" ? <IdentityPanel /> : <AirSettings />}
    </div>
  );
}

function OnboardingFlow({ onDone }: { onDone: () => void }) {
  const { state } = useOnboarding();

  switch (state.step) {
    case "welcome":
      return <Welcome />;
    case "name":
      return <NameAgent />;
    case "generating":
    case "registering":
      return <GenerateAndRegister />;
    case "done":
      return <Done onFinish={onDone} />;
  }
}
