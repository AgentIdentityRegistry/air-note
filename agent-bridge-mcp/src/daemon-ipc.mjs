// src/daemon-ipc.mjs — the daemon's local socket layer (spec §5, §7).
// A Unix-domain socket at {AGENT_BRIDGE_HOME}/daemon.sock speaking line-delimited JSON.
// THE DAEMON ENFORCES EACH SUBSCRIBER'S ROLE GATE before writing to that subscriber —
// a client never chooses its own filter (the dumb-fan-out confidentiality hole, spec §5).
import { join } from "node:path";
import { statSync, rmSync, chmodSync, lstatSync } from "node:fs";
import { createServer, createConnection } from "node:net";
import { bridgeHome } from "./identity.mjs";
import { channelGate, roomChannelGate } from "./channel.mjs";
import { deriveRoom } from "./rooms.mjs";
import { shortPeer } from "./peers.mjs";
import { classifySendError } from "./core.mjs";

export const socketPath = () => join(bridgeHome(), "daemon.sock");

// Frame ceiling: 1 MiB, matching watch.mjs's MAX_SSE_BUF. Bodies have no upstream size cap,
// so the ceiling must clear any plausible message; an over-ceiling line fails LOUDLY via
// onError (a silent drop here would be invisible mail loss for a channel client).
export const MAX_FRAME = 1 << 20;

// send-err reasons ride the wire to GUIs and embed relay-controlled response text, which is
// length-unbounded (a proxy's 502 HTML page, a chatty federated relay). Cap + de-control it at
// the serialization boundary; rendering-side escaping stays the GUI's job.
export const REASON_MAX_CHARS = 512;
const clipReason = (s) => String(s).replace(/[\x00-\x08\x0b-\x1f\x7f]/g, " ").slice(0, REASON_MAX_CHARS);

/** One JSON object per newline-terminated line. */
export function encodeFrame(obj) {
  return JSON.stringify(obj) + "\n";
}

/** Incremental line parser: feed(chunk); emits parsed frames via onFrame.
 *  A malformed line or an over-long line is reported to onError and skipped —
 *  one bad frame must never kill the connection handler loop. */
export function makeLineParser(onFrame, { maxLine = MAX_FRAME, onError = () => {} } = {}) {
  let buf = "";
  return (chunk) => {
    buf += chunk.toString("utf8");
    if (buf.length > maxLine && !buf.includes("\n")) {
      onError(new Error(`line exceeds ${maxLine} bytes`));
      buf = "";
      return;
    }
    let nl;
    while ((nl = buf.indexOf("\n")) !== -1) {
      const line = buf.slice(0, nl);
      buf = buf.slice(nl + 1);
      if (!line.trim()) continue;
      try {
        onFrame(JSON.parse(line));
      } catch (err) {
        onError(err);
      }
    }
  };
}

// Phase 2 socket roles. `bridge` is deliberately ABSENT: the bridge runs as an in-process
// daemon sink (spec §3); a socket bridge role arrives in Phase 4 when it becomes detachable.
export const ROLES = new Set(["channel", "viewer"]);

/** May `m` cross the socket to a subscriber with `role`? (spec §5 — confidentiality boundary.)
 *  channel: the existing channel policy — 1:1 via channelGate, rooms via roomChannelGate.
 *  viewer:  banner-equivalent visibility — mute-only (mirrors bannerSink in daemon-sinks.mjs).
 *  Presentation policy (raise-hand, addressing) stays in the CLIENT (makeChannelPush). */
export function admitForRole(role, m, { mute = new Set() } = {}) {
  if (role === "viewer") {
    return !(mute.has(m?.contact) || mute.has(m?.from) || mute.has(shortPeer(m?.from)));
  }
  if (role === "channel") {
    return m?.room_id ? roomChannelGate(m, deriveRoom(m.room_id), mute) : channelGate(m, mute);
  }
  return false;
}

/** Refuse to operate out of a home dir that group/other can write: anyone who can write
 *  the dir can swap the socket (spec §5). A shared-path home is unsupported. */
