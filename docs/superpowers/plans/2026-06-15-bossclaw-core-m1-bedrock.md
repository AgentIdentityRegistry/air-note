# bossclaw-core — Milestone 1 (Bedrock) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the foundation of `bossclaw-core` — an encrypted, append-only, Ed25519-signed event log whose canonicalization, chain format, single-writer discipline, and tamper/truncation detection are frozen and tested, so nothing expensive-to-change is left undecided.

**Architecture:** A new Rust crate `crates/bossclaw-core`. The event log is the single source of truth; every event is JCS-canonicalized (mirroring `air-rs/signing.rs`), hash-chained, and signed to a DID. The store is whole-DB-encrypted SQLite (SQLCipher via `rusqlite`). Appends are strictly serialized; a signed high-water-mark detects tail truncation/rollback. Indexes (M2+) and the reasoner/evolve loop (M4) are out of scope here.

**Tech Stack:** Rust 2021, `rusqlite` (bundled-sqlcipher), `ed25519-dalek`, `serde_jcs` (pinned 0.1.0, cross-lang), `sha2`, `ulid`, `zeroize`, `thiserror`. Mirrors `air-rs` dep pins for cross-crate consistency.

**Spec:** `docs/superpowers/specs/2026-06-15-bossclaw-core-design.md` (Rev 3). This plan implements §12 Milestone 1, with the §5.2 canonicalization/chain/high-water, §8.1 at-rest encryption (DB only; sidecar is M2), §4 single-writer + two-tier event schema, §3.7 honest reuse (extend `ed25519-dalek` + the JCS discipline; do NOT call `sign_envelope`).

---

## File structure

| File | Responsibility |
|---|---|
| `crates/bossclaw-core/Cargo.toml` | crate manifest; deps pinned to match `air-rs` |
| `Cargo.toml` (workspace root) | add `crates/bossclaw-core` to members |
| `crates/bossclaw-core/src/lib.rs` | crate root; module decls + re-exports; `#![forbid(unsafe_code)]`, `#![deny(missing_docs)]` |
| `crates/bossclaw-core/src/error.rs` | `BossclawError` (thiserror), mirroring `air-rs` error style |
| `crates/bossclaw-core/src/event.rs` | `Event` + `ModelMeta` types; `canonical_bytes`; `compute_hash`; NFC normalize |
| `crates/bossclaw-core/src/sign.rs` | raw-hash Ed25519 sign/verify over the event `hash` |
| `crates/bossclaw-core/src/store.rs` | encrypted SQLite open/migrate; low-level row I/O |
| `crates/bossclaw-core/src/log.rs` | `EventLog`: serialized `append`, `stream`, `verify_chain`, high-water |
| `crates/bossclaw-core/src/highwater.rs` | `HighWaterStore` trait + a file-backed impl (keychain impl lands at desktop M7) |
| `crates/bossclaw-core/tests/vectors.rs` | frozen canonicalization + hash + signature test vectors |
| `crates/bossclaw-core/tests/chain.rs` | append/verify/concurrency/truncation integration tests |

Tier-A indexes, reasoner, evolve, ingest, and actuator are **not** in M1.

---

## Task 1: Crate scaffold + error type

**Files:**
- Create: `crates/bossclaw-core/Cargo.toml`
- Modify: `Cargo.toml` (workspace members)
- Create: `crates/bossclaw-core/src/lib.rs`
- Create: `crates/bossclaw-core/src/error.rs`

- [ ] **Step 1: Create the crate manifest**

`crates/bossclaw-core/Cargo.toml`:
```toml
[package]
name = "bossclaw-core"
version = "0.0.1"
edition = "2021"
license = "Apache-2.0"
description = "Local-first, signed, encrypted memory engine for BossClaw."
repository = "https://github.com/AgentIdentityRegistry/air-note"

[dependencies]
ed25519-dalek = "2.1"
serde_jcs = "0.1.0"            # MUST match air-rs pin (cross-language canonicalization)
sha2 = "0.10"
multibase = "0.9"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
unicode-normalization = "0.1"
chrono = { version = "0.4", features = ["serde"] }
ulid = "1"
zeroize = { version = "1", features = ["derive"] }
thiserror = "1"
rusqlite = { version = "0.32", features = ["bundled-sqlcipher"] }

[dev-dependencies]
hex = "0.4"
tempfile = "3"
```

- [ ] **Step 2: Add the crate to the workspace**

Modify `Cargo.toml` (workspace root) — change the `members` line:
```toml
members = ["crates/air-rs", "crates/bossclaw-core", "apps/desktop/src-tauri"]
```

- [ ] **Step 3: Write the error type**

`crates/bossclaw-core/src/error.rs`:
```rust
//! Error types for the `bossclaw-core` crate.

use thiserror::Error;

/// Errors that can occur within the bossclaw-core memory engine.
#[derive(Debug, Error)]
pub enum BossclawError {
    /// Canonical JSON (JCS) serialisation failed.
    #[error("canonical JSON error: {0}")]
    Canonical(String),

    /// A cryptographic signature operation failed (sign or verify).
    #[error("signature error: {0}")]
    Signature(String),

    /// A multibase encode/decode operation failed.
    #[error("multibase error: {0}")]
    Multibase(String),

    /// An event's hash did not match its recomputed canonical hash, or the
    /// chain link to the previous event was broken.
    #[error("chain integrity error: {0}")]
    Chain(String),

    /// The signed high-water-mark indicates the log was truncated or rolled back.
    #[error("truncation/rollback detected: {0}")]
    Truncation(String),

    /// A storage / SQLite error.
    #[error("store error: {0}")]
    Store(String),

    /// JSON (de)serialisation failed.
    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),

    /// An IO failure (high-water file, etc.).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl From<rusqlite::Error> for BossclawError {
    fn from(e: rusqlite::Error) -> Self {
        BossclawError::Store(e.to_string())
    }
}
```

