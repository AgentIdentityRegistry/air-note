// The socket protocol's cross-language contract (AI-inbox design §5): every fixture frame must
// round-trip the JS framing layer, and every REQUEST fixture must elicit the catalogued RESPONSE
// shape from a real server. Phase A2's Rust suite reads the SAME fixtures file.
import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync, chmodSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { encodeFrame, makeLineParser, createIpcServer, socketPath } from "../src/daemon-ipc.mjs";
import { createConnection } from "node:net";

const FIXTURES = JSON.parse(readFileSync(new URL("./fixtures/socket-frames.json", import.meta.url), "utf8"));

let dir;
beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "air-msg-proto-"));
  chmodSync(dir, 0o700);
  process.env.AGENT_BRIDGE_HOME = dir;
});
afterEach(() => { rmSync(dir, { recursive: true, force: true }); });

const until = async (cond, ms = 3000) => {
  const t0 = Date.now();
  while (!cond()) {
    if (Date.now() - t0 > ms) throw new Error("until: timed out");
    await new Promise((r) => setTimeout(r, 10));
  }
};

test("every fixture frame round-trips encodeFrame → makeLineParser unchanged", () => {
  for (const group of [FIXTURES.client_to_daemon, FIXTURES.daemon_to_client]) {
    for (const [name, frame] of Object.entries(group)) {
      const got = [];
      makeLineParser((f) => got.push(f))(encodeFrame(frame));
      assert.deepEqual(got, [frame], `fixture ${name} must round-trip`);
    }
  }
});

test("request fixtures elicit the catalogued response SHAPES from a live server", async () => {
  const ipc = createIpcServer({
    daemonInfo: { pid: 4242, start_time: "2026-06-11T00:00:00.000Z", did: FIXTURES.daemon_to_client.hello_ok.did },
    sendFn: async () => ({ envelope_id: FIXTURES.daemon_to_client.send_ok.envelope_id, encrypted: true }),
    log: () => {},
  });
  await ipc.listen();
  try {
    const sock = createConnection(socketPath());
    const frames = [];
    sock.on("data", makeLineParser((f) => frames.push(f), { onError: () => {} }));
    await new Promise((r) => sock.once("connect", r));
    sock.write(encodeFrame(FIXTURES.client_to_daemon.hello));
    await until(() => frames.some((f) => f.type === "hello-ok"));
    sock.write(encodeFrame(FIXTURES.client_to_daemon.ping));
    sock.write(encodeFrame(FIXTURES.client_to_daemon.status_request));
    sock.write(encodeFrame(FIXTURES.client_to_daemon.send));
    await until(() => frames.some((f) => f.type === "pong")
      && frames.some((f) => f.type === "status")
      && frames.some((f) => f.type === "send-ok"));
    // Shape assertions: same KEYS as the catalogued responses (values vary by environment).
    const shapeOf = (o) => Object.keys(o).sort();
    assert.deepEqual(shapeOf(frames.find((f) => f.type === "hello-ok")), shapeOf(FIXTURES.daemon_to_client.hello_ok));
    assert.deepEqual(shapeOf(frames.find((f) => f.type === "status")), shapeOf(FIXTURES.daemon_to_client.status));
    assert.deepEqual(shapeOf(frames.find((f) => f.type === "send-ok")), shapeOf(FIXTURES.daemon_to_client.send_ok));
    assert.equal(frames.find((f) => f.type === "send-ok").id, FIXTURES.client_to_daemon.send.id);
    sock.destroy();
  } finally { await ipc.close(); }
});

test("the message fixture IS a real deliver() emission (critic H2 — photograph, not painting)", async () => {
  // Drive the daemon's own fan-out with the onMessage-shaped object implied by the fixture
  // (deliver() stamps relay_seq from seq); the emitted frame must deepEqual the fixture EXACTLY.
  const fixtureFrame = FIXTURES.daemon_to_client.message;
  const { relay_seq, ...onMessageShaped } = fixtureFrame.message;
  const ipc = createIpcServer({ daemonInfo: { pid: 1, start_time: "t", did: "did:wba:me" }, log: () => {} });
  await ipc.listen();
  try {
    const sock = createConnection(socketPath());
    const frames = [];
    sock.on("data", makeLineParser((f) => frames.push(f), { onError: () => {} }));
    await new Promise((r) => sock.once("connect", r));
    sock.write(encodeFrame({ type: "hello", role: "viewer" }));
    await until(() => frames.some((f) => f.type === "hello-ok"));
    ipc.sink.deliver(onMessageShaped);
    await until(() => frames.some((f) => f.type === "message"));
    assert.deepEqual(frames.find((f) => f.type === "message"), fixtureFrame);
    sock.destroy();
  } finally { await ipc.close(); }
});
