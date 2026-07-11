//! Session-capture path discipline for the daemon (SP3).
//!
//! The daemon captures Claude Code session transcripts (`~/.claude/projects/…`).
//! The two path inputs — the **session id** and the **transcript path** — arrive
//! from attacker-influenceable sources (hook stdin, MCP clients, sweeper-parsed
//! filenames), so every use is gated by this module (spec I4): a strict session-id
//! allowlist and a confined, canonicalize-free careful-open. See [`paths`].
pub mod paths;
