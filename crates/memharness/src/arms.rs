//! The two retrieval arms + the shared answerer seam (spec §4). AIR = wire recall → PageResolver
//! (fail-loud) → hits. GBrain = `gbrain call query` subprocess, JSON output parsed per Probe A.
//! Both feed the SAME `Answerer` with a context budgeted by NUMBER OF HITS (k) — not chars
//! (Rev 2: a char budget reintroduces the chunk-vs-page truncation confound). Retrieval seams
//! are traits so the run loop tests hermetically.

use crate::corpus::page_id_from_gbrain_slug;
use crate::resolve::PageResolver;

/// One retrieved hit, normalized to the arm-independent page-id space.
#[derive(Debug, Clone)]
pub struct RetrievedHit {
    pub page_id: String,
    pub snippet: String,
}

/// 0-based rank of `gold_page_id` in the (page-deduped) hits, or None.
pub fn gold_rank(hits: &[RetrievedHit], gold_page_id: &str) -> Option<usize> {
    hits.iter().position(|h| h.page_id == gold_page_id)
}

/// Dedup by page id, FIRST occurrence (best rank) wins — applied to BOTH arms before rank
/// scoring (Rev 2, finding 2: multi-chunk hits from one page must not distort success@k/MRR).
pub fn dedup_by_page(hits: Vec<RetrievedHit>) -> Vec<RetrievedHit> {
    let mut seen = std::collections::HashSet::new();
    hits.into_iter().filter(|h| seen.insert(h.page_id.clone())).collect()
}

// ── Context packing (spec §4 Rev 2): budget = NUMBER OF HITS (k). ──

/// The ONLY truncation: a per-snippet safety cap for the local model's context window,
/// applied IDENTICALLY to both arms and counted. In chars (not bytes) — KO-safe.
pub const PER_SNIPPET_CHAR_CAP: usize = 4000;

/// Per-arm packing stats, recorded in the report (with the chunk-vs-page granularity note).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct PackStats {
    pub sources_packed: usize,
    pub snippets_truncated: usize,
    pub chars_packed: usize,
}

/// Pack ALL hits (budget = hit count = k). Char-boundary-safe per-snippet cap; stats returned.
pub fn pack_context(hits: &[RetrievedHit]) -> (String, PackStats) {
    let mut ctx = String::new();
    let mut stats = PackStats::default();
    for h in hits {
        let n_chars = h.snippet.chars().count();
        let snippet: std::borrow::Cow<'_, str> = if n_chars > PER_SNIPPET_CHAR_CAP {
            stats.snippets_truncated += 1;
            std::borrow::Cow::Owned(h.snippet.chars().take(PER_SNIPPET_CHAR_CAP).collect())
        } else {
            std::borrow::Cow::Borrowed(h.snippet.as_str())
        };
        stats.sources_packed += 1;
        stats.chars_packed += snippet.chars().count();
        ctx.push_str(&snippet);
        ctx.push('\n');
    }
    (ctx, stats)
}

// ── Retrieval seams (Rev 2, finding 4: injectable for the hermetic run-loop test). ──

/// The AIR arm seam. `&mut` because the live impl drives an async wire client on an owned
/// runtime.
pub trait AirRetriever {
    fn retrieve(&mut self, query: &str, k: usize) -> anyhow::Result<Vec<RetrievedHit>>;
}

/// The GBrain arm seam.
pub trait GbrainRetriever {
    fn retrieve(&self, query: &str, k: usize) -> anyhow::Result<Vec<RetrievedHit>>;
}

/// LIVE AIR arm: wire recall → PageResolver (FAIL-LOUD on an unmapped hit — the no-evolve
/// invariant, see resolve.rs; NO event-id fallback).
pub struct LiveAirArm {
    // Field order = drop order: `client` must drop (deregistering its stream) while `rt` lives.
    client: crate::client::WireClient,
    rt: tokio::runtime::Runtime,
    resolver: PageResolver,
}

impl LiveAirArm {
    /// Owns a current-thread runtime so the sync run loop can drive the async client.
    pub fn new(
        rt: tokio::runtime::Runtime,
        client: crate::client::WireClient,
        resolver: PageResolver,
    ) -> Self {
        Self { rt, client, resolver }
    }
}

