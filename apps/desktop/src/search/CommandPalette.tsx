import { useEffect, useReducer, useRef } from "react";
import { createPortal } from "react-dom";
import { useInbox } from "../state/inbox";
import { globalSearch, defaultSearchDeps } from "./globalSearch";
import { paletteReducer, initialPaletteState, selectedResult } from "./paletteReducer";
import { flattenResults } from "./rankResults";
import { type GroupedResults, type SearchResult, type NavTarget } from "./types";

const DEBOUNCE_MS = 180;

type SearchFn = (query: string, deps: ReturnType<typeof defaultSearchDeps>) => Promise<GroupedResults>;

export function CommandPalette({
  open,
  onClose,
  onNavigate,
  search = globalSearch as SearchFn,
}: {
  open: boolean;
  onClose: () => void;
  onNavigate: (target: NavTarget) => void;
  /** Injectable for tests; defaults to the real façade. */
  search?: SearchFn;
}) {
  const { conversations, contacts } = useInbox();
  const [state, dispatch] = useReducer(paletteReducer, initialPaletteState);
  const inputRef = useRef<HTMLInputElement>(null);
  // Latest conversations, read at debounce-fire time. Kept OUT of the search effect's
  // dependency array on purpose: callers (and the live inbox) hand us a fresh array
  // identity on every render, so depending on it would re-run the effect each render and
  // re-dispatch "loading" (a non-bailing reducer case) — an infinite render loop.
  const conversationsRef = useRef(conversations);
  conversationsRef.current = conversations;
  // Same trap as conversationsRef: useInbox() hands us a fresh Map identity each render,
  // so contacts must be read via ref at fire-time, never enter the search effect's deps.
  const contactsRef = useRef(contacts);
  contactsRef.current = contacts;

  // Reset + focus on open.
  useEffect(() => {
    if (open) {
      dispatch({ type: "reset" });
      inputRef.current?.focus();
    }
  }, [open]);

  // Debounced search whenever the query changes while open.
  useEffect(() => {
    if (!open) return;
    const q = state.query;
    if (!q.trim()) {
      dispatch({ type: "reset" });
      return;
    }
    dispatch({ type: "loading" });
    const id = setTimeout(() => {
      search(q, defaultSearchDeps(conversationsRef.current, contactsRef.current)).then((results) => dispatch({ type: "setResults", results }));
    }, DEBOUNCE_MS);
    return () => clearTimeout(id);
    // conversations intentionally read via ref (see conversationsRef above), not a dep.
  }, [open, state.query, search]);

  // Keyboard: arrows move, Enter navigates, Esc closes.
  useEffect(() => {
    if (!open) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === "ArrowDown") { e.preventDefault(); dispatch({ type: "move", delta: 1 }); }
      else if (e.key === "ArrowUp") { e.preventDefault(); dispatch({ type: "move", delta: -1 }); }
      else if (e.key === "Escape") { e.preventDefault(); onClose(); }
      else if (e.key === "Enter") {
        const sel = selectedResult(state);
        if (sel) { onNavigate(sel.target); onClose(); }
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [open, state, onClose, onNavigate]);

  if (!open) return null;

  const groups: Array<{ label: string; items: SearchResult[]; error: boolean; source: string }> = [
    { label: "Memory", items: state.results.memory, error: state.results.errors.memory, source: "memory" },
    { label: "Conversations", items: state.results.conversations, error: state.results.errors.conversations, source: "conversations" },
    { label: "Files", items: state.results.files, error: state.results.errors.files, source: "files" },
  ];
  const flat = flattenResults(state.results);
  const hasAny = flat.length > 0;

  return createPortal(
    <div
      className="command-palette-backdrop"
      onMouseDown={(e) => { if (e.target === e.currentTarget) onClose(); }}
    >
      <div className="command-palette" role="dialog" aria-modal="true" aria-label="Global search">
        <input
          ref={inputRef}
          className="command-palette-input"
          placeholder="Search memory, conversations, files…"
          value={state.query}
          onChange={(e) => dispatch({ type: "setQuery", query: e.target.value })}
        />
        <div className="command-palette-results">
          {!state.query.trim() ? (
            <p className="command-palette-empty">Type to search across memory, conversations, and files.</p>
          ) : !hasAny && state.status === "ready" ? (
            <p className="command-palette-empty">No results for “{state.query}”.</p>
          ) : (
            groups.map((g) =>
              g.error ? (
                <p key={g.source} className="command-palette-error">Couldn’t search {g.label.toLowerCase()}.</p>
              ) : g.items.length > 0 ? (
                <div key={g.source}>
                  <div className="command-palette-group-label">{g.label}</div>
                  {g.items.map((item) => {
                    const isSelected = flat[state.selectedIndex]?.id === item.id;
                    return (
                      <button
                        key={item.id}
                        type="button"
                        className={isSelected ? "command-palette-item selected" : "command-palette-item"}
                        onMouseDown={(e) => { e.preventDefault(); onNavigate(item.target); onClose(); }}
                      >
                        <span className="command-palette-item-title">{item.title}</span>
                        <span className="command-palette-item-snippet">{item.snippet}</span>
                      </button>
                    );
                  })}
                </div>
              ) : null,
            )
          )}
        </div>
      </div>
    </div>,
    document.body,
  );
}
