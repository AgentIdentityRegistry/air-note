import { test } from "node:test";
import assert from "node:assert/strict";
import { shortPeer } from "../src/peers.mjs";

test("shortPeer: extracts the AIR-id from a full DID", () => {
  assert.equal(shortPeer("did:wba:agentidentityregistry.org:agents:AIR-2JE0-EM7W-JNBK"), "AIR-2JE0-EM7W-JNBK");
});
test("shortPeer: passes through a value with no AIR-id", () => {
  assert.equal(shortPeer("did:web:example.com"), "did:web:example.com");
});
test("shortPeer: coerces a non-string to string", () => {
  assert.equal(shortPeer(12345), "12345");
});
