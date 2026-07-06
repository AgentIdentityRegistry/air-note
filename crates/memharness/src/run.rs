//! Run orchestration (spec §4–§6): the per-query loop, segment bucketing, blind judging, the
//! audit ladder (sample → trust → expand-to-100%), and egress accounting. Pure over the
//! injected seams (`AirRetriever`/`GbrainRetriever`/`Answerer`/`PairJudge`) so e2e #2 drives it
//! hermetically; `main.rs` supplies the live seams.

use std::collections::{BTreeMap, BTreeSet};

use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

use crate::arms::{dedup_by_page, gold_rank, pack_context, AirRetriever, Answerer, GbrainRetriever};
use crate::judge::{
    assign_blind, judge_pair_blind, select_audit_indices, trust_verdict, Blind, PairJudge,
    TrustVerdict, Verdict, AUDIT_FLOOR,
};
use crate::mine::MinedQuery;
use crate::report::{ExamplePair, PackTotals, SegmentResult};
use crate::stats::{
    bootstrap_ci_mean, mean_reciprocal_rank, mean_success_at_k, success_at_k,
    wilcoxon_signed_rank, GoldRank,
};
use crate::synth::SynthQuery;

/// Segment tag: where a query came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum QuerySource {
    Real,
    Synthetic,
}

/// One query case. `gold_page_id`: Some = known-item (mechanical), None = open (judged).
/// Serialized FIELD ORDER is part of the on-disk frozen-list identity (`cases::save_cases`
/// shas the exact bytes) — do not reorder fields or add unordered-map fields.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct QueryCase {
    pub text: String,
    pub lang: String, // "en" | "ko"
    pub source: QuerySource,
    pub gold_page_id: Option<String>,
}

/// Run knobs. `k` is BOTH retrieval depth and scoring depth — retrieval-k == scoring-k, ONE
/// knob (spec §4 Rev 2, finding 14).
pub struct RunConfig {
    pub k: usize,
    pub seed: u64,
    pub local_only: bool,
}

/// One known-item case's mechanical result — persisted in scores.json so a later run over the
/// SAME frozen case list can be compared PAIRED by `case_idx` (spec §3.0.3). Open cases have no
/// mechanical result and never appear here.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CaseResult {
    pub case_idx: usize,
    pub label: String,
    pub air_rank: Option<usize>,
    pub gbrain_rank: Option<usize>,
    pub air_success: bool,
    pub gbrain_success: bool,
}

/// Everything the loop produces; `report.rs` renders it.
pub struct RunOutcome {
    pub segments: Vec<SegmentResult>,
    pub case_results: Vec<CaseResult>,
    pub trust: TrustVerdict,
    pub egress_pairs_sent: usize,
    pub examples: Vec<ExamplePair>,
    pub air_pack: PackTotals,
    pub gbrain_pack: PackTotals,
}

/// Build the unified case list from mined real + synthetic queries (language of a real query
/// comes from its own text via the Hangul heuristic).
pub fn cases_from(mined: Vec<MinedQuery>, synth: Vec<SynthQuery>) -> Vec<QueryCase> {
    let mut out = Vec::with_capacity(mined.len() + synth.len());
    for m in mined {
        let lang = match crate::frontmatter::detect_lang(&m.text) {
            crate::frontmatter::Lang::Ko => "ko".to_string(),
            crate::frontmatter::Lang::En => "en".to_string(),
        };
        out.push(QueryCase { lang, source: QuerySource::Real, gold_page_id: m.gold_page_id, text: m.text });
    }
    for s in synth {
        out.push(QueryCase {
            text: s.text,
            lang: s.lang,
            source: QuerySource::Synthetic,
            gold_page_id: Some(s.gold_page_id),
        });
    }
    out
}

/// A judged open query's full record (kept for auditing + examples).
struct OpenRecord {
    case_idx: usize,
    air_answer: String,
    gbrain_answer: String,
    air_ctx: String,
    gbrain_ctx: String,
    blind: Blind,
    local_verdict: Verdict,
}

/// Per-segment accumulators.
#[derive(Default)]
struct Bucket {
    air_ranks: Vec<GoldRank>,
    gbrain_ranks: Vec<GoldRank>,
    open_air_scores: Vec<f64>, // 1.0 AirWins / 0.0 GbrainWins / 0.5 Tie|Uncertain
    n: usize,
}

