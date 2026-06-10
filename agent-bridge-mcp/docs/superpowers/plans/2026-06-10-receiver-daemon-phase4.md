# Receiver Daemon — Implementation Plan (Phase 4: reconnect, resume-on-reattach, §7 table, installers)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** v2 — v1 review returned **APPROVE-WITH-FIXES** (all wire-fact citations verified by the reviewer; the persistent wrapper's restart-resume and close-stops-loop scenarios were reproduced verbatim against the real server and passed; server-destroy was probed to fire client-side `close`, which Task 4 depends on). v2 applies every finding: **H1** systemd quoting marked as a manual-verify boundary + de-tautologized test comment; **H2** real identity fixture (`seed_hex` is the load-bearing field); **M1** baseline snapshot moved BEFORE the first connect (the reviewer probed a real loss window: a frame unparsed in the client buffer while the cursor advances past it); **M3** status reply excludes the requesting subscriber; **M5** quoted `node-version`; plus a backoff-escalation test, the watch-insertion disambiguation, rationale comments (sleep-before-retry, silent deliberate close, room notices in the viewer feed), log path beside the resolved home, and the half-install recovery hint. v3 — Task 2's post-landing quality review demonstrated two lifecycle defects in this plan's own Step-3 code (close-during-in-flight undead socket; throwing onAttach masquerading as a failed connect); code and plan amended together, with deterministic seam-based regression tests. Task 3's composition review added two fixes: a stdin-'end' orphan guard in channel-server (SDK onclose never fires on host death — probed), and gap-replay pagination in makeReplayer (page past the 500-row limit; short page terminates) with a deterministic pageSize=2 test. Task 4 amendment: the backstop-recovery test resumes the paused socket after destroy (paused sockets defer 'close'); liveness pings rejected. Task 5 amendment: watch viewer holds a ref'd keepAlive (unref'd backoff timer cannot pin the loop — probed exit-0 mid-outage); `log: () => {}` silences raw transport lines from the curated stdout feed; spawn harness waits exits via a consumed-event-safe `waitExit` helper; survival test added as C1 regression guard. Task 7 amendment: queryDaemonStatus guards the timeout-vs-handshake race (same undead-socket class as 2a59f75) with a connectFn seam test; writeDaemonPid moved after listen (no stranded PID on failed bind; kills a benign split-brain warning window). Task 8 amendment: systemd `Environment=` quoted (a space in the home path would silently split the assignment — wrong-data, not an error); servicePlan now exposes `logPath` on both returned plan objects; Task 9 must `mkdirSync(dirname(plan.logPath), { recursive: true })` because launchd opens StandardOutPath pre-spawn and will not create missing parent directories. Task 9 amendments (review): HELP bridge claim corrected (bridge refuses, not attaches); --detach hoists ensureIdentity into the parent (first-run registration raced the 3s poll into an orphan daemon); honest uninstall/already-running messages; ENOENT surfaced.

**Goal:** Complete the daemon's roadmap-Phase-1 plumbing (spec §7 + §8): clients auto-reconnect with backoff and resume via `since_seq` in hello (closing the at-least-once "OR reconnect" trigger from §6); `air-msg watch` and `air-msg bridge` get their §7 decision-table rows; `air-msg daemon status` reports live socket state over IPC; `air-msg daemon install|uninstall` generates + loads launchd/systemd-user units; `daemon start --detach` backgrounds the process.

**Architecture:** All reconnect logic lives client-side in `daemon-ipc.mjs` as a `connectDaemonPersistent()` wrapper around the existing `connectDaemon()` (which stays API-stable). The hello frame gains an optional `since_seq`; the daemon answers a channel hello carrying it with an immediate `{type:"gap", after_seq: since_seq}` — the client's EXISTING Phase-3 replayer then replays the hole from the archive. The daemon stays stateless about client history (the client's archive is the replay source; strict cursor already guarantees it is complete). Installers are pure content generators in a new `src/service.mjs` with thin CLI wiring; the `launchctl`/`systemctl` *load* step is exercised manually (spec §9).

**Carried critic flags — resolutions baked into this plan:**
- *"Should a 4×HWM backstop-destroy emit a final gap hint?"* → **No.** A wedged socket cannot usefully receive a hint (writing to it is the failure mode). Recovery is the reconnect path: a destroyed channel client reconnects with `since_seq` and replays. Task 4 proves this end-to-end over real sockets; Task 11 documents it in spec §6.
- *"Linux small-SO_SNDBUF cross-check of the overflow-test sizing."* → **Resolved empirically by CI.** The suite is hermetic since PR #11; Task 10 adds a GitHub Actions job running the full messaging suite on `ubuntu-latest`, which exercises the Phase-3 overflow tests under a Linux kernel's unix-socket buffer defaults. The tests' existing `clientStats().dropped > 0` positive assertions fail LOUDLY if sizing assumptions don't hold there.

**Tech Stack:** Node ≥22 stdlib only. No new dependencies.

**Spec:** `agent-bridge-mcp/docs/superpowers/specs/2026-06-05-receiver-daemon-design.md` §6 (reconnect gap trigger), §7 (decision table + reconnect contract), §8 (lifecycle/install/status), §9 (testing), §11 (open question: MCP-host relaunch — closed by reconnect).

**Repo rules that bind every task:** temp-home idiom in every test (`bridgeHome()` THROWS under the runner without `AGENT_BRIDGE_HOME`); import shared helpers, never copy; bare `node --test` only as single files (`node --test test/<file>`); branch `feat/daemon-phase4` from current `main` (`58a8d9c`); work from `~/air-note/agent-bridge-mcp` (the CI workflow file in Task 10 is the one exception — it lives at the REPO ROOT `.github/workflows/`).

**Wire facts this plan relies on (verified 2026-06-10 against main `58a8d9c`):**
- `connectDaemon({role, onMessage, onGap, onClose, handshakeMs, log})` resolves `{close, _sock}` after hello-ok; rejects `{code:"DAEMON_DOWN"}` (daemon-ipc.mjs:217); hello is `{type:"hello", role}` (daemon-ipc.mjs:228); the server ignores unknown post-hello frames (daemon-ipc.mjs:144) and unknown hello FIELDS (only `frame.type`/`frame.role` are read, daemon-ipc.mjs:113) — `since_seq` is backward/forward compatible.
- Server hello branch creates `sub = {socket, role, lastSeq, dropped}` then writes `hello-ok` spread with `daemonInfo = {pid, start_time, did}` (daemon-ipc.mjs:119-121, daemon.mjs:84).
- `deliver()` stamps `relay_seq` from `m.seq` and tracks per-sub `lastSeq` (daemon-ipc.mjs:160-185); gap frames are `{type:"gap", after_seq}` (daemon-ipc.mjs:132).
- `channel-server.mjs` exit-0 stopgap at channel-server.mjs:47-51; daemon-first try/catch falls back to standalone on `DAEMON_DOWN` (channel-server.mjs:59-62); replayer wiring at channel-server.mjs:42-46.
- `cli.mjs` watch case acquires the lock immediately and runs standalone watch (cli.mjs:275-308); bridge case at cli.mjs:310-374; daemon subcommands start|stop|status only (cli.mjs:575-604); HELP's launchd snippet (cli.mjs:135-144) uses `/usr/bin/env air-msg` — broken under launchd (launchd provides no user PATH), superseded by Task 8's absolute-path generators.
- `daemonStatus()` returns `{running, pid, start_time, cursor}` reading the PID file + `archiveExists()`-gated `getCursor()` (daemon.mjs:59-71). `getCursor`/`archiveExists` are exported by archive.mjs (imported at daemon.mjs:11).
- `startDaemon()` holds the consumer lock BEFORE `ipc.listen()` (daemon.mjs:76-87); `prepareSocketPath()` unlinks ONLY under the lock (daemon-ipc.mjs:80-82); `isDaemonRunning(isAlive)` checks PID file + liveness (daemon.mjs:46-49).
- Test helpers: `ipcFor(over)` builds a server with `DAEMON_INFO={pid:4242, start_time:"2026-06-10T00:00:00.000Z", did:"did:wba:me"}`; `rawClient(role)` connects + hellos + collects frames; `until(cond, ms)` polls at 5 ms (test/daemon-ipc.test.mjs:19-156). Temp-home `beforeEach`/`afterEach` idiom at test/daemon-ipc.test.mjs:9-15.
- `package.json`: `bin: {"air-msg": "src/cli.mjs"}`, engines `>=22`, test script `node --test`. agent-bridge-mcp is a STANDALONE npm package, NOT in the root npm workspace.
- `ensureIdentity()` is network-silent when `identity.json` exists: `loadIdentity()` (identity.mjs:60-67) re-derives the keypair via `generateIdentity(stored.seed_hex)` — **`seed_hex` is the load-bearing field**; a fixture without it silently yields a fresh, unrelated key instead of failing loudly (critic v1 H2, verified). The real on-disk shape carries `version, name, air_id, did, seed_hex, public_key_base64url, public_key_multibase, relay_url, air_url, agent_secret` (identity.mjs:133).
- The daemon `case`'s `rest` is the raw argv tail (`["daemon","start","--detach"]`), so `rest.includes("--detach")` works (critic-verified in scope at cli.mjs:576) — do not "helpfully" refactor it to the `parseRoomArgs` output.
- **Still to verify at execution:** whether agent-bridge-mcp has a `package-lock.json` (Task 10 uses `npm ci` if yes, `npm install` if no).

