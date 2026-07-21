# Rung 5 — SP-V1: the export side (bossclaw-canon + bossclaw-bundle + SetBinding/ExportBundle + air-verify CLI + app export UI)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the owner select memories and export a self-contained, brain-sealed, offline-verifiable `.airmem` bundle (identity-bound via a signed attestation), with a Rust library + `air-verify` CLI that render an L1 self-consistency verdict and honest per-item provenance labels.

**Architecture:** A new wasm-clean leaf crate `bossclaw-canon` is extracted from `bossclaw-core` (zero behavior change) carrying the canonical-bytes/hash/sign primitives so the verifier can reproduce the engine's exact bytes without pulling the engine's non-wasm deps; a second leaf crate `bossclaw-bundle` (canon-only) builds and verifies the `.airmem` format (canonical JSON, domain-separated Merkle tree, master seal, identity binding). The daemon gains three App-only wire ops (`BrainVerifyingKey`, `SetBinding`, `ExportBundle`) that store the app-minted binding as a signed config event and produce sealed bundles pure-read; a `did:wba`-signed binding is minted app-side and the Library gains a multi-select export + disclosure review sheet.

**Tech Stack:** Rust (workspace crates, `edition 2021`, `#![forbid(unsafe_code)]`), `ed25519-dalek`/`serde_jcs`/`unicode-normalization`/`sha2`/`multibase` (verify side RNG-free), Tauri v2 + `tauri-plugin-dialog`, TypeScript (strict) + React + Vitest, CSS design tokens only.

### File Structure

Created:
- `crates/bossclaw-canon/Cargo.toml` — leaf crate manifest; consensus-critical exact-pinned crypto/canon deps (C-NEW-3).
- `crates/bossclaw-canon/src/lib.rs` — crate root; re-exports `event`, `sign`, `EXTERNAL_ORIGIN`, `is_external`, ed25519 types, `CanonError`.
- `crates/bossclaw-canon/src/error.rs` — `CanonError` (the 4 variants `event`/`sign` need).
- `crates/bossclaw-canon/src/event.rs` — moved: `Event`, `ModelMeta`, `canonical_bytes`, `compute_hash`, `EXTERNAL_ORIGIN`, `is_external`.
- `crates/bossclaw-canon/src/sign.rs` — moved: `sign_hash`/`verify_hash`; added general `sign_bytes`/`verify_bytes`.
- `crates/bossclaw-canon/tests/vectors.rs` — known-answer + cross-version regression vectors (C-NEW-3).
- `crates/bossclaw-bundle/Cargo.toml` — leaf crate manifest; depends on `bossclaw-canon` ONLY.
- `crates/bossclaw-bundle/src/lib.rs` — crate root; re-exports the public build/verify API + types.
- `crates/bossclaw-bundle/src/format.rs` — `.airmem` types (`Airmem`, `Manifest`, `AirmemItem`/`ItemClass`, `Binding`, `BindingPayload`) + canonical JSON helpers.
- `crates/bossclaw-bundle/src/merkle.rs` — domain-separated Merkle leaf/root (finding A7).
- `crates/bossclaw-bundle/src/binding.rs` — binding canonical form, `binding_hash`, internal-consistency verify, `binding_signing_bytes`.
- `crates/bossclaw-bundle/src/build.rs` — `build_bundle(...)`: assemble items → Merkle root → binding_hash → master seal.
- `crates/bossclaw-bundle/src/verify.rs` — `verify(&Airmem, &dyn IdentityResolver) → Verdict`; L1 checklist + `VerifyError` enum (§7) + L2 hook.
- `crates/bossclaw-bundle/src/resolver.rs` — `IdentityResolver` trait + `OfflineResolver` + `MockResolver` (tests).
- `crates/air-verify/Cargo.toml` — native CLI crate manifest (zero external deps beyond bundle).
- `crates/air-verify/src/main.rs` — `air-verify <file> [--offline]` argument handling + verdict rendering + exit codes.
- `crates/air-verify/tests/clean_machine.rs` — clean-HOME e2e (no keys) over committed fixtures.
- `tests/vectors/` (repo root) — committed `.airmem` conformance fixtures (valid + each tamper class) for SP-V2 cross-repo CI.
- `apps/desktop/src/memory/ExportReviewSheet.tsx` — the §6 disclosure review sheet.
- `apps/desktop/src/memory/ExportReviewSheet.test.tsx` — vitest for the review sheet.

Modified:
- `Cargo.toml` (workspace) — add `crates/bossclaw-canon`, `crates/bossclaw-bundle`, `crates/air-verify` members.
- `crates/bossclaw-core/Cargo.toml` — add `bossclaw-canon` + `bossclaw-bundle` path deps.
- `crates/bossclaw-core/src/lib.rs` — re-export canon modules (`pub use bossclaw_canon::{event, sign};`); keep public surface identical.
- `crates/bossclaw-core/src/error.rs` — `impl From<CanonError> for BossclawError` (byte-identical Display).
- `crates/bossclaw-core/src/graph.rs` — replace the `EXTERNAL_ORIGIN` const with `pub use bossclaw_canon::EXTERNAL_ORIGIN;`.
- `crates/bossclaw-core/src/ingest.rs` — replace the local `is_external` with `pub use bossclaw_canon::is_external;`.
- `crates/bossclaw-core/src/log.rs` — binding storage primitives + brain-verifying-key getter + `EventLog::export_bundle`.
- `crates/bossclawd-proto/src/lib.rs` — 3 App-only `Request` variants + 3 `Response` variants + guest-refusal tests.
- `crates/bossclawd/Cargo.toml` — add `bossclaw-bundle` (for wire type) if needed (build lives in core).
- `crates/bossclawd/src/server.rs` — dispatch arms for the 3 new ops.
- `crates/bossclawd/src/engine/mod.rs` — `EngineHandle` methods: `brain_verifying_key`, `set_binding`, `export_bundle`.
- `crates/bossclawd/tests/authz.rs` — the 3 new ops are guest-refused.
- `apps/desktop/src-tauri/src/engine/client.rs` + `engine/mod.rs` — app-side proxies for the 3 ops.
- `apps/desktop/src-tauri/src/commands/integrations.rs` (or a new `commands/export.rs`) — `export_bundle` tauri command (binding mint on first export + file save).
- `apps/desktop/src-tauri/src/main.rs` — register the new command(s).
- `apps/desktop/src/api/integrations.ts` (or a new `api/export.ts`) — TS wrapper.
- `apps/desktop/src/memory/LibraryPanel.tsx` — multi-select + "Export signed bundle" entry point.

**Status:** Rev 1 of this plan. Anchored to spec Rev 3 (`docs/superpowers/specs/2026-07-20-rung5-verifiable-memory-design.md`, both reviewers plan-ready). SP-V1 scope ONLY (spec §1: export side — Stories A-export + C; L1/offline verification). SP-V2 (verify page, registry L2 HTTP, `PublishClaim` pin) is OUT of scope. **Build is gated behind the R4-A dogfood verdict** — do not start implementation before the go/no-go after Sun 2026-07-27.

