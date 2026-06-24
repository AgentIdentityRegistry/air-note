import { type View, MAIN_NAV } from "./nav";
import { NavItem } from "./NavItem";
import { useTheme } from "../state/theme";

export function Sidebar({
  view,
  onNavigate,
  inboxUnread,
  reviewCount,
  onOpenSearch,
}: {
  view: View;
  onNavigate: (v: View) => void;
  inboxUnread: number;
  reviewCount: number;
  onOpenSearch: () => void;
}) {
  const { theme, toggleTheme } = useTheme();
  const countFor = (v: View): number | undefined =>
    v === "inbox" ? inboxUnread : v === "review" ? reviewCount : undefined;

  return (
    <aside className="sidebar">
      <div className="sidebar-top">
        <div className="brand">
          <h1>AIR Agent</h1>
        </div>
        <button
          type="button"
          className="secondary-btn sidebar-search-trigger"
          onClick={onOpenSearch}
          aria-label="Open global search"
        >
          <span>Search…</span>
          <span className="sidebar-search-kbd">⌘K</span>
        </button>
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
