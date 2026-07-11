//! Tauri commands for the SP2/SP3 one-click Claude Code integration. Thin: resolve the real paths
//! (adapter binary via current_exe, socket via bossclawd-paths, config under $HOME) and delegate to
//! the tested pure `crate::integrations` core, plus the SP3 consent wiring to the engine's capture
//! flags (the M4 guarantee lives in the `backfill` value chosen here — see the wiring helpers).

use crate::commands::identity::AppState;
use crate::engine::{Engine, EngineOpError};
use crate::integrations::{claude_code, ClaudeCodePaths, ClaudeCodeStatus};
use std::path::PathBuf;
use tauri::State;

#[derive(serde::Serialize)]
pub struct IntegrationsStatusDto {
    pub claude_code: ClaudeCodeStatus,
}

/// The user's home dir for config resolution. Errors if `$HOME` is unset (headless/degraded env).
fn home_dir() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "HOME is not set".to_string())
}

/// The Claude Code transcripts root: `~/.claude/projects` (what the daemon sweeper scans).
fn projects_root() -> Result<PathBuf, String> {
    Ok(ClaudeCodePaths::under(&home_dir()?).claude_dir.join("projects"))
}

#[tauri::command]
pub fn integrations_status() -> Result<IntegrationsStatusDto, String> {
    let paths = ClaudeCodePaths::under(&home_dir()?);
    let claude_code = claude_code::detect(&paths).map_err(|e| e.to_string())?;
    Ok(IntegrationsStatusDto { claude_code })
}

/// Connect Claude Code: write the config (mcpServers + SessionStart nudge + SessionEnd capture hook),
/// THEN — per the user's consent — enable capture. `capture_consent` sets BOTH capture flags in one
/// call (spec §6a: ongoing capture + the one-time ~30-day backfill), so `backfill == capture_consent`.
///
/// **Order + failure handling.** The config is written FIRST; only if that succeeds do we flip the
/// engine flags. A config-write failure aborts BEFORE any capture is enabled (never capture-on with no
/// MCP wiring). If the config succeeds but enabling the flag fails, the config STAYS (recall / remember
/// / snapshot are already wired) and the error is surfaced to the UI; a re-Connect heals it (both
/// `connect` and `set_capture_enabled` are idempotent). We never silently swallow the enable failure.
#[tauri::command]
pub async fn integrations_connect_claude_code(
    capture_consent: bool,
    state: State<'_, AppState>,
) -> Result<IntegrationsStatusDto, String> {
    let paths = ClaudeCodePaths::under(&home_dir()?);
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let binary = crate::engine::daemon::resolve_memory_bin_path(&exe);
    // Fail early + clearly if the sidecar isn't next to the app (e.g. a dev build that didn't stage
    // it) instead of writing a dangling command that fails cryptically inside Claude Code later.
    if !binary.exists() {
        return Err(format!(
            "air-memory-mcp not found at {} — rebuild the app bundle before connecting",
            binary.display()
        ));
    }
    let socket = bossclawd_paths::resolve_socket_path(&bossclawd_paths::resolve_data_dir());
    claude_code::connect(&paths, &binary, &socket).map_err(|e| e.to_string())?;
    connect_capture_wiring(state.engine.as_ref(), capture_consent).await.map_err(|e| e.to_string())?;
    let claude_code = claude_code::detect(&paths).map_err(|e| e.to_string())?;
    Ok(IntegrationsStatusDto { claude_code })
}

#[tauri::command]
pub fn integrations_disconnect_claude_code() -> Result<IntegrationsStatusDto, String> {
    let paths = ClaudeCodePaths::under(&home_dir()?);
    claude_code::disconnect(&paths).map_err(|e| e.to_string())?;
    let claude_code = claude_code::detect(&paths).map_err(|e| e.to_string())?;
    Ok(IntegrationsStatusDto { claude_code })
}

/// App-side count of Claude Code transcripts under `~/.claude/projects`, for the Connect consent
/// disclosure ("…including your recent sessions (~30 days, N found)"). Pure filesystem read; a missing
/// projects dir is `0`, never an error.
#[tauri::command]
pub fn integrations_backfill_count() -> Result<u64, String> {
    Ok(claude_code::backfill_candidate_count(&projects_root()?))
}

/// The Integrations capture toggle — FORWARD-ONLY (the M4 guarantee): it passes `backfill: false`
/// ALWAYS, so a user who declined history at Connect (or connected then toggled capture on later)
/// never has their ~30-day backlog silently imported. Returns the refreshed detect status.
#[tauri::command]
pub async fn integrations_set_capture_enabled(
    enabled: bool,
    state: State<'_, AppState>,
) -> Result<IntegrationsStatusDto, String> {
    toggle_capture_wiring(state.engine.as_ref(), enabled).await.map_err(|e| e.to_string())?;
    let claude_code = claude_code::detect(&ClaudeCodePaths::under(&home_dir()?)).map_err(|e| e.to_string())?;
    Ok(IntegrationsStatusDto { claude_code })
}

