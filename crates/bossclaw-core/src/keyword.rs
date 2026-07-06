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
    fn multi_word_queries_become_per_term_or() {
        // The rung-1 fix (retrieval-floor spec Rev 2 §3.2): terms match INDEPENDENTLY, ranked
        // by the BM25 that is already there — not as one exact adjacent phrase.
        assert_eq!(escape_fts_query("hello world"), r#""hello" OR "world""#);
    }

    #[test]
    fn single_term_stays_one_quoted_phrase() {
        assert_eq!(escape_fts_query("hello"), r#""hello""#);
    }

    #[test]
    fn operator_words_are_quoted_terms_never_operators() {
        // User tokens that spell FTS5 operators stay INSIDE quotes — injection-safe.
        assert_eq!(escape_fts_query("foo OR bar"), r#""foo" OR "OR" OR "bar""#);
        assert_eq!(escape_fts_query("NOT valid"), r#""NOT" OR "valid""#);
        assert_eq!(escape_fts_query("a AND b"), r#""a" OR "AND" OR "b""#);
        assert_eq!(escape_fts_query("near(foo bar)"), r#""near(foo" OR "bar)""#);
        assert_eq!(escape_fts_query("prefix*"), r#""prefix*""#);
    }

    #[test]
    fn embedded_double_quotes_are_doubled_per_term() {
        assert_eq!(escape_fts_query(r#"a"b"#), r#""a""b""#);
        assert_eq!(escape_fts_query(r#"say "hi"#), r#""say" OR """hi""#);
    }

    #[test]
    fn punctuation_only_terms_are_tolerated() {
        // Critic-verified: FTS5 tolerates a quoted punctuation token; wasteful but never an error.
        assert_eq!(escape_fts_query("foo - bar"), r#""foo" OR "-" OR "bar""#);
    }

    #[test]
    fn korean_terms_tokenize_on_whitespace() {
        assert_eq!(escape_fts_query("메모리 하니스"), r#""메모리" OR "하니스""#);
    }

    #[test]
    fn multiline_mined_queries_split_across_lines() {
        assert_eq!(
            escape_fts_query("line one\nline two"),
            r#""line" OR "one" OR "line" OR "two""#
        );
    }

    #[test]
    fn empty_and_whitespace_only_keep_the_empty_phrase_contract() {
        assert_eq!(escape_fts_query(""), "\"\"");
        assert_eq!(escape_fts_query("   \n\t"), "\"\"");
    }
}
