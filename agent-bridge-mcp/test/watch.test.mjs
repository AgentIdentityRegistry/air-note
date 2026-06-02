import { test } from "node:test";
import assert from "node:assert/strict";
import { coalesce, processNewMessages, watch } from "../src/watch.mjs";

const msg = (over = {}) => ({
  from: "did:wba:x:agents:AIR-KENNY", contact: "kenny",
  envelope_id: "e1", body: { type: "text", text: "hi" }, ...over,
});

test("coalesce: one message → one notice with its text", () => {
  const out = coalesce([msg()], new Set());
  assert.equal(out.length, 1);
  assert.equal(out[0].peer, "did:wba:x:agents:AIR-KENNY");
  assert.equal(out[0].title, "kenny");
  assert.equal(out[0].message, "hi");
  assert.equal(out[0].count, 1);
});

test("coalesce: N messages from one peer → one notice ('N new messages')", () => {
  const out = coalesce(
    [msg({ envelope_id: "e1" }), msg({ envelope_id: "e2", body: { type: "text", text: "yo" } })],
    new Set(),
  );
  assert.equal(out.length, 1);
  assert.equal(out[0].count, 2);
  assert.equal(out[0].message, "2 new messages");
});

test("coalesce: two peers → two notices", () => {
  const out = coalesce(
    [msg(), msg({ from: "did:wba:x:agents:AIR-MIA", contact: "mia", envelope_id: "e9" })],
    new Set(),
  );
  assert.equal(out.length, 2);
  const peers = out.map((o) => o.peer).sort();
  assert.deepEqual(peers, ["did:wba:x:agents:AIR-KENNY", "did:wba:x:agents:AIR-MIA"]);
});

test("coalesce: muted peers (by did OR contact) are dropped", () => {
  const out = coalesce([msg(), msg({ from: "did:wba:x:agents:AIR-MIA", contact: "mia", envelope_id: "e9" })],
    new Set(["kenny"]));
  assert.equal(out.length, 1);
  assert.equal(out[0].peer, "did:wba:x:agents:AIR-MIA");
});

test("coalesce: title falls back to a short DID when no contact alias", () => {
  const out = coalesce([msg({ contact: undefined })], new Set());
  assert.ok(out[0].title.includes("AIR-KENNY"));
});

test("coalesce: body rendering for non-text messages", () => {
  assert.equal(coalesce([msg({ body: { type: "unavailable" } })], new Set())[0].message, "(could not decrypt)");
  assert.equal(coalesce([msg({ body: undefined })], new Set())[0].message, "(no content)");
  assert.equal(coalesce([msg({ body: { type: "image" } })], new Set())[0].message, "(message)");
});

test("coalesce: mute by DID and by short AIR-id also work", () => {
  assert.equal(coalesce([msg()], new Set(["did:wba:x:agents:AIR-KENNY"])).length, 0);
  assert.equal(coalesce([msg()], new Set(["AIR-KENNY"])).length, 0);
});

test("coalesce: empty input → empty output", () => {
  assert.deepEqual(coalesce([], new Set()), []);
});

test("processNewMessages: notifies once per peer, dedupes by envelope_id, attaches openCommand", async () => {
  const calls = [];
  const notifier = { notify: async (n) => calls.push(n) };
  const openResolver = (peer) => ["OPEN", peer];
  const seen = new Set();

  const result = {
    messages: [
      { from: "did:x:AIR-KENNY", contact: "kenny", envelope_id: "e1", body: { type: "text", text: "hi" } },
      { from: "did:x:AIR-KENNY", contact: "kenny", envelope_id: "e1", body: { type: "text", text: "hi" } }, // dup
    ],
  };
  await processNewMessages(result, { notifier, openResolver, seen, mute: new Set() });

  assert.equal(calls.length, 1);
  assert.equal(calls[0].title, "kenny");
  assert.equal(calls[0].message, "hi");
  assert.deepEqual(calls[0].openCommand, ["OPEN", "did:x:AIR-KENNY"]);
  assert.ok(seen.has("e1"));
});

test("processNewMessages: nothing new → no notifications", async () => {
  const calls = [];
  const notifier = { notify: async (n) => calls.push(n) };
  const seen = new Set(["e1"]);
  await processNewMessages(
    { messages: [{ from: "did:x:AIR-KENNY", envelope_id: "e1", body: { type: "text", text: "hi" } }] },
    { notifier, openResolver: () => null, seen, mute: new Set() },
  );
  assert.equal(calls.length, 0);
});

test("processNewMessages: null/undefined result → no notifications, no throw", async () => {
  const calls = [];
  const notifier = { notify: async (n) => calls.push(n) };
  await processNewMessages(undefined, { notifier, openResolver: () => null, seen: new Set(), mute: new Set() });
  await processNewMessages(null, { notifier, openResolver: () => null, seen: new Set(), mute: new Set() });
  assert.equal(calls.length, 0);
});

// A minimal fake SSE Response body: yields the given frames then ends.
function fakeSseResponse(frames) {
  const enc = new TextEncoder();
  let i = 0;
  return {
    ok: true,
    body: {
      getReader() {
        return {
          read() {
            if (i < frames.length) return Promise.resolve({ value: enc.encode(frames[i++]), done: false });
            return Promise.resolve({ value: undefined, done: true });
          },
          cancel() {},
        };
      },
    },
  };
}

test("watch: an SSE envelope frame triggers receive() and a notification, then aborts", async () => {
  const ac = new AbortController();
  const calls = [];
  const notifier = { notify: async (n) => calls.push(n) };

  let receiveCount = 0;
  const receiveFn = async () => {
    receiveCount += 1;
    if (receiveCount === 1) {
      return { messages: [{ from: "did:x:AIR-KENNY", contact: "kenny", envelope_id: "e1", body: { type: "text", text: "hi" } }] };
    }
    return { messages: [] };
  };

  const frames = [`: ready\n\n`, `event: envelope\ndata: {"seq":5,"envelope_id":"e1"}\n\n`];
  const fetchImpl = async () => fakeSseResponse(frames);

  const done = watch({
    signal: ac.signal,
    identity: { did: "did:x:AIR-ME", relay_url: "https://relay.test" },
    receiveFn, notifier,
    openResolver: () => ["OPEN"],
    getCursorFn: () => 0,
    fetchImpl,
    intervalMs: 10, coalesceMs: 0, backoffCapMs: 10,
    onIdle: () => ac.abort(), // abort once the stream ends and we'd reconnect/poll
  });

  await done;
  assert.ok(receiveCount >= 1, "receive() was called at least once");
  assert.equal(calls.length, 1);
  assert.equal(calls[0].title, "kenny");
});

test("watch: SSE unavailable → falls back to polling receive() and still notifies", async () => {
  const ac = new AbortController();
  const calls = [];
  const notifier = { notify: async (n) => { calls.push(n); ac.abort(); } }; // stop after first notify
  let n = 0;
  const receiveFn = async () => (++n === 1
    ? { messages: [{ from: "did:x:AIR-MIA", contact: "mia", envelope_id: "p1", body: { type: "text", text: "yo" } }] }
    : { messages: [] });
  const fetchImpl = async () => { throw new Error("no SSE here"); }; // force the poll path
  await watch({
    signal: ac.signal,
    identity: { did: "did:x:AIR-ME", relay_url: "https://relay.test" },
    receiveFn, notifier, openResolver: () => null, getCursorFn: () => 0,
    fetchImpl, intervalMs: 5, backoffCapMs: 5,
  });
  assert.equal(calls.length, 1);
  assert.equal(calls[0].title, "mia");
});
