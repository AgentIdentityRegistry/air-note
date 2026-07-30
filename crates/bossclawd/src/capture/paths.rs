//! I4 path discipline: the only gate between attacker-influenceable input (session
//! ids + transcript paths from hooks / MCP clients / the sweeper) and the daemon's
//! filesystem access.
//!
//! - [`valid_session_id`] — a strict `[A-Za-z0-9_-]`, ≤128-byte allowlist, checked
//!   before ANY path is built from an id, regardless of the id's source.
//! - [`open_transcript_confined`] — opens a `.jsonl` transcript with ingest-grade
//!   containment: the extension is checked lexically first, the path is proven
//!   lexically under the projects root with no `..`, and the file is opened via the
//!   shared `openat`-fd-chain careful-open (`O_NOFOLLOW` on every component, never
//!   `canonicalize`), rejecting symlinks and non-regular files.

use std::io;
use std::path::Path;

/// Environment override for the Claude Code projects root (tests / dev). When set it
/// wins over the `<home>/.claude/projects` default so tests never touch the real dir.
const ENV_CLAUDE_PROJECTS_ROOT: &str = "AIR_CLAUDE_PROJECTS_ROOT";

/// I4: a session id is path-safe iff non-empty, ≤128 bytes, `[A-Za-z0-9_-]` only
/// (Claude Code ids are UUIDs — this is a superset). Checked before ANY path use,
/// regardless of source (hook stdin, MCP client, sweeper-parsed filename). Rejects
/// path separators, `.`/`..`, NUL, whitespace, and any non-ASCII byte.
pub fn valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Claude Code projects root: the [`ENV_CLAUDE_PROJECTS_ROOT`] override (tests/dev),
/// else `<home>/.claude/projects`. Home is read from `HOME`; if unset it falls back
/// to the current directory with a loud log line (same style as `resolve_data_dir`'s
/// degraded fallback in main.rs), so the returned path still ends with
/// `.claude/projects` and a headless launchd/systemd environment stays diagnosable.
pub fn claude_projects_root() -> std::path::PathBuf {
    if let Some(over) = std::env::var_os(ENV_CLAUDE_PROJECTS_ROOT) {
        return std::path::PathBuf::from(over);
    }
    std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            // Visible in launchd/systemd logs so a degraded environment is diagnosable, not silent.
            eprintln!(
                "bossclawd: HOME is unset; falling back to the current directory for the \
                 Claude projects root (degraded environment — set {ENV_CLAUDE_PROJECTS_ROOT} \
                 to pin it)"
            );
            std::path::PathBuf::from(".")
        })
        .join(".claude")
        .join("projects")
}

/// The owner's `$HOME` rendered in Claude Code's project-slug encoding (every `/` becomes `-`), or
/// `None` when `HOME` is unset or empty.
///
/// DELIBERATELY not sharing [`claude_projects_root`]'s `.`-fallback: that fallback exists so a
/// degraded launchd environment still yields a scannable path, but here a wrong home slug would
/// strip the wrong prefix off a project slug and could ship the owner's username. `None` makes the
/// caller fail CLOSED instead (omit the project), which is the only safe direction for a value that
/// gets sealed into a shareable file.
pub fn home_project_slug() -> Option<String> {
    let home = std::env::var_os("HOME")?;
    let home = home.to_string_lossy();
    if home.is_empty() {
        return None;
    }
    Some(home.trim_end_matches('/').replace('/', "-"))
}

/// Decode a Claude Code project slug into a name that is safe for a stranger to read, given the
/// owner's home slug from [`home_project_slug`]. PURE, so the production shape is unit-testable
/// without touching the environment.
///
/// Claude Code names its project directories by rewriting the session's cwd with `/` → `-`, so the
/// real shape on this machine is `-Users-ahnkwangwook-air-note` — the username is IN the slug, and
/// nothing downstream can split it back out (`bossclaw_core::project_display_name` has no separator
/// to work with, which is exactly how the leak shipped). Stripping the home slug is the one
/// reduction that provably removes the username, because the daemon is the only layer that knows
/// both `$HOME` and the encoding.
///
/// Returns `None` — meaning OMIT the key — unless the slug is provably under home:
/// - a slug that is not `<home_slug>-<something>` (a project outside home, or a different user
///   whose name merely starts with ours) cannot be reduced safely;
/// - a slug equal to the home slug names the home directory itself, which has no project name.
///
/// RESIDUAL, accepted and documented: what survives is the path RELATIVE to home, still
/// dash-joined — `-Users-x-Desktop-QT` yields `Desktop-QT`, not `QT`. The last component is NOT
/// recoverable, because the encoding is ambiguous: `-Users-x-air-note` could be `air-note` or
/// `air/note`. Reducing further would have to guess, and guessing is what caused this bug. The
/// username and everything above home are gone, which is what the owner ruling (`d0c5b14`) and A-N1
/// require.
pub fn receiver_safe_project(slug: &str, home_slug: &str) -> Option<String> {
    if home_slug.is_empty() {
        return None;
    }
    let rest = slug.strip_prefix(home_slug)?;
    // Require a real component boundary: without this, home slug `-Users-ann` would also strip the
    // start of `-Users-annabel-repo` and leak a DIFFERENT user's name as `abel-repo`.
    let rest = rest.strip_prefix('-')?;
    if rest.is_empty() {
        return None;
    }
    Some(rest.to_string())
}

