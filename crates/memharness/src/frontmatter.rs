//! Pure text helpers: strip a leading YAML frontmatter block; detect Korean content.
//! Whether stripping is APPLIED is probe-pinned (`corpus::STRIP_FRONTMATTER`, spec §2 Rev 2) —
//! this module only provides the mechanism.

/// Strip a leading YAML frontmatter block: the file MUST begin (byte 0) with a `---` line,
/// and the block ends at the next line that is exactly `---`. Returns the remainder after that
/// closing fence's newline. No opening fence at byte 0 or no closing fence → input unchanged
/// (a lone `---` / horizontal rule is NOT frontmatter).
pub fn strip_frontmatter(input: &str) -> &str {
    let after_open = match input.strip_prefix("---\n") {
        Some(rest) => rest,
        None => return input,
    };
    let mut search_from = 0usize;
    loop {
        let slice = &after_open[search_from..];
        if let Some(rel) = slice.find("---\n") {
            let abs = search_from + rel;
            let at_line_start = abs == 0 || after_open.as_bytes()[abs - 1] == b'\n';
            if at_line_start {
                return &after_open[abs + 4..];
            }
            search_from = abs + 4;
        } else if let Some(rel) = slice.find("---") {
            let abs = search_from + rel;
            let at_line_start = abs == 0 || after_open.as_bytes()[abs - 1] == b'\n';
            if at_line_start && abs + 3 == after_open.len() {
                return ""; // closing fence at EOF with no trailing newline
            }
            return input;
        } else {
            return input;
        }
    }
}

/// Coarse language tag. Phase 0 only isolates the KO segment (the expected bilingual gap,
/// spec §3/§8). ANY Hangul codepoint ⇒ `Ko`; mixed folds into `Ko` deliberately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    Ko,
}

/// Hangul ranges: Syllables U+AC00–U+D7A3, Jamo U+1100–U+11FF, Compat Jamo U+3130–U+318F.
pub fn detect_lang(text: &str) -> Lang {
    let has_hangul = text.chars().any(|c| {
        let u = c as u32;
        (0xAC00..=0xD7A3).contains(&u)
            || (0x1100..=0x11FF).contains(&u)
            || (0x3130..=0x318F).contains(&u)
    });
    if has_hangul { Lang::Ko } else { Lang::En }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_leading_frontmatter_block() {
        let input = "---\ntitle: Hello\ntags: [a, b]\n---\n# Body\ntext here\n";
        assert_eq!(strip_frontmatter(input), "# Body\ntext here\n");
    }

    #[test]
    fn no_frontmatter_is_returned_unchanged() {
        let input = "# Body\nno frontmatter\n";
        assert_eq!(strip_frontmatter(input), input);
    }

    #[test]
    fn a_lone_triple_dash_is_not_frontmatter() {
        let input = "# Body\n---\nmore\n";
        assert_eq!(strip_frontmatter(input), input);
        let unclosed = "---\ntitle: x\nnever closes\n";
        assert_eq!(strip_frontmatter(unclosed), unclosed);
    }

    #[test]
    fn frontmatter_must_start_at_byte_zero() {
        let input = "\n---\ntitle: x\n---\nbody\n";
        assert_eq!(strip_frontmatter(input), input, "leading blank line means no frontmatter");
    }

    #[test]
    fn detects_korean_by_hangul_presence() {
        assert_eq!(detect_lang("안녕하세요 세계"), Lang::Ko);
        assert_eq!(detect_lang("hello world"), Lang::En);
        assert_eq!(detect_lang("the term 메모리 means memory"), Lang::Ko);
        assert_eq!(detect_lang(""), Lang::En);
    }
}