fn bucket_label(case: &QueryCase) -> String {
    let src = match case.source {
        QuerySource::Real => "real",
        QuerySource::Synthetic => "synthetic",
    };
    let kind = if case.gold_page_id.is_some() { "known-item" } else { "open" };
    format!("{src}·{}·{kind}", case.lang)
}

/// AIR's numeric score for a verdict (ties/uncertains split the point; gbrain = 1 − air).
fn air_score(v: Verdict) -> f64 {
    match v {
        Verdict::AirWins => 1.0,
        Verdict::GbrainWins => 0.0,
        Verdict::Tie | Verdict::Uncertain => 0.5,
    }
}

const BOOTSTRAP_ITERS: usize = 2000;
const CI_CONF: f64 = 0.95;

/// The whole measurement, over injected seams (spec §4–§6). Deterministic given the seed
/// (fixed query order; seeded blinding/sampling/bootstrap).
pub fn run_queries(
    cfg: &RunConfig,
    cases: &[QueryCase],
    air: &mut dyn AirRetriever,
    gbrain: &dyn GbrainRetriever,
    answerer: &dyn Answerer,
    judge: &dyn PairJudge,
    auditor: Option<&dyn PairJudge>,
) -> anyhow::Result<RunOutcome> {
    let mut rng = ChaCha8Rng::seed_from_u64(cfg.seed);
    let mut buckets: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut opens: Vec<OpenRecord> = Vec::new();
    let mut case_results: Vec<CaseResult> = Vec::new();
    let mut air_pack = PackTotals::default();
    let mut gbrain_pack = PackTotals::default();

    // ── Per-query loop (fixed order = deterministic). ──
    for (case_idx, case) in cases.iter().enumerate() {
        // Both arms retrieve at the SAME k; page-dedup BEFORE any scoring (finding 2).
        let air_hits = dedup_by_page(air.retrieve(&case.text, cfg.k)?);
        let gbrain_hits = dedup_by_page(gbrain.retrieve(&case.text, cfg.k)?);
        let bucket = buckets.entry(bucket_label(case)).or_default();
        bucket.n += 1;

        match &case.gold_page_id {
            // Known-item: mechanical success@k/MRR, no judge (spec §5) — recorded per-case for
            // paired cross-run comparison (spec §3.0.3).
            Some(gold) => {
                let air_rank = gold_rank(&air_hits, gold);
                let gbrain_rank = gold_rank(&gbrain_hits, gold);
                case_results.push(CaseResult {
                    case_idx,
                    label: bucket_label(case),
                    air_rank,
                    gbrain_rank,
                    air_success: success_at_k(&air_rank, cfg.k),
                    gbrain_success: success_at_k(&gbrain_rank, cfg.k),
                });
                bucket.air_ranks.push(air_rank);
                bucket.gbrain_ranks.push(gbrain_rank);
            }
            // Open: identical answerer both arms → blind position-swapped local judging.
            None => {
                let (air_ctx, a_stats) = pack_context(&air_hits);
                let (gbrain_ctx, g_stats) = pack_context(&gbrain_hits);
                air_pack.add(&a_stats);
                gbrain_pack.add(&g_stats);
                let air_answer = answerer.answer(&case.text, &air_ctx)?;
                let gbrain_answer = answerer.answer(&case.text, &gbrain_ctx)?;
                let blind = assign_blind(&mut rng);
                let local_verdict =
                    judge_pair_blind(judge, blind, &case.text, &air_answer, &gbrain_answer)?;
                opens.push(OpenRecord {
                    case_idx, air_answer, gbrain_answer, air_ctx, gbrain_ctx, blind, local_verdict,
                });
            }
        }
    }

    // ── Audit ladder (spec §5 Rev 2): sample ∪ uncertains → trust → expand if untrusted. ──
    let uncertain_idx: Vec<usize> = opens
        .iter()
        .enumerate()
        .filter(|(_, o)| o.local_verdict == Verdict::Uncertain)
        .map(|(i, _)| i)
        .collect();
    let audit_n_too_small = opens.len() < AUDIT_FLOOR;
    let mut egress_pairs_sent = 0usize;
    let mut audit_failed = false;
    let mut audited: BTreeMap<usize, Verdict> = BTreeMap::new();

    let trust = match auditor {
        Some(auditor) => {
            let initial = select_audit_indices(opens.len(), &uncertain_idx, &mut rng);
            audit_set(&initial, &opens, cases, auditor, &mut audited, &mut egress_pairs_sent, &mut audit_failed);
            let mut t = trust_from(&opens, &audited, false, audit_failed, audit_n_too_small);
            if !t.trusted && !t.audit_incomplete {
                // Auto-expand to 100% of open queries and recompute (spec §5).
                let all: BTreeSet<usize> = (0..opens.len()).collect();
                audit_set(&all, &opens, cases, auditor, &mut audited, &mut egress_pairs_sent, &mut audit_failed);
                t = trust_from(&opens, &audited, true, audit_failed, audit_n_too_small);
            }
            t
        }
        // --local-only: no audit; the report says so plainly (spec §6).
        None => trust_verdict(&[], &[], false, true, audit_n_too_small),
    };

    // ── Fold LOCAL verdicts into buckets. (The audit measures the JUDGE; it does not replace
    //    the judge's scores — spec §5's trust contract.) ──
    for o in &opens {
        let label = bucket_label(&cases[o.case_idx]);
        buckets
            .get_mut(&label)
            .expect("bucket created in the loop above")
            .open_air_scores
            .push(air_score(o.local_verdict));
    }

    // ── Per-bucket stats (BTreeMap order + one threaded rng = deterministic). ──
    let mut segments = Vec::with_capacity(buckets.len());
    for (label, b) in &buckets {
        segments.push(segment_result(label, b, cfg.k, &mut rng));
    }

    let examples = pick_examples(&opens, cases);
    Ok(RunOutcome { segments, case_results, trust, egress_pairs_sent, examples, air_pack, gbrain_pack })
}