export function assertSafeHome(home = bridgeHome()) {
  const mode = statSync(home).mode & 0o777;
  if (mode & 0o022) {
    throw new Error(`refusing socket in group/other-writable home ${home} (mode ${mode.toString(8)})`);
  }
}

/** Remove a stale socket file before bind. ONLY safe because the caller already holds the
 *  consumer lock — the lock is the single-daemon mutex, so nothing live owns this path. */
export function prepareSocketPath() {
  try { rmSync(socketPath(), { force: true }); } catch { /* best effort */ }
}

/** §7 stale-socket hygiene for LEGACY entrypoints: after a standalone consumer ACQUIRED the
 *  consumer lock (the lock proves no live daemon owns the path), remove any leftover socket so
 *  later probes fail fast with ENOENT instead of ECONNREFUSED. Never call without the lock. */
export function cleanStaleSocket() {
  prepareSocketPath();   // byte-identical rm — single-source the idiom; distinct doc contracts kept
}

/** The daemon's socket server. Returned `sink` plugs into fanOut ({name, deliver}).
 *  Each subscriber declared a role at hello; deliver() applies admitForRole per subscriber
 *  BEFORE writing — the daemon enforces, the client never chooses (spec §5).
 *
 *  Flow control (spec §6): a subscriber whose unflushed node-side queue (`socket.writableLength`)
 *  exceeds `highWaterMark` has writes SKIPPED with a per-subscriber count; on progress (drain or
 *  a successful write back below the threshold) `channel` gets ONE gap frame (its cue to replay
 *  from the archive) while viewer/bridge just log the drop count (best-effort roles). A queue
 *  past 4×highWaterMark means truly wedged — that client is destroyed (the absolute backstop:
 *  an always-on daemon must never let one wedged local client balloon its memory). */
