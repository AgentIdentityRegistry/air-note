//! M6a "Safe Hands" actuator tests (T1): the write-grant authority and the
//! `is_write_allowed` path-segment-descent predicate.
//!
//! Harness preamble is copied from `tests/extraction.rs` (lines ~23-32) so this
//! binary reuses the hermetic `EventLog` factory without cross-binary imports
//! (test binaries cannot import each other's helpers).

use bossclaw_core::actuator::{Taint, WriteOp, WriteProposal};
use bossclaw_core::embed::MockEmbedder;
use bossclaw_core::event::Event;
use bossclaw_core::log::EventLog;
use ed25519_dalek::SigningKey;
use serde_json::json;

// ── Constants (copied from extraction.rs) ─────────────────────────────────────

const DEK: [u8; 32] = [42u8; 32];
const KEY_BYTES: [u8; 32] = [7u8; 32];

// ── Log factory (copied from extraction.rs) ───────────────────────────────────

fn open_log(dir: &std::path::Path) -> EventLog {
    let key = SigningKey::from_bytes(&KEY_BYTES);
    EventLog::open(&dir.join("m.db"), &DEK, key).unwrap()
}

// ── Harness helpers (copied from extraction.rs — no cross-binary imports) ──────
// The T2 taint tests need a real ingested file (an external `file_ingested`) and
// a clean seed memory; these mirror `tests/extraction.rs` lines ~36-74 exactly.

fn mk_memory(text: &str) -> Event {
    Event {
        id: String::new(),
        ts: String::new(),
        valid_time: None,
        event_type: "memory".to_string(),
        content: json!({ "text": text }),
        model_meta: None,
        prev_hash: String::new(),
        hash: None,
        signed_by_did: "did:wba:AIR-TEST".to_string(),
        signature: None,
    }
}

/// Append a memory and bring every derived structure up to date. Returns its id —
/// a CLEAN (non-external) source id for the taint tests.
fn seed_memory(log: &EventLog, embedder: &MockEmbedder, text: &str) -> String {
    let id = log.append(mk_memory(text)).unwrap();
    log.rederive_pending(embedder).unwrap();
    log.rebuild_indexes(embedder).unwrap();
    log.rebuild_graph().unwrap();
    log.rebuild_entity_index(embedder).unwrap();
    id
}

/// Write `text` to <dir>/g/<name>, grant it (READ), ingest, rebuild all derived
/// structures. After this the file is a tracked, EXTERNAL `file_ingested` — the
/// engine-anchor target for the cite-around test. Returns the file's canonical path.
fn ingest_file(
    log: &EventLog,
    emb: &MockEmbedder,
    dir: &std::path::Path,
    name: &str,
    text: &[u8],
) -> std::path::PathBuf {
    let folder = dir.join("g");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join(name), text).unwrap();
    log.add_grant(&folder).unwrap();
    log.ingest_all(&bossclaw_core::ingest::ParserRouter::native_only(), emb).unwrap();
    log.rederive_pending(emb).unwrap();
    log.rebuild_indexes(emb).unwrap();
    log.rebuild_graph().unwrap();
    log.rebuild_entity_index(emb).unwrap();
    // The walk admits no symlink/`..`, so grant_root.join(name) IS the canonical path.
    std::fs::canonicalize(folder.join(name)).unwrap()
}