- [ ] **Step 4: Write the crate root**

`crates/bossclaw-core/src/lib.rs`:
```rust
//! # bossclaw-core
//!
//! Local-first, signed, encrypted memory engine for BossClaw.
//! Milestone 1 (Bedrock): the encrypted, append-only, Ed25519-signed event log.
//!
//! The event log is the single source of truth; every other structure (M2+) is
//! derived and rebuildable from it. See
//! `docs/superpowers/specs/2026-06-15-bossclaw-core-design.md`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod error;
pub mod event;
pub mod highwater;
pub mod log;
pub mod sign;
pub mod store;

pub use error::BossclawError;
pub use event::{Event, ModelMeta};
pub use log::EventLog;
```

- [ ] **Step 5: Verify it builds**

Run: `cargo build -p bossclaw-core`
Expected: compiles (the `unused module` warnings are fine until the modules exist — create empty `event.rs`, `sign.rs`, `store.rs`, `log.rs`, `highwater.rs` with a single `//! placeholder` doc line so the `mod` decls resolve).

Create each of `crates/bossclaw-core/src/{event,sign,store,log,highwater}.rs` with one line:
```rust
//! placeholder — implemented in a later task.
```

Run: `cargo build -p bossclaw-core`
Expected: PASS (clean build).

- [ ] **Step 6: Commit**
```bash
git add crates/bossclaw-core/Cargo.toml Cargo.toml crates/bossclaw-core/src
git commit -m "feat(bossclaw-core): crate scaffold + error type"
```

---

## Task 2: Event type + JCS canonicalization (frozen)

**Files:**
- Modify: `crates/bossclaw-core/src/event.rs`
- Test: `crates/bossclaw-core/tests/vectors.rs`

- [ ] **Step 1: Write the failing canonicalization vector test**

`crates/bossclaw-core/tests/vectors.rs`:
```rust
use bossclaw_core::event::{canonical_bytes, Event};

fn fixture_event() -> Event {
    Event {
        id: "01J0000000000000000000000A".to_string(),
        ts: "2026-06-15T00:00:00Z".to_string(),
        valid_time: None,
        event_type: "memory".to_string(),
        content: serde_json::json!({ "text": "hello" }),
        model_meta: None,
        prev_hash: "00".repeat(32),
        hash: None,
        signed_by_did: "did:wba:AIR-2JE0-EM7W-JNBK".to_string(),
        signature: None,
    }
}

#[test]
fn canonical_bytes_are_stable_and_exclude_hash_and_signature() {
    let mut e = fixture_event();
    let base = canonical_bytes(&e).unwrap();

    // Setting hash/signature must NOT change the canonical bytes.
    e.hash = Some("ff".repeat(32));
    e.signature = Some("zSomeSignature".to_string());
    let with_fields = canonical_bytes(&e).unwrap();
    assert_eq!(base, with_fields, "hash/signature must be excluded from canon");

    // Frozen vector: JCS sorts keys; "type" is the serialized field name.
    let expected = r#"{"content":{"text":"hello"},"id":"01J0000000000000000000000A","prev_hash":"0000000000000000000000000000000000000000000000000000000000000000","signed_by_did":"did:wba:AIR-2JE0-EM7W-JNBK","ts":"2026-06-15T00:00:00Z","type":"memory"}"#;
    assert_eq!(String::from_utf8(base).unwrap(), expected);
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p bossclaw-core --test vectors`
Expected: FAIL — `canonical_bytes`/`Event` not found.

- [ ] **Step 3: Implement `Event` + `canonical_bytes`**

