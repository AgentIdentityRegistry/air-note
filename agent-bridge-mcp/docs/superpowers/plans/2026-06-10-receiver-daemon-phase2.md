# Receiver Daemon — Implementation Plan (Phase 2: socket + per-subscriber gate + channel thin-client)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** v2 — reworked 2026-06-10 after an independent critic review (verdict REWORK → all 7 findings + gaps addressed: `relay_seq` stamped at the socket boundary; backpressure floor; channel-client lifecycle made graceful + symmetric; condition-polled tests; 1 MiB frame ceiling; pre-hello reaper; lock⇒unlink⇒bind invariant test; `bridge`-role deferral documented; live smoke marked MANUAL).

**Goal:** Add the Unix-domain-socket layer to the receiver daemon so N local clients can subscribe to the one relay pull simultaneously — with the **daemon** (never the client) enforcing each subscriber's role gate — and refactor `channel-server.mjs` into a lock-free thin client of that socket.

**Architecture:** Phase 1 (shipped, PR #8) runs ONE `watch()` loop in `daemon.mjs` and fans each message to in-process sinks (`fanout.mjs`, sink = `{name, deliver(m)}`). Phase 2 adds `src/daemon-ipc.mjs`: a socket server registered as one more sink, which internally fans to connected subscribers **after applying that subscriber's role filter** (spec §5 — the Critical fix from review: a "dumb fan-out" would leak all decrypted plaintext to any local process). `channel-server.mjs` becomes daemon-first: attach as a `channel` client (no consumer lock), fall back to today's standalone mode when no daemon runs.

**Tech Stack:** Node ≥22 stdlib only (`node:net` Unix socket, line-delimited JSON). No new dependencies.

**Spec:** `agent-bridge-mcp/docs/superpowers/specs/2026-06-05-receiver-daemon-design.md` (§3 components, §5 gate, §7 row semantics, §9 testing). Phase 3 (rich delivery semantics: per-role buffers, gap/replay, at-least-once) and Phase 4 (decision table, reconnect, installers) are OUT of this plan — but Phase 2 ships a minimal backpressure FLOOR (drop a stuck client) because "zero flow control on an always-on process" is not the same thing as "rich semantics deferred."

**Repo rules that bind every task:**
- Tests MUST set `AGENT_BRIDGE_HOME` to a temp dir — `bridgeHome()` **throws** under the test runner without it (the 2026-06-10 guard). Use the `mkdtempSync` + `beforeEach/afterEach` idiom from `test/archive.test.mjs`.
- Import shared helpers (`channelGate`, `roomChannelGate`, `deriveRoom`, `shortPeer`, `parseMuteSet`) — never inline-copy.
- Run the suite as bare `node --test` (a directory arg is broken on Node 25). Single files: `node --test test/<file>`.
- Work from `~/air-note/agent-bridge-mcp`. Branch: `feat/daemon-phase2-socket`.

**Resolved spec question (record in Task 8):** §11-Q1 — relay `/pull` filters `acked_at IS NULL` AND is cursor-driven (`since=N`); daemon+legacy share one home → one cursor + archive PK `(envelope_id, direction)` dedup, so a brief handoff overlap is at-least-once, never lossy (verified in `~/air-site/relay/src/index.js` L193–237).

**Protocol (line-delimited JSON, one object per `\n`-terminated line):**
- client→daemon: `{"type":"hello","role":"channel"|"viewer"}` (first frame, mandatory, within 5 s or the server reaps the connection) · `{"type":"ping"}`
- daemon→client: `{"type":"hello-ok","pid","start_time","did"}` · `{"type":"error","reason"}` then close · `{"type":"message","message":<wire>}` · `{"type":"pong"}`
- **`wire` = the pushed message with `relay_seq` stamped from `m.seq` at the socket boundary.** The object `core.receive()` hands to `onMessage` carries `seq` (core.mjs:467 room / :537 1:1) — `relay_seq` exists only on the archive row. The daemon normalizes on the way out so Phase 3's gap/replay can key on `message.relay_seq` with no frame change.
- Frame ceiling: **1 MiB** default (`maxLine`), matching `watch.mjs`'s `MAX_SSE_BUF` (1<<20). Real bodies today are ~300 B, but no upstream cap exists; an over-ceiling frame must fail LOUDLY (onError), never silently wedge. Unknown frame types from a subscribed client are ignored (forward compatibility).
- `bridge` is **deliberately not a socket role in Phase 2** — the bridge runs as an in-process daemon sink (spec §3); a socket `bridge` role arrives in Phase 4 when the bridge becomes detachable. `ROLES` hard-rejects it until then.

---

### Task 1: Frame codec (pure)

**Files:**
- Create: `src/daemon-ipc.mjs` (codec section only)
- Test: `test/daemon-ipc.test.mjs`

- [ ] **Step 1: Write the failing tests**

Create `test/daemon-ipc.test.mjs`:

```js
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
```

- [ ] **Step 2: Run to verify failure**

Run: `node --test test/daemon-ipc.test.mjs`
Expected: FAIL — `daemon-ipc.mjs` does not exist.

- [ ] **Step 3: Implement the codec**

Create `src/daemon-ipc.mjs`:

```js
// src/daemon-ipc.mjs — the daemon's local socket layer (spec §5, §7).
// A Unix-domain socket at {AGENT_BRIDGE_HOME}/daemon.sock speaking line-delimited JSON.
// THE DAEMON ENFORCES EACH SUBSCRIBER'S ROLE GATE before writing to that subscriber —
// a client never chooses its own filter (the dumb-fan-out confidentiality hole, spec §5).
import { join } from "node:path";
import { bridgeHome } from "./identity.mjs";

export const socketPath = () => join(bridgeHome(), "daemon.sock");

// Frame ceiling: 1 MiB, matching watch.mjs's MAX_SSE_BUF. Bodies have no upstream size cap,
// so the ceiling must clear any plausible message; an over-ceiling line fails LOUDLY via
// onError (a silent drop here would be invisible mail loss for a channel client).
export const MAX_FRAME = 1 << 20;

/** One JSON object per newline-terminated line. */
export function encodeFrame(obj) {
  return JSON.stringify(obj) + "\n";
}

/** Incremental line parser: feed(chunk); emits parsed frames via onFrame.
 *  A malformed line or an over-long line is reported to onError and skipped —
 *  one bad frame must never kill the connection handler loop. */
export function makeLineParser(onFrame, { maxLine = MAX_FRAME, onError = () => {} } = {}) {
  let buf = "";
  return (chunk) => {
    buf += chunk.toString("utf8");
    if (buf.length > maxLine && !buf.includes("\n")) {
      onError(new Error(`line exceeds ${maxLine} bytes`));
      buf = "";
      return;
    }
    let nl;
    while ((nl = buf.indexOf("\n")) !== -1) {
      const line = buf.slice(0, nl);
      buf = buf.slice(nl + 1);
      if (!line.trim()) continue;
      try {
        onFrame(JSON.parse(line));
      } catch (err) {
        onError(err);
      }
    }
  };
}
```

- [ ] **Step 4: Run to verify pass**

Run: `node --test test/daemon-ipc.test.mjs`
Expected: PASS (6/6).

- [ ] **Step 5: Commit**

```bash
git add src/daemon-ipc.mjs test/daemon-ipc.test.mjs
git commit -m "feat(daemon): frame codec — 1MiB ceiling, loud over-cap failure, chunk-safe parser"
```

---

### Task 2: Role admission (the gate, daemon-side)

**Files:**
- Modify: `src/daemon-ipc.mjs` (append)
- Test: `test/daemon-ipc.test.mjs` (append)

- [ ] **Step 1: Write the failing tests** (append to `test/daemon-ipc.test.mjs`)

```js
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
```

- [ ] **Step 2: Run to verify failure**

Run: `node --test test/daemon-ipc.test.mjs`
Expected: FAIL — `admitForRole`/`ROLES` not exported.

- [ ] **Step 3: Implement** (append to `src/daemon-ipc.mjs`; extend the import block)

```js
import { channelGate, roomChannelGate } from "./channel.mjs";
import { deriveRoom } from "./rooms.mjs";
import { shortPeer } from "./peers.mjs";

// Phase 2 socket roles. `bridge` is deliberately ABSENT: the bridge runs as an in-process
// daemon sink (spec §3); a socket bridge role arrives in Phase 4 when it becomes detachable.
export const ROLES = new Set(["channel", "viewer"]);

/** May `m` cross the socket to a subscriber with `role`? (spec §5 — confidentiality boundary.)
 *  channel: the existing channel policy — 1:1 via channelGate, rooms via roomChannelGate.
 *  viewer:  banner-equivalent visibility — mute-only (mirrors bannerSink in daemon-sinks.mjs).
 *  Presentation policy (raise-hand, addressing) stays in the CLIENT (makeChannelPush). */
export function admitForRole(role, m, { mute = new Set() } = {}) {
  if (role === "viewer") {
    return !(mute.has(m?.contact) || mute.has(m?.from) || mute.has(shortPeer(m?.from)));
  }
  if (role === "channel") {
    return m?.room_id ? roomChannelGate(m, deriveRoom(m.room_id), mute) : channelGate(m, mute);
  }
  return false;
}
```

- [ ] **Step 4: Run to verify pass**

Run: `node --test test/daemon-ipc.test.mjs`
Expected: PASS (11/11).

- [ ] **Step 5: Commit**

```bash
git add src/daemon-ipc.mjs test/daemon-ipc.test.mjs
git commit -m "feat(daemon): per-role admission — daemon-enforced channel/viewer gates (spec §5)"
```

---

### Task 3: Home-safety check + socket hygiene

**Files:**
- Modify: `src/daemon-ipc.mjs` (append)
- Test: `test/daemon-ipc.test.mjs` (append)

- [ ] **Step 1: Write the failing tests** (append)

```js
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
```

- [ ] **Step 2: Run to verify failure**

Run: `node --test test/daemon-ipc.test.mjs`
Expected: FAIL — `assertSafeHome`/`prepareSocketPath` not exported.

- [ ] **Step 3: Implement** (append to `src/daemon-ipc.mjs`; add `statSync`, `rmSync` to a `node:fs` import)

```js
import { statSync, rmSync } from "node:fs";

/** Refuse to operate out of a home dir that group/other can write: anyone who can write
 *  the dir can swap the socket (spec §5). A shared-path home is unsupported. */
export function assertSafeHome(home = bridgeHome()) {
  const mode = statSync(home).mode & 0o777;
  if (mode & 0o022) {
    throw new Error(`refusing socket in group/other-writable home ${home} (mode ${mode.toString(8)})`);
  }
}

/** Remove a stale socket file before bind. ONLY safe because the caller already holds the
 *  consumer lock — the lock is the single-daemon mutex, so nothing live owns this path. */
export function prepareSocketPath() {
  try { rmSync(socketPath(), { force: true }); } catch { /* best effort */ }
}
```

- [ ] **Step 4: Run to verify pass**

Run: `node --test test/daemon-ipc.test.mjs`
Expected: PASS (13/13).

- [ ] **Step 5: Commit**

```bash
git add src/daemon-ipc.mjs test/daemon-ipc.test.mjs
git commit -m "feat(daemon): home-safety assertion + lock-guarded stale-socket cleanup"
```

---

### Task 4: The IPC server (handshake, registry, gated broadcast, flow-control floor)

**Files:**
- Modify: `src/daemon-ipc.mjs` (append)
- Test: `test/daemon-ipc.test.mjs` (append)

- [ ] **Step 1: Write the failing tests** (append). These use a REAL Unix socket inside the temp home.

```js
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
```

- [ ] **Step 2: Run to verify failure**

Run: `node --test test/daemon-ipc.test.mjs`
Expected: FAIL — `createIpcServer` not exported.

- [ ] **Step 3: Implement** (append to `src/daemon-ipc.mjs`; add `createServer` to a `node:net` import, `chmodSync`/`lstatSync` to `node:fs`)

```js
import { createServer } from "node:net";
import { chmodSync, lstatSync } from "node:fs";

/** The daemon's socket server. Returned `sink` plugs into fanOut ({name, deliver}).
 *  Each subscriber declared a role at hello; deliver() applies admitForRole per subscriber
 *  BEFORE writing — the daemon enforces, the client never chooses (spec §5).
 *
 *  Flow control (Phase-2 FLOOR, not the Phase-3 §6 semantics): a subscriber whose unflushed
 *  node-side queue (`socket.writableLength`) exceeds `highWaterMark` is DROPPED with a log —
 *  an always-on daemon must never let one wedged local client balloon its memory. Phase 3
 *  replaces drop-the-client with per-role buffers + gap/replay for `channel`. */
export function createIpcServer({
  mute = new Set(),
  daemonInfo = {},
  highWaterMark = 1 << 20,                    // 1 MiB queued per client, matches MAX_FRAME
  helloTimeoutMs = 5000,                      // pre-hello idlers are reaped (fd-pin guard)
  log = (s) => process.stderr.write(s + "\n"),
} = {}) {
  const subscribers = new Set();   // { socket, role }

  const server = createServer((socket) => {
    let sub = null;
    const reaper = setTimeout(() => { if (!sub) socket.destroy(); }, helloTimeoutMs);
    const feed = makeLineParser((frame) => {
      if (!sub) {
        if (frame.type !== "hello" || !ROLES.has(frame.role)) {
          socket.write(encodeFrame({ type: "error", reason: "first frame must be hello with role channel|viewer" }));
          socket.destroy();
          return;
        }
        clearTimeout(reaper);
        sub = { socket, role: frame.role };
        subscribers.add(sub);
        socket.write(encodeFrame({ type: "hello-ok", ...daemonInfo }));
        log(`[daemon] client attached: role=${frame.role} (${subscribers.size} connected)`);
        return;
      }
      if (frame.type === "ping") socket.write(encodeFrame({ type: "pong" }));
      // Duplicate hello and unknown frames from a subscribed client: ignored (forward compat).
    }, { onError: () => { socket.write(encodeFrame({ type: "error", reason: "bad frame" })); socket.destroy(); } });

    socket.on("data", feed);
    const drop = () => {
      clearTimeout(reaper);
      if (sub) { subscribers.delete(sub); log(`[daemon] client detached (${subscribers.size} connected)`); sub = null; }
    };
    socket.on("close", drop);
    socket.on("error", drop);
  });

  return {
    /** fanOut-compatible sink: write `m` to every subscriber whose role admits it. */
    sink: {
      name: "socket",
      deliver: (m) => {
        // Stamp relay_seq at the boundary: onMessage objects carry `seq` (core.mjs:467/:537);
        // `relay_seq` otherwise exists only on the archive row. Phase 3 keys gap/replay on this.
        const wire = m && m.seq !== undefined && m.relay_seq === undefined ? { ...m, relay_seq: m.seq } : m;
        for (const sub of subscribers) {
          if (!admitForRole(sub.role, m, { mute })) continue;
          if (sub.socket.writableLength > highWaterMark) {
            log(`[daemon] dropping slow ${sub.role} client (writableLength=${sub.socket.writableLength})`);
            sub.socket.destroy();               // 'close' handler deregisters it
            continue;
          }
          // Best-effort write; real dead-socket reclamation is the close/error handler above.
          sub.socket.write(encodeFrame({ type: "message", message: wire }));
        }
      },
    },
    clientCount: () => subscribers.size,
    listen: async () => {
      assertSafeHome();
      prepareSocketPath();                  // caller (startDaemon) holds the consumer lock
      await new Promise((resolve, reject) => {
        server.once("error", reject);
        server.listen(socketPath(), resolve);
      });
      chmodSync(socketPath(), 0o600);
      const st = lstatSync(socketPath());   // re-stat after listen: TOCTOU bind-hijack guard (spec §5)
      if (!st.isSocket() || (st.mode & 0o777) !== 0o600 || (process.getuid && st.uid !== process.getuid())) {
        await new Promise((res) => server.close(res));
        throw new Error("socket failed post-listen owner/mode verification");
      }
    },
    close: async () => {
      for (const sub of subscribers) sub.socket.destroy();
      subscribers.clear();
      await new Promise((res) => server.close(res));
      try { rmSync(socketPath(), { force: true }); } catch { /* best effort */ }
    },
  };
}
```

- [ ] **Step 4: Run to verify pass**

Run: `node --test test/daemon-ipc.test.mjs`
Expected: PASS (20/20).

- [ ] **Step 5: Commit**

```bash
git add src/daemon-ipc.mjs test/daemon-ipc.test.mjs
git commit -m "feat(daemon): IPC server — gated broadcast, relay_seq stamping, HWM drop floor, pre-hello reaper"
```

---

### Task 5: `connectDaemon()` client helper

**Files:**
- Modify: `src/daemon-ipc.mjs` (append)
- Test: `test/daemon-ipc.test.mjs` (append)

- [ ] **Step 1: Write the failing tests** (append)

```js
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
```

- [ ] **Step 2: Run to verify failure**

Run: `node --test test/daemon-ipc.test.mjs`
Expected: FAIL — `connectDaemon` not exported.

- [ ] **Step 3: Implement** (append; add `createConnection` to the `node:net` import)

```js
import { createConnection } from "node:net";

/** Connect to the local daemon socket as `role`. Resolves AFTER hello-ok with a {close()}
 *  handle; gated messages stream to onMessage(m). Rejects with {code:"DAEMON_DOWN"} when no
 *  daemon is reachable (callers use that to fall back to legacy standalone — spec §7).
 *  Reconnect/backoff is Phase 4; Phase 2 surfaces onClose and lets the caller decide. */
export function connectDaemon({ role, onMessage, onClose = () => {}, handshakeMs = 3000, log = (s) => process.stderr.write(s + "\n") }) {
  return new Promise((resolve, reject) => {
    const sock = createConnection(socketPath());
    const fail = (reason, cause) => {
      sock.destroy();
      reject(Object.assign(new Error(reason), { code: "DAEMON_DOWN", cause }));
    };
    const timer = setTimeout(() => fail("daemon handshake timed out"), handshakeMs);
    let ready = false;

    sock.once("error", (e) => { if (!ready) { clearTimeout(timer); fail(`no daemon: ${e.code}`, e); } });
    sock.once("connect", () => sock.write(encodeFrame({ type: "hello", role })));
    const feed = makeLineParser((frame) => {
      if (!ready) {
        clearTimeout(timer);
        if (frame.type === "hello-ok") {
          ready = true;
          log(`[client] attached to air-msgd pid=${frame.pid} as ${role}`);
          resolve({ close: () => sock.destroy() });
        } else {
          fail(`daemon refused: ${frame.reason ?? frame.type}`);
        }
        return;
      }
      if (frame.type === "message") onMessage(frame.message);
      // pong + unknown server frames: ignored.
    }, { onError: (e) => log(`[client] bad frame from daemon: ${e.message}`) });
    sock.on("data", feed);
    sock.on("close", () => { if (ready) onClose(); });
  });
}
```

- [ ] **Step 4: Run to verify pass**

Run: `node --test test/daemon-ipc.test.mjs`
Expected: PASS (23/23).

- [ ] **Step 5: Commit**

```bash
git add src/daemon-ipc.mjs test/daemon-ipc.test.mjs
git commit -m "feat(daemon): connectDaemon() client — handshake, gated stream, DAEMON_DOWN fallback signal"
```

---

### Task 6: Wire the socket sink into the daemon

**Files:**
- Modify: `src/daemon.mjs` (startDaemon only — runDaemon is already sink-agnostic)
- Test: `test/daemon-ipc.test.mjs` (append — composition test through runDaemon)

- [ ] **Step 1: Write the failing composition test** (append)

```js
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
```

- [ ] **Step 2: Run to verify it passes already** (this is a composition proof, not new logic)

Run: `node --test test/daemon-ipc.test.mjs`
Expected: PASS (24/24) — `runDaemon` + `ipc.sink` compose with zero changes. (If it fails, fix before proceeding.)

- [ ] **Step 3: Wire `startDaemon`** — in `src/daemon.mjs`, add the import:

```js
import { createIpcServer } from "./daemon-ipc.mjs";
```

**Edit boundary (exact):** the lines
```js
  const startTime = new Date().toISOString();
  writeDaemonPid({ pid: process.pid, startTime });
```
**stay untouched** — the new block references `startTime`. Replace ONLY from `const mute = parseMuteSet();` through the end of the existing `try/finally` with:

```js
  const mute = parseMuteSet();
  const notifier = await createNotifier();         // click-to-open is a later-phase item (see bannerSink)
  const ipc = createIpcServer({
    mute,
    daemonInfo: { pid: process.pid, start_time: startTime, did: identity.did },
    log,
  });
  await ipc.listen();                              // safe: we hold the consumer lock (single-daemon mutex)
  const sinks = [bannerSink({ notifier, mute }), ipc.sink];

  const ac = new AbortController();
  const stop = () => ac.abort();
  process.once("SIGINT", stop);
  process.once("SIGTERM", stop);
  try {
    await runDaemon({ identity, sinks, signal: ac.signal, log });
  } finally {
    await ipc.close();                             // unlinks the socket
    clearDaemonPid();
    releaseConsumerLock();
  }
```

- [ ] **Step 4: Full daemon tests still green**

Run: `node --test test/daemon-ipc.test.mjs test/daemon.test.mjs test/daemon-pid.test.mjs test/daemon-sinks.test.mjs test/fanout.test.mjs`
Expected: ALL PASS.

- [ ] **Step 5: Commit**

```bash
git add src/daemon.mjs test/daemon-ipc.test.mjs
git commit -m "feat(daemon): socket sink wired into startDaemon — N gated subscribers off one pull"
```

---

### Task 7: `channel-server.mjs` → daemon-first thin client

**Files:**
- Modify: `src/channel-server.mjs`

**Lifecycle decision (explicit, per review):** on daemon disconnect the attached client **exits 0** — a clean "daemon went away" shutdown, NOT a crash. Whether the MCP host auto-relaunches on exit is unverified (open question recorded in the spec §11 by Task 8); Phase 4's reconnect/backoff supersedes this stopgap either way. Both branches get symmetric SIGINT/SIGTERM handling so the asymmetry can't be misread as an oversight.

- [ ] **Step 1: Refactor `main()`** — replace the current `main()` with:

```js
async function main() {
  await server.connect(new StdioServerTransport());
  const identity = await ensureIdentity();
  const mute = parseMuteSet();
  const log = (s) => process.stderr.write(s + "\n");
  // makeChannelPush keeps its own gate — harmless double-gating: the DAEMON's copy is the
  // security boundary (other clients can't skip it); this one is local defense-in-depth.
  const push = makeChannelPush(server, { mute, me: { airId: identity.air_id, did: identity.did } });

  // Daemon-first (spec §7): attach as a gated channel client — NO consumer lock held.
  try {
    await connectDaemon({
      role: "channel",
      onMessage: push,
      onClose: () => {
        // Deliberate Phase-2 stopgap: clean exit on daemon restart/stop. Mail push pauses
        // until this server is relaunched; Phase 4 replaces this with reconnect/backoff.
        log("air-msg-channel: daemon connection closed — exiting cleanly (Phase 4 adds reconnect)");
        process.exit(0);
      },
      log,
    });
    log(`air-msg-channel v${CORE_VERSION} attached to air-msgd for ${identity.did} (gate enforced by daemon)`);
    process.once("SIGINT", () => process.exit(0));   // no lock held on this path — plain clean exit
    process.once("SIGTERM", () => process.exit(0));
    await new Promise(() => {});                     // stay alive until a signal or daemon close
  } catch (e) {
    if (e.code !== "DAEMON_DOWN") throw e;
    log("air-msg-channel: no daemon — running standalone (legacy)");
  }

  // Legacy standalone fallback: unchanged Phase-1 behavior, takes the single consumer lock.
  if (!acquireOrExit("channel-server")) return;
  const ac = new AbortController();
  process.once("SIGINT", () => { ac.abort(); releaseConsumerLock(); });
  process.once("SIGTERM", () => { ac.abort(); releaseConsumerLock(); });
  process.stderr.write(`air-msg-channel v${CORE_VERSION} watching ${identity.did} (push gate: verified+pinned)\n`);
  await watch({
    signal: ac.signal,
    identity,
    notifier: { notify: async () => {} },
    openResolver: () => null,
    onMessage: push,
  }).catch((e) => { if (e?.name !== "AbortError") throw e; });   // clean signal shutdown
  releaseConsumerLock();
}
```

Add the import at the top: `import { connectDaemon } from "./daemon-ipc.mjs";`
Note `push` is now built once and shared by both paths (it was inline in the `watch()` call before).

- [ ] **Step 2: Syntax + suite check**

Run: `node --check src/channel-server.mjs && node --test test/rooms-channel.test.mjs test/daemon-ipc.test.mjs`
Expected: clean check; ALL PASS (channel.mjs logic untouched — only the server shell changed).

- [ ] **Step 3: Commit**

```bash
git add src/channel-server.mjs
git commit -m "refactor(channel): daemon-first thin client — lock-free attach, clean exit-0 stopgap, legacy fallback"
```

---

### Task 8: Spec notes (§11) + full verification + live smoke

**Files:**
- Modify: `agent-bridge-mcp/docs/superpowers/specs/2026-06-05-receiver-daemon-design.md` (§11)

- [ ] **Step 1: Record the resolved + new open questions** — in the spec's §11, replace the first bullet with:

```markdown
- ~~Does the relay's `/pull` SSE `since` guarantee no gap if two consumers briefly overlap…~~
  **RESOLVED 2026-06-10:** `/pull` (poll AND SSE) filters `acked_at IS NULL` and is cursor-driven
  (`since=N`) — see `~/air-site/relay/src/index.js` L193–237. Daemon and legacy share one
  `AGENT_BRIDGE_HOME` → one cursor + the archive's `(envelope_id, direction)` PK dedup, so a brief
  handoff overlap is at-least-once, never lossy.
```

and append two bullets:

```markdown
- Phase 2 note: the wire protocol stamps `relay_seq` from the pushed object's `seq` at the socket
  boundary (`core.receive()`'s onMessage objects carry `seq`; `relay_seq` exists only on archive
  rows) — Phase 3's gap/replay keys on `message.relay_seq` with no frame change.
- Open (Phase 4): does the MCP host auto-relaunch a channel server that exits 0 when the daemon
  goes away? Phase 2 ships clean-exit-on-disconnect as a stopgap; reconnect/backoff supersedes it.
```

- [ ] **Step 2: Full suite + hermeticity proof**

```bash
cd ~/air-note/agent-bridge-mcp
before=$(sqlite3 -readonly ~/.air-msg/archive.db "SELECT COUNT(*) FROM messages")
node --test 2>&1 | grep -E "^ℹ (tests|pass|fail|todo)"
after=$(sqlite3 -readonly ~/.air-msg/archive.db "SELECT COUNT(*) FROM messages")
echo "real-archive delta: $((after-before)) (must be 0)"
```
Expected: `fail 0`, delta 0. (Baseline before this plan: 236 tests / 233 pass / 3 todo; Phase 2 adds 24.)

- [ ] **Step 3: ⚠️ MANUAL — do not run unattended.** Live smoke (registers ONE throwaway agent against production AIR — a human runs this, then cleans up):

```bash
# Terminal A — daemon on a throwaway home (short /tmp path: unix sockets cap at ~104 chars):
export AGENT_BRIDGE_HOME=$(mktemp -d /tmp/airsmoke.XXXX)
node src/cli.mjs register --name "phase2-smoke" && node src/cli.mjs daemon start
# Terminal B — attach a viewer through the real socket:
export AGENT_BRIDGE_HOME=<same dir>
node -e 'import("./src/daemon-ipc.mjs").then(async ({ connectDaemon }) => {
  await connectDaemon({ role: "viewer", onMessage: (m) => console.log("VIEWER:", m.body?.text) });
})' &
# Terminal B — self-send (delivered back through the relay):
node src/cli.mjs send <own-AIR-id-from-register> "phase2 smoke"
# EXPECT: Terminal A logs the banner sink firing; Terminal B prints "VIEWER: phase2 smoke".
# Ctrl-C the daemon → clean exit, daemon.sock + daemon.pid + consumer.lock all gone.
# Kill the backgrounded viewer (kill %1) — note: a backgrounded-and-forgotten viewer is exactly
# the stuck-client case the HWM drop floor exists for.
```
Cleanup: demote the throwaway agent —
`cd ~/air-site/api && npx wrangler d1 execute air-registry --remote --command "UPDATE agents SET is_demo=1 WHERE name='phase2-smoke'"`

- [ ] **Step 4: Commit + PR**

```bash
git add agent-bridge-mcp/docs/superpowers/specs/2026-06-05-receiver-daemon-design.md
git commit -m "docs(daemon): spec §11 — Q1 resolved (overlap at-least-once); relay_seq stamping + exit-0 stopgap noted"
git push -u origin feat/daemon-phase2-socket
gh pr create --repo AgentIdentityRegistry/air-note --base main \
  --title "feat(daemon): Phase 2 — socket + daemon-enforced per-subscriber gate + channel thin-client"
```
PR body: summarize spec §5 enforcement, the critic-review hardening (relay_seq stamping, HWM floor, pre-hello reaper, exit-0 lifecycle), test counts, hermeticity delta-0. The live smoke (Step 3) is run by a human BEFORE merge; paste its output in the PR. Merge on green CI.

---

## Self-Review (against spec §5/§7/§9 + the 2026-06-10 critic review)

- **§3 components:** `daemon-ipc.mjs` (Tasks 1–5), `channel-server` thin client (Task 7), daemon wiring (Task 6). `service/*.mjs` is Phase 4. ✓
- **§5 gate:** admission lives in the DAEMON (Task 2 + Task 4's per-subscriber deliver); the explicit invariant test is Task 4's gating test + Task 6's composition test. 0600 socket + unsafe-home refusal + post-listen re-stat: Tasks 3–4. The lock⇒unlink⇒bind invariant now has its own test (Task 4 test 2). Untrusted-body fence: untouched. **Critic confirmed the gate sound — preserved unchanged through rework.** ✓
- **§5 `bridge` role:** deliberately deferred — the bridge is an in-process sink (spec §3); a socket `bridge` role lands in Phase 4 when the bridge becomes detachable. `ROLES` intentionally rejects it until then (documented in code + protocol section). ✓
- **§6 delivery:** rich semantics (buffers, gap/replay, at-least-once) remain Phase 3 — but Phase 2 ships the **flow-control floor** (HWM drop + log + test) because zero backpressure on an always-on process is a regression, not a deferral. `relay_seq` is stamped at the boundary so Phase 3 needs no frame change (the v1 plan's claim was factually wrong — fixed). ✓
- **§7 resolution:** Phase 2 ships the two rows that matter for the channel (socket live → attach; no daemon → standalone fallback via `DAEMON_DOWN`); stale-socket unlink happens daemon-side under the lock (tested). Disconnect → **exit 0** (deliberate stopgap, documented in code + spec §11); full decision table + reconnect + `EADDRINUSE` bind-loser = Phase 4. ✓
- **§9 testing:** framing unit tests incl. ~100 KB round-trip + loud over-cap (T1), gate admit/deny matrix incl. a real room (T2), socket lifecycle over a real Unix socket incl. stale-file bind, stuck-client drop, pre-hello reap, duplicate-hello pin (T4–T5), stubbed-relay-equivalent composition through `runDaemon` (T6), MANUAL live smoke (T8). All waits are condition-polled (`until()`), never fixed sleeps. ✓
- **Placeholder scan:** none — every step has complete code/commands. Names consistent: `encodeFrame`, `makeLineParser`, `MAX_FRAME`, `ROLES`, `admitForRole`, `assertSafeHome`, `prepareSocketPath`, `socketPath`, `createIpcServer` (`sink`/`listen`/`close`/`clientCount`, opts `highWaterMark`/`helloTimeoutMs`), `connectDaemon` (`DAEMON_DOWN`), `ipcFor`/`rawClient`/`until` test helpers. ✓
- **Known risks:** Unix-socket path length (~104-byte cap) — temp homes are short; a very deep `AGENT_BRIDGE_HOME` makes `listen()` throw cleanly. The HWM drop floor means a wedged `channel` client silently loses its stream until relaunch — acceptable ONLY because Phase 3's gap/replay is the designed remedy; the drop is logged loudly daemon-side.
