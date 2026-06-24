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

mod common;

use bossclaw_core::error::BossclawError;
use bossclaw_core::reason::Reasoner;
use bossclaw_core::{EventLog, MandateAction, WriteOp, MAX_RECIPE_LEN};
use ed25519_dalek::SigningKey;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
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

// M6c §5.5 / D8: the mandate proposer's INDEPENDENT kill switch is default-open,
// sticky-off, and fail-closed — a direct mirror of `proposals_enabled`.
#[test]
fn mandates_enabled_sticky_default_open() {
    let (log, _tmp) = setup();
    assert!(log.mandates_enabled().unwrap()); // default-open
    log.set_mandates_enabled(false).unwrap();
    assert!(!log.mandates_enabled().unwrap()); // sticky off
    log.set_mandates_enabled(true).unwrap();
    assert!(log.mandates_enabled().unwrap()); // later explicit true re-enables
}

// §5.6 / finding C: a proposal recorded through the M6c path stamps its OWN producer
// (`m6c-mandate-proposer`), NOT the hardcoded M6b `m6b-reconciler`. The event shape is
// otherwise unchanged — only `model_meta.model_id` is parameterized. Task 9's per-mandate
// cap relies on this stamp to distinguish mandate proposals from reconciler proposals.
#[test]
fn m6c_proposal_records_its_own_producer() {
    let (log, _tmp) = setup();
    let id = log
        .append_write_proposal_with(
            "out/i.md",
            "edit",
            "deadbeef",
            3,
            "sync",
            &json!({"mandate": "M1", "target": "out/i.md", "sources_hash": "h"}),
            &json!({}),
            &["M1".to_string()],
            bossclaw_core::graph::M6C_PROPOSER_PRODUCER,
        )
        .unwrap();
    let ev = log.event_by_id(&id).unwrap().expect("proposal event readable by id");
    assert_eq!(
        ev.model_meta.expect("Tier-B").model_id,
        "m6c-mandate-proposer",
        "M6c-path proposal must stamp its own producer, not m6b-reconciler"
    );
}

// TASK 7 — synthesis cache round-trip: the stored bytes AND the synth-time lineage
// (Finding B: the exact source ids read at synthesis time) come back together in one row.
#[test]
fn cache_roundtrip_returns_bytes_and_synth_lineage() {
    let (log, _tmp) = setup();
    log.put_synthesis_cache("M1", "srchash", b"INDEX", "h_expected", &["s1".into(), "s2".into()])
        .unwrap();
    let hit = log.get_synthesis_cache("M1", "srchash").unwrap().unwrap();
    assert_eq!(hit.expected_bytes, b"INDEX");
    assert_eq!(hit.expected_hash, "h_expected");
    assert_eq!(
        hit.source_event_ids_at_synth,
        vec!["s1".to_string(), "s2".into()]
    );
}

// TASK 7 — bounded growth + lifecycle: a write at a NEW source-state evicts prior
// source-states for the mandate (Finding F), and revoking the mandate purges all its rows.
#[test]
fn cache_write_evicts_prior_states_and_revoke_purges() {
    let (log, _tmp) = setup();
    log.put_synthesis_cache("M1", "old", b"A", "ha", &["s1".into()]).unwrap();
    log.put_synthesis_cache("M1", "new", b"B", "hb", &["s1".into()]).unwrap(); // evicts "old"
    assert!(log.get_synthesis_cache("M1", "old").unwrap().is_none());
    assert!(log.get_synthesis_cache("M1", "new").unwrap().is_some());
    log.revoke_mandate("M1").ok(); // purges all M1 rows
    assert!(log.get_synthesis_cache("M1", "new").unwrap().is_none());
}

// TASK 8 — decline-stickiness (spec §5.4): a sync mandate's "stale file" condition
// PERSISTS after a decline (the file is still out of sync), so M6b's predicate — which
// lets a declined proposal re-fire — would re-nag every tick. M6c keys suppression on the
// SOURCE-STATE (`inducing_key` = {mandate, target, sources_hash}) and treats a decline as
// sticky FOR THAT source-state. A NEW source-state (different `sources_hash`) is a fresh
// ask. This is the ONLY behavioural delta from `is_proposal_suppressed`; M6b stays green.
#[test]
fn declined_sync_is_sticky_for_that_source_state() {
    let (log, _tmp) = setup();
    let key = json!({"mandate":"M1","target":"out/i.md","sources_hash":"S1"});
    assert!(!log.is_mandate_proposal_suppressed("out/i.md", &key).unwrap()); // fresh: not suppressed
    let pid = log
        .append_write_proposal_with(
            "out/i.md",
            "edit",
            "h",
            1,
            "r",
            &key,
            &json!({}),
            &["M1".into()],
            bossclaw_core::graph::M6C_PROPOSER_PRODUCER,
        )
        .unwrap();
    log.decline_write_proposal(&pid, "not now").unwrap();
    assert!(log.is_mandate_proposal_suppressed("out/i.md", &key).unwrap()); // declined → STICKY (no re-nag)
    let key2 = json!({"mandate":"M1","target":"out/i.md","sources_hash":"S2"});
    assert!(!log.is_mandate_proposal_suppressed("out/i.md", &key2).unwrap()); // a new source-state is a fresh ask
}

// ════════════════════════════════════════════════════════════════════════════════
// TASK 9a — `mandate_phase_for`: the gather + cached-or-synth DECISION half.
//
// These tests drive ONLY the gather/synth decision (`MandateAction`): no event is
// appended and `propose_write` is NOT called (that is Task 9b). The reasoner is
// faked two ways, mirroring `tests/reconcile.rs`'s `DispatchReasoner`:
//   - `CountingReasoner` PANICS if the synthesis turn ever fires — it proves the
//     elide / cache-hit paths reach NO LLM call (and counts other turns harmlessly).
//   - `SyncReasoner` answers the synthesis turn with a fixed `{ "synced_content": .. }`
//     keyed structurally on the `build_recipe_prompt` frame lead line ("You are
//     synthesizing synced content"), exactly as the reconcile dispatcher keys on
//     "You are correcting a file".
// ════════════════════════════════════════════════════════════════════════════════

/// The engine-fixed lead line of `mandate::build_recipe_prompt`'s TRUSTED frame.
/// It can only originate above the source fence, so matching on it is robust against
/// any (fenced, untrusted) source byte — the same discipline the reconcile dispatcher
/// uses for the rewrite turn. Kept in ONE place so the tests can't drift from the frame.
const RECIPE_FRAME_LEAD: &str = "You are synthesizing synced content";

