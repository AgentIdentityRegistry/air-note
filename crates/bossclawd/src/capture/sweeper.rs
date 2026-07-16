//! A9: the daemon's capture SWEEPER — the durability guarantee AND the backfill engine.
//!
//! # Two jobs
//! - **Durability.** A crash / SIGKILL / power loss / missed SessionEnd poke can leave a
//!   just-finished session uncaptured. The sweeper is the guarantee that it is captured
//!   within one sweep (spec §4c). It also `heal_orphans`-reconciles the two `.md`⇄event
//!   crash windows on every pass (boot included).
//! - **Backfill.** The first sweep after Connect imports the last ~30 days of quiet
//!   transcripts (spec §6a) — bounded to [`CAPTURE_PER_SWEEP`] per sweep so a fresh connect
//!   is not a thundering herd; the backlog drains over successive sweeps.
//!
//! It ties together A5 (path discipline: [`claude_projects_root`]/[`valid_session_id`]/
//! [`open_transcript_confined`]), A6 ([`render_transcript`]), A7 ([`store_capture`]/
//! [`heal_orphans`]), and A8 (the consent flags).
//!
//! # The pure core vs the daemon boundary
//! All selection logic lives in the pure, clock-free [`decide_sweep`] (unit-tested). The
//! effectful [`run_sweep_once`] wraps it with the filesystem scan + engine reads + the
//! capture writes; [`spawn`] is the thin tokio loop (mirrors `engine::scheduler::spawn`)
//! that reads the wall clock at the boundary and hands `now` in.
//!
//! # Two load-bearing contracts (from the A7/A8 reviews)
//! - **Contract 1 (store-file-first self-heal).** The cheap "already captured" skip fires
//!   ONLY when the session is current with the SAME sha AND its `.md` exists on disk. If the
//!   `.md` is missing (out-of-band deletion, a crash heal window, or a regenerated
//!   placeholder that never got the real body), the session is (re)rendered so
//!   [`store_capture`] writes the real transcript body FIRST — the engine then dedups the
//!   event to a no-op. We never pre-skip at the event level and bypass the file write.
//!   This is why [`heal_orphans`] runs AFTER the capture pass, not before: a dangling
//!   event's `.md` must still read as MISSING at decide time so it re-renders — heal then
//!   only regenerates placeholders for truly-gone transcripts.
//! - **Contract 2 (gate the timestamp on `enabled`).** `capture_enabled_at` is a stale
//!   leftover on a disabled brain, so it is read ONLY after `onboarded && capture_enabled`
//!   is confirmed. [`decide_sweep`] returns early (never touching `capture_enabled_at`) when
//!   either is false, and [`run_sweep_once`] reads the window inputs only inside that gate.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::capture::paths::{claude_projects_root, open_transcript_confined, valid_session_id};
use crate::capture::render::{render_bounds, render_transcript};
use crate::capture::store::{heal_orphans, store_capture, CaptureIdentity, HealReport, CAPTURE_TOOL};
use crate::engine::EngineHandle;

/// How often the loop wakes to consider a sweep (~5 min) — mirrors `EVOLVE_INTERVAL`.
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(300);

/// A transcript touched within this many seconds of `now` is treated as still-being-written
/// and skipped THIS sweep (a torn/partial capture would only be superseded next sweep). The
/// SweepEnd poke path (A11) does NOT apply this floor — an explicit SessionEnd IS the quiet
/// signal (spec §4c). Sweep path only.
pub const QUIET_SECS: i64 = 600;

/// Hard cap on transcripts (re)captured per sweep (mirrors `MANDATE_AUTOAPPLY_PER_SWEEP`): no
/// thundering herd on first connect; the backfill backlog spreads across successive sweeps;
/// index invalidation is batched (each `store_capture` invalidates once, already).
pub const CAPTURE_PER_SWEEP: usize = 8;

