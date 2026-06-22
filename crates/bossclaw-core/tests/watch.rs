//! M6c Task 10 — the live filesystem watcher + debounced self-driver (Layer 2).
//!
//! The watcher CANNOT write. It only DRIVES the proposer: on a debounced filesystem
//! tick it re-ingests changed sources then runs one evolve tick, whose M6c mandate
//! phase proposes writes through M6a's gate (still human-confirmed). These hermetic
//! tests exercise the watcher → proposer pipeline WITHOUT real OS fs events by calling
//! the directly-testable `drive_once` + the coalesce path in isolation.
//!
//! Single-writer invariant (§6.3 / I4): the `MandateWatcher` holds ONE `Arc<EventLog>`
//! and calls its methods — it never opens a second store/connection. These tests
//! construct the `EventLog`, wrap it in an `Arc`, hand a clone to the watcher, and keep
//! the OTHER clone to assert on the SAME log the watcher drove.
#![cfg(unix)]

mod common;

use bossclaw_core::embed::MockEmbedder;
use bossclaw_core::graph::{M6C_PROPOSER_PRODUCER, WRITE_PROPOSAL_EVENT_TYPE};
use bossclaw_core::ingest::ParserRouter;
use bossclaw_core::reason::Reasoner;
use bossclaw_core::watch::{Coalescer, MandateWatcher};
use bossclaw_core::EventLog;
use ed25519_dalek::SigningKey;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::TempDir;

/// Deterministic data-encryption key for the hermetic SQLCipher store.
const DEK: [u8; 32] = [42u8; 32];
/// Deterministic ed25519 seed so signatures are reproducible across runs.
const KEY_BYTES: [u8; 32] = [7u8; 32];
/// MockEmbedder width — any fixed dim works for these path/proposal assertions.
const EMBED_DIM: usize = 64;
/// The lead line of `build_recipe_prompt`'s trusted frame — the synthesis-turn marker
/// the test reasoner matches on (mirrors `tests/mandate.rs::RECIPE_FRAME_LEAD`).
const RECIPE_FRAME_LEAD: &str = "You are synthesizing synced content";

/// A reasoner that answers EVERY mandate synthesis turn with the SAME fixed bytes
/// (matched on the `build_recipe_prompt` frame lead). Non-synthesis turns get an empty
/// object so a stray summarize/extraction call never crashes the test. Mirrors
/// `tests/mandate.rs::MandateReasoner` — the watcher needs a real reasoner to drive.
struct MandateReasoner {
    synced: String,
}

impl MandateReasoner {
    fn new(synced: &str) -> Self {
        Self { synced: synced.to_string() }
    }
}

impl Reasoner for MandateReasoner {
    fn complete_json(&self, _system: &str, prompt: &str, _schema: &Value) -> Result<Value, bossclaw_core::error::BossclawError> {
        if prompt.contains(RECIPE_FRAME_LEAD) {
            return Ok(json!({ "synced_content": self.synced }));
        }
        Ok(json!({}))
    }
    fn model_id(&self) -> &str {
        "m6c-mandate-proposer"
    }
}

/// Open a fresh `EventLog` on a tempdir-backed home with a read-granted `src` and a
/// SEPARATE write-granted `out` (out OUTSIDE the read root → Finding-A self-loop guard
/// holds). Returns the log wrapped in an `Arc` plus `(tmp, src, out)`. The `Arc` is the
/// SOLE store handle — the watcher gets a clone of THIS, never a second `EventLog::open`.
fn phase_log_arc() -> (Arc<EventLog>, TempDir, PathBuf, PathBuf) {
    let home = tempfile::tempdir().expect("create home tempdir");
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let log = EventLog::open(&home.path().join("m.db"), &DEK, key).expect("open EventLog");
    let src = home.path().join("src");
    std::fs::create_dir_all(&src).expect("create src dir");
    let out = home.path().join("out");
    std::fs::create_dir_all(&out).expect("create out dir");
    log.add_grant(&src).expect("read-grant src");
    log.add_write_grant(&out).expect("write-grant out");
    (Arc::new(log), home, src, out)
}

/// Advance the evolve cursor PAST every event seeded so far, so the next evolve tick
/// processes NO `memory` event (the seq is a 1-based autoincrement with no deletes → the
/// event count IS the tip seq). The mandate phase still sees `current_files()` (cursor-
/// independent), so only the synthesis turn fires. Mirrors `tests/mandate.rs`.
fn skip_all_as_evolve_subjects(log: &EventLog) {
    let tip = log.stream_all().unwrap().len() as i64;
    log.set_evolve_cursor(tip).unwrap();
}

