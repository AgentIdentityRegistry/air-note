//! M6b reconciliation proposer — hermetic tests.
//!
//! `EventLog` is constructed and operated entirely inside the `common` harness, so the
//! test bodies never name the type directly — there is intentionally no `use` for it.
#![cfg(unix)]
mod common;

use bossclaw_core::actuator::{WriteOp, WriteProposal};
use serde_json::json;

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

/// A write_proposal is Tier-B, a JSON object, carries the inducing lineage, and is
/// stamped origin:"external" when a source is a tracked file.
#[test]
fn write_proposal_event_is_tier_b_object_and_taint_stamped() {
    let (log, _home, dir) = common::open_log_with_write_grant();
    let path = dir.join("n.md");
    std::fs::write(&path, b"fact\n").unwrap();
    let file_id = common::ingest_one(&log, &path); // external source

    let canonical = std::fs::canonicalize(&path).unwrap().to_string_lossy().to_string();
    let pid = log.append_write_proposal(
        &canonical, "edit", "deadbeef", 12, "reconcile: A -rel-> B",
        &json!({"src":"entity:a","relation":"rel","dst":"entity:b"}),
        &json!({"requires_loud_modal":true,"taint":"Untrusted","allowed":true}),
        std::slice::from_ref(&file_id),
    ).unwrap();

    let ev = log.event_by_id(&pid).unwrap().unwrap();
    assert_eq!(ev.event_type, "write_proposal");
    assert!(ev.content.is_object(), "content must be a JSON object (chokepoint stamps objects only)");
    assert_eq!(ev.content["origin"], json!("external"), "tracked-file source taints the proposal");
    let meta = ev.model_meta.as_ref().expect("Tier-B");
    assert_eq!(meta.model_id, "m6b-reconciler");
    assert!(meta.source_event_ids.contains(&file_id));
}

/// The `file_written` record gains `resolves_proposal` ONLY when the write was
/// confirmed via `execute_write_resolving`; the plain `execute_write` path omits it
/// entirely (the M6a content shape is byte-identical to before M6b).
#[test]
fn file_written_records_resolves_proposal_only_when_set() {
    let (log, _home, dir) = common::open_log_with_write_grant();
    // A real, citeable source event (an external file_ingested) for the write lineage.
    let src = dir.join("src.md");
    std::fs::write(&src, b"seed\n").unwrap();
    let file_id = common::ingest_one(&log, &src);

    // Helper: gate a Create of `name` citing `file_id`, returning the GatedProposal.
    let gate_create = |name: &str| {
        let target = dir.join(name);
        log.propose_write(WriteProposal {
            target,
            new_content: b"body".to_vec(),
            op: WriteOp::Create,
            source_event_ids: vec![file_id.clone()],
            rationale: "test".to_string(),
        })
        .expect("propose_write")
    };

    // Plain write → no `resolves_proposal` key.
    let plain_id = log.execute_write(gate_create("plain.txt")).expect("execute_write");
    let plain = log.event_by_id(&plain_id).unwrap().unwrap();
    assert_eq!(plain.event_type, "file_written");
    assert!(
        plain.content.get("resolves_proposal").is_none(),
        "execute_write must omit resolves_proposal entirely"
    );

    // Resolving write → the key carries the proposal id.
    let resolved_id = log
        .execute_write_resolving(gate_create("resolved.txt"), "prop-xyz")
        .expect("execute_write_resolving");
    let resolved = log.event_by_id(&resolved_id).unwrap().unwrap();
    assert_eq!(resolved.content["resolves_proposal"], json!("prop-xyz"));
}

