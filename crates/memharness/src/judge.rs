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
    debug_assert_eq!(a.len(), b.len(), "verdict vectors must be paired");
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
    debug_assert_eq!(a.len(), b.len(), "verdict vectors must be paired");
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

use rand::seq::SliceRandom;
use rand::Rng;
use std::collections::BTreeSet;

/// A position-level pick (what a blind judge names): A, B, or TIE.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PosPick {
    A,
    B,
    Tie,
}

/// The pairwise-judging seam: ONE trait serves both the LOCAL judge (Ollama) and the CLOUD
/// auditor (Anthropic) — both are blind, position-swapped pickers. `Ok(None)` = the reply was
/// ambiguous → the caller records `Uncertain` (never dropped, never fabricated).
pub trait PairJudge {
    fn pick(&self, query: &str, answer_a: &str, answer_b: &str) -> anyhow::Result<Option<PosPick>>;
}

/// The shared judging prompt: both the local judge and the cloud auditor use IT (identical
/// instructions; only the model differs), constrained to EXACTLY one output token.
pub fn pairwise_prompt(query: &str, answer_a: &str, answer_b: &str) -> String {
    format!(
        "You are comparing two answers to the same question.\n\nQuestion: {query}\n\n\
         Answer A:\n{answer_a}\n\nAnswer B:\n{answer_b}\n\n\
         Which answer is better (more correct, more complete, better grounded)? \
         Reply with exactly one token: A, B, or TIE."
    )
}

/// Parse a one-token judge reply. Exact match first (trimmed, case-folded); tokenized fallback
/// ONLY if exactly one signal appears among {A, B, TIE} with no ambiguity blockers. Anything
/// else → None (→ `Uncertain`).
///
/// Fallback case rule: single-letter signals must be UPPERCASE — lowercase 'a'/'b' are English
/// articles/pronoun fragments ("It's a draw"), never picks; their presence anywhere marks the
/// reply conversational → ambiguous → None. TIE is case-insensitive (no English-word collision).
pub fn parse_pick_token(text: &str) -> Option<PosPick> {
    let t = text.trim().to_uppercase();
    match t.as_str() {
        "A" => return Some(PosPick::A),
        "B" => return Some(PosPick::B),
        "TIE" => return Some(PosPick::Tie),
        _ => {}
    }
    // Tokenize the ORIGINAL text: case carries meaning here (uppercase = pick, lowercase = prose).
    let (mut has_a, mut has_b, mut has_tie, mut blocked) = (false, false, false, false);
    for token in text.split(|c: char| !c.is_ascii_alphanumeric()).filter(|s| !s.is_empty()) {
        match token {
            "A" => has_a = true,
            "B" => has_b = true,
            "a" | "b" => blocked = true,
            _ if token.eq_ignore_ascii_case("tie") => has_tie = true,
            _ => {}
        }
    }
    let signals = [has_a, has_b, has_tie].iter().filter(|s| **s).count();
    if blocked || signals != 1 {
        return None;
    }
    if has_a {
        Some(PosPick::A)
    } else if has_b {
        Some(PosPick::B)
    } else {
        Some(PosPick::Tie)
    }
}

/// The local judge: the SAME Ollama model as the answerer, behind the shared trait.
pub struct OllamaJudge {
    pub model: String,
}

impl PairJudge for OllamaJudge {
    fn pick(&self, query: &str, answer_a: &str, answer_b: &str) -> anyhow::Result<Option<PosPick>> {
        let reply = crate::ollama::generate(&self.model, &pairwise_prompt(query, answer_a, answer_b))?;
        Ok(parse_pick_token(&reply))
    }
}

/// Per-pair blind assignment: whether AIR's answer is shown as "A". The judge NEVER sees arm
/// names — only positions (spec §5 blinding).
#[derive(Debug, Clone, Copy)]
pub struct Blind {
    pub air_is_a: bool,
}

/// Seeded per-pair coin flip.
pub fn assign_blind<R: Rng>(rng: &mut R) -> Blind {
    Blind { air_is_a: rng.gen_bool(0.5) }
}

