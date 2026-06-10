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
import { assertSafeHome, prepareSocketPath, socketPath, cleanStaleSocket } from "../src/daemon-ipc.mjs";

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
function rawClient(role, helloExtra = {}) {
  return new Promise((resolve, reject) => {
    const frames = [];
    const sock = createConnection(socketPath());
    const feed = makeLineParser((f) => {
      frames.push(f);
      if (f.type === "hello-ok" || f.type === "error") resolve({ sock, frames });
    });
    sock.on("data", feed);
    sock.once("error", reject);
    sock.once("connect", () => sock.write(encodeFrame({ type: "hello", role, ...helloExtra })));
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

test("ipc server: a stuck (non-reading) client is destroyed once its queue passes the 4×HWM backstop", async () => {
  chmodSync(dir, 0o700);
  const ipc = ipcFor({ highWaterMark: 1024 });   // tiny HWM so the test stays light
  await ipc.listen();
  try {
    const { sock } = await rawClient("viewer");
    sock.pause();                                // simulate a wedged local client
    const fat = { envelope_id: "eFat", from: "did:wba:x", verified: false, body: { type: "text", text: "x".repeat(65536) } };
    // Push until node-side queueing exceeds 4×HWM (one 64KiB frame overshoots the whole
    // (HWM, 4×HWM] skip band, so this lands straight in destroy — Phase-3 backstop semantics).
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

import { connectDaemon, connectDaemonPersistent, probeDaemon, queryDaemonStatus } from "../src/daemon-ipc.mjs";

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

test("overflow(channel): writes are skipped over the HWM, then ONE gap frame arrives on drain", async () => {
  chmodSync(dir, 0o700);
  const ipc = ipcFor({ highWaterMark: 2048 });
  await ipc.listen();
  try {
    const ch = await rawClient("channel");
    // Body sizing (critic H-A, empirically derived): a single frame must land writableLength
    // INSIDE the skip band (HWM, 4×HWM] = (2048, 8192] — 16KiB frames overshoot the whole band
    // into destroy. ~3KiB frames are in-band; the send count must saturate the kernel send
    // buffer (SO_SNDBUF varies by platform), so send far more than any plausible buffer holds.
    const mk = (i) => ({ envelope_id: `eC${i}`, seq: 100 + i, from: "did:wba:x", contact: "al",
      verified: true, key_changed: false, body: { type: "text", text: "y".repeat(3000) } });
    ch.sock.pause();                                  // wedge the client
    for (let i = 0; i < 600 && ipc.clientCount() === 1; i++) await ipc.sink.deliver(mk(i));
    assert.equal(ipc.clientCount(), 1);               // skip band — never destroyed (guarded mid-burst)
    assert.ok(ipc.clientStats()[0].dropped > 0, "skip path must have engaged");   // positive proof (critic M-A)
    ch.sock.resume();                                 // drain
    await until(() => ch.frames.some((f) => f.type === "gap"));
    const gap = ch.frames.find((f) => f.type === "gap");
    assert.equal(typeof gap.after_seq, "number");     // last successfully WRITTEN seq
    const delivered = ch.frames.filter((f) => f.type === "message").map((f) => f.message.relay_seq);
    assert.equal(gap.after_seq, Math.max(...delivered));   // gap starts exactly after the last write
    ch.sock.destroy();
  } finally { await ipc.close(); }
});

test("overflow(channel): SLOW-STEADY reader — gap arrives via flush-on-progress (no reliance on 'drain')", async () => {
  chmodSync(dir, 0o700);
  const ipc = ipcFor({ highWaterMark: 2048 });
  await ipc.listen();
  try {
    const ch = await rawClient("channel");
    const mk = (i, size) => ({ envelope_id: `eS${i}`, seq: 200 + i, from: "did:wba:x", contact: "al",
      verified: true, key_changed: false, body: { type: "text", text: "y".repeat(size) } });
    ch.sock.pause();
    for (let i = 0; i < 600 && ipc.clientCount() === 1; i++) await ipc.sink.deliver(mk(i, 3000));   // in-band frames (critic H-A)
    assert.equal(ipc.clientCount(), 1);                                   // never destroyed (guarded mid-burst)
    assert.ok(ipc.clientStats()[0].dropped > 0, "skip path must have engaged");   // critic M-A
    // Slow-steady regime: partial reads free the queue gradually; the gap must arrive on the
    // next successful WRITE below the threshold — NOT only when the buffer fully empties.
    let gotGap = false;
    const tick = setInterval(() => { ch.sock.read(); }, 10);   // read() = all available (read(n) returns null when n > buffered)
    try {
      for (let i = 0; i < 200 && !gotGap; i++) {
        await ipc.sink.deliver(mk(1000 + i, 64));                         // small follow-ups (ids past the burst range)
        gotGap = ch.frames.some((f) => f.type === "gap");
        await new Promise((r) => setTimeout(r, 5));
      }
    } finally { clearInterval(tick); }
    assert.equal(gotGap, true);                                           // C1: no silent starvation
    assert.equal(ipc.clientCount(), 1);                                   // and never destroyed
    ch.sock.destroy();
  } finally { await ipc.close(); }
});

test("overflow(viewer): messages are dropped with a count, no gap frame, client stays", async () => {
  chmodSync(dir, 0o700);
  const logs = [];
  const ipc = ipcFor({ highWaterMark: 2048, log: (s) => logs.push(s) });
  await ipc.listen();
  try {
    const v = await rawClient("viewer");
    v.sock.pause();
    const fat = (i) => ({ envelope_id: `eV${i}`, seq: i, from: "did:wba:x", verified: false,
      body: { type: "text", text: "z".repeat(3000) } });                   // in-band (critic H-A)
    for (let i = 0; i < 600 && ipc.clientCount() === 1; i++) await ipc.sink.deliver(fat(i));
    assert.equal(ipc.clientCount(), 1);               // stays connected (guarded mid-burst)
    assert.ok(ipc.clientStats()[0].dropped > 0, "skip path must have engaged");   // critic M-A
    v.sock.resume();
    await until(() => logs.some((l) => /dropped \d+ msgs to viewer/.test(l)));
    assert.equal(v.frames.some((f) => f.type === "gap"), false);   // gap is channel-only
    v.sock.destroy();
  } finally { await ipc.close(); }
});

test("overflow backstop: a socket wedged past 4×HWM is destroyed", async () => {
  chmodSync(dir, 0o700);
  const ipc = ipcFor({ highWaterMark: 512 });
  await ipc.listen();
  try {
    const v = await rawClient("viewer");
    v.sock.pause();
    // One frame far larger than 4×HWM lands in the queue, then the next deliver sees it wedged.
    const huge = { envelope_id: "eHuge", seq: 1, from: "did:wba:x", verified: false,
      body: { type: "text", text: "w".repeat(262144) } };
    await ipc.sink.deliver(huge);
    await ipc.sink.deliver({ ...huge, envelope_id: "eHuge2", seq: 2 });
    await until(() => ipc.clientCount() === 0);       // backstop fired
  } finally { await ipc.close(); }
});

test("connectDaemon: real gap round-trip — wedge beneath the client, progress, onGap fires", async () => {
  chmodSync(dir, 0o700);
  const ipc = ipcFor({ highWaterMark: 2048 });
  await ipc.listen();
  try {
    const got = []; let gapAt = null;
    const handle = await connectDaemon({ role: "channel", onMessage: (m) => got.push(m.relay_seq),
      onGap: (seq) => { gapAt = seq; }, log: () => {} });
    const mk = (i, size) => ({ envelope_id: `eG${i}`, seq: 300 + i, from: "did:wba:x", contact: "al",
      verified: true, key_changed: false, body: { type: "text", text: "g".repeat(size) } });
    await ipc.sink.deliver(mk(0, 64));                       // one clean write → lastSeq ≥ 300
    await until(() => got.length === 1);
    handle._sock.pause();                                     // wedge beneath the client parser
    for (let i = 1; i <= 600 && ipc.clientCount() === 1; i++) await ipc.sink.deliver(mk(i, 3000));   // in-band skips (critic H-A)
    assert.equal(ipc.clientCount(), 1);                       // never destroyed during the wedge
    handle._sock.resume();                                    // progress
    await ipc.sink.deliver(mk(99, 64));                       // flush-on-progress emits the gap
    await until(() => gapAt !== null);
    assert.ok(gapAt >= 300 && gapAt < 901, `after_seq=${gapAt} must be the last WRITTEN seq`);
    handle.close();
  } finally { await ipc.close(); }
});

test("hello with since_seq (channel): hello-ok is followed by an immediate gap at exactly since_seq", async () => {
  chmodSync(dir, 0o700);
  const ipc = ipcFor();
  await ipc.listen();
  try {
    const ch = await rawClient("channel", { since_seq: 41 });
    await until(() => ch.frames.some((f) => f.type === "gap"));
    const gap = ch.frames.find((f) => f.type === "gap");
    assert.equal(gap.after_seq, 41);
    // hello-ok must arrive BEFORE the gap (client resolves its handle first)
    assert.ok(ch.frames.findIndex((f) => f.type === "hello-ok") < ch.frames.indexOf(gap));
    ch.sock.destroy();
  } finally { await ipc.close(); }
});

test("hello with since_seq: viewer gets NO gap (best-effort role); channel without since_seq gets NO gap (live-from-attach)", async () => {
  chmodSync(dir, 0o700);
  const ipc = ipcFor();
  await ipc.listen();
  try {
    const v = await rawClient("viewer", { since_seq: 41 });
    const ch = await rawClient("channel");
    await new Promise((r) => setTimeout(r, 50));   // give a wrong gap time to arrive
    assert.equal(v.frames.some((f) => f.type === "gap"), false);
    assert.equal(ch.frames.some((f) => f.type === "gap"), false);
    v.sock.destroy(); ch.sock.destroy();
  } finally { await ipc.close(); }
});

test("hello with since_seq seeds lastSeq so a later gap is anchored at since_seq, not 0", async () => {
  chmodSync(dir, 0o700);
  const ipc = ipcFor();
  await ipc.listen();
  try {
    const ch = await rawClient("channel", { since_seq: 41 });
    await until(() => ch.frames.some((f) => f.type === "gap"));
    assert.equal(ipc.clientStats()[0].lastSeq, 41);   // not null — flushPending's after_seq is anchored
    ch.sock.destroy();
  } finally { await ipc.close(); }
});

test("connectDaemon: sinceSeq option puts since_seq on the wire and the gap round-trips to onGap", async () => {
  chmodSync(dir, 0o700);
  const ipc = ipcFor();
  await ipc.listen();
  try {
    let gapAt = null;
    const h = await connectDaemon({ role: "channel", sinceSeq: 7, onMessage: () => {}, onGap: (s) => { gapAt = s; }, log: () => {} });
    await until(() => gapAt !== null);
    assert.equal(gapAt, 7);
    h.close();
  } finally { await ipc.close(); }
});

test("connectDaemonPersistent: first attempt with no daemon rejects DAEMON_DOWN (caller falls back standalone)", async () => {
  chmodSync(dir, 0o700);
  await assert.rejects(
    () => connectDaemonPersistent({ role: "viewer", onMessage: () => {}, log: () => {} }),
    (e) => e.code === "DAEMON_DOWN",
  );
});

test("connectDaemonPersistent: survives a daemon restart and resumes with since_seq = max seen relay_seq", async () => {
  chmodSync(dir, 0o700);
  const a = ipcFor();
  await a.listen();
  const got = []; const gaps = []; let attaches = 0;
  const handle = await connectDaemonPersistent({
    role: "channel",
    onMessage: (m) => got.push(m.relay_seq),
    onGap: (s) => gaps.push(s),
    onAttach: () => { attaches += 1; },
    initialBackoffMs: 10, backoffCapMs: 50,
    log: () => {},
  });
  try {
    await a.sink.deliver({ envelope_id: "p1", seq: 7, from: "did:wba:x", contact: "al", verified: true, key_changed: false, body: { type: "text", text: "hi" } });
    await until(() => got.length === 1);
    await a.close();                                   // daemon "restart": all clients dropped
    const b = ipcFor();
    await b.listen();                                  // same dir → same socket path
    try {
      await until(() => attaches === 2, 5000);         // wrapper reconnected by itself
      await until(() => gaps.length === 1, 5000);      // resume: hello carried since_seq=7 → gap(7)
      assert.equal(gaps[0], 7);
      await b.sink.deliver({ envelope_id: "p2", seq: 8, from: "did:wba:x", contact: "al", verified: true, key_changed: false, body: { type: "text", text: "again" } });
      await until(() => got.length === 2);
      assert.deepEqual(got, [7, 8]);                   // live flow resumed on the new daemon
    } finally { handle.close(); await b.close(); }
  } catch (e) { handle.close(); await a.close().catch(() => {}); throw e; }
});

test("connectDaemonPersistent: reconnect with NOTHING seen falls back to the cursorFn baseline (outage-window mail is not lost)", async () => {
  chmodSync(dir, 0o700);
  const a = ipcFor();
  await a.listen();
  const gaps = []; let attaches = 0;
  const handle = await connectDaemonPersistent({
    role: "channel",
    onMessage: () => {},
    onGap: (s) => gaps.push(s),
    onAttach: () => { attaches += 1; },
    cursorFn: () => 42,                                // archive cursor snapshot at first attach
    initialBackoffMs: 10, backoffCapMs: 50,
    log: () => {},
  });
  try {
    assert.equal(gaps.length, 0);                      // FIRST attach is live-from-attach: no since_seq, no gap
    await a.close();
    const b = ipcFor();
    await b.listen();
    try {
      await until(() => attaches === 2, 5000);
      await until(() => gaps.length === 1, 5000);
      assert.equal(gaps[0], 42);                       // saw no frames → resumed from the baseline
    } finally { handle.close(); await b.close(); }
  } catch (e) { handle.close(); await a.close().catch(() => {}); throw e; }
});

test("connectDaemonPersistent: survives MULTIPLE backoff cycles while the daemon stays down, then reattaches", async () => {
  chmodSync(dir, 0o700);
  const a = ipcFor();
  await a.listen();
  let attaches = 0;
  const handle = await connectDaemonPersistent({
    role: "viewer", onMessage: () => {}, onAttach: () => { attaches += 1; },
    initialBackoffMs: 10, backoffCapMs: 20,
    log: () => {},
  });
  try {
    await a.close();
    await new Promise((r) => setTimeout(r, 120));      // several failed retry cycles at the 20ms cap
    const b = ipcFor();
    await b.listen();
    try {
      await until(() => attaches === 2, 5000);         // the loop survived repeated failures + the cap
      assert.equal(b.clientCount(), 1);
    } finally { handle.close(); await b.close(); }
  } catch (e) { handle.close(); throw e; }
});

test("connectDaemonPersistent: close() stops the reconnect loop for good", async () => {
  chmodSync(dir, 0o700);
  const a = ipcFor();
  await a.listen();
  let attaches = 0;
  const handle = await connectDaemonPersistent({
    role: "viewer", onMessage: () => {}, onAttach: () => { attaches += 1; },
    initialBackoffMs: 10, log: () => {},
  });
  handle.close();
  await a.close();
  const b = ipcFor();
  await b.listen();
  try {
    await new Promise((r) => setTimeout(r, 150));      // > several backoff periods
    assert.equal(attaches, 1);                         // never re-attached after close()
    assert.equal(b.clientCount(), 0);
  } finally { await b.close(); }
});

test("connectDaemonPersistent: close() during an in-flight reconnect closes the late-arriving socket (no undead connection)", async () => {
  // Seam-based: no real sockets, no timing races beyond the intentional ~20ms gate window.
  // call 1: succeeds immediately, captures onClose so we can trigger a reconnect.
  // call 2: blocks until we release the gate, simulating a long handshake.
  let capturedOnClose = null;
  let lateCloseCalled = false;
  let callCount = 0;
  let releaseGate;
  const gate = new Promise((r) => { releaseGate = r; });

  const fakeConnect = ({ onClose }) => {
    callCount += 1;
    if (callCount === 1) {
      capturedOnClose = onClose;
      return Promise.resolve({ close: () => {}, _sock: null });
    }
    // call 2: block until gate released, then return a handle whose close() we track
    return gate.then(() => ({ close: () => { lateCloseCalled = true; }, _sock: null }));
  };

  let attaches = 0;
  const handle = await connectDaemonPersistent({
    role: "viewer", onMessage: () => {}, onAttach: () => { attaches += 1; },
    connectFn: fakeConnect, initialBackoffMs: 5, log: () => {},
  });
  // Trigger reconnect loop (loop is now sleeping 5ms then will await the gated fakeConnect call 2)
  capturedOnClose();
  await new Promise((r) => setTimeout(r, 20));  // loop is now awaiting the gated connectFn
  handle.close();                               // close() races the in-flight handshake
  releaseGate();                                // release: connectFn call 2 resolves late
  await new Promise((r) => setTimeout(r, 10)); // give the resolution microtask time to run
  assert.equal(lateCloseCalled, true,  "late-arriving socket must be closed immediately");
  assert.equal(attaches, 1,            "onAttach must fire exactly once (first attach only)");
});

test("connectDaemonPersistent: a throwing onAttach on reattach is logged, not retried as a failed connect", async () => {
  // Seam-based: connectFn always succeeds; onAttach throws on its 2nd invocation.
  // The bug: without the fix, the throw is caught by the bare catch, increments backoff,
  // and the loop retries — calling connectFn a 3rd+ time while the 2nd connection is live.
  let capturedOnClose = null;
  let callCount = 0;
  const logs = [];

  const fakeConnect = ({ onClose }) => {
    callCount += 1;
    if (callCount === 1) capturedOnClose = onClose;
    return Promise.resolve({ close: () => {}, _sock: null });
  };

  let attachCount = 0;
  const handle = await connectDaemonPersistent({
    role: "viewer", onMessage: () => {},
    onAttach: () => {
      attachCount += 1;
      if (attachCount === 2) throw new Error("boom from onAttach");
    },
    connectFn: fakeConnect, initialBackoffMs: 5, backoffCapMs: 10,
    log: (s) => logs.push(s),
  });
  capturedOnClose();                            // trigger the reconnect loop
  await new Promise((r) => setTimeout(r, 40)); // several would-be 5ms retry cycles
  handle.close();
  assert.equal(callCount, 2, "connectFn must be called exactly twice — no pile-up of retries");
  assert.ok(logs.some((l) => /onAttach callback threw/.test(l)), "throw must be logged, not swallowed");
});

test("backstop recovery: a channel client destroyed at 4×HWM reconnects and resumes via since_seq gap (no final-gap-hint needed)", async () => {
  chmodSync(dir, 0o700);
  const ipc = ipcFor({ highWaterMark: 512 });
  await ipc.listen();
  try {
    const got = []; const gaps = []; let attaches = 0;
    const handle = await connectDaemonPersistent({
      role: "channel",
      onMessage: (m) => got.push(m.relay_seq),
      onGap: (s) => gaps.push(s),
      onAttach: () => { attaches += 1; },
      initialBackoffMs: 10, backoffCapMs: 50,
      log: () => {},
    });
    try {
      await ipc.sink.deliver({ envelope_id: "b1", seq: 5, from: "did:wba:x", contact: "al", verified: true, key_changed: false, body: { type: "text", text: "ok" } });
      await until(() => got.length === 1);
      handle._sock().pause();                          // wedge beneath the client parser
      const huge = (i) => ({ envelope_id: `bH${i}`, seq: 10 + i, from: "did:wba:x", contact: "al",
        verified: true, key_changed: false, body: { type: "text", text: "w".repeat(262144) } });
      await ipc.sink.deliver(huge(0));                 // lands in the queue (>4×512 once written)
      await ipc.sink.deliver(huge(1));                 // next deliver sees it wedged → destroy
      await until(() => ipc.clientCount() === 0);      // backstop fired (Phase 3 semantics)
      // A paused socket defers ALL stream events — including 'close' from the server-side
      // destroy (Node readable semantics). Resume models the wedged consumer RECOVERING; in
      // production the same wedge is a blocked event loop, which defers the close identically
      // and unblocks the same way. No client-side liveness machinery is warranted for a local
      // Unix socket (no half-open failure mode) — rejected as over-engineering.
      handle._sock().resume();
      await until(() => attaches === 2, 5000);         // wrapper reconnected
      await until(() => gaps.length >= 1, 5000);       // resume gap arrived
      // The drain after resume may parse the wedged-but-buffered huge frame (seq 10) before the
      // close fires — or the server-side destroy may have truncated it mid-line. Both anchors
      // are safe under at-least-once: replay is strictly-greater + envelope_id-deduped.
      assert.ok([5, 10].includes(gaps[0]), `gap anchored at last fully-seen seq, got ${gaps[0]}`);
      assert.equal(ipc.clientCount(), 1);              // healthy again
    } finally { handle.close(); }
  } finally { await ipc.close(); }
});

test("cleanStaleSocket: removes a stale socket file; never throws when absent", () => {
  chmodSync(dir, 0o700);
  writeFileSync(socketPath(), "");                     // stale leftover from a crashed daemon
  cleanStaleSocket();
  assert.equal(existsSync(socketPath()), false);
  cleanStaleSocket();                                  // absent → still fine
});

test("probeDaemon: true against a live daemon socket, false when nothing listens", async () => {
  chmodSync(dir, 0o700);
  assert.equal(await probeDaemon({ timeoutMs: 300 }), false);   // nothing there
  const ipc = ipcFor();
  await ipc.listen();
  try {
    assert.equal(await probeDaemon(), true);
    await until(() => ipc.clientCount() === 0);                 // probe detached cleanly
  } finally { await ipc.close(); }
});

test("status frame: reply carries the OTHER clients (requester excluded), last_seq, and statusExtraFn fields", async () => {
  chmodSync(dir, 0o700);
  const ipc = ipcFor({ statusExtraFn: () => ({ sinks: ["banner", "socket"] }) });
  await ipc.listen();
  try {
    const ch = await rawClient("channel");
    const v = await rawClient("viewer");
    await ipc.sink.deliver({ envelope_id: "st1", seq: 9, from: "did:wba:x", contact: "al", verified: true, key_changed: false, body: { type: "text", text: "s" } });
    await until(() => ch.frames.some((f) => f.type === "message"));
    ch.sock.write(encodeFrame({ type: "status" }));
    await until(() => ch.frames.some((f) => f.type === "status"));
    const st = ch.frames.find((f) => f.type === "status");
    assert.equal(st.last_seq, 9);
    // The requesting channel client is EXCLUDED (critic v1 M3); the viewer saw the delivery too.
    assert.deepEqual(st.clients, [{ role: "viewer", dropped: 0, lastSeq: 9 }]);
    assert.deepEqual(st.sinks, ["banner", "socket"]);
    assert.equal(st.socket, socketPath());
    ch.sock.destroy(); v.sock.destroy();
  } finally { await ipc.close(); }
});

test("queryDaemonStatus: round-trips the status; null when no daemon", async () => {
  chmodSync(dir, 0o700);
  assert.equal(await queryDaemonStatus({ timeoutMs: 300 }), null);
  const ipc = ipcFor();
  await ipc.listen();
  try {
    const st = await queryDaemonStatus();
    assert.equal(st.last_seq, null);                   // nothing delivered yet
    assert.deepEqual(st.clients, []);                  // idle daemon: the probe itself is excluded
  } finally { await ipc.close(); }
});

test("queryDaemonStatus: timeout-raced handshake socket is closed, not leaked", async () => {
  chmodSync(dir, 0o700);
  let closed = false;
  let wrote = false;
  // Fake handle whose handshake resolves ~40ms after the 20ms outer timeout fires.
  const fakeHandle = { close: () => { closed = true; }, _sock: { write: () => { wrote = true; } } };
  let resolveConnect;
  const connectFn = () => new Promise((res) => { resolveConnect = res; });
  const resultPromise = queryDaemonStatus({ timeoutMs: 20, connectFn });
  // Let the 20ms timeout fire first (result is null), THEN resolve the handshake.
  await new Promise((r) => setTimeout(r, 40));
  resolveConnect(fakeHandle);
  const result = await resultPromise;
  assert.equal(result, null);        // timed out
  assert.equal(closed, true);        // late-arriving socket was closed
  assert.equal(wrote, false);        // status request was never written to a dead socket
});
