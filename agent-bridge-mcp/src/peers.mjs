// peers.mjs — small shared helpers for peer identifiers (DID ↔ short AIR-id).

/** Short AIR-id label from a DID (or pass the value through if it has none). */
export function shortPeer(did) {
  const m = String(did).match(/AIR-[A-Za-z0-9-]+/);
  return m ? m[0] : String(did);
}

/** Parse the AIRMSG_MUTE env value (comma-separated alias/DID/AIR-id) into a Set,
 *  trimming whitespace and dropping empty entries. undefined/empty → empty Set. */
export function parseMuteSet(env = process.env.AIRMSG_MUTE) {
  return new Set((env || "").split(",").map((s) => s.trim()).filter(Boolean));
}
