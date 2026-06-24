import { useEffect, useState } from "react";
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
import { MemoryPanel } from "./memory/MemoryPanel";
import { ReviewPanel } from "./review/ReviewPanel";
import { MandatesPanel } from "./mandates/MandatesPanel";
import { AirSettings } from "./settings/AirSettings";
import { Button } from "./components/Button";
import { listProposals } from "./api/engine";

/** How often the Review nav badge refreshes its pending count while that tab is active. */
const REVIEW_POLL_MS = 5000;

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

type View = "identity" | "inbox" | "memory" | "review" | "mandates" | "settings";

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
        <Button variant={view === "memory" ? "primary" : "secondary"} onClick={() => setView("memory")}>Memory</Button>
        <ReviewNavButton active={view === "review"} onClick={() => setView("review")} />
        <Button variant={view === "mandates" ? "primary" : "secondary"} onClick={() => setView("mandates")}>Mandates</Button>
        <Button variant={view === "settings" ? "primary" : "secondary"} onClick={() => setView("settings")}>Settings</Button>
      </nav>
      {view === "identity" ? <IdentityPanel /> : view === "inbox" ? <InboxPanel /> : view === "memory" ? <MemoryPanel /> : view === "review" ? <ReviewPanel /> : view === "mandates" ? <MandatesPanel /> : <AirSettings />}
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

function ReviewNavButton({ active, onClick }: { active: boolean; onClick: () => void }) {
  const { identity } = useIdentity();
  const [count, setCount] = useState(0);
  useEffect(() => {
    if (!identity) { setCount(0); return; } // no engine before onboarding → don't poll.
    let alive = true;
    const refresh = () => {
      listProposals()
        .then((ps) => { if (alive) setCount(ps.length); })
        .catch(() => { if (alive) setCount(0); });
    };
    refresh(); // one-shot whenever identity present (keeps the badge fresh on tab switch).
    // Only the active tab pays the poll; inactive tabs rely on the one-shot above.
    const id = active ? setInterval(refresh, REVIEW_POLL_MS) : undefined;
    return () => { alive = false; if (id) clearInterval(id); };
  }, [identity, active]);
  return (
    <Button variant={active ? "primary" : "secondary"} onClick={onClick}>
      Review{count > 0 ? ` (${count})` : ""}
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
