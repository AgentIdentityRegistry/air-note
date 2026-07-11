//! SP2 config-writer core: shared types + JSON read/merge helpers + an atomic 0600 file write.
//! Pure over injected paths (I6) so every case is a hermetic temp-dir test — no real `$HOME`.
//! (The `claude_code` adapter module is declared in Task 5, when its file is created.)

use std::path::{Path, PathBuf};

pub mod claude_code;

/// Status of the Claude Code integration on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaudeCodeStatus {
    /// Claude Code not detected (neither `~/.claude.json` nor `~/.claude/` exists).
    NotFound,
    /// Detected, but our `air-memory` MCP server is not registered.
    NotConnected,
    /// Our `air-memory` MCP server is registered.
    Connected,
}

/// The Claude Code config paths, resolved under a home dir (pure — tests pass a temp home).
#[derive(Debug, Clone)]
pub struct ClaudeCodePaths {
    pub claude_dir: PathBuf,  // ~/.claude
    pub claude_json: PathBuf, // ~/.claude.json   (mcpServers)
    pub settings_json: PathBuf, // ~/.claude/settings.json (hooks.SessionStart)
}

impl ClaudeCodePaths {
    pub fn under(home: &Path) -> Self {
        Self {
            claude_dir: home.join(".claude"),
            claude_json: home.join(".claude.json"),
            settings_json: home.join(".claude").join("settings.json"),
        }
    }
}

/// Read a JSON file: `Ok(None)` if absent OR empty/whitespace-only (treated as "fresh"), `Ok(Some)`
/// if it parses, `Err` if it exists with real content that is malformed — so callers fail loud and
/// NEVER clobber an unparseable user file (I5). (Reads follow a symlink, which is safe: the write
/// path replaces the link rather than writing through it — see `atomic_write_0600`.)
pub(crate) fn read_json_object(path: &Path) -> std::io::Result<Option<serde_json::Value>> {
    let bytes = match std::fs::read(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
        Ok(bytes) => bytes,
    };
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(None); // 0-byte or whitespace-only → fresh, not malformed (review m2).
    }
    let v = serde_json::from_slice(&bytes).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("couldn't parse {}: {e}; nothing changed", path.display()),
        )
    })?;
    Ok(Some(v))
}

/// Serialize pretty + trailing newline (matches how editors/Claude Code leave these files). With the
/// `preserve_order` feature on (Task 4 Step 1) existing keys keep their order — only our key is added.
pub(crate) fn to_pretty(v: &serde_json::Value) -> Vec<u8> {
    let mut s = serde_json::to_string_pretty(v).expect("serialize json");
    s.push('\n');
    s.into_bytes()
}

// The born-0600 atomic write (`~/.claude.json`, `~/.claude/settings.json`) + born-0700 private dir
// are a SECURITY-critical primitive (no world-readable window) also used by the daemon's SP3
// capture store. They live in ONE place — `bossclawd-paths`, whose charter is exactly "single
// source of truth so the two can't drift" — and are re-exported here so every existing
// `super::{atomic_write_0600, make_private_dir}` call site (and the SP2 test below) is unchanged.
pub(crate) use bossclawd_paths::{atomic_write_0600, make_private_dir};

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn atomic_write_creates_0600_and_replaces_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("cfg.json");

        atomic_write_0600(&target, b"{\"a\":1}").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"{\"a\":1}");
        let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "written file must be 0600, got {mode:o}");

        // Overwrite replaces the content, still 0600 (born 0600, never a looser window).
        atomic_write_0600(&target, b"{\"a\":2}").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"{\"a\":2}");
        let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn read_json_object_absent_or_empty_is_none_present_is_some_malformed_is_err() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.json");
        assert!(read_json_object(&p).unwrap().is_none(), "absent → None");

        std::fs::write(&p, b"   \n").unwrap();
        assert!(read_json_object(&p).unwrap().is_none(), "empty/whitespace → None (fresh, not malformed)");

        std::fs::write(&p, b"{\"k\":1}").unwrap();
        assert!(read_json_object(&p).unwrap().is_some(), "valid object → Some");

        std::fs::write(&p, b"not json {").unwrap();
        assert!(read_json_object(&p).is_err(), "malformed → Err (fail-loud, never clobber)");
    }
}
