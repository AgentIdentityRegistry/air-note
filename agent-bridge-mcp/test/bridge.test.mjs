import { test } from "node:test";
import assert from "node:assert/strict";
import { badgeFor, bridgeFormat, replyTier } from "../src/bridge.mjs";

const msg = (over = {}) => ({
  from: "did:wba:x:agents:AIR-ALICE", contact: "alice", verified: true,
  body: { type: "text", text: "hi there" }, thread_id: "t1", envelope_id: "e1", ...over,
});

test("badgeFor: verified+unchanged → verified; else UNVERIFIED", () => {
  assert.equal(badgeFor(msg()), "✓ verified");
  assert.equal(badgeFor(msg({ verified: false })), "⚠️ UNVERIFIED");
  assert.equal(badgeFor(msg({ key_changed: true })), "⚠️ UNVERIFIED");
});

test("badgeFor: a spoofed display name CANNOT forge the badge (it's crypto-only)", () => {
  assert.equal(badgeFor(msg({ verified: false, contact: "Alice ✓ verified" })), "⚠️ UNVERIFIED");
});

test("bridgeFormat full: badge is a sender-unreachable PREFIX + the body text", () => {
  const p = bridgeFormat(msg({ body: { type: "text", text: "ping" } }));
  assert.ok(p.title.startsWith("✓ verified"));
  assert.ok(p.title.includes("alice"));
  assert.equal(p.body, "ping");
});

test("bridgeFormat meta: body text is withheld", () => {
  const p = bridgeFormat(msg(), { bodyMode: "meta" });
  assert.equal(p.body, "(open AIR Note to read)");
});

test("bridgeFormat: markup in the body is passed through verbatim (caller sends plain text)", () => {
  const p = bridgeFormat(msg({ body: { type: "text", text: "*bold* [x](http://evil)" } }));
  assert.equal(p.body, "*bold* [x](http://evil)");
});

test("bridgeFormat: non-text + empty + absent bodies show markers, never 'undefined'", () => {
  assert.equal(bridgeFormat(msg({ body: { type: "image" } })).body, "(non-text message)");
  assert.equal(bridgeFormat(msg({ body: { type: "unavailable" } })).body, "(could not decrypt)");
  assert.equal(bridgeFormat(msg({ body: { type: "text" } })).body, "(empty message)");
  assert.equal(bridgeFormat(msg({ body: undefined })).body, "(no content)");
});

test("bridgeFormat: no pinned alias → short AIR-id as the title name", () => {
  assert.ok(bridgeFormat(msg({ contact: undefined })).title.includes("AIR-ALICE"));
});

test("replyTier: verified route → one-tap; unverified → confirm", () => {
  assert.equal(replyTier({ verified: true }), "one-tap");
  assert.equal(replyTier({ verified: false }), "confirm");
  assert.equal(replyTier(null), "confirm");
});
