//! M6c Task 10 — the live filesystem watcher + debounced self-driver (Layer 2, §6).
//!
//! This is the hands-off layer: a `notify` watcher over the active read-grant source
//! roots that, on a debounced burst of filesystem changes, re-ingests the changed
//! sources and runs one evolve tick. The evolve tick's M6c mandate phase then proposes
//! writes — but the WATCHER ITSELF CANNOT WRITE. Every write still flows through M6a's
//! gate and stays human-confirmed (§5). The watcher only decides *when* to look.
//!
//! # Single-writer invariant (§6.3 / security finding I4 — the #1 rule)
//!
//! The whole engine serializes writes on [`EventLog`]'s inner mutex + rename-lock.
//! Opening a SECOND SQLCipher connection over the same DB file would be a second writer
//! and break that. So [`MandateWatcher`] holds ONE [`Arc<EventLog>`] — a clone of the
//! caller's single instance — and calls its methods. It NEVER constructs or clones a
//! second [`EventLog`]/`Store`. The `Arc` is shared with the watcher thread; the inner
//! mutex serializes the thread's `drive_once` against any foreground caller.
//!
//! # Why `notify` is fine under `#![forbid(unsafe_code)]`
//!
//! `notify`'s platform backends (FSEvents/inotify/kqueue FFI) contain `unsafe`, but that
//! `unsafe` lives INSIDE the `notify` crate, not here. `#![forbid(unsafe_code)]` is a
//! per-crate lint; bossclaw-core writes no `unsafe`, so the attribute still holds. The
//! whole module is `#[cfg(unix)]`, as is the `notify` dependency, so a non-unix target
//! never even resolves `notify`.
//!
//! # Testability
//!
//! [`MandateWatcher::drive_once`] and the [`Coalescer`] (the bounded-channel + debounce
//! decision) are directly callable WITHOUT a running OS watcher, so the hermetic tests
//! exercise the watcher → proposer pipeline (and the storm-coalesce proof) with no real
//! filesystem events.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use notify::{RecursiveMode, Watcher};

use crate::embed::Embedder;
use crate::error::BossclawError;
use crate::evolve::{debounce_due, EVOLVE_DEBOUNCE_MS};
use crate::ingest::ParserRouter;
use crate::log::EventLog;
use crate::reason::Reasoner;

/// The bounded channel depth for the raw `notify` event stream (§6, the "storm can't
/// OOM" bound). Filesystem bursts (a `git checkout`, an editor save-all) can deliver
/// thousands of events in a blink; we only need to know *that* something changed, not to
/// retain every event. So the channel is intentionally TINY: once it is full, further
/// events are dropped at [`SyncSender::try_send`] and a single "rescan" flag is set
/// instead of growing the queue unboundedly. A debounced tick then re-ingests the whole
/// tree regardless of which individual events were dropped — coalescing is lossless for
/// our purposes because `ingest_all` always rescans every active grant.
pub const WATCH_CHANNEL_BOUND: usize = 256;

/// The directly-testable debounce-coalesce core (§6, proof 8), split out from the OS
/// event source so it can be unit-tested with no real `notify` watcher.
///
/// It owns a BOUNDED [`sync_channel`]: the producer side ([`Coalescer::offer`], driven
/// by the `notify` callback in production) does a non-blocking [`SyncSender::try_send`];
/// on a full channel it sets [`Coalescer::rescan_requested`] rather than blocking or
/// growing the queue (the OOM guard). The consumer side coalesces: [`Coalescer::offer`]
/// also stamps the last-event clock, [`Coalescer::drive_due`] reports whether the
/// debounce window has elapsed since that stamp, and [`Coalescer::mark_driven`] clears
/// the pending/rescan state after a drive. The net effect: a burst of N events collapses
/// to ONE drive per quiet window, never N.
pub struct Coalescer {
    /// Producer handle for the bounded event channel. Kept so [`offer`](Self::offer) can
    /// `try_send` a unit token; we never read the tokens individually — the channel is a
    /// bounded buffer whose only job is to coalesce + back-pressure into the rescan flag.
    tx: SyncSender<()>,
    /// Consumer handle. Drained (counted, not inspected) by [`mark_driven`](Self::mark_driven)
    /// so the queued depth resets when a drive consumes the coalesced burst.
    rx: Receiver<()>,
    /// True once at least one event has arrived and not yet been driven.
    pending: bool,
    /// True once the bounded channel overflowed (a burst exceeded [`WATCH_CHANNEL_BOUND`]
    /// / the test bound) — the driver should do a full rescan, which `ingest_all` already
    /// does, so this just guarantees `pending` stays set even when events were dropped.
    rescan: bool,
    /// Monotonic-ms stamp of the most recent [`offer`](Self::offer). The debounce window
    /// is measured from here, so the tick fires only after the burst goes quiet.
    last_event_ms: u128,
}

