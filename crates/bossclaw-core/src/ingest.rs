//! M5a ingest pipeline: read-only ingest of user-granted folders into the signed
//! log as recallable, externally-tainted `file_ingested` events.
//!
//! Safety model (spec §6): kernel-enforced containment via an `openat`-fd-chain
//! walk with `O_NOFOLLOW` on every descent + a per-OS careful final open; a
//! never-touch hazard-reduction filter; per-path dedup/version-supersede; and the
//! taint root (`origin: "external"` inside signed content). NO subprocess, NO
//! `unsafe` (rustix encapsulates the syscalls); rich formats (PDF/docx) are M5b.

use std::io::Read;
use std::path::PathBuf;
use std::time::{Duration, Instant};

// `Event` is used by the cross-platform `is_external` classifier, so it stays
// un-gated. `sha2` (content hashing) and `EventLog` (the orchestrator impl) are
// consumed ONLY by the `#[cfg(unix)]` ingest path — gate them so the Windows
// build has no unused imports (the walk, hence the orchestrator, is unix-only).
use crate::event::Event;
#[cfg(unix)]
use crate::log::EventLog;
#[cfg(unix)]
use sha2::{Digest, Sha256};

/// Max bytes read per file. Files larger than this are skipped (recorded), not
/// truncated — a partial body would corrupt content_hash + recall. 10 MiB covers
/// notes/markdown/code; rich/large formats wait for M5b.
const MAX_FILE_BYTES: usize = 10 * 1024 * 1024;
/// Larger byte cap for rich formats routed to the sandboxed parser (M5b). M5a's
/// `MAX_FILE_BYTES` (10 MiB) is sized for text/notes; PDFs/Office docs are
/// routinely larger, so the walk applies this cap to rich extensions instead.
const MAX_RICH_FILE_BYTES: usize = 100 * 1024 * 1024;

/// Extensions routed to the sandboxed `markitdown` parser (lowercase, no dot).
/// Single source of truth for BOTH the parser-aware byte budget AND dispatch.
const RICH_EXTS: &[&str] = &["pdf", "docx", "pptx", "xlsx", "xls", "msg"];

/// True if `ext` (lowercased, no dot) is a rich format handled by the sandboxed parser.
pub(crate) fn is_rich_ext(ext: Option<&str>) -> bool {
    ext.is_some_and(|e| RICH_EXTS.contains(&e))
}

/// Whole-run wall-clock budget (spec §6.2). The walk stops cleanly past this and
/// records a budget skip, so a pathological tree never hangs the engine.
const INGEST_WALL_CLOCK: Duration = Duration::from_secs(300);
/// Max directory nesting depth (defense against pathological/looping trees).
/// The inode-seen set dedups **file** hardlinks; **directory** cycles are bounded
/// here (directories are NOT inserted into the seen set), so this depth cap is the
/// loop guard for directories.
const MAX_WALK_DEPTH: usize = 64;
/// Max entries buffered per directory (bounds an adversarial million-entry fan-out;
/// generous for real note folders). Exceeding it records a loud skip and stops
/// collecting further entries in that directory.
const MAX_DIR_ENTRIES: usize = 100_000;

/// Directory names never descended into (hazard reduction, NOT a containment
/// boundary — the boundary is the grant + informed consent; spec §6.3). Matched
/// **case-insensitively**: the primary platform (macOS/APFS) is case-insensitive,
/// so a case-sensitive filter would let `.SSH` bypass it. Keep these LOWERCASE.
const NEVER_TOUCH_DIRS: &[&str] =
    &[".ssh", ".aws", ".azure", ".gnupg", ".git", ".kube", ".docker", "gcloud"];
/// Exact file names never ingested (LOWERCASE; matched case-insensitively).
const NEVER_TOUCH_FILES: &[&str] = &[
    ".env", ".netrc", ".pgpass", ".git-credentials", "wallet.dat",
    ".npmrc", ".pypirc", ".dockercfg", "known_hosts",
];
/// Glob patterns never ingested. Only two shapes: `*.ext` (suffix) and `prefix*`
/// (prefix). LOWERCASE; matched case-insensitively. Single-sourced + tested.
const NEVER_TOUCH_GLOBS: &[&str] = &[
    "*.key", "*.pem", "*.p12", "*.pfx", "*.gpg", "*.asc", "id_*",
    "*.keychain", "*.kdbx", "*.jks", "*.ppk", "*.mobileconfig", "*.ovpn",
];

/// True if `name_lc` (already lowercased by the caller) matches a `*.ext` (suffix)
/// or `prefix*` (prefix) glob. Patterns are already lowercase.
fn matches_glob(name_lc: &str, pattern: &str) -> bool {
    if let Some(suffix) = pattern.strip_prefix('*') {
        name_lc.ends_with(suffix)
    } else if let Some(prefix) = pattern.strip_suffix('*') {
        name_lc.starts_with(prefix)
    } else {
        name_lc == pattern
    }
}

/// Whether a directory component must never be descended (hazard reduction).
/// Case-insensitive. Also catches the `.config/gh` pair via `rel_dir`.
fn is_never_touch_dir(name: &str, rel_dir: &str) -> bool {
    let name_lc = name.to_lowercase();
    NEVER_TOUCH_DIRS.contains(&name_lc.as_str()) || rel_dir.to_lowercase().ends_with(".config/gh")
}

/// Whether a file component must never be ingested. Case-insensitive.
fn is_never_touch_file(name: &str) -> bool {
    let name_lc = name.to_lowercase();
    NEVER_TOUCH_FILES.contains(&name_lc.as_str()) || NEVER_TOUCH_GLOBS.iter().any(|g| matches_glob(&name_lc, g))
}

/// A sanitized type hint for parser dispatch (spec §4). Carries the lowercased
/// file extension ONLY — never a resolvable path — so a `Parser` can never
/// re-resolve or escape the contained read the orchestrator already performed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PathHint {
    /// Lowercased extension without the dot (e.g. `"md"`), or `None` if absent.
    pub ext: Option<String>,
}

/// Why one file could not be ingested. Per-file and best-effort: these become a
/// `(path, reason)` entry in [`IngestReport`], never a hard failure of the run.
#[derive(Debug)]
pub enum IngestError {
    /// The bytes are not valid UTF-8 (M5a parses text/markdown only; rich
    /// formats wait for M5b's sandboxed parser).
    NonUtf8,
    /// The careful open refused the file (symlink / escape / TOCTOU swap), or a
    /// containment invariant failed. The file is dropped (fail closed).
    Containment(String),
    /// The file exceeded the byte cap (skipped, not truncated).
    TooLarge,
    /// A parser-internal conversion error (reserved for M5b).
    Parse(String),
    /// An OS error while reading the contained handle.
    Io(String),
    /// The sandbox could not be established (feature off, venv missing/invalid,
    /// or the egress probe did not prove network denial). Skipped, not failed —
    /// native ingest continues (fail-closed).
    SandboxUnavailable(String),
    /// The jailed parser exceeded the wall-clock budget and was killed. Failed.
    Timeout,
}

impl std::fmt::Display for IngestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IngestError::NonUtf8 => write!(f, "not valid UTF-8 (rich formats are M5b)"),
            IngestError::Containment(m) => write!(f, "containment refused: {m}"),
            IngestError::TooLarge => write!(f, "exceeds byte cap"),
            IngestError::Parse(m) => write!(f, "parse error: {m}"),
            IngestError::Io(m) => write!(f, "io error: {m}"),
            IngestError::SandboxUnavailable(m) => write!(f, "sandbox unavailable: {m}"),
            IngestError::Timeout => write!(f, "parser timed out"),
        }
    }
}

impl IngestError {
    /// True if this error should be recorded as a `skipped` (benign: file not
    /// ingestable here) rather than a `failed` (a safety/IO problem). Single
    /// source of truth for the `ingest_grant_inner` routing. `TooLarge` is here
    /// for completeness — the read path intercepts it before `convert`.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub(crate) fn is_skip(&self) -> bool {
        matches!(self, IngestError::NonUtf8 | IngestError::TooLarge | IngestError::SandboxUnavailable(_))
    }
}

/// The pluggable converter (spec §4 / D2). Takes already-read, contained bytes +
/// a sanitized hint and returns text. M5a ships [`NativeTextParser`] (UTF-8) and
/// [`MockParser`]; M5b adds a sandboxed-`markitdown` impl behind a feature.
pub trait Parser: Send + Sync {
    /// Convert `raw` bytes to text, or a per-file [`IngestError`].
    fn convert(&self, raw: &[u8], hint: &PathHint) -> Result<String, IngestError>;
    /// Stable id stamped into `file_ingested` provenance (`parser_id`).
    fn parser_id(&self) -> &str;
}

/// The M5a native parser: in-process strict UTF-8 decode. Non-UTF-8 bytes (most
/// binary formats) → [`IngestError::NonUtf8`] (skipped). The `hint` is unused in
/// M5a (any valid-UTF-8 file is text); M5b's parser dispatches on it.
pub struct NativeTextParser;

impl Parser for NativeTextParser {
    fn convert(&self, raw: &[u8], _hint: &PathHint) -> Result<String, IngestError> {
        std::str::from_utf8(raw).map(|s| s.to_string()).map_err(|_| IngestError::NonUtf8)
    }
    fn parser_id(&self) -> &str { "native-text-v1" }
}

