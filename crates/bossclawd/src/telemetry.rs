//! Recall-miss telemetry (SP3 A12) — the rung-1/2 retrieval-floor tuning signal (spec §8) AND, since Rung-4 R4-A,
//! the ACTIVE input to the reflect loop, which reads these miss queries each quiet tick to repair coverage
//! gaps (design §2.4). With cloud consent ON, gathered material may egress under the existing consent.
//!
//! Every recall the daemon serves is recorded here: a one-line append to the append-only
//! `<data_dir>/telemetry/recall.jsonl` and a durable bump of the lifetime counters in
//! `<data_dir>/telemetry/counters.json`. The `RecallStats` op (App-only) reads these back for the
//! Library's read-only tuning panel: how many recalls, how many found nothing, and the most recent
//! miss QUERIES so a human can see what the memory is failing to answer.
//!
//! # Best-effort by contract
//! [`Telemetry::record`] is INFALLIBLE: any I/O error is logged (`eprintln!`, consistent with the
//! rest of the daemon) and swallowed. A telemetry failure must NEVER fail a recall (critic m2) —
//! the recall has already completed by the time we record, and losing a telemetry line is strictly
//! better than turning a served recall into an error.
//!
//! # Rotation-proof counters
//! The lifetime `total`/`misses` counters live in `counters.json`, rewritten atomically on each
//! record; the recent-miss ring lives in its own `recent_misses.json`. Both are INDEPENDENT of the
//! `recall.jsonl` line log, so rotating (or truncating) that log NEVER resets a count or drops a
//! recent miss (critic m1).
//!
//! # Privacy surface (documented at the source)
//! `recall.jsonl` and `recent_misses.json` store the user's RAW QUERY strings in plaintext. They
//! are local-only and born `0600` under a `0700` dir. They record QUERIES ONLY — never a recall
//! RESULT, title, or note text — so a deleted session's content can never resurface through
//! telemetry (spec §7b honesty). Plan B's settings copy discloses this store to the user.
//!
//! Since Rung-4 R4-A the reflect loop READS these miss queries (never the results) to drive dossier
//! refreshes; queries stay local unless the owner has enabled the cloud reasoner (I2). Still QUERIES ONLY.
//!
//! Unix-only, to match the capture siblings and reuse `bossclawd-paths`' `#[cfg(unix)]` 0600/0700
//! primitives (no new dependency).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use bossclawd_paths::{atomic_write_0600, make_private_dir};
use bossclawd_proto::types::{RecallMissWire, RecallStatsWire};
use serde::{Deserialize, Serialize};

/// How many recent miss queries the ring retains (newest-first).
const RECENT_MISSES_CAP: usize = 20;
/// Roll `recall.jsonl` to `recall.jsonl.1` once it grows past this many bytes. The durable counters
/// + recent-miss ring live in their own files, so rotation loses only OLD raw lines (critic m1).
const ROTATE_BYTES: u64 = 5 * 1024 * 1024;

const TELEMETRY_DIR: &str = "telemetry";
const LINE_LOG: &str = "recall.jsonl";
const COUNTERS_FILE: &str = "counters.json";
const RECENT_MISSES_FILE: &str = "recent_misses.json";

/// Serializes the counters + recent-miss + rotation read-modify-write across concurrent recalls in
/// one daemon process, so two simultaneous records can't lose a count or write a torn ring. The
/// daemon is single-owner, so there is no cross-process writer to coordinate with. The lock is held
/// only for the (tiny, fast) file operations of one record — negligible next to a recall's cost.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// The durable lifetime counters, persisted as `counters.json` and rewritten atomically on each
/// record so they survive line-log rotation.
#[derive(Serialize, Deserialize, Default, Clone, Copy)]
struct Counters {
    total: u64,
    misses: u64,
}

/// Handle to a brain's recall-telemetry store under `<data_dir>/telemetry/`. Cheap to hold (just a
/// dir path), so the dispatch constructs one per `Recall` / `RecallStats` call rather than threading
/// shared state through the accept loop.
pub struct Telemetry {
    dir: PathBuf,
}

impl Telemetry {
    /// Open (creating, born `0700`, if absent) the telemetry dir under `data_dir`. A pre-existing
    /// dir keeps its perms untouched ([`make_private_dir`] never loosens).
    pub fn open(data_dir: &Path) -> io::Result<Self> {
        let dir = data_dir.join(TELEMETRY_DIR);
        make_private_dir(&dir)?;
        Ok(Self { dir })
    }

    /// Record one recall's outcome. INFALLIBLE by contract: any I/O error is logged and swallowed
    /// so a telemetry failure can never fail the recall that produced it (critic m2). `hits` is the
    /// number of results the recall returned (0 == a miss); `top_score` is the best hit's score, if
    /// any. The `at` timestamp is read here, at the daemon boundary (telemetry is not
    /// determinism-sensitive, so a clock read is fine — the recall dispatch core stays clock-free).
    pub fn record(&self, query: &str, hits: usize, top_score: Option<f32>) {
        if let Err(e) = self.record_inner(query, hits, top_score) {
            eprintln!("telemetry: recall telemetry record failed (non-fatal): {e}");
        }
    }

    fn record_inner(&self, query: &str, hits: usize, top_score: Option<f32>) -> io::Result<()> {
        let at = now_epoch();
        let _guard = WRITE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);