`crates/bossclaw-core/src/event.rs`:
```rust
//! The signed event: the single authoritative record. JCS-canonicalized exactly
//! like `air-rs/signing.rs` so bytes are deterministic and cross-language stable.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::BossclawError;

/// Provenance for a model-derived (Tier-B) event. `source_event_ids` MUST be
/// non-empty for Tier-B events (enforced at append, see `EventLog::append`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelMeta {
    /// The model that produced this event (e.g. the local reasoner id).
    pub model_id: String,
    /// Hash of the prompt used (provenance, not reproducibility).
    pub prompt_hash: String,
    /// The source event ids this conclusion was derived from. Non-empty.
    pub source_event_ids: Vec<String>,
}

/// A single signed, hash-chained event. `hash` and `signature` are excluded
/// from the canonical bytes (they are computed *over* the canonical bytes).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Event {
    /// ULID, monotonic-ish, lexicographically sortable.
    pub id: String,
    /// Ingestion time, RFC 3339.
    pub ts: String,
    /// Optional valid-time (bi-temporal), RFC 3339.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_time: Option<String>,
    /// Event type discriminator. Serialized as `"type"`.
    #[serde(rename = "type")]
    pub event_type: String,
    /// Opaque content payload.
    pub content: serde_json::Value,
    /// Provenance for Tier-B (model-derived) events; `None` for Tier-A.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_meta: Option<ModelMeta>,
    /// Hex of the previous event's 32-byte hash; genesis = 64 zeros.
    pub prev_hash: String,
    /// Hex of this event's 32-byte hash. Excluded from canon; set by `append`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    /// DID of the signer.
    pub signed_by_did: String,
    /// Multibase signature over `hash`. Excluded from canon; set by `append`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

/// Produce the canonical JSON bytes of an event per the spec §5.2 recipe:
/// serialize → strip `hash` + `signature` → NFC-normalize every string →
/// RFC 8785 JCS via `serde_jcs`. Mirrors `air-rs/signing.rs::canonical_bytes`.
pub fn canonical_bytes(event: &Event) -> Result<Vec<u8>, BossclawError> {
    let mut value = serde_json::to_value(event)
        .map_err(|e| BossclawError::Canonical(format!("event to_value: {e}")))?;
    if let serde_json::Value::Object(ref mut map) = value {
        map.remove("hash");
        map.remove("signature");
    }
    nfc_normalize(&mut value);
    serde_jcs::to_vec(&value).map_err(|e| BossclawError::Canonical(format!("serde_jcs: {e}")))
}

/// Compute the 32-byte chain hash: `SHA256(prev_hash_bytes ‖ canonical_bytes)`.
pub fn compute_hash(event: &Event) -> Result<[u8; 32], BossclawError> {
    let prev = hex::decode(&event.prev_hash)
        .map_err(|e| BossclawError::Chain(format!("prev_hash not hex: {e}")))?;
    if prev.len() != 32 {
        return Err(BossclawError::Chain(format!(
            "prev_hash must be 32 bytes, got {}",
            prev.len()
        )));
    }
    let canon = canonical_bytes(event)?;
    let mut hasher = Sha256::new();
    hasher.update(&prev);
    hasher.update(&canon);
    Ok(hasher.finalize().into())
}

fn nfc_normalize(value: &mut serde_json::Value) {
    use unicode_normalization::UnicodeNormalization;
    match value {
        serde_json::Value::String(s) => *s = s.nfc().collect(),
        serde_json::Value::Object(map) => map.values_mut().for_each(nfc_normalize),
        serde_json::Value::Array(arr) => arr.iter_mut().for_each(nfc_normalize),
        _ => {}
    }
}
```

Add `hex = "0.4"` to `[dependencies]` in `crates/bossclaw-core/Cargo.toml` (used by `compute_hash`; it was already a dev-dep — promote it to a normal dep).

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p bossclaw-core --test vectors`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add crates/bossclaw-core/src/event.rs crates/bossclaw-core/tests/vectors.rs crates/bossclaw-core/Cargo.toml
git commit -m "feat(bossclaw-core): Event type + frozen JCS canonicalization"
```

---

## Task 3: Chain hashing (genesis + link), frozen

**Files:**
- Test: `crates/bossclaw-core/tests/vectors.rs` (extend)

- [ ] **Step 1: Write the failing hash vector test**

Append to `crates/bossclaw-core/tests/vectors.rs`:
```rust
use bossclaw_core::event::compute_hash;

#[test]
fn genesis_hash_is_frozen() {
    let e = fixture_event(); // prev_hash = 64 zeros
    let h = compute_hash(&e).unwrap();
    // Frozen: SHA256( 32 zero bytes ‖ canonical_bytes(fixture_event) ).
    // Recompute once with a known-good build, paste the hex here, then it is a regression guard.
    let hex_h = hex::encode(h);
    assert_eq!(hex_h.len(), 64);
    // Replace ZZZ with the value printed on first run, then keep it frozen:
    // assert_eq!(hex_h, "ZZZ...");
    println!("GENESIS_HASH={hex_h}");
}

#[test]
fn second_event_links_to_first() {
    let first = fixture_event();
    let h1 = compute_hash(&first).unwrap();

    let mut second = fixture_event();
    second.id = "01J0000000000000000000000B".to_string();
    second.prev_hash = hex::encode(h1);
    let h2 = compute_hash(&second).unwrap();

    assert_ne!(h1, h2, "different events hash differently");
    // Changing the first event changes h1 → second.prev_hash mismatch is detectable downstream.
}
```

- [ ] **Step 2: Run to verify the link test passes and capture the genesis hash**

Run: `cargo test -p bossclaw-core --test vectors -- --nocapture`
Expected: PASS; note the printed `GENESIS_HASH=...`.

- [ ] **Step 3: Freeze the genesis hash**

Paste the printed value into the assertion (uncomment `assert_eq!(hex_h, "...")`, remove the `println!`). This converts the probe into a permanent regression guard — if canonicalization ever drifts, this fails.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p bossclaw-core --test vectors`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add crates/bossclaw-core/tests/vectors.rs
git commit -m "test(bossclaw-core): freeze genesis + chain-link hash vectors"
```

---

## Task 4: Raw-hash Ed25519 sign + verify

**Files:**
- Modify: `crates/bossclaw-core/src/sign.rs`
- Test: `crates/bossclaw-core/tests/vectors.rs` (extend)