/// Audit every not-yet-audited index in `set`: the auditor judges the SAME pair blind +
/// position-swapped (2 API calls = ONE egressed pair). A call failure sets `audit_failed`
/// (→ "audit incomplete", trust unavailable — never fabricated) and stops further egress.
fn audit_set(
    set: &BTreeSet<usize>,
    opens: &[OpenRecord],
    cases: &[QueryCase],
    auditor: &dyn PairJudge,
    audited: &mut BTreeMap<usize, Verdict>,
    egress_pairs_sent: &mut usize,
    audit_failed: &mut bool,
) {
    for &i in set {
        if audited.contains_key(&i) || *audit_failed {
            continue;
        }
        let o = &opens[i];
        *egress_pairs_sent += 1;
        match judge_pair_blind(auditor, o.blind, &cases[o.case_idx].text, &o.air_answer, &o.gbrain_answer) {
            Ok(v) => {
                audited.insert(i, v);
            }
            Err(e) => {
                eprintln!("memharness: cloud audit failed on pair {i}: {e} — trust verdict will be unavailable");
                *audit_failed = true;
            }
        }
    }
}

/// Pair local vs cloud verdicts over the audited set → the trust verdict.
fn trust_from(
    opens: &[OpenRecord],
    audited: &BTreeMap<usize, Verdict>,
    expanded: bool,
    audit_failed: bool,
    audit_n_too_small: bool,
) -> TrustVerdict {
    let local: Vec<Verdict> = audited.keys().map(|&i| opens[i].local_verdict).collect();
    let cloud: Vec<Verdict> = audited.values().copied().collect();
    let incomplete = audit_failed || (local.is_empty() && !opens.is_empty());
    trust_verdict(&local, &cloud, expanded, incomplete, audit_n_too_small)
}

