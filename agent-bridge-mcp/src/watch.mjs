// watch.mjs — the doorbell loop. Listens to the relay (SSE-first, poll fallback),
// calls the existing core.receive() to actually pull/verify/decrypt/archive, then
// coalesces newly-received messages into subtle notifications. No messaging logic.

import * as coreDefault from "./core.mjs";
import { getCursor as getCursorDefault } from "./archive.mjs";
import { shortPeer } from "./peers.mjs";

/** Render a one-line preview of a message body for a notification. */
function bodyText(body) {
  if (!body) return "(no content)";
  if (body.type === "text") return body.text;
  if (body.type === "unavailable") return "(could not decrypt)";
  return "(message)";
}

/**
 * Group received messages into per-peer notices, dropping muted peers.
 * Pure. `mute` is a Set of alias OR DID OR AIR-id strings.
 * @returns {{peer:string,title:string,message:string,count:number}[]}
 */
export function coalesce(messages, mute = new Set()) {
  const groups = new Map(); // peer did → { title, first, count }
  for (const m of messages) {
    const peer = m.from;
    const alias = m.contact;
    const airId = shortPeer(peer);
    if (mute.has(alias) || mute.has(peer) || mute.has(airId)) continue;
    if (!groups.has(peer)) {
      groups.set(peer, { peer, title: alias || airId, first: bodyText(m.body), count: 0 });
    }
    groups.get(peer).count += 1;
  }
  return [...groups.values()].map((g) => ({
    peer: g.peer,
    title: g.title,
    message: g.count === 1 ? g.first : `${g.count} new messages`,
    count: g.count,
  }));
}

/**
 * Turn a core.receive() result into notifications.
 * Filters to messages not already seen (by envelope_id), coalesces per peer,
 * and dispatches via the injected notifier with a resolved click command.
 * @param {{messages:Array}} result            return value of core.receive()
 * @param {object} deps
 * @param {{notify:Function}} deps.notifier
 * @param {(peer:string,info:object)=>(string[]|null)} deps.openResolver
 * @param {Set<string>} deps.seen              envelope_ids already notified (mutated)
 * @param {Set<string>} deps.mute
 * @param {(m:object)=>void} [deps.onMessage]  optional per-message hook (live feed)
 */
export async function processNewMessages(result, { notifier, openResolver, seen, mute, onMessage }) {
  const fresh = [];
  for (const m of result?.messages ?? []) {
    if (!m || seen.has(m.envelope_id)) continue;
    seen.add(m.envelope_id);
    fresh.push(m);
    onMessage?.(m);
  }
  if (!fresh.length) return;
  for (const notice of coalesce(fresh, mute)) {
    const openCommand = openResolver(notice.peer, { count: notice.count });
    await notifier.notify({ title: notice.title, message: notice.message, openCommand });
  }
}

const sleep = (ms, signal) => new Promise((res) => {
  const t = setTimeout(res, ms);
  signal?.addEventListener("abort", () => { clearTimeout(t); res(); }, { once: true });
});

const MAX_SSE_BUF = 1 << 20; // 1 MiB — a relay that never frames must not grow memory unbounded

/** Read an SSE stream, invoking onEnvelope() whenever an `event: envelope` frame arrives.
 *  Comments (`: ready`, `: heartbeat`) are ignored. Returns true if ANY frame (comment or
 *  envelope) was seen — the caller uses that to decide whether the stream was productive
 *  (a 200-then-immediate-close delivers no frames and must back off, not hammer). Returns
 *  when the stream ends/aborts/overflows. */
async function readSse(response, onEnvelope, signal) {
  const reader = response.body.getReader();
  const dec = new TextDecoder();
  let buf = "";
  let sawFrame = false;
  try {
    while (!signal?.aborted) {
      const { value, done } = await reader.read();
      if (done) break;
      buf += dec.decode(value, { stream: true });
      if (buf.length > MAX_SSE_BUF) break; // misframed/adversarial stream → drop + reconnect
      let idx;
      while ((idx = buf.indexOf("\n\n")) !== -1) {
        const frame = buf.slice(0, idx);
        buf = buf.slice(idx + 2);
        sawFrame = true;
        if (frame.startsWith(":")) continue;          // comment / heartbeat
        if (/(^|\n)event:\s*envelope/.test(frame)) await onEnvelope();
      }
    }
  } finally {
    // Await the cancel: on abort, undici settles the request teardown through this
    // promise. Not awaiting it leaves an AbortError rejection detached → unhandled.
    try { await reader.cancel(); } catch { /* already closed */ }
  }
  return sawFrame;
}

/**
 * The doorbell loop. Runs until `signal` aborts.
 * Injected edges keep it testable: receiveFn, notifier, openResolver, getCursorFn, fetchImpl.
 */
export async function watch({
  signal,
  identity,
  receiveFn = coreDefault.receiveAll, // drain has_more to completion each wake (watch/bridge/channel)
  notifier,
  openResolver,
  getCursorFn = getCursorDefault,
  fetchImpl = fetch,
  intervalMs = Number(process.env.AIRMSG_WATCH_INTERVAL_MS) || 5000,
  coalesceMs = Number(process.env.AIRMSG_COALESCE_MS) || 8000, // interface-only (design §8): bursts coalesce per receive() batch; no timer debounce
  backoffCapMs = 5000,
  mute = new Set((process.env.AIRMSG_MUTE || "").split(",").map((s) => s.trim()).filter(Boolean)),
  onIdle,                       // test hook: called when a poll cycle finds nothing
  onMessage,                    // optional per-message hook (live feed)
  log = (s) => process.stderr.write(s + "\n"),
} = {}) {
  const seen = new Set();
  let inFlight = false;

  // Serialize receive(): readSse awaits each onEnvelope sequentially (so pumps never
  // overlap within a stream); inFlight additionally guards the post-stream catch-up pump.
  // A skipped pump loses nothing — receive() reads from the stored cursor and catches up.
  const pump = async () => {
    if (inFlight || signal?.aborted) return;
    inFlight = true;
    try {
      const result = await receiveFn({});
      await processNewMessages(result, { notifier, openResolver, seen, mute, onMessage });
      if (!(result?.messages?.length) && onIdle) onIdle();
    } catch (err) {
      if (!signal?.aborted) log(`[watch] receive error: ${err.message ?? err}`);
    } finally {
      inFlight = false;
    }
  };

  let backoff = 1000;
  while (!signal?.aborted) {
    let streamed = false;
    try {
      const since = (() => { try { return getCursorFn(); } catch { return 0; } })();
      const url = `${identity.relay_url}/pull/${encodeURIComponent(identity.did)}?stream=1&since=${since}`;
      const resp = await fetchImpl(url, { headers: { accept: "text/event-stream" }, signal });
      if (resp.ok && resp.body) {
        streamed = true;
        // Reset backoff ONLY if the stream actually delivered frames. A relay that
        // returns 200 then closes immediately delivers nothing → keep backing off
        // instead of reconnecting every 1s.
        const sawFrame = await readSse(resp, pump, signal);
        if (sawFrame) backoff = 1000;
      }
    } catch (err) {
      if (signal?.aborted) break; // intentional shutdown — not an error
      log(`[watch] sse error: ${err.message ?? err}`);
    }
    if (signal?.aborted) break;

    // Stream ended/unavailable → one catch-up poll, then back off (and try SSE again).
    await pump();
    if (signal?.aborted) break;
    if (!streamed) {
      await sleep(intervalMs, signal);
    } else {
      await sleep(Math.min(backoff, backoffCapMs), signal);
      backoff = Math.min(backoff * 2, backoffCapMs);
    }
  }
}
