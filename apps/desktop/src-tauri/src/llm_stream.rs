use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, RwLock};
use tauri::{AppHandle, Emitter, Manager};

use crate::vault::secret_get_cached;

const OPENAI_COMPAT_KEY_NAME: &str = "openai_compat_api_key";
const OPENAI_KEY_NAME: &str = "openai_api_key";
const GOOGLE_KEY_NAME: &str = "google_api_key";
const ANTHROPIC_KEY_NAME: &str = "anthropic_api_key";
const DEFAULT_BASE_URL: &str = "https://api.openai.com";
const DEFAULT_MODEL: &str = "gpt-5-mini";
const BALANCED_MODEL: &str = "gpt-5-mini";
const GEMINI_BALANCED_MODEL: &str = "gemini-2.5-flash";
const CLAUDE_BALANCED_MODEL: &str = "claude-sonnet-4-6";
const DEFAULT_GOOGLE_MODEL: &str = "gemini-2.5-flash";
const DEFAULT_ANTHROPIC_MODEL: &str = "claude-sonnet-4-6";

static ACTIVE_STREAMS: LazyLock<RwLock<HashMap<String, Arc<AtomicBool>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentRecord {
    id: String,
    #[serde(default)]
    purpose: String,
    provider: Option<String>,
    archived: Option<bool>,
    #[serde(default, alias = "openaiCompatModelOverride")]
    model_id: Option<String>,
    openai_compat_base_url_override: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // fields deserialized from settings JSON; Phase 2 will consume them
struct SettingsRecord {
    #[serde(default = "default_base_url", alias = "openaiCompatBaseUrl")]
    openai_compat_base_url: String,
    #[serde(default = "default_model", alias = "openaiCompatModel")]
    openai_compat_model: String,
    #[serde(default = "default_model_mode", alias = "openaiCompatModelMode")]
    openai_compat_model_mode: String,
    #[serde(default = "default_model_tier", alias = "openaiCompatTier")]
    openai_compat_tier: String,
    #[serde(default, alias = "openaiCompatModelId")]
    openai_compat_model_id: String,
    #[serde(default = "default_anthropic_model", alias = "anthropicModel")]
    anthropic_model: String,
    #[serde(default = "default_model_mode", alias = "anthropicModelMode")]
    anthropic_model_mode: String,
    #[serde(default = "default_model_tier", alias = "anthropicTier")]
    anthropic_tier: String,
    #[serde(default, alias = "anthropicModelId")]
    anthropic_model_id: String,
    #[serde(default = "default_google_model", alias = "googleModel")]
    google_model: String,
    #[serde(default = "default_model_mode", alias = "googleModelMode")]
    google_model_mode: String,
    #[serde(default = "default_model_tier", alias = "googleTier")]
    google_tier: String,
    #[serde(default, alias = "googleModelId")]
    google_model_id: String,
}

#[derive(Debug, Serialize, Clone)]
struct UsagePayload {
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    total_tokens: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)] // Phase 2: non-streaming path not yet wired to Tauri command
pub struct NonStreamingGenerateResponse {
    text: String,
    model: String,
    usage: Option<UsagePayload>,
}

fn default_base_url() -> String {
    DEFAULT_BASE_URL.to_string()
}

fn default_model() -> String {
    DEFAULT_MODEL.to_string()
}

fn default_google_model() -> String {
    DEFAULT_GOOGLE_MODEL.to_string()
}

fn default_anthropic_model() -> String {
    DEFAULT_ANTHROPIC_MODEL.to_string()
}

fn default_model_mode() -> String {
    "tier".to_string()
}

fn default_model_tier() -> String {
    "balanced".to_string()
}

fn default_settings() -> SettingsRecord {
    SettingsRecord {
        openai_compat_base_url: default_base_url(),
        openai_compat_model: default_model(),
        openai_compat_model_mode: default_model_mode(),
        openai_compat_tier: default_model_tier(),
        openai_compat_model_id: default_model(),
        anthropic_model: default_anthropic_model(),
        anthropic_model_mode: default_model_mode(),
        anthropic_tier: default_model_tier(),
        anthropic_model_id: default_anthropic_model(),
        google_model: default_google_model(),
        google_model_mode: default_model_mode(),
        google_tier: default_model_tier(),
        google_model_id: default_google_model(),
    }
}

