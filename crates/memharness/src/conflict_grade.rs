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
            cry_wolf_rate: 1.0 - precision,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_jsonl_pairs_and_fails_loud_on_empty() {
        let jsonl = "\
{\"a\":\"uses Vercel\",\"b\":\"left Vercel\",\"label\":\"contradicts\",\"winner\":\"newer\"}
{\"a\":\"tabs in Python\",\"b\":\"spaces in Go\",\"label\":\"coexist\"}
";
        let pairs = parse_pairs(jsonl).expect("parse");
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].label, PairLabel::Contradicts);
        assert!(matches!(pairs[0].winner, Some(_)));
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
}
