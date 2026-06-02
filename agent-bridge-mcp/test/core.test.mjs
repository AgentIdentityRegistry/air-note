import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { resolveRecipient, didFromAirId } from "../src/core.mjs";

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
