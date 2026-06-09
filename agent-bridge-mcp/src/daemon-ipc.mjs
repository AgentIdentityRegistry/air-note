// src/daemon-ipc.mjs — the daemon's local socket layer (spec §5, §7).
// A Unix-domain socket at {AGENT_BRIDGE_HOME}/daemon.sock speaking line-delimited JSON.
// THE DAEMON ENFORCES EACH SUBSCRIBER'S ROLE GATE before writing to that subscriber —
// a client never chooses its own filter (the dumb-fan-out confidentiality hole, spec §5).
import { join } from "node:path";
import { statSync, rmSync } from "node:fs";
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
