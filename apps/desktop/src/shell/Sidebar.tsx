import { type View, MAIN_NAV, isBrainView } from "./nav";
import { NavItem } from "./NavItem";
import { useTheme } from "../state/theme";

export function Sidebar({
  view,
  onNavigate,
  inboxUnread,
  reviewCount,
}: {
  view: View;
  onNavigate: (v: View) => void;
  inboxUnread: number;
  reviewCount: number;
}) {
  const { theme, toggleTheme } = useTheme();
  // The review count surfaces on the Brain tab (whose nav item routes at a Brain view — the
  // Library landing) now that Review lives inside the Brain hub.
  const countFor = (v: View): number | undefined =>
    v === "inbox" ? inboxUnread : isBrainView(v) ? reviewCount : undefined;

  return (
    <aside className="sidebar">
      <div className="brand">
        <h1>AIR Agent</h1>
      </div>

      <nav className="tab-list" aria-label="Primary">
        {MAIN_NAV.map((item) => (
          <NavItem
            key={item.view}
            view={item.view}
            label={item.label}
            count={countFor(item.view)}
            active={isBrainView(item.view) ? isBrainView(view) : view === item.view}
            onNavigate={onNavigate}
          />
        ))}
      </nav>

      <div className="sidebar-footer-icons">
        <button
          type="button"
          className={view === "settings" ? "icon-btn active" : "icon-btn"}
          aria-label="Settings"
          aria-current={view === "settings" ? "page" : undefined}
          title="Settings"
          onClick={() => onNavigate("settings")}
        >
          <GearIcon />
        </button>
        <button
          type="button"
          className="icon-btn"
          aria-label="Toggle light or dark theme"
          title="Toggle light or dark theme"
          onClick={toggleTheme}
        >
          {theme === "dark" ? <SunIcon /> : <MoonIcon />}
        </button>
      </div>
    </aside>
  );
}

/** 16px inline icons for the footer. `aria-hidden` — the button carries the label. */
function GearIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden focusable="false">
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
    </svg>
  );
}

function SunIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden focusable="false">
      <circle cx="12" cy="12" r="5" />
      <path d="M12 1v2M12 21v2M4.22 4.22l1.42 1.42M18.36 18.36l1.42 1.42M1 12h2M21 12h2M4.22 19.78l1.42-1.42M18.36 5.64l1.42-1.42" />
    </svg>
  );
}

function MoonIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden focusable="false">
      <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z" />
    </svg>
  );
}
