import type { HitDto } from "../api/engine";
import { KIND_LABEL } from "../memory/recallView";
import type { GroupedResults, SearchResult } from "./types";

/** Map recall hits to memory SearchResults (capped, ordered as recall returned them). */
export function memoryResults(hits: HitDto[], cap = 5): SearchResult[] {
  return hits.slice(0, cap).map((h) => ({
    id: `mem:${h.event_id}`,
    kind: "memory" as const,
    title: KIND_LABEL[h.kind] ?? h.kind,
    snippet: h.text,
    target: { view: "memory" as const },
  }));
}

/** Flatten grouped results into the keyboard-navigation order: memory → conversations → files. */
export function flattenResults(g: GroupedResults): SearchResult[] {
  return [...g.memory, ...g.conversations, ...g.files];
}
