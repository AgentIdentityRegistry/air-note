//! §5.3(d)+(e) evidence probes — REPORTED, never SHIP-gated in R4-A. PURE scoring over
//! driver-precomputed inputs; the live driver (Peter-gated) retrieves/composes via `daemon.engine()`.

use crate::arms::{dedup_by_page, gold_rank, RetrievedHit};
use crate::judge::{assign_blind, judge_pair_blind, trust_verdict, PairJudge, TrustVerdict, Verdict,
    AUDIT_FLOOR};
use rand::SeedableRng;

/// §5.3(d) pre-registered bands (OQ5 LOCK, recorded 2026-07-19 at plan review, per §5.5): the reflected
/// arm's success@k lift over baseline on the DISJOINT paraphrase set B. `≥ +5pp` = promising; `≤ 0pp` =
/// kill. REPORTED kill-criterion — NOT a CI gate and NOT a tuning knob (critic guard): the agreed
/// value + date live here and at the §5.5 pre-registration; LOWERING the kill bar after a failed dogfood
/// requires a written rationale in the design changelog. A kill bar you can quietly lower is not a kill bar.
pub const HELD_OUT_LIFT_PROMISING_PP: f64 = 5.0;
pub const HELD_OUT_LIFT_KILL_PP: f64 = 0.0;

/// Success@k over per-case `(gold_page_id, retrieved hits)` — dedup-by-page then rank, the run.rs scoring
/// path's exact semantics. PURE.
pub fn score_success_at_k(cases: &[(String, Vec<RetrievedHit>)], k: usize) -> f64 {
    if cases.is_empty() {
        return 0.0;
    }
    let hits = cases
        .iter()
        .filter(|(gold, hs)| gold_rank(&dedup_by_page(hs.clone()), gold).is_some_and(|r| r < k))
        .count();
    hits as f64 / cases.len() as f64
}

/// The (d) report: baseline vs reflected success@k on set B + the band verdict (REPORTED only).
pub struct HeldOutReport {
    pub baseline: f64,
    pub reflected: f64,
    pub lift_pp: f64,
    pub verdict: &'static str,
}

pub fn held_out_report(baseline: f64, reflected: f64) -> HeldOutReport {
    let lift_pp = (reflected - baseline) * 100.0;
    let verdict = if lift_pp >= HELD_OUT_LIFT_PROMISING_PP {
        "promising"
    } else if lift_pp <= HELD_OUT_LIFT_KILL_PP {
        "kill"
    } else {
        "inconclusive"
    };
    HeldOutReport { baseline, reflected, lift_pp, verdict }
}

/// One (e) A/B item: the SAME question answered twice — once composed from the topic's DOSSIER page,
/// once from its RAW CITED memories (the driver composes both through the same answerer).
pub struct AbItem {
    pub query: String,
    pub dossier_answer: String,
    pub source_answer: String,
}

/// The (e) outcome. `lift_pp` is `Some` ONLY when the judge cleared the Phase-0 trust ladder
/// (`TRUST_AGREEMENT_MIN`/`TRUST_KAPPA_MIN`, judge.rs:68-69) — otherwise `None` and the report prints
/// the lift as UNINTERPRETABLE (the honest tokens are `verdict.audit_incomplete` / `!verdict.trusted`).
pub struct AbOutcome {
    pub verdict: TrustVerdict,
    pub dossier_wins: usize,
    pub source_wins: usize,
    pub ties: usize,
    pub uncertain: usize,
    pub lift_pp: Option<f64>,
}

