// §7 decision-table rows, exercised through the REAL CLI (cli-args lesson: parser-level bugs
// are invisible to core unit tests). Each spawn gets a temp home with a PRE-SEEDED identity —
// VERIFY FIRST that ensureIdentity() is network-silent when identity.json exists; if it is not,
// delete this file and extend the unit coverage instead, saying so in the commit message.
// VERIFIED (empirically): ensureIdentity() calls loadIdentity() (identity.mjs:60-67) which
// re-derives the keypair via generateIdentity(stored.seed_hex) — no network call when file exists.
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
  // REAL identity.json shape (identity.mjs:133). seed_hex is the LOAD-BEARING field: loadIdentity()
  // re-derives the keypair via generateIdentity(stored.seed_hex), and without it a fresh unrelated
  // key is silently generated instead of failing loudly (critic v1 H2). ensureIdentity() makes no
  // network call on this path (verified: identity.mjs:60-67). Public-key fields are re-derived
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
    await new Promise((r) => child.once("exit", r));
    await ipc.close();
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
      new Promise((r) => child.once("exit", r)),
      new Promise((_, reject) => setTimeout(() => reject(new Error("bridge did not exit within 3 s — probeDaemon not yet wired (Task 6 RED)")), 3000)),
    ]);
    assert.equal(code, 1);
    assert.match(out.stderr, /daemon owns the message pull/);
  } finally {
    child.kill("SIGKILL");
    await new Promise((r) => child.once("exit", r));
    await ipc.close();
  }
});
