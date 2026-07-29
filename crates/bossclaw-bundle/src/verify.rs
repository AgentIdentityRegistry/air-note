//! Verify an `.airmem`. L1 = self-consistent offline. Fail-closed: the FIRST mismatch is the verdict.
//!
//! THREAT MODEL: this runs in a WASM verifier over a FULLY attacker-supplied document. No panic, no
//! trap, no hang is acceptable — every rejection is a specific typed [`VerifyError`]. Three habits
//! carry that guarantee, and none of them is optional:
//! * **Recompute, never trust.** `items[].leaf` is not covered by any signature (it is blanked
//!   before its own leaf hash) and is therefore freely rewritable with the seal AND root still
//!   green. Verify recomputes every leaf and compares (`ItemHashMismatch`).
//! * **Canonicalize FROM THE PARSED STRUCT.** The received byte stream is never re-signed against
//!   itself; [`crate::format::canonical_json`] re-serializes the struct. That is only safe because
//!   the four hash-feeding types carry `#[serde(deny_unknown_fields)]`, so an injected key fails at
//!   the parse boundary instead of being silently dropped under a valid seal.
//! * **Length-gate before decoding.** `multibase::decode` is O(n²) on base58 (measured: 100k chars
//!   ≈ 1.2s, 400k ≈ 19s). Every attacker-controlled encoded string — the seal, each item signature,
//!   the binding's identity signature, and (in `binding.rs`) the keys — is bounded BEFORE decode.
//!   The export-side `BundleTooLarge` cap does NOT protect this code: it is a daemon build-time
//!   refusal on owner data, not a gate on a received file.
//! * **Bound the WORK, not just the strings.** An attacker mints their own brain key, so every item
//!   in a hostile bundle can be internally valid and force the full per-item cost; [`MAX_ITEMS`] is
//!   the floor under that. It is not the whole answer — the host must also cap raw bytes before
//!   parsing, which belongs at the parse boundary rather than here.
//!
//! CANONICALIZATION SCOPE (a conscious choice, Task-2 review #3): `.airmem` manifest/binding/item
//! canonicalization is **JCS-only — it does NOT NFC-normalize**, unlike
//! `bossclaw_canon::event::canonical_bytes`, which NFC-normalizes before JCS. For SP-V1 + SP-V2
//! that is byte-identical-by-construction because build, native verify, and the WASM verifier are
//! all this one crate, so the two sides can never disagree. It makes `.airmem` **same-crate-verify-
//! only**: a future FOREIGN verifier (the C2PA/VC/SCITT lift, spec §9 non-goal) MUST revisit this,
//! together with the ASCII-only `display`-key constraint enforced in [`check_item_shape`], which
//! exists for the same reason (`serde_jcs 0.1.0` sorts keys by UTF-8 bytes while RFC-8785 §3.2.3
//! mandates UTF-16 code-unit order; they diverge for any non-BMP key, and every emoji is non-BMP).
//!
//! IDENTITY INVARIANT (spec §2.5 C3): **an L1 pass is never identity evidence.** Every key L1
//! touches — the brain key in the manifest, the identity key in the binding — is chosen by whoever
//! produced the file, so a self-consistent bundle proves only internal consistency. That is why the
//! verdict always carries an explicit [`IdentityLevel`] and why the L1-only value is named
//! [`IdentityLevel::UnverifiedOffline`]. No surface may render an L1 pass as "verified identity".

use bossclaw_canon::event::{canonical_bytes, compute_hash, is_external, Event};
use bossclaw_canon::sign::{verify_bytes, verify_hash, VerifyingKey};

use self::semver_lite::major;
use crate::binding::{binding_hash, decode_ed25519_key, verify_binding_internal, MAX_ENCODED_SIG_LEN};
use crate::display::sanitize_for_display;
use crate::format::{Airmem, AirmemItem, ItemClass};
use crate::merkle;
use crate::resolver::IdentityResolver;

/// One failure class per §7. `{i}` arms carry the item index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyError {
    /// The master seal does not verify against `manifest.brain_verifying_key`.
    SealInvalid,
    /// Item `i`'s stamp is bad OR not by `manifest.brain_verifying_key` (A6).
    ItemStampInvalid(usize),
    /// Item `i`'s recorded leaf ≠ recomputed leaf.
    ItemHashMismatch(usize),
    /// The Merkle root recompute ≠ `manifest.merkle_root`.
    TreeMismatch,
    /// The binding's internal identity signature does not verify.
    BindingInvalid,
    /// `binding.payload.brain_verifying_key` ≠ `manifest.brain_verifying_key`.
    BindingKeyMismatch,
    /// `binding.payload.did` ≠ `manifest.did` (C1).
    BindingDidMismatch,
    /// `H(jcs(binding))` ≠ `manifest.binding_hash` (C-NEW-2).
    BindingHashMismatch,
    /// A stamped item's carried origin token disagrees with its recomputed origin (A5).
    OriginMismatch(usize),
    /// L2 only: `manifest.did` did not resolve via the registry (L1 verdict still reported).
    IdentityUnresolved,
    /// `format_version` major is newer than the verifier's.
    FormatTooNew,
    /// Structurally unparseable, a count that disagrees with the array, an item whose field set is
    /// illegal for its class, or an encoded string too long to be a legitimate signature. The
    /// payload is a human-readable detail for the ❌ surface.
    Malformed(String),
}

/// The identity assurance level rendered alongside the L1 verdict. L1 alone = Unverified (C3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityLevel {
    /// L1 only: the did was not resolved, so identity is UNVERIFIED. The bundle is internally
    /// consistent and nothing more — every key it contains was chosen by its producer. This value
    /// must never be rendered as identity evidence (spec §2.5 C3).
    UnverifiedOffline,
    /// L2: the registry published an identity key for `manifest.did` and its RAW 32 key bytes equal
    /// the binding's identity key, so the offline-checkable binding now attributes to a real did.
    RegistryResolved,
}

/// The verdict: L1 ok + an identity level + per-item honest origin labels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    /// One honest origin label per item, in item (= leaf) order.
    pub item_labels: Vec<String>,
    /// How much identity assurance the verdict carries.
    pub identity: IdentityLevel,
}

/// The newest `.airmem` MAJOR this verifier understands.
const SUPPORTED_MAJOR: u64 = 1;

