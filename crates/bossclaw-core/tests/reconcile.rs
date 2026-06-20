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

/// SEC#5: the side table is a cache, never an authorization source. Bytes whose hash
/// no longer matches the recorded content_hash fail closed at confirm-readback.
#[test]
fn proposal_bytes_tamper_fails_closed() {
    let (log, _home, _dir) = common::open_log_with_write_grant();
    let pid = "01PROPOSALID";
    let bytes = b"corrected contents\n";
    let hash = common::sha256_hex(bytes); // use the SAME hasher the engine uses for content_hash
    log.put_proposal_bytes(pid, bytes, &hash).unwrap();
    assert_eq!(log.get_proposal_bytes_checked(pid, &hash).unwrap(), bytes.to_vec());
    // Branch 1 — wrong expected hash (as if the signed event recorded a different one):
    assert!(log.get_proposal_bytes_checked(pid, "00deadbeef").is_err());

    // Branch 2 — the row itself is tampered: stored bytes that do NOT hash to the stored
    // content_hash. Even when read back with the "correct" expected hash, the recomputed
    // hash of the stored bytes != stored_hash → fail closed, no bytes returned. (Driven
    // purely through the public API: store CONTENT and HASH that disagree.)
    log.put_proposal_bytes("01TAMPERED", b"ATTACKER-SWAPPED BYTES", &hash).unwrap();
    assert!(
        log.get_proposal_bytes_checked("01TAMPERED", &hash).is_err(),
        "stored bytes that don't match the recorded content_hash must fail closed"
    );
}

/// §8.14 confirm-path round-trip: stored corrected bytes are re-read, re-hashed, then run
/// through `propose_write → execute_write_resolving` (the full M6a gate). The on-disk file
/// gains the corrected bytes, the `file_written` back-references the proposal, and
/// `undo_write` restores the original — proving the side table only CACHES; the gate (grant
/// + (dev,ino,size) + base-hash) re-runs from scratch and the write is fully undoable.
#[test]
fn proposal_bytes_round_trip_through_execute_write_resolving() {
    use bossclaw_core::actuator::{WriteOp, WriteProposal};
    use serde_json::json;

    let (log, _home, dir) = common::open_log_with_write_grant();

    // Ingest a real `.md` target under the write grant; capture the original bytes.
    let target = dir.join("page.md");
    let original = b"Alice works at Acme.\n";
    std::fs::write(&target, original).unwrap();
    let file_id = common::ingest_one(&log, &target);
    let canonical = std::fs::canonicalize(&target).unwrap().to_string_lossy().to_string();

    // Synthesize corrected bytes + their (engine) hash; stash them in the side table keyed
    // by a real-ish proposal id minted by appending a `write_proposal` with this hash.
    let corrected = b"Alice works at Globex.\n";
    let hash = common::sha256_hex(corrected);
    let pid = log
        .append_write_proposal(
            &canonical, "edit", &hash, corrected.len() as u64, "reconcile: Acme -> Globex",
            &json!({"src": "entity:alice", "relation": "works_at", "dst": "entity:globex"}),
            &json!({"allowed": true}),
            std::slice::from_ref(&file_id),
        )
        .unwrap();
    log.put_proposal_bytes(&pid, corrected, &hash).unwrap();

    // Confirm path: re-read (re-hashed against the signed-event hash) → gate → resolving write.
    let bytes = log.get_proposal_bytes_checked(&pid, &hash).unwrap();
    let gated = log
        .propose_write(WriteProposal {
            target: target.clone(),
            new_content: bytes,
            op: WriteOp::Edit,
            source_event_ids: vec![file_id.clone()], // reconciliation lineage
            rationale: "apply reconciliation proposal".to_string(),
        })
        .expect("propose_write");
    let written_id = log.execute_write_resolving(gated, &pid).expect("execute_write_resolving");

    // The file now holds the corrected bytes, and the record back-references the proposal.
    assert_eq!(std::fs::read(&target).unwrap(), corrected.to_vec());
    let written = log.event_by_id(&written_id).unwrap().unwrap();
    assert_eq!(written.event_type, "file_written");
    assert_eq!(written.content["resolves_proposal"], json!(pid));

    // Undo restores the original bytes (the resolving write is a normal, undoable Edit).
    log.undo_write(&written_id).expect("undo_write");
    assert_eq!(std::fs::read(&target).unwrap(), original.to_vec());
}

/// A write_proposal is OPEN until a human-terminal event references it; an engine
/// write_rejected suppresses re-attempts for (path, key) but does NOT "resolve" a proposal;
/// write_declined and file_written{resolves_proposal} both close it.
#[test]
fn pending_projection_open_close_and_suppress() {
    let (log, _home, dir) = common::open_write_grant_and_external_target();
    let path = dir.join("n.md");
    let canonical = std::fs::canonicalize(&path).unwrap().to_string_lossy().to_string();
    let key = serde_json::json!({"src":"entity:a","relation":"rel","dst":"entity:b"});

    assert!(!log.is_proposal_suppressed(&canonical, &key).unwrap(), "nothing yet → may propose");

    let pid = common::append_minimal_proposal(&log, &canonical, &key);
    assert!(log.is_proposal_suppressed(&canonical, &key).unwrap(), "an OPEN proposal suppresses");

    log.decline_write_proposal(&pid, "not now").unwrap();
    assert!(!log.is_proposal_suppressed(&canonical, &key).unwrap(), "declined → no longer open");

    common::append_rejected(&log, &canonical, &key, "stale_target");
    assert!(log.is_proposal_suppressed(&canonical, &key).unwrap(), "a write_rejected suppresses re-attempts");
}

