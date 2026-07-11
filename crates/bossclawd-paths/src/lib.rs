//! Single source of truth for how the `bossclawd` daemon and its clients resolve the daemon's
//! **data dir** and **Unix socket path**. The daemon (`bossclawd`) and the `air-memory-mcp` adapter
//! both depend on this crate so their resolution can never drift apart — a drift would let the
//! adapter connect to a path the daemon no longer serves (and fail with only a vague "unavailable").
//!
//! Kept in its own crate — NOT in `bossclawd-proto` — because OS filesystem-path conventions are
//! not wire-protocol concerns; the wire crate stays limited to the frame codec + message types.
//!
//! Resolution (the default data dir mirrors the platform app-data dir for the AIR Agent bundle so
//! the daemon and the desktop app share ONE store):
//! - **data dir:** [`ENV_DATA_DIR`] if set, else the per-OS app-data dir ([`app_data_dir`] — macOS
//!   `~/Library/Application Support/<id>`, Linux `$XDG_DATA_HOME | ~/.local/share/<id>`), else the
//!   current dir (`.`) as a last-resort degraded-environment fallback.
//! - **socket:** [`ENV_SOCKET`] if set, else `<data_dir>/`[`SOCKET_FILE`].
//!
//! The public wrappers read the real process environment; the resolution *logic* lives in pure
//! `*_from` helpers that take their inputs as arguments, so the tests exercise every permutation
//! without mutating process-global env (which is racy under Rust's parallel test execution).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Env override for the data dir (the DB, socket, and lock live here by default).
pub const ENV_DATA_DIR: &str = "BOSSCLAWD_DATA_DIR";
/// Env override for the socket path (default: `<data_dir>/`[`SOCKET_FILE`]).
pub const ENV_SOCKET: &str = "BOSSCLAWD_SOCKET";
/// The default socket file name under the data dir.
pub const SOCKET_FILE: &str = "bossclawd.sock";
/// The AIR Agent bundle identifier — the app's per-OS data-dir folder name. The daemon derives its
/// default data dir under this so it lands on the SAME store the desktop app uses.
pub const APP_DIR_NAME: &str = "ai.air-agent.desktop";

/// The platform app-data dir for the AIR Agent bundle id (matches Tauri's `app_data_dir`), or
/// `None` when the environment can't provide one (`HOME` unset on macOS; both `XDG_DATA_HOME` and
/// `HOME` unset on Linux) — a headless/degraded environment. Reads `HOME` / `XDG_DATA_HOME`.
pub fn app_data_dir() -> Option<PathBuf> {
    app_data_dir_from(std::env::var_os("HOME"), std::env::var_os("XDG_DATA_HOME"))
}

/// The data dir: [`ENV_DATA_DIR`] if set, else [`app_data_dir`], else the current dir (`.`).
///
/// This is the PURE resolution — it does not log. A caller that wants to surface the degraded `.`
/// fallback (e.g. the daemon, into its launchd/systemd log) should check [`app_data_dir`] itself
/// and log before falling back. Reads [`ENV_DATA_DIR`] and (via [`app_data_dir`]) `HOME`/`XDG`.
pub fn resolve_data_dir() -> PathBuf {
    data_dir_from(std::env::var_os(ENV_DATA_DIR), app_data_dir())
}

/// The socket path: [`ENV_SOCKET`] if set, else `<data_dir>/`[`SOCKET_FILE`]. Reads [`ENV_SOCKET`].
pub fn resolve_socket_path(data_dir: &Path) -> PathBuf {
    socket_path_from(std::env::var_os(ENV_SOCKET), data_dir)
}

// ── Pure resolution logic (env values passed in), so tests cover every permutation without
//    touching process-global env. ──────────────────────────────────────────────────────────────

/// macOS: `<home>/Library/Application Support/<APP_DIR_NAME>` (XDG is not used on macOS).
#[cfg(target_os = "macos")]
fn app_data_dir_from(home: Option<OsString>, _xdg: Option<OsString>) -> Option<PathBuf> {
    home.map(|h| PathBuf::from(h).join("Library/Application Support").join(APP_DIR_NAME))
}

