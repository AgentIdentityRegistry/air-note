//! Shared hermetic test harness for the M6b reconciliation-proposer tests.
//!
//! Minimal by design — only what Task 1 needs. Later M6b tasks extend this with
//! their own helpers; do not duplicate the log factory or ingest entrypoint.
#![cfg(unix)]
#![allow(dead_code)] // helpers are consumed test-by-test; silence until all are used.

use bossclaw_core::embed::MockEmbedder;
use bossclaw_core::ingest::ParserRouter;
use bossclaw_core::EventLog;
use ed25519_dalek::SigningKey;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// Deterministic data-encryption key for the hermetic SQLCipher store.
const DEK: [u8; 32] = [42u8; 32];
/// Deterministic ed25519 seed so signatures are reproducible across runs.
const KEY_BYTES: [u8; 32] = [7u8; 32];
/// MockEmbedder dimension — any fixed width works for these path-only assertions.
const EMBED_DIM: usize = 64;

/// Open a fresh `EventLog` on a tempdir-backed home, create a sibling files dir, and
/// grant BOTH read and write on that files dir:
/// - `add_grant` (READ) so `ingest_all` discovers the files (it iterates active READ
///   grants), and
/// - `add_write_grant` (WRITE) so the dir is a valid reconciliation write target.
///
/// Returns `(log, home_tempdir, files_dir)`. The `TempDir` is returned so the caller
/// keeps it alive for the test's duration (dropping it deletes the store + files).
pub fn open_log_with_write_grant() -> (EventLog, TempDir, PathBuf) {
    let home = tempfile::tempdir().expect("create home tempdir");
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let log = EventLog::open(&home.path().join("m.db"), &DEK, key).expect("open EventLog");

    let dir = home.path().join("files");
    std::fs::create_dir_all(&dir).expect("create files dir");
    log.add_grant(&dir).expect("read-grant files dir");
    log.add_write_grant(&dir).expect("write-grant files dir");

    (log, home, dir)
}

/// Ingest one already-written file under a granted dir and return its `file_ingested`
/// event id. Uses the native UTF-8 parser path (plain `.md`/`.txt`), so it is fully
/// hermetic — no sandboxed markitdown subprocess. Resolves the id from the public
/// `current_files()` projection by matching the file's canonical path.
pub fn ingest_one(log: &EventLog, path: &Path) -> String {
    let embedder = MockEmbedder::new(EMBED_DIM);
    log.ingest_all(&ParserRouter::native_only(), &embedder)
        .expect("ingest_all");

    let canonical = std::fs::canonicalize(path)
        .expect("canonicalize ingested path")
        .to_string_lossy()
        .to_string();
    log.current_files()
        .expect("read current_files")
        .into_iter()
        .find(|rec| rec.canonical_path == canonical)
        .map(|rec| rec.file_event_id)
        .unwrap_or_else(|| panic!("no current file_ingested event for {canonical}"))
}
