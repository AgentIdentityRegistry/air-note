import type { Conversation } from "./model";
import type { ConversationSummary } from "../api/inbox";

/** Authoritative sidebar = every archived conversation (`summaries`), enriched with preview/unread
 *  from `grouped` (conversations built from loaded rows), plus any conv that exists only in loaded
 *  rows (a new live conversation this session). Newest-first. (C1 fix.) */
export function mergeSidebar(summaries: ConversationSummary[], grouped: Conversation[]): Conversation[] {
  const byKey = new Map(grouped.map((g) => [g.convKey, g]));
  const used = new Set<string>();
  const out: Conversation[] = [];
  for (const s of summaries) {
    used.add(s.conv_key);
    const g = byKey.get(s.conv_key);
    out.push(g ?? { convKey: s.conv_key, kind: s.kind, lastTimestamp: s.last_timestamp, lastText: "", unread: 0 });
  }
  for (const g of grouped) if (!used.has(g.convKey)) out.push(g);
  return out.sort((a, b) =>
    a.lastTimestamp === b.lastTimestamp ? (a.convKey < b.convKey ? -1 : 1)
      : a.lastTimestamp < b.lastTimestamp ? 1 : -1); // m4: stable tie-break on convKey
}
