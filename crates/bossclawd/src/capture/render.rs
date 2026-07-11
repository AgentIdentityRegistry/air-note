//! A6: deterministic, bounded transcript renderer (Claude Code JSONL → Markdown).
//!
//! # What this is
//! The daemon captures Claude Code session transcripts and needs a compact,
//! human-readable Markdown rendering of each one. This module does that
//! **deterministically** — identical input bytes always produce identical
//! Markdown (spec I5): no LLM anywhere, no clock reads in the OUTPUT, and no
//! `HashMap` iteration in the emitted text (`serde_json::Value` maps are the
//! default `BTreeMap`, so object keys iterate in sorted order).
//!
//! # Defensive parsing (the per-line schema is officially unpublished)
//! Every line is parsed with a "recognize or skip" discipline — we never index a
//! JSON value without checking, never `unwrap` on untrusted content, and never
//! panic. Recognized turns (`user` / `assistant` with a `message.content`) are
//! rendered; everything else (`queue-operation`, hook `attachment`s, `system`,
//! and any unknown future `type`) is skipped and counted in `skipped_unknown`.
//!
//! # Bounds (the D2 obligation delegated by A5)
//! A5's `open_transcript_confined` returns a **bare** `std::fs::File` with **no
//! size cap** — it deliberately does not reuse ingest's `read_all_capped`. THIS
//! module is the cap point. [`render_transcript`] reads at most
//! `bounds.max_transcript_bytes` through a byte-limited reader; a file larger
//! than that is `Err(TooLarge)`, never an unbounded slurp / OOM. Per line:
//! - `len > max_line_bytes` → drop the line, `oversized_lines += 1` (the oversized
//!   JSON is never handed to the parser — the parser's input is bounded too);
//! - a non-terminated trailing line (a torn tail on a live, still-being-appended
//!   file) → drop it, set `dropped_torn_tail`;
//! - a per-render `wall_clock` budget is a coarse safety valve checked during the
//!   line loop (it never enters the rendered content, so determinism holds).
//!
//! # Two entry points (get the confinement boundary right)
//! - [`render_transcript`] takes an already-open `File` — the sweeper/dispatch
//!   path (A9/A11) passes the handle A5's confined open produced, so containment
//!   is already enforced upstream and this fn must **not** re-open by path. This
//!   is where the D2 cap lives.
//! - [`render_transcript_path`] is a thin `File::open` + delegate convenience for
//!   **trusted** inputs (test fixtures). Production must not route
//!   attacker-influenceable paths through it — it does no confinement.

use std::fs::File;
use std::io::Read;
// `Path` is only used by the test-only `render_transcript_path`, so the import is
// gated the same way to avoid an unused-import warning in production builds.
#[cfg(any(test, feature = "test-helpers"))]
use std::path::Path;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use thiserror::Error;

/// Max bytes of a `tool_use` one-liner's argument detail (whitespace-collapsed,
/// truncated on a char boundary). Keeps a pathological `input` blob from bloating
/// a single body line while still showing the call's shape.
const TOOL_DETAIL_MAX_BYTES: usize = 160;

/// Max CHARS of the derived title (first user prompt, whitespace-collapsed). This
/// is a multilingual-first product, so the title caps at 120 *visible characters*,
/// not bytes — a Korean/CJK title keeps 120 glyphs rather than ~40.
const TITLE_MAX_CHARS: usize = 120;

/// Coarse wall-clock cadence: check the elapsed budget every N processed lines.
/// The input is already size-capped, so a per-line clock read would be wasteful;
/// this keeps the safety valve cheap without touching output determinism.
const WALL_CLOCK_CHECK_EVERY: usize = 1024;

/// Input bounds mirroring ingest's discipline (spec §4a). Constructed by the
/// caller from `CAPTURE_MAX_TRANSCRIPT_BYTES` / `CAPTURE_MAX_LINE_BYTES` /
/// `CAPTURE_WALL_CLOCK`.
pub struct RenderBounds {
    /// Whole-file cap (D2): a transcript larger than this is refused loudly
    /// (`TooLarge`) rather than read into memory. Default 64 MiB.
    pub max_transcript_bytes: u64,
    /// Per-line cap: a longer line is dropped and counted, never parsed. Default 2 MiB.
    pub max_line_bytes: usize,
    /// Coarse per-render wall-clock budget (safety valve). Default 30 s.
    pub wall_clock: Duration,
}