fn app_data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map_err(|_| "Unable to access app data directory".to_string())
}

fn normalize_https_base(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Provider base URL is empty.".to_string());
    }

    let mut parsed = Url::parse(trimmed).map_err(|_| "Provider base URL is invalid.".to_string())?;
    if parsed.scheme() != "https" {
        return Err("Provider base URL must use HTTPS.".to_string());
    }

    parsed.set_fragment(None);
    parsed.set_query(None);

    let path = parsed.path().trim_end_matches('/').to_string();
    parsed
        .set_path(if path.is_empty() { "/" } else { &path });

    Ok(parsed.to_string().trim_end_matches('/').to_string())
}

fn chat_completions_url(base: &str) -> Result<Url, String> {
    let mut normalized = normalize_https_base(base)?;
    normalized.push('/');

    let parsed = Url::parse(&normalized).map_err(|_| "Provider base URL is invalid.".to_string())?;
    parsed
        .join("v1/chat/completions")
        .map_err(|_| "Unable to build chat completions URL.".to_string())
}

fn models_url(base: &str) -> Result<Url, String> {
    let mut normalized = normalize_https_base(base)?;
    normalized.push('/');

    let parsed = Url::parse(&normalized).map_err(|_| "Provider base URL is invalid.".to_string())?;
    parsed
        .join("v1/models")
        .map_err(|_| "Unable to build models URL.".to_string())
}

fn provider_api_key_with_name(key_name: &str, label: &str) -> Result<String, String> {
    match secret_get_cached(key_name)? {
        Some(value) => {
            if value.trim().is_empty() {
                Err(format!("Missing {} API key in Vault.", label))
            } else {
                Ok(value)
            }
        }
        None => Err(format!("Missing {} API key in Vault.", label)),
    }
}

fn provider_api_key() -> Result<String, String> {
    provider_api_key_with_name(OPENAI_COMPAT_KEY_NAME, "openai-compatible")
}

fn read_settings(app: &AppHandle) -> SettingsRecord {
    let path = match app_data_dir(app) {
        Ok(dir) => dir.join("settings.json"),
        Err(_) => return default_settings(),
    };

    let raw = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(_) => return default_settings(),
    };

    serde_json::from_str::<SettingsRecord>(&raw).unwrap_or_else(|_| default_settings())
}

fn read_agent(app: &AppHandle, agent_id: &str) -> Option<AgentRecord> {
    let path = app_data_dir(app).ok()?.join("agents.json");
    let raw = fs::read_to_string(path).ok()?;
    let values = serde_json::from_str::<Vec<Value>>(&raw).ok()?;
    let mut latest_version: i64 = -1;
    let mut latest_record: Option<AgentRecord> = None;

    for value in values {
        if let Some(data) = value.get("data") {
            let Some(object_id) = value.get("id").and_then(Value::as_str) else {
                continue;
            };
            if object_id != agent_id {
                continue;
            }

            let version = value.get("version").and_then(Value::as_i64).unwrap_or(0);
            let Ok(mut record) = serde_json::from_value::<AgentRecord>(data.clone()) else {
                continue;
            };
            if record.archived.unwrap_or(false) {
                continue;
            }

            if record.id.trim().is_empty() {
                record.id = object_id.to_string();
            }

            if version > latest_version {
                latest_version = version;
                latest_record = Some(record);
            }
            continue;
        }

        let Ok(record) = serde_json::from_value::<AgentRecord>(value) else {
            continue;
        };
        if record.id == agent_id && !record.archived.unwrap_or(false) {
            return Some(record);
        }
    }

    latest_record
}

fn effective_provider_base(app: &AppHandle, agent_id: &str) -> Result<String, String> {
    let settings = read_settings(app);
    let agent = read_agent(app, agent_id);

    let candidate = agent
        .as_ref()
        .and_then(|record| record.openai_compat_base_url_override.clone())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(settings.openai_compat_base_url);

    normalize_https_base(&candidate)
}

fn balanced_model_for_tier(tier: &str) -> &'static str {
    match tier {
        "fast" => "gpt-5-nano",
        "advanced" => "gpt-5.2",
        _ => BALANCED_MODEL,
    }
}