/// Open a transcript with ingest-grade containment (I4). `candidate` is the FULL
/// transcript path, of which `root` must be a lexical prefix — this fn derives the
/// root-relative path itself (contrast `bossclaw_core::ingest::open_beneath_confined`,
/// which takes a root-RELATIVE path). Two lexical policy checks
/// run BEFORE any filesystem syscall — (1) the final component's extension must be
/// `.jsonl`; (2) the path must be lexically under `root` with no `..` component
/// (strictest: a single `..` is a rejection, never a normalization) — and then the
/// validated relative path is handed to the shared `openat`-fd-chain careful-open
/// (`O_NOFOLLOW` on every component, `openat2 RESOLVE_BENEATH|RESOLVE_NO_SYMLINKS`
/// where available, an fstat regular-file reject; NEVER `canonicalize`, which is the
/// check-then-open TOCTOU the ingest module already solved). Returns the open file.
/// Every rejection is an [`io::Error`] whose message names the check that failed.
pub fn open_transcript_confined(root: &Path, candidate: &Path) -> io::Result<std::fs::File> {
    // (1) Extension gate on the final component, lexical, before any syscall.
    if candidate.extension().and_then(|e| e.to_str()) != Some("jsonl") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "transcript path must end in .jsonl",
        ));
    }

    // (2) Lexical containment: derive the root-relative path WITHOUT canonicalizing.
    // `strip_prefix` is a pure component-wise prefix match, so a path with a different
    // prefix (e.g. /etc/passwd) is rejected here, and a path that textually stays
    // under `root` but uses `..` keeps its `..` component for the next check.
    let relative = candidate.strip_prefix(root).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "transcript path is not under the projects root",
        )
    })?;
    if relative
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "transcript path contains a `..` component",
        ));
    }

    open_transcript_confined_impl(root, relative)
}

/// Unix: kernel-enforced containment via the shared `bossclaw-core` careful-open
/// (the same fd chain the ingest walk uses). The relative path is already
/// extension-checked and `..`-free; `open_beneath_confined` re-validates defensively.
#[cfg(unix)]
fn open_transcript_confined_impl(root: &Path, relative: &Path) -> io::Result<std::fs::File> {
    bossclaw_core::ingest::open_beneath_confined(root, relative)
}

