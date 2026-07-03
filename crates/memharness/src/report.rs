//! The per-run report (spec §7): markdown + raw scores JSON into
//! `~/.air-harness/reports/<timestamp>/`. NEVER under the repo/workspace — the guard resolves
//! symlinks and not-yet-created paths (Rev 2). Renders the INVALID-RUN drift banner (>5%),
//! the trust verdict FIRST, per-arm pack stats, and every honesty caveat.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::arms::PackStats;
use crate::corpus::CorpusManifest;
use crate::judge::TrustVerdict;
use crate::stats::WilcoxonResult;

/// Drift above this fraction invalidates the run (spec §2 Rev 2).
pub const DRIFT_INVALID_FRACTION: f64 = 0.05;

/// One segment row.
#[derive(Debug, Clone, Serialize)]
pub struct SegmentResult {
    pub label: String, // "real·en·known-item" etc.
    pub n: usize,
    pub air_success_at_k: f64,
    pub gbrain_success_at_k: f64,
    pub air_mrr: f64,
    pub gbrain_mrr: f64,
    pub air_win_rate: f64, // open segments only (0.0 on known-item rows)
    pub ci_low: f64,
    pub ci_high: f64,
    pub wilcoxon: Option<WilcoxonResult>,
}

/// Per-arm packing totals across the run (Rev 2, finding 9).
#[derive(Debug, Clone, Default, Serialize)]
pub struct PackTotals {
    pub sources_packed: usize,
    pub snippets_truncated: usize,
    pub chars_packed: usize,
}

impl PackTotals {
    pub fn add(&mut self, s: &PackStats) {
        self.sources_packed += s.sources_packed;
        self.snippets_truncated += s.snippets_truncated;
        self.chars_packed += s.chars_packed;
    }
}

/// One example win/loss with retrieved-context diffs (spec §7).
#[derive(Debug, Clone, Serialize)]
pub struct ExamplePair {
    pub query: String,
    pub winner: String, // "AIR" | "GBrain"
    pub air_context: String,
    pub gbrain_context: String,
}

/// The full report model (markdown + raw JSON).
#[derive(Debug, Clone, Serialize)]
pub struct ReportModel {
    pub trust: TrustVerdict,
    pub k: usize,
    pub segments: Vec<SegmentResult>,
    pub corpus: CorpusManifest,
    pub gbrain_version: String,
    pub gbrain_page_count: Option<usize>,
    /// |gbrain_pages − corpus_pages| / corpus_pages; None = count unavailable ("drift unknown").
    pub drift_fraction: Option<f64>,
    pub ollama_model: String,
    pub egress_pairs_sent: usize,
    pub local_only: bool,
    pub near_dedup_applied: bool, // false in Phase 0 → explicit caveat
    pub air_pack: PackTotals,
    pub gbrain_pack: PackTotals,
    pub examples: Vec<ExamplePair>,
}

/// Canonicalize the nearest EXISTING ancestor of `p`, rejoining the non-existing tail — so a
/// symlinked or not-yet-created reports dir resolves to its REAL location before the guard
/// compares (Rev 2, finding 7).
fn canonicalize_nearest_existing(p: &Path) -> PathBuf {
    let mut existing = p;
    loop {
        if existing.as_os_str().is_empty() {
            return p.to_path_buf();
        }
        if let Ok(canon) = std::fs::canonicalize(existing) {
            let tail = p.strip_prefix(existing).unwrap_or_else(|_| Path::new(""));
            return canon.join(tail);
        }
        match existing.parent() {
            Some(parent) => existing = parent,
            None => return p.to_path_buf(),
        }
    }
}

/// Refuse any reports dir under the WORKSPACE (nearest `.git` ancestor of `repo_root`) —
/// reports quote brain content and the repo is public. Symlink- and nonexistent-path-resistant.
pub fn ensure_outside_repo(reports_dir: &Path, repo_root: &Path) -> anyhow::Result<()> {
    let repo_canon = std::fs::canonicalize(repo_root).unwrap_or_else(|_| repo_root.to_path_buf());
    let forbidden = repo_canon
        .ancestors()
        .find(|p| p.join(".git").exists())
        .map(|p| p.to_path_buf())
        .unwrap_or(repo_canon);
    let resolved = canonicalize_nearest_existing(reports_dir);
    if resolved.starts_with(&forbidden) {
        anyhow::bail!(
            "refusing to write reports inside the repo ({resolved:?} is under {forbidden:?}); \
             reports quote brain content"
        );
    }
    Ok(())
}

