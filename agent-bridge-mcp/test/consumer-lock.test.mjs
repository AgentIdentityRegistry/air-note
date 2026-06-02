import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { acquireConsumerLock, releaseConsumerLock, isPidAlive, acquireOrExit } from "../src/consumer-lock.mjs";

let dir;
beforeEach(() => { dir = mkdtempSync(join(tmpdir(), "air-msg-lock-")); });
afterEach(() => { rmSync(dir, { recursive: true, force: true }); });

test("acquire on empty dir succeeds and writes the pid file", () => {
  const r = acquireConsumerLock({ name: "watch", home: dir, pid: 100, isAlive: () => true });
  assert.equal(r.acquired, true);
  assert.ok(existsSync(join(dir, "consumer.lock")));
});

test("a second LIVE consumer is refused, with the holder", () => {
  acquireConsumerLock({ name: "watch", home: dir, pid: 100, isAlive: () => true });
  const r = acquireConsumerLock({ name: "bridge", home: dir, pid: 200, isAlive: () => true });
  assert.equal(r.acquired, false);
  assert.equal(r.holder.pid, 100);
  assert.equal(r.holder.name, "watch");
});

test("a stale lock (dead holder) is reclaimed", () => {
  acquireConsumerLock({ name: "watch", home: dir, pid: 100, isAlive: () => true });
  const r = acquireConsumerLock({ name: "bridge", home: dir, pid: 200, isAlive: (p) => p === 200 });
  assert.equal(r.acquired, true);
});

test("re-acquire by the same pid is idempotent", () => {
  acquireConsumerLock({ name: "watch", home: dir, pid: 100, isAlive: () => true });
  const r = acquireConsumerLock({ name: "watch", home: dir, pid: 100, isAlive: () => true });
  assert.equal(r.acquired, true);
});

test("release removes the lock iff we own it", () => {
  acquireConsumerLock({ name: "watch", home: dir, pid: 100, isAlive: () => true });
  releaseConsumerLock({ home: dir, pid: 999 });
  assert.ok(existsSync(join(dir, "consumer.lock")));
  releaseConsumerLock({ home: dir, pid: 100 });
  assert.ok(!existsSync(join(dir, "consumer.lock")));
});

test("isPidAlive: this process is alive; an absurd pid is not", () => {
  assert.equal(isPidAlive(process.pid), true);
  assert.equal(isPidAlive(2 ** 30), false);
});

test("acquireOrExit: prints + exits 1 when held by a live consumer", () => {
  acquireConsumerLock({ name: "watch", home: dir, pid: 100, isAlive: () => true });
  const logs = []; let code = null;
  const ok = acquireOrExit("bridge", { home: dir, pid: 200, isAlive: () => true,
    log: (s) => logs.push(s), exit: (n) => { code = n; } });
  assert.equal(ok, false);
  assert.equal(code, 1);
  assert.ok(logs.some((l) => l.includes("another live consumer") && l.includes("100")));
});
