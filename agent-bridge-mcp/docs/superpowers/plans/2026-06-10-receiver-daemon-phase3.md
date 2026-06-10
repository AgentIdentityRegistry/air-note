# Receiver Daemon — Implementation Plan (Phase 3: delivery semantics — gap/replay + strict cursor)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** v2 — reworked 2026-06-10 after independent critic review (verdict REWORK: C1 gap starvation under slow-steady readers — empirically reproduced; H1 blocklist bypass on replay; H2 join-notice replayed as room chat; M1–M3, L1–L2). All findings addressed below; gap emission is now flush-on-progress, replay is blocklist-aware and excludes synthetic join notices, and the connectDaemon gap round-trip test is real.

**Goal:** Give the `channel` role at-least-once delivery (spec §6): a slow/stuck channel subscriber gets a `{type:"gap"}` marker instead of silent loss and replays the hole from the local archive; the daemon's pull makes the archive write a precondition for advancing the cursor so the archive is a complete replay source; `viewer` overflow becomes drop-messages-with-a-count instead of drop-the-client.

**Architecture:** Phase 2 (merged, PR #12) ships the socket layer with a drop-the-client HWM floor. Phase 3 refines per-role: `deliver()` keeps per-subscriber state (`lastSeq`, `dropped`); over the HWM it SKIPS writes (counting) for all roles; when the socket drains, a `channel` subscriber receives `{type:"gap", after_seq}` and replays `relay_seq > after_seq` from the archive via a new `replaySince()`, deduped by `envelope_id` client-side; a 4×HWM `destroy` backstop remains for truly wedged sockets. To make replay faithful to the live gate, the archive gains a `key_changed` column (the live channel path withholds key-changed senders; replay must too). In daemon mode `receive(..., {strict:true})` never advances the cursor past the first failed archive write (pure policy helper `cursorAdvanceTarget`, wired through `receiveAll` → `watch`'s injectable `receiveFn`).

**Tech Stack:** Node ≥22 stdlib only. No new dependencies.

**Spec:** `agent-bridge-mcp/docs/superpowers/specs/2026-06-05-receiver-daemon-design.md` §6 (delivery semantics), §4 (live-from-attach; archive is the backlog source). Reconnect/backoff + the full §7 decision table remain Phase 4.

**Repo rules that bind every task:** temp-home idiom in every test (`bridgeHome()` throws under the runner otherwise); import shared helpers, never copy; bare `node --test` only as single files (`node --test test/<file>`); branch `feat/daemon-phase3-delivery` from current `main` (`8daa9fd`); work from `~/air-note/agent-bridge-mcp`.

**Wire facts this plan relies on (verified 2026-06-10):**
- `archiveMessage(rec)` INSERTs columns `(envelope_id, direction, thread_id, peer_did, from_did, to_did, timestamp, body_json, encrypted, verified, relay_seq, room_id, archived_at)` (archive.mjs:71).
- `openArchive()` migrates via `PRAGMA table_info(messages)` + `ALTER TABLE ... ADD COLUMN` (spam/room_id precedent, archive.mjs:57-63).
- `parseRow(r)` returns `{envelope_id, direction, thread_id, peer_did, from, to, timestamp, body, encrypted, verified, spam, relay_seq, room_id, archived_at}` (archive.mjs:86).
- The 1:1 receive path computes `key_changed` (core.mjs ~L543) but does NOT persist it; room messages never set it.
- `receive({since, limit})` (core.mjs:382) advances the cursor at the end even when archive writes failed (comment ~L570); `receiveAll({since, limit, maxPages}, receiveFn)` (core.mjs:597) loops it; `watch({receiveFn = coreDefault.receiveAll, ...})` (watch.mjs:116) is injectable.
- Phase-2 wire frames stamp `relay_seq` (daemon-ipc deliver()); `connectDaemon` ignores unknown server frames (so `gap` is backward-safe).
- `getContactByDid(did)` (contacts.mjs:136) returns the pinned contact (or undefined) — the replay adapter re-derives `contact` from CURRENT pin state.

---

### Task 1: Archive learns `key_changed`

**Files:**
- Modify: `src/archive.mjs` (SCHEMA messages table, `openArchive` migration, `archiveMessage`, `parseRow`)
- Modify: `src/core.mjs` (the 1:1 `archiveMessage({...})` call ~L563 gains `key_changed`)
- Test: `test/archive.test.mjs` (append)

- [ ] **Step 1: Write the failing test** (append to `test/archive.test.mjs`, inside the existing temp-home scaffolding):

```js
test("key_changed round-trips through the archive (and defaults false for old writers)", () => {
  archiveMessage(rec({ envelope_id: "ekc", key_changed: true }));
  archiveMessage(rec({ envelope_id: "ekc0" }));                       // writer omits the field
  const rows = history({ limit: 10 });
  assert.equal(rows.find((r) => r.envelope_id === "ekc").key_changed, true);
  assert.equal(rows.find((r) => r.envelope_id === "ekc0").key_changed, false);
});
```

- [ ] **Step 2: Run to verify failure**

Run: `node --test test/archive.test.mjs`
Expected: FAIL — `key_changed` is `undefined` on rows.

- [ ] **Step 3: Implement.** In `src/archive.mjs`:

(a) In the `SCHEMA` `CREATE TABLE ... messages` statement, after the `verified INTEGER NOT NULL,` line add:
```sql
      key_changed  INTEGER NOT NULL DEFAULT 0,
```
(b) In `openArchive()` after the `room_id` migration block, add (same PRAGMA-guarded pattern):
```js
  if (!cols.includes("key_changed")) {
    // Replay fidelity (spec §6): the live channel gate withholds key-changed senders; the
    // archive must record that bit or a replay would push what live deliberately withheld.
    db.prepare(`ALTER TABLE messages ADD COLUMN key_changed INTEGER NOT NULL DEFAULT 0`).run();
  }
```
(c) In `archiveMessage`, extend the column list with `key_changed` (after `verified`) and the VALUES with one more `?`, passing `rec.key_changed ? 1 : 0` (after `rec.verified ? 1 : 0`).
(d) In `parseRow`, after `verified: !!r.verified,` add `key_changed: !!r.key_changed,`.

In `src/core.mjs`, in the **1:1** `archiveMessage({ ... })` call (the one that already passes `relay_seq: m.seq` and no `room_id`), add the field:
```js
        key_changed: !!key_changed,
```
(Room-message and join-notice archive calls stay unchanged — they never compute `key_changed`; the column defaults to 0.)

- [ ] **Step 4: Run to verify pass**

Run: `node --test test/archive.test.mjs test/archive-rooms.test.mjs test/archive-integration.test.mjs`
Expected: ALL PASS (existing tests unaffected — new column has a default).

- [ ] **Step 5: Commit**

```bash
git add src/archive.mjs src/core.mjs test/archive.test.mjs
git commit -m "feat(archive): persist key_changed — replay must withhold what the live gate withheld"
```

---

### Task 2: `replaySince()` — the replay query

**Files:**
- Modify: `src/archive.mjs` (new export)
- Test: `test/archive.test.mjs` (append)

- [ ] **Step 1: Write the failing test** (append):

```js
import { replaySince } from "../src/archive.mjs";

test("replaySince: received-only, spam-excluded, relay_seq ascending, strictly after since_seq", () => {
  archiveMessage(rec({ envelope_id: "r1", direction: "received", relay_seq: 10 }));
  archiveMessage(rec({ envelope_id: "r2", direction: "received", relay_seq: 11 }));
  archiveMessage(rec({ envelope_id: "r3", direction: "received", relay_seq: 12 }));
  archiveMessage(rec({ envelope_id: "s1", direction: "sent", relay_seq: 13 }));      // not replayed
  archiveMessage(rec({ envelope_id: "r4", direction: "received" }));                  // no relay_seq → not replayed
  archiveMessage(rec({ envelope_id: "room9:joined", direction: "received", relay_seq: 14, room_id: "room9" })); // synthetic join notice → never replayed (H2)
  markSpam("r3");
  const rows = replaySince(10);
  assert.deepEqual(rows.map((r) => r.envelope_id), ["r2"]);   // >10, received, non-spam, has seq, not a join notice
  assert.deepEqual(replaySince(0).map((r) => r.envelope_id), ["r1", "r2"]);   // ascending
  assert.equal(replaySince(0, { limit: 1 }).length, 1);
});
```

- [ ] **Step 2: Run to verify failure**

Run: `node --test test/archive.test.mjs`
Expected: FAIL — `replaySince` not exported.

- [ ] **Step 3: Implement** (append to `src/archive.mjs`):

```js
/** Replay source for at-least-once channel delivery (spec §6): received, non-spam rows with a
 *  relay_seq STRICTLY AFTER since_seq, oldest-first. Synthetic room-join notices
 *  (envelope_id "<room_id>:joined") are EXCLUDED: live surfaces them once as a system inbox
 *  notice and never as channel chat — replaying them would route a system notice into the
 *  room-chat push path (critic H2). The caller re-applies the channel gate — rows carry
 *  verified + key_changed so the replay can withhold what live withheld. */
export function replaySince(since_seq, { limit = 500 } = {}) {
  const db = openArchive();
  return db.prepare(
    `SELECT * FROM messages
      WHERE direction = 'received' AND spam = 0 AND relay_seq IS NOT NULL AND relay_seq > ?
        AND envelope_id NOT LIKE '%:joined'
      ORDER BY relay_seq ASC LIMIT ?`
  ).all(Number(since_seq) || 0, limit).map(parseRow);
}
```

- [ ] **Step 4: Run to verify pass**

Run: `node --test test/archive.test.mjs`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/archive.mjs test/archive.test.mjs
git commit -m "feat(archive): replaySince() — ordered post-gap replay source for the channel"
```

---

### Task 3: Strict cursor policy (pure) + receive/receiveAll wiring

**Files:**
- Modify: `src/core.mjs` (new pure export `cursorAdvanceTarget`; `receive` gains `strict`; `receiveAll` passes it through)
- Test: `test/core.test.mjs` (append — pure-policy tests only; the impure wiring is covered by review + the daemon integration in Task 6)

- [ ] **Step 1: Write the failing tests** (append to `test/core.test.mjs`):

```js
import { cursorAdvanceTarget } from "../src/core.mjs";

test("cursorAdvanceTarget: default mode advances past the whole delivered batch even with failures", () => {
  assert.equal(cursorAdvanceTarget({ deliveredSeqs: [5, 6, 7], failedSeqs: [6], strict: false }), 7);
});

test("cursorAdvanceTarget: strict mode never advances past the first archive failure", () => {
  assert.equal(cursorAdvanceTarget({ deliveredSeqs: [5, 6, 7], failedSeqs: [6], strict: true }), 5);
  assert.equal(cursorAdvanceTarget({ deliveredSeqs: [5, 6, 7], failedSeqs: [5], strict: true }), null); // nothing safe
  assert.equal(cursorAdvanceTarget({ deliveredSeqs: [5, 6, 7], failedSeqs: [], strict: true }), 7);
});

test("cursorAdvanceTarget: empty batch → null (no cursor touch)", () => {
  assert.equal(cursorAdvanceTarget({ deliveredSeqs: [], failedSeqs: [], strict: true }), null);
});
```

- [ ] **Step 2: Run to verify failure**

Run: `node --test test/core.test.mjs`
Expected: FAIL — `cursorAdvanceTarget` not exported.

- [ ] **Step 3: Implement.** In `src/core.mjs` add the pure helper (near the other small exported helpers):

```js
/** Pure cursor policy (spec §6). Default: the diary is best-effort — advance past the whole
 *  delivered batch even if some archive writes failed (live delivery already happened).
 *  Strict (daemon mode): the archive is the channel's replay source, so never advance past
 *  the first failed write — those messages will be re-pulled and re-archived next wake.
 *  Returns the seq to advance to, or null to leave the cursor untouched. */
export function cursorAdvanceTarget({ deliveredSeqs, failedSeqs, strict }) {
  if (!deliveredSeqs.length) return null;
  const max = Math.max(...deliveredSeqs);
  if (!strict || !failedSeqs.length) return max;
  const firstFail = Math.min(...failedSeqs);
  const safe = deliveredSeqs.filter((s) => s < firstFail);
  return safe.length ? Math.max(...safe) : null;
}
```

Then wire it:
- `receive({ since, limit } = {})` → `receive({ since, limit, strict = false } = {})`.
- Inside `receive`, collect failures: declare `const archiveFailedSeqs = [];` next to the existing `drops` set; in **each** of the three `archiveMessage` `catch` blocks (room message, join notice, 1:1) add `archiveFailedSeqs.push(m.seq);` as the first line.
- Replace the end-of-receive cursor block
```js
  if (batch.messages.length) {
    try { setCursor(Math.max(...batch.messages.map((m) => m.seq))); }
    catch (err) { console.error(`[archive] failed to advance cursor: ${err.message ?? err}`); }
```
with
```js
  const cursorTarget = cursorAdvanceTarget({
    deliveredSeqs: batch.messages.map((m) => m.seq),
    failedSeqs: archiveFailedSeqs,
    strict,
  });
  if (cursorTarget != null) {
    try { setCursor(cursorTarget); }
    catch (err) { console.error(`[archive] failed to advance cursor: ${err.message ?? err}`); }
```
(keep the surrounding braces/comment structure intact — the existing comment about best-effort stays, amended by the helper's doc).
- `receiveAll({ since, limit, maxPages = 100 }, receiveFn = receive)` → `receiveAll({ since, limit, maxPages = 100, strict = false }, receiveFn = receive)` and pass `strict` into each `receiveFn({ ... })` call.

- [ ] **Step 4: Run to verify pass**

Run: `node --test test/core.test.mjs`
Expected: PASS (new policy tests + all existing).

- [ ] **Step 5: Commit**

```bash
git add src/core.mjs test/core.test.mjs
git commit -m "feat(core): strict cursor policy — daemon mode never advances past a failed archive write"
```

---

### Task 4: Daemon uses strict mode

**Files:**
- Modify: `src/daemon.mjs` (`runDaemon` threads a strict `receiveFn` into `watch`)
- Test: `test/daemon.test.mjs` (append)

- [ ] **Step 1: Write the failing test** (append to `test/daemon.test.mjs`, using its existing stub idiom):

```js
test("runDaemon: drives watch with a STRICT receiveFn (archive is the replay source)", async () => {
  let sawOpts = null;
  const watchFn = async ({ receiveFn }) => { sawOpts = await receiveFn({ since: 1 }); };
  await runDaemon({
    identity: { did: "did:wba:me" }, sinks: [], watchFn, log: () => {},
    receiveAllFn: async (opts) => opts,            // injectable seam: echo what receive would get
  });
  assert.equal(sawOpts.strict, true);
  assert.equal(sawOpts.since, 1);
});
```

- [ ] **Step 2: Run to verify failure**

Run: `node --test test/daemon.test.mjs`
Expected: FAIL — `watchFn` receives no `receiveFn` / no strict.

- [ ] **Step 3: Implement.** In `src/daemon.mjs`:
- Add the import: `import { receiveAll } from "./core.mjs";`
- Extend `runDaemon`'s signature with `receiveAllFn = receiveAll` and pass a strict wrapper into the watch call:

```js
export async function runDaemon({ identity, sinks, signal, watchFn = watch, receiveAllFn = receiveAll, log = (s) => process.stderr.write(s + "\n") }) {
  log(`[daemon] up: ${identity.did} · sinks: ${sinks.map((s) => s.name).join(", ") || "(none)"}`);
  await watchFn({
    signal,
    identity,
    notifier: { notify: async () => {} }, // the banner is a SINK now, not watch's own notifier
    openResolver: () => null,
    receiveFn: (opts = {}) => receiveAllFn({ ...opts, strict: true }),   // spec §6: archive-precondition cursor
    onMessage: (m) => fanOut(m, sinks, log),
  });
}
```

- [ ] **Step 4: Run to verify pass**

Run: `node --test test/daemon.test.mjs test/daemon-ipc.test.mjs`
Expected: ALL PASS (the Phase-2 composition test still passes — `watchFn` stubs ignore the extra option).

- [ ] **Step 5: Commit**

```bash
git add src/daemon.mjs test/daemon.test.mjs
git commit -m "feat(daemon): pull in strict mode — cursor waits for the archive (replay source integrity)"
```

---

### Task 5: Per-role overflow — skip+count, gap on drain, destroy backstop

**Files:**
- Modify: `src/daemon-ipc.mjs` (`createIpcServer` subscriber state + `deliver`)
- Test: `test/daemon-ipc.test.mjs` (append)

- [ ] **Step 1: Write the failing tests** (append; reuse `ipcFor`/`rawClient`/`until`):

```js
test("overflow(channel): writes are skipped over the HWM, then ONE gap frame arrives on drain", async () => {
  chmodSync(dir, 0o700);
  const ipc = ipcFor({ highWaterMark: 2048 });
  await ipc.listen();
  try {
    const ch = await rawClient("channel");
    const mk = (i) => ({ envelope_id: `eC${i}`, seq: 100 + i, from: "did:wba:x", contact: "al",
      verified: true, key_changed: false, body: { type: "text", text: "y".repeat(16384) } });
    ch.sock.pause();                                  // wedge the client
    for (let i = 0; i < 24; i++) await ipc.sink.deliver(mk(i));
    assert.equal(ipc.clientCount(), 1);               // NOT destroyed (was Phase-2 behavior)
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
    for (let i = 0; i < 24; i++) await ipc.sink.deliver(mk(i, 16384));   // wedge past HWM → drops accumulate
    assert.equal(ipc.clientCount(), 1);                                   // not destroyed (below 4× backstop)
    // Slow-steady regime: partial reads free the queue gradually; the gap must arrive on the
    // next successful WRITE below the threshold — NOT only when the buffer fully empties.
    let gotGap = false;
    const tick = setInterval(() => { ch.sock.read(65536); }, 10);
    try {
      for (let i = 0; i < 200 && !gotGap; i++) {
        await ipc.sink.deliver(mk(100 + i, 64));                          // small follow-ups
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
      body: { type: "text", text: "z".repeat(16384) } });
    for (let i = 0; i < 24; i++) await ipc.sink.deliver(fat(i));
    assert.equal(ipc.clientCount(), 1);               // stays connected
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
```

- [ ] **Step 2: Run to verify failure**

Run: `node --test test/daemon-ipc.test.mjs`
Expected: FAIL — Phase-2 destroys at 1×HWM (first test's `clientCount()===1` assertion breaks).

- [ ] **Step 3: Implement.** In `createIpcServer`, replace the subscriber bookkeeping + `deliver`:

(a) When registering a subscriber (the `hello` branch), create it with state:
```js
        sub = { socket, role: frame.role, lastSeq: null, dropped: 0 };
```
(b) After registration, give the socket a drain handler that converts accumulated drops into a single gap (channel) or a log line (viewer/bridge):
```js
        // C1 (critic, empirically reproduced): 'drain' fires only after a write() returned
        // false AND the buffer empties to ZERO — a slow-but-steady reader can keep the queue
        // non-empty forever while our skip-pattern suppresses write(), starving the gap.
        // Therefore: flush pending drops on ANY progress (drain OR a successful write below
        // the threshold), never on drain alone.
        const flushPending = () => {
          if (!sub || sub.dropped === 0) return;
          if (sub.socket.writableLength > highWaterMark) return;   // still congested — wait for progress
          if (sub.role === "channel") {
            socket.write(encodeFrame({ type: "gap", after_seq: sub.lastSeq ?? 0 }));
            log(`[daemon] gap → channel client (skipped ${sub.dropped}, after_seq=${sub.lastSeq ?? 0})`);
          } else {
            log(`[daemon] dropped ${sub.dropped} msgs to ${sub.role} client (slow consumer)`);
          }
          sub.dropped = 0;
        };
        sub.flushPending = flushPending;
        socket.on("drain", flushPending);
```
(c) Replace `deliver` with skip-over-HWM + backstop:
```js
      deliver: (m) => {
        // Stamp relay_seq at the boundary (Phase 2): onMessage objects carry `seq`.
        const wire = m && m.seq !== undefined && m.relay_seq === undefined ? { ...m, relay_seq: m.seq } : m;
        for (const sub of subscribers) {
          if (!admitForRole(sub.role, m, { mute })) continue;
          const queued = sub.socket.writableLength;
          if (queued > highWaterMark * 4) {
            // Truly wedged: protect the daemon (absolute backstop; spec §6 floor).
            log(`[daemon] destroying wedged ${sub.role} client (writableLength=${queued})`);
            sub.socket.destroy();
            continue;
          }
          if (queued > highWaterMark) {
            // Soft overflow: skip this write. channel recovers via gap+replay on drain;
            // viewer/bridge are best-effort (drop + count) per spec §6.
            sub.dropped += 1;
            continue;
          }
          sub.socket.write(encodeFrame({ type: "message", message: wire }));
          if (wire && wire.relay_seq !== undefined) sub.lastSeq = wire.relay_seq;
          // INVARIANT (critic M1): every channel-admissible message carries seq (receive()
          // always sets it), so lastSeq is never null once a real write happened — a gap's
          // after_seq=0 full-window replay can only occur before ANY successful write (bounded
          // by replaySince's limit).
          sub.flushPending?.();   // C1: emit any pending gap now that we made progress
        }
      },
```

- [ ] **Step 4: Run to verify pass**

Run: `node --test test/daemon-ipc.test.mjs`
Expected: ALL PASS — including the Phase-2 suite (note: the old "stuck client is dropped" test asserted destroy at 1×HWM with 64KiB bodies and HWM=1024; 64 KiB > 4×1024 so it still destroys — the backstop preserves it. If it fails, adjust THAT test's name/comment to "backstop" semantics, not its assertions).

- [ ] **Step 5: Commit**

```bash
git add src/daemon-ipc.mjs test/daemon-ipc.test.mjs
git commit -m "feat(daemon): per-role overflow — skip+count, gap-on-drain for channel, 4xHWM backstop"
```

---

### Task 6: Client side — `onGap` + the replay pipeline

**Files:**
- Modify: `src/daemon-ipc.mjs` (`connectDaemon` gains `onGap`)
- Create: `src/channel-replay.mjs` (row→message adapter + deduped replayer — pure, testable)
- Modify: `src/channel-server.mjs` (wire `onGap` → replayer)
- Test: `test/channel-replay.test.mjs` (new), `test/daemon-ipc.test.mjs` (append one onGap test)

- [ ] **Step 1: Write the failing tests.**

Append to `test/daemon-ipc.test.mjs` — a REAL gap round-trip (critic M3: required, not optional; the handle exposes its socket as a test seam):
```js
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
    for (let i = 1; i <= 24; i++) await ipc.sink.deliver(mk(i, 16384));   // skips accumulate
    handle._sock.resume();                                    // progress
    await ipc.sink.deliver(mk(99, 64));                       // flush-on-progress emits the gap
    await until(() => gapAt !== null);
    assert.ok(gapAt >= 300 && gapAt < 324, `after_seq=${gapAt} must be the last WRITTEN seq`);
    handle.close();
  } finally { await ipc.close(); }
});
```



Create `test/channel-replay.test.mjs`:
```js
import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { rowToMessage, makeReplayer } from "../src/channel-replay.mjs";

let dir;
beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "air-msg-replay-"));
  process.env.AGENT_BRIDGE_HOME = dir;
});
afterEach(() => {
  rmSync(dir, { recursive: true, force: true });
});