/// Other unix (Linux, BSD): XDG base-dir spec — `$XDG_DATA_HOME/<id>`, else `<home>/.local/share/<id>`.
#[cfg(all(unix, not(target_os = "macos")))]
fn app_data_dir_from(home: Option<OsString>, xdg: Option<OsString>) -> Option<PathBuf> {
    if let Some(xdg) = xdg {
        return Some(PathBuf::from(xdg).join(APP_DIR_NAME));
    }
    home.map(|h| PathBuf::from(h).join(".local/share").join(APP_DIR_NAME))
}

/// Non-unix targets: the `bossclawd` daemon is Unix-only, so there is no app-data dir to resolve.
/// This branch exists only so `bossclawd-paths` still compiles in a `cargo build --workspace` on a
/// non-unix target; the daemon and adapter never actually run here.
#[cfg(not(unix))]
fn app_data_dir_from(_home: Option<OsString>, _xdg: Option<OsString>) -> Option<PathBuf> {
    None
}

fn data_dir_from(env_data_dir: Option<OsString>, app_data_dir: Option<PathBuf>) -> PathBuf {
    if let Some(dir) = env_data_dir {
        return PathBuf::from(dir);
    }
    app_data_dir.unwrap_or_else(|| PathBuf::from("."))
}

fn socket_path_from(env_socket: Option<OsString>, data_dir: &Path) -> PathBuf {
    env_socket.map(PathBuf::from).unwrap_or_else(|| data_dir.join(SOCKET_FILE))
}

// ── Owner-only (0600/0700) filesystem primitives ────────────────────────────────────────────────
//
// A SINGLE source of truth for the born-0600 atomic write + born-0700 private dir, shared by the
// desktop SP2 config-writer (`~/.claude.json`, `~/.claude/settings.json`) and the daemon's SP3
// capture store (`<data_dir>/sessions/<sid>.md`). This is a SECURITY-critical primitive (no
// world-readable window) — keeping ONE copy is exactly this crate's charter, so the two callers
// can never drift. Unix-only (POSIX mode bits); the callers that need it are unix-only too.

/// Atomically write `bytes` to `target` at mode `0600` with NO world-readable window: create a temp
/// file in the SAME dir **born 0600** (the `O_CREAT` creation mode — NOT a chmod-after, so the file
/// is never briefly world-readable), fsync, then `rename` over the target (atomic within one
/// filesystem; symlink-safe — `rename` replaces the target, it does not write through a link). The
/// temp name is `.{target-filename}.air-tmp.{pid}.{seq}` in the target's own dir (a unique sibling,
/// removed on any failure so no temp is ever left behind). Std-only, zero dependencies.
#[cfg(unix)]
pub fn atomic_write_0600(target: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);

    let dir = target.parent().unwrap_or_else(|| Path::new("."));
    // The temp name derives from the target's own filename; the fallback is only reachable for a
    // target with no filename (never in practice — both callers pass a real file path).
    let name = target.file_name().and_then(|n| n.to_str()).unwrap_or("air");
    let tmp = dir.join(format!(
        ".{name}.air-tmp.{}.{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));

    let write = || -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true) // fail if the temp name exists → never write into another file
            .mode(0o600) // born 0600, not chmod-after
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()
    };
    if let Err(e) = write().and_then(|()| std::fs::rename(&tmp, target)) {
        let _ = std::fs::remove_file(&tmp); // best-effort: never leave a temp behind
        return Err(e);
    }
    Ok(())
}