/// One transcript the scan found under the Claude projects root. `session_id` is the file
/// STEM (A5-validated); `project` is the parent slug directory NAME — Claude Code's
/// cwd-encoding, used verbatim as a stable per-repo key (decoding it to a real filesystem
/// path is unofficial and NOT required for snapshot/project-scoping). `mtime` is the file's
/// last-modified epoch second. `sha_if_known` is the stored transcript sha ADOPTED via a
/// cheap mtime pre-filter (see `build_current_map`) — `None` means "unknown, must render".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranscriptCandidate {
    pub session_id: String,
    pub project: String,
    pub path: PathBuf,
    pub mtime: i64,
    pub sha_if_known: Option<String>,
}

/// A currently-captured session as the decision core sees it: the stored transcript sha (from the
/// signed `session_captured` event), whether its `.md` view exists on disk right now, and whether
/// that `.md` is a RECOVERY STUB (A7 heal-(b) placeholder, marked `recovered: false`). A stub
/// carries the event's real sha and a fresh mtime, so it is sha- and mtime-indistinguishable from
/// a real render — `md_is_stub` is the ONLY signal that forces the sweeper to re-render it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CurrentCapture {
    pub sha256: String,
    pub md_exists: bool,
    pub md_is_stub: bool,
}

/// Everything [`decide_sweep`] needs, as plain data — NO filesystem, NO engine, NO clock
/// (`now` is passed in). This is where Contracts 1 & 2 live and are unit-tested.
pub struct SweepDecisionInput {
    /// The daemon's onboarding verdict (A8 gate).
    pub onboarded: bool,
    /// The sticky ongoing-capture flag (A8, default OFF).
    pub capture_enabled: bool,
    /// One-time consent to import transcripts predating `capture_enabled_at` (A8, default OFF).
    pub backfill_consented: bool,
    /// The instant capture last flipped ON. Contract 2: only meaningful (only read) when
    /// `capture_enabled` is true. A missing value is resolved to `i64::MAX` by the caller (no
    /// forward window → only backfill imports history).
    pub capture_enabled_at: i64,
    /// Wall-clock epoch second, read at the daemon boundary and passed in (core stays clock-free).
    pub now: i64,
    /// The transcripts the scan found.
    pub candidates: Vec<TranscriptCandidate>,
    /// `session_id -> current capture` for the sessions the signed log currently holds.
    pub already_current: BTreeMap<String, CurrentCapture>,
    /// Owner-deleted `session_id`s — never re-captured (I9).
    pub tombstoned: BTreeSet<String>,
    /// Per-sweep cap on the returned selection ([`CAPTURE_PER_SWEEP`] in production).
    pub cap: usize,
}

/// What one [`run_sweep_once`] did. All-zero + `gated_off` on a non-connected brain (I10).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SweepReport {
    /// `onboarded && capture_enabled` was false — nothing scanned, nothing created (I10).
    pub gated_off: bool,
    /// Valid transcript candidates the scan produced.
    pub scanned: usize,
    /// Files skipped because their stem failed the A5 session-id allowlist.
    pub invalid_ids: usize,
    /// Candidates [`decide_sweep`] selected to (re)capture this sweep.
    pub decided: usize,
    /// Successfully rendered + stored.
    pub captured: usize,
    /// Confined-open or render failures (skipped, counted, never fatal).
    pub render_failures: usize,
    /// Store failures (including the rare I9 reject race — the engine is the durable backstop).
    pub store_failures: usize,
    /// What the post-capture [`heal_orphans`] reconciled.
    pub heal: HealReport,
}

