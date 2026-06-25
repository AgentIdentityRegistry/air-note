import type { ContactView } from "../api/inbox";

/** Strip a `did:wba:<domain>:agents:` prefix down to the bare AIR id (the historical `short()`). */
export function shortDid(did: string): string {
  return did.replace(/^did:wba:[^:]+:agents:/, "");
}

/**
 * Resolve a peer DID to a human display name.
 * Precedence: user `alias` → registry `name` → `short(did)`.
 * Rooms are NOT people — callers gate on `kind === "peer"` (see `conversationLabel`).
 */
export function displayName(did: string, contact?: ContactView): string {
  const alias = contact?.alias?.trim();
  const name = contact?.name?.trim();
  return alias || name || shortDid(did);
}

/** The published `@handle` for a contact (with leading `@`), or null when unclaimed. */
export function handleOf(contact?: ContactView): string | null {
  const u = contact?.username?.trim();
  return u ? `@${u}` : null;
}

/** Index a contacts payload by DID for O(1) lookups during rendering/search. */
export function contactsByDid(contacts: ContactView[]): Map<string, ContactView> {
  return new Map(contacts.map((cv) => [cv.did, cv]));
}

/**
 * The label + optional handle for a conversation row / thread head / search title.
 * Peers resolve through `displayName`/`handleOf`; rooms keep their id and carry no handle.
 */
export function conversationLabel(
  convKey: string,
  kind: "room" | "peer",
  contact?: ContactView,
): { label: string; handle: string | null } {
  if (kind === "room") return { label: convKey, handle: null };
  return { label: displayName(convKey, contact), handle: handleOf(contact) };
}
