import { test } from "node:test";
import assert from "node:assert/strict";
import { shortPeer, parseMuteSet } from "../src/peers.mjs";

test("shortPeer: extracts the AIR-id from a full DID", () => {
  assert.equal(shortPeer("did:wba:agentidentityregistry.org:agents:AIR-2JE0-EM7W-JNBK"), "AIR-2JE0-EM7W-JNBK");
});
test("shortPeer: passes through a value with no AIR-id", () => {
  assert.equal(shortPeer("did:web:example.com"), "did:web:example.com");
});
test("shortPeer: coerces a non-string to string", () => {
  assert.equal(shortPeer(12345), "12345");
});

test("parseMuteSet: splits, trims, and drops empty entries", () => {
  assert.deepEqual(parseMuteSet("a, b ,,c"), new Set(["a", "b", "c"]));
});
test("parseMuteSet: empty string → empty Set", () => {
  assert.deepEqual(parseMuteSet(""), new Set());
});
test("parseMuteSet: defaults to AIRMSG_MUTE env when no arg", () => {
  const saved = process.env.AIRMSG_MUTE;
  try {
    process.env.AIRMSG_MUTE = "kenny, alice";
    assert.deepEqual(parseMuteSet(), new Set(["kenny", "alice"]));
    delete process.env.AIRMSG_MUTE;
    assert.deepEqual(parseMuteSet(), new Set());      // unset env → empty Set
  } finally {
    if (saved === undefined) delete process.env.AIRMSG_MUTE;
    else process.env.AIRMSG_MUTE = saved;
  }
});
