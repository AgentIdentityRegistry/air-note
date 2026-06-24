/** The backend's local-display-name cap (`MAX_DISPLAY_NAME_LEN`, commands/identity.rs). Mirrored
 *  here for fast inline feedback. The client counts trimmed UTF-16 code units while the backend
 *  counts Unicode scalar values, so the two can diverge on astral-plane input — the backend is the
 *  authority and re-validates on `rename_identity`. */
export const MAX_DISPLAY_NAME_LEN = 80;

/** A pure validation result: ok with the trimmed name, or a single human-readable error. */
export type DisplayNameResult =
  | { ok: true; name: string }
  | { ok: false; error: string };

/**
 * Validate a proposed local display name CLIENT-SIDE for fast feedback (the backend's
 * `validate_display_name` is still the authority and re-checks on rename). Pure + deterministic.
 * Returns the trimmed name on success.
 */
export function validateDisplayName(raw: string): DisplayNameResult {
  const name = raw.trim();
  if (name === "") return { ok: false, error: "Name can’t be empty." };
  if (name.length > MAX_DISPLAY_NAME_LEN) {
    return { ok: false, error: `Name is too long (max ${MAX_DISPLAY_NAME_LEN} characters).` };
  }
  return { ok: true, name };
}