/// A reasoner that must NEVER be asked to synthesize. Any synthesis turn (recognized
/// by the `build_recipe_prompt` frame lead) is a hard test failure — this is how the
/// elide-before-LLM and cache-hit paths assert "the model was not called". A counter
/// records how many synthesis turns fired (0 is the contract); non-synthesis turns
/// are answered with an empty object so an unrelated call would not panic.
struct CountingReasoner {
    synth_calls: AtomicUsize,
}

impl CountingReasoner {
    fn new() -> Self {
        Self { synth_calls: AtomicUsize::new(0) }
    }
    fn synth_calls(&self) -> usize {
        self.synth_calls.load(Ordering::Relaxed)
    }
}

impl Reasoner for CountingReasoner {
    fn complete_json(
        &self,
        _system: &str,
        prompt: &str,
        _schema: &Value,
    ) -> Result<Value, BossclawError> {
        if prompt.contains(RECIPE_FRAME_LEAD) {
            self.synth_calls.fetch_add(1, Ordering::Relaxed);
            panic!("synthesis reasoner invoked, but this path must reach NO LLM call");
        }
        Ok(serde_json::json!({}))
    }
    fn model_id(&self) -> &str {
        "m6c-counting-test"
    }
}

/// A reasoner that answers the synthesis turn with a fixed `synced_content`, matched
/// structurally on the `build_recipe_prompt` frame lead (NOT the system line). Mirrors
/// `tests/reconcile.rs::DispatchReasoner`: the laborious-to-reproduce prompt is matched
/// by its engine-fixed frame marker instead of scripted byte-for-byte.
struct SyncReasoner {
    synced: String,
}

impl SyncReasoner {
    fn new(synced: &str) -> Self {
        Self { synced: synced.to_string() }
    }
}

impl Reasoner for SyncReasoner {
    fn complete_json(
        &self,
        _system: &str,
        prompt: &str,
        _schema: &Value,
    ) -> Result<Value, BossclawError> {
        if prompt.contains(RECIPE_FRAME_LEAD) {
            return Ok(serde_json::json!({ "synced_content": self.synced }));
        }
        // No other turn is expected on this path; a scripted-miss is a loud test bug.
        panic!("unexpected non-synthesis reasoner turn");
    }
    fn model_id(&self) -> &str {
        "m6c-sync-test"
    }
}

/// Write `body` into `name` under the READ-granted `src` dir and ingest it, returning
/// its `file_ingested` event id (the engine-gathered lineage id `mandate_phase_for`
/// must use). Reuses the hermetic `common::ingest_one` (native UTF-8 parser path).
fn ingest_source(log: &EventLog, src: &Path, name: &str, body: &[u8]) -> String {
    let path = src.join(name);
    std::fs::write(&path, body).expect("write source file");
    common::ingest_one(log, &path)
}

/// Open a log with a read-granted `src` and a write-granted `out` (out OUTSIDE the read
/// root, satisfying the Finding-A self-loop guard), and return `(log, tmp, src, out)`.
/// The shared mandate-test scaffolding (`setup`/`scoped_dirs`) wired for the phase tests.
fn phase_log() -> (EventLog, TempDir, PathBuf, PathBuf) {
    let (log, tmp) = setup();
    let (src, out) = scoped_dirs(&tmp);
    read_grant(&log, &src);
    write_grant(&log, &out);
    (log, tmp, src, out)
}

// 9a — empty source set: a mandate whose source scope has NO ingested files must
// `Elide` and reach NO LLM call (finding G: never synthesize from nothing).
#[test]
fn empty_source_set_elides() {
    let (log, _tmp, src, out) = phase_log();
    let target = out.join("index.md");
    let id = log.add_mandate(&target, &src, "index the titles").unwrap();
    let m = &log.active_mandates().unwrap()[0];
    assert_eq!(m.mandate_grant_id, id);

    let reasoner = CountingReasoner::new();
    let action = log.mandate_phase_for(m, &reasoner).unwrap();
    assert!(matches!(action, MandateAction::Elide), "no sources → Elide");
    assert!(reasoner.synth_calls() == 0, "the reasoner must NOT be called");
}

// 9a — over-cap sources: when the combined fenced source block exceeds
// MAX_INPUT_TEXT_BYTES the phase must `Elide` (finding E: surface nothing, stay
// retryable) — NEVER a Propose carrying truncated content.
#[test]
fn over_cap_sources_elide_not_truncate() {
    let (log, _tmp, src, out) = phase_log();
    // One source already larger than the reasoner input cap; fencing only adds to it,
    // so the combined fenced length is guaranteed over the cap.
    let big = "x".repeat(bossclaw_core::extract::MAX_INPUT_TEXT_BYTES + 1);
    let big_id = ingest_source(&log, &src, "big.md", big.as_bytes());
    let target = out.join("index.md");
    let id = log.add_mandate(&target, &src, "index it").unwrap();
    let m = &log.active_mandates().unwrap()[0];
    assert_eq!(m.mandate_grant_id, id);
    // Guard against a false-positive Elide: the big source IS in scope (so the elide we
    // assert below is the OVER-CAP elide, not the empty-source one).
    assert!(
        log.current_files().unwrap().iter().any(|r| r.file_event_id == big_id),
        "the oversized source must be ingested + in scope"
    );

    let reasoner = CountingReasoner::new();
    let action = log.mandate_phase_for(m, &reasoner).unwrap();
    assert!(matches!(action, MandateAction::Elide), "over-cap → Elide, not a truncated Propose");
    assert!(reasoner.synth_calls() == 0, "over-cap elides BEFORE any LLM call");
}

/// Recompute the engine's `sources_hash` for the single ingested source `sid`: sha256
/// over the sorted (canonical_path, content_hash) pairs (trivially one entry). Matches
/// `mandate_phase_for`'s rule byte-for-byte so a test can assert what is (or is NOT)
/// cached. The projection's `content_hash` is captured at ingest, so this stays valid
/// even after the file is later deleted or overwritten on disk (no re-ingest happened).
fn sources_hash_for(log: &EventLog, sid: &str) -> String {
    use sha2::{Digest, Sha256};
    let rec = log
        .current_files()
        .unwrap()
        .into_iter()
        .find(|r| r.file_event_id == sid)
        .expect("the ingested source is still in the files projection");
    let mut hasher = Sha256::new();
    hasher.update(rec.canonical_path.as_bytes());
    hasher.update([0x00]);
    hasher.update(rec.content_hash.as_bytes());
    hasher.update([0x00]);
    hex::encode(hasher.finalize())
}