impl Coalescer {
    /// Build a coalescer with a bounded channel of depth `bound`. Production uses
    /// [`WATCH_CHANNEL_BOUND`]; tests pass a tiny bound to exercise the overflow → rescan
    /// path deterministically.
    pub fn new(bound: usize) -> Self {
        let (tx, rx) = sync_channel(bound);
        Self { tx, rx, pending: false, rescan: false, last_event_ms: 0 }
    }

    /// Record one filesystem event at clock `now_ms` (the `notify` callback calls this).
    /// Non-blocking: on a full channel it sets the rescan flag instead of blocking the
    /// watcher thread or growing the queue (the OOM guard). Always marks work pending and
    /// stamps the debounce clock so the window is measured from the LAST event in a burst.
    pub fn offer_at(&mut self, now_ms: u128) {
        self.pending = true;
        self.last_event_ms = now_ms;
        match self.tx.try_send(()) {
            Ok(()) => {}
            // Full or disconnected: drop the token, request a rescan. `ingest_all`
            // rescans every grant, so a dropped token loses no information — the next
            // drive still sees the change.
            Err(TrySendError::Full(())) | Err(TrySendError::Disconnected(())) => {
                self.rescan = true;
            }
        }
    }

    /// [`offer_at`](Self::offer_at) with the process monotonic clock — the production
    /// entry the `notify` callback uses. (Tests call `offer_at` with a fixed clock.)
    pub fn offer(&mut self) {
        self.offer_at(monotonic_ms());
    }

    /// Whether a debounced drive is due at clock `now_ms`: work is pending AND at least
    /// [`EVOLVE_DEBOUNCE_MS`]-many (here `debounce_ms`) have elapsed since the last event.
    /// Uses the PURE [`debounce_due`] from `evolve.rs` so the scheduler decision is
    /// single-sourced and saturating against a backwards clock.
    pub fn drive_due(&self, now_ms: u128, debounce_ms: u128) -> bool {
        self.pending && debounce_due(now_ms, self.last_event_ms, debounce_ms)
    }

    /// Clear the pending + rescan state after a drive consumed the coalesced burst. Drains
    /// every buffered token (a single drive answers for the whole burst → coalescing).
    pub fn mark_driven(&mut self) {
        while self.rx.try_recv().is_ok() {}
        self.pending = false;
        self.rescan = false;
    }

    /// True iff a burst overflowed the bounded channel since the last drive — the OOM
    /// guard tripped (events were dropped, a rescan is implied).
    pub fn rescan_requested(&self) -> bool {
        self.rescan
    }

    /// True iff there is un-driven work pending.
    pub fn pending(&self) -> bool {
        self.pending
    }

    /// The current buffered token count (bounded by the channel depth). Proves the storm
    /// guard: under any burst this stays ≤ the channel bound — overflow trips `rescan`,
    /// never queue growth.
    pub fn queued(&self) -> usize {
        self.rx.try_iter().count()
    }
}

/// Process monotonic clock in whole milliseconds, from a fixed process-lifetime base.
/// Used only for the live debounce; the pure decision is [`debounce_due`] so tests never
/// touch the wall clock.
fn monotonic_ms() -> u128 {
    use std::sync::OnceLock;
    static BASE: OnceLock<Instant> = OnceLock::new();
    BASE.get_or_init(Instant::now).elapsed().as_millis()
}

/// The live filesystem watcher + self-driver (§6). Holds the SINGLE [`Arc<EventLog>`]
/// (never a second store), plus the collaborators a drive needs: a [`ParserRouter`] for
/// re-ingest, an [`Embedder`], and a [`Reasoner`] for the evolve/mandate phase. All three
/// collaborator traits are `Send + Sync`, and [`EventLog`] is `Send + Sync`, so the whole
/// watcher is `Send` and can be moved onto the watcher thread.
pub struct MandateWatcher {
    /// The ONE shared store handle (§6.3). A clone of the caller's single [`EventLog`];
    /// the watcher calls its methods and never opens a second connection.
    log: Arc<EventLog>,
    /// Re-ingest router (native + rich parsers). Owned so a drive can `ingest_all`.
    router: ParserRouter,
    /// The embedder the re-ingest + recall path needs.
    embedder: Box<dyn Embedder>,
    /// The reasoner the evolve/mandate-synthesis phase needs.
    reasoner: Box<dyn Reasoner>,
}

