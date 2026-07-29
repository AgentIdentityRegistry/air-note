//! Rung-5 SP-V1 — the COMMITTED `.airmem` conformance vectors.
//!
//! Every other test in this workspace proves our code agrees with itself: it builds a bundle with
//! `build_bundle` and verifies it with `verify`, so a change that moves BOTH sides in step stays
//! invisible. These vectors are the opposite instrument. They are bytes checked into
//! `tests/vectors/`, and this file only READS them — so any change to the format, the leaf recipe,
//! the Merkle tag bytes, the canonical-JSON discipline or the verdict ordering breaks a test even
//! when build and verify still agree with each other. That is also what makes them consumable by a
//! FOREIGN implementation (SP-V2's `air-site` verify-page CI, spec §8 cross-repo conformance).
//!
//! THE TAG-BYTE GUARD (Task-3/4 carry-forward, the reason this file exists at all). The frozen A7
//! domain-separation tags — `0x00` for a leaf, `0x01` for an internal node — had only INDIRECT unit
//! coverage: mutation showed that deleting BOTH tags was caught by exactly one hand-computed
//! assertion in `merkle.rs`, and that the `domain_separation_*` test SURVIVED that deletion. So the
//! digests below are written out in full: [`VALID_LEAVES`] pins the leaf tag, [`VALID_MERKLE_ROOT`]
//! pins the internal tag, and the 3-item shape exercises BOTH a paired internal node and the
//! odd-node promotion rule in one root. Changing a tag byte changes these numbers, and no amount of
//! build/verify co-evolution can hide it.
//!
//! REGENERATION. `AIRMEM_REGEN=1 cargo test -p bossclaw-bundle --test conformance` re-authors every
//! vector from the fixed keys below before the readers run (once per process, via [`std::sync::Once`],
//! so a parallel test run cannot race a half-written file). Without the variable the committed files
//! are the sole source of truth. Regenerating is NOT a way to make a failure go away: if a digest
//! constant below stops matching, the format changed, and that is a decision to make deliberately
//! (bump `FORMAT_VERSION`, re-freeze the constants, tell SP-V2).
//!
//! Every vector is authored from FIXED key material, so the files are byte-reproducible.

use std::path::PathBuf;
use std::sync::Once;

use bossclaw_bundle::binding::binding_signing_bytes;
use bossclaw_bundle::build::{build_bundle, BuildInput, ItemInput};
use bossclaw_bundle::format::{canonical_json, Airmem, AirmemItem, Binding, BindingPayload, ItemClass};
use bossclaw_bundle::{merkle, object_keys_are_ascii, sanitize_for_display};
use bossclaw_bundle::{verify, IdentityLevel, OfflineResolver, VerifyError, BINDING_PURPOSE};
use bossclaw_canon::event::{canonical_bytes, compute_hash, Event};
use bossclaw_canon::sign::{sign_bytes, sign_hash, verify_bytes, SigningKey};

// ── Fixed key material. Every vector is reproducible from these five seeds. ────────────────────

/// The honest brain key: seals every manifest and stamps the note in `valid.airmem`.
const BRAIN: [u8; 32] = [1u8; 32];
/// The honest identity key: signs the binding card.
const IDENTITY: [u8; 32] = [9u8; 32];
/// A SECOND identity, for the re-attribution vector — a stranger re-minting a card over this
/// brain's key so the memories read as theirs.
const ATTACKER_IDENTITY: [u8; 32] = [42u8; 32];
/// A SECOND brain, for the foreign-stamp vector — a validly-signed event by a brain that is not the
/// one the manifest names (A6 laundering).
const FOREIGN_BRAIN: [u8; 32] = [77u8; 32];
/// A SECOND brain used to re-seal `tamper_seal.airmem`, so its seal is a WELL-FORMED signature that
/// simply is not by the brain the manifest names — a stronger vector than a garbage string.
const WRONG_SEALER: [u8; 32] = [8u8; 32];

/// The did every honest vector claims.
const DID: &str = "did:wba:example.com:me";
/// Fixed timestamps: a vector with a clock in it is not a vector.
const WHEN: &str = "2026-07-21T00:00:00Z";

// ── THE FROZEN DIGESTS ────────────────────────────────────────────────────────────────────────
// These are the cross-repo checkable numbers. See this file's header for why they are written out.