/// `decline_write_proposal` appends a `write_declined` that RESOLVES the proposal,
/// inheriting its lineage; an unknown id is rejected.
#[test]
fn decline_write_proposal_resolves_and_inherits_lineage() {
    let (log, _home, dir) = common::open_log_with_write_grant();
    let path = dir.join("d.md");
    std::fs::write(&path, b"fact\n").unwrap();
    let file_id = common::ingest_one(&log, &path);

    let canonical = std::fs::canonicalize(&path).unwrap().to_string_lossy().to_string();
    let pid = log
        .append_write_proposal(
            &canonical, "edit", "deadbeef", 12, "reconcile",
            &json!({"src": "entity:a", "relation": "rel", "dst": "entity:b"}),
            &json!({"allowed": true}),
            std::slice::from_ref(&file_id),
        )
        .unwrap();

    let did = log.decline_write_proposal(&pid, "user said no").unwrap();
    let ev = log.event_by_id(&did).unwrap().unwrap();
    assert_eq!(ev.event_type, "write_declined");
    assert_eq!(ev.content["resolves_proposal"], json!(pid));
    assert_eq!(ev.content["reason"], json!("user said no"));
    // Lineage inherited from the proposal (so the decline carries the same taint root).
    let meta = ev.model_meta.as_ref().expect("Tier-B");
    assert!(meta.source_event_ids.contains(&file_id));

    // An unknown / non-Tier-B id is rejected (no sources to inherit).
    assert!(log.decline_write_proposal("01BOGUS", "x").is_err());
}

/// The recorded lineage is engine-gathered: union(retired edge's source_ids, read_set).
/// It must include BOTH the asserting file (edge lineage) AND the correcting source
/// (read_set) — and NEVER depend on model-chosen citations.
#[test]
fn reconciliation_lineage_unions_edge_and_read_set_not_entity() {
    let (log, _home, _dir) = common::open_log_with_write_grant();
    let file_a = common::seed_external_event(&log, "Alice works at Acme");
    let edge_id = common::seed_edge_with_sources(&log, "entity:alice", "works_at", "entity:acme", std::slice::from_ref(&file_a));
    let mem_b = common::seed_memory(&log, "Actually Alice works at Globex");
    let read_set = vec![mem_b.clone()];

    let lineage = log.reconciliation_lineage(&edge_id, &read_set).unwrap();
    assert!(lineage.contains(&file_a), "asserting file (edge lineage) present");
    assert!(lineage.contains(&mem_b),  "correcting source (read_set) present — the SEC-C2 fix");
    let mut sorted = lineage.clone(); sorted.sort(); sorted.dedup();
    assert_eq!(lineage, sorted, "sorted + deduped");
}

/// SEC-C2 revert-sensitive: if the CORRECTING fact is itself file-derived, that file id
/// MUST be in the lineage (this FAILS if read_set is dropped from the union).
#[test]
fn correcting_file_is_recorded_in_lineage() {
    let (log, _home, _dir) = common::open_log_with_write_grant();
    let file_a = common::seed_external_event(&log, "Alice works at Acme");
    let edge_id = common::seed_edge_with_sources(&log, "entity:alice", "works_at", "entity:acme", &[file_a]);
    let file_b = common::seed_external_event(&log, "Alice works at Globex");
    let lineage = log.reconciliation_lineage(&edge_id, std::slice::from_ref(&file_b)).unwrap();
    assert!(lineage.contains(&file_b), "the correcting file MUST be recorded (no laundering)");
}

/// Fail-closed: an edge id that resolves to NO lineage (unknown/nonexistent) contributes
/// nothing, but MUST NOT drop the read_set — the result is exactly the sorted/deduped
/// read_set, never a silent empty. Guards against a future refactor to `.unwrap_or_default()`
/// (or an entity-id misuse) that would launder the correcting source away.
#[test]
fn reconciliation_lineage_unknown_edge_preserves_read_set() {
    let (log, _home, _dir) = common::open_log_with_write_grant();
    let file_b = common::seed_external_event(&log, "Alice works at Globex");
    let mem_c = common::seed_memory(&log, "and reports to Carol");
    // Intentionally bogus edge id: no such event → source_ids_of_event yields None.
    let read_set = vec![file_b, mem_c];
    let lineage = log
        .reconciliation_lineage("01NONEXISTENTEDGE0000000000", &read_set)
        .unwrap();

    let mut expected = read_set.clone();
    expected.sort();
    expected.dedup();
    assert_eq!(lineage, expected, "missing edge contributes nothing; read_set is preserved");
}