/// PURE sweep selection (the unit-tested core). Clock-free: `now` is in `input`. Returns the
/// candidates to (re)capture, deterministically ordered oldest-transcript-first (so backfill
/// never starves an old session) and capped at `input.cap`. Contracts 1 & 2 live here.
pub fn decide_sweep(input: &SweepDecisionInput) -> Vec<TranscriptCandidate> {
    // Top-level gate (I10 + Contract 2): a non-onboarded or capture-disabled brain selects
    // NOTHING — and, critically, `capture_enabled_at` is not consulted at all (it is stale on a
    // disabled brain), because `include_candidate` is never reached.
    if !input.onboarded || !input.capture_enabled {
        return Vec::new();
    }
    let mut selected: Vec<&TranscriptCandidate> =
        input.candidates.iter().filter(|c| include_candidate(c, input)).collect();
    // Deterministic, fair order: oldest transcript first (backfill never starves an old session),
    // `session_id` as a stable tiebreak. Then cap.
    selected.sort_by(|a, b| a.mtime.cmp(&b.mtime).then_with(|| a.session_id.cmp(&b.session_id)));
    selected.into_iter().take(input.cap).cloned().collect()
}

/// Whether one candidate should be (re)captured this sweep. Assumes the top-level
/// `onboarded && capture_enabled` gate already passed (Contract 2).
fn include_candidate(c: &TranscriptCandidate, input: &SweepDecisionInput) -> bool {
    // I9: an owner-deleted session is gone forever — never resurrect it.
    if input.tombstoned.contains(&c.session_id) {
        return false;
    }
    // Quiet floor: a file modified within QUIET_SECS of now is still being written; skip it
    // this sweep. `saturating_sub` yields a NEGATIVE value for a future mtime (clock skew) —
    // still < QUIET_SECS, so such a file is excluded too — the safe direction.
    if input.now.saturating_sub(c.mtime) < QUIET_SECS {
        return false;
    }
    match input.already_current.get(&c.session_id) {
        // Already captured → MAINTENANCE, not a new import, so the backfill/forward window does
        // NOT apply. Contract 1: skip the re-render ONLY when the .md exists, is a REAL render
        // (NOT a recovery stub), AND the content is unchanged (adopted-same sha). A MISSING .md
        // (out-of-band delete, crash heal window) OR a recovery STUB (present but a placeholder
        // body carrying the real sha) MUST re-render so store writes the real body — the engine
        // then dedups the event to a no-op. Without the stub check a stub is sha-indistinguishable
        // from a real render and would be skipped forever (cap-starved disaster restore).
        Some(cur) => {
            let same_sha = c.sha_if_known.as_deref() == Some(cur.sha256.as_str());
            !(cur.md_exists && !cur.md_is_stub && same_sha)
        }
        // New (never-captured, non-tombstoned): the M4 backfill window. Import iff it was written
        // after capture was enabled (forward capture) OR the owner consented to backfill history.
        None => c.mtime >= input.capture_enabled_at || input.backfill_consented,
    }
}

