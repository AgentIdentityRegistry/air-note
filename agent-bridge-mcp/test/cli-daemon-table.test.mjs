// §7 decision-table rows, exercised through the REAL CLI (cli-args lesson: parser-level bugs
// are invisible to core unit tests). Each spawn gets a temp home with a PRE-SEEDED identity —
// VERIFY FIRST that ensureIdentity() is network-silent when identity.json exists; if it is not,
// delete this file and extend the unit coverage instead, saying so in the commit message.
// VERIFIED (empirically): ensureIdentity()/loadIdentity() re-derive the keypair from seed_hex —
// no network call when identity.json exists (loadIdentity returns immediately if the file is present).
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
  // (the unref'd backoff timer cannot pin the event loop). This test would fail if keepAlive
  // is removed — verified by probing that path (exitCode === 0 after 600ms without keepAlive).
  const ipc = createIpcServer({ daemonInfo: { pid: 4242, start_time: "t", did: "did:wba:me" }, log: () => {} });
  await ipc.listen();
  const { child, out } = runCli(["watch"]);
  try {
    await until(() => out.stdout.includes("attached to air-msgd"));
    await until(() => ipc.clientCount() === 1);
    // Kill the first daemon — triggers onDetach + backoff reconnect loop.
    await ipc.close();
    // Wait ~600ms while reconnecting — process must still be alive.
    await new Promise((r) => setTimeout(r, 600));
    assert.equal(child.exitCode, null, "watch viewer must stay alive during daemon outage (C1: ref'd keepAlive required)");
    // Bring up a new daemon on the same home — wrapper reconnects automatically.
    const ipc2 = createIpcServer({ daemonInfo: { pid: 4243, start_time: "t2", did: "did:wba:me" }, log: () => {} });
    await ipc2.listen();
    try {
      // Wait for the second attach line to appear in stdout.
      await until(() => out.stdout.indexOf("attached to air-msgd", out.stdout.indexOf("attached to air-msgd") + 1) !== -1, 8000);
      await until(() => ipc2.clientCount() === 1, 8000);
      await ipc2.sink.deliver({ envelope_id: "w2", seq: 4, from: "did:wba:x", verified: true, encrypted: false, body: { type: "text", text: "post-restart-mail" } });
      await until(() => out.stdout.includes("post-restart-mail"), 5000);
    } finally {
      await ipc2.close();
    }
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
    // acquireOrExit — the timeout kills it so this test fails cleanly rather than timing out
    // the whole runner). Task 6's RED: probeDaemon not yet wired → child won't self-exit.
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