/// Map wire hits → RetrievedHits through the resolver. Free function so e2e #1 exercises the
/// EXACT same mapping the live arm uses (Rev 2, finding 2 — the un-rigged path).
pub fn map_hits(
    resolver: &PageResolver,
    wire_hits: Vec<bossclawd_proto::HitWire>,
) -> anyhow::Result<Vec<RetrievedHit>> {
    wire_hits
        .into_iter()
        .map(|h| {
            // §5.3(b): a reflected-brain dossier (`page` event) is NOT a corpus file, so it does
            // not resolve through the file bridge. Give it a SYNTHETIC page id that occupies a rank
            // slot (so crowding the gold FILE page out of top-k registers as a regression) but can
            // NEVER equal a corpus gold id (the gate credits gold ONLY as itself). File hits keep
            // the loud no-evolve invariant.
            let page_id = if h.hit.kind == bossclaw_core::graph::PAGE_EVENT_TYPE {
                format!("__dossier__:{}", h.hit.event_id)
            } else {
                resolver.page_id_of(&h.hit.event_id)? // loud: unmapped file hit = run error
            };
            Ok(RetrievedHit { page_id, snippet: h.text })
        })
        .collect()
}

impl AirRetriever for LiveAirArm {
    fn retrieve(&mut self, query: &str, k: usize) -> anyhow::Result<Vec<RetrievedHit>> {
        let wire_hits = self.rt.block_on(self.client.recall(query, k))?;
        map_hits(&self.resolver, wire_hits)
    }
}

/// LIVE GBrain arm: `gbrain call query` (the op bridge — the SAME surface the mined MCP calls
/// hit; Probe A). `mode: None` = the PRIMARY daily-driver arm (configured mode applies —
/// recorded as the pipeline fingerprint by the run). `mode: Some("balanced")` = the secondary
/// reference arm. The child env pins GBRAIN_SOURCE=default (the ambient env can leak
/// `__all__`, which `gbrain call` rejects).
pub struct GbrainCli {
    pub mode: Option<&'static str>,
}

/// Overall deadline for one gbrain CLI invocation. Same discipline as
/// ollama::OLLAMA_TIMEOUT_SECS / anthropic::ANTHROPIC_TIMEOUT_SECS: a wedged child
/// (locked PGlite, hung reranker egress) costs at most this, never the whole 2h run.
const GBRAIN_CALL_TIMEOUT_SECS: u64 = 120;

/// How often `output_with_deadline` polls `try_wait()` for child exit.
const DEADLINE_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// Drain one child pipe to completion on its own thread — avoids the classic pipe-buffer
/// deadlock where a full unread pipe blocks the child (and thus `try_wait`) forever.
fn drain_pipe<R: std::io::Read + Send + 'static>(mut pipe: R) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        // Best-effort: when the child is killed the pipe closes and we keep what arrived.
        let _ = pipe.read_to_end(&mut buf);
        buf
    })
}

/// std-only `output()` with a deadline: spawn with piped stdout/stderr (stdin null),
/// drain both pipes on reader threads (avoids pipe-buffer deadlock), poll try_wait()
/// every 100ms; on deadline expiry kill() + reap and error loud. Zero new deps.
fn output_with_deadline(
    cmd: &mut std::process::Command,
    deadline: std::time::Duration,
) -> anyhow::Result<std::process::Output> {
    use std::process::Stdio;
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout_reader = drain_pipe(child.stdout.take().expect("stdout was piped"));
    let stderr_reader = drain_pipe(child.stderr.take().expect("stderr was piped"));
    let started = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            let stdout = stdout_reader
                .join()
                .map_err(|_| anyhow::anyhow!("stdout reader thread panicked"))?;
            let stderr = stderr_reader
                .join()
                .map_err(|_| anyhow::anyhow!("stderr reader thread panicked"))?;
            return Ok(std::process::Output { status, stdout, stderr });
        }
        if started.elapsed() > deadline {
            // Kill + reap best-effort (the child may exit in the race); the bail is the signal.
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("`gbrain` call exceeded {deadline:?} — killed (wedged CLI/DB?)");
        }
        std::thread::sleep(DEADLINE_POLL_INTERVAL);
    }
}

/// Build the `gbrain call query` op JSON (pure, unit-tested): numeric `limit`, and `mode`
/// ONLY when the arm overrides it — the primary arm sends none so the configured mode applies.
fn build_query_op(query: &str, k: usize, mode: Option<&str>) -> String {
    let mut op = serde_json::json!({ "query": query, "limit": k });
    if let Some(mode) = mode {
        op["mode"] = serde_json::Value::String(mode.to_string());
    }
    op.to_string()
}