/// Run ONE sweep: gate → scan → reconcile → decide → capture → heal. `now` is the wall-clock
/// epoch second (read by [`spawn`] at the daemon boundary). Never panics; per-file errors are
/// swallowed + counted so one bad transcript never aborts the sweep.
pub async fn run_sweep_once(engine: &EngineHandle, data_dir: &Path, now: i64) -> SweepReport {
    let mut report = SweepReport::default();

    // (1) Gate. Contract 2: confirm `onboarded && capture_enabled` BEFORE reading
    // `capture_enabled_at`/`backfill_consented` (both stale on a disabled brain). A gated-off
    // sweep scans nothing, creates nothing, heals nothing (I10 — non-connected users unaffected).
    let onboarded = crate::identity::is_onboarded(data_dir);
    if !onboarded || !engine.capture_enabled(onboarded).await.unwrap_or(false) {
        report.gated_off = true;
        return report;
    }
    let backfill_consented = engine.backfill_consented(onboarded).await.unwrap_or(false);
    // An enabled brain always has a `capture_enabled_at`; a missing one is degenerate — fail
    // safe by treating it as i64::MAX (no forward window; only backfill imports history).
    let capture_enabled_at = engine
        .capture_enabled_at(onboarded)
        .await
        .ok()
        .flatten()
        .unwrap_or(i64::MAX);

    // (2) Scan the Claude projects root: one level of project dirs, then *.jsonl leaves (A5 D3:
    // the one-level walk + `valid_session_id` on the stem bounds path depth/components).
    let root = claude_projects_root();
    let mut candidates = scan_candidates(&root, &mut report);

    // (3) Reconcile against the signed log: current captures (sha + md_exists) and tombstones.
    let sessions_dir = data_dir.join("sessions");
    let already_current = build_current_map(engine, &sessions_dir, &mut candidates).await;
    let tombstoned: BTreeSet<String> = engine
        .deleted_session_ids()
        .await
        .unwrap_or_default()
        .into_iter()
        .collect();

    // (4) Decide (pure).
    let decided = decide_sweep(&SweepDecisionInput {
        onboarded,
        capture_enabled: true,
        backfill_consented,
        capture_enabled_at,
        now,
        candidates,
        already_current,
        tombstoned,
        cap: CAPTURE_PER_SWEEP,
    });
    report.decided = decided.len();

    // (5) Capture: confined open → render → store (file-then-event). Errors are counted, never fatal.
    let bounds = render_bounds();
    for cand in &decided {
        let file = match open_transcript_confined(&root, &cand.path) {
            Ok(f) => f,
            Err(e) => {
                report.render_failures += 1;
                eprintln!("sweeper: confined open failed: {e}");
                continue;
            }
        };
        let rendered = match render_transcript(file, &bounds) {
            Ok(r) => r,
            Err(e) => {
                report.render_failures += 1;
                eprintln!("sweeper: render failed: {e}");
                continue;
            }
        };
        let id = CaptureIdentity {
            session_id: cand.session_id.clone(),
            project: cand.project.clone(),
            tool: CAPTURE_TOOL.to_string(),
        };
        match store_capture(engine, data_dir, &id, &rendered).await {
            Ok(()) => report.captured += 1,
            Err(e) => {
                report.store_failures += 1;
                eprintln!("sweeper: store failed: {e}");
            }
        }
    }

    // (6) Heal crash windows AFTER the capture pass (Contract 1 — see the module docs): a
    // dangling event's .md was still MISSING at decide time, so it re-rendered above; heal now
    // only regenerates placeholders for events whose transcript is truly gone, and reconciles
    // any orphan .md the scan didn't cover.
    report.heal = heal_orphans(engine, data_dir).await.unwrap_or_default();

    report
}

/// Spawn the background sweep loop on the daemon's tokio runtime (mirrors
/// `engine::scheduler::spawn`). `interval`'s first tick fires immediately, so a just-restarted
/// daemon sweeps (heals + backfills) at once. `MissedTickBehavior::Skip` keeps a slow sweep
/// from bursting catch-up wakes. The wall clock is read HERE — the boundary — so the core
/// stays clock-free. Errors are already swallowed inside `run_sweep_once`; a panic in a
/// spawned task can't take down the daemon (each connection/scheduler task is independent).
pub fn spawn(engine: Arc<EngineHandle>, data_dir: PathBuf) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(SWEEP_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await; // immediate on the first iteration → a boot sweep.
            let now = system_time_to_epoch(Some(SystemTime::now()));
            let report = run_sweep_once(&engine, &data_dir, now).await;
            // Stay quiet on a gated-off / no-op sweep; surface only real work so the autonomous
            // loop is observable without spamming the launchd/systemd log.
            if report.captured > 0
                || report.render_failures > 0
                || report.store_failures > 0
                || report.heal != HealReport::default()
            {
                eprintln!(
                    "sweeper: captured {} / decided {} (scanned {}, invalid-id {}, render-fail {}, \
                     store-fail {}); healed {}+{}",
                    report.captured,
                    report.decided,
                    report.scanned,
                    report.invalid_ids,
                    report.render_failures,
                    report.store_failures,
                    report.heal.orphan_files_captured,
                    report.heal.dangling_events_regenerated,
                );
            }
        }
    });
}