/// Non-unix: the confined careful-open needs `openat`/`O_NOFOLLOW`, which this
/// crate only implements on unix (the daemon's whole engine feature set is
/// unix-gated). Mirrors how `ingest` splits its per-OS open — fail loud, never
/// fall back to an unconfined open.
#[cfg(not(unix))]
fn open_transcript_confined_impl(_root: &Path, _relative: &Path) -> io::Result<std::fs::File> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "confined transcript open is only supported on unix",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_session_id_allowlist() {
        assert!(valid_session_id("308cecc6-bd70-4b83-bde0-1a6277cc3d90"));
        assert!(valid_session_id("abc_DEF-123"));
        for bad in ["", "../x", "a/b", "a\0b", ".", "..", "a b", "café", &"x".repeat(129)] {
            assert!(!valid_session_id(bad), "{bad:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn open_transcript_confined_rejects_escapes_and_symlinks() {
        let root = tempfile::tempdir().unwrap(); // stands in for ~/.claude/projects
        let proj = root.path().join("-Users-x-repo");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("ok.jsonl"), b"{}\n").unwrap();
        std::fs::write(root.path().join("outside.txt"), b"secret").unwrap();
        std::os::unix::fs::symlink(root.path().join("outside.txt"), proj.join("evil.jsonl")).unwrap();

        assert!(open_transcript_confined(root.path(), &proj.join("ok.jsonl")).is_ok());
        // symlink leaf:
        assert!(open_transcript_confined(root.path(), &proj.join("evil.jsonl")).is_err());
        // not .jsonl + escape shape:
        assert!(open_transcript_confined(root.path(), &root.path().join("outside.txt")).is_err());
        // absolute path outside the root entirely:
        assert!(open_transcript_confined(root.path(), std::path::Path::new("/etc/passwd")).is_err());
        // a `.jsonl` that PASSES the extension gate but is not lexically under `root`
        // (exercises the containment `strip_prefix` reject, not the extension reject);
        // lexical-only, so the target need not exist — proving no syscall was made:
        assert!(open_transcript_confined(root.path(), std::path::Path::new("/nonexistent-abs/x.jsonl")).is_err());
        let sibling = root.path().parent().unwrap().join("sibling.jsonl");
        assert!(open_transcript_confined(root.path(), &sibling).is_err());
        // traversal that textually stays under root but uses `..`:
        assert!(open_transcript_confined(root.path(), &proj.join("../outside.txt")).is_err());
        // ISOLATE the `..` branch: `../outside.txt` above is also caught by the
        // extension gate, so on its own it wouldn't fail if the `..` check were
        // deleted. This input PASSES the extension gate, its target EXISTS at the
        // resolved location (so the rejection is provably containment, not ENOENT),
        // and the error message must name the `..` component:
        std::fs::write(root.path().join("outside.jsonl"), b"{}\n").unwrap();
        let err = open_transcript_confined(root.path(), &proj.join("../outside.jsonl"))
            .expect_err("a `..` traversal to an existing .jsonl must be refused");
        assert!(err.to_string().contains(".."), "rejection must come from the `..` check, got: {err}");
        // symlinked DIRECTORY component escaping the root:
        let evil_dir = proj.join("evil_dir");
        std::os::unix::fs::symlink(root.path().parent().unwrap(), &evil_dir).unwrap();
        assert!(open_transcript_confined(root.path(), &evil_dir.join("x.jsonl")).is_err());
        // non-regular file (FIFO) named like a valid transcript — must be refused,
        // not read (careful-open's fstat regular-file gate). Created via the POSIX
        // `mkfifo(1)` tool (same convention as the ingest core's fifo test).
        let fifo = proj.join("f.jsonl");
        let status = std::process::Command::new("mkfifo").arg(&fifo).status();
        match status {
            Ok(s) if s.success() => {
                assert!(open_transcript_confined(root.path(), &fifo).is_err(),
                    "a fifo transcript must be refused as non-regular");
            }
            _ => eprintln!("skipping fifo sub-case: `mkfifo` unavailable on this host"),
        }
    }

    #[test]
    fn claude_projects_root_env_override_wins() {
        // Uses a UNIQUE env var (`AIR_CLAUDE_PROJECTS_ROOT`) that no other test in
        // this crate touches, so the set/remove pair is race-free without a serial
        // harness (mirrors the crate's existing env-mutation test convention).
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("AIR_CLAUDE_PROJECTS_ROOT", tmp.path());
        assert_eq!(claude_projects_root(), tmp.path());
        std::env::remove_var("AIR_CLAUDE_PROJECTS_ROOT");
        // Without the override → `<home>/.claude/projects`. HOME is left untouched
        // (never mutated), so we assert only the invariant suffix, not the home root.
        assert!(claude_projects_root().ends_with(".claude/projects"));
    }

    /// The decoder is what stops every exported session from carrying the owner's username.
    /// Driven with the shape this machine actually holds (`~/.claude/projects` contains
    /// `-Users-ahnkwangwook-air-note`), and pure, so no environment is touched.
    #[test]
    fn receiver_safe_project_strips_the_home_slug_and_fails_closed() {
        let home = "-Users-ahnkwangwook";
        // The real production shape → the project, username gone.
        assert_eq!(
            receiver_safe_project("-Users-ahnkwangwook-air-note", home).as_deref(),
            Some("air-note")
        );
        // Nested under home: the RELATIVE remainder survives, dash-joined. Documented residual —
        // the last component is unrecoverable because the encoding is ambiguous.
        assert_eq!(
            receiver_safe_project("-Users-ahnkwangwook-Desktop-QT", home).as_deref(),
            Some("Desktop-QT")
        );
        // Whatever comes back, the username is never in it.
        for slug in ["-Users-ahnkwangwook-air-note", "-Users-ahnkwangwook-Desktop-QT"] {
            let out = receiver_safe_project(slug, home).unwrap();
            assert!(!out.contains("ahnkwangwook"), "username survived in {out:?}");
        }
        // FAIL CLOSED, every direction:
        assert_eq!(receiver_safe_project(home, home), None, "home itself names no project");
        assert_eq!(
            receiver_safe_project("-opt-work-repo", home),
            None,
            "a project outside home cannot be reduced safely"
        );
        assert_eq!(receiver_safe_project("-Users-ahnkwangwook-air-note", ""), None, "no home slug");
        // The boundary check: a DIFFERENT user whose name merely starts with ours must not be
        // silently truncated into `abel-repo`.
        assert_eq!(
            receiver_safe_project("-Users-ahnkwangwookabel-repo", home),
            None,
            "a prefix match without a component boundary is refused"
        );
    }

    /// `home_project_slug` encodes exactly as Claude Code does (`/` → `-`), including the leading
    /// separator, so its output is a real prefix of a project slug.
    #[test]
    fn home_project_slug_matches_claude_codes_encoding() {
        // Pure re-derivation of the same rule, so this asserts the transform without mutating HOME.
        let encode = |home: &str| home.trim_end_matches('/').replace('/', "-");
        assert_eq!(encode("/Users/ahnkwangwook"), "-Users-ahnkwangwook");
        assert_eq!(encode("/Users/ahnkwangwook/"), "-Users-ahnkwangwook", "trailing slash ignored");
        // And the real reader agrees with it for this process's HOME, when HOME is set.
        if let Some(home) = std::env::var_os("HOME") {
            let expected = encode(&home.to_string_lossy());
            assert_eq!(home_project_slug().as_deref(), Some(expected.as_str()));
        }
    }
}