const row = (over = {}) => ({
  envelope_id: "e1", direction: "received", thread_id: "t1",
  peer_did: "did:wba:p", from: "did:wba:p", to: "did:wba:me", timestamp: "2026-06-10T00:00:00.000Z",
  body: { type: "text", text: "hi" }, encrypted: true, verified: true, key_changed: false,
  spam: false, relay_seq: 41, room_id: undefined, archived_at: "2026-06-10T00:00:01.000Z", ...over,
});

test("rowToMessage: maps an archive row to the wire/live message shape, re-deriving contact from CURRENT pins", () => {
  const m = rowToMessage(row(), { contactLookup: (did) => (did === "did:wba:p" ? { alias: "pat" } : undefined) });
  assert.equal(m.envelope_id, "e1");
  assert.equal(m.from, "did:wba:p");
  assert.equal(m.contact, "pat");                  // current pin state, not a stored alias
  assert.equal(m.seq, 41);
  assert.equal(m.relay_seq, 41);
  assert.equal(m.verified, true);
  assert.equal(m.key_changed, false);
  assert.equal(m.received_at, "2026-06-10T00:00:00.000Z");
  assert.deepEqual(m.body, { type: "text", text: "hi" });
});

test("rowToMessage: an unpinned sender yields no contact (the channel gate will withhold it)", () => {
  const m = rowToMessage(row(), { contactLookup: () => undefined });
  assert.equal(m.contact, undefined);
});