/// De-blind a position pick to arm identity. `swapped` = this pick came from the SECOND
/// (position-swapped) judging call, where A shows what was B.
pub fn deblind_pick(blind: Blind, pick: PosPick, swapped: bool) -> Verdict {
    let air_is_a = if swapped { !blind.air_is_a } else { blind.air_is_a };
    match pick {
        PosPick::Tie => Verdict::Tie,
        PosPick::A => {
            if air_is_a { Verdict::AirWins } else { Verdict::GbrainWins }
        }
        PosPick::B => {
            if air_is_a { Verdict::GbrainWins } else { Verdict::AirWins }
        }
    }
}

/// Resolve the two de-blinded, position-swapped judgments: agree → that verdict; disagree →
/// `Uncertain` (spec §5).
pub fn resolve_swap(first: Verdict, swapped: Verdict) -> Verdict {
    if first == swapped { first } else { Verdict::Uncertain }
}

/// Judge one pair blind + position-swapped: two `pick` calls (assigned order, then swapped),
/// each de-blinded; any ambiguous reply OR ordering disagreement → `Uncertain`. Used for BOTH
/// the local judge and the cloud auditor (identical protocol; only the model differs).
pub fn judge_pair_blind(
    judge: &dyn PairJudge,
    blind: Blind,
    query: &str,
    air_answer: &str,
    gbrain_answer: &str,
) -> anyhow::Result<Verdict> {
    let (first_a, first_b) = if blind.air_is_a {
        (air_answer, gbrain_answer)
    } else {
        (gbrain_answer, air_answer)
    };
    let Some(p1) = judge.pick(query, first_a, first_b)? else {
        return Ok(Verdict::Uncertain);
    };
    let Some(p2) = judge.pick(query, first_b, first_a)? else {
        return Ok(Verdict::Uncertain);
    };
    Ok(resolve_swap(deblind_pick(blind, p1, false), deblind_pick(blind, p2, true)))
}

/// Audit-sample floor + fraction (spec §5 Rev 2).
pub const AUDIT_FLOOR: usize = 30;
pub const AUDIT_FRACTION: f64 = 0.15;