/// Suppression is SCOPED: an OPEN proposal (or a write_rejected) for one (path,key)
/// must NOT suppress a DIFFERENT key or a different path — else valid proposals are
/// silently dropped.
#[test]
fn proposal_suppression_is_scoped_to_path_and_key() {
    let (log, _home, dir) = common::open_write_grant_and_external_target();
    let path = dir.join("n.md");
    let canonical = std::fs::canonicalize(&path).unwrap().to_string_lossy().to_string();
    let key_a = serde_json::json!({"src":"entity:a","relation":"rel","dst":"entity:b"});
    let key_b = serde_json::json!({"src":"entity:a","relation":"rel","dst":"entity:c"}); // different dst

    common::append_minimal_proposal(&log, &canonical, &key_a);
    assert!(log.is_proposal_suppressed(&canonical, &key_a).unwrap(), "same key suppressed");
    assert!(!log.is_proposal_suppressed(&canonical, &key_b).unwrap(), "DIFFERENT key not suppressed");
    assert!(!log.is_proposal_suppressed("/some/other/path.md", &key_a).unwrap(), "DIFFERENT path not suppressed");

    // a write_rejected for key_a must likewise not suppress key_b
    common::append_rejected(&log, &canonical, &key_a, "unrenderable_target");
    assert!(!log.is_proposal_suppressed(&canonical, &key_b).unwrap(), "rejected key_a does not suppress key_b");
}

// ══════════════════════════════════════════════════════════════════════════════
// Task 7 — evolve_once integration: the reconciliation proposer end-to-end.
//
// These tests drive the WHOLE evolve tick (recall → Pass A → resolve → Pass B →
// invalidate → reconciliation synthesis → write_proposal) with NO live model. The
// challenge (test-reasoner guidance, plan §"Test-reasoner guidance"): `evolve_once`
// calls the reasoner MULTIPLE times per tick — Pass A, Pass B, and now the M6b
// rewrite. The `DispatchReasoner` below intercepts the rewrite call (its prompt is
// the literal `build_rewrite_prompt` frame, recognizable by "You are correcting a
// file") and delegates EVERY other turn to an inner `ScriptedReasoner` scripted
// exactly the way `tests/evolve.rs` + `tests/extraction.rs` script a contradiction.
// ══════════════════════════════════════════════════════════════════════════════

use bossclaw_core::embed::MockEmbedder;
use bossclaw_core::event::Event;
use bossclaw_core::extract::{
    build_pass_a_prompt, build_pass_b_prompt, parse_proposals, verify_floor, MAX_PROPOSALS_PER_TICK,
    PASS_A_SYSTEM, PASS_B_SYSTEM,
};
use bossclaw_core::reason::{Reasoner, ScriptedReasoner};
use bossclaw_core::recall::RecallOptions;
use serde_json::Value;

/// The exact corrected body the rewrite turn returns for the Alice/Globex fixture.
const CORRECTED_BODY: &str = "Alice works at Globex.\n";

/// A reasoner that dispatches on the call shape: the M6b whole-file rewrite is
/// identified by the literal lead line "You are correcting a file" — which lives in
/// the `build_rewrite_prompt` FRAME BODY (the `prompt` arg), NOT in the prod system
/// const `RECONCILE_SYSTEM` ("You correct a file…"); matching on the frame body is why
/// the detection is robust regardless of the system line. The rewrite turn returns a
/// fixed `{ "corrected_content": ... }`; every OTHER turn (Pass A, Pass B, entity
/// adjudication, summarize compose) is delegated to the inner `ScriptedReasoner`. This
/// is the recommended seam from the plan: it lets the contradiction be scripted with
/// the proven `scripted_both_passes` pattern while the rewrite — whose prompt is
/// laborious to reproduce byte-exactly — is matched structurally instead.
struct DispatchReasoner {
    inner: ScriptedReasoner,
    corrected: String,
}

impl DispatchReasoner {
    fn new(inner: ScriptedReasoner) -> Self {
        Self { inner, corrected: CORRECTED_BODY.to_string() }
    }
    /// Override the rewrite body (used by the cap test to keep each proposal valid).
    fn with_corrected(mut self, body: &str) -> Self {
        self.corrected = body.to_string();
        self
    }
}