/// A minimal proposal builder for the gate tests.
fn proposal(
    target: &std::path::Path,
    op: WriteOp,
    new_content: &[u8],
    sources: &[String],
) -> WriteProposal {
    WriteProposal {
        target: target.to_path_buf(),
        new_content: new_content.to_vec(),
        op,
        source_event_ids: sources.to_vec(),
        rationale: "test".to_string(),
    }
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

// ╔══════════════════════════════════════════════════════════════════════════╗
// ║  T2: propose_write — the PURE gate (engine-anchored taint + fail-closed)   ║
// ╚══════════════════════════════════════════════════════════════════════════╝

// ── L11 cite-around (REVERT-SENSITIVE) ────────────────────────────────────────
//
// A malicious caller edits a TRACKED, EXTERNAL ingested file but cites ONLY a
// clean memory id. The verdict MUST still be `Untrusted`, because step 4 (the
// engine anchor) derives taint from the TARGET itself — never the citation list.
//
// REVERT-SENSITIVITY: this test MUST fail if step 4 (the `current_file_for_path`
// engine anchor in `propose_write`) is removed, because then the gate would
// degrade to caller-cites-only and the clean citation would read `Clean`.
// (Verified by temporarily commenting the anchor — see the build report.)
#[test]
fn taint_engine_anchored_cite_around_is_untrusted() {
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(16);
    let log = open_log(dir.path());

    // F is an ingested file under a READ grant → an external `file_ingested`.
    let f_canonical = ingest_file(&log, &emb, dir.path(), "notes.md", b"ingested body");

    // A separate CLEAN source (a plain memory) the attacker will cite instead.
    let clean_id = seed_memory(&log, &emb, "totally benign memory");

    // Grant WRITE over F's directory so `allowed` is true and the only thing under
    // test is the taint verdict (not the grant gate).
    log.add_write_grant(&dir.path().join("g")).unwrap();

    // Edit F, citing ONLY the clean memory id (the cite-around attack).
    let gated = log
        .propose_write(proposal(&f_canonical, WriteOp::Edit, b"attacker payload", &[clean_id]))
        .unwrap();

    assert_eq!(
        gated.verdict.taint,
        Taint::Untrusted,
        "editing a tracked external file is Untrusted EVEN with a clean-only citation \
         (engine anchor, step 4) — if this reads Clean the anchor was removed"
    );
    assert!(
        gated.verdict.requires_loud_modal,
        "an Untrusted write must force the loud modal"
    );
    assert!(
        gated.verdict.reject_reason.is_none(),
        "the write is allowed-but-loud, not rejected: {:?}",
        gated.verdict.reject_reason
    );
    // The engine-anchored provenance (the file_ingested event) is surfaced AND
    // marked external, so the user can trace the influence.
    assert!(
        gated.verdict.provenance.iter().any(|p| p.is_external),
        "the engine-anchored external provenance must be surfaced"
    );
}

// ── L11 hardlink-alias (REVERT-SENSITIVE — T2 review Critical) ────────────────
//
// A hardlink is a second directory entry with a DIFFERENT name but the SAME inode.
// `canonicalize` does NOT collapse it to the tracked file's path, so a PATH-only
// anchor misses a write made through the alias. The IDENTITY anchor `(dev,ino)`
// catches it: editing `alias` (a hardlink to tracked external F) with a clean-only
// citation MUST still be `Untrusted` + loud.
//
// REVERT-SENSITIVITY: this MUST fail if the inode match (`by_inode` /
// `tracked_file_with_identity`) is removed from step 4 — then the gate sees only
// the alias's unique path, finds no tracked record, and reads `Clean`/quiet.
// (Verified by temporarily dropping `by_inode` — see the fix report.)
#[test]
fn taint_engine_anchored_hardlink_alias_is_untrusted() {
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(16);
    let log = open_log(dir.path());

    // F is an ingested file under a READ grant → tracked, external `file_ingested`.
    let f_canonical = ingest_file(&log, &emb, dir.path(), "tracked.md", b"ingested body");

    // A SECOND name in the same dir, hardlinked to F: different path, SAME inode.
    let alias = f_canonical.parent().unwrap().join("alias.md");
    std::fs::hard_link(&f_canonical, &alias).unwrap();
    // Precondition: the alias is genuinely NOT the tracked canonical path.
    assert_ne!(
        std::fs::canonicalize(&alias).unwrap(),
        f_canonical,
        "the hardlink alias must have a distinct canonical path (else the test is moot)"
    );

    // WRITE grant over F's dir, and a CLEAN-only citation (the laundering attempt).
    log.add_write_grant(&dir.path().join("g")).unwrap();
    let clean_id = seed_memory(&log, &emb, "totally benign memory");

    let gated = log
        .propose_write(proposal(&alias, WriteOp::Edit, b"attacker payload", &[clean_id]))
        .unwrap();

    assert_eq!(
        gated.verdict.taint,
        Taint::Untrusted,
        "editing a HARDLINK ALIAS of a tracked external file is Untrusted (identity \
         anchor) — if this reads Clean the inode match was removed"
    );
    assert!(
        gated.verdict.requires_loud_modal,
        "an Untrusted write must force the loud modal"
    );
    // The alias resolves to the SAME file_event_id, so the engine provenance is shown.
    assert!(
        gated.verdict.provenance.iter().any(|p| p.is_external),
        "the engine-anchored external provenance must be surfaced for the alias too"
    );
}

// ── L10 fail-closed-over-set (REVERT-SENSITIVE) ───────────────────────────────
//
// One clean id + one NON-EXISTENT id. The whole proposal MUST be `Untrusted`
// because the gate fails closed over the SET (an unresolvable source taints all).
//
// REVERT-SENSITIVITY: this test MUST fail if step 1 is changed to
// `filter_map(resolvable).any(external)` — that would silently drop the
// non-existent id and judge only the clean one, reading `Clean`. The target here
// is a plain (non-tracked) file in a write-granted dir, so step 4 does NOT also
// taint it — the ONLY thing that can make this Untrusted is step 1's set rule.
// (Verified by temporarily switching to filter_map — see the build report.)
#[test]
fn taint_fail_closed_over_unresolvable_source_set() {
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(16);
    let log = open_log(dir.path());

    // A plain, NON-tracked file (no ingest) in a write-granted dir.
    let target = dir.path().join("plain.txt");
    std::fs::write(&target, b"existing").unwrap();
    log.add_write_grant(dir.path()).unwrap();

    let clean_id = seed_memory(&log, &emb, "benign memory");
    let nonexistent = "01ZZZZZZZZZZZZZZZZZZZZZZZZZ".to_string();

    let gated = log
        .propose_write(proposal(
            &target,
            WriteOp::Edit,
            b"new bytes",
            &[clean_id, nonexistent],
        ))
        .unwrap();

    assert_eq!(
        gated.verdict.taint,
        Taint::Untrusted,
        "an unresolvable cited id taints the WHOLE proposal (fail-closed over the set, \
         L10) — if this reads Clean the gate filter_map'd the resolvable subset"
    );
    assert!(gated.verdict.requires_loud_modal, "Untrusted forces the loud modal");
}

// Control for the test above: with ALL sources resolvable AND clean, AND a plain
// non-tracked target, the verdict is Clean — proving the Untrusted result above is
// caused by the unresolvable id, not by some always-on taint.
#[test]
fn taint_all_clean_resolvable_sources_is_clean() {
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(16);
    let log = open_log(dir.path());

    let target = dir.path().join("plain.txt");
    std::fs::write(&target, b"existing").unwrap();
    log.add_write_grant(dir.path()).unwrap();

    let clean_id = seed_memory(&log, &emb, "benign memory");

    let gated = log
        .propose_write(proposal(&target, WriteOp::Edit, b"ordinary new text", &[clean_id]))
        .unwrap();

    assert_eq!(
        gated.verdict.taint,
        Taint::Clean,
        "a plain target edited with a clean, resolvable citation is Clean"
    );
    assert!(
        !gated.verdict.requires_loud_modal,
        "a clean, non-delete, non-secret edit must NOT force the loud modal"
    );
    assert!(gated.verdict.reject_reason.is_none());
    assert!(gated.verdict.allowed, "target is under an active write grant");
}

// ── op × existence matrix → reject_reason ─────────────────────────────────────

#[test]
fn reject_create_of_existing_target() {
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(16);
    let log = open_log(dir.path());
    let target = dir.path().join("exists.txt");
    std::fs::write(&target, b"already here").unwrap();
    log.add_write_grant(dir.path()).unwrap();
    let clean = seed_memory(&log, &emb, "m");

    let gated = log
        .propose_write(proposal(&target, WriteOp::Create, b"x", &[clean]))
        .unwrap();
    assert!(
        gated.verdict.reject_reason.is_some(),
        "Create where the target already exists must be rejected"
    );
}

#[test]
fn reject_edit_of_absent_target() {
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(16);
    let log = open_log(dir.path());
    log.add_write_grant(dir.path()).unwrap();
    let clean = seed_memory(&log, &emb, "m");
    let absent = dir.path().join("nope.txt");

    let gated = log
        .propose_write(proposal(&absent, WriteOp::Edit, b"x", &[clean]))
        .unwrap();
    assert!(
        gated.verdict.reject_reason.is_some(),
        "Edit of a non-existent target must be rejected"
    );
}

#[test]
fn reject_delete_of_absent_target() {
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(16);
    let log = open_log(dir.path());
    log.add_write_grant(dir.path()).unwrap();
    let clean = seed_memory(&log, &emb, "m");
    let absent = dir.path().join("nope.txt");

    let gated = log
        .propose_write(proposal(&absent, WriteOp::Delete, b"", &[clean]))
        .unwrap();
    assert!(
        gated.verdict.reject_reason.is_some(),
        "Delete of a non-existent target must be rejected"
    );
}

#[test]
fn reject_symlink_final_component() {
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(16);
    let log = open_log(dir.path());
    log.add_write_grant(dir.path()).unwrap();
    let clean = seed_memory(&log, &emb, "m");

    // A real file + a symlink pointing at it; the symlink is the proposed target.
    let real = dir.path().join("real.txt");
    std::fs::write(&real, b"data").unwrap();
    let link = dir.path().join("link.txt");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let gated = log
        .propose_write(proposal(&link, WriteOp::Edit, b"x", &[clean]))
        .unwrap();
    assert!(
        gated.verdict.reject_reason.is_some(),
        "an existing-symlink final component must be rejected (never written through)"
    );
}

// ── monotonic loud-modal ──────────────────────────────────────────────────────

#[test]
fn loud_modal_clean_plain_edit_is_quiet() {
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(16);
    let log = open_log(dir.path());
    let target = dir.path().join("p.txt");
    std::fs::write(&target, b"old").unwrap();
    log.add_write_grant(dir.path()).unwrap();
    let clean = seed_memory(&log, &emb, "m");

    let gated = log
        .propose_write(proposal(&target, WriteOp::Edit, b"plain new content", &[clean]))
        .unwrap();
    assert!(
        !gated.verdict.requires_loud_modal,
        "a clean, non-delete, secret-free edit is quiet"
    );
}

#[test]
fn loud_modal_clean_edit_with_secret_diff_is_loud() {
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(16);
    let log = open_log(dir.path());
    let target = dir.path().join("p.txt");
    std::fs::write(&target, b"old").unwrap();
    log.add_write_grant(dir.path()).unwrap();
    let clean = seed_memory(&log, &emb, "m");

    // Secret-shaped new content (a long high-entropy token) → diff-guard escalates.
    let gated = log
        .propose_write(proposal(
            &target,
            WriteOp::Edit,
            b"token=ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789",
            &[clean],
        ))
        .unwrap();
    assert!(
        gated.verdict.diff_flags.touches_secret_shaped,
        "the high-entropy token must trip the advisory guard"
    );
    assert!(
        gated.verdict.requires_loud_modal,
        "a secret-shaped diff escalates the modal even on a clean edit"
    );
}

#[test]
fn loud_modal_any_delete_is_loud() {
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(16);
    let log = open_log(dir.path());
    let target = dir.path().join("p.txt");
    std::fs::write(&target, b"plain ordinary text, nothing secret").unwrap();
    log.add_write_grant(dir.path()).unwrap();
    let clean = seed_memory(&log, &emb, "m");

    let gated = log
        .propose_write(proposal(&target, WriteOp::Delete, b"", &[clean]))
        .unwrap();
    assert!(
        gated.verdict.requires_loud_modal,
        "every delete forces the loud modal, regardless of content"
    );
}

// ── base capture: Edit populates hash+identity; Create leaves them None ────────

#[test]
fn base_capture_edit_populates_hash_and_identity() {
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(16);
    let log = open_log(dir.path());
    let target = dir.path().join("p.txt");
    std::fs::write(&target, b"current bytes").unwrap();
    log.add_write_grant(dir.path()).unwrap();
    let clean = seed_memory(&log, &emb, "m");

    let gated = log
        .propose_write(proposal(&target, WriteOp::Edit, b"new", &[clean]))
        .unwrap();

    // hex SHA-256 of "current bytes".
    let expected = {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(b"current bytes"))
    };
    assert_eq!(
        gated.verdict.base_content_hash.as_deref(),
        Some(expected.as_str()),
        "base_content_hash is the hex SHA-256 of the current file bytes"
    );
    let id = gated.verdict.base_identity.expect("identity captured for an edit");
    assert_eq!(id.size, b"current bytes".len() as u64, "size matches the file");
    assert!(id.ino != 0, "a real inode was captured");
}

