import { test } from "node:test";
import assert from "node:assert/strict";
import {
  generateIdentity,
  ed25519PubToX25519,
  ed25519SeedToX25519,
  x25519PubMultibase,
  x25519PubFromMultibase,
  buildAad,
  sealBody,
  openBody,
} from "../src/crypto.mjs";
import { x25519 } from "@noble/curves/ed25519.js";

test("ed25519→x25519: derived keypair agrees with a fresh ephemeral (ECDH symmetry)", () => {
  const id = generateIdentity();
  const recipientX_pub = ed25519PubToX25519(id.rawPublicKey);          // padlock from stamp
  const recipientX_priv = ed25519SeedToX25519(Buffer.from(id.seedHex, "hex"));

  const ephPriv = x25519.utils.randomPrivateKey();
  const ephPub = x25519.getPublicKey(ephPriv);

  const sharedSender = x25519.getSharedSecret(ephPriv, recipientX_pub);
  const sharedRecipient = x25519.getSharedSecret(recipientX_priv, ephPub);
  assert.deepEqual(Buffer.from(sharedSender), Buffer.from(sharedRecipient));
  assert.equal(recipientX_pub.length, 32);
});

test("x25519 multibase round-trips and uses the 0xec01 multicodec", () => {
  const id = generateIdentity();
  const xpub = ed25519PubToX25519(id.rawPublicKey);
  const mb = x25519PubMultibase(xpub);
  assert.ok(mb.startsWith("z6LS"), `expected did:key x25519 prefix, got ${mb.slice(0, 6)}`);
  assert.deepEqual(x25519PubFromMultibase(mb), Buffer.from(xpub));
});

const ENV = {
  id: "11111111-1111-4111-8111-111111111111",
  from: "did:wba:example:agents:AIR-AAAA",
  to: "did:wba:example:agents:AIR-BBBB",
  thread_id: "22222222-2222-4222-8222-222222222222",
};

test("sealBody → openBody round-trips the original body", () => {
  const recipient = generateIdentity();
  const aad = buildAad(ENV);
  const body = { type: "text", text: "hello 🔐" };
  const enc = sealBody(body, recipient.rawPublicKey, aad);

  assert.equal(enc.type, "encrypted");
  assert.equal(enc.alg, "x25519-hkdf-sha256-chacha20poly1305");
  assert.equal(enc.v, 1);
  assert.ok(enc.epk.startsWith("z6LS"));

  const opened = openBody(enc, recipient.seedHex, aad);
  assert.deepEqual(opened, body);
});

test("openBody fails with the wrong recipient key", () => {
  const recipient = generateIdentity();
  const wrong = generateIdentity();
  const aad = buildAad(ENV);
  const enc = sealBody({ type: "text", text: "secret" }, recipient.rawPublicKey, aad);
  assert.throws(() => openBody(enc, wrong.seedHex, aad));
});

test("openBody fails when the ciphertext is tampered", () => {
  const recipient = generateIdentity();
  const aad = buildAad(ENV);
  const enc = sealBody({ type: "text", text: "secret" }, recipient.rawPublicKey, aad);
  const ctBuf = Buffer.from(enc.ct, "base64url");
  ctBuf[0] ^= 0x01; // flip a bit
  enc.ct = ctBuf.toString("base64url");
  assert.throws(() => openBody(enc, recipient.seedHex, aad));
});

test("openBody fails when the AAD (envelope address) differs", () => {
  const recipient = generateIdentity();
  const enc = sealBody({ type: "text", text: "secret" }, recipient.rawPublicKey, buildAad(ENV));
  const otherAad = buildAad({ ...ENV, to: "did:wba:example:agents:AIR-CCCC" });
  assert.throws(() => openBody(enc, recipient.seedHex, otherAad));
});

test("openBody fails on a malformed epk", () => {
  const recipient = generateIdentity();
  const aad = buildAad(ENV);
  const enc = sealBody({ type: "text", text: "secret" }, recipient.rawPublicKey, aad);
  enc.epk = "zNotARealMultibaseKey";
  assert.throws(() => openBody(enc, recipient.seedHex, aad));
});

import { buildOutboundEnvelope } from "../src/core.mjs";
import { verifyEnvelope } from "../src/crypto.mjs";