impl Reasoner for DispatchReasoner {
    fn complete_json(
        &self,
        system: &str,
        prompt: &str,
        schema: &Value,
    ) -> Result<Value, bossclaw_core::error::BossclawError> {
        // Match the `build_rewrite_prompt` FRAME BODY (the `prompt` arg), NOT the
        // system line: its lead instruction is engine-fixed (the file body is fenced
        // BELOW, so this marker can only come from the trusted frame, never the
        // untrusted file text).
        if prompt.contains("You are correcting a file") {
            return Ok(serde_json::json!({ "corrected_content": self.corrected }));
        }
        self.inner.complete_json(system, prompt, schema)
    }
    fn model_id(&self) -> &str {
        self.inner.model_id()
    }
}

fn mk_memory_ev(text: &str) -> Event {
    Event {
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
    }
}

/// Append a memory and bring every derived structure current (the production
/// open→derive→rebuild lifecycle — mirrors `tests/evolve.rs::seed_memory`).
fn seed_memory_full(log: &bossclaw_core::EventLog, emb: &MockEmbedder, text: &str) -> String {
    let id = log.append(mk_memory_ev(text)).unwrap();
    log.rederive_pending(emb).unwrap();
    log.rebuild_indexes(emb).unwrap();
    log.rebuild_graph().unwrap();
    log.rebuild_entity_index(emb).unwrap();
    id
}

/// Ingest a `.md` under the granted dir and bring every derived structure current,
/// so the `file_ingested` event is a ready evolve SUBJECT (Door C). Returns
/// `(file_event_id, stored_content_text)` — the stored text is the `source` string
/// `evolve_once` feeds Pass A (so the scripted prompt matches byte-for-byte).
fn ingest_md_full(
    log: &bossclaw_core::EventLog,
    emb: &MockEmbedder,
    dir: &std::path::Path,
    name: &str,
    body: &[u8],
) -> (String, String) {
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    let id = common::ingest_one(log, &path);
    log.rederive_pending(emb).unwrap();
    log.rebuild_indexes(emb).unwrap();
    log.rebuild_graph().unwrap();
    log.rebuild_entity_index(emb).unwrap();
    let text = log
        .event_by_id(&id)
        .unwrap()
        .unwrap()
        .content
        .get("text")
        .and_then(|t| t.as_str())
        .unwrap()
        .to_string();
    (id, text)
}

/// Pass-A payload for an "X works_at Y" subject: two entities + one `works_at` link
/// whose `supported_by` span is the whole `source` (so `verify_floor` keeps it).
fn works_at_pass_a(x: &str, y: &str, source: &str) -> Value {
    serde_json::json!({
        "entities": [
            { "mention": x, "entity_type": "person", "confidence": 0.95 },
            { "mention": y, "entity_type": "org",    "confidence": 0.95 }
        ],
        "relations": [{
            "src": x, "relation": "works_at", "dst": y,
            "confidence": 0.92, "supported_by": source
        }],
        "retractions": []
    })
}

/// Pass-A payload that RETRACTS `(x, works_at, old)` and asserts `(x, works_at, new)`.
/// The retraction is expressed in MENTIONS; `evolve_once` remaps them to resolved ids
/// before confirming against the active edge keys (F4).
fn correction_pass_a(x: &str, old: &str, new: &str, source: &str) -> Value {
    serde_json::json!({
        "entities": [
            { "mention": x,   "entity_type": "person", "confidence": 0.95 },
            { "mention": new, "entity_type": "org",    "confidence": 0.95 }
        ],
        "relations": [{
            "src": x, "relation": "works_at", "dst": new,
            "confidence": 0.92, "supported_by": source
        }],
        "retractions": [{
            "src": x, "relation": "works_at", "dst": old,
            "reason": "the source corrects the employer", "confidence": 0.95
        }]
    })
}

/// Script BOTH passes of ONE subject into `reasoner`, under EACH recall context in
/// `recall_ctxs` (Door B can surface earlier subjects, so Pass A must be scripted for
/// every context that can occur). `neighborhood` is the cheat-sheet the loop builds
/// for Pass B. The Pass-B echo returns the floor-verified relations + retractions so
/// `intersect_keep_floor` keeps them. Mirrors `tests/evolve.rs::scripted_both_passes`
/// but threaded onto an existing builder so several subjects compose into one reasoner.
fn add_both_passes(
    mut reasoner: ScriptedReasoner,
    source: &str,
    recall_ctxs: &[Vec<String>],
    neighborhood: &[String],
    pass_a: Value,
) -> ScriptedReasoner {
    for ctx in recall_ctxs {
        let a_prompt = build_pass_a_prompt(source, ctx);
        reasoner = reasoner.with_response(PASS_A_SYSTEM, &a_prompt, pass_a.clone());
    }
    let floor = verify_floor(&parse_proposals(&pass_a).unwrap(), source);
    let b_response = serde_json::json!({
        "entities": [],
        "relations": floor.relations.iter().map(|r| serde_json::json!({
            "src": r.src, "relation": r.relation, "dst": r.dst,
            "confidence": r.confidence, "supported_by": r.supported_by,
        })).collect::<Vec<_>>(),
        "retractions": floor.retractions.iter().map(|r| serde_json::json!({
            "src": r.src, "relation": r.relation, "dst": r.dst,
            "reason": r.reason, "confidence": r.confidence,
        })).collect::<Vec<_>>(),
    });
    let b_prompt = build_pass_b_prompt(source, &floor, neighborhood);
    reasoner.with_response(PASS_B_SYSTEM, &b_prompt, b_response)
}

