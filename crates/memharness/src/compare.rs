//! Paired cross-run comparison (spec §3.0.3) — the rung gate's statistics. Joins two runs'
//! per-case AIR success flags by `case_idx`, REFUSING unless both runs carry the SAME frozen
//! case-list sha (different lists ⇒ unpaired noise, the exact flaw the freeze protocol exists
//! to kill). Per segment: paired Wilcoxon (candidate vs baseline) + s@k delta.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Context as _;

use crate::run::CaseResult;
use crate::stats::{wilcoxon_signed_rank, WilcoxonResult};

/// One segment's paired comparison row.
#[derive(Debug)]
pub struct SegmentComparison {
    pub label: String,
    pub n: usize,
    pub baseline_s_at_k: f64,
    pub candidate_s_at_k: f64,
    /// Paired Wilcoxon over per-case success flags, candidate vs baseline.
    pub wilcoxon: WilcoxonResult,
}

/// Pull (case_list_sha, corpus_sha, case_results) out of a scores.json string. Parses via
/// `Value` because `ReportModel` deliberately does not derive Deserialize — only these three
/// fields matter here.
fn extract_run(role: &str, scores_json: &str) -> anyhow::Result<(String, String, Vec<CaseResult>)> {
    let v: serde_json::Value = serde_json::from_str(scores_json)
        .with_context(|| format!("parsing {role} scores.json (pass scores.json, not report.md)"))?;
    let case_sha = v
        .get("case_list_sha")
        .and_then(|s| s.as_str())
        .map(str::to_string)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "{role} scores.json carries no case_list_sha — the run was not frozen \
                 (--cases/--save-cases); compare refuses unfrozen runs"
            )
        })?;
    let corpus_sha = v
        .get("corpus_sha")
        .and_then(|s| s.as_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("{role} scores.json has no corpus_sha (pre-rung-0 run?)"))?;
    let results: Vec<CaseResult> = serde_json::from_value(
        v.get("case_results").cloned().ok_or_else(|| {
            anyhow::anyhow!("{role} scores.json has no case_results (pre-rung-0 run?)")
        })?,
    )?;
    if results.is_empty() {
        anyhow::bail!("{role} case_results is empty — nothing to pair");
    }
    Ok((case_sha, corpus_sha, results))
}

/// Compare candidate vs baseline PAIRED by `case_idx` over an identical frozen case list AND
/// an identical corpus snapshot (spec §3.0.1: gates across snapshots are invalid — enforced
/// here in the tool, not left to a human reading two reports).
pub fn compare_runs(baseline: &str, candidate: &str) -> anyhow::Result<Vec<SegmentComparison>> {
    let (sha_b, corpus_b, base) = extract_run("baseline", baseline)?;
    let (sha_c, corpus_c, cand) = extract_run("candidate", candidate)?;
    if corpus_b != corpus_c {
        anyhow::bail!(
            "corpus snapshot mismatch ({corpus_b} vs {corpus_c}) — the runs ingested different \
             corpora; a gate across snapshots is invalid (spec §3.0.1)"
        );
    }
    if sha_b != sha_c {
        anyhow::bail!(
            "case-list sha mismatch ({sha_b} vs {sha_c}) — the runs used different frozen \
             lists; pairing them would be exactly the churn the freeze protocol exists to kill"
        );
    }
    if base.len() != cand.len() {
        anyhow::bail!("case_results length mismatch ({} vs {})", base.len(), cand.len());
    }
    let base_by_idx: BTreeMap<usize, &CaseResult> = base.iter().map(|c| (c.case_idx, c)).collect();
    // Duplicate-idx guards (quality review, Important): a hand-damaged scores.json with a
    // repeated case_idx would otherwise silently double-pair one baseline case and drop
    // another. With BOTH sides unique + equal lengths + every candidate idx found in baseline,
    // the pairing is a proven bijection — corruption fails loud, never mis-scores a gate.
    if base_by_idx.len() != base.len() {
        anyhow::bail!("baseline contains duplicate case_idx — corrupt scores.json");
    }
    let mut seen_cand_idx = BTreeSet::new();
    let mut by_label: BTreeMap<String, (Vec<f64>, Vec<f64>)> = BTreeMap::new();
    for c in &cand {
        if !seen_cand_idx.insert(c.case_idx) {
            anyhow::bail!(
                "case_idx {} appears twice in candidate — corrupt scores.json",
                c.case_idx
            );
        }
        let b = base_by_idx.get(&c.case_idx).ok_or_else(|| {
            anyhow::anyhow!("case_idx {} in candidate but not baseline — corrupt pairing", c.case_idx)
        })?;
        if b.label != c.label {
            anyhow::bail!("case_idx {} label mismatch ({} vs {})", c.case_idx, b.label, c.label);
        }
        let entry = by_label.entry(c.label.clone()).or_default();
        entry.0.push(if b.air_success { 1.0 } else { 0.0 });
        entry.1.push(if c.air_success { 1.0 } else { 0.0 });
    }
    Ok(by_label
        .into_iter()
        .map(|(label, (b, c))| {
            let n = b.len();
            SegmentComparison {
                label,
                n,
                baseline_s_at_k: b.iter().sum::<f64>() / n as f64,
                candidate_s_at_k: c.iter().sum::<f64>() / n as f64,
                // wilcoxon_signed_rank(a, b) tests diffs a[i]−b[i]: candidate first.
                wilcoxon: wilcoxon_signed_rank(&c, &b),
            }
        })
        .collect())
}

