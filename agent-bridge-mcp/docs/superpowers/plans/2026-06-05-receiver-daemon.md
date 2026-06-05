# Receiver Daemon — Implementation Plan (Phase 1: daemon core + in-process fan-out)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the always-on receiver daemon's core — one process that owns the single relay consumer lock, runs `watch()` once, and fans every received message out to a list of in-process sinks (starting with the OS banner) — plus `air-msg daemon start|stop|status`.

**Architecture:** `watch()` is already a one-subscriber pub/sub engine (its `onMessage` hook). The daemon drives `watch()` with `onMessage = (m) => fanOut(m, sinks)`, where `fanOut` delivers each message to every registered sink in isolation. Phase 1 wires only in-process sinks (banner); the Phase 2 socket layer attaches dynamic sinks to the same `fanOut`. Spec: `docs/superpowers/specs/2026-06-05-receiver-daemon-design.md` (§3 components, §4 data flow). The `has_more` pagination prerequisite (§2) is **DONE** (merged `cce1002`).

**Tech Stack:** Node ≥22 ESM, `node:test`, `node:fs`, the existing `watch.mjs` / `notifier.mjs` / `consumer-lock.mjs` / `archive.mjs` / `peers.mjs`.

**Test runner note:** on Node 25 `node --test test/` is broken — run single files: `node --test test/<file>.test.mjs`.

---

### Task 1: Fan-out hub

**Files:**
- Create: `agent-bridge-mcp/src/fanout.mjs`
- Test: `agent-bridge-mcp/test/fanout.test.mjs`

- [ ] **Step 1: Write the failing test**

```js
// test/fanout.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import { fanOut } from "../src/fanout.mjs";

test("fanOut delivers the message to every sink", async () => {
  const seen = [];
  const sinks = [
    { name: "a", deliver: (m) => seen.push(`a:${m.id}`) },
    { name: "b", deliver: (m) => seen.push(`b:${m.id}`) },
  ];
  await fanOut({ id: "m1" }, sinks);
  assert.deepEqual(seen.sort(), ["a:m1", "b:m1"]);
});

test("fanOut isolates a throwing sink — others still receive + it is logged", async () => {
  const seen = [];
  const logs = [];
  const sinks = [
    { name: "bad", deliver: () => { throw new Error("boom"); } },
    { name: "good", deliver: (m) => seen.push(m.id) },
  ];
  await fanOut({ id: "m1" }, sinks, (s) => logs.push(s));
  assert.deepEqual(seen, ["m1"]);
  assert.equal(logs.length, 1);
  assert.match(logs[0], /sink "bad" failed: boom/);
});

test("fanOut on no sinks is a no-op", async () => {
  await fanOut({ id: "m1" }, undefined); // must not throw
  await fanOut({ id: "m1" }, []);
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd agent-bridge-mcp && node --test test/fanout.test.mjs`
Expected: FAIL — `Cannot find module '../src/fanout.mjs'`.

- [ ] **Step 3: Write minimal implementation**

```js
// src/fanout.mjs — deliver one received message to every daemon sink, in isolation.
// A sink is { name: string, deliver: (message) => void | Promise<void> }. One sink that throws
// or rejects must never block the others or bubble into the daemon's single receive loop, so each
// deliver() is wrapped and the fan-out runs them concurrently.

/** Deliver `message` to every sink, isolating per-sink failures (logged, never thrown). */
export async function fanOut(message, sinks, log = (s) => process.stderr.write(s + "\n")) {
  await Promise.all((sinks ?? []).map(async (sink) => {
    try {
      await sink.deliver(message);
    } catch (err) {
      log(`[daemon] sink "${sink.name}" failed: ${err?.message ?? err}`);
    }
  }));
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd agent-bridge-mcp && node --test test/fanout.test.mjs`
Expected: PASS — 3 tests.

- [ ] **Step 5: Commit**

```bash
git add agent-bridge-mcp/src/fanout.mjs agent-bridge-mcp/test/fanout.test.mjs
git commit -m "feat(daemon): fan-out hub — deliver one message to N isolated sinks"
```

---

### Task 2: Banner sink

**Files:**
- Create: `agent-bridge-mcp/src/daemon-sinks.mjs`
- Test: `agent-bridge-mcp/test/daemon-sinks.test.mjs`

