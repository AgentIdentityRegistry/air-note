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

import { admitForRole, ROLES } from "../src/daemon-ipc.mjs";
import { roomCreateLocal, roomInviteLocal } from "../src/core.mjs";

const M = (over = {}) => ({
  envelope_id: "e1", from: "did:wba:agentidentityregistry.org:agents:AIR-AAAA-BBBB-CCCC",
  contact: "alice", verified: true, key_changed: false,
  body: { type: "text", text: "hi" }, ...over,
});

test("ROLES: exactly channel and viewer in Phase 2 (bridge is an in-process sink until Phase 4)", () => {
  assert.deepEqual([...ROLES].sort(), ["channel", "viewer"]);
});

test("admitForRole(channel): verified+pinned+key-unchanged admits; each violation denies", () => {
  assert.equal(admitForRole("channel", M()), true);
  assert.equal(admitForRole("channel", M({ verified: false })), false);
  assert.equal(admitForRole("channel", M({ contact: undefined })), false);
  assert.equal(admitForRole("channel", M({ key_changed: true })), false);
  assert.equal(admitForRole("channel", M(), { mute: new Set(["alice"]) }), false);
});

test("admitForRole(viewer): mute-only — unverified still visible, muted (alias/did/short-id) not", () => {
  assert.equal(admitForRole("viewer", M({ verified: false, contact: undefined })), true);
  assert.equal(admitForRole("viewer", M(), { mute: new Set(["alice"]) }), false);
  assert.equal(admitForRole("viewer", M(), { mute: new Set([M().from]) }), false);
  assert.equal(admitForRole("viewer", M(), { mute: new Set(["AIR-AAAA-BBBB-CCCC"]) }), false);
});

test("admitForRole: unknown role admits nothing", () => {
  assert.equal(admitForRole("root", M()), false);
  assert.equal(admitForRole(undefined, M()), false);
});

test("admitForRole(channel): room messages route through the ROOM gate (member admits, non-member denies)", () => {
  const stubSigner = (b) => ({ ...b, op_sig: "zSIG" });
  const founder = { did: "did:wba:f", public_key_multibase: "zF", privateKey: null };
  const { room_id } = roomCreateLocal({ identity: founder, name: "GateTest", signer: stubSigner });
  roomInviteLocal({ identity: founder, room_id, member_did: "did:wba:m", member_pubkey: "zM", signer: stubSigner });
  const member = M({ room_id, from: "did:wba:m", contact: "mate" });
  const stranger = M({ room_id, from: "did:wba:stranger", contact: "sus" });
  assert.equal(admitForRole("channel", member), true);
  assert.equal(admitForRole("channel", stranger), false);
});

import { chmodSync, mkdirSync, statSync, writeFileSync, existsSync } from "node:fs";
import { assertSafeHome, prepareSocketPath, socketPath } from "../src/daemon-ipc.mjs";

test("assertSafeHome: 0700 home passes; group- or other-writable home refuses", () => {
  const home = join(dir, "h1"); mkdirSync(home, { mode: 0o700 });
  assert.doesNotThrow(() => assertSafeHome(home));
  chmodSync(home, 0o770);
  assert.throws(() => assertSafeHome(home), /writable/);
  chmodSync(home, 0o707);
  assert.throws(() => assertSafeHome(home), /writable/);
});

test("prepareSocketPath: unlinks a stale socket file (caller holds the consumer lock)", () => {
  // bridgeHome() === dir in these tests; plant a stale file at the real socket path.
  chmodSync(dir, 0o700);
  writeFileSync(socketPath(), "");
  prepareSocketPath();
  assert.equal(existsSync(socketPath()), false);
});

import { createConnection } from "node:net";
import { createIpcServer } from "../src/daemon-ipc.mjs";

/** Minimal raw test client: connect, send hello, collect frames. */
function rawClient(role) {
  return new Promise((resolve, reject) => {
    const frames = [];
    const sock = createConnection(socketPath());
    const feed = makeLineParser((f) => {
      frames.push(f);
      if (f.type === "hello-ok" || f.type === "error") resolve({ sock, frames });
    });
    sock.on("data", feed);
    sock.once("error", reject);
    sock.once("connect", () => sock.write(encodeFrame({ type: "hello", role })));
  });
}