test("makeReplayer: replays rows after the gap through push, dedupes envelope_ids across replay+live", async () => {
  const pushed = [];
  const rows = [row({ envelope_id: "eA", relay_seq: 42 }), row({ envelope_id: "eB", relay_seq: 43 })];
  const replayer = makeReplayer({
    push: (m) => pushed.push(m.envelope_id),
    replaySinceFn: (s) => rows.filter((r) => r.relay_seq > s),
    contactLookup: () => ({ alias: "pat" }),
  });
  replayer.live({ envelope_id: "eA", seq: 42 });     // live frame seen BEFORE the gap fires
  await replayer.gap(41);
  assert.deepEqual(pushed, ["eA", "eB"]);            // eA pushed once (live), replay added only eB... 
});

test("makeReplayer: live() pushes and records; replay never double-pushes what live already pushed", async () => {
  const pushed = [];
  const replayer = makeReplayer({
    push: (m) => pushed.push(m.envelope_id),
    replaySinceFn: () => [row({ envelope_id: "eDup", relay_seq: 50 })],
    contactLookup: () => undefined,
  });
  replayer.live({ envelope_id: "eDup", seq: 50 });
  await replayer.gap(49);
  assert.deepEqual(pushed, ["eDup"]);                // exactly once
});

test("makeReplayer: a sender blocked AFTER archival is not replayed (live drops blocked at receive)", async () => {
  const pushed = [];
  const replayer = makeReplayer({
    push: (m) => pushed.push(m.envelope_id),
    replaySinceFn: () => [row({ envelope_id: "eBlocked", relay_seq: 60, from: "did:wba:evil" }),
                          row({ envelope_id: "eFine", relay_seq: 61 })],
    contactLookup: () => ({ alias: "pat" }),
    isBlockedFn: (did) => did === "did:wba:evil",
  });
  await replayer.gap(59);
  assert.deepEqual(pushed, ["eFine"]);     // critic H1: the blocklist holds on replay too
});