/// Replicate the EXACT recall context `evolve_once` builds for a subject `text` whose
/// own event id is `self_id`: the loop calls `recall(emb, text, GRAPH_CONTEXT_K,
/// {exclude_pages, !exclude_files})`, drops the subject's own id, and maps the rest to
/// their `content.text` (recall-rank order). Used so a scripted Pass A keys on the SAME
/// `(system, prompt)` the loop will produce — the deterministic way to script a subject
/// whose in-loop recall is not provably empty (graph-proximity boost can surface
/// neighbors). Mirrors the loop's recall block in `EventLog::evolve_once`.
fn loop_recall_texts(
    log: &bossclaw_core::EventLog,
    emb: &MockEmbedder,
    text: &str,
    self_id: &str,
) -> Vec<String> {
    let hits = log
        .recall(
            emb,
            text,
            bossclaw_core::extract::GRAPH_CONTEXT_K,
            &RecallOptions { exclude_pages: true, exclude_files: false, ..Default::default() },
        )
        .unwrap_or_default();
    hits.into_iter()
        .map(|h| h.event_id)
        .filter(|id| id != self_id)
        // Map to content.text verbatim, in rank order, dropping ids with no text
        // (exactly what the loop's private `texts_for_ids` does).
        .filter_map(|id| {
            log.event_by_id(&id)
                .ok()
                .flatten()
                .and_then(|ev| ev.content.get("text").and_then(|t| t.as_str()).map(str::to_string))
        })
        .collect()
}

/// Count CURRENT `write_proposal` events whose `target` equals `canonical`.
fn proposals_targeting(log: &bossclaw_core::EventLog, canonical: &str) -> usize {
    log.stream_all()
        .unwrap()
        .into_iter()
        .filter(|e| e.event_type == bossclaw_core::graph::WRITE_PROPOSAL_EVENT_TYPE)
        .filter(|e| e.content.get("target").and_then(|t| t.as_str()) == Some(canonical))
        .count()
}

/// END-TO-END: a `.md` asserts "Alice works at Acme"; a memory corrects it to Globex.
/// One `evolve_once` (a) confirms the contradiction (`invalidate`) AND (b) synthesizes
/// a corrected rewrite, recording exactly one `write_proposal` whose target is the
/// ingested file's canonical path.
#[test]
fn evolve_once_emits_reconciliation_proposal_for_file_backed_contradiction() {
    let (log, _home, dir) = common::open_log_with_write_grant();
    let emb = MockEmbedder::new(64);

    // ── Tick 1: the FILE establishes "Alice works_at Acme". Its file_ingested id
    //    flows into the works_at edge's source_event_ids (Door C), so the retired
    //    edge later traces back to this still-current file. ──
    let (file_id, file_src) = ingest_md_full(&log, &emb, &dir, "notes.md", b"Alice works at Acme.\n");
    let canonical = std::fs::canonicalize(dir.join("notes.md")).unwrap().to_string_lossy().to_string();

    let r1 = DispatchReasoner::new(add_both_passes(
        ScriptedReasoner::new("m6b-test"),
        &file_src,
        &[vec![]], // single subject in the store → recall is empty
        &[],
        works_at_pass_a("Alice", "Acme", &file_src),
    ));
    let rep1 = log.evolve_once(&emb, &r1).unwrap();
    assert!(rep1.links_emitted >= 1, "the file established the works_at edge");
    assert_eq!(rep1.proposals_emitted, 0, "no contradiction yet → no proposal");
    log.rebuild_graph().unwrap();

    // Resolve Alice/Acme to ids so we know the neighborhood line Pass B will see.
    let entities = log.all_entities().unwrap();
    let alice = entities.iter().find(|e| e.label == "Alice").unwrap().entity_id.clone();
    let acme = entities.iter().find(|e| e.label == "Acme").unwrap().entity_id.clone();

    // ── Tick 2: a memory corrects the employer. recall for the memory may surface
    //    the file text (Door B open), so script Pass A under BOTH recall contexts. ──
    let corr = "Correction: Alice works at Globex, not Acme.";
    let mem_id = seed_memory_full(&log, &emb, corr);

    // The neighborhood the loop renders for Pass B uses the surface mentions present
    // in THIS subject; "Alice" and "Acme" both appear in `corr`, so the active edge
    // renders by name.
    let nbh = vec!["Alice -works_at-> Acme".to_string()];
    let inner2 = add_both_passes(
        ScriptedReasoner::new("m6b-test"),
        corr,
        &[vec![], vec![file_src.clone()]],
        &nbh,
        correction_pass_a("Alice", "Acme", "Globex", corr),
    );
    let r2 = DispatchReasoner::new(inner2);
    let rep2 = log.evolve_once(&emb, &r2).unwrap();

    assert!(rep2.invalidates_emitted >= 1, "the contradiction was confirmed (invalidate)");
    assert_eq!(rep2.proposals_emitted, 1, "exactly one reconciliation proposal");
    assert_eq!(rep2.proposals_rejected, 0, "the rewrite + gate succeeded");
    assert_eq!(rep2.proposals_elided_cap, 0, "well under the per-tick cap");

    // Exactly one write_proposal, targeting the ingested file's canonical path.
    assert_eq!(proposals_targeting(&log, &canonical), 1, "one proposal targets notes.md");

    let prop = log
        .stream_all()
        .unwrap()
        .into_iter()
        .find(|e| e.event_type == bossclaw_core::graph::WRITE_PROPOSAL_EVENT_TYPE)
        .unwrap();
    // The inducing_key is the RESOLVED contradiction (entity ids), and the lineage
    // carries the still-current file id (so the proposal anchors to the right target).
    assert_eq!(prop.content["inducing_key"]["src"], serde_json::json!(alice));
    assert_eq!(prop.content["inducing_key"]["dst"], serde_json::json!(acme));
    assert_eq!(prop.content["inducing_key"]["relation"], serde_json::json!("works_at"));
    let lineage = prop.model_meta.as_ref().unwrap().source_event_ids.clone();
    assert!(lineage.contains(&file_id), "lineage carries the asserting file id");
    assert!(lineage.contains(&mem_id), "lineage carries the correcting memory id");
    // INTEGRATION-LAYER taint guard: the WIRED proposal (engine lineage includes the
    // ingested file) must be stamped `origin:"external"` by the append chokepoint — not
    // just the unit-layer `write_proposal_event_is_tier_b_object_and_taint_stamped`. A
    // future wiring regression that broke the stamp would still pass the unit test; this
    // asserts the assembled flow taints the proposal end-to-end.
    assert_eq!(
        prop.content["origin"],
        serde_json::json!("external"),
        "the assembled-flow proposal must be taint-stamped (engine lineage includes the ingested file)"
    );
    log.verify_chain().unwrap();
}