**Toolchain prerequisite (once):** `rustup target add wasm32-unknown-unknown` (Task 1's wasm check needs it).

---

## Task 1 — Extract `bossclaw-canon` (zero behavior change)

Move the canonical-bytes/hash/sign primitives + the external-origin taint classifier into a wasm-clean leaf crate that `bossclaw-core` re-exports, so every existing `crate::event::…` / `crate::sign::…` / `crate::graph::EXTERNAL_ORIGIN` / `crate::ingest::is_external` call site keeps compiling unchanged.

**Files:**
- Create `crates/bossclaw-canon/Cargo.toml`
- Create `crates/bossclaw-canon/src/lib.rs`
- Create `crates/bossclaw-canon/src/error.rs`
- Create `crates/bossclaw-canon/src/event.rs`
- Create `crates/bossclaw-canon/src/sign.rs`
- Create `crates/bossclaw-canon/tests/vectors.rs`
- Modify `Cargo.toml` (workspace `members`, line 3)
- Modify `crates/bossclaw-core/Cargo.toml` (`[dependencies]`, after line 8)
- Modify `crates/bossclaw-core/src/lib.rs` (module decls near lines 24 & 44; re-export near line 60)
- Modify `crates/bossclaw-core/src/error.rs` (add `From<CanonError>`, after line 56)
- Modify `crates/bossclaw-core/src/graph.rs` (const at line 75)
- Modify `crates/bossclaw-core/src/ingest.rs` (`is_external` at line 716)

Steps:

- [ ] Add the crate to the workspace. Edit `Cargo.toml` line 3 members list to include `"crates/bossclaw-canon"` (append inside the array). Create `crates/bossclaw-canon/Cargo.toml`:
  ```toml
  [package]
  name = "bossclaw-canon"
  version = "0.0.1"
  edition = "2021"
  license = "Apache-2.0"
  description = "Canonical event bytes + Ed25519 hash-signing, extracted from bossclaw-core so the Rung-5 verifier can reproduce the engine's exact bytes on wasm32 without engine deps."
  repository = "https://github.com/AgentIdentityRegistry/air-note"

  # ALL deps are wasm32-unknown-unknown clean; the verify side needs NO RNG (signing is
  # deterministic Ed25519). CONSENSUS-CRITICAL PINS (spec §2.1 C-NEW-3): serde_jcs +
  # unicode-normalization + the ed25519-dalek/multibase/sha2 encoding surface are exact-pinned
  # to the versions the log's historical events were signed under — a passive bump could
  # re-canonicalize history to different bytes and silently break verify_chain + every stamp.
  # tests/vectors.rs carries the cross-version regression vector that fails if any of them drift.
  [dependencies]
  serde = { version = "1", features = ["derive"] }
  serde_json = "1"
  serde_jcs = "=0.1.0"
  unicode-normalization = "=0.1.25"
  sha2 = "=0.10.9"
  hex = "0.4"
  ed25519-dalek = "=2.2.0"
  multibase = "=0.9.2"
  thiserror = "1"
  ```
  (Versions verified against `Cargo.lock` on 2026-07-21: `serde_jcs 0.1.0`, `unicode-normalization 0.1.25`, `sha2 0.10.9`, `ed25519-dalek 2.2.0`, `multibase 0.9.2`.)

- [ ] Write `crates/bossclaw-canon/src/error.rs` — the four variants `event.rs`/`sign.rs` use today (`crates/bossclaw-core/src/error.rs:9,13,17,22`), with byte-identical `#[error]` strings so any surfaced string is unchanged:
  ```rust
  //! Error type for the extracted canonical/signing primitives.
  use thiserror::Error;

  /// Errors from canonical-bytes production and Ed25519 hash-signing. The Display strings are
  /// byte-identical to the pre-extraction `BossclawError` variants so no user-facing string moves.
  #[derive(Debug, Error)]
  pub enum CanonError {
      /// Canonical JSON (JCS) serialisation failed.
      #[error("canonical JSON error: {0}")]
      Canonical(String),
      /// A cryptographic signature operation failed (sign or verify).
      #[error("signature error: {0}")]
      Signature(String),
      /// A multibase encode/decode operation failed.
      #[error("multibase error: {0}")]
      Multibase(String),
      /// An event's hash did not recompute, or a chain-adjacent decode failed.
      #[error("chain integrity error: {0}")]
      Chain(String),
  }
  ```

- [ ] Write `crates/bossclaw-canon/src/event.rs` — a verbatim move of `crates/bossclaw-core/src/event.rs:1-92` with two edits: (a) `use crate::error::CanonError;` replaces the `BossclawError` import and every `BossclawError` in signatures becomes `CanonError`; (b) append the extracted `EXTERNAL_ORIGIN` const and `is_external` fn (moved from `graph.rs:75` and `ingest.rs:716`):
  ```rust
  /// The taint stamp written at `content["origin"]` of every externally-sourced event
  /// (remember() notes, captured sessions, file ingests). Single-sourced so the stamp site and
  /// the `is_external` classifier can never drift. (Moved from graph.rs:75, zero value change.)
  pub const EXTERNAL_ORIGIN: &str = "external";

  /// True iff `event` is externally-tainted — reads the single-sourced `EXTERNAL_ORIGIN` stamp.
  /// (Moved from ingest.rs:716, zero behavior change.)
  pub fn is_external(event: &Event) -> bool {
      event.content.get("origin").and_then(|v| v.as_str()) == Some(EXTERNAL_ORIGIN)
  }
  ```

- [ ] Write `crates/bossclaw-canon/src/sign.rs` — a verbatim move of `crates/bossclaw-core/src/sign.rs:1-37` (`BossclawError` → `CanonError`), then add the general-purpose signing pair the bundle crate needs (arbitrary-length message; the manifest seal + binding signature are NOT 32-byte hashes) and re-export the ed25519 types so downstream crates stay canon-only:
  ```rust
  pub use ed25519_dalek::{Signature, SigningKey, Verifier, VerifyingKey};

  /// Sign an arbitrary message, returning a multibase base58btc (`z`) string. Ed25519 signs any
  /// length natively (no pre-hash), so the master seal + binding signature use this, not `sign_hash`.
  pub fn sign_bytes(msg: &[u8], key: &SigningKey) -> String {
      use ed25519_dalek::Signer;
      let sig: Signature = key.sign(msg);
      multibase::encode(multibase::Base::Base58Btc, sig.to_bytes())
  }

  /// Verify a multibase base58btc signature over an arbitrary message.
  pub fn verify_bytes(msg: &[u8], signature_mb: &str, key: &VerifyingKey) -> Result<(), CanonError> {
      let (_b, raw) = multibase::decode(signature_mb)
          .map_err(|e| CanonError::Multibase(format!("decode: {e}")))?;
      let bytes: [u8; 64] = raw.as_slice().try_into()
          .map_err(|_| CanonError::Signature(format!("sig must be 64 bytes, got {}", raw.len())))?;
      key.verify(msg, &Signature::from_bytes(&bytes))
          .map_err(|e| CanonError::Signature(e.to_string()))
  }
  ```

- [ ] Write `crates/bossclaw-canon/src/lib.rs`:
  ```rust
  //! Canonical event bytes + Ed25519 hash-signing, extracted from bossclaw-core (zero behavior
  //! change) so the Rung-5 verifier reproduces the engine's exact bytes on wasm32.
  #![forbid(unsafe_code)]
  #![deny(missing_docs)]

  pub mod error;
  pub mod event;
  pub mod sign;

  pub use error::CanonError;
  pub use event::{canonical_bytes, compute_hash, is_external, Event, ModelMeta, EXTERNAL_ORIGIN};
  pub use sign::{sign_bytes, sign_hash, verify_bytes, verify_hash, SigningKey, VerifyingKey};
  ```

- [ ] Rewire `bossclaw-core` to re-export canon (the zero-behavior-change seam). In `crates/bossclaw-core/Cargo.toml` `[dependencies]` add `bossclaw-canon = { path = "../bossclaw-canon" }`. In `crates/bossclaw-core/src/lib.rs`: DELETE `pub mod event;` (line 24) and `pub mod sign;` (line 44); ADD `pub use bossclaw_canon::{event, sign};` (so `crate::event::…` and `crate::sign::…` still resolve). Keep `pub use event::{Event, ModelMeta};` (line 60) — it now resolves through the re-export. In `crates/bossclaw-core/src/graph.rs` replace the `EXTERNAL_ORIGIN` const (line 75) with `pub use bossclaw_canon::EXTERNAL_ORIGIN;`. In `crates/bossclaw-core/src/ingest.rs` replace the local `pub fn is_external` (line 716) with `pub use bossclaw_canon::is_external;` (the lib.rs re-export `pub use ingest::{is_external, …}` at line 90 still works). In `crates/bossclaw-core/src/error.rs` add after line 56:
  ```rust
  impl From<bossclaw_canon::CanonError> for BossclawError {
      fn from(e: bossclaw_canon::CanonError) -> Self {
          use bossclaw_canon::CanonError as C;
          match e {
              C::Canonical(s) => BossclawError::Canonical(s),
              C::Signature(s) => BossclawError::Signature(s),
              C::Multibase(s) => BossclawError::Multibase(s),
              C::Chain(s) => BossclawError::Chain(s),
          }
      }
  }
  ```
  This keeps every `canonical_bytes(&e)?` / `compute_hash(&e)?` / `verify_hash(...)?` inside `bossclaw-core` (e.g. `log.rs:1215,1349,1361`) compiling: `?` converts `CanonError → BossclawError`, variant-for-variant, string-preserving.

- [ ] Run the pin/parity check (foreground): `cargo test -p bossclaw-core --test vectors`. The EXISTING `crates/bossclaw-core/tests/vectors.rs` imports `bossclaw_core::event::{canonical_bytes, compute_hash, Event, ModelMeta}` + `bossclaw_core::sign::{sign_hash, verify_hash}` and asserts the frozen canonical string + genesis hash `9089b0bd99a3f72e37653c2e8da756aeeb737085c0faa9a1ae5d0defc35dbde9` — it MUST stay green through the re-export (this IS the byte-identity guard). Expected: PASS. Then `cargo build -p bossclaw-core` — expected: clean (proves the internal call sites still resolve).

- [ ] Add canon's own known-answer + cross-version regression vector so canon is guarded independently of core (C-NEW-3). Write `crates/bossclaw-canon/tests/vectors.rs`:
  ```rust
  use bossclaw_canon::event::{canonical_bytes, compute_hash, is_external, Event, EXTERNAL_ORIGIN};
  use bossclaw_canon::sign::{sign_hash, verify_hash};
  use bossclaw_canon::SigningKey;

  fn fixture() -> Event {
      Event {
          id: "01J0000000000000000000000A".into(),
          ts: "2026-06-15T00:00:00Z".into(),
          valid_time: None,
          event_type: "memory".into(),
          content: serde_json::json!({ "text": "hello" }),
          model_meta: None,
          prev_hash: "00".repeat(32),
          hash: None,
          signed_by_did: "did:wba:AIR-2JE0-EM7W-JNBK".into(),
          signature: None,
      }
  }

  #[test]
  fn canonical_bytes_frozen() {
      let expected = r#"{"content":{"text":"hello"},"id":"01J0000000000000000000000A","prev_hash":"0000000000000000000000000000000000000000000000000000000000000000","signed_by_did":"did:wba:AIR-2JE0-EM7W-JNBK","ts":"2026-06-15T00:00:00Z","type":"memory"}"#;
      assert_eq!(String::from_utf8(canonical_bytes(&fixture()).unwrap()).unwrap(), expected);
  }

  #[test]
  fn genesis_hash_frozen() {
      assert_eq!(
          hex::encode(compute_hash(&fixture()).unwrap()),
          "9089b0bd99a3f72e37653c2e8da756aeeb737085c0faa9a1ae5d0defc35dbde9",
          "a dep bump changed canonical bytes — DO NOT rebase the pins to fix this"
      );
  }

  #[test]
  fn sign_verify_and_origin() {
      let key = SigningKey::from_bytes(&[7u8; 32]);
      let h = compute_hash(&fixture()).unwrap();
      let sig = sign_hash(&h, &key);
      assert!(sig.starts_with('z'));
      verify_hash(&h, &sig, &key.verifying_key()).unwrap();
      let mut ext = fixture();
      ext.content = serde_json::json!({ "text": "x", "origin": EXTERNAL_ORIGIN });
      assert!(is_external(&ext));
      assert!(!is_external(&fixture()));
  }
  ```
  Run (foreground): `cargo test -p bossclaw-canon`. Expected: 3 pass.

- [ ] Prove wasm-cleanliness (foreground): `cargo check -p bossclaw-canon --target wasm32-unknown-unknown`. Expected: clean (no rusqlite/hnsw/tokenizers reachable; verify side needs no RNG).

- [ ] Commit: `git add crates/bossclaw-canon Cargo.toml crates/bossclaw-core/Cargo.toml crates/bossclaw-core/src/lib.rs crates/bossclaw-core/src/error.rs crates/bossclaw-core/src/graph.rs crates/bossclaw-core/src/ingest.rs && git commit -m "feat(rung5): extract bossclaw-canon leaf crate (zero behavior change, wasm-clean)"`

---

## Task 2 — `bossclaw-bundle`: the `.airmem` format types + canonical JSON

Scaffold the second leaf crate and define the on-disk document as canonical-JSON serde types with a round-trip test. No Merkle, no seal yet — just the shapes and the canonicalizer.

**Files:**
- Create `crates/bossclaw-bundle/Cargo.toml`
- Create `crates/bossclaw-bundle/src/lib.rs`
- Create `crates/bossclaw-bundle/src/format.rs`
- Modify `Cargo.toml` (workspace members)

Steps:

- [ ] Add the workspace member (`Cargo.toml` line 3, append `"crates/bossclaw-bundle"`). Create `crates/bossclaw-bundle/Cargo.toml`:
  ```toml
  [package]
  name = "bossclaw-bundle"
  version = "0.0.1"
  edition = "2021"
  license = "Apache-2.0"
  description = "Build + verify of the Rung-5 .airmem signed memory bundle. Depends on bossclaw-canon ONLY (never the engine)."
  repository = "https://github.com/AgentIdentityRegistry/air-note"

  [dependencies]
  bossclaw-canon = { path = "../bossclaw-canon" }
  serde = { version = "1", features = ["derive"] }
  serde_json = "1"
  serde_jcs = "=0.1.0"
  sha2 = "=0.10.9"
  hex = "0.4"
  thiserror = "1"
  ```
  (No `ed25519-dalek`/`multibase` direct dep — reached through `bossclaw-canon` re-exports, keeping the "canon-only" seam.)

- [ ] Write `crates/bossclaw-bundle/src/format.rs` with the canonical shapes. No floats anywhere (counts are `u64`, times are RFC3339 strings). The seal signs `jcs(manifest)`; `items`/`binding`/`seal` live OUTSIDE the manifest and are committed via `merkle_root`/`binding_hash`:
  ```rust
  //! The `.airmem` document shapes + the canonical-JSON serializer. Standards-shaped (spec §2.1):
  //! liftable into C2PA/VC/SCITT later without re-signing semantics.
  use serde::{Deserialize, Serialize};

  /// The `.airmem` version. Verifier refuses a newer MAJOR (spec §2.2 `FormatTooNew`).
  pub const FORMAT_VERSION: &str = "1.0.0";

  /// The whole document: `{ manifest, items, binding, seal }`.
  #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
  pub struct Airmem {
      /// The sealed sub-document (commits to items via `merkle_root`, to the card via `binding_hash`).
      pub manifest: Manifest,
      /// The disclosed memories, in leaf order.
      pub items: Vec<AirmemItem>,
      /// The ID card (hash-committed by `manifest.binding_hash`).
      pub binding: Binding,
      /// One master Ed25519 signature (multibase) by the brain key over `jcs(manifest)`.
      pub seal: String,
  }

  /// The seal's message.
  #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
  pub struct Manifest {
      /// Semver; verifier refuses a newer major.
      pub format_version: String,
      /// Export time, RFC3339.
      pub created_at: String,
      /// THE authoritative identity claim (spec §2.3). Resolution always starts here, never binding.
      pub did: String,
      /// The brain verifying key (multibase). Every item stamp is checked against THIS only (A6).
      pub brain_verifying_key: String,
      /// Free-text selection description shown to the receiver.
      pub selection_description: String,
      /// Number of items (integer; canonical-JSON no-float discipline).
      pub item_count: u64,
      /// Hex of the Merkle root over `items`.
      pub merkle_root: String,
      /// Hex of `H(jcs(binding))` — the seal thereby atomically covers the ID card (C1).
      pub binding_hash: String,
  }

  /// Which verification class an item belongs to (spec §2.2).
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
  #[serde(rename_all = "snake_case")]
  pub enum ItemClass {
      /// External `remember()` notes: disclosed event canonical bytes + original write-time signature.
      Stamped,
      /// Sessions/ingests/dossiers: content + display metadata ONLY (export-time vouching, H1).
      SealVouched,
  }

  /// One disclosed memory. `leaf` is EXCLUDED from the leaf hash (spec §2.2 finding A7).
  #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
  pub struct AirmemItem {
      /// Hex of this item's Merkle leaf hash. Excluded when computing the leaf (see merkle.rs).
      pub leaf: String,
      /// The verification class.
      pub class: ItemClass,
      /// Display kind: `"note"` | `"session"` | `"ingest"` | `"dossier"`.
      pub kind: String,
      /// STAMPED only: the original event's canonical JSON text (UTF-8; hash/sig excluded, per canon).
      #[serde(skip_serializing_if = "Option::is_none")]
      pub event_bytes: Option<String>,
      /// STAMPED only: the original write-time multibase signature over the event hash.
      #[serde(skip_serializing_if = "Option::is_none")]
      pub signature: Option<String>,
      /// SEAL_VOUCHED only: the disclosed content text.
      #[serde(skip_serializing_if = "Option::is_none")]
      pub content: Option<String>,
      /// SEAL_VOUCHED only: safe display metadata (NEVER paths/session_id/grant_root/lineage — A-N1).
      #[serde(skip_serializing_if = "Option::is_none")]
      pub display: Option<serde_json::Value>,
      /// SEAL_VOUCHED only: exporter-asserted origin label (rendered weaker — H2/C-NEW-1).
      #[serde(skip_serializing_if = "Option::is_none")]
      pub origin_label: Option<String>,
  }

  /// The ID card payload (canonical) + its identity signature.
  #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
  pub struct Binding {
      /// The signed fields (spec §2.3).
      pub payload: BindingPayload,
      /// Multibase base58btc signature over `jcs(payload)` by the identity key (`did_wba.rs:33` wrapped).
      pub identity_signature: String,
  }

  /// The binding payload — hash-committed by `manifest.binding_hash`.
  #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
  pub struct BindingPayload {
      /// The brain verifying key (multibase). MUST equal `manifest.brain_verifying_key` (C1).
      pub brain_verifying_key: String,
      /// The identity verifying key (multibase) — the offline-checkable key (A3/C3).
      pub identity_verifying_key: String,
      /// The AIR did. MUST equal `manifest.did` (C1).
      pub did: String,
      /// Fixed `"memory-signing"`.
      pub purpose: String,
      /// Monotonic integer reserved for future rotation semantics (A8/C9). First-write-wins per epoch.
      pub epoch: u64,
      /// Mint time, RFC3339.
      pub created_at: String,
  }

  /// Canonical JSON bytes (JCS RFC-8785) of any serializable value — the single canonicalizer.
  pub fn canonical_json(value: &impl Serialize) -> Result<Vec<u8>, serde_json::Error> {
      serde_jcs::to_vec(value)
  }
  ```

- [ ] Write `crates/bossclaw-bundle/src/lib.rs` (grows each task):
  ```rust
  //! Build + verify of the Rung-5 `.airmem` signed memory bundle. Canon-only.
  #![forbid(unsafe_code)]
  #![deny(missing_docs)]

  pub mod format;

  pub use format::{Airmem, AirmemItem, Binding, BindingPayload, ItemClass, Manifest, FORMAT_VERSION};
  ```

- [ ] Write the round-trip test at the bottom of `format.rs`:
  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      fn sample() -> Airmem {
          Airmem {
              manifest: Manifest {
                  format_version: FORMAT_VERSION.into(),
                  created_at: "2026-07-21T00:00:00Z".into(),
                  did: "did:wba:example.com:me".into(),
                  brain_verifying_key: "zBrain".into(),
                  selection_description: "2 notes".into(),
                  item_count: 1,
                  merkle_root: "ab".repeat(32),
                  binding_hash: "cd".repeat(32),
              },
              items: vec![AirmemItem {
                  leaf: "ef".repeat(32), class: ItemClass::Stamped, kind: "note".into(),
                  event_bytes: Some("{}".into()), signature: Some("zSig".into()),
                  content: None, display: None, origin_label: None,
              }],
              binding: Binding {
                  payload: BindingPayload {
                      brain_verifying_key: "zBrain".into(), identity_verifying_key: "zId".into(),
                      did: "did:wba:example.com:me".into(), purpose: "memory-signing".into(),
                      epoch: 1, created_at: "2026-07-21T00:00:00Z".into(),
                  },
                  identity_signature: "zIdSig".into(),
              },
              seal: "zSeal".into(),
          }
      }
      #[test]
      fn airmem_round_trips_through_canonical_json() {
          let a = sample();
          let bytes = canonical_json(&a).unwrap();
          let back: Airmem = serde_json::from_slice(&bytes).unwrap();
          assert_eq!(a, back);
      }
      #[test]
      fn canonical_json_is_key_sorted_and_deterministic() {
          let a = sample();
          assert_eq!(canonical_json(&a).unwrap(), canonical_json(&a).unwrap());
          let s = String::from_utf8(canonical_json(&a.manifest).unwrap()).unwrap();
          assert!(s.find("binding_hash").unwrap() < s.find("brain_verifying_key").unwrap(),
              "JCS sorts keys");
      }
  }
  ```
  Run (foreground): `cargo test -p bossclaw-bundle`. Expected: 2 pass.

- [ ] Commit: `git add crates/bossclaw-bundle Cargo.toml && git commit -m "feat(rung5): bossclaw-bundle .airmem format types + canonical JSON"`

---

## Task 3 — Merkle module (domain-separated, frozen rules)

**Files:**
- Create `crates/bossclaw-bundle/src/merkle.rs`
- Modify `crates/bossclaw-bundle/src/lib.rs` (add `pub mod merkle;`)

Steps:

- [ ] Write the failing test first (create `merkle.rs` with only the test + `todo!()` stubs won't compile cleanly, so write signatures returning `Default` then the test). Add to `merkle.rs`:
  ```rust
  //! Domain-separated Merkle tree over items (spec §2.2 finding A7): leaf = H(0x00 ‖ item), internal
  //! = H(0x01 ‖ left ‖ right), odd node promoted UNPAIRED (no duplicate-last), leaf order = item order.
  use sha2::{Digest, Sha256};
  use crate::format::AirmemItem;

  /// The 32-byte leaf hash of one item: `SHA256(0x00 ‖ jcs(item_without_leaf_field))`.
  pub fn leaf_hash(item: &AirmemItem) -> [u8; 32] {
      let mut naked = item.clone();
      naked.leaf = String::new(); // the leaf field is excluded from its own hash
      let canon = crate::format::canonical_json(&naked).expect("item is always serializable");
      let mut h = Sha256::new();
      h.update([0x00]);
      h.update(&canon);
      h.finalize().into()
  }

  /// The Merkle root over pre-computed leaf hashes (leaf order preserved). Empty input is disallowed
  /// upstream (`EmptySelection`); a single leaf is its own root.
  pub fn root(leaves: &[[u8; 32]]) -> [u8; 32] {
      assert!(!leaves.is_empty(), "root() requires >= 1 leaf; empty selection is rejected upstream");
      let mut level: Vec<[u8; 32]> = leaves.to_vec();
      while level.len() > 1 {
          let mut next = Vec::with_capacity(level.len().div_ceil(2));
          let mut i = 0;
          while i < level.len() {
              if i + 1 < level.len() {
                  let mut h = Sha256::new();
                  h.update([0x01]);
                  h.update(level[i]);
                  h.update(level[i + 1]);
                  next.push(h.finalize().into());
                  i += 2;
              } else {
                  next.push(level[i]); // odd node promoted UNPAIRED
                  i += 1;
              }
          }
          level = next;
      }
      level[0]
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      use crate::format::{AirmemItem, ItemClass};
      fn item(leaf: &str, content: &str) -> AirmemItem {
          AirmemItem { leaf: leaf.into(), class: ItemClass::SealVouched, kind: "session".into(),
              event_bytes: None, signature: None, content: Some(content.into()), display: None,
              origin_label: Some("labeled".into()) }
      }
      #[test]
      fn leaf_excludes_leaf_field() {
          let a = item("aa", "same");
          let b = item("bb", "same"); // different `leaf`, same everything else
          assert_eq!(leaf_hash(&a), leaf_hash(&b), "the leaf field must not affect its own hash");
      }
      #[test]
      fn domain_separation_defeats_leaf_as_internal_second_preimage() {
          // An attacker who knows two sibling leaves L,R cannot forge an item whose LEAF equals the
          // internal node H(0x01 ‖ L ‖ R): leaves are H(0x00 ‖ …), a different domain byte.
          let l = leaf_hash(&item("", "l"));
          let r = leaf_hash(&item("", "r"));
          let internal = root(&[l, r]);
          let leaf_domain = leaf_hash(&item("", "forge"));
          assert_ne!(internal, leaf_domain);
      }
      #[test]
      fn odd_node_promoted_unpaired_not_duplicated() {
          let a = leaf_hash(&item("", "a"));
          let b = leaf_hash(&item("", "b"));
          let c = leaf_hash(&item("", "c"));
          // 3 leaves: level1 = [H(a,b), c]; root = H(H(a,b), c). NOT H(H(a,b), H(c,c)).
          let mut h = Sha256::new();
          h.update([0x01]); h.update(a); h.update(b);
          let ab: [u8; 32] = h.finalize().into();
          let mut h2 = Sha256::new();
          h2.update([0x01]); h2.update(ab); h2.update(c);
          let expected: [u8; 32] = h2.finalize().into();
          assert_eq!(root(&[a, b, c]), expected);
      }
      #[test]
      fn single_leaf_is_its_own_root() {
          let a = leaf_hash(&item("", "solo"));
          assert_eq!(root(&[a]), a);
      }
      #[test]
      fn order_matters() {
          let a = leaf_hash(&item("", "a"));
          let b = leaf_hash(&item("", "b"));
          assert_ne!(root(&[a, b]), root(&[b, a]));
      }
  }
  ```
  Add `pub mod merkle;` to `lib.rs`.

- [ ] Run (foreground): `cargo test -p bossclaw-bundle merkle`. Expected: 5 pass. (The code above is the minimal implementation; if written test-first, stub `root`/`leaf_hash` to `[0u8;32]` first, watch the domain/odd-node/order tests fail, then fill in.)

- [ ] Commit: `git add crates/bossclaw-bundle/src/merkle.rs crates/bossclaw-bundle/src/lib.rs && git commit -m "feat(rung5): domain-separated Merkle tree (frozen A7 rules)"`

---

## Task 4 — Binding: canonical form, `binding_hash`, internal-consistency verify

**Files:**
- Create `crates/bossclaw-bundle/src/binding.rs`
- Modify `crates/bossclaw-bundle/src/lib.rs` (`pub mod binding;` + re-exports)

Steps:

- [ ] Write `crates/bossclaw-bundle/src/binding.rs`:
  ```rust
  //! Binding (ID card) canonical form: the bytes the identity key signs, the hash the seal commits to,
  //! and the L1 internal-consistency check. Identity resolution NEVER starts here (spec §2.3) — the
  //! sealed `manifest.did` is authoritative; this only proves the card is self-consistent.
  use sha2::{Digest, Sha256};
  use bossclaw_canon::sign::{verify_bytes, VerifyingKey};
  use crate::format::{Binding, BindingPayload};

  /// The exact bytes the identity key signs (and re-verifies): `jcs(payload)`.
  pub fn binding_signing_bytes(payload: &BindingPayload) -> Vec<u8> {
      crate::format::canonical_json(payload).expect("binding payload is always serializable")
  }

  /// `H(jcs(binding))` — the value `manifest.binding_hash` must equal (C-NEW-2). Hashes the WHOLE
  /// binding (payload + identity_signature) so a tampered signature also trips `BindingHashMismatch`.
  pub fn binding_hash(binding: &Binding) -> [u8; 32] {
      let canon = crate::format::canonical_json(binding).expect("binding is always serializable");
      Sha256::new().chain_update(&canon).finalize().into()
  }

  /// L1 internal consistency: the identity signature verifies over `jcs(payload)` against the
  /// EMBEDDED `identity_verifying_key`. Provides ZERO identity assurance (the key is attacker-
  /// choosable, finding C3) — the equality checks against the sealed manifest live in verify.rs.
  pub fn verify_binding_internal(binding: &Binding) -> bool {
      let raw = match multibase_key(&binding.payload.identity_verifying_key) {
          Some(k) => k,
          None => return false,
      };
      let vk = match VerifyingKey::from_bytes(&raw) {
          Ok(k) => k,
          Err(_) => return false,
      };
      verify_bytes(&binding_signing_bytes(&binding.payload), &binding.identity_signature, &vk).is_ok()
  }

  /// Decode a multibase-wrapped 32-byte Ed25519 public key. Accepts both the bare 32-byte multibase
  /// form (used for binding keys) — the multikey `0xed01` prefix form is a resolver/display concern.
  pub(crate) fn multibase_key(mb: &str) -> Option<[u8; 32]> {
      let (_b, raw) = multibase::decode(mb).ok()?;
      raw.as_slice().try_into().ok()
  }
  ```
  Add `multibase = "=0.9.2"` to `crates/bossclaw-bundle/Cargo.toml` `[dependencies]` (the binding decodes multibase keys directly). Add `pub mod binding;` + `pub use binding::{binding_hash, binding_signing_bytes, verify_binding_internal};` to `lib.rs`.

- [ ] Write the test at the bottom of `binding.rs` (mints an identity key, signs the payload, asserts internal verify + hash stability + tamper failure):
  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      use bossclaw_canon::sign::{sign_bytes, SigningKey};
      fn signed_binding(id_key: &SigningKey, epoch: u64) -> Binding {
          let idvk = multibase::encode(multibase::Base::Base58Btc, id_key.verifying_key().to_bytes());
          let payload = BindingPayload {
              brain_verifying_key: "zBrain".into(), identity_verifying_key: idvk,
              did: "did:wba:example.com:me".into(), purpose: "memory-signing".into(),
              epoch, created_at: "2026-07-21T00:00:00Z".into(),
          };
          let sig = sign_bytes(&binding_signing_bytes(&payload), id_key);
          Binding { payload, identity_signature: sig }
      }
      #[test]
      fn internal_verify_passes_for_a_correctly_signed_card() {
          let k = SigningKey::from_bytes(&[3u8; 32]);
          assert!(verify_binding_internal(&signed_binding(&k, 1)));
      }
      #[test]
      fn internal_verify_fails_if_payload_tampered() {
          let k = SigningKey::from_bytes(&[3u8; 32]);
          let mut b = signed_binding(&k, 1);
          b.payload.did = "did:wba:evil.com:attacker".into(); // sig no longer covers this
          assert!(!verify_binding_internal(&b));
      }
      #[test]
      fn binding_hash_is_deterministic_and_covers_signature() {
          let k = SigningKey::from_bytes(&[3u8; 32]);
          let b = signed_binding(&k, 1);
          let h = binding_hash(&b);
          assert_eq!(h, binding_hash(&b));
          let mut tampered = b.clone();
          tampered.identity_signature = "zDIFFERENT".into();
          assert_ne!(h, binding_hash(&tampered));
      }
  }
  ```
  Run (foreground): `cargo test -p bossclaw-bundle binding`. Expected: 3 pass.

