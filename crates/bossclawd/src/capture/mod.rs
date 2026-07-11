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

// The capture STORE (SP3 A7): writes the rendered Markdown to disk (0600 under a
// 0700 dir, atomic temp+rename) THEN records the signed `session_captured` event
// (file-then-event, spec §4b), with `heal_orphans` reconciling both crash windows.
// Unix-only: it drives the `EngineHandle` (bossclaw-core is Unix-only) and pins
// POSIX 0600/0700 mode bits, exactly like `#[cfg(unix)] mod engine`.
#[cfg(unix)]
pub mod store;
