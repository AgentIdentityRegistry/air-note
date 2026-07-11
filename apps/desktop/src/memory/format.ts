/**
 * Shared Library formatting helpers. A leaf module (imports nothing from the panels), so both
 * LibraryPanel and SessionReader — and C6's RecallStats strip + notes list — can share one copy.
 */

/** Epoch seconds → a short, locale-formatted date (date only). */
export function formatDay(epochSeconds: number): string {
  return new Date(epochSeconds * 1000).toLocaleDateString();
}