export function createIpcServer({
  mute = new Set(),
  daemonInfo = {},
  highWaterMark = 1 << 20,                    // 1 MiB queued per client, matches MAX_FRAME
  helloTimeoutMs = 5000,                      // pre-hello idlers are reaped (fd-pin guard)
  statusExtraFn = () => ({}),
  sendFn = null,                              // injected by startDaemon (core.send); null → send unavailable
  log = (s) => process.stderr.write(s + "\n"),
} = {}) {
  const subscribers = new Set();   // { socket, role, lastSeq, dropped, flushPending }
  let lastDeliveredSeq = null;

  // Socket-stream HWM = policy HWM: 'drain' only fires after a write() returned FALSE, and
  // write() only returns false once writableLength crosses the STREAM's writableHighWaterMark
  // (default 16 KiB) — but the skip band stops writing at the POLICY threshold first. Unless
  // the two thresholds coincide, the crossing write never returns false, drain is never armed,
  // and a fully-drained client would wait forever for its gap frame.
  const server = createServer({ highWaterMark }, (socket) => {
    let sub = null;
    const reaper = setTimeout(() => { if (!sub) socket.destroy(); }, helloTimeoutMs);
    const feed = makeLineParser((frame) => {
      if (!sub) {
        if (frame.type !== "hello" || !ROLES.has(frame.role)) {
          socket.write(encodeFrame({ type: "error", reason: "first frame must be hello with role channel|viewer" }));
          socket.destroy();
          return;
        }
        clearTimeout(reaper);
        // Hoist ONE constant so seed-and-gap share a value AND the seed is channel-only:
        // a viewer's reported lastSeq must never reflect a seq that was never delivered to it
        // (since_seq is ignored for best-effort roles; the constant guards both the seed and the gap).
        const resumeSeq = frame.role === "channel" && Number.isFinite(frame.since_seq) ? frame.since_seq : null;
        sub = { socket, role: frame.role, lastSeq: resumeSeq, dropped: 0 };
        subscribers.add(sub);
        socket.write(encodeFrame({ type: "hello-ok", ...daemonInfo }));
        log(`[daemon] client attached: role=${frame.role} (${subscribers.size} connected)`);
        // C1 (critic, empirically reproduced): 'drain' fires only after a write() returned
        // false AND the buffer empties to ZERO — a slow-but-steady reader can keep the queue
        // non-empty forever while our skip-pattern suppresses write(), starving the gap.
        // Therefore: flush pending drops on ANY progress (drain OR a successful write below
        // the threshold), never on drain alone.
        const flushPending = () => {
          if (!sub || sub.dropped === 0) return;
          if (sub.socket.writableLength > highWaterMark) return;   // still congested — wait for progress
          if (sub.role === "channel") {
            socket.write(encodeFrame({ type: "gap", after_seq: sub.lastSeq ?? 0 }));
            log(`[daemon] gap → channel client (skipped ${sub.dropped}, after_seq=${sub.lastSeq ?? 0})`);
          } else {
            log(`[daemon] dropped ${sub.dropped} msgs to ${sub.role} client (slow consumer)`);
          }
          sub.dropped = 0;
        };
        sub.flushPending = flushPending;
        socket.on("drain", flushPending);
        // Resume-on-reattach (spec §6 "OR reconnect"): a channel client that declares where it
        // left off gets an immediate gap; its Phase-3 replayer fills the hole from the archive.
        // The daemon stays stateless about client history — the client's archive is the source.
        // Viewer/bridge are best-effort roles: since_seq is ignored for them.
        if (resumeSeq !== null) {
          socket.write(encodeFrame({ type: "gap", after_seq: resumeSeq }));
          log(`[daemon] resume: gap → channel client (after_seq=${resumeSeq})`);
        }
        return;
      }
      if (frame.type === "ping") socket.write(encodeFrame({ type: "pong" }));
      // Status probes appear in the attach/detach log lines — accepted noise for interactive use;
      // revisit (e.g. a probe:true hello hint) only if status polling becomes a pattern.
      if (frame.type === "status") {
        socket.write(encodeFrame({
          type: "status",
          socket: socketPath(),
          last_seq: lastDeliveredSeq,
          // Exclude the ASKING subscriber: `daemon status` attaches a throwaway viewer probe, and
          // counting it would make an idle daemon always report one client (critic v1 M3).
          clients: [...subscribers].filter((s) => s !== sub).map((s) => ({ role: s.role, lastSeq: s.lastSeq, dropped: s.dropped })),
          ...statusExtraFn(),
        }));
      }
      if (frame.type === "send") {
        // Send-over-socket (AI-inbox design §3): the daemon is the only key-holder and the only
        // archive writer, so surfaces send THROUGH it. Role-agnostic by design — roles are
        // DELIVERY filters; the 0600 socket is the OS user boundary (any local process could
        // already run `air-msg send`). No correlation id → no ack (we cannot address one).
        if (frame.id === undefined) return;
        if (!frame.to || frame.body === undefined || frame.body === null) {
          socket.write(encodeFrame({ type: "send-err", id: frame.id, retryable: false, reason: "send requires to + body" }));
          return;
        }
        if (!sendFn) {
          socket.write(encodeFrame({ type: "send-err", id: frame.id, retryable: false, reason: "send unavailable (daemon started without a send function)" }));
          return;
        }
        // Async on purpose: the ack arrives whenever core.send settles; the parser loop never
        // blocks. Guard the write — the requester may have disconnected while the relay was slow.
        // plaintext is optional wire-side and defaults false (CLI --plaintext parity; the desktop
        // always sends encrypted — the field exists for tests and tooling).
        sendFn({ to: frame.to, body: frame.body, plaintext: frame.plaintext === true })
          .then((r) => {
            // destroyed-guard is defensive — a write to a destroyed socket is benign on current Node (probed); the .catch below is the load-bearing guard.
            if (socket.destroyed) return;
            socket.write(encodeFrame({ type: "send-ok", id: frame.id, envelope_id: r.envelope_id, encrypted: r.encrypted ?? true }));
          })
          .catch((err) => {
            if (socket.destroyed) return;
            const verdict = classifySendError(err);
            const reason = clipReason(verdict.reason);
            log(`[daemon] send failed (${verdict.retryable ? "retryable" : "terminal"}): ${reason}`);
            socket.write(encodeFrame({ type: "send-err", id: frame.id, retryable: verdict.retryable, reason }));
          });
        return;
      }
      // Duplicate hello and unknown frames from a subscribed client: ignored (forward compat).
    }, { onError: () => { socket.write(encodeFrame({ type: "error", reason: "bad frame" })); socket.destroy(); } });

    socket.on("data", feed);
    const drop = () => {
      clearTimeout(reaper);
      if (sub) { subscribers.delete(sub); log(`[daemon] client detached (${subscribers.size} connected)`); sub = null; }
    };
    socket.on("close", drop);
    socket.on("error", drop);
  });

  return {
    /** fanOut-compatible sink: write `m` to every subscriber whose role admits it. */
    sink: {
      name: "socket",
      deliver: (m) => {
        // Stamp relay_seq at the boundary (Phase 2): onMessage objects carry `seq`.
        const wire = m && m.seq !== undefined && m.relay_seq === undefined ? { ...m, relay_seq: m.seq } : m;
        // "Last seq FANNED OUT" (advances even if every subscriber skipped/destroyed) — the per-client
        // lastSeq/dropped in the status reply are the per-write markers; both together make a wedged client diagnosable.
        if (wire && Number.isFinite(wire.relay_seq)) lastDeliveredSeq = wire.relay_seq;
        for (const sub of subscribers) {
          if (!admitForRole(sub.role, m, { mute })) continue;
          const queued = sub.socket.writableLength;
          if (queued > highWaterMark * 4) {
            // Truly wedged: protect the daemon (absolute backstop; spec §6 floor).
            log(`[daemon] destroying wedged ${sub.role} client (writableLength=${queued})`);
            sub.socket.destroy();
            continue;
          }
          if (queued > highWaterMark) {
            // Soft overflow: skip this write. channel recovers via gap+replay on progress;
            // viewer/bridge are best-effort (drop + count) per spec §6.
            sub.dropped += 1;
            continue;
          }
          sub.socket.write(encodeFrame({ type: "message", message: wire }));
          if (wire && wire.relay_seq !== undefined) sub.lastSeq = wire.relay_seq;
          // INVARIANT (critic M1): every channel-admissible message carries seq (receive()
          // always sets it), so lastSeq is never null once a real write happened — a gap's
          // after_seq=0 full-window replay can only occur before ANY successful write (bounded
          // by replaySince's limit).
          sub.flushPending?.();   // C1: emit any pending gap now that we made progress
        }
      },
    },
    clientCount: () => subscribers.size,
    clientStats: () => [...subscribers].map((s) => ({ role: s.role, dropped: s.dropped, lastSeq: s.lastSeq })),   // test seam
    listen: async () => {
      assertSafeHome();
      prepareSocketPath();                  // caller (startDaemon) holds the consumer lock
      await new Promise((resolve, reject) => {
        server.once("error", reject);
        server.listen(socketPath(), resolve);
      });
      chmodSync(socketPath(), 0o600);
      const st = lstatSync(socketPath());   // re-stat after listen: TOCTOU bind-hijack guard (spec §5)
      if (!st.isSocket() || (st.mode & 0o777) !== 0o600 || (process.getuid && st.uid !== process.getuid())) {
        await new Promise((res) => server.close(res));
        throw new Error("socket failed post-listen owner/mode verification");
      }
    },
    close: async () => {
      for (const sub of subscribers) sub.socket.destroy();
      subscribers.clear();
      await new Promise((res) => server.close(res));
      try { rmSync(socketPath(), { force: true }); } catch { /* best effort */ }
    },
  };
}