test("makeReplayer: bounded memory — the seen-set keeps only the most recent maxSeen ids", async () => {
  const replayer = makeReplayer({ push: () => {}, replaySinceFn: () => [], contactLookup: () => undefined, maxSeen: 3 });
  for (let i = 0; i < 10; i++) replayer.live({ envelope_id: `e${i}`, seq: i });
  assert.equal(replayer.seenSize(), 3);
});
```

- [ ] **Step 2: Run to verify failure**

Run: `node --test test/channel-replay.test.mjs`
Expected: FAIL — module does not exist.

- [ ] **Step 3: Implement.**

Create `src/channel-replay.mjs`:
```js
// src/channel-replay.mjs — the channel client's at-least-once recovery (spec §6).
// On {type:"gap", after_seq} the client replays the hole from the LOCAL archive and pushes
// each row through the SAME makeChannelPush pipeline as live frames. The pipeline's own gate
// (channelGate / room gates) re-filters every replayed row — rows carry verified + key_changed,
// and `contact` is re-derived from CURRENT pin state, so replay can never push more than live.
import { replaySince } from "./archive.mjs";
import { getContactByDid } from "./contacts.mjs";
import { isBlocked } from "./moderation.mjs";

/** Map an archive row (parseRow shape) to the live/wire message shape makeChannelPush expects. */
export function rowToMessage(row, { contactLookup = getContactByDid } = {}) {
  const contact = contactLookup(row.from);
  return {
    seq: row.relay_seq,
    relay_seq: row.relay_seq,
    from: row.from,
    ...(contact?.alias ? { contact: contact.alias } : {}),
    envelope_id: row.envelope_id,
    received_at: row.timestamp,
    verified: row.verified,
    encrypted: row.encrypted,
    ...(row.key_changed ? { key_changed: true } : {}),
    ...(row.room_id ? { room_id: row.room_id } : {}),
    body: row.body,
    thread_id: row.thread_id,
  };
}

