import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { encodeFrame, makeLineParser } from "../src/daemon-ipc.mjs";

let dir;
beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "air-msg-ipc-"));
  process.env.AGENT_BRIDGE_HOME = dir;
});
afterEach(() => {
  rmSync(dir, { recursive: true, force: true });
});

/** Poll a condition instead of sleeping a fixed interval — socket tests must not
 *  bet on wall-clock (flaky on loaded runners). Same ceiling for a true hang. */
const until = async (cond, ms = 2000) => {
  const t0 = Date.now();
  while (!cond()) {
    if (Date.now() - t0 > ms) throw new Error("until: timed out");
    await new Promise((r) => setTimeout(r, 5));
  }
};

test("encodeFrame: one JSON object per newline-terminated line", () => {
  assert.equal(encodeFrame({ type: "ping" }), '{"type":"ping"}\n');
});

test("makeLineParser: reassembles split chunks and splits coalesced frames", () => {
  const got = [];
  const feed = makeLineParser((f) => got.push(f));
  feed(Buffer.from('{"type":"he'));
  feed(Buffer.from('llo","role":"viewer"}\n{"type":"ping"}\n'));
  assert.deepEqual(got, [{ type: "hello", role: "viewer" }, { type: "ping" }]);
});

test("makeLineParser: invalid JSON reports onError and keeps parsing later lines", () => {
  const got = []; const errs = [];
  const feed = makeLineParser((f) => got.push(f), { onError: (e) => errs.push(e) });
  feed(Buffer.from("not-json\n"));
  feed(Buffer.from('{"type":"ping"}\n'));
  assert.equal(errs.length, 1);
  assert.deepEqual(got, [{ type: "ping" }]);
});

test("makeLineParser: a line beyond maxLine reports onError LOUDLY and resets the buffer", () => {
  const errs = [];
  const feed = makeLineParser(() => {}, { maxLine: 16, onError: (e) => errs.push(e) });
  feed(Buffer.from("x".repeat(64)));
  assert.equal(errs.length, 1);
  assert.match(errs[0].message, /exceeds/);
});

test("makeLineParser: a realistic large frame (~100 KB body) round-trips intact under the default ceiling", () => {
  const big = { type: "message", message: { envelope_id: "eBig", body: { type: "text", text: "x".repeat(100_000) } } };
  const got = [];
  const feed = makeLineParser((f) => got.push(f));
  feed(Buffer.from(encodeFrame(big)));
  assert.equal(got.length, 1);
  assert.equal(got[0].message.body.text.length, 100_000);
});

test("default maxLine is 1 MiB (matches watch.mjs MAX_SSE_BUF)", () => {
  const errs = [];
  const feed = makeLineParser(() => {}, { onError: (e) => errs.push(e) });
  feed(Buffer.from("y".repeat((1 << 20) + 1)));   // one over the ceiling, no newline
  assert.equal(errs.length, 1);
});
