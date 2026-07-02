//! Single-owner arbitration for the `bossclawd` daemon.
//!
//! Only one live daemon may run per home directory: they all advance ONE shared
//! memory cursor, so two daemons would silently eat each other's work. This
//! module turns that into a loud, correct refusal. It is a Rust port of
//! `agent-bridge-mcp/src/consumer-lock.mjs` — same semantics: **reclaim stale,
//! refuse live**.
//!
//! # Acquisition protocol ([`acquire_or_refuse`])
//! Given a lock-file path and a socket path, in order:
//! 1. Try to `connect()` the socket. An answering listener == a live owner →
//!    refuse, touching NO files ([`LockError::LiveOwner`] / [`RefusedBy::Socket`]).
//! 2. No answer → read the PID from the lock file. If that PID is alive
//!    (signal-0 probe) → refuse ([`RefusedBy::LivePid`]).
//! 3. Dead or absent PID → reclaim: unlink the stale socket + stale lock, write
//!    our own PID file (mode `0600`), and return a [`LockGuard`].
//!
//! The returned guard removes the lock file it wrote on drop — and only if we
//! still own it (the PID recorded in the file is ours), mirroring
//! `releaseConsumerLock`'s "release iff we own it" intent. The guard does NOT
//! remove the socket: in the daemon the socket is owned by the accept-loop
//! (Task 4), which unbinds it on its own shutdown; the lock only reclaims a
//! *stale* socket left by a dead owner.
//!
//! # The lock file is ADVISORY — the socket bind is the true mutex
//! Check-then-write is not atomic: two daemons started simultaneously can BOTH
//! pass checks (1)–(2) and both write a lock file. The authoritative
//! single-owner gate is the socket bind itself — Task 4 must treat a
//! `UnixListener::bind` failure (`EADDRINUSE`) as the final arbiter (the loser
//! of the race dies at bind), and `acquire_or_refuse` as the fast-path advisory
//! check that keeps the common cases (stale leftovers, an already-running
//! daemon) loud and correct.
//!
//! # Deliberate divergence from the mjs
//! We refuse on ANY live PID in the lock file, including our own (the mjs
//! allows re-entrant self-acquire via its `holder.pid !== pid` check). Safe
//! because a genuine second daemon always has a different PID, and bossclawd
//! never re-acquires within one process.

#[cfg(unix)]
mod imp {
    use serde::{Deserialize, Serialize};
    use std::fs;
    use std::io;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixStream;
    use std::path::{Path, PathBuf};

    use nix::sys::signal::kill;
    use nix::unistd::Pid;