#[test]
fn base_capture_create_leaves_hash_and_identity_none() {
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(16);
    let log = open_log(dir.path());
    log.add_write_grant(dir.path()).unwrap();
    let clean = seed_memory(&log, &emb, "m");
    let absent = dir.path().join("new.txt");

    let gated = log
        .propose_write(proposal(&absent, WriteOp::Create, b"hello", &[clean]))
        .unwrap();
    assert!(gated.verdict.reject_reason.is_none(), "a fresh Create is allowed");
    assert!(
        gated.verdict.base_content_hash.is_none(),
        "a Create has no base content hash"
    );
    assert!(
        gated.verdict.base_identity.is_none(),
        "a Create has no base identity"
    );
}

// ── provenance: a file-derived source populates origin_path / is_external ──────

#[test]
fn provenance_file_source_populates_origin_path_and_external() {
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(16);
    let log = open_log(dir.path());

    // Ingest a file → get its external file_ingested event id to CITE directly.
    ingest_file(&log, &emb, dir.path(), "src.md", b"file body");
    let file_ev = log
        .stream_all()
        .unwrap()
        .into_iter()
        .find(|e| e.event_type == bossclaw_core::graph::FILE_INGESTED_EVENT_TYPE)
        .unwrap();

    // A plain target to write (so the result is driven by the cited source).
    let target = dir.path().join("out.txt");
    std::fs::write(&target, b"old").unwrap();
    log.add_write_grant(dir.path()).unwrap();

    let gated = log
        .propose_write(proposal(
            &target,
            WriteOp::Edit,
            b"x",
            std::slice::from_ref(&file_ev.id),
        ))
        .unwrap();

    let prov = gated
        .verdict
        .provenance
        .iter()
        .find(|p| p.event_id == file_ev.id)
        .expect("the cited file source is in the provenance list");
    assert!(prov.is_external, "a file_ingested source is external");
    assert!(
        prov.origin_path.is_some(),
        "a file-derived source populates origin_path"
    );
    assert_eq!(prov.kind, "file_ingested");
    // Citing an external source also makes the whole proposal Untrusted (step 1).
    assert_eq!(gated.verdict.taint, Taint::Untrusted);
}

