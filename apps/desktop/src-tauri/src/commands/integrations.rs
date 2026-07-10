//! Tauri commands for the SP2 one-click Claude Code integration. Thin: resolve the real paths
//! (adapter binary via current_exe, socket via bossclawd-paths, config under $HOME) and delegate to
//! the tested pure `crate::integrations` core.

use crate::integrations::{claude_code, ClaudeCodePaths, ClaudeCodeStatus};
use std::path::PathBuf;

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

#[tauri::command]
pub fn integrations_status() -> Result<IntegrationsStatusDto, String> {
    let paths = ClaudeCodePaths::under(&home_dir()?);
    let claude_code = claude_code::detect(&paths).map_err(|e| e.to_string())?;
    Ok(IntegrationsStatusDto { claude_code })
}

#[tauri::command]
pub fn integrations_connect_claude_code() -> Result<IntegrationsStatusDto, String> {
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

// Codex: SP2 follow-up — a sibling `*_codex` command trio + a `crate::integrations::codex` adapter
// writing ~/.codex/config.toml, reusing atomic_write_0600 + the status enum. No dead code today.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_paths_resolve_under_home() {
        let p = ClaudeCodePaths::under(std::path::Path::new("/home/me"));
        assert_eq!(p.claude_json, PathBuf::from("/home/me/.claude.json"));
        assert_eq!(p.settings_json, PathBuf::from("/home/me/.claude/settings.json"));
    }
}
