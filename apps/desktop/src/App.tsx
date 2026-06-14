import { useState } from "react";
import { IdentityProvider, useIdentity } from "./state/identity";
import { OnboardingProvider, useOnboarding } from "./state/onboarding";
import { InboxProvider, useInbox } from "./state/inbox";
import { AiLoopProvider } from "./state/aiLoop";
import { Welcome } from "./onboarding/Welcome";
import { NameAgent } from "./onboarding/NameAgent";
import { GenerateAndRegister } from "./onboarding/GenerateAndRegister";
import { Done } from "./onboarding/Done";
import { IdentityPanel } from "./identity/IdentityPanel";
import { InboxPanel } from "./inbox/InboxPanel";
import { AirSettings } from "./settings/AirSettings";
import { Button } from "./components/Button";

export default function App() {
  return (
    <IdentityProvider>
      <OnboardingProvider>
        <InboxProvider>
          <AiLoopProvider>
            <Shell />
          </AiLoopProvider>
        </InboxProvider>
      </OnboardingProvider>
    </IdentityProvider>
  );
}

type View = "identity" | "inbox" | "settings";

function Shell() {
  const { identity, loading } = useIdentity();
  const [view, setView] = useState<View>("identity");
  const [onboardingDone, setOnboardingDone] = useState(false);

  if (loading) return <div style={{ padding: "2rem" }}>Loading...</div>;
  if (!identity && !onboardingDone) {
    return <div style={{ padding: "2rem", maxWidth: 600 }}><OnboardingFlow onDone={() => setOnboardingDone(true)} /></div>;
  }
  return (
    <div style={{ padding: "2rem", maxWidth: 760, fontFamily: "system-ui" }}>
      <nav style={{ display: "flex", gap: 8, marginBottom: 16 }}>
        <Button variant={view === "identity" ? "primary" : "secondary"} onClick={() => setView("identity")}>Identity</Button>
        <InboxNavButton active={view === "inbox"} onClick={() => setView("inbox")} />
        <Button variant={view === "settings" ? "primary" : "secondary"} onClick={() => setView("settings")}>Settings</Button>
      </nav>
      {view === "identity" ? <IdentityPanel /> : view === "inbox" ? <InboxPanel /> : <AirSettings />}
    </div>
  );
}

function InboxNavButton({ active, onClick }: { active: boolean; onClick: () => void }) {
  const { totalUnread } = useInbox();
  return (
    <Button variant={active ? "primary" : "secondary"} onClick={onClick}>
      Inbox{totalUnread > 0 ? ` (${totalUnread})` : ""}
    </Button>
  );
}

function OnboardingFlow({ onDone }: { onDone: () => void }) {
  const { state } = useOnboarding();
  switch (state.step) {
    case "welcome": return <Welcome />;
    case "name": return <NameAgent />;
    case "generating":
    case "registering": return <GenerateAndRegister />;
    case "done": return <Done onFinish={onDone} />;
  }
}