- [ ] Commit: `git add crates/bossclaw-bundle/src/binding.rs crates/bossclaw-bundle/src/lib.rs crates/bossclaw-bundle/Cargo.toml && git commit -m "feat(rung5): binding canonical form + binding_hash + internal-consistency verify"`

---

## Task 5 — `build_bundle`: assemble → root → binding_hash → master seal

**Files:**
- Create `crates/bossclaw-bundle/src/build.rs`
- Modify `crates/bossclaw-bundle/src/lib.rs` (`pub mod build;` + re-exports)

Steps:

- [ ] Write `crates/bossclaw-bundle/src/build.rs`. The caller (core `EventLog::export_bundle`) hands in already-gathered item inputs + the binding + the brain `SigningKey`; this function computes leaves, the root, the binding hash, assembles the manifest, and master-seals it. Item inputs are class-typed so the two disclosure shapes can't be mixed up:
  ```rust
  //! Assemble a sealed `.airmem` from gathered inputs. Pure: no engine, no I/O, no clock (the caller
  //! passes `created_at`). The brain `SigningKey` seals the canonical manifest.
  use bossclaw_canon::sign::{sign_bytes, SigningKey};
  use crate::binding::binding_hash;
  use crate::format::{Airmem, AirmemItem, Binding, ItemClass, Manifest, FORMAT_VERSION};
  use crate::merkle;

  /// One gathered memory to disclose, class-typed so build can't confuse the two shapes.
  pub enum ItemInput {
      /// External note: its canonical event bytes (UTF-8 JSON) + original write-time signature.
      Stamped { event_bytes: String, signature: String },
      /// Session/ingest/dossier: content + safe display metadata + exporter-asserted origin label.
      SealVouched { kind: String, content: String, display: serde_json::Value, origin_label: String },
  }

  /// Everything build needs that isn't derivable here.
  pub struct BuildInput<'a> {
      /// Export time (RFC3339) — caller-supplied (build is clock-free).
      pub created_at: String,
      /// THE identity claim.
      pub did: String,
      /// Multibase brain verifying key.
      pub brain_verifying_key: String,
      /// Free-text selection description.
      pub selection_description: String,
      /// The gathered items, in the order they will appear (leaf order = item order).
      pub items: Vec<ItemInput>,
      /// The latest stored binding (verbatim).
      pub binding: Binding,
      /// The brain signing key (seals the manifest).
      pub brain_key: &'a SigningKey,
  }

  /// Build + seal. `items` MUST be non-empty (caller enforces `EmptySelection`).
  pub fn build_bundle(input: BuildInput<'_>) -> Airmem {
      // 1. Materialize items WITHOUT leaves, compute each leaf, then stamp the leaf hex in.
      let mut items: Vec<AirmemItem> = input.items.into_iter().map(|ii| match ii {
          ItemInput::Stamped { event_bytes, signature } => AirmemItem {
              leaf: String::new(), class: ItemClass::Stamped, kind: "note".into(),
              event_bytes: Some(event_bytes), signature: Some(signature),
              content: None, display: None, origin_label: None,
          },
          ItemInput::SealVouched { kind, content, display, origin_label } => AirmemItem {
              leaf: String::new(), class: ItemClass::SealVouched, kind,
              event_bytes: None, signature: None,
              content: Some(content), display: Some(display), origin_label: Some(origin_label),
          },
      }).collect();
      let leaves: Vec<[u8; 32]> = items.iter().map(merkle::leaf_hash).collect();
      for (item, leaf) in items.iter_mut().zip(&leaves) {
          item.leaf = hex::encode(leaf);
      }
      // 2. Root + binding hash.
      let merkle_root = hex::encode(merkle::root(&leaves));
      let binding_hash_hex = hex::encode(binding_hash(&input.binding));
      // 3. Manifest, then master seal over jcs(manifest).
      let manifest = Manifest {
          format_version: FORMAT_VERSION.into(),
          created_at: input.created_at,
          did: input.did,
          brain_verifying_key: input.brain_verifying_key,
          selection_description: input.selection_description,
          item_count: items.len() as u64,
          merkle_root,
          binding_hash: binding_hash_hex,
      };
      let seal = sign_bytes(&crate::format::canonical_json(&manifest).expect("manifest serializable"),
          input.brain_key);
      Airmem { manifest, items, binding: input.binding, seal }
  }
  ```
  Add `pub mod build;` + `pub use build::{build_bundle, BuildInput, ItemInput};` to `lib.rs`.

