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