fn resolve_settings_model(settings: &SettingsRecord) -> String {
    let mode = settings.openai_compat_model_mode.trim();
    if mode == "tier" {
        return balanced_model_for_tier(settings.openai_compat_tier.trim()).to_string();
    }

    let explicit = settings.openai_compat_model_id.trim();
    if !explicit.is_empty() {
        return explicit.to_string();
    }

    let legacy = settings.openai_compat_model.trim();
    if !legacy.is_empty() {
        return legacy.to_string();
    }

    BALANCED_MODEL.to_string()
}

#[allow(dead_code)] // Phase 2: Google provider path not yet wired
fn resolve_google_settings_model(settings: &SettingsRecord) -> String {
    if settings.google_model_mode.trim() == "tier" {
        return match settings.google_tier.trim() {
            "fast" => "gemini-2.5-flash-lite".to_string(),
            "advanced" => "gemini-2.5-pro".to_string(),
            _ => DEFAULT_GOOGLE_MODEL.to_string(),
        };
    }

    let explicit = settings.google_model_id.trim();
    if !explicit.is_empty() {
        return explicit.to_string();
    }

    let legacy = settings.google_model.trim();
    if !legacy.is_empty() {
        return legacy.to_string();
    }

    DEFAULT_GOOGLE_MODEL.to_string()
}

#[allow(dead_code)] // Phase 2: Anthropic provider path not yet wired
fn resolve_anthropic_settings_model(settings: &SettingsRecord) -> String {
    if settings.anthropic_model_mode.trim() == "tier" {
        return match settings.anthropic_tier.trim() {
            "fast" => "claude-haiku-4-5".to_string(),
            "advanced" => "claude-opus-4-6".to_string(),
            _ => DEFAULT_ANTHROPIC_MODEL.to_string(),
        };
    }

    let explicit = settings.anthropic_model_id.trim();
    if !explicit.is_empty() {
        return explicit.to_string();
    }

    let legacy = settings.anthropic_model.trim();
    if !legacy.is_empty() {
        return legacy.to_string();
    }

    DEFAULT_ANTHROPIC_MODEL.to_string()
}