/** Deduped replay coordinator. live(m) for every streamed frame; gap(after_seq) replays the
 *  hole. A bounded seen-set (envelope_id) prevents double-push where replay and live overlap —
 *  best-effort dedup (critic L2): eviction under sustained back-to-back gaps can in principle
 *  allow a rare double-push, which is harmless under at-least-once semantics. Do not "fix" the
 *  bound into an unbounded set. Blocked senders are skipped (critic H1): live enforces the
 *  blocklist at receive (core.mjs:397) and NO downstream gate rechecks it, so replay must. */
export function makeReplayer({ push, replaySinceFn = replaySince, contactLookup = getContactByDid, isBlockedFn = isBlocked, maxSeen = 1000, log = (s) => process.stderr.write(s + "\n") }) {
  const seen = new Set();
  const remember = (id) => {
    seen.add(id);
    if (seen.size > maxSeen) seen.delete(seen.values().next().value);   // FIFO-ish bound
  };
  return {
    live: (m) => {
      if (m?.envelope_id) {
        if (seen.has(m.envelope_id)) return;
        remember(m.envelope_id);
      }
      push(m);
    },
    gap: async (after_seq) => {
      const rows = replaySinceFn(after_seq);
      log(`[channel] gap after_seq=${after_seq} — replaying ${rows.length} from archive`);
      for (const row of rows) {
        if (isBlockedFn(row.from)) continue;   // critic H1: blocked-after-archive must not replay
        if (seen.has(row.envelope_id)) continue;
        remember(row.envelope_id);
        push(rowToMessage(row, { contactLookup }));
      }
    },
    seenSize: () => seen.size,
  };
}
```

In `src/daemon-ipc.mjs`, extend `connectDaemon({ role, onMessage, onClose, onGap = () => {}, ... })`, route the frame (one line in the parser, next to the `message` route), and expose the socket on the resolved handle as a test seam (critic M3 — the real round-trip test above depends on it):
```js
      if (frame.type === "gap") onGap(frame.after_seq);
