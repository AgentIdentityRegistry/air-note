//! Mine real queries from Claude Code transcripts: `mcp__gbrain__{query,search,recall}` calls
//! become queries; a `mcp__gbrain__get_page` within the next N=5 tool calls of the SAME session
//! labels that page as the used answer (spec §3). Exact dedup (near-dup deferred — a REPORTED
//! caveat, `near_dedup_applied: false`).
//!
//! SHAPE (probe-reconciled 2026-07-03): real transcript lines nest tool calls under
//! `message.content[]` items with `type == "tool_use"`, keyed by line-level `sessionId` — NOT
//! the flat recon shape. Lines that fail to parse (string-form content, garbage) are skipped;
//! only content items whose type is exactly "tool_use" become tool calls, so tool-NAME mentions
//! in text/tool_result content (the 175x naive-grep trap, Probe C) can never be mined.

/// A get_page within this many subsequent tool calls (same session) labels a query.
pub const LABEL_WINDOW: usize = 5;

use serde::Deserialize;

/// A mined real query + its optional implicit known-item label.
#[derive(Debug, Clone)]
pub struct MinedQuery {
    pub text: String,
    pub gold_page_id: Option<String>,
    pub session: String,
}

/// One flattened tool call (the unit the LABEL_WINDOW counts).
struct ToolCall {
    name: String,
    input: serde_json::Value,
    session: String,
}

#[derive(Deserialize)]
struct Line {
    #[serde(default, rename = "sessionId")]
    session_id: String,
    #[serde(default)]
    message: Message,
}
#[derive(Deserialize, Default)]
struct Message {
    #[serde(default)]
    content: Vec<ContentItem>,
}
#[derive(Deserialize)]
struct ContentItem {
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    input: serde_json::Value,
}

const QUERY_TOOLS: [&str; 3] =
    ["mcp__gbrain__query", "mcp__gbrain__search", "mcp__gbrain__recall"];
const GET_PAGE_TOOL: &str = "mcp__gbrain__get_page";

/// Flatten one transcript's JSONL into tool calls, in line order (lines that don't parse are
/// skipped — transcripts are heterogeneous; string-form `content` fails the array parse and is
/// skipped, which is correct: such lines carry no tool calls).
fn flatten_tool_calls(jsonl: &str) -> Vec<ToolCall> {
    let mut calls = Vec::new();
    for line in jsonl.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(parsed) = serde_json::from_str::<Line>(line) else { continue };
        for item in parsed.message.content {
            if item.kind == "tool_use" {
                calls.push(ToolCall {
                    name: item.name,
                    input: item.input,
                    session: parsed.session_id.clone(),
                });
            }
        }
    }
    calls
}

/// Mine ONE transcript's JSONL. Query tools → queries; a get_page within `LABEL_WINDOW`
/// subsequent tool calls of the same session labels the most recent unlabeled query.
/// Exact-text dedup last.
pub fn mine_transcript(jsonl: &str) -> Vec<MinedQuery> {
    let calls = flatten_tool_calls(jsonl);

    let mut mined: Vec<MinedQuery> = Vec::new();
    for (i, call) in calls.iter().enumerate() {
        if QUERY_TOOLS.contains(&call.name.as_str()) {
            if let Some(text) = call.input.get("query").and_then(|v| v.as_str()) {
                mined.push(MinedQuery {
                    text: text.to_string(),
                    gold_page_id: None,
                    session: call.session.clone(),
                });
            }
        } else if call.name == GET_PAGE_TOOL {
            if let Some(slug) = call.input.get("slug").and_then(|v| v.as_str()) {
                label_recent(&mut mined, &calls, i, &call.session, slug);
            }
        }
    }
    dedup_exact(mined)
}

/// Merge-mine MANY transcripts, then exact-dedup the union (a query repeated across files
/// counts once; a later duplicate's label fills an unlabeled first occurrence).
pub fn mine_all<'a>(docs: impl IntoIterator<Item = &'a str>) -> Vec<MinedQuery> {
    let mut all = Vec::new();
    for d in docs {
        all.extend(mine_transcript(d));
    }
    dedup_exact(all)
}