// ── empty sources → reject ────────────────────────────────────────────────────

#[test]
fn reject_empty_source_event_ids() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let target = dir.path().join("p.txt");
    std::fs::write(&target, b"old").unwrap();
    log.add_write_grant(dir.path()).unwrap();

    let gated = log
        .propose_write(proposal(&target, WriteOp::Edit, b"x", &[]))
        .unwrap();
    assert!(
        gated.verdict.reject_reason.is_some(),
        "empty source_event_ids must be rejected (Tier-B needs lineage)"
    );
}

// ╔══════════════════════════════════════════════════════════════════════════╗
// ║  T4: execute_write — critical-section re-check + atomic mutate + record    ║
// ╚══════════════════════════════════════════════════════════════════════════╝

/// Read the appended `file_written` event back through the PUBLIC API. Asserts the
/// type so a wrong-id read is caught loudly.
fn file_written_event(log: &EventLog, id: &str) -> Event {
    let ev = log
        .event_by_id(id)
        .unwrap()
        .expect("file_written event must be readable via the public API");
    assert_eq!(
        ev.event_type,
        bossclaw_core::graph::FILE_WRITTEN_EVENT_TYPE,
        "execute_write must append a file_written event"
    );
    ev
}

// ── Happy paths: Create / Edit / Delete mutate the FS + record correctly ──────

#[test]
fn execute_create_writes_file_and_records_event() {
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(16);
    let log = open_log(dir.path());
    log.add_write_grant(dir.path()).unwrap();
    let clean = seed_memory(&log, &emb, "m");

    let target = dir.path().join("new.txt");
    let gated = log
        .propose_write(proposal(&target, WriteOp::Create, b"fresh bytes", std::slice::from_ref(&clean)))
        .unwrap();
    let id = log.execute_write(gated).unwrap();

    // FS mutated: the file now exists with EXACTLY the proposed bytes.
    assert_eq!(std::fs::read(&target).unwrap(), b"fresh bytes");

    // Event recorded with the right shape (spec §7.2).
    let ev = file_written_event(&log, &id);
    assert_eq!(ev.content["op"], "create");
    let canonical_target = std::fs::canonicalize(&target).unwrap().to_string_lossy().to_string();
    assert_eq!(ev.content["target"], serde_json::Value::String(canonical_target));
    let expected_hash = {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(b"fresh bytes"))
    };
    assert_eq!(ev.content["content_hash"], expected_hash);
    assert_eq!(ev.content["byte_size"], 11);
    // Create has NO prev_content_hash (omitted entirely).
    assert!(ev.content.get("prev_content_hash").is_none(), "Create omits prev_content_hash");
    // Tier-B by construction (model_meta set, prompt_hash empty).
    let meta = ev.model_meta.expect("file_written is always Tier-B");
    assert_eq!(meta.model_id, "m6a-actuator");
    assert_eq!(meta.prompt_hash, "");
    assert_eq!(meta.source_event_ids, vec![clean]);
}

#[test]
fn execute_edit_overwrites_file_and_records_prev_hash() {
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(16);
    let log = open_log(dir.path());
    let target = dir.path().join("p.txt");
    std::fs::write(&target, b"old contents").unwrap();
    log.add_write_grant(dir.path()).unwrap();
    let clean = seed_memory(&log, &emb, "m");

    let gated = log
        .propose_write(proposal(&target, WriteOp::Edit, b"new contents here", &[clean]))
        .unwrap();
    let id = log.execute_write(gated).unwrap();

    assert_eq!(std::fs::read(&target).unwrap(), b"new contents here");

    let ev = file_written_event(&log, &id);
    assert_eq!(ev.content["op"], "edit");
    let (new_hash, old_hash) = {
        use sha2::{Digest, Sha256};
        (hex::encode(Sha256::digest(b"new contents here")), hex::encode(Sha256::digest(b"old contents")))
    };
    assert_eq!(ev.content["content_hash"], new_hash);
    assert_eq!(ev.content["prev_content_hash"], old_hash, "Edit records the prior content hash");
    assert_eq!(ev.content["byte_size"], 17);
}