// 9a — FAIL CLOSED on a read-Err in-scope source. The `files` projection still LISTS the
// source, but `fs::read` returns ENOENT (the file was deleted on disk). The phase must
// `Elide` (NOT synthesize a partial view, NOT cache anything) so it stays retryable once
// the source is readable again — caching partial bytes under the full-set `sources_hash`
// would be permanently stale (the source-state, hence the hash, is unchanged).
#[test]
fn unreadable_source_fails_closed_to_elide() {
    let (log, _tmp, src, out) = phase_log();
    let sid = ingest_source(&log, &src, "a.md", b"Alpha\n");
    let target = out.join("index.md");
    let id = log.add_mandate(&target, &src, "index the titles").unwrap();
    let m = &log.active_mandates().unwrap()[0];
    // Capture the hash BEFORE deleting (projection content_hash is stable post-delete).
    let sources_hash = sources_hash_for(&log, &sid);
    // Delete the on-disk file: the projection still lists it, but `fs::read` → ENOENT.
    std::fs::remove_file(src.join("a.md")).unwrap();

    let reasoner = CountingReasoner::new();
    let action = log.mandate_phase_for(m, &reasoner).unwrap();
    assert!(matches!(action, MandateAction::Elide), "unreadable in-scope source → Elide");
    assert!(reasoner.synth_calls() == 0, "fail-closed elide reaches NO LLM call");
    assert!(
        log.get_synthesis_cache(&id, &sources_hash).unwrap().is_none(),
        "nothing is cached on the read-Err Elide path"
    );
}

// 9a — FAIL CLOSED on a non-UTF-8 in-scope source. The source ingested as UTF-8 but is
// OVERWRITTEN on disk with invalid UTF-8 bytes. The phase must `Elide` (a lossy
// `from_utf8_lossy` view would synthesize from replacement chars, defeating convergence)
// and cache nothing.
#[test]
fn non_utf8_source_fails_closed_to_elide() {
    let (log, _tmp, src, out) = phase_log();
    let sid = ingest_source(&log, &src, "a.md", b"Alpha\n");
    let target = out.join("index.md");
    let id = log.add_mandate(&target, &src, "index the titles").unwrap();
    let m = &log.active_mandates().unwrap()[0];
    // Hash reflects the ORIGINAL ingest (no re-ingest), so it equals what the phase computes.
    let sources_hash = sources_hash_for(&log, &sid);
    // Overwrite the on-disk bytes with an invalid UTF-8 sequence.
    std::fs::write(src.join("a.md"), [0xff, 0xfe, 0x00]).unwrap();

    let reasoner = CountingReasoner::new();
    let action = log.mandate_phase_for(m, &reasoner).unwrap();
    assert!(matches!(action, MandateAction::Elide), "non-UTF-8 in-scope source → Elide");
    assert!(reasoner.synth_calls() == 0, "fail-closed elide reaches NO LLM call");
    assert!(
        log.get_synthesis_cache(&id, &sources_hash).unwrap().is_none(),
        "nothing is cached on the non-UTF-8 Elide path"
    );
}

// 9a — empty synthesis: a model that returns `{ "synced_content": "" }` is a genuine
// synthesis failure → `Reject` (finding G: never truncate the target to empty).
#[test]
fn empty_synced_content_rejects() {
    let (log, _tmp, src, out) = phase_log();
    ingest_source(&log, &src, "a.md", b"Alpha\n");
    let target = out.join("index.md");
    let id = log.add_mandate(&target, &src, "index the titles").unwrap();
    let m = &log.active_mandates().unwrap()[0];
    assert_eq!(m.mandate_grant_id, id);

    let reasoner = SyncReasoner::new(""); // empty synthesis
    let action = log.mandate_phase_for(m, &reasoner).unwrap();
    assert!(matches!(action, MandateAction::Reject { .. }), "empty synced_content → Reject");
}

// 9a — cache hit skips the LLM: pre-seed the synthesis cache for the EXACT computed
// `sources_hash`; the phase must return a `Propose` built from the cached bytes WITHOUT
// any reasoner call. The expected `sources_hash` is reproduced here by the same rule the
// engine uses: sha256 over the sorted `(canonical_path, content_hash)` pairs.
#[test]
fn cache_hit_skips_llm() {
    use sha2::{Digest, Sha256};
    let (log, _tmp, src, out) = phase_log();
    let sid = ingest_source(&log, &src, "a.md", b"Alpha\n");
    let target = out.join("index.md");
    let id = log.add_mandate(&target, &src, "index the titles").unwrap();
    let m = &log.active_mandates().unwrap()[0];

    // Reproduce the engine's sources_hash for the single in-scope source.
    let sources_hash = sources_hash_for(&log, &sid);

    let cached_bytes = b"CACHED INDEX\n";
    let cached_hash = hex::encode(Sha256::digest(cached_bytes));
    log.put_synthesis_cache(
        &id,
        &sources_hash,
        cached_bytes,
        &cached_hash,
        std::slice::from_ref(&sid),
    )
    .unwrap();

    let reasoner = CountingReasoner::new();
    let action = log.mandate_phase_for(m, &reasoner).unwrap();
    match action {
        MandateAction::Propose { expected, lineage, op, sources_hash: ret_hash } => {
            assert!(expected == cached_bytes, "Propose uses the CACHED bytes");
            assert!(op == WriteOp::Create, "target absent → Create");
            // Lineage is the engine-gathered union (mandate ∪ source id), sorted+deduped.
            assert!(lineage.contains(&id), "lineage carries the mandate id");
            assert!(lineage.contains(&sid), "lineage carries the engine-gathered source id");
            // The returned sources_hash IS the engine's computed hash (Task 9b threads it
            // out so the inducing_key cannot drift from the phase's own digest).
            assert_eq!(ret_hash, sources_hash, "Propose returns the engine-computed sources_hash");
        }
        other => panic!("cache hit must Propose, got {other:?}"),
    }
    assert!(reasoner.synth_calls() == 0, "cache hit reaches NO LLM call");
}