/// Every CURRENT `write_proposal` event targeting `canonical`.
fn proposals_for(log: &EventLog, canonical: &str) -> Vec<bossclaw_core::event::Event> {
    log.stream_all()
        .unwrap()
        .into_iter()
        .filter(|e| e.event_type == WRITE_PROPOSAL_EVENT_TYPE)
        .filter(|e| e.content.get("target").and_then(|t| t.as_str()) == Some(canonical))
        .collect()
}

/// Count ALL `file_ingested` events (the watcher's re-ingest of a changed source must
/// add at least one).
fn ingested_count(log: &EventLog) -> usize {
    log.current_files().unwrap().len()
}

/// The canonical TARGET string the engine records for a not-yet-created Create target
/// (canonicalize the parent, re-join the leaf). Mirrors `tests/mandate.rs::canon`.
fn canon(p: &Path) -> String {
    if let Ok(real) = std::fs::canonicalize(p) {
        return real.to_string_lossy().to_string();
    }
    let parent = std::fs::canonicalize(p.parent().expect("target has a parent")).expect("parent resolves");
    parent.join(p.file_name().expect("target has a leaf")).to_string_lossy().to_string()
}

// ── PROOF: drive_once ingests a changed source THEN evolves → a proposal is emitted. ──
// Build a MandateWatcher over a read-granted src + write-granted out + a mandate. Write a
// source file (no OS event delivered). Call `drive_once` DIRECTLY → assert (a) ingest_all
// picked the source up (a file_ingested now exists) AND (b) the mandate phase emitted a
// write_proposal carrying the M6c producer stamp. This is the watcher → proposer pipeline
// end-to-end with NO real OS fs events.
#[test]
fn watcher_drive_step_ingests_then_evolves() {
    let (log, _tmp, src, out) = phase_log_arc();
    let target = out.join("index.md");
    log.add_mandate(&target, &src, "index the titles").unwrap();

    // The watcher holds a CLONE of the one Arc<EventLog> — never a second store.
    let watcher = MandateWatcher::new(
        Arc::clone(&log),
        ParserRouter::native_only(),
        Box::new(MockEmbedder::new(EMBED_DIM)),
        Box::new(MandateReasoner::new("INDEX: Alpha\n")),
    );

    // Before any drive: no source ingested yet.
    assert_eq!(ingested_count(&log), 0, "no source ingested before the first drive");

    // A source lands on disk under the read root (simulating the fs change the watcher's
    // notify backend would observe — we drive the step directly, no OS event needed).
    std::fs::write(src.join("a.md"), b"Alpha\n").unwrap();
    // Keep the synthesis turn the ONLY model call (no memory subjects to extract).
    skip_all_as_evolve_subjects(&log);

    // ONE debounced tick: ingest_all (re-ingest the changed source) then evolve_once (the
    // M6c mandate phase reacts).
    watcher.drive_once().expect("drive_once = ingest_all then evolve_once on the shared log");

    // (a) The change was re-ingested on the SAME log the watcher drove.
    assert_eq!(ingested_count(&log), 1, "drive_once re-ingested the changed source (file_ingested)");
    // (b) The mandate phase proposed a write through the gate — the watcher → proposer
    //     pipeline works without real OS events.
    let proposals = proposals_for(&log, &canon(&target));
    assert_eq!(proposals.len(), 1, "the mandate phase emitted exactly one write_proposal");
    assert_eq!(
        proposals[0].model_meta.as_ref().expect("Tier-B").model_id,
        M6C_PROPOSER_PRODUCER,
        "the emitted proposal is M6c-stamped (the watcher drove the mandate proposer)"
    );
    // The watcher itself wrote NOTHING to disk — the proposal is OPEN, not a file_written.
    assert!(!target.exists(), "the watcher proposes only; it never writes the target");
    log.verify_chain().unwrap();
}

/// A reasoner that ERRORS on the synthesis turn — a transient reasoner failure (spec §10:
/// the reasoner's output is data; a failure is a retryable no-op, never log corruption).
struct ErroringReasoner;
impl Reasoner for ErroringReasoner {
    fn complete_json(&self, _system: &str, prompt: &str, _schema: &Value) -> Result<Value, bossclaw_core::error::BossclawError> {
        if prompt.contains(RECIPE_FRAME_LEAD) {
            return Err(bossclaw_core::error::BossclawError::Reasoner("transient synthesis failure".into()));
        }
        Ok(json!({}))
    }
    fn model_id(&self) -> &str {
        "m6c-erroring-test"
    }
}

