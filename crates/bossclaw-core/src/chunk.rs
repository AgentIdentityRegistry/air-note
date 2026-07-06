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

/// Overlap must be strictly smaller than the budget, else a chunk full of
/// carried-over context has zero room for new content and the sliding window
/// never advances (infinite loop). Enforced at compile time.
const _: () = assert!(CHUNK_OVERLAP_CHARS < CHUNK_BUDGET_CHARS);

/// The separator that delimits paragraph/heading blocks. A run of one-or-more
/// blank lines collapses to this canonical join when whole blocks are packed
/// back into a single chunk.
const BLOCK_SEPARATOR: &str = "\n\n";

/// Split `text` into 1..N overlapping, char-boundary-safe chunks (spec §3.4).
/// Returns an empty Vec for empty/whitespace-only input. Text within budget
/// returns exactly `[text.to_string()]` (short docs unchanged).
pub fn chunk_text(text: &str) -> Vec<String> {
    if text.trim().is_empty() {
        return Vec::new();
    }
    let total_chars = text.chars().count();
    if total_chars <= CHUNK_BUDGET_CHARS {
        return vec![text.to_string()]; // fast path: one unchanged chunk
    }

    // 1) Split into paragraph/heading blocks on blank-line boundaries, dropping
    //    empty (whitespace-only) blocks — a run of blank lines is one separator.
    let blocks: Vec<Vec<char>> = split_into_blocks(text);

    // 2) Greedily pack whole blocks into chunks; hard-split any single block that
    //    exceeds the budget; 3+4) carry CHUNK_OVERLAP_CHARS of context forward.
    let separator: Vec<char> = BLOCK_SEPARATOR.chars().collect();
    let mut chunks: Vec<Vec<char>> = Vec::new();
    let mut current: Vec<char> = Vec::new();

    for block in &blocks {
        // A block that fits whole into the remaining room joins the current chunk.
        let joined_len = current.len()
            + if current.is_empty() { 0 } else { separator.len() }
            + block.len();
        if joined_len <= CHUNK_BUDGET_CHARS {
            if !current.is_empty() {
                current.extend_from_slice(&separator);
            }
            current.extend_from_slice(block);
            continue;
        }

        // The block does not fit. Flush what we have, then place the block —
        // hard-splitting it across as many overlap-prefixed chunks as needed.
        flush(&mut current, &mut chunks);
        emit_block_with_overlap(block, &mut chunks);
    }
    flush(&mut current, &mut chunks);

    chunks.into_iter().map(|c| c.into_iter().collect()).collect()
}

/// Split on runs of one-or-more blank lines, returning non-empty char-vec blocks.
/// Leading/trailing whitespace inside a block is trimmed so a block's char count
/// reflects real content (and a whitespace-only block is dropped entirely).
fn split_into_blocks(text: &str) -> Vec<Vec<char>> {
    // A "blank line" run is two-or-more consecutive newlines (ignoring the
    // spaces/tabs that may pad an otherwise-empty line).
    text.split("\n\n")
        .flat_map(|part| part.split("\r\n\r\n"))
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| p.chars().collect())
        .collect()
}

/// Move `current` into `chunks` as a finished chunk, if it has any content.
/// Never emits an empty chunk (guard rail i).
fn flush(current: &mut Vec<char>, chunks: &mut Vec<Vec<char>>) {
    if !current.is_empty() {
        chunks.push(std::mem::take(current));
    }
}

/// Place a single (possibly oversized) block into `chunks`, hard-splitting it on
/// char-window boundaries. Each new chunk after the first carries the trailing
/// `CHUNK_OVERLAP_CHARS` chars of the previous chunk as leading context. All
/// slicing is on the block's `char` vector, so no codepoint is ever split.
fn emit_block_with_overlap(block: &[char], chunks: &mut Vec<Vec<char>>) {
    let mut pos = 0;
    while pos < block.len() {
        // Seed the chunk with overlap carried from the immediately-preceding chunk.
        let mut chunk: Vec<char> = overlap_tail(chunks);
        // Fill the remaining room from the block. `overlap < budget` (compile-time
        // guarded) guarantees room >= 1, so `pos` always advances.
        let room = CHUNK_BUDGET_CHARS - chunk.len();
        let take = room.min(block.len() - pos);
        chunk.extend_from_slice(&block[pos..pos + take]);
        pos += take;
        chunks.push(chunk);
    }
}

/// The trailing `CHUNK_OVERLAP_CHARS` chars of the last emitted chunk, or an
/// empty vec if there is none. Derived from an already-chunked char slice, so
/// the overlap can never split a codepoint (guard rail iii).
fn overlap_tail(chunks: &[Vec<char>]) -> Vec<char> {
    match chunks.last() {
        Some(prev) => {
            let start = prev.len().saturating_sub(CHUNK_OVERLAP_CHARS);
            prev[start..].to_vec()
        }
        None => Vec::new(),
    }
}

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
