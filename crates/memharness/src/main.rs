//! memharness — DEV-ONLY blind A/B measuring stick: AIR engine vs GBrain, on Peter's own
//! corpus + queries, end-to-end. NEVER SHIPS (see Cargo.toml). Spec (Rev 2):
//! docs/superpowers/specs/2026-07-03-air-agent-memharness-phase0-design.md
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};

use memharness::corpus::CorpusManifest;
use memharness::frontmatter::Lang;
use memharness::judge::PairJudge;
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

#[derive(clap::Args)]
struct RunArgs {
    /// Disable ALL cloud egress (no Anthropic audit; wider error bars).
    #[arg(long)]
    local_only: bool,
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
    memharness::report::ensure_outside_repo(&reports_dir, &repo_root)?;
    memharness::ollama::preflight(&args.model)?;
    let api_key = if args.local_only {
        None
    } else {
        let key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| anyhow::anyhow!("ANTHROPIC_API_KEY not set (or pass --local-only)"))?;
        memharness::anthropic::preflight(&key)?; // one-token call — a bad key/model dies HERE
        Some(key)
    };

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

    // ── Seams. ──
    let mut air = memharness::arms::LiveAirArm::new(rt, client, resolver);
    let gbrain = memharness::arms::GbrainCli { mode: None };
    let answerer = memharness::arms::OllamaAnswerer { model: args.model.clone() };
    let judge = memharness::judge::OllamaJudge { model: args.model.clone() };
    let auditor = api_key.map(|api_key| memharness::anthropic::AnthropicAuditor { api_key });
    let cfg = RunConfig { k: args.k, seed: args.seed, local_only: args.local_only };

    let stage = std::time::Instant::now();
    let outcome = memharness::run::run_queries(
        &cfg,
        &cases,
        &mut air,
        &gbrain,
        &answerer,
        &judge,
        auditor.as_ref().map(|a| a as &dyn PairJudge),
    )?;
    log_stage_secs("run_queries", stage);

    // ── Drift check + report (spec §2/§7 Rev 2). ──
    let (gbrain_version, gbrain_page_count) = memharness::arms::gbrain_version_and_count();
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
        drift_fraction,
        ollama_model: args.model.clone(),
        egress_pairs_sent: outcome.egress_pairs_sent,
        local_only: args.local_only,
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
    log_stage_secs("transcript mining", stage);

    // Synthetic: language per page from its prepared text; seeded stratified selection.
    let stage = std::time::Instant::now();
    use rand::SeedableRng;
    let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(args.seed);
    let pages: Vec<PageRef> = manifest
        .entries
        .iter()
        .map(|e| {
            let text = std::fs::read_to_string(corpus_home.join(format!("{}.md", e.page_id)))
                .unwrap_or_default();
            let lang = match memharness::frontmatter::detect_lang(&text) {
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
        let text = std::fs::read_to_string(corpus_home.join(format!("{}.md", page.page_id)))?;
        synth.extend(generator.generate_queries(page, &text)?);
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