/** Connect to the local daemon socket as `role`. Resolves AFTER hello-ok with a {close()}
 *  handle; gated messages stream to onMessage(m). Rejects with {code:"DAEMON_DOWN"} when no
 *  daemon is reachable (callers use that to fall back to legacy standalone — spec §7).
 *  `sinceSeq`: pass the max-seen relay_seq; the daemon answers a channel hello bearing it with
 *  an immediate gap into `onGap` so the Phase-3 replayer can fill the outage window.
 *  For persistent reconnect with backoff, use `connectDaemonPersistent` instead. */
export function connectDaemon({ role, onMessage, onGap = () => {}, onClose = () => {}, onStatus = () => {}, sinceSeq, handshakeMs = 3000, log = (s) => process.stderr.write(s + "\n") }) {
  return new Promise((resolve, reject) => {
    const sock = createConnection(socketPath());
    const fail = (reason, cause) => {
      sock.destroy();
      reject(Object.assign(new Error(reason), { code: "DAEMON_DOWN", cause }));
    };
    const timer = setTimeout(() => fail("daemon handshake timed out"), handshakeMs);
    let ready = false;

    sock.once("error", (e) => { if (!ready) { clearTimeout(timer); fail(`no daemon: ${e.code}`, e); } });
    sock.once("connect", () => sock.write(encodeFrame({
      type: "hello", role, ...(Number.isFinite(sinceSeq) ? { since_seq: sinceSeq } : {}),
    })));
    const feed = makeLineParser((frame) => {
      if (!ready) {
        clearTimeout(timer);
        if (frame.type === "hello-ok") {
          ready = true;
          log(`[client] attached to air-msgd pid=${frame.pid} as ${role}`);
          resolve({ close: () => sock.destroy(), _sock: sock });   // _sock: same-module use (queryDaemonStatus) + test seam (gap round-trip); not for external callers
        } else {
          fail(`daemon refused: ${frame.reason ?? frame.type}`);
        }
        return;
      }
      if (frame.type === "message") onMessage(frame.message);
      if (frame.type === "gap") onGap(frame.after_seq);
      if (frame.type === "status") onStatus(frame);
      // pong + unknown server frames: ignored.
    }, { onError: (e) => log(`[client] bad frame from daemon: ${e.message}`) });
    sock.on("data", feed);
    sock.on("close", () => { if (ready) onClose(); });
  });
}