// 9a (helpful) — in-sync target elides: when the target on disk ALREADY equals the
// (cache-supplied) expected bytes, the phase must `Elide` (nothing to do), comparing
// against ON-DISK bytes, not the stale files projection.
#[test]
fn in_sync_target_elides() {
    use sha2::{Digest, Sha256};
    let (log, _tmp, src, out) = phase_log();
    let sid = ingest_source(&log, &src, "a.md", b"Alpha\n");
    let target = out.join("index.md");
    let id = log.add_mandate(&target, &src, "index the titles").unwrap();
    let m = &log.active_mandates().unwrap()[0];

    let sources_hash = sources_hash_for(&log, &sid);

    let expected = b"IN SYNC\n";
    // Put the SAME bytes both in the cache (so synth resolves) and on disk at the target.
    log.put_synthesis_cache(
        &id,
        &sources_hash,
        expected,
        &hex::encode(Sha256::digest(expected)),
        &[sid],
    )
    .unwrap();
    std::fs::write(&target, expected).unwrap();

    let reasoner = CountingReasoner::new();
    let action = log.mandate_phase_for(m, &reasoner).unwrap();
    assert!(matches!(action, MandateAction::Elide), "on-disk == expected → Elide");
    assert!(reasoner.synth_calls() == 0, "in-sync via cache reaches NO LLM call");
}

// ════════════════════════════════════════════════════════════════════════════════
// TASK 9b — the mandate proposer WIRED into `evolve_once` (§7 security capstone).
//
// These proofs drive the WHOLE `evolve_once` tick and assert on the RECORDED events.
// To isolate the mandate phase hermetically, each test advances the evolve cursor PAST
// every seeded event (`set_evolve_cursor(tip)`) so the memory-extraction batch is empty
// and the ONLY reasoner calls are the mandate synthesis turns — exactly the cursor
// trick `tests/reconcile.rs`'s cap test uses. The mandate phase runs AFTER summarize;
// with no dirty topics, summarize is a no-op, so the synthesis turn is the sole call.
// ════════════════════════════════════════════════════════════════════════════════

use bossclaw_core::actuator::WriteProposal;
use bossclaw_core::embed::MockEmbedder;
use bossclaw_core::extract::MAX_PROPOSALS_PER_TICK;
use bossclaw_core::graph::{
    M6C_PROPOSER_PRODUCER, WRITE_PROPOSAL_EVENT_TYPE, WRITE_REJECTED_EVENT_TYPE,
};
use std::sync::atomic::AtomicU64;

/// A reasoner that answers EVERY mandate synthesis turn with the SAME fixed bytes
/// (matched on the `build_recipe_prompt` frame lead). Any non-synthesis turn returns an
/// empty object so an unexpected summarize/extraction call never crashes the proof
/// (the cursor trick should prevent those, but failing soft keeps the proof about the
/// property under test). The full-loop analogue of 9a's `SyncReasoner`.
struct MandateReasoner {
    synced: String,
}

impl MandateReasoner {
    fn new(synced: &str) -> Self {
        Self { synced: synced.to_string() }
    }
}

impl Reasoner for MandateReasoner {
    fn complete_json(&self, _system: &str, prompt: &str, _schema: &Value) -> Result<Value, BossclawError> {
        if prompt.contains(RECIPE_FRAME_LEAD) {
            return Ok(json!({ "synced_content": self.synced }));
        }
        Ok(json!({}))
    }
    fn model_id(&self) -> &str {
        "m6c-mandate-proposer"
    }
}

/// A NONDETERMINISTIC reasoner: every synthesis turn returns DIFFERENT bytes (a
/// monotonically-increasing counter is appended). Proves convergence (proof 4) comes
/// from the per-source-state synthesis cache (§5.2), NOT from any LLM determinism — if
/// the phase re-synthesized on an unchanged source-state, this reasoner's ever-changing
/// output would propose forever.
struct NondeterministicReasoner {
    calls: AtomicU64,
}

impl NondeterministicReasoner {
    fn new() -> Self {
        Self { calls: AtomicU64::new(0) }
    }
    fn calls(&self) -> u64 {
        self.calls.load(Ordering::Relaxed)
    }
}

impl Reasoner for NondeterministicReasoner {
    fn complete_json(&self, _system: &str, prompt: &str, _schema: &Value) -> Result<Value, BossclawError> {
        if prompt.contains(RECIPE_FRAME_LEAD) {
            let n = self.calls.fetch_add(1, Ordering::Relaxed);
            // A DISTINCT body per call — convergence cannot come from identical output.
            return Ok(json!({ "synced_content": format!("synthesis variant {n}\n") }));
        }
        Ok(json!({}))
    }
    fn model_id(&self) -> &str {
        "m6c-nondeterministic-test"
    }
}

/// Advance the evolve cursor PAST every event seeded so far, so the next `evolve_once`
/// processes NO memory (the seq is a 1-based autoincrement with no deletes → the event
/// count IS the tip seq). The mandate phase still sees `current_files()` (cursor-
/// independent), so only the synthesis turn fires. Mirrors the reconcile cap test.
fn skip_all_as_evolve_subjects(log: &EventLog) {
    let tip = log.stream_all().unwrap().len() as i64;
    log.set_evolve_cursor(tip).unwrap();
}

/// Every CURRENT `write_proposal` event targeting `canonical` (M6c-stamped or not).
fn proposals_for(log: &EventLog, canonical: &str) -> Vec<bossclaw_core::event::Event> {
    log.stream_all()
        .unwrap()
        .into_iter()
        .filter(|e| e.event_type == WRITE_PROPOSAL_EVENT_TYPE)
        .filter(|e| e.content.get("target").and_then(|t| t.as_str()) == Some(canonical))
        .collect()
}

/// Count ALL `write_rejected` events in the log (used to assert a cap NEVER rejects).
fn rejected_count(log: &EventLog) -> usize {
    log.stream_all()
        .unwrap()
        .into_iter()
        .filter(|e| e.event_type == WRITE_REJECTED_EVENT_TYPE)
        .count()
}

/// The canonical TARGET string the engine records (the form `add_mandate` stores and
/// `run_mandate` stamps as a proposal/rejection `target`). Mirrors the engine's
/// `canonicalize_target_or_parent`: canonicalize the path if it exists, else canonicalize
/// the PARENT and re-join the (not-yet-created) leaf NOFOLLOW. Required because most
/// mandate targets are Creates that do not exist on disk yet (`fs::canonicalize` would err).
fn canon(p: &Path) -> String {
    if let Ok(real) = std::fs::canonicalize(p) {
        return real.to_string_lossy().to_string();
    }
    let parent = std::fs::canonicalize(p.parent().expect("target has a parent")).expect("parent resolves");
    parent.join(p.file_name().expect("target has a leaf")).to_string_lossy().to_string()
}

