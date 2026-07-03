//! Loopback HTTP client for Ollama (127.0.0.1:11434) via `ureq` v2 (mirrors bossclaw-core's
//! pin). Two uses: the availability preflight (`/api/tags`) and single-turn generation
//! (`/api/generate`). Default model pinned by Probe B.

/// Probe-B-pinned default local model (the evolve loop's tier is `qwen2.5:7b-instruct`;
/// match whichever tag is actually installed).
pub const DEFAULT_OLLAMA_MODEL: &str = "qwen2.5:7b-instruct";

use std::sync::OnceLock;
use std::time::Duration;

use anyhow::Context;
use serde::Deserialize;

/// Loopback host:port Ollama listens on — the single source for both endpoint URLs and
/// the human-readable errors (a macro so `concat!` can splice it, keeping URL and prose
/// from ever drifting apart).
macro_rules! ollama_addr {
    () => {
        "127.0.0.1:11434"
    };
}
const OLLAMA_ADDR: &str = ollama_addr!();
const OLLAMA_TAGS_URL: &str = concat!("http://", ollama_addr!(), "/api/tags");
const OLLAMA_GENERATE_URL: &str = concat!("http://", ollama_addr!(), "/api/generate");

/// Overall per-request deadline (connect + write + read). Mirrors bossclaw-core's
/// `OLLAMA_TIMEOUT_SECS` discipline: a wedged `ollama serve` must cost at most this,
/// never an infinite hang mid-run; 120s is 8-24x the typical 5-15s generate latency,
/// so legitimate slow generations (cold model load) are never killed.
const OLLAMA_TIMEOUT_SECS: u64 = 120;

/// Cap on how much of an Ollama error body we quote back — enough for its
/// `{"error":"..."}` cause, without dumping an arbitrarily large response into logs.
const ERROR_BODY_MAX_CHARS: usize = 500;

/// Shared keep-alive agent: the run's ~800 sequential calls reuse one connection instead
/// of re-handshaking per call, and every request gets the overall deadline (the bare
/// `ureq` free functions have NO read timeout in ureq 2).
fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new().timeout(Duration::from_secs(OLLAMA_TIMEOUT_SECS)).build()
    })
}

/// Status line + truncated body of a non-2xx response. Ollama puts the real cause in the
/// body (`{"error":"..."}`: OOM, context exceeded, model unloaded) — discarding it makes
/// a call-500-of-800 failure undiagnosable.
fn status_detail(code: u16, resp: ureq::Response) -> String {
    let body = resp.into_string().unwrap_or_default();
    let body: String = body.chars().take(ERROR_BODY_MAX_CHARS).collect();
    format!("status {code}: {body}")
}

#[derive(Deserialize)]
struct TagsBody {
    models: Vec<TagModel>,
}
#[derive(Deserialize)]
struct TagModel {
    name: String,
}

/// Parse model names out of an `/api/tags` body.
pub fn parse_tag_names(body: &str) -> anyhow::Result<Vec<String>> {
    let parsed: TagsBody = serde_json::from_str(body)?;
    Ok(parsed.models.into_iter().map(|m| m.name).collect())
}

/// Require `model` in `names`, else a clear actionable error.
pub fn require_model(names: &[String], model: &str) -> anyhow::Result<()> {
    if names.iter().any(|n| n == model) {
        Ok(())
    } else {
        anyhow::bail!(
            "Ollama model '{model}' is not installed (have: {}). Run: `ollama pull {model}` \
             (or pass --model <installed-tag>).",
            names.join(", ")
        )
    }
}

/// LIVE preflight: `/api/tags` + require `model`. Failures name the fix.
pub fn preflight(model: &str) -> anyhow::Result<()> {
    let body = agent()
        .get(OLLAMA_TAGS_URL)
        .call()
        .map_err(|e| match e {
            // The server IS up but errored — saying "not reachable / ollama serve" here
            // would be wrong advice; report what it actually said.
            ureq::Error::Status(code, resp) => anyhow::anyhow!(
                "Ollama {OLLAMA_TAGS_URL} returned an error: {}",
                status_detail(code, resp)
            ),
            other => anyhow::anyhow!(
                "Ollama not reachable on {OLLAMA_ADDR} ({other}). Start it with `ollama serve`."
            ),
        })?
        .into_string()
        .context("reading ollama /api/tags response")?;
    require_model(&parse_tag_names(&body)?, model)
}

#[derive(serde::Serialize)]
struct GenerateReq<'a> {
    model: &'a str,
    prompt: &'a str,
    stream: bool,
}
#[derive(Deserialize)]
struct GenerateResp {
    response: String,
}

/// Single-turn generation (`stream:false` → one JSON object). Used by synth, the answerer,
/// and the local judge.
pub fn generate(model: &str, prompt: &str) -> anyhow::Result<String> {
    let resp: GenerateResp = agent()
        .post(OLLAMA_GENERATE_URL)
        .send_json(GenerateReq { model, prompt, stream: false })
        .map_err(|e| match e {
            ureq::Error::Status(code, resp) => {
                anyhow::anyhow!("Ollama generate failed: {}", status_detail(code, resp))
            }
            other => anyhow::anyhow!("Ollama generate failed: {other}"),
        })?
        .into_json()
        .context("parsing ollama /api/generate response")?;
    Ok(resp.response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_model_names_from_tags_body() {
        let body = r#"{"models":[{"name":"qwen2.5:7b"},{"name":"llama3:8b"}]}"#;
        let names = parse_tag_names(body).unwrap();
        assert!(names.contains(&"qwen2.5:7b".to_string()));
        assert!(names.contains(&"llama3:8b".to_string()));
    }

    #[test]
    fn missing_model_yields_clear_error() {
        let names = vec!["llama3:8b".to_string()];
        let err = require_model(&names, "qwen2.5:7b").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("qwen2.5:7b"), "names the missing model: {msg}");
        assert!(msg.contains("ollama pull"), "tells the user the fix: {msg}");
    }

    #[test]
    fn present_model_passes() {
        let names = vec!["qwen2.5:7b".to_string()];
        assert!(require_model(&names, "qwen2.5:7b").is_ok());
    }

    #[test]
    fn malformed_tags_body_is_an_error() {
        assert!(parse_tag_names("not json").is_err());
    }
}