/// Ceiling on items in a RECEIVED bundle. Verify costs ~55 µs per stamped item natively (several×
/// that in WASM), and an attacker mints their OWN brain key, so every item in a hostile bundle can
/// be internally valid and force 100% of the work — an unbounded count is therefore an availability
/// hole, not merely a slow path. Generous enough for a whole-brain Story-C export. The host must
/// ALSO cap raw bytes before parsing; that bound belongs at the parse boundary, not here.
///
/// `pub` so the WRITE side enforces the SAME ceiling: without it an export could build a bundle
/// that AIR's own verifier then refuses as `Malformed` (demonstrated in review with 50,001 items),
/// and a re-typed literal on the build side could drift from the value enforced here.
pub const MAX_ITEMS: usize = 50_000;

/// The only legal binding purpose — a domain-separation tag (spec §2.3), so a card minted for any
/// other identity-signed protocol can never be lifted into a bundle and honored here.
///
/// `pub` (re-exported from the crate root) so the WRITE side shares this exact string: the daemon's
/// `SetBinding` validation refuses a wrong-purpose card at storage time, and a duplicated literal
/// there could drift from the value the verifier enforces at read time.
pub const BINDING_PURPOSE: &str = "memory-signing";

/// The only `kind` a [`ItemClass::Stamped`] item may carry (build stamps exactly this).
const KIND_NOTE: &str = "note";

/// Full L1 verification, then an L2 attempt via `resolver` (offline resolver = stays L1).
///
/// # Errors
/// Returns the FIRST [`VerifyError`] encountered — verification is fail-closed, so one bad byte is
/// the whole verdict. Never panics: every attacker-reachable path returns a typed error.
pub fn verify(bundle: &Airmem, resolver: &dyn IdentityResolver) -> Result<Verdict, VerifyError> {
    let version_major = major(&bundle.manifest.format_version)
        .ok_or_else(|| VerifyError::Malformed("manifest.format_version: not a semver".into()))?;
    if version_major > SUPPORTED_MAJOR {
        return Err(VerifyError::FormatTooNew);
    }

    // Structural pre-checks BEFORE any hashing. `merkle::root` asserts a non-empty leaf slice and
    // `assert!` is NOT compiled out in release, so an empty `items` array would otherwise trap the
    // WASM verifier instead of rendering a clean ❌ (Task-3 review #4).
    if bundle.items.is_empty() {
        return Err(VerifyError::Malformed("items: must not be empty".into()));
    }
    if bundle.items.len() > MAX_ITEMS {
        return Err(VerifyError::Malformed(format!(
            "items: {} entries exceeds the {MAX_ITEMS} ceiling",
            bundle.items.len()
        )));
    }
    if bundle.manifest.item_count != bundle.items.len() as u64 {
        return Err(VerifyError::Malformed(format!(
            "manifest.item_count is {} but items has {} entries",
            bundle.manifest.item_count,
            bundle.items.len()
        )));
    }

    let brain_vk = decode_vk(&bundle.manifest.brain_verifying_key).ok_or_else(|| {
        VerifyError::Malformed("manifest.brain_verifying_key: not an Ed25519 key".into())
    })?;
    if bundle.seal.len() > MAX_ENCODED_SIG_LEN {
        return Err(VerifyError::Malformed(format!(
            "seal: encoded length {} exceeds {MAX_ENCODED_SIG_LEN}",
            bundle.seal.len()
        )));
    }
    let manifest_bytes = crate::format::canonical_json(&bundle.manifest)
        .map_err(|e| VerifyError::Malformed(e.to_string()))?;
    verify_bytes(&manifest_bytes, &bundle.seal, &brain_vk).map_err(|_| VerifyError::SealInvalid)?;

    if hex::encode(binding_hash(&bundle.binding)) != bundle.manifest.binding_hash {
        return Err(VerifyError::BindingHashMismatch);
    }
    // Gate before `verify_binding_internal`, which decodes this string (the key it also decodes is
    // already gated inside `decode_ed25519_key`).
    if bundle.binding.identity_signature.len() > MAX_ENCODED_SIG_LEN {
        return Err(VerifyError::Malformed(format!(
            "binding.identity_signature: encoded length {} exceeds {MAX_ENCODED_SIG_LEN}",
            bundle.binding.identity_signature.len()
        )));
    }
    if !verify_binding_internal(&bundle.binding) {
        return Err(VerifyError::BindingInvalid);
    }
    if bundle.binding.payload.brain_verifying_key != bundle.manifest.brain_verifying_key {
        return Err(VerifyError::BindingKeyMismatch);
    }
    if bundle.binding.payload.did != bundle.manifest.did {
        return Err(VerifyError::BindingDidMismatch);
    }
    // Domain separation. Nothing else in the system validates this tag, so it is inert only for as
    // long as `memory-signing` is the ONLY payload AIR ever asks an identity key to sign. The moment
    // a second one exists with these six fields, an unenforced `purpose` lets an attacker lift that
    // card over their own brain key and reach RegistryResolved — re-attribution (C1) by side door.
    if bundle.binding.payload.purpose != BINDING_PURPOSE {
        // Sanitized where the detail is BUILT, not where it is printed: the producer of a
        // verifying bundle chooses this string, and every surface that quotes it back — the CLI,
        // the app sheet, SP-V2's WASM host — would otherwise have to re-solve the same hole.
        return Err(VerifyError::Malformed(format!(
            "binding.payload.purpose must be \"{BINDING_PURPOSE}\", got \"{}\"",
            sanitize_for_display(&bundle.binding.payload.purpose)
        )));
    }

    let mut leaves = Vec::with_capacity(bundle.items.len());
    let mut item_labels = Vec::with_capacity(bundle.items.len());
    for (i, item) in bundle.items.iter().enumerate() {
        // The shape check runs FIRST, before the leaf recompute: the illegal combinations it
        // rejects are ones the hash and seal cannot see (see `check_item_shape`).
        check_item_shape(i, item)?;
        let leaf = merkle::leaf_hash(item);
        if hex::encode(leaf) != item.leaf {
            return Err(VerifyError::ItemHashMismatch(i));
        }
        leaves.push(leaf);
        item_labels.push(verify_item(i, item, &brain_vk)?);
    }
    if hex::encode(merkle::root(&leaves)) != bundle.manifest.merkle_root {
        return Err(VerifyError::TreeMismatch);
    }

    let identity = match resolver.resolve(&bundle.manifest.did) {
        None => IdentityLevel::UnverifiedOffline,
        Some(registry_key_mb) => {
            // Encoding-agnostic: compare RAW 32-byte keys (registry multikey vs binding bare — #6).
            //
            // DEFERRED (Task-4 review #7): ed25519-dalek 2.2.0's `VerifyingKey::from_bytes` accepts
            // all four small-order points and even non-canonical `[0xff; 32]`, and `verify_bytes`
            // uses the non-strict `Verifier::verify`. That is harmless at L1 by design (the key is
            // attacker-chosen and L1 carries zero identity assurance), and re-verifying the binding
            // with `verify_strict` once the registry has pinned the key by raw bytes would close
            // the class here. It is deferred rather than half-done because it needs
            // `bossclaw-canon` to expose a strict verify (its `Signature` type is private today),
            // and because L2 is inert in SP-V1 — `OfflineResolver` is the only resolver that ships,
            // so no production path reaches this arm until the real HTTPS resolver lands in SP-V2.
            // Do it there, with the resolver.
            let registry_raw = decode_ed25519_key(&registry_key_mb);
            let binding_raw = decode_ed25519_key(&bundle.binding.payload.identity_verifying_key);
            match (registry_raw, binding_raw) {
                (Some(a), Some(b)) if a == b => IdentityLevel::RegistryResolved,
                _ => return Err(VerifyError::IdentityUnresolved),
            }
        }
    };
    Ok(Verdict { item_labels, identity })
}

