/** A pure validation result: ok with the normalized handle, or a single human-readable error. */
export type UsernameResult =
  | { ok: true; username: string }
  | { ok: false; error: string };

/**
 * Validate a proposed published `@handle` CLIENT-SIDE for fast feedback (the backend's
 * `validate_username`, commands/identity.rs, is the authority and re-checks on claim). Pure +
 * deterministic. Trims, lowercases, then enforces `^[a-z0-9_]{3,30}$` on the result — ASCII only,
 * so anything that still doesn't match after lowercasing is rejected. Returns the normalized handle
 * on success. The reserved-word denylist is the registry's job, not the desktop's.
 */
export function validateUsername(raw: string): UsernameResult {
  const username = raw.trim().toLowerCase();
  if (username.length < 3 || username.length > 30) {
    return { ok: false, error: "Username must be 3–30 characters." };
  }
  if (!/^[a-z0-9_]+$/.test(username)) {
    return { ok: false, error: "Username can only use letters, numbers, and underscores." };
  }
  return { ok: true, username };
}