impl MandateWatcher {
    /// Build a watcher around the caller's SINGLE [`Arc<EventLog>`]. Pass a *clone* of the
    /// one engine instance (`Arc::clone(&log)`) — NEVER a freshly `open`ed second store.
    /// The collaborators are owned by the watcher so a drive (possibly on the watcher
    /// thread) has everything it needs without borrowing across the thread boundary.
    pub fn new(
        log: Arc<EventLog>,
        router: ParserRouter,
        embedder: Box<dyn Embedder>,
        reasoner: Box<dyn Reasoner>,
    ) -> Self {
        Self { log, router, embedder, reasoner }
    }

    /// One debounced step (§6): re-ingest changed sources, THEN run one evolve tick whose
    /// M6c mandate phase reacts. Both calls go to the SHARED [`EventLog`] — the engine's
    /// own mutex serializes this against any foreground writer (single-writer holds).
    ///
    /// This is deliberately a clean, directly-callable method (no OS fs event required),
    /// so the hermetic test drives the whole watcher → proposer pipeline by calling it.
    /// The mandate phase inside `evolve_once` still proposes through M6a's gate — the
    /// watcher writes nothing itself.
    pub fn drive_once(&self) -> Result<(), BossclawError> {
        // 1. Re-ingest every active read grant (a changed source becomes a fresh
        //    file_ingested / supersede on the shared log).
        self.log.ingest_all(&self.router, self.embedder.as_ref())?;
        // 2. One evolve tick: curation + the M6c mandate phase (proposes through the gate).
        self.log.evolve_once(self.embedder.as_ref(), self.reasoner.as_ref())?;
        Ok(())
    }

    /// The active read-grant source roots (the dirs whose changes should drive the
    /// proposer). Sourced from the engine's read-grant projection so the watch set is
    /// exactly the read-granted ingest set — the watcher can never observe (or react to)
    /// anything outside an active read grant.
    pub fn source_roots(&self) -> Result<Vec<PathBuf>, BossclawError> {
        Ok(self
            .log
            .grants()?
            .into_iter()
            .filter(|g| !g.revoked)
            .map(|g| PathBuf::from(g.canonical_root))
            .collect())
    }

    /// Self-write belt (defense-in-depth, §6). Returns the set of canonical paths the
    /// engine itself wrote (every committed `file_written` target). The watch loop ignores
    /// events whose path is in this set, so the engine's own write never drives a fresh
    /// tick.
    ///
    /// This is belt-and-suspenders: the STRUCTURAL guarantee (a mandate target must live
    /// OUTSIDE every read root — Finding A / Task 2) already prevents the self-loop, since
    /// a write target is never inside a watched source root. This filter is a second,
    /// independent line in case a future change weakens that, and it documents the intent.
    /// It reads the `target` recorded on each `file_written` — the SOLE-constructor write
    /// event (§5) — so the set is exactly the paths the engine authored.
    ///
    /// # Cost + why it stays a full scan (deliberate, M2)
    ///
    /// This rebuilds the set with a full `stream_all` history scan — O(total events) per
    /// call, and the live loop calls it once per drive — and the set grows monotonically
    /// with the engine's lifetime write count. That cost is accepted ON PURPOSE rather than
    /// optimized, because the belt is VESTIGIAL behind Finding A: a `file_written` `target`
    /// lives outside every read root, so it can never appear in the watched read-root event
    /// stream, so [`event_is_self_write`] essentially never matches. Threading a high-water
    /// seq through the watcher to make the refresh incremental (scan only `file_written`
    /// since the last drive's seq, or cap the set) would add real state + complexity to a
    /// path that, by construction, has no live effect. NOTED FUTURE OPTIMIZATION: if the
    /// belt ever becomes load-bearing (Finding A weakened) OR the per-drive scan shows up in
    /// a profile, switch to a bounded/incremental refresh then — not before (YAGNI).
    pub fn self_written_targets(&self) -> Result<HashSet<String>, BossclawError> {
        let mut out = HashSet::new();
        for ev in self.log.stream_all()? {
            if ev.event_type == crate::graph::FILE_WRITTEN_EVENT_TYPE {
                // The `file_written` content records the canonical path under `"target"`
                // (the JSON content field; the DB column is named `canonical_target`).
                if let Some(t) = ev.content.get("target").and_then(|v| v.as_str()) {
                    out.insert(t.to_string());
                }
            }
        }
        Ok(out)
    }

