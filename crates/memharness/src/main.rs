//! memharness — DEV-ONLY blind A/B measuring stick: AIR engine vs GBrain, on Peter's own
//! corpus + queries, end-to-end. NEVER SHIPS (see Cargo.toml). Spec (Rev 2):
//! docs/superpowers/specs/2026-07-03-air-agent-memharness-phase0-design.md
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use clap::{Parser, Subcommand};

use memharness::arms::{AirRetriever, Answerer, GbrainRetriever, RetrievedHit};
use memharness::corpus::CorpusManifest;
use memharness::frontmatter::Lang;
use memharness::judge::{PairJudge, PosPick};
use memharness::run::{cases_from, QueryCase, QuerySource, RunConfig};
use memharness::synth::{PageRef, QueryGenerator};

#[derive(Parser)]
#[command(name = "memharness", about = "DEV-ONLY blind A/B measuring stick: AIR engine vs GBrain.")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run one A/B measurement and write a report.
    Run(RunArgs),
}

/// Which model judges open-query answer quality (known-item scoring is mechanical, no judge).
#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum JudgeMode {
    /// Local Ollama model — free + private, but the 2026-07-06 baseline showed it agreeing with
    /// the cloud reference only ~53% of the time, so its open-query verdicts aren't yet trustworthy.
    Local,
    /// Haiku via the API — sharp, pennies, zero local load. Needs a key; the audit ladder
    /// self-checks Haiku against the Sonnet audit model. Incompatible with --local-only.
    Cloud,
}

#[derive(clap::Args)]
struct RunArgs {
    /// Disable ALL cloud egress (no Anthropic audit; wider error bars).
    #[arg(long)]
    local_only: bool,
    /// Who judges open-query answer quality: `local` (Ollama) or `cloud` (Haiku, self-checked
    /// against Sonnet). Cloud needs an ANTHROPIC_API_KEY and is incompatible with --local-only.
    #[arg(long, value_enum, default_value = "local")]
    judge: JudgeMode,
    /// Local Ollama model for answerer/judge/synth.
    #[arg(long, default_value = memharness::ollama::DEFAULT_OLLAMA_MODEL)]
    model: String,
    /// Retrieval depth AND scoring depth for both arms (one knob: retrieval-k == scoring-k).
    #[arg(long, default_value_t = 10)]
    k: usize,
    /// Corpus source (default ~/brain).
    #[arg(long)]
    corpus: Option<PathBuf>,
    /// Reports output dir (default ~/.air-harness/reports).
    #[arg(long)]
    reports_dir: Option<PathBuf>,
    /// RNG seed for all sampling/blinding/bootstrap.
    #[arg(long, default_value_t = 42)]
    seed: u64,
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Run(args) => run(args),
    }
}

/// Synthetic-query volume target (spec §3: ~200–400).
const SYNTH_TARGET: usize = 300;

/// One wall-clock line per stage — the run's budget math is dominated by per-call costs
/// (measured ~4s per gbrain CLI call), so each stage's real cost must be visible.
fn log_stage_secs(stage: &str, started: std::time::Instant) {
    eprintln!("memharness: [t] {stage} took {:.1}s", started.elapsed().as_secs_f64());
}