- [ ] Write the test (build a 2-item bundle with a synthetic brain key + a signed binding; assert leaves populated, root non-empty, seal present, `item_count == 2`, and that the seal verifies against the brain key over the canonical manifest):
  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      use bossclaw_canon::sign::{sign_bytes, verify_bytes, SigningKey};
      use crate::binding::binding_signing_bytes;
      use crate::format::BindingPayload;

      fn binding(brain_mb: &str) -> Binding {
          let idk = SigningKey::from_bytes(&[9u8; 32]);
          let idvk = multibase::encode(multibase::Base::Base58Btc, idk.verifying_key().to_bytes());
          let payload = BindingPayload {
              brain_verifying_key: brain_mb.into(), identity_verifying_key: idvk,
              did: "did:wba:example.com:me".into(), purpose: "memory-signing".into(),
              epoch: 1, created_at: "2026-07-21T00:00:00Z".into(),
          };
          let sig = sign_bytes(&binding_signing_bytes(&payload), &idk);
          Binding { payload, identity_signature: sig }
      }

      #[test]
      fn build_seals_a_verifiable_manifest() {
          let brain = SigningKey::from_bytes(&[1u8; 32]);
          let brain_mb = multibase::encode(multibase::Base::Base58Btc, brain.verifying_key().to_bytes());
          let a = build_bundle(BuildInput {
              created_at: "2026-07-21T00:00:00Z".into(),
              did: "did:wba:example.com:me".into(),
              brain_verifying_key: brain_mb.clone(),
              selection_description: "1 note + 1 session".into(),
              items: vec![
                  ItemInput::Stamped { event_bytes: "{}".into(), signature: "zSig".into() },
                  ItemInput::SealVouched { kind: "session".into(), content: "body".into(),
                      display: serde_json::json!({"title":"S"}), origin_label: "labeled".into() },
              ],
              binding: binding(&brain_mb),
              brain_key: &brain,
          });
          assert_eq!(a.manifest.item_count, 2);
          assert!(a.items.iter().all(|i| i.leaf.len() == 64));
          let manifest_bytes = crate::format::canonical_json(&a.manifest).unwrap();
          verify_bytes(&manifest_bytes, &a.seal, &brain.verifying_key()).expect("seal verifies");
      }
  }
  ```
  Run (foreground): `cargo test -p bossclaw-bundle build`. Expected: 1 pass.

- [ ] Commit: `git add crates/bossclaw-bundle/src/build.rs crates/bossclaw-bundle/src/lib.rs && git commit -m "feat(rung5): build_bundle — assemble items, Merkle root, binding_hash, master seal"`

---

## Task 6 — `verify` L1: error enum, checklist, tamper matrix, forgery, no-leak

The heart of the spec (§2.5 L1, §7 error enum, §8 tamper matrix). Offline, fail-closed, one bad byte = headline ❌.

**Files:**
- Create `crates/bossclaw-bundle/src/verify.rs`
- Modify `crates/bossclaw-bundle/src/lib.rs` (`pub mod verify;` + re-exports)

Steps:

- [ ] Write the `VerifyError` enum + `Verdict` + `verify` L1 checklist in `verify.rs`. The identity level is separate (`IdentityLevel`) so L1 never renders a green card (C3). L2 is threaded via a resolver (Task 7) but the L1 body is complete now with an offline default:
  ```rust
  //! Verify an `.airmem`. L1 = self-consistent offline (seal, item stamps, Merkle root, binding_hash,
  //! binding internal + equality checks). Fail-closed: the FIRST mismatch is the verdict (H3).
  use bossclaw_canon::event::{canonical_bytes, compute_hash, is_external, Event};
  use bossclaw_canon::sign::{verify_bytes, verify_hash, VerifyingKey};
  use semver_lite::major; // hand-rolled below; NO external semver dep
  use crate::binding::{binding_hash, multibase_key, verify_binding_internal};
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
      /// A stamped item's carried label disagrees with its recomputed origin (A5).
      OriginMismatch(usize),
      /// L2 only: `manifest.did` did not resolve via the registry (L1 verdict still reported).
      IdentityUnresolved,
      /// `format_version` major is newer than the verifier's.
      FormatTooNew,
      /// Structurally unparseable / an item is missing a required field for its class.
      Malformed(String),
  }

  /// The identity assurance level rendered alongside the L1 verdict. L1 alone = Unverified (C3).
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub enum IdentityLevel {
      /// L1 only — the embedded key is attacker-choosable, so identity is unverified offline.
      UnverifiedOffline,
      /// L2 — `manifest.did` resolved via the registry and its key matched the binding.
      RegistryResolved,
  }

  /// The verdict: L1 ok + an identity level + per-item derived origin labels.
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct Verdict {
      /// Per-item human origin label (verifier-derived for stamped, exporter-asserted for seal-vouched).
      pub item_labels: Vec<String>,
      /// The identity assurance level.
      pub identity: IdentityLevel,
  }

  /// The verifier's supported major.
  const SUPPORTED_MAJOR: u64 = 1;

  /// Full L1 verification, then an L2 attempt via `resolver` (offline resolver = stays L1).
  pub fn verify(bundle: &Airmem, resolver: &dyn IdentityResolver) -> Result<Verdict, VerifyError> {
      // 0. Version gate.
      if major(&bundle.manifest.format_version).ok_or_else(|| VerifyError::Malformed("format_version".into()))? > SUPPORTED_MAJOR {
          return Err(VerifyError::FormatTooNew);
      }
      // 1. Seal over jcs(manifest) against manifest.brain_verifying_key.
      let brain_vk = decode_vk(&bundle.manifest.brain_verifying_key)
          .ok_or_else(|| VerifyError::Malformed("brain_verifying_key".into()))?;
      let manifest_bytes = crate::format::canonical_json(&bundle.manifest)
          .map_err(|e| VerifyError::Malformed(e.to_string()))?;
      verify_bytes(&manifest_bytes, &bundle.seal, &brain_vk).map_err(|_| VerifyError::SealInvalid)?;
      // 2. binding_hash recompute (distinct L1 step, C-NEW-2).
      if hex::encode(binding_hash(&bundle.binding)) != bundle.manifest.binding_hash {
          return Err(VerifyError::BindingHashMismatch);
      }
      // 3. Binding internal + the two equality checks against the SEALED manifest (C1).
      if !verify_binding_internal(&bundle.binding) { return Err(VerifyError::BindingInvalid); }
      if bundle.binding.payload.brain_verifying_key != bundle.manifest.brain_verifying_key {
          return Err(VerifyError::BindingKeyMismatch);
      }
      if bundle.binding.payload.did != bundle.manifest.did { return Err(VerifyError::BindingDidMismatch); }
      // 4. Per-item: leaf recompute, stamp against manifest brain key ONLY, origin derivation.
      let mut leaves = Vec::with_capacity(bundle.items.len());
      let mut item_labels = Vec::with_capacity(bundle.items.len());
      for (i, item) in bundle.items.iter().enumerate() {
          let leaf = merkle::leaf_hash(item);
          if hex::encode(leaf) != item.leaf { return Err(VerifyError::ItemHashMismatch(i)); }
          leaves.push(leaf);
          item_labels.push(verify_item(i, item, &brain_vk)?);
      }
      // 5. Merkle root.
      if hex::encode(merkle::root(&leaves)) != bundle.manifest.merkle_root {
          return Err(VerifyError::TreeMismatch);
      }
      // 6. L2 attempt (offline resolver → Unverified; a resolved+matching key → RegistryResolved).
      let identity = match resolver.resolve(&bundle.manifest.did) {
          None => IdentityLevel::UnverifiedOffline,
          Some(registry_key_mb) => {
              if registry_key_mb == bundle.binding.payload.identity_verifying_key {
                  IdentityLevel::RegistryResolved
              } else {
                  return Err(VerifyError::IdentityUnresolved);
              }
          }
      };
      Ok(Verdict { item_labels, identity })
  }

  /// Verify one item and return its honest origin label (§3-H2).
  fn verify_item(i: usize, item: &AirmemItem, brain_vk: &VerifyingKey) -> Result<String, VerifyError> {
      match item.class {
          ItemClass::Stamped => {
              let bytes = item.event_bytes.as_ref().ok_or(VerifyError::ItemStampInvalid(i))?;
              let sig = item.signature.as_ref().ok_or(VerifyError::ItemStampInvalid(i))?;
              let ev: Event = serde_json::from_str(bytes).map_err(|_| VerifyError::ItemStampInvalid(i))?;
              // Re-canonicalization stability: disclosed bytes MUST be the event's canonical form.
              let recanon = canonical_bytes(&ev).map_err(|_| VerifyError::ItemStampInvalid(i))?;
              if recanon != bytes.as_bytes() { return Err(VerifyError::ItemStampInvalid(i)); }
              let hash = compute_hash(&ev).map_err(|_| VerifyError::ItemStampInvalid(i))?;
              // Stamp checked against manifest.brain_verifying_key ONLY (A6, no per-item did resolution).
              verify_hash(&hash, sig, brain_vk).map_err(|_| VerifyError::ItemStampInvalid(i))?;
              // Verifier-derived origin. External note → the pinned copy; else "origin unattested".
              Ok(if is_external(&ev) {
                  "this brain recorded these bytes; provenance of the underlying text is not asserted".into()
              } else {
                  "origin unattested".into() // is_external=false ∧ kind≠dossier (C-NEW-1b)
              })
          }
          ItemClass::SealVouched => {
              // Exporter-asserted label, rendered weaker (H2/C-NEW-1). Cross-check happens visually,
              // not cryptographically — there is no recomputable origin for these classes.
              let label = item.origin_label.clone().unwrap_or_default();
              Ok(format!("labeled machine-derived by the exporter; not independently verified ({label})"))
          }
      }
  }

  fn decode_vk(mb: &str) -> Option<VerifyingKey> {
      VerifyingKey::from_bytes(&multibase_key(mb)?).ok()
  }

  /// Minimal in-module major-version parse (avoids an external `semver` dep). `"1.2.3" → Some(1)`.
  mod semver_lite {
      pub fn major(v: &str) -> Option<u64> { v.split('.').next()?.parse().ok() }
  }
  ```
  Add `pub mod verify;` + `pub use verify::{verify, IdentityLevel, Verdict, VerifyError};` to `lib.rs`. (`OriginMismatch` is reserved in the enum for the exporter-lied-origin case; wire it in the tamper test below by hand-crafting a stamped item whose `kind`/label the daemon would set — for SP-V1 the stamped label is verifier-derived so `OriginMismatch` fires only if a future carried label is added; keep the arm + a dedicated test that a mismatched carried label is rejected once carried. Document this: stamped items in SP-V1 carry NO display label, so `OriginMismatch` is exercised by the conformance vector in Task 12, not producible by `build_bundle`. If a reviewer wants it live now, add an optional `carried_origin` field to stamped items and compare — noted as a plan decision, deferred to keep the stamped shape lean.)

- [ ] Write the tamper-matrix + forgery + no-leak tests. Put a shared `valid_bundle()` helper (build via `build_bundle` with a real brain key + signed binding + one real stamped note whose event you sign, and one seal-vouched session) at the top of the test module. Then table-driven flips:
  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      use bossclaw_canon::event::{canonical_bytes, compute_hash, Event};
      use bossclaw_canon::sign::{sign_bytes, sign_hash, SigningKey};
      use crate::binding::binding_signing_bytes;
      use crate::build::{build_bundle, BuildInput, ItemInput};
      use crate::format::{Binding, BindingPayload};
      use crate::resolver::OfflineResolver;

      struct Fx { bundle: Airmem, brain: SigningKey }

      fn valid_bundle() -> Fx {
          let brain = SigningKey::from_bytes(&[1u8; 32]);
          let brain_mb = multibase::encode(multibase::Base::Base58Btc, brain.verifying_key().to_bytes());
          // A real external note event, signed by the brain key (the write-time stamp).
          let mut ev = Event {
              id: "01J0000000000000000000000A".into(), ts: "2026-06-15T00:00:00Z".into(),
              valid_time: None, event_type: "memory".into(),
              content: serde_json::json!({ "text": "shared note", "origin": "external" }),
              model_meta: None, prev_hash: "00".repeat(32), hash: None,
              signed_by_did: "did:wba:example.com:me".into(), signature: None,
          };
          let event_bytes = String::from_utf8(canonical_bytes(&ev).unwrap()).unwrap();
          let sig = sign_hash(&compute_hash(&ev).unwrap(), &brain);
          let _ = &mut ev;
          // Binding by a real identity key.
          let idk = SigningKey::from_bytes(&[9u8; 32]);
          let idvk = multibase::encode(multibase::Base::Base58Btc, idk.verifying_key().to_bytes());
          let payload = BindingPayload {
              brain_verifying_key: brain_mb.clone(), identity_verifying_key: idvk,
              did: "did:wba:example.com:me".into(), purpose: "memory-signing".into(),
              epoch: 1, created_at: "2026-07-21T00:00:00Z".into(),
          };
          let bsig = sign_bytes(&binding_signing_bytes(&payload), &idk);
          let bundle = build_bundle(BuildInput {
              created_at: "2026-07-21T00:00:00Z".into(), did: "did:wba:example.com:me".into(),
              brain_verifying_key: brain_mb, selection_description: "1 note + 1 session".into(),
              items: vec![
                  ItemInput::Stamped { event_bytes, signature: sig },
                  ItemInput::SealVouched { kind: "session".into(), content: "session body".into(),
                      display: serde_json::json!({"title":"S","project":"repo"}),
                      origin_label: "captured session".into() },
              ],
              binding: Binding { payload, identity_signature: bsig },
              brain_key: &brain,
          });
          Fx { bundle, brain }
      }

      fn off() -> OfflineResolver { OfflineResolver }

      #[test]
      fn valid_bundle_verifies_green_offline() {
          let v = verify(&valid_bundle().bundle, &off()).expect("valid bundle verifies");
          assert_eq!(v.identity, IdentityLevel::UnverifiedOffline);
          assert_eq!(v.item_labels.len(), 2);
          assert!(v.item_labels[0].contains("provenance of the underlying text is not asserted"));
          assert!(v.item_labels[1].contains("not independently verified"));
      }

      // ---- TAMPER MATRIX: one flip per field class → its specific §7 error ----
      #[test] fn flip_seal() {
          let mut b = valid_bundle().bundle; b.seal = "zBAD".into();
          assert_eq!(verify(&b, &off()), Err(VerifyError::SealInvalid));
      }
      #[test] fn flip_item_content_stamped() {
          let mut b = valid_bundle().bundle;
          b.items[0].event_bytes = Some(b.items[0].event_bytes.clone().unwrap().replace("shared", "forged"));
          // leaf recompute trips FIRST (content is inside the leaf) → ItemHashMismatch.
          assert_eq!(verify(&b, &off()), Err(VerifyError::ItemHashMismatch(0)));
      }
      #[test] fn flip_item_leaf_field() {
          let mut b = valid_bundle().bundle; b.items[0].leaf = "00".repeat(32);
          assert_eq!(verify(&b, &off()), Err(VerifyError::ItemHashMismatch(0)));
      }
      #[test] fn flip_original_signature() {
          // Fix the leaf so the stamp check (not the leaf) is what fails.
          let mut b = valid_bundle().bundle;
          b.items[0].signature = Some("z3yq".into());
          b.items[0].leaf = hex::encode(merkle::leaf_hash(&b.items[0]));
          // The Merkle root now differs (leaf changed) → recompute root too, so re-seal is impossible
          // for an attacker; but here we only proved the signature path. Assert stamp-invalid by
          // rebuilding leaves+root+reseal via a helper is out of scope — instead flip the signature
          // WITHOUT touching the leaf-excluded set: signature IS leaf-excluded? No — signature is in
          // the item, inside the leaf. So a lone signature flip trips ItemHashMismatch. Documented:
          // the stamp-invalid path is exercised by the FOREIGN-EVENT LAUNDERING test below (a validly
          // self-signed event by a different key, leaf-consistent, re-sealed) → ItemStampInvalid.
          assert!(matches!(verify(&b, &off()), Err(VerifyError::ItemHashMismatch(0))));
      }
      #[test] fn flip_merkle_root() {
          let mut b = valid_bundle().bundle; b.manifest.merkle_root = "00".repeat(32);
          // manifest changed → seal fails first (headline). Re-seal to isolate TreeMismatch:
          let brain = SigningKey::from_bytes(&[1u8; 32]);
          b.seal = sign_bytes(&crate::format::canonical_json(&b.manifest).unwrap(), &brain);
          assert_eq!(verify(&b, &off()), Err(VerifyError::TreeMismatch));
      }
      #[test] fn flip_binding_hash() {
          let mut b = valid_bundle().bundle; b.manifest.binding_hash = "00".repeat(32);
          let brain = SigningKey::from_bytes(&[1u8; 32]);
          b.seal = sign_bytes(&crate::format::canonical_json(&b.manifest).unwrap(), &brain);
          assert_eq!(verify(&b, &off()), Err(VerifyError::BindingHashMismatch));
      }
      #[test] fn flip_manifest_did_without_binding() {
          let mut b = valid_bundle().bundle; b.manifest.did = "did:wba:evil.com:x".into();
          let brain = SigningKey::from_bytes(&[1u8; 32]);
          b.seal = sign_bytes(&crate::format::canonical_json(&b.manifest).unwrap(), &brain);
          assert_eq!(verify(&b, &off()), Err(VerifyError::BindingDidMismatch));
      }
      #[test] fn format_too_new() {
          let mut b = valid_bundle().bundle; b.manifest.format_version = "2.0.0".into();
          let brain = SigningKey::from_bytes(&[1u8; 32]);
          b.seal = sign_bytes(&crate::format::canonical_json(&b.manifest).unwrap(), &brain);
          assert_eq!(verify(&b, &off()), Err(VerifyError::FormatTooNew));
      }

      // ---- RE-ATTRIBUTION FORGERY (C1): replace binding with a fresh attestation by a DIFFERENT
      //      identity over the SAME brain key. Both layers fail independently. ----
      #[test] fn re_attribution_forgery() {
          let mut b = valid_bundle().bundle;
          let attacker = SigningKey::from_bytes(&[42u8; 32]);
          let avk = multibase::encode(multibase::Base::Base58Btc, attacker.verifying_key().to_bytes());
          let payload = BindingPayload {
              brain_verifying_key: b.manifest.brain_verifying_key.clone(),
              identity_verifying_key: avk, did: "did:wba:evil.com:attacker".into(),
              purpose: "memory-signing".into(), epoch: 1, created_at: "2026-07-21T00:00:00Z".into(),
          };
          let asig = sign_bytes(&binding_signing_bytes(&payload), &attacker);
          b.binding = Binding { payload, identity_signature: asig };
          // binding_hash no longer matches the sealed manifest → BindingHashMismatch (layer 1). Even if
          // the attacker recomputes binding_hash and re-seals, binding.did ≠ manifest.did → BindingDid.
          assert_eq!(verify(&b, &off()), Err(VerifyError::BindingHashMismatch));
      }

      // ---- FOREIGN-EVENT LAUNDERING (A6): a stamped item validly signed by a DIFFERENT brain key,
      //      re-sealed under this bundle's key → ItemStampInvalid (stamp checked vs manifest key ONLY).
      #[test] fn foreign_event_laundering() {
          let brain = SigningKey::from_bytes(&[1u8; 32]);
          let foreign = SigningKey::from_bytes(&[77u8; 32]);
          let ev = Event {
              id: "01J000000000000000000000FF".into(), ts: "2026-06-15T00:00:00Z".into(),
              valid_time: None, event_type: "memory".into(),
              content: serde_json::json!({ "text": "foreign", "origin": "external" }),
              model_meta: None, prev_hash: "00".repeat(32), hash: None,
              signed_by_did: "did:wba:foreign.com:x".into(), signature: None,
          };
          let event_bytes = String::from_utf8(canonical_bytes(&ev).unwrap()).unwrap();
          let fsig = sign_hash(&compute_hash(&ev).unwrap(), &foreign); // valid in ISOLATION
          let mut b = valid_bundle().bundle;
          b.items[0] = AirmemItem { leaf: String::new(), class: ItemClass::Stamped, kind: "note".into(),
              event_bytes: Some(event_bytes), signature: Some(fsig), content: None, display: None,
              origin_label: None };
          b.items[0].leaf = hex::encode(merkle::leaf_hash(&b.items[0]));
          // Re-root + re-seal so ONLY the stamp check can fail (isolates A6).
          let leaves: Vec<_> = b.items.iter().map(merkle::leaf_hash).collect();
          b.manifest.merkle_root = hex::encode(merkle::root(&leaves));
          b.seal = sign_bytes(&crate::format::canonical_json(&b.manifest).unwrap(), &brain);
          assert_eq!(verify(&b, &off()), Err(VerifyError::ItemStampInvalid(0)));
      }

      // ---- SEAL-VOUCHED NO-LEAK (A-N1 + A4/C5): the file contains NO forbidden fields anywhere. ----
      #[test] fn seal_vouched_discloses_no_local_metadata() {
          let b = valid_bundle().bundle;
          let whole = serde_json::to_string(&b).unwrap();
          for needle in ["source_event_ids", "prompt_hash", "session_id", "grant_root", "canonical_path"] {
              assert!(!whole.contains(needle), "seal-vouched item leaked `{needle}`");
          }
          // The seal-vouched item carries NO raw event bytes.
          assert!(b.items[1].event_bytes.is_none());
      }
  }
  ```
  (Note the honest inline comment on `flip_original_signature`: a lone signature flip trips `ItemHashMismatch` because the signature lives inside the leaf; the true stamp-invalid path is proven by `foreign_event_laundering`. Keep both — the matrix stays complete and honest.)

