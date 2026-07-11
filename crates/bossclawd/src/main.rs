//! `bossclawd` binary entry point (M1a Task 4).
//!
//! Startup order (documented; each step logs to stderr — launchd/systemd capture it; NEVER logs secrets):
//! 1. Resolve paths (env overrides → data-dir defaults that land on the SAME dir the Tauri app uses
//!    for `ai.air-agent.desktop`, so daemon + app share ONE store).
//! 2. Advisory single-owner lock (`lock::acquire_or_refuse`): refused → log + exit 0 (a live owner exists).
//! 3. Bind the Unix socket at `0600`; `AddrInUse` → log + exit 0 (the authoritative single-owner gate —
//!    the loser of a bind race steps aside).
//! 4. Build the shared vault → keystore → embedder → reasoner cell + `ConfigReasonerProvider` →
//!    ONE `EngineHandle` in an `Arc`.
//! 5. Reseed the reasoner config from the signed log (onboarded-gated, like the app's boot reseed) so a
//!    Cloud choice survives restart — readiness is recomputed from the live signed consent + vault fp.
//! 6. Spawn the evolve scheduler.
//! 7. Accept loop: per-connection task holding a clone of the shared engine `Arc`.

// The bin is its OWN compilation unit — the lib's `#![forbid(unsafe_code)]` does not cover it,
// so the attribute is repeated here (Task 9 gate). The umask/uid syscalls go through nix's safe
// bindings; there is no `unsafe` anywhere in the daemon.
#![forbid(unsafe_code)]

// The whole daemon is Unix-only (bossclaw-core = bundled SQLCipher + rustix + POSIX sockets/signals).
// On a non-unix target the bin is an inert stub so `cargo build` on that target still succeeds.
#[cfg(not(unix))]
fn main() {
    eprintln!("bossclawd is Unix-only (macOS/Linux); this target is unsupported.");
    std::process::exit(0);
}

#[cfg(unix)]
fn main() {
    unix_main::run();
}

#[cfg(unix)]
mod unix_main {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use bossclawd::engine::reason::ReasonerConfig;
    use bossclawd::engine::{reseed_reasoner_cell, scheduler, EngineHandle};
    use bossclawd::{lock, server, vault};
    use tokio::net::UnixListener;

    /// Env override for the embedder model dir (default: `<data_dir>/models/potion-base-8M`). NOT a
    /// Tauri `resource_dir` — the daemon locates the bundled model via this env / install path.
    const ENV_MODEL_DIR: &str = "BOSSCLAWD_MODEL_DIR";
    /// The lock file name under the data dir (advisory single-owner lock; the socket bind is authoritative).
    const LOCK_FILE: &str = "bossclawd.lock";

    // The data-dir + socket consts (`BOSSCLAWD_DATA_DIR`, `BOSSCLAWD_SOCKET`, the `bossclawd.sock`
    // socket file, the `ai.air-agent.desktop` bundle id) and their resolution live in the shared
    // `bossclawd-paths` crate, so the daemon and the `air-memory-mcp` adapter resolve them identically
    // (a drift would leave the adapter looking for the daemon at the wrong path).

    pub fn run() {
        // Scrubbing panic hook (egress-security review L-2): a panic PAYLOAD can embed engine
        // state (paths, snippet text, error strings), so log ONLY the panic LOCATION — never the
        // payload, never a backtrace. Installed before anything else so every later panic
        // (including in connection/scheduler tasks) is scrubbed in the launchd/systemd log.
        std::panic::set_hook(Box::new(|info| match info.location() {
            Some(loc) => eprintln!("bossclawd: panic at {loc} (payload suppressed)"),
            None => eprintln!("bossclawd: panic at unknown location (payload suppressed)"),
        }));
        // A real multi-thread runtime (unlike the app's `.setup()` outside a reactor): the scheduler
        // + connection tasks spawn freely, and `spawn_blocking` engine calls get worker threads.
        let rt = tokio::runtime::Runtime::new().expect("build tokio runtime");
        rt.block_on(async_main());
    }

