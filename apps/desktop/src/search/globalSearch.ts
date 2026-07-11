import {
  recall as recallOp,
  listFiles as listFilesOp,
  listSessions as listSessionsOp,
  type HitDto,
  type FileRecordDto,
  type SessionSummaryDto,
} from "../api/engine";
import type { Conversation } from "../inbox/model";
import type { ContactView } from "../api/inbox";
import { filterConversations } from "./filterConversations";
import { filterFiles } from "./filterFiles";
import { filterSessions } from "./filterSessions";
import { memoryResults } from "./rankResults";
import { type GroupedResults, EMPTY_RESULTS, RESULTS_PER_GROUP } from "./types";

/** Injected I/O + data, so the façade is testable without the engine. */
export type GlobalSearchDeps = {
  recall: (q: string, k: number) => Promise<HitDto[]>;
  listFiles: () => Promise<FileRecordDto[]>;
  listSessions: () => Promise<SessionSummaryDto[]>;
  conversations: Conversation[];
  contacts: Map<string, ContactView>;
};

/**
 * Fan out to memory (recall), captured sessions (listSessions), files (listFiles), and the
 * in-memory conversations concurrently. Each source is isolated: a rejection yields an empty
 * group + errors.<source> = true.
 */
export async function globalSearch(query: string, deps: GlobalSearchDeps): Promise<GroupedResults> {
  const q = query.trim();
  if (!q) return EMPTY_RESULTS;

  const [mem, sessions, files] = await Promise.allSettled([
    deps.recall(q, RESULTS_PER_GROUP),
    deps.listSessions(),
    deps.listFiles(),
  ]);

  return {
    memory: mem.status === "fulfilled" ? memoryResults(mem.value, RESULTS_PER_GROUP) : [],
    sessions: sessions.status === "fulfilled" ? filterSessions(sessions.value, q) : [],
    conversations: filterConversations(deps.conversations, q, deps.contacts),
    files: files.status === "fulfilled" ? filterFiles(files.value, q) : [],
    errors: {
      memory: mem.status === "rejected",
      sessions: sessions.status === "rejected",
      conversations: false, // pure client-side filter cannot fail
      files: files.status === "rejected",
    },
  };
}

/** Wire the real engine ops + the live conversation list + contacts map at the call site. */
export const defaultSearchDeps = (
  conversations: Conversation[],
  contacts: Map<string, ContactView>,
): GlobalSearchDeps => ({
  recall: recallOp,
  listFiles: listFilesOp,
  listSessions: listSessionsOp,
  conversations,
  contacts,
});
