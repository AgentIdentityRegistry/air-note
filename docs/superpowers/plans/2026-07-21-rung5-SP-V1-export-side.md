# Rung 5 — SP-V1: the export side (bossclaw-canon + bossclaw-bundle + SetBinding/ExportBundle + air-verify CLI + app export UI)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the owner select memories (notes, captured sessions, ingested-file extracts) and export a self-contained, brain-sealed, offline-verifiable `.airmem` bundle (identity-bound via a signed attestation), with a Rust library + `air-verify` CLI that render an L1 self-consistency verdict and honest, kind-aware per-item provenance labels.

**Architecture:** A new wasm-clean leaf crate `bossclaw-canon` is extracted from `bossclaw-core` (zero behavior change) carrying the canonical-bytes/hash/sign primitives so the verifier reproduces the engine's exact bytes without pulling the engine's non-wasm deps; a second leaf crate `bossclaw-bundle` (canon-only) builds and verifies the `.airmem` format (canonical JSON, domain-separated Merkle tree, master seal, identity binding). The daemon gains three App-only wire ops (`BrainVerifyingKey`, `SetBinding`, `ExportBundle`); it reads session bodies through its own bounded capture reader and passes content into a pure-read core export (core stays fs-free for daemon-owned paths); a `did:wba`-signed binding is minted app-side and the Library gains a multi-select export + kind-aware disclosure review sheet.

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
- `crates/bossclaw-bundle/src/merkle.rs` — domain-separated Merkle leaf/root; excludes the `leaf` + `carried_origin` display fields (finding A7).
- `crates/bossclaw-bundle/src/binding.rs` — binding canonical form, `binding_hash`, internal-consistency verify, `binding_signing_bytes`, encoding-agnostic key decode.
- `crates/bossclaw-bundle/src/resolver.rs` — `IdentityResolver` trait + `OfflineResolver` + `MockResolver` (tests).
- `crates/bossclaw-bundle/src/build.rs` — `build_bundle(...)`: assemble items → Merkle root → binding_hash → master seal.
- `crates/bossclaw-bundle/src/verify.rs` — `verify(&Airmem, &dyn IdentityResolver) → Verdict`; L1 checklist + `VerifyError` enum (§7) + kind-aware labels + L2 (decode-both-to-bytes).
- `crates/air-verify/Cargo.toml` — native CLI crate manifest (zero external deps beyond bundle).
- `crates/air-verify/src/main.rs` — `air-verify <file> [--offline]` argument handling + verdict rendering + exit codes.
- `crates/air-verify/tests/clean_machine.rs` — clean-HOME e2e (no keys) over an in-crate-built fixture.
- `tests/vectors/` (repo root) — committed `.airmem` conformance fixtures (valid + each tamper class + origin_mismatch + re_attribution) for SP-V2 cross-repo CI.
- `apps/desktop/src/memory/ExportReviewSheet.tsx` — the §6 kind-aware disclosure review sheet.
- `apps/desktop/src/memory/ExportReviewSheet.test.tsx` — vitest for the review sheet.

Modified:
- `Cargo.toml` (workspace) — add `crates/bossclaw-canon`, `crates/bossclaw-bundle`, `crates/air-verify` members.
- `crates/bossclaw-core/Cargo.toml` — add `bossclaw-canon` + `bossclaw-bundle` path deps.
- `crates/bossclaw-core/src/lib.rs` — re-export canon modules; export `ExportSelection`/`ExportError`/`CurrentIngest`/`MAX_EXPORT_BYTES`.
- `crates/bossclaw-core/src/error.rs` — `impl From<CanonError> for BossclawError` (byte-identical Display).
- `crates/bossclaw-core/src/graph.rs` — replace the `EXTERNAL_ORIGIN` const with a canon re-export.
- `crates/bossclaw-core/src/ingest.rs` — replace the local `is_external` with a canon re-export.
- `crates/bossclaw-core/src/log.rs` — binding storage primitives + brain-key getter + `current_ingests` + `estimate_export_bytes` + `EventLog::export_bundle`.
- `crates/bossclawd-proto/src/lib.rs` — 3 App-only `Request` variants + `ExportSelectionWire` (incl. ingests) + `Response::Bundle`/`Response::BrainVerifyingKey`/`Response::BundleTooLarge` + guest-refusal tests.
- `crates/bossclawd/Cargo.toml` — add `bossclaw-bundle`.
- `crates/bossclawd/src/server.rs` — dispatch arms (estimate→size-guard→read bodies→export) for the 3 ops.
- `crates/bossclawd/src/engine/mod.rs` — `EngineHandle` methods: `brain_verifying_key`, `set_binding`, `estimate_export_bytes`, `export_bundle`.
- `crates/bossclawd/tests/authz.rs` — the 3 new ops are guest-refused.
- `apps/desktop/src-tauri/src/engine/client.rs` + `engine/mod.rs` — app-side proxies for the 3 ops.
- `apps/desktop/src-tauri/src/commands/export.rs` (new) + `commands/mod.rs` — `export_bundle` tauri command (binding mint + file save).
- `apps/desktop/src-tauri/src/main.rs` — register the new command.
- `apps/desktop/src/api/export.ts` (new) — TS wrapper.
- `apps/desktop/src/memory/LibraryPanel.tsx` — multi-select (notes + sessions + ingests) + "Export signed bundle" entry point.

**Status:** **Rev 2.1.** Anchored to spec Rev 3 as amended on-branch (commit `8c5d17a`: Story-C scope honest — ingests in, dossiers empty-by-construction; kind-aware seal-vouched labels; `BundleTooLarge` in §7; `BrainVerifyingKey` in §2.4, commit `472fc02`). SP-V1 scope ONLY (spec §1: export side — Stories A-export + C; L1/offline verification). SP-V2 (verify page, registry L2 HTTP, `PublishClaim` pin) is OUT of scope. **Build is gated behind the R4-A dogfood verdict** — do not start before the go/no-go after Sun 2026-07-27.

**Rev 2 changelog:** Folded the dual plan review (architect SOUND-WITH-CHANGES, critic REWORK, no Blocker). Convergent: added ingested-file gathering (`current_ingests`) so Story C is honest, a pre-frame `BundleTooLarge{bytes,max}` size guard + a measurement checkpoint + a whole-brain large-fixture test, and merged the L2 resolver into Task 6 (resolver created first; renumbered to 11 tasks). Architect: session bodies now read by the daemon through its bounded capture reader (`read_capture_markdown`, 16 MiB) and passed into a fs-free core export, missing body → placeholder; dropped the dead `ConfigFlag::Binding`; L2 key comparison decodes both sides to `[u8;32]` (encoding-agnostic). Critic: `OriginMismatch` is now LIVE via a leaf-excluded `carried_origin` cross-check; a server-side stamped guard (memory + external only); kind-aware seal-vouched labels (session/ingest/dossier, never cross-rendered); `flip_original_signature` now asserts `ItemStampInvalid`; expanded tamper matrix (BindingKeyMismatch, BindingInvalid, re-attribution→BindingDidMismatch, ts-flip, bare-field→SealInvalid); C-NEW-1b origin-unattested test; clippy-slop removed. Citation fix: `open_log` at `log.rs:10309`.

**Rev 2.1 changelog:** Final confirmation round (architect SOUND / critic APPROVE-WITH-CHANGES, 2 Minors). NEW-A: the daemon dispatch now bounds the ACTUAL wire-serialized `Response::Bundle` length against `MAX_FRAME` (the frame JSON-escapes the already-JSON `.airmem`, so the 2 MiB gap below `MAX_EXPORT_BYTES` is smaller than typical JSON-in-JSON escaping — a ~30 MiB bundle could pass the belt yet trip a generic frame error); a wasted-build cannot substitute for the typed `BundleTooLarge`. The pre-build estimate stays as the coarse guard. NEW-B: the seal-vouched no-leak test now includes an ingest item, and a new core test builds a REAL `file_ingested` event (with a `provenance{canonical_path, content_hash, grant_root}` block, ingest.rs:698-711) and asserts the serialized bundle discloses none of those strings.

