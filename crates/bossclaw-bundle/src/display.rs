//! Rendering attacker-chosen text without letting it forge the surface it is rendered on.
//!
//! THE PREMISE OF L1 IS THAT THE PRODUCER CHOSE EVERYTHING. An attacker mints their own brain key
//! and their own identity key, so a bundle that VERIFIES can still carry any `did`, any `purpose`,
//! any `kind` they like — and those strings are exactly the ones a verdict surface quotes back.
//! A newline in a `did` prints a second line of the tool's own output; an ANSI cursor-up plus
//! erase-line deletes the honest lines above it, leaving a forged green verdict with no residue.
//! Demonstrated end to end against the shipped `air-verify` binary (exit 0, with a `did` that
//! printed its own `identity: registry-resolved`).
//!
//! Sanitizing at the point the string is BUILT is what keeps this from having to be re-solved by
//! every consumer: the CLI, Task 10's app sheet, and SP-V2's WASM host all render these details.

/// Longest attacker-chosen run of text any surface renders. A verifying bundle may carry a ~32 MiB
/// `did`; a receiver needs to SEE it, not be flooded by it, and 256 characters is far more than any
/// honest value of these fields.
pub const MAX_RENDERED_CHARS: usize = 256;

/// The stand-in printed for a character that could forge a line. U+FFFD is the standard "something
/// was here that cannot be shown" marker, so the receiver sees evidence of tampering rather than a
/// silently cleaned-up string.
const REPLACEMENT: char = '\u{fffd}';

/// Make `s` safe to render: every line-forging character becomes [`REPLACEMENT`], and the result is
/// truncated to [`MAX_RENDERED_CHARS`] with a `…` marking that something was cut.
///
/// "Line-forging" is `char::is_control` (C0, DEL, and the C1 range — this is what covers `\n`, `\r`,
/// and the `ESC` that starts every ANSI escape sequence) PLUS U+2028/U+2029, which are not `Cc` but
/// are line breaks to enough renderers to matter for the app and WASM surfaces this is shared with.
/// Homoglyph and bidi-override spoofing are deliberately NOT in scope: they mislead about what a
/// string SAYS, which no escaping fixes, whereas this closes the narrower hole of a string escaping
/// its own line and impersonating the verifier's own output.
pub fn sanitize_for_display(s: &str) -> String {
    let mut out: String = s
        .chars()
        .take(MAX_RENDERED_CHARS)
        .map(|c| if forges_a_line(c) { REPLACEMENT } else { c })
        .collect();
    // Walks at most MAX_RENDERED_CHARS + 1 characters, so a 32 MiB string costs no more than a
    // short one — the whole point of bounding this before anyone iterates it.
    if s.chars().nth(MAX_RENDERED_CHARS).is_some() {
        out.push('…');
    }
    out
}

fn forges_a_line(c: char) -> bool {
    c.is_control() || c == '\u{2028}' || c == '\u{2029}'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newlines_and_escapes_cannot_survive_into_a_rendered_line() {
        let forged = "did:wba:evil.com:x\nidentity: registry-resolved\u{1b}[1A\u{1b}[2K";
        let safe = sanitize_for_display(forged);
        assert!(!safe.contains('\n'), "{safe}");
        assert!(!safe.contains('\u{1b}'), "{safe}");
        assert!(!safe.contains('\r'), "{safe}");
        // The words survive — the receiver still sees WHAT the file claimed, on ONE line.
        assert!(safe.contains("did:wba:evil.com:x"), "{safe}");
        assert_eq!(safe.lines().count(), 1, "{safe}");
    }

    #[test]
    fn unicode_line_separators_are_forging_characters_too() {
        for sep in ['\u{2028}', '\u{2029}'] {
            let safe = sanitize_for_display(&format!("a{sep}b"));
            assert_eq!(safe, format!("a{REPLACEMENT}b"));
        }
    }

    #[test]
    fn over_long_text_is_truncated_and_says_so() {
        let safe = sanitize_for_display(&"x".repeat(MAX_RENDERED_CHARS * 4));
        assert_eq!(safe.chars().count(), MAX_RENDERED_CHARS + 1);
        assert!(safe.ends_with('…'));
    }

    #[test]
    fn text_at_the_limit_is_not_marked_as_truncated() {
        // The `…` must mean something was cut, or it becomes noise on every honest value.
        let safe = sanitize_for_display(&"x".repeat(MAX_RENDERED_CHARS));
        assert_eq!(safe, "x".repeat(MAX_RENDERED_CHARS));
    }

    #[test]
    fn honest_text_including_non_ascii_passes_through_unchanged() {
        for honest in ["did:wba:example.com:me", "session", "memory-signing", "réunion 会議"] {
            assert_eq!(sanitize_for_display(honest), honest);
        }
    }
}