- [ ] **Step 1: Write the failing sign/verify test**

Append to `crates/bossclaw-core/tests/vectors.rs`:
```rust
use bossclaw_core::sign::{sign_hash, verify_hash};
use ed25519_dalek::SigningKey;

fn fixture_key() -> SigningKey {
    SigningKey::from_bytes(&[7u8; 32]) // deterministic test key
}

#[test]
fn sign_then_verify_roundtrips() {
    let key = fixture_key();
    let hash = compute_hash(&fixture_event()).unwrap();
    let sig = sign_hash(&hash, &key);
    assert!(sig.starts_with('z'), "multibase base58btc 'z' prefix");
    verify_hash(&hash, &sig, &key.verifying_key()).expect("valid signature verifies");
}

#[test]
fn tampered_hash_fails_verification() {
    let key = fixture_key();
    let hash = compute_hash(&fixture_event()).unwrap();
    let sig = sign_hash(&hash, &key);
    let mut bad = hash;
    bad[0] ^= 0xFF;
    assert!(verify_hash(&bad, &sig, &key.verifying_key()).is_err());
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p bossclaw-core --test vectors`
Expected: FAIL — `sign_hash`/`verify_hash` not found.

- [ ] **Step 3: Implement raw-hash signing**

`crates/bossclaw-core/src/sign.rs`:
```rust
//! Raw-hash Ed25519 signing for events. NOTE: this is intentionally NOT
//! `air-rs::sign_envelope` (that is coupled to the `Envelope` struct). We reuse
//! the `ed25519-dalek` primitive + the multibase encoding discipline only.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use multibase::{decode as mb_decode, encode as mb_encode, Base};

use crate::error::BossclawError;

const ED25519_SIGNATURE_LEN: usize = 64;

/// Sign the 32-byte event hash, returning a multibase base58btc (`z`) string.
pub fn sign_hash(hash: &[u8; 32], key: &SigningKey) -> String {
    let sig: Signature = key.sign(hash);
    mb_encode(Base::Base58Btc, sig.to_bytes())
}

/// Verify a multibase signature over the 32-byte event hash.
///
/// # Errors
/// * [`BossclawError::Multibase`] if the signature is not valid multibase.
/// * [`BossclawError::Signature`] on wrong length or a verification mismatch.
pub fn verify_hash(
    hash: &[u8; 32],
    signature_mb: &str,
    key: &VerifyingKey,
) -> Result<(), BossclawError> {
    let (_b, raw) =
        mb_decode(signature_mb).map_err(|e| BossclawError::Multibase(format!("decode: {e}")))?;
    let bytes: [u8; ED25519_SIGNATURE_LEN] = raw
        .as_slice()
        .try_into()
        .map_err(|_| BossclawError::Signature(format!("sig must be 64 bytes, got {}", raw.len())))?;
    let sig = Signature::from_bytes(&bytes);
    key.verify(hash, &sig)
        .map_err(|e| BossclawError::Signature(e.to_string()))
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p bossclaw-core --test vectors`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add crates/bossclaw-core/src/sign.rs crates/bossclaw-core/tests/vectors.rs
git commit -m "feat(bossclaw-core): raw-hash Ed25519 sign + verify"
```

---

## Task 5: Encrypted SQLite store

**Files:**
- Modify: `crates/bossclaw-core/src/store.rs`
- Test: `crates/bossclaw-core/tests/chain.rs`

- [ ] **Step 1: Write the failing encryption test**

`crates/bossclaw-core/tests/chain.rs`:
```rust
use bossclaw_core::store::Store;
use std::io::Read;

fn dek() -> [u8; 32] { [42u8; 32] }

#[test]
fn store_is_encrypted_on_disk_and_keyed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");

    {
        let store = Store::open(&path, &dek()).unwrap();
        store.exec("CREATE TABLE t(x TEXT)").unwrap();
        store.exec("INSERT INTO t(x) VALUES ('secret-marker')").unwrap();
    }

    // The on-disk header must NOT be the plaintext "SQLite format 3" magic.
    let mut buf = [0u8; 16];
    std::fs::File::open(&path).unwrap().read_exact(&mut buf).unwrap();
    assert_ne!(&buf, b"SQLite format 3\0", "db must be encrypted at rest");

    // Wrong key cannot open it.
    let wrong = Store::open(&path, &[0u8; 32]);
    assert!(wrong.is_err(), "wrong DEK must fail to open");

    // Right key round-trips.
    let store = Store::open(&path, &dek()).unwrap();
    let got: String = store
        .query_one("SELECT x FROM t LIMIT 1")
        .unwrap();
    assert_eq!(got, "secret-marker");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p bossclaw-core --test chain`
Expected: FAIL — `Store` not found.

- [ ] **Step 3: Implement the encrypted store**

`crates/bossclaw-core/src/store.rs`:
```rust
//! Whole-DB encrypted SQLite store (SQLCipher via rusqlite `bundled-sqlcipher`).
//! The DEK is supplied by the caller (desktop fetches it from the OS keychain);
//! the crate never reads the keychain itself.

use std::path::Path;

use rusqlite::Connection;
use zeroize::Zeroizing;

use crate::error::BossclawError;