test("buildOutboundEnvelope encrypts by default and stays signature-valid", () => {
  const sender = generateIdentity();
  const recipient = generateIdentity();
  const identity = { did: "did:wba:example:agents:AIR-SEND", privateKey: sender.privateKey };
  const env = buildOutboundEnvelope({
    identity,
    recipientDid: "did:wba:example:agents:AIR-RECV",
    recipientEd25519Pub: recipient.rawPublicKey,
    body: "hi there",
  });
  assert.equal(env.body.type, "encrypted");
  assert.ok(verifyEnvelope(env, sender.rawPublicKey), "signature must verify over the encrypted body");
});

test("buildOutboundEnvelope leaves plaintext when plaintext:true", () => {
  const sender = generateIdentity();
  const identity = { did: "did:wba:example:agents:AIR-SEND", privateKey: sender.privateKey };
  const env = buildOutboundEnvelope({
    identity,
    recipientDid: "did:wba:example:agents:AIR-RECV",
    recipientEd25519Pub: null,
    body: "hi",
    plaintext: true,
  });
  assert.equal(env.body.type, "text");
  assert.equal(env.body.text, "hi");
});

test("buildOutboundEnvelope throws when key is null and plaintext is not set (no silent downgrade)", () => {
  const sender = generateIdentity();
  const identity = { did: "did:wba:example:agents:AIR-SEND", privateKey: sender.privateKey };
  assert.throws(
    () => buildOutboundEnvelope({ identity, recipientDid: "did:wba:example:agents:AIR-RECV", recipientEd25519Pub: null, body: "hi" }),
    /refusing to send unencrypted/,
  );
});

import { decodeReceivedMessage } from "../src/core.mjs";

test("decodeReceivedMessage decrypts an encrypted body and flags encrypted:true", () => {
  const sender = generateIdentity();
  const recipient = generateIdentity();
  const identity = { did: "did:wba:example:agents:AIR-SEND", privateKey: sender.privateKey };
  const env = buildOutboundEnvelope({
    identity,
    recipientDid: "did:wba:example:agents:AIR-RECV",
    recipientEd25519Pub: recipient.rawPublicKey,
    body: { type: "text", text: "sealed!" },
  });
  const out = decodeReceivedMessage(env, recipient.seedHex);
  assert.equal(out.encrypted, true);
  assert.deepEqual(out.body, { type: "text", text: "sealed!" });
  assert.equal(out.decrypt_error, undefined);
});

test("decodeReceivedMessage passes through cleartext bodies with encrypted:false", () => {
  const recipient = generateIdentity();
  const env = { body: { type: "text", text: "open note" } };
  const out = decodeReceivedMessage(env, recipient.seedHex);
  assert.equal(out.encrypted, false);
  assert.deepEqual(out.body, { type: "text", text: "open note" });
});

test("decodeReceivedMessage reports decrypt_error without throwing", () => {
  const sender = generateIdentity();
  const recipient = generateIdentity();
  const wrong = generateIdentity();
  const identity = { did: "did:wba:example:agents:AIR-SEND", privateKey: sender.privateKey };
  const env = buildOutboundEnvelope({
    identity,
    recipientDid: "did:wba:example:agents:AIR-RECV",
    recipientEd25519Pub: recipient.rawPublicKey,
    body: "secret",
  });
  const out = decodeReceivedMessage(env, wrong.seedHex);
  assert.equal(out.encrypted, true);
  assert.ok(out.decrypt_error, "should report an error string");
  assert.equal(out.body, undefined);
});

test("decodeReceivedMessage on an UNVERIFIED encrypted body flags encrypted:true and hides the ciphertext", () => {
  const sender = generateIdentity();
  const recipient = generateIdentity();
  const identity = { did: "did:wba:example:agents:AIR-SEND", privateKey: sender.privateKey };
  const env = buildOutboundEnvelope({
    identity, recipientDid: "did:wba:example:agents:AIR-RECV",
    recipientEd25519Pub: recipient.rawPublicKey, body: "secret",
  });
  const out = decodeReceivedMessage(env, recipient.seedHex, false); // verified = false
  assert.equal(out.encrypted, true);
  assert.equal(out.body, undefined); // raw ciphertext must NOT be surfaced
});

test("decodeReceivedMessage on an UNVERIFIED cleartext body passes it through as encrypted:false", () => {
  const recipient = generateIdentity();
  const env = { body: { type: "text", text: "open" } };
  const out = decodeReceivedMessage(env, recipient.seedHex, false);
  assert.equal(out.encrypted, false);
  assert.deepEqual(out.body, { type: "text", text: "open" });
});
