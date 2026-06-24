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
  // The review count surfaces on the Brain (memory) tab now that Review lives inside the Brain hub.
  const countFor = (v: View): number | undefined =>
    v === "inbox" ? inboxUnread : v === "memory" ? reviewCount : undefined;

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
            active={item.view === "memory" ? isBrainView(view) : view === item.view}
            onNavigate={onNavigate}
          />
        ))}
      </nav>

      <div className="sidebar-footer">
        <button
          type="button"
          className="secondary-btn theme-toggle-btn"
          aria-label="Toggle light or dark theme"
          onClick={toggleTheme}
        >
          {theme === "dark" ? "☀ Light" : "☾ Dark"}
        </button>
        <NavItem view="settings" label="Settings" active={view === "settings"} onNavigate={onNavigate} />
      </div>
    </aside>
  );
}
