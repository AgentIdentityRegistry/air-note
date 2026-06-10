import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { resolveRecipient, didFromAirId, cursorAdvanceTarget } from "../src/core.mjs";

// resolveRecipient's alias branch reads the contacts store; point it at an empty temp
// home so an unknown alias has no matching contact and falls through unchanged.
let dir;
beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "air-msg-core-"));
  process.env.AGENT_BRIDGE_HOME = dir;
});
afterEach(() => {
  rmSync(dir, { recursive: true, force: true });
  delete process.env.AGENT_BRIDGE_HOME;
});

test("didFromAirId builds the canonical did:wba DID", () => {
  assert.equal(didFromAirId("AIR-1A2B-3C4D"), "did:wba:agentidentityregistry.org:agents:AIR-1A2B-3C4D");
});

test("resolveRecipient normalizes a bare AIR-id to the full DID (so it reaches the right relay queue)", () => {
  assert.equal(resolveRecipient("AIR-1A2B-3C4D"), "did:wba:agentidentityregistry.org:agents:AIR-1A2B-3C4D");
});

test("resolveRecipient leaves a full DID unchanged", () => {
  const did = "did:wba:agentidentityregistry.org:agents:AIR-1A2B-3C4D";
  assert.equal(resolveRecipient(did), did);
});

test("resolveRecipient leaves an unknown alias unchanged (no matching contact)", () => {
  assert.equal(resolveRecipient("kenny"), "kenny");
});

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