// ── PROOF 1 — cannot widen the grant (propose AND execute). ─────────────────────────
// (a) With the TARGET's write-grant revoked, the gate rejects at PROPOSE → no proposal,
//     a `write_rejected` recorded (the never-widen check fires at propose time).
// (b) With a grant revoked BETWEEN propose and confirm, `execute_write_resolving`'s
//     re-check rejects → no write. The load-bearing boundary is EXECUTE-time.
#[test]
fn proof1a_cannot_widen_grant_rejected_at_propose() {
    let (log, _tmp, src, out) = phase_log();
    ingest_source(&log, &src, "a.md", b"Alpha\n");
    let target = out.join("index.md");
    log.add_mandate(&target, &src, "index the titles").unwrap();
    // Revoke the TARGET's write-grant (the `out` dir). The mandate stays active, but the
    // target is no longer under any active write-grant → the gate must reject.
    log.revoke_write_grant(&out).unwrap();
    skip_all_as_evolve_subjects(&log);

    let emb = MockEmbedder::new(64);
    let reasoner = MandateReasoner::new("INDEX: Alpha\n");
    let rep = log.evolve_once(&emb, &reasoner).unwrap();

    assert_eq!(rep.proposals_emitted, 0, "a target outside any write-grant cannot be proposed");
    assert!(rep.proposals_rejected >= 1, "the never-widen gate records a write_rejected");
    assert!(proposals_for(&log, &canon(&target)).is_empty(), "no write_proposal for the un-granted target");
    // No file was written (a proposal is not a write anyway; assert the target is absent).
    assert!(!target.exists(), "no file_written: the proposer never writes, and the gate rejected");
    log.verify_chain().unwrap();
}

#[test]
fn proof1b_cannot_widen_grant_rejected_at_execute_after_revoke() {
    // Emit a real M6c proposal, then revoke the grant BEFORE confirming. The confirm path
    // (`propose_write` → `execute_write_resolving`) must fail closed — the execute-time
    // re-check is the load-bearing boundary.
    let (log, _tmp, src, out) = phase_log();
    ingest_source(&log, &src, "a.md", b"Alpha\n");
    let target = out.join("index.md");
    log.add_mandate(&target, &src, "index the titles").unwrap();
    skip_all_as_evolve_subjects(&log);

    let emb = MockEmbedder::new(64);
    let reasoner = MandateReasoner::new("INDEX: Alpha\n");
    let rep = log.evolve_once(&emb, &reasoner).unwrap();
    assert_eq!(rep.proposals_emitted, 1, "with the grant intact, one proposal is emitted");

    // Capture the emitted proposal's own recorded fields for the confirm path.
    let prop = proposals_for(&log, &canon(&target)).into_iter().next().expect("the M6c proposal");
    let pid = prop.id.clone();
    let hash = prop.content["new_content_hash"].as_str().unwrap().to_string();
    let lineage = prop.model_meta.as_ref().expect("Tier-B").source_event_ids.clone();
    let bytes = log.get_proposal_bytes_checked(&pid, &hash).unwrap();

    // Now REVOKE the target's write-grant — between propose and confirm.
    log.revoke_write_grant(&out).unwrap();

    // Re-gate + execute: the gate's `allowed` is now false (and the execute-time re-check
    // would also fail). Either way the write must be refused and the file must not appear.
    let gated = log
        .propose_write(WriteProposal {
            target: target.clone(),
            new_content: bytes,
            op: WriteOp::Create,
            source_event_ids: lineage,
            rationale: "confirm".into(),
        })
        .expect("propose_write returns a verdict (rejecting, not erroring)");
    // ack=false: the revoked write-grant (`!allowed`) is the fail-closed cause under test; it is
    // checked before the engine loud-gate, so the documented revoke rejection still fires (SP5 d).
    let exec = log.execute_write_resolving(gated, &pid, false);
    assert!(exec.is_err(), "execute must fail closed once the grant is revoked (never-widen at execute)");
    assert!(!target.exists(), "no file was written after the grant was revoked");
    log.verify_chain().unwrap();
}

// ── PROOF 2 — external PROVENANCE is preserved; an IN-SCOPE mandate yields a Clean verdict. ──
// A mandate whose source is EXTERNAL (an ingested file, origin:"external") → the emitted
// `write_proposal` carries that source's `file_ingested` id in `source_event_ids` and is
// stamped origin:"external" — the provenance taint is NEVER shed from the record.
// SP5 change (c): because the source is IN the mandate's authorized `source_scope` and the
// target is the mandate's own target, the gate VERDICT is Clean / not-loud (the user's grant
// authorizes auto-apply). The complementary "an UNAUTHORIZED external source still taints"
// security proof lives in tests/reconcile.rs (out-of-scope / sibling / m6b-target / post-revoke
// all stay Untrusted + loud — the trust rule cannot launder taint for anything unauthorized).
#[test]
fn proof2_cannot_shed_taint_external_source_stamps_proposal() {
    let (log, _tmp, src, out) = phase_log();
    let sid = ingest_source(&log, &src, "a.md", b"Alpha\n"); // EXTERNAL file_ingested id
    let target = out.join("index.md");
    log.add_mandate(&target, &src, "index the titles").unwrap();
    skip_all_as_evolve_subjects(&log);

    let emb = MockEmbedder::new(64);
    let reasoner = MandateReasoner::new("INDEX: Alpha\n");
    let rep = log.evolve_once(&emb, &reasoner).unwrap();
    assert_eq!(rep.proposals_emitted, 1, "one proposal from the external source");

    let prop = proposals_for(&log, &canon(&target)).into_iter().next().expect("the proposal");
    // Lineage carries the engine-gathered external source id.
    let lineage = prop.model_meta.as_ref().expect("Tier-B").source_event_ids.clone();
    assert!(lineage.contains(&sid), "lineage carries the external source's file_ingested id");
    // The append chokepoint stamped origin:"external" — the provenance is preserved (never shed).
    assert_eq!(prop.content["origin"], json!("external"), "external source's provenance is recorded");
    // SP5 (c): an IN-SCOPE authorized mandate source yields a Clean / not-loud verdict (auto-appliable).
    // An unauthorized / out-of-scope source would stay Untrusted + loud (proven in tests/reconcile.rs).
    assert_eq!(prop.content["verdict_summary"]["taint"], json!("Clean"), "in-scope mandate source ⇒ Clean verdict");
    assert_eq!(prop.content["verdict_summary"]["requires_loud_modal"], json!(false), "Clean ⇒ not loud (auto-appliable)");
    log.verify_chain().unwrap();
}

