// src/channel-replay.mjs — the channel client's at-least-once recovery (spec §6).
// On {type:"gap", after_seq} the client replays the hole from the LOCAL archive and pushes
// each row through the SAME makeChannelPush pipeline as live frames. The pipeline's own gate
// (channelGate / room gates) re-filters every replayed row — rows carry verified + key_changed,
// and `contact` is re-derived from CURRENT pin state, so replay can never push more than live.
import { replaySince } from "./archive.mjs";
import { getContactByDid } from "./contacts.mjs";
import { isBlocked } from "./moderation.mjs";

/** Map an archive row (parseRow shape) to the live/wire message shape makeChannelPush expects. */
export function rowToMessage(row, { contactLookup = getContactByDid } = {}) {
  const contact = contactLookup(row.from);
  return {
    seq: row.relay_seq,
    relay_seq: row.relay_seq,
    from: row.from,
    ...(contact?.alias ? { contact: contact.alias } : {}),
    envelope_id: row.envelope_id,
    received_at: row.timestamp,
    verified: row.verified,
    encrypted: row.encrypted,
    ...(row.key_changed ? { key_changed: true } : {}),
    ...(row.room_id ? { room_id: row.room_id } : {}),
    body: row.body,
    thread_id: row.thread_id,
  };
}

/** Deduped replay coordinator. live(m) for every streamed frame; gap(after_seq) replays the
 *  hole. A bounded seen-set (envelope_id) prevents double-push where replay and live overlap —
 *  best-effort dedup (critic L2): eviction under sustained back-to-back gaps can in principle
 *  allow a rare double-push, which is harmless under at-least-once semantics. Do not "fix" the
 *  bound into an unbounded set. Blocked senders are skipped (critic H1): live enforces the
 *  blocklist at receive (core.mjs:397) and NO downstream gate rechecks it, so replay must. */
export function makeReplayer({ push, replaySinceFn = replaySince, contactLookup = getContactByDid, isBlockedFn = isBlocked, maxSeen = 1000, pageSize = 500, log = (s) => process.stderr.write(s + "\n") }) {
  const seen = new Set();
  const remember = (id) => {
    seen.add(id);
    if (seen.size > maxSeen) seen.delete(seen.values().next().value);   // FIFO-ish bound
  };
  return {
    live: (m) => {
      if (m?.envelope_id) {
        if (seen.has(m.envelope_id)) return;
        remember(m.envelope_id);
      }
      push(m);
    },
    gap: async (after_seq) => {
      // A daemon-outage window can exceed one page; a silent truncation here would be invisible
      // mail loss — paginate until a short page signals the end of the archive hole.
      let since = after_seq;
      let total = 0;
      while (true) {
        const rows = replaySinceFn(since, { limit: pageSize });
        for (const row of rows) {
          if (isBlockedFn(row.from)) continue;   // critic H1: blocked-after-archive must not replay
          if (seen.has(row.envelope_id)) continue;
          remember(row.envelope_id);
          push(rowToMessage(row, { contactLookup }));
        }
        total += rows.length;
        if (rows.length < pageSize) break;        // short page = end of hole
        since = rows[rows.length - 1].relay_seq;
      }
      log(`[channel] gap after_seq=${after_seq} — replaying ${total} from archive`);
    },
    seenSize: () => seen.size,
  };
}
