//! M5a ingest pipeline: read-only ingest of user-granted folders into the signed
//! log as recallable, externally-tainted `file_ingested` events.
//!
//! Safety model (spec §6): kernel-enforced containment via an `openat`-fd-chain
//! walk with `O_NOFOLLOW` on every descent + a per-OS careful final open; a
//! never-touch hazard-reduction filter; per-path dedup/version-supersede; and the
//! taint root (`origin: "external"` inside signed content). NO subprocess, NO
//! `unsafe` (rustix encapsulates the syscalls); rich formats (PDF/docx) are M5b.

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