- [ ] **Step 1: Write the failing test**

```js
// test/daemon-sinks.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import { bannerSink } from "../src/daemon-sinks.mjs";

test("bannerSink rings the notifier with the sender alias + text body", async () => {
  const calls = [];
  const sink = bannerSink({ notifier: { notify: async (n) => calls.push(n) } });
  await sink.deliver({ from: "did:wba:agentidentityregistry.org:agents:AIR-XY", contact: "kenny", body: { type: "text", text: "hi" } });
  assert.equal(sink.name, "banner");
  assert.equal(calls.length, 1);
  assert.equal(calls[0].title, "kenny");
  assert.equal(calls[0].message, "hi");
});

test("bannerSink drops a muted peer (matched by alias)", async () => {
  const calls = [];
  const sink = bannerSink({ notifier: { notify: async (n) => calls.push(n) }, mute: new Set(["kenny"]) });
  await sink.deliver({ from: "did:x:AIR-XY", contact: "kenny", body: { type: "text", text: "hi" } });
  assert.equal(calls.length, 0);
});

test("bannerSink falls back to a short AIR-id when there is no alias", async () => {
  const calls = [];
  const sink = bannerSink({ notifier: { notify: async (n) => calls.push(n) } });
  await sink.deliver({ from: "did:wba:agentidentityregistry.org:agents:AIR-3C33-M64E-KQKJ", body: { type: "text", text: "yo" } });
  assert.match(calls[0].title, /AIR-3C33/);
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd agent-bridge-mcp && node --test test/daemon-sinks.test.mjs`
Expected: FAIL — `Cannot find module '../src/daemon-sinks.mjs'`.

- [ ] **Step 3: Write minimal implementation**