/// The feature-off / no-jail stand-in for the rich parser: every rich file is
/// reported `SandboxUnavailable` (→ skipped), so the engine degrades to exactly
/// M5a behavior when `markitdown` is disabled or no venv is present.
pub struct NullRichParser;
impl Parser for NullRichParser {
    fn convert(&self, _raw: &[u8], _hint: &PathHint) -> Result<String, IngestError> {
        Err(IngestError::SandboxUnavailable("markitdown parser not available".into()))
    }
    fn parser_id(&self) -> &str { "null-rich" }
}

/// Selects the parser for a file by its extension hint. Holds the native + rich
/// parsers; the orchestrator calls `pick` then uses the SAME returned parser for
/// both `convert` and `parser_id` (correct provenance).
pub struct ParserRouter {
    native: Box<dyn Parser>,
    rich: Box<dyn Parser>,
}
impl ParserRouter {
    /// Construct a router with explicit native and rich parser implementations.
    pub fn new(native: Box<dyn Parser>, rich: Box<dyn Parser>) -> Self { Self { native, rich } }
    /// M5a-equivalent default: native text + a null rich parser (rich → skip).
    pub fn native_only() -> Self { Self { native: Box::new(NativeTextParser), rich: Box::new(NullRichParser) } }
    /// The chosen parser for `hint`.
    pub fn pick(&self, hint: &PathHint) -> &dyn Parser {
        if is_rich_ext(hint.ext.as_deref()) { self.rich.as_ref() } else { self.native.as_ref() }
    }
}

/// A test double that returns a fixed string regardless of input.
#[cfg(test)]
pub struct MockParser {
    /// The text every `convert` call returns.
    pub output: String,
}

#[cfg(test)]
impl Parser for MockParser {
    fn convert(&self, _raw: &[u8], _hint: &PathHint) -> Result<String, IngestError> {
        Ok(self.output.clone())
    }
    fn parser_id(&self) -> &str { "mock" }
}

/// Best-effort accounting of one ingest run (spec §4). LOUD by design: callers
/// surface `skipped`/`failed` to the user (e.g. "N files matched the never-touch
/// filter"). `superseded` counts files whose content changed since last ingest.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct IngestReport {
    /// New files appended this run.
    pub ingested: usize,
    /// Changed files whose prior version was superseded this run.
    pub superseded: usize,
    /// Unchanged files (same path + same content hash) — no-op.
    pub deduped: usize,
    /// Files intentionally not ingested, with reason (never-touch, non-UTF-8,
    /// oversize, wall-clock budget, …).
    pub skipped: Vec<(PathBuf, String)>,
    /// Files dropped due to a safety/IO error, with reason (containment, io, …).
    pub failed: Vec<(PathBuf, String)>,
}

/// A per-run identity for hardlink/inode dedup. On Unix this is `(dev, ino)`; on
/// Windows (where rustix has no `openat`) it falls back to the canonical path —
/// a documented weaker guarantee (hardlinks are not deduped on Windows).
// The walk's dedup `HashSet` now uses this type, but each variant is constructed
// only on its matching OS (`DevIno` on unix, `Path` on Windows), so the off-target
// variant is dead per build — the allow silences that per-variant case.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum FileIdentity {
    /// Unix `(st_dev, st_ino)`.
    DevIno(u64, u64),
    /// Windows canonical-path fallback.
    Path(PathBuf),
}

/// A file the careful open proved is contained beneath the grant root with no
/// symlink traversal. The orchestrator reads its bytes ONCE (owning identity
/// hashing); the `Parser` never sees this handle or any path.
#[derive(Debug)]
pub(crate) struct ContainedFile {
    file: std::fs::File,
    identity: FileIdentity,
    size: u64,
}

impl ContainedFile {
    // Consumed by the unix walk's dedup + oversize check. On Windows the walk is
    // cfg'd out (no `openat`), so these accessors have no caller there yet — keep
    // the allow only on non-unix until the Windows ingest path lands.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub(crate) fn identity(&self) -> &FileIdentity {
        &self.identity
    }
    #[cfg_attr(not(unix), allow(dead_code))]
    pub(crate) fn size(&self) -> u64 {
        self.size
    }

    /// Read up to `cap` bytes. Returns [`IngestError::TooLarge`] if the file has
    /// more than `cap` bytes (read `cap + 1` and check) — never a truncated body.
    pub(crate) fn read_all_capped(mut self, cap: usize) -> Result<Vec<u8>, IngestError> {
        let mut buf = Vec::with_capacity(self.size.min(cap as u64) as usize);
        // saturating_add: `cap + 1` would wrap if cap == usize::MAX (theoretical —
        // cap is a small constant — but free to guard).
        let read = (&mut self.file)
            .take((cap as u64).saturating_add(1))
            .read_to_end(&mut buf)
            .map_err(|e| IngestError::Io(e.to_string()))?;
        if read > cap {
            return Err(IngestError::TooLarge);
        }
        Ok(buf)
    }

    /// File mtime as RFC 3339 UTC (provenance only).
    pub(crate) fn modified_at_rfc3339(&self) -> String {
        use chrono::{DateTime, Utc};
        self.file.metadata().ok()
            .and_then(|m| m.modified().ok())
            .map(|t| DateTime::<Utc>::from(t).to_rfc3339())
            .unwrap_or_else(|| "1970-01-01T00:00:00+00:00".to_string())
    }
}

// ── Unix: open the final file from a parent dir fd with O_NOFOLLOW. The fd-chain
//    walk (Task 6) reached `dir_fd` via O_NOFOLLOW descents, so a NOFOLLOW open
//    here refuses a final-component symlink AND a dir swapped to a symlink after
//    readdir named it (TOCTOU). On Linux we additionally use openat2 with
//    RESOLVE_BENEATH|RESOLVE_NO_SYMLINKS (spec D3) when the kernel supports it. ──
// Called by the walk (Task 6) for every regular-file leaf it surfaces.
#[cfg(unix)]
pub(crate) fn careful_open_file(
    dir_fd: &std::os::fd::OwnedFd,
    name: &std::ffi::OsStr,
) -> Result<ContainedFile, IngestError> {
    use rustix::fs::{Mode, OFlags};
    use std::os::unix::ffi::OsStrExt;

    #[cfg(target_os = "linux")]
    let owned = {
        use rustix::fs::{openat2, ResolveFlags};
        // O_NONBLOCK so a FIFO swapped in during the post-readdir TOCTOU window
        // returns immediately (a writer-less FIFO read-only open would otherwise
        // block forever); the fstat type-reject below then drops it. Cleared on
        // the accepted regular-file path before any read.
        match openat2(
            dir_fd,
            name.as_bytes(),
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
        ) {
            Ok(fd) => fd,
            // Pre-5.6 kernels lack openat2 → fall back to the NOFOLLOW open, which
            // still refuses a final-component symlink (the chain gave containment).
            Err(rustix::io::Errno::NOSYS) => rustix::fs::openat(
                dir_fd,
                name.as_bytes(),
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|e| IngestError::Containment(e.to_string()))?,
            Err(e) => return Err(IngestError::Containment(e.to_string())),
        }
    };
    // O_NONBLOCK so a FIFO swapped in during the post-readdir TOCTOU window returns
    // immediately instead of blocking on a writer; the fstat type-reject drops it.
    #[cfg(not(target_os = "linux"))]
    let owned = rustix::fs::openat(
        dir_fd,
        name.as_bytes(),
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| IngestError::Containment(e.to_string()))?;

    let st = rustix::fs::fstat(&owned).map_err(|e| IngestError::Io(e.to_string()))?;
    // Reject anything that is not a regular file (fifos, devices, dirs).
    if rustix::fs::FileType::from_raw_mode(st.st_mode) != rustix::fs::FileType::RegularFile {
        return Err(IngestError::Containment("not a regular file".into()));
    }
    // Accepted as a regular file: clear O_NONBLOCK (set above only to dodge the
    // FIFO-open hang) so `read_all_capped` is an ordinary blocking read. Regular
    // files ignore O_NONBLOCK for I/O, but clearing keeps the handle conventional.
    let fl = rustix::fs::fcntl_getfl(&owned).map_err(|e| IngestError::Io(e.to_string()))?;
    rustix::fs::fcntl_setfl(&owned, fl.difference(OFlags::NONBLOCK))
        .map_err(|e| IngestError::Io(e.to_string()))?;
    Ok(ContainedFile {
        file: std::fs::File::from(owned),
        // dev_t/ino_t widths differ per-OS (apple dev_t=i32); the cast can
        // sign-extend, but the value feeds dedup identity only (intra-run
        // consistency), not containment — so it is acceptable.
        identity: FileIdentity::DevIno(st.st_dev as u64, st.st_ino as u64),
        size: st.st_size as u64,
    })
}