// ── PROOF 3 — lineage is engine-gathered (model citations are ignored). ─────────────
// A scripted reasoner emits `synced_content` that EMBEDS an id-looking string trying to
// "cite" a forbidden id. That string is ABSENT from the proposal's `source_event_ids`;
// the set == {mandate_id} ∪ {actual in-scope source ids}, nothing the model named.
#[test]
fn proof3_lineage_is_engine_gathered_not_model_cited() {
    let (log, _tmp, src, out) = phase_log();
    let sid = ingest_source(&log, &src, "a.md", b"Alpha\n");
    let mid = {
        let target = out.join("index.md");
        log.add_mandate(&target, &src, "index the titles").unwrap()
    };
    let target = out.join("index.md");
    skip_all_as_evolve_subjects(&log);

    // The synthesized body literally contains an attacker-chosen id-looking token.
    let forbidden = "01FORBIDDENIDAAAAAAAAAAAAAA";
    let emb = MockEmbedder::new(64);
    let reasoner = MandateReasoner::new(&format!("INDEX cites {forbidden} as a source\n"));
    let rep = log.evolve_once(&emb, &reasoner).unwrap();
    assert_eq!(rep.proposals_emitted, 1);

    let prop = proposals_for(&log, &canon(&target)).into_iter().next().unwrap();
    let lineage = prop.model_meta.as_ref().expect("Tier-B").source_event_ids.clone();
    assert!(!lineage.contains(&forbidden.to_string()), "a model-named id is NEVER in the lineage");
    // The set is EXACTLY {mandate} ∪ {in-scope source} (sorted/deduped).
    let mut expected = vec![mid.clone(), sid.clone()];
    expected.sort();
    expected.dedup();
    let mut got = lineage.clone();
    got.sort();
    assert_eq!(got, expected, "lineage == {{mandate}} ∪ {{actual in-scope source ids}}");
    log.verify_chain().unwrap();
}

// ── PROOF 3b — cache-hit lineage covers DEPARTED taint (Finding B regression guard). ─
// Seed the synthesis cache with a synth-lineage id (an external source) that is NO LONGER
// in the current in-scope set, and force a CACHE HIT (the current in-scope set produces a
// matching `sources_hash` but no longer includes the departed source). The emitted
// proposal must STILL carry the departed source's `file_ingested` id (from
// `source_event_ids_at_synth`) AND be origin:"external"+loud. This FAILS against a design
// that re-derives lineage only from the CURRENT scope.
#[test]
fn proof3b_cache_hit_lineage_covers_departed_taint() {
    use sha2::{Digest, Sha256};
    let (log, _tmp, src, out) = phase_log();
    // The CURRENTLY in-scope source (under the mandate's `src` scope; what the hash covers).
    let current_sid = ingest_source(&log, &src, "current.md", b"Current\n");
    // A DEPARTED external source living OUTSIDE the mandate's source scope: ingest it under a
    // SEPARATE read-granted dir (`other`), so it is genuinely NOT in `src`'s in-scope set —
    // but it IS a real external `file_ingested` (origin:"external"). This models a source that
    // was in scope at synthesis time and has since left scope (Finding B), via the plan's
    // "seed the cache directly with a synth-lineage id that's no longer in-scope" route.
    let other = _tmp.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    read_grant(&log, &other);
    let departed_sid = ingest_source(&log, &other, "departed.md", b"Departed external\n");

    // Sanity: the current source is in `src`'s in-scope set; the departed source is NOT.
    let scope = std::fs::canonicalize(&src).unwrap();
    let in_scope: Vec<String> = log
        .current_files()
        .unwrap()
        .into_iter()
        .filter(|r| std::path::Path::new(&r.canonical_path).starts_with(&scope))
        .map(|r| r.file_event_id)
        .collect();
    assert!(in_scope.contains(&current_sid), "the current source is in the mandate's scope");
    assert!(!in_scope.contains(&departed_sid), "the departed source is OUTSIDE the mandate's scope");

    let target = out.join("index.md");
    let mid = log.add_mandate(&target, &src, "index the titles").unwrap();

    // Compute the sources_hash over the CURRENT in-scope set (just `current.md`) exactly as
    // the phase does, and SEED the cache under it — but with the synth-lineage carrying the
    // DEPARTED external id (as if synthesis happened while it was still in scope, Finding B).
    let sources_hash = sources_hash_for(&log, &current_sid);
    let cached = b"INDEX (synthesized while departed.md was in scope)\n";
    log.put_synthesis_cache(
        &mid,
        &sources_hash,
        cached,
        &hex::encode(Sha256::digest(cached)),
        std::slice::from_ref(&departed_sid), // the departed external id travels in the cache row
    )
    .unwrap();

    skip_all_as_evolve_subjects(&log);
    let emb = MockEmbedder::new(64);
    // A reasoner that MUST NOT be called — a cache hit reaches no LLM. If synthesis fires,
    // the empty-object answer would yield empty synced_content → Reject, failing the proof.
    let reasoner = CountingReasoner::new();
    let rep = log.evolve_once(&emb, &reasoner).unwrap();
    assert_eq!(rep.proposals_emitted, 1, "the cache hit proposes (no LLM call)");
    assert_eq!(reasoner.synth_calls(), 0, "cache hit reaches NO synthesis turn");

    let prop = proposals_for(&log, &canon(&target)).into_iter().next().unwrap();
    let lineage = prop.model_meta.as_ref().expect("Tier-B").source_event_ids.clone();
    assert!(
        lineage.contains(&departed_sid),
        "the DEPARTED external source id is STILL in the lineage (from source_event_ids_at_synth)"
    );
    // Departed source is external → the proposal must remain origin:"external" + loud.
    assert_eq!(prop.content["origin"], json!("external"), "departed external taint is NOT shed on a cache hit");
    assert_eq!(prop.content["verdict_summary"]["requires_loud_modal"], json!(true), "still loud");
    log.verify_chain().unwrap();
}