#[test]
fn execute_delete_hard_deletes_file_and_records_zero_size() {
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(16);
    let log = open_log(dir.path());
    let target = dir.path().join("doomed.txt");
    std::fs::write(&target, b"delete me").unwrap();
    log.add_write_grant(dir.path()).unwrap();
    let clean = seed_memory(&log, &emb, "m");

    let gated = log
        .propose_write(proposal(&target, WriteOp::Delete, b"", &[clean]))
        .unwrap();
    let id = log.execute_write(gated).unwrap();

    // Hard-deleted: the file is GONE (no OS trash — spec W5).
    assert!(!target.exists(), "delete must hard-remove the file");

    let ev = file_written_event(&log, &id);
    assert_eq!(ev.content["op"], "delete");
    assert_eq!(ev.content["byte_size"], 0, "a delete records byte_size 0");
    // content_hash == prev_content_hash == the deleted file's hash (spec §7.2 reading).
    let deleted_hash = {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(b"delete me"))
    };
    assert_eq!(ev.content["content_hash"], deleted_hash);
    assert_eq!(ev.content["prev_content_hash"], deleted_hash);
}

// ── TOCTOU path-swap: pass the gate, swap the target out-of-grant, then execute ─
//
// The benign target passes propose; before execute we replace it with a SYMLINK
// pointing outside the grant. The execute-time fd-relative NOFOLLOW open must
// refuse the symlinked final component ⇒ fail-closed, and the outside file is
// untouched.
#[test]
fn execute_toctou_path_swap_to_symlink_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(16);
    let log = open_log(dir.path());
    let target = dir.path().join("p.txt");
    std::fs::write(&target, b"benign original").unwrap();
    log.add_write_grant(dir.path()).unwrap();
    let clean = seed_memory(&log, &emb, "m");

    // A sentinel OUTSIDE the grant that the swap would aim at.
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join("secret.txt");
    std::fs::write(&secret, b"do not touch").unwrap();

    let gated = log
        .propose_write(proposal(&target, WriteOp::Edit, b"payload", &[clean]))
        .unwrap();
    assert!(gated.verdict.reject_reason.is_none(), "gate passes for the benign target");

    // TOCTOU: replace the real target with a symlink to the outside secret.
    std::fs::remove_file(&target).unwrap();
    std::os::unix::fs::symlink(&secret, &target).unwrap();

    let err = log.execute_write(gated);
    assert!(err.is_err(), "a final-component swapped to a symlink must fail closed");
    assert_eq!(
        std::fs::read(&secret).unwrap(),
        b"do not touch",
        "the out-of-grant secret must be untouched by the refused write"
    );
}

// ── Same-content / different-inode swap (REVERT-SENSITIVE) ────────────────────
//
// Replace the target with a SAME-BYTES, DIFFERENT-INODE file between propose and
// execute. The content hash still matches, so ONLY the (dev,ino) half of the base
// guard can catch it. Must FAIL if that half is dropped.
//
// REVERT-SENSITIVITY: comment out the `now_identity != want_identity` check in
// `execute_write` step 3 → this test goes from fail-closed (Err) to a SUCCESSFUL
// overwrite of the swapped-in inode, so the assertion `err.is_err()` fails.
// (Demonstrated below — see the build report.)
#[test]
fn execute_same_content_different_inode_swap_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(16);
    let log = open_log(dir.path());
    let target = dir.path().join("p.txt");
    std::fs::write(&target, b"identical bytes").unwrap();
    log.add_write_grant(dir.path()).unwrap();
    let clean = seed_memory(&log, &emb, "m");

    let gated = log
        .propose_write(proposal(&target, WriteOp::Edit, b"payload", &[clean]))
        .unwrap();
    let ino_before = std::fs::metadata(&target).unwrap().ino_u64();

    // Swap: remove the target and recreate it with the SAME bytes → a NEW inode.
    std::fs::remove_file(&target).unwrap();
    std::fs::write(&target, b"identical bytes").unwrap();
    let ino_after = std::fs::metadata(&target).unwrap().ino_u64();
    assert_ne!(ino_before, ino_after, "the swap must produce a different inode (else the test is moot)");

    let err = log.execute_write(gated);
    assert!(
        err.is_err(),
        "a same-content different-inode swap must fail closed on the (dev,ino) half of the guard"
    );
    // The swapped-in file is left UNCHANGED (still the identical bytes, not 'payload').
    assert_eq!(std::fs::read(&target).unwrap(), b"identical bytes");
}

// ── Base content-change between propose and execute → fail-closed ─────────────
#[test]
fn execute_base_content_change_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(16);
    let log = open_log(dir.path());
    let target = dir.path().join("p.txt");
    std::fs::write(&target, b"version one").unwrap();
    log.add_write_grant(dir.path()).unwrap();
    let clean = seed_memory(&log, &emb, "m");

    let gated = log
        .propose_write(proposal(&target, WriteOp::Edit, b"payload", &[clean]))
        .unwrap();

    // Mutate the base bytes IN PLACE (same inode, different content) after propose.
    std::fs::write(&target, b"version two changed").unwrap();

    let err = log.execute_write(gated);
    assert!(err.is_err(), "a base content change since propose must fail closed");
    assert_eq!(
        std::fs::read(&target).unwrap(),
        b"version two changed",
        "the changed file must NOT be clobbered by the stale write"
    );
}

// ── Create-of-existing (racing a file in) → fail-closed (no-clobber) ──────────
#[test]
fn execute_create_racing_existing_file_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(16);
    let log = open_log(dir.path());
    log.add_write_grant(dir.path()).unwrap();
    let clean = seed_memory(&log, &emb, "m");

    let target = dir.path().join("racer.txt");
    // Gate passes while the target is absent.
    let gated = log
        .propose_write(proposal(&target, WriteOp::Create, b"mine", &[clean]))
        .unwrap();
    assert!(gated.verdict.reject_reason.is_none(), "Create of an absent target passes the gate");

    // RACE: a file appears at the target name before execute.
    std::fs::write(&target, b"someone else got here first").unwrap();

    let err = log.execute_write(gated);
    assert!(err.is_err(), "Create must not clobber a file that raced into place");
    assert_eq!(
        std::fs::read(&target).unwrap(),
        b"someone else got here first",
        "the racing file's bytes must be preserved (no-clobber)"
    );
}