// ── M1: a failing tick is a recoverable no-op the loop survives. ─────────────────────
// The live `watch` loop swallows a `drive_once` error (log-and-continue) so one bad tick
// never kills the self-driver. This proves the property `drive_once` guarantees that on:
// a tick whose synthesis FAILS leaves the log UNCORRUPTED (chain verifies, no proposal
// from the failed synthesis), and a LATER tick — once the transient condition clears —
// still drives the proposer. (The loop runs `drive_once` repeatedly; this asserts ticks
// are independent + chain-safe across a failure, which is exactly what makes the
// log-and-continue safe.)
#[test]
fn failing_tick_is_a_recoverable_noop() {
    let (log, _tmp, src, out) = phase_log_arc();
    let target = out.join("index.md");
    log.add_mandate(&target, &src, "index the titles").unwrap();
    std::fs::write(src.join("a.md"), b"Alpha\n").unwrap();

    // Tick 1: a watcher whose reasoner ERRORS on synthesis. The evolve loop treats a
    // reasoner failure as a retryable no-op, so the mandate phase proposes nothing — but
    // the source IS ingested and, crucially, the log stays valid (no half-written event).
    let failing = MandateWatcher::new(
        Arc::clone(&log),
        ParserRouter::native_only(),
        Box::new(MockEmbedder::new(EMBED_DIM)),
        Box::new(ErroringReasoner),
    );
    skip_all_as_evolve_subjects(&log);
    failing.drive_once().expect("a transient reasoner failure is a no-op, not a hard error");
    assert!(
        proposals_for(&log, &canon(&target)).is_empty(),
        "a failed synthesis proposes nothing (retryable)"
    );
    log.verify_chain().expect("the log is uncorrupted after a failed tick");

    // Tick 2: the transient condition has cleared (a working reasoner). The SAME shared log
    // — driven a tick later, exactly as the log-and-continue loop would — now proposes.
    let working = MandateWatcher::new(
        Arc::clone(&log),
        ParserRouter::native_only(),
        Box::new(MockEmbedder::new(EMBED_DIM)),
        Box::new(MandateReasoner::new("INDEX: Alpha\n")),
    );
    skip_all_as_evolve_subjects(&log);
    working.drive_once().expect("the next tick drives cleanly after the prior tick failed");
    assert_eq!(
        proposals_for(&log, &canon(&target)).len(),
        1,
        "the self-driver recovers on a later tick — a bad tick did not break it"
    );
    log.verify_chain().unwrap();
}

// ── PROOF 8: an event storm COALESCES to a bounded number of drives (not N). ──────────
// Feed a burst of N synthetic fs events into the coalesce path DIRECTLY (no OS, no real
// watcher thread). The bounded channel + debounce collapse the whole burst into ONE
// pending drive: `drain_due` fires at most once per debounce window regardless of N, and
// a channel overflow sets the rescan flag instead of growing the queue. Assert the drive
// count is bounded (== 1 here), NOT N, and that a flood beyond the channel bound trips
// rescan rather than OOMing.
#[test]
fn event_storm_coalesces() {
    // A small bound makes the overflow path testable: a burst far larger than the bound
    // must NOT grow the queue — it trips the rescan flag.
    const BOUND: usize = 4;
    const DEBOUNCE_MS: u128 = 50;
    let mut coalescer = Coalescer::new(BOUND);

    // Storm: 1000 events arrive "instantly" at the SAME logical clock instant `t0` (we
    // drive the coalesce path directly with a fixed clock — no OS, no wall-clock). Every
    // offer is accepted as PENDING work, but offers beyond BOUND overflow the channel and
    // set the rescan flag rather than enqueueing unboundedly.
    let t0: u128 = 1_000;
    let n = 1000;
    for _ in 0..n {
        coalescer.offer_at(t0);
    }
    assert!(coalescer.pending(), "the burst left work pending");
    assert!(
        coalescer.rescan_requested(),
        "a burst larger than the channel bound trips the rescan flag (no unbounded growth)"
    );
    assert!(
        coalescer.queued() <= BOUND,
        "the channel never grows past its bound ({BOUND}) under a storm of {n} — overflow sets rescan, not queue growth"
    );

    // Within the SAME debounce window, NO tick is due yet (events still arriving).
    assert!(!coalescer.drive_due(t0, DEBOUNCE_MS), "no drive within the debounce window");

    // After the window elapses with no further events, EXACTLY ONE drive is due — the
    // whole storm of {n} collapsed into a single coalesced tick.
    let mut drives = 0usize;
    if coalescer.drive_due(t0 + DEBOUNCE_MS, DEBOUNCE_MS) {
        coalescer.mark_driven(); // the driver would call drive_once() here
        drives += 1;
    }
    // The coalesced drive drained the pending work; a second poll in the same quiet window
    // is NOT due again (no new events).
    if coalescer.drive_due(t0 + DEBOUNCE_MS + DEBOUNCE_MS, DEBOUNCE_MS) {
        coalescer.mark_driven();
        drives += 1;
    }
    assert_eq!(drives, 1, "a storm of {n} events COALESCED to exactly 1 drive (bounded), not {n}");
    assert!(!coalescer.pending(), "the coalesced drive cleared the pending work + rescan flag");
}
