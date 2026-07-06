//! Corpus preparation: copy `~/brain/*.md` into the harness home (frontmatter stripping is
//! PROBE-PINNED, spec §2 Rev 2), skip dot-entries, record a sha256 manifest. The
//! `~/brain`-relative path stem is the arm-independent page identity (spec §5).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use serde::Serialize;
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

use crate::frontmatter::strip_frontmatter;

/// Probe-A-pinned (Rev 2): strip frontmatter ONLY if GBrain strips it before chunking; if
/// GBrain indexes frontmatter, both systems must index it. Default assumes GBrain strips —
/// Task 1 confirms and the implementer flips this if reality differs.
pub const STRIP_FRONTMATTER: bool = true;

/// One manifest entry: page id + sha256 of the bytes actually indexed + byte count.
#[derive(Debug, Clone, Serialize)]
pub struct ManifestEntry {
    pub page_id: String,
    pub sha256: String,
    pub bytes: u64,
}

/// The full manifest recorded in the report (spec §2): snapshot time + per-file entries.
#[derive(Debug, Clone, Serialize)]
pub struct CorpusManifest {
    pub snapshot_unix_secs: u64,
    pub file_count: usize,
    pub total_bytes: u64,
    pub entries: Vec<ManifestEntry>,
}

/// `~/brain`-relative path ("air/foo.md") → page id ("air/foo"). The ".md" suffix is stripped
/// case-insensitively, and the id is NFC-normalized so filesystem-provided names (which may
/// arrive NFD, e.g. from HFS+) join GBrain's NFC slugs instead of failing silently.
pub fn page_id_from_rel(rel: &str) -> String {
    strip_md_suffix(rel).nfc().collect()
}

/// GBrain slug → the SAME page id space (Probe A pins slugs as stem form; a stray ".md" is
/// tolerated so a match is never missed on a formatting quirk).
pub fn page_id_from_gbrain_slug(slug: &str) -> String {
    page_id_from_rel(slug)
}

/// Strip a trailing ".md" of any ASCII case ("x.md", "x.MD", "x.Md" → "x"). A matched suffix is
/// pure ASCII, so the 3-byte cut is always a char boundary.
fn strip_md_suffix(rel: &str) -> &str {
    let n = rel.len();
    if n >= 3 && rel.as_bytes()[n - 3..].eq_ignore_ascii_case(b".md") {
        &rel[..n - 3]
    } else {
        rel
    }
}

/// Lowercase hex of a digest (avoids pulling `hex` for one call site).
pub fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(&mut s, "{b:02x}");
    }
    s
}

/// Recursively copy every `*.md` under `src` into `dst`, optionally stripping YAML frontmatter
/// (`strip` is the probe-pinned `STRIP_FRONTMATTER`), skipping any entry whose name starts with
/// '.' (files AND dirs), recording a sha256 manifest of the bytes ACTUALLY indexed. Sorted for
/// reproducible manifests.
///
/// `dst` must be an empty/fresh directory — stale files are not detected or manifested.
pub fn prepare_corpus(src: &Path, dst: &Path, strip: bool) -> anyhow::Result<CorpusManifest> {
    let mut rels: Vec<PathBuf> = Vec::new();
    collect_md(src, src, &mut rels)?;
    rels.sort();

    let mut entries = Vec::with_capacity(rels.len());
    let mut total_bytes = 0u64;
    for rel in &rels {
        let src_path = src.join(rel);
        let raw = std::fs::read_to_string(&src_path)
            .with_context(|| format!("reading {}", src_path.display()))?;
        let text = if strip { strip_frontmatter(&raw).to_string() } else { raw };
        let out_path = dst.join(rel);
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(&out_path, text.as_bytes())
            .with_context(|| format!("writing {}", out_path.display()))?;
        let sha256 = hex_lower(&Sha256::digest(text.as_bytes()));
        let bytes = text.len() as u64;
        total_bytes += bytes;
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        entries.push(ManifestEntry { page_id: page_id_from_rel(&rel_str), sha256, bytes });
    }
    let snapshot_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .expect("system clock before 1970");
    Ok(CorpusManifest { snapshot_unix_secs, file_count: entries.len(), total_bytes, entries })
}

