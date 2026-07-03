//! Corpus preparation: copy `~/brain/*.md` into the harness home (frontmatter stripping is
//! PROBE-PINNED, spec §2 Rev 2), skip dot-entries, record a sha256 manifest. The
//! `~/brain`-relative path stem is the arm-independent page identity (spec §5).

/// Probe-A-pinned (Rev 2): strip frontmatter ONLY if GBrain strips it before chunking; if
/// GBrain indexes frontmatter, both systems must index it. Default assumes GBrain strips —
/// Task 1 confirms and the implementer flips this if reality differs.
pub const STRIP_FRONTMATTER: bool = true;

use serde::Serialize;

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

/// `~/brain`-relative path ("air/foo.md") → page id ("air/foo").
pub fn page_id_from_rel(rel: &str) -> String {
    rel.strip_suffix(".md").unwrap_or(rel).to_string()
}

/// GBrain slug → the SAME page id space (Probe A pins slugs as stem form; a stray ".md" is
/// tolerated so a match is never missed on a formatting quirk).
pub fn page_id_from_gbrain_slug(slug: &str) -> String {
    slug.strip_suffix(".md").unwrap_or(slug).to_string()
}

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::frontmatter::strip_frontmatter;

/// Lowercase hex of a digest (avoids pulling `hex` for one call site).
pub fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Recursively copy every `*.md` under `src` into `dst`, optionally stripping YAML frontmatter
/// (`strip` is the probe-pinned `STRIP_FRONTMATTER`), skipping any entry whose name starts with
/// '.' (files AND dirs), recording a sha256 manifest of the bytes ACTUALLY indexed. Sorted for
/// reproducible manifests.
pub fn prepare_corpus(src: &Path, dst: &Path, strip: bool) -> anyhow::Result<CorpusManifest> {
    let mut rels: Vec<PathBuf> = Vec::new();
    collect_md(src, src, &mut rels)?;
    rels.sort();

    let mut entries = Vec::with_capacity(rels.len());
    let mut total_bytes = 0u64;
    for rel in &rels {
        let raw = std::fs::read_to_string(src.join(rel))?;
        let text = if strip { strip_frontmatter(&raw).to_string() } else { raw };
        let out_path = dst.join(rel);
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&out_path, text.as_bytes())?;
        let sha256 = hex_lower(&Sha256::digest(text.as_bytes()));
        let bytes = text.len() as u64;
        total_bytes += bytes;
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        entries.push(ManifestEntry { page_id: page_id_from_rel(&rel_str), sha256, bytes });
    }
    let snapshot_unix_secs =
        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    Ok(CorpusManifest { snapshot_unix_secs, file_count: entries.len(), total_bytes, entries })
}

/// Depth-first collect of `*.md` RELATIVE paths, skipping dot-entries at every level.
fn collect_md(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            collect_md(root, &path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            out.push(path.strip_prefix(root).unwrap_or(&path).to_path_buf());
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
}
