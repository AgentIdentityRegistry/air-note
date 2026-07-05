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
/// Inputs must be finite; NaN values would otherwise be scored arbitrarily. `conf` must lie in
/// (0,1) — outside it the percentile bounds invert.
pub fn bootstrap_ci_mean<R: Rng>(
    data: &[f64],
    iters: usize,
    conf: f64,
    rng: &mut R,
) -> (f64, f64) {
    debug_assert!(data.iter().all(|v| v.is_finite()), "bootstrap inputs must be finite");
    debug_assert!(conf > 0.0 && conf < 1.0, "conf must be in (0,1)");
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

/// The result of a Wilcoxon signed-rank test on paired samples.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct WilcoxonResult {
    /// Non-zero pairwise differences (zeros dropped — the standard 'wilcox' convention).
    pub n_nonzero: usize,
    /// W = min(sum of positive ranks, sum of negative ranks).
    pub w_statistic: f64,
    /// Two-sided p via the tie-corrected normal approximation with continuity correction.
    /// n_nonzero == 0 → 1.0 (no evidence of a difference).
    pub p_value: f64,
    /// True when n_nonzero < 25: the normal approximation is unreliable there. Phase 0 REPORTS
    /// this flag (spec §5 honesty) instead of shipping the exact table. On binary-flag data
    /// (known-item segments) the test is additionally tie-heavy — the report carries that
    /// caveat too (spec §8 Rev 2).
    pub small_n_approx: bool,
}

