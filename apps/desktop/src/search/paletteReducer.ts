import { type GroupedResults, EMPTY_RESULTS, type SearchResult } from "./types";
import { flattenResults } from "./rankResults";

export type PaletteState = {
  query: string;
  results: GroupedResults;
  selectedIndex: number;
  status: "idle" | "loading" | "ready";
};

export const initialPaletteState: PaletteState = {
  query: "",
  results: EMPTY_RESULTS,
  selectedIndex: 0,
  status: "idle",
};

export type PaletteAction =
  | { type: "reset" }
  | { type: "setQuery"; query: string }
  | { type: "loading" }
  | { type: "setResults"; results: GroupedResults }
  | { type: "move"; delta: 1 | -1 };

export function paletteReducer(state: PaletteState, action: PaletteAction): PaletteState {
  switch (action.type) {
    case "reset":
      return initialPaletteState;
    case "setQuery":
      return { ...state, query: action.query };
    case "loading":
      return state.status === "loading" ? state : { ...state, status: "loading" };
    case "setResults":
      return { ...state, results: action.results, selectedIndex: 0, status: "ready" };
    case "move": {
      const n = flattenResults(state.results).length;
      if (n === 0) return state;
      const next = (state.selectedIndex + action.delta + n) % n;
      return { ...state, selectedIndex: next };
    }
    default: {
      const _exhaustive: never = action;
      return _exhaustive;
    }
  }
}

/** The currently-highlighted result, or null when the list is empty. */
export function selectedResult(state: PaletteState): SearchResult | null {
  return flattenResults(state.results)[state.selectedIndex] ?? null;
}
