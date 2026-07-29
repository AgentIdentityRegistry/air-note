# `.airmem` conformance vectors

Committed bytes. **Nothing here is generated at test time** — these files are the specification's
executable half, and any implementation that claims to read `.airmem` must land on the verdicts
below, byte for byte.

- **Format version:** `1.0.0` (`bossclaw_bundle::format::FORMAT_VERSION`). A verifier refuses a newer
  MAJOR (`FormatTooNew`).
- **Consumed by:** `crates/bossclaw-bundle/tests/conformance.rs` (this repo) and, from SP-V2, the
  `air-site` verify-page CI — the cross-repo half of spec §8. Treat these files as a published
  interface: changing one is changing the format.
- **Regenerate:** `AIRMEM_REGEN=1 cargo test -p bossclaw-bundle --test conformance`. Every vector is
  authored from fixed key seeds, so the output is byte-reproducible. Regeneration is **not** a way
  to make a failing digest assertion go away — see "Frozen digests" below.

## Why committed bytes rather than generated fixtures

Every other test in the workspace builds a bundle with `build_bundle` and checks it with `verify`,
so a change that moves **both** sides in step is invisible to all of them. These files do not move.
A change to the leaf recipe, the Merkle tag bytes, the canonical-JSON discipline, or the order in
which verify makes its decisions breaks a test here even when build and verify still agree with each
other.

## Frozen digests (the domain-separation guard)

`valid.airmem` carries **three** items on purpose. Three leaves means the root is
`H(0x01 ‖ H(0x01 ‖ leaf0 ‖ leaf1) ‖ leaf2)` — one *paired* internal node and one *promoted* odd node
— so a single root value pins both frozen A7 rules at once.

The three leaf hashes and the root are written out as constants in `conformance.rs`
(`VALID_LEAVES`, `VALID_MERKLE_ROOT`). That is the **direct** guard on the domain-separation tags:

- leaf = `SHA256(0x00 ‖ jcs(item))`, with `leaf` and `carried_origin` blanked first;
- internal = `SHA256(0x01 ‖ left ‖ right)`;
- an odd node is **promoted unpaired** — never duplicated;
- leaf order = item order.

Mutation-verified: deleting both tag bytes fails **8** of these vector tests, while the unit suite in
`merkle.rs` catches it with exactly one hand-computed assertion. If a digest constant stops matching,
the format changed — bump `FORMAT_VERSION`, re-freeze deliberately, and tell the cross-repo consumer.

## The `display` key charset rule

`AirmemItem.display` is the format's only free-form object, and object keys there **MUST be ASCII**.

`serde_jcs 0.1.0` sorts keys by UTF-8 bytes; RFC-8785 §3.2.3 mandates UTF-16 code-unit order. The two
diverge for any non-BMP key, and every emoji is non-BMP — so a non-ASCII key would make a *foreign*
verifier compute a different leaf hash from ours. Verify rejects such keys in the **shape** check,
which runs *before* the leaf recompute, so a conformant verifier never has to canonicalize the
divergent object at all.

`conformance.rs` sweeps every vector for this and fails if any of them carries a non-ASCII `display`
key — except `non_ascii_display.airmem`, whose entire job is to be that document and be rejected.

## Canonicalization scope

`.airmem` manifest/binding/item canonicalization is **JCS only — it does not NFC-normalize**, unlike
`bossclaw_canon::event::canonical_bytes` (which does, before JCS, for the event bytes a stamped item
carries). This is a conscious, recorded choice: for SP-V1/SP-V2 build, native verify and the WASM
verifier are one crate, so the two sides cannot disagree. A future foreign verifier (a C2PA/VC/SCITT
lift, spec §9 non-goal) must revisit it.

## The vectors

| File | Expected verdict | What it pins |
| --- | --- | --- |
| `valid.airmem` | ✅ verifies, `IdentityLevel::UnverifiedOffline` | The honest document: a stamped external note, a seal-vouched session, a seal-vouched ingest, plus the frozen leaf/root digests and the three kind-aware labels. |
| `tamper_seal.airmem` | `SealInvalid` | A well-formed Ed25519 seal by the *wrong* brain — not a garbage string. Fail-closed on the first check. |
| `tamper_item.airmem` | `ItemHashMismatch(0)` | A rewritten `items[].leaf` hex. No signature covers `leaf` (it is blanked before its own hash), so a verifier must **recompute** every leaf, never trust the stored value. Seal and root stay green in this file. |
| `tamper_binding_hash.airmem` | `BindingHashMismatch` | The sealed `binding_hash` no longer matches the embedded card (C-NEW-2). |
| `origin_mismatch.airmem` | `OriginMismatch(0)` | A5: the carried origin token disagrees with the origin recomputed from the stamp-covered event bytes. `carried_origin` is Merkle-excluded, so no re-seal was needed — the cross-check is the only thing that can catch it. |
| `re_attribution.airmem` | `BindingDidMismatch` | **Memory theft.** A stranger's card, minted over *this* brain's verifying key, with both hash layers recomputed so they pass. `binding.did ≠ manifest.did` stops it, and `manifest.did` is inside the seal. |
| `seal_vouched_carried_origin.airmem` | `Malformed` (`carried_origin`) | The proven attack: the strong write-time attestation token painted onto an export-time-vouched item. The seal **and** the root stay green (the test asserts both), because `carried_origin` is Merkle-excluded and a seal-vouched item has no `event_bytes` for the A5 cross-check to work from. The class↔field-set rule is the sole enforcement point. |
| `foreign_stamp.airmem` | `ItemStampInvalid(0)` | A6 laundering: an item validly signed by a *different* brain. Every stamp is checked against `manifest.brain_verifying_key` and nothing else. |
| `item_count_mismatch.airmem` | `Malformed` (`item_count`) | The sealed count is a producer's claim; it must still be cross-checked against the array. |
| `empty_items.airmem` | `Malformed` (`empty`) | `merkle::root` opens with an `assert!` that survives release builds, so an empty `items` array must be rejected *before* it — otherwise a hostile document traps the WASM verifier instead of rendering a clean ❌. |
| `wrong_purpose.airmem` | `Malformed` (`purpose`) | Domain separation: a card minted for another identity-signed protocol, re-signed and re-committed so it is perfect in every other respect. Without the tag it could be lifted over an attacker's own brain key. |
| `non_ascii_display.airmem` | `Malformed` (`display`) | The negative control for the ASCII rule above. The **only** vector permitted to carry a non-ASCII `display` key. |
| `did_injection.airmem` | ✅ **verifies** | Output injection. An attacker mints every key, so a `did` full of newlines and ANSI escapes rides inside an internally *perfect* document. Nothing in the format constrains a did — the guarantee lives at every render boundary (`sanitize_for_display`). A consumer should verify its own rendering against this file. |

## Key material

Fixed seeds, so the corpus is byte-reproducible: brain `[1u8; 32]`, identity `[9u8; 32]`, attacker
identity `[42u8; 32]`, foreign brain `[77u8; 32]`, wrong sealer `[8u8; 32]`. Timestamps are fixed at
`2026-07-21T00:00:00Z` — a vector with a clock in it is not a vector.
