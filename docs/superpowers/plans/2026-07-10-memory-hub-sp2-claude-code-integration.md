# SP2 — One-Click Claude Code Integration — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a Settings ▸ Integrations "Connect Claude Code" button that safely merges (and cleanly removes) the `air-memory` MCP-server entry in `~/.claude.json` plus a minimal SessionStart nudge in `~/.claude/settings.json`, so every Claude Code session everywhere can `recall`/`remember`.

**Architecture:** The bundled `air-memory-mcp` binary (SP1) ships as a second Tauri `externalBin` sidecar next to the app exe. A pure Rust config-writer (`integrations::claude_code`, hermetic temp-dir tests) does detect/connect/disconnect over injected paths honoring merge-never-replace + atomic-`0600` + idempotent + surgically-reversible + fail-loud invariants. Thin Tauri commands resolve the real paths (binary via `current_exe`, socket via `bossclawd-paths`, config via `$HOME`) and call it. A React panel inside `AirSettings` is a status + button surface over three commands.

**Tech Stack:** Rust (serde_json + tempfile — both already deps; std `PermissionsExt`), Tauri 2 commands, React + TypeScript, vitest. **Unix-only** (matches the daemon/memory-hub reality; commands are `#[cfg(unix)]`).

**Branch:** `feat-memory-hub-sp2-claude-code-integration` (already created, stacked on SP1 `feat-memory-hub-sp1-code-loop` / PR #76). Spec: `docs/superpowers/specs/2026-07-10-memory-hub-sp2-claude-code-integration-design.md`.

**Reviews (Peter's standing ask):** after the plan, run architect + critic + a dedicated security review **before** any code (U2 mutates the user's real `$HOME` files). Per-task two-stage (spec→quality) review during execution; whole-branch review before the PR flips to ready.

---

## File Structure

**Create:**
- `apps/desktop/src-tauri/src/integrations/mod.rs` — pure config-writer core: shared types (`ClaudeCodeStatus`, `ClaudeCodePaths`), JSON read/merge helpers, `atomic_write_0600`.
- `apps/desktop/src-tauri/src/integrations/claude_code.rs` — the Claude Code adapter: `detect` / `connect` / `disconnect` + their unit tests.
- `apps/desktop/src-tauri/src/commands/integrations.rs` — thin Tauri commands + DTOs; path resolution.
- `apps/desktop/src/api/integrations.ts` — frontend invoke wrappers + types.
- `apps/desktop/src/settings/IntegrationsPanel.tsx` — the Settings section UI.
- `apps/desktop/src/settings/IntegrationsPanel.test.tsx` — vitest for the panel.

**Modify:**
- `crates/air-memory-mcp/src/lib.rs` — add `pub const NUDGE_TEXT`.
- `crates/air-memory-mcp/src/main.rs` — handle the `nudge` subcommand.
- `crates/air-memory-mcp/tests/nudge.rs` — new integration test for the subcommand (Create).
- `apps/desktop/src-tauri/src/engine/daemon.rs` — extract a generic `resolve_sibling_bin`; add `resolve_memory_bin_path`.
- `apps/desktop/src-tauri/src/main.rs` — `mod integrations;`, register the 3 commands in `generate_handler!`.
- `apps/desktop/src-tauri/src/commands/mod.rs` — `pub mod integrations;`.
- `apps/desktop/src-tauri/tauri.bundle.conf.json` — `externalBin += "binaries/air-memory-mcp"`.
- `scripts/dev-build-signed.sh` — build + stage + verify the `air-memory-mcp` sidecar.
- `apps/desktop/src/settings/AirSettings.tsx` — render `<IntegrationsPanel />`.
- `crates/air-memory-mcp/README.md` — replace the manual wiring section with the one-click story.

---

## Task 1: The SessionStart nudge — `NUDGE_TEXT` + `nudge` subcommand

**Files:**
- Modify: `crates/air-memory-mcp/src/lib.rs`
- Modify: `crates/air-memory-mcp/src/main.rs`
- Test: `crates/air-memory-mcp/tests/nudge.rs` (Create)

- [ ] **Step 1: Write the failing test**

Cargo exposes the built binary path to integration tests as `env!("CARGO_BIN_EXE_air-memory-mcp")` — no new dep needed.

`crates/air-memory-mcp/tests/nudge.rs`:
```rust
//! The `nudge` subcommand prints the static SessionStart reminder and exits 0 without touching
//! the daemon socket. SP2 wires this as the Claude Code SessionStart hook command.
use std::process::Command;

#[test]
fn nudge_subcommand_prints_nudge_text_and_exits_zero() {
    let out = Command::new(env!("CARGO_BIN_EXE_air-memory-mcp"))
        .arg("nudge")
        .output()
        .expect("run air-memory-mcp nudge");

    assert!(out.status.success(), "nudge must exit 0; got {:?}", out.status);
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        air_memory_mcp::NUDGE_TEXT,
        "stdout must be exactly NUDGE_TEXT (no trailing newline added)"
    );
    // It must NOT emit the server's socket banner (that only prints on the server path).
    let err = String::from_utf8(out.stderr).unwrap();
    assert!(!err.contains("using daemon socket"), "nudge must not start the server: {err}");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p air-memory-mcp --test nudge`
Expected: FAIL — `NUDGE_TEXT` not found in `air_memory_mcp`, and/or `nudge` arg still starts the server (prints the banner).

- [ ] **Step 3: Add `NUDGE_TEXT` to `lib.rs`**

Append to `crates/air-memory-mcp/src/lib.rs`:
```rust
/// The static reminder the SP2 SessionStart hook injects into every Claude Code session so the
/// agent proactively uses the `recall`/`remember` tools. No network/daemon call — a fixed string.
/// Kept here (not `main.rs`) so integration tests can assert the hook output byte-for-byte.
pub const NUDGE_TEXT: &str = "\
AIR long-term memory is available via the `air-memory` MCP tools: \
`recall(query)` to search your past notes and decisions, and \
`remember(text)` to save durable ones. Recall relevant context before starting a task; \
remember decisions worth keeping. Saved notes are stored as external/untrusted \
(recallable, never auto-applied).";
```

- [ ] **Step 4: Handle the subcommand in `main.rs`**

Insert at the very top of `main()` in `crates/air-memory-mcp/src/main.rs`, before `daemon::resolve_socket_path()`:
```rust
    // SP2: the `nudge` subcommand prints the static SessionStart reminder and exits — no socket,
    // no server. Claude Code's SessionStart hook runs `air-memory-mcp nudge`.
    if std::env::args().nth(1).as_deref() == Some("nudge") {
        print!("{}", air_memory_mcp::NUDGE_TEXT);
        return Ok(());
    }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p air-memory-mcp --test nudge` → Expected: PASS.
Run: `cargo clippy -p air-memory-mcp --all-targets -- -D warnings` → Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/air-memory-mcp/src/lib.rs crates/air-memory-mcp/src/main.rs crates/air-memory-mcp/tests/nudge.rs
git commit -m "$(cat <<'EOF'
feat(air-memory-mcp): add `nudge` subcommand for the SP2 SessionStart hook

Prints a static NUDGE_TEXT reminder and exits 0 without touching the socket,
so Claude Code's SessionStart hook can run `air-memory-mcp nudge` cross-platform
with no shell-quoting of the message.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Bundle `air-memory-mcp` as a Tauri `externalBin` sidecar

**Files:**
- Modify: `apps/desktop/src-tauri/tauri.bundle.conf.json`
- Modify: `scripts/dev-build-signed.sh`

This is build/packaging config; its "test" is the build script's own bundled-binary assertion.

- [ ] **Step 1: Add the sidecar to the bundle config**

In `apps/desktop/src-tauri/tauri.bundle.conf.json`, extend `externalBin`:
```json
    "externalBin": ["binaries/bossclawd", "binaries/air-memory-mcp"]
```

- [ ] **Step 2: Stage + verify the sidecar in `dev-build-signed.sh`**

In `scripts/dev-build-signed.sh`, right after the existing bossclawd staging block (the `cp -f "target/debug/bossclawd" "${SIDECAR_PATH}"` line), add the twin:
```bash
# --- 1b. Build + stage the air-memory-mcp sidecar (SP2) --------------------
# Same externalBin rail as bossclawd: Tauri copies it to Contents/MacOS/air-memory-mcp,
# the sibling-of-exe path resolve_memory_bin_path() resolves at runtime.
readonly MEMORY_SIDECAR_PATH="${SIDECAR_DIR}/air-memory-mcp-${TARGET_TRIPLE}"
echo "Building air-memory-mcp (debug) for ${TARGET_TRIPLE}…"
cargo build -p air-memory-mcp
cp -f "target/debug/air-memory-mcp" "${MEMORY_SIDECAR_PATH}"
```

Then, after the existing bundled-daemon assertion block (the `DAEMON_BIN=...; [ -x "${DAEMON_BIN}" ] || { ... }`), add:
```bash
MEMORY_BIN="${APP}/Contents/MacOS/air-memory-mcp"
[ -x "${MEMORY_BIN}" ] || { echo "ERROR: bundled air-memory-mcp missing at ${MEMORY_BIN} (externalBin not bundled?)" >&2; exit 1; }
```

- [ ] **Step 3: Verify config well-formed**

Run: `jq -e '.bundle.externalBin | index("binaries/air-memory-mcp")' apps/desktop/src-tauri/tauri.bundle.conf.json`
Expected: prints an index (non-null), exit 0. (Adjust the jq path if `externalBin` is nested differently — confirm the real key path in the file first.)
Run: `bash -n scripts/dev-build-signed.sh` → Expected: no syntax errors.

- [ ] **Step 4: Commit**

```bash
git add apps/desktop/src-tauri/tauri.bundle.conf.json scripts/dev-build-signed.sh
git commit -m "$(cat <<'EOF'
build(desktop): bundle air-memory-mcp as a second externalBin sidecar (SP2)

Ships the SP1 MCP adapter next to the app exe (Contents/MacOS/air-memory-mcp)
so one-click integration can point Claude Code at a stable absolute path.
dev-build-signed.sh builds + stages + asserts the bundled binary, mirroring
the bossclawd sidecar.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Resolve the sidecar path — generic `resolve_sibling_bin` + `resolve_memory_bin_path`

**Files:**
- Modify: `apps/desktop/src-tauri/src/engine/daemon.rs`
- Test: same file's `#[cfg(test)] mod tests`

DRY: `resolve_bin_path` (bossclawd) and the new memory resolver are identical logic with a different bin name + env var. Extract one helper.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` in `daemon.rs` (mirrors the existing `resolve_bin_path` env/sibling tests — reuse the same `EnvGuard` helper already in that module):
```rust
    #[test]
    fn resolve_memory_bin_env_override_wins() {
        let _g = EnvGuard::set("AIR_MEMORY_MCP_BIN", "/opt/bin/air-memory-mcp");
        assert_eq!(
            resolve_memory_bin_path(Path::new("/apps/AIR.app/Contents/MacOS/air_agent_desktop")),
            PathBuf::from("/opt/bin/air-memory-mcp")
        );
    }

    #[test]
    fn resolve_memory_bin_defaults_to_sibling_of_exe() {
        let _g = EnvGuard::remove("AIR_MEMORY_MCP_BIN");
        // No sibling on disk → falls through to the primary sibling candidate (named, for a later
        // spawn error). Same contract the bossclawd resolver's default test asserts.
        let got = resolve_memory_bin_path(Path::new("/nonexistent/dir/air_agent_desktop"));
        assert_eq!(got, PathBuf::from("/nonexistent/dir/air-memory-mcp"));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p air_agent_desktop engine::daemon::tests::resolve_memory 2>&1 | tail -20`
(Crate package name confirmed `air_agent_desktop` in `apps/desktop/src-tauri/Cargo.toml`.)
Expected: FAIL — `resolve_memory_bin_path` not defined.

- [ ] **Step 3: Refactor to a generic helper + add the memory resolver**

In `daemon.rs`, keep `BIN_NAME`/`ENV_BIN` for bossclawd. Add the generic helper and rewrite `resolve_bin_path` to call it (behavior identical), then add the memory resolver + its constants:
```rust
const ENV_MEMORY_BIN: &str = "AIR_MEMORY_MCP_BIN";
const MEMORY_BIN_NAME: &str = "air-memory-mcp";

/// Resolve a sibling binary bundled next to the app exe: `env_var` override → sibling of the exe →
/// parent-sibling dev fallback → the primary sibling candidate (named, so a later spawn error is
/// legible). Single source for both the daemon and the MCP-adapter binaries (DRY).
fn resolve_sibling_bin(current_exe: &Path, bin_name: &str, env_var: &str) -> PathBuf {
    if let Some(p) = std::env::var_os(env_var) {
        return PathBuf::from(p);
    }
    let exe_dir = current_exe.parent().unwrap_or_else(|| Path::new("."));
    let sibling = exe_dir.join(bin_name);
    if sibling.exists() {
        return sibling;
    }
    let parent_sibling = exe_dir.parent().map(|p| p.join(bin_name));
    match parent_sibling {
        Some(p) if p.exists() => p,
        _ => sibling,
    }
}

/// The `bossclawd` daemon binary next to the app exe.
pub fn resolve_bin_path(current_exe: &Path) -> PathBuf {
    resolve_sibling_bin(current_exe, BIN_NAME, ENV_BIN)
}

/// The `air-memory-mcp` adapter binary next to the app exe (SP2 one-click integration writes this
/// absolute path into the Claude Code MCP config).
pub fn resolve_memory_bin_path(current_exe: &Path) -> PathBuf {
    resolve_sibling_bin(current_exe, MEMORY_BIN_NAME, ENV_MEMORY_BIN)
}
```
Delete the old body of `resolve_bin_path` (now delegated). Keep the existing `resolve_bin_path` tests — they must still pass unchanged.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p air_agent_desktop engine::daemon 2>&1 | tail -20` → Expected: all daemon tests PASS (old + new).
Run: `cargo clippy -p air_agent_desktop --all-targets -- -D warnings` → Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src-tauri/src/engine/daemon.rs
git commit -m "$(cat <<'EOF'
refactor(desktop): generic resolve_sibling_bin + resolve_memory_bin_path (SP2)

DRY the exe-sibling resolution shared by the bossclawd daemon and the new
air-memory-mcp adapter; add AIR_MEMORY_MCP_BIN override. resolve_bin_path
behavior unchanged (existing tests green).

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Config-writer core — types, JSON helpers, `atomic_write_0600`

**Files:**
- Create: `apps/desktop/src-tauri/src/integrations/mod.rs`
- Modify: `apps/desktop/src-tauri/src/main.rs` (add `mod integrations;`)

- [ ] **Step 1: Register the module (compile scaffold)**

In `apps/desktop/src-tauri/src/main.rs`, add near the other `mod` declarations:
```rust
#[cfg(unix)]
mod integrations;
```

- [ ] **Step 2: Write the failing test**

`apps/desktop/src-tauri/src/integrations/mod.rs` — start with the atomic-write test:
```rust
//! SP2 config-writer core: shared types + JSON merge/read helpers + an atomic 0600 file write.
//! Pure over injected paths (I6) so every case is a hermetic temp-dir test — no real `$HOME`.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub mod claude_code;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_creates_0600_and_replaces_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("cfg.json");

        atomic_write_0600(&target, b"{\"a\":1}").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"{\"a\":1}");
        let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "written file must be 0600, got {mode:o}");

        // Overwrite replaces the content, still 0600.
        atomic_write_0600(&target, b"{\"a\":2}").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"{\"a\":2}");
        let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn read_json_object_absent_is_none_present_is_some_malformed_is_err() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.json");
        assert!(read_json_object(&p).unwrap().is_none(), "absent → None");

        std::fs::write(&p, b"{\"k\":1}").unwrap();
        assert!(read_json_object(&p).unwrap().is_some(), "valid object → Some");

        std::fs::write(&p, b"not json {").unwrap();
        assert!(read_json_object(&p).is_err(), "malformed → Err (fail-loud, never clobber)");
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p air_agent_desktop integrations::tests 2>&1 | tail -20`
Expected: FAIL — `atomic_write_0600` / `read_json_object` not defined.

- [ ] **Step 4: Implement the core helpers**

Add to `integrations/mod.rs` (above the `#[cfg(test)]`):
```rust
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
    pub claude_dir: PathBuf,    // ~/.claude
    pub claude_json: PathBuf,   // ~/.claude.json   (mcpServers)
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

/// Read a JSON file: `Ok(None)` if absent, `Ok(Some(value))` if it parses, `Err` if it exists but
/// is malformed (so callers fail loud and NEVER clobber an unparseable user file — I5).
pub(crate) fn read_json_object(path: &Path) -> std::io::Result<Option<serde_json::Value>> {
    match std::fs::read(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
        Ok(bytes) => {
            let v: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("couldn't parse {}: {e}; nothing changed", path.display()),
                )
            })?;
            Ok(Some(v))
        }
    }
}

/// Serialize pretty + trailing newline (matches how editors/Claude Code leave these files).
pub(crate) fn to_pretty(v: &serde_json::Value) -> Vec<u8> {
    let mut s = serde_json::to_string_pretty(v).expect("serialize json");
    s.push('\n');
    s.into_bytes()
}

/// Atomically write `bytes` to `target` with mode 0600 and no world-readable window: write a temp
/// file in the SAME dir (born 0600 via tempfile on unix; enforced explicitly), fsync, then rename
/// over the target (atomic within one filesystem) — I2.
pub(crate) fn atomic_write_0600(target: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let dir = target.parent().unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    tmp.as_file()
        .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    tmp.persist(target).map_err(|e| e.error)?;
    Ok(())
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p air_agent_desktop integrations::tests 2>&1 | tail -20` → Expected: PASS.
Run: `cargo clippy -p air_agent_desktop --all-targets -- -D warnings` → Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add apps/desktop/src-tauri/src/integrations/mod.rs apps/desktop/src-tauri/src/main.rs
git commit -m "$(cat <<'EOF'
feat(desktop): integrations config-writer core — types + atomic 0600 write (SP2)

ClaudeCodeStatus, ClaudeCodePaths (pure under a home dir), fail-loud
read_json_object, and atomic_write_0600 (temp-in-dir → fsync → rename, 0600).
Hermetic temp-dir tests.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: `detect` — the three-state status read

**Files:**
- Create: `apps/desktop/src-tauri/src/integrations/claude_code.rs`

- [ ] **Step 1: Write the failing test**

`apps/desktop/src-tauri/src/integrations/claude_code.rs`:
```rust
//! The Claude Code adapter: detect / connect / disconnect the `air-memory` MCP server + SessionStart
//! nudge in the user's real config, merging (never replacing) and reversibly. Pure over injected
//! paths; hermetic temp-dir tests. Codex will be a sibling adapter here (SP2 follow-up).

use super::{atomic_write_0600, read_json_object, to_pretty, ClaudeCodePaths, ClaudeCodeStatus};
use std::path::Path;

/// The mcpServers key + the substring that identifies our SessionStart hook command (for
/// idempotent re-write + surgical removal). The command also contains " nudge".
const MCP_SERVER_KEY: &str = "air-memory";
const HOOK_MARKER: &str = "air-memory-mcp";

#[cfg(test)]
mod tests {
    use super::*;

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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p air_agent_desktop integrations::claude_code::tests::detect 2>&1 | tail -20`
Expected: FAIL — `detect` not defined.

- [ ] **Step 3: Implement `detect`**

Add above the `#[cfg(test)]` in `claude_code.rs`:
```rust
/// Read-only status: `NotFound` if Claude Code isn't detected, else `Connected` iff `~/.claude.json`
/// parses and has `mcpServers["air-memory"]`. Lenient on malformed (→ `NotConnected`).
pub fn detect(paths: &ClaudeCodePaths) -> std::io::Result<ClaudeCodeStatus> {
    let present = paths.claude_json.exists() || paths.claude_dir.exists();
    if !present {
        return Ok(ClaudeCodeStatus::NotFound);
    }
    let connected = read_json_object(&paths.claude_json)
        .ok()
        .flatten()
        .and_then(|v| {
            v.get("mcpServers")
                .and_then(|m| m.get(MCP_SERVER_KEY))
                .map(|_| ())
        })
        .is_some();
    Ok(if connected {
        ClaudeCodeStatus::Connected
    } else {
        ClaudeCodeStatus::NotConnected
    })
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p air_agent_desktop integrations::claude_code::tests::detect 2>&1 | tail -20` → Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src-tauri/src/integrations/claude_code.rs
git commit -m "$(cat <<'EOF'
feat(desktop): Claude Code integration detect() — three-state status (SP2)

NotFound / NotConnected / Connected over injected paths; lenient on a
malformed ~/.claude.json (connect() fails loud instead). Hermetic tests.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: `connect` — merge the MCP server + append the nudge hook

**Files:**
- Modify: `apps/desktop/src-tauri/src/integrations/claude_code.rs`

- [ ] **Step 1: Write the failing tests**

Add to the `tests` mod in `claude_code.rs`:
```rust
    fn bin() -> &'static Path {
        Path::new("/Applications/AIR Agent.app/Contents/MacOS/air-memory-mcp")
    }
    fn sock() -> &'static Path {
        Path::new("/Users/me/Library/Application Support/ai.air-agent.desktop/bossclawd.sock")
    }
    fn read(p: &Path) -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap()
    }
    fn mode(p: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn connect_fresh_creates_both_files_0600_with_our_entries() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        connect(&p, bin(), sock()).unwrap();

        let cj = read(&p.claude_json);
        assert_eq!(cj["mcpServers"][MCP_SERVER_KEY]["type"], "stdio");
        assert_eq!(cj["mcpServers"][MCP_SERVER_KEY]["command"], bin().to_string_lossy().as_ref());
        assert_eq!(
            cj["mcpServers"][MCP_SERVER_KEY]["env"]["BOSSCLAWD_SOCKET"],
            sock().to_string_lossy().as_ref()
        );
        assert_eq!(mode(&p.claude_json), 0o600);

        let sj = read(&p.settings_json);
        let groups = sj["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(groups.len(), 1);
        let cmd = groups[0]["hooks"][0]["command"].as_str().unwrap();
        assert!(cmd.contains(HOOK_MARKER) && cmd.contains("nudge"), "hook runs our nudge: {cmd}");
        assert_eq!(mode(&p.settings_json), 0o600);
        assert_eq!(detect(&p).unwrap(), ClaudeCodeStatus::Connected);
    }

    #[test]
    fn connect_preserves_foreign_mcp_servers_and_foreign_hooks() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        std::fs::write(
            &p.claude_json,
            br#"{"mcpServers":{"chrome":{"type":"stdio","command":"x"}},"otherTopKey":42}"#,
        )
        .unwrap();
        std::fs::create_dir(&p.claude_dir).unwrap();
        std::fs::write(
            &p.settings_json,
            br#"{"theme":"dark","hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"node other.mjs"}]}]}}"#,
        )
        .unwrap();

        connect(&p, bin(), sock()).unwrap();

        let cj = read(&p.claude_json);
        assert_eq!(cj["mcpServers"]["chrome"]["command"], "x", "foreign server survives");
        assert_eq!(cj["otherTopKey"], 42, "foreign top-level key survives");
        assert!(cj["mcpServers"][MCP_SERVER_KEY].is_object(), "our server added");

        let sj = read(&p.settings_json);
        assert_eq!(sj["theme"], "dark", "foreign settings key survives");
        let groups = sj["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(groups.len(), 2, "foreign hook survives, ours appended");
        assert_eq!(groups[0]["hooks"][0]["command"], "node other.mjs");
    }

    #[test]
    fn connect_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        connect(&p, bin(), sock()).unwrap();
        connect(&p, bin(), sock()).unwrap();

        let cj = read(&p.claude_json);
        assert!(cj["mcpServers"][MCP_SERVER_KEY].is_object());
        let sj = read(&p.settings_json);
        // Exactly one of OUR groups (a second connect replaces, not duplicates).
        let ours = sj["hooks"]["SessionStart"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|g| {
                g["hooks"][0]["command"]
                    .as_str()
                    .is_some_and(|c| c.contains(HOOK_MARKER))
            })
            .count();
        assert_eq!(ours, 1, "idempotent: one nudge group, not two");
    }

    #[test]
    fn connect_refuses_malformed_claude_json_without_clobber() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        std::fs::write(&p.claude_json, b"not json {").unwrap();
        let err = connect(&p, bin(), sock()).unwrap_err();
        assert!(err.to_string().contains("couldn't parse"), "fail loud: {err}");
        // Untouched.
        assert_eq!(std::fs::read(&p.claude_json).unwrap(), b"not json {");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p air_agent_desktop integrations::claude_code::tests::connect 2>&1 | tail -20`
Expected: FAIL — `connect` not defined.

- [ ] **Step 3: Implement `connect` + its JSON-merge helpers**

Add to `claude_code.rs` (above `#[cfg(test)]`):
```rust
/// Build the mcpServers entry pointing Claude Code at our bundled adapter + the daemon socket.
fn mcp_server_entry(binary: &Path, socket: &Path) -> serde_json::Value {
    serde_json::json!({
        "type": "stdio",
        "command": binary.to_string_lossy(),
        "args": [],
        "env": { "BOSSCLAWD_SOCKET": socket.to_string_lossy() },
    })
}

/// Build the SessionStart hook group that runs `air-memory-mcp nudge`. The binary path is quoted so
/// paths with spaces (e.g. "AIR Agent.app") survive the shell.
fn nudge_hook_group(binary: &Path) -> serde_json::Value {
    serde_json::json!({
        "hooks": [{
            "type": "command",
            "command": format!("\"{}\" nudge", binary.to_string_lossy()),
            "timeout": 5,
        }]
    })
}

/// True iff a SessionStart group is ONE OF OURS (any inner hook command references our binary).
fn is_our_hook_group(group: &serde_json::Value) -> bool {
    group
        .get("hooks")
        .and_then(|h| h.as_array())
        .is_some_and(|hooks| {
            hooks.iter().any(|h| {
                h.get("command")
                    .and_then(|c| c.as_str())
                    .is_some_and(|c| c.contains(HOOK_MARKER))
            })
        })
}

/// Write the `air-memory` MCP server + SessionStart nudge into the Claude Code config, merging
/// (never replacing) and idempotently. Fail-loud on a malformed existing file (never clobbers).
pub fn connect(paths: &ClaudeCodePaths, binary: &Path, socket: &Path) -> std::io::Result<()> {
    // 1. ~/.claude.json — set mcpServers["air-memory"].
    let mut root = read_json_object(&paths.claude_json)?.unwrap_or_else(|| serde_json::json!({}));
    let obj = root
        .as_object_mut()
        .ok_or_else(|| invalid(&paths.claude_json, "top-level JSON is not an object"))?;
    let servers = obj
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));
    let servers = servers
        .as_object_mut()
        .ok_or_else(|| invalid(&paths.claude_json, "\"mcpServers\" is not an object"))?;
    servers.insert(MCP_SERVER_KEY.to_string(), mcp_server_entry(binary, socket));
    atomic_write_0600(&paths.claude_json, &to_pretty(&root))?;

    // 2. ~/.claude/settings.json — prune our old group(s), append a fresh nudge group.
    std::fs::create_dir_all(&paths.claude_dir)?;
    let mut settings =
        read_json_object(&paths.settings_json)?.unwrap_or_else(|| serde_json::json!({}));
    let sobj = settings
        .as_object_mut()
        .ok_or_else(|| invalid(&paths.settings_json, "top-level JSON is not an object"))?;
    let hooks = sobj.entry("hooks").or_insert_with(|| serde_json::json!({}));
    let hooks = hooks
        .as_object_mut()
        .ok_or_else(|| invalid(&paths.settings_json, "\"hooks\" is not an object"))?;
    let starts = hooks
        .entry("SessionStart")
        .or_insert_with(|| serde_json::json!([]));
    let starts = starts
        .as_array_mut()
        .ok_or_else(|| invalid(&paths.settings_json, "\"hooks.SessionStart\" is not an array"))?;
    starts.retain(|g| !is_our_hook_group(g));
    starts.push(nudge_hook_group(binary));
    atomic_write_0600(&paths.settings_json, &to_pretty(&settings))?;
    Ok(())
}

fn invalid(path: &Path, why: &str) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("couldn't parse {}: {why}; nothing changed", path.display()),
    )
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p air_agent_desktop integrations::claude_code::tests 2>&1 | tail -20` → Expected: PASS (all detect + connect tests).
Run: `cargo clippy -p air_agent_desktop --all-targets -- -D warnings` → Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src-tauri/src/integrations/claude_code.rs
git commit -m "$(cat <<'EOF'
feat(desktop): Claude Code connect() — merge MCP server + nudge hook (SP2)

Read-merge-write both files: set mcpServers["air-memory"], prune+append our
SessionStart nudge group. Idempotent, preserves foreign servers/hooks/keys,
fail-loud on malformed (no clobber). Hermetic tests incl. the real machine's
"foreign SessionStart hook already present" case.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: `disconnect` — surgically remove only our entries

**Files:**
- Modify: `apps/desktop/src-tauri/src/integrations/claude_code.rs`

- [ ] **Step 1: Write the failing tests**

Add to the `tests` mod:
```rust
    #[test]
    fn disconnect_removes_only_ours_and_keeps_foreign() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        // Seed foreign + ours.
        std::fs::write(
            &p.claude_json,
            br#"{"mcpServers":{"chrome":{"command":"x"}},"top":1}"#,
        )
        .unwrap();
        std::fs::create_dir(&p.claude_dir).unwrap();
        std::fs::write(
            &p.settings_json,
            br#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"node other.mjs"}]}]}}"#,
        )
        .unwrap();
        connect(&p, bin(), sock()).unwrap();

        disconnect(&p).unwrap();

        let cj = read(&p.claude_json);
        assert!(cj["mcpServers"][MCP_SERVER_KEY].is_null(), "our server removed");
        assert_eq!(cj["mcpServers"]["chrome"]["command"], "x", "foreign server kept");
        assert_eq!(cj["top"], 1, "foreign top-level key kept");

        let sj = read(&p.settings_json);
        let groups = sj["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(groups.len(), 1, "only the foreign hook remains");
        assert_eq!(groups[0]["hooks"][0]["command"], "node other.mjs");
        assert_eq!(detect(&p).unwrap(), ClaudeCodeStatus::NotConnected);
    }

    #[test]
    fn disconnect_is_a_noop_when_not_connected() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        std::fs::write(&p.claude_json, br#"{"mcpServers":{"chrome":{}}}"#).unwrap();
        disconnect(&p).unwrap(); // must not error
        assert_eq!(read(&p.claude_json)["mcpServers"]["chrome"].is_object(), true);
    }

    #[test]
    fn disconnect_refuses_malformed_without_clobber() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        std::fs::write(&p.claude_json, b"not json {").unwrap();
        assert!(disconnect(&p).is_err());
        assert_eq!(std::fs::read(&p.claude_json).unwrap(), b"not json {");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p air_agent_desktop integrations::claude_code::tests::disconnect 2>&1 | tail -20`
Expected: FAIL — `disconnect` not defined.

- [ ] **Step 3: Implement `disconnect`**

Add to `claude_code.rs`:
```rust
/// Remove ONLY our `air-memory` MCP server + our SessionStart nudge group(s), preserving everything
/// else. Absent files → nothing to do. Fail-loud on a malformed existing file (no clobber).
pub fn disconnect(paths: &ClaudeCodePaths) -> std::io::Result<()> {
    if let Some(mut root) = read_json_object(&paths.claude_json)? {
        let mut changed = false;
        if let Some(servers) = root.get_mut("mcpServers").and_then(|m| m.as_object_mut()) {
            changed |= servers.remove(MCP_SERVER_KEY).is_some();
        }
        if changed {
            atomic_write_0600(&paths.claude_json, &to_pretty(&root))?;
        }
    }
    if let Some(mut settings) = read_json_object(&paths.settings_json)? {
        let mut changed = false;
        if let Some(starts) = settings
            .get_mut("hooks")
            .and_then(|h| h.get_mut("SessionStart"))
            .and_then(|s| s.as_array_mut())
        {
            let before = starts.len();
            starts.retain(|g| !is_our_hook_group(g));
            changed = starts.len() != before;
        }
        if changed {
            atomic_write_0600(&paths.settings_json, &to_pretty(&settings))?;
        }
    }
    Ok(())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p air_agent_desktop integrations::claude_code 2>&1 | tail -20` → Expected: PASS (all detect/connect/disconnect).
Run: `cargo clippy -p air_agent_desktop --all-targets -- -D warnings` → Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src-tauri/src/integrations/claude_code.rs
git commit -m "$(cat <<'EOF'
feat(desktop): Claude Code disconnect() — surgical reversal (SP2)

Removes only mcpServers["air-memory"] + our SessionStart nudge group(s);
foreign servers/hooks/keys survive. No-op when absent; fail-loud on malformed.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Tauri command surface

**Files:**
- Create: `apps/desktop/src-tauri/src/commands/integrations.rs`
- Modify: `apps/desktop/src-tauri/src/commands/mod.rs`
- Modify: `apps/desktop/src-tauri/src/main.rs`

- [ ] **Step 1: Write the failing test**

The commands are thin wrappers over tested pure fns; the one piece with logic is resolving the home dir. Put that behind a pure helper and test it. `apps/desktop/src-tauri/src/commands/integrations.rs`:
```rust
//! Tauri commands for the SP2 one-click Claude Code integration. Thin: resolve the real paths
//! (adapter binary via current_exe, socket via bossclawd-paths, config under $HOME) and delegate to
//! the tested pure `crate::integrations` core.

use crate::integrations::{self, claude_code, ClaudeCodePaths, ClaudeCodeStatus};
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
```

- [ ] **Step 2: Run test to verify it fails (compile)**

Run: `cargo test -p air_agent_desktop commands::integrations 2>&1 | tail -20`
Expected: FAIL to compile — `commands::integrations` not declared in `commands/mod.rs` yet.

- [ ] **Step 3: Wire the module + implement the commands**

In `apps/desktop/src-tauri/src/commands/mod.rs` add:
```rust
#[cfg(unix)]
pub mod integrations;
```

Append the commands to `commands/integrations.rs`:
```rust
#[tauri::command]
pub fn integrations_status() -> Result<IntegrationsStatusDto, String> {
    let paths = ClaudeCodePaths::under(&home_dir()?);
    let claude_code = claude_code::detect(&paths).map_err(|e| e.to_string())?;
    Ok(IntegrationsStatusDto { claude_code })
}

#[tauri::command]
pub fn integrations_connect_claude_code() -> Result<IntegrationsStatusDto, String> {
    let home = home_dir()?;
    let paths = ClaudeCodePaths::under(&home);
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let binary = crate::engine::daemon::resolve_memory_bin_path(&exe);
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

// Codex: SP2 follow-up — a sibling `commands::integrations::*_codex` + `integrations::codex`
// adapter writing ~/.codex/config.toml, reusing atomic_write_0600 + the status enum.
```
Note: `integrations` is imported for the seam comment + future use; if clippy flags the unused import at this stage, scope the `use` to what the commands reference (`claude_code`, `ClaudeCodePaths`, `ClaudeCodeStatus`).

In `apps/desktop/src-tauri/src/main.rs`, add to `generate_handler!` (next to the other `#[cfg(unix)] commands::engine::*` entries):
```rust
            #[cfg(unix)]
            commands::integrations::integrations_status,
            #[cfg(unix)]
            commands::integrations::integrations_connect_claude_code,
            #[cfg(unix)]
            commands::integrations::integrations_disconnect_claude_code,
```

- [ ] **Step 4: Run tests + build to verify**

Run: `cargo test -p air_agent_desktop commands::integrations 2>&1 | tail -20` → Expected: PASS.
Run: `cargo build -p air_agent_desktop 2>&1 | tail -5` → Expected: builds (command registration type-checks).
Run: `cargo clippy -p air_agent_desktop --all-targets -- -D warnings` → Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src-tauri/src/commands/integrations.rs apps/desktop/src-tauri/src/commands/mod.rs apps/desktop/src-tauri/src/main.rs
git commit -m "$(cat <<'EOF'
feat(desktop): Tauri commands for one-click Claude Code integration (SP2)

integrations_status / _connect_claude_code / _disconnect_claude_code —
thin wrappers resolving the adapter binary (current_exe), socket
(bossclawd-paths), and config ($HOME), delegating to the pure core.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: Frontend API wrappers

**Files:**
- Create: `apps/desktop/src/api/integrations.ts`

- [ ] **Step 1: Write the failing test**

`apps/desktop/src/api/integrations.test.ts`:
```ts
import { describe, it, expect, vi, beforeEach } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { integrationsStatus, connectClaudeCode, disconnectClaudeCode } from "./integrations";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

describe("api/integrations", () => {
  beforeEach(() => vi.resetAllMocks());

  it("integrationsStatus invokes the status command", async () => {
    vi.mocked(invoke).mockResolvedValue({ claude_code: "not_connected" });
    expect(await integrationsStatus()).toEqual({ claude_code: "not_connected" });
    expect(invoke).toHaveBeenCalledWith("integrations_status");
  });

  it("connect/disconnect invoke their commands", async () => {
    vi.mocked(invoke).mockResolvedValue({ claude_code: "connected" });
    await connectClaudeCode();
    expect(invoke).toHaveBeenCalledWith("integrations_connect_claude_code");
    vi.mocked(invoke).mockResolvedValue({ claude_code: "not_connected" });
    await disconnectClaudeCode();
    expect(invoke).toHaveBeenCalledWith("integrations_disconnect_claude_code");
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm run -w @air-agent/desktop test -- src/api/integrations.test.ts` (or the repo's vitest invocation — confirm from `package.json`)
Expected: FAIL — `./integrations` module not found.

- [ ] **Step 3: Implement the wrappers**

`apps/desktop/src/api/integrations.ts`:
```ts
import { invoke } from "@tauri-apps/api/core";

/** Mirrors the Rust `ClaudeCodeStatus` (serde snake_case). */
export type ClaudeCodeStatus = "not_found" | "not_connected" | "connected";
export type IntegrationsStatusDto = { claude_code: ClaudeCodeStatus };

export const integrationsStatus = (): Promise<IntegrationsStatusDto> =>
  invoke<IntegrationsStatusDto>("integrations_status");
export const connectClaudeCode = (): Promise<IntegrationsStatusDto> =>
  invoke<IntegrationsStatusDto>("integrations_connect_claude_code");
export const disconnectClaudeCode = (): Promise<IntegrationsStatusDto> =>
  invoke<IntegrationsStatusDto>("integrations_disconnect_claude_code");
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npm run -w @air-agent/desktop test -- src/api/integrations.test.ts` → Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src/api/integrations.ts apps/desktop/src/api/integrations.test.ts
git commit -m "$(cat <<'EOF'
feat(desktop): frontend api/integrations wrappers (SP2)

integrationsStatus / connectClaudeCode / disconnectClaudeCode over invoke,
mirroring the Rust status DTO.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: Settings ▸ Integrations panel

**Files:**
- Create: `apps/desktop/src/settings/IntegrationsPanel.tsx`
- Create: `apps/desktop/src/settings/IntegrationsPanel.test.tsx`
- Modify: `apps/desktop/src/settings/AirSettings.tsx`

- [ ] **Step 1: Write the failing test**

`apps/desktop/src/settings/IntegrationsPanel.test.tsx` (mock the api module — the "DI/mock-the-module over mockIPC" lesson):
```tsx
// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { IntegrationsPanel } from "./IntegrationsPanel";
import * as api from "../api/integrations";

vi.mock("../api/integrations", () => ({
  integrationsStatus: vi.fn(),
  connectClaudeCode: vi.fn(),
  disconnectClaudeCode: vi.fn(),
}));

describe("IntegrationsPanel", () => {
  beforeEach(() => vi.resetAllMocks());

  it("shows Connect when detected but not connected", async () => {
    vi.mocked(api.integrationsStatus).mockResolvedValue({ claude_code: "not_connected" });
    render(<IntegrationsPanel />);
    expect(await screen.findByRole("button", { name: /connect claude code/i })).toBeEnabled();
  });

  it("shows Disconnect when connected", async () => {
    vi.mocked(api.integrationsStatus).mockResolvedValue({ claude_code: "connected" });
    render(<IntegrationsPanel />);
    expect(await screen.findByRole("button", { name: /disconnect/i })).toBeInTheDocument();
  });

  it("disables the action and hints when Claude Code is not found", async () => {
    vi.mocked(api.integrationsStatus).mockResolvedValue({ claude_code: "not_found" });
    render(<IntegrationsPanel />);
    expect(await screen.findByText(/install claude code/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /connect claude code/i })).toBeDisabled();
  });

  it("clicking Connect calls the command and refreshes to Connected", async () => {
    vi.mocked(api.integrationsStatus).mockResolvedValue({ claude_code: "not_connected" });
    vi.mocked(api.connectClaudeCode).mockResolvedValue({ claude_code: "connected" });
    render(<IntegrationsPanel />);
    fireEvent.click(await screen.findByRole("button", { name: /connect claude code/i }));
    await waitFor(() => expect(api.connectClaudeCode).toHaveBeenCalledOnce());
    expect(await screen.findByRole("button", { name: /disconnect/i })).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm run -w @air-agent/desktop test -- src/settings/IntegrationsPanel.test.tsx`
Expected: FAIL — `./IntegrationsPanel` not found.

- [ ] **Step 3: Implement the panel**

`apps/desktop/src/settings/IntegrationsPanel.tsx` (tokens only — no hardcoded colors; mirrors SourcesPanel's section shell):
```tsx
import { useEffect, useState } from "react";
import { Button } from "../components/Button";
import {
  integrationsStatus, connectClaudeCode, disconnectClaudeCode,
  type ClaudeCodeStatus,
} from "../api/integrations";

export function IntegrationsPanel() {
  const [status, setStatus] = useState<ClaudeCodeStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = async () => {
    try {
      setStatus((await integrationsStatus()).claude_code);
    } catch (e) {
      setError(String(e));
    }
  };
  useEffect(() => { void refresh(); }, []);

  const run = async (fn: () => Promise<{ claude_code: ClaudeCodeStatus }>) => {
    setBusy(true);
    setError(null);
    try {
      setStatus((await fn()).claude_code);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const notFound = status === "not_found";
  const connected = status === "connected";

  return (
    <div style={{ marginTop: 24, paddingTop: 16, borderTop: "1px solid var(--border-soft)" }}>
      <div style={{ fontWeight: 600, marginBottom: 4 }}>Integrations</div>
      <p style={{ color: "var(--text-secondary)", fontSize: 13 }}>
        Connect your coding tools to your agent’s memory. Claude Code will be able to recall your
        notes and remember new ones — in every project.
      </p>

      <div style={{ display: "flex", alignItems: "center", gap: 8, margin: "8px 0" }}>
        <span style={{ fontSize: 13 }}>Claude Code</span>
        {connected ? (
          <Button variant="secondary" disabled={busy} onClick={() => void run(disconnectClaudeCode)}>
            {busy ? "Working…" : "Disconnect"}
          </Button>
        ) : (
          <Button
            variant="primary"
            disabled={busy || notFound}
            onClick={() => void run(connectClaudeCode)}
          >
            {busy ? "Connecting…" : "Connect Claude Code"}
          </Button>
        )}
      </div>

      {connected ? (
        <p style={{ fontSize: 12, color: "var(--text-tertiary)" }}>
          Connected. Takes effect the next time you start Claude Code.
        </p>
      ) : null}
      {notFound ? (
        <p style={{ fontSize: 13, color: "var(--text-secondary)" }}>
          Claude Code not found — install it first, then reopen this page.
        </p>
      ) : null}
      {error ? <p style={{ fontSize: 13, color: "var(--error)" }}>{error}</p> : null}
    </div>
  );
}
```

Wire it into `apps/desktop/src/settings/AirSettings.tsx` — add the import and render `<IntegrationsPanel />` right after `<SourcesPanel />`:
```tsx
import { IntegrationsPanel } from "./IntegrationsPanel";
// …
        <SourcesPanel />
        <IntegrationsPanel />
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `npm run -w @air-agent/desktop test -- src/settings/IntegrationsPanel.test.tsx` → Expected: PASS.
Run: `npm run -w @air-agent/desktop typecheck` and the repo's eslint (0 warnings). Confirm 0 hardcoded colors: `grep -nE "#[0-9a-fA-F]{3,6}" apps/desktop/src/settings/IntegrationsPanel.tsx` → Expected: no matches.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src/settings/IntegrationsPanel.tsx apps/desktop/src/settings/IntegrationsPanel.test.tsx apps/desktop/src/settings/AirSettings.tsx
git commit -m "$(cat <<'EOF'
feat(desktop): Settings ▸ Integrations panel — one-click Claude Code (SP2)

Status + Connect/Disconnect over the integrations commands; disabled with a
hint when Claude Code isn't detected; tokens only (no hardcoded colors).

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: End-to-end round-trip + README

**Files:**
- Modify: `apps/desktop/src-tauri/src/integrations/claude_code.rs` (add one in-crate `#[cfg(test)]` round-trip test)
- Modify: `crates/air-memory-mcp/README.md`

**Layout note (verified):** `air_agent_desktop` is a **bin-only crate — there is NO `apps/desktop/src-tauri/src/lib.rs`**, so a `tests/` integration test cannot import the crate. The end-to-end round-trip therefore lives as an in-crate `#[cfg(test)]` test inside `claude_code.rs`, reusing that module's existing test helpers (`paths`, `bin`, `sock`, `read`, `mode`). It runs under `cargo test -p air_agent_desktop` with the rest of the unit tests.

- [ ] **Step 1: Write the regression-guard test**

This documents the full connect → detect → disconnect cycle with foreign entries in BOTH files (the real-machine state) in one place. Because Tasks 4–7 already implement the behavior, this is a **regression/documentation guard**, not RED-first (that's fine — it locks the whole cycle). Add to the `tests` mod in `apps/desktop/src-tauri/src/integrations/claude_code.rs`:
```rust
    #[test]
    fn end_to_end_connect_disconnect_preserves_foreign_and_stays_0600() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        std::fs::write(&p.claude_json, br#"{"mcpServers":{"chrome":{"command":"x"}}}"#).unwrap();
        std::fs::create_dir(&p.claude_dir).unwrap();
        std::fs::write(
            &p.settings_json,
            br#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"node keep.mjs"}]}]}}"#,
        )
        .unwrap();

        connect(&p, bin(), sock()).unwrap();
        assert_eq!(detect(&p).unwrap(), ClaudeCodeStatus::Connected);
        assert_eq!(mode(&p.claude_json), 0o600);
        assert_eq!(mode(&p.settings_json), 0o600);

        disconnect(&p).unwrap();
        assert_eq!(detect(&p).unwrap(), ClaudeCodeStatus::NotConnected);

        let cj = read(&p.claude_json);
        assert_eq!(cj["mcpServers"]["chrome"]["command"], "x", "foreign server survives the cycle");
        let sj = read(&p.settings_json);
        let groups = sj["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(groups.len(), 1, "only the foreign hook remains");
        assert_eq!(groups[0]["hooks"][0]["command"], "node keep.mjs");
    }
```

- [ ] **Step 2: Run it**

Run: `cargo test -p air_agent_desktop integrations::claude_code::tests::end_to_end 2>&1 | tail -20`
Expected: PASS (behavior from Tasks 4–7). If it fails, a prior task regressed — fix there, don't paper over it here.

- [ ] **Step 3: Full module test pass**

Run: `cargo test -p air_agent_desktop integrations::claude_code 2>&1 | tail -20` → Expected: PASS (all detect/connect/disconnect + e2e).

- [ ] **Step 4: Update the README**

In `crates/air-memory-mcp/README.md`, replace the "Wire it into Claude Code (manual, SP1)" section with a one-click-first version:
```markdown
## Wire it into Claude Code

**One click (recommended):** in the AIR Agent app, open **Settings ▸ Integrations** and click
**Connect Claude Code**. This writes the `air-memory` MCP server to `~/.claude.json` and a
SessionStart nudge to `~/.claude/settings.json` (merging with your existing config, never
replacing it), so every Claude Code session everywhere can `recall`/`remember`. **Disconnect**
removes exactly those entries. Takes effect on your next Claude Code session.

**Manual (advanced / headless):** add to your `.mcp.json` — see the entry shape below.
```
Keep the existing JSON snippet + the `BOSSCLAWD_SOCKET` / daemon-down notes beneath, relabeled as the manual path.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p air_agent_desktop integrations::claude_code 2>&1 | tail -20` → Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add apps/desktop/src-tauri/src/integrations/claude_code.rs crates/air-memory-mcp/README.md
git commit -m "$(cat <<'EOF'
test(desktop)+docs: SP2 end-to-end round-trip + one-click README

Connect→detect→disconnect preserves a foreign server + foreign SessionStart
hook, both files 0600 throughout. README leads with one-click, keeps the
manual path for headless/advanced use.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
)"
```

---

## Final gates (before the PR flips to ready)

Run from repo root:
```bash
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p air-memory-mcp -p air_agent_desktop
npm run -w @air-agent/desktop typecheck
npm run -w @air-agent/desktop test
# eslint (0 warnings) per the repo's lint script; and:
grep -rnE "#[0-9a-fA-F]{3,6}" apps/desktop/src/settings/IntegrationsPanel.tsx   # expect: no matches
```
All green. Then whole-branch review (+ the pre-code architect/critic/security pass already done). Confirm the daemon-crates CI job still passes (it compiles/tests `air-memory-mcp`). Rebase onto `main` once PR #76 merges; open the SP2 PR (stacked on #76 until then).

---

## Self-Review (plan vs spec)

- **Spec coverage:** U1 (bundling) → Tasks 2+3; U2 (config-writer) → Tasks 4–7; U3 (nudge) → Task 1; U4 (commands) → Task 8; U5 (panel) → Tasks 9–10; U6 (docs+e2e) → Task 11. Invariants I1–I7: merge (T6/T7 foreign-preservation tests), atomic-0600 (T4 + T11 mode asserts), idempotent (T6), reversible (T7), fail-loud (T4/T6/T7 malformed tests), pure-over-paths (all `under(home)` tests), agent-seam (T8 Codex comment + no dead code). All §8 error cases have a task/test except the "concurrent running Claude Code" race (documented-only, by design). ✓
- **Placeholder scan:** every step has real code/commands + expected output. Layout facts verified against the repo (crate `air_agent_desktop`, bin-only / no `lib.rs` → e2e is an in-crate test; workspace `@air-agent/desktop`; `serde_json`+`tempfile` already deps). No blanks. ✓
- **Type consistency:** `ClaudeCodeStatus` (Rust snake_case serde) ↔ `"not_found"|"not_connected"|"connected"` (TS); `MCP_SERVER_KEY="air-memory"` + `HOOK_MARKER="air-memory-mcp"` used consistently in connect/disconnect/detect; `resolve_memory_bin_path` name matches Task 3 → Task 8; command names match Rust `#[tauri::command]` fns → TS `invoke("…")` strings → panel test `.toHaveBeenCalledWith`. ✓