- [ ] Run (foreground): `cargo test -p bossclaw-bundle verify`. Expected: all pass. Then `cargo test -p bossclaw-bundle` (whole crate) green.

- [ ] Commit: `git add crates/bossclaw-bundle/src/verify.rs crates/bossclaw-bundle/src/lib.rs && git commit -m "feat(rung5): verify L1 checklist + §7 error enum + tamper matrix + forgery + no-leak"`

---

## Task 7 — L2 as an injectable resolver trait (offline default + mocked registry)

The real HTTP registry lookup is SP-V2; SP-V1 ships the seam + an offline default + a mocked-resolver test proving the L2 branch.

**Files:**
- Create `crates/bossclaw-bundle/src/resolver.rs`
- Modify `crates/bossclaw-bundle/src/lib.rs` (`pub mod resolver;` + re-exports); this task must precede Task 6's compile — reorder so `resolver.rs` exists before `verify.rs` references it, OR land both in one commit. (Recommended: create `resolver.rs` as the FIRST step of Task 6; this task then only adds the mocked-resolver test. Kept separate here for reviewability.)

Steps:

- [ ] Write `crates/bossclaw-bundle/src/resolver.rs`:
  ```rust
  //! The L2 identity-resolution seam. Registry-mediated ONLY (spec §2.5 finding C7): keyed by did,
  //! NEVER a fetch of the did's own domain. The real HTTPS impl is SP-V2; SP-V1 ships the trait, an
  //! offline default (always None → stays L1), and a mock for tests.
  pub trait IdentityResolver {
      /// Resolve `did` to its registry-published identity verifying key (multibase), or `None` when
      /// the did is unresolvable / non-registry / offline. `None` keeps the verdict at L1.
      fn resolve(&self, did: &str) -> Option<String>;
  }

  /// The `--offline` default: resolves nothing, so identity renders "unverified (offline)".
  pub struct OfflineResolver;
  impl IdentityResolver for OfflineResolver {
      fn resolve(&self, _did: &str) -> Option<String> { None }
  }

  #[cfg(test)]
  pub(crate) struct MockResolver {
      pub did: String,
      pub key: String,
  }
  #[cfg(test)]
  impl IdentityResolver for MockResolver {
      fn resolve(&self, did: &str) -> Option<String> {
          if did == self.did { Some(self.key.clone()) } else { None }
      }
  }
  ```
  Add `pub mod resolver;` + `pub use resolver::{IdentityResolver, OfflineResolver};` to `lib.rs`.

- [ ] Add the mocked-resolver L2 test to `verify.rs`'s test module (proves the RegistryResolved branch + the key-mismatch `IdentityUnresolved`):
  ```rust
  #[test]
  fn l2_registry_resolved_when_key_matches() {
      let b = valid_bundle().bundle;
      let resolver = crate::resolver::MockResolver {
          did: b.manifest.did.clone(),
          key: b.binding.payload.identity_verifying_key.clone(),
      };
      let v = verify(&b, &resolver).unwrap();
      assert_eq!(v.identity, IdentityLevel::RegistryResolved);
  }
  #[test]
  fn l2_unresolved_when_registry_key_differs() {
      let b = valid_bundle().bundle;
      let resolver = crate::resolver::MockResolver { did: b.manifest.did.clone(), key: "zWRONG".into() };
      assert_eq!(verify(&b, &resolver), Err(VerifyError::IdentityUnresolved));
  }
  ```
  Run (foreground): `cargo test -p bossclaw-bundle`. Expected: green.

- [ ] Commit: `git add crates/bossclaw-bundle/src/resolver.rs crates/bossclaw-bundle/src/lib.rs crates/bossclaw-bundle/src/verify.rs && git commit -m "feat(rung5): L2 injectable IdentityResolver seam (offline default + mocked test)"`

---

## Task 8 — Core: binding storage primitives + `EventLog::export_bundle`

Store the app-minted binding as a signed config event (house pattern), expose the brain verifying key, and add the pure-read export that gathers items + calls `bossclaw-bundle`. This is core-only, testable against a real in-memory `EventLog`.

**Files:**
- Modify `crates/bossclaw-core/Cargo.toml` (add `bossclaw-bundle` path dep)
- Modify `crates/bossclaw-core/src/log.rs` (config const + `ConfigFlag` arm near line 285-321; storage/reader + brain key getter + `export_bundle`)

Steps:

- [ ] Add the dep + config key. In `crates/bossclaw-core/Cargo.toml` `[dependencies]` add `bossclaw-bundle = { path = "../bossclaw-bundle" }`. In `log.rs` add a const beside the others (near line 278): `const BINDING_KEY: &str = "identity_binding";` and add `Binding` to the `ConfigFlag` enum (near line 285) + its `key()` arm (near line 310) returning `BINDING_KEY`. (Mirrors `Reflect`/`REFLECT_ENABLED_KEY` exactly — `log.rs:278,304,321`.)

