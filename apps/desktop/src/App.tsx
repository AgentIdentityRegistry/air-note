import { useCallback, useState } from "react";
import { IdentityProvider, useIdentity } from "./state/identity";
import { OnboardingProvider, useOnboarding } from "./state/onboarding";
import { InboxProvider, useInbox } from "./state/inbox";
import { AiLoopProvider } from "./state/aiLoop";
import { ThemeProvider } from "./state/theme";
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
import { Sidebar } from "./shell/Sidebar";
import { useReviewCount } from "./shell/useReviewCount";
import type { View } from "./shell/nav";
import { CommandPalette } from "./search/CommandPalette";
import { useCommandPaletteHotkey } from "./shell/useCommandPaletteHotkey";
import type { NavTarget } from "./search/types";

export default function App() {
  return (
    <ThemeProvider>
      <IdentityProvider>
        <OnboardingProvider>
          <InboxProvider>
            <AiLoopProvider>
              <Shell />
            </AiLoopProvider>
          </InboxProvider>
        </OnboardingProvider>
      </IdentityProvider>
    </ThemeProvider>
  );
}

function Shell() {
  const { identity, loading } = useIdentity();
  const { totalUnread, select } = useInbox();
  const reviewCount = useReviewCount(!!identity);
  const [view, setView] = useState<View>("identity");
  const [onboardingDone, setOnboardingDone] = useState(false);
  const [searchOpen, setSearchOpen] = useState(false);
  const openSearch = useCallback(() => setSearchOpen(true), []);
  useCommandPaletteHotkey(openSearch);

  const navigateTo = useCallback((target: NavTarget) => {
    setView(target.view);
    if (target.view === "inbox" && target.convKey) select(target.convKey);
  }, [select]);

  if (loading) return <div className="app-loading">Loading…</div>;
  if (!identity && !onboardingDone) {
    return (
      <div className="onboarding-wrap">
        <OnboardingFlow onDone={() => setOnboardingDone(true)} />
      </div>
    );
  }

  return (
    <div className="app-shell">
      <Sidebar view={view} onNavigate={setView} inboxUnread={totalUnread} reviewCount={reviewCount} onOpenSearch={openSearch} />
      <main className="main-area">
        {view === "identity" ? (
          <IdentityPanel />
        ) : view === "inbox" ? (
          <InboxPanel />
        ) : view === "memory" ? (
          <MemoryPanel />
        ) : view === "review" ? (
          <ReviewPanel />
        ) : view === "mandates" ? (
          <MandatesPanel />
        ) : (
          <AirSettings />
        )}
      </main>
      <CommandPalette open={searchOpen} onClose={() => setSearchOpen(false)} onNavigate={navigateTo} />
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