```js
// src/daemon-sinks.mjs — in-process sinks for the receiver daemon. A sink wraps an existing
// delivery surface (OS banner, …) behind the { name, deliver(message) } contract fanOut expects.
import { shortPeer } from "./peers.mjs";

/** OS-banner sink: ring the local notifier per received message, honoring a mute set
 *  (alias OR DID OR short AIR-id). `notifier` is the object from createNotifier(). */
export function bannerSink({ notifier, mute = new Set(), openResolver = () => null } = {}) {
  return {
    name: "banner",
    deliver: async (m) => {
      const alias = m.contact;
      const airId = shortPeer(m.from);
      if (mute.has(alias) || mute.has(m.from) || mute.has(airId)) return;
      const body = m.body?.type === "text" ? m.body.text : "(message)";
      await notifier.notify({ title: alias || airId, message: body, openCommand: openResolver(m.from, {}) });
    },
  };
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd agent-bridge-mcp && node --test test/daemon-sinks.test.mjs`
Expected: PASS — 3 tests. (If `shortPeer`'s output format differs, adjust the `/AIR-3C33/` match to what `peers.mjs` returns — read `src/peers.mjs` first.)

- [ ] **Step 5: Commit**

```bash
git add agent-bridge-mcp/src/daemon-sinks.mjs agent-bridge-mcp/test/daemon-sinks.test.mjs
git commit -m "feat(daemon): banner sink (OS notifier, mute-aware)"
```

---

### Task 3: Daemon assembly (drive watch → fan-out)

**Files:**
- Create: `agent-bridge-mcp/src/daemon.mjs`
- Test: `agent-bridge-mcp/test/daemon.test.mjs`

- [ ] **Step 1: Write the failing test**

```js
// test/daemon.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import { runDaemon } from "../src/daemon.mjs";

test("runDaemon fans a received message out to every sink", async () => {
  const delivered = [];
  const sinks = [
    { name: "x", deliver: (m) => delivered.push(`x:${m.id}`) },
    { name: "y", deliver: (m) => delivered.push(`y:${m.id}`) },
  ];
  // fake watch() that fires one message through onMessage, then returns
  const watchFn = async ({ onMessage }) => { await onMessage({ id: "m1" }); };
  await runDaemon({ identity: { did: "did:x:AIR-ME" }, sinks, watchFn, log: () => {} });
  assert.deepEqual(delivered.sort(), ["x:m1", "y:m1"]);
});

test("runDaemon passes the abort signal through to watch", async () => {
  let sawSignal = false;
  const ac = new AbortController();
  const watchFn = async ({ signal }) => { sawSignal = signal === ac.signal; };
  await runDaemon({ identity: { did: "did:x:AIR-ME" }, sinks: [], signal: ac.signal, watchFn, log: () => {} });
  assert.equal(sawSignal, true);
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd agent-bridge-mcp && node --test test/daemon.test.mjs`
Expected: FAIL — `Cannot find module '../src/daemon.mjs'`.

- [ ] **Step 3: Write minimal implementation**

```js
// src/daemon.mjs — the always-on receiver daemon. Owns the single consumer lock, runs ONE
// watch() loop, and fans every received message out to its in-process sinks. The Phase 2 socket
// layer attaches additional dynamic sinks to the same fanOut; this phase wires in-process only.
import { watch } from "./watch.mjs";
import { fanOut } from "./fanout.mjs";

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
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd agent-bridge-mcp && node --test test/daemon.test.mjs`
Expected: PASS — 2 tests.

- [ ] **Step 5: Commit**

```bash
git add agent-bridge-mcp/src/daemon.mjs agent-bridge-mcp/test/daemon.test.mjs
git commit -m "feat(daemon): assembly — drive watch() into the fan-out hub"
```

---

### Task 4: PID file + run-state helpers

**Files:**
- Modify: `agent-bridge-mcp/src/daemon.mjs` (append the helpers)
- Test: `agent-bridge-mcp/test/daemon-pid.test.mjs`

- [ ] **Step 1: Write the failing test**

```js
// test/daemon-pid.test.mjs
import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { writeDaemonPid, readDaemonPid, isDaemonRunning, clearDaemonPid } from "../src/daemon.mjs";

let dir;
beforeEach(() => { dir = mkdtempSync(join(tmpdir(), "air-daemon-")); process.env.AGENT_BRIDGE_HOME = dir; });
afterEach(() => { rmSync(dir, { recursive: true, force: true }); });

test("writeDaemonPid + readDaemonPid round-trips {pid, start_time}", () => {
  writeDaemonPid({ pid: 4242, startTime: "2026-06-05T00:00:00Z" });
  assert.deepEqual(readDaemonPid(), { pid: 4242, start_time: "2026-06-05T00:00:00Z" });
});

test("isDaemonRunning is true only when the recorded PID is alive", () => {
  writeDaemonPid({ pid: 4242, startTime: "x" });
  assert.equal(isDaemonRunning(() => true), true);
  assert.equal(isDaemonRunning(() => false), false);
});

test("clearDaemonPid removes the file → not running", () => {
  writeDaemonPid({ pid: 4242, startTime: "x" });
  clearDaemonPid();
  assert.equal(readDaemonPid(), null);
  assert.equal(isDaemonRunning(() => true), false);
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd agent-bridge-mcp && node --test test/daemon-pid.test.mjs`
Expected: FAIL — `writeDaemonPid is not a function` (not yet exported).

- [ ] **Step 3: Write minimal implementation** (append to `src/daemon.mjs`)

```js
import { readFileSync, writeFileSync, existsSync, rmSync } from "node:fs";
import { join } from "node:path";
import { bridgeHome } from "./identity.mjs";
import { isPidAlive } from "./consumer-lock.mjs";

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
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd agent-bridge-mcp && node --test test/daemon-pid.test.mjs`
Expected: PASS — 3 tests.

- [ ] **Step 5: Commit**

```bash
git add agent-bridge-mcp/src/daemon.mjs agent-bridge-mcp/test/daemon-pid.test.mjs
git commit -m "feat(daemon): PID file + run-state helpers (pid+start_time)"
```

---

### Task 5: `daemonStatus()` + the `air-msg daemon status` command

**Files:**
- Modify: `agent-bridge-mcp/src/daemon.mjs` (append `daemonStatus`)
- Modify: `agent-bridge-mcp/src/cli.mjs` (add the `daemon` command — `status` sub)
- Test: `agent-bridge-mcp/test/daemon-pid.test.mjs` (add a case)

- [ ] **Step 1: Write the failing test** (append to `test/daemon-pid.test.mjs`)

```js
import { daemonStatus } from "../src/daemon.mjs";

test("daemonStatus reports running + pid + start_time", () => {
  writeDaemonPid({ pid: 99, startTime: "2026-06-05T01:02:03Z" });
  const s = daemonStatus(() => true);
  assert.equal(s.running, true);
  assert.equal(s.pid, 99);
  assert.equal(s.start_time, "2026-06-05T01:02:03Z");
});

test("daemonStatus reports not-running when no PID file", () => {
  const s = daemonStatus(() => true);
  assert.equal(s.running, false);
  assert.equal(s.pid, null);
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd agent-bridge-mcp && node --test test/daemon-pid.test.mjs`
Expected: FAIL — `daemonStatus is not a function`.

- [ ] **Step 3: Write minimal implementation** (append to `src/daemon.mjs`)

```js
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
```

Then in `src/cli.mjs`, add a `case "daemon":` to the top-level `switch (cmd)` (mirror the `room` case's sub-dispatch via `parseRoomArgs`-style parsing — reuse `parseRoomArgs(rest)` to get `sub`). For Phase 1 wire only `status`:

```js
    case "daemon": {
      const { sub } = parseRoomArgs(rest); // reuse the sub-arg splitter
      switch (sub) {
        case "status": {
          const { daemonStatus } = await import("./daemon.mjs");
          const s = daemonStatus();
          console.log(`daemon: ${s.running ? c.green("running") : c.dim("stopped")}` +
            (s.pid ? `  ${c.dim("pid " + s.pid + (s.start_time ? " since " + s.start_time : ""))}` : ""));
          console.log(`cursor: ${s.cursor ?? "?"}`);
          break;
        }
        default:
          console.error(`unknown daemon subcommand: ${sub || "(none)"}`);
          console.log(`  daemon subcommands: start | stop | status`);
          process.exit(1);
      }
      break;
    }
```

Add `air-msg daemon status` to the `HELP` string.

- [ ] **Step 4: Run test to verify it passes + smoke the CLI**

Run: `cd agent-bridge-mcp && node --test test/daemon-pid.test.mjs`
Expected: PASS.
Run: `AGENT_BRIDGE_HOME=/tmp/air-daemon-smoke node src/cli.mjs daemon status`
Expected: prints `daemon: stopped` + `cursor: ?` (no daemon, fresh home — must NOT register/throw; `daemonStatus` reads files only).

- [ ] **Step 5: Commit**

```bash
git add agent-bridge-mcp/src/daemon.mjs agent-bridge-mcp/src/cli.mjs agent-bridge-mcp/test/daemon-pid.test.mjs
git commit -m "feat(daemon): daemonStatus() + air-msg daemon status"
```

---

### Task 6: `air-msg daemon start` / `stop`

**Files:**
- Modify: `agent-bridge-mcp/src/cli.mjs` (the `daemon` case — add `start`, `stop`)
- Modify: `agent-bridge-mcp/src/daemon.mjs` (a `startDaemon()` entry that wires lock + sinks + signals)

- [ ] **Step 1: Write the implementation** (process lifecycle — verified manually in Step 2, no unit test for signal handling)

Append to `src/daemon.mjs`:

```js
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
```

In `src/cli.mjs` `case "daemon"`, add:

```js
        case "start": {
          const { startDaemon } = await import("./daemon.mjs");
          await startDaemon();
          break;
        }
        case "stop": {
          const { readDaemonPid, clearDaemonPid } = await import("./daemon.mjs");
          const rec = readDaemonPid();
          if (!rec) { console.log(c.dim("daemon not running")); break; }
          try { process.kill(rec.pid, "SIGTERM"); console.log(`${c.green("✓ stopped")} ${c.dim("pid " + rec.pid)}`); }
          catch (e) { console.error(`could not signal pid ${rec.pid}: ${e.message}`); clearDaemonPid(); }
          break;
        }
```

- [ ] **Step 2: Verify manually** (two terminals, against the live relay)

```bash
# Terminal A — start the daemon for a throwaway identity
AGENT_BRIDGE_HOME=/tmp/air-daemon-A node src/cli.mjs register --name "daemon-test"
AGENT_BRIDGE_HOME=/tmp/air-daemon-A node src/cli.mjs daemon start   # blocks; prints "[daemon] up"
# Terminal B — status + send to A; expect an OS banner from the daemon
AGENT_BRIDGE_HOME=/tmp/air-daemon-A node src/cli.mjs daemon status   # daemon: running pid …
# (from another identity) send to A's AIR-id → a macOS banner fires
# Terminal A: Ctrl-C → "[daemon]" exits cleanly; daemon.pid + consumer.lock are gone
ls /tmp/air-daemon-A   # no daemon.pid, no consumer.lock
```
Expected: a banner appears on send; `daemon status` shows running; clean shutdown clears both files. Confirm only ONE consumer (running `air-msg watch` on the same home while the daemon runs should `exit(1)` with the lock message — that loud conflict is correct; Phase 4 turns it into a graceful attach).

- [ ] **Step 3: Run the full suite (no regressions)**

Run: `cd agent-bridge-mcp && node --test`
Expected: all prior tests pass + the new daemon tests; 0 fail.

- [ ] **Step 4: Commit**

```bash
git add agent-bridge-mcp/src/daemon.mjs agent-bridge-mcp/src/cli.mjs
git commit -m "feat(daemon): air-msg daemon start/stop (banner sink, lock-guarded)"
```

---

## Phase 1 done = shippable

After Task 6: `air-msg daemon start` runs an always-on process that rings the banner for incoming mail and is controllable via `daemon status` / `daemon stop`. It holds the single consumer lock (so it correctly conflicts, loudly, with a standalone `watch` for now). This is the foundation every later phase plugs into.

**Phase 1 follow-on (small):** a `telegramSink` wrapping `makeBridgeOutbound` (+ the inbound listener) so the daemon delivers banner **and** Telegram from one process — it's a new sink in `daemon-sinks.mjs` + an entry in `startDaemon`'s sink list; the inbound reply loop moves in alongside `runDaemon`.

## Subsequent phases (each its own spec-grounded plan)

- **Phase 2 — socket + per-subscriber gate + channel client.** `src/daemon-ipc.mjs`: a Unix-domain-socket server (`${AGENT_BRIDGE_HOME}/daemon.sock`, 0600, refuse to bind if the home is group/other-writable, re-stat after listen) speaking line-delimited JSON (`hello` with a declared `role`, `message`, `ping`). The daemon adds a "socket sink" that, **per connected subscriber, applies that role's gate before writing** (`channel` → verified+pinned+!key_changed; `viewer` → mute-only) — the daemon enforces, the client never chooses (spec §5, the Critical fix). Refactor `channel-server.mjs` into a thin client: validate+connect the socket FIRST, then `server.connect(transport)`, then push role-gated mail. Tests: an in-memory `net` pair exercising handshake + the gate admit/deny invariant.
- **Phase 3 — delivery semantics (spec §6).** Per-role buffers (best-effort drop-on-overflow for `viewer`/`bridge`, logged with a count); `channel` = at-least-once: a `{type:gap, after_seq}` marker on overflow/reconnect + client replay from `history({ since_seq })` deduped by `envelope_id`; make the archive write a precondition for advancing the cursor in daemon mode.
- **Phase 4 — lifecycle + installers (spec §7, §8).** The daemon↔legacy decision table (socket live → attach; stale socket + dead daemon → unlink under lock + standalone; none → standalone), reconnect/backoff, `EADDRINUSE` bind-loser, the richer `daemon status` (clients+roles, last seq), and `daemon install`/`uninstall` writing a launchd LaunchAgent (macOS) + systemd-user service (Linux). POSIX-only (Windows is v2 per spec §2/§10).

---

## Self-review (against the spec)

- **§3 components:** Phase 1 covers `daemon.mjs` (core) + `fanout` + the banner sink + CLI `daemon`. `daemon-ipc.mjs` and the channel-client refactor are Phase 2 (mapped). ✓
- **§4 data flow:** relay → `watch()` (already drains `has_more`, merged) → `onMessage` → `fanOut` to sinks. ✓
- **§5 gate:** explicitly Phase 2 (no socket in Phase 1, so no cross-process plaintext yet — in-process sinks only). Noted. ✓
- **Placeholders:** none — every step has runnable code/commands.
- **Type consistency:** the sink shape `{ name, deliver(m) }` is identical across `fanout.mjs`, `daemon-sinks.mjs`, `daemon.mjs`, and `startDaemon`. PID record `{ pid, start_time }` consistent across write/read/status. ✓