/// Char-boundary-safe example truncation (Rev 2, finding 11 — Korean text!).
fn truncate_for_example(s: &str) -> String {
    const MAX: usize = 200;
    if s.chars().count() <= MAX {
        s.to_string()
    } else {
        let head: String = s.chars().take(MAX).collect();
        format!("{head}…")
    }
}

/// Render the report. Order: INVALID-RUN banner (if drift > threshold) → judge-trust verdict →
/// headline → EN/KO → known-item vs open → caveats → egress → examples (spec §7 Rev 2).
pub fn render_markdown(r: &ReportModel) -> String {
    let mut s = String::new();
    // 0. Drift banner (Rev 2, finding 10) — the FIRST thing a reader sees.
    if let Some(d) = r.drift_fraction {
        if d > DRIFT_INVALID_FRACTION {
            s.push_str(&format!(
                "# ⚠ INVALID RUN — corpus drift {:.1}% (> {:.0}% threshold)\n\n\
                 GBrain's index and ~/brain diverge too far for an apples-to-apples comparison. \
                 Re-sync GBrain and re-run.\n\n",
                d * 100.0,
                DRIFT_INVALID_FRACTION * 100.0
            ));
        }
    }
    // 1. Judge-trust verdict (LEADS).
    s.push_str("## Judge-trust verdict\n");
    if r.trust.audit_incomplete {
        if r.trust.audited_count == 0 && r.segments.iter().all(|seg| !seg.label.contains("open")) {
            s.push_str("No open queries this run — nothing to judge; the trust verdict is not applicable.\n\n");
        } else if r.local_only {
            s.push_str("No audit this run (--local-only). Local-judge scores are unverified.\n\n");
        } else {
            s.push_str(
                "Trust verdict UNAVAILABLE — the cloud audit did not complete (API failure). \
                 Local-judge scores are unverified this run.\n\n",
            );
        }
    } else {
        s.push_str(&format!(
            "Local vs cloud agreement: {:.1}% · Cohen's kappa: {:.3} · audited {} pairs · **{}**{}{}\n\n",
            r.trust.agreement * 100.0,
            r.trust.kappa,
            r.trust.audited_count,
            if r.trust.trusted { "TRUSTED" } else { "the local judge is not yet trustworthy" },
            if r.trust.expanded_to_full_audit { " (audit auto-expanded to 100%)" } else { "" },
            if r.trust.audit_n_too_small { " · audit n too small; trust verdict indicative only" } else { "" },
        ));
    }
    // 2. Headline.
    s.push_str("## Headline\n");
    s.push_str(&format!(
        "Corpus: {} pages, {} bytes · k={} (retrieval k == scoring k) · model={} · gbrain {}{}{}\n\n",
        r.corpus.file_count,
        r.corpus.total_bytes,
        r.k,
        r.ollama_model,
        r.gbrain_version,
        match r.gbrain_page_count {
            Some(p) => format!(" ({p} pages indexed)"),
            None => " (page count unavailable — drift unknown)".to_string(),
        },
        match r.drift_fraction {
            Some(d) => format!(" · drift {:.1}%", d * 100.0),
            None => String::new(),
        },
    ));
    s.push_str("| segment | n | AIR s@k | GBrain s@k | AIR MRR | GBrain MRR | AIR win% | 95% CI |\n");
    s.push_str("|---|---|---|---|---|---|---|---|\n");
    for seg in &r.segments {
        s.push_str(&format!(
            "| {} | {} | {:.3} | {:.3} | {:.3} | {:.3} | {:.1}% | [{:.3}, {:.3}] |\n",
            seg.label, seg.n, seg.air_success_at_k, seg.gbrain_success_at_k,
            seg.air_mrr, seg.gbrain_mrr, seg.air_win_rate * 100.0, seg.ci_low, seg.ci_high,
        ));
    }
    s.push('\n');
    // 3. EN vs KO (the expected bilingual gap made visible).
    s.push_str("### EN vs KO\n");
    for seg in r.segments.iter().filter(|s| s.label.contains("·en·") || s.label.contains("·ko·")) {
        s.push_str(&format!(
            "- {} (n={}) : AIR s@k {:.3} vs GBrain {:.3}\n",
            seg.label, seg.n, seg.air_success_at_k, seg.gbrain_success_at_k
        ));
    }
    s.push('\n');
    // 4. Known-item vs open.
    s.push_str("### Known-item vs open\n");
    for seg in &r.segments {
        s.push_str(&format!("- {} : n={}\n", seg.label, seg.n));
    }
    s.push('\n');
    // 5. Context packing (Rev 2, finding 9) + granularity note.
    s.push_str("### Context packing (budget = k hits, both arms)\n");
    s.push_str(&format!(
        "AIR: {} sources, {} truncated, {} chars · GBrain: {} sources, {} truncated, {} chars\n",
        r.air_pack.sources_packed, r.air_pack.snippets_truncated, r.air_pack.chars_packed,
        r.gbrain_pack.sources_packed, r.gbrain_pack.snippets_truncated, r.gbrain_pack.chars_packed,
    ));
    s.push_str(
        "> Granularity note: GBrain returns CHUNKS, AIR returns page-level snippets — hit-count \
         budgeting avoids a char-truncation confound, but per-hit sizes differ by design; \
         per-arm chars are reported for transparency.\n\n",
    );
    // 6. Honesty caveats.
    for seg in &r.segments {
        if let Some(w) = &seg.wilcoxon {
            if w.small_n_approx {
                s.push_str(&format!(
                    "> small-n ({}): Wilcoxon p={:.4} via normal approx — exact test advised.\n",
                    seg.label, w.p_value
                ));
            }
        }
    }
    s.push_str(
        "> Known-item Wilcoxon runs on binary success flags (heavy ties) — read it as \
         sign-test-like (spec §8 Rev 2).\n",
    );
    if !r.near_dedup_applied {
        s.push_str("> Caveat: near-duplicate query collapse is NOT applied in Phase 0 (exact dedup only).\n");
    }
    // 7. Egress accounting.
    s.push_str(&format!(
        "\n### Egress\n{} query/answer pair(s) egressed to the cloud audit (2 position-swapped \
         calls each){}. GBrain's own pipeline may egress per its config (its normal behavior).\n",
        r.egress_pairs_sent,
        if r.local_only { " — zero: --local-only" } else { "" },
    ));
    // 8. Examples.
    s.push_str("\n### Examples (wins/losses with context diffs)\n");
    for ex in &r.examples {
        s.push_str(&format!(
            "- **{}** — winner: {}\n  - AIR ctx: {}\n  - GBrain ctx: {}\n",
            ex.query,
            ex.winner,
            truncate_for_example(&ex.air_context),
            truncate_for_example(&ex.gbrain_context),
        ));
    }
    s
}