/// Create `dir` (and parents) at mode `0700` **only when we create it**; a pre-existing dir keeps
/// its perms untouched (so we never loosen — or clobber — an existing directory). `DirBuilder`'s
/// `mode` applies to the components it makes. Std-only, zero dependencies.
#[cfg(unix)]
pub fn make_private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    if dir.exists() {
        return Ok(());
    }
    std::fs::DirBuilder::new().mode(0o700).recursive(true).create(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(s: &str) -> OsString {
        OsString::from(s)
    }

    #[test]
    fn env_socket_override_wins_regardless_of_data_dir() {
        assert_eq!(
            socket_path_from(Some(os("/run/custom.sock")), Path::new("/ignored")),
            PathBuf::from("/run/custom.sock")
        );
    }

    #[test]
    fn socket_defaults_to_socket_file_under_data_dir() {
        assert_eq!(SOCKET_FILE, "bossclawd.sock");
        assert_eq!(
            socket_path_from(None, Path::new("/data/store")),
            PathBuf::from("/data/store/bossclawd.sock")
        );
    }

    #[test]
    fn env_data_dir_override_wins_over_app_dir() {
        assert_eq!(
            data_dir_from(Some(os("/pinned")), Some(PathBuf::from("/app/dir"))),
            PathBuf::from("/pinned")
        );
    }

    #[test]
    fn data_dir_uses_app_dir_when_no_env() {
        assert_eq!(
            data_dir_from(None, Some(PathBuf::from("/app/dir"))),
            PathBuf::from("/app/dir")
        );
    }

    #[test]
    fn data_dir_falls_back_to_dot_when_no_env_and_no_app_dir() {
        assert_eq!(data_dir_from(None, None), PathBuf::from("."));
    }

    #[test]
    fn app_dir_name_is_the_bundle_id() {
        assert_eq!(APP_DIR_NAME, "ai.air-agent.desktop");
    }

    // Per-OS app-data-dir shape. Each block is cfg-gated to the OS whose branch is compiled; CI's
    // macOS + ubuntu runners each cover their own branch (a single test binary only ever sees one).

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_app_data_dir_is_application_support_and_ignores_xdg() {
        let expected = Some(PathBuf::from(
            "/Users/me/Library/Application Support/ai.air-agent.desktop",
        ));
        assert_eq!(app_data_dir_from(Some(os("/Users/me")), None), expected);
        // XDG is irrelevant on macOS — same result even when set.
        assert_eq!(app_data_dir_from(Some(os("/Users/me")), Some(os("/xdg"))), expected);
        // No HOME → None (the degraded case the daemon logs before falling back to ".").
        assert_eq!(app_data_dir_from(None, None), None);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn linux_app_data_dir_prefers_xdg_then_home_share() {
        // XDG_DATA_HOME wins when set.
        assert_eq!(
            app_data_dir_from(Some(os("/home/me")), Some(os("/xdg/data"))),
            Some(PathBuf::from("/xdg/data/ai.air-agent.desktop"))
        );
        // Else fall back to ~/.local/share.
        assert_eq!(
            app_data_dir_from(Some(os("/home/me")), None),
            Some(PathBuf::from("/home/me/.local/share/ai.air-agent.desktop"))
        );
        // Neither set → None.
        assert_eq!(app_data_dir_from(None, None), None);
    }

    // The 0600/0700 primitive at its canonical home. Std-only (no tempfile dev-dep, keeping the
    // crate dependency-free): a unique dir under the OS temp dir, cleaned up at the end.
    #[cfg(unix)]
    #[test]
    fn atomic_write_is_born_0600_and_private_dir_is_0700() {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);

        let base = std::env::temp_dir().join(format!(
            "bossclawd-paths-test.{}.{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        // make_private_dir creates a fresh dir born 0700.
        make_private_dir(&base).unwrap();
        assert_eq!(base.metadata().unwrap().permissions().mode() & 0o777, 0o700);

        // atomic_write_0600 writes born 0600; an overwrite stays 0600 (never a looser window).
        let target = base.join("secret.md");
        atomic_write_0600(&target, b"one").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"one");
        assert_eq!(target.metadata().unwrap().permissions().mode() & 0o777, 0o600);
        atomic_write_0600(&target, b"two").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"two");
        assert_eq!(target.metadata().unwrap().permissions().mode() & 0o777, 0o600);

        // A pre-existing dir keeps its perms — make_private_dir never loosens or clobbers.
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o755)).unwrap();
        make_private_dir(&base).unwrap();
        assert_eq!(
            base.metadata().unwrap().permissions().mode() & 0o777,
            0o755,
            "an existing dir is left untouched"
        );

        std::fs::remove_dir_all(&base).unwrap();
    }
}
