# AI Inbox — Implementation Plan (Phase A1: messaging-stack delta — send op + archive WAL + protocol contract)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** v2 — v1 review returned **REWORK** with empirically proven findings, all applied: **C1** the E2E stub relay now echoes `envelope_id: envelope.id` in its receipt (`core.send` returns `receipt.envelope_id` — the relay's word, not the local uuid; the v1 stub returned neither, a guaranteed-red E2E that also contradicted the normative fixture); **H1** `plaintext` is DESIGNED INTO the send op (frame field, forwarding, fixture, PROTOCOL.md, unit test) instead of a mid-E2E pivot that reopened three committed tasks; **H2** the `message` fixture now matches a REAL `deliver()` emission (conditional fields like `key_changed` are OMITTED when falsy — pinned by a live deliver-vs-fixture deepEqual test) and PROTOCOL.md documents optional-when-falsy fields; **H3** the busy_timeout assertion reads the row's value key-agnostically (Node 22 CI vs 25 dev); plus M1 (classifier branch comment), M2 (import folded), M3 (executor return-placement self-check), a send-racing-shutdown test, `encrypted:false` asserted on the plaintext ack, an E2E stderr drain, and a Task 6 production-relay receipt verification.

**Goal:** The two messaging-stack changes the desktop AI-inbox design (spec `docs/superpowers/specs/2026-06-11-desktop-ai-inbox-design.md`, repo root) requires: a `{type:"send"}` request op on the daemon socket (with a retryable/terminal error taxonomy) and `archive.db` in WAL mode (cross-process readers must not lock against the daemon writer), plus the protocol contract artifacts (`PROTOCOL.md` + cross-language frame fixtures) that Phase A2's Rust client will be built against.

**Architecture:** The daemon stays the single archive writer and the only key-holder: a post-hello subscriber sends `{type:"send", id, to, body}`; the daemon runs the EXISTING `core.send` (resolve → seal → sign → POST → archive) through an injectable `sendFn` seam and answers `{type:"send-ok", id, envelope_id, encrypted}` or `{type:"send-err", id, retryable, reason}`. Retryability is classified by a pure function over the thrown error (relay 5xx / network → retryable; validation / unresolvable / refuse-unencrypted → terminal); `postEnvelope` gains a structured `status` field on its error so the classifier never regex-parses messages. WAL is a one-PRAGMA writer-side change with a busy_timeout; readers are Phase A2's concern but an in-process two-connection test pins the property now.

**Tech Stack:** Node ≥22 stdlib only. No new dependencies.

**Repo rules that bind every task:** temp-home idiom in every test (`bridgeHome()` THROWS under the runner without `AGENT_BRIDGE_HOME`); import shared helpers, never copy; run tests as single files only (`node --test test/<file>` — NEVER `node --test test/`, broken on Node 25); branch `feat/ai-inbox-a1-stack` from current `main` (`622caa4`); work from `~/air-note/agent-bridge-mcp`.

