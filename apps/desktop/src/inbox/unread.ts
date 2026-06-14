import { convKey, type ThreadItem } from "./model";

export function addUnread(set: Set<string>, envelopeId: string): Set<string> {
  const next = new Set(set); next.add(envelopeId); return next;
}

/** Clear unread for every loaded item belonging to `convKeyToClear`. */
export function clearConv(set: Set<string>, loaded: ThreadItem[], convKeyToClear: string): Set<string> {
  const next = new Set(set);
  for (const it of loaded) if (convKey(it) === convKeyToClear) next.delete(it.envelope_id);
  return next;
}
