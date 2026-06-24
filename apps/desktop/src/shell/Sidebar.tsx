import { type View, MAIN_NAV } from "./nav";
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
  const countFor = (v: View): number | undefined =>
    v === "inbox" ? inboxUnread : v === "review" ? reviewCount : undefined;

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
            active={view === item.view}
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
