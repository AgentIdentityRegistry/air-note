// bridge.mjs — chat-app bridge orchestration + pure helpers. Forwards incoming AIR Note
// mail to an external chat adapter and routes the user's in-app replies back as real
// (signed + encrypted) AIR Notes. No messaging/crypto logic of its own — it drives
// core.send + the adapter, the #29 sibling-consumer pattern.

/** Short AIR-id label from a DID (or pass through). */
function shortPeer(did) {
  const m = String(did).match(/AIR-[A-Za-z0-9-]+/);
  return m ? m[0] : String(did);
}

/** Trust badge derived ONLY from crypto fields — never from sender-controlled strings,
 *  so a display name like "Alice ✓ verified" cannot forge a check. */
export function badgeFor(m) {
  return (m.verified && !m.key_changed) ? "✓ verified" : "⚠️ UNVERIFIED";
}

/** One-line body text (text → text; else a marker; never raw structure or 'undefined').
 *  Unlike channel.mjs, no ⟦⟧ fence-strip: Telegram gets plain text (no parse_mode), not AI context. */
function bodyText(body) {
  if (!body) return "(no content)";
  if (body.type === "text") return body.text != null ? String(body.text) : "(empty message)";
  if (body.type === "unavailable") return "(could not decrypt)";
  return "(non-text message)";
}

/**
 * Build the plain-text Telegram ping for a received message. The caller sends with NO
 * parse_mode, so a hostile body/name cannot inject markup or fake links. The badge is a
 * sender-unreachable PREFIX. `bodyMode` is "full" (default) or "meta".
 * @returns {{title:string, body:string, badge:string}}
 */
export function bridgeFormat(m, { bodyMode = "full" } = {}) {
  const who = m.contact || shortPeer(m.from);
  const badge = badgeFor(m);
  const body = bodyMode === "meta" ? "(open AIR Note to read)" : bodyText(m.body);
  return { title: `${badge} · 📬 ${who}`, body, badge };
}

/** Reply tier from a stored route: verified+pinned → one-tap; else an explicit confirm. */
export function replyTier(route) {
  return route && route.verified ? "one-tap" : "confirm";
}