impl GbrainRetriever for GbrainCli {
    fn retrieve(&self, query: &str, k: usize) -> anyhow::Result<Vec<RetrievedHit>> {
        let mut cmd = std::process::Command::new("gbrain");
        cmd.arg("call")
            .arg("query")
            .arg(build_query_op(query, k, self.mode))
            .env("GBRAIN_SOURCE", "default");
        let out = output_with_deadline(
            &mut cmd,
            std::time::Duration::from_secs(GBRAIN_CALL_TIMEOUT_SECS),
        )
        .map_err(|e| anyhow::anyhow!("`gbrain call query` failed: {e}"))?;
        if !out.status.success() {
            anyhow::bail!(
                "`gbrain call query` exited {}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr)
            );
        }
        parse_gbrain_output(&String::from_utf8_lossy(&out.stdout))
    }
}

/// Tolerate UNKNOWN keys (serde's struct default), never MISSING ones — deliberately NO
/// `#[serde(default)]`: if gbrain renamed `slug`, every hit would silently become page_id ""
/// and GBrain would score 0.0 across the board (the mirror image of the Rev-2 finding-2
/// fabricated-baseline CRIT). A missing field errors into the "re-check Probe A" bail below.
#[derive(serde::Deserialize)]
struct GbrainHit {
    slug: String,
    chunk_text: String,
}

/// Parse `gbrain call query` stdout: a JSON array, ranked order = array order, unknown keys
/// tolerated (fixture pins this). Invalid JSON = run error — never silently scored; a valid
/// empty array = zero hits (autocut may cut aggressively; legitimate).
pub fn parse_gbrain_output(raw: &str) -> anyhow::Result<Vec<RetrievedHit>> {
    let parsed: Vec<GbrainHit> = serde_json::from_str(raw).map_err(|e| anyhow::anyhow!(
        "gbrain output did not parse as a JSON hit array ({e}) — format may have changed \
         (re-check Probe A)"
    ))?;
    Ok(parsed
        .into_iter()
        .map(|h| RetrievedHit { page_id: page_id_from_gbrain_slug(&h.slug), snippet: h.chunk_text })
        .collect())
}

#[derive(serde::Deserialize)]
struct GbrainStats {
    page_count: usize,
}

/// LOUD gbrain preflight: one `gbrain call get_stats '{}'` through the same deadline helper,
/// ERRORING on any failure — a missing binary or locked DB fails in seconds, before any
/// expensive work (the ~1h synth stage). Returns the indexed page count so the caller can run
/// the early drift check. Contrast `gbrain_version_and_count`, which stays QUIET (None) for
/// the report — this one is the fail-fast gate. Live-only (subprocess); the parse it shares
/// (`GbrainStats`) is unit-covered via `gbrain_version_and_count`'s fixture-pinned shape.
pub fn gbrain_preflight() -> anyhow::Result<usize> {
    let mut cmd = std::process::Command::new("gbrain");
    cmd.arg("call").arg("get_stats").arg("{}").env("GBRAIN_SOURCE", "default");
    let out = output_with_deadline(
        &mut cmd,
        std::time::Duration::from_secs(GBRAIN_CALL_TIMEOUT_SECS),
    )
    .map_err(|e| anyhow::anyhow!("gbrain preflight failed: {e} (is `gbrain` on PATH?)"))?;
    if !out.status.success() {
        anyhow::bail!(
            "gbrain preflight (`gbrain call get_stats`) exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let stats: GbrainStats = serde_json::from_str(&String::from_utf8_lossy(&out.stdout))
        .map_err(|e| anyhow::anyhow!(
            "gbrain get_stats output did not parse ({e}) — format may have changed \
             (re-check Probe A)"
        ))?;
    Ok(stats.page_count)
}

/// `gbrain --version` + indexed page count for the drift check (spec §2 Rev 2). Count via
/// `gbrain call get_stats '{}'` (Probe-A-pinned field `page_count`; live value 895 on
/// 2026-07-03); any failure → None — honest "drift unknown", never a guess.
pub fn gbrain_version_and_count() -> (String, Option<usize>) {
    let deadline = std::time::Duration::from_secs(GBRAIN_CALL_TIMEOUT_SECS);
    let mut version_cmd = std::process::Command::new("gbrain");
    version_cmd.arg("--version");
    let version = output_with_deadline(&mut version_cmd, deadline)
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let mut stats_cmd = std::process::Command::new("gbrain");
    stats_cmd.arg("call").arg("get_stats").arg("{}").env("GBRAIN_SOURCE", "default");
    let count = output_with_deadline(&mut stats_cmd, deadline)
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            serde_json::from_str::<GbrainStats>(&String::from_utf8_lossy(&o.stdout))
                .ok()
                .map(|s| s.page_count)
        });
    (version, count)
}