    async fn async_main() {
        let data_dir = resolve_data_dir();
        if let Err(e) = std::fs::create_dir_all(&data_dir) {
            eprintln!("bossclawd: cannot create data dir {}: {e}", data_dir.display());
            std::process::exit(1);
        }
        let sock_path = bossclawd_paths::resolve_socket_path(&data_dir);
        let lock_path = data_dir.join(LOCK_FILE);
        // Rung-2 resolution inputs: the env override (dev/harness, highest priority), the bundled
        // English default dir, and the models root under which a downloaded language pack is staged.
        // `ENV_MODEL_DIR` is read here AND again inside `resolve_bundled_model_dir` — intentional,
        // not a duplication bug: this read yields the override PATH ITSELF (`env_override`, wired
        // straight into `with_resolution` as the highest-priority resolution input), while the read
        // inside `resolve_bundled_model_dir` yields the BUNDLED-DEFAULT fallback (used only when no
        // signed language pack is active) — the two roles happen to share one env var so a dev/harness
        // override fully replaces the model dir exactly like the pre-rung-2 single-model behaviour.
        let env_override = std::env::var_os(ENV_MODEL_DIR).map(PathBuf::from);
        let bundled_dir = resolve_bundled_model_dir(&data_dir);
        let data_models_root = data_dir.join("models");

        // (2) Advisory single-owner lock. A live owner answering the socket, or a live PID in the
        // lock file → step aside (exit 0). This is the fast-path check; the bind below is the real gate.
        let _guard = match lock::acquire_or_refuse(&lock_path, &sock_path) {
            Ok(g) => g,
            Err(lock::LockError::LiveOwner(by)) => {
                eprintln!("bossclawd: a live owner already holds the lock (refused by {by:?}); exiting.");
                std::process::exit(0);
            }
            Err(lock::LockError::Io(e)) => {
                eprintln!("bossclawd: lock I/O error: {e}");
                std::process::exit(1);
            }
        };

        // (3) Bind the socket at 0600. `acquire_or_refuse` already unlinked any stale socket, so a
        // bind failure with AddrInUse means a live owner won a race — step aside (exit 0), the
        // authoritative single-owner arbiter. Any other bind error is fatal.
        let listener = match bind_socket_0600(&sock_path) {
            Ok(l) => l,
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                eprintln!("bossclawd: socket already bound by a live owner; exiting.");
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("bossclawd: cannot bind socket {}: {e}", sock_path.display());
                std::process::exit(1);
            }
        };

        // (4) Build ONE EngineHandle: shared vault (same service as the app) → keystore + embedder +
        // config-driven reasoner. The reasoner cell is shared between the provider closure (read
        // each tick) and the engine itself (`with_reasoner_cell`): the engine REFRESHES it after
        // every successful `SetReasonerConfig`/`EnableCloudReasoner` persist, so a mode flip —
        // including a Cloud→Local revocation — takes effect on the next tick without a daemon
        // restart (the M1a Task 6 review fix; boot additionally reseeds it just below).
        let engine_vault = vault::engine_vault();
        let reasoner_cfg = Arc::new(Mutex::new(ReasonerConfig::default()));
        let embedder = Arc::new(bossclawd::engine::embed::ResourceModel2Vec::with_resolution(
            env_override,
            bundled_dir,
            data_models_root,
            bossclawd::engine::embed::MODEL_ID.to_string(),
        ));
        let reasoner_provider = {
            let cell = reasoner_cfg.clone();
            Arc::new(bossclawd::engine::reason::ConfigReasonerProvider::new(move || {
                cell.lock().unwrap_or_else(|p| p.into_inner()).clone()
            }))
        };
        let engine = Arc::new(
            EngineHandle::new(engine_vault, data_dir.clone(), embedder, reasoner_provider)
                .with_reasoner_cell(reasoner_cfg.clone()),
        );

        // (5) Reseed the reasoner config from the signed log (onboarded-gated), so a Cloud choice
        // survives restart. Onboarding is the daemon-local "<data_dir>/identity.json parses" check —
        // the same authority the app uses. Readiness is recomputed later from live consent + vault fp.
        let onboarded = bossclawd::identity::is_onboarded(&data_dir);
        reseed_reasoner_cell(&engine, &reasoner_cfg, onboarded).await;

        // (5b) Resume a consented-but-interrupted language migration (rung 2; I6). No-op unless a
        // signed InProgress record exists. Runs in the background; the UI polls model_state.
        engine.resume_migration_if_pending(onboarded).await;

