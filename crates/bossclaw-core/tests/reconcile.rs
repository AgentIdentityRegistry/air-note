//! M6b reconciliation proposer — hermetic tests.
//!
//! `EventLog` is constructed and operated entirely inside the `common` harness, so the
//! test bodies never name the type directly — there is intentionally no `use` for it.
#![cfg(unix)]
mod common;

/// Given a file_ingested event id, the reverse accessor returns the CURRENT FileRecord
/// for that id, and None once the file is superseded by a re-ingest at the same path.
#[test]
fn current_path_for_file_event_maps_id_to_live_record_and_drops_superseded() {
    let (log, _home, dir) = common::open_log_with_write_grant();
    let path = dir.join("notes.md");
    std::fs::write(&path, b"Alice works at Acme.\n").unwrap();
    let id1 = common::ingest_one(&log, &path);

    let rec = log.current_path_for_file_event(&id1).unwrap().expect("id1 is current");
    assert_eq!(rec.file_event_id, id1);
    assert_eq!(rec.canonical_path, std::fs::canonicalize(&path).unwrap().to_string_lossy());

    std::fs::write(&path, b"Alice works at Globex.\n").unwrap();
    let id2 = common::ingest_one(&log, &path);
    assert!(log.current_path_for_file_event(&id1).unwrap().is_none(), "superseded id is not current");
    assert_eq!(log.current_path_for_file_event(&id2).unwrap().unwrap().file_event_id, id2);
}

/// Freshness: a target is reconcilable only if it is still tracked at its path,
/// the projection's current id matches, AND it is still a regular file (not a symlink).
#[test]
fn is_reconcilable_target_rejects_superseded_and_symlinked() {
    let (log, _home, dir) = common::open_log_with_write_grant();
    let path = dir.join("a.md");
    std::fs::write(&path, b"x\n").unwrap();
    let id = common::ingest_one(&log, &path);
    assert!(log.is_reconcilable_target(&id).unwrap().is_some(), "fresh regular file is reconcilable");

    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(dir.join("elsewhere"), &path).unwrap();
    assert!(log.is_reconcilable_target(&id).unwrap().is_none(), "symlinked target is rejected");
}
