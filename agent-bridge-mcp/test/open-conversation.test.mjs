import { test } from "node:test";
import assert from "node:assert/strict";
import {
  shq, sanitizeId, wrapInTerminal, resolveOpenCommand,
} from "../src/open-conversation.mjs";

test("shq single-quotes and escapes embedded quotes", () => {
  assert.equal(shq("ab"), "'ab'");
  assert.equal(shq("a'b"), "'a'\\''b'");
});

test("sanitizeId strips shell metacharacters, keeps DID chars", () => {
  assert.equal(sanitizeId("did:wba:x:agents:AIR-1A2B-3C4D"), "did:wba:x:agents:AIR-1A2B-3C4D");
  assert.equal(sanitizeId('x"; rm -rf ~ #'), "xrm-rf"); // spaces+metachars removed
  assert.equal(sanitizeId("kenny"), "kenny");
});

test("wrapInTerminal (darwin) builds an osascript argv that opens Terminal", () => {
  const argv = wrapInTerminal("air-msg history --with kenny", "darwin");
  assert.equal(argv[0], "osascript");
  assert.ok(argv.includes('tell application "Terminal" to do script "air-msg history --with kenny"'));
  assert.ok(argv.includes('tell application "Terminal" to activate'));
});

test("wrapInTerminal (win32 / linux) builds the right shell argv", () => {
  assert.deepEqual(wrapInTerminal("X", "win32"), ["cmd", "/c", "start", "cmd", "/k", "X"]);
  assert.deepEqual(
    wrapInTerminal("X", "linux"),
    ["x-terminal-emulator", "-e", "bash", "-lc", "X; exec bash"],
  );
});

test("resolveOpenCommand: terminal-history (default) opens history for the peer", () => {
  const argv = resolveOpenCommand("AIR-PEER", { mode: "terminal-history", platform: "darwin", airMsgBin: "air-msg" });
  assert.equal(argv[0], "osascript");
  assert.ok(argv.some((a) => a.includes("air-msg history --with AIR-PEER")));
});

test("resolveOpenCommand: ai mode uses the chosen AI CLI, agnostic", () => {
  const claude = resolveOpenCommand("AIR-PEER", { mode: "ai", aiCmd: "claude", platform: "darwin" });
  assert.ok(claude.some((a) => a.includes("claude '")));
  const gemini = resolveOpenCommand("AIR-PEER", { mode: "ai", aiCmd: "gemini", platform: "darwin" });
  assert.ok(gemini.some((a) => a.includes("gemini -i '")));
  const other = resolveOpenCommand("AIR-PEER", { mode: "ai", aiCmd: "myagent", platform: "darwin" });
  assert.ok(other.some((a) => a.includes("myagent '"))); // falls back to `<bin> '<prompt>'`
});

test("resolveOpenCommand: ai mode with no aiCmd defaults to claude", () => {
  const argv = resolveOpenCommand("AIR-PEER", { mode: "ai", platform: "darwin" });
  assert.ok(argv.some((a) => a.includes("claude '")));
});

test("resolveOpenCommand: command:<template> expands sanitized placeholders via sh -c", () => {
  const argv = resolveOpenCommand("AIR-PEER", {
    mode: "command:open air-msg://chat/{peer}", platform: "darwin",
  });
  assert.deepEqual(argv, ["sh", "-c", "open air-msg://chat/AIR-PEER"]);
});

test("resolveOpenCommand: a malicious peer cannot inject (sanitized before use)", () => {
  const argv = resolveOpenCommand('AIR"; rm -rf ~ #', {
    mode: "command:echo {peer}", platform: "darwin",
  });
  assert.deepEqual(argv, ["sh", "-c", "echo AIRrm-rf"]); // shell metacharacters + spaces stripped
});

test("resolveOpenCommand: none → null", () => {
  assert.equal(resolveOpenCommand("AIR-PEER", { mode: "none" }), null);
});