fn run(args: RunArgs) -> anyhow::Result<()> {
    let run_started = std::time::Instant::now();
    let corpus_src = args.corpus.clone().unwrap_or_else(|| dirs_home().join("brain"));
    let reports_dir = args
        .reports_dir
        .clone()
        .unwrap_or_else(|| dirs_home().join(".air-harness").join("reports"));
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    // ── Fail-fast preflights BEFORE any expensive work (spec §5 Rev 2). ──
    if args.judge == JudgeMode::Cloud && args.local_only {
        anyhow::bail!(
            "--judge cloud needs the cloud (an ANTHROPIC_API_KEY) and is incompatible with --local-only"
        );
    }
    memharness::report::ensure_outside_repo(&reports_dir, &repo_root)?;
    memharness::ollama::preflight(&args.model)?;
    let api_key = if args.local_only {
        None
    } else {
        let key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| anyhow::anyhow!("ANTHROPIC_API_KEY not set (or pass --local-only)"))?;
        // Preflight the AUDIT model (Sonnet) — always the reference the judge is checked against.
        memharness::anthropic::preflight(&key, memharness::anthropic::AUDIT_MODEL)?;
        // And the cloud JUDGE model (Haiku) when it will do the judging — a bad key/model dies HERE.
        if args.judge == JudgeMode::Cloud {
            memharness::anthropic::preflight(&key, memharness::anthropic::JUDGE_MODEL)?;
        }
        Some(key)
    };
    // Loud gbrain preflight (review follow-up): a missing binary or locked DB fails in ~5s
    // here, not after the ~1h synth stage. The count also feeds the EARLY drift bail below.
    let gbrain_pages = memharness::arms::gbrain_preflight()?;
    eprintln!("memharness: gbrain preflight OK — {gbrain_pages} pages indexed");

    // ── Daemon with the REAL embedder (spec §1 Rev 2 — never the mock on the live path). ──
    let mut daemon = memharness::daemon::HarnessDaemon::spawn_real()?;
    let corpus_home = daemon.home().join("corpus");
    std::fs::create_dir_all(&corpus_home)?;
    let stage = std::time::Instant::now();
    let manifest = memharness::corpus::prepare_corpus(
        &corpus_src,
        &corpus_home,
        memharness::corpus::STRIP_FRONTMATTER, // Probe-A-pinned (spec §2 Rev 2)
    )?;
    log_stage_secs("corpus prep", stage);
    if manifest.file_count == 0 {
        anyhow::bail!("corpus at {corpus_src:?} contains no .md pages");
    }
    eprintln!("memharness: corpus prepared — {} pages, {} bytes", manifest.file_count, manifest.total_bytes);
    // Early drift bail (review follow-up): additive INSURANCE — the end-of-run measurement
    // still lands in the report; this refuses to SPEND the run budget on a result already
    // known to be INVALID. Scoped to the ~/brain corpus: drift vs the gbrain index is only
    // meaningful when the harness corpus IS the brain gbrain indexes. A fixture/experiment
    // `--corpus` (the runbook's smoke run) diverges from the index structurally and must still
    // reach the full pipeline — its report carries the INVALID banner instead of bailing here.
    // Both sides are CANONICALIZED so a trailing slash, a symlinked ~/brain, or $HOME variance
    // can't silently disable the insurance (corpus_src exists — we just prepared it; if ~/brain
    // is absent, canonicalize→None→not-brain→fall through to the report banner, never a wrong
    // early bail). (Structural baseline today: 867 harness pages vs 895 indexed ≈ 3.2% — under
    // the threshold, but with <2% headroom.)
    let corpus_is_brain = match (
        std::fs::canonicalize(&corpus_src).ok(),
        std::fs::canonicalize(dirs_home().join("brain")).ok(),
    ) {
        (Some(corpus), Some(brain)) => corpus == brain,
        _ => false,
    };
    let drift = (gbrain_pages as f64 - manifest.file_count as f64).abs()
        / manifest.file_count.max(1) as f64;
    if corpus_is_brain && drift > memharness::report::DRIFT_INVALID_FRACTION {
        anyhow::bail!(
            "corpus drift {:.1}% > {:.0}% — the run would render INVALID; re-sync GBrain (or \
             investigate exclusion asymmetry: dot-entries/symlinks/non-md are skipped by the \
             harness) before spending the run budget",
            drift * 100.0,
            memharness::report::DRIFT_INVALID_FRACTION * 100.0
        );
    }

    // ── Ingest over the wire + build the event→page bridge (spec §5 Rev 2). ──
    let stage = std::time::Instant::now();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let mut client = rt.block_on(memharness::client::WireClient::connect(daemon.socket_path()))?;
    rt.block_on(client.add_grant(&corpus_home))?;
    let ingest = rt.block_on(client.run_ingest())?;
    eprintln!("memharness: ingested {} ({} failed)", ingest.ingested, ingest.failed.len());
    if !ingest.failed.is_empty() {
        anyhow::bail!("{} ingest failures — refusing to score a partial corpus", ingest.failed.len());
    }
    let records = rt.block_on(client.list_files())?;
    let resolver = memharness::resolve::PageResolver::from_file_records(&records, &corpus_home)?;
    log_stage_secs("ingest+bridge", stage);

    // ── Query set: mined real + synthetic (both seeded/deterministic). ──
    let cases = build_query_cases(&args, &manifest, &corpus_home)?;
    eprintln!("memharness: {} query cases built", cases.len());

    // ── Seams (wrapped in the live-only visibility decorators below). ──
    let mut air = HeartbeatAir {
        inner: memharness::arms::LiveAirArm::new(rt, client, resolver),
        done: 0,
        total: cases.len(),
        started: std::time::Instant::now(),
    };
    let gbrain = GbrainWithContext(memharness::arms::GbrainCli { mode: None });
    let answerer = AnswererWithContext(memharness::arms::OllamaAnswerer { model: args.model.clone() });
    let local_judge = JudgeWithContext(memharness::judge::OllamaJudge { model: args.model.clone() });
    // Auditor (Sonnet): the reference the judge is checked against; present whenever a key is.
    let auditor = api_key.as_ref().map(|k| memharness::anthropic::AnthropicAuditor {
        api_key: k.clone(),
        model: memharness::anthropic::AUDIT_MODEL.to_string(),
    });
    // Judge: local Ollama (default) or cloud Haiku (`--judge cloud`; key validated above). With a
    // cloud judge, judge=Haiku + auditor=Sonnet, so the trust verdict becomes "is Haiku trustworthy?".
    let cloud_judge = match (args.judge, &api_key) {
        (JudgeMode::Cloud, Some(k)) => Some(memharness::anthropic::AnthropicAuditor {
            api_key: k.clone(),
            model: memharness::anthropic::JUDGE_MODEL.to_string(),
        }),
        _ => None,
    };
    let judge: &dyn PairJudge = match &cloud_judge {
        Some(cj) => cj,
        None => &local_judge,
    };
    let cfg = RunConfig { k: args.k, seed: args.seed, local_only: args.local_only };

    let stage = std::time::Instant::now();
    let outcome = memharness::run::run_queries(
        &cfg,
        &cases,
        &mut air,
        &gbrain,
        &answerer,
        judge,
        auditor.as_ref().map(|a| a as &dyn PairJudge),
    )?;
    log_stage_secs("run_queries", stage);

    // ── Drift check + report (spec §2/§7 Rev 2). ──
    let (gbrain_version, gbrain_page_count) = memharness::arms::gbrain_version_and_count();
    let (gbrain_mode, gbrain_reranker) = memharness::arms::gbrain_pipeline_fingerprint();
    let drift_fraction = gbrain_page_count.map(|p| {
        (p as f64 - manifest.file_count as f64).abs() / (manifest.file_count.max(1)) as f64
    });
    let report = memharness::report::ReportModel {
        trust: outcome.trust,
        k: args.k,
        segments: outcome.segments,
        corpus: manifest,
        gbrain_version,
        gbrain_page_count,
        gbrain_mode,
        gbrain_reranker,
        drift_fraction,
        ollama_model: args.model.clone(),
        egress_pairs_sent: outcome.egress_pairs_sent,
        local_only: args.local_only,
        cloud_judge_model: (args.judge == JudgeMode::Cloud)
            .then(|| memharness::anthropic::JUDGE_MODEL.to_string()),
        near_dedup_applied: false, // exact-only dedup — an EXPLICIT caveat, not a silent gap
        air_pack: outcome.air_pack,
        gbrain_pack: outcome.gbrain_pack,
        examples: outcome.examples,
    };
    let stage = std::time::Instant::now();
    let out_dir = memharness::report::write_report(&reports_dir, &repo_root, &report)?;
    log_stage_secs("report write", stage);
    eprintln!("memharness: report written to {}", out_dir.display());
    daemon.kill();
    log_stage_secs("total", run_started);
    Ok(())
}