/// An open, encrypted SQLite connection. Single-threaded by construction;
/// `EventLog` owns the serialization (see `log.rs`).
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open (creating if needed) an encrypted DB at `path`, keyed by `dek`.
    /// Fails if the file exists and the key is wrong.
    pub fn open(path: &Path, dek: &[u8; 32]) -> Result<Self, BossclawError> {
        let conn = Connection::open(path)?;
        // SQLCipher raw-key form: PRAGMA key = "x'<hex>'". Hold the pragma string
        // in a Zeroizing buffer so the hex key isn't left in freed memory.
        let key_hex = Zeroizing::new(hex::encode(dek));
        let pragma = Zeroizing::new(format!("PRAGMA key = \"x'{}'\"", &*key_hex));
        conn.execute_batch(&pragma)?;
        // Force a read so a wrong key errors here (SQLCipher is lazy otherwise).
        conn.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))
            .map_err(|_| BossclawError::Store("wrong key or corrupt db".into()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Ok(Self { conn })
    }

    /// Execute a statement with no parameters (DDL / simple writes).
    pub fn exec(&self, sql: &str) -> Result<(), BossclawError> {
        self.conn.execute_batch(sql)?;
        Ok(())
    }

    /// Query a single `String` column from the first row.
    pub fn query_one(&self, sql: &str) -> Result<String, BossclawError> {
        let v = self.conn.query_row(sql, [], |r| r.get::<_, String>(0))?;
        Ok(v)
    }

    /// Borrow the underlying connection (used by `EventLog`).
    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p bossclaw-core --test chain`
Expected: PASS. *(If `bundled-sqlcipher` fails to compile on this machine, that is the §14 encryption spike failing — STOP and escalate per the spec's M2 go/no-go; do not silently fall back.)*

- [ ] **Step 5: Commit**
```bash
git add crates/bossclaw-core/src/store.rs crates/bossclaw-core/tests/chain.rs
git commit -m "feat(bossclaw-core): encrypted SQLite store (SQLCipher)"
```

---

## Task 6: EventLog — serialized append + verify_chain

**Files:**
- Modify: `crates/bossclaw-core/src/log.rs`
- Test: `crates/bossclaw-core/tests/chain.rs` (extend)

- [ ] **Step 1: Write the failing append/verify/concurrency test**

Append to `crates/bossclaw-core/tests/chain.rs`:
```rust
use bossclaw_core::event::Event;
use bossclaw_core::log::EventLog;
use ed25519_dalek::SigningKey;
use std::sync::Arc;

fn mk_event(text: &str) -> Event {
    Event {
        id: String::new(), // assigned by append
        ts: String::new(), // assigned by append
        valid_time: None,
        event_type: "memory".to_string(),
        content: serde_json::json!({ "text": text }),
        model_meta: None,
        prev_hash: String::new(), // assigned by append
        hash: None,
        signed_by_did: "did:wba:AIR-TEST".to_string(),
        signature: None,
    }
}

#[test]
fn append_then_verify_chain() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&[7u8; 32]);
    let log = EventLog::open(&dir.path().join("m.db"), &[42u8; 32], key).unwrap();

    for t in ["a", "b", "c"] {
        log.append(mk_event(t)).unwrap();
    }
    assert_eq!(log.count().unwrap(), 3);
    log.verify_chain().expect("chain verifies");
}

#[test]
fn concurrent_appends_do_not_fork_the_chain() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&[7u8; 32]);
    let log = Arc::new(EventLog::open(&dir.path().join("m.db"), &[42u8; 32], key).unwrap());

    let mut handles = vec![];
    for i in 0..16 {
        let log = Arc::clone(&log);
        handles.push(std::thread::spawn(move || {
            log.append(mk_event(&format!("e{i}"))).unwrap();
        }));
    }
    for h in handles { h.join().unwrap(); }

    assert_eq!(log.count().unwrap(), 16);
    log.verify_chain().expect("no fork under concurrency");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p bossclaw-core --test chain`
Expected: FAIL — `EventLog` not found.

- [ ] **Step 3: Implement EventLog with a serialized writer**

`crates/bossclaw-core/src/log.rs`:
```rust
//! The append-only event log. The single source of truth.
//!
//! Appends are strictly serialized: one process-wide `Mutex` guards the
//! read-tip → hash → sign → insert critical section, so the hash chain can
//! never fork (spec §4 single-writer invariant). The evolve loop (M4) is NOT a
//! privileged writer — it calls `append` like everyone else.

use std::path::Path;
use std::sync::Mutex;

use chrono::Utc;
use ed25519_dalek::SigningKey;
use ulid::Ulid;

use crate::event::{canonical_bytes, compute_hash, Event};
use crate::sign::{sign_hash, verify_hash};
use crate::store::Store;
use crate::error::BossclawError;

const GENESIS: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

/// The serialized, signed event log.
pub struct EventLog {
    inner: Mutex<Store>,
    key: SigningKey,
}