// ── PROOF 4 — convergence incl. a NONDETERMINISTIC model. ───────────────────────────
// After a confirmed write with unchanged sources, the NEXT tick proposes nothing — using
// a reasoner that returns DIFFERENT bytes each call. Convergence comes from the per-
// source-state cache (§5.2), not LLM determinism.
#[test]
fn proof4_convergence_with_a_nondeterministic_model() {
    let (log, _tmp, src, out) = phase_log();
    ingest_source(&log, &src, "a.md", b"Alpha\n");
    let target = out.join("index.md");
    log.add_mandate(&target, &src, "index the titles").unwrap();
    skip_all_as_evolve_subjects(&log);

    let emb = MockEmbedder::new(64);
    let reasoner = NondeterministicReasoner::new();

    // Tick 1: a proposal is synthesized (first variant). Confirm it onto disk so the target
    // now equals the cached expected bytes for THIS source-state.
    let rep1 = log.evolve_once(&emb, &reasoner).unwrap();
    assert_eq!(rep1.proposals_emitted, 1, "tick 1 proposes");
    let prop = proposals_for(&log, &canon(&target)).into_iter().next().unwrap();
    let pid = prop.id.clone();
    let hash = prop.content["new_content_hash"].as_str().unwrap().to_string();
    let lineage = prop.model_meta.as_ref().unwrap().source_event_ids.clone();
    let bytes = log.get_proposal_bytes_checked(&pid, &hash).unwrap();
    let gated = log
        .propose_write(WriteProposal {
            target: target.clone(),
            new_content: bytes.clone(),
            op: WriteOp::Create,
            source_event_ids: lineage,
            rationale: "confirm".into(),
        })
        .unwrap();
    // The source is in-scope for this mandate, so the SP5 (c) trust rule keeps the verdict Clean
    // (not loud) ⇒ ack=false clears the engine loud-gate (SP5 change d).
    log.execute_write_resolving(gated, &pid, false).unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), bytes, "the confirmed bytes are on disk");
    let calls_after_tick1 = reasoner.calls();

    // Tick 2: sources unchanged → cache HIT → on-disk == expected → ELIDE. Even though the
    // reasoner WOULD return different bytes, it is never called (convergence from the cache).
    skip_all_as_evolve_subjects(&log);
    let rep2 = log.evolve_once(&emb, &reasoner).unwrap();
    assert_eq!(rep2.proposals_emitted, 0, "tick 2 proposes nothing (converged)");
    assert_eq!(rep2.proposals_rejected, 0, "convergence is an Elide, not a reject");
    assert_eq!(
        reasoner.calls(),
        calls_after_tick1,
        "the nondeterministic model was NOT called on tick 2 (cache hit) — convergence is from the cache"
    );
    log.verify_chain().unwrap();
}

// ── PROOF 5 — decline-stickiness. ───────────────────────────────────────────────────
// Decline a sync proposal → the next tick (unchanged sources) proposes nothing for that
// source-state; then CHANGE a source → it proposes the new version.
#[test]
fn proof5_decline_is_sticky_until_a_source_changes() {
    let (log, _tmp, src, out) = phase_log();
    ingest_source(&log, &src, "a.md", b"Alpha\n");
    let target = out.join("index.md");
    log.add_mandate(&target, &src, "index the titles").unwrap();
    skip_all_as_evolve_subjects(&log);

    let emb = MockEmbedder::new(64);
    let reasoner = MandateReasoner::new("INDEX v1\n");

    // Tick 1: one proposal. DECLINE it.
    let rep1 = log.evolve_once(&emb, &reasoner).unwrap();
    assert_eq!(rep1.proposals_emitted, 1, "tick 1 proposes");
    let pid = proposals_for(&log, &canon(&target)).into_iter().next().unwrap().id;
    log.decline_write_proposal(&pid, "not now").unwrap();

    // Tick 2: sources UNCHANGED → the declined source-state is sticky → propose nothing.
    skip_all_as_evolve_subjects(&log);
    let rep2 = log.evolve_once(&emb, &reasoner).unwrap();
    assert_eq!(rep2.proposals_emitted, 0, "a declined source-state does NOT re-nag (sticky)");
    assert_eq!(rep2.proposals_rejected, 0, "decline-stickiness is an Elide, not a reject");

    // CHANGE a source → a NEW source-state (different sources_hash) → a fresh ask.
    std::fs::write(src.join("a.md"), b"Alpha CHANGED\n").unwrap();
    log.ingest_all(&bossclaw_core::ingest::ParserRouter::native_only(), &emb).unwrap();
    skip_all_as_evolve_subjects(&log);
    let reasoner2 = MandateReasoner::new("INDEX v2\n");
    let rep3 = log.evolve_once(&emb, &reasoner2).unwrap();
    assert_eq!(rep3.proposals_emitted, 1, "a changed source is a fresh source-state → proposes again");
    log.verify_chain().unwrap();
}

// ── PROOF 6 — caps (hermetic, no clock). ────────────────────────────────────────────
// More "due" mandates than MAX_PROPOSALS_PER_TICK in one tick → only cap-many proposals
// emitted, the rest ELIDE (no event, retryable). Assert NO permanent write_rejected from
// the cap, and that an emitted proposal's recorded model_meta.model_id is the M6c stamp.
#[test]
fn proof6_caps_elide_the_overflow_and_never_reject() {
    let (log, _tmp, src, out) = phase_log();
    // Build MAX_PROPOSALS_PER_TICK + 2 mandates, each with its OWN distinct source + target,
    // so each is independently "due" this tick. (The global per-tick cap is SHARED across
    // all mandates via report.proposals_emitted.)
    let n = MAX_PROPOSALS_PER_TICK + 2;
    let emb = MockEmbedder::new(64);
    for i in 0..n {
        // A distinct source subtree per mandate so their sources_hashes (and targets) differ.
        let sdir = src.join(format!("s{i}"));
        std::fs::create_dir_all(&sdir).unwrap();
        log.add_grant(&sdir).unwrap();
        std::fs::write(sdir.join("a.md"), format!("Body {i}\n").as_bytes()).unwrap();
        log.add_mandate(&out.join(format!("index{i}.md")), &sdir, "index it").unwrap();
    }
    // Ingest all the sources once.
    log.ingest_all(&bossclaw_core::ingest::ParserRouter::native_only(), &emb).unwrap();
    skip_all_as_evolve_subjects(&log);

    let reasoner = MandateReasoner::new("INDEX\n");
    let rep = log.evolve_once(&emb, &reasoner).unwrap();

    assert_eq!(rep.proposals_emitted, MAX_PROPOSALS_PER_TICK, "only cap-many proposals are emitted");
    assert_eq!(rep.proposals_elided_cap, n - MAX_PROPOSALS_PER_TICK, "the overflow ELIDES (counted)");
    assert_eq!(rep.proposals_rejected, 0, "a cap is NEVER a write_rejected (retryable)");
    assert_eq!(rejected_count(&log), 0, "no permanent write_rejected event from the cap");

    // The emitted proposal carries the M6c producer stamp (distinguishes mandate proposals).
    let emitted: Vec<bossclaw_core::event::Event> = log
        .stream_all()
        .unwrap()
        .into_iter()
        .filter(|e| e.event_type == WRITE_PROPOSAL_EVENT_TYPE)
        .collect();
    assert_eq!(emitted.len(), MAX_PROPOSALS_PER_TICK, "exactly cap-many proposal events recorded");
    for e in &emitted {
        assert_eq!(
            e.model_meta.as_ref().expect("Tier-B").model_id,
            M6C_PROPOSER_PRODUCER,
            "a mandate proposal records model_id == \"m6c-mandate-proposer\""
        );
    }
    log.verify_chain().unwrap();
}