// ── Live-only seam decorators (review follow-up). run.rs stays byte-pinned; these wrap the
// live seams HERE so a ~2h run is observable (query N/M heartbeat) and a mid-run failure
// names its position/query. They add VISIBILITY, not recovery — fail-loud is preserved:
// every error still propagates, just with context attached. ──

/// Heartbeat cadence: print on the first AIR call and every Nth after.
const HEARTBEAT_EVERY: usize = 25;

/// Query-preview length for error context (chars, not bytes — KO-safe).
const QUERY_PREVIEW_CHARS: usize = 120;

/// Char-boundary-safe query preview for error messages.
fn query_preview(q: &str) -> String {
    if q.chars().count() > QUERY_PREVIEW_CHARS {
        let cut: String = q.chars().take(QUERY_PREVIEW_CHARS).collect();
        format!("{cut}…")
    } else {
        q.to_string()
    }
}

/// AIR-arm decorator: progress heartbeat + positional error context. The AIR arm leads every
/// per-query iteration in `run_queries`, so its call count IS the run's progress counter.
struct HeartbeatAir {
    inner: memharness::arms::LiveAirArm,
    done: usize,
    total: usize,
    started: std::time::Instant,
}

impl AirRetriever for HeartbeatAir {
    fn retrieve(&mut self, query: &str, k: usize) -> anyhow::Result<Vec<RetrievedHit>> {
        self.done += 1;
        if self.done == 1 || self.done.is_multiple_of(HEARTBEAT_EVERY) {
            eprintln!(
                "memharness: query {}/{} ({:.0}s elapsed)",
                self.done,
                self.total,
                self.started.elapsed().as_secs_f64()
            );
        }
        self.inner
            .retrieve(query, k)
            .with_context(|| format!("AIR retrieval failed on query {}/{}", self.done, self.total))
    }
}

