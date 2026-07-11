//! Session-capture path discipline for the daemon (SP3).
//!
//! The daemon captures Claude Code session transcripts (`~/.claude/projects/…`).
//! The two path inputs — the **session id** and the **transcript path** — arrive
//! from attacker-influenceable sources (hook stdin, MCP clients, sweeper-parsed
//! filenames), so every use is gated by this module (spec I4): a strict session-id
//! allowlist and a confined, canonicalize-free careful-open. See [`paths`].
pub mod paths;

/// Deterministic JSONL→Markdown transcript renderer (SP3 A6). Reads one EOF
/// snapshot of a Claude Code transcript through a byte-limited reader (the D2
/// size-cap point delegated by A5's cap-free confined open), parses each line
/// defensively, and emits stable front-matter + a readable body with NO LLM and
/// NO clock reads in the output (spec §4a, I5). See [`render`].
pub mod render;
