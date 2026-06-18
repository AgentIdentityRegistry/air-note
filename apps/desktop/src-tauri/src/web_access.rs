use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, COOKIE, USER_AGENT};
use reqwest::{Client, Url};
use serde::Serialize;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tauri::{AppHandle, Manager};

use crate::vault::{secret_delete_cached, secret_get_cached, secret_set_cached};

const HELPER_DIR_NAME: &str = "playwright-helper";
const PW_FETCH_SCRIPT_NAME: &str = "fetch_rendered.mjs";
const WEB_USER_AGENT: &str = "AIRAgentDesktop/1.0 (+web.extract)";
const MAX_WEB_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebFetchResponse {
    pub final_url: String,
    pub status: u16,
    pub content_type: Option<String>,
    pub html: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PwDetectResponse {
    pub node_found: bool,
    pub node_version: Option<String>,
    pub helper_installed: bool,
    pub helper_path: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PwInstallResponse {
    pub logs: String,
    pub helper_path: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PwFetchRenderedResponse {
    pub html: String,
}

fn app_data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map_err(|_| "Unable to access app data directory".to_string())
}

fn helper_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app_data_dir(app)?.join(HELPER_DIR_NAME))
}

fn normalize_host(host: &str) -> Result<String, String> {
    let trimmed = host.trim().to_lowercase();
    if trimmed.is_empty() {
        return Err("Host is required.".to_string());
    }

    let normalized = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        Url::parse(&trimmed)
            .map_err(|_| "Host is invalid.".to_string())?
            .host_str()
            .ok_or_else(|| "Host is invalid.".to_string())?
            .to_string()
    } else {
        trimmed
    };

    if !normalized
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | ':'))
    {
        return Err("Host contains invalid characters.".to_string());
    }

    Ok(normalized)
}

fn parse_http_url(url: &str) -> Result<Url, String> {
    let parsed = Url::parse(url.trim()).map_err(|_| "URL is invalid.".to_string())?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err("Only http and https URLs are supported.".to_string());
    }
    Ok(parsed)
}

fn host_from_url(url: &Url) -> Result<String, String> {
    let host = url
        .host_str()
        .ok_or_else(|| "URL host is invalid.".to_string())?
        .to_lowercase();
    if let Some(port) = url.port() {
        Ok(format!("{}:{}", host, port))
    } else {
        Ok(host)
    }
}

fn web_auth_cache_key(host: &str) -> Result<String, String> {
    let normalized_host = normalize_host(host)?;
    Ok(format!("web_auth::{}", normalized_host))
}

fn build_http_client() -> Result<Client, String> {
    Client::builder()
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|_| "Unable to initialize HTTP client.".to_string())
}

async fn fetch_response_text(
    response: reqwest::Response,
) -> Result<WebFetchResponse, String> {
    if let Some(length) = response.content_length() {
        if length > MAX_WEB_RESPONSE_BYTES as u64 {
            return Err("Response is too large.".to_string());
        }
    }

    let final_url = response.url().to_string();
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string());

    let html_bytes = response
        .bytes()
        .await
        .map_err(|_| "Unable to read response body.".to_string())?;
    if html_bytes.len() > MAX_WEB_RESPONSE_BYTES {
        return Err("Response is too large.".to_string());
    }
    let html = String::from_utf8_lossy(&html_bytes).to_string();

    Ok(WebFetchResponse {
        final_url,
        status,
        content_type,
        html,
    })
}

#[tauri::command]
pub fn web_auth_set(host: String, value: String) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err("Auth token is empty.".to_string());
    }

    let key = web_auth_cache_key(&host)?;
    secret_set_cached(&key, &value)
}

#[tauri::command]
pub fn web_auth_has(host: String) -> Result<bool, String> {
    let key = web_auth_cache_key(&host)?;
    match secret_get_cached(&key)? {
        Some(value) => Ok(!value.trim().is_empty()),
        None => Ok(false),
    }
}

#[tauri::command]
pub fn web_auth_delete(host: String) -> Result<(), String> {
    let key = web_auth_cache_key(&host)?;
    secret_delete_cached(&key)
}

#[tauri::command]
pub async fn web_fetch_public(url: String) -> Result<WebFetchResponse, String> {
    let parsed_url = parse_http_url(&url)?;
    let client = build_http_client()?;

    let response = client
        .get(parsed_url)
        .header(USER_AGENT, WEB_USER_AGENT)
        .send()
        .await
        .map_err(|_| "Unable to fetch URL.".to_string())?;

    fetch_response_text(response).await
}

#[tauri::command]
pub async fn web_fetch_auth(url: String, host: String) -> Result<WebFetchResponse, String> {
    let parsed_url = parse_http_url(&url)?;
    let parsed_host = host_from_url(&parsed_url)?;
    let normalized_host = normalize_host(&host)?;

    if parsed_host != normalized_host {
        return Err("URL host does not match the provided auth host.".to_string());
    }

    let cache_key = web_auth_cache_key(&normalized_host)?;
    let stored_secret = match secret_get_cached(&cache_key)? {
        Some(value) if !value.trim().is_empty() => value,
        Some(_) | None => {
            return Err("No auth token configured for this host. Set one in Web Access settings.".to_string())
        }
    };

    let client = build_http_client()?;
    let mut request = client
        .get(parsed_url)
        .header(USER_AGENT, WEB_USER_AGENT);

    let normalized_secret = stored_secret.trim();
    if let Some(cookie_value) = normalized_secret.strip_prefix("cookie:") {
        request = request.header(COOKIE, cookie_value.trim());
    } else if let Some(bearer_value) = normalized_secret.strip_prefix("bearer:") {
        request = request.header(AUTHORIZATION, format!("Bearer {}", bearer_value.trim()));
    } else if let Some(basic_value) = normalized_secret.strip_prefix("basic:") {
        request = request.header(AUTHORIZATION, format!("Basic {}", basic_value.trim()));
    } else {
        return Err(
            "Invalid auth token format. Use cookie:, bearer:, or basic: prefix.".to_string(),
        );
    }

    let response = request
        .send()
        .await
        .map_err(|_| "Unable to fetch URL with auth.".to_string())?;

    fetch_response_text(response).await
}