/// `valid.airmem`'s three leaf hashes, in item order: `SHA256(0x00 ‖ jcs(item))` with `leaf` and
/// `carried_origin` blanked. Pins the LEAF tag directly — delete or change `0x00` and all three move.
const VALID_LEAVES: [&str; 3] = [
    "04b6504846e9e27694d740feb65ece6dc2d993602c50069b792e1544f97cd40d",
    "8063b9fee226ae6c319590994fdae3dc4b5c650b4d88afbfbef19221a7f15498",
    "f334cb9a5991f82f1e0b27689848c08959eac28bf15675c65426ebf753d11bd0",
];

/// `valid.airmem`'s Merkle root over those three leaves:
/// `H(0x01 ‖ H(0x01 ‖ leaf0 ‖ leaf1) ‖ leaf2)` — a paired internal node AND an odd-node promotion,
/// so this one number pins the INTERNAL tag and the no-duplicate-last rule together.
const VALID_MERKLE_ROOT: &str = "33d8a4f3d5c91a151a2fcc072d708485138839b1fcc460e02f0c9fa3c6428c5f";

// ── Vector loading ────────────────────────────────────────────────────────────────────────────

/// Repo-root `tests/vectors/` — deliberately OUTSIDE this crate, because the vectors belong to the
/// format, not to the Rust implementation that happens to read them first.
fn vectors_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/vectors")
}

