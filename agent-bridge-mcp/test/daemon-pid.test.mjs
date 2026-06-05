// test/daemon-pid.test.mjs
import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { writeDaemonPid, readDaemonPid, isDaemonRunning, clearDaemonPid, daemonStatus } from "../src/daemon.mjs";

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
