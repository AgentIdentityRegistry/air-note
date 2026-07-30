//! The Rung-4 R4-A reflection non-regression gate (spec §5.2/§5.3). Drives BOTH loops to quiescence over the
//! frozen corpus + a seeded retire + a frozen synthetic miss set, scores both arms, and gates with
//! `recall_regressed`. Union-coverage is REPORTED only, never gated (critic New-Blocker-1). Dev-only.

/// A hard cap on evolve+reflect drive cycles; non-convergence is a FAIL-LOUD error, never a silent cap.
/// Headroom assumption (OQ5 lock): each cycle attempts ≤ REFLECT_MISSES_PER_TICK(4) misses +
/// REFLECT_REFRESH_PER_TICK(4) refreshes, so 64 cycles ≈ 256 miss-attempts against a frozen synthetic
/// miss set of ≤ 20 (each miss terminal within ≤ REFLECT_MISS_ATTEMPT_BUDGET(3) attempts) — an order of
/// magnitude of slack; hitting the cap therefore indicates a REAL non-convergence bug, not a tight bound.
pub const MAX_QUIESCENCE_CYCLES: usize = 64;

/// Drive evolve + reflect to quiescence (both queues drained) on an ALREADY-INGESTED, evolve+reflect-ENABLED
/// brain. Returns the cycle count, or an error if it did not converge within `MAX_QUIESCENCE_CYCLES`.
/// `tick_evolve` / `tick_reflect` are injected so hermetic tests can drive doubles and the live path drives
/// the real `EngineHandle::{evolve_once, reflect_once}`.
pub fn drive_to_quiescence(
    mut tick_evolve: impl FnMut() -> anyhow::Result<usize>, // returns remaining evolve queue depth
    mut tick_reflect: impl FnMut() -> anyhow::Result<usize>, // returns open-miss count after the tick
) -> anyhow::Result<usize> {
    let mut prev_evolve_left: Option<usize> = None;
    for cycle in 1..=MAX_QUIESCENCE_CYCLES {
        let evolve_left = tick_evolve()?;
        let misses_left = tick_reflect()?;
        // Per-cycle trace (stderr): a stalled queue must NAME itself — the first live run bailed at
        // the cap with no way to tell WHICH loop stuck, at what depth, or whether ticks did work.
        eprintln!(
            "memharness: reflect-gate — cycle {cycle}/{MAX_QUIESCENCE_CYCLES}: \
             evolve_left={evolve_left} misses_left={misses_left}"
        );
        // Quiescence = reflect drained AND evolve at a FIXED POINT (no progress this cycle).
        // `evolve_left == 0` was the naive bound and can NEVER converge on a file-heavy corpus:
        // `queue_depth` counts `memory` + `file_ingested` events (log.rs `evolve_status`), but M4a
        // extraction processes `memory` events ONLY and never advances its cursor past deferred
        // file items (extract.rs top-of-module) — the depth is a permanent floor there, not a
        // backlog. The fixed point converges honestly on both corpus shapes, and the per-cycle
        // trace keeps a genuinely-stuck queue visible instead of hidden.
        if misses_left == 0 && prev_evolve_left == Some(evolve_left) {
            return Ok(cycle);
        }
        prev_evolve_left = Some(evolve_left);
    }
    anyhow::bail!(
        "reflect gate: evolve+reflect did not reach quiescence in {MAX_QUIESCENCE_CYCLES} cycles — refusing \
         to score a non-converged pass (a bounded loop must terminate, not spin nights)"
    )
}

/// One case's union-coverage inputs, precomputed by the driver (OQ2 unblocked this: the recall wire `Hit`
/// carries no cites, so the driver reads each top-k dossier hit's page event via
/// `daemon.engine().get_or_open(true)` → `log.stream_all()` → `model_meta.source_event_ids`, maps every
/// cited FILE id through the SAME `PageResolver` (non-file cites are skipped), and records whether any
/// resolved cite equals this case's `gold_page_id`).
pub struct UnionCase {
    /// Whether the gold FILE page itself made top-k (from a fresh re-retrieval over the reflected
    /// brain — the scored `CaseResult`s don't expose per-case ranks, so union-coverage re-recalls).
    pub gold_in_topk: bool,
    /// The best (lowest) rank of a top-k dossier hit whose cites resolve to this case's gold, if any.
    pub best_citing_dossier_rank: Option<usize>,
}