fn normalize_model_override(model_id: Option<String>) -> Option<String> {
    model_id.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

#[allow(dead_code)] // Phase 2: Gemini model normalization not yet called
fn normalize_gemini_model(model: &str) -> String {
    let trimmed = model.trim();
    trimmed
        .strip_prefix("models/")
        .unwrap_or(trimmed)
        .to_string()
}

fn effective_provider_model(app: &AppHandle, agent_id: &str) -> String {
    let settings = read_settings(app);
    let agent = read_agent(app, agent_id);

    let model = agent
        .as_ref()
        .and_then(|record| normalize_model_override(record.model_id.clone()))
        .unwrap_or_else(|| resolve_settings_model(&settings));

    if model.trim().is_empty() {
        BALANCED_MODEL.to_string()
    } else {
        model
    }
}

fn extract_provider_error_message(body: &str) -> String {
    if let Ok(value) = serde_json::from_str::<Value>(body) {
        if let Some(message) = value
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
        {
            return message.to_string();
        }
        if let Some(message) = value.get("message").and_then(Value::as_str) {
            return message.to_string();
        }
    }

    let snippet = body
        .chars()
        .take(240)
        .collect::<String>()
        .replace('\n', " ");
    if snippet.trim().is_empty() {
        "Provider request failed.".to_string()
    } else {
        snippet
    }
}

fn is_model_access_error(status: u16, message: &str) -> bool {
    if status != 400 && status != 401 && status != 403 && status != 404 && status != 422 {
        return false;
    }

    let normalized = message.to_lowercase();
    if !normalized.contains("model") {
        return false;
    }

    normalized.contains("not found")
        || normalized.contains("does not exist")
        || normalized.contains("not available")
        || normalized.contains("not permitted")
        || normalized.contains("permission")
        || normalized.contains("unsupported")
        || normalized.contains("no access")
}

fn system_prompt_for_agent(app: &AppHandle, agent_id: &str) -> String {
    let default_prompt = "You are BossClaw assistant. Be concise and practical.".to_string();
    let agent = match read_agent(app, agent_id) {
        Some(value) => value,
        None => return default_prompt,
    };

    if let Some(provider) = agent.provider {
        if provider != "openai_compat" {
            return default_prompt;
        }
    }

    let purpose = agent.purpose.trim();
    if purpose.is_empty() {
        return default_prompt;
    }

    format!(
        "You are the BossClaw desktop assistant for this agent. Agent purpose: {}",
        purpose
    )
}

fn extract_delta_text(value: &Value) -> Option<String> {
    let delta = value
        .get("choices")
        .and_then(|choices| choices.get(0))
        .and_then(|choice| choice.get("delta"))
        .and_then(|delta| delta.get("content"));

    match delta {
        Some(Value::String(text)) => {
            if text.is_empty() {
                None
            } else {
                Some(text.to_string())
            }
        }
        Some(Value::Array(items)) => {
            let mut joined = String::new();
            for item in items {
                if let Some(text) = item.get("text").and_then(Value::as_str) {
                    joined.push_str(text);
                }
            }
            if joined.is_empty() { None } else { Some(joined) }
        }
        _ => None,
    }
}

fn extract_usage(value: &Value) -> Option<UsagePayload> {
    let usage = value.get("usage")?;

    Some(UsagePayload {
        prompt_tokens: usage.get("prompt_tokens").and_then(Value::as_u64),
        completion_tokens: usage.get("completion_tokens").and_then(Value::as_u64),
        total_tokens: usage.get("total_tokens").and_then(Value::as_u64),
    })
}

fn extract_message_text(value: &Value) -> Option<String> {
    let content = value
        .get("choices")
        .and_then(|choices| choices.get(0))
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))?;

    match content {
        Value::String(text) => {
            if text.trim().is_empty() {
                None
            } else {
                Some(text.to_string())
            }
        }
        Value::Array(parts) => {
            let mut joined = String::new();
            for part in parts {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    joined.push_str(text);
                } else if let Some(text) = part.get("content").and_then(Value::as_str) {
                    joined.push_str(text);
                }
            }

            if joined.trim().is_empty() {
                None
            } else {
                Some(joined)
            }
        }
        _ => None,
    }
}

fn extract_gemini_text(value: &Value) -> Option<String> {
    let parts = value
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|candidates| candidates.first())
        .and_then(|candidate| candidate.get("content"))
        .and_then(|content| content.get("parts"))
        .and_then(Value::as_array)?;

    let mut text = String::new();
    for part in parts {
        if let Some(fragment) = part.get("text").and_then(Value::as_str) {
            text.push_str(fragment);
        }
    }

    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

