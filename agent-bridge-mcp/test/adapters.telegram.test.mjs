import { test } from "node:test";
import assert from "node:assert/strict";
import { createTelegramAdapter, captureFirstChat } from "../src/adapters/telegram.mjs";

/** A scripted fetch: each call shifts the next handler off the queue. */
function scriptedFetch(handlers) {
  const calls = [];
  const q = [...handlers];
  const fetchImpl = async (url, opts) => {
    calls.push({ url, body: opts?.body ? JSON.parse(opts.body) : null });
    const h = q.shift() || (() => ({ ok: true, status: 200, json: async () => ({ ok: true, result: [] }) }));
    return h(url, opts);
  };
  return { fetchImpl, calls };
}
const okResult = (result) => () => ({ ok: true, status: 200, json: async () => ({ ok: true, result }) });

test("send: POSTs sendMessage with chat_id + text and NO parse_mode; returns the message id", async () => {
  const { fetchImpl, calls } = scriptedFetch([okResult({ message_id: 777 })]);
  const a = createTelegramAdapter({ token: "T", chatId: 555, fetchImpl });
  const id = await a.send({ title: "✓ verified · 📬 alice", body: "ping" });
  assert.equal(id, "777");
  assert.ok(calls[0].url.endsWith("/sendMessage"));
  assert.equal(calls[0].body.chat_id, 555);
  assert.equal(calls[0].body.text, "✓ verified · 📬 alice\nping");
  assert.equal("parse_mode" in calls[0].body, false);
});

test("send: a 429 is retried once after retry_after, then succeeds", async () => {
  const { fetchImpl, calls } = scriptedFetch([
    () => ({ ok: false, status: 429, json: async () => ({ ok: false, parameters: { retry_after: 0 } }) }),
    okResult({ message_id: 9 }),
  ]);
  const a = createTelegramAdapter({ token: "T", chatId: 1, fetchImpl });
  assert.equal(await a.send({ title: "t", body: "b" }), "9");
  assert.equal(calls.length, 2); // one 429 + exactly one retry
});

test("send: a 429 during shutdown (aborted signal) stops retrying and degrades to null", async () => {
  const ac = new AbortController(); ac.abort();
  const { fetchImpl, calls } = scriptedFetch([
    () => ({ ok: false, status: 429, json: async () => ({ ok: false, parameters: { retry_after: 0 } }) }),
  ]);
  const a = createTelegramAdapter({ token: "T", chatId: 1, fetchImpl, signal: ac.signal });
  assert.equal(await a.send({ title: "t", body: "b" }), null);
  assert.equal(calls.length, 1); // did NOT retry after abort
});

test("listen: a matching-chat reply calls onReply with the reply_to id + text, then advances the offset", async () => {
  let savedOffset = 0;
  const ac = new AbortController();
  const update = { update_id: 10, message: { message_id: 50, chat: { id: 555 }, text: "yes do it",
    reply_to_message: { message_id: 777 } } };
  const { fetchImpl } = scriptedFetch([
    okResult([update]),
    () => { ac.abort(); return { ok: true, status: 200, json: async () => ({ ok: true, result: [] }) }; },
  ]);
  const seen = [];
  const a = createTelegramAdapter({ token: "T", chatId: 555, fetchImpl,
    getOffset: () => savedOffset, setOffset: (o) => { savedOffset = o; } });
  await a.listen({ signal: ac.signal, onReply: async (r) => { seen.push(r); } });
  assert.equal(seen.length, 1);
  assert.equal(seen[0].replyToExternalId, "777");
  assert.equal(seen[0].text, "yes do it");
  assert.equal(savedOffset, 11);
});

test("listen: an update from a FOREIGN chat is ignored but still acked (offset advances)", async () => {
  let savedOffset = 0;
  const ac = new AbortController();
  const foreign = { update_id: 20, message: { message_id: 1, chat: { id: 999 }, text: "hi" } };
  const { fetchImpl } = scriptedFetch([
    okResult([foreign]),
    () => { ac.abort(); return { ok: true, status: 200, json: async () => ({ ok: true, result: [] }) }; },
  ]);
  const seen = [];
  const a = createTelegramAdapter({ token: "T", chatId: 555, fetchImpl,
    getOffset: () => savedOffset, setOffset: (o) => { savedOffset = o; } });
  await a.listen({ signal: ac.signal, onReply: async (r) => seen.push(r) });
  assert.equal(seen.length, 0);
  assert.equal(savedOffset, 21);
});

test("listen: when onReply throws, the offset is NOT advanced past it (at-least-once)", async () => {
  let savedOffset = 0;
  const ac = new AbortController();
  const update = { update_id: 30, message: { message_id: 5, chat: { id: 555 }, text: "x",
    reply_to_message: { message_id: 1 } } };
  const { fetchImpl } = scriptedFetch([
    okResult([update]),
    () => { ac.abort(); return { ok: true, status: 200, json: async () => ({ ok: true, result: [] }) }; },
  ]);
  const a = createTelegramAdapter({ token: "T", chatId: 555, fetchImpl,
    getOffset: () => savedOffset, setOffset: (o) => { savedOffset = o; } });
  await a.listen({ signal: ac.signal, onReply: async () => { throw new Error("send failed"); } });
  assert.equal(savedOffset, 0);
});

test("listen: the reply() callback sends a sendMessage back to the saved chat", async () => {
  let savedOffset = 0;
  const ac = new AbortController();
  const update = { update_id: 40, message: { message_id: 7, chat: { id: 555 }, text: "ok",
    reply_to_message: { message_id: 2 } } };
  const { fetchImpl, calls } = scriptedFetch([
    okResult([update]),
    okResult({ message_id: 8 }),
    () => { ac.abort(); return { ok: true, status: 200, json: async () => ({ ok: true, result: [] }) }; },
  ]);
  const a = createTelegramAdapter({ token: "T", chatId: 555, fetchImpl,
    getOffset: () => savedOffset, setOffset: (o) => { savedOffset = o; } });
  await a.listen({ signal: ac.signal, onReply: async (r) => { await r.reply("✓ sent to alice"); } });
  const sent = calls.find((cl) => cl.url.endsWith("/sendMessage") && cl.body.text === "✓ sent to alice");
  assert.ok(sent);
  assert.equal(sent.body.chat_id, 555);
  assert.equal(sent.body.reply_to_message_id, 7); // threads under the originating message_id
});

test("captureFirstChat: returns the chat id of the first message from any chat", async () => {
  const { fetchImpl } = scriptedFetch([
    okResult([]),
    okResult([{ update_id: 1, message: { chat: { id: 4242 }, text: "/start" } }]),
  ]);
  const id = await captureFirstChat({ token: "T", fetchImpl, pollDelayMs: 0, maxPolls: 5 });
  assert.equal(id, 4242);
});
