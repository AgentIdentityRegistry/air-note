export type Theme = "light" | "dark";

/** localStorage key holding the user's explicit theme choice. */
export const THEME_STORAGE_KEY = "air.theme";

/** Parse a stored value into a Theme, or null if absent/invalid. */
export function parseStoredTheme(raw: string | null): Theme | null {
  return raw === "light" || raw === "dark" ? raw : null;
}

/** A stored choice always wins; otherwise follow the OS dark-mode preference. */
export function resolveInitialTheme(stored: Theme | null, prefersDark: boolean): Theme {
  return stored ?? (prefersDark ? "dark" : "light");
}
