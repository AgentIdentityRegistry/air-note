//! The blind pairwise judging layer (spec §5): verdicts, Cohen's kappa, the trust verdict,
//! blind A/B assignment, position-swap resolution, the shared `PairJudge` trait (the LOCAL
//! judge AND the CLOUD auditor are both blind position-swapped pickers), and the audit-sample
//! selection (`max(30, 15%)` ∪ uncertains, Rev 2).

use serde::Serialize;

/// A pairwise judgment outcome, de-blinded to arm identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Verdict {
    AirWins,
    GbrainWins,
    Tie,
    /// The two position-swapped judgments disagreed, or a reply was ambiguous → uncertain
    /// (always audited; never dropped).
    Uncertain,
}

/// Kappa category (Uncertain folds into Tie in the 3×3 table — an uncertain call compared to a
/// decisive one is a non-decision, counted as disagreement with any decisive pick).
fn category(v: Verdict) -> usize {
    match v {
        Verdict::AirWins => 0,
        Verdict::GbrainWins => 1,
        Verdict::Tie | Verdict::Uncertain => 2,
    }
}

/// Raw agreement fraction between equal-length verdict vectors.
pub fn raw_agreement(a: &[Verdict], b: &[Verdict]) -> f64 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let agree = a.iter().zip(b).filter(|(x, y)| category(**x) == category(**y)).count();
    agree as f64 / a.len() as f64
}

/// Cohen's kappa over {AirWins, GbrainWins, Tie/Uncertain}. Empty/mismatched → 0.
///
/// Degenerate case: when both raters are all-in on one category, p_e = 1 forces p_o = 1 and
/// kappa reports 1.0 — formally kappa is undefined (0/0) there; perfect raw agreement is
/// genuinely the best case, so this cannot flip a trust verdict dishonestly, but readers
/// should know 1.0 in that case means 'vacuous chance-correction', not 'strong beyond-chance
/// agreement'.
pub fn cohens_kappa(a: &[Verdict], b: &[Verdict]) -> f64 {
    let n = a.len();
    if n == 0 || n != b.len() {
        return 0.0;
    }
    let p_o = raw_agreement(a, b);
    let mut ca = [0.0f64; 3];
    let mut cb = [0.0f64; 3];
    for i in 0..n {
        ca[category(a[i])] += 1.0;
        cb[category(b[i])] += 1.0;
    }
    let nf = n as f64;
    let p_e: f64 = (0..3).map(|c| (ca[c] / nf) * (cb[c] / nf)).sum();
    if (1.0 - p_e).abs() < 1e-12 {
        return if (p_o - 1.0).abs() < 1e-12 { 1.0 } else { 0.0 };
    }
    (p_o - p_e) / (1.0 - p_e)
}

/// Trust thresholds (spec §5): trusted iff agreement ≥ 0.85 AND kappa ≥ 0.6.
pub const TRUST_AGREEMENT_MIN: f64 = 0.85;
pub const TRUST_KAPPA_MIN: f64 = 0.6;

/// The judge-trust verdict the report LEADS with.
#[derive(Debug, Clone, Serialize)]
pub struct TrustVerdict {
    pub audited_count: usize,
    pub agreement: f64,
    pub kappa: f64,
    pub trusted: bool,
    /// The run auto-expanded the audit to 100% because trust failed (spec §5).
    pub expanded_to_full_audit: bool,
    /// The cloud audit could not complete (API failure / --local-only) → verdict UNAVAILABLE,
    /// never fabricated.
    pub audit_incomplete: bool,
    /// Rev 2: the open-query pool was smaller than AUDIT_FLOOR → "audit n too small; trust
    /// verdict indicative only" in the report.
    pub audit_n_too_small: bool,
}

/// Compute the trust verdict from paired local-vs-cloud verdicts on the AUDITED set.
pub fn trust_verdict(
    local: &[Verdict],
    cloud: &[Verdict],
    expanded_to_full_audit: bool,
    audit_incomplete: bool,
    audit_n_too_small: bool,
) -> TrustVerdict {
    if audit_incomplete || local.is_empty() {
        return TrustVerdict {
            audited_count: local.len(),
            agreement: 0.0,
            kappa: 0.0,
            trusted: false,
            expanded_to_full_audit,
            audit_incomplete: true,
            audit_n_too_small,
        };
    }
    let agreement = raw_agreement(local, cloud);
    let kappa = cohens_kappa(local, cloud);
    TrustVerdict {
        audited_count: local.len(),
        agreement,
        kappa,
        trusted: agreement >= TRUST_AGREEMENT_MIN && kappa >= TRUST_KAPPA_MIN,
        expanded_to_full_audit,
        audit_incomplete: false,
        audit_n_too_small,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cohens_kappa_known_values() {
        let a = vec![Verdict::AirWins, Verdict::GbrainWins, Verdict::Tie, Verdict::AirWins];
        let b = a.clone();
        assert!((cohens_kappa(&a, &b) - 1.0).abs() < 1e-9, "perfect agreement → 1");
        let x = vec![Verdict::AirWins; 4];
        let y = vec![Verdict::GbrainWins; 4];
        assert!(cohens_kappa(&x, &y) <= 0.0, "total disagreement → ≤ 0");
        assert!((cohens_kappa(&[], &[]) - 0.0).abs() < 1e-9, "empty → 0, no panic");
    }

    #[test]
    fn raw_agreement_fraction() {
        let a = vec![Verdict::AirWins, Verdict::GbrainWins, Verdict::Tie];
        let b = vec![Verdict::AirWins, Verdict::AirWins, Verdict::Tie];
        assert!((raw_agreement(&a, &b) - (2.0 / 3.0)).abs() < 1e-9);
    }

    #[test]
    fn trust_verdict_thresholds_and_flags() {
        // 9/10 agree, decisive both sides → trusted.
        let mut local = vec![Verdict::AirWins; 5];
        local.extend(vec![Verdict::GbrainWins; 5]);
        let mut cloud = local.clone();
        cloud[9] = Verdict::AirWins; // one disagreement → 90% agreement
        let t = trust_verdict(&local, &cloud, false, false, false);
        assert!(t.trusted, "agreement {} kappa {}", t.agreement, t.kappa);
        assert!(!t.audit_incomplete && !t.audit_n_too_small);

        // Audit incomplete → NOT trusted, flagged, never fabricated.
        let t = trust_verdict(&[], &[], false, true, false);
        assert!(!t.trusted && t.audit_incomplete);

        // Rev 2: open pool < AUDIT_FLOOR → indicative-only flag carried through.
        let t = trust_verdict(&local, &cloud, false, false, true);
        assert!(t.audit_n_too_small);
    }
}