const DAEMON_INFO = { pid: 4242, start_time: "2026-06-10T00:00:00.000Z", did: "did:wba:me" };
const ipcFor = (over = {}) =>
  createIpcServer({ mute: new Set(), daemonInfo: DAEMON_INFO, log: () => {}, ...over });

test("ipc server: hello → hello-ok with daemon info; socket file is 0600", async () => {
  chmodSync(dir, 0o700);
  const ipc = ipcFor();
  await ipc.listen();
  try {
    const { sock, frames } = await rawClient("viewer");
    assert.deepEqual(frames[0], { type: "hello-ok", ...DAEMON_INFO });
    assert.equal(ipc.clientCount(), 1);
    assert.equal(statSync(socketPath()).mode & 0o777, 0o600);
    sock.destroy();
  } finally { await ipc.close(); }
});

test("ipc server: listen succeeds over a planted stale socket file (lock ⇒ unlink ⇒ bind invariant)", async () => {
  chmodSync(dir, 0o700);
  writeFileSync(socketPath(), "");            // stale leftover from a crashed daemon
  const ipc = ipcFor();
  await ipc.listen();                          // must NOT throw EADDRINUSE
  try {
    const { sock, frames } = await rawClient("viewer");
    assert.equal(frames[0].type, "hello-ok");
    sock.destroy();
  } finally { await ipc.close(); }
});

test("ipc server: a bad role gets an error frame and is disconnected", async () => {
  chmodSync(dir, 0o700);
  const ipc = ipcFor();
  await ipc.listen();
  try {
    const { sock, frames } = await rawClient("root");
    assert.equal(frames[0].type, "error");
    await new Promise((res) => sock.once("close", res));   // server closed it
    assert.equal(ipc.clientCount(), 0);
  } finally { await ipc.close(); }
});

test("ipc sink: per-subscriber gating — viewer sees unverified mail, channel does not; relay_seq stamped from seq", async () => {
  chmodSync(dir, 0o700);
  const ipc = ipcFor();
  await ipc.listen();
  try {
    const viewer = await rawClient("viewer");
    const channel = await rawClient("channel");
    const unverified = { envelope_id: "eU", seq: 7, from: "did:wba:x", verified: false, body: { type: "text", text: "spam?" } };
    const verified = { envelope_id: "eV", seq: 8, from: "did:wba:x", contact: "al", verified: true, key_changed: false, body: { type: "text", text: "real" } };
    await ipc.sink.deliver(unverified);
    await ipc.sink.deliver(verified);
    const got = (c) => c.frames.filter((f) => f.type === "message").map((f) => f.message.envelope_id);
    await until(() => got(viewer).length === 2 && got(channel).length === 1);
    assert.deepEqual(got(viewer), ["eU", "eV"]);            // viewer: mute-only
    assert.deepEqual(got(channel), ["eV"]);                 // channel: gate enforced BY THE DAEMON
    const wire = viewer.frames.find((f) => f.type === "message" && f.message.envelope_id === "eU").message;
    assert.equal(wire.relay_seq, 7);                        // Phase-3 readiness: stamped at the boundary
    viewer.sock.destroy(); channel.sock.destroy();
  } finally { await ipc.close(); }
});

test("ipc server: ping → pong; a second hello is ignored (still functional); disconnect deregisters", async () => {
  chmodSync(dir, 0o700);
  const ipc = ipcFor();
  await ipc.listen();
  try {
    const { sock, frames } = await rawClient("viewer");
    sock.write(encodeFrame({ type: "hello", role: "viewer" }));   // duplicate hello: ignored
    sock.write(encodeFrame({ type: "ping" }));
    await until(() => frames.some((f) => f.type === "pong"));
    assert.equal(frames.filter((f) => f.type === "error").length, 0);
    sock.destroy();
    await until(() => ipc.clientCount() === 0);
  } finally { await ipc.close(); }
});

