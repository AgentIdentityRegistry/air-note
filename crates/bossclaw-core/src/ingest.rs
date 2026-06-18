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
}

impl std::fmt::Display for IngestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IngestError::NonUtf8 => write!(f, "not valid UTF-8 (rich formats are M5b)"),
            IngestError::Containment(m) => write!(f, "containment refused: {m}"),
            IngestError::TooLarge => write!(f, "exceeds byte cap"),
            IngestError::Parse(m) => write!(f, "parse error: {m}"),
            IngestError::Io(m) => write!(f, "io error: {m}"),
        }
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
// `Path` variant + the type itself are only constructed on their matching OS;
// wired into the walk's dedup map in Task 6/7.
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
    // Read once by `read_all_capped`, which the walk calls in Task 6/7; until then
    // the lib build (no tests) sees the field as unread.
    #[allow(dead_code)]
    file: std::fs::File,
    identity: FileIdentity,
    size: u64,
}

impl ContainedFile {
    // `identity`/`size` are consumed by the walk's dedup + budget logic in Task 6/7.
    #[allow(dead_code)]
    pub(crate) fn identity(&self) -> &FileIdentity {
        &self.identity
    }
    #[allow(dead_code)]
    pub(crate) fn size(&self) -> u64 {
        self.size
    }

    /// Read up to `cap` bytes. Returns [`IngestError::TooLarge`] if the file has
    /// more than `cap` bytes (read `cap + 1` and check) — never a truncated body.
    // Called by the walk orchestrator in Task 6/7 (the single contained read).
    #[allow(dead_code)]
    pub(crate) fn read_all_capped(mut self, cap: usize) -> Result<Vec<u8>, IngestError> {
        let mut buf = Vec::with_capacity(self.size.min(cap as u64) as usize);
        let read = (&mut self.file)
            .take(cap as u64 + 1)
            .read_to_end(&mut buf)
            .map_err(|e| IngestError::Io(e.to_string()))?;
        if read > cap {
            return Err(IngestError::TooLarge);
        }
        Ok(buf)
    }
}

// ── Unix: open the final file from a parent dir fd with O_NOFOLLOW. The fd-chain
//    walk (Task 6) reached `dir_fd` via O_NOFOLLOW descents, so a NOFOLLOW open
//    here refuses a final-component symlink AND a dir swapped to a symlink after
//    readdir named it (TOCTOU). On Linux we additionally use openat2 with
//    RESOLVE_BENEATH|RESOLVE_NO_SYMLINKS (spec D3) when the kernel supports it. ──
// Wired into the walk in Task 6 (it produces the `dir_fd` chain that calls this).
#[allow(dead_code)]
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
        match openat2(
            dir_fd,
            name.as_bytes(),
            OFlags::RDONLY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
        ) {
            Ok(fd) => fd,
            // Pre-5.6 kernels lack openat2 → fall back to the NOFOLLOW open, which
            // still refuses a final-component symlink (the chain gave containment).
            Err(rustix::io::Errno::NOSYS) => rustix::fs::openat(
                dir_fd,
                name.as_bytes(),
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|e| IngestError::Containment(e.to_string()))?,
            Err(e) => return Err(IngestError::Containment(e.to_string())),
        }
    };
    #[cfg(not(target_os = "linux"))]
    let owned = rustix::fs::openat(
        dir_fd,
        name.as_bytes(),
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| IngestError::Containment(e.to_string()))?;

    let st = rustix::fs::fstat(&owned).map_err(|e| IngestError::Io(e.to_string()))?;
    // Reject anything that is not a regular file (fifos, devices, dirs).
    if rustix::fs::FileType::from_raw_mode(st.st_mode) != rustix::fs::FileType::RegularFile {
        return Err(IngestError::Containment("not a regular file".into()));
    }
    Ok(ContainedFile {
        file: std::fs::File::from(owned),
        identity: FileIdentity::DevIno(st.st_dev as u64, st.st_ino as u64),
        size: st.st_size as u64,
    })
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn path_hint_carries_no_path() {
        // Compile-time guarantee: PathHint has exactly one field, the extension.
        let h = PathHint { ext: Some("txt".into()) };
        assert_eq!(h.ext.as_deref(), Some("txt"));
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

    #[test]
    fn read_all_capped_rejects_oversize() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("big.txt"), vec![b'x'; 100]).unwrap();
        let dfd = open_dir(dir.path());
        let cf = careful_open_file(&dfd, std::ffi::OsStr::new("big.txt")).unwrap();
        assert!(matches!(cf.read_all_capped(10), Err(IngestError::TooLarge)));
    }
}
