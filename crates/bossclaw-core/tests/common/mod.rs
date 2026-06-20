//! Shared hermetic test harness for the M6b reconciliation-proposer tests.
//!
//! Minimal by design — only what Task 1 needs. Later M6b tasks extend this with
//! their own helpers; do not duplicate the log factory or ingest entrypoint.
#![cfg(unix)]
#![allow(dead_code)] // helpers are consumed test-by-test; silence until all are used.

use bossclaw_core::embed::MockEmbedder;
use bossclaw_core::event::Event;
use bossclaw_core::ingest::ParserRouter;
use bossclaw_core::EventLog;
use ed25519_dalek::SigningKey;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
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

/// Monotonic suffix so every `seed_external_event` writes to a DISTINCT path —
/// `ingest_one` keys on canonical path, so two calls with identical TEXT must still
/// land on different files to yield two distinct, independent `file_ingested` ids
/// (the anti-laundering tests rely on the asserting file ≠ the correcting file).
static SEED_FILE_SEQ: AtomicU64 = AtomicU64::new(0);

/// Seed a tracked EXTERNAL source: write `text` into a uniquely-named `.md` under the
/// granted files dir and ingest it, returning its `file_ingested` event id. That id
/// `is_external`, so a Tier-B event citing it is taint-stamped at the append chokepoint.
/// Reuses [`ingest_one`] — the simplest hermetic external-event factory.
pub fn seed_external_event(log: &EventLog, text: &str) -> String {
    // The files dir is the READ+WRITE-granted sibling created by the log factory; ingest
    // discovers files anywhere under that active grant root. Recover it from the public
    // write-grants projection (the harness grants exactly one root).
    let root = log
        .write_grants()
        .expect("read write grants")
        .into_iter()
        .find(|g| !g.revoked)
        .map(|g| g.canonical_root)
        .expect("an active write-granted files dir exists");
    let n = SEED_FILE_SEQ.fetch_add(1, Ordering::Relaxed);
    let path = PathBuf::from(root).join(format!("seed_{n}.md"));
    std::fs::write(&path, text.as_bytes()).expect("write seed external file");
    ingest_one(log, &path)
}

/// Seed a machine `link` (edge) with the given `source_event_ids`, returning the link
/// EVENT id — which `fold_edges` adopts verbatim as the `edge_id`. Wraps the real
/// [`EventLog::link_machine`] API (non-manual producer, fixed confidence); `sources`
/// becomes its `source_event_ids` so the retired edge carries the asserting file's lineage.
pub fn seed_edge_with_sources(
    log: &EventLog,
    src: &str,
    relation: &str,
    dst: &str,
    sources: &[String],
) -> String {
    log.link_machine(src, relation, dst, 0.9, "m6b-test-linker", sources)
        .expect("seed machine link")
}

/// Seed a plain `memory` event and return its id. Mirrors the `mk_memory` Event shape
/// used across the graph tests, appended via the public [`EventLog::append`].
pub fn seed_memory(log: &EventLog, text: &str) -> String {
    log.append(Event {
        id: String::new(),
        ts: String::new(),
        valid_time: None,
        event_type: "memory".to_string(),
        content: serde_json::json!({ "text": text }),
        model_meta: None,
        prev_hash: String::new(),
        hash: None,
        signed_by_did: "did:wba:AIR-TEST".to_string(),
        signature: None,
    })
    .expect("seed memory event")
}

/// Hex SHA-256 of `bytes`, replicating EXACTLY the engine's `file_written.content_hash`
/// computation (`hex::encode(Sha256::digest(..))` in `log.rs`). Tests use this so a hash
/// they hand to `put_proposal_bytes` / `append_write_proposal` equals what the signed
/// event records and what `get_proposal_bytes_checked` recomputes — no second hasher.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

/// Like [`open_log_with_write_grant`], and ALSO write + ingest a tracked `n.md` under the
/// files dir so a real EXTERNAL reconciliation target exists. The `n.md` `file_ingested`
/// id is recoverable via the public `current_files()` projection (used by the proposal/
/// rejection helpers below to supply a non-empty, tracked-file lineage). Returns the dir.
pub fn open_write_grant_and_external_target() -> (EventLog, TempDir, PathBuf) {
    let (log, home, dir) = open_log_with_write_grant();
    let path = dir.join("n.md");
    std::fs::write(&path, b"Alice works at Acme.\n").expect("write n.md target");
    ingest_one(&log, &path);
    (log, home, dir)
}

/// The `file_ingested` event id for an already-ingested target at `canonical`. Reads the
/// public `current_files()` projection — the same lookup `ingest_one` uses — so proposal
/// lineage cites a real, tracked file (satisfying Tier-B + taint-stamp invariants).
fn tracked_file_id(log: &EventLog, canonical: &str) -> String {
    log.current_files()
        .expect("read current_files")
        .into_iter()
        .find(|rec| rec.canonical_path == canonical)
        .map(|rec| rec.file_event_id)
        .unwrap_or_else(|| panic!("no current file_ingested event for {canonical}"))
}

/// Append a minimal OPEN `write_proposal` for `(canonical, key)` and return its id. The
/// lineage cites the tracked target file at `canonical` (non-empty + a real file, so the
/// Tier-B taint-stamp invariants hold). Mirrors the builder call shape the proposer uses.
pub fn append_minimal_proposal(log: &EventLog, canonical: &str, key: &Value) -> String {
    let file_id = tracked_file_id(log, canonical);
    log.append_write_proposal(
        canonical,
        "edit",
        "deadbeef",
        0,
        "rationale",
        key,
        &serde_json::json!({}),
        std::slice::from_ref(&file_id),
    )
    .expect("append minimal write_proposal")
}

/// Append an engine-side `write_rejected` for `(canonical, key)` and return its id. Cites
/// the tracked target file as lineage; this independently suppresses re-attempts but is NOT
/// a proposal and NEVER resolves one.
pub fn append_rejected(log: &EventLog, canonical: &str, key: &Value, reason: &str) -> String {
    let file_id = tracked_file_id(log, canonical);
    log.append_write_rejected(Some(canonical), reason, key, std::slice::from_ref(&file_id))
        .expect("append write_rejected")
}