    /// Spin a live `notify` recommended watcher over the active read-grant source `roots`
    /// and run the debounce/coalesce loop until the watcher is dropped. The self-write belt
    /// filters the raw event stream (events touching ONLY engine-written paths are dropped
    /// before they become a wake-up token); a debounced tick then calls
    /// [`drive_once`](Self::drive_once).
    ///
    /// BLOCKING: this runs the loop on the calling thread; the production caller spawns it
    /// on a dedicated thread (the watcher owns its `Send` collaborators for exactly that).
    /// The hermetic tests do NOT call this — they call [`drive_once`](Self::drive_once) and
    /// drive a [`Coalescer`] directly, so no real OS watcher is needed in CI.
    ///
    /// Single-writer note: the `notify` callback only ever calls [`Coalescer::offer`] (no
    /// store access); ALL store access stays on this loop thread via `drive_once`, so the
    /// engine's mutex is the only synchronization needed.
    pub fn watch(&self, roots: &[PathBuf]) -> Result<(), BossclawError> {
        let mut coalescer = Coalescer::new(WATCH_CHANNEL_BOUND);
        // Self-write belt state (defense-in-depth, §6): the set of canonical paths the
        // engine itself wrote. Shared with the `notify` callback (read) and refreshed by
        // the loop after each drive (write). The callback DROPS any event that touches only
        // these paths, so the engine's own `file_written` can never become a wake token.
        // (The structural guard — write target outside every read root, Finding A — already
        // means such a path can't appear in the watched stream; this is belt-and-suspenders.)
        let self_written = Arc::new(Mutex::new(self.self_written_targets()?));
        let belt = Arc::clone(&self_written);

        // The notify callback is the PRODUCER: it owns its own tiny offer-channel so the
        // FFI thread never touches the store or the coalescer's consumer state. We forward
        // each KEPT raw event into a std channel the loop drains into the coalescer.
        let (raw_tx, raw_rx) = sync_channel::<()>(WATCH_CHANNEL_BOUND);
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            // Only Ok events carry paths; an Err means "the watch is noisy" → wake anyway.
            let keep = match &res {
                Ok(ev) => {
                    let set = belt.lock().expect("self-write belt mutex");
                    !event_is_self_write(&ev.paths, &set)
                }
                Err(_) => true,
            };
            // A kept event means "something changed" — we only need the wake-up. Non-blocking
            // send; a full channel is fine (the loop's coalescer trips rescan), so the send
            // result is ignored.
            if keep {
                let _ = raw_tx.try_send(());
            }
        })
        .map_err(|e| BossclawError::Io(std::io::Error::other(format!("notify watcher init: {e}"))))?;

        for root in roots {
            watcher
                .watch(root.as_path(), RecursiveMode::Recursive)
                .map_err(|e| BossclawError::Io(std::io::Error::other(format!("notify watch {root:?}: {e}"))))?;
        }

        // Debounce loop: drain raw events into the coalescer, fire a coalesced drive once
        // the burst goes quiet. A short poll keeps the loop responsive without busy-spin.
        loop {
            // Drain whatever the callback has queued since the last poll (non-blocking),
            // stamping each into the coalescer's debounce clock.
            while raw_rx.try_recv().is_ok() {
                coalescer.offer();
            }
            if coalescer.drive_due(monotonic_ms(), EVOLVE_DEBOUNCE_MS) {
                // LOG-AND-CONTINUE: a single bad tick (a transient store/reasoner error)
                // must NEVER kill the live self-driver — propagating here would silently
                // stop the loop forever. The error made this tick a no-op (the engine's own
                // mutex guard releases on the early return inside `drive_once`, so the
                // foreground store is NOT poisoned); the next debounced tick retries.
                if let Err(e) = self.drive_once() {
                    log::warn!("watch: drive_once tick failed, continuing: {e}");
                }
                coalescer.mark_driven();
                // Refresh the belt (the drive may have committed new `file_written`s, so the
                // callback must ignore those too on the next burst). A refresh failure keeps
                // the PRIOR set — also non-fatal to the loop.
                match self.self_written_targets() {
                    Ok(set) => *self_written.lock().expect("self-write belt mutex") = set,
                    Err(e) => log::warn!("watch: belt refresh failed, keeping prior set: {e}"),
                }
            }
            // Block briefly for the next event so the loop is not a busy-wait; the recv
            // timeout is the debounce granularity. A disconnect (watcher dropped) ends it.
            match raw_rx.recv_timeout(std::time::Duration::from_millis(EVOLVE_DEBOUNCE_MS as u64)) {
                Ok(()) => coalescer.offer(),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        Ok(())
    }
}

