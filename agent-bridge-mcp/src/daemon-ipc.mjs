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

export const socketPath = () => join(bridgeHome(), "daemon.sock");

// Frame ceiling: 1 MiB, matching watch.mjs's MAX_SSE_BUF. Bodies have no upstream size cap,
// so the ceiling must clear any plausible message; an over-ceiling line fails LOUDLY via
// onError (a silent drop here would be invisible mail loss for a channel client).
export const MAX_FRAME = 1 << 20;

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
  log = (s) => process.stderr.write(s + "\n"),
} = {}) {
  const subscribers = new Set();   // { socket, role, lastSeq, dropped, flushPending }

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
        sub = { socket, role: frame.role, lastSeq: null, dropped: 0 };
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
        return;
      }
      if (frame.type === "ping") socket.write(encodeFrame({ type: "pong" }));
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
 *  Reconnect/backoff is Phase 4; Phase 2 surfaces onClose and lets the caller decide. */
export function connectDaemon({ role, onMessage, onClose = () => {}, handshakeMs = 3000, log = (s) => process.stderr.write(s + "\n") }) {
  return new Promise((resolve, reject) => {
    const sock = createConnection(socketPath());
    const fail = (reason, cause) => {
      sock.destroy();
      reject(Object.assign(new Error(reason), { code: "DAEMON_DOWN", cause }));
    };
    const timer = setTimeout(() => fail("daemon handshake timed out"), handshakeMs);
    let ready = false;

    sock.once("error", (e) => { if (!ready) { clearTimeout(timer); fail(`no daemon: ${e.code}`, e); } });
    sock.once("connect", () => sock.write(encodeFrame({ type: "hello", role })));
    const feed = makeLineParser((frame) => {
      if (!ready) {
        clearTimeout(timer);
        if (frame.type === "hello-ok") {
          ready = true;
          log(`[client] attached to air-msgd pid=${frame.pid} as ${role}`);
          resolve({ close: () => sock.destroy() });
        } else {
          fail(`daemon refused: ${frame.reason ?? frame.type}`);
        }
        return;
      }
      if (frame.type === "message") onMessage(frame.message);
      // pong + unknown server frames: ignored.
    }, { onError: (e) => log(`[client] bad frame from daemon: ${e.message}`) });
    sock.on("data", feed);
    sock.on("close", () => { if (ready) onClose(); });
  });
}