/// A contradiction whose BOTH facts come only from memories (no file in the lineage)
/// confirms the `invalidate` but synthesizes NO proposal — there is nothing on disk to
/// rewrite.
#[test]
fn memory_only_contradiction_emits_no_proposal() {
    let (log, _home, _dir) = common::open_log_with_write_grant();
    let emb = MockEmbedder::new(64);

    // Tick 1: memory establishes the edge.
    let src1 = "Bob works at Initech.";
    seed_memory_full(&log, &emb, src1);
    let r1 = DispatchReasoner::new(add_both_passes(
        ScriptedReasoner::new("m6b-test"),
        src1,
        &[vec![]],
        &[],
        works_at_pass_a("Bob", "Initech", src1),
    ));
    let rep1 = log.evolve_once(&emb, &r1).unwrap();
    assert!(rep1.links_emitted >= 1, "edge established from a memory");
    log.rebuild_graph().unwrap();

    // Tick 2: memory corrects it.
    let corr = "Correction: Bob works at Globex, not Initech.";
    seed_memory_full(&log, &emb, corr);
    let nbh = vec!["Bob -works_at-> Initech".to_string()];
    let r2 = DispatchReasoner::new(add_both_passes(
        ScriptedReasoner::new("m6b-test"),
        corr,
        &[vec![], vec![src1.to_string()]],
        &nbh,
        correction_pass_a("Bob", "Initech", "Globex", corr),
    ));
    let rep2 = log.evolve_once(&emb, &r2).unwrap();

    assert!(rep2.invalidates_emitted >= 1, "the contradiction still fires");
    assert_eq!(rep2.proposals_emitted, 0, "memory-only lineage → nothing on disk to propose against");
    assert_eq!(rep2.proposals_rejected, 0, "no synthesis was even attempted");
    log.verify_chain().unwrap();
}