---

### Task 1: `since_seq` in hello — resume-on-reattach (both ends of the protocol)

**Files:**
- Modify: `src/daemon-ipc.mjs` (server hello branch ~L113-141; `connectDaemon` ~L217-248)
- Test: `test/daemon-ipc.test.mjs` (extend `rawClient`, append tests)

- [ ] **Step 1: Extend the `rawClient` helper** to pass extra hello fields (backward compatible):

In `test/daemon-ipc.test.mjs`, change the `rawClient(role)` signature to `rawClient(role, helloExtra = {})` and the hello write to:
```js
    sock.write(encodeFrame({ type: "hello", role, ...helloExtra }));
```

- [ ] **Step 2: Write the failing tests** (append):

```js
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

test("hello with since_seq seeds lastSeq: an overflow gap before any write resumes from since_seq, not 0", async () => {
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
```

- [ ] **Step 3: Run to verify failure**

Run: `node --test test/daemon-ipc.test.mjs`
Expected: the four new tests FAIL (no gap frames arrive; `lastSeq` is null; `sinceSeq` ignored).

- [ ] **Step 4: Implement.** In `src/daemon-ipc.mjs`:

(a) Server — in the hello branch, replace the `sub = ...` line (daemon-ipc.mjs:119) with:
```js
        sub = { socket, role: frame.role, lastSeq: Number.isFinite(frame.since_seq) ? frame.since_seq : null, dropped: 0 };
```
and AFTER the `socket.on("drain", flushPending);` line (end of the hello branch), add:
```js
        // Resume-on-reattach (spec §6 "OR reconnect"): a channel client that declares where it
        // left off gets an immediate gap; its Phase-3 replayer fills the hole from the archive.
        // The daemon stays stateless about client history — the client's archive is the source.
        // Viewer/bridge are best-effort roles: since_seq is ignored for them.
        if (sub.role === "channel" && Number.isFinite(frame.since_seq)) {
          socket.write(encodeFrame({ type: "gap", after_seq: frame.since_seq }));
          log(`[daemon] resume: gap → channel client (after_seq=${frame.since_seq})`);
        }
```

(b) Client — extend `connectDaemon`'s signature with `sinceSeq` (after `onClose`):
```js
export function connectDaemon({ role, onMessage, onGap = () => {}, onClose = () => {}, sinceSeq, handshakeMs = 3000, log = (s) => process.stderr.write(s + "\n") }) {
```
and the hello write (daemon-ipc.mjs:228) becomes:
```js
    sock.once("connect", () => sock.write(encodeFrame({
      type: "hello", role, ...(Number.isFinite(sinceSeq) ? { since_seq: sinceSeq } : {}),
    })));
```

- [ ] **Step 5: Run to verify pass**

Run: `node --test test/daemon-ipc.test.mjs`
Expected: ALL PASS (Phase 2/3 tests unaffected — no since_seq → behavior unchanged).

- [ ] **Step 6: Commit**

```bash
git add src/daemon-ipc.mjs test/daemon-ipc.test.mjs
git commit -m "feat(daemon): since_seq in hello — resume-on-reattach gap for channel clients"
```

---

### Task 2: `connectDaemonPersistent()` — reconnect with backoff + since_seq tracking

**Files:**
- Modify: `src/daemon-ipc.mjs` (new export, after `connectDaemon`)
- Test: `test/daemon-ipc.test.mjs` (append)

- [ ] **Step 1: Write the failing tests** (append):

```js
import { connectDaemonPersistent } from "../src/daemon-ipc.mjs";   // add to the existing import line

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

// Lifecycle regression tests (v3 fix): deterministic via the connectFn seam — no real sockets.

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
```

- [ ] **Step 2: Run to verify failure**

Run: `node --test test/daemon-ipc.test.mjs`
Expected: FAIL — `connectDaemonPersistent` not exported.

- [ ] **Step 3: Implement** (append to `src/daemon-ipc.mjs`, after `connectDaemon`):

```js
/** Persistent daemon client (spec §7 reconnect contract): wraps connectDaemon with auto-reconnect
 *  + exponential backoff (reset on a successful attach), and resume-on-reattach for `channel`.
 *
 *  Resume bookkeeping: tracks the max relay_seq seen on message frames; every REconnect sends
 *  since_seq = maxSeen ?? baseline, where baseline = cursorFn() snapshotted at the FIRST attach.
 *  The baseline closes the outage-window hole: a client that saw no frames before the daemon
 *  bounced would otherwise reattach live-from-attach and silently miss mail the restarted daemon
 *  pulled in between. cursorFn is injected (archive stays out of this transport module); the
 *  FIRST attach deliberately sends NO since_seq — a fresh session is live-from-attach (spec §4).
 *
 *  The returned promise settles like connectDaemon: rejects DAEMON_DOWN if the FIRST attempt
 *  finds no daemon (callers use that to fall back to legacy standalone, spec §7); after the
 *  first successful attach it never gives up until close().
 *
 *  Callbacks (onAttach, onDetach, onMessage, onGap) are invoked from socket event handlers and
 *  the reconnect loop; they are guarded against throws, but SHOULD not throw. */
export function connectDaemonPersistent({
  role, onMessage, onGap = () => {}, onAttach = () => {}, onDetach = () => {},
  cursorFn = () => null,
  initialBackoffMs = 500, backoffCapMs = 5000,
  connectFn = connectDaemon,                            // test seam
  log = (s) => process.stderr.write(s + "\n"),
}) {
  let stopped = false;
  let current = null;
  let maxSeen = null;
  // Baseline BEFORE the first connect (critic v1 M1, probed): a frame can sit unparsed in this
  // client's socket buffer while the daemon advances the cursor past it — a post-attach snapshot
  // can then skip that seq on resume (replaySince is strictly-greater-than). An early, over-broad
  // baseline only costs a few envelope_id-deduped replays; a late one loses mail.
  let baseline = null;
  try { baseline = cursorFn(); } catch { baseline = null; }
  const seen = (m) => {
    if (m && Number.isFinite(m.relay_seq) && (maxSeen === null || m.relay_seq > maxSeen)) maxSeen = m.relay_seq;
    onMessage(m);
  };

  const reconnectLoop = async () => {
    let backoff = initialBackoffMs;
    while (!stopped) {
      // Sleep BEFORE the first retry on purpose: onClose means the daemon is gone RIGHT NOW —
      // an immediate attempt would just burn an ECONNREFUSED. unref: a closed handle must not
      // pin the event loop for a pending backoff tick.
      await new Promise((r) => { const t = setTimeout(r, backoff); t.unref?.(); });
      if (stopped) return;
      const sinceSeq = maxSeen ?? baseline ?? undefined;
      let next;
      try {
        next = await connectFn({
          role, sinceSeq, onMessage: seen, onGap,
          onClose: () => { if (!stopped) { try { onDetach(); } catch (e) { log(`[client] onDetach callback threw: ${e?.message ?? e}`); } void reconnectLoop(); } },
          log,
        });
      } catch {
        backoff = Math.min(backoff * 2, backoffCapMs);  // daemon still down — keep trying
        continue;
      }
      if (stopped) { next.close(); return; }  // close() raced the in-flight handshake — kill the undead socket
      current = next;
      // User callbacks OUTSIDE the try: a throwing onAttach must not masquerade as a failed
      // connect (it would silently pile up duplicate live connections). Loud log, no retry.
      try { onAttach(); } catch (e) { log(`[client] onAttach callback threw: ${e?.message ?? e}`); }
      log(`[client] reattached to air-msgd as ${role}${Number.isFinite(sinceSeq) ? ` (resume since_seq=${sinceSeq})` : ""}`);
      return;
    }
  };

  return connectFn({
    role, onMessage: seen, onGap,
    onClose: () => { if (!stopped) { try { onDetach(); } catch (e) { log(`[client] onDetach callback threw: ${e?.message ?? e}`); } void reconnectLoop(); } },
    log,
  }).then((first) => {
    current = first;
    onAttach();
    return {
      // A deliberate close() is SILENT (no onDetach): `stopped` flips first, so the socket's
      // close event takes the if(!stopped) branch. Don't "fix" this into a spurious detach log.
      close: () => { stopped = true; current?.close(); },
      _sock: () => current?._sock,                      // test seam (backstop-recovery round-trip)
    };
  });
}
```

