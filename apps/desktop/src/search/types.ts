import type { View } from "../shell/nav";

/** Where pressing Enter on a result takes the user. */
export type NavTarget = { view: View; convKey?: string };

export type SearchResultKind = "memory" | "conversation" | "file";

export type SearchResult = {
  id: string;
  kind: SearchResultKind;
  title: string;
  snippet: string;
  target: NavTarget;
};

export type GroupedResults = {
  memory: SearchResult[];
  conversations: SearchResult[];
  files: SearchResult[];
  errors: { memory: boolean; conversations: boolean; files: boolean };
};

/** The all-empty result set (empty query, or before the first search). */
export const EMPTY_RESULTS: GroupedResults = {
  memory: [],
  conversations: [],
  files: [],
  errors: { memory: false, conversations: false, files: false },
};

/** Max results shown per group (memory / conversations / files) in the command palette. */
export const RESULTS_PER_GROUP = 5;
