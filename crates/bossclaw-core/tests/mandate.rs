//! M6c mandate primitive — hermetic tests (Layer 1a).
//!
//! Covers the `mandates` projection + `add_mandate`/`revoke_mandate`/`active_mandates`
//! and the two LOAD-BEARING grant-time guards:
//! - **Finding A (self-loop):** the target MUST be outside every active read-grant root
//!   (so the engine's own confirmed write can never be re-ingested as a source).
//! - **Finding D (recipe cap):** an over-`MAX_RECIPE_LEN` recipe is rejected at grant,
//!   never silently truncated.
//!
//! Mirrors the `tests/reconcile.rs` harness posture: a tempdir-backed `EventLog`
//! opened via the public `EventLog::open`, with read/write grants laid down through
//! the public `add_grant`/`add_write_grant` APIs.
#![cfg(unix)]

use bossclaw_core::{EventLog, MAX_RECIPE_LEN};
use ed25519_dalek::SigningKey;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// Deterministic data-encryption key for the hermetic SQLCipher store.
const DEK: [u8; 32] = [42u8; 32];
/// Deterministic ed25519 seed so signatures are reproducible across runs.
const KEY_BYTES: [u8; 32] = [7u8; 32];

/// Open a fresh `EventLog` on a tempdir-backed home. Returns `(log, home_tempdir)`;
/// the `TempDir` is returned so the caller keeps it alive for the test's duration
/// (dropping it deletes the store + any created dirs).
fn setup() -> (EventLog, TempDir) {
    let home = tempfile::tempdir().expect("create home tempdir");
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let log = EventLog::open(&home.path().join("m.db"), &DEK, key).expect("open EventLog");
    (log, home)
}

/// Read-grant `dir` (sources are watched/ingested under active READ grants).
fn read_grant(log: &EventLog, dir: &Path) {
    log.add_grant(dir).expect("read-grant dir");
}

/// Write-grant `dir` (a mandate target must live under an active WRITE grant).
fn write_grant(log: &EventLog, dir: &Path) {
    log.add_write_grant(dir).expect("write-grant dir");
}

/// Make a `src` dir (read-granted) and a SEPARATE `out` dir (write-granted), with
/// `out` OUTSIDE the read root so the Finding-A self-loop guard is satisfied.
fn scoped_dirs(tmp: &TempDir) -> (PathBuf, PathBuf) {
    let log_dir = tmp.path();
    let src = log_dir.join("src");
    std::fs::create_dir_all(&src).expect("create src dir");
    let out = log_dir.join("out");
    std::fs::create_dir_all(&out).expect("create out dir");
    (src, out)
}

// Grant a valid mandate (target write-granted, OUTSIDE any read root) → active_mandates lists it.
#[test]
fn mandate_grant_and_active() {
    let (log, _tmp) = setup();
    let (src, out) = scoped_dirs(&_tmp);
    read_grant(&log, &src); // sources watched here
    write_grant(&log, &out); // target writable here
    let target = out.join("index.md");
    let id = log.add_mandate(&target, &src, "an index of titles").unwrap();
    let ms = log.active_mandates().unwrap();
    assert_eq!(ms.len(), 1);
    assert_eq!(ms[0].mandate_grant_id, id);
    assert!(!ms[0].revoked);
}

// Revoke is sticky → active_mandates drops it.
#[test]
fn mandate_revoke_sticky() {
    let (log, _tmp) = setup();
    let (src, out) = scoped_dirs(&_tmp); // src(read) + out(write)
    read_grant(&log, &src);
    write_grant(&log, &out);
    let id = log.add_mandate(&out.join("i.md"), &src, "recipe").unwrap();
    log.revoke_mandate(&id).unwrap();
    assert!(log.active_mandates().unwrap().is_empty());
}

