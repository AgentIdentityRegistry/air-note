//! Security test: no plaintext leaks to disk from the SQLCipher store.
//!
//! FTS5 index shadow tables live INSIDE the SQLCipher database and are
//! therefore encrypted with the rest of the file.  `PRAGMA temp_store =
//! MEMORY` (set in `EventLog::open`) ensures that FTS5 index-merge temporaries
//! are kept in process memory and never spilled to disk.
//!
//! This test proves both properties empirically:
//!
//! 1. **No plaintext marker on disk** — after writing events whose text
//!    contains a unique canary token and rebuilding all indexes, every file in
//!    the temp directory is scanned for the canary bytes.  Zero occurrences
//!    must be found.
//!
//! 2. **Encrypted header** — the database file must NOT begin with the
//!    SQLite plaintext magic bytes `b"SQLite format 3\0"`.
//!
//! 3. **Correct key decrypts** — re-opening with the right DEK and running
//!    `keyword_search` returns the canary event.
//!
//! 4. **Wrong key fails** — re-opening with a wrong DEK is rejected by
//!    `EventLog::open` (SQLCipher returns an error on the first read).

use bossclaw_core::embed::MockEmbedder;
use bossclaw_core::event::Event;
use bossclaw_core::log::EventLog;
use ed25519_dalek::SigningKey;
use serde_json::json;
use std::fs;

/// Data-encryption key used for all tests in this file.
const DEK: [u8; 32] = [42u8; 32];
/// A different key — used to prove that wrong-key opens are rejected.
const WRONG_DEK: [u8; 32] = [99u8; 32];
/// Ed25519 signing key seed.
const KEY_BYTES: [u8; 32] = [7u8; 32];
/// Dimension for the MockEmbedder. Must be > 0.
const MID_DIM: usize = 64;

/// A token that is unlikely to appear in any SQLite/SQLCipher metadata,
/// FTS5 internals, or WAL headers by coincidence.
const CANARY: &str = "ZqPlaintextLeakCanary42_unmistakable";

fn mk_canary_event() -> Event {
    Event {
        id: String::new(),
        ts: String::new(),
        valid_time: None,
        event_type: "memory".to_string(),
        content: json!({ "text": CANARY }),
        model_meta: None,
        prev_hash: String::new(),
        hash: None,
        signed_by_did: "did:wba:AIR-TEST".to_string(),
        signature: None,
    }
}

/// Scan every file in `dir` for `needle` bytes, returning the total count of
/// occurrences across all files.
fn scan_dir_for_bytes(dir: &std::path::Path, needle: &[u8]) -> usize {
    let mut total = 0usize;
    let entries = fs::read_dir(dir).expect("read_dir failed");
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            let data = fs::read(&path).unwrap_or_default();
            // Count non-overlapping occurrences of `needle`.
            let mut pos = 0;
            while pos + needle.len() <= data.len() {
                if data[pos..pos + needle.len()] == *needle {
                    total += 1;
                    pos += needle.len();
                } else {
                    pos += 1;
                }
            }
        }
    }
    total
}

#[test]
fn no_plaintext_canary_on_disk_and_encrypted_header() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("secure.db");

    // ── Write canary events + rebuild all indexes ─────────────────────────────
    {
        let key = SigningKey::from_bytes(&KEY_BYTES);
        let log = EventLog::open(&db_path, &DEK, key).unwrap();

        // Append several memory events; at least one contains the canary token.
        log.append(mk_canary_event()).unwrap();
        log.append(mk_canary_event()).unwrap();
        log.append(mk_canary_event()).unwrap();

        let embedder = MockEmbedder::new(MID_DIM);
        log.rederive_pending(&embedder).unwrap();
        // rebuild_indexes populates both the vector index and the FTS index.
        log.rebuild_indexes(&embedder).unwrap();
        // Drop `log` here — connection is closed, WAL checkpoint runs on drop.
    }

    // ── 1. No plaintext canary anywhere on disk ───────────────────────────────
    let canary_bytes = CANARY.as_bytes();
    let occurrences = scan_dir_for_bytes(dir.path(), canary_bytes);
    assert_eq!(
        occurrences, 0,
        "canary token must not appear in any on-disk file — \
         found {occurrences} occurrence(s) in {:?}",
        dir.path()
    );

    // ── 2. Database file header is NOT the SQLite plaintext magic ────────────
    let db_bytes = fs::read(&db_path).expect("db file not found");
    assert!(
        db_bytes.len() >= 16,
        "db file too short to contain a header"
    );
    let sqlite_magic = b"SQLite format 3\0";
    assert_ne!(
        &db_bytes[..16],
        sqlite_magic,
        "database file must not start with the plaintext SQLite magic bytes \
         (file is unencrypted)"
    );

    // ── 3. Correct key: re-open succeeds and canary is searchable ─────────────
    {
        let key = SigningKey::from_bytes(&KEY_BYTES);
        let log = EventLog::open(&db_path, &DEK, key).unwrap();
        let embedder = MockEmbedder::new(MID_DIM);
        // Rebuild the in-memory indexes (they are never persisted).
        log.rebuild_indexes(&embedder).unwrap();

        let hits = log.keyword_search(CANARY, 5).unwrap();
        assert!(
            !hits.is_empty(),
            "correct key must allow reading the canary event via keyword_search"
        );
    }

    // ── 4. Wrong key: open must be rejected ───────────────────────────────────
    {
        let key = SigningKey::from_bytes(&KEY_BYTES);
        let result = EventLog::open(&db_path, &WRONG_DEK, key);
        assert!(
            result.is_err(),
            "wrong DEK must cause EventLog::open to fail, but got Ok"
        );
    }
}