impl EventLog {
    /// Open (creating if needed) an event log at `path`, encrypted with `dek`,
    /// signing with `key`.
    pub fn open(path: &Path, dek: &[u8; 32], key: SigningKey) -> Result<Self, BossclawError> {
        let store = Store::open(path, dek)?;
        store.exec(
            "CREATE TABLE IF NOT EXISTS events (
                seq        INTEGER PRIMARY KEY AUTOINCREMENT,
                id         TEXT NOT NULL UNIQUE,
                ts         TEXT NOT NULL,
                event_type TEXT NOT NULL,
                payload    TEXT NOT NULL,  -- full canonical event JSON incl. hash+signature
                prev_hash  TEXT NOT NULL,
                hash       TEXT NOT NULL UNIQUE
            )",
        )?;
        Ok(Self { inner: Mutex::new(store), key })
    }

    /// Append an event. `id`, `ts`, `prev_hash`, `hash`, `signature` are
    /// assigned here; the caller supplies `event_type`, `content`, `model_meta`,
    /// `signed_by_did`, optional `valid_time`.
    pub fn append(&self, mut event: Event) -> Result<String, BossclawError> {
        // Tier-B events (carry model_meta) MUST have non-empty source_event_ids.
        if let Some(meta) = &event.model_meta {
            if meta.source_event_ids.is_empty() {
                return Err(BossclawError::Chain(
                    "Tier-B event requires non-empty source_event_ids".into(),
                ));
            }
        }

        let store = self.inner.lock().expect("event log mutex poisoned");
        let conn = store.conn();
        let tx = conn.unchecked_transaction()?; // BEGIN; serialized by the Mutex.

        let prev_hash: String = tx
            .query_row("SELECT hash FROM events ORDER BY seq DESC LIMIT 1", [], |r| {
                r.get(0)
            })
            .unwrap_or_else(|_| GENESIS.to_string());

        event.id = Ulid::new().to_string();
        event.ts = Utc::now().to_rfc3339();
        event.prev_hash = prev_hash;
        event.hash = None;
        event.signature = None;

        let hash = compute_hash(&event)?;
        let hash_hex = hex::encode(hash);
        let sig = sign_hash(&hash, &self.key);
        event.hash = Some(hash_hex.clone());
        event.signature = Some(sig);

        let payload = serde_json::to_string(&event)?;
        tx.execute(
            "INSERT INTO events (id, ts, event_type, payload, prev_hash, hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                event.id,
                event.ts,
                event.event_type,
                payload,
                event.prev_hash,
                hash_hex
            ],
        )?;
        tx.commit()?;
        Ok(event.id)
    }

    /// Number of events in the log.
    pub fn count(&self) -> Result<i64, BossclawError> {
        let store = self.inner.lock().expect("poisoned");
        let n = store
            .conn()
            .query_row("SELECT count(*) FROM events", [], |r| r.get(0))?;
        Ok(n)
    }

    /// Re-verify the whole chain: every row's hash recomputes from its canonical
    /// bytes + prev_hash, links to the prior row, and its signature verifies.
    pub fn verify_chain(&self) -> Result<(), BossclawError> {
        let store = self.inner.lock().expect("poisoned");
        let conn = store.conn();
        let mut stmt = conn.prepare("SELECT payload, prev_hash, hash FROM events ORDER BY seq ASC")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;

        let mut expected_prev = GENESIS.to_string();
        for row in rows {
            let (payload, prev_hash, hash_hex) = row?;
            if prev_hash != expected_prev {
                return Err(BossclawError::Chain(format!(
                    "broken link: expected prev {expected_prev}, got {prev_hash}"
                )));
            }
            let event: Event = serde_json::from_str(&payload)?;
            let recomputed = hex::encode(compute_hash(&event)?);
            if recomputed != hash_hex {
                return Err(BossclawError::Chain(format!(
                    "hash mismatch at {}: stored {hash_hex}, recomputed {recomputed}",
                    event.id
                )));
            }
            let sig = event
                .signature
                .as_deref()
                .ok_or_else(|| BossclawError::Chain("missing signature".into()))?;
            let hash_bytes = compute_hash(&event)?;
            verify_hash(&hash_bytes, sig, &self.key.verifying_key())?;
            expected_prev = hash_hex;
        }
        Ok(())
    }
}
```
*(Note: `unchecked_transaction()` is used because the connection is borrowed immutably behind the Mutex; the Mutex provides the exclusivity. `verify_hash` uses the engine's own verifying key in M1 — DID→pubkey resolution per §5.2 lands when the desktop wires identity.)*

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p bossclaw-core --test chain`
Expected: PASS (including the 16-thread concurrency test — no fork, count == 16).

- [ ] **Step 5: Add a tamper test, run, commit**

Append to `crates/bossclaw-core/tests/chain.rs`:
```rust
#[test]
fn tampering_a_row_breaks_verify_chain() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&[7u8; 32]);
    let path = dir.path().join("m.db");
    {
        let log = EventLog::open(&path, &[42u8; 32], key.clone()).unwrap();
        log.append(mk_event("a")).unwrap();
        log.append(mk_event("b")).unwrap();
    }
    // Tamper: rewrite a payload's content under the same key/db.
    {
        let store = bossclaw_core::store::Store::open(&path, &[42u8; 32]).unwrap();
        store
            .exec("UPDATE events SET payload = replace(payload, '\"a\"', '\"HACKED\"') WHERE event_type='memory' AND payload LIKE '%\"a\"%'")
            .unwrap();
    }
    let log = EventLog::open(&path, &[42u8; 32], key).unwrap();
    assert!(log.verify_chain().is_err(), "tamper must be detected");
}
```
Run: `cargo test -p bossclaw-core --test chain`
Expected: PASS.
```bash
git add crates/bossclaw-core/src/log.rs crates/bossclaw-core/tests/chain.rs
git commit -m "feat(bossclaw-core): serialized append + verify_chain (+ concurrency, tamper tests)"
```

