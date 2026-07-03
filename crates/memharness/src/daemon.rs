//! The isolated in-process daemon: a real `bossclawd` accept loop on a private 0600 socket under
//! a per-run temp home, on its own current-thread runtime + OS thread (the desktop `TestDaemon`
//! pattern — killing the runtime tears down the accept loop AND every connection task). NEVER
//! touches the OS keychain (provider-key cache seeded empty).
//!
//! Embedder (spec §1 Rev 2): the LIVE run injects the PRODUCTION `ResourceModel2Vec`
//! (potion-base-8M) via `spawn_real`; `spawn_mock_for_plumbing_tests` exists ONLY for hermetic
//! plumbing tests — quality numbers come from the live run with the real embedder.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use bossclawd::engine::embed::{EmbedderProvider, ResourceModel2Vec};
use tokio::sync::Notify;

/// A running isolated daemon. `dir` (temp home + socket) lives as long as this struct;
/// `rt` is `None` after `kill()`.
pub struct HarnessDaemon {
    dir: tempfile::TempDir,
    sock: PathBuf,
    rt: Option<DaemonRuntime>,
}

struct DaemonRuntime {
    shutdown: Arc<Notify>,
    thread: std::thread::JoinHandle<()>,
}

/// The env override the production daemon also honors.
const ENV_MODEL_DIR: &str = "BOSSCLAWD_MODEL_DIR";

/// The repo checkout's model dir (populated by scripts/fetch-model.sh).
pub fn repo_model_dir_fallback() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop/src-tauri/resources/models/potion-base-8M")
}

/// Model-dir resolution with an explicit override (testable core): the override or the repo
/// fallback MUST contain `model.safetensors`, else a fail-fast error naming the fix — this is
/// the real-embedder preflight, mirroring the Ollama preflight (spec §1 Rev 2).
pub fn resolve_model_dir_from(env_override: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    let dir = env_override.unwrap_or_else(repo_model_dir_fallback);
    if dir.join("model.safetensors").is_file() {
        Ok(dir)
    } else {
        anyhow::bail!(
            "embedder model dir {dir:?} is missing model.safetensors — run scripts/fetch-model.sh \
             (or set {ENV_MODEL_DIR} to a populated model dir)"
        )
    }
}

/// Live resolution: `BOSSCLAWD_MODEL_DIR` env → repo fallback, preflighted.
pub fn resolve_real_model_dir() -> anyhow::Result<PathBuf> {
    resolve_model_dir_from(std::env::var_os(ENV_MODEL_DIR).map(PathBuf::from))
}

impl HarnessDaemon {
    /// LIVE-RUN constructor: the PRODUCTION embedder (`ResourceModel2Vec`, potion-base-8M),
    /// model dir resolved + preflighted. This is the ONLY constructor `main.rs` uses.
    pub fn spawn_real() -> anyhow::Result<Self> {
        let model_dir = resolve_real_model_dir()?;
        Self::spawn_with_provider(Arc::new(ResourceModel2Vec::new(model_dir)))
    }

    /// PLUMBING TESTS ONLY: the dim-8 mock embedder (via bossclawd's `test_engine` default).
    /// Quality numbers come from the live run with the real embedder — never from this.
    pub fn spawn_mock_for_plumbing_tests() -> anyhow::Result<Self> {
        Self::spawn_inner(None)
    }

    /// Spawn with an explicit embedder provider (the `spawn_real` path; also lets a future
    /// experiment inject a candidate embedder behind the same seam).
    pub fn spawn_with_provider(provider: Arc<dyn EmbedderProvider>) -> anyhow::Result<Self> {
        Self::spawn_inner(Some(provider))
    }

    fn spawn_inner(provider: Option<Arc<dyn EmbedderProvider>>) -> anyhow::Result<Self> {
        // HERMETIC: seed the process-global provider-key cache EMPTY so provider-key reads
        // short-circuit and never hit the OS keychain (keychain-ACL hang hazard).
        bossclawd::vault::seed_secret_cache_for_test(std::collections::HashMap::new());
        let dir = tempfile::tempdir()?;
        let sock = dir.path().join("bossclawd.sock");
        let rt = Self::start_runtime(&sock, dir.path().to_path_buf(), provider)?;
        Ok(Self { dir, sock, rt: Some(rt) })
    }

