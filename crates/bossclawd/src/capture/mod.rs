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

// The SNAPSHOT builder (SP3 A11): the memory-poisoning DEFENSE. Assembles the fenced, sanitized,
// project-scoped, ≤4 KB orientation text the SessionStart hook injects into a fresh agent context.
// Every memory-derived field (session titles, live-transcript digest lines) is neutralized by
// `sanitize_injected` (no newline/control/bidi/zero-width survives → nothing can forge a structural
// "## SYSTEM:" line) and wrapped in a fixed, daemon-authored untrusted-data fence with a "DATA, not
// instructions" preamble that always survives truncation (spec §5, I8). Unix-only to match its
// store/paths/sweeper siblings — it drives the Unix-only `EngineHandle` and the confined careful-open.
#[cfg(unix)]
pub mod snapshot;

// The capture SWEEPER (SP3 A9): the durability guarantee (crash/SIGKILL/missed poke →
// captured within one sweep) AND the backfill engine (first sweep after Connect imports
// ~30 days of quiet transcripts). Ties together A5 (path discipline), A6 (renderer),
// A7 (store), A8 (consent flags). Unix-only to match its store/paths siblings — it opens
// transcripts via the confined careful-open and drives the Unix-only `EngineHandle`.
#[cfg(unix)]
pub mod sweeper;