        // Durable counters FIRST (the headline tuning signal): bump total, and on a miss bump
        // misses. Both live in their own atomically-rewritten files, so rotating the raw line log
        // below never resets them (critic m1).
        let is_miss = hits == 0;
        let mut counters = self.read_counters();
        counters.total = counters.total.saturating_add(1);
        if is_miss {
            counters.misses = counters.misses.saturating_add(1);
        }
        // Write the counters BEFORE the recent-miss ring: a crash between the two then leaves the
        // more-durable headline number "counted but not yet ringed" rather than "ringed but
        // uncounted" (the ring is a best-effort recent view; the counters are the signal).
        self.write_counters(&counters)?;
        if is_miss {
            self.push_recent_miss(query, at)?;
        }

        // Then the raw forensic line (append-only, born 0600, O_APPEND), and finally best-effort
        // rotation once it grows past the cap.
        self.append_line(query, hits, top_score, at)?;
        self.rotate_if_large()
    }

    /// Read back the assembled stats for the `RecallStats` op: lifetime counters + the recent-miss
    /// ring (queries only, newest-first, ≤ [`RECENT_MISSES_CAP`]). Absent/corrupt files read as the
    /// empty default rather than an error, so a fresh brain reports `0/0/[]`.
    pub fn stats(&self) -> io::Result<RecallStatsWire> {
        let _guard = WRITE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let counters = self.read_counters();
        let recent_misses = self.read_recent_misses();
        Ok(RecallStatsWire { total: counters.total, misses: counters.misses, recent_misses })
    }

    /// Roll the line log to `recall.jsonl.1` unconditionally (the test hook; the size-triggered path
    /// shares [`Self::rotate`]). Counters + the recent-miss ring are untouched — they live in their
    /// own files (critic m1).
    pub fn force_rotate(&self) -> io::Result<()> {
        let _guard = WRITE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        self.rotate()
    }

    // ── Internal file helpers. All callers that mutate the counters/ring/log hold `WRITE_LOCK`. ──

    /// Append one JSON line describing the recall to the append-only log, born `0600` (the creation
    /// `mode` applies only when `O_CREAT` creates the file; subsequent opens keep the 0600 perms and
    /// just `O_APPEND`). The line records the QUERY (spec §8) — never any result/title/note text.
    fn append_line(&self, query: &str, hits: usize, top_score: Option<f32>, at: i64) -> io::Result<()> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;

        let line = serde_json::json!({ "at": at, "query": query, "hits": hits, "top_score": top_score });
        let mut bytes = serde_json::to_vec(&line).map_err(io::Error::other)?;
        bytes.push(b'\n');
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(self.line_log())?;
        f.write_all(&bytes)
    }

    /// Read the durable counters, defaulting to `0/0` on an absent or unparseable file (best-effort).
    fn read_counters(&self) -> Counters {
        std::fs::read(self.counters_file())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Atomically rewrite the durable counters (born 0600, no world-readable window).
    fn write_counters(&self, counters: &Counters) -> io::Result<()> {
        let bytes = serde_json::to_vec(counters).map_err(io::Error::other)?;
        atomic_write_0600(&self.counters_file(), &bytes)
    }

    /// Read the recent-miss ring, defaulting to empty on an absent or unparseable file.
    fn read_recent_misses(&self) -> Vec<RecallMissWire> {
        std::fs::read(self.recent_misses_file())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Push one miss query onto the front of the ring (newest-first), cap at [`RECENT_MISSES_CAP`],
    /// and atomically rewrite it (born 0600). Stores the QUERY only.
    fn push_recent_miss(&self, query: &str, at: i64) -> io::Result<()> {
        let mut ring = self.read_recent_misses();
        ring.insert(0, RecallMissWire { query: query.to_string(), at });
        ring.truncate(RECENT_MISSES_CAP);
        let bytes = serde_json::to_vec(&ring).map_err(io::Error::other)?;
        atomic_write_0600(&self.recent_misses_file(), &bytes)
    }

    /// Roll the line log to `.1` when it has grown past [`ROTATE_BYTES`]. No-op if it is absent or
    /// still small. Best-effort housekeeping — never touches the durable counters/ring.
    fn rotate_if_large(&self) -> io::Result<()> {
        let too_big =
            std::fs::metadata(self.line_log()).map(|m| m.len() > ROTATE_BYTES).unwrap_or(false);
        if too_big {
            self.rotate()?;
        }
        Ok(())
    }

    /// Roll the current line log to `recall.jsonl.1` (single generation, replacing any prior `.1`).
    /// `rename` preserves the source's `0600` mode. Absent line log → nothing to rotate. Assumes the
    /// caller holds [`WRITE_LOCK`] (it is not reentrant — do not lock here).
    fn rotate(&self) -> io::Result<()> {
        let log = self.line_log();
        if !log.exists() {
            return Ok(());
        }
        std::fs::rename(&log, self.dir.join(format!("{LINE_LOG}.1")))
    }

    fn line_log(&self) -> PathBuf {
        self.dir.join(LINE_LOG)
    }
    fn counters_file(&self) -> PathBuf {
        self.dir.join(COUNTERS_FILE)
    }
    fn recent_misses_file(&self) -> PathBuf {
        self.dir.join(RECENT_MISSES_FILE)
    }
}

/// Unix-epoch seconds, read at the daemon boundary. Saturates to `0` on the impossible pre-1970
/// clock (telemetry timestamps are advisory, so a fail-safe floor beats propagating an error).
fn now_epoch() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}
