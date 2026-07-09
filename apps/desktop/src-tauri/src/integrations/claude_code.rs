//! The Claude Code adapter: detect / connect / disconnect the `air-memory` MCP server + SessionStart
//! nudge in the user's real config, merging (never replacing) and reversibly. Pure over injected
//! paths; hermetic temp-dir tests. Codex will be a sibling adapter here (SP2 follow-up).

use super::{read_json_object, ClaudeCodePaths, ClaudeCodeStatus};

/// The mcpServers key that names our entry — the presence check for `detect` and the idempotent
/// key for connect/disconnect (added in Task 6/7).
const MCP_SERVER_KEY: &str = "air-memory";

/// Read-only status: `NotFound` if Claude Code isn't detected, else `Connected` iff `~/.claude.json`
/// parses and has `mcpServers["air-memory"]`. Lenient on malformed (→ `NotConnected`).
#[allow(dead_code)] // SP2: consumed by connect/disconnect (Task 6/7) + the Tauri command (Task 8)
pub fn detect(paths: &ClaudeCodePaths) -> std::io::Result<ClaudeCodeStatus> {
    let present = paths.claude_json.exists() || paths.claude_dir.exists();
    if !present {
        return Ok(ClaudeCodeStatus::NotFound);
    }
    let connected = read_json_object(&paths.claude_json)
        .ok()
        .flatten()
        .and_then(|v| v.get("mcpServers").and_then(|m| m.get(MCP_SERVER_KEY)).map(|_| ()))
        .is_some();
    Ok(if connected {
        ClaudeCodeStatus::Connected
    } else {
        ClaudeCodeStatus::NotConnected
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn paths(home: &Path) -> ClaudeCodePaths {
        ClaudeCodePaths::under(home)
    }

    #[test]
    fn detect_not_found_when_no_claude_config_at_all() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(detect(&paths(dir.path())).unwrap(), ClaudeCodeStatus::NotFound);
    }

    #[test]
    fn detect_not_connected_when_claude_present_without_our_key() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".claude.json"), br#"{"mcpServers":{"chrome":{}}}"#).unwrap();
        assert_eq!(detect(&paths(dir.path())).unwrap(), ClaudeCodeStatus::NotConnected);
    }

    #[test]
    fn detect_not_connected_when_only_claude_dir_exists() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".claude")).unwrap();
        assert_eq!(detect(&paths(dir.path())).unwrap(), ClaudeCodeStatus::NotConnected);
    }

    #[test]
    fn detect_connected_when_our_key_present() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".claude.json"),
            br#"{"mcpServers":{"air-memory":{"type":"stdio"}}}"#,
        )
        .unwrap();
        assert_eq!(detect(&paths(dir.path())).unwrap(), ClaudeCodeStatus::Connected);
    }

    #[test]
    fn detect_lenient_on_malformed_claude_json_reports_not_connected() {
        // A malformed ~/.claude.json can't contain our key → NotConnected (connect() is where the
        // parse error fails loud; status stays a safe read).
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".claude.json"), b"not json {").unwrap();
        assert_eq!(detect(&paths(dir.path())).unwrap(), ClaudeCodeStatus::NotConnected);
    }
}