test("ipc server: a stuck (non-reading) client is dropped once its queue passes the high-water mark", async () => {
  chmodSync(dir, 0o700);
  const ipc = ipcFor({ highWaterMark: 1024 });   // tiny HWM so the test stays light
  await ipc.listen();
  try {
    const { sock } = await rawClient("viewer");
    sock.pause();                                // simulate a wedged local client
    const fat = { envelope_id: "eFat", from: "did:wba:x", verified: false, body: { type: "text", text: "x".repeat(65536) } };
    // Push until node-side queueing exceeds the HWM (kernel buffer absorbs the first writes).
    for (let i = 0; i < 64 && ipc.clientCount() > 0; i++) await ipc.sink.deliver({ ...fat, envelope_id: `eFat${i}` });
    await until(() => ipc.clientCount() === 0);  // daemon protected itself: stuck client dropped
  } finally { await ipc.close(); }
});

test("ipc server: a silent connection that never says hello is reaped", async () => {
  chmodSync(dir, 0o700);
  const ipc = ipcFor({ helloTimeoutMs: 50 });
  await ipc.listen();
  try {
    const sock = createConnection(socketPath());
    await new Promise((res) => sock.once("connect", res));
    await new Promise((res) => sock.once("close", res));   // server reaps the pre-hello idler
    assert.equal(ipc.clientCount(), 0);
  } finally { await ipc.close(); }
});

import { connectDaemon } from "../src/daemon-ipc.mjs";

test("connectDaemon: handshakes, then delivers admitted messages to onMessage", async () => {
  chmodSync(dir, 0o700);
  const ipc = ipcFor();
  await ipc.listen();
  try {
    const got = [];
    const handle = await connectDaemon({ role: "viewer", onMessage: (m) => got.push(m.envelope_id), log: () => {} });
    await ipc.sink.deliver({ envelope_id: "e9", from: "did:wba:x", verified: false, body: { type: "text", text: "yo" } });
    await until(() => got.length === 1);
    assert.deepEqual(got, ["e9"]);
    handle.close();
  } finally { await ipc.close(); }
});

test("connectDaemon: no daemon → rejects with code DAEMON_DOWN", async () => {
  chmodSync(dir, 0o700);   // no server listening in this temp home
  await assert.rejects(
    connectDaemon({ role: "viewer", onMessage: () => {}, log: () => {} }),
    (e) => e.code === "DAEMON_DOWN",
  );
});

test("connectDaemon: onClose fires when the daemon goes away", async () => {
  chmodSync(dir, 0o700);
  const ipc = ipcFor();
  await ipc.listen();
  let closed = false;
  const handle = await connectDaemon({ role: "viewer", onMessage: () => {}, onClose: () => { closed = true; }, log: () => {} });
  await ipc.close();
  await until(() => closed);
  handle.close();
});

import { runDaemon } from "../src/daemon.mjs";

test("composition: runDaemon fans one watch() message to banner sink AND gated socket subscribers", async () => {
  chmodSync(dir, 0o700);
  const ipc = ipcFor();
  await ipc.listen();
  try {
    const bannered = [];
    const bannerStub = { name: "banner", deliver: (m) => bannered.push(m.envelope_id) };
    const viewer = await rawClient("viewer");
    const channel = await rawClient("channel");

    const verified = { envelope_id: "eOK", from: "did:wba:x", contact: "al", verified: true, key_changed: false, body: { type: "text", text: "hi" } };
    const unverified = { envelope_id: "eNO", from: "did:wba:x", verified: false, body: { type: "text", text: "??" } };
    // watchFn stub: emit two messages then resolve (daemon loop ends). Same shape as daemon.test.mjs:13.
    const watchFn = async ({ onMessage }) => { await onMessage(verified); await onMessage(unverified); };

    await runDaemon({ identity: { did: "did:wba:me" }, sinks: [bannerStub, ipc.sink], watchFn, log: () => {} });
    const got = (c) => c.frames.filter((f) => f.type === "message").map((f) => f.message.envelope_id);
    await until(() => got(viewer).length === 2 && got(channel).length === 1);

    assert.deepEqual(bannered, ["eOK", "eNO"]);   // in-process banner saw both (its own mute logic is separate)
    assert.deepEqual(got(viewer), ["eOK", "eNO"]);
    assert.deepEqual(got(channel), ["eOK"]);
    viewer.sock.destroy(); channel.sock.destroy();
  } finally { await ipc.close(); }
});