```
```js
          resolve({ close: () => sock.destroy(), _sock: sock });   // _sock: test seam (gap round-trip), not public API
```

In `src/channel-server.mjs`, wire the replayer on the daemon-attached path:
```js
import { makeReplayer } from "./channel-replay.mjs";
```
and replace the `connectDaemon({ role: "channel", onMessage: push, ... })` call so both live and gap routes flow through the replayer:
```js
    const replayer = makeReplayer({ push, log });
    await connectDaemon({
      role: "channel",
      onMessage: (m) => replayer.live(m),
      onGap: (after_seq) => { replayer.gap(after_seq).catch((e) => log(`[channel] replay failed: ${e.message ?? e}`)); },
      onClose: () => {
        log("air-msg-channel: daemon connection closed — exiting cleanly (Phase 4 adds reconnect)");
        process.exit(0);
      },
      log,
    });
```
(The legacy standalone path keeps plain `push` — no daemon, no gaps.)

- [ ] **Step 4: Run to verify pass**

Run: `node --check src/channel-server.mjs && node --test test/channel-replay.test.mjs test/daemon-ipc.test.mjs`
Expected: clean check; ALL PASS.

- [ ] **Step 5: Commit**

```bash
git add src/channel-replay.mjs src/daemon-ipc.mjs src/channel-server.mjs test/channel-replay.test.mjs test/daemon-ipc.test.mjs
git commit -m "feat(channel): gap-triggered archive replay — deduped, gate-refiltered, at-least-once"
```

---

### Task 7: Spec §6 note + full verification + PR

**Files:**
- Modify: `agent-bridge-mcp/docs/superpowers/specs/2026-06-05-receiver-daemon-design.md` (§6)

- [ ] **Step 1: Spec note.** In §6, append one bullet at the end of the section:

```markdown
- **Phase 3 (2026-06-10) implements this** with refinements: replay fidelity requires the
  archive to record `key_changed` (added, default 0); the replay adapter re-derives `contact`
  from CURRENT pins and re-checks the BLOCKLIST (live enforces it only at receive — replay must
  too); synthetic room-join notices are excluded from replay (live never pushes them as chat).
  Overflow is skip+count for all roles; a `channel` subscriber's pending gap is emitted on
  **flush-on-progress** (the next successful write below the threshold OR drain — never drain
  alone, which starves under slow-steady readers), with a 4×HWM destroy backstop for wedged
  sockets. The concrete numbers supersede this section's illustrative 256 KiB: skip above
  1 MiB (`highWaterMark`), destroy at 4×. Strict cursor mode lives behind `receive({strict})`,
  used only by the daemon — it trades liveness for completeness: a persistently failing archive
  (e.g. disk full) halts cursor advance and re-delivers the current page each wake (banner
  re-rings) until writes succeed; degraded-but-safe and self-correcting. The "OR reconnect" gap
  trigger in this section is Phase 4 (resume-on-reattach via since_seq in hello).
