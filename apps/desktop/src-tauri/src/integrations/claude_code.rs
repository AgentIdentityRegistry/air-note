//! The Claude Code adapter: detect / connect / disconnect the `air-memory` MCP server + SessionStart
//! nudge in the user's real config, merging (never replacing) and reversibly. Pure over injected
//! paths; hermetic temp-dir tests. Codex will be a sibling adapter here (SP2 follow-up).

use super::{
    atomic_write_0600, make_private_dir, read_json_object, to_pretty, ClaudeCodePaths,
    ClaudeCodeStatus,
};
use std::path::Path;

/// The mcpServers key that names our entry — the presence check for `detect` and the idempotent
/// key for connect/disconnect (added in Task 6/7).
const MCP_SERVER_KEY: &str = "air-memory";

/// Identifies our SessionStart hook command for surgical removal — matched together with the
/// `nudge` subcommand (BOTH required) so a foreign hook that merely mentions the string is never
/// removed (review Low).
const HOOK_MARKER: &str = "air-memory-mcp";

/// Read-only status (SP3 R2 — three states): `NotFound` if Claude Code isn't detected; else
/// `NotConnected` unless `~/.claude.json` parses with `mcpServers["air-memory"]`; else
/// `Connected { capture }` where `capture` is true iff `settings.json`'s `hooks.SessionEnd[]` also
/// holds OUR `capture-notify` group. An SP2-era connect (mcpServers present, no capture hook) is thus
/// `Connected { capture: false }`, which Plan C surfaces as "Re-connect to enable session capture".
/// Lenient on malformed at every layer (malformed `claude.json` → `NotConnected`; missing/malformed
/// `settings.json` → `capture: false`) — status is a safe read; `connect()` is where a parse error
/// fails loud.
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
    if !connected {
        return Ok(ClaudeCodeStatus::NotConnected);
    }
    Ok(ClaudeCodeStatus::Connected { capture: session_end_has_our_capture(&paths.settings_json) })
}

/// True iff `settings.json`'s `hooks.SessionEnd[]` contains OUR capture group — the B4 marker (a
/// command with BOTH `air-memory-mcp` AND `capture-notify`, via [`is_our_capture_group`]). Lenient by
/// design (this backs a read-only status, not a mutation): a missing / malformed / oddly-shaped
/// `settings.json` → `false`. Reuses the same predicate `connect`/`disconnect` write and remove with,
/// so detect can never disagree with what was actually installed.
fn session_end_has_our_capture(settings_json: &Path) -> bool {
    read_json_object(settings_json)
        .ok()
        .flatten()
        .and_then(|v| {
            v.get("hooks")
                .and_then(|h| h.get("SessionEnd"))
                .and_then(|e| e.as_array())
                .map(|groups| groups.iter().any(is_our_capture_group))
        })
        .unwrap_or(false)
}

/// Count Claude Code transcripts under `projects_root` for the Connect consent disclosure
/// (spec §6a: "…including your recent sessions (~30 days, N found)"). Mirrors the daemon sweeper's
/// walk (A9 `scan_candidates`): ONE level of project dirs, then `.jsonl` leaves directly inside each —
/// so the disclosed N equals what the sweeper would actually scan, never an over-promise. A missing /
/// unreadable root is `0`, NEVER an error: this is a best-effort disclosure, and a not-yet-connected
/// user (no `~/.claude/projects`) legitimately has zero. Nested-deeper directories and stray
/// root-level files are not transcripts here, exactly as the sweeper ignores them.
pub fn backfill_candidate_count(projects_root: &Path) -> u64 {
    let Ok(projects) = std::fs::read_dir(projects_root) else {
        return 0; // absent / unreadable root → nothing to import (I10 parity with the sweeper).
    };
    let mut count = 0u64;
    for project in projects.flatten() {
        if !project.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue; // only project directories hold transcripts (stray root files are ignored).
        }
        let Ok(entries) = std::fs::read_dir(project.path()) else {
            continue; // unreadable project dir — skip it, don't abort the whole count.
        };
        for entry in entries.flatten() {
            let is_transcript = entry.path().extension().and_then(|e| e.to_str()) == Some("jsonl")
                && entry.file_type().map(|t| t.is_file()).unwrap_or(false);
            if is_transcript {
                count += 1; // a `.jsonl` leaf directly inside a project dir (no recursion).
            }
        }
    }
    count
}