/// Write markdown + raw JSON into `reports_dir/<timestamp>/` AFTER the guard. Returns the dir.
pub fn write_report(
    reports_dir: &Path,
    repo_root: &Path,
    r: &ReportModel,
) -> anyhow::Result<PathBuf> {
    ensure_outside_repo(reports_dir, repo_root)?;
    let out_dir = reports_dir.join(r.corpus.snapshot_unix_secs.to_string());
    std::fs::create_dir_all(&out_dir)?;
    std::fs::write(out_dir.join("report.md"), render_markdown(r))?;
    std::fs::write(out_dir.join("scores.json"), serde_json::to_vec_pretty(r)?)?;
    Ok(out_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    impl ReportModel {
        /// Fully synthetic sample for the golden snapshot (no brain content).
        pub fn sample_for_test() -> ReportModel {
            use crate::corpus::{CorpusManifest, ManifestEntry};
            use crate::judge::TrustVerdict;
            ReportModel {
                trust: TrustVerdict {
                    audited_count: 20, agreement: 0.9, kappa: 0.72, trusted: true,
                    expanded_to_full_audit: false, audit_incomplete: false, audit_n_too_small: false,
                },
                k: 10,
                segments: vec![SegmentResult {
                    label: "synthetic·en·known-item".into(), n: 3,
                    air_success_at_k: 0.667, gbrain_success_at_k: 1.0,
                    air_mrr: 0.5, gbrain_mrr: 0.833, air_win_rate: 0.0,
                    ci_low: 0.3, ci_high: 1.0, wilcoxon: None,
                }],
                corpus: CorpusManifest {
                    snapshot_unix_secs: 1000, file_count: 3, total_bytes: 42,
                    entries: vec![ManifestEntry {
                        page_id: "en/alpha".into(), sha256: "deadbeef".into(), bytes: 14,
                    }],
                },
                gbrain_version: "gbrain 0.42".into(),
                gbrain_page_count: Some(866),
                drift_fraction: Some(0.01),
                ollama_model: "qwen2.5:7b".into(),
                egress_pairs_sent: 4,
                local_only: false,
                near_dedup_applied: false,
                air_pack: PackTotals { sources_packed: 30, snippets_truncated: 1, chars_packed: 9000 },
                gbrain_pack: PackTotals { sources_packed: 30, snippets_truncated: 0, chars_packed: 4000 },
                examples: vec![ExamplePair {
                    query: "who is alpha".into(), winner: "GBrain".into(),
                    air_context: "alpha ctx".into(), gbrain_context: "beta ctx".into(),
                }],
            }
        }

        /// Expected markdown for `sample_for_test()` — self-consistent golden (asserts render
        /// determinism; the load-bearing structure assertions are the section-order checks).
        pub fn sample_golden_markdown() -> String {
            render_markdown(&ReportModel::sample_for_test())
        }
    }

    /// The workspace root (nearest ancestor with .git) — what the guard must protect.
    fn workspace_root() -> std::path::PathBuf {
        let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
        crate_root
            .ancestors()
            .find(|p| p.join(".git").exists())
            .expect("workspace root")
            .to_path_buf()
    }

    #[test]
    fn refuses_repo_workspace_and_symlinked_paths() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")); // crates/memharness
        // (a) Inside this crate.
        assert!(ensure_outside_repo(&repo_root.join("reports"), repo_root).is_err());
        // (b) Rev 2: under the WORKSPACE root but outside crates/memharness.
        let ws_target = workspace_root().join("target/harness-reports");
        let err = ensure_outside_repo(&ws_target, repo_root).unwrap_err();
        assert!(err.to_string().contains("inside the repo"), "workspace guarded: {err}");
        // (c) Rev 2: a symlink that LOOKS outside but resolves INTO the workspace.
        let tmp = tempfile::tempdir().unwrap();
        let link = tmp.path().join("sneaky");
        std::os::unix::fs::symlink(workspace_root().join("target"), &link).unwrap();
        assert!(
            ensure_outside_repo(&link.join("reports"), repo_root).is_err(),
            "symlink into the repo is refused"
        );
        // (d) A genuinely outside dir (possibly not yet created) is allowed.
        assert!(ensure_outside_repo(&tmp.path().join("ok/reports"), repo_root).is_ok());
    }

    #[test]
    fn truncation_is_char_boundary_safe_for_korean() {
        // Rev 2 (finding 11): byte-slicing panics mid-codepoint; chars() must not.
        let ko = "감마선은".repeat(200); // > 200 chars, all multibyte
        let out = truncate_for_example(&ko);
        assert!(out.chars().count() <= 201, "200 chars + ellipsis");
    }

    #[test]
    fn renders_trust_first_drift_banner_and_caveats() {
        let mut report = ReportModel::sample_for_test();
        let md = render_markdown(&report);
        let trust_idx = md.find("## Judge-trust verdict").expect("trust section present");
        let headline_idx = md.find("## Headline").expect("headline present");
        assert!(trust_idx < headline_idx, "trust verdict LEADS");
        assert!(md.contains("### EN vs KO"));
        assert!(md.contains("(n=3)"), "per-language sample counts visible in EN vs KO (synth-doc promise)");
        assert!(md.contains("### Known-item vs open"));
        assert!(md.contains("retrieval k == scoring k"), "one-knob statement (finding 14)");
        assert!(md.contains("Context packing"), "per-arm pack stats present");
        assert!(md.contains("near-duplicate"), "exact-only dedup caveat present");
        assert!(md.contains("binary success flags"), "tie-heavy Wilcoxon caveat present");
        assert!(!md.contains("INVALID RUN"), "no banner at 0 drift");
        // GOLDEN on synthetic data: render is deterministic.
        assert_eq!(md, ReportModel::sample_golden_markdown());

        // Rev 2 (finding 10): drift > 5% → the INVALID-RUN banner leads the whole report.
        report.drift_fraction = Some(0.12);
        let md2 = render_markdown(&report);
        assert!(md2.starts_with("# ⚠ INVALID RUN"), "banner is the FIRST line");

        // audit_n_too_small renders its indicative-only note.
        report.drift_fraction = None;
        report.trust.audit_n_too_small = true;
        assert!(render_markdown(&report).contains("indicative only"));

        // A run with zero open queries: audit_incomplete must not read as a failure.
        report.trust.audit_incomplete = true;
        report.trust.audited_count = 0;
        report.segments[0].label = "synthetic·en·known-item".into();
        let md3 = render_markdown(&report);
        assert!(md3.contains("No open queries this run"), "no-opens wording, not a failure claim: {md3}");
    }
}