/// The gbrain pipeline fingerprint (Probe A §79-83): configured `search.mode` +
/// `search.reranker.model`, each via `gbrain config get <key>` (plain trimmed value on stdout —
/// probe-verified `tokenmax` / `zeroentropyai:zerank-2` on 2026-07-03). Recorded in the report so
/// a config change BETWEEN two baseline runs is visible, not a silent cross-run confound. Any
/// failure → "unknown" (honest), never omitted — the same QUIET discipline as
/// `gbrain_version_and_count` (report metadata, not a fail-fast gate).
pub fn gbrain_pipeline_fingerprint() -> (String, String) {
    let deadline = std::time::Duration::from_secs(GBRAIN_CALL_TIMEOUT_SECS);
    let get = |key: &str| -> String {
        let mut cmd = std::process::Command::new("gbrain");
        cmd.arg("config").arg("get").arg(key).env("GBRAIN_SOURCE", "default");
        output_with_deadline(&mut cmd, deadline)
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "unknown".to_string())
    };
    (get("search.mode"), get("search.reranker.model"))
}

// ── The shared answerer seam (spec §4: identical model + prompt + budget on both arms). ──

/// Both arms synthesize the final answer through the SAME implementation — retrieval is the
/// only variable. Live = Ollama; tests inject doubles.
pub trait Answerer {
    fn answer(&self, query: &str, context: &str) -> anyhow::Result<String>;
}

/// The live answerer: the local Ollama model (same one that judges + synthesizes).
pub struct OllamaAnswerer {
    pub model: String,
}