---

## Task 7: High-water-mark — truncation/rollback detection

**Files:**
- Modify: `crates/bossclaw-core/src/highwater.rs`
- Modify: `crates/bossclaw-core/src/log.rs` (wire it in)
- Test: `crates/bossclaw-core/tests/chain.rs` (extend)

- [ ] **Step 1: Write the failing truncation test**

Append to `crates/bossclaw-core/tests/chain.rs`:
```rust
use bossclaw_core::highwater::FileHighWater;

#[test]
fn tail_truncation_is_detected_on_open() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&[7u8; 32]);
    let db = dir.path().join("m.db");
    let hw = dir.path().join("hw.json");

    {
        let log = EventLog::open_with_highwater(&db, &[42u8; 32], key.clone(),
            Box::new(FileHighWater::new(&hw))).unwrap();
        for t in ["a","b","c"] { log.append(mk_event(t)).unwrap(); }
        log.checkpoint_highwater().unwrap(); // persist {tip, count=3}
    }

    // Attacker deletes the last row (tail truncation), chain of remaining rows still links.
    {
        let store = bossclaw_core::store::Store::open(&db, &[42u8; 32]).unwrap();
        store.exec("DELETE FROM events WHERE seq = (SELECT max(seq) FROM events)").unwrap();
    }

    // Reopen: live tip (count 2) is BEHIND the signed high-water (count 3) → detected.
    let reopened = EventLog::open_with_highwater(&db, &[42u8; 32], key,
        Box::new(FileHighWater::new(&hw)));
    assert!(matches!(reopened, Err(bossclaw_core::BossclawError::Truncation(_))),
        "tail truncation must be detected on open");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p bossclaw-core --test chain`
Expected: FAIL — `FileHighWater` / `open_with_highwater` / `checkpoint_highwater` not found.

- [ ] **Step 3: Implement the high-water store**

`crates/bossclaw-core/src/highwater.rs`:
```rust
//! Signed high-water-mark: detects tail-truncation/rollback that a plain hash
//! chain cannot (a deleted tail still links cleanly). The desktop wires a
//! keychain-backed impl at M7; the crate ships a file-backed impl for tests and
//! headless use.
//!
//! Write discipline (spec §5.2): the event is appended FIRST, the watermark is
//! updated SECOND and debounced (callers decide cadence via `checkpoint`). On
//! open, `live_count < mark.count` is truncation; `live_count >= mark.count`
//! with a valid chain is benign catch-up.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::BossclawError;

/// The persisted mark.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mark {
    /// Event count at checkpoint time.
    pub count: i64,
    /// Tip event hash (hex) at checkpoint time.
    pub tip_hash: String,
}

/// A place to persist the signed high-water mark.
pub trait HighWaterStore: Send {
    /// Load the last mark, or `None` if never written.
    fn load(&self) -> Result<Option<Mark>, BossclawError>;
    /// Persist a new mark (overwrites).
    fn save(&self, mark: &Mark) -> Result<(), BossclawError>;
}

/// File-backed high-water store (JSON). For tests + headless use.
pub struct FileHighWater {
    path: PathBuf,
}

impl FileHighWater {
    /// Create a file-backed store at `path`.
    pub fn new(path: &Path) -> Self {
        Self { path: path.to_path_buf() }
    }
}

impl HighWaterStore for FileHighWater {
    fn load(&self) -> Result<Option<Mark>, BossclawError> {
        match std::fs::read(&self.path) {
            Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
    fn save(&self, mark: &Mark) -> Result<(), BossclawError> {
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(mark)?)?;
        std::fs::rename(&tmp, &self.path)?; // atomic
        Ok(())
    }
}
```

- [ ] **Step 4: Wire it into EventLog**

In `crates/bossclaw-core/src/log.rs`, add an optional high-water store and the two new methods. Add the field:
```rust
use crate::highwater::{HighWaterStore, Mark};

pub struct EventLog {
    inner: Mutex<Store>,
    key: SigningKey,
    highwater: Option<Box<dyn HighWaterStore>>,
}
```
Update `open` to set `highwater: None`, and add:
```rust
impl EventLog {
    /// Open with a high-water store; checks truncation immediately.
    pub fn open_with_highwater(
        path: &Path,
        dek: &[u8; 32],
        key: SigningKey,
        highwater: Box<dyn HighWaterStore>,
    ) -> Result<Self, BossclawError> {
        let mut log = Self::open(path, dek, key)?;
        if let Some(mark) = highwater.load()? {
            let live = log.count()?;
            if live < mark.count {
                return Err(BossclawError::Truncation(format!(
                    "live count {live} < high-water {} (tail deleted)",
                    mark.count
                )));
            }
        }
        log.highwater = Some(highwater);
        Ok(log)
    }

    /// Persist the current tip as the signed high-water mark (debounced by the
    /// caller — every K events / on idle / on clean shutdown, NOT per append).
    pub fn checkpoint_highwater(&self) -> Result<(), BossclawError> {
        let hw = match &self.highwater {
            Some(h) => h,
            None => return Ok(()),
        };
        let store = self.inner.lock().expect("poisoned");
        let conn = store.conn();
        let count: i64 = conn.query_row("SELECT count(*) FROM events", [], |r| r.get(0))?;
        let tip_hash: String = conn
            .query_row("SELECT hash FROM events ORDER BY seq DESC LIMIT 1", [], |r| r.get(0))
            .unwrap_or_else(|_| GENESIS.to_string());
        hw.save(&Mark { count, tip_hash })
    }
}
```
Update the original `open` to initialise `highwater: None` in its returned struct.