fn extract_claude_text(value: &Value) -> Option<String> {
    let parts = value.get("content").and_then(Value::as_array)?;
    let mut text = String::new();
    for part in parts {
        if let Some(fragment) = part.get("text").and_then(Value::as_str) {
            text.push_str(fragment);
        }
    }

    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

async fn request_non_stream_completion(
    client: &Client,
    endpoint: &Url,
    api_key: &str,
    body: &Value,
) -> Result<Value, (u16, String)> {
    let response = client
        .post(endpoint.clone())
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {}", api_key))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(reqwest::header::ACCEPT, "application/json")
        .json(body)
        .send()
        .await
        .map_err(|_| (0, "Unable to reach provider endpoint.".to_string()))?;

    let status = response.status();
    let status_code = status.as_u16();
    let response_text = response.text().await.unwrap_or_default();

    if !status.is_success() {
        return Err((status_code, extract_provider_error_message(&response_text)));
    }

    serde_json::from_str::<Value>(&response_text)
        .map_err(|_| (status_code, "Provider returned invalid JSON.".to_string()))
}

async fn request_planner_completion(
    client: &Client,
    endpoint: &Url,
    api_key: &str,
    model: &str,
    planner_system_prompt: &str,
    planner_user_prompt: &str,
) -> Result<Value, (u16, String)> {
    let mut body = json!({
        "model": model,
        "stream": false,
        "response_format": { "type": "json_object" },
        "messages": [
            { "role": "system", "content": planner_system_prompt },
            { "role": "user", "content": planner_user_prompt }
        ]
    });

    match request_non_stream_completion(client, endpoint, api_key, &body).await {
        Ok(value) => Ok(value),
        Err((status, _message)) if status == 400 || status == 404 || status == 422 => {
            if let Some(object) = body.as_object_mut() {
                object.remove("response_format");
            }
            request_non_stream_completion(client, endpoint, api_key, &body).await
        }
        Err(error) => Err(error),
    }
}

fn register_stream(run_id: &str) -> Result<Arc<AtomicBool>, String> {
    let mut streams = ACTIVE_STREAMS
        .write()
        .map_err(|_| "Unable to acquire stream state lock.".to_string())?;

    if streams.contains_key(run_id) {
        return Err("A stream is already running for this run ID.".to_string());
    }

    let cancel_flag = Arc::new(AtomicBool::new(false));
    streams.insert(run_id.to_string(), cancel_flag.clone());
    Ok(cancel_flag)
}

fn remove_stream(run_id: &str) {
    if let Ok(mut streams) = ACTIVE_STREAMS.write() {
        streams.remove(run_id);
    }
}

fn emit_chunk(app: &AppHandle, run_id: &str, delta: &str) {
    let _ = app.emit(
        "llm_stream_chunk",
        json!({
            "runId": run_id,
            "delta": delta
        }),
    );
}

fn emit_done(
    app: &AppHandle,
    run_id: &str,
    cancelled: bool,
    usage: Option<UsagePayload>,
    model: &str,
) {
    let _ = app.emit(
        "llm_stream_done",
        json!({
            "runId": run_id,
            "cancelled": cancelled,
            "usage": usage,
            "model": model
        }),
    );
}

fn emit_error(app: &AppHandle, run_id: &str, message: &str) {
    let _ = app.emit(
        "llm_stream_error",
        json!({
            "runId": run_id,
            "message": message
        }),
    );
}

fn emit_notice(app: &AppHandle, run_id: &str, message: &str, detail: &str) {
    let _ = app.emit(
        "llm_stream_notice",
        json!({
            "runId": run_id,
            "message": message,
            "detail": detail
        }),
    );
}

async fn send_stream_request(
    client: &Client,
    endpoint: &Url,
    api_key: &str,
    model: &str,
    system_prompt: &str,
    prompt: &str,
) -> Result<reqwest::Response, String> {
    let body = json!({
        "model": model,
        "stream": true,
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user", "content": prompt }
        ]
    });

    client
        .post(endpoint.clone())
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {}", api_key))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(reqwest::header::ACCEPT, "text/event-stream")
        .json(&body)
        .send()
        .await
        .map_err(|_| "Unable to reach provider endpoint.".to_string())
}

