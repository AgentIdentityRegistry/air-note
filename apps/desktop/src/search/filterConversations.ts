import type { Conversation } from "../inbox/model";
import type { ContactView } from "../api/inbox";
import { conversationLabel } from "../inbox/displayName";
import { type SearchResult, RESULTS_PER_GROUP } from "./types";

/** Pure client-side filter over already-loaded conversation summaries (name/handle + convKey + preview). */
export function filterConversations(
  convs: Conversation[],
  query: string,
  contacts?: Map<string, ContactView>,
  cap = RESULTS_PER_GROUP,
): SearchResult[] {
  const q = query.trim().toLowerCase();
  if (!q) return [];
  return convs
    .filter((c) => {
      const contact = c.kind === "peer" ? contacts?.get(c.convKey) : undefined;
      const { label, handle } = conversationLabel(c.convKey, c.kind, contact);
      return (
        c.convKey.toLowerCase().includes(q) ||
        c.lastText.toLowerCase().includes(q) ||
        label.toLowerCase().includes(q) ||
        (handle ?? "").toLowerCase().includes(q)
      );
    })
    .slice(0, cap)
    .map((c) => {
      const contact = c.kind === "peer" ? contacts?.get(c.convKey) : undefined;
      const { label } = conversationLabel(c.convKey, c.kind, contact);
      return {
        id: `conv:${c.convKey}`,
        kind: "conversation" as const,
        title: label,
        snippet: c.lastText,
        target: { view: "inbox" as const, convKey: c.convKey },
      };
    });
}
