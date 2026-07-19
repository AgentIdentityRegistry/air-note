//! The `event_id → page_id` bridge (spec §5 Rev 2, review-critical).
//!
//! INVARIANT (load-bearing): the harness NEVER runs an evolve tick, so every recall hit is a
//! `file_ingested` event whose `event_id` equals the `file_event_id` that `ListFiles` reports
//! for its source file (bossclaw-core `graph.rs`: `file_event_id: ev.id`). If a recall hit does
//! not map through this table, that invariant has broken (e.g. someone added an evolve call to
//! the harness, whose minted memory events have no file mapping) → the mapping FAILS LOUD as a
//! run error. There is deliberately NO fallback to the raw event id: a silent fallback could
//! never match a gold page id, would score AIR 0.0 on every known-item query, and would
//! fabricate a losing baseline.

use std::collections::HashMap;
use std::path::Path;

use bossclawd_proto::types::FileRecordMirror;

use crate::corpus::page_id_from_rel;

/// The bridge table: `file_event_id → page_id`. Built once per run, right after ingest.
/// `Clone` so the reflect-gate can hold one copy in the scoring AIR arm and one for the
/// union-coverage cites-mapping (both map cites through the SAME table — dev-only).
#[derive(Clone)]
pub struct PageResolver {
    by_event: HashMap<String, String>,
}

impl PageResolver {
    /// Build from `ListFiles` records: `file_event_id → canonical_path` → strip the
    /// (canonicalized) corpus-root prefix → page id. A record outside the corpus root is an
    /// error (the harness grants exactly one root).
    pub fn from_file_records(
        records: &[FileRecordMirror],
        corpus_root: &Path,
    ) -> anyhow::Result<Self> {
        let root = std::fs::canonicalize(corpus_root)
            .map_err(|e| anyhow::anyhow!("canonicalize corpus root {corpus_root:?}: {e}"))?;
        let root_str = root.to_string_lossy().to_string();
        let mut by_event = HashMap::with_capacity(records.len());
        for r in records {
            let rel = r
                .canonical_path
                .strip_prefix(&root_str)
                .and_then(|s| s.strip_prefix('/'))
                .ok_or_else(|| anyhow::anyhow!(
                    "ingested file {} is not under the corpus root {root_str}",
                    r.canonical_path
                ))?;
            by_event.insert(r.file_event_id.clone(), page_id_from_rel(rel));
        }
        Ok(Self { by_event })
    }

    /// Map a recall hit's event id to its page id. FAILS LOUD on an unmapped id — see the
    /// module docs: no evolve ⇒ every hit is a file_ingested event; an unmapped hit means the
    /// invariant broke, and scoring must stop rather than silently zero AIR's scores.
    pub fn page_id_of(&self, event_id: &str) -> anyhow::Result<String> {
        self.by_event.get(event_id).cloned().ok_or_else(|| anyhow::anyhow!(
            "recall hit event {event_id} does not map to an ingested file — the no-evolve \
             invariant broke (or ListFiles is stale); refusing to score (no silent fallback)"
        ))
    }

    /// TEST-ONLY ctor: fill `by_event` directly from `(file_event_id, page_id)` pairs, bypassing
    /// the file-record bridge. Lets the reflection page-arm test map a file event to a known gold
    /// id without spinning a corpus; the load-bearing `from_file_records` ctor is untouched.
    #[cfg(test)]
    pub(crate) fn from_pairs_for_test(pairs: &[(&str, &str)]) -> Self {
        Self {
            by_event: pairs.iter().map(|(ev, pid)| (ev.to_string(), pid.to_string())).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bossclawd_proto::types::FileRecordMirror;

    fn record(root: &std::path::Path, rel: &str, event_id: &str) -> FileRecordMirror {
        FileRecordMirror {
            canonical_path: root.join(rel).to_string_lossy().to_string(),
            file_event_id: event_id.to_string(),
            content_hash: "h".to_string(),
            grant_root: root.to_string_lossy().to_string(),
        }
    }

    #[test]
    fn maps_event_ids_to_page_ids() {
        let root = tempfile::tempdir().unwrap();
        let canon = std::fs::canonicalize(root.path()).unwrap();
        let records = vec![record(&canon, "air/foo.md", "ev1"), record(&canon, "top.md", "ev2")];
        let r = PageResolver::from_file_records(&records, root.path()).unwrap();
        assert_eq!(r.page_id_of("ev1").unwrap(), "air/foo");
        assert_eq!(r.page_id_of("ev2").unwrap(), "top");
    }

    #[test]
    fn unmapped_event_id_fails_loud_naming_the_invariant() {
        let root = tempfile::tempdir().unwrap();
        let r = PageResolver::from_file_records(&[], root.path()).unwrap();
        let err = r.page_id_of("evolve-minted-event").unwrap_err();
        assert!(err.to_string().contains("invariant"), "names the broken invariant: {err}");
    }

    #[test]
    fn record_outside_corpus_root_is_an_error() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let canon_other = std::fs::canonicalize(other.path()).unwrap();
        let records = vec![record(&canon_other, "x.md", "ev1")];
        assert!(PageResolver::from_file_records(&records, root.path()).is_err());

        // Degenerate equal-path case: a record whose canonical_path EQUALS the canonicalized
        // root itself has no '/' boundary after the prefix (it is not a file UNDER the root)
        // and must fail loud instead of minting a page id from an empty rel path.
        let canon_root = std::fs::canonicalize(root.path()).unwrap();
        let equal = vec![FileRecordMirror {
            canonical_path: canon_root.to_string_lossy().to_string(),
            file_event_id: "ev-root".to_string(),
            content_hash: "h".to_string(),
            grant_root: canon_root.to_string_lossy().to_string(),
        }];
        assert!(
            PageResolver::from_file_records(&equal, root.path()).is_err(),
            "canonical_path == corpus root must be an error"
        );

        // Sibling-prefix hazard: root `<tmp>/a` vs a record under `<tmp>/ab/` — a naive string
        // strip_prefix would silently mint page id "b/x"; the '/' boundary check must ERROR.
        let base = tempfile::tempdir().unwrap();
        let root_a = base.path().join("a");
        let sib_ab = base.path().join("ab");
        std::fs::create_dir(&root_a).unwrap();
        std::fs::create_dir(&sib_ab).unwrap();
        let canon_ab = std::fs::canonicalize(&sib_ab).unwrap();
        let sib_records = vec![record(&canon_ab, "x.md", "ev-sib")];
        assert!(
            PageResolver::from_file_records(&sib_records, &root_a).is_err(),
            "sibling-prefix path (root /a vs /ab) must be an error, not page id b/x"
        );
    }
}
