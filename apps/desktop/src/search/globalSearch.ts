import { recall as recallOp, listFiles as listFilesOp, type HitDto, type FileRecordDto } from "../api/engine";
import type { Conversation } from "../inbox/model";
import { filterConversations } from "./filterConversations";
import { filterFiles } from "./filterFiles";
import { memoryResults } from "./rankResults";
import { type GroupedResults, EMPTY_RESULTS } from "./types";

/** Injected I/O + data, so the façade is testable without the engine. */
export type GlobalSearchDeps = {
  recall: (q: string, k: number) => Promise<HitDto[]>;
  listFiles: () => Promise<FileRecordDto[]>;
  conversations: Conversation[];
};

const MEMORY_K = 5;

/**
 * Fan out to memory (recall), files (listFiles), and the in-memory conversations concurrently.
 * Each source is isolated: a rejection yields an empty group + errors.<source> = true.
 */
export async function globalSearch(query: string, deps: GlobalSearchDeps): Promise<GroupedResults> {
  const q = query.trim();
  if (!q) return EMPTY_RESULTS;

  const [mem, files] = await Promise.allSettled([deps.recall(q, MEMORY_K), deps.listFiles()]);

  return {
    memory: mem.status === "fulfilled" ? memoryResults(mem.value) : [],
    conversations: filterConversations(deps.conversations, q),
    files: files.status === "fulfilled" ? filterFiles(files.value, q) : [],
    errors: {
      memory: mem.status === "rejected",
      conversations: false, // pure client-side filter cannot fail
      files: files.status === "rejected",
    },
  };
}

/** Wire the real engine ops + the live conversation list at the call site. */
export const defaultSearchDeps = (conversations: Conversation[]): GlobalSearchDeps => ({
  recall: recallOp,
  listFiles: listFilesOp,
  conversations,
});