**Wire facts this plan relies on (verified 2026-06-11 against main `622caa4`):**
- `postEnvelope(identity, recipient, envelope)` (core.mjs:347-354): `fetch` POST to `${identity.relay_url}/inbox/<recipient>`; on non-OK throws `new Error(\`relay ${resp.status}: ...\`)` — status embedded in TEXT only (no structured field today); network failure surfaces as fetch's `TypeError` (with `cause`).
- `core.send({to, body, thread_id, in_reply_to, plaintext})` (core.mjs:355+): validates to/body (plain `Error`s), `resolveRecipient`, `resolveAgentPublicKey` (refuse-unencrypted error at core.mjs:130-135: "refusing to send unencrypted"), `buildOutboundEnvelope`, `postEnvelope`, then best-effort `archiveMessage` of the sent row. No `process.exit`, no stdout coupling — safe for IPC invocation (verified by the design's second-opinion review).
- `openArchive()` (archive.mjs:50-74): memoized `_db`; `new DatabaseSync(path)`; NO journal_mode/busy_timeout PRAGMAs today (rollback journal — the cross-process locking hazard the design's review flagged as Critical); existing ALTER-TABLE migration pattern with `PRAGMA table_info`.
- daemon-ipc post-hello frame handler (daemon-ipc.mjs ~L164-178): `ping` → pong; `status` → status reply (requester excluded); unknown frames ignored. `sub` is the per-connection subscriber record; pre-hello non-hello frames get `{type:"error"}` + destroy.
- `createIpcServer({mute, daemonInfo, highWaterMark, helloTimeoutMs, statusExtraFn, log})` (daemon-ipc.mjs:104-118); returns `{sink, clientCount, clientStats, listen, close}`. `startDaemon` (daemon.mjs:74+) constructs it and currently passes `mute`, `daemonInfo`, `statusExtraFn`, `log`.
- Test helpers in test/daemon-ipc.test.mjs: `ipcFor(over)`, `rawClient(role, helloExtra)`, `until(cond, ms)`; temp-home beforeEach/afterEach.
- Spawn-test idiom in test/cli-daemon-table.test.mjs: seeded REAL-shape `identity.json` (`seed_hex` load-bearing; `relay_url`/`air_url` controllable per fixture), `runCli`, `waitExit`, `until`.
- Suite baseline: **305 tests / 302 pass / 0 fail / 3 todo**, hermetic.
- **Verify at execution (not yet confirmed):** node:sqlite `DatabaseSync` exposes `.exec(sql)` for PRAGMAs (if not, use `db.prepare("PRAGMA journal_mode=WAL").get()` — journal_mode RETURNS a row and must be read, not run; busy_timeout likewise returns a row in some builds — prefer `.get()` for both if `.exec` misbehaves).

---

### Task 1: Archive goes WAL (writer-side; the cross-process reader unblock)

**Files:**
- Modify: `src/archive.mjs` (`openArchive`, after `new DatabaseSync(path)`)
- Test: `test/archive.test.mjs` (append)

- [ ] **Step 1: Write the failing tests** (append to `test/archive.test.mjs`, inside its temp-home scaffolding; add `DatabaseSync` to the existing `node:sqlite` import if absent, plus `archivePath` to the archive import if exported — if `archivePath` is not exported, derive the path as `join(process.env.AGENT_BRIDGE_HOME, "archive.db")`):

```js
test("openArchive: journal mode is WAL with a busy timeout (cross-process readers must not lock against the writer)", () => {
  openArchive();                                            // first open sets the persistent property
  const db = openArchive();
  assert.equal(db.prepare("PRAGMA journal_mode").get().journal_mode, "wal");
  // Key-agnostic read (critic H3): Node 22 (CI) and 25 (dev) may name the busy_timeout row's
  // column differently; the VALUE is the contract.
  assert.equal(Number(Object.values(db.prepare("PRAGMA busy_timeout").get())[0]), 5000);
});

test("openArchive: a pre-existing rollback-journal DB converts to WAL on open", () => {
  // Simulate an archive created by an OLDER build: same schema, default journal mode.
  const path = join(process.env.AGENT_BRIDGE_HOME, "archive.db");
  mkdirSync(process.env.AGENT_BRIDGE_HOME, { recursive: true, mode: 0o700 });
  const old = new DatabaseSync(path);
  old.exec("CREATE TABLE IF NOT EXISTS marker (x INTEGER)");
  assert.equal(old.prepare("PRAGMA journal_mode").get().journal_mode, "delete");
  old.close();
  const db = openArchive();                                 // our open must convert it
  assert.equal(db.prepare("PRAGMA journal_mode").get().journal_mode, "wal");
});

test("openArchive: a second READ-ONLY connection reads while the writer holds rows (WAL property)", () => {
  archiveMessage(rec({ envelope_id: "wal1" }));
  const path = join(process.env.AGENT_BRIDGE_HOME, "archive.db");
  const reader = new DatabaseSync(path, { readOnly: true });
  try {
    const n = reader.prepare("SELECT COUNT(*) AS n FROM messages").get().n;
    assert.equal(n >= 1, true);                             // reader sees committed rows, no SQLITE_BUSY
  } finally { reader.close(); }
});
```
(`rec` is the file's existing row-builder helper — reuse it; do NOT redefine. If the second test's
"delete" assertion fails because node:sqlite defaults differently on this platform, assert
`!== "wal"` instead — the conversion claim is the point, not the legacy mode's name.)

- [ ] **Step 2: Run to verify failure**

Run: `node --test test/archive.test.mjs`
Expected: FAIL — journal_mode is `delete` (or another non-wal mode), busy_timeout 0.

- [ ] **Step 3: Implement.** In `src/archive.mjs` `openArchive()`, immediately after `const db = new DatabaseSync(path);` add:

```js
  // WAL + busy timeout (AI-inbox design §5, second-opinion Critical): the desktop reads this DB
  // read-only from another process while the daemon writes. Under the default rollback journal a
  // writer's EXCLUSIVE lock and a reader's SHARED lock are mutually exclusive → SQLITE_BUSY
  // exactly during gap-replay bursts. WAL allows one writer + many readers; it is a PERSISTENT
  // file property the WRITER must set. journal_mode/busy_timeout are read-back pragmas — use
  // .get(), not .run() (some builds error on run for row-returning pragmas).
  // busy_timeout FIRST: the WAL conversion itself can contend with a concurrent writer and must
  // inherit the retry window.
  db.prepare("PRAGMA busy_timeout=5000").get();
  db.prepare("PRAGMA journal_mode=WAL").get();
```

- [ ] **Step 4: Run to verify pass**

Run: `node --test test/archive.test.mjs test/archive-rooms.test.mjs test/archive-integration.test.mjs`
Expected: ALL PASS (existing tests unaffected — WAL is transparent to single-connection use).

- [ ] **Step 5: Commit**

```bash
git add src/archive.mjs test/archive.test.mjs
git commit -m "feat(archive): WAL journal + busy timeout — cross-process readers must not lock against the daemon"
```

---

### Task 2: Structured relay errors + the retryable/terminal classifier

**Files:**
- Modify: `src/core.mjs` (`postEnvelope` error gains `status`; new pure export `classifySendError`)
- Test: `test/core.test.mjs` (append)

- [ ] **Step 1: Write the failing tests** (append to `test/core.test.mjs`):

```js
// Fold classifySendError into test/core.test.mjs's EXISTING core import line (currently
// `import { resolveRecipient, didFromAirId, cursorAdvanceTarget } from "../src/core.mjs";` at
// the top of the file) — never a duplicate module import.

test("classifySendError: relay 5xx and network failures are retryable", () => {
  assert.deepEqual(classifySendError(Object.assign(new Error("relay 503: nope"), { status: 503 })),
    { retryable: true, reason: "relay 503: nope" });
  const netErr = new TypeError("fetch failed");
  netErr.cause = Object.assign(new Error("connect ECONNREFUSED"), { code: "ECONNREFUSED" });
  assert.equal(classifySendError(netErr).retryable, true);
  assert.equal(classifySendError(Object.assign(new TypeError("fetch failed"), {})).retryable, true);
});

test("classifySendError: relay 4xx, validation, and refuse-unencrypted are terminal", () => {
  assert.equal(classifySendError(Object.assign(new Error("relay 404: unknown inbox"), { status: 404 })).retryable, false);
  assert.equal(classifySendError(new Error("recipient (DID, AIR ID, or contact alias) is required")).retryable, false);
  assert.equal(classifySendError(new Error("cannot resolve recipient's key from AIR — refusing to send unencrypted. Pass plaintext:true to send in the clear on purpose.")).retryable, false);
  assert.equal(classifySendError(new Error("anything unknown")).retryable, false);   // default terminal
});
```

- [ ] **Step 2: Run to verify failure**

Run: `node --test test/core.test.mjs`
Expected: FAIL — `classifySendError` not exported.

- [ ] **Step 3: Implement.** In `src/core.mjs`:

(a) In `postEnvelope` (core.mjs:347-354), replace the throw line:
```js
  if (!resp.ok) throw new Error(`relay ${resp.status}: ${await resp.text()}`);
```
with:
```js
  if (!resp.ok) {
    // Structured status (AI-inbox design §3): the send-over-socket ack must tell a GUI whether
    // retrying can help; a classifier should never regex-parse error prose for the code.
    throw Object.assign(new Error(`relay ${resp.status}: ${await resp.text()}`), { status: resp.status });
  }
```

(b) Add the pure classifier near the other small exported helpers:
```js
/** Classify a send() failure for the socket ack (AI-inbox design §3): retryable means "trying
 *  again later can plausibly succeed" — relay 5xx or a network-level fetch failure. Everything
 *  else (relay 4xx, validation, unresolvable recipient, refuse-unencrypted) is terminal: a blind
 *  retry would loop forever. Unknown errors default to TERMINAL — the retry affordance must never
 *  attach to an error we cannot reason about. */
export function classifySendError(err) {
  const reason = String(err?.message ?? err);
  if (typeof err?.status === "number") return { retryable: err.status >= 500, reason };
  // Real undici fetch network failures ride the TypeError branch (their code is often buried
  // DEEPER than cause.code, e.g. under cause.cause or an AggregateError — probed). The code-list
  // branch covers errors WRAPPED by other layers that surface a flat code; it is belt, not braces.
  // AbortError falls through to terminal by default — revisit if a fetch timeout is ever added.
  const networkish = err instanceof TypeError
    || ["ECONNREFUSED", "ENOTFOUND", "ETIMEDOUT", "ECONNRESET", "EAI_AGAIN"].includes(err?.cause?.code ?? err?.code);
  return { retryable: !!networkish, reason };
}
```

- [ ] **Step 4: Run to verify pass**

Run: `node --test test/core.test.mjs`
Expected: PASS (new tests + all existing — the postEnvelope change only ADDS a field).

- [ ] **Step 5: Commit**

```bash
git add src/core.mjs test/core.test.mjs
git commit -m "feat(core): structured relay-error status + classifySendError — retryable vs terminal for GUI acks"
```

---

### Task 3: The `send` request op on the daemon socket

**Files:**
- Modify: `src/daemon-ipc.mjs` (`createIpcServer` gains `sendFn`; post-hello handler gains the send branch)
- Modify: `src/daemon.mjs` (`startDaemon` wires `sendFn: core.send`)
- Test: `test/daemon-ipc.test.mjs` (append)

- [ ] **Step 1: Write the failing tests** (append; reuse `ipcFor`/`rawClient`/`until`):

```js
test("send op: a post-hello subscriber sends; ack carries the correlation id and envelope_id", async () => {
  chmodSync(dir, 0o700);
  const calls = [];
  const ipc = ipcFor({ sendFn: async (args) => { calls.push(args); return { envelope_id: "e-sent-1", encrypted: true }; } });
  await ipc.listen();
  try {
    const v = await rawClient("viewer");                     // roles are delivery filters; send is role-agnostic
    v.sock.write(encodeFrame({ type: "send", id: "corr-1", to: "did:wba:peer", body: { type: "text", text: "hi" } }));
    await until(() => v.frames.some((f) => f.type === "send-ok"));
    const ok = v.frames.find((f) => f.type === "send-ok");
    assert.equal(ok.id, "corr-1");
    assert.equal(ok.envelope_id, "e-sent-1");
    assert.equal(ok.encrypted, true);
    // plaintext is OPTIONAL on the wire and defaults false — absent on the frame, false at sendFn.
    assert.deepEqual(calls, [{ to: "did:wba:peer", body: { type: "text", text: "hi" }, plaintext: false }]);
    v.sock.destroy();
  } finally { await ipc.close(); }
});

test("send op: plaintext:true on the frame is forwarded to sendFn (the CLI --plaintext parity field)", async () => {
  chmodSync(dir, 0o700);
  const calls = [];
  const ipc = ipcFor({ sendFn: async (args) => { calls.push(args); return { envelope_id: "e-pt", encrypted: false }; } });
  await ipc.listen();
  try {
    const v = await rawClient("viewer");
    v.sock.write(encodeFrame({ type: "send", id: "pt-1", to: "did:wba:peer", body: { type: "text", text: "clear" }, plaintext: true }));
    await until(() => v.frames.some((f) => f.type === "send-ok"));
    assert.equal(v.frames.find((f) => f.type === "send-ok").encrypted, false);
    assert.deepEqual(calls, [{ to: "did:wba:peer", body: { type: "text", text: "clear" }, plaintext: true }]);
    v.sock.destroy();
  } finally { await ipc.close(); }
});

test("send op: a send racing daemon shutdown neither crashes nor leaks an unhandled rejection", async () => {
  chmodSync(dir, 0o700);
  let release;
  const gate = new Promise((r) => { release = r; });
  const ipc = ipcFor({ sendFn: async () => { await gate; return { envelope_id: "late", encrypted: true }; } });
  await ipc.listen();
  const v = await rawClient("viewer");
  v.sock.write(encodeFrame({ type: "send", id: "race-1", to: "x", body: { type: "text", text: "a" } }));
  await new Promise((r) => setTimeout(r, 30));             // the send is now pending inside sendFn
  await ipc.close();                                        // shutdown destroys all subscriber sockets
  release();                                                // sendFn settles AFTER the socket died
  await new Promise((r) => setTimeout(r, 50));              // an unhandled rejection would fail the test runner
  assert.ok(true);                                          // surviving to here IS the assertion
});

test("send op: failures ack as send-err with the classifier's retryable verdict", async () => {
  chmodSync(dir, 0o700);
  let fail = Object.assign(new Error("relay 503: down"), { status: 503 });
  const ipc = ipcFor({ sendFn: async () => { throw fail; } });
  await ipc.listen();
  try {
    const ch = await rawClient("channel");
    ch.sock.write(encodeFrame({ type: "send", id: "c1", to: "x", body: { type: "text", text: "a" } }));
    await until(() => ch.frames.some((f) => f.type === "send-err" && f.id === "c1"));
    const e1 = ch.frames.find((f) => f.type === "send-err" && f.id === "c1");
    assert.equal(e1.retryable, true);
    assert.match(e1.reason, /relay 503/);
    fail = new Error("recipient (DID, AIR ID, or contact alias) is required");
    ch.sock.write(encodeFrame({ type: "send", id: "c2", to: "", body: { type: "text", text: "a" } }));
    await until(() => ch.frames.some((f) => f.type === "send-err" && f.id === "c2"));
    assert.equal(ch.frames.find((f) => f.type === "send-err" && f.id === "c2").retryable, false);
    ch.sock.destroy();
  } finally { await ipc.close(); }
});

test("send op: malformed requests (missing id/to/body) ack terminal without calling sendFn; pre-hello send is refused", async () => {
  chmodSync(dir, 0o700);
  let called = 0;
  const ipc = ipcFor({ sendFn: async () => { called += 1; return { envelope_id: "x", encrypted: true }; } });
  await ipc.listen();
  try {
    const v = await rawClient("viewer");
    v.sock.write(encodeFrame({ type: "send", id: "m1", body: { type: "text", text: "no-to" } }));
    await until(() => v.frames.some((f) => f.type === "send-err" && f.id === "m1"));
    assert.equal(v.frames.find((f) => f.id === "m1").retryable, false);
    v.sock.write(encodeFrame({ type: "send", to: "x", body: { type: "text", text: "no-id" } }));
    await new Promise((r) => setTimeout(r, 50));
    assert.equal(v.frames.some((f) => f.type === "send-err" && f.id === undefined), false);  // no-id → ignored (cannot correlate an ack)
    assert.equal(called, 0);
    v.sock.destroy();
    // Pre-hello: the first frame must be hello — a send instead gets the standard error+destroy.
    const raw = createConnection(socketPath());
    const frames = [];
    const feed = makeLineParser((f) => frames.push(f), { onError: () => {} });
    raw.on("data", feed);
    await new Promise((r) => raw.once("connect", r));
    raw.write(encodeFrame({ type: "send", id: "ph", to: "x", body: { type: "text", text: "a" } }));
    await until(() => frames.some((f) => f.type === "error"));
    assert.equal(called, 0);
    raw.destroy();
  } finally { await ipc.close(); }
});

test("send op: the wire reason is capped and control-char-stripped (relay-controlled text must not flood the socket)", async () => {
  chmodSync(dir, 0o700);
  const huge = "relay 502: " + "<html>x\x07\x00".repeat(4000);   // ~36k chars + control bytes
  const ipc = ipcFor({ sendFn: async () => { throw Object.assign(new Error(huge), { status: 502 }); } });
  await ipc.listen();
  try {
    const v = await rawClient("viewer");
    v.sock.write(encodeFrame({ type: "send", id: "cap-1", to: "x", body: { type: "text", text: "a" } }));
    await until(() => v.frames.some((f) => f.type === "send-err" && f.id === "cap-1"));
    const e = v.frames.find((f) => f.id === "cap-1");
    assert.equal(e.reason.length <= 512, true, `reason must be capped, got ${e.reason.length}`);
    assert.equal(/[\x00-\x08\x0b-\x1f\x7f]/.test(e.reason), false, "control chars must be stripped");
    assert.equal(e.retryable, true);                       // the verdict is untouched by clipping
    v.sock.destroy();
  } finally { await ipc.close(); }
});

test("send op: no sendFn wired → terminal send-err 'send unavailable'", async () => {
  chmodSync(dir, 0o700);
  const ipc = ipcFor();                                      // ipcFor passes no sendFn
  await ipc.listen();
  try {
    const v = await rawClient("viewer");
    v.sock.write(encodeFrame({ type: "send", id: "u1", to: "x", body: { type: "text", text: "a" } }));
    await until(() => v.frames.some((f) => f.type === "send-err" && f.id === "u1"));
    const e = v.frames.find((f) => f.id === "u1");
    assert.equal(e.retryable, false);
    assert.match(e.reason, /send unavailable/);
    v.sock.destroy();
  } finally { await ipc.close(); }
});
```
(`createConnection` is already imported by the module under test; add it to the TEST file's
`node:net` import if absent, alongside the existing imports of `encodeFrame`, `makeLineParser`,
`socketPath` — extend the existing import lines, never duplicate.)

- [ ] **Step 2: Run to verify failure**

Run: `node --test test/daemon-ipc.test.mjs`
Expected: the four new tests FAIL (no acks arrive; the pre-hello case already passes its error
assertion via the existing handshake guard — its load-bearing assert is `called === 0`).

- [ ] **Step 3: Implement.** In `src/daemon-ipc.mjs`:

(a) Import the classifier (top of file, with the other local imports):
```js
import { classifySendError } from "./core.mjs";
```
And near MAX_FRAME, add the reason cap (Task 2 review, Important):
```js
// send-err reasons ride the wire to GUIs and embed relay-controlled response text, which is
// length-unbounded (a proxy's 502 HTML page, a chatty federated relay). Cap + de-control it at
// the serialization boundary; rendering-side escaping stays the GUI's job.
export const REASON_MAX_CHARS = 512;
const clipReason = (s) => String(s).replace(/[\x00-\x08\x0b-\x1f\x7f]/g, " ").slice(0, REASON_MAX_CHARS);
```
**Circular-import check (do this BEFORE wiring):** `core.mjs` must not import from
`daemon-ipc.mjs` (verify with `grep -n "daemon-ipc" src/core.mjs` — expected: no hits). If a cycle
exists, move `classifySendError` into its own `src/send-verdict.mjs` and import from both sides.

(b) `createIpcServer` options gain (after `statusExtraFn`):
```js
  sendFn = null,                              // injected by startDaemon (core.send); null → send unavailable
```

(c) In the post-hello frame handler, after the `status` branch, add:
```js
      if (frame.type === "send") {
        // Send-over-socket (AI-inbox design §3): the daemon is the only key-holder and the only
        // archive writer, so surfaces send THROUGH it. Role-agnostic by design — roles are
        // DELIVERY filters; the 0600 socket is the OS user boundary (any local process could
        // already run `air-msg send`). No correlation id → no ack (we cannot address one).
        if (frame.id === undefined) return;
        if (!frame.to || frame.body === undefined || frame.body === null) {
          socket.write(encodeFrame({ type: "send-err", id: frame.id, retryable: false, reason: "send requires to + body" }));
          return;
        }
        if (!sendFn) {
          socket.write(encodeFrame({ type: "send-err", id: frame.id, retryable: false, reason: "send unavailable (daemon started without a send function)" }));
          return;
        }
        // Async on purpose: the ack arrives whenever core.send settles; the parser loop never
        // blocks. Guard the write — the requester may have disconnected while the relay was slow.
        // plaintext is optional wire-side and defaults false (CLI --plaintext parity; the desktop
        // always sends encrypted — the field exists for tests and tooling).
        sendFn({ to: frame.to, body: frame.body, plaintext: frame.plaintext === true })
          .then((r) => {
            if (socket.destroyed) return;
            socket.write(encodeFrame({ type: "send-ok", id: frame.id, envelope_id: r.envelope_id, encrypted: r.encrypted ?? true }));
          })
          .catch((err) => {
            if (socket.destroyed) return;
            const verdict = classifySendError(err);
            const reason = clipReason(verdict.reason);
            log(`[daemon] send failed (${verdict.retryable ? "retryable" : "terminal"}): ${reason}`);
            socket.write(encodeFrame({ type: "send-err", id: frame.id, retryable: verdict.retryable, reason }));
          });
        return;
      }
```

In `src/daemon.mjs`:
- Extend the core import: `import { receiveAll, send as coreSend } from "./core.mjs";` (check the
  current import line first — it may already import more; extend, never duplicate).
- In `startDaemon`'s `createIpcServer({...})` call, add after `statusExtraFn`:
```js
    sendFn: ({ to, body, plaintext }) => coreSend({ to, body, plaintext }),
```
**Executor self-check (critic M3):** the existing `ping`/`status` branches fall through without
`return`; your `send` branch early-returns. Confirm after editing that the trailing
"duplicate hello and unknown frames ignored" comment path is still reachable for unknown types
(it is, as long as the send branch only returns for `frame.type === "send"`).

- [ ] **Step 4: Verify the send result shape.** `core.send` must RETURN `{envelope_id, encrypted}`
for the ack. Check its return statement (core.mjs after the archive block): if it returns a
different shape (e.g. the relay receipt or `{envelope, receipt}`), ADAPT THE WIRING in daemon.mjs
(`sendFn: async ({to, body}) => { const r = await coreSend({to, body}); return { envelope_id: r.<actual-id-field>, encrypted: r.<actual-enc-field> }; }`)
— do NOT change `core.send`'s return shape (the CLI and MCP server consume it). State what you
found in the commit message.

- [ ] **Step 5: Run to verify pass**

Run: `node --test test/daemon-ipc.test.mjs test/daemon.test.mjs`
Expected: ALL PASS.

- [ ] **Step 6: Commit**

```bash
git add src/daemon-ipc.mjs src/daemon.mjs test/daemon-ipc.test.mjs
git commit -m "feat(daemon): send-over-socket — role-agnostic request op, classified acks, daemon stays sole key-holder"
```

---

### Task 4: PROTOCOL.md + cross-language frame fixtures

**Files:**
- Create: `docs/PROTOCOL.md` (in agent-bridge-mcp/docs/ — the messaging stack's own docs dir)
- Create: `test/fixtures/socket-frames.json`
- Test: `test/protocol-fixtures.test.mjs` (new)

- [ ] **Step 1: Create the fixtures file** (`test/fixtures/socket-frames.json`) — one canonical
instance of every frame type, both directions. This file is THE cross-language contract: Phase
A2's Rust client asserts against the same file (workspace-relative read), the Ed25519
interop-vector precedent applied to the socket layer.

```json
{
  "version": 1,
  "comment": "Canonical socket frames. JS asserts the daemon emits/accepts these; Rust (Phase A2) asserts its client builds/parses these. One JSON object per line on the wire, newline-terminated, 1 MiB line ceiling.",
  "client_to_daemon": {
    "hello": { "type": "hello", "role": "viewer" },
    "hello_channel_resume": { "type": "hello", "role": "channel", "since_seq": 41 },
    "ping": { "type": "ping" },
    "status_request": { "type": "status" },
    "send": { "type": "send", "id": "11111111-2222-4333-8444-555555555555", "to": "did:wba:agentidentityregistry.org:agents:AIR-TEST-TEST-TEST", "body": { "type": "text", "text": "hello from a GUI" } },
    "send_plaintext": { "type": "send", "id": "22222222-3333-4444-8555-666666666666", "to": "did:wba:agentidentityregistry.org:agents:AIR-TEST-TEST-TEST", "body": { "type": "text", "text": "deliberately clear" }, "plaintext": true }
  },
  "daemon_to_client": {
    "hello_ok": { "type": "hello-ok", "pid": 4242, "start_time": "2026-06-11T00:00:00.000Z", "did": "did:wba:agentidentityregistry.org:agents:AIR-TEST-TEST-TEST" },
    "message": { "type": "message", "message": { "seq": 7, "relay_seq": 7, "envelope_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee", "from": "did:wba:agentidentityregistry.org:agents:AIR-PEER-PEER-PEER", "contact": "pat", "verified": true, "encrypted": true, "received_at": "2026-06-11T00:00:01.000Z", "thread_id": "tttttttt-uuuu-4vvv-8www-xxxxxxxxxxxx", "body": { "type": "text", "text": "hi" } } },
    "comment_message_optional_fields": "message.message: contact / key_changed / thread_id / body presence is CONDITIONAL — the daemon OMITS falsy/unset fields (a real key_changed:false frame carries NO key_changed key; an unpinned sender carries NO contact). The fixture shows a pinned, verified, key-unchanged sender. Parsers MUST treat these as optional.",
    "gap": { "type": "gap", "after_seq": 41 },
    "pong": { "type": "pong" },
    "status": { "type": "status", "socket": "/home/user/.air-msg/daemon.sock", "last_seq": 7, "clients": [{ "role": "viewer", "lastSeq": 7, "dropped": 0 }], "sinks": ["banner", "socket"] },
    "send_ok": { "type": "send-ok", "id": "11111111-2222-4333-8444-555555555555", "envelope_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee", "encrypted": true },
    "send_err": { "type": "send-err", "id": "11111111-2222-4333-8444-555555555555", "retryable": true, "reason": "relay 503: down" },
    "error": { "type": "error", "reason": "first frame must be hello with role channel|viewer" }
  }
}
```

- [ ] **Step 2: Write the fixture test** (create `test/protocol-fixtures.test.mjs`):

```js
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
```

- [ ] **Step 3: Run** `node --test test/protocol-fixtures.test.mjs` — Expected: PASS (Tasks 1–3
landed; if a shape mismatches, fix the FIXTURE to match reality and note it — the daemon is the
source of truth, the fixtures are its photograph).

- [ ] **Step 4: Write `docs/PROTOCOL.md`** — the frame catalog. Content requirements (write real
prose, no placeholders): framing rules (one JSON object per newline-terminated line; 1 MiB line
ceiling; malformed/oversized line → `{type:"error"}` + disconnect); the handshake (first frame
MUST be hello with `role: "viewer"|"channel"`; optional `since_seq` — channel-only resume, the
daemon answers with an immediate `gap`); role semantics table (viewer = mute-filtered everything;
channel = verified+pinned+key-unchanged / room-gated; roles are DELIVERY filters — `send` is
role-agnostic); each frame type with a field table and direction, mirroring
`test/fixtures/socket-frames.json` (state that the fixtures file is normative for shapes); flow
control summary (per-subscriber skip above HWM, gap-on-progress for channel, 4×HWM destroy
backstop; reconnect with since_seq is the recovery); the send op contract incl. the optional
`plaintext` boolean (default false; CLI `--plaintext` parity), the retryable/terminal taxonomy,
the no-id-no-ack rule, the `reason` cap (≤512 chars, control characters stripped at the daemon —
relay-controlled text is length-unbounded; rendering-side escaping stays the GUI's job), and an
explicit statement that the ack is INTENTIONALLY MINIMAL
(`id, envelope_id, encrypted` — no thread_id/relay_seq; the archive is the source of truth for
sent-row detail); a "Message fields" note that `contact`/`key_changed`/`thread_id`/`body` are
OPTIONAL — omitted when falsy/unset (parsers must not require them); a "Versioning" section
(unknown frame types are ignored by both sides — additive evolution; the fixtures file carries
`version`). Cross-link the daemon design spec §5/§6 and the AI-inbox design §3/§5.

- [ ] **Step 5: Commit**

```bash
git add docs/PROTOCOL.md test/fixtures/socket-frames.json test/protocol-fixtures.test.mjs
git commit -m "docs(daemon): PROTOCOL.md + cross-language socket-frame fixtures — the A2 Rust client's contract"
```

---

### Task 5: E2E — a real daemon, a local stub relay, a send round-trip

**Files:**
- Test: `test/daemon-send-e2e.test.mjs` (new)

- [ ] **Step 1: Write the test** (new file; this is the whole-path proof: socket frame →
core.send → seal/sign → HTTP POST → archive row → ack):

```js
// E2E for send-over-socket (AI-inbox design §3): a REAL daemon process is spawned on a temp home
// whose identity points at a LOCAL stub relay; a raw socket client sends; we assert the ack, the
// relay-side POST, and the archived sent row. Stub-down proves the retryable path. Hermetic: no
// real network, no real home.
import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync, chmodSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { createServer as createHttpServer } from "node:http";
import { createConnection } from "node:net";
import { encodeFrame, makeLineParser } from "../src/daemon-ipc.mjs";
import { DatabaseSync } from "node:sqlite";

const CLI = fileURLToPath(new URL("../src/cli.mjs", import.meta.url));
let dir, relay, relayPosts, relayPort, daemon;

const until = async (cond, ms = 8000) => {
  const t0 = Date.now();
  while (!cond()) {
    if (Date.now() - t0 > ms) throw new Error("until: timed out");
    await new Promise((r) => setTimeout(r, 25));
  }
};

beforeEach(async () => {
  dir = mkdtempSync(join(tmpdir(), "air-msg-e2e-"));
  chmodSync(dir, 0o700);
  process.env.AGENT_BRIDGE_HOME = dir;
  relayPosts = [];
  relay = createHttpServer((req, res) => {
    if (req.method === "POST" && req.url.startsWith("/inbox/")) {
      let buf = "";
      req.on("data", (c) => { buf += c; });
      req.on("end", () => {
        const envelope = JSON.parse(buf);
        relayPosts.push({ url: req.url, envelope });
        res.writeHead(200, { "content-type": "application/json" });
        // The receipt MUST echo envelope_id (critic C1, probed): core.send returns
        // receipt.envelope_id — the relay's word — while the archive row stores envelope.id.
        // The real relay echoes it (archive-integration.test.mjs pins the same shape).
        res.end(JSON.stringify({ envelope_id: envelope.id, seq: relayPosts.length }));
      });
      return;
    }
    res.writeHead(404); res.end();                       // pull/SSE → 404; the daemon's watch loop backs off (designed degraded mode)
  });
  await new Promise((r) => relay.listen(0, "127.0.0.1", r));
  relayPort = relay.address().port;
  // REAL identity shape; relay_url points at the stub. seed_hex is load-bearing (loadIdentity
  // re-derives the keypair); air_url is .invalid — the PLAINTEXT send path never resolves keys.
  writeFileSync(join(dir, "identity.json"), JSON.stringify({
    version: 1, name: "e2e", air_id: "AIR-TEST-TEST-TEST",
    did: "did:wba:agentidentityregistry.org:agents:AIR-TEST-TEST-TEST",
    seed_hex: "00".repeat(32), public_key_base64url: "", public_key_multibase: "",
    relay_url: `http://127.0.0.1:${relayPort}`, air_url: "https://air.invalid", agent_secret: "e2e",
  }), { mode: 0o600 });
  daemon = spawn(process.execPath, [CLI, "daemon", "start"], {
    env: { ...process.env, AGENT_BRIDGE_HOME: dir, NO_COLOR: "1" },
    stdio: ["ignore", "pipe", "pipe"],
  });
  // Drain both pipes (critic hardening): the daemon's watch loop logs against the 404-ing stub;
  // an undrained pipe buffer is a latent wedge if logging ever gets chattier.
  daemon.stdout.on("data", () => {});
  daemon.stderr.on("data", () => {});
});

