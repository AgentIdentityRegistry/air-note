//! Loopback HTTP client for Ollama (127.0.0.1:11434) via `ureq` v2 (mirrors bossclaw-core's
//! pin). Two uses: the availability preflight (`/api/tags`) and single-turn generation
//! (`/api/generate`). Default model pinned by Probe B.

/// Probe-B-pinned default local model (the evolve loop's tier is `qwen2.5:7b-instruct`;
/// match whichever tag is actually installed).
pub const DEFAULT_OLLAMA_MODEL: &str = "qwen2.5:7b-instruct";

use serde::Deserialize;

const OLLAMA_TAGS_URL: &str = "http://127.0.0.1:11434/api/tags";
const OLLAMA_GENERATE_URL: &str = "http://127.0.0.1:11434/api/generate";

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
    let body = ureq::get(OLLAMA_TAGS_URL)
        .call()
        .map_err(|e| anyhow::anyhow!(
            "Ollama not reachable on 127.0.0.1:11434 ({e}). Start it with `ollama serve`."
        ))?
        .into_string()?;
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
    let resp: GenerateResp = ureq::post(OLLAMA_GENERATE_URL)
        .send_json(GenerateReq { model, prompt, stream: false })
        .map_err(|e| anyhow::anyhow!("Ollama generate failed: {e}"))?
        .into_json()?;
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
}
