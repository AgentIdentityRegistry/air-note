//! Phase-0 grader: run the bossclaw-core conflict judge over a FROZEN labelled set
//! and report catch-rate + cry-wolf-rate + a precision CI lower bound vs the §9 gate.

use bossclaw_core::conflict::Winner;

/// Ground-truth label for a pair. `Contradicts` = a real conflict (with a `winner`);
/// `Coexist` = looks similar but is legitimately both-true; `Unrelated` = a distractor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PairLabel {
    Contradicts,
    Coexist,
    Unrelated,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct LabelledPair {
    pub a: String,
    pub b: String,
    pub label: PairLabel,
    #[serde(default)]
    pub winner: Option<Winner>,
}

/// Parse a JSONL body of labelled pairs. Fails loud on an empty set (mirrors
/// `cases.rs`'s zero-case guard — a silent 0-case grade is a false "pass").
pub fn parse_pairs(body: &str) -> anyhow::Result<Vec<LabelledPair>> {
    let pairs: Vec<LabelledPair> = body
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str::<LabelledPair>(l).map_err(anyhow::Error::from))
        .collect::<anyhow::Result<_>>()?;
    anyhow::ensure!(!pairs.is_empty(), "no labelled conflict pairs found");
    Ok(pairs)
}

/// One graded pair: did the judge flag it (Some verdict), and was it truly a conflict?
#[derive(Debug, Clone, Copy)]
pub struct Outcome {
    pub flagged: bool,
    pub truly_contradicts: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Metrics {
    pub true_positives: usize,
    pub false_positives: usize,
    pub false_negatives: usize,
    pub true_negatives: usize,
    pub recall: f64,        // catch rate = TP / (TP + FN)
    pub precision: f64,     // TP / (TP + FP)
    pub cry_wolf_rate: f64, // 1 - precision
}

impl Metrics {
    pub fn from_outcomes(o: &[Outcome]) -> Self {
        let (mut tp, mut fp, mut fn_, mut tn) = (0, 0, 0, 0);
        for x in o {
            match (x.flagged, x.truly_contradicts) {
                (true, true) => tp += 1,
                (true, false) => fp += 1,
                (false, true) => fn_ += 1,
                (false, false) => tn += 1,
            }
        }
        let recall = ratio(tp, tp + fn_);
        let precision = ratio(tp, tp + fp);
        Self {
            true_positives: tp,
            false_positives: fp,
            false_negatives: fn_,
            true_negatives: tn,
            recall,
            precision,
            // No flags at all → no false alarms (FP=0), not "100% false alarms". Only 1 - precision
            // once something has actually been flagged.
            cry_wolf_rate: if tp + fp == 0 { 0.0 } else { 1.0 - precision },
        }
    }
}

/// n/d as f64, with 0/0 → 0.0 (no flags or no positives → the metric is 0, not NaN).
fn ratio(n: usize, d: usize) -> f64 {
    if d == 0 {
        0.0
    } else {
        n as f64 / d as f64
    }
}

use rand::SeedableRng;
use rand_chacha::ChaCha8Rng; // the crate's portable RNG — StdRng is NOT reproducible across
                             // versions/platforms, wrong for a reproducible-measurement harness.

/// The DEFERRED binding gate (spec §9) — applied on the larger owner-sourced set, NOT the tiny
/// seed (a precision CI over ~5 flags is degenerate; see the plan preamble). Built now so it's
/// ready. Phase 0's real EXIT is the raw-count smoke (Task 7 `smoke_ok`).
pub const GATE_PRECISION_CI_LOWER: f64 = 0.90;
pub const GATE_RECALL_MIN: f64 = 0.30;
/// Bootstrap CI confidence level = `bootstrap_ci_mean`'s 3rd arg (stats.rs debug-asserts 0<conf<1).
pub const GATE_CI_CONF: f64 = 0.90;

#[derive(Debug, Clone)]
pub struct GradeResult {
    pub metrics: Metrics,
    pub precision_ci_lower: f64,
    /// The DEFERRED statistical gate — meaningful only on the larger set. On the seed, read
    /// `smoke_ok(&result.metrics)` instead (Task 7).
    pub passes: bool,
}

/// Grade a set of outcomes: compute metrics, bootstrap the precision CI lower bound
/// (over the per-FLAG correctness indicators), and apply the deferred §9 gate. `seed` makes
/// the bootstrap deterministic for hermetic tests.
pub fn grade(outcomes: &[Outcome], seed: u64) -> GradeResult {
    let metrics = Metrics::from_outcomes(outcomes);
    // Precision = mean of {1 if a flagged pair was a true contradiction else 0}.
    let flagged_correct: Vec<f64> = outcomes
        .iter()
        .filter(|o| o.flagged)
        .map(|o| if o.truly_contradicts { 1.0 } else { 0.0 })
        .collect();
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let ci = if flagged_correct.is_empty() {
        (0.0, 0.0)
    } else {
        crate::stats::bootstrap_ci_mean(&flagged_correct, 2000, GATE_CI_CONF, &mut rng)
    };
    let precision_ci_lower = ci.0;
    let passes = precision_ci_lower >= GATE_PRECISION_CI_LOWER && metrics.recall >= GATE_RECALL_MIN;
    GradeResult { metrics, precision_ci_lower, passes }
}

/// Phase-0 EXIT gate (the honest raw-count smoke on the tiny seed — spec-review re-scope):
/// the judge raised NO false alarms and caught at least a couple of the real contradictions.
/// This is what Task 7/8 assert; `grade().passes` is the deferred statistical gate for the big set.
pub fn smoke_ok(m: &Metrics) -> bool {
    m.false_positives == 0 && m.true_positives >= 2
}

/// The headline PASS/FAIL line for a graded run. `binding=true` reports the §9 statistical
/// binding gate (precision CI-lower ≥ [`GATE_PRECISION_CI_LOWER`] AND recall ≥ [`GATE_RECALL_MIN`],
/// = `GradeResult::passes`) — meaningful ONLY on the 50+-pair set, where the precision CI is not
/// degenerate. `binding=false` reports the tiny-seed plumbing SMOKE ([`smoke_ok`]). Binding mode
/// echoes the CI-lower + recall numbers so the verdict is auditable, not a bare boolean.
pub fn verdict_line(g: &GradeResult, binding: bool) -> String {
    if binding {
        format!(
            "BINDING GATE {} (precision CI-lower {:.3} ≥ {:.2} AND recall {:.3} ≥ {:.2})",
            if g.passes { "PASS" } else { "FAIL" },
            g.precision_ci_lower,
            GATE_PRECISION_CI_LOWER,
            g.metrics.recall,
            GATE_RECALL_MIN,
        )
    } else {
        format!(
            "SMOKE {}",
            if smoke_ok(&g.metrics) { "PASS (0 FP, ≥2 caught)" } else { "FAIL" },
        )
    }
}

use bossclaw_core::conflict::{judge_pair, Verdict};
use bossclaw_core::reason::Reasoner;

/// Per-pair detail for verbose grading / threshold tuning: the ground-truth label plus the
/// flagged verdict (`Some` iff [`judge_pair`] returned actionable). Lets the CLI show exactly
/// which pairs the judge got wrong and at what self-reported confidence — the direct input to
/// tuning `CONFLICT_CONF_MIN`.
#[derive(Debug, Clone)]
pub struct GradedPair {
    pub a: String,
    pub b: String,
    pub label: PairLabel,
    pub verdict: Option<Verdict>,
}

impl GradedPair {
    pub fn flagged(&self) -> bool {
        self.verdict.is_some()
    }
    pub fn truly_contradicts(&self) -> bool {
        self.label == PairLabel::Contradicts
    }
    /// A misclassification: flagged a non-conflict (FP) or missed a real one (FN).
    pub fn is_error(&self) -> bool {
        self.flagged() != self.truly_contradicts()
    }
}

/// Run the judge over every pair, returning the aggregate grade AND per-pair detail (for
/// `--verbose` error inspection / threshold tuning). A transport error on any pair fails the
/// whole run loudly (a partial grade is a misleading grade).
pub fn run_grade_detailed(
    pairs: &[LabelledPair],
    judge: &dyn Reasoner,
    seed: u64,
) -> anyhow::Result<(GradeResult, Vec<GradedPair>)> {
    let mut graded = Vec::with_capacity(pairs.len());
    for p in pairs {
        let verdict = judge_pair(judge, &p.a, &p.b)?;
        graded.push(GradedPair { a: p.a.clone(), b: p.b.clone(), label: p.label, verdict });
    }
    let outcomes: Vec<Outcome> = graded
        .iter()
        .map(|g| Outcome { flagged: g.flagged(), truly_contradicts: g.truly_contradicts() })
        .collect();
    Ok((grade(&outcomes, seed), graded))
}

/// Run the judge over every labelled pair and grade it (aggregate only). A judge transport error
/// on any pair fails the whole run loudly (a partial grade is a misleading grade).
pub fn run_grade(
    pairs: &[LabelledPair],
    judge: &dyn Reasoner,
    seed: u64,
) -> anyhow::Result<GradeResult> {
    run_grade_detailed(pairs, judge, seed).map(|(g, _)| g)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bossclaw_core::conflict::{build_conflict_prompt, CONFLICT_SYSTEM};
    use bossclaw_core::reason::ScriptedReasoner;

    #[test]
    fn loads_jsonl_pairs_and_fails_loud_on_empty() {
        let jsonl = "\
{\"a\":\"uses Vercel\",\"b\":\"left Vercel\",\"label\":\"contradicts\",\"winner\":\"newer\"}
{\"a\":\"tabs in Python\",\"b\":\"spaces in Go\",\"label\":\"coexist\"}
";
        let pairs = parse_pairs(jsonl).expect("parse");
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].label, PairLabel::Contradicts);
        assert!(pairs[0].winner.is_some());
        assert_eq!(pairs[1].label, PairLabel::Coexist);
        // empty input is a loud error, never a silent zero-case run (memharness convention)
        assert!(parse_pairs("   \n").is_err());
    }