    /// Which check refused acquisition — surfaced so the daemon's exit-0 log
    /// (Task 4) can say *why* it stepped aside.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum RefusedBy {
        /// The socket had an answering listener: a live owner is serving.
        Socket,
        /// The socket was silent, but the lock file's PID is still alive.
        LivePid,
    }

    /// Errors from lock acquisition. Minimal by design, mirroring the workspace
    /// convention (`bossclaw-core`'s `thiserror`-derived enum).
    #[derive(Debug, thiserror::Error)]
    pub enum LockError {
        /// A live owner already holds the lock; refuse without disturbing it.
        /// Carries which check refused, for the daemon's step-aside log.
        #[error("another live owner holds the lock (refused by {0:?})")]
        LiveOwner(RefusedBy),

        /// A filesystem operation failed while reclaiming or writing the lock.
        #[error("io error: {0}")]
        Io(#[from] io::Error),
    }

    /// The contents of a lock file: who holds it, and since when.
    ///
    /// The mjs `Holder` also carries a `name` field (which of several consumer
    /// roles — watch/channel-server/bridge — holds the lock). It is dropped
    /// here INTENTIONALLY: bossclawd is single-role, so a name would be a
    /// constant. Do not "fix" it back in.
    #[derive(Debug, Serialize, Deserialize)]
    struct Holder {
        pid: i32,
        since: String,
    }

    /// An RAII guard proving this process holds the lock. Dropping it removes the
    /// lock file iff this process still owns it. Best-effort on drop (never panics).
    #[must_use = "dropping the guard releases the lock immediately"]
    #[derive(Debug)]
    pub struct LockGuard {
        lock_path: PathBuf,
        pid: i32,
    }

    impl LockGuard {
        /// The PID recorded in the lock file (this process).
        pub fn pid(&self) -> i32 {
            self.pid
        }

        /// The lock file this guard owns.
        pub fn lock_path(&self) -> &Path {
            &self.lock_path
        }
    }

    impl Drop for LockGuard {
        fn drop(&mut self) {
            // Release iff we still own it: re-read the holder and only unlink
            // when the recorded PID is ours. Best-effort — never throws (mirrors
            // `releaseConsumerLock`).
            if let Ok(bytes) = fs::read(&self.lock_path) {
                if let Ok(holder) = serde_json::from_slice::<Holder>(&bytes) {
                    if holder.pid == self.pid {
                        let _ = fs::remove_file(&self.lock_path);
                    }
                }
            }
        }
    }

    /// Is a process alive? Signal 0 probes existence without killing it.
    /// `EPERM` means the process exists but is not ours (still "alive").
    ///
    /// CAVEAT: a recorded PID may have been RECYCLED to an unrelated live
    /// process since the old owner died, so this can yield a spurious
    /// `LivePid` refusal. The socket check is the reliable liveness signal;
    /// this is a best-effort secondary probe (same limitation as the mjs).
    pub fn is_pid_alive(pid: i32) -> bool {
        if pid <= 0 {
            return false;
        }
        match kill(Pid::from_raw(pid), None) {
            Ok(()) => true,
            Err(nix::errno::Errno::EPERM) => true,
            Err(_) => false,
        }
    }

    /// Acquire the single-owner lock, or refuse if a live owner already holds it.
    ///
    /// See the [module docs](self) for the full protocol. On success the lock
    /// file at `lock_path` holds our PID (mode `0600`) and the returned
    /// [`LockGuard`] removes it on drop.
    pub fn acquire_or_refuse(
        lock_path: &Path,
        sock_path: &Path,
    ) -> Result<LockGuard, LockError> {
        // (1) Live listener answering the socket == live owner. Refuse WITHOUT
        // touching any files.
        if UnixStream::connect(sock_path).is_ok() {
            return Err(LockError::LiveOwner(RefusedBy::Socket));
        }

        // (2) No listener. Read the lock file's PID; a live PID == live owner.
        // (A missing/corrupt lock file is simply not-alive → reclaimable.)
        if let Ok(bytes) = fs::read(lock_path) {
            if let Ok(holder) = serde_json::from_slice::<Holder>(&bytes) {
                if is_pid_alive(holder.pid) {
                    return Err(LockError::LiveOwner(RefusedBy::LivePid));
                }
            }
        }

        // (3) Reclaim: unlink stale socket + stale lock (best-effort; absence is
        // fine), then write our own PID file at mode 0600.
        remove_if_present(sock_path)?;
        remove_if_present(lock_path)?;

        let pid = std::process::id() as i32;
        write_lock_file(lock_path, pid)?;

        Ok(LockGuard {
            lock_path: lock_path.to_path_buf(),
            pid,
        })
    }

    /// Remove a path if it exists; a `NotFound` is not an error (nothing to reclaim).
    fn remove_if_present(path: &Path) -> io::Result<()> {
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Write the lock file with our PID at mode `0600`. The mode is enforced with
    /// an explicit `set_permissions` after the write (`fs::write` respects the
    /// umask, which could widen the bits), matching `consumer-lock.mjs`'s
    /// write-then-`chmod(0600)`.
    fn write_lock_file(lock_path: &Path, pid: i32) -> io::Result<()> {
        let holder = Holder {
            pid,
            since: now_epoch_secs(),
        };
        let json = serde_json::to_vec(&holder).map_err(io::Error::from)?;
        fs::write(lock_path, &json)?;
        fs::set_permissions(lock_path, fs::Permissions::from_mode(0o600))?;
        Ok(())
    }

    /// A dependency-light UTC timestamp (raw epoch seconds) for the `since`
    /// field. Purely informational (the mjs stores `new Date().toISOString()`);
    /// the arbitration logic never parses it, so we avoid pulling `chrono` here.
    fn now_epoch_secs() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        format!("{secs}")
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::os::unix::net::UnixListener;
        use std::process::Command;

        /// A PID guaranteed not to be alive: spawn `true`, wait for it to exit,
        /// then reuse its (now-freed) PID promptly. No sleeps, no hardcoded PIDs.
        fn dead_pid() -> i32 {
            let mut child = Command::new("true").spawn().expect("spawn `true`");
            let pid = child.id() as i32;
            child.wait().expect("wait for `true` to exit");
            pid
        }

        fn lock_and_sock(dir: &Path) -> (PathBuf, PathBuf) {
            (dir.join("consumer.lock"), dir.join("bossclawd.sock"))
        }

        // (a) Acquiring in an empty dir succeeds + writes a 0600 PID file.
        #[test]
        fn acquire_in_empty_dir_succeeds_and_writes_0600_pid_file() {
            let tmp = tempfile::tempdir().unwrap();
            let (lock_path, sock_path) = lock_and_sock(tmp.path());

            let guard = acquire_or_refuse(&lock_path, &sock_path).expect("acquire");

            // The lock file exists and records our PID.
            assert_eq!(guard.pid(), std::process::id() as i32);
            let bytes = fs::read(&lock_path).unwrap();
            let holder: Holder = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(holder.pid, std::process::id() as i32);

            // The PID file is mode 0600 (owner rw, nothing else) — TESTED.
            let mode = fs::metadata(&lock_path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "lock file must be 0600, got {mode:o}");
        }

        // (b) A second acquire while the first is held (LIVE PID, no listener) is REFUSED.
        // We hold the lock via our own (alive) PID and leave NO socket listener, so the
        // refusal is exercised through the PID branch, not the socket branch.
        #[test]
        fn second_acquire_with_live_pid_is_refused() {
            let tmp = tempfile::tempdir().unwrap();
            let (lock_path, sock_path) = lock_and_sock(tmp.path());

            // Write a lock file owned by OUR live PID, but with no socket listener.
            write_lock_file(&lock_path, std::process::id() as i32).unwrap();
            assert!(!sock_path.exists());

            let err = acquire_or_refuse(&lock_path, &sock_path).expect_err("must refuse");
            assert!(matches!(err, LockError::LiveOwner(RefusedBy::LivePid)));

            // Refusal must NOT have disturbed the incumbent's lock file.
            let holder: Holder =
                serde_json::from_slice(&fs::read(&lock_path).unwrap()).unwrap();
            assert_eq!(holder.pid, std::process::id() as i32);
        }

        // (b') An answering socket listener == live owner → REFUSED by the socket check,
        // touching NO files. This is the primary "live owner" path in the daemon.
        #[test]
        fn acquire_with_answering_socket_is_refused_untouched() {
            let tmp = tempfile::tempdir().unwrap();
            let (lock_path, sock_path) = lock_and_sock(tmp.path());

            // A real listener answers `connect()`.
            let _listener = UnixListener::bind(&sock_path).expect("bind listener");
            // A lock file with a DEAD pid is present: proves the socket check wins
            // FIRST (before the PID check) and that no file is unlinked.
            write_lock_file(&lock_path, dead_pid()).unwrap();

            let err = acquire_or_refuse(&lock_path, &sock_path).expect_err("must refuse");
            assert!(matches!(err, LockError::LiveOwner(RefusedBy::Socket)));

            // Nothing touched: both the stale lock and the live socket remain.
            assert!(lock_path.exists(), "lock file must be untouched");
            assert!(sock_path.exists(), "socket must be untouched");
        }

        // (c) A lock file with a DEAD PID is RECLAIMED.
        #[test]
        fn dead_pid_lock_is_reclaimed() {
            let tmp = tempfile::tempdir().unwrap();
            let (lock_path, sock_path) = lock_and_sock(tmp.path());

            write_lock_file(&lock_path, dead_pid()).unwrap();

            let guard = acquire_or_refuse(&lock_path, &sock_path).expect("reclaim");
            assert_eq!(guard.pid(), std::process::id() as i32);
            // The lock file now records OUR pid, not the dead one.
            let holder: Holder =
                serde_json::from_slice(&fs::read(&lock_path).unwrap()).unwrap();
            assert_eq!(holder.pid, std::process::id() as i32);
        }

        // (d) A STALE socket file (no listener) is detectable as reclaimable: acquisition
        // succeeds AND the stale socket file is unlinked as part of the reclaim.
        #[test]
        fn stale_socket_no_listener_is_reclaimable() {
            let tmp = tempfile::tempdir().unwrap();
            let (lock_path, sock_path) = lock_and_sock(tmp.path());

            // A leftover socket *file* with nothing listening on it. `connect()`
            // must fail (nobody home), so it is reclaimable, not a live owner.
            fs::write(&sock_path, b"").unwrap();
            assert!(sock_path.exists());
            assert!(
                UnixStream::connect(&sock_path).is_err(),
                "a plain file / dead socket must not answer connect()"
            );

            let guard = acquire_or_refuse(&lock_path, &sock_path).expect("reclaim");
            assert_eq!(guard.pid(), std::process::id() as i32);
            // Reclaim unlinked the stale socket file.
            assert!(!sock_path.exists(), "stale socket must be unlinked on reclaim");
        }

        // Ported mjs semantics: the guard removes the lock on drop, iff we own it.
        #[test]
        fn guard_removes_lock_on_drop() {
            let tmp = tempfile::tempdir().unwrap();
            let (lock_path, sock_path) = lock_and_sock(tmp.path());

            {
                let _guard = acquire_or_refuse(&lock_path, &sock_path).expect("acquire");
                assert!(lock_path.exists());
            } // guard dropped here

            assert!(!lock_path.exists(), "guard must remove its own lock on drop");
        }

        // Ported mjs semantics: the guard does NOT remove a lock it no longer owns
        // (another owner overwrote the PID file). Mirrors "release iff we own it".
        #[test]
        fn guard_does_not_remove_lock_it_no_longer_owns() {
            let tmp = tempfile::tempdir().unwrap();
            let (lock_path, sock_path) = lock_and_sock(tmp.path());

            let guard = acquire_or_refuse(&lock_path, &sock_path).expect("acquire");
            // Simulate a successor taking over: overwrite the lock with a different PID.
            let other_pid = guard.pid() + 1;
            write_lock_file(&lock_path, other_pid).unwrap();

            drop(guard);

            // The successor's lock survives — we only release what we still own.
            assert!(lock_path.exists(), "must not remove a lock owned by another PID");
            let holder: Holder =
                serde_json::from_slice(&fs::read(&lock_path).unwrap()).unwrap();
            assert_eq!(holder.pid, other_pid);
        }

        // `is_pid_alive`: our own PID is alive; a freed child PID is not.
        #[test]
        fn is_pid_alive_own_true_dead_false() {
            assert!(is_pid_alive(std::process::id() as i32));
            assert!(!is_pid_alive(dead_pid()));
            assert!(!is_pid_alive(0));
            assert!(!is_pid_alive(-1));
        }
    }
}

#[cfg(unix)]
pub use imp::{acquire_or_refuse, is_pid_alive, LockError, LockGuard, RefusedBy};
