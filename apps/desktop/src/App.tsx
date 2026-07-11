import { useCallback, useEffect, useRef, useState } from "react";
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
import { BrainPanel } from "./memory/BrainPanel";
import { AirSettings } from "./settings/AirSettings";
import { Sidebar } from "./shell/Sidebar";
import { MainSearch } from "./shell/MainSearch";
import { useReviewCount } from "./shell/useReviewCount";
import { landingView, type View } from "./shell/nav";
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
  const [onboardingDone, setOnboardingDone] = useState(false);
  // Identity loads asynchronously, so at mount we can't yet know if the user is onboarded.
  // Start on the landing for the not-yet-onboarded state (identity) and, once onboarding
  // resolves true, flip to the Library exactly once (see the effect below). A fresh user
  // never has the flip fire, so the onboarding gate is never bypassed.
  const onboarded = !!identity || onboardingDone;
  const [view, setView] = useState<View>(() => landingView(onboarded));
  const landedRef = useRef(false);
  const [searchOpen, setSearchOpen] = useState(false);
  const openSearch = useCallback(() => setSearchOpen(true), []);
  useCommandPaletteHotkey(openSearch);

  useEffect(() => {
    // One-shot: land an onboarded user on their Library the first time onboarding resolves
    // true, without clobbering any later navigation the user makes.
    if (onboarded && !landedRef.current) {
      landedRef.current = true;
      setView(landingView(true));
    }
  }, [onboarded]);

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
      <Sidebar view={view} onNavigate={setView} inboxUnread={totalUnread} reviewCount={reviewCount} />
      {/* The inbox fills the viewport and owns its own scroll; other panels scroll the whole area. */}
      <main className={view === "inbox" ? "main-area main-area-fill" : "main-area"}>
        <MainSearch onOpen={openSearch} />
        {view === "identity" ? (
          <IdentityPanel />
        ) : view === "inbox" ? (
          <InboxPanel />
        ) : view === "settings" ? (
          <AirSettings />
        ) : (
          <BrainPanel view={view} onSubNav={setView} reviewCount={reviewCount} />
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