/// GBrain-arm decorator: error context naming the query (no counting).
struct GbrainWithContext(memharness::arms::GbrainCli);

impl GbrainRetriever for GbrainWithContext {
    fn retrieve(&self, query: &str, k: usize) -> anyhow::Result<Vec<RetrievedHit>> {
        self.0
            .retrieve(query, k)
            .with_context(|| format!("GBrain retrieval failed on query: {}", query_preview(query)))
    }
}

/// Answerer decorator: error context naming the query (no counting).
struct AnswererWithContext(memharness::arms::OllamaAnswerer);

impl Answerer for AnswererWithContext {
    fn answer(&self, query: &str, context: &str) -> anyhow::Result<String> {
        self.0
            .answer(query, context)
            .with_context(|| format!("answerer failed on query: {}", query_preview(query)))
    }
}

/// Local-judge decorator: error context naming the query (no counting). The cloud auditor is
/// NOT wrapped — `run_queries` already names the failing pair index on audit errors.
struct JudgeWithContext(memharness::judge::OllamaJudge);

impl PairJudge for JudgeWithContext {
    fn pick(&self, query: &str, answer_a: &str, answer_b: &str) -> anyhow::Result<Option<PosPick>> {
        self.0
            .pick(query, answer_a, answer_b)
            .with_context(|| format!("local judge failed on query: {}", query_preview(query)))
    }
}

