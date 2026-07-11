//! Rung 3 semantic-conflict judge core: prompt + schema + verdict + threshold,
//! over the `Reasoner` seam. Reused by the daemon detection pass (Phase 2) and
//! graded by `memharness conflict-grade` (Phase 0). The judge only ever produces
//! a Verdict — never a mutation (spec I1).

// NB: BossclawError is re-exported at the crate root (lib.rs `pub use`), NOT via
// crate::reason (its import there is private → `crate::reason::BossclawError` is E0603).
use crate::BossclawError;
use crate::reason::Reasoner;

/// Which side of a contradiction the judge believes is correct. `Unclear` and a
/// `false` `contradicts` are both non-actionable (see [`judge_pair`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Winner {
    /// The newer memory is the correct one.
    Newer,
    /// The older memory is the correct one.
    Older,
    /// The judge cannot tell which side is correct (non-actionable).
    Unclear,
}

/// The judge's structured answer for one candidate pair. `why`/`confidence` are
/// model self-reports over attacker-influenceable input (spec I7): callers that
/// PERSIST a verdict must sanitize/bound `why` and coarsen `confidence` — the
/// harness does neither (it only measures), which is fine (it never surfaces them).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Verdict {
    /// Whether the two snippets factually contradict each other.
    pub contradicts: bool,
    /// Which side the judge believes is correct.
    pub winner: Winner,
    /// 0..=100 model self-reported confidence.
    pub confidence: u8,
    /// The judge's free-text rationale (untrusted; see the type note).
    pub why: String,
}

/// JSON schema constraining the judge's emission (passed to `complete_json`;
/// Ollama honors it as the `format` field, exactly like `reconcile::rewrite_schema`).
pub fn conflict_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "required": ["contradicts", "winner", "confidence", "why"],
        "properties": {
            "contradicts": { "type": "boolean" },
            "winner": { "type": "string", "enum": ["newer", "older", "unclear"] },
            "confidence": { "type": "integer", "minimum": 0, "maximum": 100 },
            "why": { "type": "string" }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdict_parses_from_model_json() {
        let v: Verdict = serde_json::from_value(serde_json::json!({
            "contradicts": true, "winner": "newer", "confidence": 82,
            "why": "one says Vercel, the other says migrated off Vercel"
        })).expect("parse");
        assert!(v.contradicts);
        assert_eq!(v.winner, Winner::Newer);
        assert_eq!(v.confidence, 82);
    }

    #[test]
    fn schema_lists_all_four_required_fields() {
        let s = conflict_schema();
        let req = s["required"].as_array().expect("required[]");
        for f in ["contradicts", "winner", "confidence", "why"] {
            assert!(req.iter().any(|v| v == f), "schema requires {f}");
        }
        // winner is an enum of exactly the three verdict labels
        let en = s["properties"]["winner"]["enum"].as_array().expect("winner enum");
        assert_eq!(en.len(), 3);
    }
}