// ── PROOF 7 — off-switch (sticky + fail-closed). ────────────────────────────────────
// `set_mandates_enabled(false)` → the mandate phase emits nothing.
#[test]
fn proof7_off_switch_suppresses_the_whole_phase() {
    let (log, _tmp, src, out) = phase_log();
    ingest_source(&log, &src, "a.md", b"Alpha\n");
    let target = out.join("index.md");
    log.add_mandate(&target, &src, "index the titles").unwrap();
    log.set_mandates_enabled(false).unwrap(); // sticky off
    skip_all_as_evolve_subjects(&log);

    let emb = MockEmbedder::new(64);
    let reasoner = MandateReasoner::new("INDEX\n");
    let rep = log.evolve_once(&emb, &reasoner).unwrap();

    assert_eq!(rep.proposals_emitted, 0, "off-switch suppresses all proposals");
    assert_eq!(rep.proposals_rejected, 0, "off-switch is a plain skip — never a write_rejected");
    assert!(proposals_for(&log, &canon(&target)).is_empty(), "no proposal event written");
    assert_eq!(rejected_count(&log), 0, "no write_rejected from the off-switch path");
    log.verify_chain().unwrap();
}

// ── PROOF 9 — structural self-loop guard (no re-ingest of own write). ───────────────
// A mandate whose target is legitimately OUTSIDE the read roots, after a confirmed write,
// does NOT re-ingest its own write as a source: a second tick with unchanged sources →
// Elide (proposes nothing). (Task 2 already proved add_mandate rejects a target under a
// read root; this proves the runtime convergence the structural guard guarantees.)
#[test]
fn proof9_confirmed_write_is_not_reingested_as_a_source() {
    let (log, _tmp, src, out) = phase_log();
    ingest_source(&log, &src, "a.md", b"Alpha\n");
    let target = out.join("index.md"); // out is OUTSIDE the read root `src`
    log.add_mandate(&target, &src, "index the titles").unwrap();
    skip_all_as_evolve_subjects(&log);

    let emb = MockEmbedder::new(64);
    let reasoner = MandateReasoner::new("INDEX: Alpha\n");

    // Tick 1: propose + confirm the write onto disk.
    let rep1 = log.evolve_once(&emb, &reasoner).unwrap();
    assert_eq!(rep1.proposals_emitted, 1);
    let prop = proposals_for(&log, &canon(&target)).into_iter().next().unwrap();
    let pid = prop.id.clone();
    let hash = prop.content["new_content_hash"].as_str().unwrap().to_string();
    let lineage = prop.model_meta.as_ref().unwrap().source_event_ids.clone();
    let bytes = log.get_proposal_bytes_checked(&pid, &hash).unwrap();
    let gated = log
        .propose_write(WriteProposal {
            target: target.clone(),
            new_content: bytes,
            op: WriteOp::Create,
            source_event_ids: lineage,
            rationale: "confirm".into(),
        })
        .unwrap();
    // In-scope mandate source ⇒ SP5 (c) trust rule keeps it Clean (not loud) ⇒ ack=false (SP5 d).
    log.execute_write_resolving(gated, &pid, false).unwrap();

    // Re-run ingest over the read root: the target lives OUTSIDE it, so the write is NOT
    // discovered as a source (the structural self-loop guarantee). The sources_hash is
    // therefore unchanged → tick 2 elides.
    log.ingest_all(&bossclaw_core::ingest::ParserRouter::native_only(), &emb).unwrap();
    skip_all_as_evolve_subjects(&log);
    let rep2 = log.evolve_once(&emb, &reasoner).unwrap();
    assert_eq!(rep2.proposals_emitted, 0, "the engine's own write is never re-ingested → converged");
    assert_eq!(rep2.proposals_rejected, 0, "convergence is an Elide, not a reject");
    log.verify_chain().unwrap();
}

// ── PROOF 10 — elide/reject degenerate cases through the PHASE (recorded events). ────
// (a) empty-source mandate → the phase Elides end-to-end (no event).
// (b) empty `synced_content` → the phase records a `write_rejected` (a genuine failure).
#[test]
fn proof10_phase_elides_on_empty_source_and_rejects_on_empty_synthesis() {
    // (a) Empty source scope (no ingested files) → Elide, nothing recorded.
    {
        let (log, _tmp, _src, out) = phase_log();
        let target = out.join("index.md");
        log.add_mandate(&target, &_src, "index the titles").unwrap();
        skip_all_as_evolve_subjects(&log);
        let emb = MockEmbedder::new(64);
        let reasoner = MandateReasoner::new("INDEX\n");
        let rep = log.evolve_once(&emb, &reasoner).unwrap();
        assert_eq!(rep.proposals_emitted, 0, "empty-source mandate emits no proposal");
        assert_eq!(rep.proposals_rejected, 0, "empty-source is an Elide, not a reject");
        assert_eq!(rejected_count(&log), 0, "no write_rejected recorded for an empty-source Elide");
    }
    // (b) A real source but the model returns empty synced_content → write_rejected recorded.
    {
        let (log, _tmp, src, out) = phase_log();
        ingest_source(&log, &src, "a.md", b"Alpha\n");
        let target = out.join("index.md");
        log.add_mandate(&target, &src, "index the titles").unwrap();
        skip_all_as_evolve_subjects(&log);
        let emb = MockEmbedder::new(64);
        let reasoner = MandateReasoner::new(""); // empty synthesis → genuine failure
        let rep = log.evolve_once(&emb, &reasoner).unwrap();
        assert_eq!(rep.proposals_emitted, 0, "empty synthesis emits no proposal");
        assert_eq!(rep.proposals_rejected, 1, "empty synthesis is a genuine failure → write_rejected");
        assert_eq!(rejected_count(&log), 1, "exactly one write_rejected recorded through the phase");
        // The recorded write_rejected is M6c-stamped and targets the mandate target.
        let rej = log
            .stream_all()
            .unwrap()
            .into_iter()
            .find(|e| e.event_type == WRITE_REJECTED_EVENT_TYPE)
            .unwrap();
        assert_eq!(rej.content["target"], json!(canon(&target)), "the rejection targets the mandate target");
        assert_eq!(rej.model_meta.as_ref().expect("Tier-B").model_id, M6C_PROPOSER_PRODUCER);
        log.verify_chain().unwrap();
    }
}
