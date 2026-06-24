import type { Conversation } from "../inbox/model";
import { type SearchResult, RESULTS_PER_GROUP } from "./types";

/** Pure client-side filter over already-loaded conversation summaries (title + preview). */
export function filterConversations(convs: Conversation[], query: string, cap = RESULTS_PER_GROUP): SearchResult[] {
  const q = query.trim().toLowerCase();
  if (!q) return [];
  return convs
    .filter((c) => c.convKey.toLowerCase().includes(q) || c.lastText.toLowerCase().includes(q))
    .slice(0, cap)
    .map((c) => ({
      id: `conv:${c.convKey}`,
      kind: "conversation" as const,
      title: c.convKey,
      snippet: c.lastText,
      target: { view: "inbox" as const, convKey: c.convKey },
    }));
}