// ── Targeted confined open of a KNOWN root-relative path (as opposed to the
//    readdir-driven `walk_grant`, which enumerates). Shared with `bossclawd`'s
//    capture module (transcript open, spec I4): both need the SAME kernel-enforced
//    containment — an `openat` fd chain with `O_NOFOLLOW` on every directory
//    descent + the `careful_open_file` leaf open — WITHOUT ever canonicalizing
//    (canonicalize-then-open is the TOCTOU this whole discipline avoids). ──
/// Open the file at `root`-relative path `relative` with ingest-grade containment:
/// an fd is anchored at `root` (opened `O_NOFOLLOW`, so even a symlinked `root` is
/// refused), each directory component is descended with `openat(O_NOFOLLOW)` (so a
/// symlinked directory component cannot redirect the walk out of `root`), and the
/// final component is opened via [`careful_open_file`] (`O_NOFOLLOW` / `openat2`
/// `RESOLVE_BENEATH|RESOLVE_NO_SYMLINKS` where available, then an fstat regular-file
/// reject). No canonicalization anywhere → no check-then-open TOCTOU.
///
/// `relative` MUST be a normal relative path; this fn re-rejects any `..`/absolute/
/// prefix component defensively (belt-and-suspenders — the caller is expected to have
/// derived + validated it already), but higher-level policy (which extension, which
/// root) lives in the caller. Every rejection is an [`std::io::Error`].
#[cfg(unix)]
pub fn open_beneath_confined(
    root: &std::path::Path,
    relative: &std::path::Path,
) -> std::io::Result<std::fs::File> {
    use rustix::fs::{Mode, OFlags};
    use std::io::{Error, ErrorKind};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Component;

    // Split into directory components + the final (leaf) component. Reject any
    // non-normal component outright (`..`, absolute root, Windows prefix); `.` is a
    // no-op and skipped. This is the strictest reading: a single `..` is a rejection,
    // never a lexical normalization.
    let mut names: Vec<&std::ffi::OsStr> = Vec::new();
    for comp in relative.components() {
        match comp {
            Component::Normal(n) => names.push(n),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    "path component escapes root (`..`/absolute not allowed)",
                ));
            }
        }
    }
    // The last normal component is the file to open; everything before it is a
    // directory to descend. An empty relative path has no file to open.
    let Some(leaf) = names.pop() else {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "empty relative path (no file component under root)",
        ));
    };

    // Anchor at `root` (NOFOLLOW: a symlinked root is refused), then descend each
    // directory component with O_NOFOLLOW. Reassigning `dir_fd` drops (closes) the
    // parent fd as we go, so no fd is leaked down the chain.
    let mut dir_fd = rustix::fs::openat(
        rustix::fs::CWD,
        root.as_os_str().as_bytes(),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| Error::other(format!("open root failed: {e}")))?;
    for name in &names {
        dir_fd = rustix::fs::openat(
            &dir_fd,
            name.as_bytes(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|e| {
            Error::new(ErrorKind::PermissionDenied, format!("containment refused at directory component: {e}"))
        })?;
    }

    // Leaf: reuse the ingest careful-open (O_NOFOLLOW/openat2 + fstat regular-file
    // reject), so a symlinked, FIFO, or otherwise non-regular leaf is refused.
    let contained = careful_open_file(&dir_fd, leaf)
        .map_err(|e| Error::new(ErrorKind::PermissionDenied, e.to_string()))?;
    Ok(contained.file)
}

// ── Windows: no openat. Canonicalize, assert containment under the grant root,
//    and reject reparse points (symlinks/junctions). Final-component-strong;
//    the intermediate-dir swap race is a documented residual (spec §6.1, D3). ──
// Wired into the walk in Task 6 (the Windows branch of the per-OS open).
#[allow(dead_code)]
#[cfg(windows)]
pub(crate) fn careful_open_windows(
    grant_root: &std::path::Path,
    candidate: &std::path::Path,
) -> Result<ContainedFile, IngestError> {
    let meta = std::fs::symlink_metadata(candidate).map_err(|e| IngestError::Io(e.to_string()))?;
    if meta.file_type().is_symlink() {
        return Err(IngestError::Containment("reparse point / symlink rejected".into()));
    }
    let canonical = std::fs::canonicalize(candidate).map_err(|e| IngestError::Io(e.to_string()))?;
    let root_canonical =
        std::fs::canonicalize(grant_root).map_err(|e| IngestError::Io(e.to_string()))?;
    if !canonical.starts_with(&root_canonical) {
        return Err(IngestError::Containment("escapes grant root".into()));
    }
    // Open, then re-check type on the OPENED handle (NOT the pre-open
    // `symlink_metadata`) so a file→dir swap between canonicalize and open cannot
    // slip a non-regular target through.
    let file = std::fs::File::open(&canonical).map_err(|e| IngestError::Io(e.to_string()))?;
    let opened = file.metadata().map_err(|e| IngestError::Io(e.to_string()))?;
    if !opened.file_type().is_file() {
        return Err(IngestError::Containment("not a regular file".into()));
    }
    Ok(ContainedFile {
        file,
        identity: FileIdentity::Path(canonical),
        size: opened.len(),
    })
}

/// Tunable safety budgets for the walk. `Default` uses the production consts;
/// tests construct tiny limits to exercise the cap/budget skip paths cheaply.
#[cfg(unix)]
#[derive(Debug, Clone)]
pub(crate) struct WalkLimits {
    pub(crate) max_file_bytes: usize,
    pub(crate) max_rich_file_bytes: usize,
    pub(crate) wall_clock: Duration,
    pub(crate) max_walk_depth: usize,
    pub(crate) max_dir_entries: usize,
}

#[cfg(unix)]
impl Default for WalkLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: MAX_FILE_BYTES,
            max_rich_file_bytes: MAX_RICH_FILE_BYTES,
            wall_clock: INGEST_WALL_CLOCK,
            max_walk_depth: MAX_WALK_DEPTH,
            max_dir_entries: MAX_DIR_ENTRIES,
        }
    }
}

/// A file the walk surfaced for ingest: the contained handle + its path + a
/// sanitized hint. `canonical_path` is `grant_root` (already canonicalized) joined
/// with the walk-relative components — safe to treat as canonical because the walk
/// admitted no symlink and no `..`, so it equals `realpath` WITHOUT re-resolving.
#[cfg(unix)]
pub(crate) struct WalkedFile {
    pub(crate) file: ContainedFile,
    pub(crate) canonical_path: PathBuf,
    pub(crate) hint: PathHint,
}

/// Recursively walk `grant_root` (already canonicalized), invoking `sink` for each
/// ingestable regular file. No-symlink-follow, never-touch-filtered, depth- and
/// wall-clock-bounded, inode-deduped within the run. `report.skipped` accumulates
/// never-touch / oversize / budget skips. Returns early (Ok) when the wall-clock
/// budget is hit (a `<budget>` skip is recorded).
#[cfg(unix)]
pub(crate) fn walk_grant(
    grant_root: &std::path::Path,
    limits: &WalkLimits,
    started: Instant,
    seen: &mut std::collections::HashSet<FileIdentity>,
    report: &mut IngestReport,
    mut sink: impl FnMut(WalkedFile) -> Result<(), crate::error::BossclawError>,
) -> Result<(), crate::error::BossclawError> {
    use rustix::fs::{Mode, OFlags};
    use std::os::unix::ffi::OsStrExt;

    let root_fd = rustix::fs::openat(
        rustix::fs::CWD, grant_root.as_os_str().as_bytes(),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty(),
    ).map_err(|e| crate::error::BossclawError::Io(std::io::Error::other(e.to_string())))?;

    // Explicit stack of (dir_fd, rel_dir, depth) so recursion can't blow the
    // native stack; each dir_fd was opened from its parent with O_NOFOLLOW.
    let mut stack: Vec<(std::os::fd::OwnedFd, String, usize)> = vec![(root_fd, String::new(), 0)];

    while let Some((dir_fd, rel_dir, depth)) = stack.pop() {
        if started.elapsed() > limits.wall_clock {
            report.skipped.push((grant_root.join(&rel_dir), "wall-clock budget exceeded".into()));
            return Ok(());
        }
        // Read entries from the dir fd. `Dir` borrows the fd; collect names first.
        let dir = rustix::fs::Dir::read_from(&dir_fd)
            .map_err(|e| crate::error::BossclawError::Io(std::io::Error::other(e.to_string())))?;
        let mut entries: Vec<std::ffi::OsString> = Vec::new();
        for entry in dir {
            let entry = entry.map_err(|e| crate::error::BossclawError::Io(std::io::Error::other(e.to_string())))?;
            let name_bytes = entry.file_name().to_bytes();
            if name_bytes == b"." || name_bytes == b".." {
                continue;
            }
            // Bound an adversarial million-entry fan-out: stop buffering past the cap
            // (the already-collected entries are still processed) and record a loud skip.
            if entries.len() >= limits.max_dir_entries {
                report.skipped.push((grant_root.join(&rel_dir), "directory entry cap exceeded".into()));
                break;
            }
            entries.push(std::ffi::OsStr::from_bytes(name_bytes).to_os_string());
        }

        for (i, name_os) in entries.into_iter().enumerate() {
            // Re-check the wall-clock budget WITHIN the dir: a single huge directory
            // syscall-storms (statat + careful_open per entry), so the per-dir check
            // at the top of the loop is not enough. The mask amortizes the clock read
            // to every 16384 entries (cheap given the per-dir entry cap above).
            if i & 0x3FFF == 0 && started.elapsed() > limits.wall_clock {
                report.skipped.push((grant_root.join(&rel_dir), "wall-clock budget exceeded".into()));
                return Ok(());
            }
            let name = name_os.to_string_lossy().to_string();
            let rel_child = if rel_dir.is_empty() { name.clone() } else { format!("{rel_dir}/{name}") };

            // statat with SYMLINK_NOFOLLOW: classify WITHOUT following symlinks.
            let st = match rustix::fs::statat(&dir_fd, name_os.as_bytes(), rustix::fs::AtFlags::SYMLINK_NOFOLLOW) {
                Ok(st) => st,
                Err(e) => { report.skipped.push((grant_root.join(&rel_child), format!("stat failed: {e}"))); continue; }
            };
            let ftype = rustix::fs::FileType::from_raw_mode(st.st_mode);

            if ftype == rustix::fs::FileType::Symlink {
                // No-symlink-follow: silently skip (not an error; expected).
                continue;
            }
            if ftype == rustix::fs::FileType::Directory {
                if is_never_touch_dir(&name, &rel_child) {
                    report.skipped.push((grant_root.join(&rel_child), "never-touch dir".into()));
                    continue;
                }
                if depth + 1 > limits.max_walk_depth {
                    report.skipped.push((grant_root.join(&rel_child), "max depth exceeded".into()));
                    continue;
                }
                let child_fd = match rustix::fs::openat(
                    &dir_fd, name_os.as_bytes(),
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty(),
                ) {
                    Ok(fd) => fd,
                    Err(e) => { report.skipped.push((grant_root.join(&rel_child), format!("dir open refused: {e}"))); continue; }
                };
                stack.push((child_fd, rel_child, depth + 1));
                continue;
            }
            if ftype != rustix::fs::FileType::RegularFile {
                continue; // fifo, socket, device — skip silently
            }

            // Regular file.
            if is_never_touch_file(&name) {
                report.skipped.push((grant_root.join(&rel_child), "never-touch file".into()));
                continue;
            }
            let cf = match careful_open_file(&dir_fd, &name_os) {
                Ok(cf) => cf,
                Err(IngestError::TooLarge) => { report.skipped.push((grant_root.join(&rel_child), "oversize".into())); continue; }
                Err(e) => { report.failed.push((grant_root.join(&rel_child), e.to_string())); continue; }
            };
            let ext = std::path::Path::new(&name).extension().map(|e| e.to_string_lossy().to_lowercase());
            let cap = if is_rich_ext(ext.as_deref()) { limits.max_rich_file_bytes } else { limits.max_file_bytes };
            if cf.size() > cap as u64 {
                report.skipped.push((grant_root.join(&rel_child), "oversize".into()));
                continue;
            }
            if !seen.insert(cf.identity().clone()) {
                continue; // same inode already ingested this run (hardlink / overlap)
            }
            let hint = PathHint { ext };
            sink(WalkedFile { file: cf, canonical_path: grant_root.join(&rel_child), hint })?;
        }
    }
    Ok(())
}