- [ ] **Step 4: Run to verify pass**

Run: `node --test test/daemon-ipc.test.mjs`
Expected: ALL PASS.

- [ ] **Step 5: Commit**

```bash
git add src/daemon-ipc.mjs test/daemon-ipc.test.mjs
git commit -m "feat(daemon): connectDaemonPersistent — reconnect/backoff + since_seq resume (spec §7)"
```

---

### Task 3: channel-server reconnects (the exit-0 stopgap dies)

**Files:**
- Modify: `src/channel-server.mjs` (the daemon-first block, channel-server.mjs:40-58)

No new unit test: the persistent wrapper is fully tested in Task 2 and the replayer in Phase 3; this is 10 lines of wiring verified by `node --check` + review (the Phase-3 plan set this precedent for channel-server wiring — state the boundary honestly in the commit).

- [ ] **Step 1: Implement.** In `src/channel-server.mjs`:

(a) Replace the `connectDaemon` import (channel-server.mjs:17) with:
```js
import { connectDaemonPersistent } from "./daemon-ipc.mjs";
```
(b) Add next to the other imports:
```js
import { getCursor, archiveExists } from "./archive.mjs";
```
(c) Replace the whole `await connectDaemon({ ... });` call (channel-server.mjs:43-54) with:
```js
    await connectDaemonPersistent({
      role: "channel",
      onMessage: (m) => replayer.live(m),
      onGap: (after_seq) => { replayer.gap(after_seq).catch((e) => log(`[channel] replay failed: ${e.message ?? e}`)); },
      // Resume baseline: the archive cursor at first attach. Same home as the daemon, read-only,
      // archiveExists()-gated so a status probe never materializes a fresh DB (daemon.mjs precedent).
      cursorFn: () => (archiveExists() ? getCursor() : null),
      onDetach: () => log("air-msg-channel: daemon connection lost — reconnecting with backoff"),
      onAttach: () => log("air-msg-channel: attached to air-msgd (gate enforced by daemon)"),
      log,
    });
```
(d) Delete the now-stale `Phase-2 stopgap` comment block and the `attached to air-msgd` log line that followed the old call (channel-server.mjs:55) — `onAttach` replaces it. The `process.once("SIGINT"/"SIGTERM")` + `await new Promise(() => {})` lines stay: the server now lives across daemon restarts.

- [ ] **Step 2: Verify + run the adjacent suites**

Run: `node --check src/channel-server.mjs && node --test test/daemon-ipc.test.mjs test/channel-replay.test.mjs`
Expected: clean check; ALL PASS.

- [ ] **Step 3: Commit**

```bash
git add src/channel-server.mjs
git commit -m "feat(channel): reconnect with backoff replaces the Phase-2 exit-0 stopgap (wiring; logic tested in daemon-ipc)"
```

---

### Task 4: Backstop-destroy recovery round-trip (carried critic flag (a), proven)

**Files:**
- Test: `test/daemon-ipc.test.mjs` (append — integration test only, no production code expected)

- [ ] **Step 1: Write the test** (append):

```js
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
```

- [ ] **Step 2: Run to verify pass**

Run: `node --test test/daemon-ipc.test.mjs`
Expected: PASS with code from Tasks 1–2 only. If it FAILS, that is a real Phase-4 integration bug — fix forward in `connectDaemonPersistent` (the pause-wedge defers ALL client stream events including 'close' — the test must resume() after the destroy to model a recovering consumer; client-side liveness pings were prototyped and REJECTED: a blocked production event loop defers close identically and defeats timers too, and local Unix sockets have no half-open failure mode).

- [ ] **Step 3: Commit**

```bash
git add test/daemon-ipc.test.mjs
git commit -m "test(daemon): backstop-destroy recovery round-trip — reconnect+since_seq IS the final-gap-hint resolution"
```

---

### Task 5: §7 table — `air-msg watch` attaches as viewer; stale-socket hygiene

**Files:**
- Modify: `src/daemon-ipc.mjs` (new tiny export `cleanStaleSocket`)
- Modify: `src/cli.mjs` (watch case, cli.mjs:275-308)
- Test: `test/daemon-ipc.test.mjs` (cleanStaleSocket), `test/cli-daemon-table.test.mjs` (new — spawn tests for the §7 rows)

- [ ] **Step 1: Write the failing unit test for the hygiene helper** (append to `test/daemon-ipc.test.mjs`):