// FINDING A: target UNDER a read-grant root → add_mandate rejects (self-loop guard).
#[test]
fn mandate_target_under_read_root_rejected() {
    let (log, _tmp) = setup();
    let dir = _tmp.path().join("notes");
    std::fs::create_dir_all(&dir).unwrap();
    read_grant(&log, &dir);
    write_grant(&log, &dir); // both granted on the same dir
    let err = log.add_mandate(&dir.join("index.md"), &dir, "self-index");
    assert!(err.is_err(), "target inside a watched read root must be rejected");
}

// UX guard: target NOT under any write-grant → reject.
#[test]
fn mandate_target_not_write_granted_rejected() {
    let (log, _tmp) = setup();
    let (src, _out) = scoped_dirs(&_tmp);
    read_grant(&log, &src);
    let ungranted = _tmp.path().join("nowhere").join("x.md");
    std::fs::create_dir_all(ungranted.parent().unwrap()).unwrap();
    assert!(log.add_mandate(&ungranted, &src, "r").is_err());
}

// FINDING D: recipe over MAX_RECIPE_LEN → reject at grant (never silently truncated).
#[test]
fn mandate_recipe_over_cap_rejected() {
    let (log, _tmp) = setup();
    let (src, out) = scoped_dirs(&_tmp);
    read_grant(&log, &src);
    write_grant(&log, &out);
    let big = "x".repeat(MAX_RECIPE_LEN + 1);
    assert!(log.add_mandate(&out.join("i.md"), &src, &big).is_err());
}

// Segment-aware Finding-A guard: a read root `/a/b` must NOT make a target under
// the SIBLING `/a/bc` look like it's inside the read root (raw-string-prefix bug).
#[test]
fn mandate_segment_aware_read_root_sibling_allowed() {
    let (log, _tmp) = setup();
    let read_root = _tmp.path().join("a").join("b");
    std::fs::create_dir_all(&read_root).unwrap();
    let sibling = _tmp.path().join("a").join("bc"); // shares the string prefix ".../a/b"
    std::fs::create_dir_all(&sibling).unwrap();
    read_grant(&log, &read_root);
    write_grant(&log, &sibling);
    // Target lives under `/a/bc`, which is NOT under read root `/a/b` segment-wise.
    let id = log
        .add_mandate(&sibling.join("index.md"), &read_root, "sibling index")
        .expect("sibling target must be accepted (segment-aware, not string-prefix)");
    let ms = log.active_mandates().unwrap();
    assert_eq!(ms.len(), 1);
    assert_eq!(ms[0].mandate_grant_id, id);
}

// FINDING A (leaf-tight): a symlink AT the target leaf that points INTO a read root
// must be rejected. `canonicalize_target_or_parent` joins the raw leaf NOFOLLOW, so the
// canonical-target form alone would miss this — `add_mandate` resolves an existing leaf
// (`std::fs::canonicalize`, which follows the symlink) before the read-root scan.
#[test]
fn mandate_target_leaf_symlink_into_read_root_rejected() {
    let (log, _tmp) = setup();
    // A real file living UNDER the read root (the would-be re-ingested output).
    let read_root = _tmp.path().join("notes");
    std::fs::create_dir_all(&read_root).unwrap();
    let secret = read_root.join("secret.md");
    std::fs::write(&secret, b"inside the read root\n").unwrap();
    read_grant(&log, &read_root);
    // A write-granted dir OUTSIDE the read root, holding a symlink whose leaf points
    // back INTO the read root. The leaf (`out/link.md`) is under the write grant (so the
    // UX write-grant guard passes), but it RESOLVES to `notes/secret.md`.
    let out = _tmp.path().join("out");
    std::fs::create_dir_all(&out).unwrap();
    write_grant(&log, &out);
    let link = out.join("link.md");
    std::os::unix::fs::symlink(&secret, &link).unwrap();
    // Targeting the symlink must be rejected: its resolved leaf is inside the read root.
    assert!(
        log.add_mandate(&link, &read_root, "leaf symlink into read root").is_err(),
        "a leaf symlink resolving into a read root must be rejected"
    );
}
