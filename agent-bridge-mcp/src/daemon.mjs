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