/// Seeded audit selection: a random `min(open_count, max(AUDIT_FLOOR, ceil(15% of open)))`
/// sample ∪ ALL uncertain indices (deduped union). A pool smaller than the floor is audited
/// in full (and the caller sets `audit_n_too_small`).
pub fn select_audit_indices<R: Rng>(
    open_count: usize,
    uncertain: &[usize],
    rng: &mut R,
) -> BTreeSet<usize> {
    debug_assert!(
        uncertain.iter().all(|i| *i < open_count),
        "uncertain indices must lie in the open pool"
    );
    let target = AUDIT_FLOOR
        .max((open_count as f64 * AUDIT_FRACTION).ceil() as usize)
        .min(open_count);
    let mut all: Vec<usize> = (0..open_count).collect();
    all.shuffle(rng);
    let mut set: BTreeSet<usize> = all.into_iter().take(target).collect();
    set.extend(uncertain.iter().copied().filter(|i| *i < open_count));
    set
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

    #[test]
    fn parse_pick_token_exact_then_tokenized_fallback() {
        // Exact one-token replies (the prompt DEMANDS these) parse first.
        assert_eq!(parse_pick_token("A"), Some(PosPick::A));
        assert_eq!(parse_pick_token(" b\n"), Some(PosPick::B));
        assert_eq!(parse_pick_token("TIE"), Some(PosPick::Tie));
        assert_eq!(parse_pick_token("tie"), Some(PosPick::Tie));
        // Tokenized fallback ONLY when exactly one signal token appears.
        assert_eq!(parse_pick_token("Answer: B"), Some(PosPick::B));
        assert_eq!(parse_pick_token("The answer is A."), Some(PosPick::A));
        // Ambiguous → None (recorded Uncertain — never dropped, never fabricated).
        assert_eq!(parse_pick_token("A or B, hard to say"), None);
        assert_eq!(parse_pick_token("I cannot decide"), None);
        // "a" as an article + "tie" → article blocker wins → ambiguous (safe: audited as Uncertain).
        assert_eq!(parse_pick_token("It's a tie between them"), None);
        // The English article "a" must NEVER be a decisive pick (review Fix 2 regressions).
        assert_eq!(parse_pick_token("It's a draw"), None);
        assert_eq!(parse_pick_token("It's a toss-up"), None);
        // Uppercase single letter with punctuation stays decisive via the fallback.
        assert_eq!(parse_pick_token("B."), Some(PosPick::B));
    }

    #[test]
    fn blind_assignment_is_seeded_and_deblinding_is_correct() {
        use rand::SeedableRng;
        use rand_chacha::ChaCha8Rng;
        let mut rng1 = ChaCha8Rng::seed_from_u64(42);
        let mut rng2 = ChaCha8Rng::seed_from_u64(42);
        assert_eq!(assign_blind(&mut rng1).air_is_a, assign_blind(&mut rng2).air_is_a);
        let air_a = Blind { air_is_a: true };
        // Unswapped: A holds AIR.
        assert_eq!(deblind_pick(air_a, PosPick::A, false), Verdict::AirWins);
        assert_eq!(deblind_pick(air_a, PosPick::B, false), Verdict::GbrainWins);
        // Swapped: positions exchanged.
        assert_eq!(deblind_pick(air_a, PosPick::A, true), Verdict::GbrainWins);
        assert_eq!(deblind_pick(air_a, PosPick::Tie, true), Verdict::Tie);
        let air_b = Blind { air_is_a: false };
        assert_eq!(deblind_pick(air_b, PosPick::A, false), Verdict::GbrainWins);
        assert_eq!(deblind_pick(air_b, PosPick::A, true), Verdict::AirWins);
    }

    #[test]
    fn resolve_swap_and_judge_pair_blind() {
        assert_eq!(resolve_swap(Verdict::AirWins, Verdict::AirWins), Verdict::AirWins);
        assert_eq!(resolve_swap(Verdict::AirWins, Verdict::GbrainWins), Verdict::Uncertain);
        assert_eq!(resolve_swap(Verdict::Tie, Verdict::Tie), Verdict::Tie);

        /// A content-based double: prefers the answer containing "GOOD" (blind-compatible — it
        /// judges CONTENT, not position, so it survives blinding and swapping).
        struct GoodJudge;
        impl PairJudge for GoodJudge {
            fn pick(&self, _q: &str, a: &str, b: &str) -> anyhow::Result<Option<PosPick>> {
                Ok(match (a.contains("GOOD"), b.contains("GOOD")) {
                    (true, false) => Some(PosPick::A),
                    (false, true) => Some(PosPick::B),
                    _ => Some(PosPick::Tie),
                })
            }
        }
        /// An ambiguity double: always returns None.
        struct MumbleJudge;
        impl PairJudge for MumbleJudge {
            fn pick(&self, _q: &str, _a: &str, _b: &str) -> anyhow::Result<Option<PosPick>> {
                Ok(None)
            }
        }

        for air_is_a in [true, false] {
            let blind = Blind { air_is_a };
            // AIR's answer holds GOOD → AirWins regardless of the blind assignment.
            let v = judge_pair_blind(&GoodJudge, blind, "q", "GOOD air", "meh gbrain").unwrap();
            assert_eq!(v, Verdict::AirWins, "air_is_a={air_is_a}");
            // GBrain's answer holds GOOD → GbrainWins regardless.
            let v = judge_pair_blind(&GoodJudge, blind, "q", "meh air", "GOOD gbrain").unwrap();
            assert_eq!(v, Verdict::GbrainWins, "air_is_a={air_is_a}");
        }
        // Ambiguous reply on either call → Uncertain.
        let v = judge_pair_blind(&MumbleJudge, Blind { air_is_a: true }, "q", "x", "y").unwrap();
        assert_eq!(v, Verdict::Uncertain);
    }

    #[test]
    fn audit_selection_floor_union_uncertains_seeded() {
        use rand::SeedableRng;
        use rand_chacha::ChaCha8Rng;
        // 200 open queries → target max(30, ceil(30)) = 30 random ∪ uncertains {5, 199}.
        let mut rng = ChaCha8Rng::seed_from_u64(42);
        let sel = select_audit_indices(200, &[5, 199], &mut rng);
        assert!(sel.len() >= 30 && sel.len() <= 32, "30 random ∪ 2 uncertains, deduped: {}", sel.len());
        assert!(sel.contains(&5) && sel.contains(&199), "ALL uncertains included");
        // Determinism.
        let mut rng2 = ChaCha8Rng::seed_from_u64(42);
        assert_eq!(sel, select_audit_indices(200, &[5, 199], &mut rng2));
        // A pool smaller than the floor → the WHOLE pool.
        let mut rng3 = ChaCha8Rng::seed_from_u64(42);
        let all = select_audit_indices(10, &[], &mut rng3);
        assert_eq!(all.len(), 10);
    }
}