/** Persistent daemon client (spec §7 reconnect contract): wraps connectDaemon with auto-reconnect
 *  + exponential backoff (reset on a successful attach), and resume-on-reattach for `channel`.
 *
 *  Resume bookkeeping: tracks the max relay_seq seen on message frames; every REconnect sends
 *  since_seq = maxSeen ?? baseline, where baseline = cursorFn() snapshotted at the FIRST attach.
 *  The baseline closes the outage-window hole: a client that saw no frames before the daemon
 *  bounced would otherwise reattach live-from-attach and silently miss mail the restarted daemon
 *  pulled in between. cursorFn is injected (archive stays out of this transport module); the
 *  FIRST attach deliberately sends NO since_seq — a fresh session is live-from-attach (spec §4).
 *
 *  The returned promise settles like connectDaemon: rejects DAEMON_DOWN if the FIRST attempt
 *  finds no daemon (callers use that to fall back to legacy standalone, spec §7); after the
 *  first successful attach it never gives up until close().
 *
 *  Callbacks (onAttach, onDetach, onMessage, onGap) are invoked from socket event handlers and
 *  the reconnect loop; they are guarded against throws on the reconnect path (the first-attach
 *  onAttach in the .then() is unguarded — callers SHOULD NOT throw from onAttach). During a
 *  backoff window the wrapper holds only an unref'd timer, so a process whose ONLY live handle
 *  is this client may exit mid-outage — callers must hold another handle (stdio transport,
 *  stdin, a server) to outlive outages. */