/// Build the mcpServers entry pointing Claude Code at our bundled adapter + the daemon socket.
/// The `command` is spawned by Claude Code as argv[0] (NOT via a shell), so it needs no quoting —
/// only the SessionStart hook string below is shell-executed.
fn mcp_server_entry(binary: &Path, socket: &Path) -> serde_json::Value {
    serde_json::json!({
        "type": "stdio",
        "command": binary.to_string_lossy(),
        "args": [],
        "env": { "BOSSCLAWD_SOCKET": socket.to_string_lossy() },
    })
}

/// POSIX single-quote a string into one inert shell literal: wrap in `'…'` and rewrite every `'` as
/// `'\''`. Single quotes disable ALL shell expansion (`"`, `$`, backtick, `\`), so a binary path
/// with a metacharacter (or a hostile `AIR_MEMORY_MCP_BIN`) cannot inject a command (review Medium).
fn sh_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The SessionStart hook group that runs `<binary> nudge`. The hook `command` IS shell-executed by
/// Claude Code, so the path is single-quote-escaped (plain double quotes would NOT neutralize
/// `"`/`$`/backtick/`\`).
fn nudge_hook_group(binary: &Path) -> serde_json::Value {
    serde_json::json!({
        "hooks": [{
            "type": "command",
            "command": format!("{} nudge", sh_single_quote(&binary.to_string_lossy())),
            "timeout": 5,
        }]
    })
}

/// The SessionEnd hook group that runs `<binary> capture-notify` — Claude Code's end-of-session poke
/// that asks the daemon to snapshot the just-finished session (SP3/B2). Same shell-executed command,
/// so the path is single-quote-escaped exactly like the nudge; the only new token is a static literal.
fn capture_hook_group(binary: &Path) -> serde_json::Value {
    serde_json::json!({
        "hooks": [{
            "type": "command",
            "command": format!("{} capture-notify", sh_single_quote(&binary.to_string_lossy())),
            "timeout": 5,
        }]
    })
}

/// True iff a SessionStart group is ONE OF OURS: an inner command that BOTH references our binary
/// (`HOOK_MARKER`) AND invokes `nudge`. Requiring both keeps disconnect from deleting a foreign hook
/// that merely mentions the string (review Low).
fn is_our_hook_group(group: &serde_json::Value) -> bool {
    group_has_command_with(group, "nudge")
}

/// True iff a SessionEnd group is ONE OF OURS: an inner command that BOTH references our binary
/// (`HOOK_MARKER`) AND invokes `capture-notify`. DISJOINT from `is_our_hook_group`'s nudge marker —
/// a `capture-notify` command never contains `nudge` and vice versa — so the two never cross-match,
/// and a foreign hook that merely mentions `air-memory-mcp` (without `capture-notify`) survives.
fn is_our_capture_group(group: &serde_json::Value) -> bool {
    group_has_command_with(group, "capture-notify")
}

/// Shared marker predicate: some inner hook command contains BOTH `HOOK_MARKER` AND `subcommand`.
fn group_has_command_with(group: &serde_json::Value, subcommand: &str) -> bool {
    group.get("hooks").and_then(|h| h.as_array()).is_some_and(|hooks| {
        hooks.iter().any(|h| {
            h.get("command")
                .and_then(|c| c.as_str())
                .is_some_and(|c| c.contains(HOOK_MARKER) && c.contains(subcommand))
        })
    })
}

/// `obj[key]` as an object, creating an empty one if absent; `Err` if it exists as a non-object.
fn ensure_object<'a>(
    obj: &'a mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    path: &Path,
) -> std::io::Result<&'a mut serde_json::Map<String, serde_json::Value>> {
    obj.entry(key)
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| invalid(path, &format!("\"{key}\" is not an object")))
}

/// `obj[key]` as an array, creating an empty one if absent; `Err` if it exists as a non-array.
fn ensure_array<'a>(
    obj: &'a mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    path: &Path,
) -> std::io::Result<&'a mut Vec<serde_json::Value>> {
    obj.entry(key)
        .or_insert_with(|| serde_json::json!([]))
        .as_array_mut()
        .ok_or_else(|| invalid(path, &format!("\"{key}\" is not an array")))
}