afterEach(async () => {
  daemon.kill("SIGTERM");
  await new Promise((r) => { (daemon.exitCode !== null) ? r() : daemon.once("exit", r); });
  await new Promise((r) => relay.close(r));
  rmSync(dir, { recursive: true, force: true });
});

const connectAndHello = async () => {
  // Wait for the spawned daemon to bind (identity load + lock + listen — typically <300 ms).
  await until(() => { try { return statSync(join(dir, "daemon.sock")).isSocket(); } catch { return false; } });
  const sock = createConnection(join(dir, "daemon.sock"));
  const frames = [];
  sock.on("data", makeLineParser((f) => frames.push(f), { onError: () => {} }));
  await new Promise((res, rej) => { sock.once("connect", res); sock.once("error", rej); });
  sock.write(encodeFrame({ type: "hello", role: "viewer" }));
  await until(() => frames.some((f) => f.type === "hello-ok"));
  return { sock, frames };
};

test("send round-trip: socket frame → daemon → stub relay POST → archived sent row → send-ok", async () => {
  const { sock, frames } = await connectAndHello();
  // plaintext (designed into the op in Task 3 — critic H1): air_url is .invalid, so the
  // encrypted path cannot resolve keys; the PLAINTEXT path exercises the same
  // wire/archive/ack machinery without key resolution. The desktop always sends encrypted.
  sock.write(encodeFrame({ type: "send", id: "e2e-1", to: "did:wba:agentidentityregistry.org:agents:AIR-PEER-PEER-PEER", body: { type: "text", text: "e2e send" }, plaintext: true }));
  await until(() => frames.some((f) => f.type === "send-ok" || f.type === "send-err"));
  const ack = frames.find((f) => f.type === "send-ok" || f.type === "send-err");
  assert.equal(ack.type, "send-ok", `expected send-ok, got: ${JSON.stringify(ack)}`);
  assert.equal(ack.id, "e2e-1");
  assert.equal(typeof ack.envelope_id === "string" && ack.envelope_id.length > 0, true,
    "send-ok.envelope_id must be a non-empty string (the relay receipt's word — critic C1)");
  assert.equal(ack.encrypted, false, "plaintext send must ack encrypted:false");
  assert.equal(relayPosts.length, 1);
  assert.match(relayPosts[0].url, /AIR-PEER-PEER-PEER/);
  const db = new DatabaseSync(join(dir, "archive.db"), { readOnly: true });
  try {
    const row = db.prepare("SELECT direction, envelope_id FROM messages WHERE direction='sent'").get();
    assert.ok(row, "sent row must be archived by the daemon");
    assert.equal(row.envelope_id, ack.envelope_id);
  } finally { db.close(); }
  sock.destroy();
});

