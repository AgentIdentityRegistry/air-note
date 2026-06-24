import { MemoryPanel } from "./MemoryPanel";
import { ReviewPanel } from "../review/ReviewPanel";
import { MandatesPanel } from "../mandates/MandatesPanel";
import { StatusBadge } from "../components/ui/StatusBadge";
import { type View, isBrainView } from "../shell/nav";

/** The Brain hub's sub-tabs, in display order. */
const SUBTABS: { view: View; label: string }[] = [
  { view: "memory", label: "Search & Evolve" },
  { view: "review", label: "Review" },
  { view: "mandates", label: "Mandates" },
];

/**
 * The Brain hub: a single panel that hosts Search & Evolve, Review, and Mandates
 * behind sub-tabs. The active sub-tab is driven by the shell `view`; clicking a
 * sub-tab navigates the shell so search NavTargets keep working. The Review
 * sub-tab carries the needs-attention count.
 */
export function BrainPanel({
  view,
  onSubNav,
  reviewCount,
}: {
  view: View;
  onSubNav: (v: View) => void;
  reviewCount: number;
}) {
  // The hub only ever receives Brain views, but default to Search & Evolve if the
  // shell ever routes a non-Brain view here.
  const active: View = isBrainView(view) ? view : "memory";
  return (
    <div>
      <nav className="chat-subtabs" aria-label="Brain sections">
        {SUBTABS.map((t) => (
          <button
            key={t.view}
            type="button"
            className={active === t.view ? "tab-inline active" : "tab-inline"}
            aria-current={active === t.view ? "page" : undefined}
            onClick={() => onSubNav(t.view)}
          >
            <span>{t.label}</span>
            {t.view === "review" && reviewCount > 0 ? (
              <StatusBadge tone="primary">{String(reviewCount)}</StatusBadge>
            ) : null}
          </button>
        ))}
      </nav>
      <div style={{ marginTop: 12 }}>
        {active === "memory" ? <MemoryPanel /> : active === "review" ? <ReviewPanel /> : <MandatesPanel />}
      </div>
    </div>
  );
}