/// The engine's ongoing-capture flag — the Integrations toggle POSITION (the counterpart the frontend
/// reads to render the toggle after `integrations_set_capture_enabled` writes it). Distinct from
/// `ClaudeCodeStatus::Connected { capture }`, which is hook PRESENCE in the config, not whether capture
/// is currently enabled. Fail-CLOSED to `false` on any error (not onboarded / daemon down): a toggle
/// read must never hard-fail, and "off" is the safe default to show.
#[tauri::command]
pub async fn integrations_capture_enabled(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(state.engine.capture_enabled().await.unwrap_or(false))
}

// ── Consent wiring (the M4 guarantee lives in the `backfill` value each path chooses). Split out as
//    small async helpers so the load-bearing property is unit-testable against a fake engine without a
//    live daemon or a full `AppState`. ──

/// Connect wiring (spec §6a): Connect consents to ongoing capture AND the one-time backfill together,
/// so BOTH the enable flag and the backfill flag equal `capture_consent`. A `false` consent leaves
/// capture off and imports nothing; A8's engine records the one-time history consent only when
/// `backfill` is true.
async fn connect_capture_wiring(engine: &Engine, capture_consent: bool) -> Result<(), EngineOpError> {
    engine.set_capture_enabled(capture_consent, capture_consent).await
}

/// Toggle wiring (M4): a later enable is forward-only — `backfill: false` ALWAYS.
async fn toggle_capture_wiring(engine: &Engine, enabled: bool) -> Result<(), EngineOpError> {
    engine.set_capture_enabled(enabled, false).await
}

// Codex: SP2 follow-up — a sibling `*_codex` command trio + a `crate::integrations::codex` adapter
// writing ~/.codex/config.toml, reusing atomic_write_0600 + the status enum. No dead code today.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::transport::Transport;
    use bossclawd_proto::{Request, Response};
    use std::sync::{Arc, Mutex};

    #[test]
    fn claude_paths_resolve_under_home() {
        let p = ClaudeCodePaths::under(std::path::Path::new("/home/me"));
        assert_eq!(p.claude_json, PathBuf::from("/home/me/.claude.json"));
        assert_eq!(p.settings_json, PathBuf::from("/home/me/.claude/settings.json"));
    }

    /// A fake transport that records the LAST request it received and answers success — lets us assert
    /// the exact `backfill` value the command layer forwards (the M4 consent property) WITHOUT a live
    /// daemon or a real `AppState`.
    struct RecordingTransport {
        last: Mutex<Option<Request>>,
    }
    impl RecordingTransport {
        fn new() -> Arc<Self> {
            Arc::new(Self { last: Mutex::new(None) })
        }
        fn last(&self) -> Request {
            self.last.lock().unwrap().clone().expect("a request was sent")
        }
    }
    #[async_trait::async_trait]
    impl Transport for RecordingTransport {
        async fn request(&self, req: Request) -> Result<Response, EngineOpError> {
            let resp = match &req {
                Request::CaptureEnabled { .. } => Response::CaptureEnabled(false),
                _ => Response::Ok,
            };
            *self.last.lock().unwrap() = Some(req);
            Ok(resp)
        }
    }

    fn engine_over(t: Arc<RecordingTransport>) -> Engine {
        Engine::new(t as Arc<dyn Transport>)
    }

    #[tokio::test]
    async fn connect_wiring_passes_backfill_equal_to_consent() {
        let t = RecordingTransport::new();
        let engine = engine_over(t.clone());

        // consent=true → SetCaptureEnabled { enabled:true, backfill:true } (Connect sets BOTH flags).
        connect_capture_wiring(&engine, true).await.expect("wiring ok");
        match t.last() {
            Request::SetCaptureEnabled { enabled, backfill, .. } => {
                assert!(enabled, "consent enables capture");
                assert!(backfill, "Connect: backfill == consent (true)");
            }
            other => panic!("expected SetCaptureEnabled, got {other:?}"),
        }

        // consent=false → { enabled:false, backfill:false } — connects (config) but capture stays OFF.
        connect_capture_wiring(&engine, false).await.expect("wiring ok");
        match t.last() {
            Request::SetCaptureEnabled { enabled, backfill, .. } => {
                assert!(!enabled, "declined consent leaves capture off");
                assert!(!backfill, "declined consent → no backfill");
            }
            other => panic!("expected SetCaptureEnabled, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn toggle_wiring_is_forward_only_backfill_always_false() {
        // The M4 guarantee at the toggle: backfill=false for BOTH enable and disable, so a later enable
        // can never re-import declined history.
        let t = RecordingTransport::new();
        let engine = engine_over(t.clone());

        toggle_capture_wiring(&engine, true).await.expect("toggle on");
        match t.last() {
            Request::SetCaptureEnabled { enabled, backfill, .. } => {
                assert!(enabled, "toggle on enables capture");
                assert!(!backfill, "FORWARD-ONLY: the toggle never consents to backfill");
            }
            other => panic!("expected SetCaptureEnabled, got {other:?}"),
        }

        toggle_capture_wiring(&engine, false).await.expect("toggle off");
        match t.last() {
            Request::SetCaptureEnabled { enabled, backfill, .. } => {
                assert!(!enabled);
                assert!(!backfill, "disable also carries backfill=false");
            }
            other => panic!("expected SetCaptureEnabled, got {other:?}"),
        }
    }
}