#[cfg(unix)]
impl EventLog {
    /// Ingest every ACTIVE granted folder (spec §4). Runs the safe pipeline per
    /// grant, sharing ONE inode-seen set + ONE wall-clock budget across the whole
    /// run, then refreshes the in-memory recall index so new files are immediately
    /// recallable. Each `ingest_grant_inner` already refreshes the graph projection
    /// (files/grants), so only the ANN/FTS index needs a final rebuild here.
    /// Returns the aggregate [`IngestReport`].
    ///
    /// Perf note (future): `ingest_grant_inner` rebuilds the graph per grant; for
    /// many grants on a large log a `defer_rebuild` flag could collapse those into
    /// one post-loop rebuild. Out of scope for M5a (grant counts are small).
    pub fn ingest_all(
        &self,
        router: &ParserRouter,
        embedder: &dyn crate::embed::Embedder,
    ) -> Result<IngestReport, crate::error::BossclawError> {
        let started = Instant::now();
        let mut report = IngestReport::default();
        let mut seen = std::collections::HashSet::new();
        let active: Vec<String> = self.grants()?.into_iter().filter(|g| !g.revoked).map(|g| g.canonical_root).collect();
        for root in active {
            self.ingest_grant_inner(std::path::Path::new(&root), router, embedder, started, &mut seen, &mut report)?;
        }
        // Make the newly-appended files recallable (graph projection already current
        // from each ingest_grant_inner's internal rebuild).
        self.rebuild_indexes(embedder)?;
        Ok(report)
    }
}

/// Build the signed content of a `file_ingested` event (D4). `text` is top-level
/// so `embeddable_text` finds it; `origin` is the taint stamp; everything is
/// inside the signed bytes (JCS canonical + byte-identical rebuild).
#[cfg(unix)]
fn file_ingested_content(
    text: &str,
    canonical_path: &str,
    raw: &[u8],
    grant_root: &str,
    parser_id: &str,
    modified_at: &str,
) -> serde_json::Value {
    let content_hash = hex::encode(Sha256::digest(raw));
    let text_hash = hex::encode(Sha256::digest(text.as_bytes()));
    serde_json::json!({
        "text": text,
        "origin": crate::graph::EXTERNAL_ORIGIN,
        "provenance": {
            "canonical_path": canonical_path,
            "content_hash": content_hash,
            "text_hash": text_hash,
            "size_bytes": raw.len(),
            "modified_at": modified_at,
            "parser_id": parser_id,
            "grant_root": grant_root,
        }
    })
}

/// True iff `event` is externally-tainted (M5a, D5). The classifier the M6
/// actuator's fail-closed lineage walk will consume; here it is the taint root +
/// a tested predicate. Reads the single-sourced `EXTERNAL_ORIGIN` stamp.
pub fn is_external(event: &Event) -> bool {
    event.content.get("origin").and_then(|v| v.as_str()) == Some(crate::graph::EXTERNAL_ORIGIN)
}

#[cfg(unix)]
impl EventLog {
    /// Ingest one already-granted, canonicalized folder `grant_root`. Walks it
    /// safely, parses each file, applies the per-path dedup/supersede decision,
    /// appending ground-truth `file_ingested` events (D4). Best-effort: per-file
    /// problems land in the returned [`IngestReport`], not as errors. `seen` is
    /// the run-wide inode-dedup set (shared across grants by `ingest_all`).
    /// Re-checks the grant is still active before EVERY append so a concurrent
    /// `revoke_grant` stops further writes (spec §7).
    pub(crate) fn ingest_grant_inner(
        &self,
        grant_root: &std::path::Path,
        router: &ParserRouter,
        embedder: &dyn crate::embed::Embedder,
        started: Instant,
        seen: &mut std::collections::HashSet<FileIdentity>,
        report: &mut IngestReport,
    ) -> Result<(), crate::error::BossclawError> {
        let grant_root_str = grant_root.to_string_lossy().to_string();
        let limits = WalkLimits::default();
        // Collect walked files first (the walk borrows dir fds; appends happen after).
        let mut walked: Vec<WalkedFile> = Vec::new();
        walk_grant(grant_root, &limits, started, seen, report, |wf| { walked.push(wf); Ok(()) })?;

        for wf in walked {
            if started.elapsed() > limits.wall_clock {
                report.skipped.push((wf.canonical_path, "wall-clock budget exceeded".into()));
                continue;
            }
            // Re-check the grant is active before doing work (revoke mid-ingest).
            let still_active = self.grants()?.iter().any(|g| g.canonical_root == grant_root_str && !g.revoked);
            if !still_active {
                report.skipped.push((wf.canonical_path, "grant revoked mid-ingest".into()));
                continue;
            }

            let canonical_path = wf.canonical_path.to_string_lossy().to_string();
            let modified_at = file_mtime_rfc3339(&wf.file);
            let read_cap = if is_rich_ext(wf.hint.ext.as_deref()) { limits.max_rich_file_bytes } else { limits.max_file_bytes };
            let raw = match wf.file.read_all_capped(read_cap) {
                Ok(b) => b,
                Err(IngestError::TooLarge) => { report.skipped.push((wf.canonical_path, "oversize".into())); continue; }
                Err(e) => { report.failed.push((wf.canonical_path, e.to_string())); continue; }
            };
            let parser = router.pick(&wf.hint);
            let text = match parser.convert(&raw, &wf.hint) {
                Ok(t) => t,
                Err(e) if e.is_skip() => { report.skipped.push((wf.canonical_path, e.to_string())); continue; }
                Err(e) => { report.failed.push((wf.canonical_path, e.to_string())); continue; }
            };
            // Empty / whitespace-only extraction (e.g. a scanned image-only PDF)
            // is NOT a signed empty event — record a skip instead.
            if text.trim().is_empty() {
                report.skipped.push((wf.canonical_path, "no extractable text".into()));
                continue;
            }
            let content = file_ingested_content(&text, &canonical_path, &raw, &grant_root_str, parser.parser_id(), &modified_at);
            let new_hash = content["provenance"]["content_hash"].as_str().unwrap().to_string();

            // ── Dedup / supersede decision (spec §4 table), keyed on canonical_path ──
            match self.current_file_for_path(&canonical_path)? {
                Some(prev) if prev.content_hash == new_hash => {
                    report.deduped += 1; // same path + same bytes → no-op
                }
                Some(prev) => {
                    // Changed bytes → atomic ground-truth supersede + new file_ingested.
                    let supersede_ev = ground_truth_supersede(&prev.file_event_id, self.signer_did());
                    let file_ev = ground_truth_file_ingested(content, self.signer_did());
                    let (_s, new_id) = self.append_pair(supersede_ev, file_ev)?;
                    self.derive_vector_for(embedder, &new_id)?;
                    report.superseded += 1;
                }
                None => {
                    let file_ev = ground_truth_file_ingested(content, self.signer_did());
                    let new_id = self.append(file_ev)?;
                    self.derive_vector_for(embedder, &new_id)?;
                    report.ingested += 1;
                }
            }
        }
        // Refresh the `files`/`grants` projection so the per-path dedup/supersede
        // decision (`current_file_for_path`) is correct on the NEXT run — the same
        // append→rebuild lifecycle `add_grant`/`revoke_grant` use. NB: this does NOT
        // rebuild the in-memory ANN/FTS recall indexes; callers (`ingest_all`, tests)
        // still run `rebuild_indexes` before recall (contract note).
        self.rebuild_graph()?;
        Ok(())
    }
}