/// Drop OUR groups from `hooks[kind]` (matched by `is_ours`), leaving foreign groups in place.
/// Returns whether anything was removed (so the caller only rewrites a file it actually changed). A
/// missing / non-array `hooks[kind]` is a no-op — nothing of ours to remove there.
fn retain_out_group(
    hooks: &mut serde_json::Map<String, serde_json::Value>,
    kind: &str,
    is_ours: fn(&serde_json::Value) -> bool,
) -> bool {
    hooks.get_mut(kind).and_then(|v| v.as_array_mut()).is_some_and(|groups| {
        let before = groups.len();
        groups.retain(|g| !is_ours(g));
        groups.len() != before
    })
}

/// Write the `air-memory` MCP server + SessionStart nudge + SessionEnd `capture-notify` hook into the
/// Claude Code config, merging (never replacing) and idempotently. **Both files are parsed +
/// shape-validated BEFORE either is written** (I5): a malformed / oddly-shaped `settings.json` fails
/// loud with `claude.json` left byte-unchanged — no partial write, no lying "nothing changed" (review
/// MAJOR M3).
pub fn connect(paths: &ClaudeCodePaths, binary: &Path, socket: &Path) -> std::io::Result<()> {
    // ── Phase 1: parse + validate + mutate IN MEMORY (no writes yet). ──
    let mut claude = read_json_object(&paths.claude_json)?.unwrap_or_else(|| serde_json::json!({}));
    let claude_obj = claude
        .as_object_mut()
        .ok_or_else(|| invalid(&paths.claude_json, "top-level JSON is not an object"))?;
    ensure_object(claude_obj, "mcpServers", &paths.claude_json)?
        .insert(MCP_SERVER_KEY.to_string(), mcp_server_entry(binary, socket));

    let mut settings =
        read_json_object(&paths.settings_json)?.unwrap_or_else(|| serde_json::json!({}));
    let settings_obj = settings
        .as_object_mut()
        .ok_or_else(|| invalid(&paths.settings_json, "top-level JSON is not an object"))?;
    let hooks = ensure_object(settings_obj, "hooks", &paths.settings_json)?;
    let starts = ensure_array(hooks, "SessionStart", &paths.settings_json)?;
    starts.retain(|g| !is_our_hook_group(g)); // idempotent: drop a prior nudge, then re-add.
    starts.push(nudge_hook_group(binary));
    // The SessionEnd capture hook joins the SAME in-memory transaction (heals an SP2-era config that
    // has the nudge but no SessionEnd): retain-out any prior capture, then re-add exactly one.
    let ends = ensure_array(hooks, "SessionEnd", &paths.settings_json)?;
    ends.retain(|g| !is_our_capture_group(g));
    ends.push(capture_hook_group(binary));

    // ── Phase 2: both parsed cleanly → create dir + write both (0600, atomic). ──
    make_private_dir(&paths.claude_dir)?;
    atomic_write_0600(&paths.claude_json, &to_pretty(&claude))?;
    atomic_write_0600(&paths.settings_json, &to_pretty(&settings))?;
    Ok(())
}

/// Remove ONLY our `air-memory` MCP server + our SessionStart nudge group(s) + our SessionEnd
/// `capture-notify` group(s), preserving everything else. **Both files are parsed BEFORE either is
/// written** (I5) — a malformed file fails loud with nothing clobbered. Absent files → nothing to do;
/// only files we actually change are rewritten.
pub fn disconnect(paths: &ClaudeCodePaths) -> std::io::Result<()> {
    // Parse both up front — a malformed file errors here, before any write.
    let claude = read_json_object(&paths.claude_json)?;
    let settings = read_json_object(&paths.settings_json)?;

    let claude_out = claude.and_then(|mut root| {
        let removed = root
            .get_mut("mcpServers")
            .and_then(|m| m.as_object_mut())
            .is_some_and(|servers| servers.remove(MCP_SERVER_KEY).is_some());
        removed.then_some(root)
    });
    let settings_out = settings.and_then(|mut root| {
        // Retain-out BOTH our hook kinds; a foreign group in EITHER array survives. `|` (not `||`) so
        // both removals always run — a changed SessionEnd must still be written even if SessionStart
        // was untouched (short-circuit would skip it).
        let changed = root.get_mut("hooks").and_then(|h| h.as_object_mut()).is_some_and(|hooks| {
            retain_out_group(hooks, "SessionStart", is_our_hook_group)
                | retain_out_group(hooks, "SessionEnd", is_our_capture_group)
        });
        changed.then_some(root)
    });

    if let Some(root) = claude_out {
        atomic_write_0600(&paths.claude_json, &to_pretty(&root))?;
    }
    if let Some(root) = settings_out {
        atomic_write_0600(&paths.settings_json, &to_pretty(&root))?;
    }
    Ok(())
}