// ── Grant revoked between propose and execute → fail-closed ───────────────────
#[test]
fn execute_grant_revoked_between_propose_and_execute_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(16);
    let log = open_log(dir.path());
    let target = dir.path().join("p.txt");
    std::fs::write(&target, b"original").unwrap();
    log.add_write_grant(dir.path()).unwrap();
    let clean = seed_memory(&log, &emb, "m");

    let gated = log
        .propose_write(proposal(&target, WriteOp::Edit, b"payload", &[clean]))
        .unwrap();
    assert!(gated.verdict.allowed, "granted at propose time");

    // Revoke the write grant AFTER propose. execute re-checks → fail-closed.
    log.revoke_write_grant(dir.path()).unwrap();

    let err = log.execute_write(gated);
    assert!(err.is_err(), "a grant revoked since propose must fail closed at execute");
    assert_eq!(std::fs::read(&target).unwrap(), b"original", "the file is untouched after a revoked-grant reject");
}

// ── A verdict carrying a reject_reason cannot be executed ─────────────────────
#[test]
fn execute_refuses_a_rejected_verdict() {
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(16);
    let log = open_log(dir.path());
    log.add_write_grant(dir.path()).unwrap();
    let clean = seed_memory(&log, &emb, "m");

    // Edit of an ABSENT target → the gate sets reject_reason.
    let absent = dir.path().join("nope.txt");
    let gated = log
        .propose_write(proposal(&absent, WriteOp::Edit, b"x", &[clean]))
        .unwrap();
    assert!(gated.verdict.reject_reason.is_some(), "precondition: the verdict is rejected");

    assert!(
        log.execute_write(gated).is_err(),
        "execute must refuse a verdict that carries a reject_reason"
    );
}

// ── No Tier-A `file_written`: the sole-constructor invariant ──────────────────
//
// The ONLY public way to produce a `file_written` is execute_write, which ALWAYS
// sets model_meta. So no `file_written` in the log can have model_meta: None.
#[test]
fn no_tier_a_file_written_can_be_produced() {
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(16);
    let log = open_log(dir.path());
    log.add_write_grant(dir.path()).unwrap();
    let clean = seed_memory(&log, &emb, "m");

    // Produce several file_written events across all three ops.
    let create_t = dir.path().join("a.txt");
    let g = log.propose_write(proposal(&create_t, WriteOp::Create, b"a", std::slice::from_ref(&clean))).unwrap();
    log.execute_write(g).unwrap();

    let edit_t = dir.path().join("b.txt");
    std::fs::write(&edit_t, b"old").unwrap();
    let g = log.propose_write(proposal(&edit_t, WriteOp::Edit, b"new", std::slice::from_ref(&clean))).unwrap();
    log.execute_write(g).unwrap();

    let del_t = dir.path().join("c.txt");
    std::fs::write(&del_t, b"bye").unwrap();
    let g = log.propose_write(proposal(&del_t, WriteOp::Delete, b"", &[clean])).unwrap();
    log.execute_write(g).unwrap();

    // EVERY file_written in the log is Tier-B (model_meta is Some, with non-empty sources).
    let all = log.stream_all().unwrap();
    let written: Vec<_> = all
        .iter()
        .filter(|e| e.event_type == bossclaw_core::graph::FILE_WRITTEN_EVENT_TYPE)
        .collect();
    assert_eq!(written.len(), 3, "all three writes were recorded");
    for ev in written {
        let meta = ev
            .model_meta
            .as_ref()
            .expect("a file_written must be Tier-B (model_meta: Some) — no Tier-A is possible");
        assert!(!meta.source_event_ids.is_empty(), "Tier-B requires non-empty sources");
        // Load-bearing (spec §6 M1): the taint chokepoint only stamps `origin` when
        // content is a JSON object. A future refactor to a non-object payload would
        // silently disable taint stamping — assert the invariant directly so it can't
        // regress unnoticed.
        assert!(
            ev.content.is_object(),
            "file_written content must be a JSON object so the taint chokepoint can stamp origin"
        );
    }
}

// ── L11 end-to-end (REVERT-SENSITIVE): editing a tracked ingested file stamps
// ── the recorded file_written `origin: "external"`, even citing only clean ids ─
//
// The chokepoint stamps `origin: external` iff a source is external. execute_write
// step 4 RE-DERIVES the target's `file_ingested` id and unions it into the recorded
// sources — so a write to a tracked external file is stamped external by
// construction, MATCHING the propose-time verdict (L11). Citing only a clean id
// must NOT launder this.
//
// REVERT-SENSITIVITY: drop step 4's source augmentation in `execute_write` (do not
// push `rec.file_event_id`) → the recorded sources are clean-only → the chokepoint
// does NOT stamp `origin` → this test's `origin == "external"` assertion fails.
// (Demonstrated below — see the build report.)
#[test]
fn execute_l11_tracked_file_edit_is_stamped_external() {
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(16);
    let log = open_log(dir.path());

    // F is an ingested file under a READ grant → an external `file_ingested`.
    let f_canonical = ingest_file(&log, &emb, dir.path(), "notes.md", b"ingested body");
    // Also write-grant F's directory so the write is allowed.
    log.add_write_grant(&dir.path().join("g")).unwrap();
    // The attacker cites ONLY a clean memory id (the cite-around).
    let clean = seed_memory(&log, &emb, "totally benign memory");

    let gated = log
        .propose_write(proposal(&f_canonical, WriteOp::Edit, b"attacker payload", std::slice::from_ref(&clean)))
        .unwrap();
    // The verdict already says Untrusted (propose-side L11); now prove the PERSISTED
    // event matches (execute-side L11 augmentation + chokepoint stamp).
    assert_eq!(gated.verdict.taint, Taint::Untrusted, "propose-side anchor flags it");

    let id = log.execute_write(gated).unwrap();
    let ev = file_written_event(&log, &id);

    // The chokepoint stamped the signed content external (because a re-derived source
    // — F's file_ingested — is external). This is the verdict ≡ persisted-stamp law.
    assert_eq!(
        ev.content["origin"], "external",
        "a write to a tracked external file must be stamped origin:external in the signed event \
         (if absent, step 4's source augmentation was dropped)"
    );
    // And the recorded sources include the engine-derived file_ingested id (not just
    // the clean citation), so the lineage is honest.
    let meta = ev.model_meta.expect("Tier-B");
    assert!(meta.source_event_ids.len() >= 2, "the clean cite PLUS the engine-derived id");
    assert!(meta.source_event_ids.contains(&clean), "the caller's clean cite is preserved");
    // The extra id resolves to a file_ingested event (the engine anchor).
    let extra: Vec<_> = meta.source_event_ids.iter().filter(|s| **s != clean).collect();
    assert_eq!(extra.len(), 1, "exactly one engine-derived source was added");
    let extra_ev = log.event_by_id(extra[0]).unwrap().expect("the augmented source resolves");
    assert_eq!(extra_ev.event_type, bossclaw_core::graph::FILE_INGESTED_EVENT_TYPE);
}

