import { type View, navBadge } from "./nav";
import { StatusBadge } from "../components/ui/StatusBadge";

export function NavItem({
  view,
  label,
  count,
  active,
  onNavigate,
}: {
  view: View;
  label: string;
  count?: number;
  active: boolean;
  onNavigate: (v: View) => void;
}) {
  const badge = navBadge(count);
  return (
    <button
      type="button"
      className={active ? "tab-btn active" : "tab-btn"}
      aria-current={active ? "page" : undefined}
      onClick={() => onNavigate(view)}
    >
      <span>{label}</span>
      {badge ? <StatusBadge tone="primary">{badge}</StatusBadge> : null}
    </button>
  );
}
