//! App-side `bossclawd` lifecycle glue (M1a Task 6): resolve where the daemon's socket + binary
//! live, probe the socket, and — only if no live owner answers — spawn the daemon and wait a bounded
//! time for it to come up.
//!
//! # The contract (spec: daemon lifecycle)
//! - The daemon is normally launchd/systemd-managed; the app does NOT own its lifecycle. On launch
//!   the app **probes-then-starts**: it connects to the socket and, if a daemon answers the `Hello`
//!   handshake, does nothing (a live owner exists). Only if the probe shows no owner does it spawn
//!   the installed binary — matching the daemon's own single-owner arbitration (it refuses to start
//!   if a live owner already holds the lock/socket), so an app spawn racing launchd is safe.
//! - **Never block or crash app boot on a missing daemon.** Every step here is best-effort and
//!   time-bounded; if the daemon never comes up the app still boots. The client's
//!   [`super::EngineOpError::Unavailable`] mapping is the UI story ("memory service unavailable").
//!
//! Path resolution is pure and unit-tested; the probe/spawn are thin async wrappers so the wiring in
//! `main.rs` stays a couple of calls.

use std::path::{Path, PathBuf};
use std::time::Duration;

use bossclawd_proto::{read_frame, write_frame, Hello, HelloOk, PROTO_VERSION};
use tokio::net::UnixStream;

/// Env override for the socket path (mirrors the daemon's `BOSSCLAWD_SOCKET`). MUST resolve to the
/// same path the daemon binds, so both sides agree without further coordination.
const ENV_SOCKET: &str = "BOSSCLAWD_SOCKET";
/// Env override for the daemon binary path.
const ENV_BIN: &str = "BOSSCLAWD_BIN";
/// The default socket file name under the app data dir (mirrors the daemon's `SOCKET_FILE`).
const SOCKET_FILE: &str = "bossclawd.sock";
/// The daemon binary's file name (co-bundled next to the app executable by the installer, Task 8).
const BIN_NAME: &str = "bossclawd";

/// How long to wait, total, for a freshly-spawned daemon to bind its socket + answer the handshake.
/// Bounded so a daemon that never comes up cannot delay app boot indefinitely.
const STARTUP_WAIT: Duration = Duration::from_secs(5);
/// Poll interval while waiting for the freshly-spawned daemon to answer.
const STARTUP_POLL: Duration = Duration::from_millis(100);
/// Per-probe connect/handshake timeout — a probe must never hang (a half-dead socket).
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Resolve the daemon socket path: `BOSSCLAWD_SOCKET` if set, else `<app_data_dir>/bossclawd.sock`.
/// Pure (given `app_data_dir` + the env) so `main.rs` passes its Tauri `app_data_dir` and the rule
/// is unit-tested. The env branch matches the daemon's `resolve_socket_path` exactly.
pub fn resolve_socket_path(app_data_dir: &Path) -> PathBuf {
    match std::env::var_os(ENV_SOCKET) {
        Some(p) => PathBuf::from(p),
        None => app_data_dir.join(SOCKET_FILE),
    }
}

/// Resolve the daemon binary: `BOSSCLAWD_BIN` if set, else a sibling of the current executable
/// (`<exe_dir>/bossclawd`, where the installer co-locates it — co-signed with the app, Task 0.5/8),
/// with a dev fallback to the cargo target dir (`<exe_dir>/../bossclawd` — e.g. from
/// `target/debug/air_agent_desktop` up to `target/debug/bossclawd`). Returns the first candidate
/// that exists, or the sibling path unconditionally if none exists (so the spawn attempt surfaces a
/// clear "not found" rather than this returning nothing). Pure given `current_exe` + the env.
pub fn resolve_bin_path(current_exe: &Path) -> PathBuf {
    if let Some(p) = std::env::var_os(ENV_BIN) {
        return PathBuf::from(p);
    }
    let exe_dir = current_exe.parent().unwrap_or_else(|| Path::new("."));
    let sibling = exe_dir.join(BIN_NAME);
    if sibling.exists() {
        return sibling;
    }
    // Dev fallback: `cargo run` puts both binaries in the SAME target dir, so the sibling above
    // already covers `cargo run`. This extra hop covers a nested-exe-dir layout (e.g. an .app
    // bundle's `MacOS/` next to a sibling `Resources/`); it is a best-effort convenience only.
    let parent_sibling = exe_dir.parent().map(|p| p.join(BIN_NAME));
    match parent_sibling {
        Some(p) if p.exists() => p,
        // Nothing found — return the primary (sibling) candidate so a later spawn error names it.
        _ => sibling,
    }
}

