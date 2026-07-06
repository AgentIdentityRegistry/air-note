//! The cloud audit: a minimal Anthropic Messages API POST via `ureq`, key from
//! `ANTHROPIC_API_KEY` env (read by main.rs). The auditor implements the SAME `PairJudge`
//! trait as the local judge — blind, position-swapped, one-token replies. Strict parse; any
//! failure degrades to "audit incomplete — trust verdict unavailable" (never fabricates).
//! A one-token PREFLIGHT runs before the expensive loop (fail fast, spec §5 Rev 2).

/// The pinned audit model (a current Sonnet-tier id — confirmed/adjusted by Probe D). Used as
/// the REFERENCE the judge is checked against, whether the judge is the local 7B or cloud Haiku.
pub const AUDIT_MODEL: &str = "claude-sonnet-5";

/// The pinned cloud-JUDGE model for `--judge cloud`: Haiku 4.5 — cheap, low-latency, and a far
/// sharper referee than a quantized local 7B (the 2026-07-06 run measured the local judge at only
/// 53% agreement with Sonnet). Verified accepted by the Messages API 2026-07-06 (the
/// `claude-haiku-4-5` alias resolves to this dated id). The audit ladder self-checks THIS judge
/// against AUDIT_MODEL, so a Haiku that drifts from Sonnet is caught — never trusted blindly.
pub const JUDGE_MODEL: &str = "claude-haiku-4-5-20251001";

use std::sync::OnceLock;
use std::time::Duration;

use anyhow::Context;
use serde::Deserialize;

use crate::judge::{pairwise_prompt, parse_pick_token, PairJudge, PosPick};

const ANTHROPIC_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Overall per-request deadline. Cloud Messages calls complete in seconds; 60s covers slow
/// tails without letting a wedged connection stall the audit loop (same discipline as
/// ollama::OLLAMA_TIMEOUT_SECS — see the Tasks 11-12 review).
const ANTHROPIC_TIMEOUT_SECS: u64 = 60;
/// Cap on error-body text quoted into error messages.
const ERROR_BODY_MAX_CHARS: usize = 500;

/// Shared keep-alive agent with the overall deadline on every request (the bare `ureq` free
/// functions have NO read timeout in ureq 2 — one wedged call would hang the whole run).
fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new().timeout(Duration::from_secs(ANTHROPIC_TIMEOUT_SECS)).build()
    })
}

#[derive(Deserialize)]
struct MessagesBody {
    content: Vec<ContentBlock>,
}
#[derive(Deserialize)]
struct ContentBlock {
    #[serde(default, rename = "type")]
    block_type: String,
    #[serde(default)]
    text: String,
}

/// Extract the first text block from a Messages body (strict: none = error).
pub fn extract_text(body: &str) -> anyhow::Result<String> {
    let parsed: MessagesBody = serde_json::from_str(body)?;
    parsed
        .content
        .iter()
        .find(|b| b.block_type == "text")
        .map(|b| b.text.clone())
        .ok_or_else(|| anyhow::anyhow!("no text block in Messages response"))
}

#[derive(serde::Serialize)]
struct MessagesReq<'a> {
    model: &'a str,
    max_tokens: u32,
    thinking: Thinking,
    messages: Vec<ReqMessage<'a>>,
}
/// Current Claude models (Sonnet AND Haiku) can enable ADAPTIVE thinking when this field is
/// omitted, and max_tokens covers thinking + text COMBINED — an unprompted thinking block would
/// eat the 4/8-token budget and return no text block, deterministically blanking the verdict.
/// Explicitly off. Verified accepted by both AUDIT_MODEL and JUDGE_MODEL (2026-07-06).
#[derive(serde::Serialize)]
struct Thinking {
    #[serde(rename = "type")]
    kind: &'static str,
}
#[derive(serde::Serialize)]
struct ReqMessage<'a> {
    role: &'a str,
    content: &'a str,
}

/// Statuses Anthropic documents as retryable: rate limit (429), server errors (500/502/503),
/// and overloaded (529). Everything else (auth, bad request, retired model) fails fast.
const RETRYABLE_STATUSES: [u16; 5] = [429, 500, 502, 503, 529];
/// Sleeps between retry attempts (the caller is sync); indexed by completed-attempt count.
const RETRY_BACKOFF_SECS: [u64; 2] = [2, 4];