/// A ground-truth `file_ingested` Event (model_meta: None → plain append/append_pair).
#[cfg(unix)]
fn ground_truth_file_ingested(content: serde_json::Value, signer_did: String) -> Event {
    Event {
        id: String::new(), ts: String::new(), valid_time: None,
        event_type: crate::graph::FILE_INGESTED_EVENT_TYPE.to_string(),
        content, model_meta: None, prev_hash: String::new(), hash: None,
        signed_by_did: signer_did, signature: None,
    }
}

/// A ground-truth `supersede` Event retiring `prior_id` (reuses SUPERSEDE_EVENT_TYPE
/// but with model_meta: None — cross-fold safety holds via disjoint event ids).
#[cfg(unix)]
fn ground_truth_supersede(prior_id: &str, signer_did: String) -> Event {
    Event {
        id: String::new(), ts: String::new(), valid_time: None,
        event_type: crate::graph::SUPERSEDE_EVENT_TYPE.to_string(),
        content: serde_json::json!({ "supersedes": prior_id }),
        model_meta: None, prev_hash: String::new(), hash: None,
        signed_by_did: signer_did, signature: None,
    }
}

/// File mtime as RFC 3339 (provenance only; NEVER a dedup/identity key).
#[cfg(unix)]
fn file_mtime_rfc3339(cf: &ContainedFile) -> String {
    cf.modified_at_rfc3339()
}

#[cfg(test)]
mod filter_tests {
    use super::*;

    #[test]
    fn never_touch_files_and_globs() {
        assert!(is_never_touch_file(".env"));
        assert!(is_never_touch_file("server.key"));
        assert!(is_never_touch_file("id_rsa"));
        assert!(is_never_touch_file("vault.kdbx"));
        assert!(is_never_touch_file("cert.p12"));
        assert!(is_never_touch_file("known_hosts"));
        // Case-insensitive (macOS/APFS): uppercase variants must also match.
        assert!(is_never_touch_file(".ENV"));
        assert!(is_never_touch_file("Server.PEM"));
        assert!(is_never_touch_file("ID_RSA"));
        assert!(!is_never_touch_file("notes.md"));
        assert!(!is_never_touch_file("readme.txt"));
    }

    #[test]
    fn never_touch_dirs_including_config_gh() {
        assert!(is_never_touch_dir(".ssh", "project/.ssh"));
        assert!(is_never_touch_dir(".SSH", "project/.SSH")); // case-insensitive (macOS)
        assert!(is_never_touch_dir(".git", "project/.git"));
        assert!(is_never_touch_dir("gh", "home/.config/gh"));
        assert!(!is_never_touch_dir("src", "project/src"));
    }

    #[test]
    fn glob_shapes() {
        assert!(matches_glob("a.pem", "*.pem"));
        assert!(matches_glob("id_ed25519", "id_*"));
        assert!(!matches_glob("pem.txt", "*.pem"));
    }
}

#[cfg(all(test, unix))]
mod walk_tests {
    use super::*;

    fn collect(root: &std::path::Path) -> (Vec<String>, IngestReport) {
        let mut report = IngestReport::default();
        let mut seen = std::collections::HashSet::new();
        let mut names = Vec::new();
        walk_grant(root, &WalkLimits::default(), Instant::now(), &mut seen, &mut report, |wf| {
            names.push(wf.canonical_path.file_name().unwrap().to_string_lossy().to_string());
            Ok(())
        }).unwrap();
        names.sort();
        (names, report)
    }

    fn collect_with(root: &std::path::Path, started: Instant, limits: &WalkLimits) -> (Vec<String>, IngestReport) {
        let mut report = IngestReport::default();
        let mut seen = std::collections::HashSet::new();
        let mut names = Vec::new();
        walk_grant(root, limits, started, &mut seen, &mut report, |wf| {
            names.push(wf.canonical_path.file_name().unwrap().to_string_lossy().to_string());
            Ok(())
        }).unwrap();
        names.sort();
        (names, report)
    }

    #[test]
    fn walk_skips_never_touch_and_symlinks_finds_regular_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("a.md"), b"a").unwrap();
        std::fs::write(root.join(".env"), b"SECRET=1").unwrap();
        std::fs::write(root.join("k.pem"), b"key").unwrap();
        std::fs::create_dir(root.join(".ssh")).unwrap();
        std::fs::write(root.join(".ssh").join("id_rsa"), b"key").unwrap();
        std::os::unix::fs::symlink(root.join("a.md"), root.join("link.md")).unwrap();

        let (names, report) = collect(root);
        assert_eq!(names, vec!["a.md".to_string()], "only the plain file is surfaced");
        assert!(report.skipped.iter().any(|(_, r)| r == "never-touch file"));
        assert!(report.skipped.iter().any(|(_, r)| r == "never-touch dir"));
    }

    #[test]
    fn walk_dedups_hardlinks_within_a_run() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("orig.txt"), b"data").unwrap();
        std::fs::hard_link(root.join("orig.txt"), root.join("dup.txt")).unwrap();
        let (names, _r) = collect(root);
        assert_eq!(names.len(), 1, "a hardlinked inode is surfaced once per run");
    }

    // ── DoS-budget regression tests ──────────────────────────────────────────

    /// Depth cap: root=0, a=1, b=2 (descended, files inside surface),
    /// c=3 would require depth+1=3 > max_walk_depth=2 → skipped.
    /// Verifies that `max_walk_depth` cuts the walk at the right boundary.
    #[test]
    fn dos_depth_cap_skips_dirs_beyond_limit() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // root/a/b/mid.md  → depth of directory b is 2; file surfaces.
        // root/a/b/c/deep.md → opening c requires depth+1=3 > 2 → c is skipped.
        std::fs::create_dir_all(root.join("a/b/c")).unwrap();
        std::fs::write(root.join("a/b/mid.md"), b"mid").unwrap();
        std::fs::write(root.join("a/b/c/deep.md"), b"deep").unwrap();

        let limits = WalkLimits { max_walk_depth: 2, ..WalkLimits::default() };
        let (names, report) = collect_with(root, Instant::now(), &limits);

        assert!(names.contains(&"mid.md".to_string()), "mid.md (depth 2) must surface");
        assert!(!names.contains(&"deep.md".to_string()), "deep.md (depth 3) must NOT surface");
        assert!(
            report.skipped.iter().any(|(_, r)| r == "max depth exceeded"),
            "skipped must record 'max depth exceeded'; got: {:?}", report.skipped
        );
    }

    /// Entry cap: with `max_dir_entries: 3` and 5 files in root, exactly 3 are
    /// surfaced and the report records `"directory entry cap exceeded"`.
    #[test]
    fn dos_entry_cap_limits_files_per_directory() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for i in 0..5u8 {
            std::fs::write(root.join(format!("f{i}.txt")), [i]).unwrap();
        }

        let limits = WalkLimits { max_dir_entries: 3, ..WalkLimits::default() };
        let (names, report) = collect_with(root, Instant::now(), &limits);

        assert_eq!(names.len(), 3, "entry cap of 3 must surface exactly 3 files; got {:?}", names);
        assert!(
            report.skipped.iter().any(|(_, r)| r == "directory entry cap exceeded"),
            "skipped must record 'directory entry cap exceeded'; got: {:?}", report.skipped
        );
    }

    /// Wall-clock budget: `Duration::ZERO` expires before the first entry is
    /// processed (the per-dir check fires on every directory pop), so no files
    /// surface and the skip reason is `"wall-clock budget exceeded"`.
    #[test]
    fn dos_wall_clock_budget_stops_walk_immediately() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("one.txt"), b"x").unwrap();

        let limits = WalkLimits { wall_clock: Duration::ZERO, ..WalkLimits::default() };
        // started = Instant::now() ensures elapsed() > ZERO immediately.
        let (names, report) = collect_with(root, Instant::now(), &limits);

        assert!(names.is_empty(), "zero budget must surface no files; got {:?}", names);
        assert!(
            report.skipped.iter().any(|(_, r)| r == "wall-clock budget exceeded"),
            "skipped must record 'wall-clock budget exceeded'; got: {:?}", report.skipped
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_rich_ext_matches_only_the_sandboxed_set() {
        for e in ["pdf", "docx", "pptx", "xlsx", "xls", "msg"] {
            assert!(is_rich_ext(Some(e)), "{e}");
        }
        for e in ["txt", "md", "csv", "json", "html", "rs"] {
            assert!(!is_rich_ext(Some(e)), "{e}");
        }
        assert!(!is_rich_ext(None));
    }

    #[test]
    fn native_parser_reads_utf8_text() {
        let p = NativeTextParser;
        let out = p.convert("# Title\nbody".as_bytes(), &PathHint { ext: Some("md".into()) }).unwrap();
        assert_eq!(out, "# Title\nbody");
        assert_eq!(p.parser_id(), "native-text-v1");
    }

    #[test]
    fn native_parser_rejects_non_utf8_as_nonutf8() {
        let p = NativeTextParser;
        // 0xFF 0xFE is not valid UTF-8.
        let err = p.convert(&[0xFF, 0xFE, 0x00], &PathHint::default()).unwrap_err();
        assert!(matches!(err, IngestError::NonUtf8));
    }

    #[test]
    fn error_skip_classification_is_correct() {
        assert_eq!(IngestError::SandboxUnavailable("x".into()).to_string(), "sandbox unavailable: x");
        assert_eq!(IngestError::Timeout.to_string(), "parser timed out");
        assert!(IngestError::SandboxUnavailable("x".into()).is_skip());
        assert!(IngestError::NonUtf8.is_skip());
        assert!(!IngestError::Timeout.is_skip());
        assert!(!IngestError::Parse("y".into()).is_skip());
    }

    #[test]
    fn path_hint_carries_no_path() {
        // Compile-time guarantee: PathHint has exactly one field, the extension.
        let h = PathHint { ext: Some("txt".into()) };
        assert_eq!(h.ext.as_deref(), Some("txt"));
    }

    #[test]
    fn taint_classifier_marks_external_origin() {
        // A file_ingested event is classified external; a memory is not.
        let file_ev = Event {
            id: "f".into(), ts: String::new(), valid_time: None,
            event_type: crate::graph::FILE_INGESTED_EVENT_TYPE.into(),
            content: serde_json::json!({ "text": "x", "origin": crate::graph::EXTERNAL_ORIGIN }),
            model_meta: None, prev_hash: String::new(), hash: None,
            signed_by_did: "did:wba:AIR-TEST".into(), signature: None,
        };
        assert!(is_external(&file_ev));
        let mem = Event {
            id: "m".into(), ts: String::new(), valid_time: None,
            event_type: crate::graph::MEMORY_EVENT_TYPE.into(),
            content: serde_json::json!({ "text": "x" }),
            model_meta: None, prev_hash: String::new(), hash: None,
            signed_by_did: "did:wba:AIR-TEST".into(), signature: None,
        };
        assert!(!is_external(&mem), "a memory carries no external taint");
    }

    #[test]
    fn ground_truth_supersede_is_not_external() {
        // The ground-truth supersede the orchestrator emits is NOT externally
        // tainted — `origin` is the single source of truth for is_external.
        let sup = Event {
            id: "s".into(), ts: String::new(), valid_time: None,
            event_type: crate::graph::SUPERSEDE_EVENT_TYPE.into(),
            content: serde_json::json!({ "supersedes": "f1" }),
            model_meta: None, prev_hash: String::new(), hash: None,
            signed_by_did: "did:wba:AIR-TEST".into(), signature: None,
        };
        assert!(!is_external(&sup), "a file supersede is ground-truth control, not external content");
    }

    #[test]
    fn router_dispatches_by_extension() {
        let r = ParserRouter::new(Box::new(MockParser{output:"N".into()}), Box::new(MockParser{output:"R".into()}));
        let pdf = PathHint{ext:Some("pdf".into())}; let txt = PathHint{ext:Some("txt".into())};
        assert_eq!(r.pick(&pdf).convert(b"",&pdf).unwrap(), "R");
        assert_eq!(r.pick(&txt).convert(b"",&txt).unwrap(), "N");
    }

    #[test]
    fn native_only_router_skips_rich_with_sandbox_unavailable() {
        let r = ParserRouter::native_only(); let pdf = PathHint{ext:Some("pdf".into())};
        assert!(matches!(r.pick(&pdf).convert(b"%PDF",&pdf).unwrap_err(), IngestError::SandboxUnavailable(_)));
    }
}

