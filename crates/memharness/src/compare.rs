//! Paired cross-run comparison (spec §3.0.3) — the rung gate's statistics. Joins two runs'
//! per-case AIR success flags by `case_idx`, REFUSING unless both runs carry the SAME frozen
//! case-list sha (different lists ⇒ unpaired noise, the exact flaw the freeze protocol exists
//! to kill). Per segment: paired Wilcoxon (candidate vs baseline) + s@k delta.

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal scores.json with `n` known-item cases; `successes` = how many are AIR-successes.
    fn scores(sha: Option<&str>, corpus: &str, n: usize, successes: usize, label: &str) -> String {
        let results: Vec<String> = (0..n)
            .map(|i| {
                format!(
                    r#"{{"case_idx":{i},"label":"{label}","air_rank":{rank},"gbrain_rank":null,"air_success":{success},"gbrain_success":false}}"#,
                    rank = if i < successes { "0" } else { "null" },
                    success = i < successes,
                )
            })
            .collect();
        format!(
            r#"{{"case_list_sha":{sha},"corpus_sha":"{corpus}","case_results":[{results}]}}"#,
            sha = sha.map_or("null".to_string(), |s| format!("\"{s}\"")),
            results = results.join(","),
        )
    }

    #[test]
    fn refuses_unfrozen_and_mismatched_runs() {
        let frozen = scores(Some("aaa"), "cc", 30, 10, "synthetic·en·known-item");
        let unfrozen = scores(None, "cc", 30, 10, "synthetic·en·known-item");
        let other_list = scores(Some("bbb"), "cc", 30, 10, "synthetic·en·known-item");
        let other_corpus = scores(Some("aaa"), "dd", 30, 10, "synthetic·en·known-item");
        assert!(
            compare_runs(&unfrozen, &frozen).unwrap_err().to_string().contains("not frozen"),
            "unfrozen baseline refused"
        );
        assert!(
            compare_runs(&frozen, &other_list).unwrap_err().to_string().contains("sha mismatch"),
            "different frozen lists refused"
        );
        assert!(
            compare_runs(&frozen, &other_corpus)
                .unwrap_err()
                .to_string()
                .contains("corpus snapshot mismatch"),
            "different corpus snapshots refused (spec §3.0.1 — gates across snapshots are invalid)"
        );
    }

    #[test]
    fn paired_improvement_is_detected_per_segment() {
        // Same frozen list + corpus; candidate turns 10/30 successes into 20/30 (10 new wins).
        let baseline = scores(Some("aaa"), "cc", 30, 10, "synthetic·en·known-item");
        let candidate = scores(Some("aaa"), "cc", 30, 20, "synthetic·en·known-item");
        let rows = compare_runs(&baseline, &candidate).unwrap();
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.label, "synthetic·en·known-item");
        assert_eq!(row.n, 30);
        assert!((row.baseline_s_at_k - 10.0 / 30.0).abs() < 1e-9);
        assert!((row.candidate_s_at_k - 20.0 / 30.0).abs() < 1e-9);
        assert!(
            row.wilcoxon.p_value < 0.05,
            "10 concordant improvements, 0 regressions must be significant: p={}",
            row.wilcoxon.p_value
        );
        // Identical runs: delta 0; the paired test must NOT claim significance.
        let same = compare_runs(&baseline, &baseline).unwrap();
        assert!((same[0].candidate_s_at_k - same[0].baseline_s_at_k).abs() < 1e-9);
        assert!(same[0].wilcoxon.p_value > 0.5, "all-tie pairing is insignificant");
    }

    #[test]
    fn corrupt_pairings_fail_loud() {
        let baseline = scores(Some("aaa"), "cc", 3, 1, "synthetic·en·known-item");
        let candidate = scores(Some("aaa"), "cc", 2, 1, "synthetic·en·known-item");
        assert!(
            compare_runs(&baseline, &candidate).unwrap_err().to_string().contains("length mismatch"),
        );
        let relabeled = scores(Some("aaa"), "cc", 3, 1, "synthetic·ko·known-item");
        assert!(
            compare_runs(&baseline, &relabeled).unwrap_err().to_string().contains("label mismatch"),
        );
    }
}