/// One Messages POST → the reply text, retried up to `RETRY_BACKOFF_SECS.len()` times on
/// retryable statuses. The request is idempotent and Anthropic documents 429/5xx/529 as
/// retryable, so a bounded retry keeps one transient blip from blanking the trust verdict of a
/// 2h run — while persistent failure still errors out (the caller records "audit incomplete"
/// and stops egress; a fabricated verdict is never produced). Transport errors and other
/// statuses fail fast. Anthropic puts the real cause (invalid key, retired model id) in the
/// JSON error body, so non-2xx responses quote it.
fn post_message(api_key: &str, model: &str, prompt: &str, max_tokens: u32) -> anyhow::Result<String> {
    let mut attempts = 0;
    loop {
        let result = agent()
            .post(ANTHROPIC_URL)
            .set("x-api-key", api_key)
            .set("anthropic-version", ANTHROPIC_VERSION)
            .set("content-type", "application/json")
            .send_json(MessagesReq {
                model,
                max_tokens,
                thinking: Thinking { kind: "disabled" },
                messages: vec![ReqMessage { role: "user", content: prompt }],
            });
        match result {
            Ok(resp) => {
                let body =
                    resp.into_string().context("reading anthropic messages response")?;
                return extract_text(&body);
            }
            Err(ureq::Error::Status(code, resp)) => {
                let retryable = RETRYABLE_STATUSES.contains(&code);
                if retryable && attempts < RETRY_BACKOFF_SECS.len() {
                    std::thread::sleep(Duration::from_secs(RETRY_BACKOFF_SECS[attempts]));
                    attempts += 1;
                    continue;
                }
                let body = resp.into_string().unwrap_or_default();
                let body: String = body.chars().take(ERROR_BODY_MAX_CHARS).collect();
                anyhow::bail!("Anthropic Messages call failed: status {code}: {body}");
            }
            Err(other) => anyhow::bail!("Anthropic Messages call failed: {other}"),
        }
    }
}

/// Fail-fast preflight (Rev 2, finding 6): ONE tiny call with the pinned model BEFORE the
/// expensive loop — a bad key or retired model id fails in seconds, not after 2h. The reply
/// content is irrelevant; only success matters.
pub fn preflight(api_key: &str, model: &str) -> anyhow::Result<()> {
    post_message(api_key, model, "Reply with exactly: OK", 4).map(|_| ()).map_err(|e| {
        anyhow::anyhow!(
            "Anthropic preflight failed ({e}). Check ANTHROPIC_API_KEY and that model \
             '{model}' exists (update the pinned model id if retired). Or run with --local-only."
        )
    })
}

/// The cloud auditor: the SAME `PairJudge` protocol as the local judge (blind +
/// position-swapped via `judge_pair_blind`; identical prompt; one-token reply parsed by the
/// shared rule — ambiguous → None → Uncertain).
pub struct AnthropicAuditor {
    pub api_key: String,
    /// Which model does the picking — AUDIT_MODEL (Sonnet) in the audit role, JUDGE_MODEL
    /// (Haiku) when it serves as the `--judge cloud` judge. Same protocol either way.
    pub model: String,
}

impl PairJudge for AnthropicAuditor {
    fn pick(&self, query: &str, answer_a: &str, answer_b: &str) -> anyhow::Result<Option<PosPick>> {
        // max_tokens 8: a truncated negation ("Answer B is worse than…") could still parse
        // decisively; frontier one-token compliance makes that rare, and the parser's
        // ambiguity rules (uppercase-only signals, article blockers) are the main guard.
        let reply = post_message(&self.api_key, &self.model, &pairwise_prompt(query, answer_a, answer_b), 8)?;
        Ok(parse_pick_token(&reply))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::judge::PosPick;

    #[test]
    fn extracts_first_text_block() {
        let body = r#"{"content":[{"type":"text","text":"A"}]}"#;
        assert_eq!(extract_text(body).unwrap(), "A");
        assert!(extract_text(r#"{"content":[]}"#).is_err(), "no text block = error");
        assert!(extract_text("not json").is_err());
    }

    #[test]
    fn messages_request_disables_thinking_and_carries_the_model() {
        // Guards the adaptive-thinking trap: omitting `thinking` lets a current model spend the
        // whole 4/8-token budget on a thinking block → no text block → audit_incomplete.
        // Also guards that the model PARAM reaches the request body (so `--judge cloud`'s Haiku
        // isn't silently sent to Sonnet).
        let req = MessagesReq {
            model: JUDGE_MODEL,
            max_tokens: 8,
            thinking: Thinking { kind: "disabled" },
            messages: vec![ReqMessage { role: "user", content: "hi" }],
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["thinking"]["type"], "disabled");
        assert_eq!(v["model"], JUDGE_MODEL, "the model param must reach the request body");
    }

    #[test]
    fn audit_reply_parses_via_the_shared_one_token_rule() {
        // The SAME parse_pick_token as the local judge (finding 12): exact first, tokenized
        // fallback, ambiguous → None (→ Uncertain — not dropped).
        assert_eq!(crate::judge::parse_pick_token("B"), Some(PosPick::B));
        assert_eq!(crate::judge::parse_pick_token("Answer: TIE"), Some(PosPick::Tie));
        assert_eq!(crate::judge::parse_pick_token("I cannot decide"), None);
    }
}