/// §5.3(e): blind position-swapped dossier-vs-source judging, reusing `judge_pair_blind` VERBATIM — the
/// "air" slot carries the DOSSIER answer and the "gbrain" slot the SOURCE answer (a slot reuse of the
/// arm-named machinery: `Verdict::AirWins` reads "dossier wins" here). AUDIT: the auditor re-judges the
/// FULL item set — the open-case pool sits at/below `AUDIT_FLOOR`, where the ladder audits in full and
/// flags `audit_n_too_small` (indicative-only), so no sampler dependency — then `trust_verdict` computes
/// agreement + κ. No auditor → `audit_incomplete` → untrusted → the lift is withheld, never fabricated.
pub fn dossier_vs_source_ab(
    items: &[AbItem],
    judge: &dyn PairJudge,
    auditor: Option<&dyn PairJudge>,
    seed: u64,
) -> anyhow::Result<AbOutcome> {
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let mut locals = Vec::with_capacity(items.len());
    for it in items {
        let blind = assign_blind(&mut rng);
        locals.push(judge_pair_blind(judge, blind, &it.query, &it.dossier_answer, &it.source_answer)?);
    }
    let verdict = match auditor {
        None => trust_verdict(&[], &[], false, true, false),
        Some(aud) => {
            let mut rng2 = rand::rngs::StdRng::seed_from_u64(seed ^ 0x5eed);
            let mut clouds = Vec::with_capacity(items.len());
            for it in items {
                let blind = assign_blind(&mut rng2);
                clouds.push(judge_pair_blind(aud, blind, &it.query, &it.dossier_answer, &it.source_answer)?);
            }
            trust_verdict(&locals, &clouds, false, false, items.len() < AUDIT_FLOOR)
        }
    };
    let (mut dossier_wins, mut source_wins, mut ties, mut uncertain) = (0usize, 0usize, 0usize, 0usize);
    for v in &locals {
        match v {
            Verdict::AirWins => dossier_wins += 1,
            Verdict::GbrainWins => source_wins += 1,
            Verdict::Tie => ties += 1,
            Verdict::Uncertain => uncertain += 1,
        }
    }
    let lift_pp = if verdict.trusted && !items.is_empty() {
        Some((dossier_wins as f64 - source_wins as f64) / items.len() as f64 * 100.0)
    } else {
        None // UNINTERPRETABLE — the report prints the verdict tokens, not a lift number
    };
    Ok(AbOutcome { verdict, dossier_wins, source_wins, ties, uncertain, lift_pp })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arms::RetrievedHit;
    use crate::judge::{PairJudge, PosPick};

    #[test]
    fn held_out_scoring_and_verdict_bands() {
        let hit = |gold: &str| {
            (gold.to_string(), vec![RetrievedHit { page_id: gold.to_string(), snippet: String::new() }])
        };
        let miss = |gold: &str| {
            (gold.to_string(), vec![RetrievedHit { page_id: "other/page".into(), snippet: String::new() }])
        };
        // Pure success@k over deduped hits: 1/2 then 2/2.
        assert_eq!(score_success_at_k(&[hit("g1"), miss("g2")], 5), 0.5);
        assert_eq!(score_success_at_k(&[hit("g1"), hit("g2")], 5), 1.0);
        // The pre-registered bands (OQ5 lock): ≥ +5pp promising, ≤ 0pp kill, else inconclusive. REPORTED,
        // never a CI assert on live numbers — these asserts pin the BAND ARITHMETIC, not any real result.
        assert_eq!(held_out_report(0.50, 0.56).verdict, "promising");
        assert_eq!(held_out_report(0.50, 0.50).verdict, "kill");
        assert_eq!(held_out_report(0.50, 0.53).verdict, "inconclusive");
    }

    /// Content-based judge double (the `GoodJudge` idiom, judge.rs:380): prefers the answer containing
    /// "DOSSIER" — it judges CONTENT, not position, so it survives blinding + swapping.
    struct DossierLover;
    impl PairJudge for DossierLover {
        fn pick(&self, _q: &str, a: &str, b: &str) -> anyhow::Result<Option<PosPick>> {
            Ok(match (a.contains("DOSSIER"), b.contains("DOSSIER")) {
                (true, false) => Some(PosPick::A),
                (false, true) => Some(PosPick::B),
                _ => Some(PosPick::Tie),
            })
        }
    }
    /// Ambiguity double (the `MumbleJudge` idiom, judge.rs:392): always None → every pair `Uncertain`.
    struct Mumble;
    impl PairJudge for Mumble {
        fn pick(&self, _q: &str, _a: &str, _b: &str) -> anyhow::Result<Option<PosPick>> {
            Ok(None)
        }
    }

    /// 3 dossier-better + 1 source-better items → MIXED verdict categories, so Cohen's κ over the
    /// perfectly-agreeing audit is well-defined (an all-one-category audit makes κ degenerate).
    fn mixed_items() -> Vec<AbItem> {
        let mut v: Vec<AbItem> = (0..3)
            .map(|i| AbItem {
                query: format!("q{i}"),
                dossier_answer: "DOSSIER grounded answer".into(),
                source_answer: "raw source answer".into(),
            })
            .collect();
        v.push(AbItem {
            query: "q3".into(),
            dossier_answer: "thin dossier answer".into(),
            source_answer: "DOSSIER-quality raw source answer".into(),
        });
        v
    }

    #[test]
    fn ab_lift_is_reported_only_under_a_trusted_judge() {
        // Agreeing judge + auditor over mixed outcomes → agreement 1.0, κ = 1.0 → trusted → Some(lift).
        let out = dossier_vs_source_ab(&mixed_items(), &DossierLover, Some(&DossierLover), 7).unwrap();
        assert!(out.verdict.trusted, "identical local/cloud verdicts clear ≥85% / κ≥0.6");
        assert_eq!((out.dossier_wins, out.source_wins, out.ties, out.uncertain), (3, 1, 0, 0));
        assert_eq!(out.lift_pp, Some(50.0), "(3-1)/4 → +50pp; reported as evidence ONLY because trusted");
        assert!(out.verdict.audit_n_too_small, "4 < AUDIT_FLOOR(30) → indicative-only flag carried");

        // NO auditor → audit_incomplete → UNTRUSTED → the lift is WITHHELD (printed UNINTERPRETABLE).
        let out = dossier_vs_source_ab(&mixed_items(), &DossierLover, None, 7).unwrap();
        assert!(!out.verdict.trusted);
        assert!(out.verdict.audit_incomplete, "no cloud audit → verdict UNAVAILABLE, never fabricated");
        assert_eq!(out.lift_pp, None, "untrusted judge → no lift claim (spec §5.3(e))");

        // A mumbling judge → all Uncertain → zero agreement with the auditor → untrusted, lift withheld.
        let out = dossier_vs_source_ab(&mixed_items(), &Mumble, Some(&DossierLover), 7).unwrap();
        assert_eq!(out.uncertain, 4);
        assert!(!out.verdict.trusted);
        assert_eq!(out.lift_pp, None);
    }
}