- [ ] Write the failing storage test first (in `log.rs`'s test module — mirror the reasoner-config round-trip test near `log.rs:11874`). Reuse the module's existing in-memory helper `open_log(dir.path())` (`log.rs:10308`, `dir = tempfile::tempdir()`, `KEY_BYTES=[7u8;32]`) — do NOT invent a constructor:
  ```rust
  #[test]
  fn binding_stores_signed_and_reads_back_highest_epoch() {
      let dir = tempfile::tempdir().unwrap();
      let log = open_log(dir.path());
      assert!(log.latest_binding().unwrap().is_none(), "no binding yet");
      let v1 = serde_json::json!({ "payload": { "epoch": 1, "did": "did:wba:x" }, "identity_signature": "zA" });
      let v2 = serde_json::json!({ "payload": { "epoch": 2, "did": "did:wba:x" }, "identity_signature": "zB" });
      log.set_binding(v1.clone()).unwrap();
      log.set_binding(v2.clone()).unwrap();
      let latest = log.latest_binding().unwrap().unwrap();
      assert_eq!(latest["payload"]["epoch"], 2, "highest epoch wins");
      assert!(log.verify_chain().is_ok(), "binding writes are signed + chained");
  }
  ```
  (Use the existing test constructor the surrounding tests use — grep `fn test_log(` / `open_in_memory` in `log.rs` tests and reuse it; do not invent one.)

- [ ] Implement the storage + readers in `log.rs` (mirror `set_reasoner_config` at `log.rs:7766` for the writer and `latest_config_value` at `log.rs:7902` for the reader; "highest epoch wins" scans all binding configs):
  ```rust
  /// Store an app-minted identity binding as a signed `config` event (house pattern — durable,
  /// tamper-evident via `verify_chain`). The ONLY writer of `BINDING_KEY`. Mirrors
  /// [`EventLog::set_reasoner_config`]. The daemon VALIDATES the attestation (identity-sig +
  /// brain-key match + epoch first-write-wins) BEFORE calling this (spec §2.3 C-NEW-4).
  pub fn set_binding(&self, attestation: serde_json::Value) -> Result<(), BossclawError> {
      self.append(Event {
          id: String::new(), ts: String::new(), valid_time: None,
          event_type: CONFIG_EVENT_TYPE.to_string(),
          content: serde_json::Value::Object({
              let mut m = serde_json::Map::new();
              m.insert(BINDING_KEY.to_string(), attestation);
              m
          }),
          model_meta: None, prev_hash: String::new(), hash: None,
          signed_by_did: self.signer_did(), signature: None,
      })?;
      Ok(())
  }

  /// The stored binding with the HIGHEST `payload.epoch`, or `None` if never set (spec §2.3
  /// "the latest stored binding (highest epoch)"). Scans every `config` event carrying `BINDING_KEY`.
  pub fn latest_binding(&self) -> Result<Option<serde_json::Value>, BossclawError> {
      let store = self.inner.lock().expect(POISON);
      let conn = store.conn();
      let mut stmt = conn.prepare("SELECT payload FROM events WHERE event_type = ?1 ORDER BY seq DESC")?;
      let rows = stmt.query_map([CONFIG_EVENT_TYPE], |r| r.get::<_, String>(0))?;
      let mut best: Option<(u64, serde_json::Value)> = None;
      for row in rows {
          let ev: Event = serde_json::from_str(&row?)?;
          if let Some(v) = ev.content.get(BINDING_KEY) {
              let epoch = v.get("payload").and_then(|p| p.get("epoch")).and_then(|e| e.as_u64()).unwrap_or(0);
              if best.as_ref().map(|(e, _)| epoch > *e).unwrap_or(true) {
                  best = Some((epoch, v.clone()));
              }
          }
      }
      Ok(best.map(|(_, v)| v))
  }

  /// The set of epochs already stored (for the daemon's first-write-wins idempotency check).
  pub fn binding_epochs(&self) -> Result<std::collections::HashSet<u64>, BossclawError> {
      let store = self.inner.lock().expect(POISON);
      let conn = store.conn();
      let mut stmt = conn.prepare("SELECT payload FROM events WHERE event_type = ?1")?;
      let rows = stmt.query_map([CONFIG_EVENT_TYPE], |r| r.get::<_, String>(0))?;
      let mut set = std::collections::HashSet::new();
      for row in rows {
          let ev: Event = serde_json::from_str(&row?)?;
          if let Some(e) = ev.content.get(BINDING_KEY).and_then(|v| v.get("payload"))
              .and_then(|p| p.get("epoch")).and_then(|e| e.as_u64()) { set.insert(e); }
      }
      Ok(set)
  }

  /// The daemon's ACTUAL brain verifying key, multibase base58btc — the value the app must round-trip
  /// (spec §2.3) and every seal/stamp is checked against (A6). Derived from the log's signing key.
  pub fn brain_verifying_key_multibase(&self) -> String {
      multibase::encode(multibase::Base::Base58Btc, self.key.verifying_key().to_bytes())
  }
  ```
  Run (foreground): `cargo test -p bossclaw-core binding`. Expected: pass.

- [ ] Add `EventLog::export_bundle` — pure-read: `verify_chain` first, `EmptySelection`/`BindingUnavailable` guards, gather notes (stamped) + sessions (seal-vouched), then `bossclaw_bundle::build_bundle`. Selection = explicit event-id lists (grounded in `current_notes`/`current_sessions` ids). Returns the canonical `.airmem` JSON text. Define a small `ExportError` (or reuse `BossclawError` variants — use `BossclawError::InvalidInput` for `EmptySelection`, `BossclawError::Chain` for the chain-invalid case surfaced by `verify_chain`, and a new `BossclawError::InvalidInput("no identity binding stored")` for `BindingUnavailable`; the daemon maps these to typed refusals):
  ```rust
  /// The owner's export selection: current-note ids (stamped) + current-session event ids (seal-vouched).
  pub struct ExportSelection {
      /// `CurrentNote.event_id`s to export as stamped items.
      pub note_event_ids: Vec<String>,
      /// `CurrentSession.event_id`s to export as seal-vouched items.
      pub session_event_ids: Vec<String>,
      /// Free-text description shown to the receiver.
      pub description: String,
      /// Export time, RFC3339 (daemon-supplied — core stays clock-free at this seam? No: append() reads
      /// the clock already; pass it so the manifest `created_at` is caller-controlled and testable).
      pub created_at: String,
  }

  /// Build a sealed `.airmem` for `selection`. PURE-READ (spec §2.4): mutates nothing, no network.
  /// Errors: chain-invalid (`verify_chain`), empty selection, or no stored binding.
  pub fn export_bundle(&self, selection: &ExportSelection) -> Result<String, BossclawError> {
      self.verify_chain()?; // S4 — never seal from a sick brain (ChainInvalid)
      if selection.note_event_ids.is_empty() && selection.session_event_ids.is_empty() {
          return Err(BossclawError::InvalidInput("empty selection".into()));
      }
      let binding_json = self.latest_binding()?
          .ok_or_else(|| BossclawError::InvalidInput("no identity binding stored".into()))?;
      let binding: bossclaw_bundle::Binding = serde_json::from_value(binding_json)
          .map_err(|e| BossclawError::Canonical(format!("stored binding malformed: {e}")))?;
      let did = binding.payload.did.clone();

      let mut items: Vec<bossclaw_bundle::ItemInput> = Vec::new();
      // Stamped notes: disclosed canonical event bytes + the original write-time signature.
      for id in &selection.note_event_ids {
          let ev = self.event_by_id(id)?
              .ok_or_else(|| BossclawError::InvalidInput(format!("note {id} not found")))?;
          let signature = ev.signature.clone()
              .ok_or_else(|| BossclawError::Chain(format!("note {id} unsigned")))?;
          let event_bytes = String::from_utf8(crate::event::canonical_bytes(&ev)?)
              .map_err(|e| BossclawError::Canonical(e.to_string()))?;
          items.push(bossclaw_bundle::ItemInput::Stamped { event_bytes, signature });
      }
      // Seal-vouched sessions: content + SAFE display only (NO path/session_id/sha — A-N1).
      let sessions = self.current_sessions()?;
      for id in &selection.session_event_ids {
          let s = sessions.iter().find(|s| &s.event_id == id)
              .ok_or_else(|| BossclawError::InvalidInput(format!("session {id} not found")))?;
          let content = std::fs::read_to_string(&s.path)
              .map_err(|e| BossclawError::Io(e))?;
          let display = serde_json::json!({
              "title": s.title, "project": s.project, "tool": s.tool,
              "started_at": s.started_at, "ended_at": s.ended_at, "approx_bytes": s.approx_bytes,
          }); // deliberately NO path, NO session_id, NO sha256 (leak boundary)
          items.push(bossclaw_bundle::ItemInput::SealVouched {
              kind: "session".into(), content, display, origin_label: "captured session".into(),
          });
      }

      let bundle = bossclaw_bundle::build_bundle(bossclaw_bundle::BuildInput {
          created_at: selection.created_at.clone(), did,
          brain_verifying_key: self.brain_verifying_key_multibase(),
          selection_description: selection.description.clone(),
          items, binding, brain_key: &self.key,
      });
      String::from_utf8(bossclaw_bundle::format::canonical_json(&bundle)
          .map_err(|e| BossclawError::Canonical(e.to_string()))?)
          .map_err(|e| BossclawError::Canonical(e.to_string()))
  }
  ```
  Export `ExportSelection` from `crates/bossclaw-core/src/lib.rs` (`pub use log::{… ExportSelection}` — add to the existing `pub use log::{…}` block near line 66).

- [ ] Write the end-to-end core export test (append a note via `remember`, store a valid binding, export, then round-trip through `bossclaw_bundle::verify` with the offline resolver → green). This is the load-bearing integration proof for the whole export side:
  ```rust
  #[test]
  fn export_bundle_verifies_green_end_to_end() {
      let dir = tempfile::tempdir().unwrap();
      let log = open_log(dir.path());
      let embedder = MockEmbedder::new(8); // match the dim used by sibling log.rs tests
      let note_id = log.remember(&embedder, "a shared fact").unwrap();
      // Mint a binding whose brain key == this log's key and identity-sign it.
      let brain_mb = log.brain_verifying_key_multibase();
      let idk = ed25519_dalek::SigningKey::from_bytes(&[9u8; 32]);
      let idvk = multibase::encode(multibase::Base::Base58Btc, idk.verifying_key().to_bytes());
      let payload = serde_json::json!({
          "brain_verifying_key": brain_mb, "identity_verifying_key": idvk,
          "did": "did:wba:example.com:me", "purpose": "memory-signing",
          "epoch": 1, "created_at": "2026-07-21T00:00:00Z" });
      let sig_bytes = bossclaw_bundle::binding::binding_signing_bytes(
          &serde_json::from_value(payload.clone()).unwrap());
      let identity_signature = bossclaw_canon::sign::sign_bytes(&sig_bytes, &idk);
      log.set_binding(serde_json::json!({ "payload": payload, "identity_signature": identity_signature })).unwrap();
      let text = log.export_bundle(&crate::log::ExportSelection {
          note_event_ids: vec![note_id], session_event_ids: vec![],
          description: "1 note".into(), created_at: "2026-07-21T00:00:00Z".into(),
      }).unwrap();
      let bundle: bossclaw_bundle::Airmem = serde_json::from_str(&text).unwrap();
      let verdict = bossclaw_bundle::verify(&bundle, &bossclaw_bundle::OfflineResolver).unwrap();
      assert_eq!(verdict.identity, bossclaw_bundle::IdentityLevel::UnverifiedOffline);
  }
  #[test]
  fn export_refuses_empty_selection_and_missing_binding() {
      let dir = tempfile::tempdir().unwrap();
      let log = open_log(dir.path());
      let embedder = MockEmbedder::new(8);
      let id = log.remember(&embedder, "n").unwrap();
      // No binding yet → BindingUnavailable-class error.
      let err = log.export_bundle(&crate::log::ExportSelection {
          note_event_ids: vec![id], session_event_ids: vec![], description: "x".into(),
          created_at: "2026-07-21T00:00:00Z".into() }).unwrap_err();
      assert!(err.to_string().contains("no identity binding stored"));
  }
  ```
  Add `bossclaw-bundle` and `bossclaw-canon` + `multibase` as needed to `crates/bossclaw-core` `[dev-dependencies]` (bundle/canon are already normal deps; add `multibase = "0.9"` under dev-deps only if the test needs the encode helper directly — it does, but `multibase` is already a normal dep of core, so no dev-dep needed).

- [ ] Run (foreground): `cargo test -p bossclaw-core export`. Expected: pass. Then `cargo test -p bossclaw-core` (whole crate) green (proves the extraction + new code didn't regress the engine).

- [ ] Commit: `git add crates/bossclaw-core/Cargo.toml crates/bossclaw-core/src/log.rs crates/bossclaw-core/src/lib.rs && git commit -m "feat(rung5): core binding storage + EventLog::export_bundle (pure-read, verify_chain-first)"`

---

## Task 9 — Proto + daemon wire ops: `BrainVerifyingKey`, `SetBinding`, `ExportBundle`

Add three App-only ops. **Compile-coherence note:** `dispatch` in `server.rs:266-506` is an EXHAUSTIVE match over `Request` — adding variants breaks the daemon build until every arm is handled. This task therefore lands the proto variants AND the dispatch arms AND the engine methods together in ONE commit; the intermediate `cargo build -p bossclawd-proto` after the proto edit is green (proto has no exhaustive consumer), but `bossclawd` will not build until its dispatch arms exist — that transient is expected and confined to this task.

**Files:**
- Modify `crates/bossclawd-proto/src/lib.rs` (3 `Request` variants near line 266; 3 `Response` variants near line 369; extend the `memory_client_allows_exactly_six_ops` "no" list near line 884)
- Modify `crates/bossclawd/src/engine/mod.rs` (3 `EngineHandle` methods)
- Modify `crates/bossclawd/src/server.rs` (3 dispatch arms)
- Modify `crates/bossclawd/tests/authz.rs` (guest-refusal assertions for the 3 ops)

Steps:

- [ ] Add the proto variants. In `crates/bossclawd-proto/src/lib.rs` `Request` enum (after `ReflectEnabled`, line 259) add:
  ```rust
      /// Rung-5 SP-V1: the daemon's brain verifying key (multibase). App-only (guest-refused by
      /// construction — `Role::allows` is unchanged). The app round-trips it to mint the binding (§2.3).
      BrainVerifyingKey { onboarded: bool },
      /// Rung-5 SP-V1: store an app-minted identity binding (§2.3). App-only. The daemon VALIDATES
      /// (identity-sig verify + brain-key match + epoch first-write-wins) before storing a config event.
      SetBinding { onboarded: bool, attestation: serde_json::Value },
      /// Rung-5 SP-V1: build a sealed `.airmem` for `selection` (§2.4). App-only. Pure-read;
      /// `verify_chain` FIRST. Returns the canonical bundle JSON text.
      ExportBundle { onboarded: bool, selection: ExportSelectionWire },
  ```
  Add the wire selection type near `RetireTarget` (line 273):
  ```rust
  /// The export selection carried on the wire (mirrors `bossclaw_core::log::ExportSelection` minus the
  /// daemon-supplied `created_at`, which the daemon stamps).
  #[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
  pub struct ExportSelectionWire {
      /// Current-note ids to export as stamped items.
      pub note_event_ids: Vec<String>,
      /// Current-session event ids to export as seal-vouched items.
      pub session_event_ids: Vec<String>,
      /// Free-text description shown to the receiver.
      pub description: String,
  }
  ```
  In the `Response` enum (after `ReflectEnabled(bool)`, line 369) add:
  ```rust
      /// `BrainVerifyingKey` result — the multibase brain verifying key.
      BrainVerifyingKey(String),
      /// `ExportBundle` result — the canonical `.airmem` JSON text.
      Bundle(String),
  ```
  (`SetBinding` returns `Response::Ok`.)

- [ ] Pin the App-only guarantee. Extend `memory_client_allows_exactly_six_ops` (line 873) `no` array with the three new ops so a future mis-admission fails here:
  ```rust
      BrainVerifyingKey { onboarded: true },
      SetBinding { onboarded: true, attestation: serde_json::Value::Null },
      ExportBundle { onboarded: true, selection: ExportSelectionWire {
          note_event_ids: vec![], session_event_ids: vec![], description: String::new() } },
  ```
  Add a serde round-trip assertion in `new_variants_round_trip_serde` (line 946) for `ExportBundle` (externally-tagged, back-compat). Run (foreground): `cargo test -p bossclawd-proto`. Expected: green (proto builds; `Role::allows` unchanged → the 3 ops are App-only by construction, exactly like the reflect ops at line 904).

- [ ] Add the `EngineHandle` methods in `crates/bossclawd/src/engine/mod.rs` (mirror `set_reflect_enabled` at line 1201 — `get_or_open` + `spawn_blocking`):
  ```rust
  /// Rung-5: the daemon's brain verifying key (multibase). Gated. App round-trips it to mint the binding.
  pub async fn brain_verifying_key(&self, onboarded: bool) -> Result<String, EngineOpError> {
      let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
      spawn_blocking(move || Ok(log.brain_verifying_key_multibase()))
          .await.map_err(|e| EngineOpError::Join(e.to_string()))?
  }

  /// Rung-5: validate + store an app-minted binding (spec §2.3 C-NEW-4). Rejects a bad identity sig,
  /// a brain-key that isn't ours, or a repeated epoch (first-write-wins).
  pub async fn set_binding(&self, onboarded: bool, attestation: serde_json::Value) -> Result<(), EngineOpError> {
      let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
      spawn_blocking(move || {
          let binding: bossclaw_bundle::Binding = serde_json::from_value(attestation.clone())
              .map_err(|e| EngineOpError::Rejected(format!("malformed binding: {e}")))?;
          // (a) identity signature verifies over jcs(payload) against the embedded identity key.
          if !bossclaw_bundle::verify_binding_internal(&binding) {
              return Err(EngineOpError::Rejected("binding identity signature invalid".into()));
          }
          // (b) brain-key match — the loop-closing check (else every future export BindingKeyMismatches).
          if binding.payload.brain_verifying_key != log.brain_verifying_key_multibase() {
              return Err(EngineOpError::Rejected("binding brain key does not match this brain".into()));
          }
          // (c) epoch first-write-wins.
          let epochs = log.binding_epochs().map_err(|e| EngineOpError::Core(e.to_string()))?;
          if epochs.contains(&binding.payload.epoch) {
              return Err(EngineOpError::Rejected(format!("binding epoch {} already stored", binding.payload.epoch)));
          }
          log.set_binding(attestation).map_err(|e| EngineOpError::Core(e.to_string()))
      }).await.map_err(|e| EngineOpError::Join(e.to_string()))?
  }

  /// Rung-5: build a sealed `.airmem` (spec §2.4). Pure-read; `verify_chain` first (inside the core method).
  pub async fn export_bundle(&self, onboarded: bool, selection: bossclaw_core::log::ExportSelection)
      -> Result<String, EngineOpError> {
      let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
      spawn_blocking(move || log.export_bundle(&selection).map_err(|e| match e {
          // EmptySelection / BindingUnavailable are BossclawError::InvalidInput → typed Rejected,
          // mirroring the existing daemon mapping at `engine/mod.rs:711`. ChainInvalid is Chain → Core.
          bossclaw_core::BossclawError::InvalidInput(m) => EngineOpError::Rejected(m),
          other => EngineOpError::Core(other.to_string()),
      }))
          .await.map_err(|e| EngineOpError::Join(e.to_string()))?
  }
  ```
  Add `bossclaw-bundle = { path = "../bossclaw-bundle" }` to `crates/bossclawd/Cargo.toml` `[dependencies]` (the daemon deserializes/validates the binding). Confirm `EngineOpError::Rejected(String)` exists (it mirrors `OpErrorKindWire::Rejected`, `bossclawd-proto/src/lib.rs:409`).

- [ ] Add the dispatch arms in `crates/bossclawd/src/server.rs` (after the `ReflectEnabled` arm, line 505). The daemon stamps `created_at`:
  ```rust
      Request::BrainVerifyingKey { onboarded } => {
          op_result(engine.brain_verifying_key(onboarded).await, Response::BrainVerifyingKey)
      }
      Request::SetBinding { onboarded, attestation } => {
          unit_result(engine.set_binding(onboarded, attestation).await)
      }
      Request::ExportBundle { onboarded, selection } => {
          let sel = bossclaw_core::log::ExportSelection {
              note_event_ids: selection.note_event_ids,
              session_event_ids: selection.session_event_ids,
              description: selection.description,
              created_at: chrono_now_rfc3339(), // reuse the daemon's existing clock helper (see below)
          };
          op_result(engine.export_bundle(onboarded, sel).await, Response::Bundle)
      }
  ```
  For `created_at`: use the daemon's existing RFC3339 clock. Grep `server.rs`/`engine/mod.rs` for an existing `rfc3339`/`Utc::now().to_rfc3339()` helper and reuse it (the core `append` already stamps event `ts` via `Utc::now().to_rfc3339()` at `log.rs:1211`); if none is exposed at the daemon boundary, add a one-line `fn now_rfc3339() -> String { chrono::Utc::now().to_rfc3339() }` next to `now_unix_secs` (used at `server.rs:495`). Do NOT introduce a new clock abstraction.

- [ ] Run (foreground): `cargo build -p bossclawd` (expected: clean now that all arms exist), then `cargo test -p bossclawd` (existing suites green — dispatch is still exhaustive).

- [ ] Add a real-socket guest-refusal test in `crates/bossclawd/tests/authz.rs` (mirror the `RoleClient` pattern already in that file, lines 20-63). Assert a `MemoryClient` connection gets `NotPermitted` for all three ops, and an `App` connection does NOT (it reaches the engine — `ExportBundle` will `Rejected` with "no identity binding stored" on the fixture, which proves it passed the role gate):
  ```rust
  #[tokio::test]
  async fn rung5_export_ops_are_app_only() {
      let (_dir, sock) = spawn_onboarded_daemon().await;
      let mut guest = RoleClient::connect(&sock, Role::MemoryClient).await;
      for req in [
          Request::BrainVerifyingKey { onboarded: true },
          Request::SetBinding { onboarded: true, attestation: serde_json::Value::Null },
          Request::ExportBundle { onboarded: true, selection: bossclawd_proto::ExportSelectionWire {
              note_event_ids: vec![], session_event_ids: vec![], description: String::new() } },
      ] {
          match guest.call(req).await {
              Response::Err { kind: OpErrorKindWire::NotPermitted, .. } => {}
              other => panic!("guest must be refused, got {other:?}"),
          }
      }
      // App reaches the engine: BrainVerifyingKey succeeds (proves the gate opened for App).
      let mut app = RoleClient::connect(&sock, Role::App).await;
      match app.call(Request::BrainVerifyingKey { onboarded: true }).await {
          Response::BrainVerifyingKey(k) => assert!(k.starts_with('z')),
          other => panic!("App BrainVerifyingKey should succeed, got {other:?}"),
      }
  }
  ```
  Run (foreground): `cargo test -p bossclawd --test authz rung5`. Expected: pass.

- [ ] Commit: `git add crates/bossclawd-proto/src/lib.rs crates/bossclawd/Cargo.toml crates/bossclawd/src/engine/mod.rs crates/bossclawd/src/server.rs crates/bossclawd/tests/authz.rs && git commit -m "feat(rung5): App-only wire ops BrainVerifyingKey/SetBinding/ExportBundle (validated, socket-tested)"`

---

## Task 10 — `air-verify` CLI crate

A tiny native crate (its own bin) so `bossclaw-bundle` stays a pure wasm-clean lib. Zero external deps beyond `bossclaw-bundle` — hand-rolled arg matching per the `air-memory-mcp` zero-dep precedent (`crates/air-memory-mcp/Cargo.toml`).

**Files:**
- Create `crates/air-verify/Cargo.toml`
- Create `crates/air-verify/src/main.rs`
- Create `crates/air-verify/tests/clean_machine.rs`
- Modify `Cargo.toml` (workspace members)

Steps:

- [ ] Add the member (`Cargo.toml` line 3) and create `crates/air-verify/Cargo.toml`:
  ```toml
  [package]
  name = "air-verify"
  version = "0.0.1"
  edition = "2021"
  license = "Apache-2.0"
  description = "air-verify: offline .airmem bundle verifier CLI (Rung-5 SP-V1). L1 self-consistency + honest per-item labels."
  repository = "https://github.com/AgentIdentityRegistry/air-note"

  [[bin]]
  name = "air-verify"
  path = "src/main.rs"

  [dependencies]
  bossclaw-bundle = { path = "../bossclaw-bundle" }
  serde_json = "1"

  [dev-dependencies]
  tempfile = "3"
  ```

- [ ] Write `crates/air-verify/src/main.rs` (std arg parsing; exit codes: 0 = L1 verified, 1 = verification failed, 2 = usage/IO). SP-V1 ships `--offline` only (no HTTP resolver — that's SP-V2):
  ```rust
  //! air-verify: offline .airmem verifier. Usage: `air-verify <file> [--offline]`.
  use std::process::ExitCode;
  use bossclaw_bundle::{verify, Airmem, IdentityLevel, OfflineResolver, VerifyError};

  fn main() -> ExitCode {
      let args: Vec<String> = std::env::args().skip(1).collect();
      let mut file: Option<String> = None;
      for a in &args {
          match a.as_str() {
              "--offline" => {} // the only supported mode in SP-V1 (registry L2 is SP-V2)
              "-h" | "--help" => { eprintln!("usage: air-verify <file.airmem> [--offline]"); return ExitCode::from(2); }
              s if s.starts_with('-') => { eprintln!("unknown flag: {s}"); return ExitCode::from(2); }
              s => { if file.replace(s.to_string()).is_some() {
                  eprintln!("only one file argument allowed"); return ExitCode::from(2); } }
          }
      }
      let Some(path) = file else { eprintln!("usage: air-verify <file.airmem> [--offline]"); return ExitCode::from(2); };
      let text = match std::fs::read_to_string(&path) {
          Ok(t) => t, Err(e) => { eprintln!("cannot read {path}: {e}"); return ExitCode::from(2); }
      };
      let bundle: Airmem = match serde_json::from_str(&text) {
          Ok(b) => b, Err(e) => { println!("❌ INVALID — not a well-formed .airmem: {e}"); return ExitCode::from(1); }
      };
      match verify(&bundle, &OfflineResolver) {
          Ok(v) => {
              println!("✅ VERIFIED (L1 self-consistent)");
              println!("recorder did: {}", bundle.manifest.did);
              println!("identity: {}", match v.identity {
                  IdentityLevel::UnverifiedOffline => "unverified (offline)",
                  IdentityLevel::RegistryResolved => "registry-resolved",
              });
              for (i, label) in v.item_labels.iter().enumerate() { println!("  item {i}: {label}"); }
              // H5 — provenance is not truth (renders on every surface).
              println!("note: ✅ proves which registered identity's brain recorded these exact bytes, \
                        dated by its own clock — NOT that the content is true.");
              ExitCode::SUCCESS
          }
          Err(e) => { println!("❌ FAILED — {}", render_err(&e)); ExitCode::from(1) }
      }
  }

  fn render_err(e: &VerifyError) -> String {
      match e {
          VerifyError::SealInvalid => "master seal invalid".into(),
          VerifyError::ItemStampInvalid(i) => format!("item {i}: stamp invalid (or not by this brain)"),
          VerifyError::ItemHashMismatch(i) => format!("item {i}: content hash mismatch"),
          VerifyError::TreeMismatch => "Merkle root mismatch".into(),
          VerifyError::BindingInvalid => "identity binding signature invalid".into(),
          VerifyError::BindingKeyMismatch => "binding brain key ≠ manifest".into(),
          VerifyError::BindingDidMismatch => "binding did ≠ manifest".into(),
          VerifyError::BindingHashMismatch => "binding hash ≠ sealed value".into(),
          VerifyError::OriginMismatch(i) => format!("item {i}: origin label disagrees with signed bytes"),
          VerifyError::IdentityUnresolved => "identity did not resolve (registry)".into(),
          VerifyError::FormatTooNew => "bundle format is newer than this verifier".into(),
          VerifyError::Malformed(d) => format!("malformed: {d}"),
      }
  }
  ```

- [ ] Write the clean-machine e2e in `crates/air-verify/tests/clean_machine.rs` (temp HOME, no keys — build a valid bundle inline via bundle's public API, write it to a temp file, run the bin, assert exit 0; then tamper one byte and assert exit 1). Use `std::process::Command::new(env!("CARGO_BIN_EXE_air-verify"))`:
  ```rust
  use std::process::Command;
  // Reuse bundle's public build API to author a valid fixture (no engine, no keys, no HOME state).
  use bossclaw_bundle::{build_bundle, BuildInput, Binding, BindingPayload, ItemInput, format::canonical_json, binding::binding_signing_bytes};
  use bossclaw_canon::sign::{sign_bytes, SigningKey};

  fn write_valid(dir: &std::path::Path) -> std::path::PathBuf {
      let brain = SigningKey::from_bytes(&[1u8; 32]);
      let brain_mb = multibase::encode(multibase::Base::Base58Btc, brain.verifying_key().to_bytes());
      let idk = SigningKey::from_bytes(&[9u8; 32]);
      let idvk = multibase::encode(multibase::Base::Base58Btc, idk.verifying_key().to_bytes());
      let payload = BindingPayload { brain_verifying_key: brain_mb.clone(), identity_verifying_key: idvk,
          did: "did:wba:example.com:me".into(), purpose: "memory-signing".into(), epoch: 1,
          created_at: "2026-07-21T00:00:00Z".into() };
      let bsig = sign_bytes(&binding_signing_bytes(&payload), &idk);
      let bundle = build_bundle(BuildInput { created_at: "2026-07-21T00:00:00Z".into(),
          did: "did:wba:example.com:me".into(), brain_verifying_key: brain_mb,
          selection_description: "1 session".into(),
          items: vec![ItemInput::SealVouched { kind: "session".into(), content: "body".into(),
              display: serde_json::json!({"title":"S"}), origin_label: "captured".into() }],
          binding: Binding { payload, identity_signature: bsig }, brain_key: &brain });
      let path = dir.join("fixture.airmem");
      std::fs::write(&path, canonical_json(&bundle).unwrap()).unwrap();
      path
  }

  #[test]
  fn cli_verifies_valid_and_rejects_tampered_on_a_clean_home() {
      let dir = tempfile::tempdir().unwrap();
      let path = write_valid(dir.path());
      let ok = Command::new(env!("CARGO_BIN_EXE_air-verify"))
          .arg(&path).arg("--offline").env("HOME", dir.path()).output().unwrap();
      assert!(ok.status.success(), "valid bundle → exit 0; stdout={}", String::from_utf8_lossy(&ok.stdout));
      assert!(String::from_utf8_lossy(&ok.stdout).contains("unverified (offline)"));

      // Flip one content byte → verification fails, exit 1.
      let mut text = std::fs::read_to_string(&path).unwrap();
      text = text.replace("body", "evil");
      let bad = dir.path().join("bad.airmem");
      std::fs::write(&bad, text).unwrap();
      let out = Command::new(env!("CARGO_BIN_EXE_air-verify"))
          .arg(&bad).arg("--offline").output().unwrap();
      assert_eq!(out.status.code(), Some(1));
      assert!(String::from_utf8_lossy(&out.stdout).contains("❌ FAILED"));
  }
  ```
  Add `bossclaw-canon = { path = "../bossclaw-canon" }` and `multibase = "0.9"` to `crates/air-verify` `[dev-dependencies]` (the test authors a fixture). Run (foreground): `cargo test -p air-verify`. Expected: pass.

- [ ] Commit: `git add crates/air-verify Cargo.toml && git commit -m "feat(rung5): air-verify CLI (offline L1 verdict + honest labels + clean-machine e2e)"`

---

## Task 11 — App: tauri command + TS api + Library multi-select + review sheet

Wire the export end-to-end in the desktop: an app-side proxy for the three ops, a tauri command that mints the binding on first export (using the `did:wba` key) and writes the file via the save dialog, a TS wrapper, Library multi-select, and the §6 disclosure review sheet with vitest.

**Files:**
- Modify `apps/desktop/src-tauri/src/engine/client.rs` + `engine/mod.rs` (proxies for the 3 ops — mirror `set_reflect_enabled`, client.rs:459 / mod.rs:518)
- Create `apps/desktop/src-tauri/src/commands/export.rs` (the export command)
- Modify `apps/desktop/src-tauri/src/commands/mod.rs` (`pub mod export;`)
- Modify `apps/desktop/src-tauri/src/main.rs` (register the command in the `generate_handler!` list, line 125)
- Create `apps/desktop/src/api/export.ts`
- Create `apps/desktop/src/memory/ExportReviewSheet.tsx` + `ExportReviewSheet.test.tsx`
- Modify `apps/desktop/src/memory/LibraryPanel.tsx`

Steps:

- [ ] Add the app-side proxies. In `apps/desktop/src-tauri/src/engine/client.rs` add three methods mirroring `set_reflect_enabled` (line 459) — `brain_verifying_key` (returns `Response::BrainVerifyingKey(String)`), `set_binding` (`self.unit(Request::SetBinding{..})`), `export_bundle` (returns `Response::Bundle(String)`). Add the same three passthroughs in `apps/desktop/src-tauri/src/engine/mod.rs` mirroring line 518. Use `bossclawd_proto::ExportSelectionWire` for the selection. Run (foreground): `cargo build -p air_agent_desktop` (crate name per memory) — expected: clean.

- [ ] Write the failing tauri-command test intent, then the command. Create `apps/desktop/src-tauri/src/commands/export.rs`. The command: (1) fetch the brain verifying key from the daemon (round-trip, §2.3); (2) if no binding is stored, mint one — load the `did:wba` identity key (`state.identity_store.load_signing_key()` → `AgentKeypair::from_secret_bytes`, `air/did_wba.rs:13`), read the did from metadata (`state.identity_store.load_metadata()`), build the `BindingPayload`, sign `jcs(payload)` with the identity key, wrap the raw 64-byte sig as multibase base58btc (`did_wba.rs:33` returns raw bytes — wrap per §2.3/§10), `SetBinding`; (3) `ExportBundle`; (4) save via the `tauri-plugin-dialog` save picker — the sibling of `pick_folder`/`pick_file` (mirror `engine_pick_folder`, `commands/engine.rs:159-168`, which uses `app.dialog().file().pick_folder(cb)`). **Confirm the exact save-method name against the installed `tauri-plugin-dialog` version** (v2 exposes `.save_file(cb)` returning an `Option<FilePath>`); the repo has no existing save-dialog call site to copy, so verify before use. Then write the bytes (temp + rename, all-or-nothing — S1). Sketch:
  ```rust
  //! Rung-5 SP-V1 export command. Mints the identity binding on first export, then produces + saves
  //! a sealed .airmem. Publishes nothing (S1: export mutates the brain only via the one-time SetBinding).
  use tauri::State;
  use crate::commands::identity::AppState;
  use bossclawd_proto::ExportSelectionWire;

  #[tauri::command]
  pub async fn export_bundle(
      app: tauri::AppHandle,
      state: State<'_, AppState>,
      note_event_ids: Vec<String>,
      session_event_ids: Vec<String>,
      description: String,
  ) -> Result<Option<String>, String> {
      let onboarded = state.identity_store.is_onboarded();
      // 1. First-export binding mint (idempotent — daemon rejects a repeat epoch; we mint epoch 1 only
      //    when none is stored, checked by attempting SetBinding and ignoring an "already stored" reject).
      ensure_binding(&state, onboarded).await?;
      // 2. Build the sealed bundle (pure-read in the daemon).
      let text = state.engine.export_bundle(onboarded, ExportSelectionWire {
          note_event_ids, session_event_ids, description,
      }).await.map_err(|e| e.to_string())?;
      // 3. Save dialog → temp+rename write. Returns the saved path, or None if cancelled.
      save_airmem(&app, &text).await
  }
  ```
  Implement `ensure_binding` (round-trip the brain key → build payload → `AgentKeypair::sign` → `multibase::encode(Base58Btc, sig)` → `SetBinding`; swallow the daemon's "already stored"/"already stored binding epoch" reject as success) and `save_airmem` (dialog `save_file` + `std::fs::write` to `<path>.tmp` then `std::fs::rename`). Register in `main.rs` `generate_handler!` (add `commands::export::export_bundle,` near line 251) and `commands/mod.rs` (`pub mod export;`). Run (foreground): `cargo build -p air_agent_desktop` and `cargo clippy -p air_agent_desktop --all-targets -- -D warnings`. Expected: clean. (A Rust unit test for `ensure_binding` idempotency can use the existing `RecordingTransport` test double seen in `commands/integrations.rs` tests — mirror that; assert the mint path emits exactly one `Request::SetBinding` and export emits one `Request::ExportBundle`.)

- [ ] Write the TS wrapper `apps/desktop/src/api/export.ts` (mirror `integrations.ts` `invoke` pattern; Tauri camelCase→snake_case for the args):
  ```ts
  import { invoke } from "@tauri-apps/api/core";

  /** Export the selected memories as a signed .airmem. Returns the saved path, or null if cancelled. */
  export const exportBundle = (
    noteEventIds: string[],
    sessionEventIds: string[],
    description: string,
  ): Promise<string | null> =>
    invoke<string | null>("export_bundle", { noteEventIds, sessionEventIds, description });
  ```

- [ ] Write the §6 review sheet + its vitest FIRST (TDD the component). Create `apps/desktop/src/memory/ExportReviewSheet.test.tsx` asserting: (a) a stamped note row states its FULL event bytes ship (A-N1: never shows less than what ships); (b) a seal-vouched session row states content-only + the exporter-asserted label rendered weaker; (c) the S5 plain-language line is present; (d) zero hardcoded colors (tokens only). Then create `ExportReviewSheet.tsx`. Props: `{ notes: {id,text}[]; sessions: {id,title}[]; onConfirm; onCancel }`. Copy per class:
  - stamped note: "Ships the full signed record of this note (its exact bytes + your brain's signature). The receiver can verify it stand-alone."
  - seal-vouched session: "Ships the session's content and title only — no file paths, no session id. Labeled 'captured session' by you; not independently verified."
  - S5 line (always): "Anyone you send this file to can read the plaintext of everything selected. Exporting does not publish anything."
  Use existing `Card`/`Button` from `../components` and CSS tokens (`var(--…)`) only — the shell-redesign gate forbids hardcoded colors (`LibraryPanel.tsx:31`). Run (foreground): `npm --prefix apps/desktop run test -- ExportReviewSheet`. Expected: pass. Also run the repo's hardcoded-color grep gate if one exists (`grep -rn "#[0-9a-fA-F]\{3,6\}" apps/desktop/src/memory/ExportReviewSheet.tsx` → expect none).

- [ ] Wire the entry point in `apps/desktop/src/memory/LibraryPanel.tsx`: add a `Set<string>` selection state for notes + sessions, a checkbox per `NoteRow`/session row (or a "select" affordance), an "Export signed bundle" button that opens `<ExportReviewSheet>`, and on confirm calls `exportBundle(...)`. Keep it minimal and token-styled; reuse the existing list rendering. Add/extend the LibraryPanel vitest if one exists (mirror `MemoryPanel.test.tsx` from memory) to cover: selecting 1 note + clicking Export opens the sheet, confirming invokes `exportBundle` with the selected ids. Run (foreground): `npm --prefix apps/desktop run test` and `npm --prefix apps/desktop run typecheck` (or `tsc --noEmit`). Expected: green.

- [ ] Commit: `git add apps/desktop && git commit -m "feat(rung5): app export — tauri command (binding mint + save), TS api, Library multi-select + disclosure review sheet"`

---

## Task 12 — Conformance vectors + final exit gate

Commit cross-repo `.airmem` fixtures for SP-V2's CI, then run the whole-workspace gate.

**Files:**
- Create `tests/vectors/valid.airmem`, `tests/vectors/tamper_seal.airmem`, `tests/vectors/tamper_item.airmem`, `tests/vectors/tamper_binding_hash.airmem`, `tests/vectors/re_attribution.airmem`, `tests/vectors/README.md`
- Create `crates/bossclaw-bundle/tests/conformance.rs` (loads the committed vectors, asserts each verdict)

Steps:

- [ ] Generate the vectors deterministically. Write `crates/bossclaw-bundle/tests/conformance.rs` with a `#[test]` that BUILDS each fixture from fixed keys (the same synthetic keys as Task 6), writes them to `tests/vectors/` ONLY when `AIRMEM_REGEN=1` is set (otherwise it READS the committed files and asserts the verdict). This keeps the committed fixtures the source of truth for SP-V2's cross-repo CI while making regeneration explicit:
  ```rust
  // Loads committed vectors and asserts each maps to its specific verdict/error. Regenerate with
  // `AIRMEM_REGEN=1 cargo test -p bossclaw-bundle --test conformance` after an INTENTIONAL format change.
  use bossclaw_bundle::{verify, Airmem, OfflineResolver, VerifyError};

  fn load(name: &str) -> Airmem {
      let path = concat_root(name);
      serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("missing vector {path}"))).unwrap()
  }
  fn concat_root(name: &str) -> String { format!("{}/../../tests/vectors/{name}", env!("CARGO_MANIFEST_DIR")) }

  #[test]
  fn valid_vector_verifies() { verify(&load("valid.airmem"), &OfflineResolver).expect("valid"); }
  #[test]
  fn tamper_seal_vector_fails() { assert_eq!(verify(&load("tamper_seal.airmem"), &OfflineResolver), Err(VerifyError::SealInvalid)); }
  #[test]
  fn tamper_item_vector_fails() { assert!(matches!(verify(&load("tamper_item.airmem"), &OfflineResolver), Err(VerifyError::ItemHashMismatch(_)))); }
  #[test]
  fn tamper_binding_hash_vector_fails() { assert_eq!(verify(&load("tamper_binding_hash.airmem"), &OfflineResolver), Err(VerifyError::BindingHashMismatch)); }
  #[test]
  fn re_attribution_vector_fails() { assert!(matches!(verify(&load("re_attribution.airmem"), &OfflineResolver), Err(VerifyError::BindingHashMismatch) | Err(VerifyError::BindingDidMismatch))); }
  ```
  Provide a `regen` helper gated on `std::env::var("AIRMEM_REGEN")` that authors each fixture (reuse the Task 6 `valid_bundle` construction + the tamper transforms) and writes them. Run `AIRMEM_REGEN=1 cargo test -p bossclaw-bundle --test conformance` once to produce the files, then `cargo test -p bossclaw-bundle --test conformance` (reading committed files) — expected: 5 pass. Write `tests/vectors/README.md` documenting the format version, the Merkle rules (A7), and that these fixtures are consumed by air-site's verify-page CI (cross-repo, spec §8).

- [ ] Final exit gate (all foreground). Run in order and require each green:
  - `cargo clippy --workspace --all-targets -- -D warnings` (house convention — the whole workspace, only here).
  - `cargo test -p bossclaw-canon && cargo test -p bossclaw-bundle && cargo test -p air-verify` (new crates).
  - `cargo test -p bossclaw-core && cargo test -p bossclawd-proto && cargo test -p bossclawd` (touched crates — proves the extraction + wire ops didn't regress).
  - `cargo test -p memharness` (recall-neutrality — export is a reader; the suite must stay green untouched, spec §8).
  - `cargo check -p bossclaw-canon --target wasm32-unknown-unknown` (wasm boundary intact).
  - `npm --prefix apps/desktop run test && npm --prefix apps/desktop run typecheck` (frontend).
  - Placeholder sweep: `grep -rn "TODO\|todo!()\|unimplemented!()\|\.skip(\|\.only(" crates/bossclaw-canon crates/bossclaw-bundle crates/air-verify apps/desktop/src/memory/ExportReviewSheet.tsx` → expect no matches (the `todo!()` scaffolds from Task 3 must be gone).

- [ ] Commit: `git add tests/vectors crates/bossclaw-bundle/tests/conformance.rs && git commit -m "feat(rung5): committed .airmem conformance vectors (cross-repo, SP-V2 CI) + final gate green"`

---

## Spec §-coverage map (SP-V1 scope)

- §1 SP-V1 split (canon FIRST, bundle, SetBinding, ExportBundle, CLI, app export) → Tasks 1-11; SP-V2 (verify page, registry HTTP L2, PublishClaim) explicitly OUT.
- §2.1 canon extraction + corrected dep list + C-NEW-3 pins + wasm-clean → Task 1.
- §2.2 format, two item classes, Merkle A7, no-float, binding_hash → Tasks 2, 3, 5, 6.
- §2.3 binding payload (incl. identity_verifying_key + epoch), SetBinding validation C-NEW-4, resolution-from-sealed-manifest → Tasks 4, 8, 9, 11.
- §2.4 wire ops App-only, ExportBundle verify_chain-first → Tasks 8, 9. (`PublishClaim` = SP-V2, omitted.)
- §2.5 L1 checklist incl. binding_hash recompute + IdentityLevel(Unverified offline) + L2 seam → Tasks 6, 7. (Web/wasm verify page = SP-V2.)
- §3 H1-H5 honesty labels (verifier-derived vs exporter-asserted, pinned external copy, origin-unattested, H5 on every surface) → Tasks 6, 10, 11.
- §4 S1/S4/S5 (export mutates nothing/no network, refuse sick chain, plaintext disclosure line), S3 guest scoping → Tasks 8, 9, 11. (S2/S6/S7 = pin/page = SP-V2.)
- §7 error enum + export refusals → Tasks 6 (verify enum), 8/9 (export refusals).
- §8 tamper matrix, re-attribution, foreign-event laundering, exporter-lied-origin, no-leak, canon regression vectors, clean-machine e2e, recall-neutrality, cross-repo conformance → Tasks 1, 6, 8, 10, 12. (Rendering-safety DOM tests = SP-V2.)
- §9 non-goals respected (no PublishClaim, no encryption, no sub-share UX, no authored class, no rotation — epoch reserved only).

## Plan-stage decisions (resolving spec §10 open questions, for reviewer confirmation)

1. **Canon extraction mechanics:** `bossclaw-core` keeps its public surface by `pub use bossclaw_canon::{event, sign};` + module-level re-exports of `EXTERNAL_ORIGIN`/`is_external`; a `From<CanonError> for BossclawError` (variant-for-variant, string-preserving) keeps every internal `?` compiling — zero behavior change, pinned by the pre-existing `tests/vectors.rs` staying green (Task 1).
2. **Brain-key round-trip:** SP-V1 adds a small App-only read op `BrainVerifyingKey` (spec §2.3 says the app "must round-trip the key from the daemon"; the shared-vault-slot alternative exists but the read-op is spec-faithful and lets `SetBinding` close the loop server-side). Flagged for reviewer sign-off as a minor scope addition beyond §2.4's three named ops.
3. **Binding canonical form:** binding keys are bare-32-byte multibase base58btc; the identity signature wraps `did_wba.rs:33`'s raw 64 bytes as multibase base58btc (§2.3/§10). The multikey `0xed01`-prefixed form (`did_wba.rs:93`) is a resolver/display concern (SP-V2), not the binding wire form.
4. **Stamped `OriginMismatch`:** SP-V1 stamped items carry NO exporter display label (label is verifier-derived), so `OriginMismatch` is exercised only by a hand-crafted conformance vector (Task 12), not producible by `build_bundle`. Adding an optional carried label to make it live is deferred; noted inline in Task 6.
5. **Story-C size / streaming vs buffer:** `ExportBundle` buffers the whole `.airmem` into one `Response::Bundle(String)`. `bossclawd-proto::MAX_FRAME` is 32 MiB (`lib.rs:443`) — a whole-brain export beyond that would need streaming; measure on a large fixture (spec §10) before deciding. Noted as a known SP-V1 bound, not a blocker for Stories A/C at current scale.
6. **`ExportSelection` scope:** SP-V1 gathers notes (stamped) + sessions (seal-vouched). Ingested-file extracts and derived dossiers reuse the identical `SealVouched` item path and can be added to the selection later with no format change (the item model already supports `kind ∈ {ingest, dossier}`).