async fn stream_chat_completion(
    app: AppHandle,
    run_id: String,
    agent_id: String,
    prompt: String,
    cancel_flag: Arc<AtomicBool>,
) {
    let mut usage: Option<UsagePayload> = None;
    let requested_model = effective_provider_model(&app, &agent_id);
    let mut active_model = requested_model.clone();

    let execution = async {
        let base_url = effective_provider_base(&app, &agent_id)?;
        let endpoint = chat_completions_url(&base_url)?;
        let api_key = provider_api_key()?;
        let system_prompt = system_prompt_for_agent(&app, &agent_id);

        let client = Client::builder()
            .build()
            .map_err(|_| "Unable to initialize provider HTTP client.".to_string())?;

        let mut response = send_stream_request(
            &client,
            &endpoint,
            &api_key,
            &active_model,
            &system_prompt,
            &prompt,
        )
        .await?;

        let status = response.status();
        if !status.is_success() {
            let status_code = status.as_u16();
            let error_body = response.text().await.unwrap_or_default();
            let error_message = extract_provider_error_message(&error_body);
            if is_model_access_error(status_code, &error_message) && active_model != BALANCED_MODEL {
                let fallback = BALANCED_MODEL.to_string();
                active_model = fallback.clone();
                emit_notice(
                    &app,
                    &run_id,
                    "That model isn't available on your account. Switching to Balanced.",
                    &format!(
                        "Model fallback applied for run {}: {} -> {}",
                        run_id, requested_model, fallback
                    ),
                );
                response = send_stream_request(
                    &client,
                    &endpoint,
                    &api_key,
                    &active_model,
                    &system_prompt,
                    &prompt,
                )
                .await?;
                if !response.status().is_success() {
                    return Err(format!(
                        "Provider request failed with HTTP {}.",
                        response.status().as_u16()
                    ));
                }
            } else {
                return Err(format!("Provider request failed with HTTP {}.", status_code));
            }
        }

        let mut buffer = String::new();
        let mut done_received = false;

        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "Provider stream interrupted while reading response.".to_string())?
        {
            if cancel_flag.load(Ordering::Relaxed) {
                return Ok(true);
            }

            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(newline_index) = buffer.find('\n') {
                let line = buffer[..newline_index]
                    .trim_end_matches('\r')
                    .to_string();
                buffer = buffer[(newline_index + 1)..].to_string();

                if cancel_flag.load(Ordering::Relaxed) {
                    return Ok(true);
                }

                if !line.starts_with("data:") {
                    continue;
                }

                let payload = line[5..].trim();
                if payload.is_empty() {
                    continue;
                }

                if payload == "[DONE]" {
                    done_received = true;
                    break;
                }

                let parsed = serde_json::from_str::<Value>(payload)
                    .map_err(|_| "Provider stream chunk was not valid JSON.".to_string())?;

                if let Some(delta) = extract_delta_text(&parsed) {
                    emit_chunk(&app, &run_id, &delta);
                }

                if let Some(extracted_usage) = extract_usage(&parsed) {
                    usage = Some(extracted_usage);
                }
            }

            if done_received {
                break;
            }
        }

        Ok(false)
    }
    .await;

    match execution {
        Ok(cancelled) => emit_done(&app, &run_id, cancelled, usage, &active_model),
        Err(message) => emit_error(&app, &run_id, &message),
    }

    remove_stream(&run_id);
}

#[tauri::command]
pub async fn llm_stream_start(
    app: AppHandle,
    run_id: String,
    agent_id: String,
    prompt: String,
) -> Result<(), String> {
    if run_id.trim().is_empty() {
        return Err("runId is required.".to_string());
    }

    if agent_id.trim().is_empty() {
        return Err("agentId is required.".to_string());
    }

    if prompt.trim().is_empty() {
        return Err("Prompt cannot be empty.".to_string());
    }

    let cancel_flag = register_stream(&run_id)?;
    let app_handle = app.clone();
    let run_id_clone = run_id.clone();
    let agent_id_clone = agent_id.clone();
    let prompt_clone = prompt.clone();

    tauri::async_runtime::spawn(async move {
        stream_chat_completion(
            app_handle,
            run_id_clone,
            agent_id_clone,
            prompt_clone,
            cancel_flag,
        )
        .await;
    });

    Ok(())
}