#[cfg(all(test, unix))]
mod containment_tests {
    use super::*;
    use rustix::fs::{Mode, OFlags};
    use std::os::unix::ffi::OsStrExt;

    // Open a directory as a NOFOLLOW dir fd (what the walk does).
    fn open_dir(path: &std::path::Path) -> std::os::fd::OwnedFd {
        rustix::fs::openat(
            rustix::fs::CWD,
            path.as_os_str().as_bytes(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .unwrap()
    }

    #[test]
    fn careful_open_reads_a_contained_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"hello").unwrap();
        let dfd = open_dir(dir.path());
        let cf = careful_open_file(&dfd, std::ffi::OsStr::new("a.txt")).unwrap();
        assert_eq!(cf.read_all_capped(1024).unwrap(), b"hello");
    }

    #[test]
    fn careful_open_refuses_a_symlink_final_component() {
        let dir = tempfile::tempdir().unwrap();
        let secret = dir.path().join("secret");
        std::fs::write(&secret, b"TOP SECRET").unwrap();
        std::os::unix::fs::symlink(&secret, dir.path().join("link")).unwrap();
        let dfd = open_dir(dir.path());
        let err = careful_open_file(&dfd, std::ffi::OsStr::new("link")).unwrap_err();
        assert!(matches!(err, IngestError::Containment(_)), "a symlink must be refused, got {err:?}");
    }

    // The TOCTOU swap: a name resolves to a real file at readdir time, then is
    // swapped to a symlink BEFORE the open. NOFOLLOW (and openat2 RESOLVE_NO_SYMLINKS)
    // must refuse — proving the open is hardened, which a static-symlink test does not.
    #[test]
    fn careful_open_refuses_a_post_readdir_symlink_swap() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real.txt");
        std::fs::write(&target, b"ok").unwrap();
        let outside = dir.path().join("outside_secret");
        std::fs::write(&outside, b"SECRET").unwrap();
        let dfd = open_dir(dir.path());
        // Simulate the race: between "the walk saw real.txt" and the open, an
        // attacker replaces real.txt with a symlink pointing outside.
        std::fs::remove_file(&target).unwrap();
        std::os::unix::fs::symlink(&outside, &target).unwrap();
        let err = careful_open_file(&dfd, std::ffi::OsStr::new("real.txt")).unwrap_err();
        assert!(matches!(err, IngestError::Containment(_)), "swapped-in symlink must be refused, got {err:?}");
        // On Linux, prove RESOLVE_NO_SYMLINKS (or O_NOFOLLOW) is what fired by
        // checking the error is ELOOP-class, not an accidental ENOENT/other. The
        // kernel surfaces a symlink refusal as ELOOP; this guards the test from
        // passing for the wrong reason. macOS is unaffected (cfg'd out).
        #[cfg(target_os = "linux")]
        {
            let IngestError::Containment(msg) = &err else {
                panic!("expected Containment, got {err:?}");
            };
            let eloop = rustix::io::Errno::LOOP.to_string();
            assert!(
                msg.contains(&eloop) || msg.to_lowercase().contains("too many levels of symbolic links"),
                "swap must fail ELOOP-class (symlink refused), got: {msg}"
            );
        }
    }

    // FIFO-swap DoS: a name resolves to a real file at readdir, then is swapped
    // for a FIFO before the open. O_NOFOLLOW does NOT help (a FIFO is not a
    // symlink), so without O_NONBLOCK the read-only open would block forever
    // waiting for a writer. The open MUST return and be rejected as non-regular.
    // If this hangs, the O_NONBLOCK guard is missing on the exercised arm.
    //
    // The FIFO is created via the POSIX `mkfifo(1)` tool (uniform across the
    // macOS + Linux CI targets): rustix 0.38's `mknodat` is `cfg(not(apple))`
    // and it ships no `mkfifoat`, and `#![forbid(unsafe_code)]` rules out a raw
    // `libc::mkfifo`. A test fixture may shell out; the "no subprocess" rule is
    // a constraint on the ingest core, not on test setup.
    #[test]
    fn careful_open_refuses_a_fifo_without_hanging() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real.txt");
        std::fs::write(&target, b"ok").unwrap();
        let dfd = open_dir(dir.path());
        // Swap the regular file for a FIFO AFTER the dir fd is open (the TOCTOU window).
        std::fs::remove_file(&target).unwrap();
        let status = std::process::Command::new("mkfifo")
            .arg(&target)
            .status()
            .expect("spawn mkfifo");
        assert!(status.success(), "mkfifo failed to create the test FIFO");
        // Must RETURN (rejected as non-regular), not block forever.
        let err = careful_open_file(&dfd, std::ffi::OsStr::new("real.txt")).unwrap_err();
        assert!(matches!(err, IngestError::Containment(_)),
            "a fifo must be refused as non-regular (no hang), got {err:?}");
    }

    #[test]
    fn read_all_capped_rejects_oversize() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("big.txt"), vec![b'x'; 100]).unwrap();
        let dfd = open_dir(dir.path());
        let cf = careful_open_file(&dfd, std::ffi::OsStr::new("big.txt")).unwrap();
        assert!(matches!(cf.read_all_capped(10), Err(IngestError::TooLarge)));
    }

    // Pins `open_beneath_confined`'s own defensive contract (its `..`/absolute/empty
    // rejections), independent of any caller's pre-validation — callers like the
    // bossclawd capture module hand it an already-clean path, so without this test
    // those branches would be dead-untested.
    #[test]
    fn open_beneath_confined_pins_its_defensive_contract() {
        use std::io::Read as _;
        let root = tempfile::tempdir().unwrap();
        let nested = root.path().join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("f.txt"), b"contained").unwrap();

        // Accept a normal nested root-relative path (and read through the handle).
        let mut file =
            open_beneath_confined(root.path(), std::path::Path::new("a/b/f.txt")).unwrap();
        let mut body = String::new();
        file.read_to_string(&mut body).unwrap();
        assert_eq!(body, "contained");

        // Reject ANY `..` component — even one that would lexically resolve back
        // inside the root (strictest reading: rejection, never normalization).
        assert!(open_beneath_confined(root.path(), std::path::Path::new("../x")).is_err());
        assert!(
            open_beneath_confined(root.path(), std::path::Path::new("a/../a/b/f.txt")).is_err(),
            "a `..` that stays under root must still be rejected outright"
        );
        // Reject an absolute path passed where a root-relative one is required.
        assert!(open_beneath_confined(root.path(), &nested.join("f.txt")).is_err());
        // Reject an empty relative path (no file component under root).
        assert!(open_beneath_confined(root.path(), std::path::Path::new("")).is_err());
    }
}

#[cfg(all(test, unix))]
mod orchestrator_tests {
    use super::*;
    use crate::embed::MockEmbedder;
    use crate::recall::RecallOptions;
    use ed25519_dalek::SigningKey;

    const DEK: [u8; 32] = [42u8; 32];
    const KEY_BYTES: [u8; 32] = [7u8; 32];