/// Label the most recent unlabeled query of `session` whose originating tool-call index is
/// within LABEL_WINDOW of `get_page_idx`.
///
/// COMPLEXITY NOTE (accepted for Phase 0): this re-scans `calls` per get_page —
/// O(calls × get_pages) worst case. At Phase-0 scale (~118 calls across 40 files) this is
/// microseconds; not worth an index structure.
fn label_recent(
    mined: &mut [MinedQuery],
    calls: &[ToolCall],
    get_page_idx: usize,
    session: &str,
    slug: &str,
) {
    let mut query_call_indices: Vec<usize> = Vec::new();
    for (idx, call) in calls.iter().enumerate() {
        if call.session == session
            && QUERY_TOOLS.contains(&call.name.as_str())
            && call.input.get("query").and_then(|v| v.as_str()).is_some()
        {
            query_call_indices.push(idx);
        }
    }
    let mut session_positions: Vec<usize> = Vec::new();
    for (pos, q) in mined.iter().enumerate() {
        if q.session == session {
            session_positions.push(pos);
        }
    }
    // Same order, same count: the k-th mined query of a session IS its k-th query tool call.
    for (&call_idx, &mined_pos) in query_call_indices.iter().zip(session_positions.iter()).rev() {
        if call_idx < get_page_idx
            && get_page_idx - call_idx <= LABEL_WINDOW
            && mined[mined_pos].gold_page_id.is_none()
        {
            mined[mined_pos].gold_page_id =
                Some(crate::corpus::page_id_from_gbrain_slug(slug));
            return;
        }
    }
}

/// Exact-text dedup keeping the FIRST occurrence; a later duplicate's label fills an
/// unlabeled first.
fn dedup_exact(mined: Vec<MinedQuery>) -> Vec<MinedQuery> {
    use std::collections::HashMap;
    let mut order: Vec<String> = Vec::new();
    let mut by_text: HashMap<String, MinedQuery> = HashMap::new();
    for q in mined {
        match by_text.get_mut(&q.text) {
            Some(existing) => {
                if existing.gold_page_id.is_none() {
                    existing.gold_page_id = q.gold_page_id;
                }
            }
            None => {
                order.push(q.text.clone());
                by_text.insert(q.text.clone(), q);
            }
        }
    }
    order.into_iter().filter_map(|t| by_text.remove(&t)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_queries_labels_and_dedups() {
        let jsonl = include_str!("../tests/fixtures/transcript_synthetic.jsonl");
        let queries = mine_transcript(jsonl);
        let aria: Vec<_> = queries.iter().filter(|q| q.text == "who is Aria Novak").collect();
        assert_eq!(aria.len(), 1, "exact duplicate deduped");
        assert_eq!(aria[0].gold_page_id.as_deref(), Some("people/aria-novak"));
        assert!(queries.iter().any(|q| q.text == "메모리 전략"));
        let ko = queries.iter().find(|q| q.text == "메모리 전략").unwrap();
        assert_eq!(ko.gold_page_id, None, "get_page outside the window is not a label");
        assert!(!queries.iter().any(|q| q.text.contains("mcp__other")));
        assert_eq!(queries.len(), 3, "exactly the 3 real queries; decoy tool-name mentions in text/tool_result never mined");
    }

    #[test]
    fn mine_all_dedups_across_documents() {
        let a = r#"{"type":"assistant","sessionId":"a","message":{"content":[{"type":"tool_use","name":"mcp__gbrain__query","input":{"query":"q1"}}]}}"#;
        let b = r#"{"type":"assistant","sessionId":"b","message":{"content":[{"type":"tool_use","name":"mcp__gbrain__query","input":{"query":"q1"}}]}}
{"type":"assistant","sessionId":"b","message":{"content":[{"type":"tool_use","name":"mcp__gbrain__get_page","input":{"slug":"x/y"}}]}}"#;
        let merged = mine_all([a, b]);
        assert_eq!(merged.len(), 1, "cross-file exact dedup");
        assert_eq!(merged[0].gold_page_id.as_deref(), Some("x/y"), "a later duplicate's label fills in");
    }
}