#[tauri::command]
pub async fn llm_plan(
    app: AppHandle,
    agent_id: String,
    user_message: String,
    context_summary: String,
) -> Result<String, String> {
    if agent_id.trim().is_empty() {
        return Err("agentId is required.".to_string());
    }

    if user_message.trim().is_empty() {
        return Err("userMessage is required.".to_string());
    }

    let base_url = effective_provider_base(&app, &agent_id)?;
    let endpoint = chat_completions_url(&base_url)?;
    let api_key = provider_api_key()?;
    let model = effective_provider_model(&app, &agent_id);
    let trimmed_user_message = user_message.trim();
    let lowered_message = trimmed_user_message.to_lowercase();
    let strong_mission_signal = lowered_message.starts_with("[mission]");
    let recurring_intent = strong_mission_signal
        || lowered_message.contains("every")
        || lowered_message.contains("daily")
        || lowered_message.contains("weekly")
        || lowered_message.contains("each minute");

    let planner_system_prompt = r#"You are the BossClaw planning engine running inside BossClaw Desktop.
BossClaw Desktop has a built-in mission scheduler and can create recurring missions directly.
Output exactly one JSON object that validates against schema id "bossclaw.plan.v1".
Rules:
- Do not include markdown, code fences, or extra commentary.
- Use "schema": "bossclaw.plan.v1".
- Use "mode": "autopilot" or "fsd".
- Include "steps" with concise titles and tool ids.
- Prefer tool "llm.generate" for text-generation steps.
- Use tool "web.extract" for website content retrieval when needed.
- For "web.extract", provide a URL in step input (JSON or plain URL).
- Never suggest OS cron, launchd, shell scripts, or bash timers for recurring automation.
- If recurring intent is detected, include top-level "missionProposal" with this format:
  {
    "type": "create_mission",
    "title": string,
    "goal": string,
    "schedule": {
      "kind": "daily" | "weekdays" | "every_minutes" | "weekly",
      "time"?: "HH:MM",
      "intervalMinutes"?: number,
      "weekday"?: 0-6
    },
    "autonomy": "autopilot" | "fsd"
  }
- Treat "[Mission]" prefix as a strong recurring intent signal and always return "missionProposal".
- If needed, include top-level "configProposal".
- Keep all fields deterministic and safe for execution planning."#;

    let planner_user_prompt = format!(
        "USER_MESSAGE:\n{}\n\nRECURRING_INTENT:\n{}\n\nMISSION_PREFIX_SIGNAL:\n{}\n\nCONTEXT_SUMMARY:\n{}\n\nReturn only valid JSON.",
        trimmed_user_message,
        if recurring_intent { "true" } else { "false" },
        if strong_mission_signal { "strong" } else { "none" },
        context_summary.trim()
    );

    let client = Client::builder()
        .build()
        .map_err(|_| "Unable to initialize provider HTTP client.".to_string())?;

    let planner_response = match request_planner_completion(
        &client,
        &endpoint,
        &api_key,
        &model,
        planner_system_prompt,
        &planner_user_prompt,
    )
    .await
    {
        Ok(value) => value,
        Err((status, message))
            if is_model_access_error(status, &message) && model != BALANCED_MODEL =>
        {
            request_planner_completion(
                &client,
                &endpoint,
                &api_key,
                BALANCED_MODEL,
                planner_system_prompt,
                &planner_user_prompt,
            )
            .await
            .map_err(|(retry_status, _)| {
                if retry_status > 0 {
                    format!("Planner request failed with HTTP {}.", retry_status)
                } else {
                    "Planner request failed.".to_string()
                }
            })?
        }
        Err((status, _)) => {
            if status > 0 {
                return Err(format!("Planner request failed with HTTP {}.", status));
            }
            return Err("Planner request failed.".to_string());
        }
    };

    extract_message_text(&planner_response)
        .ok_or_else(|| "Planner response did not contain message content.".to_string())
}

#[tauri::command]
pub async fn gemini_generate(prompt: String, model: String) -> Result<String, String> {
    let trimmed_prompt = prompt.trim();
    if trimmed_prompt.is_empty() {
        return Err("Prompt is required.".to_string());
    }

    let selected_model = if model.trim().is_empty() {
        GEMINI_BALANCED_MODEL.to_string()
    } else {
        model.trim().to_string()
    };

    let api_key = provider_api_key_with_name(GOOGLE_KEY_NAME, "Google")?;
    let endpoint = Url::parse(&format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
        selected_model, api_key
    ))
    .map_err(|_| "Unable to build Gemini endpoint URL.".to_string())?;

    let client = Client::builder()
        .build()
        .map_err(|_| "Unable to initialize provider HTTP client.".to_string())?;

    let body = json!({
        "contents": [
            { "role": "user", "parts": [{ "text": trimmed_prompt }] }
        ]
    });

    let response = client
        .post(endpoint)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(reqwest::header::ACCEPT, "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|_| "Unable to reach provider endpoint.".to_string())?;

    let status = response.status();
    let status_code = status.as_u16();
    let response_text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        let message = extract_provider_error_message(&response_text);
        return Err(if status_code > 0 {
            format!("Provider request failed with HTTP {}: {}", status_code, message)
        } else {
            "Provider request failed.".to_string()
        });
    }

    let payload = serde_json::from_str::<Value>(&response_text)
        .map_err(|_| "Provider returned invalid JSON.".to_string())?;
    extract_gemini_text(&payload)
        .ok_or_else(|| "Provider response did not contain message content.".to_string())
}

