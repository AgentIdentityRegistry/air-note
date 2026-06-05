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