/// Union-coverage (REPORTED, never gated — the §5.3(b) demotion): among cases whose gold FILE page is NOT
/// in top-k, how many have a top-k dossier hit CITING the gold, and at what ranks. Informs the future
/// dossier-primacy decision honestly; NEVER feeds `recall_regressed`. PURE over driver-precomputed inputs.
pub struct UnionCoverageReport {
    pub gold_missing_topk: usize,
    pub covered_by_citing_dossier: usize,
    pub covering_ranks: Vec<usize>,
}

pub fn union_coverage(cases: &[UnionCase]) -> UnionCoverageReport {
    let mut r = UnionCoverageReport {
        gold_missing_topk: 0,
        covered_by_citing_dossier: 0,
        covering_ranks: Vec::new(),
    };
    for c in cases {
        if c.gold_in_topk {
            continue; // the gate already credits gold-as-itself; union-coverage reports only the gap cases
        }
        r.gold_missing_topk += 1;
        if let Some(rank) = c.best_citing_dossier_rank {
            r.covered_by_citing_dossier += 1;
            r.covering_ranks.push(rank);
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn union_coverage_counts_only_gap_cases_and_their_citing_dossiers() {
        let cases = [
            UnionCase { gold_in_topk: true, best_citing_dossier_rank: Some(1) }, // gate case — ignored here
            UnionCase { gold_in_topk: false, best_citing_dossier_rank: Some(3) }, // covered gap
            UnionCase { gold_in_topk: false, best_citing_dossier_rank: None },    // uncovered gap
        ];
        let r = union_coverage(&cases);
        assert_eq!(r.gold_missing_topk, 2, "only gold-missing cases enter the metric");
        assert_eq!(r.covered_by_citing_dossier, 1);
        assert_eq!(r.covering_ranks, vec![3]);
    }

    #[test]
    fn drive_to_quiescence_converges_and_fails_loud_on_a_spinner() {
        // Converges: evolve drains in 2 cycles, misses in 3 → quiescent at cycle 3.
        let (mut e, mut m) = (2usize, 3usize);
        let cycles = drive_to_quiescence(
            || { e = e.saturating_sub(1); Ok(e) },
            || { m = m.saturating_sub(1); Ok(m) },
        )
        .unwrap();
        assert_eq!(cycles, 3);
        // Converges on a PERMANENT FLOOR (the file-heavy-corpus reality that broke the first live
        // run): queue_depth counts deferred `file_ingested` items evolve never processes — a
        // STABLE nonzero depth with reflect drained IS quiescence under fixed-point semantics.
        let mut m2 = 3usize;
        let cycles = drive_to_quiescence(|| Ok(880), || {
            m2 = m2.saturating_sub(1);
            Ok(m2)
        })
        .unwrap();
        assert_eq!(cycles, 3, "floor stable since cycle 1; misses drain at cycle 3");
        // Misses that never drain fail LOUD at the cap, never silently score.
        let err = drive_to_quiescence(|| Ok(0), || Ok(1)).unwrap_err();
        assert!(err.to_string().contains("did not reach quiescence"), "loud non-convergence: {err}");
        // An evolve queue that never STABILIZES (churn, not a floor) also fails loud.
        let mut flip = false;
        let err = drive_to_quiescence(
            || {
                flip = !flip;
                Ok(if flip { 5 } else { 6 })
            },
            || Ok(0),
        )
        .unwrap_err();
        assert!(err.to_string().contains("did not reach quiescence"), "churn is not quiescence: {err}");
    }
}