/// Enforce the `class ↔ field-set` invariant (Task-5 review #1) plus the encoded-length and
/// `display`-charset constraints. The flat [`AirmemItem`] permits combinations the format forbids,
/// and **the cryptographic commitments cannot see them**:
/// * `carried_origin` is Merkle-EXCLUDED by design, so flipping it never breaks a leaf or the root;
/// * a seal-vouched item has no `event_bytes`, so the A5 origin cross-check in [`verify_item`] has
///   no substrate to compare against and simply never runs.
///
/// A reviewer demonstrated the consequence empirically: injecting `carried_origin: Some("external")`
/// into a seal-vouched item leaves BOTH the seal and the root verifying green, painting a weak,
/// export-time-vouched memory with the strong write-time attestation token. The
/// `carried_origin`-absent-on-seal-vouched clause below is the SOLE enforcement point.
fn check_item_shape(i: usize, item: &AirmemItem) -> Result<(), VerifyError> {
    let bad = |detail: &str| VerifyError::Malformed(format!("items[{i}]: {detail}"));
    match item.class {
        ItemClass::Stamped => {
            if item.kind != KIND_NOTE {
                // Sanitized at the build point, for the same reason as `purpose` above.
                return Err(bad(&format!(
                    "class=stamped requires kind=\"{KIND_NOTE}\", got \"{}\"",
                    sanitize_for_display(&item.kind)
                )));
            }
            let Some(signature) = item.signature.as_deref() else {
                return Err(bad("class=stamped requires signature"));
            };
            if item.event_bytes.is_none() {
                return Err(bad("class=stamped requires event_bytes"));
            }
            if item.content.is_some() {
                return Err(bad("class=stamped must not carry content"));
            }
            if item.display.is_some() {
                return Err(bad("class=stamped must not carry display"));
            }
            if signature.len() > MAX_ENCODED_SIG_LEN {
                return Err(bad(&format!(
                    "signature: encoded length {} exceeds {MAX_ENCODED_SIG_LEN}",
                    signature.len()
                )));
            }
        }
        ItemClass::SealVouched => {
            if item.kind == KIND_NOTE {
                return Err(bad("kind=\"note\" requires class=stamped"));
            }
            if item.content.is_none() {
                return Err(bad("class=seal_vouched requires content"));
            }
            if item.event_bytes.is_some() {
                return Err(bad("class=seal_vouched must not carry event_bytes"));
            }
            if item.signature.is_some() {
                return Err(bad("class=seal_vouched must not carry signature"));
            }
            // THE load-bearing clause — see this function's doc comment.
            if item.carried_origin.is_some() {
                return Err(bad("class=seal_vouched must not carry carried_origin"));
            }
            if let Some(display) = &item.display {
                if !object_keys_are_ascii(display) {
                    return Err(bad("display: object keys must be ASCII (JCS key-order conformance)"));
                }
            }
        }
    }
    Ok(())
}

/// True iff every object key at every depth of `value` is ASCII. Recursion depth is bounded by
/// `serde_json`'s own 128-level parse limit (the `unbounded_depth` feature is NOT enabled), so an
/// adversarially nested `display` cannot reach this with a stack-blowing shape.
///
/// `pub` so the BUILD side can guarantee on the write side exactly what this verifier enforces on
/// the read side (`bossclaw_core::log::export_bundle` authors every `display` key). Two private
/// copies of a conformance rule is precisely the drift this constraint exists to prevent.
pub fn object_keys_are_ascii(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(map) => {
            map.iter().all(|(k, v)| k.is_ascii() && object_keys_are_ascii(v))
        }
        serde_json::Value::Array(items) => items.iter().all(object_keys_are_ascii),
        _ => true,
    }
}

/// Verify one item and return its honest origin label (§3-H2). Stamped: cross-check carried vs
/// derived (OriginMismatch) + pinned copy. Seal-vouched: KIND-AWARE label (never cross-rendered).
fn verify_item(i: usize, item: &AirmemItem, brain_vk: &VerifyingKey) -> Result<String, VerifyError> {
    match item.class {
        ItemClass::Stamped => {
            // `check_item_shape` already proved both are present; the Option handling here is what
            // keeps that a compile-time guarantee rather than an `unwrap` waiting for a refactor.
            let bytes = item.event_bytes.as_ref().ok_or(VerifyError::ItemStampInvalid(i))?;
            let sig = item.signature.as_ref().ok_or(VerifyError::ItemStampInvalid(i))?;
            let ev: Event = serde_json::from_str(bytes).map_err(|_| VerifyError::ItemStampInvalid(i))?;
            let recanon = canonical_bytes(&ev).map_err(|_| VerifyError::ItemStampInvalid(i))?;
            if recanon != bytes.as_bytes() {
                return Err(VerifyError::ItemStampInvalid(i));
            }
            let hash = compute_hash(&ev).map_err(|_| VerifyError::ItemStampInvalid(i))?;
            verify_hash(&hash, sig, brain_vk).map_err(|_| VerifyError::ItemStampInvalid(i))?;
            let external = is_external(&ev);
            let derived = if external { "external" } else { "unattested" };
            // A5: any carried display token is cross-checked against the recomputed origin.
            if let Some(carried) = item.carried_origin.as_deref() {
                if carried != derived {
                    return Err(VerifyError::OriginMismatch(i));
                }
            }
            Ok(if external {
                "this brain recorded these bytes; provenance of the underlying text is not asserted".into()
            } else {
                "origin unattested".into() // is_external=false ∧ kind≠dossier (C-NEW-1b)
            })
        }
        ItemClass::SealVouched => Ok(seal_vouched_label(&item.kind)),
    }
}

