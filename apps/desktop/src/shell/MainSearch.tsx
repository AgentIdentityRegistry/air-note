/**
 * The prominent main-screen search field (ChatGPT/Claude style). It is a button
 * that opens the existing CommandPalette overlay; ⌘K opens the same overlay via
 * `useCommandPaletteHotkey`, so both paths share one search surface.
 */
export function MainSearch({ onOpen }: { onOpen: () => void }) {
  return (
    <button
      type="button"
      className="main-search"
      onClick={onOpen}
      aria-label="Search memory, conversations, and files"
    >
      <span className="main-search-icon" aria-hidden>
        ⌕
      </span>
      <span className="main-search-placeholder">Search memory, conversations, files…</span>
      <span className="main-search-kbd">⌘K</span>
    </button>
  );
}
