//! Frozen query-case persistence (spec §3.0.2): save the built case list once, reload it on
//! every later rung run, so all Phase 1 gates compare IDENTICAL cases. Format = JSONL (one
//! `QueryCase` per line) + sha256 over the exact file bytes — the case-list identity the report
//! records and `compare` (spec §3.0.3) enforces before pairing runs.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::{QueryCase, QuerySource};

    fn sample() -> Vec<QueryCase> {
        vec![
            QueryCase {
                text: "memharness probe findings".into(),
                lang: "en".into(),
                source: QuerySource::Real,
                gold_page_id: Some("air/session-start-protocol".into()),
            },
            QueryCase {
                text: "메모리 하니스는 무엇인가?".into(),
                lang: "ko".into(),
                source: QuerySource::Synthetic,
                gold_page_id: None,
            },
        ]
    }

    #[test]
    fn round_trips_identically_with_stable_sha() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("frozen/cases.jsonl"); // parent must be auto-created
        let saved_sha = save_cases(&path, &sample()).unwrap();
        let (loaded, loaded_sha) = load_cases(&path).unwrap();
        assert_eq!(loaded_sha, saved_sha, "sha identity survives the round trip");
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].text, "memharness probe findings");
        assert_eq!(loaded[0].gold_page_id.as_deref(), Some("air/session-start-protocol"));
        assert_eq!(loaded[1].text, "메모리 하니스는 무엇인가?", "KO text byte-identical");
        assert!(matches!(loaded[1].source, QuerySource::Synthetic));
        assert!(loaded[1].gold_page_id.is_none());
        // Saving the same list again produces the same bytes → same sha (determinism).
        let path2 = dir.path().join("again.jsonl");
        assert_eq!(save_cases(&path2, &sample()).unwrap(), saved_sha);
    }

    #[test]
    fn zero_cases_and_corrupt_lines_fail_loud() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty.jsonl");
        std::fs::write(&empty, "").unwrap();
        let err = load_cases(&empty).unwrap_err().to_string();
        assert!(err.contains("zero cases"), "empty frozen list is always a mistake: {err}");

        let corrupt = dir.path().join("corrupt.jsonl");
        std::fs::write(&corrupt, "{\"text\":\"ok\",\"lang\":\"en\",\"source\":\"Real\",\"gold_page_id\":null}\nnot json\n").unwrap();
        let err = load_cases(&corrupt).unwrap_err().to_string();
        assert!(err.contains("line 2"), "corrupt line is named: {err}");

        let missing = dir.path().join("nope.jsonl");
        assert!(load_cases(&missing).is_err(), "missing file errors");
    }
}
