import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { buildOsascriptArgs, resolveBackend, createNotifier } from "../src/notifier.mjs";

const saved = {};
beforeEach(() => { saved.NOTIFY = process.env.AIRMSG_NOTIFY; });
afterEach(() => {
  if (saved.NOTIFY === undefined) delete process.env.AIRMSG_NOTIFY;
  else process.env.AIRMSG_NOTIFY = saved.NOTIFY;
});

test("buildOsascriptArgs escapes quotes and embeds title + message", () => {
  const argv = buildOsascriptArgs({ title: "air-msg", message: 'hi "there"' });
  assert.equal(argv[0], "-e");
  assert.ok(argv[1].includes('display notification "hi \\"there\\""'));
  assert.ok(argv[1].includes('with title "air-msg"'));
});

test("resolveBackend honors an explicit override", async () => {
  process.env.AIRMSG_NOTIFY = "bell";
  assert.equal(await resolveBackend(), "bell");
  process.env.AIRMSG_NOTIFY = "none";
  assert.equal(await resolveBackend(), "none");
});

test("resolveBackend prefers osascript on darwin even when node-notifier loads (terminal-notifier is unreliable on modern macOS)", async () => {
  delete process.env.AIRMSG_NOTIFY;
  let loaded = false;
  const b = await resolveBackend({ loadNotifier: async () => { loaded = true; return {}; }, platform: "darwin" });
  assert.equal(b, "osascript");
  assert.equal(loaded, false); // darwin short-circuits — never even attempts node-notifier
});

test("resolveBackend prefers node-notifier off-darwin when it loads", async () => {
  delete process.env.AIRMSG_NOTIFY;
  const b = await resolveBackend({ loadNotifier: async () => ({}), platform: "linux" });
  assert.equal(b, "node-notifier");
});

test("resolveBackend falls back to bell off-darwin when node-notifier is absent", async () => {
  delete process.env.AIRMSG_NOTIFY;
  const b = await resolveBackend({ loadNotifier: async () => { throw new Error("nope"); }, platform: "linux" });
  assert.equal(b, "bell");
});

test("createNotifier with backend=none is a silent no-op (does not throw)", async () => {
  process.env.AIRMSG_NOTIFY = "none";
  const n = await createNotifier();
  assert.equal(n.backend, "none");
  await n.notify({ title: "t", message: "m" }); // must resolve, do nothing
});

test("notify swallows backend errors (best-effort)", async () => {
  process.env.AIRMSG_NOTIFY = "node-notifier";
  const n = await createNotifier({
    loadNotifier: async () => ({ notify: () => { throw new Error("boom"); } }),
  });
  await n.notify({ title: "t", message: "m" }); // must not reject
});

test("notify via osascript backend spawns osascript with escaped args (newlines collapsed)", async () => {
  const spawned = [];
  process.env.AIRMSG_NOTIFY = "osascript";
  const n = await createNotifier({ spawnFn: (cmd, args) => { spawned.push({ cmd, args }); return { unref() {} }; } });
  await n.notify({ title: "air-msg", message: "line1\nline2" });
  assert.equal(spawned.length, 1);
  assert.equal(spawned[0].cmd, "osascript");
  assert.deepEqual(spawned[0].args, buildOsascriptArgs({ title: "air-msg", message: "line1\nline2" }));
  assert.ok(spawned[0].args[1].includes("line1 line2"));
  assert.ok(!spawned[0].args[1].includes("\n"));
});