/// Scan `<root>/<project-slug>/<session-id>.jsonl` — ONE level of project dirs, then `.jsonl`
/// leaves. Ignores non-dirs, non-`.jsonl`, and anything unreadable (a non-connected / absent
/// root is simply empty — I10: no side effects). Invalid stems are counted, not built into paths.
fn scan_candidates(root: &Path, report: &mut SweepReport) -> Vec<TranscriptCandidate> {
    let mut out = Vec::new();
    let Ok(projects) = std::fs::read_dir(root) else {
        return out; // absent/unreadable root → nothing to sweep.
    };
    for project in projects.flatten() {
        if !project.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue; // only project directories hold transcripts.
        }
        let slug = project.file_name().to_string_lossy().into_owned();
        let Ok(entries) = std::fs::read_dir(project.path()) else {
            continue; // unreadable project dir — skip, don't abort the sweep.
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue; // ignore non-transcripts.
            }
            // session_id = the file STEM, A5-validated before it is used as a key/path component.
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                report.invalid_ids += 1;
                continue;
            };
            if !valid_session_id(stem) {
                report.invalid_ids += 1;
                continue;
            }
            let mtime = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .map(|t| system_time_to_epoch(Some(t)))
                .unwrap_or(0);
            report.scanned += 1;
            out.push(TranscriptCandidate {
                session_id: stem.to_string(),
                project: slug.clone(),
                path,
                mtime,
                sha_if_known: None,
            });
        }
    }
    out
}

/// Build the `already_current` map (sha + md_exists) AND populate each current candidate's
/// `sha_if_known` via a cheap mtime pre-filter — NO re-hashing. If a current session's
/// transcript has NOT been modified since we wrote its `.md` (`transcript mtime <= .md mtime`),
/// the on-disk render is byte-current, so we ADOPT the stored sha and [`decide_sweep`] skips the
/// re-render. Any doubt — `.md` missing, transcript touched since, session not current — leaves
/// sha `None` → the candidate is (re)rendered and the engine's own sha dedup collapses a
/// genuinely-identical re-render to a no-op. The skip is gated on `md_exists`, so a session whose
/// `.md` is missing is NEVER skipped (Contract 1's placeholder heal).
async fn build_current_map(
    engine: &EngineHandle,
    sessions_dir: &Path,
    candidates: &mut [TranscriptCandidate],
) -> BTreeMap<String, CurrentCapture> {
    let current = engine.current_sessions().await.unwrap_or_default();
    let mut map: BTreeMap<String, CurrentCapture> = BTreeMap::new();
    for cs in &current {
        let md_path = sessions_dir.join(format!("{}.md", cs.session_id));
        let md_exists = md_path.exists();
        // A present .md that is a RECOVERY STUB (heal-(b) placeholder) is treated as "not a real
        // render" for the skip decision, so its still-present transcript re-renders next sweep.
        // Cheap bounded front-matter read; only for the sessions that actually have a .md.
        let md_is_stub = md_exists && crate::capture::store::is_recovery_stub(&md_path);
        map.insert(
            cs.session_id.clone(),
            CurrentCapture { sha256: cs.sha256.clone(), md_exists, md_is_stub },
        );
    }
    for cand in candidates.iter_mut() {
        let Some(cur) = map.get(&cand.session_id) else {
            continue; // not currently captured → must render (new / backfill).
        };
        if !cur.md_exists {
            continue; // Contract 1: missing .md must re-render — never adopt a skip-sha.
        }
        let md_mtime = std::fs::metadata(sessions_dir.join(format!("{}.md", cand.session_id)))
            .ok()
            .and_then(|m| m.modified().ok())
            .map(|t| system_time_to_epoch(Some(t)))
            .unwrap_or(i64::MIN); // unreadable .md mtime → never adopt (re-render).
        if cand.mtime <= md_mtime {
            cand.sha_if_known = Some(cur.sha256.clone());
        }
    }
    map
}