/// Probe `sock_path` for a LIVE daemon: connect + do the `Hello`/`HelloOk` handshake, bounded by
/// [`PROBE_TIMEOUT`]. `true` iff a daemon answered with a matching protocol version. Any failure
/// (no socket, connect refused, timeout, version skew, garbage) → `false` (no live owner). Never
/// hangs, never panics — a probe must be safe to call on every boot.
pub async fn probe(sock_path: &Path) -> bool {
    let attempt = async {
        let mut stream = UnixStream::connect(sock_path).await.ok()?;
        let hello = Hello { proto_version: PROTO_VERSION };
        let hello_bytes = serde_json::to_vec(&hello).ok()?;
        write_frame(&mut stream, &hello_bytes).await.ok()?;
        let reply = read_frame(&mut stream).await.ok()?;
        let hello_ok: HelloOk = serde_json::from_slice(&reply).ok()?;
        (hello_ok.proto_version == PROTO_VERSION).then_some(())
    };
    matches!(tokio::time::timeout(PROBE_TIMEOUT, attempt).await, Ok(Some(())))
}

/// Ensure a daemon is answering on `sock_path`, best-effort: probe first, and only if no live owner
/// answers spawn `bin_path` and wait (bounded by [`STARTUP_WAIT`]) for it to come up. Returns `true`
/// if a daemon is answering by the time we return (already-up OR successfully started), `false`
/// otherwise. NEVER blocks longer than the bounded wait, NEVER panics, NEVER errors out to the
/// caller — a missing daemon must not stop the app from booting (the client's `Unavailable` mapping
/// covers the degraded UX).
pub async fn ensure_started(sock_path: &Path, bin_path: &Path) -> bool {
    // Fast path: a live owner (launchd/systemd, or a prior app spawn) already answers.
    if probe(sock_path).await {
        return true;
    }
    // No owner — spawn the installed binary. The daemon's own single-owner arbitration makes this
    // safe even if launchd starts one concurrently (the loser of the bind race steps aside).
    //
    // Spawn semantics (documented; at most ONE spawn per app boot):
    // - stdio: the child INHERITS the app's stdio (`Command`'s default — deliberate). In dev that
    //   puts the daemon's stderr log in the app's console, which is the useful behavior; an
    //   installed, service-managed daemon is started by launchd/systemd with the service
    //   definition's own log routing, not through this path.
    // - lifetime/reaping: we never kill or supervise the daemon (its life belongs to the service
    //   manager). But a child we spawned must still be REAPED when it exits — in particular the
    //   loser of the single-owner bind race exits 0 almost immediately and would otherwise linger
    //   as a zombie until app exit. A detached waiter thread `wait()`s on it: if the child exits
    //   it is reaped at once; if the daemon runs for the app's lifetime the thread just stays
    //   parked in `wait()` (one thread, bounded by the one-spawn-per-boot rule).
    match std::process::Command::new(bin_path).spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => {
            eprintln!(
                "air-agent: could not spawn bossclawd at {}: {e} (memory features will be \
                 unavailable until the daemon is running)",
                bin_path.display()
            );
            return false;
        }
    }
    // Bounded wait for the freshly-spawned daemon to bind + answer.
    let deadline = tokio::time::Instant::now() + STARTUP_WAIT;
    while tokio::time::Instant::now() < deadline {
        if probe(sock_path).await {
            return true;
        }
        tokio::time::sleep(STARTUP_POLL).await;
    }
    eprintln!(
        "air-agent: bossclawd did not answer within {STARTUP_WAIT:?} of spawn; memory features \
         will be unavailable until it is up"
    );
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes every env-touching test: env vars are process-global, and the harness runs tests
    /// on parallel threads, so an unserialized set/remove of the SAME var would race. Held (via the
    /// guard below) for the whole test body. Poison-recovered — a panicked test must not wedge the
    /// remaining env tests.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A guard that pins an env var for the test's duration — either SET to a value (the override
    /// fixtures) or REMOVED (the default-case fixtures, so they always run and never silently skip
    /// when a developer's shell exports the var) — and restores the previous state after. Also
    /// holds [`ENV_LOCK`] so env tests serialize.
    struct EnvGuard {
        key: &'static str,
        prev: Option<std::ffi::OsString>,
        _serial: std::sync::MutexGuard<'static, ()>,
    }
    impl EnvGuard {
        fn set(key: &'static str, val: &str) -> Self {
            let _serial = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            let prev = std::env::var_os(key);
            std::env::set_var(key, val);
            Self { key, prev, _serial }
        }
        /// Remove `key` for the test's duration: the default-branch fixture. Unlike a conditional
        /// skip (`if var_os(..).is_none()`), the test body ALWAYS runs.
        fn remove(key: &'static str) -> Self {
            let _serial = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            let prev = std::env::var_os(key);
            std::env::remove_var(key);
            Self { key, prev, _serial }
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }

    #[test]
    fn socket_path_defaults_under_app_data_dir() {
        // No env → `<app_data_dir>/bossclawd.sock`. The guard REMOVES any exported override so
        // this default-case test always runs (no silent skip).
        let _g = EnvGuard::remove(ENV_SOCKET);
        let got = resolve_socket_path(Path::new("/data/ai.air-agent.desktop"));
        assert_eq!(got, PathBuf::from("/data/ai.air-agent.desktop/bossclawd.sock"));
    }

    #[test]
    fn socket_path_honours_env_override() {
        let _g = EnvGuard::set(ENV_SOCKET, "/custom/place.sock");
        let got = resolve_socket_path(Path::new("/data/whatever"));
        assert_eq!(got, PathBuf::from("/custom/place.sock"));
    }

    #[test]
    fn bin_path_honours_env_override() {
        let _g = EnvGuard::set(ENV_BIN, "/opt/bin/bossclawd");
        let got = resolve_bin_path(Path::new("/apps/AIR.app/Contents/MacOS/air_agent_desktop"));
        assert_eq!(got, PathBuf::from("/opt/bin/bossclawd"));
    }

    #[test]
    fn bin_path_falls_back_to_sibling_when_no_env_and_none_exist() {
        // With no env override and a made-up (non-existent) exe dir, the resolver returns the
        // primary SIBLING candidate (so a later spawn error names a concrete path), never empty.
        let _g = EnvGuard::remove(ENV_BIN);
        let exe = Path::new("/nonexistent-dir-xyz/MacOS/air_agent_desktop");
        let got = resolve_bin_path(exe);
        assert_eq!(got, PathBuf::from("/nonexistent-dir-xyz/MacOS/bossclawd"));
    }

    #[test]
    fn bin_path_prefers_an_existing_sibling() {
        // A real sibling binary next to the "exe" is preferred over the parent fallback.
        let _g = EnvGuard::remove(ENV_BIN);
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("air_agent_desktop");
        std::fs::write(&exe, b"exe").unwrap();
        let sibling = dir.path().join(BIN_NAME);
        std::fs::write(&sibling, b"daemon").unwrap();
        let got = resolve_bin_path(&exe);
        assert_eq!(got, sibling);
    }

    #[tokio::test]
    async fn probe_is_false_for_a_missing_socket() {
        // A path with no socket must probe false FAST (bounded), never hang.
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.sock");
        assert!(!probe(&missing).await);
    }

    #[tokio::test]
    async fn ensure_started_returns_false_when_bin_missing_and_no_owner() {
        // No live owner + a non-existent binary → best-effort false, NEVER a hang or panic (the app
        // must still boot). Bounded by the internal startup wait; we cap it here too as a guard.
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("bossclawd.sock");
        let bin = dir.path().join("does-not-exist-bossclawd");
        let got = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            ensure_started(&sock, &bin),
        )
        .await
        .expect("ensure_started must not hang");
        assert!(!got, "no owner + missing binary → false");
    }
}