/// Why a render could not complete. `TooLarge` is the loud D2 refusal the caller
/// (sweeper/dispatch) turns into a `skipped` record; the others are I/O / budget.
#[derive(Debug, Error)]
pub enum RenderError {
    /// The transcript exceeds `max_transcript_bytes` — refused before it can be
    /// slurped into memory. `actual` is the file's length when known.
    #[error("transcript exceeds the {limit}-byte cap (actual {actual} bytes)")]
    TooLarge { limit: u64, actual: u64 },
    /// Reading the file failed.
    #[error("reading transcript: {0}")]
    Io(#[from] std::io::Error),
    /// The per-render wall-clock budget elapsed before rendering finished.
    #[error("render exceeded the wall-clock budget")]
    WallClock,
}

/// A rendered transcript. The renderer only sees the transcript itself, so the
/// front-matter/fields here are what's derivable from it; the STORE task (A7)
/// adds `session_id` / project / path when it writes the file.
pub struct Rendered {
    /// First user prompt, whitespace-collapsed, ≤ [`TITLE_MAX_CHARS`] chars
    /// (empty if the transcript has no user prompt).
    pub title: String,
    /// The full Markdown document: stable front-matter block then the body.
    pub markdown: String,
    /// The body ALONE — the rendered conversation without the renderer's front-matter
    /// block. The store (A7) composes ONE coherent front-matter block (its session
    /// fields merged with these raw parts) and appends this body, so it never has to
    /// re-parse `markdown`'s front-matter back off. `markdown == <front-matter> + body`.
    pub body: String,
    /// Lowercase hex SHA-256 over the bytes actually read (the size-capped EOF
    /// snapshot). The signed capture event mirrors this (single source of truth).
    pub sha256: String,
    /// First parseable line timestamp as a Unix epoch second, or 0 if none
    /// (deterministic fallback — NO clock read).
    pub started_at: i64,
    /// Last parseable line timestamp as a Unix epoch second, or 0 if none.
    pub ended_at: i64,
    /// Bytes actually read (the snapshot length, ≤ `max_transcript_bytes`).
    pub approx_bytes: u64,
    /// A non-terminated trailing line was dropped (a torn tail on a live file).
    pub dropped_torn_tail: bool,
    /// Count of lines dropped for exceeding `max_line_bytes`.
    pub oversized_lines: u32,
    /// Count of skipped noise/unknown lines (queue-operation, hook attachments,
    /// `system`, unrecognized `type`, and mid-file unparseable lines).
    pub skipped_unknown: u32,
}

/// Open a **trusted** transcript path and render it (test/convenience entry
/// point — does NO confinement). Production code with an attacker-influenceable
/// path must open via A5's `open_transcript_confined` and call
/// [`render_transcript`] on the resulting handle instead.
///
/// Test-only surface: gated behind `cfg(test)` / the `test-helpers` feature (the
/// same idiom as `server::spawn_for_test`) so the unconfined path can never be
/// reached from production code.
#[cfg(any(test, feature = "test-helpers"))]
pub fn render_transcript_path(path: &Path, bounds: &RenderBounds) -> Result<Rendered, RenderError> {
    let file = File::open(path)?;
    render_transcript(file, bounds)
}

/// Render an already-open (confinement-vetted) transcript `File`. This is the
/// **D2 cap point** for A5's cap-free `File`: it reads at most
/// `bounds.max_transcript_bytes` through a byte-limited reader and returns
/// `TooLarge` rather than reading an over-budget file into memory.
pub fn render_transcript(file: File, bounds: &RenderBounds) -> Result<Rendered, RenderError> {
    // Cheap early refusal when the length is known and already over budget — this
    // avoids even the bounded read for a plainly oversized file.
    let meta_len = file.metadata().ok().map(|m| m.len());
    if let Some(len) = meta_len {
        if len > bounds.max_transcript_bytes {
            return Err(RenderError::TooLarge {
                limit: bounds.max_transcript_bytes,
                actual: len,
            });
        }
    }

    // Bounded slurp: read at most max+1 bytes, so memory is capped even when the
    // metadata length was stale (a live file may have grown since the stat).
    // Reading one byte past the cap lets us DETECT an over-budget file.
    let read_ceiling = bounds.max_transcript_bytes.saturating_add(1);
    let mut buf = Vec::new();
    file.take(read_ceiling).read_to_end(&mut buf)?;
    if buf.len() as u64 > bounds.max_transcript_bytes {
        return Err(RenderError::TooLarge {
            limit: bounds.max_transcript_bytes,
            actual: meta_len.unwrap_or(buf.len() as u64),
        });
    }

    render_bytes(&buf, bounds)
}

/// The pure, deterministic core: bytes in → `Rendered` out. Separated from I/O so
/// the size-cap/torn-tail/oversize logic is a total function of the snapshot bytes
/// (identical bytes ⇒ identical `markdown`). `WallClock` is the only reason it can
/// stop early, and that never depends on the CONTENT.
fn render_bytes(buf: &[u8], bounds: &RenderBounds) -> Result<Rendered, RenderError> {
    let render_start = Instant::now();
    let sha256 = hex_sha256(buf);
    let approx_bytes = buf.len() as u64;

    // A complete transcript ends with '\n'; trailing bytes AFTER the last newline
    // are a torn tail on a live file (dropped). `split` yields an empty final
    // element when the buffer ends with '\n', which we discard either way.
    let has_trailing_newline = buf.last() == Some(&b'\n');
    let mut segments: Vec<&[u8]> = buf.split(|&b| b == b'\n').collect();
    let mut dropped_torn_tail = false;
    if let Some(last) = segments.pop() {
        if !has_trailing_newline && !last.is_empty() {
            dropped_torn_tail = true;
        }
    }

    let mut oversized_lines: u32 = 0;
    let mut skipped_unknown: u32 = 0;
    let mut started_at: Option<i64> = None;
    let mut ended_at: i64 = 0;
    let mut title: Option<String> = None;
    let mut body = String::new();

    for (i, seg) in segments.iter().enumerate() {
        if i % WALL_CLOCK_CHECK_EVERY == 0 && render_start.elapsed() > bounds.wall_clock {
            return Err(RenderError::WallClock);
        }
        if seg.is_empty() {
            continue; // blank line between records
        }
        if seg.len() > bounds.max_line_bytes {
            oversized_lines += 1;
            continue; // never hand an oversized blob to the JSON parser
        }
        let val: serde_json::Value = match serde_json::from_slice(seg) {
            Ok(v) => v,
            Err(_) => {
                skipped_unknown += 1; // corrupt / truncated mid-file line — not fatal
                continue;
            }
        };

        // started/ended derive from parseable line timestamps (no clock read).
        if let Some(epoch) = val.get("timestamp").and_then(|t| t.as_str()).and_then(parse_epoch) {
            started_at.get_or_insert(epoch);
            ended_at = epoch;
        }

        match val.get("type").and_then(|t| t.as_str()) {
            Some("user") => render_message(&val, Role::User, &mut body, &mut title),
            Some("assistant") => render_message(&val, Role::Assistant, &mut body, &mut title),
            _ => skipped_unknown += 1, // queue-operation / attachment / system / unknown
        }
    }

    let markdown = assemble(
        &sha256,
        started_at.unwrap_or(0),
        ended_at,
        oversized_lines,
        skipped_unknown,
        dropped_torn_tail,
        &body,
    );

    Ok(Rendered {
        title: title.unwrap_or_default(),
        markdown,
        body,
        sha256,
        started_at: started_at.unwrap_or(0),
        ended_at,
        approx_bytes,
        dropped_torn_tail,
        oversized_lines,
        skipped_unknown,
    })
}

/// Which side of the conversation a record is; only affects the body heading and
/// whether its text can seed the title.
#[derive(Clone, Copy)]
enum Role {
    User,
    Assistant,
}

impl Role {
    fn heading(self) -> &'static str {
        match self {
            Role::User => "## You",
            Role::Assistant => "## Assistant",
        }
    }
}

