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
    rt: tokio::runtime::Runtime,
    client: crate::client::WireClient,
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
            Ok(RetrievedHit {
                page_id: resolver.page_id_of(&h.hit.event_id)?, // loud: unmapped = run error
                snippet: h.text,
            })
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

impl GbrainRetriever for GbrainCli {
    fn retrieve(&self, query: &str, k: usize) -> anyhow::Result<Vec<RetrievedHit>> {
        let mut op = serde_json::json!({ "query": query, "limit": k });
        if let Some(mode) = self.mode {
            op["mode"] = serde_json::Value::String(mode.to_string());
        }
        let out = std::process::Command::new("gbrain")
            .arg("call")
            .arg("query")
            .arg(op.to_string())
            .env("GBRAIN_SOURCE", "default")
            .output()
            .map_err(|e| anyhow::anyhow!("failed to spawn `gbrain call query`: {e}"))?;
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

#[derive(serde::Deserialize)]
struct GbrainHit {
    #[serde(default)]
    slug: String,
    #[serde(default)]
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

/// `gbrain --version` + indexed page count for the drift check (spec §2 Rev 2). Count via
/// `gbrain call get_stats '{}'` (Probe-A-pinned field `page_count`; live value 895 on
/// 2026-07-03); any failure → None — honest "drift unknown", never a guess.
pub fn gbrain_version_and_count() -> (String, Option<usize>) {
    let version = std::process::Command::new("gbrain")
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let count = std::process::Command::new("gbrain")
        .arg("call")
        .arg("get_stats")
        .arg("{}")
        .env("GBRAIN_SOURCE", "default")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            serde_json::from_str::<GbrainStats>(&String::from_utf8_lossy(&o.stdout))
                .ok()
                .map(|s| s.page_count)
        });
    (version, count)
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
}
