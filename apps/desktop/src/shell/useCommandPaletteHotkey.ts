import { useEffect } from "react";

/**
 * Install a global ⌘K (macOS) / Ctrl-K (others) listener that calls `onOpen`.
 * `onOpen` is read through a ref-free dependency so callers pass a stable handler
 * (e.g. a useState setter) without re-binding every render.
 */
export function useCommandPaletteHotkey(onOpen: () => void): void {
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        onOpen();
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [onOpen]);
}