/// Depth-first collect of `*.md` RELATIVE paths (extension matched case-insensitively), skipping
/// dot-entries at every level. Symlinks are skipped, never followed — no link cycles, and no
/// file outside `root` can be indexed under a root-relative id.
fn collect_md(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    let iter =
        std::fs::read_dir(dir).with_context(|| format!("reading dir {}", dir.display()))?;
    for entry in iter {
        let entry = entry.with_context(|| format!("reading an entry of {}", dir.display()))?;
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        // file_type() does NOT follow symlinks — dispatch on the link itself, not its target.
        let file_type = entry
            .file_type()
            .with_context(|| format!("reading file type of {}", entry.path().display()))?;
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            collect_md(root, &path, out)?;
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("md"))
        {
            // A walked path not under `root` means the id would be wrong AND `dst.join(rel)`
            // with an absolute `rel` would discard `dst` — fail hard toward safety.
            let rel = path.strip_prefix(root).map_err(|e| {
                anyhow::anyhow!(
                    "path {} escaped corpus root {}: {e}",
                    path.display(),
                    root.display()
                )
            })?;
            out.push(rel.to_path_buf());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_id_is_brain_relative_stem() {
        assert_eq!(page_id_from_rel("air/foo.md"), "air/foo");
        assert_eq!(page_id_from_rel("people/kwang-wook-ahn.md"), "people/kwang-wook-ahn");
        assert_eq!(page_id_from_rel("top.md"), "top");
    }

    #[test]
    fn gbrain_slug_maps_to_same_page_id() {
        assert_eq!(page_id_from_gbrain_slug("air/foo"), "air/foo");
        assert_eq!(page_id_from_gbrain_slug("air/foo.md"), "air/foo");
    }

    #[test]
    fn manifest_entry_holds_id_and_hash() {
        let e = ManifestEntry { page_id: "air/foo".into(), sha256: "abc".into(), bytes: 12 };
        assert_eq!(e.page_id, "air/foo");
        assert_eq!(e.bytes, 12);
    }

    #[test]
    fn prepare_copies_md_strips_frontmatter_skips_dotdirs() {
        use std::fs;
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        fs::create_dir_all(src.path().join("air")).unwrap();
        fs::write(src.path().join("air/foo.md"), "---\ntitle: F\n---\n# Foo\nbody\n").unwrap();
        fs::create_dir_all(src.path().join(".obsidian")).unwrap();
        fs::write(src.path().join(".obsidian/cache.md"), "junk\n").unwrap();
        fs::write(src.path().join(".hidden.md"), "junk\n").unwrap();
        fs::write(src.path().join("air/notes.txt"), "not markdown\n").unwrap();

        let manifest = prepare_corpus(src.path(), dst.path(), true).unwrap();

        assert_eq!(manifest.file_count, 1);
        assert_eq!(manifest.entries[0].page_id, "air/foo");
        let copied = fs::read_to_string(dst.path().join("air/foo.md")).unwrap();
        assert_eq!(copied, "# Foo\nbody\n");
        assert!(!dst.path().join(".obsidian").exists());
        assert!(!dst.path().join(".hidden.md").exists());
        assert!(!dst.path().join("air/notes.txt").exists());
        use sha2::{Digest, Sha256};
        assert_eq!(manifest.entries[0].sha256, hex_lower(&Sha256::digest(b"# Foo\nbody\n")));
    }

    #[test]
    fn page_id_normalizes_unicode_to_nfc() {
        use unicode_normalization::UnicodeNormalization;
        // Construct the NFD form programmatically so this source file stays NFC-clean.
        let nfd: String = "concepts/투자일임업.md".nfd().collect();
        let nfc = "concepts/투자일임업";
        assert_ne!(nfd, format!("{nfc}.md"), "fixture must actually be decomposed");
        assert_eq!(page_id_from_rel(&nfd), nfc);
        assert_eq!(page_id_from_gbrain_slug(nfc), page_id_from_rel(&nfd));
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_skipped_never_followed() {
        use std::fs;
        use std::os::unix::fs::symlink;
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        fs::create_dir_all(src.path().join("real")).unwrap();
        fs::write(src.path().join("real/note.md"), "# real\n").unwrap();
        // A tree OUTSIDE the corpus root, reachable only via symlinks: one dir link, one file link.
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("evil.md"), "# outside\n").unwrap();
        symlink(outside.path(), src.path().join("linkdir")).unwrap();
        symlink(outside.path().join("evil.md"), src.path().join("linkfile.md")).unwrap();

        let manifest = prepare_corpus(src.path(), dst.path(), true).unwrap();

        assert_eq!(manifest.file_count, 1);
        assert_eq!(manifest.entries[0].page_id, "real/note");
    }

    #[test]
    fn manifest_is_sorted_and_skips_nested_dot_dirs() {
        use std::fs;
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        fs::write(src.path().join("b.md"), "b\n").unwrap();
        fs::write(src.path().join("a.md"), "a\n").unwrap();
        fs::create_dir_all(src.path().join("air/.trash")).unwrap();
        fs::write(src.path().join("air/.trash/x.md"), "x\n").unwrap();
        fs::write(src.path().join("air/z.md"), "z\n").unwrap();

        let manifest = prepare_corpus(src.path(), dst.path(), false).unwrap();

        let ids: Vec<&str> = manifest.entries.iter().map(|e| e.page_id.as_str()).collect();
        assert_eq!(ids, ["a", "air/z", "b"], "manifest order must be exactly sorted");
        assert!(!dst.path().join("air/.trash").exists());
    }

    #[test]
    fn uppercase_md_extension_is_collected() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("NOTES.MD"), "# notes\n").unwrap();

        let manifest = prepare_corpus(src.path(), dst.path(), false).unwrap();

        assert_eq!(manifest.file_count, 1);
        // Page id keeps the original case minus the suffix (stripped case-insensitively).
        assert_eq!(manifest.entries[0].page_id, "NOTES");
    }

    #[test]
    fn prepare_with_strip_false_keeps_frontmatter() {
        // Rev 2 (spec §2): if Probe A finds GBrain INDEXES frontmatter, the harness must not strip.
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        let raw = "---\ntitle: F\n---\n# Foo\nbody\n";
        std::fs::write(src.path().join("foo.md"), raw).unwrap();
        let manifest = prepare_corpus(src.path(), dst.path(), false).unwrap();
        assert_eq!(std::fs::read_to_string(dst.path().join("foo.md")).unwrap(), raw);
        assert_eq!(manifest.entries[0].bytes, raw.len() as u64);
    }

    #[test]
    fn manifest_sha_is_stable_and_content_sensitive() {
        let m = CorpusManifest {
            snapshot_unix_secs: 1,
            file_count: 2,
            total_bytes: 10,
            entries: vec![
                ManifestEntry { page_id: "a".into(), sha256: "s1".into(), bytes: 5 },
                ManifestEntry { page_id: "b".into(), sha256: "s2".into(), bytes: 5 },
            ],
        };
        let sha = manifest_sha(&m);
        assert_eq!(sha.len(), 64, "sha256 hex");
        assert_eq!(sha, manifest_sha(&m), "deterministic");
        let mut m2 = m.clone();
        m2.entries[1].sha256 = "s3".into();
        assert_ne!(manifest_sha(&m2), sha, "content change changes the id");
        let mut m3 = m.clone();
        m3.snapshot_unix_secs = 999; // copy TIME must not change identity
        assert_eq!(manifest_sha(&m3), sha, "snapshot time is not part of identity");
    }
}
