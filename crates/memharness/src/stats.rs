//! Pure scoring + statistics: success@k, MRR, bootstrap CIs, Wilcoxon signed-rank. No numeric
//! deps — hand-rolled + unit-tested against independently computed reference values.

/// The 0-based rank of the gold page in the (page-deduped) retrieved list; `None` = missed.
pub type GoldRank = Option<usize>;

/// success@k: gold at 0-based rank < k. NOTE: k here is the SAME `--k` used for retrieval
/// (retrieval-k == scoring-k, spec §4 Rev 2 — one knob).
pub fn success_at_k(rank: &GoldRank, k: usize) -> bool {
    matches!(rank, Some(r) if *r < k)
}

/// Reciprocal rank: 1/(rank+1), or 0 if missed.
pub fn mrr_of(rank: &GoldRank) -> f64 {
    match rank {
        Some(r) => 1.0 / (*r as f64 + 1.0),
        None => 0.0,
    }
}

/// Mean success@k over many queries.
pub fn mean_success_at_k(ranks: &[GoldRank], k: usize) -> f64 {
    if ranks.is_empty() {
        return 0.0;
    }
    ranks.iter().filter(|r| success_at_k(r, k)).count() as f64 / ranks.len() as f64
}

/// Mean reciprocal rank over many queries.
pub fn mean_reciprocal_rank(ranks: &[GoldRank]) -> f64 {
    if ranks.is_empty() {
        return 0.0;
    }
    ranks.iter().map(mrr_of).sum::<f64>() / ranks.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_at_k_and_mrr_known_values() {
        let ranks = vec![Some(0usize)];
        assert!(success_at_k(&ranks[0], 5));
        assert!((mrr_of(&ranks[0]) - 1.0).abs() < 1e-9);
        let r = Some(2usize);
        assert!(success_at_k(&r, 5));
        assert!(!success_at_k(&r, 2));
        assert!((mrr_of(&r) - (1.0 / 3.0)).abs() < 1e-9);
        let none: Option<usize> = None;
        assert!(!success_at_k(&none, 10));
        assert!((mrr_of(&none) - 0.0).abs() < 1e-9);
    }

    #[test]
    fn mean_success_at_k_over_many() {
        let ranks = vec![Some(0), Some(4), None, Some(1)];
        assert!((mean_success_at_k(&ranks, 5) - 0.75).abs() < 1e-9);
    }
}
