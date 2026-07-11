//! `air-memory-mcp` library surface: the daemon client (`daemon`) and the MCP JSON-RPC handler
//! (`mcp`). The bin (`main.rs`) is a thin stdio loop over these. Split into a lib so integration
//! tests can drive `daemon`/`mcp` directly.

pub mod daemon;
pub mod hook;
pub mod mcp;

/// The static reminder the SP2 SessionStart hook injects into every Claude Code session so the
/// agent proactively uses the `recall`/`remember` tools. No network/daemon call — a fixed string.
/// Kept here (not `main.rs`) so integration tests can assert the hook output byte-for-byte.
pub const NUDGE_TEXT: &str = "\
AIR long-term memory is available via the `air-memory` MCP tools: \
`recall(query)` to search your past notes and decisions, and \
`remember(text)` to save durable ones. Recall relevant context before starting a task; \
remember decisions worth keeping. Saved notes are stored as external/untrusted \
(recallable, never auto-applied).";

/// Produce the SessionStart nudge text (B3): a LIVE, project-aware orientation snapshot when the
/// daemon answers, else the static [`NUDGE_TEXT`] on ANY failure. Fail-quiet by contract (spec §5 /
/// I1): the SessionStart hook must never break or block session start, so every path returns a
/// printable String — nothing here can panic, and nothing can hang (the daemon call is bounded by
/// `daemon::SNAPSHOT_TIMEOUT`; `HookInput` came from a byte-bounded stdin read).
///
/// Factored out of the `nudge` subcommand so integration tests can drive it against a fake daemon
/// with an injected `sock`. The daemon call fires ONLY when a matchable project key exists — the
/// transcript's parent-dir slug ([`hook::snapshot_project`]), byte-identical to what capture stored —
/// so the snapshot actually returns THIS repo's sessions. No `transcript_path` → no key → static nudge
/// (never even contacting the socket).
pub async fn run_nudge(sock: &std::path::Path, h: hook::HookInput) -> String {
    match hook::snapshot_project(&h) {
        Some(project) => {
            // Empty/absent source → "startup" (a fresh start), consistent with the empty-is-absent
            // treatment of the other hook fields.
            let source = h.source.filter(|s| !s.is_empty()).unwrap_or_else(|| "startup".to_string());
            daemon::tool_snapshot(
                sock,
                &project,
                &source,
                h.session_id.filter(|s| !s.is_empty()),
                h.transcript_path.filter(|s| !s.is_empty()),
            )
            .await
            .unwrap_or_else(|_| NUDGE_TEXT.to_string())
        }
        None => NUDGE_TEXT.to_string(),
    }
}