/// Read one committed vector. Every reader goes through here, so the regen hook has exactly one
/// place to hang and cannot half-run.
fn load(name: &str) -> Airmem {
    static REGEN: Once = Once::new();
    REGEN.call_once(|| {
        if std::env::var_os("AIRMEM_REGEN").is_some() {
            regenerate();
        }
    });
    let path = vectors_dir().join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("missing vector {}: {e}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("vector {} does not parse as an .airmem: {e}", path.display()))
}

/// Every vector this suite knows about, with the file name it lives under. The ASCII-conformance
/// sweep walks this list, so a vector added without being listed here is a vector that skipped the
/// sweep — which is exactly the drift the sweep exists to prevent.
const ALL_VECTORS: &[&str] = &[
    "valid.airmem",
    "tamper_seal.airmem",
    "tamper_item.airmem",
    "tamper_binding_hash.airmem",
    "origin_mismatch.airmem",
    "re_attribution.airmem",
    "seal_vouched_carried_origin.airmem",
    "foreign_stamp.airmem",
    "item_count_mismatch.airmem",
    "empty_items.airmem",
    "wrong_purpose.airmem",
    "non_ascii_display.airmem",
    "did_injection.airmem",
];

/// The ONE vector allowed to carry a non-ASCII `display` key — it is the negative control for that
/// very rule. See `display_keys_are_ascii_in_every_vector_but_the_negative_control`.
const NON_ASCII_VECTOR: &str = "non_ascii_display.airmem";

fn off() -> OfflineResolver {
    OfflineResolver
}

/// The `Malformed` detail, or a panic naming what came back instead.
fn malformed_detail(e: &VerifyError) -> &str {
    match e {
        VerifyError::Malformed(d) => d,
        other => panic!("expected Malformed, got {other:?}"),
    }
}

// ── THE VERDICTS ──────────────────────────────────────────────────────────────────────────────

/// The honest vector verifies, stays at L1, and renders the kind-aware labels — three item classes
/// in one file (a stamped external note, a seal-vouched session, a seal-vouched ingest).
#[test]
fn valid_vector_verifies_green_offline() {
    let v = verify(&load("valid.airmem"), &off()).expect("the committed valid vector must verify");
    assert_eq!(
        v.identity,
        IdentityLevel::UnverifiedOffline,
        "offline resolution can never upgrade identity (spec §2.5 C3)"
    );
    assert_eq!(
        v.item_labels,
        vec![
            "this brain recorded these bytes; provenance of the underlying text is not asserted",
            "captured session, content only; not independently verified",
            "ingested file extract; not independently verified",
        ],
        "the honest per-kind labels are part of the format's contract, not a rendering detail"
    );
}

/// THE TAG-BYTE GUARD. The committed leaves and root are recomputed from the committed items and
/// compared against digests frozen in THIS SOURCE FILE — so a change to the domain-separation tags
/// (or to the leaf recipe, or to JCS canonicalization) fails here even if build and verify were
/// changed together and still agree with each other.
#[test]
fn frozen_merkle_digests_match_the_committed_vector() {
    let b = load("valid.airmem");
    assert_eq!(b.items.len(), 3, "the shape the frozen digests were taken over");

    let leaves: Vec<[u8; 32]> = b.items.iter().map(merkle::leaf_hash).collect();
    for (i, (leaf, frozen)) in leaves.iter().zip(VALID_LEAVES).enumerate() {
        assert_eq!(
            hex::encode(leaf),
            frozen,
            "item {i}'s leaf moved. If you changed the 0x00 LEAF tag, the leaf recipe, or the \
             canonical-JSON discipline, that is a FORMAT CHANGE: bump FORMAT_VERSION and re-freeze \
             these constants deliberately."
        );
        assert_eq!(
            b.items[i].leaf, frozen,
            "item {i}'s STORED leaf disagrees with the frozen digest — the committed file itself drifted"
        );
    }
    let root = hex::encode(merkle::root(&leaves));
    assert_eq!(
        root, VALID_MERKLE_ROOT,
        "the Merkle root moved. Three leaves means one PAIRED internal node plus one PROMOTED odd \
         node, so this single number pins the 0x01 INTERNAL tag and the no-duplicate-last rule."
    );
    assert_eq!(
        b.manifest.merkle_root, VALID_MERKLE_ROOT,
        "the SEALED root must equal the frozen digest — this is the value a foreign verifier checks"
    );
}

/// A seal that is a well-formed Ed25519 signature by the WRONG brain. Fail-closed on the first
/// check, before anything else in the document is trusted.
#[test]
fn tamper_seal_fails() {
    assert_eq!(verify(&load("tamper_seal.airmem"), &off()), Err(VerifyError::SealInvalid));
}

/// A rewritten `items[].leaf` hex. `leaf` is blanked before its own hash, so NO signature covers it
/// — it is freely rewritable with the seal and the root still green. Verify must RECOMPUTE every
/// leaf rather than trust the stored hex (Task-5 review, proven).
#[test]
fn tamper_item_fails() {
    let b = load("tamper_item.airmem");
    assert_eq!(verify(&b, &off()), Err(VerifyError::ItemHashMismatch(0)));
    // Prove the file really is the "seal still green" shape this vector is about.
    let brain_vk = SigningKey::from_bytes(&BRAIN).verifying_key();
    verify_bytes(&canonical_json(&b.manifest).unwrap(), &b.seal, &brain_vk)
        .expect("the seal is untouched — only the uncommitted leaf hex was rewritten");
}

/// The sealed `binding_hash` no longer matches the embedded card (C-NEW-2).
#[test]
fn tamper_binding_hash_fails() {
    assert_eq!(
        verify(&load("tamper_binding_hash.airmem"), &off()),
        Err(VerifyError::BindingHashMismatch)
    );
}

/// A5: the exporter's `carried_origin` token disagrees with the origin recomputed from the
/// stamp-covered event bytes. `carried_origin` is Merkle-EXCLUDED, so a lone flip needs no re-seal
/// — the cross-check is the only thing that can catch it.
#[test]
fn origin_mismatch_fails() {
    assert_eq!(verify(&load("origin_mismatch.airmem"), &off()), Err(VerifyError::OriginMismatch(0)));
}

/// RE-ATTRIBUTION (C1) — the memory-theft shape the design review called a Blocker. A stranger
/// mints their own card over THIS brain's verifying key so the memories read as theirs, then
/// recomputes `binding_hash` and re-seals so both hash layers pass. `binding.did ≠ manifest.did` is
/// what stops it, and `manifest.did` is inside the seal.
#[test]
fn re_attribution_fails() {
    let e = verify(&load("re_attribution.airmem"), &off()).unwrap_err();
    assert!(
        matches!(e, VerifyError::BindingHashMismatch | VerifyError::BindingDidMismatch),
        "re-attribution must fail on one of the two binding layers, got {e:?}"
    );
    assert_eq!(e, VerifyError::BindingDidMismatch, "this vector is the RECOMPUTED (layer 2) form");
}

/// THE PROVEN ATTACK the class↔field-set invariant exists for: `carried_origin` painted onto a
/// SEAL-VOUCHED item. `carried_origin` is Merkle-excluded and a seal-vouched item has no
/// `event_bytes`, so the seal stays green, the root stays green, and the A5 cross-check has no
/// substrate to run against — the shape rule is the SOLE enforcement point. The vector asserts both
/// commitments really are green, or it would prove nothing.
#[test]
fn seal_vouched_carried_origin_is_malformed_with_every_commitment_still_green() {
    let b = load("seal_vouched_carried_origin.airmem");
    assert_eq!(
        b.items[1].carried_origin.as_deref(),
        Some("external"),
        "the vector must actually carry the painted token"
    );
    // Seal: green.
    let brain_vk = SigningKey::from_bytes(&BRAIN).verifying_key();
    verify_bytes(&canonical_json(&b.manifest).unwrap(), &b.seal, &brain_vk)
        .expect("the seal still verifies — that is the whole point of this vector");
    // Leaves and root: green.
    let leaves: Vec<[u8; 32]> = b.items.iter().map(merkle::leaf_hash).collect();
    assert_eq!(b.items[1].leaf, hex::encode(leaves[1]), "the painted item's leaf still matches");
    assert_eq!(b.manifest.merkle_root, hex::encode(merkle::root(&leaves)), "the root still matches");
    // ...and the shape rule is what rejects.
    let e = verify(&b, &off()).unwrap_err();
    assert!(malformed_detail(&e).contains("carried_origin"), "got {e:?}");
}

/// A6 FOREIGN-EVENT LAUNDERING: an item whose stamp is a perfectly valid signature by a DIFFERENT
/// brain key. Every item stamp is checked against `manifest.brain_verifying_key` and nothing else,
/// so a stranger's signed memory cannot be re-hosted inside this brain's bundle.
#[test]
fn foreign_stamp_fails() {
    assert_eq!(verify(&load("foreign_stamp.airmem"), &off()), Err(VerifyError::ItemStampInvalid(0)));
}

/// `manifest.item_count` disagreeing with `items.len()`. Sealed, so the count is a claim the
/// producer made under signature; it must still be cross-checked against the array.
#[test]
fn item_count_mismatch_is_malformed() {
    let e = verify(&load("item_count_mismatch.airmem"), &off()).unwrap_err();
    assert!(malformed_detail(&e).contains("item_count"), "got {e:?}");
}

/// `items: []`. `merkle::root` opens with an `assert!` that is NOT compiled out in release, so
/// reaching it with a hostile document would TRAP the WASM verifier instead of rendering a clean ❌.
/// This vector is the artifact that keeps that pre-check honest across implementations.
#[test]
fn empty_items_is_malformed_never_a_panic() {
    let b = load("empty_items.airmem");
    assert!(b.items.is_empty(), "the vector must actually be empty, or it proves nothing");
    let e = verify(&b, &off()).unwrap_err();
    assert!(malformed_detail(&e).contains("empty"), "got {e:?}");
}

/// DOMAIN SEPARATION (spec §2.3): a card minted for a DIFFERENT identity-signed protocol, re-signed
/// and re-committed so it is perfect in every other respect. Without the `purpose` tag an attacker
/// could lift such a card over their own brain key and reach a resolved identity — re-attribution
/// by side door.
#[test]
fn wrong_purpose_is_malformed() {
    let b = load("wrong_purpose.airmem");
    assert_ne!(b.binding.payload.purpose, BINDING_PURPOSE, "the vector carries a foreign purpose");
    assert!(
        bossclaw_bundle::verify_binding_internal(&b.binding),
        "the card self-verifies — only the purpose tag may be what rejects it"
    );
    let e = verify(&b, &off()).unwrap_err();
    assert!(malformed_detail(&e).contains("purpose"), "got {e:?}");
}

/// FOREIGN-VERIFIER CONFORMANCE (Task-3 review #5, proven): `serde_jcs 0.1.0` sorts object keys by
/// UTF-8 bytes while RFC-8785 §3.2.3 mandates UTF-16 code-unit order. The two diverge for any
/// non-BMP key, and every emoji is non-BMP. `display` is the format's only free-form object, so
/// non-ASCII keys there are refused — and refused by the SHAPE check, which runs BEFORE the leaf
/// recompute, so a conformant verifier never has to canonicalize the divergent object at all.
#[test]
fn non_ascii_display_keys_are_malformed() {
    let e = verify(&load(NON_ASCII_VECTOR), &off()).unwrap_err();
    assert!(malformed_detail(&e).contains("display"), "got {e:?}");
}

/// THE CONSTRAINT, ENCODED IN THE VECTOR SET ITSELF. No vector may carry a bundle a foreign
/// verifier would canonicalize differently from us — except the one whose entire job is to be that
/// bundle and be REJECTED for it. Without this sweep a future vector could quietly ship an emoji
/// `display` key and turn the whole corpus non-portable.
#[test]
fn display_keys_are_ascii_in_every_vector_but_the_negative_control() {
    for name in ALL_VECTORS {
        let b = load(name);
        let ascii = b
            .items
            .iter()
            .filter_map(|i| i.display.as_ref())
            .all(object_keys_are_ascii);
        if *name == NON_ASCII_VECTOR {
            assert!(!ascii, "{name} is the negative control and MUST carry a non-ASCII display key");
        } else {
            assert!(
                ascii,
                "{name} carries a non-ASCII `display` key. A foreign verifier would compute a \
                 different leaf hash for it (JCS UTF-8 vs RFC-8785 UTF-16 key order), so this \
                 vector would be unusable as cross-repo conformance data."
            );
        }
    }
}

/// OUTPUT INJECTION. An attacker mints every key in their own bundle, so a `did` full of newlines
/// and ANSI escapes rides inside a document that is INTERNALLY PERFECT — this vector VERIFIES.
/// That is the finding: nothing in the format constrains a did, so the guarantee has to live at
/// every render boundary, and `sanitize_for_display` is the shared implementation of it. The vector
/// exists so a consumer (this repo's CLI, the app sheet, SP-V2's WASM host) has a real file to
/// prove its own rendering against.
#[test]
fn did_injection_verifies_and_must_be_sanitized_by_every_renderer() {
    let b = load("did_injection.airmem");
    verify(&b, &off()).expect("a forged did does not make a bundle inconsistent — that is the point");
    assert!(
        b.manifest.did.contains('\n') && b.manifest.did.contains('\u{1b}'),
        "the vector must actually carry the line-forging characters"
    );
    let shown = sanitize_for_display(&b.manifest.did);
    assert!(!shown.contains('\n') && !shown.contains('\u{1b}'), "rendered raw this forges a line: {shown:?}");
    assert_eq!(shown.lines().count(), 1, "a rendered did occupies exactly one line");
    assert!(shown.contains("did:wba:evil.com:x"), "the receiver still sees WHAT the file claimed");
}

/// The whole corpus in one place: every vector parses, and each one lands on the verdict this suite
/// says it does. A vector that stopped being loadable — or that silently started passing — would
/// otherwise only show up in whichever individual test happened to name it.
#[test]
fn every_vector_is_loadable_and_lands_on_its_documented_verdict() {
    let passes = ["valid.airmem", "did_injection.airmem"];
    for name in ALL_VECTORS {
        let outcome = verify(&load(name), &off());
        if passes.contains(name) {
            assert!(outcome.is_ok(), "{name} is a PASSING vector, got {outcome:?}");
        } else {
            assert!(outcome.is_err(), "{name} is a REJECTING vector but it verified");
        }
    }
}

// ── REGENERATION ──────────────────────────────────────────────────────────────────────────────
// Everything below authors the committed files. It runs ONLY under `AIRMEM_REGEN=1`.

/// The honest 3-item bundle every vector is derived from: a stamped external note, a seal-vouched
/// session, a seal-vouched ingest. Three items on purpose — the Merkle root then covers one PAIRED
/// internal node and one PROMOTED odd node, so the frozen root pins both A7 rules at once.
fn valid_bundle() -> Airmem {
    let brain = SigningKey::from_bytes(&BRAIN);
    let brain_mb = multibase::encode(multibase::Base::Base58Btc, brain.verifying_key().to_bytes());
    let ev = note_event("shared note");
    let event_bytes = String::from_utf8(canonical_bytes(&ev).unwrap()).unwrap();
    let signature = sign_hash(&compute_hash(&ev).unwrap(), &brain);
    build_bundle(BuildInput {
        created_at: WHEN.into(),
        did: DID.into(),
        brain_verifying_key: brain_mb.clone(),
        selection_description: "1 note + 1 session + 1 ingest".into(),
        items: vec![
            ItemInput::Stamped { event_bytes, signature, carried_origin: "external".into() },
            ItemInput::SealVouched {
                kind: "session".into(),
                content: "## You\nHow do I revoke a trust?\n\n## Assistant\nFile a revocation.\n".into(),
                display: Some(serde_json::json!({
                    "title": "revoking a trust", "tool": "claude-code", "project": "wills-and-trusts",
                    "started_at": 1_783_764_000, "ended_at": 1_783_764_009, "approx_bytes": 64,
                })),
            },
            ItemInput::SealVouched {
                kind: "ingest".into(),
                content: "The lease term begins on the first of the month.".into(),
                display: None,
            },
        ],
        binding: honest_binding(&brain_mb),
        brain_key: &brain,
    })
}

/// The one event shape the stamped class carries: an EXTERNAL `remember()` note. Only external
/// notes are exportable as stamped items (core's own server-side guard), so no other origin shape
/// belongs in this corpus.
fn note_event(text: &str) -> Event {
    let content = serde_json::json!({ "text": text, "origin": "external" });
    Event {
        id: "01J0000000000000000000000A".into(),
        ts: "2026-06-15T00:00:00Z".into(),
        valid_time: None,
        event_type: "memory".into(),
        content,
        model_meta: None,
        prev_hash: "00".repeat(32),
        hash: None,
        signed_by_did: DID.into(),
        signature: None,
    }
}

/// A card signed by `key`, claiming `did` and `purpose`, over `brain_mb`.
fn card(key_bytes: &[u8; 32], brain_mb: &str, did: &str, purpose: &str) -> Binding {
    let key = SigningKey::from_bytes(key_bytes);
    let payload = BindingPayload {
        brain_verifying_key: brain_mb.into(),
        identity_verifying_key: multibase::encode(
            multibase::Base::Base58Btc,
            key.verifying_key().to_bytes(),
        ),
        did: did.into(),
        purpose: purpose.into(),
        epoch: 1,
        created_at: WHEN.into(),
    };
    let identity_signature = sign_bytes(&binding_signing_bytes(&payload), &key);
    Binding { payload, identity_signature }
}

fn honest_binding(brain_mb: &str) -> Binding {
    card(&IDENTITY, brain_mb, DID, BINDING_PURPOSE)
}

/// Re-sign the manifest as it stands, with `key_bytes` — used both to restore an honest seal after
/// a deliberate manifest edit and (with a foreign key) to author `tamper_seal`.
fn reseal_with(b: &mut Airmem, key_bytes: &[u8; 32]) {
    b.seal = sign_bytes(&canonical_json(&b.manifest).unwrap(), &SigningKey::from_bytes(key_bytes));
}

fn reseal(b: &mut Airmem) {
    reseal_with(b, &BRAIN);
}

/// Re-leaf + re-root + re-count + re-seal: every cryptographic commitment green again, so the
/// vector proves that a check OTHER than the hashes is what rejects it.
fn reindex(b: &mut Airmem) {
    for i in 0..b.items.len() {
        b.items[i].leaf = hex::encode(merkle::leaf_hash(&b.items[i]));
    }
    let leaves: Vec<[u8; 32]> = b.items.iter().map(merkle::leaf_hash).collect();
    b.manifest.merkle_root = hex::encode(merkle::root(&leaves));
    b.manifest.item_count = b.items.len() as u64;
    reseal(b);
}

/// Recommit the (edited) card into the sealed manifest, so `binding_hash` passes and a LATER check
/// is what rejects.
fn recommit_binding(b: &mut Airmem) {
    b.manifest.binding_hash = hex::encode(bossclaw_bundle::binding_hash(&b.binding));
    reseal(b);
}

/// Author every vector from the fixed keys. Files are written as canonical (JCS) JSON with a
/// trailing newline, so `git diff` on a regenerated corpus is readable.
fn regenerate() {
    let dir = vectors_dir();
    std::fs::create_dir_all(&dir).expect("create tests/vectors");
    let brain_mb = multibase::encode(
        multibase::Base::Base58Btc,
        SigningKey::from_bytes(&BRAIN).verifying_key().to_bytes(),
    );

    let mut written: Vec<&str> = Vec::new();
    let mut write = |name: &'static str, b: &Airmem| {
        let mut text = String::from_utf8(canonical_json(b).expect("a bundle always serializes"))
            .expect("canonical JSON is UTF-8");
        text.push('\n');
        std::fs::write(dir.join(name), text).unwrap_or_else(|e| panic!("write {name}: {e}"));
        written.push(name);
    };

    write("valid.airmem", &valid_bundle());

    // A well-formed signature by a brain that is not the one the manifest names.
    let mut b = valid_bundle();
    reseal_with(&mut b, &WRONG_SEALER);
    write("tamper_seal.airmem", &b);

    // The uncommitted `leaf` hex, rewritten. Seal and root are untouched.
    let mut b = valid_bundle();
    b.items[0].leaf = "00".repeat(32);
    write("tamper_item.airmem", &b);

    let mut b = valid_bundle();
    b.manifest.binding_hash = "00".repeat(32);
    reseal(&mut b);
    write("tamper_binding_hash.airmem", &b);

    // carried_origin is leaf-EXCLUDED, so this flip needs no re-index.
    let mut b = valid_bundle();
    b.items[0].carried_origin = Some("unattested".into());
    write("origin_mismatch.airmem", &b);

    // A stranger's card over this brain's key, with BOTH hash layers recomputed.
    let mut b = valid_bundle();
    b.binding = card(&ATTACKER_IDENTITY, &brain_mb, "did:wba:evil.com:attacker", BINDING_PURPOSE);
    recommit_binding(&mut b);
    write("re_attribution.airmem", &b);

    // The strong write-time token painted onto an export-time-vouched item. NO re-index: the point
    // is that every commitment is ALREADY green.
    let mut b = valid_bundle();
    b.items[1].carried_origin = Some("external".into());
    write("seal_vouched_carried_origin.airmem", &b);

    // A6: a validly-signed event by a foreign brain, re-indexed so only the stamp check can reject.
    let mut b = valid_bundle();
    let foreign = SigningKey::from_bytes(&FOREIGN_BRAIN);
    let mut ev = note_event("a memory from someone else's brain");
    ev.id = "01J000000000000000000000FF".into();
    b.items[0] = AirmemItem {
        leaf: String::new(),
        class: ItemClass::Stamped,
        kind: "note".into(),
        event_bytes: Some(String::from_utf8(canonical_bytes(&ev).unwrap()).unwrap()),
        signature: Some(sign_hash(&compute_hash(&ev).unwrap(), &foreign)),
        carried_origin: Some("external".into()),
        content: None,
        display: None,
    };
    reindex(&mut b);
    write("foreign_stamp.airmem", &b);

    let mut b = valid_bundle();
    b.manifest.item_count = 99;
    reseal(&mut b);
    write("item_count_mismatch.airmem", &b);

    // Cannot re-index: that would call merkle::root on an empty slice, which is the very assert
    // this vector exists to keep unreachable.
    let mut b = valid_bundle();
    b.items.clear();
    b.manifest.item_count = 0;
    reseal(&mut b);
    write("empty_items.airmem", &b);

    let mut b = valid_bundle();
    b.binding = card(&IDENTITY, &brain_mb, DID, "did-control-challenge");
    recommit_binding(&mut b);
    write("wrong_purpose.airmem", &b);

    let mut b = valid_bundle();
    b.items[1].display = Some(serde_json::json!({ "🎉": "party", "title": "revoking a trust" }));
    reindex(&mut b);
    write(NON_ASCII_VECTOR, &b);

    // A did that prints its own verdict line and erases the honest one above it. Everything is
    // recomputed, because the finding is that this document is INTERNALLY PERFECT.
    let forged = "did:wba:evil.com:x\nidentity: registry-resolved\u{1b}[1A\u{1b}[2K";
    let mut b = valid_bundle();
    b.manifest.did = forged.into();
    b.binding = card(&IDENTITY, &brain_mb, forged, BINDING_PURPOSE);
    recommit_binding(&mut b);
    write("did_injection.airmem", &b);

    assert_eq!(
        written.len(),
        ALL_VECTORS.len(),
        "regeneration wrote {written:?} but ALL_VECTORS lists {ALL_VECTORS:?} — the two must agree \
         or a vector escapes the ASCII sweep"
    );
}