/// One segment row. Known-item buckets: success@k/MRR both arms + CI/Wilcoxon over paired
/// binary success flags (tie-heavy — the report carries the caveat). Open buckets: win-rate =
/// mean AIR score + CI/Wilcoxon over paired scores (gbrain = 1 − air by construction).
fn segment_result<R: rand::Rng>(label: &str, b: &Bucket, k: usize, rng: &mut R) -> SegmentResult {
    if !b.air_ranks.is_empty() {
        let air_flags: Vec<f64> =
            b.air_ranks.iter().map(|r| if success_at_k(r, k) { 1.0 } else { 0.0 }).collect();
        let gb_flags: Vec<f64> =
            b.gbrain_ranks.iter().map(|r| if success_at_k(r, k) { 1.0 } else { 0.0 }).collect();
        let diffs: Vec<f64> = air_flags.iter().zip(&gb_flags).map(|(a, g)| a - g).collect();
        let (ci_low, ci_high) = bootstrap_ci_mean(&diffs, BOOTSTRAP_ITERS, CI_CONF, rng);
        SegmentResult {
            label: label.to_string(),
            n: b.n,
            air_success_at_k: mean_success_at_k(&b.air_ranks, k),
            gbrain_success_at_k: mean_success_at_k(&b.gbrain_ranks, k),
            air_mrr: mean_reciprocal_rank(&b.air_ranks),
            gbrain_mrr: mean_reciprocal_rank(&b.gbrain_ranks),
            air_win_rate: 0.0, // known-item rows are mechanical, not win-rated
            ci_low,
            ci_high,
            wilcoxon: Some(wilcoxon_signed_rank(&air_flags, &gb_flags)),
        }
    } else {
        let air_scores = &b.open_air_scores;
        let gb_scores: Vec<f64> = air_scores.iter().map(|s| 1.0 - s).collect();
        let (ci_low, ci_high) = bootstrap_ci_mean(air_scores, BOOTSTRAP_ITERS, CI_CONF, rng);
        let win_rate = if air_scores.is_empty() {
            0.0
        } else {
            air_scores.iter().sum::<f64>() / air_scores.len() as f64
        };
        SegmentResult {
            label: label.to_string(),
            n: b.n,
            air_success_at_k: 0.0,
            gbrain_success_at_k: 0.0,
            air_mrr: 0.0,
            gbrain_mrr: 0.0,
            air_win_rate: win_rate,
            ci_low,
            ci_high,
            wilcoxon: Some(wilcoxon_signed_rank(air_scores, &gb_scores)),
        }
    }
}