/// Mined real queries (every transcript under ~/.claude/projects) + seeded stratified
/// synthetic generation over the prepared corpus.
fn build_query_cases(
    args: &RunArgs,
    manifest: &CorpusManifest,
    corpus_home: &Path,
) -> anyhow::Result<Vec<QueryCase>> {
    // Real: read every *.jsonl under ~/.claude/projects (best-effort per file). Paths are
    // gathered FIRST and SORTED before reading — `read_dir` order is nondeterministic, and a
    // same-text query mined from two files takes its label from whichever comes first, so
    // unsorted order would change the real-query pool between runs.
    let stage = std::time::Instant::now();
    let mut paths: Vec<PathBuf> = Vec::new();
    collect_jsonl(&dirs_home().join(".claude/projects"), &mut paths);
    paths.sort();
    let docs: Vec<String> = paths.iter().filter_map(|p| std::fs::read_to_string(p).ok()).collect();
    eprintln!("memharness: {} transcript files read", docs.len());
    let mined = memharness::mine::mine_all(docs.iter().map(String::as_str));
    eprintln!("memharness: {} real queries after dedup", mined.len());
    if mined.is_empty() {
        eprintln!(
            "memharness: WARNING — zero real queries mined; transcript shape may have drifted \
             (re-check Probe C); proceeding synth-only"
        );
    }
    log_stage_secs("transcript mining", stage);

    // Synthetic: language per page from its prepared text; seeded stratified selection.
    let stage = std::time::Instant::now();
    use rand::SeedableRng;
    let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(args.seed);
    // Read every prepared page ONCE, propagating errors — an unreadable page we JUST wrote is a
    // real anomaly, not something to silently mislabel as English (the old `.unwrap_or_default()`
    // could bucket a KO page into the EN segment, defeating the whole point of the bilingual
    // split). Reused for BOTH language detection and synth generation below (no double read of
    // the sampled pages). Path reconstruction from the NFC-normalized page id relies on APFS
    // being case/normalization-insensitive (macOS-only dev tool).
    let mut page_text: std::collections::HashMap<String, String> =
        std::collections::HashMap::with_capacity(manifest.entries.len());
    for e in &manifest.entries {
        let text = std::fs::read_to_string(corpus_home.join(format!("{}.md", e.page_id)))
            .with_context(|| format!("reading prepared page {} (language detection)", e.page_id))?;
        page_text.insert(e.page_id.clone(), text);
    }
    let pages: Vec<PageRef> = manifest
        .entries
        .iter()
        .map(|e| {
            let lang = match memharness::frontmatter::detect_lang(&page_text[&e.page_id]) {
                Lang::Ko => "ko".to_string(),
                Lang::En => "en".to_string(),
            };
            PageRef { page_id: e.page_id.clone(), lang }
        })
        .collect();
    let sampled = memharness::synth::stratified_sample(&pages, SYNTH_TARGET, &mut rng);
    let generator = memharness::synth::OllamaQueryGenerator { model: args.model.clone() };
    let mut synth = Vec::with_capacity(sampled.len());
    for page in &sampled {
        synth.extend(generator.generate_queries(page, &page_text[&page.page_id])?);
    }
    eprintln!("memharness: {} synthetic queries generated", synth.len());
    log_stage_secs("synth generation", stage);

    let cases = cases_from(mined, synth);
    // A mined gold page can predate the corpus snapshot (renamed/deleted since the transcript).
    // Such cases score as misses on BOTH arms — symmetric, so scores stay comparable — but a
    // large count silently weakens the real known-item segment; make it visible.
    let corpus_ids: std::collections::HashSet<&str> =
        manifest.entries.iter().map(|e| e.page_id.as_str()).collect();
    let absent_gold = cases
        .iter()
        .filter(|c| {
            c.source == QuerySource::Real
                && c.gold_page_id.as_deref().is_some_and(|g| !corpus_ids.contains(g))
        })
        .count();
    eprintln!(
        "memharness: {absent_gold} real known-item queries reference pages absent from the corpus \
         (they score as misses on both arms — symmetric)"
    );
    Ok(cases)
}

/// Recursively gather *.jsonl PATHS (unreadable dirs/files are skipped — transcripts are
/// best-effort input, and a single unreadable one must not kill the run). Callers must sort
/// before reading: `read_dir` order is nondeterministic and mining labels first-occurrence-wins.
fn collect_jsonl(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod cli_tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_run_with_defaults_and_flags() {
        let cli = Cli::parse_from(["memharness", "run"]);
        let Command::Run(args) = cli.command;
        assert_eq!(args.k, 10, "default k (retrieval == scoring)");
        assert_eq!(args.model, memharness::ollama::DEFAULT_OLLAMA_MODEL);
        assert_eq!(args.seed, 42, "default seed");
        assert!(!args.local_only);
        assert!(args.judge == JudgeMode::Local, "default judge = local (backward compatible)");

        let cli = Cli::parse_from(["memharness", "run", "--judge", "cloud"]);
        let Command::Run(args) = cli.command;
        assert!(args.judge == JudgeMode::Cloud, "--judge cloud parses");

        let cli = Cli::parse_from([
            "memharness", "run", "--local-only", "--k", "5", "--model", "llama3:8b",
            "--seed", "7", "--corpus", "/tmp/brain", "--reports-dir", "/tmp/reports",
        ]);
        let Command::Run(args) = cli.command;
        assert!(args.local_only);
        assert_eq!(args.k, 5);
        assert_eq!(args.model, "llama3:8b");
        assert_eq!(args.seed, 7);
        assert_eq!(args.corpus, Some("/tmp/brain".into()));
        assert_eq!(args.reports_dir, Some("/tmp/reports".into()));
    }
}
