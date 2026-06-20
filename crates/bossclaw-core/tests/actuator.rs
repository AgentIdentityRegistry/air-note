//! M6a "Safe Hands" actuator tests (T1): the write-grant authority and the
//! `is_write_allowed` path-segment-descent predicate.
//!
//! Harness preamble is copied from `tests/extraction.rs` (lines ~23-32) so this
//! binary reuses the hermetic `EventLog` factory without cross-binary imports
//! (test binaries cannot import each other's helpers).

use bossclaw_core::log::EventLog;
use ed25519_dalek::SigningKey;

// ── Constants (copied from extraction.rs) ─────────────────────────────────────

const DEK: [u8; 32] = [42u8; 32];
const KEY_BYTES: [u8; 32] = [7u8; 32];

// ── Log factory (copied from extraction.rs) ───────────────────────────────────

fn open_log(dir: &std::path::Path) -> EventLog {
    let key = SigningKey::from_bytes(&KEY_BYTES);
    EventLog::open(&dir.join("m.db"), &DEK, key).unwrap()
}

// ── Write-grant authority: grant / membership / revoke / re-grant ─────────────

#[test]
fn write_grant_authorizes_paths_under_root() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    log.add_write_grant(dir.path()).unwrap();

    // The granted root itself and a file under it are both writable.
    assert!(
        log.is_write_allowed(dir.path()).unwrap(),
        "the granted root is itself a member"
    );
    assert!(
        log.is_write_allowed(&dir.path().join("f.txt")).unwrap(),
        "an existing-parent file under the root is writable"
    );
}

#[test]
fn write_grant_denies_paths_outside_root() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    log.add_write_grant(dir.path()).unwrap();

    assert!(
        !log.is_write_allowed(outside.path()).unwrap(),
        "a path under a different root is not writable"
    );
}

#[test]
fn write_revoke_denies_then_regrant_reallows() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("f.txt");
    let log = open_log(dir.path());

    log.add_write_grant(dir.path()).unwrap();
    assert!(log.is_write_allowed(&target).unwrap(), "granted → allowed");

    log.revoke_write_grant(dir.path()).unwrap();
    assert!(
        !log.is_write_allowed(&target).unwrap(),
        "revoked → denied (last-writer-wins)"
    );

    log.add_write_grant(dir.path()).unwrap();
    assert!(
        log.is_write_allowed(&target).unwrap(),
        "re-granted → allowed again (last-writer-wins)"
    );
}

// ── Read ≠ write: a READ grant must NEVER authorize a write ───────────────────

#[test]
fn read_grant_does_not_authorize_write() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());

    // A READ grant (M5a) on the dir — and NO write grant.
    log.add_grant(dir.path()).unwrap();

    assert!(
        !log.is_write_allowed(&dir.path().join("f")).unwrap(),
        "a read grant must not authorize a write"
    );
    assert!(
        log.write_grants().unwrap().is_empty(),
        "a read `grant` event must not populate the write_grants projection"
    );
}

// ── Create (target absent): canonicalize the PARENT, test the parent ──────────

#[test]
fn write_allowed_for_absent_create_target_via_parent() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    log.add_write_grant(dir.path()).unwrap();

    let absent = dir.path().join("does-not-exist.txt");
    assert!(
        !absent.exists(),
        "precondition: the create target must not exist yet"
    );
    assert!(
        log.is_write_allowed(&absent).unwrap(),
        "a not-yet-existing target under the root is writable (parent canonicalization)"
    );
}

// ── Path-segment safety: a sibling sharing the root's string prefix is OUT ─────

#[test]
fn write_grant_is_path_segment_aware_not_string_prefix() {
    let dir = tempfile::tempdir().unwrap();
    // A sibling directory whose name shares the granted root's full string
    // prefix: `/tmp/.../X` granted, `/tmp/.../X-evil` must NOT be under it.
    let sibling = std::path::PathBuf::from(format!("{}-evil", dir.path().display()));
    std::fs::create_dir(&sibling).unwrap();

    let log = open_log(dir.path());
    log.add_write_grant(dir.path()).unwrap();

    assert!(
        !log.is_write_allowed(&sibling).unwrap(),
        "a sibling sharing the string prefix is NOT a path-segment descendant"
    );
    assert!(
        !log.is_write_allowed(&sibling.join("f.txt")).unwrap(),
        "nor is a file inside that prefix-sharing sibling"
    );

    std::fs::remove_dir_all(&sibling).ok();
}
