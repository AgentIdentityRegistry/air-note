import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { generateIdentity, sealBodyWithEphemeral, openBody, buildAad } from "../src/crypto.mjs";

const vectorsPath = fileURLToPath(new URL("../../crates/air-rs/tests/e2e_interop_vectors.json", import.meta.url));
const { vectors } = JSON.parse(readFileSync(vectorsPath, "utf8"));

test("interop vectors file is non-empty", () => {
  assert.ok(vectors.length > 0, "interop vectors file must contain at least one vector");
});

for (const [i, v] of vectors.entries()) {
  test(`interop vector ${i}: JS reproduces the expected sealed body`, () => {
    const id = generateIdentity(v.recipient_seed_hex);
    const aad = buildAad(v.env);
    const enc = sealBodyWithEphemeral(
      v.body, id.rawPublicKey, aad,
      Buffer.from(v.eph_secret_hex, "hex"), Buffer.from(v.nonce_hex, "hex"),
    );
    assert.deepEqual(enc, v.expected, "JS sealed body must match the frozen vector");
  });

  test(`interop vector ${i}: JS opens the expected sealed body`, () => {
    const aad = buildAad(v.env);
    assert.deepEqual(openBody(v.expected, v.recipient_seed_hex, aad), v.body);
  });
}
