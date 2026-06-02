// consumer-lock.mjs — single live-consumer lock for the shared relay pull cursor.
// watch, channel-server, and bridge all advance ONE cursor (archive.mjs pull_cursor),
// so only one may run per identity. This turns "two daemons silently eat each other's
// mail" into a loud, correct error. The lock is a PID file at <home>/consumer.lock (0600).

import { readFileSync, writeFileSync, rmSync, existsSync, chmodSync } from "node:fs";
import { join } from "node:path";
import { bridgeHome } from "./identity.mjs";

const lockPath = (home) => join(home, "consumer.lock");

/** Is a process alive? Signal 0 probes without killing. EPERM = exists but not ours. */
export function isPidAlive(pid, kill = process.kill) {
  if (!pid || pid <= 0) return false;
  try { kill(pid, 0); return true; }
  catch (e) { return e.code === "EPERM"; }
}

/**
 * Acquire the consumer lock. Returns { acquired:true } on success, or
 * { acquired:false, holder } if a LIVE consumer already holds it. A stale lock
 * (dead holder PID) or our own is reclaimed. All deps injectable for tests.
 */
export function acquireConsumerLock({
  name = "consumer", home = bridgeHome(), pid = process.pid, isAlive = isPidAlive,
} = {}) {
  const path = lockPath(home);
  if (existsSync(path)) {
    let holder = null;
    try { holder = JSON.parse(readFileSync(path, "utf8")); } catch { holder = null; }
    if (holder && holder.pid !== pid && isAlive(holder.pid)) {
      return { acquired: false, holder };
    }
  }
  writeFileSync(path, JSON.stringify({ pid, name, since: new Date().toISOString() }), { mode: 0o600 });
  try { chmodSync(path, 0o600); } catch { /* best effort on non-POSIX */ }
  return { acquired: true };
}

/** Release the lock iff we own it. Best-effort, never throws. */
export function releaseConsumerLock({ home = bridgeHome(), pid = process.pid } = {}) {
  const path = lockPath(home);
  try {
    if (!existsSync(path)) return;
    const holder = JSON.parse(readFileSync(path, "utf8"));
    if (holder.pid === pid) rmSync(path, { force: true });
  } catch { /* best effort */ }
}

/** Daemon-entrypoint guard: acquire, or print a clear message + exit(1). Returns acquired? */
export function acquireOrExit(name, {
  home = bridgeHome(), pid = process.pid, isAlive = isPidAlive,
  log = console.error, exit = (n) => process.exit(n),
} = {}) {
  const r = acquireConsumerLock({ name, home, pid, isAlive });
  if (!r.acquired) {
    log(`✗ another live consumer (PID ${r.holder?.pid}, "${r.holder?.name}") holds the relay cursor — stop it first.`);
    exit(1);
    return false;
  }
  return true;
}
