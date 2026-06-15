//! FTS5 query escaping utilities.
//!
//! This module contains **only** pure Rust helpers — no SQL, no I/O. All
//! database operations that use these helpers live on [`crate::log::EventLog`].

/// Escape an arbitrary user string so it can be used safely as an FTS5
/// `MATCH` expression.
///
/// FTS5 parses its `MATCH` argument as a query language that recognises
/// operators such as `OR`, `AND`, `NOT`, `NEAR`, `*`, and unbalanced
/// double-quotes as phrase-open/close.  A user-supplied string can contain
/// any of these tokens and would otherwise mutate query semantics or cause
/// a parse error.
///
/// This function wraps the raw string as an FTS5 **quoted phrase**:
/// ```text
/// "raw string here"
/// ```
/// Double-quote characters inside the raw string are doubled (`"` → `""`)
/// following the FTS5 string-literal escaping rule, so the entire input is
/// treated as a literal phrase and no operator injection is possible.
///
/// # Examples
/// ```
/// use bossclaw_core::keyword::escape_fts_query;
///
/// // Plain text — wrapped in quotes.
/// assert_eq!(escape_fts_query("hello world"), "\"hello world\"");
///
/// // FTS5 operator — neutralised inside the quoted phrase.
/// assert_eq!(escape_fts_query("foo OR bar"), "\"foo OR bar\"");
///
/// // Embedded double-quote — doubled per FTS5 escaping rules.
/// assert_eq!(escape_fts_query(r#"say "hi""#), r#""say ""hi""""#);
/// ```
pub fn escape_fts_query(raw: &str) -> String {
    // Double every existing double-quote, then wrap the whole string in quotes.
    let escaped = raw.replace('"', "\"\"");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_quoted() {
        assert_eq!(escape_fts_query("hello world"), "\"hello world\"");
    }

    #[test]
    fn fts5_operators_are_neutralised() {
        // These would be parsed as operators if not quoted.
        assert_eq!(escape_fts_query("foo OR bar"), "\"foo OR bar\"");
        assert_eq!(escape_fts_query("NOT valid"), "\"NOT valid\"");
        assert_eq!(escape_fts_query("a AND b"), "\"a AND b\"");
        assert_eq!(escape_fts_query("near(foo bar)"), "\"near(foo bar)\"");
        assert_eq!(escape_fts_query("prefix*"), "\"prefix*\"");
    }

    #[test]
    fn embedded_double_quotes_are_doubled() {
        // FTS5 escaping: each " inside becomes "".
        assert_eq!(escape_fts_query(r#"say "hi""#), r#""say ""hi""""#);
    }

    #[test]
    fn mixed_operators_and_quotes() {
        let raw = r#"foo OR "bar"#;
        let escaped = escape_fts_query(raw);
        // Should not panic and must be a valid quoted phrase (starts/ends with ").
        assert!(escaped.starts_with('"'));
        assert!(escaped.ends_with('"'));
    }

    #[test]
    fn empty_string_produces_empty_phrase() {
        assert_eq!(escape_fts_query(""), "\"\"");
    }
}
