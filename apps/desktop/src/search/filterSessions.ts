import type { SessionSummaryDto } from "../api/engine";
import { type SearchResult, RESULTS_PER_GROUP } from "./types";

/** Pure client-side filter over the already-loaded captured-session list (title/project). */
export function filterSessions(
  sessions: SessionSummaryDto[],
  query: string,
  cap = RESULTS_PER_GROUP,
): SearchResult[] {
  const q = query.trim().toLowerCase();
  if (!q) return [];
  return sessions
    .filter((s) => s.title.toLowerCase().includes(q) || s.project.toLowerCase().includes(q))
    .slice(0, cap)
    .map((s) => ({
      id: `session:${s.session_id}`,
      kind: "session" as const,
      title: s.title,
      snippet: s.project,
      target: { view: "library" as const },
    }));
}
