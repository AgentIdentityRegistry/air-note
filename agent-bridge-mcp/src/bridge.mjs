// bridge.mjs — chat-app bridge orchestration + pure helpers. Forwards incoming AIR Note
// mail to an external chat adapter and routes the user's in-app replies back as real
// (signed + encrypted) AIR Notes. No messaging/crypto logic of its own — it drives
// core.send + the adapter, the #29 sibling-consumer pattern.

import { putRoute, getRoute } from "./bridge-routes.mjs";
import { shortPeer } from "./peers.mjs";

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

/** Build the watch() onMessage(m) hook: format → adapter.send → store the route.
 *  Detached-promise so a slow/failed Telegram send never crashes the watch loop. */
export function makeBridgeOutbound({
  adapter, bodyMode = "full", now = () => Date.now(),
  putRouteFn = putRoute, log = (s) => process.stderr.write(s + "\n"),
}) {
  return (m) => {
    Promise.resolve().then(async () => {
      const externalId = await adapter.send(bridgeFormat(m, { bodyMode }));
      if (!externalId) return; // send degraded → nothing to route a reply back to
      putRouteFn({
        platform: adapter.name, external_id: externalId,
        peer_did: m.from, contact: m.contact ?? null,
        thread_id: m.thread_id ?? null, envelope_id: m.envelope_id ?? null,
        // one-tap reply requires verified + key-unchanged + PINNED (has a contact alias);
        // an unpinned-but-signature-verified sender falls to the /yes confirm tier.
        verified: !!(m.verified && !m.key_changed && m.contact), created_at: now(),
      });
    }).catch((err) => log(`[bridge] outbound failed: ${err.message ?? err}`));
  };
}

/** In-memory pending-reply store for UNVERIFIED senders (held until /yes, short TTL). */
export function makeConfirmStore({ ttlMs = 120_000, now = () => Date.now() } = {}) {
  const pending = new Map(); // externalId → { text, expiresAt }
  return {
    put(externalId, text) { pending.set(externalId, { text, expiresAt: now() + ttlMs }); },
    get(externalId) {
      const e = pending.get(externalId);
      if (!e || e.expiresAt < now()) { pending.delete(externalId); return null; }
      return e.text;
    },
    clear(externalId) { pending.delete(externalId); },
    purge() {
      const t = now();
      let removed = 0;
      for (const [k, e] of pending) if (e.expiresAt < t) { pending.delete(k); removed++; }
      return removed;
    },
  };
}

/** Short AIR-id (or DID) for an ack line. */
function destLabel(route) { return route.contact || shortPeer(route.peer_did); }

/** Build the adapter onReply handler: route lookup → reply-safety tier → core.send → ack.
 *  Throws if sendFn throws, so the adapter leaves the update un-acked (at-least-once). */
export function makeReplyHandler({ sendFn, getRouteFn = getRoute, confirm, platform = "telegram" }) {
  return async ({ replyToExternalId, text, reply }) => {
    confirm.purge();
    if (!replyToExternalId) {
      await reply("↩️ Reply to a specific message so I know who to send it to.");
      return;
    }
    const route = getRouteFn({ platform, external_id: replyToExternalId });
    if (!route) {
      await reply("That conversation is too old to reply to here — open AIR Note to reply.");
      return;
    }
    const isYes = text.trim() === "/yes";

    if (replyTier(route) === "confirm") {
      if (!isYes) {
        confirm.put(replyToExternalId, text);
        await reply("⚠️ This sender is UNVERIFIED. Reply /yes (to this message) within 2 min to send anyway.");
        return;
      }
      const held = confirm.get(replyToExternalId);
      if (held == null) { await reply("Nothing pending to confirm (it may have expired). Send your reply again."); return; }
      await sendFn({ to: route.peer_did, body: held, thread_id: route.thread_id, in_reply_to: route.envelope_id });
      confirm.clear(replyToExternalId);
      await reply(`✓ sent to ${destLabel(route)}`);
      return;
    }

    // verified + pinned → one-tap (a literal "/yes" here is just text)
    await sendFn({ to: route.peer_did, body: text, thread_id: route.thread_id, in_reply_to: route.envelope_id });
    await reply(`✓ sent to ${destLabel(route)}`);
  };
}