    #[test]
    fn metrics_from_a_known_confusion_matrix() {
        // flagged? (Some) × truly-contradicts?  →  TP FP FN TN
        let outcomes = vec![
            Outcome { flagged: true,  truly_contradicts: true  }, // TP
            Outcome { flagged: true,  truly_contradicts: true  }, // TP
            Outcome { flagged: true,  truly_contradicts: false }, // FP (cry wolf)
            Outcome { flagged: false, truly_contradicts: true  }, // FN (miss)
            Outcome { flagged: false, truly_contradicts: false }, // TN
        ];
        let m = Metrics::from_outcomes(&outcomes);
        assert_eq!(m.true_positives, 2);
        assert_eq!(m.false_positives, 1);
        assert_eq!(m.false_negatives, 1);
        // recall (catch) = TP/(TP+FN) = 2/3 ; precision = TP/(TP+FP) = 2/3
        assert!((m.recall - 2.0 / 3.0).abs() < 1e-9);
        assert!((m.precision - 2.0 / 3.0).abs() < 1e-9);
        // cry-wolf = 1 - precision = 1/3
        assert!((m.cry_wolf_rate - 1.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn gate_passes_only_when_precision_ci_lower_and_recall_clear_the_bar() {
        // 20 flags, all correct → precision 1.0, tight CI well above 0.90; recall high.
        let all_good: Vec<Outcome> = (0..20)
            .map(|_| Outcome { flagged: true, truly_contradicts: true })
            .chain((0..5).map(|_| Outcome { flagged: false, truly_contradicts: false }))
            .collect();
        let g = grade(&all_good, /*seed*/ 42);
        assert!(g.passes, "clean judge clears the gate");
        assert!(g.precision_ci_lower >= 0.90);
        assert!(g.metrics.recall >= 0.30);

        // Half the flags are wrong → precision ~0.5 → gate fails.
        let noisy: Vec<Outcome> = (0..10)
            .map(|_| Outcome { flagged: true, truly_contradicts: true })
            .chain((0..10).map(|_| Outcome { flagged: true, truly_contradicts: false }))
            .collect();
        assert!(!grade(&noisy, 42).passes, "cry-wolf judge fails the gate");
    }

    #[test]
    fn gate_fails_on_low_recall_even_with_perfect_precision() {
        // 20 correct flags (precision 1.0, ci_lower ≥ 0.90) but 60 missed true contradictions
        // → recall = 20 / (20 + 60) = 0.25 < GATE_RECALL_MIN. The precision arm ALONE would pass;
        // only the recall conjunct in `grade()` fails it. Guards that arm against silent deletion.
        let low_recall: Vec<Outcome> = (0..20)
            .map(|_| Outcome { flagged: true, truly_contradicts: true })
            .chain((0..60).map(|_| Outcome { flagged: false, truly_contradicts: true }))
            .collect();
        let g = grade(&low_recall, 42);
        assert!(g.precision_ci_lower >= 0.90, "precision arm alone would pass");
        assert!((g.metrics.recall - 0.25).abs() < 1e-9);
        assert!(!g.passes, "low recall must fail the gate even at perfect precision");
    }

    #[test]
    fn grade_zero_flagged_branch_is_honest() {
        // Judge flags nothing, yet some pairs truly contradict: the empty-flag branch must report
        // ci_lower 0.0, fail the gate, fail the smoke — and NOT cry "100% false alarms" (FP=0).
        let none_flagged: Vec<Outcome> = (0..5)
            .map(|_| Outcome { flagged: false, truly_contradicts: true })
            .chain((0..3).map(|_| Outcome { flagged: false, truly_contradicts: false }))
            .collect();
        let g = grade(&none_flagged, 42);
        assert_eq!(g.precision_ci_lower, 0.0);
        assert!(!g.passes);
        assert!(!smoke_ok(&g.metrics));
        assert_eq!(g.metrics.cry_wolf_rate, 0.0, "no flags → no false alarms, not 100%");
    }

    #[test]
    fn run_grade_over_scripted_judge_on_the_seed_fixture() {
        let body = include_str!("../fixtures/conflict-seed.jsonl");
        let pairs = parse_pairs(body).expect("seed parses");
        // Script a PERFECT judge: flag exactly the `contradicts` pairs at high confidence.
        let mut r = ScriptedReasoner::new("scripted");
        for p in &pairs {
            let prompt = build_conflict_prompt(&p.a, &p.b);
            let resp = match p.label {
                PairLabel::Contradicts => serde_json::json!({
                    "contradicts": true, "winner": "newer", "confidence": 95, "why": "opposite claims"
                }),
                _ => serde_json::json!({
                    "contradicts": false, "winner": "unclear", "confidence": 95, "why": "not a conflict"
                }),
            };
            r = r.with_response(CONFLICT_SYSTEM, &prompt, resp);
        }
        let g = run_grade(&pairs, &r, 42).expect("grade ok");
        // A perfect judge clears the Phase-0 raw-count SMOKE (0 FP, ≥2 caught).
        assert!(smoke_ok(&g.metrics), "perfect judge passes the smoke: {:?}", g.metrics);
        assert_eq!(g.metrics.false_positives, 0);
        assert_eq!(g.metrics.true_positives, 5, "all 5 true contradictions flagged");
    }

    #[test]
    fn run_grade_fails_the_smoke_when_the_judge_cries_wolf() {
        // Exercises the real pipeline's FAIL path (not just synthetic Outcomes): a judge that
        // ALSO flags a coexist pair as a contradiction must trip the smoke (false_positives > 0).
        let body = include_str!("../fixtures/conflict-seed.jsonl");
        let pairs = parse_pairs(body).expect("seed parses");
        let mut r = ScriptedReasoner::new("crywolf");
        for p in &pairs {
            let prompt = build_conflict_prompt(&p.a, &p.b);
            // Flag every contradicts pair AND every coexist pair (the false alarms).
            let flag = matches!(p.label, PairLabel::Contradicts | PairLabel::Coexist);
            let resp = serde_json::json!({
                "contradicts": flag, "winner": if flag { "newer" } else { "unclear" },
                "confidence": 95, "why": "x"
            });
            r = r.with_response(CONFLICT_SYSTEM, &prompt, resp);
        }
        let g = run_grade(&pairs, &r, 42).expect("grade ok");
        assert_eq!(g.metrics.false_positives, 2, "both coexist pairs falsely flagged");
        assert!(!smoke_ok(&g.metrics), "a cry-wolf judge FAILS the smoke");
    }

    #[test]
    fn verdict_line_reports_smoke_vs_binding_gate() {
        // A clean run: 20 correct flags + 5 clean abstentions → clears BOTH gates.
        let good: Vec<Outcome> = (0..20)
            .map(|_| Outcome { flagged: true, truly_contradicts: true })
            .chain((0..5).map(|_| Outcome { flagged: false, truly_contradicts: false }))
            .collect();
        let g = grade(&good, 42);
        assert!(verdict_line(&g, false).starts_with("SMOKE PASS"));
        assert!(verdict_line(&g, true).starts_with("BINDING GATE PASS"));
        // Binding mode echoes the auditable numbers, not a bare boolean.
        let bl = verdict_line(&g, true);
        assert!(bl.contains("precision CI-lower") && bl.contains("recall"));

        // A cry-wolf run (half the flags wrong) fails the binding gate.
        let noisy: Vec<Outcome> = (0..10)
            .map(|_| Outcome { flagged: true, truly_contradicts: true })
            .chain((0..10).map(|_| Outcome { flagged: true, truly_contradicts: false }))
            .collect();
        assert!(verdict_line(&grade(&noisy, 42), true).starts_with("BINDING GATE FAIL"));
    }

    #[test]
    fn detailed_run_marks_fp_and_fn_pairs_with_confidence() {
        use bossclaw_core::conflict::Winner;
        // A coexist pair the judge WRONGLY flags (FP) + a real contradiction it MISSES (FN, dropped
        // below the confidence floor). Verbose mode must expose both, incl. the FP's confidence.
        let pairs = vec![
            LabelledPair { a: "x1".into(), b: "y1".into(), label: PairLabel::Coexist, winner: None },
            LabelledPair {
                a: "x2".into(),
                b: "y2".into(),
                label: PairLabel::Contradicts,
                winner: Some(Winner::Newer),
            },
        ];
        let r = ScriptedReasoner::new("mixed")
            .with_response(
                CONFLICT_SYSTEM,
                &build_conflict_prompt("x1", "y1"),
                serde_json::json!({"contradicts": true, "winner": "newer", "confidence": 80, "why": "looks conflicty"}),
            )
            .with_response(
                CONFLICT_SYSTEM,
                &build_conflict_prompt("x2", "y2"),
                serde_json::json!({"contradicts": true, "winner": "newer", "confidence": 5, "why": "unsure"}),
            );
        let (_g, graded) = run_grade_detailed(&pairs, &r, 42).expect("ok");
        let fp = &graded[0];
        assert!(fp.flagged() && fp.is_error(), "coexist flagged = FP error");
        assert_eq!(fp.verdict.as_ref().expect("flagged verdict").confidence, 80);
        let miss = &graded[1];
        assert!(!miss.flagged() && miss.is_error(), "missed contradiction = FN error");
        assert!(miss.verdict.is_none(), "a dropped pair carries no flagged verdict");
    }
}