// ╔══════════════════════════════════════════════════════════════════════════╗
// ║  T5: N-deep undo store + undo_write (re-gated)                             ║
// ╚══════════════════════════════════════════════════════════════════════════╝

/// Run one Edit `target → new_content`, citing `clean`, and return the
/// `file_written` id. A small helper so the undo tests read as a sequence of
/// edits without repeating the propose/execute dance.
fn do_edit(log: &EventLog, target: &std::path::Path, new_content: &[u8], clean: &str) -> String {
    let gated = log
        .propose_write(proposal(target, WriteOp::Edit, new_content, std::slice::from_ref(&clean.to_string())))
        .unwrap();
    log.execute_write(gated).unwrap()
}

// ── undo of Edit restores the prior content + records file_written(undo_of) ────
#[test]
fn undo_edit_restores_prior_content_and_records_event() {
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(16);
    let log = open_log(dir.path());
    let target = dir.path().join("p.txt");
    std::fs::write(&target, b"v0 original").unwrap();
    log.add_write_grant(dir.path()).unwrap();
    let clean = seed_memory(&log, &emb, "m");

    // Edit v0 → v1. Undoing this must restore v0.
    let write_id = do_edit(&log, &target, b"v1 edited bytes", &clean);
    assert_eq!(std::fs::read(&target).unwrap(), b"v1 edited bytes");

    let undo_id = log.undo_write(&write_id).unwrap();
    assert_eq!(
        std::fs::read(&target).unwrap(),
        b"v0 original",
        "undo of an Edit restores the exact prior bytes"
    );

    // The undo is itself a recorded file_written carrying undo_of + source=[orig].
    let ev = file_written_event(&log, &undo_id);
    assert_eq!(ev.content["op"], "edit", "an Edit-restore is recorded as an edit");
    assert_eq!(
        ev.content["undo_of"], serde_json::Value::String(write_id.clone()),
        "the undo records the id it reverses"
    );
    let meta = ev.model_meta.expect("undo is Tier-B");
    assert!(
        meta.source_event_ids.contains(&write_id),
        "the undo's lineage cites the original write id"
    );
}

// ── undo of Create removes the file ───────────────────────────────────────────
#[test]
fn undo_create_removes_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(16);
    let log = open_log(dir.path());
    log.add_write_grant(dir.path()).unwrap();
    let clean = seed_memory(&log, &emb, "m");

    let target = dir.path().join("created.txt");
    let gated = log
        .propose_write(proposal(&target, WriteOp::Create, b"new file", std::slice::from_ref(&clean)))
        .unwrap();
    let write_id = log.execute_write(gated).unwrap();
    assert!(target.exists(), "precondition: create made the file");

    let undo_id = log.undo_write(&write_id).unwrap();
    assert!(!target.exists(), "undo of a Create hard-removes the created file");

    let ev = file_written_event(&log, &undo_id);
    assert_eq!(ev.content["op"], "delete", "undoing a Create is recorded as a delete");
    assert_eq!(ev.content["undo_of"], serde_json::Value::String(write_id));
}

// ── undo of Delete recreates the file from the captured pre-bytes ──────────────
#[test]
fn undo_delete_recreates_file_from_pre_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(16);
    let log = open_log(dir.path());
    let target = dir.path().join("doomed.txt");
    std::fs::write(&target, b"precious contents").unwrap();
    log.add_write_grant(dir.path()).unwrap();
    let clean = seed_memory(&log, &emb, "m");

    let gated = log
        .propose_write(proposal(&target, WriteOp::Delete, b"", std::slice::from_ref(&clean)))
        .unwrap();
    let write_id = log.execute_write(gated).unwrap();
    assert!(!target.exists(), "precondition: delete removed the file");

    let undo_id = log.undo_write(&write_id).unwrap();
    assert!(target.exists(), "undo of a Delete recreates the file");
    assert_eq!(
        std::fs::read(&target).unwrap(),
        b"precious contents",
        "the recreated file has the exact deleted bytes"
    );

    let ev = file_written_event(&log, &undo_id);
    assert_eq!(ev.content["op"], "create", "undoing a Delete is recorded as a create");
    assert_eq!(ev.content["undo_of"], serde_json::Value::String(write_id));
}