fn run_command(
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
) -> Result<String, String> {
    let mut command = Command::new(program);
    command.args(args);
    if let Some(directory) = cwd {
        command.current_dir(directory);
    }

    let output = command
        .output()
        .map_err(|_| format!("Failed to run {}", program))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if output.status.success() {
        if stderr.is_empty() {
            Ok(stdout)
        } else {
            Ok(format!("{}\n{}", stdout, stderr))
        }
    } else {
        let error_snippet = stderr
            .chars()
            .take(220)
            .collect::<String>()
            .replace('\n', " ");
        Err(if error_snippet.is_empty() {
            format!("{} failed", program)
        } else {
            format!("{} failed: {}", program, error_snippet)
        })
    }
}

fn ensure_helper_script(helper_path: &Path) -> Result<(), String> {
    let script_path = helper_path.join(PW_FETCH_SCRIPT_NAME);
    let script = r#"import { chromium } from 'playwright';

const url = process.argv[2];
if (!url) {
  process.stdout.write(JSON.stringify({ ok: false, error: 'URL is required' }));
  process.exit(1);
}

(async () => {
  try {
    const browser = await chromium.launch({ headless: true });
    const page = await browser.newPage({ userAgent: 'AIRAgentDesktop/1.0 (+web.extract interactive)' });
    await page.goto(url, { waitUntil: 'networkidle', timeout: 45000 });
    const html = await page.content();
    await browser.close();
    process.stdout.write(JSON.stringify({ ok: true, html }));
  } catch (error) {
    process.stdout.write(JSON.stringify({ ok: false, error: error instanceof Error ? error.message : 'Unknown browser error' }));
    process.exit(1);
  }
})();
"#;

    fs::write(script_path, script).map_err(|_| "Unable to write Playwright helper script.".to_string())
}

fn detect_node_version() -> Option<String> {
    let output = Command::new("node").arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }

    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[tauri::command]
pub fn pw_detect(app: AppHandle) -> Result<PwDetectResponse, String> {
    let helper_path = helper_dir(&app)?;
    let node_version = detect_node_version();
    let helper_installed = helper_path.join("node_modules/playwright").exists()
        && helper_path.join(PW_FETCH_SCRIPT_NAME).exists();

    Ok(PwDetectResponse {
        node_found: node_version.is_some(),
        node_version,
        helper_installed,
        helper_path: Some(helper_path.to_string_lossy().to_string()),
    })
}

#[tauri::command]
pub fn pw_install(app: AppHandle) -> Result<PwInstallResponse, String> {
    let helper_path = helper_dir(&app)?;
    fs::create_dir_all(&helper_path)
        .map_err(|_| "Unable to create Playwright helper directory.".to_string())?;

    let mut logs: Vec<String> = Vec::new();

    let node_version = detect_node_version();
    if node_version.is_none() {
        return Err("Node.js is required for Browser Mode helper installation.".to_string());
    }

    logs.push(format!("node {}", node_version.unwrap_or_default()));

    if !helper_path.join("package.json").exists() {
        logs.push("npm init -y".to_string());
        logs.push(run_command("npm", &["init", "-y"], Some(&helper_path))?);
    }

    logs.push("npm install playwright".to_string());
    logs.push(run_command("npm", &["install", "playwright"], Some(&helper_path))?);

    logs.push("npx playwright install chromium".to_string());
    logs.push(run_command(
        "npx",
        &["playwright", "install", "chromium"],
        Some(&helper_path),
    )?);

    ensure_helper_script(&helper_path)?;

    Ok(PwInstallResponse {
        logs: logs.join("\n"),
        helper_path: helper_path.to_string_lossy().to_string(),
    })
}

#[tauri::command]
pub fn pw_fetch_rendered(app: AppHandle, url: String) -> Result<PwFetchRenderedResponse, String> {
    let parsed_url = parse_http_url(&url)?;
    let helper_path = helper_dir(&app)?;
    let script_path = helper_path.join(PW_FETCH_SCRIPT_NAME);

    if !helper_path.join("node_modules/playwright").exists() || !script_path.exists() {
        return Err("Browser Mode helper is not installed. Install it in Web Access settings.".to_string());
    }

    let output = Command::new("node")
        .arg(script_path)
        .arg(parsed_url.to_string())
        .current_dir(&helper_path)
        .output()
        .map_err(|_| "Unable to run Browser Mode helper.".to_string())?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    if stdout.trim().is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr)
            .chars()
            .take(200)
            .collect::<String>();
        return Err(if stderr.trim().is_empty() {
            "Browser helper returned no output.".to_string()
        } else {
            format!("Browser helper error: {}", stderr)
        });
    }

    let parsed: Value =
        serde_json::from_str(stdout.trim()).map_err(|_| "Browser helper output was not valid JSON.".to_string())?;

    let ok = parsed.get("ok").and_then(Value::as_bool).unwrap_or(false);
    if !ok {
        let message = parsed
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("Browser helper failed.");
        return Err(message.to_string());
    }

    let html = parsed
        .get("html")
        .and_then(Value::as_str)
        .ok_or_else(|| "Browser helper did not return HTML.".to_string())?
        .to_string();

    Ok(PwFetchRenderedResponse { html })
}