    fn run_ingest(log: &EventLog, root: &std::path::Path, router: &ParserRouter, emb: &MockEmbedder) -> IngestReport {
        let mut report = IngestReport::default();
        let mut seen = std::collections::HashSet::new();
        log.ingest_grant_inner(root, router, emb, Instant::now(), &mut seen, &mut report).unwrap();
        report
    }

    #[test]
    fn fresh_then_dedup_then_supersede() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("m.db");
        let folder = dir.path().join("notes");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("a.md"), b"# v1").unwrap();

        let emb = MockEmbedder::new(16);
        let log = EventLog::open(&db, &DEK, SigningKey::from_bytes(&KEY_BYTES)).unwrap();
        log.add_grant(&folder).unwrap();
        let canonical_folder = std::fs::canonicalize(&folder).unwrap();

        let r1 = run_ingest(&log, &canonical_folder, &ParserRouter::native_only(), &emb);
        assert_eq!((r1.ingested, r1.deduped, r1.superseded), (1, 0, 0));

        let r2 = run_ingest(&log, &canonical_folder, &ParserRouter::native_only(), &emb);
        assert_eq!((r2.ingested, r2.deduped, r2.superseded), (0, 1, 0));

        std::fs::write(folder.join("a.md"), b"# v2 changed").unwrap();
        let r3 = run_ingest(&log, &canonical_folder, &ParserRouter::native_only(), &emb);
        assert_eq!((r3.ingested, r3.deduped, r3.superseded), (0, 0, 1));

        let canonical_file = canonical_folder.join("a.md").to_string_lossy().to_string();
        let rec = log.current_file_for_path(&canonical_file).unwrap().unwrap();
        let ev = log.stream_all().unwrap().into_iter().find(|e| e.id == rec.file_event_id).unwrap();
        assert_eq!(ev.content["text"], "# v2 changed");
        assert!(is_external(&ev), "file_ingested is externally tainted");
    }

    #[test]
    fn mtime_change_without_byte_change_does_not_supersede() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("m.db");
        let folder = dir.path().join("notes");
        std::fs::create_dir(&folder).unwrap();
        let f = folder.join("a.md");
        std::fs::write(&f, b"identical bytes").unwrap();
        let emb = MockEmbedder::new(16);
        let log = EventLog::open(&db, &DEK, SigningKey::from_bytes(&KEY_BYTES)).unwrap();
        log.add_grant(&folder).unwrap();
        let canonical = std::fs::canonicalize(&folder).unwrap();
        assert_eq!(run_ingest(&log, &canonical, &ParserRouter::native_only(), &emb).ingested, 1);

        // Rewrite IDENTICAL bytes (bumps mtime, content_hash unchanged).
        std::fs::write(&f, b"identical bytes").unwrap();
        let r = run_ingest(&log, &canonical, &ParserRouter::native_only(), &emb);
        assert_eq!((r.ingested, r.superseded, r.deduped), (0, 0, 1),
            "mtime is provenance-only; identical bytes → dedup, NEVER supersede");
    }

    #[test]
    fn recall_returns_only_current_version_and_drops_revoked() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("m.db");
        let folder = dir.path().join("notes");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("topic.md"), b"alpha unique-token-v1").unwrap();

        let emb = MockEmbedder::new(16);
        let log = EventLog::open_with_recall(&db, &DEK, SigningKey::from_bytes(&KEY_BYTES), &emb).unwrap();
        log.add_grant(&folder).unwrap();
        let canonical_folder = std::fs::canonicalize(&folder).unwrap();
        run_ingest(&log, &canonical_folder, &ParserRouter::native_only(), &emb);

        // Change the file, re-ingest → v1 superseded by v2.
        std::fs::write(folder.join("topic.md"), b"alpha unique-token-v2").unwrap();
        run_ingest(&log, &canonical_folder, &ParserRouter::native_only(), &emb);
        log.rebuild_indexes(&emb).unwrap();
        log.rebuild_graph().unwrap();

        // Recall: only the CURRENT (v2) file id survives the new arm.
        let hits = log.recall(&emb, "alpha", 10, &Default::default()).unwrap();
        let file_hits: Vec<_> = hits.iter().filter(|h| h.kind == crate::graph::FILE_INGESTED_EVENT_TYPE).collect();
        assert_eq!(file_hits.len(), 1, "only the current version surfaces, never both");
        let canonical_file = canonical_folder.join("topic.md").to_string_lossy().to_string();
        let cur = log.current_file_for_path(&canonical_file).unwrap().unwrap();
        assert_eq!(file_hits[0].event_id, cur.file_event_id);
        assert!(file_hits[0].sources.contains(&crate::recall::RecallSource::Keyword),
            "the keyword (FTS) arm surfaces the file — proves ingest + rebuild_indexes populated FTS, not only vectors");

        // Revoke the grant → the file is excluded from recall (still in the log).
        log.revoke_grant(&canonical_folder).unwrap();
        let hits2 = log.recall(&emb, "alpha", 10, &Default::default()).unwrap();
        assert!(hits2.iter().all(|h| h.kind != crate::graph::FILE_INGESTED_EVENT_TYPE),
            "a revoked grant's files do not surface in recall");
    }

    // Door A OPEN: a file_ingested event IS now an evolve extraction subject.
    #[test]
    fn ingested_files_are_evolve_subjects() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("notes");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("a.md"), b"some note").unwrap();
        let emb = MockEmbedder::new(16);
        let log = EventLog::open(&dir.path().join("m.db"), &DEK, SigningKey::from_bytes(&KEY_BYTES)).unwrap();
        log.add_grant(&folder).unwrap();
        let canonical = std::fs::canonicalize(&folder).unwrap();
        assert_eq!(run_ingest(&log, &canonical, &ParserRouter::native_only(), &emb).ingested, 1);
        assert_eq!(log.evolve_status().unwrap().queue_depth, 1,
            "file_ingested events are now evolve subjects (Door A)");
    }

    // Door B OPEN: the evolve loop's internal recall now SURFACES file text as context.
    // Assert the EXACT RecallOptions the loop uses returns file hits, while
    // user-facing recall (defaults) also does — proving the door is open.
    #[test]
    fn evolve_context_recall_includes_file_text() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("notes");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("a.md"), b"zztoken external poison").unwrap();
        let emb = MockEmbedder::new(16);
        let log = EventLog::open_with_recall(&dir.path().join("m.db"), &DEK, SigningKey::from_bytes(&KEY_BYTES), &emb).unwrap();
        log.add_grant(&folder).unwrap();
        let canonical = std::fs::canonicalize(&folder).unwrap();
        run_ingest(&log, &canonical, &ParserRouter::native_only(), &emb);
        log.rebuild_indexes(&emb).unwrap();
        log.rebuild_graph().unwrap();

        // User-facing recall surfaces the file…
        let user = log.recall(&emb, "zztoken", 10, &Default::default()).unwrap();
        assert!(user.iter().any(|h| h.kind == crate::graph::FILE_INGESTED_EVENT_TYPE), "default recall returns the file");
        // Door B OPEN: the evolve-context recall (the loop's exact options) now SURFACES the file.
        // The taint chokepoint + the Pass-A fence keep it safe.
        let ctx = log.recall(&emb, "zztoken", 10, &RecallOptions { exclude_pages: true, exclude_files: false, ..Default::default() }).unwrap();
        assert!(ctx.iter().any(|h| h.kind == crate::graph::FILE_INGESTED_EVENT_TYPE),
            "Door B: file text is available as evolve context");
    }

    #[test]
    fn ingest_all_iterates_active_grants_and_is_recallable() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("m.db");
        let g1 = dir.path().join("notes1");
        let g2 = dir.path().join("notes2");
        std::fs::create_dir(&g1).unwrap();
        std::fs::create_dir(&g2).unwrap();
        std::fs::write(g1.join("x.md"), b"gamma unique-x").unwrap();
        std::fs::write(g2.join("y.md"), b"gamma unique-y").unwrap();

        let emb = MockEmbedder::new(16);
        let log = EventLog::open_with_recall(&db, &DEK, SigningKey::from_bytes(&KEY_BYTES), &emb).unwrap();
        log.add_grant(&g1).unwrap();
        log.add_grant(&g2).unwrap();

        let report = log.ingest_all(&ParserRouter::native_only(), &emb).unwrap();
        assert_eq!(report.ingested, 2, "both granted folders' files ingested");
        let hits = log.recall(&emb, "gamma", 10, &Default::default()).unwrap();
        assert_eq!(hits.iter().filter(|h| h.kind == crate::graph::FILE_INGESTED_EVENT_TYPE).count(), 2);
    }

    #[test]
    fn revoked_grant_is_not_ingested_by_ingest_all() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("m.db");
        let folder = dir.path().join("notes");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("a.md"), b"delta").unwrap();
        let emb = MockEmbedder::new(16);
        let log = EventLog::open_with_recall(&db, &DEK, SigningKey::from_bytes(&KEY_BYTES), &emb).unwrap();
        log.add_grant(&folder).unwrap();
        log.revoke_grant(&std::fs::canonicalize(&folder).unwrap()).unwrap();
        let report = log.ingest_all(&ParserRouter::native_only(), &emb).unwrap();
        assert_eq!(report.ingested, 0, "ingest_all skips revoked grants");
    }

    #[test]
    fn file_ingested_survives_byte_identical_rebuild() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("m.db");
        let folder = dir.path().join("notes");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("a.md"), b"stable bytes").unwrap();
        let emb = MockEmbedder::new(16);

        let recorded_hash = {
            let log = EventLog::open(&db, &DEK, SigningKey::from_bytes(&KEY_BYTES)).unwrap();
            log.add_grant(&folder).unwrap();
            let canonical = std::fs::canonicalize(&folder).unwrap();
            run_ingest(&log, &canonical, &ParserRouter::native_only(), &emb);
            let ev = log.stream_all().unwrap().into_iter()
                .find(|e| e.event_type == crate::graph::FILE_INGESTED_EVENT_TYPE).unwrap();
            ev.hash.clone().unwrap()
        };
        // Reopen: the chain re-verifies and the file_ingested hash is unchanged.
        let log2 = EventLog::open_with_recall(&db, &DEK, SigningKey::from_bytes(&KEY_BYTES), &emb).unwrap();
        log2.verify_chain().unwrap();
        let ev2 = log2.stream_all().unwrap().into_iter()
            .find(|e| e.event_type == crate::graph::FILE_INGESTED_EVENT_TYPE).unwrap();
        assert_eq!(ev2.hash.unwrap(), recorded_hash, "file_ingested is byte-identical across reopen");
    }

    /// Read-cap proof (PR16): the READ cap in `ingest_grant_inner` — not just the
    /// walk gate — is parser-aware. `ingest_grant_inner` uses `WalkLimits::default()`
    /// internally (not injectable), so this drives the REAL production caps with real
    /// sizes: an ~11 MiB `.pdf` is over the 10 MiB native cap but under the 100 MiB
    /// rich cap (ingested), while an ~11 MiB `.txt` is over the native cap (skipped
    /// "oversize"). Both filler bodies are `b'a'` (valid UTF-8). We use
    /// `NativeTextParser` for both slots so the cap logic — not parser dispatch —
    /// is what is under test. Production would use `NullRichParser` (→ skip), but
    /// that would mask the size decision; `native_only()` is the correct choice for
    /// a dispatch test, not a cap test.
    #[test]
    fn read_cap_is_rich_aware_for_oversize_native_but_undersize_rich() {
        const ELEVEN_MIB: usize = 11 * 1024 * 1024;
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("m.db");
        let folder = dir.path().join("notes");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("big.pdf"), vec![b'a'; ELEVEN_MIB]).unwrap();
        std::fs::write(folder.join("big.txt"), vec![b'a'; ELEVEN_MIB]).unwrap();

        let emb = MockEmbedder::new(16);
        let log = EventLog::open(&db, &DEK, SigningKey::from_bytes(&KEY_BYTES)).unwrap();
        log.add_grant(&folder).unwrap();
        let canonical_folder = std::fs::canonicalize(&folder).unwrap();

        // NativeTextParser for BOTH slots: we are testing the cap routing, not
        // the parser dispatch. The real rich slot (NullRichParser) would return
        // SandboxUnavailable and mask the size decision.
        let router = ParserRouter::new(
            Box::new(NativeTextParser),
            Box::new(NativeTextParser),
        );
        let report = log.ingest_all(&router, &emb).unwrap();

        // The 11 MiB .pdf clears the 100 MiB rich cap and is ingested…
        assert_eq!(report.ingested, 1, "only the rich .pdf ingests; got {report:?}");
        let pdf_path = canonical_folder.join("big.pdf").to_string_lossy().to_string();
        assert!(log.current_file_for_path(&pdf_path).unwrap().is_some(), "the .pdf must be recorded");
        // …while the 11 MiB .txt is over the 10 MiB native cap → skipped "oversize".
        let txt_path = canonical_folder.join("big.txt");
        assert!(
            report.skipped.iter().any(|(p, r)| *p == txt_path && r == "oversize"),
            "the native .txt over the native cap must skip 'oversize'; got: {:?}", report.skipped
        );
    }

    // ── T9: fd-leak proof ──────────────────────────────────────────────────────
    /// Proves that the parent's SQLCipher DB handle (opened in the same process)
    /// does NOT leak into the jailed child as an open regular-file fd. Requires the
    /// venv + jail to be available; gated + ignored so CI skips it without the env.
    #[cfg(all(any(target_os = "macos", target_os = "linux"), feature = "markitdown"))]
    #[test]
    #[ignore]
    fn db_handle_does_not_leak_into_the_jailed_child() {
        let dir = tempfile::tempdir().unwrap();
        // A live EventLog → its SQLCipher DB handle + signing key are open in THIS process.
        let _log = EventLog::open(&dir.path().join("m.db"), &DEK, SigningKey::from_bytes(&KEY_BYTES)).unwrap();
        let leaked = crate::sandbox::spawn_jailed_fd_scan();
        assert!(leaked.is_empty(), "a regular-file fd leaked into the jailed child (the DB handle?): {leaked:?}");
    }

    // ── T9: end-to-end ingest through the jail ─────────────────────────────────
    /// A granted folder containing a real PDF is ingested through the sandboxed
    /// markitdown parser; the resulting event must be externally tainted (is_external).
    #[cfg(all(any(target_os = "macos", target_os = "linux"), feature = "markitdown"))]
    #[test]
    #[ignore]
    fn ingest_grant_with_real_pdf_creates_external_taint_event() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("docs");
        std::fs::create_dir(&folder).unwrap();
        std::fs::copy("tests/fixtures/hello.pdf", folder.join("hello.pdf")).unwrap();
        let emb = MockEmbedder::new(16);
        let log = EventLog::open(&dir.path().join("m.db"), &DEK, SigningKey::from_bytes(&KEY_BYTES)).unwrap();
        log.add_grant(&folder).unwrap();
        let router = ParserRouter::new(
            Box::new(NativeTextParser),
            Box::new(crate::sandbox::SandboxedMarkitdownParser::discover().expect("venv + jail")),
        );
        let report = log.ingest_all(&router, &emb).unwrap();
        assert_eq!(report.ingested, 1, "the PDF should ingest; got {report:?}");
        // The ingested event must be externally tainted (the taint root M6 consumes).
        let canonical = std::fs::canonicalize(&folder).unwrap().join("hello.pdf").to_string_lossy().to_string();
        let rec = log.current_file_for_path(&canonical).unwrap().expect("pdf recorded");
        let ev = log.stream_all().unwrap().into_iter().find(|e| e.id == rec.file_event_id).unwrap();
        assert!(is_external(&ev), "file_ingested from a real PDF must carry external taint; content={:?}", ev.content);
    }

    // ── T9: feature-off degradation (hermetic — no venv, not ignored) ──────────
    /// With `ParserRouter::native_only()`, a PDF in a granted folder must be recorded
    /// as skipped with reason "sandbox unavailable" — no panic, no ingest, no event.
    #[test]
    fn feature_off_router_skips_pdf_with_sandbox_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("docs");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("x.pdf"), b"%PDF-1.4 not really").unwrap();
        let emb = MockEmbedder::new(16);
        let log = EventLog::open(&dir.path().join("m.db"), &DEK, SigningKey::from_bytes(&KEY_BYTES)).unwrap();
        log.add_grant(&folder).unwrap();
        let report = log.ingest_all(&ParserRouter::native_only(), &emb).unwrap();
        assert_eq!(report.ingested, 0);
        assert!(
            report.skipped.iter().any(|(p, r)| p.ends_with("x.pdf") && r.contains("sandbox unavailable")),
            "got {:?}", report.skipped
        );
    }

    #[test]
    fn empty_extraction_is_skipped_not_an_empty_event() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("m.db");
        let folder = dir.path().join("notes");
        std::fs::create_dir(&folder).unwrap();
        // A .md file whose parser returns whitespace-only text.
        std::fs::write(folder.join("blank.md"), b"   ").unwrap();

        let emb = MockEmbedder::new(16);
        let log = EventLog::open(&db, &DEK, SigningKey::from_bytes(&KEY_BYTES)).unwrap();
        log.add_grant(&folder).unwrap();
        let canonical_folder = std::fs::canonicalize(&folder).unwrap();

        // Router with a MockParser that returns whitespace for every file.
        let router = ParserRouter::new(
            Box::new(MockParser { output: "   ".into() }),
            Box::new(NullRichParser),
        );
        let report = run_ingest(&log, &canonical_folder, &router, &emb);
        assert_eq!(report.ingested, 0, "whitespace-only extraction must not produce an event");
        assert!(
            report.skipped.iter().any(|(_, r)| r == "no extractable text"),
            "must record 'no extractable text' skip; got: {:?}", report.skipped
        );
    }
}

/// Read/walk budget is parser-aware (M5b T1): rich extensions get the larger
/// `max_rich_file_bytes` cap; everything else stays on `max_file_bytes`.
#[cfg(all(test, unix))]
mod rich_budget_tests {
    use super::*;

    #[test]
    fn walk_applies_rich_budget_to_rich_ext_and_native_budget_to_others() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("big.pdf"), vec![b'a'; 500]).unwrap();
        std::fs::write(dir.path().join("big.txt"), vec![b'a'; 500]).unwrap();
        let limits = WalkLimits { max_file_bytes: 100, max_rich_file_bytes: 10_000, ..Default::default() };
        let (mut seen, mut report, mut walked) = (std::collections::HashSet::new(), IngestReport::default(), Vec::new());
        walk_grant(dir.path(), &limits, Instant::now(), &mut seen, &mut report, |wf| { walked.push(wf.canonical_path); Ok(()) }).unwrap();
        assert!(walked.iter().any(|p| p.ends_with("big.pdf")), "rich file should pass its larger budget");
        assert!(report.skipped.iter().any(|(p, r)| p.ends_with("big.txt") && r == "oversize"), "native file over native cap should skip");
    }
}
