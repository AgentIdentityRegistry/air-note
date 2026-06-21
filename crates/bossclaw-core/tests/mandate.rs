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
    assert!(matches!(action, MandateAction::Reject(_)), "empty synced_content → Reject");
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
        MandateAction::Propose { expected, lineage, op } => {
            assert!(expected == cached_bytes, "Propose uses the CACHED bytes");
            assert!(op == WriteOp::Create, "target absent → Create");
            // Lineage is the engine-gathered union (mandate ∪ source id), sorted+deduped.
            assert!(lineage.contains(&id), "lineage carries the mandate id");
            assert!(lineage.contains(&sid), "lineage carries the engine-gathered source id");
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