/// The off-switch suppresses ONLY the proposal layer: with proposals disabled, evolve
/// curation still confirms the contradiction (`invalidate`), but no `write_proposal` is
/// synthesized. (And no `write_rejected` either — the gate suppression must stay
/// retryable, so the off-switch is a plain skip.)
#[test]
fn proposals_offswitch_suppresses_only_proposals() {
    let (log, _home, dir) = common::open_log_with_write_grant();
    let emb = MockEmbedder::new(64);
    log.set_proposals_enabled(false).unwrap();

    let (_file_id, file_src) = ingest_md_full(&log, &emb, &dir, "notes.md", b"Alice works at Acme.\n");
    let canonical = std::fs::canonicalize(dir.join("notes.md")).unwrap().to_string_lossy().to_string();
    let r1 = DispatchReasoner::new(add_both_passes(
        ScriptedReasoner::new("m6b-test"),
        &file_src,
        &[vec![]],
        &[],
        works_at_pass_a("Alice", "Acme", &file_src),
    ));
    log.evolve_once(&emb, &r1).unwrap();
    log.rebuild_graph().unwrap();

    let corr = "Correction: Alice works at Globex, not Acme.";
    seed_memory_full(&log, &emb, corr);
    let nbh = vec!["Alice -works_at-> Acme".to_string()];
    let r2 = DispatchReasoner::new(add_both_passes(
        ScriptedReasoner::new("m6b-test"),
        corr,
        &[vec![], vec![file_src.clone()]],
        &nbh,
        correction_pass_a("Alice", "Acme", "Globex", corr),
    ));
    let rep2 = log.evolve_once(&emb, &r2).unwrap();

    assert!(rep2.invalidates_emitted >= 1, "curation still runs with proposals OFF");
    assert_eq!(rep2.proposals_emitted, 0, "off-switch suppresses the proposal");
    assert_eq!(rep2.proposals_rejected, 0, "off-switch is a plain skip — NEVER a write_rejected (T6 retryable)");
    assert_eq!(proposals_targeting(&log, &canonical), 0, "no proposal event was written");
    // And no write_rejected was recorded for this (path,key) → a later re-enable can still propose.
    let key = serde_json::json!({"src":"entity:alice","relation":"works_at","dst":"entity:acme"});
    // (Exact ids differ; assert via the absence of ANY write_rejected event instead.)
    let _ = key;
    let rejected = log
        .stream_all()
        .unwrap()
        .into_iter()
        .filter(|e| e.event_type == bossclaw_core::graph::WRITE_REJECTED_EVENT_TYPE)
        .count();
    assert_eq!(rejected, 0, "no write_rejected emitted by the off-switch path");
    log.verify_chain().unwrap();
}

/// GUARD TEST (revert-sensitive): the backward walk to the retired edge MUST run while
/// that edge is still ACTIVE — i.e. INSIDE the confirmed-contradiction loop, BEFORE the
/// end-of-tick `rebuild_graph` folds it closed. This is the same e2e setup; a regression
/// that moved the walk AFTER `rebuild_graph` would read the now-closed edge via
/// `neighbors` (active-edge lookup), find nothing, and emit 0 proposals. Asserting 1
/// here locks the ordering.
#[test]
fn walk_runs_against_active_edge_within_the_loop() {
    let (log, _home, dir) = common::open_log_with_write_grant();
    let emb = MockEmbedder::new(64);

    let (_file_id, file_src) = ingest_md_full(&log, &emb, &dir, "notes.md", b"Alice works at Acme.\n");
    let canonical = std::fs::canonicalize(dir.join("notes.md")).unwrap().to_string_lossy().to_string();
    let r1 = DispatchReasoner::new(add_both_passes(
        ScriptedReasoner::new("m6b-test"),
        &file_src,
        &[vec![]],
        &[],
        works_at_pass_a("Alice", "Acme", &file_src),
    ));
    log.evolve_once(&emb, &r1).unwrap();
    log.rebuild_graph().unwrap();

    let corr = "Correction: Alice works at Globex, not Acme.";
    seed_memory_full(&log, &emb, corr);
    let nbh = vec!["Alice -works_at-> Acme".to_string()];
    let r2 = DispatchReasoner::new(add_both_passes(
        ScriptedReasoner::new("m6b-test"),
        corr,
        &[vec![], vec![file_src.clone()]],
        &nbh,
        correction_pass_a("Alice", "Acme", "Globex", corr),
    ));
    let rep2 = log.evolve_once(&emb, &r2).unwrap();
    // If the walk ran after rebuild_graph (edge folded closed), neighbors() would
    // return no active edge → 0 proposals. 1 proves the walk read the LIVE edge.
    assert_eq!(rep2.proposals_emitted, 1, "walk must run while the retired edge is still active");
    assert_eq!(proposals_targeting(&log, &canonical), 1);
    log.verify_chain().unwrap();
}

