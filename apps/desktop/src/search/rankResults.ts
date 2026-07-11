import type { HitDto } from "../api/engine";
import { KIND_LABEL } from "../memory/recallView";
import { type GroupedResults, type SearchResult, RESULTS_PER_GROUP } from "./types";

/** Map recall hits to memory SearchResults (capped, ordered as recall returned them). */
export function memoryResults(hits: HitDto[], cap = RESULTS_PER_GROUP): SearchResult[] {
  return hits.slice(0, cap).map((h) => ({
    id: `mem:${h.event_id}`,
    kind: "memory" as const,
    title: KIND_LABEL[h.kind] ?? h.kind,
    snippet: h.text,
    target: { view: "memory" as const },
  }));
}

/**
 * Flatten grouped results into the keyboard-navigation order:
 * memory → sessions (Library) → conversations → files.
 * MUST stay in lockstep with the palette's `groups` array order, or arrow-key
 * selection (which indexes into this flat list) points at the wrong row.
 */
export function flattenResults(g: GroupedResults): SearchResult[] {
  return [...g.memory, ...g.sessions, ...g.conversations, ...g.files];
}