```

- [ ] **Step 2: Full suite + hermeticity**

```bash
cd ~/air-note/agent-bridge-mcp
before=$(sqlite3 -readonly ~/.air-msg/archive.db "SELECT COUNT(*) FROM messages")
node --test 2>&1 | grep -E "^ℹ (tests|pass|fail|todo)"
after=$(sqlite3 -readonly ~/.air-msg/archive.db "SELECT COUNT(*) FROM messages")
echo "real-archive delta: $((after-before)) (must be 0)"
```
Expected: `fail 0`, delta 0. (Baseline at branch: 260 tests / 257 pass / 3 todo.)

- [ ] **Step 3: Commit + PR**

```bash
git add agent-bridge-mcp/docs/superpowers/specs/2026-06-05-receiver-daemon-design.md
git commit -m "docs(daemon): spec §6 — Phase 3 implementation notes (key_changed fidelity, gap-on-drain, strict cursor)"
git push -u origin feat/daemon-phase3-delivery
gh pr create --repo AgentIdentityRegistry/air-note --base main \
  --title "feat(daemon): Phase 3 — at-least-once channel delivery (gap/replay) + strict cursor"
```
No manual live smoke this phase: a gap is hard to stage by hand and the drain path is integration-tested over real sockets (Task 5); the Phase-2 smoke already proved the live relay→socket path. State this in the PR body.

---

## Self-Review (against spec §6 + the Phase-2 review bar)

- **§6 channel at-least-once:** gap on drain (T5) + archive replay deduped by envelope_id (T6) + strict cursor so the archive is complete (T3, T4). ✓
- **§6 viewer/bridge best-effort:** skip+count+log, client stays; 4×HWM backstop replaces Phase 2's 1×HWM destroy (T5) — Phase 2's floor remains as the backstop, not deleted. ✓
- **Replay fidelity:** three holes closed — the archive lacked `key_changed` (T1, found in planning); the BLOCKLIST is enforced only at receive so replay re-checks it (T6, critic H1); synthetic join notices are excluded from replay so a system notice can never re-enter as room chat (T2, critic H2). `contact` re-derived from current pins; the client pipeline re-gates everything. ✓
- **Gap liveness (critic C1, empirically reproduced):** gap emission is flush-on-progress — pending drops flush on the next successful write below the threshold OR on drain, never drain-alone (which starves under slow-steady readers). Tested in BOTH regimes: pause-resume recovery AND the slow-steady partial-read loop. ✓
- **Pure seams for the untestable:** cursor policy is a pure function (T3) because fault-injecting `receive()`'s archive writes would need network stubs; the wiring is 6 lines verified by review; the daemon's strict flag is tested via the injectable `receiveAllFn` (T4). Honest coverage boundary, stated. ✓
- **The connectDaemon gap test is a REAL round-trip** (critic M3): the handle exposes `_sock` as a documented test seam; the test wedges beneath the client parser, makes progress, and asserts `onGap` fires with the last-written seq. ✓
- **Placeholder scan:** every step has complete code; names consistent (`replaySince`, `cursorAdvanceTarget`, `rowToMessage`, `makeReplayer` (`live`/`gap`/`seenSize`, opts incl. `isBlockedFn`), `receiveAllFn`, `onGap`, `_sock`, sub state `{lastSeq, dropped, flushPending}`). The v1 essay-comment test was replaced by the real round-trip. ✓
- **Phase boundary:** reconnect/backoff, `since_seq` in hello (resume-on-reattach), bridge socket role, installers — all Phase 4. ✓