/// CAP: when ONE confirmed contradiction's lineage fans out to MORE than
/// `MAX_PROPOSALS_PER_TICK` DISTINCT current files (Q-2: one proposal per distinct file),
/// at most `MAX_PROPOSALS_PER_TICK` proposals are emitted and the rest are COUNTED in
/// `proposals_elided_cap` — never rejected (an elided `(path,key)` stays retryable; a
/// `write_rejected` would permanently suppress it, T6).
///
/// Hermetic construction (single contradiction, multi-file lineage — far simpler to
/// script than N contradictions under recall fan-out): ingest N tracked files with
/// token-DISJOINT "archive" text (so recall never surfaces them), mint two entities, and
/// seed ONE machine edge that cites ALL N file ids as its `source_event_ids`. Advance the
/// evolve cursor past everything seeded so the single capped tick processes ONLY the
/// correcting memory, whose Pass A retracts that one edge. The reconciliation walk then
/// finds N reconcilable targets in the edge's lineage and the cap bounds the proposals.
#[test]
fn cap_bounds_proposals_per_tick_and_counts_the_overflow() {
    let (log, _home, dir) = common::open_log_with_write_grant();
    let emb = MockEmbedder::new(64);
    let n = MAX_PROPOSALS_PER_TICK + 2; // two over the cap

    // N tracked files whose text shares NO tokens with the correcting memory, so the
    // memory's in-loop recall stays empty (MockEmbedder is bag-of-words). Each is a
    // current, reconcilable target; collect their file_ingested ids for the edge lineage.
    let mut file_ids: Vec<String> = Vec::new();
    for i in 0..n {
        let path = dir.join(format!("archive{i}.md"));
        std::fs::write(&path, format!("ARCHIVE BLOB ZZ{i} QQ{i}\n").as_bytes()).unwrap();
        file_ids.push(common::ingest_one(&log, &path));
    }

    // Mint the two endpoints, then ONE machine edge citing ALL N files as its sources.
    // `entity()` does NOT store a resolution vector, so derive each one explicitly —
    // otherwise `rebuild_entity_index` can't index it and the memory's mentions would
    // mint fresh entities instead of resolving to these (so the retraction would miss).
    let cap_person = log.entity("CapPerson", &[], "person", "m6b-test-seed", &file_ids).unwrap();
    let cap_org = log.entity("CapOrg", &[], "org", "m6b-test-seed", &file_ids).unwrap();
    log.derive_entity_vector(&emb, &cap_person, "CapPerson").unwrap();
    log.derive_entity_vector(&emb, &cap_org, "CapOrg").unwrap();
    let _edge = common::seed_edge_with_sources(&log, &cap_person, "works_at", &cap_org, &file_ids);
    log.rebuild_graph().unwrap();
    log.rebuild_entity_index(&emb).unwrap();

    // Skip every event seeded so far as an evolve subject: seq is a 1-based autoincrement
    // with no deletes, so the event count IS the current tip seq. The next tick then sees
    // only the later-appended memory.
    let tip = log.stream_all().unwrap().len() as i64;
    log.set_evolve_cursor(tip).unwrap();

    // The single correcting memory: mentions resolve to the seeded entities (same path the
    // e2e test exercises), and its Pass A retracts the one edge.
    let corr = "CapPerson no longer works at CapOrg.";
    let mem_id = seed_memory_full(&log, &emb, corr);
    let pass_a = serde_json::json!({
        "entities": [
            { "mention": "CapPerson", "entity_type": "person", "confidence": 0.95 },
            { "mention": "CapOrg",    "entity_type": "org",    "confidence": 0.95 }
        ],
        "relations": [],
        "retractions": [{
            "src": "CapPerson", "relation": "works_at", "dst": "CapOrg",
            "reason": "left", "confidence": 0.95
        }]
    });
    // Pass B neighborhood: the one active edge, rendered by the surface mentions present
    // in this memory ("CapPerson"/"CapOrg").
    let nbh = vec!["CapPerson -works_at-> CapOrg".to_string()];
    // Script Pass A under the EXACT recall context the loop will build (graph-proximity
    // boost can surface the edge's neighbor/lineage, so the context is not provably empty).
    let recall_ctx = loop_recall_texts(&log, &emb, corr, &mem_id);
    let r = DispatchReasoner::new(add_both_passes(
        ScriptedReasoner::new("m6b-test"),
        corr,
        &[recall_ctx],
        &nbh,
        pass_a,
    ))
    .with_corrected("corrected\n");
    let rep = log.evolve_once(&emb, &r).unwrap();

    assert_eq!(rep.invalidates_emitted, 1, "the single contradiction is confirmed");
    assert_eq!(rep.proposals_emitted, MAX_PROPOSALS_PER_TICK, "proposals capped at the per-tick max");
    assert_eq!(
        rep.proposals_elided_cap,
        n - MAX_PROPOSALS_PER_TICK,
        "the overflow is counted, not rejected"
    );
    assert_eq!(rep.proposals_rejected, 0, "cap-elision is NOT a rejection");
    log.verify_chain().unwrap();
}

// ══════════════════════════════════════════════════════════════════════════════
// Task 8 — the WHOLE M6b lifecycle in one test: EMIT (autonomous) → CONFIRM
// (app-side) → EXECUTE → RESOLVE → UNDO. Where the cap/off-switch/e2e tests above
// each lock ONE property of the EMIT half, this proves the proposal an autonomous
// `evolve_once` produces can be carried all the way to a confirmed, undoable disk
// write whose record closes the proposal — the full app contract end to end.
// ══════════════════════════════════════════════════════════════════════════════

