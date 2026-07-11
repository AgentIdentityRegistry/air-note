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