/// Render one `user`/`assistant` record's `message.content` into `body`, seeding
/// `title` from the first user prompt text. Fully defensive: a missing/oddly-typed
/// `message`, `content`, or block simply yields nothing (never a panic).
fn render_message(val: &serde_json::Value, role: Role, body: &mut String, title: &mut Option<String>) {
    let content = match val.get("message").and_then(|m| m.get("content")) {
        Some(c) => c,
        None => return,
    };
    match content {
        // Plain-string content is the common shape for a real user prompt.
        serde_json::Value::String(text) => emit_text(role, text, body, title),
        // Array content carries typed blocks (text / tool_use / tool_result /
        // thinking / …). Iterate in file order; render what we recognize.
        serde_json::Value::Array(blocks) => {
            for block in blocks {
                match block.get("type").and_then(|t| t.as_str()) {
                    Some("text") => {
                        if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                            emit_text(role, text, body, title);
                        }
                    }
                    Some("tool_use") => emit_tool_use(block, body),
                    Some("tool_result") => emit_tool_result(block, body),
                    // thinking / fallback / images / unknown blocks are not body content.
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

/// Emit a text block under its role heading, preserving the text VERBATIM (the A6
/// contract: embedded pseudo-directives / control chars are DATA, not commands —
/// A11 fences them later). Seeds the title from the first user text only.
fn emit_text(role: Role, text: &str, body: &mut String, title: &mut Option<String>) {
    if matches!(role, Role::User) && title.is_none() {
        // Char-based cap: keep up to TITLE_MAX_CHARS visible glyphs (multilingual).
        let t: String = collapse_ws(text).chars().take(TITLE_MAX_CHARS).collect();
        if !t.is_empty() {
            *title = Some(t);
        }
    }
    body.push_str(role.heading());
    body.push('\n');
    body.push_str(text);
    body.push_str("\n\n");
}

/// Emit a `tool_use` as a single readable line: `▸ <ToolName>: <compact args>`.
/// The args are the block's `input` as compact JSON (sorted keys — `serde_json`'s
/// default map is a `BTreeMap`, so this is deterministic), whitespace-collapsed
/// and truncated so one call can't dominate the body.
fn emit_tool_use(block: &serde_json::Value, body: &mut String) {
    let name = block.get("name").and_then(|n| n.as_str()).unwrap_or("tool");
    let detail = match block.get("input") {
        Some(input) if !input.is_null() => {
            truncate_on_boundary(collapse_ws(&input.to_string()), TOOL_DETAIL_MAX_BYTES)
        }
        _ => String::new(),
    };
    body.push_str("\u{25b8} "); // ▸
    body.push_str(name);
    if !detail.is_empty() {
        body.push_str(": ");
        body.push_str(&detail);
    }
    body.push('\n');
}

/// Emit a `tool_result` as a short REFERENCE, never the (potentially huge) spilled
/// content — the id ties it back to the originating `tool_use`.
fn emit_tool_result(block: &serde_json::Value, body: &mut String) {
    body.push_str("\u{21a9} tool_result"); // ↩
    if let Some(id) = block.get("tool_use_id").and_then(|t| t.as_str()) {
        if !id.is_empty() {
            body.push_str(" (");
            body.push_str(id);
            body.push(')');
        }
    }
    body.push('\n');
}

/// Build the document: a stable, alphabetically-keyed front-matter block followed
/// by the body. Keys are sorted so the bytes are reproducible across renders.
#[allow(clippy::too_many_arguments)]
fn assemble(
    sha256: &str,
    started_at: i64,
    ended_at: i64,
    oversized_lines: u32,
    skipped_unknown: u32,
    dropped_torn_tail: bool,
    body: &str,
) -> String {
    let mut md = String::with_capacity(body.len() + 256);
    md.push_str("---\n");
    md.push_str(&format!("ended_at: {ended_at}\n"));
    md.push_str(&format!("lines_oversized: {oversized_lines}\n"));
    md.push_str(&format!("lines_skipped: {skipped_unknown}\n"));
    md.push_str(&format!("sha256: {sha256}\n"));
    md.push_str(&format!("started_at: {started_at}\n"));
    md.push_str(&format!("torn_tail: {dropped_torn_tail}\n"));
    md.push_str("---\n\n");
    md.push_str(body);
    md
}

/// Lowercase hex SHA-256 of the given bytes (the engine's `hex::encode(Sha256::…)`
/// convention).
fn hex_sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Parse an RFC 3339 timestamp (e.g. `2026-07-11T10:00:00.000Z`) to a Unix epoch
/// second. Returns `None` on any unparseable value — the caller falls back to 0.
fn parse_epoch(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp())
}

/// Collapse every run of ASCII/Unicode whitespace to a single space and trim.
/// Non-whitespace control characters are preserved (they're data). The caller
/// applies its own cap (char-based for the title, byte-based for tool one-liners),
/// so this never caps on its own. Used for the title and tool-call one-liners,
/// never for body prose.
fn collapse_ws(s: &str) -> String {
    let mut out = String::new();
    let mut prev_ws = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !out.is_empty() && !prev_ws {
                out.push(' ');
            }
            prev_ws = true;
        } else {
            out.push(ch);
            prev_ws = false;
        }
    }
    while out.ends_with(' ') {
        out.pop();
    }
    out
}

/// Truncate `s` to at most `max_bytes` bytes without splitting a UTF-8 char.
fn truncate_on_boundary(mut s: String, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
    s
}
