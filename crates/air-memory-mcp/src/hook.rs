//! Defensive parser for the JSON Claude Code writes to a hook's **stdin**, shared by the two hook
//! paths that consume it: `capture-notify` (SessionEnd → `{session_id, transcript_path}`) and the
//! snapshot `nudge` (SessionStart → `{source, session_id, cwd, transcript_path}`). Both call
//! [`HookInput::read_from_stdin`] and degrade gracefully on any failure (the snapshot falls back to
//! the static nudge, the capture-notify silently skips), so this module never panics and never
//! reads unbounded from stdin (the read is BYTE-capped at 1 MiB).
//!
//! The harness owns this schema and may change it, so every field is `Option` and the parse is
//! purely defensive (spec §2). String fields are control-char-stripped on parse (spec §6) so nothing
//! hostile — a newline smuggled into a `session_id`, a control char in a path — can reach a log line
//! or the daemon wire.
//!
//! **Scope vs. the A11 snapshot sanitizer.** This is deliberately *simpler* than
//! `bossclaw-core`'s `strip_bidi_controls`/`sanitize_ident` (in `summarize.rs`). That sanitizer
//! guards auto-injected snapshot **content** — untrusted memory text that becomes agent context —
//! so it must also neutralize the full bidi/zero-width/TAG spoofing surface. These hook fields are
//! **operator/harness-controlled metadata** (a session id, a file path, an enum-ish source/reason),
//! not injected context. The realistic hazard is a raw control character (esp. `\n`/`\r`) corrupting
//! a log line or wire frame, so plain control-character stripping is the right, minimal scope here —
//! we do NOT pull in the heavier bidi sanitizer.

use std::io::Read;

use serde_json::Value;

/// Upper bound on bytes read from stdin in [`HookInput::read_from_stdin`]. A hook payload is a tiny
/// JSON object; this only exists so a wedged/hostile writer can never make us slurp unbounded.
const MAX_STDIN_BYTES: u64 = 1024 * 1024; // 1 MiB

/// A Claude Code hook stdin payload. Every field is `Option` — the harness owns this schema and may
/// change it; parse defensively (spec §2/§6). String fields are control-char-stripped on parse so
/// nothing hostile (a newline in a `session_id`, control chars in a path) reaches logs or the wire.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct HookInput {
    pub session_id: Option<String>,
    pub transcript_path: Option<String>,
    pub cwd: Option<String>,
    pub source: Option<String>, // SessionStart: startup|resume|clear|compact
    pub reason: Option<String>, // SessionEnd
}

impl HookInput {
    /// Parse one hook-stdin JSON object. Errs on non-JSON ([`HookParseError::NotJson`]) or a
    /// non-object top-level value ([`HookParseError::NotObject`]); a missing field, or a field
    /// present but not a string, becomes `None` (defensive — never an error). Each recognized string
    /// field is control-char-stripped (see the module docs) before it is stored.
    pub fn parse(input: &str) -> Result<Self, HookParseError> {
        let value: Value = serde_json::from_str(input).map_err(|_| HookParseError::NotJson)?;
        let obj = value.as_object().ok_or(HookParseError::NotObject)?;
        // A field present but not a string → `None` (defensive): we only ever surface a clean owned
        // String or nothing, never a coerced non-string.
        let field = |key: &str| -> Option<String> {
            obj.get(key).and_then(Value::as_str).map(strip_control_chars)
        };
        Ok(HookInput {
            session_id: field("session_id"),
            transcript_path: field("transcript_path"),
            cwd: field("cwd"),
            source: field("source"),
            reason: field("reason"),
        })
    }

    /// Read ALL of stdin (BYTE-bounded to [`MAX_STDIN_BYTES`]) and parse it. On ANY failure — read
    /// error, truncation past the bound, non-UTF-8, non-JSON, non-object — return
    /// [`HookInput::default`] so the hook paths degrade gracefully. [`Read::take`] caps the read at 1
    /// MiB so a hostile/wedged writer can never make us slurp unbounded *memory*. Note this bounds
    /// BYTES, not TIME: a writer that holds the pipe open, trickles a few bytes, then stalls without
    /// EOF would still block `read_to_string`. That is a non-issue in deployment — the hook pipe is
    /// harness-owned and closed after the tiny payload — so no timeout is warranted for a millisecond
    /// hook subprocess.
    pub fn read_from_stdin() -> Self {
        let mut buf = String::new();
        let stdin = std::io::stdin();
        if stdin.lock().take(MAX_STDIN_BYTES).read_to_string(&mut buf).is_err() {
            return Self::default();
        }
        Self::parse(&buf).unwrap_or_default()
    }
}

