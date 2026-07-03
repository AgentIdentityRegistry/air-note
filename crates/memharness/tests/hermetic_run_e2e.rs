//! Hermetic e2e #2: `run_queries` end-to-end with EVERY external seam doubled — AIR retrieval,
//! GBrain retrieval, answerer, local judge, cloud auditor. Proves the audit ladder (floor
//! sample → trust → expand-to-100%), the local-only path, and egress accounting, with zero
//! network anywhere.

use memharness::arms::{AirRetriever, Answerer, GbrainRetriever, RetrievedHit};
use memharness::judge::{PairJudge, PosPick};
use memharness::run::{run_queries, QueryCase, QuerySource, RunConfig};

struct AirDouble;
impl AirRetriever for AirDouble {
    fn retrieve(&mut self, _q: &str, _k: usize) -> anyhow::Result<Vec<RetrievedHit>> {
        Ok(vec![RetrievedHit { page_id: "p/air".into(), snippet: "GOODCTX".into() }])
    }
}
struct GbrainDouble;
impl GbrainRetriever for GbrainDouble {
    fn retrieve(&self, _q: &str, _k: usize) -> anyhow::Result<Vec<RetrievedHit>> {
        Ok(vec![RetrievedHit { page_id: "p/gb".into(), snippet: "meh".into() }])
    }
}
struct EchoAnswerer;
impl Answerer for EchoAnswerer {
    fn answer(&self, _q: &str, context: &str) -> anyhow::Result<String> {
        Ok(format!("answer[{context}]"))
    }
}
/// Content judge: prefers GOODCTX (blind-compatible — judges answers, not positions).
struct GoodJudge;
impl PairJudge for GoodJudge {
    fn pick(&self, _q: &str, a: &str, b: &str) -> anyhow::Result<Option<PosPick>> {
        Ok(match (a.contains("GOODCTX"), b.contains("GOODCTX")) {
            (true, false) => Some(PosPick::A),
            (false, true) => Some(PosPick::B),
            _ => Some(PosPick::Tie),
        })
    }
}
/// Contrarian auditor: prefers the answer WITHOUT GOODCTX → 0% agreement with GoodJudge.
struct ContrarianAuditor;
impl PairJudge for ContrarianAuditor {
    fn pick(&self, _q: &str, a: &str, b: &str) -> anyhow::Result<Option<PosPick>> {
        Ok(match (a.contains("GOODCTX"), b.contains("GOODCTX")) {
            (true, false) => Some(PosPick::B),
            (false, true) => Some(PosPick::A),
            _ => Some(PosPick::Tie),
        })
    }
}

fn open_cases(n: usize) -> Vec<QueryCase> {
    (0..n)
        .map(|i| QueryCase {
            text: format!("open query {i}"),
            lang: "en".into(),
            source: QuerySource::Real,
            gold_page_id: None,
        })
        .collect()
}

#[test]
fn agreeing_auditor_trusts_at_the_floor_sample() {
    // 40 open queries > AUDIT_FLOOR → the initial sample is max(30, 6) = 30 pairs; agreement
    // is perfect → trusted, NO expansion, egress == 30.
    let cfg = RunConfig { k: 10, seed: 42, local_only: false };
    let outcome = run_queries(
        &cfg, &open_cases(40), &mut AirDouble, &GbrainDouble, &EchoAnswerer, &GoodJudge,
        Some(&GoodJudge),
    )
    .unwrap();
    assert!(outcome.trust.trusted);
    assert!(!outcome.trust.expanded_to_full_audit);
    assert!(!outcome.trust.audit_n_too_small, "pool 40 ≥ floor 30");
    assert_eq!(outcome.trust.audited_count, 30, "the max(30, 15%·40)=30 sample");
    assert_eq!(outcome.egress_pairs_sent, 30);
    // AIR won every pair (GOODCTX on the AIR side) → win rate 1.0.
    assert!((outcome.segments[0].air_win_rate - 1.0).abs() < 1e-9);
}

#[test]
fn disagreeing_auditor_forces_full_expansion_and_flags_untrusted() {
    let cfg = RunConfig { k: 10, seed: 42, local_only: false };
    let outcome = run_queries(
        &cfg, &open_cases(40), &mut AirDouble, &GbrainDouble, &EchoAnswerer, &GoodJudge,
        Some(&ContrarianAuditor),
    )
    .unwrap();
    assert!(!outcome.trust.trusted, "0% agreement can never trust");
    assert!(outcome.trust.expanded_to_full_audit, "auto-expanded to 100% (spec §5)");
    assert_eq!(outcome.trust.audited_count, 40, "every open pair audited after expansion");
    assert_eq!(outcome.egress_pairs_sent, 40);
}

#[test]
fn local_only_never_egresses() {
    let cfg = RunConfig { k: 10, seed: 42, local_only: true };
    let outcome = run_queries(
        &cfg, &open_cases(40), &mut AirDouble, &GbrainDouble, &EchoAnswerer, &GoodJudge, None,
    )
    .unwrap();
    assert!(outcome.trust.audit_incomplete, "no audit this run");
    assert_eq!(outcome.egress_pairs_sent, 0);
}
