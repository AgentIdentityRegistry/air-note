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

import { makeBridgeOutbound, makeConfirmStore, makeReplyHandler } from "../src/bridge.mjs";

/** A fake adapter that records sends + hands out a reply() that records acks. */
function fakeAdapter() {
  const sent = []; const acks = [];
  return {
    name: "telegram", acks, sent,
    async send(ping) { sent.push(ping); return String(sent.length); }, // externalId = "1","2",...
    reply: (t) => { acks.push(t); return Promise.resolve(); },
  };
}

test("outbound: a message is sent + a route stored keyed by the returned external id", async () => {
  const adapter = fakeAdapter();
  const routes = [];
  const hook = makeBridgeOutbound({ adapter, now: () => 123, putRouteFn: (r) => routes.push(r) });
  hook({ from: "did:wba:x:agents:AIR-ALICE", contact: "alice", verified: true,
    thread_id: "t1", envelope_id: "e1", body: { type: "text", text: "hi" } });
  await new Promise((r) => setTimeout(r, 0));
  assert.equal(adapter.sent.length, 1);
  assert.equal(routes.length, 1);
  assert.equal(routes[0].external_id, "1");
  assert.equal(routes[0].peer_did, "did:wba:x:agents:AIR-ALICE");
  assert.equal(routes[0].thread_id, "t1");
  assert.equal(routes[0].verified, true);
  assert.equal(routes[0].created_at, 123);
});

test("outbound: a verified but UNPINNED sender (no contact) is stored as NOT one-tap (needs /yes)", async () => {
  const adapter = fakeAdapter();
  const routes = [];
  makeBridgeOutbound({ adapter, putRouteFn: (r) => routes.push(r) })(
    { from: "did:wba:x:agents:AIR-STRANGER", verified: true, body: { type: "text", text: "hi" } });
  await new Promise((r) => setTimeout(r, 0));
  assert.equal(routes[0].verified, false); // signature-verified but not pinned → confirm tier
});

test("outbound: a forged/UNVERIFIED message (verified:false) is stored NOT one-tap — even if a contact alias is present", async () => {
  const adapter = fakeAdapter();
  const routes = [];
  // A forged `from` lands verified:false at receive(); even a (spoofed) contact alias must not promote it to one-tap.
  makeBridgeOutbound({ adapter, putRouteFn: (r) => routes.push(r) })(
    { from: "did:wba:x:agents:AIR-FORGED", contact: "alice", verified: false, body: { type: "text", text: "hi" } });
  await new Promise((r) => setTimeout(r, 0));
  assert.equal(routes[0].verified, false); // unverified ⇒ confirm tier, never auto-routed
});

test("outbound: a failed send (null id) stores no route", async () => {
  const adapter = { name: "telegram", async send() { return null; } };
  const routes = [];
  // contact present, but the null send-id short-circuits before putRoute → no route regardless
  makeBridgeOutbound({ adapter, putRouteFn: (r) => routes.push(r) })(
    { from: "did:x:AIR-A", contact: "a", verified: true, body: { type: "text", text: "x" } });
  await new Promise((r) => setTimeout(r, 0));
  assert.equal(routes.length, 0);
});

test("outbound: a throwing send never throws out of the hook", async () => {
  const adapter = { name: "telegram", async send() { throw new Error("boom"); } };
  const logs = [];
  let threw = false;
  try {
    makeBridgeOutbound({ adapter, log: (s) => logs.push(s) })(
      { from: "did:x:AIR-A", contact: "a", verified: true, body: { type: "text", text: "x" } });
  } catch { threw = true; }
  assert.equal(threw, false);
  await new Promise((r) => setTimeout(r, 0));
  assert.ok(logs.some((l) => l.includes("outbound failed")));
});

const verifiedRoute = { peer_did: "did:wba:x:agents:AIR-ALICE", contact: "alice",
  thread_id: "t1", envelope_id: "e1", verified: true };
const unverifiedRoute = { peer_did: "did:wba:x:agents:AIR-BOB", contact: null,
  thread_id: "t2", envelope_id: "e2", verified: false };