// ── N-deep: N+1 edits → oldest GC'd, last N undo in LIFO order ─────────────────
//
// UNDO_DEPTH is 16; do 17 sequential edits, then undo repeatedly. The last 16
// undo correctly (LIFO: each undo restores the bytes from just before that
// write). The 17th-from-top (the oldest, whose row was GC'd) can no longer undo.
#[test]
fn undo_is_n_deep_with_lifo_and_oldest_gc() {
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(16);
    let log = open_log(dir.path());
    let target = dir.path().join("p.txt");
    // Content "state 0" is the base before the FIRST edit.
    std::fs::write(&target, b"state 0").unwrap();
    log.add_write_grant(dir.path()).unwrap();
    let clean = seed_memory(&log, &emb, "m");

    // UNDO_DEPTH = 16; one more than that to force a GC of the oldest row.
    const DEPTH: usize = 16;
    let edits = DEPTH + 1; // 17 edits → states 1..=17
    let mut write_ids = Vec::new();
    for i in 1..=edits {
        let content = format!("state {i}");
        write_ids.push(do_edit(&log, &target, content.as_bytes(), &clean));
    }
    assert_eq!(std::fs::read(&target).unwrap(), b"state 17");

    // The oldest write (write_ids[0], which captured "state 0" as its pre-bytes)
    // had its undo row GC'd once the 17th edit landed → undoing it fails closed.
    assert!(
        log.undo_write(&write_ids[0]).is_err(),
        "the oldest pre-bytes were GC'd (N-deep retention) → its undo must fail closed"
    );

    // The last DEPTH writes undo in LIFO order: undo write 17 → "state 16",
    // undo write 16 → "state 15", … undo write 2 → "state 1".
    for i in (edits - DEPTH + 1..=edits).rev() {
        log.undo_write(&write_ids[i - 1]).unwrap();
        let expected = format!("state {}", i - 1);
        assert_eq!(
            std::fs::read(&target).unwrap(),
            expected.as_bytes(),
            "undoing write {i} (LIFO) must restore the bytes from just before it"
        );
    }
    // After undoing back through write 2, the file holds "state 1".
    assert_eq!(std::fs::read(&target).unwrap(), b"state 1");
}

// ── undo RE-GATES (a): grant revoked after the write → undo fails closed ───────
//
// REVERT-SENSITIVITY: undo runs the restore through the FULL propose→execute
// path against CURRENT grants. Revoking the write grant after the original write
// makes the restore fail the grant gate. If undo_write bypassed the gate (wrote
// the pre-bytes directly), this would SUCCEED — so the assertion catches a
// missing re-gate. (Demonstrated in the build report.)
#[test]
fn undo_re_gates_revoked_grant_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(16);
    let log = open_log(dir.path());
    let target = dir.path().join("p.txt");
    std::fs::write(&target, b"v0").unwrap();
    log.add_write_grant(dir.path()).unwrap();
    let clean = seed_memory(&log, &emb, "m");

    let write_id = do_edit(&log, &target, b"v1", &clean);
    assert_eq!(std::fs::read(&target).unwrap(), b"v1");

    // Revoke the write grant, THEN try to undo. The undo restore must be
    // re-gated against the (now-revoked) grant → fail closed.
    log.revoke_write_grant(dir.path()).unwrap();
    assert!(
        log.undo_write(&write_id).is_err(),
        "undo must re-gate: a revoked write grant makes the restore fail closed"
    );
    assert_eq!(
        std::fs::read(&target).unwrap(),
        b"v1",
        "the file is untouched when the re-gated undo is refused"
    );
}

// ── undo RE-GATES (b): target identity diverged (inode swap) → fail closed ─────
//
// REVERT-SENSITIVITY: between the write and the undo, swap the target for a
// different inode with the SAME bytes that the undo expects to overwrite. The
// undo restore is an Edit whose base guard re-asserts (dev,ino) — the swap
// diverges it → fail closed. If the identity half of the guard were dropped, the
// undo would clobber the swapped-in inode. (Demonstrated in the build report.)
#[test]
fn undo_re_gates_target_identity_swap_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let emb = MockEmbedder::new(16);
    let log = open_log(dir.path());
    let target = dir.path().join("p.txt");
    std::fs::write(&target, b"v0 original").unwrap();
    log.add_write_grant(dir.path()).unwrap();
    let clean = seed_memory(&log, &emb, "m");

    // Edit v0 → v1. The undo of this is an Edit that overwrites the CURRENT file
    // (whose identity must match what the undo recorded: the v1 inode).
    let write_id = do_edit(&log, &target, b"v1 edited", &clean);
    let ino_v1 = std::fs::metadata(&target).unwrap().ino_u64();

    // Swap the target for a DIFFERENT inode (same name) — same bytes as the
    // current v1, so only the (dev,ino) half of the guard can catch it.
    std::fs::remove_file(&target).unwrap();
    std::fs::write(&target, b"v1 edited").unwrap();
    let ino_swapped = std::fs::metadata(&target).unwrap().ino_u64();
    assert_ne!(ino_v1, ino_swapped, "the swap must change the inode (else the test is moot)");

    assert!(
        log.undo_write(&write_id).is_err(),
        "undo must re-gate: a diverged target identity makes the restore fail closed"
    );
    assert_eq!(
        std::fs::read(&target).unwrap(),
        b"v1 edited",
        "the swapped-in inode is untouched by the refused undo"
    );
}

// NOTE: undo RE-GATES (c) — the tampered-pre_bytes hash-mismatch revert-sensitive
// test — lives IN-CRATE (`src/log.rs` `#[cfg(test)]`), because tampering the
// encrypted `undo_state` row needs the private store handle. Keeping it in-crate
// avoids exposing any public "corrupt my undo bytes" surface or an extra feature
// flag, so `cargo test -p bossclaw-core` compiles with zero features. See
// `undo_tamper_pre_bytes_fails_closed` there.

/// Tiny extension trait so the tests can read a unix inode without repeating the
/// `MetadataExt` import dance at each call site.
trait InoExt {
    fn ino_u64(&self) -> u64;
}
impl InoExt for std::fs::Metadata {
    fn ino_u64(&self) -> u64 {
        use std::os::unix::fs::MetadataExt;
        self.ino()
    }
}