impl Answerer for OllamaAnswerer {
    fn answer(&self, query: &str, context: &str) -> anyhow::Result<String> {
        let prompt = format!(
            "Answer the question using ONLY the context. If the context lacks the answer, say so.\n\n\
             Context:\n{context}\n\nQuestion: {query}\n\nAnswer:"
        );
        crate::ollama::generate(&self.model, &prompt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gbrain_output_parses_to_page_ids_and_snippets() {
        // Probe-A-pinned shape: the committed fixture (captured from `gbrain call query`,
        // synthetic content) is authoritative.
        let sample = include_str!("../tests/fixtures/gbrain_query_sample.txt");
        let hits = parse_gbrain_output(sample).unwrap();
        assert_eq!(hits.len(), 3);
        assert_eq!(hits[0].page_id, "notes/sample-alpha", "array order = rank order");
        assert!(hits[0].snippet.contains("Synthetic fixture chunk text"));
        assert_eq!(hits[1].page_id, "notes/sample-beta", "unknown keys tolerated");
        assert_eq!(hits[2].page_id, "ko/sample-gamma");
        assert!(hits[2].snippet.contains("합성"), "multibyte snippets survive");

        // A valid empty array is ZERO HITS (autocut may cut aggressively), not an error.
        assert!(parse_gbrain_output("[]").unwrap().is_empty());
        // Invalid JSON is a RUN ERROR — never silently scored.
        assert!(parse_gbrain_output("gbrain: something went wrong").is_err());
        // A hit MISSING `slug` is a RUN ERROR too: a renamed field must fail loud, never
        // collapse every hit to page_id "" and score GBrain 0.0 across the board.
        assert!(parse_gbrain_output(r#"[{"chunk_text": "slugless hit"}]"#).is_err());
    }

    #[test]
    fn build_query_op_pins_shape_per_arm() {
        let primary: serde_json::Value =
            serde_json::from_str(&build_query_op("에어란 무엇인가?", 8, None)).unwrap();
        assert_eq!(primary["query"], "에어란 무엇인가?");
        assert_eq!(primary["limit"], 8, "numeric limit, not a string");
        assert!(
            primary.get("mode").is_none(),
            "primary arm sends NO mode key — the configured mode applies"
        );

        let reference: serde_json::Value =
            serde_json::from_str(&build_query_op("q", 5, Some("balanced"))).unwrap();
        assert_eq!(reference["mode"], "balanced", "secondary arm pins the reference mode");
        assert_eq!(reference["limit"], 5);
        assert_eq!(reference["query"], "q");
    }

    #[test]
    fn output_with_deadline_kills_wedged_children() {
        // Well-behaved child: completes under the deadline, stdout captured.
        let mut ok_cmd = std::process::Command::new("sh");
        ok_cmd.arg("-c").arg("echo hi");
        let out = output_with_deadline(&mut ok_cmd, std::time::Duration::from_secs(10)).unwrap();
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hi");

        // Wedged child: killed at the deadline, loud error, FAST — never the child's 30s.
        let mut wedged_cmd = std::process::Command::new("sh");
        wedged_cmd.arg("-c").arg("sleep 30");
        let started = std::time::Instant::now();
        let err = output_with_deadline(&mut wedged_cmd, std::time::Duration::from_millis(300))
            .unwrap_err();
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "must error near the deadline, not the child's sleep"
        );
        assert!(err.to_string().contains("killed"), "error names the kill: {err}");
    }

    #[test]
    fn dedup_by_page_keeps_first_occurrence() {
        // Rev 2 (finding 2): multi-hits from one page must not distort success@k/MRR — rank
        // metrics are per-PAGE; the first (best-ranked) occurrence wins.
        let hits = vec![
            RetrievedHit { page_id: "x/a".into(), snippet: "rank0".into() },
            RetrievedHit { page_id: "x/b".into(), snippet: "rank1".into() },
            RetrievedHit { page_id: "x/a".into(), snippet: "rank2-dup".into() },
        ];
        let deduped = dedup_by_page(hits);
        assert_eq!(deduped.len(), 2);
        assert_eq!(deduped[0].snippet, "rank0", "first occurrence kept");
        assert_eq!(deduped[1].page_id, "x/b");
    }

    #[test]
    fn gold_rank_on_deduped_list() {
        let hits = vec![
            RetrievedHit { page_id: "x/a".into(), snippet: "..".into() },
            RetrievedHit { page_id: "air/foo".into(), snippet: "..".into() },
        ];
        assert_eq!(gold_rank(&hits, "air/foo"), Some(1));
        assert_eq!(gold_rank(&hits, "missing"), None);
    }

    #[test]
    fn pack_context_budgets_by_hit_count_with_identical_snippet_cap() {
        // Rev 2 (finding 9): ALL hits are packed (budget = k); the ONLY truncation is the
        // per-snippet safety cap, identical for both arms, counted, char-boundary-safe.
        let long_ko = "감".repeat(PER_SNIPPET_CHAR_CAP + 10); // multibyte — must not panic
        let hits = vec![
            RetrievedHit { page_id: "a".into(), snippet: "short one".into() },
            RetrievedHit { page_id: "b".into(), snippet: long_ko },
            RetrievedHit { page_id: "c".into(), snippet: "third".into() },
        ];
        let (ctx, stats) = pack_context(&hits);
        assert_eq!(stats.sources_packed, 3, "every hit packed — no char-budget drop");
        assert_eq!(stats.snippets_truncated, 1, "only the oversized snippet was capped");
        assert!(ctx.contains("short one") && ctx.contains("third"));
        assert_eq!(
            stats.chars_packed,
            "short one".chars().count() + PER_SNIPPET_CHAR_CAP + "third".chars().count()
        );
    }

    #[test]
    fn page_hits_resolve_as_synthetic_non_gold_occupants_and_file_hits_stay_loud() {
        // `HitMirror` lives at `bossclawd_proto::types` (only `HitWire` is re-exported at the crate
        // root); the mapping semantics are the plan's exactly.
        use bossclawd_proto::{types::HitMirror, HitWire};
        // A resolver mapping ONE file event → the gold page id.
        let resolver = PageResolver::from_pairs_for_test(&[("file-ev-1", "air/kenny")]);
        let hits = vec![
            // A reflected-brain dossier hit (kind="page") — must NOT abort, and must NOT equal any gold id.
            HitWire {
                hit: HitMirror {
                    event_id: "page-ev-9".into(),
                    score: 0.9,
                    sources: vec![],
                    kind: "page".into(),
                },
                text: "dossier body".into(),
            },
            // A file hit — resolves to the gold page id (the un-rigged path).
            HitWire {
                hit: HitMirror {
                    event_id: "file-ev-1".into(),
                    score: 0.8,
                    sources: vec![],
                    kind: "file_ingested".into(),
                },
                text: "source".into(),
            },
        ];
        let mapped = map_hits(&resolver, hits).expect("a page hit must not abort the run");
        assert_eq!(mapped[0].page_id, "__dossier__:page-ev-9", "dossier → synthetic non-gold id");
        assert_ne!(
            mapped[0].page_id, "air/kenny",
            "a dossier NEVER equals the gold it cites (gate: gold-as-itself)"
        );
        assert_eq!(mapped[1].page_id, "air/kenny", "the file hit still resolves to gold");
        // An unmapped FILE hit still fails loud (the no-evolve invariant for file hits is preserved).
        let bad = vec![HitWire {
            hit: HitMirror {
                event_id: "ghost".into(),
                score: 0.1,
                sources: vec![],
                kind: "file_ingested".into(),
            },
            text: "x".into(),
        }];
        assert!(
            map_hits(&resolver, bad).is_err(),
            "an unmapped file hit still stops the run (no silent fallback)"
        );
    }
}