/// The self-write belt predicate (§6, defense-in-depth): true iff EVERY path the event
/// touches is a known engine-written target (so the event is the engine observing its own
/// write and should be ignored). An event touching ANY non-self-written path is kept (it
/// is a real source change). An event with NO paths is never a self-write (kept). PURE —
/// no I/O — so it is directly unit-testable. Paths are compared as their canonical-string
/// form; `notify` reports absolute paths, and the belt set holds the engine's recorded
/// canonical `target`s.
fn event_is_self_write(paths: &[PathBuf], self_written: &HashSet<String>) -> bool {
    !paths.is_empty()
        && paths
            .iter()
            .all(|p| self_written.contains(&canonical_string(p)))
}

/// Best-effort canonical-string form of `p` for belt comparison: the real path if it
/// resolves, else the path as-given (a just-deleted or not-yet-created path may not
/// canonicalize, in which case the lexical form is the best available key).
fn canonical_string(p: &Path) -> String {
    std::fs::canonicalize(p)
        .map(|c| c.to_string_lossy().to_string())
        .unwrap_or_else(|_| p.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::{event_is_self_write, Coalescer};
    use std::collections::HashSet;
    use std::path::PathBuf;

    /// The self-write belt (§6): an event touching ONLY engine-written paths is a self-
    /// write (ignored); an event touching ANY other path, or no path at all, is kept.
    #[test]
    fn self_write_belt_ignores_only_pure_engine_writes() {
        // Use a path that exists (the crate dir) so `canonical_string` resolves to a stable
        // key the belt set can hold; a non-existent path falls back to its lexical form.
        let mine = std::env::current_dir().unwrap();
        let mine_key = std::fs::canonicalize(&mine).unwrap().to_string_lossy().to_string();
        let mut belt = HashSet::new();
        belt.insert(mine_key);

        // ONLY the engine's own written path → self-write → ignore.
        assert!(event_is_self_write(std::slice::from_ref(&mine), &belt), "pure engine-write event is ignored");
        // A real source change (a path NOT in the belt) → keep (drives a tick).
        let other = PathBuf::from("/definitely/not/a/written/target.md");
        assert!(!event_is_self_write(std::slice::from_ref(&other), &belt), "a real source change is kept");
        // Mixed (engine path + a real source change) → keep (the source change matters).
        assert!(!event_is_self_write(&[mine, other], &belt), "any non-self path keeps the event");
        // No paths (a bare rescan/notice) → never a self-write → keep.
        assert!(!event_is_self_write(&[], &belt), "a pathless event is never a self-write");
    }

    /// A burst far larger than the channel bound stays bounded: the queue never grows past
    /// the bound, the overflow trips the rescan flag, and the whole burst coalesces to ONE
    /// due drive once the window elapses (the in-crate twin of the integration proof 8).
    #[test]
    fn storm_is_bounded_and_coalesces_to_one_drive() {
        const BOUND: usize = 8;
        const DEBOUNCE_MS: u128 = 50;
        let mut c = Coalescer::new(BOUND);
        // 5000 events at the same logical instant.
        for _ in 0..5000 {
            c.offer_at(1_000);
        }
        assert!(c.pending(), "burst left work pending");
        assert!(c.rescan_requested(), "overflow tripped rescan (no unbounded growth)");
        assert!(c.queued() <= BOUND, "queue stayed within the bound under the storm");
        // Not due inside the window; due exactly once after it; not due again while quiet.
        assert!(!c.drive_due(1_000, DEBOUNCE_MS), "no drive inside the debounce window");
        assert!(c.drive_due(1_000 + DEBOUNCE_MS, DEBOUNCE_MS), "one drive due after the window");
        c.mark_driven();
        assert!(!c.drive_due(1_000 + DEBOUNCE_MS * 4, DEBOUNCE_MS), "no second drive while quiet");
        assert!(!c.pending(), "the drive cleared the pending + rescan state");
    }
}