        // (6) Spawn the two background loops as siblings, each OFF by default with every gate
        // re-read per wake: the evolve scheduler, and the capture sweeper (SP3 A9 — the
        // durability guarantee + backfill engine; sweeps immediately on boot to heal crash
        // windows and import quiet transcripts, then every SWEEP_INTERVAL).
        scheduler::spawn(engine.clone(), data_dir.clone());
        bossclawd::capture::sweeper::spawn(engine.clone(), data_dir.clone());

        eprintln!(
            "bossclawd: serving on {} (pid {})",
            sock_path.display(),
            std::process::id()
        );

        // (7) Accept loop — the SHARED `server::run_accept_loop` (also used by the test spawn
        // helper, so the roundtrip tests cover this exact path). One task per connection, each
        // holding a clone of the shared engine Arc; every peer is same-uid-checked (SO_PEERCRED /
        // LOCAL_PEERCRED) before any frame is read — defense-in-depth over the 0600 socket.
        // Never returns in normal operation.
        server::run_accept_loop(engine, listener).await;
    }

    /// Resolve the data dir: `BOSSCLAWD_DATA_DIR` if set, else the app's per-OS data dir for
    /// `ai.air-agent.desktop` (macOS `~/Library/Application Support/<id>`, Linux
    /// `$XDG_DATA_HOME|~/.local/share/<id>`). Falls back to the current dir if HOME is unset
    /// (a headless/degraded environment) so the daemon still starts rather than panicking.
    ///
    /// The env-name const + the per-OS resolution live in `bossclawd-paths` (shared verbatim with
    /// the `air-memory-mcp` adapter). This wrapper adds the daemon-specific launchd/systemd log on
    /// the degraded `.` fallback — the pure `bossclawd_paths::resolve_data_dir` does not log.
    fn resolve_data_dir() -> PathBuf {
        if let Some(dir) = std::env::var_os(bossclawd_paths::ENV_DATA_DIR) {
            return PathBuf::from(dir);
        }
        bossclawd_paths::app_data_dir().unwrap_or_else(|| {
            // Visible in launchd/systemd logs so a degraded environment is diagnosable, not silent.
            eprintln!(
                "bossclawd: HOME is unset; falling back to the current directory for data \
                 (degraded environment — set {} to pin the store location)",
                bossclawd_paths::ENV_DATA_DIR
            );
            PathBuf::from(".")
        })
    }

    /// Bundled English model dir: `BOSSCLAWD_MODEL_DIR` (dev/harness override) if set, else the
    /// staged default `<data_dir>/models/potion-base-8M` (the install path the daemon's installer
    /// stages the bundled model into — NOT a Tauri `resource_dir`).
    fn resolve_bundled_model_dir(data_dir: &std::path::Path) -> PathBuf {
        std::env::var_os(ENV_MODEL_DIR)
            .map(PathBuf::from)
            .unwrap_or_else(|| data_dir.join("models/potion-base-8M"))
    }

    /// Bind a Unix listener at mode `0600` (owner-only). The umask is pre-set so no window exists
    /// where the socket is world-accessible: `bind` creates the node under the (tightened) umask,
    /// then `set_permissions` pins `0600` explicitly (belt-and-suspenders, matching the lock file).
    fn bind_socket_0600(sock_path: &std::path::Path) -> std::io::Result<UnixListener> {
        use std::os::unix::fs::PermissionsExt;
        // Tighten the umask so the socket inode is created owner-only from the start; restore after.
        let prev = set_umask(0o077);
        let listener = UnixListener::bind(sock_path);
        set_umask(prev);
        let listener = listener?;
        // Explicitly pin 0600 (defense-in-depth; the umask already restricted it).
        std::fs::set_permissions(sock_path, std::fs::Permissions::from_mode(0o600))?;
        Ok(listener)
    }

    /// Set the process umask via `nix` (a SAFE binding — this keeps `#![forbid(unsafe_code)]`; there
    /// is NO `unsafe` block here). Returns the previous umask so the caller can restore it.
    fn set_umask(mask: u32) -> u32 {
        use nix::sys::stat::{umask, Mode};
        // `Mode::from_bits_truncate` keeps only valid permission bits; the previous mode is returned.
        let prev = umask(Mode::from_bits_truncate(mask as nix::libc::mode_t));
        prev.bits() as u32
    }
}