    /// Own current-thread runtime + OS thread; blocks until the listener is bound
    /// (`sync_channel` handshake) so a client connect can't race the bind.
    fn start_runtime(
        sock: &Path,
        home: PathBuf,
        provider: Option<Arc<dyn EmbedderProvider>>,
    ) -> anyhow::Result<DaemonRuntime> {
        use std::os::unix::fs::PermissionsExt;
        let shutdown = Arc::new(Notify::new());
        let shutdown_for_thread = shutdown.clone();
        let sock_buf = sock.to_path_buf();
        let (bound_tx, bound_rx) = std::sync::mpsc::sync_channel::<anyhow::Result<()>>(0);
        let thread = std::thread::spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = bound_tx.send(Err(anyhow::anyhow!("build daemon runtime: {e}")));
                    return;
                }
            };
            rt.block_on(async move {
                let listener = match tokio::net::UnixListener::bind(&sock_buf) {
                    Ok(l) => l,
                    Err(e) => {
                        let _ = bound_tx.send(Err(anyhow::anyhow!("bind socket: {e}")));
                        return;
                    }
                };
                // Pin 0600 (owner-only), matching production bind_socket_0600.
                if let Err(e) = std::fs::set_permissions(
                    &sock_buf,
                    std::fs::Permissions::from_mode(0o600),
                ) {
                    let _ = bound_tx.send(Err(anyhow::anyhow!("chmod socket 0600: {e}")));
                    return;
                }
                let engine = Arc::new(match provider {
                    Some(p) => bossclawd::server::test_engine_with_embedder(home, p),
                    None => bossclawd::server::test_engine(home),
                });
                if bound_tx.send(Ok(())).is_err() {
                    return; // caller gone
                }
                tokio::select! {
                    _ = bossclawd::server::run_accept_loop(engine, listener) => {}
                    _ = shutdown_for_thread.notified() => {}
                }
            });
            // Runtime dropped at end of scope → all daemon tasks gone.
        });
        bound_rx.recv().map_err(|_| anyhow::anyhow!("daemon thread died before binding"))??;
        Ok(DaemonRuntime { shutdown, thread })
    }

    /// The private socket path (for a `WireClient`).
    pub fn socket_path(&self) -> &Path {
        &self.sock
    }

    /// The per-run home (corpus is copied under it).
    pub fn home(&self) -> &Path {
        self.dir.path()
    }

    /// Fully kill the daemon: notify shutdown, join the thread (drops the runtime + every
    /// connection task), remove the socket file.
    pub fn kill(&mut self) {
        if let Some(rt) = self.rt.take() {
            rt.shutdown.notify_waiters();
            let _ = rt.thread.join();
        }
        let _ = std::fs::remove_file(&self.sock);
    }
}

impl Drop for HarnessDaemon {
    fn drop(&mut self) {
        if let Some(rt) = self.rt.take() {
            rt.shutdown.notify_waiters();
            let _ = rt.thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawns_a_0600_socket_and_kills_clean() {
        // Mock embedder: PLUMBING TEST ONLY — quality numbers come from the live run with the
        // real embedder (spec §1 Rev 2).
        let mut d = HarnessDaemon::spawn_mock_for_plumbing_tests().expect("spawn daemon");
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(d.socket_path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "socket must be 0600, got {mode:o}");
        d.kill();
        assert!(!d.socket_path().exists(), "socket removed on kill");
    }

    #[test]
    fn model_dir_resolution_prefers_override_then_repo_fallback() {
        // Override wins when it points at a dir holding model.safetensors.
        let fake = tempfile::tempdir().unwrap();
        std::fs::write(fake.path().join("model.safetensors"), b"weights").unwrap();
        let got = resolve_model_dir_from(Some(fake.path().to_path_buf())).unwrap();
        assert_eq!(got, fake.path());

        // An override pointing at a dir WITHOUT the model file fails with the fetch hint.
        let empty = tempfile::tempdir().unwrap();
        let err = resolve_model_dir_from(Some(empty.path().to_path_buf())).unwrap_err();
        assert!(err.to_string().contains("fetch-model.sh"), "actionable: {err}");

        // No override → the repo fallback path is named (existence checked at spawn_real time;
        // on this checkout it exists because fetch-model.sh has been run).
        let fallback = repo_model_dir_fallback();
        assert!(fallback.ends_with("apps/desktop/src-tauri/resources/models/potion-base-8M"));
    }
}