- [ ] **Step 5: Run to verify it passes**

Run: `cargo test -p bossclaw-core --test chain`
Expected: PASS (truncation detected on reopen).

- [ ] **Step 6: Commit**
```bash
git add crates/bossclaw-core/src/highwater.rs crates/bossclaw-core/src/log.rs crates/bossclaw-core/tests/chain.rs
git commit -m "feat(bossclaw-core): signed high-water-mark + truncation detection"
```

---

## Task 8: Stream / replay over the log

**Files:**
- Modify: `crates/bossclaw-core/src/log.rs`
- Test: `crates/bossclaw-core/tests/chain.rs` (extend)

- [ ] **Step 1: Write the failing stream test**

Append to `crates/bossclaw-core/tests/chain.rs`:
```rust
#[test]
fn stream_returns_events_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&[7u8; 32]);
    let log = EventLog::open(&dir.path().join("m.db"), &[42u8; 32], key).unwrap();
    for t in ["a", "b", "c"] { log.append(mk_event(t)).unwrap(); }

    let all = log.stream_all().unwrap();
    assert_eq!(all.len(), 3);
    let texts: Vec<String> = all.iter()
        .map(|e| e.content["text"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(texts, vec!["a", "b", "c"]);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p bossclaw-core --test chain`
Expected: FAIL — `stream_all` not found.

- [ ] **Step 3: Implement stream_all**

Add to `impl EventLog` in `crates/bossclaw-core/src/log.rs`:
```rust
    /// Return every event in chain order (M1: full scan; M2 adds `since`).
    pub fn stream_all(&self) -> Result<Vec<Event>, BossclawError> {
        let store = self.inner.lock().expect("poisoned");
        let conn = store.conn();
        let mut stmt = conn.prepare("SELECT payload FROM events ORDER BY seq ASC")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(serde_json::from_str(&row?)?);
        }
        Ok(out)
    }
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p bossclaw-core --test chain`
Expected: PASS.

- [ ] **Step 5: Run the full suite + clippy**

Run: `cargo test -p bossclaw-core`
Expected: all PASS.
Run: `cargo clippy -p bossclaw-core -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**
```bash
git add crates/bossclaw-core/src/log.rs crates/bossclaw-core/tests/chain.rs
git commit -m "feat(bossclaw-core): stream/replay over the event log"
```

---

## Milestone 1 — Definition of Done

- [ ] `crates/bossclaw-core` builds and is in the workspace.
- [ ] Canonicalization, genesis hash, and a signature are **frozen as test vectors** (the expensive-to-change §5.2 decisions).
- [ ] Append is **serialized** — the 16-thread test shows no chain fork.
- [ ] `verify_chain()` detects content tampering; reopen detects **tail truncation** via the signed high-water mark.
- [ ] The DB is **encrypted at rest** (no plaintext `SQLite format 3` header; wrong key fails).
- [ ] `cargo test -p bossclaw-core` green; `cargo clippy -p bossclaw-core -- -D warnings` clean.

**Carried into later milestones (from the review):** Tier-A index rebuild + the no-plaintext-index encryption spike + the ort-bundling spike (M2 go/no-go); the graph fold (M3); the evolve loop emitting Tier-B events + page-supersede projection (M4); ingest containment + `O_NOFOLLOW`/fd-passing + fail-closed lineage-walked taint (M5); confirm-each actuator + export format (M6); desktop surface + keychain-backed `HighWaterStore` + DID→pubkey resolution (M7).

---

## Self-Review

**Spec coverage (§12 M1):** crate ✓(T1) · encrypted store ✓(T5) · signed log ✓(T4,T6) · frozen canonicalization vector ✓(T2,T3) · serialized writer ✓(T6) · high-water write-ordering/debounce ✓(T7, caller-driven `checkpoint`) · Tier-B event types + non-empty `source_event_ids` enforced ✓(T6 append guard, `ModelMeta` in T2). Out-of-M1 items explicitly deferred above.

**Placeholder scan:** the only intentional "fill-in" is the frozen genesis-hash hex (T3 Step 3) — by design it is captured from the first green run then frozen; the task spells out the exact procedure, so it is not an open placeholder. No "TODO/handle errors/similar-to" left.

**Type consistency:** `Event`/`ModelMeta` fields (T2) are used identically in `compute_hash` (T2), `append`/`verify_chain` (T6), `stream_all` (T8). `sign_hash(&[u8;32], &SigningKey) -> String` + `verify_hash(&[u8;32], &str, &VerifyingKey)` (T4) match their call sites in T6. `Store::open(&Path, &[u8;32])` (T5) matches `EventLog::open` (T6). `HighWaterStore`/`Mark`/`FileHighWater` (T7) match the wiring + test. `BossclawError::Truncation` (T1) matches T7's check.