**Toolchain prerequisite (once):** `rustup target add wasm32-unknown-unknown` (Task 1's wasm check needs it).

**Pre-build measurement checkpoint (Story-C 32 MiB cliff — run BEFORE Task 8, record in the PR):** on a real dogfood brain, estimate the whole-brain export size with the SHIPPED read ops — sum `ListSessions[].approx_bytes` + `Σ len(ListNotes[].text)` + `Σ len(ListFiles ingest text)` (a one-liner over the app's existing `listSessions()`/`listNotes()`/`listFiles()` results, or `air-memory-mcp` over the socket). If the total approaches `MAX_EXPORT_BYTES` (30 MiB), escalate streaming (spec §10) before shipping select-all; `BundleTooLarge` is the shipped guard until then.

---

## Task 1 — Extract `bossclaw-canon` (zero behavior change)

Move the canonical-bytes/hash/sign primitives + the external-origin taint classifier into a wasm-clean leaf crate that `bossclaw-core` re-exports, so every existing `crate::event::…` / `crate::sign::…` / `crate::graph::EXTERNAL_ORIGIN` / `crate::ingest::is_external` call site keeps compiling unchanged.

**Files:**
- Create `crates/bossclaw-canon/Cargo.toml`, `src/lib.rs`, `src/error.rs`, `src/event.rs`, `src/sign.rs`, `tests/vectors.rs`
- Modify `Cargo.toml` (workspace `members`, line 3)
- Modify `crates/bossclaw-core/Cargo.toml` (`[dependencies]`, after line 8)
- Modify `crates/bossclaw-core/src/lib.rs` (module decls near lines 24 & 44; re-export near line 60)
- Modify `crates/bossclaw-core/src/error.rs` (add `From<CanonError>`, after line 56)
- Modify `crates/bossclaw-core/src/graph.rs` (const at line 75)
- Modify `crates/bossclaw-core/src/ingest.rs` (`is_external` at line 716)

Steps:

- [ ] Add the crate to the workspace. Edit `Cargo.toml` line 3 members list to include `"crates/bossclaw-canon"`. Create `crates/bossclaw-canon/Cargo.toml`:
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

- [ ] Write `crates/bossclaw-canon/src/error.rs` — the four variants `event.rs`/`sign.rs` use today (`crates/bossclaw-core/src/error.rs:9,13,17,22`), byte-identical `#[error]` strings:
  ```rust
  //! Error type for the extracted canonical/signing primitives.
  use thiserror::Error;

  /// Errors from canonical-bytes production and Ed25519 hash-signing. Display strings are
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

- [ ] Write `crates/bossclaw-canon/src/event.rs` — a verbatim move of `crates/bossclaw-core/src/event.rs:1-92` with two edits: (a) `use crate::error::CanonError;` replaces the `BossclawError` import and every `BossclawError` in signatures becomes `CanonError`; (b) append the extracted `EXTERNAL_ORIGIN` const + `is_external` fn (moved from `graph.rs:75` and `ingest.rs:716`):
  ```rust
  /// The taint stamp written at `content["origin"]` of every externally-sourced event (remember()
  /// notes, captured sessions, file ingests). Single-sourced so the stamp site and the `is_external`
  /// classifier can never drift. (Moved from graph.rs:75, zero value change.)
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

- [ ] Rewire `bossclaw-core` to re-export canon (the zero-behavior-change seam). In `crates/bossclaw-core/Cargo.toml` `[dependencies]` add `bossclaw-canon = { path = "../bossclaw-canon" }`. In `crates/bossclaw-core/src/lib.rs`: DELETE `pub mod event;` (line 24) and `pub mod sign;` (line 44); ADD `pub use bossclaw_canon::{event, sign};` (so `crate::event::…` and `crate::sign::…` still resolve). Keep `pub use event::{Event, ModelMeta};` (line 60). In `crates/bossclaw-core/src/graph.rs` replace the `EXTERNAL_ORIGIN` const (line 75) with `pub use bossclaw_canon::EXTERNAL_ORIGIN;`. In `crates/bossclaw-core/src/ingest.rs` replace the local `pub fn is_external` (line 716) with `pub use bossclaw_canon::is_external;` (the lib.rs re-export `pub use ingest::{is_external, …}` at line 90 still works). In `crates/bossclaw-core/src/error.rs` add after line 56:
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

- [ ] Run the pin/parity check (foreground): `cargo test -p bossclaw-core --test vectors`. The EXISTING `crates/bossclaw-core/tests/vectors.rs` imports `bossclaw_core::event::{…}` + `bossclaw_core::sign::{…}` and asserts the frozen canonical string + genesis hash `9089b0bd99a3f72e37653c2e8da756aeeb737085c0faa9a1ae5d0defc35dbde9` — it MUST stay green through the re-export (this IS the byte-identity guard). Then `cargo build -p bossclaw-core` — expected: clean (internal call sites resolve).

- [ ] Add canon's own known-answer + cross-version regression vector (C-NEW-3). Write `crates/bossclaw-canon/tests/vectors.rs`:
  ```rust
  use bossclaw_canon::event::{canonical_bytes, compute_hash, is_external, Event, EXTERNAL_ORIGIN};
  use bossclaw_canon::sign::{sign_hash, verify_hash};
  use bossclaw_canon::SigningKey;

  fn fixture() -> Event {
      Event {
          id: "01J0000000000000000000000A".into(), ts: "2026-06-15T00:00:00Z".into(),
          valid_time: None, event_type: "memory".into(),
          content: serde_json::json!({ "text": "hello" }), model_meta: None,
          prev_hash: "00".repeat(32), hash: None,
          signed_by_did: "did:wba:AIR-2JE0-EM7W-JNBK".into(), signature: None,
      }
  }

  #[test]
  fn canonical_bytes_frozen() {
      let expected = r#"{"content":{"text":"hello"},"id":"01J0000000000000000000000A","prev_hash":"0000000000000000000000000000000000000000000000000000000000000000","signed_by_did":"did:wba:AIR-2JE0-EM7W-JNBK","ts":"2026-06-15T00:00:00Z","type":"memory"}"#;
      assert_eq!(String::from_utf8(canonical_bytes(&fixture()).unwrap()).unwrap(), expected);
  }

  #[test]
  fn genesis_hash_frozen() {
      assert_eq!(hex::encode(compute_hash(&fixture()).unwrap()),
          "9089b0bd99a3f72e37653c2e8da756aeeb737085c0faa9a1ae5d0defc35dbde9",
          "a dep bump changed canonical bytes — DO NOT rebase the pins to fix this");
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

- [ ] Prove wasm-cleanliness (foreground): `cargo check -p bossclaw-canon --target wasm32-unknown-unknown`. Expected: clean.

- [ ] Commit: `git add crates/bossclaw-canon Cargo.toml crates/bossclaw-core/Cargo.toml crates/bossclaw-core/src/lib.rs crates/bossclaw-core/src/error.rs crates/bossclaw-core/src/graph.rs crates/bossclaw-core/src/ingest.rs && git commit -m "feat(rung5): extract bossclaw-canon leaf crate (zero behavior change, wasm-clean)"`

---

## Task 2 — `bossclaw-bundle`: the `.airmem` format types + canonical JSON

**Files:**
- Create `crates/bossclaw-bundle/Cargo.toml`, `src/lib.rs`, `src/format.rs`
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
  multibase = "=0.9.2"
  thiserror = "1"
  ```
  (`multibase` is a direct dep — the binding decodes multibase keys; `ed25519-dalek` is reached through `bossclaw-canon` re-exports, keeping the "canon-only" crypto seam.)

- [ ] Write `crates/bossclaw-bundle/src/format.rs`. No floats anywhere (counts `u64`, times RFC3339 strings). The seal signs `jcs(manifest)`; `items`/`binding`/`seal` live OUTSIDE the manifest and are committed via `merkle_root`/`binding_hash`. Stamped items carry `carried_origin` (a display-only origin token cross-checked against the signed event bytes — Merkle-excluded, see merkle.rs); seal-vouched items carry NO origin field (the label is kind-derived by the verifier):
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

  /// One disclosed memory. `leaf` and `carried_origin` are EXCLUDED from the leaf hash (finding A7 +
  /// critic C-origin: `carried_origin` is a display token whose integrity comes from the verifier's
  /// cross-check against the stamp-covered event bytes, so a flip yields `OriginMismatch`, not a hash break).
  #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
  pub struct AirmemItem {
      /// Hex of this item's Merkle leaf hash. Excluded when computing the leaf (see merkle.rs).
      pub leaf: String,
      /// The verification class.
      pub class: ItemClass,
      /// Display kind: `"note"` | `"session"` | `"ingest"` | `"dossier"`. Drives the verifier's label.
      pub kind: String,
      /// STAMPED only: the original event's canonical JSON text (UTF-8; hash/sig excluded, per canon).
      #[serde(skip_serializing_if = "Option::is_none")]
      pub event_bytes: Option<String>,
      /// STAMPED only: the original write-time multibase signature over the event hash.
      #[serde(skip_serializing_if = "Option::is_none")]
      pub signature: Option<String>,
      /// STAMPED only: the carried origin token (`"external"`/`"unattested"`), cross-checked by verify
      /// against the recomputed origin (A5 → `OriginMismatch`). Merkle-excluded.
      #[serde(skip_serializing_if = "Option::is_none")]
      pub carried_origin: Option<String>,
      /// SEAL_VOUCHED only: the disclosed content text (a session ships its FULL transcript).
      #[serde(skip_serializing_if = "Option::is_none")]
      pub content: Option<String>,
      /// SEAL_VOUCHED only: safe display metadata (NEVER paths/session_id/grant_root/lineage — A-N1).
      #[serde(skip_serializing_if = "Option::is_none")]
      pub display: Option<serde_json::Value>,
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
                  format_version: FORMAT_VERSION.into(), created_at: "2026-07-21T00:00:00Z".into(),
                  did: "did:wba:example.com:me".into(), brain_verifying_key: "zBrain".into(),
                  selection_description: "2 items".into(), item_count: 1,
                  merkle_root: "ab".repeat(32), binding_hash: "cd".repeat(32),
              },
              items: vec![AirmemItem {
                  leaf: "ef".repeat(32), class: ItemClass::Stamped, kind: "note".into(),
                  event_bytes: Some("{}".into()), signature: Some("zSig".into()),
                  carried_origin: Some("external".into()), content: None, display: None,
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
          let back: Airmem = serde_json::from_slice(&canonical_json(&a).unwrap()).unwrap();
          assert_eq!(a, back);
      }
      #[test]
      fn canonical_json_is_key_sorted_and_deterministic() {
          let a = sample();
          assert_eq!(canonical_json(&a).unwrap(), canonical_json(&a).unwrap());
          let s = String::from_utf8(canonical_json(&a.manifest).unwrap()).unwrap();
          assert!(s.find("binding_hash").unwrap() < s.find("brain_verifying_key").unwrap(), "JCS sorts keys");
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

- [ ] Write `merkle.rs`. `leaf_hash` blanks BOTH the `leaf` field AND `carried_origin` before hashing (the two display fields excluded from item integrity):
  ```rust
  //! Domain-separated Merkle tree over items (spec §2.2 finding A7): leaf = H(0x00 ‖ item), internal
  //! = H(0x01 ‖ left ‖ right), odd node promoted UNPAIRED (no duplicate-last), leaf order = item order.
  use sha2::{Digest, Sha256};
  use crate::format::AirmemItem;

  /// The 32-byte leaf hash of one item: `SHA256(0x00 ‖ jcs(item_without_display_fields))`. The `leaf`
  /// and `carried_origin` fields are blanked first — `leaf` cannot hash itself, and `carried_origin`
  /// is a cross-checked display token (verify recomputes origin from the signed event bytes).
  pub fn leaf_hash(item: &AirmemItem) -> [u8; 32] {
      let mut naked = item.clone();
      naked.leaf = String::new();
      naked.carried_origin = None;
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
                  h.update([0x01]); h.update(level[i]); h.update(level[i + 1]);
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
              event_bytes: None, signature: None, carried_origin: None,
              content: Some(content.into()), display: None }
      }
      #[test]
      fn leaf_excludes_leaf_and_carried_origin_fields() {
          let a = item("aa", "same");
          let mut b = item("bb", "same"); // different `leaf`
          b.carried_origin = Some("external".into()); // and a carried_origin
          assert_eq!(leaf_hash(&a), leaf_hash(&b), "leaf + carried_origin must not affect the leaf hash");
      }
      #[test]
      fn domain_separation_defeats_leaf_as_internal_second_preimage() {
          let l = leaf_hash(&item("", "l"));
          let r = leaf_hash(&item("", "r"));
          assert_ne!(root(&[l, r]), leaf_hash(&item("", "forge")));
      }
      #[test]
      fn odd_node_promoted_unpaired_not_duplicated() {
          let a = leaf_hash(&item("", "a"));
          let b = leaf_hash(&item("", "b"));
          let c = leaf_hash(&item("", "c"));
          let mut h = Sha256::new(); h.update([0x01]); h.update(a); h.update(b);
          let ab: [u8; 32] = h.finalize().into();
          let mut h2 = Sha256::new(); h2.update([0x01]); h2.update(ab); h2.update(c);
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

- [ ] Run (foreground): `cargo test -p bossclaw-bundle merkle`. Expected: 5 pass.

- [ ] Commit: `git add crates/bossclaw-bundle/src/merkle.rs crates/bossclaw-bundle/src/lib.rs && git commit -m "feat(rung5): domain-separated Merkle tree (frozen A7 rules, display fields excluded)"`

---

## Task 4 — Binding: canonical form, `binding_hash`, internal-consistency verify, encoding-agnostic key decode

**Files:**
- Create `crates/bossclaw-bundle/src/binding.rs`
- Modify `crates/bossclaw-bundle/src/lib.rs` (`pub mod binding;` + re-exports)

Steps:

- [ ] Write `binding.rs`. `decode_ed25519_key` handles BOTH the bare-32-byte multibase form (binding keys) AND the `0xed01`-prefixed multikey form (what the registry publishes, finding #6) so L2 comparison is encoding-agnostic:
  ```rust
  //! Binding (ID card) canonical form: the bytes the identity key signs, the hash the seal commits to,
  //! the L1 internal-consistency check, and an encoding-agnostic Ed25519 key decode. Identity
  //! resolution NEVER starts here (spec §2.3) — the sealed `manifest.did` is authoritative.
  use sha2::{Digest, Sha256};
  use bossclaw_canon::sign::{verify_bytes, VerifyingKey};
  use crate::format::{Binding, BindingPayload};

  /// The exact bytes the identity key signs (and re-verifies): `jcs(payload)`.
  pub fn binding_signing_bytes(payload: &BindingPayload) -> Vec<u8> {
      crate::format::canonical_json(payload).expect("binding payload is always serializable")
  }

  /// `H(jcs(binding))` — the value `manifest.binding_hash` must equal (C-NEW-2). Hashes the WHOLE
  /// binding so a tampered signature also trips `BindingHashMismatch`.
  pub fn binding_hash(binding: &Binding) -> [u8; 32] {
      let canon = crate::format::canonical_json(binding).expect("binding is always serializable");
      Sha256::new().chain_update(&canon).finalize().into()
  }

  /// L1 internal consistency: the identity signature verifies over `jcs(payload)` against the EMBEDDED
  /// `identity_verifying_key`. ZERO identity assurance (the key is attacker-choosable, C3).
  pub fn verify_binding_internal(binding: &Binding) -> bool {
      let Some(raw) = decode_ed25519_key(&binding.payload.identity_verifying_key) else { return false };
      let Ok(vk) = VerifyingKey::from_bytes(&raw) else { return false };
      verify_bytes(&binding_signing_bytes(&binding.payload), &binding.identity_signature, &vk).is_ok()
  }

  /// Decode a multibase Ed25519 public key to its raw 32 bytes, accepting BOTH the bare-32 form and
  /// the `0xed01`-prefixed multikey form (finding #6: the registry publishes multikey; the binding
  /// stores bare — L2 must compare the RAW KEY BYTES, never the encoded strings).
  pub fn decode_ed25519_key(mb: &str) -> Option<[u8; 32]> {
      let (_b, raw) = multibase::decode(mb).ok()?;
      let bytes: &[u8] = if raw.len() == 34 && raw[0] == 0xed && raw[1] == 0x01 { &raw[2..] } else { &raw };
      bytes.try_into().ok()
  }
  ```
  Add `pub mod binding;` + `pub use binding::{binding_hash, binding_signing_bytes, decode_ed25519_key, verify_binding_internal};` to `lib.rs`.

- [ ] Write the test at the bottom of `binding.rs`:
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
          assert!(verify_binding_internal(&signed_binding(&SigningKey::from_bytes(&[3u8; 32]), 1)));
      }
      #[test]
      fn internal_verify_fails_if_payload_tampered() {
          let mut b = signed_binding(&SigningKey::from_bytes(&[3u8; 32]), 1);
          b.payload.did = "did:wba:evil.com:attacker".into();
          assert!(!verify_binding_internal(&b));
      }
      #[test]
      fn binding_hash_covers_signature() {
          let b = signed_binding(&SigningKey::from_bytes(&[3u8; 32]), 1);
          let h = binding_hash(&b);
          assert_eq!(h, binding_hash(&b));
          let mut t = b.clone(); t.identity_signature = "zDIFFERENT".into();
          assert_ne!(h, binding_hash(&t));
      }
      #[test]
      fn decode_accepts_bare_and_multikey_forms() {
          let k = SigningKey::from_bytes(&[5u8; 32]);
          let bytes = k.verifying_key().to_bytes();
          let bare = multibase::encode(multibase::Base::Base58Btc, bytes);
          let mut multikey = vec![0xed, 0x01]; multikey.extend_from_slice(&bytes);
          let mk = multibase::encode(multibase::Base::Base58Btc, &multikey);
          assert_eq!(decode_ed25519_key(&bare).unwrap(), bytes);
          assert_eq!(decode_ed25519_key(&mk).unwrap(), bytes, "multikey and bare decode to the SAME 32 bytes");
      }
  }
  ```
  Run (foreground): `cargo test -p bossclaw-bundle binding`. Expected: 4 pass.

- [ ] Commit: `git add crates/bossclaw-bundle/src/binding.rs crates/bossclaw-bundle/src/lib.rs && git commit -m "feat(rung5): binding canonical form + binding_hash + internal verify + encoding-agnostic key decode"`

---

## Task 5 — `build_bundle`: assemble → root → binding_hash → master seal

**Files:**
- Create `crates/bossclaw-bundle/src/build.rs`
- Modify `crates/bossclaw-bundle/src/lib.rs` (`pub mod build;` + re-exports)

Steps:

- [ ] Write `build.rs`. Class-typed inputs; stamped items carry `carried_origin` (derived by the core caller, which holds the parsed event); seal-vouched carry a kind + content + optional display, NO origin field:
  ```rust
  //! Assemble a sealed `.airmem` from gathered inputs. Pure: no engine, no I/O, no clock (the caller
  //! passes `created_at`). The brain `SigningKey` seals the canonical manifest.
  use bossclaw_canon::sign::{sign_bytes, SigningKey};
  use crate::binding::binding_hash;
  use crate::format::{Airmem, AirmemItem, Binding, ItemClass, Manifest, FORMAT_VERSION};
  use crate::merkle;

  /// One gathered memory to disclose.
  pub enum ItemInput {
      /// External note: canonical event bytes + original write-time signature + derived origin token.
      Stamped { event_bytes: String, signature: String, carried_origin: String },
      /// Session/ingest/dossier: kind + content + optional safe display metadata.
      SealVouched { kind: String, content: String, display: Option<serde_json::Value> },
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
      /// The gathered items, in leaf order (leaf order = item order).
      pub items: Vec<ItemInput>,
      /// The latest stored binding (verbatim).
      pub binding: Binding,
      /// The brain signing key (seals the manifest).
      pub brain_key: &'a SigningKey,
  }

  /// Build + seal. `items` MUST be non-empty (caller enforces `EmptySelection`).
  pub fn build_bundle(input: BuildInput<'_>) -> Airmem {
      let mut items: Vec<AirmemItem> = input.items.into_iter().map(|ii| match ii {
          ItemInput::Stamped { event_bytes, signature, carried_origin } => AirmemItem {
              leaf: String::new(), class: ItemClass::Stamped, kind: "note".into(),
              event_bytes: Some(event_bytes), signature: Some(signature),
              carried_origin: Some(carried_origin), content: None, display: None,
          },
          ItemInput::SealVouched { kind, content, display } => AirmemItem {
              leaf: String::new(), class: ItemClass::SealVouched, kind,
              event_bytes: None, signature: None, carried_origin: None,
              content: Some(content), display,
          },
      }).collect();
      let leaves: Vec<[u8; 32]> = items.iter().map(merkle::leaf_hash).collect();
      for (item, leaf) in items.iter_mut().zip(&leaves) { item.leaf = hex::encode(leaf); }
      let manifest = Manifest {
          format_version: FORMAT_VERSION.into(), created_at: input.created_at, did: input.did,
          brain_verifying_key: input.brain_verifying_key,
          selection_description: input.selection_description,
          item_count: items.len() as u64,
          merkle_root: hex::encode(merkle::root(&leaves)),
          binding_hash: hex::encode(binding_hash(&input.binding)),
      };
      let seal = sign_bytes(&crate::format::canonical_json(&manifest).expect("manifest serializable"),
          input.brain_key);
      Airmem { manifest, items, binding: input.binding, seal }
  }
  ```
  Add `pub mod build;` + `pub use build::{build_bundle, BuildInput, ItemInput};` to `lib.rs`.

- [ ] Write the test:
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
              created_at: "2026-07-21T00:00:00Z".into(), did: "did:wba:example.com:me".into(),
              brain_verifying_key: brain_mb.clone(), selection_description: "1 note + 1 session".into(),
              items: vec![
                  ItemInput::Stamped { event_bytes: "{}".into(), signature: "zSig".into(), carried_origin: "external".into() },
                  ItemInput::SealVouched { kind: "session".into(), content: "body".into(), display: Some(serde_json::json!({"title":"S"})) },
              ],
              binding: binding(&brain_mb), brain_key: &brain,
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

## Task 6 — `verify`: resolver seam + L1 checklist + tamper matrix + forgery + kind-aware labels + L2

The heart of the spec (§2.5 L1/L2, §7 error enum, §8 tamper matrix, §3-H2 kind-aware labels). Offline, fail-closed, one bad byte = ❌. The resolver seam is created FIRST (folded from the old Task 7).

**Files:**
- Create `crates/bossclaw-bundle/src/resolver.rs` (FIRST — verify.rs references it)
- Create `crates/bossclaw-bundle/src/verify.rs`
- Modify `crates/bossclaw-bundle/src/lib.rs` (`pub mod resolver;` + `pub mod verify;` + re-exports)

Steps:

- [ ] Create the resolver seam FIRST. Write `crates/bossclaw-bundle/src/resolver.rs`:
  ```rust
  //! The L2 identity-resolution seam. Registry-mediated ONLY (spec §2.5 finding C7): keyed by did,
  //! NEVER a fetch of the did's own domain. The real HTTPS impl is SP-V2; SP-V1 ships the trait, an
  //! offline default (always None → stays L1), and a mock for tests.
  pub trait IdentityResolver {
      /// Resolve `did` to its registry-published identity verifying key (multibase), or `None` when
      /// unresolvable / non-registry / offline. `None` keeps the verdict at L1.
      fn resolve(&self, did: &str) -> Option<String>;
  }

  /// The `--offline` default: resolves nothing, so identity renders "unverified (offline)".
  pub struct OfflineResolver;
  impl IdentityResolver for OfflineResolver {
      fn resolve(&self, _did: &str) -> Option<String> { None }
  }

  #[cfg(test)]
  pub(crate) struct MockResolver { pub did: String, pub key: String }
  #[cfg(test)]
  impl IdentityResolver for MockResolver {
      fn resolve(&self, did: &str) -> Option<String> {
          if did == self.did { Some(self.key.clone()) } else { None }
      }
  }
  ```
  Add `pub mod resolver;` + `pub use resolver::{IdentityResolver, OfflineResolver};` to `lib.rs`.

- [ ] Write `verify.rs`. Note `use self::semver_lite::major;` (the submodule is declared in-file — architect's real compile fix). L2 compares RAW KEY BYTES via `decode_ed25519_key` (finding #6). Labels are kind-aware for seal-vouched, cross-checked for stamped:
  ```rust
  //! Verify an `.airmem`. L1 = self-consistent offline. Fail-closed: the FIRST mismatch is the verdict.
  use bossclaw_canon::event::{canonical_bytes, compute_hash, is_external, Event};
  use bossclaw_canon::sign::{verify_bytes, verify_hash, VerifyingKey};
  use self::semver_lite::major;
  use crate::binding::{binding_hash, decode_ed25519_key, verify_binding_internal};
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
      /// Structurally unparseable / an item is missing a required field for its class.
      Malformed(String),
  }

  /// The identity assurance level rendered alongside the L1 verdict. L1 alone = Unverified (C3).
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub enum IdentityLevel { UnverifiedOffline, RegistryResolved }

  /// The verdict: L1 ok + an identity level + per-item honest origin labels.
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct Verdict { pub item_labels: Vec<String>, pub identity: IdentityLevel }

  const SUPPORTED_MAJOR: u64 = 1;

  /// Full L1 verification, then an L2 attempt via `resolver` (offline resolver = stays L1).
  pub fn verify(bundle: &Airmem, resolver: &dyn IdentityResolver) -> Result<Verdict, VerifyError> {
      if major(&bundle.manifest.format_version).ok_or_else(|| VerifyError::Malformed("format_version".into()))? > SUPPORTED_MAJOR {
          return Err(VerifyError::FormatTooNew);
      }
      let brain_vk = decode_vk(&bundle.manifest.brain_verifying_key)
          .ok_or_else(|| VerifyError::Malformed("brain_verifying_key".into()))?;
      let manifest_bytes = crate::format::canonical_json(&bundle.manifest)
          .map_err(|e| VerifyError::Malformed(e.to_string()))?;
      verify_bytes(&manifest_bytes, &bundle.seal, &brain_vk).map_err(|_| VerifyError::SealInvalid)?;
      if hex::encode(binding_hash(&bundle.binding)) != bundle.manifest.binding_hash {
          return Err(VerifyError::BindingHashMismatch);
      }
      if !verify_binding_internal(&bundle.binding) { return Err(VerifyError::BindingInvalid); }
      if bundle.binding.payload.brain_verifying_key != bundle.manifest.brain_verifying_key {
          return Err(VerifyError::BindingKeyMismatch);
      }
      if bundle.binding.payload.did != bundle.manifest.did { return Err(VerifyError::BindingDidMismatch); }
      let mut leaves = Vec::with_capacity(bundle.items.len());
      let mut item_labels = Vec::with_capacity(bundle.items.len());
      for (i, item) in bundle.items.iter().enumerate() {
          let leaf = merkle::leaf_hash(item);
          if hex::encode(leaf) != item.leaf { return Err(VerifyError::ItemHashMismatch(i)); }
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
              match (decode_ed25519_key(&registry_key_mb), decode_ed25519_key(&bundle.binding.payload.identity_verifying_key)) {
                  (Some(a), Some(b)) if a == b => IdentityLevel::RegistryResolved,
                  _ => return Err(VerifyError::IdentityUnresolved),
              }
          }
      };
      Ok(Verdict { item_labels, identity })
  }

  /// Verify one item and return its honest origin label (§3-H2). Stamped: cross-check carried vs
  /// derived (OriginMismatch) + pinned copy. Seal-vouched: KIND-AWARE label (never cross-rendered).
  fn verify_item(i: usize, item: &AirmemItem, brain_vk: &VerifyingKey) -> Result<String, VerifyError> {
      match item.class {
          ItemClass::Stamped => {
              let bytes = item.event_bytes.as_ref().ok_or(VerifyError::ItemStampInvalid(i))?;
              let sig = item.signature.as_ref().ok_or(VerifyError::ItemStampInvalid(i))?;
              let ev: Event = serde_json::from_str(bytes).map_err(|_| VerifyError::ItemStampInvalid(i))?;
              let recanon = canonical_bytes(&ev).map_err(|_| VerifyError::ItemStampInvalid(i))?;
              if recanon != bytes.as_bytes() { return Err(VerifyError::ItemStampInvalid(i)); }
              let hash = compute_hash(&ev).map_err(|_| VerifyError::ItemStampInvalid(i))?;
              verify_hash(&hash, sig, brain_vk).map_err(|_| VerifyError::ItemStampInvalid(i))?;
              let derived = if is_external(&ev) { "external" } else { "unattested" };
              // A5: any carried display token is cross-checked against the recomputed origin.
              if let Some(carried) = item.carried_origin.as_deref() {
                  if carried != derived { return Err(VerifyError::OriginMismatch(i)); }
              }
              Ok(if is_external(&ev) {
                  "this brain recorded these bytes; provenance of the underlying text is not asserted".into()
              } else {
                  "origin unattested".into() // is_external=false ∧ kind≠dossier (C-NEW-1b)
              })
          }
          ItemClass::SealVouched => Ok(seal_vouched_label(&item.kind)),
      }
  }

  /// KIND-AWARE seal-vouched label (spec §3-H2 amended). The dossier phrasing NEVER renders on a
  /// session/ingest ("machine-derived" would be false there — plan review C5).
  fn seal_vouched_label(kind: &str) -> String {
      match kind {
          "session" => "captured session, content only; not independently verified",
          "ingest" => "ingested file extract; not independently verified",
          "dossier" => "machine-derived by the exporter; not independently verified",
          _ => "exporter-vouched; not independently verified",
      }.to_string()
  }

  fn decode_vk(mb: &str) -> Option<VerifyingKey> { VerifyingKey::from_bytes(&decode_ed25519_key(mb)?).ok() }

  /// Minimal in-module major-version parse (avoids an external `semver` dep). `"1.2.3" → Some(1)`.
  mod semver_lite {
      pub fn major(v: &str) -> Option<u64> { v.split('.').next()?.parse().ok() }
  }
  ```
  Add `pub mod verify;` + `pub use verify::{verify, IdentityLevel, Verdict, VerifyError};` to `lib.rs`.

- [ ] Write the tamper-matrix + forgery + label + no-leak tests. Shared `valid_bundle()` helper (clippy-clean — no dead `mut`, no `let _ = &mut`):
  ```rust
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
          let content = if external { serde_json::json!({ "text": text, "origin": "external" }) }
              else { serde_json::json!({ "text": text }) };
          Event { id: "01J0000000000000000000000A".into(), ts: "2026-06-15T00:00:00Z".into(),
              valid_time: None, event_type: "memory".into(), content, model_meta: None,
              prev_hash: "00".repeat(32), hash: None, signed_by_did: "did:wba:example.com:me".into(),
              signature: None }
      }

      fn valid_bundle() -> Airmem {
          let brain = SigningKey::from_bytes(&BRAIN);
          let brain_mb = multibase::encode(multibase::Base::Base58Btc, brain.verifying_key().to_bytes());
          let ev = note_event("shared note", true);
          let event_bytes = String::from_utf8(canonical_bytes(&ev).unwrap()).unwrap();
          let sig = sign_hash(&compute_hash(&ev).unwrap(), &brain);
          let idk = SigningKey::from_bytes(&[9u8; 32]);
          let idvk = multibase::encode(multibase::Base::Base58Btc, idk.verifying_key().to_bytes());
          let payload = BindingPayload { brain_verifying_key: brain_mb.clone(), identity_verifying_key: idvk,
              did: "did:wba:example.com:me".into(), purpose: "memory-signing".into(), epoch: 1,
              created_at: "2026-07-21T00:00:00Z".into() };
          let bsig = sign_bytes(&binding_signing_bytes(&payload), &idk);
          build_bundle(BuildInput { created_at: "2026-07-21T00:00:00Z".into(),
              did: "did:wba:example.com:me".into(), brain_verifying_key: brain_mb,
              selection_description: "1 note + 1 session".into(),
              items: vec![
                  ItemInput::Stamped { event_bytes, signature: sig, carried_origin: "external".into() },
                  ItemInput::SealVouched { kind: "session".into(), content: "session body".into(),
                      display: Some(serde_json::json!({"title":"S","project":"repo"})) },
              ],
              binding: Binding { payload, identity_signature: bsig }, brain_key: &brain })
      }

      fn reseal(b: &mut Airmem) {
          let brain = SigningKey::from_bytes(&BRAIN);
          b.seal = sign_bytes(&crate::format::canonical_json(&b.manifest).unwrap(), &brain);
      }
      fn off() -> OfflineResolver { OfflineResolver }

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
          b.items[0].leaf = hex::encode(merkle::leaf_hash(&b.items[0]));
          let leaves: Vec<_> = b.items.iter().map(merkle::leaf_hash).collect();
          b.manifest.merkle_root = hex::encode(merkle::root(&leaves)); reseal(&mut b);
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

      // ---- BindingKeyMismatch: change manifest brain key + re-seal under a SECOND brain key ----
      #[test] fn binding_key_mismatch() {
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
      #[test] fn binding_invalid() {
          let mut b = valid_bundle();
          b.binding.payload.created_at = "1999-01-01T00:00:00Z".into(); // signature no longer covers it
          b.manifest.binding_hash = hex::encode(crate::binding::binding_hash(&b.binding));
          reseal(&mut b);
          assert_eq!(verify(&b, &off()), Err(VerifyError::BindingInvalid));
      }

      // ---- RE-ATTRIBUTION FORGERY (C1): fresh binding by a DIFFERENT identity over the same brain key.
      //      Layer 1 (binding_hash) tested here; layer 2 (BindingDidMismatch) after recompute+reseal. ----
      #[test] fn re_attribution_layer1_binding_hash() {
          let mut b = valid_bundle();
          let attacker = SigningKey::from_bytes(&[42u8; 32]);
          let avk = multibase::encode(multibase::Base::Base58Btc, attacker.verifying_key().to_bytes());
          let payload = BindingPayload { brain_verifying_key: b.manifest.brain_verifying_key.clone(),
              identity_verifying_key: avk, did: "did:wba:evil.com:attacker".into(),
              purpose: "memory-signing".into(), epoch: 1, created_at: "2026-07-21T00:00:00Z".into() };
          let asig = sign_bytes(&binding_signing_bytes(&payload), &attacker);
          b.binding = Binding { payload, identity_signature: asig };
          assert_eq!(verify(&b, &off()), Err(VerifyError::BindingHashMismatch));
      }
      #[test] fn re_attribution_layer2_binding_did() {
          let mut b = valid_bundle();
          let attacker = SigningKey::from_bytes(&[42u8; 32]);
          let avk = multibase::encode(multibase::Base::Base58Btc, attacker.verifying_key().to_bytes());
          let payload = BindingPayload { brain_verifying_key: b.manifest.brain_verifying_key.clone(),
              identity_verifying_key: avk, did: "did:wba:evil.com:attacker".into(),
              purpose: "memory-signing".into(), epoch: 1, created_at: "2026-07-21T00:00:00Z".into() };
          let asig = sign_bytes(&binding_signing_bytes(&payload), &attacker);
          b.binding = Binding { payload, identity_signature: asig };
          b.manifest.binding_hash = hex::encode(crate::binding::binding_hash(&b.binding)); reseal(&mut b);
          assert_eq!(verify(&b, &off()), Err(VerifyError::BindingDidMismatch)); // did ≠ sealed manifest.did
      }

      // ---- FOREIGN-EVENT LAUNDERING (A6): stamped item validly signed by a DIFFERENT brain key. ----
      #[test] fn foreign_event_laundering() {
          let brain = SigningKey::from_bytes(&BRAIN);
          let foreign = SigningKey::from_bytes(&[77u8; 32]);
          let mut ev = note_event("foreign", true); ev.id = "01J000000000000000000000FF".into();
          let event_bytes = String::from_utf8(canonical_bytes(&ev).unwrap()).unwrap();
          let fsig = sign_hash(&compute_hash(&ev).unwrap(), &foreign);
          let mut b = valid_bundle();
          b.items[0] = AirmemItem { leaf: String::new(), class: ItemClass::Stamped, kind: "note".into(),
              event_bytes: Some(event_bytes), signature: Some(fsig), carried_origin: Some("external".into()),
              content: None, display: None };
          b.items[0].leaf = hex::encode(merkle::leaf_hash(&b.items[0]));
          let leaves: Vec<_> = b.items.iter().map(merkle::leaf_hash).collect();
          b.manifest.merkle_root = hex::encode(merkle::root(&leaves)); reseal(&mut b);
          assert_eq!(verify(&b, &off()), Err(VerifyError::ItemStampInvalid(0)));
      }

      // ---- EXPORTER-LIED-ORIGIN (A5): carried_origin disagrees with the signed bytes → OriginMismatch.
      //      carried_origin is leaf-EXCLUDED, so a lone flip needs no re-seal. ----
      #[test] fn exporter_lied_origin() {
          let mut b = valid_bundle();
          b.items[0].carried_origin = Some("unattested".into()); // bytes say external
          assert_eq!(verify(&b, &off()), Err(VerifyError::OriginMismatch(0)));
      }

      // ---- C-NEW-1b: an origin-LESS stamped note renders EXACTLY "origin unattested", never brain-authored. ----
      #[test] fn origin_less_note_renders_unattested() {
          let brain = SigningKey::from_bytes(&BRAIN);
          let ev = note_event("owner note", false); // NO origin stamp
          let event_bytes = String::from_utf8(canonical_bytes(&ev).unwrap()).unwrap();
          let sig = sign_hash(&compute_hash(&ev).unwrap(), &brain);
          let mut b = valid_bundle();
          b.items[0] = AirmemItem { leaf: String::new(), class: ItemClass::Stamped, kind: "note".into(),
              event_bytes: Some(event_bytes), signature: Some(sig), carried_origin: Some("unattested".into()),
              content: None, display: None };
          b.items[0].leaf = hex::encode(merkle::leaf_hash(&b.items[0]));
          let leaves: Vec<_> = b.items.iter().map(merkle::leaf_hash).collect();
          b.manifest.merkle_root = hex::encode(merkle::root(&leaves)); reseal(&mut b);
          let v = verify(&b, &off()).unwrap();
          assert_eq!(v.item_labels[0], "origin unattested");
      }

      // ---- SEAL-VOUCHED NO-LEAK (A-N1 + A4/C5) — covers BOTH session AND ingest classes (NEW-B). ----
      #[test] fn seal_vouched_discloses_no_local_metadata() {
          let mut b = valid_bundle(); // items[0]=stamped note, items[1]=session
          // Add an ingest seal-vouched item (content-only; the item shape has NO provenance field —
          // the real file_ingested provenance block is stripped by core, proven end-to-end in Task 11).
          let mut ingest = AirmemItem { leaf: String::new(), class: ItemClass::SealVouched,
              kind: "ingest".into(), event_bytes: None, signature: None, carried_origin: None,
              content: Some("extracted file text".into()), display: None };
          ingest.leaf = hex::encode(merkle::leaf_hash(&ingest));
          b.items.push(ingest);
          let leaves: Vec<_> = b.items.iter().map(merkle::leaf_hash).collect();
          b.manifest.merkle_root = hex::encode(merkle::root(&leaves));
          b.manifest.item_count = b.items.len() as u64;
          reseal(&mut b);
          verify(&b, &off()).expect("still verifies with the ingest item");
          let whole = serde_json::to_string(&b).unwrap();
          for needle in ["source_event_ids", "prompt_hash", "session_id", "grant_root", "canonical_path", "content_hash"] {
              assert!(!whole.contains(needle), "seal-vouched item leaked `{needle}`");
          }
          assert!(b.items[1].event_bytes.is_none() && b.items[2].event_bytes.is_none());
      }

      // ---- L2 (mocked registry) — proves the RegistryResolved branch + key-mismatch. ----
      #[test] fn l2_registry_resolved_when_key_matches() {
          let b = valid_bundle();
          let r = MockResolver { did: b.manifest.did.clone(), key: b.binding.payload.identity_verifying_key.clone() };
          assert_eq!(verify(&b, &r).unwrap().identity, IdentityLevel::RegistryResolved);
      }
      #[test] fn l2_registry_resolved_across_encodings() {
          // Registry publishes the 0xed01 multikey form; the binding stores bare — must STILL match (#6).
          let b = valid_bundle();
          let bare = decode_ed25519_key(&b.binding.payload.identity_verifying_key).unwrap();
          let mut multikey = vec![0xed, 0x01]; multikey.extend_from_slice(&bare);
          let mk = multibase::encode(multibase::Base::Base58Btc, &multikey);
          let r = MockResolver { did: b.manifest.did.clone(), key: mk };
          assert_eq!(verify(&b, &r).unwrap().identity, IdentityLevel::RegistryResolved);
      }
      #[test] fn l2_unresolved_when_registry_key_differs() {
          let b = valid_bundle();
          let other = multibase::encode(multibase::Base::Base58Btc, [0u8; 32]);
          let r = MockResolver { did: b.manifest.did.clone(), key: other };
          assert_eq!(verify(&b, &r), Err(VerifyError::IdentityUnresolved));
      }
  }
  ```

- [ ] Run (foreground): `cargo test -p bossclaw-bundle`. Expected: all pass (whole crate).

- [ ] Commit: `git add crates/bossclaw-bundle/src/resolver.rs crates/bossclaw-bundle/src/verify.rs crates/bossclaw-bundle/src/lib.rs && git commit -m "feat(rung5): verify L1 + §7 enum + full tamper matrix + kind-aware labels + L2 (encoding-agnostic)"`

---

## Task 7 — Core: binding storage + `current_ingests` + `estimate_export_bytes` + `EventLog::export_bundle`

Store the app-minted binding as a signed config event, expose the brain verifying key, add the current-ingest projection + the pre-frame size estimate, and add the pure-read export (fs-free; session bodies are supplied by the daemon).

**Files:**
- Modify `crates/bossclaw-core/Cargo.toml` (add `bossclaw-bundle` path dep)
- Modify `crates/bossclaw-core/src/log.rs` (const + storage/readers + `current_ingests` + `estimate_export_bytes` + `export_bundle` + `ExportSelection`/`ExportError`/`CurrentIngest`/`MAX_EXPORT_BYTES`)
- Modify `crates/bossclaw-core/src/lib.rs` (`pub use log::{…}` block near line 66)

Steps:

- [ ] Add the dep + config key (NO `ConfigFlag` variant — architect #5: the storage path uses the const directly). In `crates/bossclaw-core/Cargo.toml` `[dependencies]` add `bossclaw-bundle = { path = "../bossclaw-bundle" }`. In `log.rs` add beside the other keys (near line 278): `const BINDING_KEY: &str = "identity_binding";` and `pub const MAX_EXPORT_BYTES: u64 = 30 * 1024 * 1024; // headroom below bossclawd-proto MAX_FRAME (32 MiB, lib.rs:443)`. Do NOT add a `ConfigFlag::Binding` arm.

- [ ] Add the types near the other pub structs. `ExportSelection` derives `Clone` (the daemon clones it for the pre-frame estimate). `ExportError`, `CurrentIngest`:
  ```rust
  /// The owner's export selection. Times/created_at are daemon-supplied.
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct ExportSelection {
      /// `CurrentNote.event_id`s → stamped items.
      pub note_event_ids: Vec<String>,
      /// `CurrentSession.event_id`s → seal-vouched items (bodies supplied by the daemon).
      pub session_event_ids: Vec<String>,
      /// `CurrentIngest.event_id`s (= `file_ingested` ULIDs) → seal-vouched items.
      pub ingest_event_ids: Vec<String>,
      /// Free-text description shown to the receiver.
      pub description: String,
      /// Export time, RFC3339 (daemon-supplied — deterministic + testable).
      pub created_at: String,
  }

  /// A current ingested file as an export subject. Mirrors `CurrentSession` shape (event id + a display
  /// key). The `canonical_path` is for the APP's selection list only — it NEVER enters the bundle (A-N1).
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct CurrentIngest { pub event_id: String, pub canonical_path: String }

  /// Typed export refusals (spec §7). `BundleTooLarge` is the pre-frame size guard.
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub enum ExportError {
      /// `verify_chain` failed — never seal from a sick brain (S4).
      ChainInvalid,
      /// Nothing selected.
      EmptySelection,
      /// No stored binding — app-side mint required first.
      BindingUnavailable,
      /// Estimated serialized size exceeds `MAX_EXPORT_BYTES`.
      BundleTooLarge { bytes: u64, max: u64 },
      /// A selected id is not an exportable subject (wrong kind / not current). Carries a reason.
      NotExportable(String),
      /// Any other core failure.
      Core(String),
  }
  ```

- [ ] Add storage/readers + the brain key getter (mirror `set_reasoner_config` at `log.rs:7766`, `latest_config_value` at `log.rs:7902`) + `current_ingests` (delegates to the existing superseded-excluded `current_files` fold, `log.rs:5141`/`882`) + `estimate_export_bytes` + the module-level `core` mapper:
  ```rust
  /// Store an app-minted identity binding as a signed `config` event (house pattern — tamper-evident
  /// via `verify_chain`). ONLY writer of `BINDING_KEY`. The daemon VALIDATES first (spec §2.3 C-NEW-4).
  pub fn set_binding(&self, attestation: serde_json::Value) -> Result<(), BossclawError> {
      self.append(Event { id: String::new(), ts: String::new(), valid_time: None,
          event_type: CONFIG_EVENT_TYPE.to_string(),
          content: serde_json::Value::Object({ let mut m = serde_json::Map::new();
              m.insert(BINDING_KEY.to_string(), attestation); m }),
          model_meta: None, prev_hash: String::new(), hash: None,
          signed_by_did: self.signer_did(), signature: None })?;
      Ok(())
  }

  /// The stored binding with the HIGHEST `payload.epoch`, or `None` if never set (spec §2.3).
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
              if best.as_ref().map(|(e, _)| epoch > *e).unwrap_or(true) { best = Some((epoch, v.clone())); }
          }
      }
      Ok(best.map(|(_, v)| v))
  }

  /// The epochs already stored (daemon's first-write-wins idempotency check).
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

  /// The daemon's ACTUAL brain verifying key, multibase base58btc (§2.3 round-trip; A6 stamp key).
  pub fn brain_verifying_key_multibase(&self) -> String {
      multibase::encode(multibase::Base::Base58Btc, self.key.verifying_key().to_bytes())
  }

  /// The CURRENT (non-superseded) ingested files as export subjects. Delegates to `current_files`
  /// (log.rs:5141 — the `files` table is the superseded-excluded file_ingested/supersede fold, log.rs:882),
  /// so it mirrors `current_sessions` without duplicating fold logic.
  pub fn current_ingests(&self) -> Result<Vec<CurrentIngest>, BossclawError> {
      Ok(self.current_files()?.into_iter()
          .map(|f| CurrentIngest { event_id: f.file_event_id, canonical_path: f.canonical_path })
          .collect())
  }

  /// Estimate the serialized `.airmem` size BEFORE reading any session body or sealing (the daemon's
  /// pre-frame `BundleTooLarge` guard). Sums stamped-note canonical bytes + ingest text + session
  /// `approx_bytes` + fixed overhead — all from in-log data (notes/ingests) or the session fold
  /// (`approx_bytes`), so NO capture `.md` is read here.
  pub fn estimate_export_bytes(&self, selection: &ExportSelection) -> Result<u64, BossclawError> {
      let mut total: u64 = 4096; // manifest + binding + seal overhead
      for id in &selection.note_event_ids {
          if let Some(ev) = self.event_by_id(id)? {
              total += crate::event::canonical_bytes(&ev)?.len() as u64
                  + ev.signature.as_ref().map(|s| s.len() as u64).unwrap_or(0) + 256;
          }
      }
      for id in &selection.ingest_event_ids {
          if let Some(ev) = self.event_by_id(id)? {
              let t = ev.content.get("text").and_then(|t| t.as_str()).map(|s| s.len()).unwrap_or(0);
              total += t as u64 + 256;
          }
      }
      let sessions = self.current_sessions()?;
      for id in &selection.session_event_ids {
          if let Some(s) = sessions.iter().find(|s| &s.event_id == id) { total += s.approx_bytes + 512; }
      }
      Ok(total)
  }
  ```
  Add a module-level helper (near the impl, NOT a method): `fn core(e: BossclawError) -> ExportError { ExportError::Core(e.to_string()) }`. Export `ExportSelection`, `ExportError`, `CurrentIngest`, `MAX_EXPORT_BYTES` from `crates/bossclaw-core/src/lib.rs` (`pub use log::{…}` block near line 66).

- [ ] Write the storage test first (reuse the module's helper `open_log(dir.path())` — `log.rs:10309`, `dir = tempfile::tempdir()`, `KEY_BYTES=[7u8;32]`; do NOT invent a constructor):
  ```rust
  #[test]
  fn binding_stores_signed_and_reads_back_highest_epoch() {
      let dir = tempfile::tempdir().unwrap();
      let log = open_log(dir.path());
      assert!(log.latest_binding().unwrap().is_none());
      log.set_binding(serde_json::json!({ "payload": { "epoch": 1 }, "identity_signature": "zA" })).unwrap();
      log.set_binding(serde_json::json!({ "payload": { "epoch": 2 }, "identity_signature": "zB" })).unwrap();
      assert_eq!(log.latest_binding().unwrap().unwrap()["payload"]["epoch"], 2);
      assert_eq!(log.binding_epochs().unwrap().len(), 2);
      assert!(log.verify_chain().is_ok());
  }
  ```
  Run (foreground): `cargo test -p bossclaw-core binding_stores`. Expected: pass (after the impl compiles).

- [ ] Add `EventLog::export_bundle` — pure-read, fs-free (session bodies passed in by the daemon; a missing body → the store.rs:485-style placeholder). Order: `verify_chain` → empty → binding → per-class gather with server-side guards → build → belt size check:
  ```rust
  /// Build a sealed `.airmem` for `selection`. PURE-READ (spec §2.4), FS-FREE: `session_bodies` maps
  /// each selected session event id → its (daemon-read, bounded) body; a session absent from the map
  /// gets a placeholder (architect #4: a missing body must not abort). Returns canonical `.airmem` text.
  pub fn export_bundle(
      &self,
      selection: &ExportSelection,
      session_bodies: &std::collections::HashMap<String, String>,
  ) -> Result<String, ExportError> {
      self.verify_chain().map_err(|_| ExportError::ChainInvalid)?; // S4
      if selection.note_event_ids.is_empty() && selection.session_event_ids.is_empty()
          && selection.ingest_event_ids.is_empty() { return Err(ExportError::EmptySelection); }
      let binding_json = self.latest_binding().map_err(core)?.ok_or(ExportError::BindingUnavailable)?;
      let binding: bossclaw_bundle::Binding = serde_json::from_value(binding_json)
          .map_err(|e| ExportError::Core(format!("stored binding malformed: {e}")))?;
      let did = binding.payload.did.clone();

      let mut items: Vec<bossclaw_bundle::ItemInput> = Vec::new();
      // Stamped notes — SERVER-SIDE A-N1 guard (memory + external ONLY; never trust the caller's class).
      for id in &selection.note_event_ids {
          let ev = self.event_by_id(id).map_err(core)?
              .ok_or_else(|| ExportError::NotExportable(format!("note {id} not found")))?;
          if ev.event_type != crate::graph::MEMORY_EVENT_TYPE || !crate::event::is_external(&ev) {
              return Err(ExportError::NotExportable(format!("{id} is not an exportable external note")));
          }
          let signature = ev.signature.clone().ok_or_else(|| ExportError::Core(format!("note {id} unsigned")))?;
          let event_bytes = String::from_utf8(crate::event::canonical_bytes(&ev).map_err(core)?)
              .map_err(|e| ExportError::Core(e.to_string()))?;
          items.push(bossclaw_bundle::ItemInput::Stamped { event_bytes, signature, carried_origin: "external".into() });
      }
      // Seal-vouched ingests — validate CURRENT + type; disclose content ONLY (no path/provenance leak).
      let current_ingest_ids: std::collections::HashSet<String> =
          self.current_ingests().map_err(core)?.into_iter().map(|i| i.event_id).collect();
      for id in &selection.ingest_event_ids {
          if !current_ingest_ids.contains(id) {
              return Err(ExportError::NotExportable(format!("ingest {id} is not a current file")));
          }
          let ev = self.event_by_id(id).map_err(core)?
              .ok_or_else(|| ExportError::NotExportable(format!("ingest {id} not found")))?;
          if ev.event_type != crate::graph::FILE_INGESTED_EVENT_TYPE {
              return Err(ExportError::NotExportable(format!("{id} is not an ingest")));
          }
          let content = ev.content.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string();
          items.push(bossclaw_bundle::ItemInput::SealVouched { kind: "ingest".into(), content, display: None });
      }
      // Seal-vouched sessions — body from the daemon-supplied map (placeholder if missing); safe display.
      let sessions = self.current_sessions().map_err(core)?;
      for id in &selection.session_event_ids {
          let s = sessions.iter().find(|s| &s.event_id == id)
              .ok_or_else(|| ExportError::NotExportable(format!("session {id} is not a current session")))?;
          let content = session_bodies.get(id).cloned()
              .unwrap_or_else(|| "[session body unavailable at export time]".to_string());
          let display = serde_json::json!({ "title": s.title, "project": s.project, "tool": s.tool,
              "started_at": s.started_at, "ended_at": s.ended_at, "approx_bytes": s.approx_bytes });
          items.push(bossclaw_bundle::ItemInput::SealVouched { kind: "session".into(), content, display: Some(display) });
      }

      let bundle = bossclaw_bundle::build_bundle(bossclaw_bundle::BuildInput {
          created_at: selection.created_at.clone(), did,
          brain_verifying_key: self.brain_verifying_key_multibase(),
          selection_description: selection.description.clone(), items, binding, brain_key: &self.key,
      });
      let text = String::from_utf8(bossclaw_bundle::format::canonical_json(&bundle)
          .map_err(|e| ExportError::Core(e.to_string()))?).map_err(|e| ExportError::Core(e.to_string()))?;
      // Belt-and-suspenders (primary guard is the daemon's pre-body `estimate_export_bytes`).
      if text.len() as u64 > MAX_EXPORT_BYTES {
          return Err(ExportError::BundleTooLarge { bytes: text.len() as u64, max: MAX_EXPORT_BYTES });
      }
      Ok(text)
  }
  ```

- [ ] Write the end-to-end + guard tests. Add a one-line read helper `pub fn latest_event_id_of_type(&self, t: &str) -> Result<Option<String>, BossclawError>` next to `event_by_id` (a `SELECT id … WHERE event_type=?1 ORDER BY seq DESC LIMIT 1`) — grep first and reuse if a sibling already exists:
  ```rust
  #[test]
  fn export_bundle_verifies_green_end_to_end() {
      let dir = tempfile::tempdir().unwrap();
      let log = open_log(dir.path());
      let embedder = MockEmbedder::new(8); // match the dim sibling log.rs tests use
      let note_id = log.remember(&embedder, "a shared fact").unwrap();
      let brain_mb = log.brain_verifying_key_multibase();
      let idk = ed25519_dalek::SigningKey::from_bytes(&[9u8; 32]);
      let idvk = multibase::encode(multibase::Base::Base58Btc, idk.verifying_key().to_bytes());
      let payload = serde_json::json!({ "brain_verifying_key": brain_mb, "identity_verifying_key": idvk,
          "did": "did:wba:example.com:me", "purpose": "memory-signing", "epoch": 1, "created_at": "2026-07-21T00:00:00Z" });
      let sig_bytes = bossclaw_bundle::binding_signing_bytes(&serde_json::from_value(payload.clone()).unwrap());
      let identity_signature = bossclaw_canon::sign::sign_bytes(&sig_bytes, &idk);
      log.set_binding(serde_json::json!({ "payload": payload, "identity_signature": identity_signature })).unwrap();
      let text = log.export_bundle(&crate::log::ExportSelection {
          note_event_ids: vec![note_id], session_event_ids: vec![], ingest_event_ids: vec![],
          description: "1 note".into(), created_at: "2026-07-21T00:00:00Z".into(),
      }, &std::collections::HashMap::new()).unwrap();
      let bundle: bossclaw_bundle::Airmem = serde_json::from_str(&text).unwrap();
      let v = bossclaw_bundle::verify(&bundle, &bossclaw_bundle::OfflineResolver).unwrap();
      assert_eq!(v.identity, bossclaw_bundle::IdentityLevel::UnverifiedOffline);
      assert_eq!(v.item_labels[0], "this brain recorded these bytes; provenance of the underlying text is not asserted");
  }

  #[test]
  fn export_rejects_missing_binding_and_non_note_ids() {
      let dir = tempfile::tempdir().unwrap();
      let log = open_log(dir.path());
      let embedder = MockEmbedder::new(8);
      let note_id = log.remember(&embedder, "n").unwrap();
      let empty = std::collections::HashMap::new();
      // No binding yet → BindingUnavailable.
      assert_eq!(log.export_bundle(&crate::log::ExportSelection { note_event_ids: vec![note_id],
          session_event_ids: vec![], ingest_event_ids: vec![], description: "x".into(),
          created_at: "2026-07-21T00:00:00Z".into() }, &empty), Err(crate::log::ExportError::BindingUnavailable));
      // Store a binding (a config event), then mis-submit that config id as a note → server-side guard.
      log.set_binding(serde_json::json!({ "payload": { "epoch": 1 }, "identity_signature": "z" })).unwrap();
      let cfg_id = log.latest_event_id_of_type(crate::graph::CONFIG_EVENT_TYPE).unwrap().unwrap();
      match log.export_bundle(&crate::log::ExportSelection { note_event_ids: vec![cfg_id],
          session_event_ids: vec![], ingest_event_ids: vec![], description: "x".into(),
          created_at: "2026-07-21T00:00:00Z".into() }, &empty) {
          Err(crate::log::ExportError::NotExportable(_)) => {}
          other => panic!("expected NotExportable, got {other:?}"),
      }
  }
  ```
  Run (foreground): `cargo test -p bossclaw-core export`. Expected: pass. Then `cargo test -p bossclaw-core` (whole crate) green.

- [ ] Commit: `git add crates/bossclaw-core/Cargo.toml crates/bossclaw-core/src/log.rs crates/bossclaw-core/src/lib.rs && git commit -m "feat(rung5): core binding storage + current_ingests + estimate + export_bundle (pure-read, fs-free, server-side guards)"`

---

## Task 8 — Proto + daemon wire ops: `BrainVerifyingKey`, `SetBinding`, `ExportBundle` (with `BundleTooLarge`)

Three App-only ops. **Compile-coherence note:** `dispatch` (`server.rs:266-506`) is EXHAUSTIVE over `Request` — adding variants breaks the daemon build until every arm is handled. This task lands the proto variants AND dispatch arms AND engine methods together in ONE commit (the transient is confined here). The daemon reads session bodies through its bounded capture reader (`read_capture_markdown`, store.rs:99) and runs the pre-frame size guard before sealing.

**Files:**
- Modify `crates/bossclawd-proto/src/lib.rs` (3 `Request` variants + `ExportSelectionWire` + 3 `Response` variants + guest tests)
- Modify `crates/bossclawd/src/engine/mod.rs` (4 `EngineHandle` methods)
- Modify `crates/bossclawd/src/server.rs` (dispatch arms + helpers)
- Modify `crates/bossclawd/Cargo.toml` (add `bossclaw-bundle`)
- Modify `crates/bossclawd/tests/authz.rs` (guest-refusal)

Steps:

- [ ] Add the proto variants. After `ReflectEnabled` (line 259) in `Request`:
  ```rust
      /// Rung-5: the daemon's brain verifying key (multibase). App-only (guest-refused by construction).
      BrainVerifyingKey { onboarded: bool },
      /// Rung-5: store an app-minted identity binding (§2.3). App-only. Daemon VALIDATES first.
      SetBinding { onboarded: bool, attestation: serde_json::Value },
      /// Rung-5: build a sealed `.airmem` for `selection` (§2.4). App-only. Pure-read; `verify_chain` first.
      ExportBundle { onboarded: bool, selection: ExportSelectionWire },
  ```
  Near `RetireTarget` (line 273):
  ```rust
  /// The export selection carried on the wire (mirrors `bossclaw_core::log::ExportSelection` minus the
  /// daemon-supplied `created_at`).
  #[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
  pub struct ExportSelectionWire {
      /// Current-note ids → stamped items.
      pub note_event_ids: Vec<String>,
      /// Current-session event ids → seal-vouched items.
      pub session_event_ids: Vec<String>,
      /// Current-ingest (`file_ingested`) event ids → seal-vouched items.
      pub ingest_event_ids: Vec<String>,
      /// Free-text description shown to the receiver.
      pub description: String,
  }
  ```
  After `ReflectEnabled(bool)` (line 369) in `Response`:
  ```rust
      /// `BrainVerifyingKey` result — the multibase brain verifying key.
      BrainVerifyingKey(String),
      /// `ExportBundle` result — the canonical `.airmem` JSON text.
      Bundle(String),
      /// `ExportBundle` pre-frame size refusal (§7 `BundleTooLarge`) — a distinct signal, not an Err.
      BundleTooLarge { bytes: u64, max: u64 },
  ```

- [ ] Pin the App-only guarantee. Extend `memory_client_allows_exactly_six_ops` (line 873) `no` array:
  ```rust
      BrainVerifyingKey { onboarded: true },
      SetBinding { onboarded: true, attestation: serde_json::Value::Null },
      ExportBundle { onboarded: true, selection: ExportSelectionWire {
          note_event_ids: vec![], session_event_ids: vec![], ingest_event_ids: vec![], description: String::new() } },
  ```
  Add an `ExportBundle` serde round-trip assertion in `new_variants_round_trip_serde` (line 946). Run (foreground): `cargo test -p bossclawd-proto`. Expected: green.

- [ ] Add `bossclaw-bundle = { path = "../bossclaw-bundle" }` to `crates/bossclawd/Cargo.toml` `[dependencies]`. Add the `EngineHandle` methods in `engine/mod.rs` (mirror `set_reflect_enabled`, line 1201). `export_bundle` returns `Result<String, ExportError>` so the dispatch can map `BundleTooLarge`/refusals distinctly:
  ```rust
  pub async fn brain_verifying_key(&self, onboarded: bool) -> Result<String, EngineOpError> {
      let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
      spawn_blocking(move || Ok(log.brain_verifying_key_multibase()))
          .await.map_err(|e| EngineOpError::Join(e.to_string()))?
  }

  pub async fn set_binding(&self, onboarded: bool, attestation: serde_json::Value) -> Result<(), EngineOpError> {
      let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
      spawn_blocking(move || {
          let b: bossclaw_bundle::Binding = serde_json::from_value(attestation.clone())
              .map_err(|e| EngineOpError::Rejected(format!("malformed binding: {e}")))?;
          if !bossclaw_bundle::verify_binding_internal(&b) {
              return Err(EngineOpError::Rejected("binding identity signature invalid".into()));
          }
          if b.payload.brain_verifying_key != log.brain_verifying_key_multibase() {
              return Err(EngineOpError::Rejected("binding brain key does not match this brain".into()));
          }
          let epochs = log.binding_epochs().map_err(|e| EngineOpError::Core(e.to_string()))?;
          if epochs.contains(&b.payload.epoch) {
              return Err(EngineOpError::Rejected(format!("binding epoch {} already stored", b.payload.epoch)));
          }
          log.set_binding(attestation).map_err(|e| EngineOpError::Core(e.to_string()))
      }).await.map_err(|e| EngineOpError::Join(e.to_string()))?
  }

  /// Pre-frame size estimate (no body reads) — the daemon's `BundleTooLarge` guard.
  pub async fn estimate_export_bytes(&self, onboarded: bool, selection: bossclaw_core::log::ExportSelection)
      -> Result<u64, EngineOpError> {
      let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
      spawn_blocking(move || log.estimate_export_bytes(&selection).map_err(|e| EngineOpError::Core(e.to_string())))
          .await.map_err(|e| EngineOpError::Join(e.to_string()))?
  }

  /// Build the sealed bundle. `session_bodies` are pre-read (bounded) by the daemon. Returns the
  /// typed `ExportError` so the dispatch maps `BundleTooLarge`/refusals to their wire signals.
  pub async fn export_bundle(&self, onboarded: bool, selection: bossclaw_core::log::ExportSelection,
      session_bodies: std::collections::HashMap<String, String>)
      -> Result<String, bossclaw_core::log::ExportError> {
      let log = self.get_or_open(onboarded).await
          .map_err(|e| bossclaw_core::log::ExportError::Core(e.to_string()))?;
      spawn_blocking(move || log.export_bundle(&selection, &session_bodies))
          .await.map_err(|e| bossclaw_core::log::ExportError::Core(e.to_string()))?
  }
  ```
  (Confirm `EngineOpError::Rejected(String)` — `engine/mod.rs:88`. `get_or_open` returns `EngineError`; map to `ExportError::Core` for the export path.)

- [ ] Add the dispatch arms in `server.rs` (after `ReflectEnabled`, line 505) + helpers. The `ExportBundle` arm: build the selection (daemon stamps `created_at`), run the size estimate → `BundleTooLarge`, read session bodies bounded (confinement-checked, placeholder on failure), then export:
  ```rust
      Request::BrainVerifyingKey { onboarded } => {
          op_result(engine.brain_verifying_key(onboarded).await, Response::BrainVerifyingKey)
      }
      Request::SetBinding { onboarded, attestation } => {
          unit_result(engine.set_binding(onboarded, attestation).await)
      }
      Request::ExportBundle { onboarded, selection } => export_dispatch(engine, onboarded, selection).await,
  ```
  Add the helper (below `dispatch`), importing `crate::capture::store::read_capture_markdown` + reusing the `get_session` confinement pattern (`server.rs:531-535`):
  ```rust
  /// The `ExportBundle` seam: estimate → size-guard → read session bodies (bounded, confined,
  /// placeholder on failure) → export. Never reads a body before the size guard (avoids a wasted
  /// build on an over-cap selection — spec §7 `BundleTooLarge`).
  async fn export_dispatch(engine: &Arc<EngineHandle>, onboarded: bool, sel: bossclawd_proto::ExportSelectionWire) -> Response {
      use bossclaw_core::log::{ExportError, ExportSelection, MAX_EXPORT_BYTES};
      let selection = ExportSelection {
          note_event_ids: sel.note_event_ids, session_event_ids: sel.session_event_ids,
          ingest_event_ids: sel.ingest_event_ids, description: sel.description,
          created_at: now_rfc3339(),
      };
      // 1. Pre-body, pre-seal size guard.
      match engine.estimate_export_bytes(onboarded, selection.clone()).await {
          Ok(bytes) if bytes > MAX_EXPORT_BYTES => return Response::BundleTooLarge { bytes, max: MAX_EXPORT_BYTES },
          Ok(_) => {}
          Err(e) => return op_error_response(e),
      }
      // 2. Read the selected session bodies (bounded + confined; placeholder on any read failure).
      let mut bodies = std::collections::HashMap::new();
      if !selection.session_event_ids.is_empty() {
          let sessions = match engine.current_sessions().await { Ok(s) => s, Err(e) => return op_error_response(e) };
          let sessions_dir = engine.data_dir().map(|d| d.join("sessions"));
          for id in &selection.session_event_ids {
              if let Some(cs) = sessions.iter().find(|c| &c.event_id == id) {
                  let confined = sessions_dir.as_ref()
                      .map(|sd| std::path::Path::new(&cs.path).starts_with(sd)).unwrap_or(false);
                  let body = if confined {
                      crate::capture::store::read_capture_markdown(std::path::Path::new(&cs.path))
                          .unwrap_or_else(|_| "[session body unavailable at export time]".to_string())
                  } else { "[session body unavailable at export time]".to_string() };
                  bodies.insert(id.clone(), body);
              }
          }
      }
      // 3. Build + seal.
      match engine.export_bundle(onboarded, selection, bodies).await {
          Ok(text) => {
              // NEW-A wire-honesty: the frame is `serde_json::to_vec(&Response::Bundle(text))` (proto
              // lib.rs:667/708), which JSON-ESCAPES the already-JSON .airmem — escaping overhead is
              // larger than the 2 MiB gap between MAX_EXPORT_BYTES (30 MiB) and MAX_FRAME (32 MiB), so a
              // bundle that passed the pre-build estimate + the core belt can still overflow the frame.
              // Bound the ACTUAL serialized Response length; a generic frame error must never replace the
              // typed refusal (spec §7). This is the authoritative size gate; the estimate is the coarse
              // pre-build guard that avoids a wasted build in the common case.
              let resp = Response::Bundle(text);
              match serde_json::to_vec(&resp) {
                  Ok(wire) if wire.len() > bossclawd_proto::MAX_FRAME =>
                      Response::BundleTooLarge { bytes: wire.len() as u64, max: bossclawd_proto::MAX_FRAME as u64 },
                  Ok(_) => resp,
                  Err(e) => Response::Err { kind: OpErrorKindWire::Core, message: e.to_string() },
              }
          }
          Err(ExportError::BundleTooLarge { bytes, max }) => Response::BundleTooLarge { bytes, max },
          Err(ExportError::ChainInvalid) => protocol_reject("cannot export: memory chain does not verify"),
          Err(ExportError::EmptySelection) => protocol_reject("empty selection"),
          Err(ExportError::BindingUnavailable) => protocol_reject("no identity binding stored"),
          Err(ExportError::NotExportable(m)) => protocol_reject(&m),
          Err(ExportError::Core(m)) => Response::Err { kind: OpErrorKindWire::Core, message: m },
      }
  }

  /// A typed `Rejected` refusal helper (mirrors the export refusals to `OpErrorKindWire::Rejected`).
  fn protocol_reject(msg: &str) -> Response { Response::Err { kind: OpErrorKindWire::Rejected, message: msg.to_string() } }

  /// RFC3339 now — the daemon boundary clock (core stays clock-free at this seam). Sits beside
  /// `now_unix_secs` (server.rs:574).
  fn now_rfc3339() -> String { chrono::Utc::now().to_rfc3339() }
  ```
  Confirm `engine.data_dir()` (used at `server.rs:531`) and `engine.current_sessions()` (used at `server.rs:520`) exist. If `crate::capture::store` is not `pub` enough, promote `read_capture_markdown` to `pub(crate)` (it is already `pub` per store.rs:99).

- [ ] Run (foreground): `cargo build -p bossclawd` (clean now that all arms exist), then `cargo test -p bossclawd`.

- [ ] Add the guest-refusal socket test in `crates/bossclawd/tests/authz.rs` (mirror the `RoleClient` pattern, lines 20-63):
  ```rust
  #[tokio::test]
  async fn rung5_export_ops_are_app_only() {
      let (_dir, sock) = spawn_onboarded_daemon().await;
      let mut guest = RoleClient::connect(&sock, Role::MemoryClient).await;
      for req in [
          Request::BrainVerifyingKey { onboarded: true },
          Request::SetBinding { onboarded: true, attestation: serde_json::Value::Null },
          Request::ExportBundle { onboarded: true, selection: bossclawd_proto::ExportSelectionWire {
              note_event_ids: vec![], session_event_ids: vec![], ingest_event_ids: vec![], description: String::new() } },
      ] {
          match guest.call(req).await {
              Response::Err { kind: OpErrorKindWire::NotPermitted, .. } => {}
              other => panic!("guest must be refused, got {other:?}"),
          }
      }
      let mut app = RoleClient::connect(&sock, Role::App).await;
      match app.call(Request::BrainVerifyingKey { onboarded: true }).await {
          Response::BrainVerifyingKey(k) => assert!(k.starts_with('z')),
          other => panic!("App BrainVerifyingKey should succeed, got {other:?}"),
      }
  }

  /// NEW-A characterization (no socket): the wire frame JSON-ESCAPES the already-JSON .airmem, so
  /// escaping overhead can push an under-MAX_EXPORT_BYTES bundle over MAX_FRAME. A `.airmem` text just
  /// under the 30 MiB core belt, once wrapped in `Response::Bundle` and serialized, exceeds the 32 MiB
  /// frame — which is exactly why `export_dispatch` bounds the SERIALIZED length, not the raw text.
  #[test]
  fn wire_frame_escaping_can_overflow_a_sub_cap_bundle() {
      use bossclawd_proto::{Response, MAX_FRAME};
      // 30 MiB of quote/backslash-heavy JSON text (worst-case escaping — each byte doubles on the wire).
      let text = "\\\"".repeat(30 * 1024 * 1024 / 2);
      assert!(text.len() <= 30 * 1024 * 1024, "raw text is within the core belt");
      let wire = serde_json::to_vec(&Response::Bundle(text)).unwrap();
      assert!(wire.len() > MAX_FRAME, "escaped wire frame overflows MAX_FRAME → export_dispatch must refuse with BundleTooLarge, not a generic frame error");
  }
  ```
  Run (foreground): `cargo test -p bossclawd --test authz rung5 && cargo test -p bossclawd --test authz wire_frame`. Expected: both pass.

- [ ] Commit: `git add crates/bossclawd-proto/src/lib.rs crates/bossclawd/Cargo.toml crates/bossclawd/src/engine/mod.rs crates/bossclawd/src/server.rs crates/bossclawd/tests/authz.rs && git commit -m "feat(rung5): App-only wire ops BrainVerifyingKey/SetBinding/ExportBundle + BundleTooLarge guard + bounded session-body reads"`

---

## Task 9 — `air-verify` CLI crate

A tiny native crate (its own bin) so `bossclaw-bundle` stays a pure wasm-clean lib. Zero external deps beyond `bossclaw-bundle` — hand-rolled arg matching per the `air-memory-mcp` zero-dep precedent.

**Files:**
- Create `crates/air-verify/Cargo.toml`, `src/main.rs`, `tests/clean_machine.rs`
- Modify `Cargo.toml` (workspace members)

Steps:

- [ ] Add the member and create `crates/air-verify/Cargo.toml`:
  ```toml
  [package]
  name = "air-verify"
  version = "0.0.1"
  edition = "2021"
  license = "Apache-2.0"
  description = "air-verify: offline .airmem bundle verifier CLI (Rung-5 SP-V1). L1 self-consistency + honest kind-aware labels."
  repository = "https://github.com/AgentIdentityRegistry/air-note"

  [[bin]]
  name = "air-verify"
  path = "src/main.rs"

  [dependencies]
  bossclaw-bundle = { path = "../bossclaw-bundle" }
  serde_json = "1"

  [dev-dependencies]
  bossclaw-canon = { path = "../bossclaw-canon" }
  multibase = "=0.9.2"
  tempfile = "3"
  ```

- [ ] Write `crates/air-verify/src/main.rs` (std args; exit 0 = L1 verified, 1 = failed, 2 = usage/IO; the per-item labels come straight from the kind-aware verdict):
  ```rust
  //! air-verify: offline .airmem verifier. Usage: `air-verify <file> [--offline]`.
  use std::process::ExitCode;
  use bossclaw_bundle::{verify, Airmem, IdentityLevel, OfflineResolver, VerifyError};

  fn main() -> ExitCode {
      let args: Vec<String> = std::env::args().skip(1).collect();
      let mut file: Option<String> = None;
      for a in &args {
          match a.as_str() {
              "--offline" => {}
              "-h" | "--help" => { eprintln!("usage: air-verify <file.airmem> [--offline]"); return ExitCode::from(2); }
              s if s.starts_with('-') => { eprintln!("unknown flag: {s}"); return ExitCode::from(2); }
              s => { if file.replace(s.to_string()).is_some() { eprintln!("only one file argument allowed"); return ExitCode::from(2); } }
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

- [ ] Write `crates/air-verify/tests/clean_machine.rs` (temp HOME, no keys; build a valid fixture via bundle's public API, run the bin, assert exit 0; tamper one byte → exit 1):
  ```rust
  use std::process::Command;
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
          items: vec![ItemInput::SealVouched { kind: "session".into(), content: "body".into(), display: Some(serde_json::json!({"title":"S"})) }],
          binding: Binding { payload, identity_signature: bsig }, brain_key: &brain });
      let path = dir.join("fixture.airmem");
      std::fs::write(&path, canonical_json(&bundle).unwrap()).unwrap();
      path
  }

  #[test]
  fn cli_verifies_valid_and_rejects_tampered_on_a_clean_home() {
      let dir = tempfile::tempdir().unwrap();
      let path = write_valid(dir.path());
      let ok = Command::new(env!("CARGO_BIN_EXE_air-verify")).arg(&path).arg("--offline").env("HOME", dir.path()).output().unwrap();
      assert!(ok.status.success(), "stdout={}", String::from_utf8_lossy(&ok.stdout));
      let out = String::from_utf8_lossy(&ok.stdout);
      assert!(out.contains("unverified (offline)"));
      assert!(out.contains("captured session, content only; not independently verified"));

      let bad = dir.path().join("bad.airmem");
      std::fs::write(&bad, std::fs::read_to_string(&path).unwrap().replace("body", "evil")).unwrap();
      let out2 = Command::new(env!("CARGO_BIN_EXE_air-verify")).arg(&bad).arg("--offline").output().unwrap();
      assert_eq!(out2.status.code(), Some(1));
      assert!(String::from_utf8_lossy(&out2.stdout).contains("❌ FAILED"));
  }
  ```
  Run (foreground): `cargo test -p air-verify`. Expected: pass.

- [ ] Commit: `git add crates/air-verify Cargo.toml && git commit -m "feat(rung5): air-verify CLI (offline L1 verdict + kind-aware labels + clean-machine e2e)"`

---

## Task 10 — App: tauri command + TS api + Library multi-select + kind-aware review sheet

**Files:**
- Modify `apps/desktop/src-tauri/src/engine/client.rs` + `engine/mod.rs` (proxies for the 3 ops — mirror `set_reflect_enabled`, client.rs:459 / mod.rs:518)
- Create `apps/desktop/src-tauri/src/commands/export.rs`; modify `commands/mod.rs` (`pub mod export;`) + `main.rs` (`generate_handler!`, line 251)
- Create `apps/desktop/src/api/export.ts`
- Create `apps/desktop/src/memory/ExportReviewSheet.tsx` + `ExportReviewSheet.test.tsx`
- Modify `apps/desktop/src/memory/LibraryPanel.tsx`

Steps:

- [ ] Add the app-side proxies. In `engine/client.rs` add `brain_verifying_key` (→ `Response::BrainVerifyingKey`), `set_binding` (`self.unit(Request::SetBinding{..})`), `export_bundle` (→ `Response::Bundle`, and map `Response::BundleTooLarge{bytes,max}` to a typed `EngineOpError::Rejected(format!("bundle too large: {bytes} bytes exceeds {max}"))` so the UI can surface it). Add the same passthroughs in `engine/mod.rs` (mirror line 518). Use `bossclawd_proto::ExportSelectionWire`. Run (foreground): `cargo build -p air_agent_desktop`.

- [ ] Create `apps/desktop/src-tauri/src/commands/export.rs`. The command: (1) fetch the brain verifying key from the daemon (round-trip, §2.3); (2) if no binding is stored, mint one — load the `did:wba` identity key (`state.identity_store.load_signing_key()` → `AgentKeypair::from_secret_bytes`, `air/did_wba.rs:13`), read the did (`state.identity_store.load_metadata()`), build `BindingPayload`, sign `jcs(payload)` with `AgentKeypair::sign` (`did_wba.rs:33` → raw 64 bytes), wrap as `multibase::encode(Base58Btc, sig)` (§2.3/§10), `SetBinding`; swallow the "already stored" reject as success (idempotent); (3) `ExportBundle`; (4) save via `tauri-plugin-dialog` — the sibling of `pick_folder`/`pick_file` (mirror `engine_pick_folder`, `commands/engine.rs:159-168`, `app.dialog().file().pick_folder(cb)`). **Confirm the save-method name against the installed `tauri-plugin-dialog` version** (v2 exposes `.save_file(cb)` → `Option<FilePath>`); no in-repo save-dialog call site exists to copy. Write bytes to `<path>.tmp` then `std::fs::rename` (all-or-nothing — S1). Sketch:
  ```rust
  //! Rung-5 SP-V1 export command. Mints the identity binding on first export, then produces + saves a
  //! sealed .airmem. Publishes nothing (S1: export mutates the brain only via the one-time SetBinding).
  use tauri::State;
  use crate::commands::identity::AppState;
  use bossclawd_proto::ExportSelectionWire;

  #[tauri::command]
  pub async fn export_bundle(app: tauri::AppHandle, state: State<'_, AppState>,
      note_event_ids: Vec<String>, session_event_ids: Vec<String>, ingest_event_ids: Vec<String>,
      description: String) -> Result<Option<String>, String> {
      let onboarded = state.identity_store.is_onboarded();
      ensure_binding(&state, onboarded).await?;   // first-export mint (idempotent)
      let text = state.engine.export_bundle(onboarded, ExportSelectionWire {
          note_event_ids, session_event_ids, ingest_event_ids, description,
      }).await.map_err(|e| e.to_string())?;
      save_airmem(&app, &text).await   // Some(path) saved, None if cancelled
  }
  ```
  Implement `ensure_binding` (round-trip brain key → build payload → `AgentKeypair::sign` → `multibase::encode` → `SetBinding`; treat an "already stored" reject as `Ok`) and `save_airmem`. Register in `main.rs` `generate_handler!` (`commands::export::export_bundle,`) and `commands/mod.rs` (`pub mod export;`). A Rust unit test (mirror the `RecordingTransport` double in `commands/integrations.rs` tests) asserts the mint path emits exactly one `Request::SetBinding` and export one `Request::ExportBundle`. Run (foreground): `cargo build -p air_agent_desktop` + `cargo clippy -p air_agent_desktop --all-targets -- -D warnings`.

- [ ] Write `apps/desktop/src/api/export.ts`:
  ```ts
  import { invoke } from "@tauri-apps/api/core";

  /** Export the selected memories as a signed .airmem. Returns the saved path, or null if cancelled. */
  export const exportBundle = (
    noteEventIds: string[], sessionEventIds: string[], ingestEventIds: string[], description: string,
  ): Promise<string | null> =>
    invoke<string | null>("export_bundle", { noteEventIds, sessionEventIds, ingestEventIds, description });
  ```

- [ ] Write `ExportReviewSheet.test.tsx` FIRST (TDD), asserting kind-aware disclosure copy (spec §6, A-N1): (a) a stamped note row states its FULL signed record ships; (b) a session row states its FULL transcript ships, content-only, "captured session … not independently verified"; (c) an ingest row states the file extract ships, content-only, "ingested file extract … not independently verified"; (d) the S5 plain-language line present; (e) zero hardcoded colors. Then `ExportReviewSheet.tsx`. Props: `{ notes:{id,text}[]; sessions:{id,title}[]; ingests:{id,path}[]; onConfirm; onCancel }`. Copy:
  - note: "Ships the full signed record of this note (its exact bytes + your brain's signature). The receiver can verify it stand-alone."
  - session: "Ships the FULL session transcript — content and title only, no file paths or session id. Labeled 'captured session'; not independently verified."
  - ingest: "Ships the full extracted file text — content only, no file path. Labeled 'ingested file extract'; not independently verified."
  - S5 (always): "Anyone you send this file to can read the plaintext of everything selected. Exporting does not publish anything."
  Use `Card`/`Button` + CSS tokens only (`var(--…)`; shell-redesign gate, `LibraryPanel.tsx:31`). Run (foreground): `npm --prefix apps/desktop run test -- ExportReviewSheet`. Also `grep -rn "#[0-9a-fA-F]\{3,6\}" apps/desktop/src/memory/ExportReviewSheet.tsx` → expect none.

- [ ] Wire the entry point in `LibraryPanel.tsx`: add `Set<string>` selection state for notes/sessions/ingests (the Library lists sessions + notes today; add ingests via the existing `listFiles()` api — `api/engine.ts`), a per-row select affordance, an "Export signed bundle" button opening `<ExportReviewSheet>`, and on confirm call `exportBundle(...)`. Token-styled; reuse existing rendering. Extend/add the LibraryPanel vitest (mirror `MemoryPanel.test.tsx`): selecting 1 note + Export opens the sheet; confirming invokes `exportBundle` with the ids. Run (foreground): `npm --prefix apps/desktop run test` + `npm --prefix apps/desktop run typecheck` (or `tsc --noEmit`).

- [ ] Commit: `git add apps/desktop && git commit -m "feat(rung5): app export — tauri command (binding mint + save), TS api, Library multi-select + kind-aware review sheet"`

---

## Task 11 — Conformance vectors + Story-C large fixture + final exit gate

**Files:**
- Create `tests/vectors/{valid,tamper_seal,tamper_item,tamper_binding_hash,origin_mismatch,re_attribution}.airmem`, `tests/vectors/README.md`
- Create `crates/bossclaw-bundle/tests/conformance.rs`
- Add the Story-C large-fixture test to `crates/bossclaw-core` tests

Steps:

- [ ] Write `crates/bossclaw-bundle/tests/conformance.rs` — reads the committed vectors and asserts each verdict/error; regenerates only under `AIRMEM_REGEN=1` (committed files stay the source of truth for SP-V2's cross-repo CI):
  ```rust
  use bossclaw_bundle::{verify, Airmem, OfflineResolver, VerifyError};
  fn load(name: &str) -> Airmem {
      let p = format!("{}/../../tests/vectors/{name}", env!("CARGO_MANIFEST_DIR"));
      serde_json::from_str(&std::fs::read_to_string(&p).unwrap_or_else(|_| panic!("missing vector {p}"))).unwrap()
  }
  #[test] fn valid_vector_verifies() { verify(&load("valid.airmem"), &OfflineResolver).expect("valid"); }
  #[test] fn tamper_seal_fails() { assert_eq!(verify(&load("tamper_seal.airmem"), &OfflineResolver), Err(VerifyError::SealInvalid)); }
  #[test] fn tamper_item_fails() { assert!(matches!(verify(&load("tamper_item.airmem"), &OfflineResolver), Err(VerifyError::ItemHashMismatch(_)))); }
  #[test] fn tamper_binding_hash_fails() { assert_eq!(verify(&load("tamper_binding_hash.airmem"), &OfflineResolver), Err(VerifyError::BindingHashMismatch)); }
  #[test] fn origin_mismatch_fails() { assert!(matches!(verify(&load("origin_mismatch.airmem"), &OfflineResolver), Err(VerifyError::OriginMismatch(_)))); }
  #[test] fn re_attribution_fails() { assert!(matches!(verify(&load("re_attribution.airmem"), &OfflineResolver), Err(VerifyError::BindingHashMismatch) | Err(VerifyError::BindingDidMismatch))); }
  ```
  Add a `regen` helper gated on `std::env::var("AIRMEM_REGEN")` that authors each fixture from fixed keys (reuse Task 6's `valid_bundle` shape + the tamper transforms; `origin_mismatch` = flip `carried_origin` on a stamped item; `re_attribution` = swap the binding). Run once `AIRMEM_REGEN=1 cargo test -p bossclaw-bundle --test conformance`, then `cargo test -p bossclaw-bundle --test conformance` (reads committed files) — expected: 6 pass. Write `tests/vectors/README.md` (format version, Merkle A7 rules, "consumed by air-site verify-page CI, cross-repo, spec §8").

- [ ] Add the Story-C whole-brain large-fixture test (notes + sessions + ingests) to `crates/bossclaw-core` tests, characterizing size + the `BundleTooLarge` bound. Append several `remember` notes + a REAL ingested file via the unix ingest path (mirror the `#[cfg(unix)]` ingest setup already in `log.rs` tests, so it lands in the `files` projection and `current_ingests()` includes it, carrying a `provenance{canonical_path, content_hash, grant_root}` block, ingest.rs:698-711) + a captured session, store a binding, then: (a) `export_bundle` over a modest select-all verifies green and `item_count` matches the selection; (b) `estimate_export_bytes` on a synthetically padded selection exceeds `MAX_EXPORT_BYTES` (assert the core pre-build estimate crosses the bound — the daemon turns this into `BundleTooLarge`); (c) NEW-B end-to-end: assert the serialized `.airmem` contains NONE of `canonical_path` / `content_hash` / `grant_root` (core discloses the ingest's `content["text"]` only, stripping the real provenance block). Keep it hermetic (`open_log` + `MockEmbedder`); the ingest arm is `#[cfg(unix)]` like the fold it mirrors. The AUTHORITATIVE wire-length belt (NEW-A: serialized `Response::Bundle` length vs `MAX_FRAME`, which the raw-text estimate cannot see because of JSON-in-JSON escaping) is characterized in Task 8's `wire_frame_escaping_can_overflow_a_sub_cap_bundle` test — this core test covers only the coarse pre-build estimate. Run (foreground): `cargo test -p bossclaw-core story_c`. Expected: pass.

- [ ] Final exit gate (all foreground; each must be green):
  - `cargo clippy --workspace --all-targets -- -D warnings` (whole workspace — only here).
  - `cargo test -p bossclaw-canon && cargo test -p bossclaw-bundle && cargo test -p air-verify` (new crates).
  - `cargo test -p bossclaw-core && cargo test -p bossclawd-proto && cargo test -p bossclawd` (touched crates).
  - `cargo test -p memharness` (recall-neutrality — export is a reader; suite stays green untouched, spec §8).
  - `cargo check -p bossclaw-canon --target wasm32-unknown-unknown` (wasm boundary intact).
  - `npm --prefix apps/desktop run test && npm --prefix apps/desktop run typecheck` (frontend).
  - Placeholder sweep: `grep -rn "TODO\|todo!()\|unimplemented!()\|\.skip(\|\.only(" crates/bossclaw-canon crates/bossclaw-bundle crates/air-verify apps/desktop/src/memory/ExportReviewSheet.tsx` → expect no matches.

- [ ] Commit: `git add tests/vectors crates/bossclaw-bundle/tests/conformance.rs crates/bossclaw-core && git commit -m "feat(rung5): committed .airmem conformance vectors (incl. origin_mismatch) + Story-C large fixture + final gate green"`

---

## Spec §-coverage map (SP-V1 scope, Rev 2)

- §1 SP-V1 split; Story C = notes + sessions + **ingests** (Task 7 `current_ingests`/gather, Task 10 UI, Task 11 large fixture). Dossiers EXCLUDED — empty by construction today (Rung 4 dormant); the `dossier` kind-label exists in verify for forward-compat but nothing gathers it (decision 6). SP-V2 (page, registry HTTP, PublishClaim) OUT.
- §2.1 canon extraction + pins + wasm-clean → Task 1.
- §2.2 format, two item classes, Merkle A7 (leaf + carried_origin excluded), no-float, binding_hash → Tasks 2, 3, 5, 6.
- §2.3 binding payload + SetBinding validation C-NEW-4 + resolution-from-sealed-manifest → Tasks 4, 7, 8, 10.
- §2.4 wire ops App-only (incl. **BrainVerifyingKey**), ExportBundle verify_chain-first → Tasks 7, 8.
- §2.5 L1 checklist incl. binding_hash recompute + Unverified-offline + L2 seam (encoding-agnostic) → Tasks 4, 6.
- §3 H1-H5: kind-aware seal-vouched labels (session/ingest/dossier, never cross-rendered), stamped cross-checked (**OriginMismatch live**), origin-unattested (C-NEW-1b), H5 on every surface → Tasks 6, 9, 10.
- §4 S1/S4/S5 + S3 guest scoping → Tasks 7, 8, 10. (S2/S6/S7 = pin/page = SP-V2.)
- §7 verify enum + export refusals incl. **BundleTooLarge** (pre-frame estimate) → Tasks 6, 7, 8.
- §8 tamper matrix (SealInvalid×2, ItemHashMismatch, ts-flip, ItemStampInvalid via re-seal, TreeMismatch, BindingHashMismatch, BindingKeyMismatch, BindingInvalid, BindingDidMismatch, OriginMismatch, FormatTooNew), re-attribution (both layers), foreign-event laundering, no-leak, canon regression vectors, clean-machine e2e, **Story-C large fixture**, recall-neutrality, cross-repo conformance (incl. origin_mismatch) → Tasks 1, 6, 7, 8, 9, 11. (Rendering-safety DOM tests = SP-V2.)
- §9 non-goals respected (no PublishClaim/encryption/sub-share/authored-class; epoch reserved; dossier derivation not verified).

## Plan-stage decisions (Rev 2)

1. **Canon extraction:** `pub use bossclaw_canon::{event, sign}` + `From<CanonError> for BossclawError` (variant-for-variant, string-preserving); pinned by the pre-existing `tests/vectors.rs` staying green (Task 1).
2. **`BrainVerifyingKey` read op:** RATIFIED into spec §2.4 (commit `472fc02`) — the app round-trips the brain key to mint the binding.
3. **Binding canonical form:** binding keys are bare-32 multibase base58btc; the identity signature wraps `did_wba.rs:33`'s raw 64 bytes as multibase base58btc. **L2 compares raw 32-byte keys** (decode-both, `0xed01` multikey OR bare) so a registry-multikey identity still matches a bare-stored binding (finding #6).
4. **`OriginMismatch` — LIVE (resolved):** stamped items carry a leaf-EXCLUDED `carried_origin` token; `build` derives it, `verify` cross-checks it against the stamp-covered event bytes → `OriginMismatch{i}`. Flip test (Task 6) + `origin_mismatch` conformance vector (Task 11).
5. **32 MiB cliff — resolved:** `estimate_export_bytes` (pre-body, pre-seal, from in-log data + session `approx_bytes`) → the daemon's `BundleTooLarge{bytes,max}` typed refusal; `MAX_EXPORT_BYTES = 30 MiB` (headroom below `MAX_FRAME`); a measurement checkpoint (top of plan) + a Story-C large fixture (Task 11) characterize the bound. Streaming stays the deferred §10 escalation.
6. **Story-C scope:** notes (stamped) + sessions + ingests (seal-vouched). **Dossiers excluded with zero data loss** — Rung 4 ships dormant, so the class is empty by construction on any real brain; the `dossier` verifier label is present for forward-compat only. Revisit when reflection dogfood turns dossiers on (spec §1 as amended).
7. **Session bodies (layering):** the daemon reads bodies through its bounded `read_capture_markdown` (16 MiB cap, store.rs:99), confinement-checks the path (mirror `get_session`, server.rs:531), and passes content into a fs-free core `export_bundle`; a missing/oversize body becomes the store.rs:485-style placeholder, never an aborted export (finding #4).