fn invalid(path: &Path, why: &str) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("couldn't parse {}: {why}; nothing changed", path.display()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn paths(home: &Path) -> ClaudeCodePaths {
        ClaudeCodePaths::under(home)
    }

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
        // mcpServers present but NO settings.json capture hook (SP2-era shape) → capture:false.
        assert_eq!(
            detect(&paths(dir.path())).unwrap(),
            ClaudeCodeStatus::Connected { capture: false }
        );
    }

    #[test]
    fn detect_lenient_on_malformed_claude_json_reports_not_connected() {
        // A malformed ~/.claude.json can't contain our key → NotConnected (connect() is where the
        // parse error fails loud; status stays a safe read).
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".claude.json"), b"not json {").unwrap();
        assert_eq!(detect(&paths(dir.path())).unwrap(), ClaudeCodeStatus::NotConnected);
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
        // connect writes the SessionEnd capture hook too → the third state reports capture:true.
        assert_eq!(detect(&p).unwrap(), ClaudeCodeStatus::Connected { capture: true });
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
        let ours = sj["hooks"]["SessionStart"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|g| g["hooks"][0]["command"].as_str().is_some_and(|c| c.contains(HOOK_MARKER)))
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
        assert_eq!(std::fs::read(&p.claude_json).unwrap(), b"not json {");
    }

    #[test]
    fn connect_preserves_existing_key_order_in_claude_json() {
        // preserve_order: a foreign key written before OURS keeps its position (no alphabetizing).
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        std::fs::write(&p.claude_json, br#"{"zzz":1,"aaa":2,"mcpServers":{"zebra":{}}}"#).unwrap();
        connect(&p, bin(), sock()).unwrap();
        let text = String::from_utf8(std::fs::read(&p.claude_json).unwrap()).unwrap();
        assert!(text.find("\"zzz\"").unwrap() < text.find("\"aaa\"").unwrap(), "top-level order kept");
        assert!(
            text.find("\"zebra\"").unwrap() < text.find("\"air-memory\"").unwrap(),
            "existing server kept its slot; ours appended after"
        );
    }

    #[test]
    fn connect_single_quotes_a_hostile_binary_path_inertly() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        let evil = Path::new("/t/a'b $(id) `x` air-memory-mcp");
        connect(&p, evil, sock()).unwrap();
        let sj = read(&p.settings_json);
        let cmd = sj["hooks"]["SessionStart"][0]["hooks"][0]["command"].as_str().unwrap();
        assert!(cmd.starts_with('\''), "path is single-quoted: {cmd}");
        assert!(cmd.ends_with("' nudge"), "only ` nudge` is outside the quotes: {cmd}");
        assert!(cmd.contains("'\\''"), "embedded single quote escaped as '\\'': {cmd}");
    }

    #[test]
    fn connect_refuses_non_object_mcp_servers_without_clobber() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        std::fs::write(&p.claude_json, br#"{"mcpServers":[1,2,3]}"#).unwrap();
        assert!(connect(&p, bin(), sock()).unwrap_err().to_string().contains("not an object"));
        assert_eq!(std::fs::read(&p.claude_json).unwrap(), br#"{"mcpServers":[1,2,3]}"#);
    }

    #[test]
    fn connect_preserves_other_hook_types_and_adds_sessionstart() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        std::fs::create_dir(&p.claude_dir).unwrap();
        std::fs::write(
            &p.settings_json,
            br#"{"hooks":{"PreCompact":[{"hooks":[{"type":"command","command":"c"}]}]}}"#,
        )
        .unwrap();
        connect(&p, bin(), sock()).unwrap();
        let sj = read(&p.settings_json);
        assert!(sj["hooks"]["PreCompact"].is_array(), "foreign hook type survives");
        assert_eq!(sj["hooks"]["SessionStart"].as_array().unwrap().len(), 1, "ours added");
    }

    #[test]
    fn connect_malformed_settings_leaves_claude_json_untouched() {
        // M3: valid claude.json + malformed settings.json → NEITHER written; the error is honest.
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        std::fs::write(&p.claude_json, br#"{"mcpServers":{"chrome":{}}}"#).unwrap();
        std::fs::create_dir(&p.claude_dir).unwrap();
        std::fs::write(&p.settings_json, b"not json {").unwrap();

        assert!(connect(&p, bin(), sock()).is_err());
        assert_eq!(
            std::fs::read(&p.claude_json).unwrap(),
            br#"{"mcpServers":{"chrome":{}}}"#,
            "claude.json must be byte-unchanged when settings.json is malformed"
        );
        assert_eq!(detect(&p).unwrap(), ClaudeCodeStatus::NotConnected);
    }

    #[test]
    fn disconnect_removes_only_ours_and_keeps_foreign() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
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
        assert!(read(&p.claude_json)["mcpServers"]["chrome"].is_object());
    }

    #[test]
    fn disconnect_refuses_malformed_without_clobber() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        std::fs::write(&p.claude_json, b"not json {").unwrap();
        assert!(disconnect(&p).is_err());
        assert_eq!(std::fs::read(&p.claude_json).unwrap(), b"not json {");
    }

    #[test]
    fn disconnect_keeps_foreign_hook_that_merely_mentions_the_marker() {
        // A user hook whose command contains "air-memory-mcp" but NOT the nudge subcommand is NOT ours.
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        std::fs::create_dir(&p.claude_dir).unwrap();
        std::fs::write(
            &p.settings_json,
            br#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"bash air-memory-mcp-audit.sh"}]}]}}"#,
        )
        .unwrap();
        disconnect(&p).unwrap();
        let sj = read(&p.settings_json);
        assert_eq!(sj["hooks"]["SessionStart"].as_array().unwrap().len(), 1, "foreign hook survives");
    }

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
        assert_eq!(detect(&p).unwrap(), ClaudeCodeStatus::Connected { capture: true });
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

    // ── B4 (SP3): the SessionEnd `capture-notify` hook, disjoint from the nudge marker. ──

    /// Count OUR groups in a hook array: an inner command with BOTH the marker AND the subcommand.
    fn count_ours(sj: &serde_json::Value, kind: &str, sub: &str) -> usize {
        sj["hooks"][kind]
            .as_array()
            .map(|groups| {
                groups
                    .iter()
                    .filter(|g| {
                        g["hooks"][0]["command"]
                            .as_str()
                            .is_some_and(|c| c.contains(HOOK_MARKER) && c.contains(sub))
                    })
                    .count()
            })
            .unwrap_or(0)
    }

    #[test]
    fn connect_writes_the_session_end_capture_hook() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        connect(&p, bin(), sock()).unwrap();

        let sj = read(&p.settings_json);
        let ends = sj["hooks"]["SessionEnd"].as_array().unwrap();
        assert_eq!(ends.len(), 1, "exactly our capture group");
        assert_eq!(
            ends[0]["hooks"][0]["command"],
            format!("{} capture-notify", sh_single_quote(&bin().to_string_lossy())),
            "SessionEnd runs our capture-notify"
        );
        assert_eq!(ends[0]["hooks"][0]["timeout"], 5);

        // The SessionStart nudge group is STILL present (both hook kinds coexist).
        assert_eq!(count_ours(&sj, "SessionStart", "nudge"), 1, "nudge group untouched");
        assert_eq!(mode(&p.settings_json), 0o600);
    }

    #[test]
    fn disconnect_removes_our_capture_hook_but_keeps_foreign_session_end_groups() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        std::fs::create_dir(&p.claude_dir).unwrap();
        // A FOREIGN SessionEnd group (someone else's hook) is present before we connect.
        std::fs::write(
            &p.settings_json,
            br#"{"hooks":{"SessionEnd":[{"hooks":[{"type":"command","command":"node cleanup.mjs"}]}]}}"#,
        )
        .unwrap();

        connect(&p, bin(), sock()).unwrap(); // adds our capture (+ nudge)
        disconnect(&p).unwrap();

        let sj = read(&p.settings_json);
        let ends = sj["hooks"]["SessionEnd"].as_array().unwrap();
        assert_eq!(ends.len(), 1, "foreign SessionEnd group survives; ours is gone");
        assert_eq!(ends[0]["hooks"][0]["command"], "node cleanup.mjs");
        assert_eq!(count_ours(&sj, "SessionEnd", "capture-notify"), 0, "our capture removed");
        // disconnect removes BOTH our hook kinds — the SessionStart nudge is gone too.
        assert_eq!(count_ours(&sj, "SessionStart", "nudge"), 0, "our nudge removed too");
    }

    #[test]
    fn connect_is_idempotent_and_heals_a_sp2_era_config() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        std::fs::create_dir(&p.claude_dir).unwrap();
        // SP2-era state: the SessionStart nudge group exists, but NO SessionEnd at all.
        let nudge_cmd = format!("{} nudge", sh_single_quote(&bin().to_string_lossy()));
        std::fs::write(
            &p.settings_json,
            serde_json::to_vec(&serde_json::json!({
                "hooks": { "SessionStart": [
                    { "hooks": [{ "type": "command", "command": nudge_cmd, "timeout": 5 }] }
                ]}
            }))
            .unwrap(),
        )
        .unwrap();

        connect(&p, bin(), sock()).unwrap();
        let sj = read(&p.settings_json);
        assert_eq!(count_ours(&sj, "SessionEnd", "capture-notify"), 1, "capture added exactly once");
        assert_eq!(count_ours(&sj, "SessionStart", "nudge"), 1, "nudge healed, not duped");

        connect(&p, bin(), sock()).unwrap(); // idempotent second run
        let sj = read(&p.settings_json);
        assert_eq!(count_ours(&sj, "SessionEnd", "capture-notify"), 1, "still one capture group");
        assert_eq!(count_ours(&sj, "SessionStart", "nudge"), 1, "still one nudge group");
    }

    #[test]
    fn capture_hook_command_is_single_quoted_against_a_hostile_binary_path() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        let evil = Path::new("/t/a'b $(id) `x` air-memory-mcp");
        connect(&p, evil, sock()).unwrap();

        let sj = read(&p.settings_json);
        let cmd = sj["hooks"]["SessionEnd"][0]["hooks"][0]["command"].as_str().unwrap();
        // Exactly the single-quote-escaped path + the static subcommand — no injection surface.
        assert_eq!(cmd, format!("{} capture-notify", sh_single_quote(&evil.to_string_lossy())));
        assert!(cmd.starts_with('\''), "path is single-quoted: {cmd}");
        assert!(cmd.ends_with("' capture-notify"), "only ` capture-notify` is outside quotes: {cmd}");
        assert!(cmd.contains("'\\''"), "embedded single quote escaped as '\\'': {cmd}");
    }

    #[test]
    fn disconnect_marker_does_not_remove_a_foreign_hook_that_merely_mentions_air_memory_mcp() {
        // A foreign SessionEnd hook whose command has "air-memory-mcp" but NOT "capture-notify"
        // (e.g. someone's own "air-memory-mcp status") must SURVIVE — our capture marker needs BOTH.
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        std::fs::create_dir(&p.claude_dir).unwrap();
        std::fs::write(
            &p.settings_json,
            br#"{"hooks":{"SessionEnd":[{"hooks":[{"type":"command","command":"air-memory-mcp status"}]}]}}"#,
        )
        .unwrap();

        disconnect(&p).unwrap();

        let sj = read(&p.settings_json);
        let ends = sj["hooks"]["SessionEnd"].as_array().unwrap();
        assert_eq!(ends.len(), 1, "foreign air-memory-mcp hook without capture-notify survives");
        assert_eq!(ends[0]["hooks"][0]["command"], "air-memory-mcp status");
    }

    // ── B5 (SP3 R2): the capture-aware THIRD detect state + the app-side backfill count. ──

    #[test]
    fn detect_reports_capture_presence_as_the_third_state() {
        // (a) mcpServers entry + SessionEnd capture hook (a full SP3 connect) → capture:true.
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        connect(&p, bin(), sock()).unwrap();
        assert_eq!(detect(&p).unwrap(), ClaudeCodeStatus::Connected { capture: true });

        // (b) mcpServers present, NO capture hook (SP2-era) → capture:false — this is the state Plan C
        // turns into "Re-connect to enable session capture". Build it by hand: our server + only a
        // SessionStart nudge (no SessionEnd), the exact shape an SP2 install left behind.
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        std::fs::write(&p.claude_json, br#"{"mcpServers":{"air-memory":{"type":"stdio"}}}"#).unwrap();
        std::fs::create_dir(&p.claude_dir).unwrap();
        let nudge_cmd = format!("{} nudge", sh_single_quote(&bin().to_string_lossy()));
        std::fs::write(
            &p.settings_json,
            serde_json::to_vec(&serde_json::json!({
                "hooks": { "SessionStart": [
                    { "hooks": [{ "type": "command", "command": nudge_cmd, "timeout": 5 }] }
                ]}
            }))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(detect(&p).unwrap(), ClaudeCodeStatus::Connected { capture: false });

        // (c) neither our server nor a hook → NotConnected.
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        std::fs::write(&p.claude_json, br#"{"mcpServers":{"chrome":{}}}"#).unwrap();
        assert_eq!(detect(&p).unwrap(), ClaudeCodeStatus::NotConnected);

        // (d) no ~/.claude at all → NotFound.
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(detect(&paths(dir.path())).unwrap(), ClaudeCodeStatus::NotFound);
    }

    #[test]
    fn detect_capture_ignores_a_foreign_session_end_hook() {
        // Connected via mcpServers, with a FOREIGN SessionEnd hook (mentions neither our marker nor
        // capture-notify) → capture stays false: the third state keys on OUR capture group only.
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        std::fs::write(&p.claude_json, br#"{"mcpServers":{"air-memory":{"type":"stdio"}}}"#).unwrap();
        std::fs::create_dir(&p.claude_dir).unwrap();
        std::fs::write(
            &p.settings_json,
            br#"{"hooks":{"SessionEnd":[{"hooks":[{"type":"command","command":"node cleanup.mjs"}]}]}}"#,
        )
        .unwrap();
        assert_eq!(detect(&p).unwrap(), ClaudeCodeStatus::Connected { capture: false });
    }

    #[test]
    fn backfill_candidate_count_counts_jsonl_under_projects_root_only() {
        // Fake `~/.claude/projects`: three project dirs holding five `.jsonl` transcripts total, plus a
        // `.txt` (ignored), a nested-deeper dir (NOT recursed — sweeper parity), and a stray root-level
        // `.jsonl` (NOT inside a project dir → ignored). Expect exactly 5.
        let root = tempfile::tempdir().unwrap();
        let r = root.path();
        std::fs::create_dir(r.join("proj-a")).unwrap();
        std::fs::write(r.join("proj-a/s1.jsonl"), b"{}\n").unwrap();
        std::fs::write(r.join("proj-a/s2.jsonl"), b"{}\n").unwrap();
        std::fs::write(r.join("proj-a/notes.txt"), b"ignore me").unwrap(); // wrong extension → ignored
        std::fs::create_dir(r.join("proj-a/deeper")).unwrap();
        std::fs::write(r.join("proj-a/deeper/x.jsonl"), b"{}\n").unwrap(); // nested → NOT counted
        std::fs::create_dir(r.join("proj-b")).unwrap();
        std::fs::write(r.join("proj-b/s3.jsonl"), b"{}\n").unwrap();
        std::fs::create_dir(r.join("proj-c")).unwrap();
        std::fs::write(r.join("proj-c/s4.jsonl"), b"{}\n").unwrap();
        std::fs::write(r.join("proj-c/s5.jsonl"), b"{}\n").unwrap();
        std::fs::write(r.join("loose.jsonl"), b"{}\n").unwrap(); // stray root file → NOT counted

        assert_eq!(backfill_candidate_count(r), 5, "all project-dir .jsonl leaves, nothing else");

        // A missing root is 0, never an error (a not-yet-connected user has no projects dir).
        assert_eq!(backfill_candidate_count(&r.join("does-not-exist")), 0);
    }
}
