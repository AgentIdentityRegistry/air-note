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
}