/// A `SystemTime` → Unix epoch second. A missing/pre-epoch time → 0 (reads as "very old", so
/// such a file is only ever imported under explicit backfill consent — the safe default). Shared
/// with `crate::conflict::sweeper::spawn`, which reads the same wall clock at its loop boundary.
pub(crate) fn system_time_to_epoch(t: Option<SystemTime>) -> i64 {
    t.and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(session_id: &str, mtime: i64, sha_if_known: Option<&str>) -> TranscriptCandidate {
        TranscriptCandidate {
            session_id: session_id.to_string(),
            project: "-Users-x-repo".to_string(),
            path: PathBuf::from(format!("/root/-Users-x-repo/{session_id}.jsonl")),
            mtime,
            sha_if_known: sha_if_known.map(str::to_string),
        }
    }

    /// A fully-enabled input at `now = 10_000`, `capture_enabled_at = 5_000`, no candidates.
    /// Individual tests override the fields they exercise.
    fn base_input() -> SweepDecisionInput {
        SweepDecisionInput {
            onboarded: true,
            capture_enabled: true,
            backfill_consented: false,
            capture_enabled_at: 5_000,
            now: 10_000,
            candidates: Vec::new(),
            already_current: BTreeMap::new(),
            tombstoned: BTreeSet::new(),
            cap: CAPTURE_PER_SWEEP,
        }
    }

    fn ids(v: &[TranscriptCandidate]) -> Vec<String> {
        v.iter().map(|c| c.session_id.clone()).collect()
    }

    /// The A5-validated `cur` capture as a `(sha, md_exists, md_is_stub)` current-fold entry.
    fn current(sha: &str, md_exists: bool, md_is_stub: bool) -> BTreeMap<String, CurrentCapture> {
        BTreeMap::from([(
            "cur".to_string(),
            CurrentCapture { sha256: sha.to_string(), md_exists, md_is_stub },
        )])
    }

    #[test]
    fn decide_gate_off_selects_nothing() {
        // I10 + Contract 2: not onboarded OR capture disabled → empty, and `capture_enabled_at`
        // is POISONED so a regression that reads it behind the gate would misbehave — the empty
        // result proves the timestamp is never consulted.
        for (onboarded, enabled) in [(false, true), (true, false), (false, false)] {
            let input = SweepDecisionInput {
                onboarded,
                capture_enabled: enabled,
                capture_enabled_at: i64::MIN, // poison — must not be read
                backfill_consented: true,
                candidates: vec![cand("s1", 9_000, None)], // would import if the gate leaked
                ..base_input()
            };
            assert!(
                decide_sweep(&input).is_empty(),
                "gated off → nothing (onboarded={onboarded}, enabled={enabled})"
            );
        }
    }

    #[test]
    fn decide_quiet_floor_excludes_still_fresh_files() {
        // A file modified within QUIET_SECS of now is still being written → excluded this sweep.
        let mut input = base_input();
        input.now = 10_000;
        input.backfill_consented = true;
        input.candidates = vec![
            cand("fresh", 10_000 - (QUIET_SECS - 1), None), // 599 s old → too fresh
            cand("quiet", 10_000 - QUIET_SECS, None),       // exactly 600 s old → captured
        ];
        assert_eq!(ids(&decide_sweep(&input)), vec!["quiet"], "the still-fresh file is skipped this sweep");
    }

    #[test]
    fn decide_backfill_window_gates_pre_enable_history() {
        // M4: a transcript older than capture_enabled_at is imported IFF backfill was consented.
        let old = cand("old", 4_000, None); // mtime 4_000 < capture_enabled_at 5_000

        let mut declined = base_input();
        declined.backfill_consented = false;
        declined.candidates = vec![old.clone()];
        assert!(decide_sweep(&declined).is_empty(), "pre-enable history is NOT imported without backfill consent");

        let mut consented = base_input();
        consented.backfill_consented = true;
        consented.candidates = vec![old];
        assert_eq!(ids(&decide_sweep(&consented)), vec!["old"], "backfill consent imports the pre-enable transcript");
    }

    #[test]
    fn decide_forward_capture_ignores_backfill() {
        // mtime >= capture_enabled_at is imported regardless of backfill (a going-forward capture).
        let mut forward = base_input();
        forward.backfill_consented = false;
        forward.candidates = vec![cand("fwd", 5_000, None)]; // exactly at the enable instant
        assert_eq!(ids(&decide_sweep(&forward)), vec!["fwd"], "a post-enable transcript is forward-captured");
    }

    #[test]
    fn decide_tombstone_never_resurrects() {
        // I9: an owner-deleted session is never re-captured, even mid forward window.
        let mut tomb = base_input();
        tomb.candidates = vec![cand("dead", 9_000, None)];
        tomb.tombstoned = BTreeSet::from(["dead".to_string()]);
        assert!(decide_sweep(&tomb).is_empty(), "a tombstoned session is never re-captured");
    }

    #[test]
    fn decide_contract1_skips_unchanged_real_render() {
        // Cheap skip: current + SAME sha + md_exists + REAL render (not a stub) → excluded.
        let mut skip = base_input();
        skip.candidates = vec![cand("cur", 9_000, Some("aa"))];
        skip.already_current = current("aa", /*md_exists=*/ true, /*md_is_stub=*/ false);
        assert!(decide_sweep(&skip).is_empty(), "an unchanged, on-disk real render is skipped");
    }

    #[test]
    fn decide_contract1_rerenders_changed_missing_or_stub() {
        // Contract 1's three re-render cases — each must be (re)captured so store writes the real
        // body first (the engine then dedups the event to a no-op where the sha is unchanged).

        // (a) Changed content: current + DIFFERENT sha (still md_exists) → supersede + re-render.
        let mut changed = base_input();
        changed.candidates = vec![cand("cur", 9_000, Some("bb"))];
        changed.already_current = current("aa", true, false);
        assert_eq!(ids(&decide_sweep(&changed)), vec!["cur"], "changed bytes supersede — re-rendered");

        // (b) PLACEHOLDER HEAL: current + SAME sha but md MISSING → INCLUDED (out-of-band deletion
        // or a crash heal window). The re-render lets store write the real body first.
        let mut heal = base_input();
        heal.candidates = vec![cand("cur", 9_000, Some("aa"))];
        heal.already_current = current("aa", /*md_exists=*/ false, false);
        assert_eq!(ids(&decide_sweep(&heal)), vec!["cur"], "a missing .md re-renders to heal the body (Contract 1!)");

        // (c) RECOVERY STUB: current + SAME sha + md_exists BUT the .md is a heal STUB (real sha,
        // placeholder body) → INCLUDED. A stub is sha-indistinguishable from a real render, so
        // without this it would be skipped forever (cap-starved disaster restore).
        let mut stub = base_input();
        stub.candidates = vec![cand("cur", 9_000, Some("aa"))];
        stub.already_current = current("aa", true, /*md_is_stub=*/ true);
        assert_eq!(ids(&decide_sweep(&stub)), vec!["cur"], "an unrecovered stub re-renders (self-heal across sweeps)");
    }

    #[test]
    fn decide_caps_at_cap_oldest_first() {
        // The selection never exceeds `cap`, and is oldest-transcript-first (fair backfill).
        let mut capped = base_input();
        capped.backfill_consented = true;
        capped.cap = 2;
        capped.candidates = vec![
            cand("c_new", 9_500, None),
            cand("a_old", 9_000, None),
            cand("b_mid", 9_200, None),
        ];
        let picked = decide_sweep(&capped);
        assert_eq!(picked.len(), 2, "capped at cap");
        assert_eq!(ids(&picked), vec!["a_old", "b_mid"], "oldest transcripts first so nothing starves");
    }
}