/// Wilcoxon signed-rank on paired `(air[i], gbrain[i])`: rank |non-zero diffs| (average ranks
/// for ties), sum ranks by sign, two-sided p via the normal approximation with tie correction
/// (−Σ(t³−t)/2 in the variance) + continuity correction.
/// Inputs must be finite; NaN pairs would otherwise be scored arbitrarily.
///
/// # Panics
/// Panics when `air` and `gbrain` differ in length (the samples are PAIRED).
pub fn wilcoxon_signed_rank(air: &[f64], gbrain: &[f64]) -> WilcoxonResult {
    assert_eq!(air.len(), gbrain.len(), "paired samples must be equal length");
    debug_assert!(
        air.iter().chain(gbrain.iter()).all(|v| v.is_finite()),
        "wilcoxon inputs must be finite"
    );
    let diffs: Vec<f64> = air
        .iter()
        .zip(gbrain.iter())
        .map(|(a, g)| a - g)
        .filter(|d| *d != 0.0)
        .collect();
    let n = diffs.len();
    if n == 0 {
        return WilcoxonResult { n_nonzero: 0, w_statistic: 0.0, p_value: 1.0, small_n_approx: true };
    }
    let mut idx: Vec<usize> = (0..n).collect();
    idx.sort_by(|&i, &j| {
        diffs[i].abs().partial_cmp(&diffs[j].abs()).unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut ranks = vec![0.0f64; n];
    let mut tie_term = 0.0f64;
    let mut i = 0;
    while i < n {
        let mut j = i;
        while j + 1 < n && diffs[idx[j + 1]].abs() == diffs[idx[i]].abs() {
            j += 1;
        }
        let group_len = j - i + 1;
        let avg_rank = ((i + 1) as f64 + (j + 1) as f64) / 2.0;
        for k in i..=j {
            ranks[idx[k]] = avg_rank;
        }
        if group_len > 1 {
            let t = group_len as f64;
            tie_term += t * t * t - t;
        }
        i = j + 1;
    }
    let mut sum_pos = 0.0f64;
    let mut sum_neg = 0.0f64;
    for k in 0..n {
        if diffs[k] > 0.0 {
            sum_pos += ranks[k];
        } else {
            sum_neg += ranks[k];
        }
    }
    let w = sum_pos.min(sum_neg);
    let nf = n as f64;
    let mean_w = nf * (nf + 1.0) / 4.0;
    let var_w = (nf * (nf + 1.0) * (2.0 * nf + 1.0) - tie_term / 2.0) / 24.0;
    let p_value = if var_w <= 0.0 {
        1.0
    } else {
        let z = (w - mean_w + 0.5).abs() / var_w.sqrt();
        two_sided_normal_p(z)
    };
    WilcoxonResult { n_nonzero: n, w_statistic: w, p_value, small_n_approx: n < 25 }
}

/// Two-sided p for |z| under the standard normal — Zelen & Severo (1964) tail approximation
/// (~1e-7 accurate; verified against exact erfc at z ∈ {0.6285, 1.959964, 2.087}).
fn two_sided_normal_p(z: f64) -> f64 {
    let z = z.abs();
    let t = 1.0 / (1.0 + 0.2316419 * z);
    let d = 0.398942280401433 * (-z * z / 2.0).exp();
    let upper_tail = d
        * t
        * (0.319381530
            + t * (-0.356563782 + t * (1.781477937 + t * (-1.821255978 + t * 1.330274429))));
    (2.0 * upper_tail).min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_at_k_and_mrr_known_values() {
        let ranks = [Some(0usize)];
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

    #[test]
    fn wilcoxon_signed_rank_known_values() {
        // Clean separation: differences [1,2,3,4,5] all positive → W = min(15, 0) = 0.
        let air = vec![2.0, 3.0, 4.0, 5.0, 6.0];
        let gbrain = vec![1.0, 1.0, 1.0, 1.0, 1.0];
        let res = wilcoxon_signed_rank(&air, &gbrain);
        assert_eq!(res.n_nonzero, 5);
        assert!((res.w_statistic - 0.0).abs() < 1e-9);
        assert!(res.p_value < 0.1, "p={}", res.p_value);

        // Identical vectors → all zero diffs dropped → n 0, p 1.
        let same = vec![1.0, 2.0, 3.0];
        let res0 = wilcoxon_signed_rank(&same, &same);
        assert_eq!(res0.n_nonzero, 0);
        assert!((res0.p_value - 1.0).abs() < 1e-9);
    }

    #[test]
    fn wilcoxon_tie_heavy_binary_flags() {
        // KNOWN-ITEM reality (Rev 2): paired binary success flags — every non-zero diff is ±1,
        // one big tie group. Reference values computed INDEPENDENTLY in Python (exact math.erfc
        // normal CDF; same tie-corrected variance + continuity correction; matches the
        // scipy.stats.wilcoxon(zero_method='wilcox', correction=True, method='approx') convention).
        //
        // A: five +1 diffs, five zeros → n=5, ranks all tie at 3.0, W=min(15,0)=0,
        //    var = (5·6·11 − 120/2)/24 = 11.25, z = |0−7.5+0.5|/√11.25 = 2.08700, p ≈ 0.036888.
        let air = vec![1.0, 1.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0, 1.0, 1.0];
        let gbrain = vec![0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0];
        let res = wilcoxon_signed_rank(&air, &gbrain);
        assert_eq!(res.n_nonzero, 5);
        assert!((res.w_statistic - 0.0).abs() < 1e-9);
        assert!((res.p_value - 0.036888).abs() < 1e-4, "p={}", res.p_value);
        assert!(res.small_n_approx, "n=5 < 25 → approximation flagged");

        // B: mixed signs, heavy ties — five +1, three −1, two zeros → n=8, ranks all 4.5,
        //    W = min(22.5, 13.5) = 13.5, var = (8·9·17 − 504/2)/24 = 40.5,
        //    z = |13.5−18+0.5|/√40.5 = 0.62854, p ≈ 0.529651.
        let air_b = vec![1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 0.0];
        let gbrain_b = vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 0.0, 1.0];
        let res_b = wilcoxon_signed_rank(&air_b, &gbrain_b);
        assert_eq!(res_b.n_nonzero, 8);
        assert!((res_b.w_statistic - 13.5).abs() < 1e-9);
        assert!((res_b.p_value - 0.529651).abs() < 1e-4, "p={}", res_b.p_value);
    }

    #[test]
    fn small_n_approx_boundary_at_25_nonzero_diffs() {
        // The false side of `small_n_approx` (n_nonzero == 25, the first "reliable" size):
        // 25 pairs, air each exactly 1.0 higher → 25 nonzero diffs.
        let gbrain: Vec<f64> = (0..25).map(|i| i as f64).collect();
        let air: Vec<f64> = gbrain.iter().map(|g| g + 1.0).collect();
        let res = wilcoxon_signed_rank(&air, &gbrain);
        assert_eq!(res.n_nonzero, 25);
        assert!(!res.small_n_approx, "n=25 → approximation considered reliable");
        // One below the boundary → flagged.
        let res24 = wilcoxon_signed_rank(&air[..24], &gbrain[..24]);
        assert_eq!(res24.n_nonzero, 24);
        assert!(res24.small_n_approx, "n=24 < 25 → approximation flagged");
    }

    #[test]
    fn bootstrap_zero_iters_returns_zero_bounds() {
        use rand::SeedableRng;
        use rand_chacha::ChaCha8Rng;
        let mut rng = ChaCha8Rng::seed_from_u64(42);
        // iters == 0 with NON-empty data → the guard returns (0,0) instead of indexing an
        // empty means vector.
        assert_eq!(bootstrap_ci_mean(&[1.0], 0, 0.95, &mut rng), (0.0, 0.0));
    }

    #[test]
    fn normal_p_sanity_at_the_5_percent_point() {
        // The classic two-sided 5% z: p(1.959964) must be ≈ 0.05 (Zelen–Severo ≈ exact to ~1e-7).
        assert!((two_sided_normal_p(1.959964) - 0.05).abs() < 1e-4);
    }
}