/// Decide whether the SessionEnd capture poke should fire, returning the
/// `(session_id, transcript_path)` pair ONLY when BOTH fields are present AND non-empty.
///
/// This is the load-bearing B1-contract filter: [`HookInput::read_from_stdin`] surfaces a
/// present-but-empty field as `Some("")` (and a control-only value strips to `""`), so an empty value
/// here means "the harness gave us nothing usable" and MUST be treated as ABSENT — we skip the poke
/// rather than send the daemon a blank id or path it would only reject. The `capture-notify`
/// subcommand pokes iff this returns `Some`.
pub fn capture_target(h: &HookInput) -> Option<(String, String)> {
    let session_id = h.session_id.as_deref().filter(|s| !s.is_empty())?;
    let transcript_path = h.transcript_path.as_deref().filter(|s| !s.is_empty())?;
    Some((session_id.to_string(), transcript_path.to_string()))
}

/// Drop every Unicode control character (C0 incl. `\n`/`\r`/`\t`, `DEL`, and the C1 range) from a
/// string field so it is safe to place in a log line or a wire frame. This is a *strip* (the char is
/// removed), not a map-to-space: `"a b\nc"` → `"a bc"` — ordinary spaces are preserved, controls
/// vanish. See the module docs for why this narrow scope (not the A11 bidi sanitizer) is correct for
/// hook metadata.
fn strip_control_chars(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}

/// Why a hook payload could not be parsed. Both variants are non-fatal to the caller — the hook
/// paths map either to [`HookInput::default`] — but keeping them distinct aids tests and any future
/// diagnostics.
#[derive(Debug)]
pub enum HookParseError {
    /// The input was not well-formed JSON (or was empty).
    NotJson,
    /// The input parsed as JSON but the top level was not an object (a bare array/string/number).
    NotObject,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_session_start_and_end_payloads() {
        let start = r#"{"session_id":"abc-123","transcript_path":"/t.jsonl","cwd":"/repo",
            "hook_event_name":"SessionStart","source":"compact"}"#;
        let h = HookInput::parse(start).unwrap();
        assert_eq!(h.source.as_deref(), Some("compact"));
        assert_eq!(h.session_id.as_deref(), Some("abc-123"));
        assert_eq!(h.transcript_path.as_deref(), Some("/t.jsonl"));
        let end = r#"{"session_id":"abc-123","transcript_path":"/t.jsonl","cwd":"/repo",
            "hook_event_name":"SessionEnd","reason":"clear"}"#;
        let h = HookInput::parse(end).unwrap();
        assert_eq!(h.reason.as_deref(), Some("clear"));
    }

    #[test]
    fn garbage_is_err_missing_fields_are_none_not_error() {
        assert!(HookInput::parse("not json").is_err());
        assert!(HookInput::parse("").is_err());
        let h = HookInput::parse(r#"{"hook_event_name":"SessionStart"}"#).unwrap();
        assert!(h.session_id.is_none() && h.transcript_path.is_none() && h.source.is_none());
    }

    #[test]
    fn control_chars_stripped_from_string_fields() {
        // hostile control chars (incl newline) stripped BEFORE the values reach logs/wire (spec §6).
        // ALL FIVE string fields go through the same strip closure — pin every one so a future
        // refactor that special-cases a single field can't silently reopen the hole.
        let h = HookInput::parse(
            "{\"session_id\":\"a b\\nc\",\"transcript_path\":\"/x\\r.jsonl\",\
             \"cwd\":\"/re\\tpo\",\"source\":\"comp\\u0000act\",\"reason\":\"cle\\u0007ar\"}",
        )
        .unwrap();
        assert_eq!(h.session_id.as_deref(), Some("a bc")); // \n dropped (strip, not map-to-space)
        assert_eq!(h.transcript_path.as_deref(), Some("/x.jsonl")); // \r dropped
        assert_eq!(h.cwd.as_deref(), Some("/repo")); // \t dropped
        assert_eq!(h.source.as_deref(), Some("compact")); // NUL dropped
        assert_eq!(h.reason.as_deref(), Some("clear")); // BEL dropped
        // No control char survives on any field.
        for field in [&h.session_id, &h.transcript_path, &h.cwd, &h.source, &h.reason] {
            assert!(!field.as_deref().unwrap().chars().any(|c| c.is_control()));
        }
    }

    #[test]
    fn non_object_json_is_err() {
        assert!(HookInput::parse("[1,2,3]").is_err());
        assert!(HookInput::parse("\"a string\"").is_err());
        assert!(HookInput::parse("42").is_err());
    }

    #[test]
    fn non_string_field_is_none_not_error() {
        // A field present but of the wrong JSON type degrades to `None`, never an error.
        let h = HookInput::parse(r#"{"session_id":42,"source":true,"cwd":["/x"]}"#).unwrap();
        assert!(h.session_id.is_none());
        assert!(h.source.is_none());
        assert!(h.cwd.is_none());
    }
}