test("reply (verified, one-tap): core.send is called with thread continuity + acked", async () => {
  const sends = []; const acks = [];
  const h = makeReplyHandler({ sendFn: async (a) => sends.push(a),
    getRouteFn: () => verifiedRoute, confirm: makeConfirmStore() });
  await h({ replyToExternalId: "1", text: "on my way", reply: (t) => { acks.push(t); } });
  assert.equal(sends.length, 1);
  assert.deepEqual(sends[0], { to: "did:wba:x:agents:AIR-ALICE", body: "on my way", thread_id: "t1", in_reply_to: "e1" });
  assert.ok(acks[0].includes("sent to alice"));
});

test("reply (no reply-to): asks the user to reply to a specific message; no send", async () => {
  const sends = []; const acks = [];
  const h = makeReplyHandler({ sendFn: async (a) => sends.push(a), getRouteFn: () => verifiedRoute, confirm: makeConfirmStore() });
  await h({ replyToExternalId: null, text: "hello", reply: (t) => acks.push(t) });
  assert.equal(sends.length, 0);
  assert.ok(acks[0].includes("Reply to a specific message"));
});

test("reply (route miss / aged out): graceful ack; no send", async () => {
  const sends = []; const acks = [];
  const h = makeReplyHandler({ sendFn: async (a) => sends.push(a), getRouteFn: () => null, confirm: makeConfirmStore() });
  await h({ replyToExternalId: "999", text: "hi", reply: (t) => acks.push(t) });
  assert.equal(sends.length, 0);
  assert.ok(acks[0].includes("too old to reply"));
});

test("reply (unverified): first reply is HELD pending /yes; nothing sent yet", async () => {
  const sends = []; const acks = [];
  const confirm = makeConfirmStore();
  const h = makeReplyHandler({ sendFn: async (a) => sends.push(a), getRouteFn: () => unverifiedRoute, confirm });
  await h({ replyToExternalId: "5", text: "secret reply", reply: (t) => acks.push(t) });
  assert.equal(sends.length, 0);
  assert.ok(acks[0].includes("UNVERIFIED"));
  assert.ok(acks[0].includes("/yes"));
  assert.match(acks[0], /claims|stranger/i); // warns the reply target itself is unverifiable (misroute risk)
});

test("reply (unverified): /yes releases the HELD text to core.send", async () => {
  const sends = []; const acks = [];
  const confirm = makeConfirmStore();
  const h = makeReplyHandler({ sendFn: async (a) => sends.push(a), getRouteFn: () => unverifiedRoute, confirm });
  await h({ replyToExternalId: "5", text: "secret reply", reply: (t) => acks.push(t) });
  await h({ replyToExternalId: "5", text: "/yes", reply: (t) => acks.push(t) });
  assert.equal(sends.length, 1);
  assert.equal(sends[0].body, "secret reply");
  assert.equal(sends[0].to, "did:wba:x:agents:AIR-BOB");
  assert.ok(acks[1].includes("sent to AIR-BOB")); // no alias → short AIR-id
});

test("reply (unverified): /yes with nothing pending (expired) asks to resend; no send", async () => {
  const sends = []; const acks = [];
  const h = makeReplyHandler({ sendFn: async (a) => sends.push(a), getRouteFn: () => unverifiedRoute, confirm: makeConfirmStore() });
  await h({ replyToExternalId: "5", text: "/yes", reply: (t) => acks.push(t) });
  assert.equal(sends.length, 0);
  assert.ok(acks[0].toLowerCase().includes("nothing pending"));
});

test("reply: a core.send failure propagates (so the adapter won't ack the update)", async () => {
  const h = makeReplyHandler({ sendFn: async () => { throw new Error("relay down"); },
    getRouteFn: () => verifiedRoute, confirm: makeConfirmStore() });
  await assert.rejects(() => h({ replyToExternalId: "1", text: "hi", reply: () => {} }), /relay down/);
});

test("confirm store: purge() removes expired entries and reports the count", () => {
  let t = 0;
  const c = makeConfirmStore({ ttlMs: 100, now: () => t });
  c.put("old1", "x"); c.put("old2", "y");
  t = 200;
  c.put("fresh", "z");
  assert.equal(c.purge(), 2);        // old1 + old2 swept
  assert.equal(c.get("fresh"), "z"); // fresh (expires at 300) survives
});
