//! Pure scoring + statistics: success@k, MRR, bootstrap CIs, Wilcoxon signed-rank. No numeric
//! deps — hand-rolled + unit-tested against independently computed reference values.

use rand::Rng;

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

/// Percentile bootstrap CI for the mean at confidence `conf`, `iters` resamples from a SEEDED
/// rng (determinism, spec §8). Empty data → (0.0, 0.0).
pub fn bootstrap_ci_mean<R: Rng>(
    data: &[f64],
    iters: usize,
    conf: f64,
    rng: &mut R,
) -> (f64, f64) {
    if data.is_empty() || iters == 0 {
        return (0.0, 0.0);
    }
    let n = data.len();
    let mut means: Vec<f64> = Vec::with_capacity(iters);
    for _ in 0..iters {
        let mut sum = 0.0;
        for _ in 0..n {
            sum += data[rng.gen_range(0..n)]; // resample WITH replacement
        }
        means.push(sum / n as f64);
    }
    means.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let alpha = (1.0 - conf) / 2.0;
    let low_idx = (alpha * iters as f64).floor() as usize;
    let high_idx = (((1.0 - alpha) * iters as f64).ceil() as usize).saturating_sub(1);
    (means[low_idx.min(iters - 1)], means[high_idx.min(iters - 1)])
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

    #[test]
    fn bootstrap_ci_is_deterministic_and_brackets_mean() {
        use rand::SeedableRng;
        use rand_chacha::ChaCha8Rng;
        let data: Vec<f64> = vec![0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0];
        let mut rng1 = ChaCha8Rng::seed_from_u64(42);
        let mut rng2 = ChaCha8Rng::seed_from_u64(42);
        let ci_a = bootstrap_ci_mean(&data, 1000, 0.95, &mut rng1);
        let ci_b = bootstrap_ci_mean(&data, 1000, 0.95, &mut rng2);
        assert_eq!(ci_a, ci_b, "same seed → identical CI");
        assert!(ci_a.0 <= 0.5 && 0.5 <= ci_a.1, "CI {ci_a:?} brackets the true mean 0.5");
        assert_eq!(bootstrap_ci_mean(&[], 1000, 0.95, &mut rng1), (0.0, 0.0), "empty → (0,0)");
    }
}
