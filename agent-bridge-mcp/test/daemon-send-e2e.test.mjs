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
