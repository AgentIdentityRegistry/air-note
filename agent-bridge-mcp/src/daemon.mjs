// src/daemon.mjs — the always-on receiver daemon. Owns the single consumer lock, runs ONE
// watch() loop, and fans every received message out to its in-process sinks. The Phase 2 socket
// layer attaches additional dynamic sinks to the same fanOut; this phase wires in-process only.
import { watch } from "./watch.mjs";
import { fanOut } from "./fanout.mjs";
import { readFileSync, writeFileSync, existsSync, rmSync } from "node:fs";
import { join } from "node:path";
import { bridgeHome } from "./identity.mjs";
import { isPidAlive } from "./consumer-lock.mjs";

/** Run the daemon: drive watch() with an onMessage that fans out to `sinks`. Injectable for tests. */
export async function runDaemon({ identity, sinks, signal, watchFn = watch, log = (s) => process.stderr.write(s + "\n") }) {
  log(`[daemon] up: ${identity.did} · sinks: ${sinks.map((s) => s.name).join(", ") || "(none)"}`);
  await watchFn({
    signal,
    identity,
    notifier: { notify: async () => {} }, // the banner is a SINK now, not watch's own notifier
    openResolver: () => null,
    onMessage: (m) => fanOut(m, sinks, log),
  });
}

const pidPath = () => join(bridgeHome(), "daemon.pid");

/** Write the daemon PID record. `start_time` (an ISO string the daemon stamps at boot) lets a
 *  reader distinguish a live daemon from an unrelated process that recycled the same PID. */
export function writeDaemonPid({ pid = process.pid, startTime } = {}) {
  writeFileSync(pidPath(), JSON.stringify({ pid, start_time: startTime ?? null }), { mode: 0o600 });
}

/** Read the daemon PID record, or null if absent/corrupt. */
export function readDaemonPid() {
  const p = pidPath();
  if (!existsSync(p)) return null;
  try { return JSON.parse(readFileSync(p, "utf8")); } catch { return null; }
}

/** Is a daemon currently running? (PID file present AND that PID alive.) Inject isAlive for tests. */
export function isDaemonRunning(isAlive = isPidAlive) {
  const rec = readDaemonPid();
  return !!rec && isAlive(rec.pid);
}

/** Remove the PID file (clean shutdown). Best-effort; never throws. */
export function clearDaemonPid() {
  try { rmSync(pidPath(), { force: true }); } catch { /* best effort */ }
}

import { getCursor } from "./archive.mjs";

/** Structured daemon status for `air-msg daemon status` (spec §8). Cursor is best-effort. */
export function daemonStatus(isAlive = isPidAlive) {
  const rec = readDaemonPid();
  let cursor = null;
  try { cursor = getCursor(); } catch { cursor = null; }
  return {
    running: !!rec && isAlive(rec.pid),
    pid: rec?.pid ?? null,
    start_time: rec?.start_time ?? null,
    cursor,
  };
}

import { ensureIdentity } from "./identity.mjs";
import { acquireOrExit, releaseConsumerLock } from "./consumer-lock.mjs";
import { createNotifier } from "./notifier.mjs";
import { bannerSink } from "./daemon-sinks.mjs";

/** Foreground daemon entrypoint: take the lock, build sinks, run until SIGINT/SIGTERM. */
export async function startDaemon({ log = (s) => process.stderr.write(s + "\n") } = {}) {
  const identity = await ensureIdentity();
  if (!acquireOrExit("daemon")) return;            // another live consumer holds the cursor
  const startTime = new Date().toISOString();
  writeDaemonPid({ pid: process.pid, startTime });

  const mute = new Set((process.env.AIRMSG_MUTE || "").split(",").map((s) => s.trim()).filter(Boolean));
  const notifier = await createNotifier({ onClick: () => {} });
  const sinks = [bannerSink({ notifier, mute })];  // Phase 1: banner only

  const ac = new AbortController();
  const stop = () => ac.abort();
  process.once("SIGINT", stop);
  process.once("SIGTERM", stop);
  try {
    await runDaemon({ identity, sinks, signal: ac.signal, log });
  } finally {
    clearDaemonPid();
    releaseConsumerLock();
  }
}
