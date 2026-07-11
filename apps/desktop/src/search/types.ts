import type { View } from "../shell/nav";

/** Where pressing Enter on a result takes the user. */
export type NavTarget = { view: View; convKey?: string };

export type SearchResultKind = "memory" | "session" | "conversation" | "file";

export type SearchResult = {
  id: string;
  kind: SearchResultKind;
  title: string;
  snippet: string;
  target: NavTarget;
};

export type GroupedResults = {
  memory: SearchResult[];
  sessions: SearchResult[];
  conversations: SearchResult[];
  files: SearchResult[];
  errors: { memory: boolean; sessions: boolean; conversations: boolean; files: boolean };
};

/** The all-empty result set (empty query, or before the first search). */
export const EMPTY_RESULTS: GroupedResults = {
  memory: [],
  sessions: [],
  conversations: [],
  files: [],
  errors: { memory: false, sessions: false, conversations: false, files: false },
};
// Frozen: this is a shared singleton (initial reducer state, empty-query result). Mutating it
// would corrupt every consumer, so lock it at every level.
Object.freeze(EMPTY_RESULTS);
Object.freeze(EMPTY_RESULTS.memory);
Object.freeze(EMPTY_RESULTS.sessions);
Object.freeze(EMPTY_RESULTS.conversations);
Object.freeze(EMPTY_RESULTS.files);
Object.freeze(EMPTY_RESULTS.errors);

/** Max results shown per group (memory / conversations / files) in the command palette. */
export const RESULTS_PER_GROUP = 5;