test("send with the relay down: send-err retryable:true", async () => {
  await new Promise((r) => relay.close(r));               // kill the stub BEFORE sending
  const { sock, frames } = await connectAndHello();
  sock.write(encodeFrame({ type: "send", id: "e2e-2", to: "did:wba:agentidentityregistry.org:agents:AIR-PEER-PEER-PEER", body: { type: "text", text: "x" }, plaintext: true }));
  await until(() => frames.some((f) => f.type === "send-err"));
  const err = frames.find((f) => f.type === "send-err");
  assert.equal(err.id, "e2e-2");
  assert.equal(err.retryable, true, `ECONNREFUSED must be retryable, got: ${JSON.stringify(err)}`);
  sock.destroy();
});
```

- [ ] **Step 2: Run** `node --test test/daemon-send-e2e.test.mjs` — Expected: PASS (plaintext
forwarding landed in Task 3; this E2E consumes it).

- [ ] **Step 3: Commit**

```bash
git add test/daemon-send-e2e.test.mjs
git commit -m "test(daemon): send-over-socket E2E — real daemon, stub relay, archived row, retryable taxonomy"
```

---

### Task 6: Spec notes + full verification + PR

**Files:**
- Modify: `docs/superpowers/specs/2026-06-05-receiver-daemon-design.md` (§6 + §8 one-liners)

- [ ] **Step 1: Spec notes.** (a) In §6, append:
```markdown
- **Phase A1 (2026-06-11, AI-inbox):** the archive runs in WAL mode with a 5 s busy timeout —
  cross-process READ-ONLY consumers (the desktop's archive reader, gap replay) proceed
  concurrently with the daemon writer; WAL is set by the writer in `openArchive()`.
```
(b) In §8, append:
```markdown
Phase A1 adds the socket's first REQUEST op: `{type:"send", id, to, body}` → `core.send` →
`{type:"send-ok"|"send-err"}` with a retryable/terminal taxonomy (`classifySendError`). Roles
remain delivery filters; send is role-agnostic (the 0600 socket is the OS user boundary). The
wire contract lives in `docs/PROTOCOL.md` + `test/fixtures/socket-frames.json` (normative,
cross-language).
```

- [ ] **Step 2: Verify the PRODUCTION relay receipt carries `envelope_id`** (critic open
question — the send-ok contract leans on it). Read-only check of the relay source:
`grep -n "envelope_id" ~/air-site/relay/src/index.js` — expect the `/inbox` POST handler's
response to include it. If it does NOT, STOP and report: that would be a product gap (send-ok
would carry undefined in production), fixed relay-side, not papered over here. State the finding
in the PR body either way.

- [ ] **Step 3: Full suite + hermeticity**

```bash
cd ~/air-note/agent-bridge-mcp
before=$(sqlite3 -readonly ~/.air-msg/archive.db "SELECT COUNT(*) FROM messages")
node --test 2>&1 | grep -E "^ℹ (tests|pass|fail|todo)"
after=$(sqlite3 -readonly ~/.air-msg/archive.db "SELECT COUNT(*) FROM messages")
echo "real-archive delta: $((after-before)) (must be 0)"
```
Expected: `fail 0`, delta 0. (Baseline at branch: 305 tests / 302 pass / 3 todo.)
**Heads-up:** the REAL `~/.air-msg/archive.db` belongs to the LIVE installed daemon — these
commands are read-only; never open it writable from tests or shells.

- [ ] **Step 4: Commit + push + PR**

```bash
git add agent-bridge-mcp/docs/superpowers/specs/2026-06-05-receiver-daemon-design.md
git commit -m "docs(daemon): spec §6/§8 — Phase A1 notes (archive WAL, send op, protocol contract)"
git push -u origin feat/ai-inbox-a1-stack
gh pr create --repo AgentIdentityRegistry/air-note --base main \
  --title "feat(daemon): AI-inbox Phase A1 — send-over-socket + archive WAL + PROTOCOL.md/fixtures"
```
PR body: what A1 ships and why (the desktop design's two stack prerequisites + the cross-language
contract); the live-daemon caveat (the user's REAL machine runs the installed daemon — merging
changes `openArchive` to WAL, which the live daemon adopts on its next restart; launchd KeepAlive
makes that automatic on the next relaunch, no manual step); verification boundary (send E2E runs
against a LOCAL stub relay — the real-relay path is exercised by the production daemon daily);
the messaging-tests CI job must be green (it runs this suite on Linux).

---

## Self-Review (against the AI-inbox design §2/§3/§5/§9 + the A-phase review bar)

- **Design §2 "exactly two messaging-stack changes":** WAL (T1) + send op (T3); the protocol
  artifacts (T4) are contract documentation, not behavior. ✓
- **Design §3 send contract:** correlation id, role-agnostic post-hello, `retryable` in send-err
  (T2 classifier + T3), no-id-no-ack, socket.destroyed guard, async non-blocking ack. ✓
- **Design §5 WAL:** writer-side PRAGMA + busy_timeout; conversion from legacy mode tested;
  read-only-reader property pinned in-process (cross-process soak is A2's Rust-side test). ✓
- **Design §5 protocol parity:** PROTOCOL.md + normative fixtures + JS round-trip/response tests
  (T4); A2 consumes the same file. ✓
- **Design §9 testing:** send op unit (T3), fixtures (T4), E2E with stub relay + archived-row +
  retryable-path (T5), hermeticity gate (T6). ✓
- **Honest verify-at-execution items, each with a stated fallback:** node:sqlite PRAGMA API
  (.get vs .exec); core.send return shape (adapt the wiring, never the core); plaintext
  forwarding (extend op + fixtures + tests if the E2E trips it); circular-import check before
  importing core into daemon-ipc. ✓
- **Placeholder scan:** every step has complete code or an exact content specification (PROTOCOL.md
  prose requirements are enumerated, not "write docs"). ✓
- **Type consistency:** `sendFn({to, body})` → `{envelope_id, encrypted}` and
  `classifySendError(err)` → `{retryable, reason}` used identically in T2/T3/T4/T5. ✓
- **Live-machine awareness:** the real installed daemon picks up WAL on next relaunch; stated in
  the PR body; tests never touch the real home. ✓
- **v2 deltas (critic v1 REWORK, all applied):** C1 stub relay echoes `envelope_id` (probed:
  `send-ok.envelope_id` is the RECEIPT's word, archive stores the local uuid — the v1 stub broke
  both the E2E and the normative contract); H1 `plaintext` designed into the op (frame field +
  forwarding + fixture + PROTOCOL.md + unit test — no mid-E2E pivot); H2 the `message` fixture is
  now a real `deliver()` photograph (conditional fields omitted-when-falsy; live deepEqual test);
  H3 busy_timeout read key-agnostically; M1-M3 (branch comment, import fold, return-placement
  self-check); + send-racing-shutdown test, `encrypted:false` ack assertion, E2E pipe drains,
  production-relay receipt verification step. ✓
