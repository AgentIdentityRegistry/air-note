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
                .map(|s| s.trim_start_matches('/'))
                .ok_or_else(|| anyhow::anyhow!(
                    "ingested file {} is outside the corpus root {root_str}",
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
    }
}