export function connectDaemonPersistent({
  role, onMessage, onGap = () => {}, onAttach = () => {}, onDetach = () => {},
  cursorFn = () => null,
  initialBackoffMs = 500, backoffCapMs = 5000,
  connectFn = connectDaemon,                            // test seam
  log = (s) => process.stderr.write(s + "\n"),
}) {
  let stopped = false;
  let current = null;
  let maxSeen = null;
  // Baseline BEFORE the first connect (critic v1 M1, probed): a frame can sit unparsed in this
  // client's socket buffer while the daemon advances the cursor past it — a post-attach snapshot
  // can then skip that seq on resume (replaySince is strictly-greater-than). An early, over-broad
  // baseline only costs a few envelope_id-deduped replays; a late one loses mail.
  let baseline = null;
  try { baseline = cursorFn(); } catch { baseline = null; }
  const seen = (m) => {
    if (m && Number.isFinite(m.relay_seq) && (maxSeen === null || m.relay_seq > maxSeen)) maxSeen = m.relay_seq;
    onMessage(m);
  };

  const reconnectLoop = async () => {
    let backoff = initialBackoffMs;
    while (!stopped) {
      // Sleep BEFORE the first retry on purpose: onClose means the daemon is gone RIGHT NOW —
      // an immediate attempt would just burn an ECONNREFUSED. unref: a closed handle must not
      // pin the event loop for a pending backoff tick.
      await new Promise((r) => { const t = setTimeout(r, backoff); t.unref?.(); });
      if (stopped) return;
      const sinceSeq = maxSeen ?? baseline ?? undefined;
      let next;
      try {
        next = await connectFn({
          role, sinceSeq, onMessage: seen, onGap,
          onClose: () => { if (!stopped) { try { onDetach(); } catch (e) { log(`[client] onDetach callback threw: ${e?.message ?? e}`); } void reconnectLoop(); } },
          log,
        });
      } catch {
        backoff = Math.min(backoff * 2, backoffCapMs);  // daemon still down — keep trying
        continue;
      }
      if (stopped) { next.close(); return; }  // close() raced the in-flight handshake — kill the undead socket
      current = next;
      // User callbacks OUTSIDE the try: a throwing onAttach must not masquerade as a failed
      // connect (it would silently pile up duplicate live connections). Loud log, no retry.
      try { onAttach(); } catch (e) { log(`[client] onAttach callback threw: ${e?.message ?? e}`); }
      log(`[client] reattached to air-msgd as ${role}${Number.isFinite(sinceSeq) ? ` (resume since_seq=${sinceSeq})` : ""}`);
      return;
    }
  };

  return connectFn({
    role, onMessage: seen, onGap,
    onClose: () => { if (!stopped) { try { onDetach(); } catch (e) { log(`[client] onDetach callback threw: ${e?.message ?? e}`); } void reconnectLoop(); } },
    log,
  }).then((first) => {
    current = first;
    onAttach();
    return {
      // A deliberate close() is SILENT (no onDetach): `stopped` flips first, so the socket's
      // close event takes the if(!stopped) branch. Don't "fix" this into a spurious detach log.
      close: () => { stopped = true; current?.close(); },
      _sock: () => current?._sock,                      // test seam (backstop-recovery round-trip)
    };
  });
}

/** One-shot liveness probe (§7 decision table): is a daemon answering the socket RIGHT NOW?
 *  Attaches as a throwaway viewer and detaches immediately — the socket answering hello is the
 *  truth the table keys on (a PID file can outlive a crashed daemon; ECONNREFUSED cannot lie). */
export async function probeDaemon({ timeoutMs = 1500 } = {}) {
  try {
    const h = await connectDaemon({ role: "viewer", onMessage: () => {}, handshakeMs: timeoutMs, log: () => {} });
    h.close();
    return true;
  } catch {
    return false;
  }
}

/** One-shot live status query for `air-msg daemon status` (spec §8): the CLI runs in a separate
 *  process, so connected-clients/last_seq state must cross the socket. Returns null if no daemon. */
export async function queryDaemonStatus({ timeoutMs = 1500, connectFn = connectDaemon } = {}) {
  return new Promise((resolve) => {
    let handle = null;
    let done = false;
    const finish = (v) => { if (!done) { done = true; handle?.close(); resolve(v); } };
    const timer = setTimeout(() => finish(null), timeoutMs);
    connectFn({
      role: "viewer",
      onMessage: () => {},                             // live frames during the query: ignored
      onStatus: (st) => { clearTimeout(timer); finish(st); },
      handshakeMs: timeoutMs,
      log: () => {},
    }).then((h) => { if (done) { h.close(); return; } handle = h; h._sock.write(encodeFrame({ type: "status" })); })
      .catch(() => { clearTimeout(timer); finish(null); });
  });
}
