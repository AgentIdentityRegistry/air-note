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
