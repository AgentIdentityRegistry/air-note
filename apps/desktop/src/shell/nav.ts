/** The set of top-level panels the shell can show. Single source of truth (App, Sidebar, search). */
export type View = "identity" | "inbox" | "memory" | "review" | "mandates" | "settings";

export type NavItemDef = { view: View; label: string };

/** Primary nav, in display order. `settings` is rendered separately (pinned to the footer). */
export const MAIN_NAV: readonly NavItemDef[] = [
  { view: "identity", label: "AIR" },
  { view: "inbox", label: "AIR Note" },
  { view: "memory", label: "Brain" },
] as const;

/** Views hosted inside the Brain hub (Brain search/evolve + Review + Mandates). */
export const BRAIN_VIEWS = ["memory", "review", "mandates"] as const;
export const isBrainView = (v: View): boolean => (BRAIN_VIEWS as readonly View[]).includes(v);

/** Display text for a nav count badge, or null when there is nothing to show. */
export function navBadge(count: number | undefined): string | null {
  return count && count > 0 ? String(count) : null;
}