/// Significance threshold for the recall-regression guard. A drop must be significant at
/// p < [`REGRESSION_ALPHA`] to count as a regression (a statistical tie is not a regression);
/// mirrors the `wilcoxon.p_value < 0.05` convention the existing compare tests already assert.
const REGRESSION_ALPHA: f64 = 0.05;

/// The FIRST segment that significantly REGRESSED: candidate s@k STRICTLY below baseline AND the
/// paired Wilcoxon rejects "no difference" at p < [`REGRESSION_ALPHA`]. Returns `None` when every
/// segment improved or merely tied (no significant drop) — the recall-NEUTRALITY verdict.
///
/// This is the harness half of the Rung-3 §9/§13 recall-neutrality proof (the core `log.rs` golden
/// test is the by-construction half): a pre/post-Phase-1 `compare` over a FROZEN corpus asserts
/// `recall_regressed(&segments).is_none()` — building/using the passage conflict index and retiring
/// passages must not drop note recall on ANY segment. An IMPROVEMENT is never a regression, and a
/// non-significant dip is treated as noise, not a regression. Segments are scanned in their stable
/// (label-sorted, from `compare_runs`' `BTreeMap`) order, so the first gating regression is
/// deterministic.
pub fn recall_regressed(segments: &[SegmentComparison]) -> Option<&SegmentComparison> {
    segments
        .iter()
        .find(|s| s.candidate_s_at_k < s.baseline_s_at_k && s.wilcoxon.p_value < REGRESSION_ALPHA)
}

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

        // A duplicate case_idx (hand-damaged file) must fail loud — silently double-pairing one
        // baseline case while dropping another would mis-score a SHIP/NO-SHIP gate (quality
        // review, Important). Both sides guarded.
        let case = |idx: usize| {
            format!(
                r#"{{"case_idx":{idx},"label":"synthetic·en·known-item","air_rank":0,"gbrain_rank":null,"air_success":true,"gbrain_success":false}}"#
            )
        };
        let dup = format!(
            r#"{{"case_list_sha":"aaa","corpus_sha":"cc","case_results":[{},{}]}}"#,
            case(0),
            case(0)
        );
        let ok2 = format!(
            r#"{{"case_list_sha":"aaa","corpus_sha":"cc","case_results":[{},{}]}}"#,
            case(0),
            case(1)
        );
        assert!(
            compare_runs(&ok2, &dup).unwrap_err().to_string().contains("appears twice"),
            "duplicate candidate case_idx refused"
        );
        assert!(
            compare_runs(&dup, &ok2).unwrap_err().to_string().contains("duplicate case_idx"),
            "duplicate baseline case_idx refused"
        );
    }

    #[test]
    fn recall_regressed_flags_only_a_significant_drop() {
        // Build segments directly (mirrors the wilcoxon.p_value idiom of the tests above): a
        // segment carries a baseline/candidate s@k plus a paired Wilcoxon p.
        fn seg(label: &str, base: f64, cand: f64, p: f64) -> SegmentComparison {
            SegmentComparison {
                label: label.into(),
                n: 30,
                baseline_s_at_k: base,
                candidate_s_at_k: cand,
                wilcoxon: WilcoxonResult {
                    n_nonzero: 30,
                    w_statistic: 0.0,
                    p_value: p,
                    small_n_approx: false,
                },
            }
        }

        // A significant regression sits AFTER a clean improvement and a non-significant dip, so it
        // is selected by the PREDICATE, not by position: candidate down AND p < 0.05.
        let segs = vec![
            seg("a·improved", 0.50, 0.80, 0.001), // candidate UP → never a regression
            seg("b·noise", 0.80, 0.78, 0.40),     // down but NOT significant → a tie, not a regression
            seg("c·regressed", 0.80, 0.50, 0.001), // down AND significant → THE gating segment
        ];
        let hit = recall_regressed(&segs).expect("a significant drop is a regression");
        assert_eq!(hit.label, "c·regressed");

        // An all-clean set (an improvement + a non-significant dip) is recall-NEUTRAL → None.
        let clean = vec![seg("x·up", 0.40, 0.60, 0.01), seg("y·tie", 0.70, 0.69, 0.90)];
        assert!(recall_regressed(&clean).is_none(), "no significant drop ⇒ neutral ⇒ None");

        // Boundary: a drop exactly AT alpha (p == 0.05) is NOT significant (the guard uses strict <),
        // so it is not flagged — pins the comparison against a silent >= slip.
        let at_alpha = vec![seg("z·edge", 0.80, 0.60, REGRESSION_ALPHA)];
        assert!(recall_regressed(&at_alpha).is_none(), "p == alpha is not < alpha ⇒ not flagged");
    }
}
