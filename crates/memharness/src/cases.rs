//! Frozen query-case persistence (spec §3.0.2): save the built case list once, reload it on
//! every later rung run, so all Phase 1 gates compare IDENTICAL cases. Format = JSONL (one
//! `QueryCase` per line) + sha256 over the exact file bytes — the case-list identity the report
//! records and `compare` (spec §3.0.3) enforces before pairing runs.

use std::path::Path;

use sha2::{Digest, Sha256};

use crate::run::QueryCase;

/// Serialize cases as JSONL bytes (one JSON object per line). Field order = struct order, so
/// the same list always produces the same bytes (the sha below is a real identity).
fn cases_to_jsonl(cases: &[QueryCase]) -> anyhow::Result<Vec<u8>> {
    let mut out = Vec::new();
    for case in cases {
        serde_json::to_writer(&mut out, case)?;
        out.push(b'\n');
    }
    Ok(out)
}

/// sha256 hex over exact JSONL bytes — the case-list identity recorded in the report.
fn jsonl_sha(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// Save cases as JSONL (creating parent dirs); returns the sha of the bytes written. REFUSES
/// to overwrite an existing file: the frozen list is an identity-bearing artifact (every gate
/// pairs runs by its sha), so re-freezing must be deliberate — delete the old list or pick a
/// new path (spec §6 "re-freeze deliberately, never silently"; same discipline as
/// `report::write_report`'s overwrite refusal).
pub fn save_cases(path: &Path, cases: &[QueryCase]) -> anyhow::Result<String> {
    if path.exists() {
        anyhow::bail!(
            "frozen case list already exists at {} — refusing to overwrite an identity-bearing \
             artifact; re-freeze deliberately (delete it or save to a new path)",
            path.display()
        );
    }
    let bytes = cases_to_jsonl(cases)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, &bytes)?;
    Ok(jsonl_sha(&bytes))
}

/// Load a frozen case list; returns (cases, sha of the bytes read). A zero-case file fails loud
/// — a frozen set with nothing in it is always a mistake, never a valid measurement input.
pub fn load_cases(path: &Path) -> anyhow::Result<(Vec<QueryCase>, String)> {
    let bytes = std::fs::read(path)
        .map_err(|e| anyhow::anyhow!("reading frozen case list {}: {e}", path.display()))?;
    let sha = jsonl_sha(&bytes);
    let mut cases = Vec::new();
    for (i, line) in bytes.split(|b| *b == b'\n').enumerate() {
        if line.is_empty() {
            continue;
        }
        let case: QueryCase = serde_json::from_slice(line).map_err(|e| {
            anyhow::anyhow!("frozen case list {} line {}: {e}", path.display(), i + 1)
        })?;
        cases.push(case);
    }
    if cases.is_empty() {
        anyhow::bail!("frozen case list {} contains zero cases", path.display());
    }
    Ok((cases, sha))
}

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
        // Overwriting an existing frozen list is REFUSED — identity artifacts die loudly
        // (re-freeze deliberately: spec §6; mirrors write_report's overwrite refusal).
        let err = save_cases(&path, &sample()).unwrap_err().to_string();
        assert!(err.contains("refusing to overwrite"), "overwrite refused: {err}");
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