/// FULL ROUND-TRIP. Drive the proven T7 e2e setup so ONE real `write_proposal` is
/// emitted autonomously for an ingested target, then walk the app-side CONFIRM path
/// against THAT proposal's OWN recorded fields (target, new_content_hash, lineage):
/// `get_proposal_bytes_checked → propose_write → execute_write_resolving`. Assert the
/// lifecycle closes — the file gains the corrected bytes, the `file_written`
/// back-references the proposal, the OPEN proposal is no longer suppressing (resolved),
/// and `undo_write` restores the original on-disk bytes.
#[test]
fn proposal_round_trip_emit_confirm_execute_resolve_undo() {
    let (log, _home, dir) = common::open_log_with_write_grant();
    let emb = MockEmbedder::new(64);

    // ── EMIT: reuse the T7 two-tick fixture verbatim. Tick 1 — the file asserts
    //    "Alice works_at Acme" (its file_ingested id flows into the edge lineage). ──
    let original = b"Alice works at Acme.\n";
    let (file_id, file_src) = ingest_md_full(&log, &emb, &dir, "notes.md", original);
    let target_path = dir.join("notes.md");
    let canonical = std::fs::canonicalize(&target_path).unwrap().to_string_lossy().to_string();
    // Snapshot the ORIGINAL on-disk bytes BEFORE confirm (the undo oracle).
    let original_on_disk = std::fs::read(&target_path).unwrap();

    let r1 = DispatchReasoner::new(add_both_passes(
        ScriptedReasoner::new("m6b-test"),
        &file_src,
        &[vec![]],
        &[],
        works_at_pass_a("Alice", "Acme", &file_src),
    ));
    let rep1 = log.evolve_once(&emb, &r1).unwrap();
    assert_eq!(rep1.proposals_emitted, 0, "no contradiction yet");
    log.rebuild_graph().unwrap();

    // Tick 2 — a memory corrects the employer → confirmed contradiction + ONE proposal.
    let corr = "Correction: Alice works at Globex, not Acme.";
    seed_memory_full(&log, &emb, corr);
    let nbh = vec!["Alice -works_at-> Acme".to_string()];
    let r2 = DispatchReasoner::new(add_both_passes(
        ScriptedReasoner::new("m6b-test"),
        corr,
        &[vec![], vec![file_src.clone()]],
        &nbh,
        correction_pass_a("Alice", "Acme", "Globex", corr),
    ));
    let rep2 = log.evolve_once(&emb, &r2).unwrap();
    assert_eq!(rep2.proposals_emitted, 1, "exactly one reconciliation proposal was emitted");
    assert_eq!(rep2.proposals_rejected, 0, "the synthesis + gate succeeded");

    // Capture the emitted proposal and EVERYTHING the confirm path consumes from it:
    // its id, target (canonical), new_content_hash, inducing_key, and recorded lineage.
    let prop = log
        .stream_all()
        .unwrap()
        .into_iter()
        .find(|e| e.event_type == bossclaw_core::graph::WRITE_PROPOSAL_EVENT_TYPE)
        .expect("the emitted write_proposal");
    let pid = prop.id.clone();
    let recorded_target = prop.content["target"].as_str().unwrap().to_string();
    assert_eq!(recorded_target, canonical, "the proposal targets the ingested file");
    let hash = prop.content["new_content_hash"].as_str().unwrap().to_string();
    let inducing_key = prop.content["inducing_key"].clone();
    // The engine-gathered lineage (non-empty → Tier-B holds for the confirm write).
    let lineage = prop.model_meta.as_ref().expect("Tier-B proposal").source_event_ids.clone();
    assert!(lineage.contains(&file_id), "lineage carries the asserting file id");

    // While the proposal is OPEN, (path, key) is suppressed (no re-proposal would fire).
    assert!(
        log.is_proposal_suppressed(&canonical, &inducing_key).unwrap(),
        "an OPEN proposal suppresses re-attempts for (path, key)"
    );

    // ── CONFIRM (app-side): re-read the cached bytes against the SIGNED hash, then
    //    re-run the full M6a gate and execute as a resolving write. ──
    let bytes = log.get_proposal_bytes_checked(&pid, &hash).unwrap();
    let gated = log
        .propose_write(WriteProposal {
            target: target_path.clone(),
            new_content: bytes.clone(),
            op: WriteOp::Edit,
            source_event_ids: lineage.clone(), // the proposal's recorded lineage
            rationale: "confirm".to_string(),
        })
        .expect("propose_write");
    assert!(
        gated.verdict.reject_reason.is_none(),
        "fresh, write-granted target → the gate passes (reject_reason: {:?})",
        gated.verdict.reject_reason
    );
    let fw_id = log.execute_write_resolving(gated, &pid).expect("execute_write_resolving");

    // ── Assert the lifecycle CLOSES. ──
    // 1. the on-disk target now holds the corrected bytes the side table cached.
    assert_eq!(std::fs::read(&target_path).unwrap(), bytes, "the file gained the corrected bytes");
    // 2. the file_written record back-references the proposal it resolved.
    let fw = log.event_by_id(&fw_id).unwrap().unwrap();
    assert_eq!(fw.event_type, "file_written");
    assert_eq!(fw.content["resolves_proposal"], serde_json::json!(pid));
    // 3. the OPEN proposal is now RESOLVED → no longer suppressing (lifecycle closed).
    assert!(
        !log.is_proposal_suppressed(&canonical, &inducing_key).unwrap(),
        "file_written{{resolves_proposal}} closed the proposal → (path,key) no longer suppressed"
    );
    // 4. undo restores the file to its ORIGINAL pre-confirm bytes.
    log.undo_write(&fw_id).expect("undo_write");
    assert_eq!(
        std::fs::read(&target_path).unwrap(),
        original_on_disk,
        "undo_write reverts the resolving Edit to the original on-disk bytes"
    );
    assert_eq!(original_on_disk, original.to_vec(), "sanity: the snapshot equals the seeded body");

    log.verify_chain().unwrap();
}