#[tauri::command]
pub async fn claude_generate(prompt: String, model: String) -> Result<String, String> {
    let trimmed_prompt = prompt.trim();
    if trimmed_prompt.is_empty() {
        return Err("Prompt is required.".to_string());
    }

    let selected_model = if model.trim().is_empty() {
        CLAUDE_BALANCED_MODEL.to_string()
    } else {
        model.trim().to_string()
    };

    let api_key = provider_api_key_with_name(ANTHROPIC_KEY_NAME, "Anthropic")?;
    let endpoint = Url::parse("https://api.anthropic.com/v1/messages")
        .map_err(|_| "Unable to build Claude endpoint URL.".to_string())?;

    let client = Client::builder()
        .build()
        .map_err(|_| "Unable to initialize provider HTTP client.".to_string())?;

    let body = json!({
        "model": selected_model,
        "max_tokens": 512,
        "messages": [
            { "role": "user", "content": trimmed_prompt }
        ]
    });

    let response = client
        .post(endpoint)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(reqwest::header::ACCEPT, "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|_| "Unable to reach provider endpoint.".to_string())?;

    let status = response.status();
    let status_code = status.as_u16();
    let response_text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        let message = extract_provider_error_message(&response_text);
        return Err(if status_code > 0 {
            format!("Provider request failed with HTTP {}: {}", status_code, message)
        } else {
            "Provider request failed.".to_string()
        });
    }

    let payload = serde_json::from_str::<Value>(&response_text)
        .map_err(|_| "Provider returned invalid JSON.".to_string())?;
    extract_claude_text(&payload)
        .ok_or_else(|| "Provider response did not contain message content.".to_string())
}

async fn list_models_for_provider(
    provider: &str,
    base_url: &str,
) -> Result<Vec<String>, String> {
    let key_name = match provider {
        "openai_compat" => OPENAI_COMPAT_KEY_NAME,
        "openai" => OPENAI_KEY_NAME,
        _ => return Err("Unsupported provider for model listing.".to_string()),
    };

    let label = match provider {
        "openai_compat" => "openai-compatible",
        "openai" => "openai",
        _ => "provider",
    };

    let endpoint = models_url(base_url)?;
    let api_key = provider_api_key_with_name(key_name, label)?;
    let client = Client::builder()
        .build()
        .map_err(|_| "Unable to initialize provider HTTP client.".to_string())?;

    let response = client
        .get(endpoint)
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {}", api_key))
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|_| "Unable to reach provider endpoint.".to_string())?;

    if !response.status().is_success() {
        return Err(format!(
            "Provider model list request failed with HTTP {}.",
            response.status().as_u16()
        ));
    }

    let payload = response
        .json::<Value>()
        .await
        .map_err(|_| "Provider model list response was invalid.".to_string())?;

    let mut models = payload
        .get("data")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("id").and_then(Value::as_str))
                .map(|id| id.trim().to_string())
                .filter(|id| !id.is_empty())
                .filter(|id| id.contains("gpt") || id.starts_with('o'))
                .collect::<Vec<String>>()
        })
        .unwrap_or_default();

    models.sort_by(|left, right| right.cmp(left));
    models.dedup();
    Ok(models)
}

#[tauri::command]
pub async fn llm_list_models(provider: String, base_url: String) -> Result<Vec<String>, String> {
    if provider.trim().is_empty() {
        return Err("Provider is required.".to_string());
    }

    let normalized_provider = provider.trim().to_lowercase();
    let normalized_base = normalize_https_base(&base_url)?;
    list_models_for_provider(&normalized_provider, &normalized_base).await
}

#[tauri::command]
pub async fn llm_openai_compat_list_models(app: AppHandle) -> Result<Vec<String>, String> {
    let settings = read_settings(&app);
    let base_url = normalize_https_base(&settings.openai_compat_base_url)?;
    list_models_for_provider("openai_compat", &base_url).await
}

#[tauri::command]
pub fn llm_stream_cancel(run_id: String) -> Result<(), String> {
    let streams = ACTIVE_STREAMS
        .read()
        .map_err(|_| "Unable to acquire stream state lock.".to_string())?;

    if let Some(cancel_flag) = streams.get(&run_id) {
        cancel_flag.store(true, Ordering::Relaxed);
        Ok(())
    } else {
        Err("No active stream found for runId.".to_string())
    }
}