/// KIND-AWARE seal-vouched label (spec §3-H2 amended). The dossier phrasing NEVER renders on a
/// session/ingest ("machine-derived" would be false there — plan review C5). The catch-all keeps a
/// minor-version-added kind honest rather than mislabelled; `check_item_shape` has already
/// guaranteed `kind != "note"` here.
fn seal_vouched_label(kind: &str) -> String {
    match kind {
        "session" => "captured session, content only; not independently verified",
        "ingest" => "ingested file extract; not independently verified",
        "dossier" => "machine-derived by the exporter; not independently verified",
        _ => "exporter-vouched; not independently verified",
    }
    .to_string()
}

fn decode_vk(mb: &str) -> Option<VerifyingKey> {
    VerifyingKey::from_bytes(&decode_ed25519_key(mb)?).ok()
}

/// Minimal in-module major-version parse (avoids an external `semver` dep). `"1.2.3" → Some(1)`.
mod semver_lite {
    pub fn major(v: &str) -> Option<u64> {
        v.split('.').next()?.parse().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bossclaw_canon::event::{canonical_bytes, compute_hash, Event};
    use bossclaw_canon::sign::{sign_bytes, sign_hash, SigningKey};
    use crate::binding::binding_signing_bytes;
    use crate::build::{build_bundle, BuildInput, ItemInput};
    use crate::format::{Binding, BindingPayload};
    use crate::resolver::{MockResolver, OfflineResolver};

    const BRAIN: [u8; 32] = [1u8; 32];

    fn note_event(text: &str, external: bool) -> Event {
        let content = if external {
            serde_json::json!({ "text": text, "origin": "external" })
        } else {
            serde_json::json!({ "text": text })
        };
        Event {
            id: "01J0000000000000000000000A".into(), ts: "2026-06-15T00:00:00Z".into(),
            valid_time: None, event_type: "memory".into(), content, model_meta: None,
            prev_hash: "00".repeat(32), hash: None, signed_by_did: "did:wba:example.com:me".into(),
            signature: None,
        }
    }

    fn valid_bundle() -> Airmem {
        let brain = SigningKey::from_bytes(&BRAIN);
        let brain_mb = multibase::encode(multibase::Base::Base58Btc, brain.verifying_key().to_bytes());
        let ev = note_event("shared note", true);
        let event_bytes = String::from_utf8(canonical_bytes(&ev).unwrap()).unwrap();
        let sig = sign_hash(&compute_hash(&ev).unwrap(), &brain);
        let idk = SigningKey::from_bytes(&[9u8; 32]);
        let idvk = multibase::encode(multibase::Base::Base58Btc, idk.verifying_key().to_bytes());
        let payload = BindingPayload {
            brain_verifying_key: brain_mb.clone(), identity_verifying_key: idvk,
            did: "did:wba:example.com:me".into(), purpose: "memory-signing".into(), epoch: 1,
            created_at: "2026-07-21T00:00:00Z".into(),
        };
        let bsig = sign_bytes(&binding_signing_bytes(&payload), &idk);
        build_bundle(BuildInput {
            created_at: "2026-07-21T00:00:00Z".into(),
            did: "did:wba:example.com:me".into(), brain_verifying_key: brain_mb,
            selection_description: "1 note + 1 session".into(),
            items: vec![
                ItemInput::Stamped { event_bytes, signature: sig, carried_origin: "external".into() },
                ItemInput::SealVouched { kind: "session".into(), content: "session body".into(),
                    display: Some(serde_json::json!({"title":"S","project":"repo"})) },
            ],
            binding: Binding { payload, identity_signature: bsig }, brain_key: &brain,
        })
    }

    /// Re-sign the manifest as-is (for tests that tamper with a manifest field on purpose).
    fn reseal(b: &mut Airmem) {
        let brain = SigningKey::from_bytes(&BRAIN);
        b.seal = sign_bytes(&crate::format::canonical_json(&b.manifest).unwrap(), &brain);
    }

    /// Re-leaf + re-root + re-count + re-seal, so EVERY cryptographic commitment is green again.
    /// Tests that use this are proving a check other than the hashes/seal is what rejects.
    fn reindex(b: &mut Airmem) {
        for i in 0..b.items.len() {
            b.items[i].leaf = hex::encode(merkle::leaf_hash(&b.items[i]));
        }
        let leaves: Vec<[u8; 32]> = b.items.iter().map(merkle::leaf_hash).collect();
        b.manifest.merkle_root = hex::encode(merkle::root(&leaves));
        b.manifest.item_count = b.items.len() as u64;
        reseal(b);
    }

    fn off() -> OfflineResolver { OfflineResolver }

    /// A seal-vouched item of any kind, un-leafed (call `reindex` after pushing).
    fn seal_vouched(kind: &str, content: &str) -> AirmemItem {
        AirmemItem {
            leaf: String::new(), class: ItemClass::SealVouched, kind: kind.into(),
            event_bytes: None, signature: None, carried_origin: None,
            content: Some(content.into()), display: None,
        }
    }

    fn malformed_detail(e: &VerifyError) -> &str {
        match e {
            VerifyError::Malformed(d) => d,
            other => panic!("expected Malformed, got {other:?}"),
        }
    }

    #[test]
    fn valid_bundle_verifies_green_offline() {
        let v = verify(&valid_bundle(), &off()).expect("valid bundle verifies");
        assert_eq!(v.identity, IdentityLevel::UnverifiedOffline);
        assert!(v.item_labels[0].contains("provenance of the underlying text is not asserted"));
        assert_eq!(v.item_labels[1], "captured session, content only; not independently verified");
    }

    // ---- TAMPER MATRIX ----
    #[test] fn flip_seal() { let mut b = valid_bundle(); b.seal = "zBAD".into();
        assert_eq!(verify(&b, &off()), Err(VerifyError::SealInvalid)); }
    #[test] fn flip_bare_manifest_field_without_reseal() { let mut b = valid_bundle();
        b.manifest.selection_description = "tampered".into(); // any manifest byte → seal fails
        assert_eq!(verify(&b, &off()), Err(VerifyError::SealInvalid)); }
    #[test] fn flip_item_content_stamped() { let mut b = valid_bundle();
        b.items[0].event_bytes = Some(b.items[0].event_bytes.clone().unwrap().replace("shared", "forged"));
        assert_eq!(verify(&b, &off()), Err(VerifyError::ItemHashMismatch(0))); } // content is in the leaf
    #[test] fn flip_item_ts_stamped() { let mut b = valid_bundle();
        b.items[0].event_bytes = Some(b.items[0].event_bytes.clone().unwrap().replace("2026-06-15", "2020-01-01"));
        assert_eq!(verify(&b, &off()), Err(VerifyError::ItemHashMismatch(0))); }
    #[test] fn flip_item_leaf_field() { let mut b = valid_bundle(); b.items[0].leaf = "00".repeat(32);
        assert_eq!(verify(&b, &off()), Err(VerifyError::ItemHashMismatch(0))); }
    #[test] fn flip_original_signature_releafed_resealed() {
        // Flip the signature, then re-leaf + re-root + re-seal so the hash/tree/seal checks PASS and the
        // STAMP check is what fails → ItemStampInvalid (critic #8).
        let mut b = valid_bundle();
        b.items[0].signature = Some(sign_hash(&[0u8; 32], &SigningKey::from_bytes(&[2u8; 32])));
        reindex(&mut b);
        assert_eq!(verify(&b, &off()), Err(VerifyError::ItemStampInvalid(0)));
    }
    #[test] fn flip_merkle_root() { let mut b = valid_bundle(); b.manifest.merkle_root = "00".repeat(32);
        reseal(&mut b); assert_eq!(verify(&b, &off()), Err(VerifyError::TreeMismatch)); }
    #[test] fn flip_binding_hash() { let mut b = valid_bundle(); b.manifest.binding_hash = "00".repeat(32);
        reseal(&mut b); assert_eq!(verify(&b, &off()), Err(VerifyError::BindingHashMismatch)); }
    #[test] fn flip_manifest_did_reaches_binding_did_mismatch() { let mut b = valid_bundle();
        b.manifest.did = "did:wba:evil.com:x".into(); reseal(&mut b);
        assert_eq!(verify(&b, &off()), Err(VerifyError::BindingDidMismatch)); }
    #[test] fn format_too_new() { let mut b = valid_bundle(); b.manifest.format_version = "2.0.0".into();
        reseal(&mut b); assert_eq!(verify(&b, &off()), Err(VerifyError::FormatTooNew)); }
    #[test] fn format_version_garbage_is_malformed() { let mut b = valid_bundle();
        b.manifest.format_version = "not-a-version".into(); reseal(&mut b);
        assert!(malformed_detail(&verify(&b, &off()).unwrap_err()).contains("format_version")); }

    // ---- BindingKeyMismatch: change manifest brain key + re-seal under a SECOND brain key ----
    #[test]
    fn binding_key_mismatch() {
        let mut b = valid_bundle();
        let brain2 = SigningKey::from_bytes(&[8u8; 32]);
        b.manifest.brain_verifying_key = multibase::encode(multibase::Base::Base58Btc, brain2.verifying_key().to_bytes());
        // The binding still carries the OLD brain key, so binding_hash is unchanged; set it to the
        // (unchanged) value + re-seal under brain2 so seal + binding_hash pass, and the
        // binding.brain_key ≠ manifest.brain_key check is what fails.
        b.manifest.binding_hash = hex::encode(crate::binding::binding_hash(&b.binding));
        b.seal = sign_bytes(&crate::format::canonical_json(&b.manifest).unwrap(), &brain2);
        assert_eq!(verify(&b, &off()), Err(VerifyError::BindingKeyMismatch));
    }

    // ---- BindingInvalid: self-inconsistent card (payload tampered post-sign) + recomputed hash + reseal ----
    #[test]
    fn binding_invalid() {
        let mut b = valid_bundle();
        b.binding.payload.created_at = "1999-01-01T00:00:00Z".into(); // signature no longer covers it
        b.manifest.binding_hash = hex::encode(crate::binding::binding_hash(&b.binding));
        reseal(&mut b);
        assert_eq!(verify(&b, &off()), Err(VerifyError::BindingInvalid));
    }

    // ---- DOMAIN SEPARATION: a card minted for a DIFFERENT identity-signed protocol is refused ----
    #[test]
    fn wrong_binding_purpose_is_malformed() {
        let mut b = valid_bundle();
        let idk = SigningKey::from_bytes(&[9u8; 32]); // the identity key valid_bundle() used
        b.binding.payload.purpose = "did-control-challenge".into();
        // Re-sign the card and re-commit it, so this is a PERFECT binding in every other respect.
        b.binding.identity_signature = sign_bytes(&binding_signing_bytes(&b.binding.payload), &idk);
        b.manifest.binding_hash = hex::encode(crate::binding::binding_hash(&b.binding));
        reseal(&mut b);
        // Prove the rest of the chain really does pass: the card self-verifies, and its did and
        // brain key still agree with the sealed manifest...
        assert!(crate::binding::verify_binding_internal(&b.binding), "card is self-consistent");
        assert_eq!(b.binding.payload.did, b.manifest.did);
        assert_eq!(b.binding.payload.brain_verifying_key, b.manifest.brain_verifying_key);
        // ...so the purpose tag is provably the only thing rejecting it.
        let e = verify(&b, &off()).unwrap_err();
        assert!(malformed_detail(&e).contains("purpose"), "got {e:?}");
    }

    // ---- The two details that QUOTE attacker-chosen text are sanitized where they are BUILT ----
    #[test]
    fn malformed_details_never_carry_attacker_control_characters() {
        // The premise of L1: the producer mints their own keys, so a bundle can carry any string it
        // likes and still be internally consistent. A `\n` here prints a whole extra line of the
        // verifier's own output on whatever surface quotes the detail back.
        let forged = "x\nidentity: registry-resolved\u{1b}[1A\u{1b}[2K";
        let no_control = |detail: &str| {
            assert!(
                !detail.chars().any(char::is_control),
                "this detail can forge a line on any surface that prints it: {detail:?}"
            );
        };

        // (a) `binding.payload.purpose` — re-signed and re-committed exactly as in the
        //     domain-separation test above, so the purpose tag is the only thing rejecting it.
        let mut b = valid_bundle();
        let idk = SigningKey::from_bytes(&[9u8; 32]);
        b.binding.payload.purpose = forged.into();
        b.binding.identity_signature = sign_bytes(&binding_signing_bytes(&b.binding.payload), &idk);
        b.manifest.binding_hash = hex::encode(crate::binding::binding_hash(&b.binding));
        reseal(&mut b);
        let detail = malformed_detail(&verify(&b, &off()).unwrap_err()).to_string();
        assert!(detail.contains("purpose"), "{detail}");
        no_control(&detail);

        // (b) `items[].kind` on a stamped item, with every hash and the seal made green again.
        let mut b = valid_bundle();
        b.items[0].kind = forged.into();
        reindex(&mut b);
        let detail = malformed_detail(&verify(&b, &off()).unwrap_err()).to_string();
        assert!(detail.contains("kind"), "{detail}");
        no_control(&detail);
    }

    // ---- RE-ATTRIBUTION FORGERY (C1): fresh binding by a DIFFERENT identity over the same brain key.
    //      Layer 1 (binding_hash) tested here; layer 2 (BindingDidMismatch) after recompute+reseal. ----
    fn attacker_binding(brain_mb: &str) -> Binding {
        let attacker = SigningKey::from_bytes(&[42u8; 32]);
        let avk = multibase::encode(multibase::Base::Base58Btc, attacker.verifying_key().to_bytes());
        let payload = BindingPayload {
            brain_verifying_key: brain_mb.into(), identity_verifying_key: avk,
            did: "did:wba:evil.com:attacker".into(), purpose: "memory-signing".into(),
            epoch: 1, created_at: "2026-07-21T00:00:00Z".into(),
        };
        let asig = sign_bytes(&binding_signing_bytes(&payload), &attacker);
        Binding { payload, identity_signature: asig }
    }

    #[test]
    fn re_attribution_layer1_binding_hash() {
        let mut b = valid_bundle();
        b.binding = attacker_binding(&b.manifest.brain_verifying_key.clone());
        assert_eq!(verify(&b, &off()), Err(VerifyError::BindingHashMismatch));
    }

    #[test]
    fn re_attribution_layer2_binding_did() {
        let mut b = valid_bundle();
        b.binding = attacker_binding(&b.manifest.brain_verifying_key.clone());
        b.manifest.binding_hash = hex::encode(crate::binding::binding_hash(&b.binding));
        reseal(&mut b);
        assert_eq!(verify(&b, &off()), Err(VerifyError::BindingDidMismatch)); // did ≠ sealed manifest.did
    }

    // ---- FOREIGN-EVENT LAUNDERING (A6): stamped item validly signed by a DIFFERENT brain key. ----
    #[test]
    fn foreign_event_laundering() {
        let foreign = SigningKey::from_bytes(&[77u8; 32]);
        let mut ev = note_event("foreign", true); ev.id = "01J000000000000000000000FF".into();
        let event_bytes = String::from_utf8(canonical_bytes(&ev).unwrap()).unwrap();
        let fsig = sign_hash(&compute_hash(&ev).unwrap(), &foreign);
        let mut b = valid_bundle();
        b.items[0] = AirmemItem { leaf: String::new(), class: ItemClass::Stamped, kind: "note".into(),
            event_bytes: Some(event_bytes), signature: Some(fsig), carried_origin: Some("external".into()),
            content: None, display: None };
        reindex(&mut b);
        assert_eq!(verify(&b, &off()), Err(VerifyError::ItemStampInvalid(0)));
    }

    // ---- EVENT-BYTES CANONICALITY: the disclosed bytes must be CANONICAL, not merely equivalent.
    //      Both cases sign honestly over the canonical hash, so `verify_hash` PASSES and the
    //      `recanon != bytes` check is the only thing that can reject. ----

    /// Swap in a stamped item built from raw `event_bytes` + a signature over its canonical hash.
    fn bundle_with_raw_event_bytes(event_bytes: String) -> Airmem {
        let brain = SigningKey::from_bytes(&BRAIN);
        let parsed: Event = serde_json::from_str(&event_bytes).expect("fixture is valid JSON");
        let sig = sign_hash(&compute_hash(&parsed).unwrap(), &brain);
        let mut b = valid_bundle();
        b.items[0] = AirmemItem {
            leaf: String::new(), class: ItemClass::Stamped, kind: "note".into(),
            event_bytes: Some(event_bytes), signature: Some(sig),
            carried_origin: Some("external".into()), content: None, display: None,
        };
        reindex(&mut b);
        b
    }

    #[test]
    fn non_canonical_event_bytes_are_rejected() {
        // Pretty-printed: valid JSON, identical fields, honest signature — but not JCS. Accepting
        // "equivalent" bytes would open a parser-differential class, e.g. duplicate keys inside the
        // free-form `content` value, which serde silently resolves last-wins.
        let ev = note_event("shared note", true);
        let event_bytes = serde_json::to_string_pretty(&ev).unwrap();
        assert_ne!(
            event_bytes.as_bytes(), canonical_bytes(&ev).unwrap().as_slice(),
            "fixture must actually be non-canonical, or the test proves nothing"
        );
        let b = bundle_with_raw_event_bytes(event_bytes);
        assert_eq!(verify(&b, &off()), Err(VerifyError::ItemStampInvalid(0)));
    }

    #[test]
    fn event_bytes_with_an_embedded_hash_field_are_rejected() {
        // `canonical_bytes` STRIPS `hash` (and `signature`) before hashing, so bytes carrying one
        // can never round-trip. Note the hash the signature covers is unchanged by the extra field,
        // which is exactly why the stamp check alone would wave this through.
        let ev = note_event("shared note", true);
        let mut v: serde_json::Value = serde_json::from_slice(&canonical_bytes(&ev).unwrap()).unwrap();
        v.as_object_mut().unwrap().insert("hash".into(), serde_json::json!("11".repeat(32)));
        let b = bundle_with_raw_event_bytes(serde_json::to_string(&v).unwrap());
        assert_eq!(verify(&b, &off()), Err(VerifyError::ItemStampInvalid(0)));
    }

    // ---- EXPORTER-LIED-ORIGIN (A5): carried_origin disagrees with the signed bytes → OriginMismatch.
    //      carried_origin is leaf-EXCLUDED, so a lone flip needs no re-seal. ----
    #[test]
    fn exporter_lied_origin() {
        let mut b = valid_bundle();
        b.items[0].carried_origin = Some("unattested".into()); // bytes say external
        assert_eq!(verify(&b, &off()), Err(VerifyError::OriginMismatch(0)));
    }

    // ---- C-NEW-1b: an origin-LESS stamped note renders EXACTLY "origin unattested", never brain-authored. ----
    #[test]
    fn origin_less_note_renders_unattested() {
        let brain = SigningKey::from_bytes(&BRAIN);
        let ev = note_event("owner note", false); // NO origin stamp
        let event_bytes = String::from_utf8(canonical_bytes(&ev).unwrap()).unwrap();
        let sig = sign_hash(&compute_hash(&ev).unwrap(), &brain);
        let mut b = valid_bundle();
        b.items[0] = AirmemItem { leaf: String::new(), class: ItemClass::Stamped, kind: "note".into(),
            event_bytes: Some(event_bytes), signature: Some(sig), carried_origin: Some("unattested".into()),
            content: None, display: None };
        reindex(&mut b);
        let v = verify(&b, &off()).unwrap();
        assert_eq!(v.item_labels[0], "origin unattested");
    }

    // ---- CLASS ↔ FIELD-SET INVARIANT (Task-5 review #1) ----

    #[test]
    fn seal_vouched_carrying_carried_origin_is_malformed() {
        // THE PROVEN ATTACK. items[1] is the seal-vouched session; painting it with the strong
        // write-time attestation token leaves EVERY cryptographic commitment green, because
        // `carried_origin` is Merkle-excluded and a seal-vouched item has no `event_bytes` for the
        // A5 cross-check to work from. This clause is the sole enforcement point.
        let mut b = valid_bundle();
        b.items[1].carried_origin = Some("external".into());
        // Prove the commitments really are untouched...
        assert_eq!(b.items[1].leaf, hex::encode(merkle::leaf_hash(&b.items[1])), "leaf still matches");
        let leaves: Vec<[u8; 32]> = b.items.iter().map(merkle::leaf_hash).collect();
        assert_eq!(b.manifest.merkle_root, hex::encode(merkle::root(&leaves)), "root still matches");
        let brain_vk = SigningKey::from_bytes(&BRAIN).verifying_key();
        verify_bytes(&crate::format::canonical_json(&b.manifest).unwrap(), &b.seal, &brain_vk)
            .expect("seal still verifies");
        // ...and that the shape invariant is what stops it.
        let d = verify(&b, &off()).unwrap_err();
        assert!(malformed_detail(&d).contains("carried_origin"), "got {d:?}");
    }

    #[test]
    fn illegal_class_field_combinations_are_malformed() {
        // Every row re-indexes first, so the seal, the leaves and the root are ALL green and the
        // class↔field-set invariant is provably the only thing rejecting.
        type Mutate = fn(&mut Airmem);
        let rows: &[(&str, Mutate, &str)] = &[
            ("stamped without event_bytes", |b| b.items[0].event_bytes = None, "event_bytes"),
            ("stamped without signature", |b| b.items[0].signature = None, "signature"),
            ("stamped carrying content", |b| b.items[0].content = Some("leak".into()), "content"),
            ("stamped carrying display", |b| b.items[0].display = Some(serde_json::json!({"a": 1})), "display"),
            ("stamped with a non-note kind", |b| b.items[0].kind = "session".into(), "kind"),
            ("seal_vouched carrying event_bytes", |b| b.items[1].event_bytes = Some("{}".into()), "event_bytes"),
            ("seal_vouched carrying signature", |b| b.items[1].signature = Some("zSig".into()), "signature"),
            ("seal_vouched without content", |b| b.items[1].content = None, "content"),
            ("seal_vouched with kind=note", |b| b.items[1].kind = "note".into(), "kind"),
        ];
        for (name, mutate, needle) in rows {
            let mut b = valid_bundle();
            mutate(&mut b);
            reindex(&mut b);
            let e = verify(&b, &off()).unwrap_err();
            assert!(malformed_detail(&e).contains(needle), "{name}: expected `{needle}` in {e:?}");
        }
    }

    #[test]
    fn empty_items_is_malformed_not_a_panic() {
        // `merkle::root` asserts on an empty slice and `assert!` survives release, so reaching it
        // with attacker data would trap the WASM verifier instead of rendering ❌.
        let mut b = valid_bundle();
        b.items.clear();
        b.manifest.item_count = 0;
        reseal(&mut b); // cannot reindex: that would call merkle::root on an empty slice
        let e = verify(&b, &off()).unwrap_err();
        assert!(malformed_detail(&e).contains("empty"), "got {e:?}");
    }

    #[test]
    fn too_many_items_is_malformed() {
        // The attacker mints their own brain key, so an oversized bundle can be entirely valid and
        // force 100% of the per-item work. Deliberately NOT reindexed: the ceiling has to reject
        // before any hashing happens, so this bundle's leaves and root are stale and never reached.
        let mut b = valid_bundle();
        b.items = vec![seal_vouched("session", "x"); MAX_ITEMS + 1];
        b.manifest.item_count = b.items.len() as u64; // so the count check cannot be what fires
        let e = verify(&b, &off()).unwrap_err();
        let detail = malformed_detail(&e);
        assert!(detail.contains(&MAX_ITEMS.to_string()), "should name the ceiling: {e:?}");
        assert!(detail.contains(&(MAX_ITEMS + 1).to_string()), "should name the count: {e:?}");
    }

    #[test]
    fn item_count_disagreeing_with_the_array_is_malformed() {
        let mut b = valid_bundle();
        b.manifest.item_count = 99;
        reseal(&mut b);
        let e = verify(&b, &off()).unwrap_err();
        assert!(malformed_detail(&e).contains("item_count"), "got {e:?}");
    }

    // ---- FOREIGN-VERIFIER CONFORMANCE: non-ASCII `display` keys are refused (Task-3 review #5) ----
    #[test]
    fn non_ascii_display_keys_are_malformed_at_any_depth() {
        for display in [
            serde_json::json!({"🎉": "party"}),                 // top level, non-BMP
            serde_json::json!({"outer": {"café": 1}}),          // nested
            serde_json::json!({"outer": [{"café": 1}]}),        // through an array
        ] {
            let mut b = valid_bundle();
            b.items[1].display = Some(display);
            reindex(&mut b);
            let e = verify(&b, &off()).unwrap_err();
            assert!(malformed_detail(&e).contains("display"), "got {e:?}");
        }
        // ...and ASCII keys with non-ASCII VALUES stay legal (keys only).
        let mut ok = valid_bundle();
        ok.items[1].display = Some(serde_json::json!({"title": "café 🎉"}));
        reindex(&mut ok);
        verify(&ok, &off()).expect("non-ASCII display VALUES are fine");
    }

    // ---- DoS: every attacker-controlled encoded string is length-gated BEFORE decode ----
    #[test]
    fn oversized_encoded_strings_are_refused_before_the_quadratic_decode() {
        let huge = format!("z{}", "Z".repeat(200_000));

        let mut seal = valid_bundle();
        seal.seal = huge.clone();
        assert!(malformed_detail(&verify(&seal, &off()).unwrap_err()).contains("seal"));

        let mut item_sig = valid_bundle();
        item_sig.items[0].signature = Some(huge.clone());
        reindex(&mut item_sig);
        assert!(malformed_detail(&verify(&item_sig, &off()).unwrap_err()).contains("signature"));

        let mut binding_sig = valid_bundle();
        binding_sig.binding.identity_signature = huge;
        binding_sig.manifest.binding_hash = hex::encode(crate::binding::binding_hash(&binding_sig.binding));
        reseal(&mut binding_sig);
        assert!(malformed_detail(&verify(&binding_sig, &off()).unwrap_err()).contains("identity_signature"));

        // Regression guard on the cap being too tight: a real seal + a real stamp still fit.
        let good = valid_bundle();
        assert!(good.seal.len() <= MAX_ENCODED_SIG_LEN, "real seal is {} chars", good.seal.len());
        assert!(good.binding.identity_signature.len() <= MAX_ENCODED_SIG_LEN);
        assert!(good.items[0].signature.as_ref().unwrap().len() <= MAX_ENCODED_SIG_LEN);
    }

    // ---- SEAL-VOUCHED NO-LEAK (A-N1 + A4/C5) — covers BOTH session AND ingest classes (NEW-B). ----
    #[test]
    fn seal_vouched_discloses_no_local_metadata() {
        let mut b = valid_bundle(); // items[0]=stamped note, items[1]=session
        // Add an ingest seal-vouched item (content-only; the item shape has NO provenance field —
        // the real file_ingested provenance block is stripped by core, proven end-to-end in Task 11).
        b.items.push(seal_vouched("ingest", "extracted file text"));
        reindex(&mut b);
        verify(&b, &off()).expect("still verifies with the ingest item");
        let whole = serde_json::to_string(&b).unwrap();
        for needle in ["source_event_ids", "prompt_hash", "session_id", "grant_root", "canonical_path", "content_hash"] {
            assert!(!whole.contains(needle), "seal-vouched item leaked `{needle}`");
        }
        assert!(b.items[1].event_bytes.is_none() && b.items[2].event_bytes.is_none());
    }

    // ---- KIND-AWARE LABELS (§3-H2): each kind gets its own honest phrasing, never cross-rendered ----
    #[test]
    fn seal_vouched_labels_are_kind_aware() {
        let mut b = valid_bundle(); // items[1] is the session
        b.items.push(seal_vouched("ingest", "extracted file text"));
        b.items.push(seal_vouched("dossier", "derived summary"));
        b.items.push(seal_vouched("horoscope", "a kind this verifier predates"));
        reindex(&mut b);
        let v = verify(&b, &off()).expect("verifies");
        assert_eq!(v.item_labels[1], "captured session, content only; not independently verified");
        assert_eq!(v.item_labels[2], "ingested file extract; not independently verified");
        assert_eq!(v.item_labels[3], "machine-derived by the exporter; not independently verified");
        assert_eq!(v.item_labels[4], "exporter-vouched; not independently verified");
        // The dossier phrasing must NEVER render on a session or an ingest (plan review C5).
        assert!(!v.item_labels[1].contains("machine-derived"));
        assert!(!v.item_labels[2].contains("machine-derived"));
    }

    // ---- L2 (mocked registry) — proves the RegistryResolved branch + key-mismatch. ----
    #[test]
    fn l2_registry_resolved_when_key_matches() {
        let b = valid_bundle();
        let r = MockResolver { did: b.manifest.did.clone(), key: b.binding.payload.identity_verifying_key.clone() };
        assert_eq!(verify(&b, &r).unwrap().identity, IdentityLevel::RegistryResolved);
    }
    #[test]
    fn l2_registry_resolved_across_encodings() {
        // Registry publishes the 0xed01 multikey form; the binding stores bare — must STILL match (#6).
        let b = valid_bundle();
        let bare = decode_ed25519_key(&b.binding.payload.identity_verifying_key).unwrap();
        let mut multikey = vec![0xed, 0x01]; multikey.extend_from_slice(&bare);
        let mk = multibase::encode(multibase::Base::Base58Btc, &multikey);
        let r = MockResolver { did: b.manifest.did.clone(), key: mk };
        assert_eq!(verify(&b, &r).unwrap().identity, IdentityLevel::RegistryResolved);
    }
    #[test]
    fn l2_unresolved_when_registry_key_differs() {
        let b = valid_bundle();
        let other = multibase::encode(multibase::Base::Base58Btc, [0u8; 32]);
        let r = MockResolver { did: b.manifest.did.clone(), key: other };
        assert_eq!(verify(&b, &r), Err(VerifyError::IdentityUnresolved));
    }
    #[test]
    fn l2_stays_l1_when_the_registry_does_not_know_the_did() {
        // A resolver that answers for SOMEONE ELSE must not silently upgrade this bundle.
        let b = valid_bundle();
        let r = MockResolver { did: "did:wba:other.com:someone".into(), key: "zWhatever".into() };
        assert_eq!(verify(&b, &r).unwrap().identity, IdentityLevel::UnverifiedOffline);
    }
}
