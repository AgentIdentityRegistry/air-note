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
}
