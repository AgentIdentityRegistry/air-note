//! SP2 config-writer core: shared types + JSON read/merge helpers + an atomic 0600 file write.
//! Pure over injected paths (I6) so every case is a hermetic temp-dir test — no real `$HOME`.
//! (The `claude_code` adapter module is declared in Task 5, when its file is created.)

use std::path::{Path, PathBuf};

/// Status of the Claude Code integration on this machine.
#[allow(dead_code)] // SP2: consumed by the claude_code adapter (Task 5/6)
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
#[allow(dead_code)] // SP2: consumed by the claude_code adapter (Task 5/6)
#[derive(Debug, Clone)]
pub struct ClaudeCodePaths {
    pub claude_dir: PathBuf,    // ~/.claude
    pub claude_json: PathBuf,   // ~/.claude.json   (mcpServers)
    pub settings_json: PathBuf, // ~/.claude/settings.json (hooks.SessionStart)
}

impl ClaudeCodePaths {
    #[allow(dead_code)] // SP2: consumed by the claude_code adapter (Task 5/6)
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
#[allow(dead_code)] // SP2: consumed by #[cfg(test)] tests + the claude_code adapter (Task 5/6)
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
#[allow(dead_code)] // SP2: consumed by the claude_code adapter (Task 5/6)
pub(crate) fn to_pretty(v: &serde_json::Value) -> Vec<u8> {
    let mut s = serde_json::to_string_pretty(v).expect("serialize json");
    s.push('\n');
    s.into_bytes()
}

/// Atomically write `bytes` to `target` at mode 0600 with NO world-readable window: create a temp
/// file in the SAME dir **born 0600** (`mode(0o600)` is the `O_CREAT` creation mode — NOT a
/// chmod-after, so the file is never briefly world-readable), fsync, then `rename` over the target
/// (atomic within one filesystem; symlink-safe — `rename` replaces a symlinked target with our
/// regular file, it does not write through the link) — I2. Std-only, zero new deps.
#[allow(dead_code)] // SP2: consumed by #[cfg(test)] tests + the claude_code adapter (Task 5/6)
pub(crate) fn atomic_write_0600(target: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);

    let dir = target.parent().unwrap_or_else(|| Path::new("."));
    let name = target.file_name().and_then(|n| n.to_str()).unwrap_or("cfg");
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

/// Create `dir` (and parents) at mode 0700 **only if we create it**; a pre-existing dir keeps its
/// perms untouched. For `~/.claude` on a fresh-file connect (review Low — the default umask would
/// otherwise make a fresh `~/.claude` 0755). `DirBuilder`'s `mode` applies to the components it makes.
#[allow(dead_code)] // SP2: consumed by the claude_code adapter (Task 5/6)
pub(crate) fn make_private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    if dir.exists() {
        return Ok(());
    }
    std::fs::DirBuilder::new().mode(0o700).recursive(true).create(dir)
}

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