/// Up to 5 AIR wins + 5 AIR losses, each with both retrieved contexts (spec §7).
fn pick_examples(opens: &[OpenRecord], cases: &[QueryCase]) -> Vec<ExamplePair> {
    let mut out = Vec::new();
    let (mut wins, mut losses) = (0usize, 0usize);
    for o in opens {
        let winner = match o.local_verdict {
            Verdict::AirWins if wins < 5 => {
                wins += 1;
                "AIR"
            }
            Verdict::GbrainWins if losses < 5 => {
                losses += 1;
                "GBrain"
            }
            _ => continue,
        };
        out.push(ExamplePair {
            query: cases[o.case_idx].text.clone(),
            winner: winner.to_string(),
            air_context: o.air_ctx.clone(),
            gbrain_context: o.gbrain_ctx.clone(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arms::{AirRetriever, Answerer, GbrainRetriever, RetrievedHit};
    use crate::judge::{PairJudge, PosPick};

    /// AIR double: gold at rank 0 for known-item cases; a GOODCTX snippet for open case 1.
    struct AirDouble;
    impl AirRetriever for AirDouble {
        fn retrieve(&mut self, query: &str, _k: usize) -> anyhow::Result<Vec<RetrievedHit>> {
            Ok(match query {
                "known-air-wins" => vec![
                    RetrievedHit { page_id: "en/alpha".into(), snippet: "a".into() },
                    // A duplicate page hit: dedup must collapse it BEFORE ranking.
                    RetrievedHit { page_id: "en/alpha".into(), snippet: "a-dup".into() },
                ],
                "open-air-good" => vec![RetrievedHit { page_id: "x".into(), snippet: "GOODCTX".into() }],
                _ => vec![RetrievedHit { page_id: "y".into(), snippet: "meh".into() }],
            })
        }
    }
    /// GBrain double: misses the gold; a GOODCTX snippet for open case 2.
    struct GbrainDouble;
    impl GbrainRetriever for GbrainDouble {
        fn retrieve(&self, query: &str, _k: usize) -> anyhow::Result<Vec<RetrievedHit>> {
            Ok(match query {
                "open-gbrain-good" => vec![RetrievedHit { page_id: "z".into(), snippet: "GOODCTX".into() }],
                _ => vec![RetrievedHit { page_id: "en/beta".into(), snippet: "b".into() }],
            })
        }
    }
    /// Answerer double: echoes its context (so the judge can see which arm had GOODCTX).
    struct EchoAnswerer;
    impl Answerer for EchoAnswerer {
        fn answer(&self, _q: &str, context: &str) -> anyhow::Result<String> {
            Ok(format!("answer[{context}]"))
        }
    }
    /// Judge double: prefers the answer containing GOODCTX (content-based → blind-compatible).
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

    fn cases() -> Vec<QueryCase> {
        vec![
            QueryCase { text: "known-air-wins".into(), lang: "en".into(), source: QuerySource::Synthetic, gold_page_id: Some("en/alpha".into()) },
            QueryCase { text: "open-air-good".into(), lang: "en".into(), source: QuerySource::Real, gold_page_id: None },
            QueryCase { text: "open-gbrain-good".into(), lang: "ko".into(), source: QuerySource::Real, gold_page_id: None },
        ]
    }

    #[test]
    fn run_queries_buckets_judges_audits_and_counts_egress() {
        let cfg = RunConfig { k: 10, seed: 42, local_only: false };
        let outcome = run_queries(
            &cfg, &cases(), &mut AirDouble, &GbrainDouble, &EchoAnswerer, &GoodJudge,
            Some(&GoodJudge), // auditor = same content judge → 100% agreement
        )
        .unwrap();

        // Buckets: synthetic·en·known-item, real·en·open, real·ko·open (BTreeMap order).
        let labels: Vec<&str> = outcome.segments.iter().map(|s| s.label.as_str()).collect();
        assert!(labels.contains(&"synthetic·en·known-item"));
        assert!(labels.contains(&"real·en·open"));
        assert!(labels.contains(&"real·ko·open"));

        // Known-item: AIR found gold at rank 0 (after page-dedup), GBrain missed.
        let ki = outcome.segments.iter().find(|s| s.label == "synthetic·en·known-item").unwrap();
        assert!((ki.air_success_at_k - 1.0).abs() < 1e-9);
        assert!((ki.gbrain_success_at_k - 0.0).abs() < 1e-9);
        assert!((ki.air_mrr - 1.0).abs() < 1e-9, "dup page hit did not displace the rank");

        // Open verdicts: AIR wins its GOODCTX case, loses the other.
        let en_open = outcome.segments.iter().find(|s| s.label == "real·en·open").unwrap();
        assert!((en_open.air_win_rate - 1.0).abs() < 1e-9);
        let ko_open = outcome.segments.iter().find(|s| s.label == "real·ko·open").unwrap();
        assert!((ko_open.air_win_rate - 0.0).abs() < 1e-9);

        // Audit: pool (2 open) < AUDIT_FLOOR → whole pool audited, flag set, perfect agreement.
        assert!(outcome.trust.trusted);
        assert!(outcome.trust.audit_n_too_small);
        assert_eq!(outcome.trust.audited_count, 2);
        assert_eq!(outcome.egress_pairs_sent, 2, "one egressed pair per audited open query");

        // Examples: one win + one loss captured with contexts.
        assert_eq!(outcome.examples.len(), 2);

        // Pack totals: every open-case hit packed on both arms.
        assert_eq!(outcome.air_pack.sources_packed, 2);
        assert_eq!(outcome.gbrain_pack.sources_packed, 2);

        // Per-case mechanical results (spec §3.0.3): exactly the ONE known-item case, with
        // ranks/flags matching the bucket math; opens are NOT recorded here.
        assert_eq!(outcome.case_results.len(), 1);
        let cr = &outcome.case_results[0];
        assert_eq!(cr.case_idx, 0, "case identity = index into the (frozen) case list");
        assert_eq!(cr.label, "synthetic·en·known-item");
        assert_eq!(cr.air_rank, Some(0));
        assert_eq!(cr.gbrain_rank, None);
        assert!(cr.air_success && !cr.gbrain_success);
    }

    #[test]
    fn local_only_skips_audit_and_reports_it() {
        let cfg = RunConfig { k: 10, seed: 42, local_only: true };
        let outcome = run_queries(
            &cfg, &cases(), &mut AirDouble, &GbrainDouble, &EchoAnswerer, &GoodJudge, None,
        )
        .unwrap();
        assert!(outcome.trust.audit_incomplete, "no audit this run");
        assert!(!outcome.trust.trusted);
        assert_eq!(outcome.egress_pairs_sent, 0, "--local-only ⇒ zero cloud egress");
    }
}
