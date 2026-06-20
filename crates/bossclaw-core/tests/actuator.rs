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
