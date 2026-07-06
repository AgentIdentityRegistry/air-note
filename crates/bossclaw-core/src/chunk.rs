//! Heading/paragraph-aware text chunking (retrieval-floor spec Rev 2 §3.4).
//!
//! One embeddable event's text is split into 1..N overlapping chunks so that a
//! long document contributes several focused embedding targets instead of one
//! averaged-out vector. Pure: no I/O, no SQL. The caller (`log.rs`) embeds each
//! returned chunk and writes it under a composite `(event_id, model_id, chunk_ix)`
//! key; recall folds the chunks back to one best-scoring hit per event.
//!
//! Invariants (asserted by the tests below):
//! - text whose char-length ≤ `CHUNK_BUDGET_CHARS` ⇒ EXACTLY ONE chunk equal to
//!   the input (short docs are byte-identical to today — no behavior change).
//! - splits prefer paragraph/heading boundaries; a paragraph larger than the
//!   budget is hard-split on a CHAR boundary (never a byte boundary — Korean and
//!   other multi-byte scripts must never be sliced mid-codepoint).
//! - consecutive chunks share `CHUNK_OVERLAP_CHARS` of trailing/leading context.
//! - chunk indices are dense and 0-based (`0..n`), stable for a given input.

/// Max chars per chunk. This is a GRANULARITY knob, NOT a context-window guard:
/// potion-base-8M is a model2vec `StaticModel` that MEAN-POOLS token embeddings
/// (no transformer window; `config.json` seq_length = 1_000_000), so a larger
/// chunk never truncates — it only DILUTES the mean over more tokens. Smaller
/// chunks = sharper, less-diluted matches; the win is measured by the frozen
/// gate (a v2 re-tune bumps the effective-id suffix and re-migrates). ~1,500
/// chars keeps most memory/page events at ONE chunk (common case unchanged).
/// Char count, never byte count. A measurement subject, not a tuned truth.
pub const CHUNK_BUDGET_CHARS: usize = 1_500;

/// Chars of overlap carried between adjacent chunks so a fact spanning a split
/// point still appears whole in at least one chunk. ~13% of the budget — enough
/// to bridge a sentence, small enough to avoid ~2× row inflation.
pub const CHUNK_OVERLAP_CHARS: usize = 200;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_exactly_one_unchanged_chunk() {
        let t = "A single short paragraph.\n\nTwo short paragraphs, still tiny.";
        let chunks = chunk_text(t);
        assert_eq!(chunks.len(), 1, "≤ budget ⇒ one chunk");
        assert_eq!(chunks[0], t, "short doc is byte-identical to input");
    }

    #[test]
    fn empty_and_whitespace_yield_no_chunks() {
        assert!(chunk_text("").is_empty(), "empty text ⇒ zero chunks (nothing to embed)");
        assert!(chunk_text("   \n\t \n").is_empty(), "whitespace-only ⇒ zero chunks");
    }

    #[test]
    fn long_text_splits_on_paragraph_boundaries_within_budget() {
        // Three paragraphs each ~700 chars; budget 1500 ⇒ para1+para2 in chunk 0,
        // para3 in chunk 1 (a paragraph is never split when it fits whole).
        let para = "x".repeat(700);
        let t = format!("{para}\n\n{para}\n\n{para}");
        let chunks = chunk_text(&t);
        assert!(chunks.len() >= 2, "3×700 chars must exceed one 1500-char chunk: {}", chunks.len());
        for c in &chunks {
            assert!(c.chars().count() <= CHUNK_BUDGET_CHARS, "each chunk within budget");
        }
    }

    #[test]
    fn oversized_paragraph_hard_splits_on_char_boundary_never_mid_codepoint() {
        // 4,000 Korean chars in ONE paragraph (no split points) forces a hard split.
        let t: String = "가".repeat(4_000);
        let chunks = chunk_text(&t);
        assert!(chunks.len() >= 3, "4000 KO chars over a 1500 budget ⇒ ≥3 chunks");
        for c in &chunks {
            assert!(c.chars().count() <= CHUNK_BUDGET_CHARS);
            // The load-bearing KO safety property: every chunk is valid UTF-8 with
            // only whole '가' codepoints — a byte slice would corrupt these.
            let preview: String = c.chars().take(8).collect();
            assert!(c.chars().all(|ch| ch == '가'), "no mid-codepoint slice: {preview:?}");
        }
        // Reassembling the chunks minus the overlaps recovers every codepoint.
        let total: usize = chunks.iter().map(|c| c.chars().count()).sum();
        assert!(total >= 4_000, "no characters dropped (overlap only adds)");
    }

    #[test]
    fn adjacent_chunks_overlap_by_the_configured_budget() {
        let para = "y".repeat(1_400);
        let t = format!("{para}\n\n{para}"); // 2 paras, each near-budget ⇒ 2 chunks
        let chunks = chunk_text(&t);
        assert!(chunks.len() >= 2);
        // The tail of chunk[i] and the head of chunk[i+1] must share overlap chars.
        let tail: String = chunks[0].chars().rev().take(CHUNK_OVERLAP_CHARS).collect();
        let head: String = chunks[1].chars().take(CHUNK_OVERLAP_CHARS).collect();
        let tail_fwd: String = tail.chars().rev().collect();
        assert!(
            chunks[1].starts_with(&head) && chunks[0].ends_with(&tail_fwd),
            "adjacent chunks carry {CHUNK_OVERLAP_CHARS} chars of shared context"
        );
    }

    #[test]
    fn chunking_is_deterministic() {
        let t = format!("{}\n\n{}", "z".repeat(2_000), "w".repeat(2_000));
        assert_eq!(chunk_text(&t), chunk_text(&t), "same input ⇒ same chunks (stable ix)");
    }
}