```js
import { cleanStaleSocket } from "../src/daemon-ipc.mjs";   // add to the existing import line

test("cleanStaleSocket: removes a stale socket file; never throws when absent", () => {
  chmodSync(dir, 0o700);
  writeFileSync(socketPath(), "");                     // stale leftover from a crashed daemon
  cleanStaleSocket();
  assert.equal(existsSync(socketPath()), false);
  cleanStaleSocket();                                  // absent → still fine
});
```
(Add `writeFileSync`, `existsSync` to the test file's `node:fs` import, and `socketPath` to the daemon-ipc import.)

- [ ] **Step 2: Run to verify failure**

Run: `node --test test/daemon-ipc.test.mjs`
Expected: FAIL — `cleanStaleSocket` not exported.

- [ ] **Step 3: Implement the helper** (in `src/daemon-ipc.mjs`, next to `prepareSocketPath`):

```js
/** §7 stale-socket hygiene for LEGACY entrypoints: after a standalone consumer ACQUIRED the
 *  consumer lock (the lock proves no live daemon owns the path), remove any leftover socket so
 *  later probes fail fast with ENOENT instead of ECONNREFUSED. Never call without the lock. */
export function cleanStaleSocket() {
  try { rmSync(socketPath(), { force: true }); } catch { /* best effort */ }
}
```

- [ ] **Step 4: Wire the watch row.** In `src/cli.mjs`, inside `case "watch":` (cli.mjs:275), insert the daemon-attach block BETWEEN `const identity = await ensureIdentity();` (cli.mjs:276) and the `if (!acquireOrExit("watch")) break;` line — i.e. ENTIRELY OUTSIDE the lock-bearing `try { ... } finally { releaseConsumerLock(); }` that follows. The viewer path holds no lock; if this block lands inside that try, its `break` would run a spurious `releaseConsumerLock()` in the finally (critic v1 ambiguity — this placement is the fix). Extract the feed-line printer so both paths share it. Note: a viewer feed has banner-equivalent visibility (spec §5), so room chat AND synthetic `room/joined` notices appear in it — `bodyText` already renders both (cli.mjs:77-83); consistent with the banner, not a bug.

```js
      const printFeedLine = (m) => {
        const who = m.contact ? m.contact : m.from;
        const enc = m.encrypted ? "🔒" : "✉️ ";
        const vrf = m.verified ? c.green("✓") : c.red("✗");
        console.log(`  ↓ ${enc} ${vrf} ${who}  ${c.dim(new Date().toISOString())}`);
        console.log(`    ${bodyText(m.body)}`);
      };
      // §7 row 1: socket live → attach as viewer (no lock, no second banner — the daemon's
      // bannerSink already rings; this terminal is a passive feed).
      try {
        const handle = await connectDaemonPersistent({
          role: "viewer",
          onMessage: printFeedLine,
          onAttach: () => console.log(`${c.green("● watching")} ${c.dim("(attached to air-msgd — daemon owns the pull; Ctrl-C detaches only this feed)")}`),
          onDetach: () => console.log(c.dim("  …daemon connection lost — reconnecting")),
          log: () => {},   // M1: suppress raw [client] transport lines from the curated stdout feed
        });
        // C1: connectDaemonPersistent holds only an unref'd backoff timer during outages — it
        // cannot pin the event loop. Signal listeners do not pin it either. This ref'd interval
        // keeps the process alive across daemon restarts until the user explicitly Ctrl-Cs.
        const keepAlive = setInterval(() => {}, 60_000);
        await new Promise((resolve) => {
          const stop = () => { console.log(c.dim("\n…detaching from daemon")); clearInterval(keepAlive); handle.close(); resolve(); };
          process.once("SIGINT", stop);
          process.once("SIGTERM", stop);
        });
        break;
      } catch (e) {
        if (e.code !== "DAEMON_DOWN") throw e;         // §7 rows 2-3: no daemon → legacy standalone below
      }
```
Then, in the standalone path that follows, replace the inline `onMessage` body (cli.mjs:296-303) with `onMessage: printFeedLine,` and add `cleanStaleSocket();` immediately after the existing `if (!acquireOrExit("watch")) break;` line (§7 row 2: lock acquired proves the socket is stale).
Finally add `connectDaemonPersistent, cleanStaleSocket` to cli.mjs's existing `./daemon-ipc.mjs` import (or create the import if absent — check the top of the file).

- [ ] **Step 5: Write the spawn test for the row** (create `test/cli-daemon-table.test.mjs`):

```js
// §7 decision-table rows, exercised through the REAL CLI (cli-args lesson: parser-level bugs
// are invisible to core unit tests). Each spawn gets a temp home with a PRE-SEEDED identity —
// VERIFY FIRST that ensureIdentity() is network-silent when identity.json exists; if it is not,
// delete this file and extend the unit coverage instead, saying so in the commit message.
// VERIFIED (empirically): ensureIdentity()/loadIdentity() re-derive the keypair from seed_hex —
// no network call when identity.json exists.
import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync, chmodSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { createIpcServer } from "../src/daemon-ipc.mjs";

const CLI = fileURLToPath(new URL("../src/cli.mjs", import.meta.url));
let dir;
beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "air-msg-table-"));
  chmodSync(dir, 0o700);
  process.env.AGENT_BRIDGE_HOME = dir;
  // REAL identity.json shape (fields from registerNewIdentity/loadIdentity in identity.mjs).
  // seed_hex is the LOAD-BEARING field: loadIdentity() re-derives the keypair via
  // generateIdentity(stored.seed_hex), and without it a fresh unrelated key is silently
  // generated instead of failing loudly (critic v1 H2). Public-key fields are re-derived
  // from seed_hex on load, so placeholders are fine; the relay/air URLs are never contacted by
  // the code paths these tests exercise (.invalid guards against that ever changing silently).
  writeFileSync(join(dir, "identity.json"), JSON.stringify({
    version: 1,
    name: "table-test",
    air_id: "AIR-TEST-TEST-TEST",
    did: "did:wba:agentidentityregistry.org:agents:AIR-TEST-TEST-TEST",
    seed_hex: "00".repeat(32),
    public_key_base64url: "",
    public_key_multibase: "",
    relay_url: "https://relay.invalid",
    air_url: "https://air.invalid",
    agent_secret: "test-secret",
  }), { mode: 0o600 });
});
afterEach(() => { rmSync(dir, { recursive: true, force: true }); });

const runCli = (args, { env = {} } = {}) => {
  const child = spawn(process.execPath, [CLI, ...args], {
    env: { ...process.env, AGENT_BRIDGE_HOME: dir, NO_COLOR: "1", ...env },
  });
  const out = { stdout: "", stderr: "" };
  child.stdout.on("data", (d) => { out.stdout += d; });
  child.stderr.on("data", (d) => { out.stderr += d; });
  return { child, out };
};

/** I1: a consumed 'exit' event never re-fires; waitExit is safe whether the child already
 *  exited (exitCode/signalCode set) or is still running (registers the once listener). */
const waitExit = (ch) => (ch.exitCode !== null || ch.signalCode !== null)
  ? Promise.resolve(ch.exitCode)
  : new Promise((r) => ch.once("exit", r));

const until = async (cond, ms = 5000) => {
  const t0 = Date.now();
  while (!cond()) {
    if (Date.now() - t0 > ms) throw new Error("until: timed out");
    await new Promise((r) => setTimeout(r, 25));
  }
};

test("§7 watch row: socket live → CLI attaches as viewer and renders daemon-delivered mail", async () => {
  const ipc = createIpcServer({ daemonInfo: { pid: 4242, start_time: "t", did: "did:wba:me" }, log: () => {} });
  await ipc.listen();
  const { child, out } = runCli(["watch"]);
  try {
    await until(() => out.stdout.includes("attached to air-msgd"));
    await until(() => ipc.clientCount() === 1);
    await ipc.sink.deliver({ envelope_id: "w1", seq: 3, from: "did:wba:x", verified: true, encrypted: true, body: { type: "text", text: "table-row-1" } });
    await until(() => out.stdout.includes("table-row-1"));
  } finally {
    child.kill("SIGINT");
    await waitExit(child);
    await ipc.close();
  }
});

test("§7 watch row: viewer survives a daemon restart and re-renders mail", async () => {
  // C1 regression guard: without the ref'd keepAlive the process exits ~500ms after onDetach
  // (the unref'd backoff timer cannot pin the event loop). This test fails if keepAlive is removed.
  const ipc = createIpcServer({ daemonInfo: { pid: 4242, start_time: "t", did: "did:wba:me" }, log: () => {} });
  await ipc.listen();
  const { child, out } = runCli(["watch"]);
  try {
    await until(() => out.stdout.includes("attached to air-msgd"));
    await until(() => ipc.clientCount() === 1);
    await ipc.close();
    await new Promise((r) => setTimeout(r, 600));
    assert.equal(child.exitCode, null, "watch viewer must stay alive during daemon outage (C1: ref'd keepAlive required)");
    const ipc2 = createIpcServer({ daemonInfo: { pid: 4243, start_time: "t2", did: "did:wba:me" }, log: () => {} });
    await ipc2.listen();
    try {
      await until(() => out.stdout.indexOf("attached to air-msgd", out.stdout.indexOf("attached to air-msgd") + 1) !== -1, 8000);
      await until(() => ipc2.clientCount() === 1, 8000);
      await ipc2.sink.deliver({ envelope_id: "w2", seq: 4, from: "did:wba:x", verified: true, encrypted: false, body: { type: "text", text: "post-restart-mail" } });
      await until(() => out.stdout.includes("post-restart-mail"), 5000);
    } finally { await ipc2.close(); }
  } finally {
    child.kill("SIGINT");
    await waitExit(child);
  }
});

test("§7 bridge row: socket live → bridge refuses with a pointer at the daemon", async () => {
  const ipc = createIpcServer({ daemonInfo: { pid: 4242, start_time: "t", did: "did:wba:me" }, log: () => {} });
  await ipc.listen();
  // Bridge config present so the refusal (not the config check) is what fires.
  writeFileSync(join(dir, "bridge.json"), JSON.stringify({ telegram: { bot_token: "x", chat_id: 1 } }), { mode: 0o600 });
  const { child, out } = runCli(["bridge"]);
  try {
    // Give the CLI 3 s to exit on its own (Task 6 wires the refusal; until then it hangs on
    // acquireOrExit). Task 6's RED: probeDaemon not yet wired → child won't self-exit.
    const code = await Promise.race([
      waitExit(child),
      new Promise((_, reject) => setTimeout(() => reject(new Error("bridge did not exit within 3 s — probeDaemon not yet wired (Task 6 RED)")), 3000)),
    ]);
    assert.equal(code, 1);
    assert.match(out.stderr, /daemon owns the message pull/);
  } finally {
    child.kill("SIGKILL");
    await waitExit(child);
    await ipc.close();
  }
});
```
**Wire-fact verification inside this step:** before trusting the seeded `identity.json`/`bridge.json` shapes, read `src/identity.mjs` (`ensureIdentity`, the on-disk identity shape) and `src/bridge-config.mjs` or wherever `loadBridgeConfig` lives (cli.mjs:313) and adjust the fixtures to the REAL shapes. If `ensureIdentity()` cannot run network-silent from a seeded file, delete this spawn file, fall back to unit coverage (`probeDaemon` in Task 6 + `cleanStaleSocket` here), and say so in the commit message.

- [ ] **Step 6: Run to verify** (watch row passes; bridge row FAILS until Task 6)

Run: `node --test test/daemon-ipc.test.mjs test/cli-daemon-table.test.mjs`
Expected: watch-row test PASS; bridge-row test FAIL (refusal not implemented yet — that is Task 6's RED).

- [ ] **Step 7: Commit**

```bash
git add src/daemon-ipc.mjs src/cli.mjs test/daemon-ipc.test.mjs test/cli-daemon-table.test.mjs
git commit -m "feat(cli): watch attaches to the daemon as viewer (spec §7 row) + stale-socket hygiene"
```

---

### Task 6: §7 table — `air-msg bridge` refusal row (`probeDaemon`)

**Files:**
- Modify: `src/daemon-ipc.mjs` (new export `probeDaemon`)
- Modify: `src/cli.mjs` (bridge case, cli.mjs:310-318)
- Test: `test/daemon-ipc.test.mjs` (append), `test/cli-daemon-table.test.mjs` (bridge row from Task 5 goes GREEN)

- [ ] **Step 1: Write the failing unit tests** (append to `test/daemon-ipc.test.mjs`):

```js
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
```

- [ ] **Step 2: Run to verify failure**

Run: `node --test test/daemon-ipc.test.mjs`
Expected: FAIL — `probeDaemon` not exported.

- [ ] **Step 3: Implement** (append to `src/daemon-ipc.mjs`):

```js
/** One-shot liveness probe (§7 decision table): is a daemon answering the socket RIGHT NOW?
 *  Attaches as a throwaway viewer and detaches immediately — the socket answering hello is the
 *  truth the table keys on (a PID file can outlive a crashed daemon; ECONNREFUSED cannot lie). */
export async function probeDaemon({ timeoutMs = 1500 } = {}) {
  try {
    const h = await connectDaemon({ role: "viewer", onMessage: () => {}, handshakeMs: timeoutMs, log: () => {} });
    h.close();
    return true;
  } catch {
    return false;
  }
}
```

- [ ] **Step 4: Wire the bridge row.** In `src/cli.mjs`, inside `case "bridge":`, AFTER the config check (cli.mjs:314-317) and BEFORE `if (!acquireOrExit("bridge")) break;`, insert:

```js
      // §7 bridge row: the daemon owns the pull; a standalone bridge beside it would fight for
      // the consumer lock and lose with a generic message — refuse with the real reason instead.
      // (In-daemon Telegram is the "bridge to doorbell-grade" roadmap item, not Phase 4.)
      if (await probeDaemon()) {
        console.error("bridge: the daemon owns the message pull on this identity.");
        console.error("Stop it first (air-msg daemon stop) to run the standalone bridge,");
        console.error("or keep the daemon — in-daemon Telegram is on the roadmap.");
        process.exit(1);
      }
```
Then add `cleanStaleSocket();` after the bridge's `if (!acquireOrExit("bridge")) break;` line, and add `probeDaemon` to cli.mjs's daemon-ipc import. Do the same `cleanStaleSocket()` insertion in channel-server.mjs's standalone fallback, right after its `if (!acquireOrExit("channel-server")) return;` (channel-server.mjs:65) — all three legacy entrypoints now follow §7 row 2.

- [ ] **Step 5: Run to verify pass**

Run: `node --check src/channel-server.mjs && node --test test/daemon-ipc.test.mjs test/cli-daemon-table.test.mjs`
Expected: ALL PASS — including Task 5's bridge-row spawn test going GREEN.

- [ ] **Step 6: Commit**

```bash
git add src/daemon-ipc.mjs src/cli.mjs src/channel-server.mjs test/daemon-ipc.test.mjs
git commit -m "feat(cli): bridge refuses beside a live daemon (spec §7 row) — probeDaemon liveness check"
```

---

### Task 7: `daemon status` learns live socket state (completes spec §8)

**Files:**
- Modify: `src/daemon-ipc.mjs` (server handles `{type:"status"}`; new export `queryDaemonStatus`; deliver() tracks `lastDeliveredSeq`)
- Modify: `src/daemon.mjs` (pass `statusExtraFn` listing sink names)
- Modify: `src/cli.mjs` (status case prints the live block, cli.mjs:591-597)
- Test: `test/daemon-ipc.test.mjs` (append)

- [ ] **Step 1: Write the failing tests** (append):

```js
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
```

- [ ] **Step 2: Run to verify failure**

Run: `node --test test/daemon-ipc.test.mjs`
Expected: FAIL — no `status` reply; `queryDaemonStatus` not exported.

- [ ] **Step 3: Implement.** In `src/daemon-ipc.mjs`:

(a) `createIpcServer` options gain `statusExtraFn = () => ({})`; add `let lastDeliveredSeq = null;` next to the `subscribers` set; in `deliver()` after the relay_seq stamp line add:
```js
        if (wire && Number.isFinite(wire.relay_seq)) lastDeliveredSeq = wire.relay_seq;
```
(b) In the post-hello frame handler, next to the `ping` line (daemon-ipc.mjs:143):
```js
      if (frame.type === "status") {
        socket.write(encodeFrame({
          type: "status",
          socket: socketPath(),
          last_seq: lastDeliveredSeq,
          // Exclude the ASKING subscriber: `daemon status` attaches a throwaway viewer probe, and
          // counting it would make an idle daemon always report one client (critic v1 M3).
          clients: [...subscribers].filter((s) => s !== sub).map((s) => ({ role: s.role, lastSeq: s.lastSeq, dropped: s.dropped })),
          ...statusExtraFn(),
        }));
      }
```
(c) Append the client helper (`connectFn = connectDaemon` is the test seam for the timeout-race guard):
```js
/** One-shot live status query for `air-msg daemon status` (spec §8): the CLI runs in a separate
 *  process, so connected-clients/last_seq state must cross the socket. Returns null if no daemon. */
export async function queryDaemonStatus({ timeoutMs = 1500, connectFn = connectDaemon } = {}) {
  return new Promise((resolve) => {
    let handle = null;
    let done = false;
    const finish = (v) => { if (!done) { done = true; handle?.close(); resolve(v); } };
    const timer = setTimeout(() => finish(null), timeoutMs);
    connectFn({
      role: "viewer",
      onMessage: () => {},                             // live frames during the query: ignored
      onStatus: (st) => { clearTimeout(timer); finish(st); },
      handshakeMs: timeoutMs,
      log: () => {},
    }).then((h) => { if (done) { h.close(); return; } handle = h; h._sock.write(encodeFrame({ type: "status" })); })
      .catch(() => { clearTimeout(timer); finish(null); });
  });
}
```
(d) `connectDaemon` gains `onStatus = () => {}` (next to `onGap`) and one route line in its parser:
```js
      if (frame.type === "status") onStatus(frame);
```

In `src/daemon.mjs` (`startDaemon`), let the IPC server name the sinks — `sinks` is built after `ipc`, so bind late; `writeDaemonPid` moves AFTER `ipc.listen()` so a failed bind never strands a PID file (also kills a benign split-brain warning window):
```js
  let sinks = [];
  const ipc = createIpcServer({
    mute,
    daemonInfo: { pid: process.pid, start_time: startTime, did: identity.did },
    statusExtraFn: () => ({ sinks: sinks.map((s) => s.name) }),
    log,
  });
  await ipc.listen();                              // safe: we hold the consumer lock (single-daemon mutex)
  writeDaemonPid({ pid: process.pid, startTime }); // written AFTER bind: a failed listen won't strand a PID file
  sinks = [bannerSink({ notifier, mute }), ipc.sink];
```
(replacing the current `writeDaemonPid(...); const ipc = ...; await ipc.listen(); const sinks = ...` block).

In `src/cli.mjs` `case "status":` (cli.mjs:591-597), after the two existing `console.log` lines add:
```js
          if (s.running) {
            const { queryDaemonStatus, socketPath } = await import("./daemon-ipc.mjs");
            const live = await queryDaemonStatus();
            if (live) {
              const roles = live.clients.map((cl) => cl.role).join(", ") || "none";
              console.log(`socket: ${socketPath()}`);
              console.log(`clients: ${live.clients.length} (${roles})  ·  last relay_seq: ${live.last_seq ?? "—"}`);
              console.log(`sinks: ${(live.sinks ?? []).join(", ") || "?"}`);
              for (const cl of live.clients) {
                if (cl.dropped > 0 || cl.lastSeq !== live.last_seq) {
                  console.log(`  · ${cl.role}: lastSeq ${cl.lastSeq ?? "—"}${cl.dropped ? `, dropped ${cl.dropped}` : ""}`);
                }
              }
            } else {
              console.log(c.yellow("socket: unreachable (split-brain or daemon starting/stopping — check daemon logs)"));
            }
          }
```

- [ ] **Step 4: Run to verify pass**

Run: `node --test test/daemon-ipc.test.mjs test/daemon.test.mjs`
Expected: ALL PASS.

- [ ] **Step 5: Commit**

```bash
git add src/daemon-ipc.mjs src/daemon.mjs src/cli.mjs test/daemon-ipc.test.mjs
git commit -m "feat(daemon): status over IPC — clients/roles/last_seq/sinks for air-msg daemon status (spec §8)"
```

---

### Task 8: `src/service.mjs` — pure launchd/systemd generators

**Files:**
- Create: `src/service.mjs`
- Test: `test/service.test.mjs` (new)

- [ ] **Step 1: Write the failing tests** (create `test/service.test.mjs`):

```js
import { test } from "node:test";
import assert from "node:assert/strict";
import { launchdPlist, systemdUnit, servicePlan, SERVICE_LABEL } from "../src/service.mjs";

const ARGS = { nodePath: "/opt/node 22/bin/node", cliPath: "/Users/me/air-note/agent-bridge-mcp/src/cli.mjs", home: "/Users/me/.air-msg", logPath: "/Users/me/.air-msg/daemon.log" };

test("launchdPlist: absolute paths, keepalive, run-at-load, log path, XML-escaped", () => {
  const xml = launchdPlist(ARGS);
  assert.match(xml, new RegExp(`<string>${SERVICE_LABEL}</string>`));
  assert.match(xml, /<string>\/opt\/node 22\/bin\/node<\/string>/);   // process.execPath verbatim (spaces fine in plist strings)
  assert.match(xml, /<string>daemon<\/string>\s*<string>start<\/string>/);
  assert.match(xml, /<key>RunAtLoad<\/key>\s*<true\/>/);
  assert.match(xml, /<key>KeepAlive<\/key>\s*<true\/>/);
  assert.match(xml, /daemon\.log/);
  assert.match(xml, /AGENT_BRIDGE_HOME/);                              // home was given → env var set
  const noHome = launchdPlist({ ...ARGS, home: undefined });
  assert.doesNotMatch(noHome, /AGENT_BRIDGE_HOME/);                    // default home → no env override
  const esc = launchdPlist({ ...ARGS, cliPath: "/tmp/a&b<c>/cli.mjs" });
  assert.match(esc, /a&amp;b&lt;c&gt;/);                               // XML entities escaped
});

test("systemdUnit: quoted ExecStart, Restart=always, default.target", () => {
  // CONTENT assertion ONLY (critic v1 H1): this proves what we EMIT, not that systemd parses it.
  // systemd does its own word-splitting (double quotes group tokens per systemd.service(5)), but
  // no systemd exists in this environment — the `systemctl --user enable --now` load is a
  // MANUAL smoke on a real Linux box before the systemd path is trusted (stated in the PR body).
  const unit = systemdUnit(ARGS);
  assert.match(unit, /ExecStart="\/opt\/node 22\/bin\/node" "\/Users\/me\/air-note\/agent-bridge-mcp\/src\/cli\.mjs" daemon start/);
  assert.match(unit, /Restart=always/);
  assert.match(unit, /Environment="AGENT_BRIDGE_HOME=/);
  assert.match(unit, /WantedBy=default\.target/);
  assert.doesNotMatch(systemdUnit({ ...ARGS, home: undefined }), /AGENT_BRIDGE_HOME/);
});

test("servicePlan: darwin → launchd plist under LaunchAgents; linux → systemd-user unit; else null", () => {
  const mac = servicePlan({ platform: "darwin", homedir: "/Users/me", nodePath: ARGS.nodePath, cliPath: ARGS.cliPath });
  assert.equal(mac.kind, "launchd");
  assert.equal(mac.file, `/Users/me/Library/LaunchAgents/${SERVICE_LABEL}.plist`);
  assert.deepEqual(mac.loadCmd, ["launchctl", "load", "-w", mac.file]);
  assert.deepEqual(mac.unloadCmd, ["launchctl", "unload", "-w", mac.file]);
  assert.match(mac.content, /<plist/);
  assert.match(mac.content, /\/Users\/me\/\.air-msg\/daemon\.log/);   // default home → log beside the real store, never /tmp
  assert.equal(mac.logPath, "/Users/me/.air-msg/daemon.log");          // installer uses this to mkdir the log directory
  const lin = servicePlan({ platform: "linux", homedir: "/home/me", nodePath: ARGS.nodePath, cliPath: ARGS.cliPath });
  assert.equal(lin.kind, "systemd");
  assert.equal(lin.file, "/home/me/.config/systemd/user/air-msg-daemon.service");
  assert.deepEqual(lin.loadCmd, ["systemctl", "--user", "enable", "--now", "air-msg-daemon.service"]);
  assert.deepEqual(lin.unloadCmd, ["systemctl", "--user", "disable", "--now", "air-msg-daemon.service"]);
  assert.equal(lin.logPath, "/home/me/.air-msg/daemon.log");           // returned for API symmetry; journald owns stdout on linux
  assert.equal(servicePlan({ platform: "win32", homedir: "C:\\u", nodePath: "n", cliPath: "c" }), null);   // spec §2: Windows is v2
});
```

- [ ] **Step 2: Run to verify failure**

Run: `node --test test/service.test.mjs`
Expected: FAIL — module does not exist.

- [ ] **Step 3: Implement** (create `src/service.mjs`):

```js
// src/service.mjs — auto-start unit generators for `air-msg daemon install` (spec §8).
// PURE string generators + a platform plan: the CLI does the file/exec I/O, tests assert content
// (spec §9: the load step itself is verified manually on a real box). Absolute paths everywhere:
// launchd/systemd provide no user PATH, so the HELP-text `/usr/bin/env air-msg` idiom cannot work.
import { join } from "node:path";

export const SERVICE_LABEL = "org.air-msg.daemon";
export const SYSTEMD_UNIT_NAME = "air-msg-daemon.service";

const xmlEscape = (s) => String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

/** macOS LaunchAgent: run the daemon at login and keep it alive. `home` (optional) pins
 *  AGENT_BRIDGE_HOME for non-default homes; `logPath` is computed by servicePlan so the log
 *  always sits beside the REAL store (~/.air-msg by default — never /tmp, which is world-readable
 *  and cleared on reboot; critic v1 note).
 *  PRECONDITION: launchd opens StandardOutPath/StandardErrorPath BEFORE spawning the process and
 *  does NOT create missing parent directories — the installer must mkdir the log directory first
 *  (Task 9 does this via `mkdirSync(dirname(plan.logPath), { recursive: true })`). */
export function launchdPlist({ nodePath, cliPath, home, logPath }) {
  return `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>${SERVICE_LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>${xmlEscape(nodePath)}</string>
    <string>${xmlEscape(cliPath)}</string>
    <string>daemon</string>
    <string>start</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>${home ? `
  <key>EnvironmentVariables</key>
  <dict><key>AGENT_BRIDGE_HOME</key><string>${xmlEscape(home)}</string></dict>` : ""}
  <key>StandardOutPath</key><string>${xmlEscape(logPath)}</string>
  <key>StandardErrorPath</key><string>${xmlEscape(logPath)}</string>
</dict>
</plist>
`;
}

/** Linux systemd-user unit: same contract as the LaunchAgent.
 *  QUOTING BOUNDARY (critic v1 H1): systemd does its own word-splitting — double quotes group
 *  tokens per systemd.service(5) — but this generator is content-tested only; no systemd exists
 *  in the dev environment, so the actual enable/--now load is a REQUIRED manual smoke on a real
 *  Linux box before the systemd path is trusted. Additional pathological-input caveats behind
 *  that same manual-smoke boundary: `%` is a systemd specifier prefix in unit values (a literal
 *  % needs %% doubling); a literal `"` or `\` inside quoted ExecStart tokens would also break
 *  C-style quoting — not fixed here, noted as a known boundary. */
export function systemdUnit({ nodePath, cliPath, home }) {
  return `[Unit]
Description=AIR Note receiver daemon (air-msg daemon start)

[Service]
ExecStart="${nodePath}" "${cliPath}" daemon start${home ? `
Environment="AGENT_BRIDGE_HOME=${home}"` : ""}
Restart=always
RestartSec=2

[Install]
WantedBy=default.target
`;
}

/** Decide file path + content + load/unload commands for this platform; null = unsupported
 *  (spec §2: Windows auto-start is v2 — no named-pipe ACL guarantee, no test box). */
export function servicePlan({ platform, homedir, nodePath, cliPath, home }) {
  // Log beside the resolved store: an explicit home, or bridgeHome()'s default ~/.air-msg.
  const logPath = join(home ?? join(homedir, ".air-msg"), "daemon.log");
  if (platform === "darwin") {
    const file = join(homedir, "Library", "LaunchAgents", `${SERVICE_LABEL}.plist`);
    return {
      kind: "launchd",
      file,
      logPath,
      content: launchdPlist({ nodePath, cliPath, home, logPath }),
      loadCmd: ["launchctl", "load", "-w", file],
      unloadCmd: ["launchctl", "unload", "-w", file],
    };
  }
  if (platform === "linux") {
    const file = join(homedir, ".config", "systemd", "user", SYSTEMD_UNIT_NAME);
    // stdout goes to journald by default on systemd; logPath is returned for API symmetry
    // and any future use — no daemon.log is written by systemd itself.
    return {
      kind: "systemd",
      file,
      logPath,
      content: systemdUnit({ nodePath, cliPath, home }),
      loadCmd: ["systemctl", "--user", "enable", "--now", SYSTEMD_UNIT_NAME],
      unloadCmd: ["systemctl", "--user", "disable", "--now", SYSTEMD_UNIT_NAME],
    };
  }
  return null;
}
```

- [ ] **Step 4: Run to verify pass**

Run: `node --test test/service.test.mjs`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/service.mjs test/service.test.mjs
git commit -m "feat(service): launchd/systemd unit generators — pure, absolute-path, tested content (spec §8)"
```

---

### Task 9: CLI `daemon install|uninstall` + `start --detach` + HELP refresh

**Files:**
- Modify: `src/cli.mjs` (daemon case, cli.mjs:575-604; HELP text cli.mjs:121-123 + 135-144)
- Test: `test/cli-args.test.mjs` (append, follow that file's existing parser-level idiom)

- [ ] **Step 1: Write the failing tests.** Read `test/cli-args.test.mjs` first and append cases in ITS idiom asserting: `daemon install`, `daemon uninstall`, and `daemon start --detach` are routed (not "unknown daemon subcommand"). If that file's harness cannot reach subcommand routing without side effects, assert at the `servicePlan` boundary instead and note it in the commit.

- [ ] **Step 2: Run to verify failure**

Run: `node --test test/cli-args.test.mjs`
Expected: new cases FAIL (subcommands unknown).

- [ ] **Step 3: Implement.** In `src/cli.mjs` `case "daemon":` add two cases before `default:` (and extend the default's hint line):

```js
        case "install": {
          const { servicePlan } = await import("./service.mjs");
          const { isDaemonRunning } = await import("./daemon.mjs");
          if (isDaemonRunning()) {
            // A manually-started daemon holds the consumer lock; the service's daemon would
            // exit(1) on acquireOrExit and launchd/systemd would relaunch-loop against it.
            console.error("a daemon is already running — stop it first: air-msg daemon stop (if it was installed as a service, use: air-msg daemon uninstall)");
            process.exit(1);
          }
          const plan = servicePlan({
            platform: process.platform,
            homedir: homedir(),
            nodePath: process.execPath,
            cliPath: fileURLToPath(import.meta.url),
            home: process.env.AGENT_BRIDGE_HOME,
          });
          if (!plan) { console.error(`auto-start install is not supported on ${process.platform} (spec: POSIX-only in v1)`); process.exit(1); }
          mkdirSync(dirname(plan.file), { recursive: true });
          mkdirSync(dirname(plan.logPath), { recursive: true });   // launchd opens StandardOutPath pre-spawn and won't create parent dirs
          writeFileSync(plan.file, plan.content, { mode: 0o644 });
          const r = spawnSync(plan.loadCmd[0], plan.loadCmd.slice(1), { stdio: "inherit" });
          if (r.status !== 0) { console.error(`${plan.loadCmd[0]} failed (exit ${r.status}${r.error ? `, ${r.error.message}` : ""}) — unit written to ${plan.file}; load it manually or clean up with: air-msg daemon uninstall`); process.exit(1); }
          console.log(`${c.green("✓ installed")} ${plan.kind} unit ${c.dim(plan.file)}`);
          console.log(c.dim("the daemon now starts at login and stays alive; check: air-msg daemon status"));
          break;
        }
        case "uninstall": {
          const { servicePlan } = await import("./service.mjs");
          const plan = servicePlan({
            platform: process.platform, homedir: homedir(),
            nodePath: process.execPath, cliPath: fileURLToPath(import.meta.url),
            home: process.env.AGENT_BRIDGE_HOME,
          });
          if (!plan) { console.error(`nothing to uninstall on ${process.platform}`); process.exit(1); }
          spawnSync(plan.unloadCmd[0], plan.unloadCmd.slice(1), { stdio: "ignore" });   // best-effort unload (attempt even if file is missing — may still be loaded)
          const { existsSync } = await import("node:fs");
          if (!existsSync(plan.file)) {
            console.log(`nothing installed at ${c.dim(plan.file)}`);
            break;
          }
          try { rmSync(plan.file, { force: true }); } catch { /* best effort */ }
          console.log(`${c.green("✓ uninstalled")} ${c.dim(plan.file)}`);
          break;
        }
```
Extend `case "start":` for `--detach` (check the raw args; verify wire-fact (b) — adjust if `rest` is pre-split by `parseRoomArgs`):
```js
        case "start": {
          // rest = the argv tail after the top-level command (["start","--detach"]) — verified in
          // scope; don't refactor to the parseRoomArgs output (critic v1 ambiguity note).
          if (rest.includes("--detach")) {
            const { isDaemonRunning, readDaemonPid } = await import("./daemon.mjs");
            if (isDaemonRunning()) {
              console.log(`daemon already running ${c.dim("pid " + readDaemonPid()?.pid)}`);
              break;
            }
            // First-run registration is network-bound; do it in the parent so the child's call
            // returns from disk and the 3s poll covers only bind+PID-write — also keeps
            // registration output visible (the child's stdio is discarded).
            await ensureIdentity();
            const child = spawn(process.execPath, [fileURLToPath(import.meta.url), "daemon", "start"], { detached: true, stdio: "ignore" });
            child.unref();
            const t0 = Date.now();
            while (!isDaemonRunning() && Date.now() - t0 < 3000) await new Promise((r) => setTimeout(r, 100));
            if (isDaemonRunning()) {
              console.log(`${c.green("✓ daemon detached")} ${c.dim("pid " + readDaemonPid()?.pid)}`);
              console.log(c.dim("note: detached logs are discarded — air-msg daemon install gives you a log file"));
            } else {
              console.error("daemon did not come up within 3s — it may still be starting (check: air-msg daemon status) or run `air-msg daemon start` in the foreground to see why");
              process.exit(1);
            }
            break;
          }
          const { startDaemon } = await import("./daemon.mjs");
          await startDaemon();
          break;
        }
```
Imports: add `homedir` (`node:os`), `fileURLToPath` (`node:url`), `spawn`/`spawnSync` (`node:child_process`), `mkdirSync`/`writeFileSync`/`rmSync` + `dirname` to cli.mjs's existing import lines (check what is already imported first — never duplicate).

HELP text: replace the daemon lines (cli.mjs:121-123) with:
```
  air-msg daemon status                  Daemon + live socket state (clients, last seq, sinks)
  air-msg daemon start [--detach]        Start the receiver daemon (foreground, or detached)
  air-msg daemon stop                    Stop the running daemon
  air-msg daemon install | uninstall     Auto-start at login (macOS launchd / Linux systemd-user)
```
and REPLACE the manual plist block (cli.mjs:135-144) with:
```
  always-on (recommended): air-msg daemon install — one always-on pull; watch/channel attach to it
  as clients (run `air-msg watch` in any terminal for a live feed); the standalone bridge needs the
  daemon stopped — in-daemon Telegram is on the roadmap.
  (service-managed daemons relaunch on stop — use air-msg daemon uninstall to remove the auto-start)
```
(The old snippet's `/usr/bin/env air-msg` does not work under launchd anyway — no user PATH.)

- [ ] **Step 4: Run to verify pass**

Run: `node --check src/cli.mjs && node --test test/cli-args.test.mjs test/service.test.mjs`
Expected: ALL PASS.

- [ ] **Step 5: Commit**

```bash
git add src/cli.mjs test/cli-args.test.mjs
git commit -m "feat(cli): daemon install/uninstall + start --detach — one-command always-on (spec §8)"
```

---

### Task 10: Messaging suite on Linux CI (carried critic flag (b), settled empirically)

**Files:**
- Create: `.github/workflows/messaging-tests.yml` (REPO ROOT — `~/air-note/.github/workflows/`)

- [ ] **Step 1: Verify workflow facts first** (domain lesson: YAML-valid ≠ runtime-valid):
list `~/air-note/.github/workflows/` and read one existing workflow for the checkout/setup-node action versions in use (actions@v5 bump landed in PR #10 — match it); confirm whether `agent-bridge-mcp/package-lock.json` exists (wire-fact (c)) → `npm ci` vs `npm install`.

- [ ] **Step 2: Create the workflow:**

```yaml
name: messaging-tests
# Runs the hermetic agent-bridge-mcp suite (278 tests; temp homes enforced by the bridgeHome
# guard) on Linux. This is the standing empirical cross-check for the Phase-3 overflow-test
# sizing under Linux unix-socket buffer defaults (SO_SNDBUF) — the tests positively assert the
# skip path engaged (clientStats().dropped > 0), so a sizing drift fails loudly here.
on:
  push:
    branches: [main]
    paths: ["agent-bridge-mcp/**", ".github/workflows/messaging-tests.yml"]
  pull_request:
    paths: ["agent-bridge-mcp/**", ".github/workflows/messaging-tests.yml"]

jobs:
  node-test:
    runs-on: ubuntu-latest
    defaults:
      run:
        working-directory: agent-bridge-mcp
    steps:
      - uses: actions/checkout@v5            # match the repo's pinned major (verify in step 1)
      - uses: actions/setup-node@v5
        with:
          node-version: "22"                 # quoted (repo convention — build.yml pins "20"); engines >=22; node --test test/ is broken on 25
      - run: npm ci                          # or npm install if no lockfile (wire-fact (c))
      - run: node --test
```

- [ ] **Step 3: Validate + commit**

Run: `node -e "console.log('yaml ok')" && npx --yes yaml-lint .github/workflows/messaging-tests.yml 2>/dev/null || python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/messaging-tests.yml')); print('yaml ok')"` (from the repo root; either validator is fine)
Expected: `yaml ok`.

```bash
git add .github/workflows/messaging-tests.yml
git commit -m "ci: run the hermetic messaging suite on Linux — standing SO_SNDBUF sizing cross-check"
```
The PR (Task 11) is itself the live verification: this workflow MUST run green on the PR before merge. If the Linux run exposes a real sizing failure in the Phase-3 overflow tests, that is the critic's flag proving out — fix the TEST SIZING (per the Phase-3 plan's in-band rule), never by weakening the `dropped > 0` assertions.

---

### Task 11: Spec notes + full verification + PR

**Files:**
- Modify: `agent-bridge-mcp/docs/superpowers/specs/2026-06-05-receiver-daemon-design.md` (§6, §7, §8, §11)

- [ ] **Step 1: Spec updates.**

(a) §6 — append one bullet after the Phase-3 bullet:
```markdown
- **Phase 4 (2026-06-10) closes the loop:** the "OR reconnect" trigger is live — hello carries an
  optional `since_seq`; the daemon answers a channel hello bearing one with an immediate
  `{type:"gap", after_seq: since_seq}` (and seeds that subscriber's `lastSeq` so a pre-write
  overflow gap is anchored, not 0). Clients reconnect via `connectDaemonPersistent` (backoff
  500ms→×2→5s cap, reset on attach), resuming from max-seen relay_seq, or — when the outage hit
  before any frame arrived — from an archive-cursor baseline snapshotted at first attach. The
  4×HWM backstop-destroy needs NO final gap hint: a wedged socket cannot usefully receive one,
  and the destroyed client's reconnect+since_seq path IS the recovery (integration-proven).
```
(b) §7 — append after the reconnect-contract paragraph:
```markdown
Phase 4 implements the table's `watch` and `bridge` rows: `air-msg watch` attaches as a `viewer`
(feed-only — the daemon's banner sink rings, the terminal never double-rings) with persistent
reconnect; `air-msg bridge` beside a live daemon refuses with a pointer (in-daemon Telegram is a
follow-up); all three legacy entrypoints unlink a stale socket immediately after winning the
consumer lock (`cleanStaleSocket` — the lock proves nothing live owns the path).
```
(c) §8 — append:
```markdown
Phase 4 ships `install`/`uninstall` (pure generators in `service.mjs`; absolute node+cli paths —
launchd/systemd provide no user PATH), `start --detach`, and the defined `status` output: the
PID-file block plus a live-over-IPC block (`{type:"status"}` frame → socket path, clients with
roles, last delivered relay_seq, enabled sinks). A PID-alive-but-socket-unreachable daemon is
reported as possible split-brain rather than guessed at.
```
(d) §11 — strike the last open question (MCP-host relaunch) the way the resolved one above it is struck, appending:
```markdown
  **RESOLVED 2026-06-10 (Phase 4):** moot — the channel server no longer exits on daemon close;
  `connectDaemonPersistent` reconnects with backoff and resumes via `since_seq`.
```

- [ ] **Step 2: Full suite + hermeticity**

```bash
cd ~/air-note/agent-bridge-mcp
before=$(sqlite3 -readonly ~/.air-msg/archive.db "SELECT COUNT(*) FROM messages")
node --test 2>&1 | grep -E "^ℹ (tests|pass|fail|todo)"
after=$(sqlite3 -readonly ~/.air-msg/archive.db "SELECT COUNT(*) FROM messages")
echo "real-archive delta: $((after-before)) (must be 0)"
```
Expected: `fail 0`, delta 0. (Baseline at branch: 278 tests / 275 pass / 3 todo.)

- [ ] **Step 3: Commit + PR**

```bash
git add agent-bridge-mcp/docs/superpowers/specs/2026-06-05-receiver-daemon-design.md
git commit -m "docs(daemon): spec §6-§8, §11 — Phase 4 implementation notes (reconnect, table rows, installers)"
git push -u origin feat/daemon-phase4
gh pr create --repo AgentIdentityRegistry/air-note --base main \
  --title "feat(daemon): Phase 4 — reconnect/resume, §7 table rows, status over IPC, installers"
```
PR body must state the verification boundary: reconnect/resume/backstop-recovery are integration-tested over real sockets; channel-server + CLI wiring verified by review + spawn tests; the `launchctl`/`systemctl` LOAD step is manual-on-a-real-box (spec §9) and deliberately not exercised in tests — a post-merge `daemon install` smoke on a real Mac is a separate, consent-gated step (it installs a persistent LaunchAgent on the user's machine). The messaging-tests CI job must be green on this PR (it is the Linux SO_SNDBUF cross-check). The systemd path carries one more explicit boundary (critic v1 H1): the unit generator is content-tested ONLY — it proves what we emit, not that systemd parses it — and requires a real-Linux `systemctl --user enable --now` smoke before the systemd installer is trusted.

---

## Self-Review (against spec §6-§9, §11 + the Phase-2/3 review bar)

- **§6 "OR reconnect" trigger:** since_seq in hello (T1) + persistent client tracking max-seen (T2) + baseline-cursor fallback for the outage-window hole (T2, tested) — the daemon stays stateless about client history; the strict-cursor archive (Phase 3) is the replay source. ✓
- **§7 reconnect contract:** backoff with reset-on-attach (T2); first-attempt DAEMON_DOWN still falls back to standalone so the table's no-daemon rows hold (T2 test 1, channel-server unchanged catch). ✓
- **§7 table rows:** watch→viewer attach with no lock and no double-banner (T5); bridge refusal with the real reason (T6); stale-socket unlink ONLY after winning the lock, in all three legacy entrypoints (T5/T6 — the lock-ordering safety argument is in the helper's doc). ✓
- **§8:** status completed over IPC with split-brain visibility (T7); installers as pure tested generators + thin CLI I/O, absolute paths because launchd/systemd have no user PATH — which also retires the HELP text's broken `/usr/bin/env` snippet (T8/T9); `--detach` with a 3s liveness wait (T9); install refuses beside a running daemon to prevent a KeepAlive relaunch-loop against the lock (T9). ✓
- **§9:** decision-table transitions get spawn-level cases through the REAL CLI (T5/T6, with an explicit pre-verified identity-seeding wire-fact and a stated fallback if it fails); generators unit-tested on content; the load step manual per spec. ✓
- **§11:** the MCP-host-relaunch open question is closed (T11d) — reconnect supersedes exit-0. ✓
- **Carried critic flags:** (a) resolved by design + an end-to-end backstop-recovery test (T4); (b) resolved by a standing Linux CI job with loud-failure semantics (T10). ✓
- **Honest coverage boundaries:** channel-server wiring is check+review (T3, precedent from Phase 3); CLI install/uninstall I/O is thin wiring over the pure `servicePlan` (T9). Both stated in commits/PR. ✓
- **Placeholder scan:** every step carries complete code or an exact instruction + fallback; names consistent across tasks (`connectDaemonPersistent` (`onAttach`/`onDetach`/`cursorFn`/`initialBackoffMs`/`backoffCapMs`/`connectFn`, handle `{close, _sock()}`), `probeDaemon`, `queryDaemonStatus`, `cleanStaleSocket`, `statusExtraFn`, `lastDeliveredSeq`, `servicePlan`/`launchdPlist`/`systemdUnit`/`SERVICE_LABEL`/`SYSTEMD_UNIT_NAME`). ✓
- **Type consistency check:** T2's handle exposes `_sock()` as a FUNCTION (current socket changes across reconnects) while plain `connectDaemon` exposes `_sock` as a value — T4 uses `handle._sock()`, T7's `queryDaemonStatus` uses the plain handle's `_sock`. Distinct on purpose; both documented as test seams. ✓
- **Phase boundary:** in-daemon Telegram sink + `bridge` socket role (doorbell-grade item), Windows, daemon-side per-client replay — all explicitly out. ✓
- **v2 deltas (critic v1, all applied):** H1 systemd quoting = stated manual-verify boundary (the content test proves emission, not parsing); H2 identity fixture = the real on-disk shape with load-bearing `seed_hex`; M1 baseline snapshot moved BEFORE the first connect (over-broad is safe, late is lossy — reviewer-probed); M3 status reply excludes the requesting subscriber; M5 `node-version` quoted; backoff-escalation test added; watch insertion pinned OUTSIDE the lock-bearing try/finally; rationale comments (sleep-before-retry, silent deliberate close, room notices in the viewer feed); install-failure cleanup hint; log path beside the resolved home, never /tmp. ✓
