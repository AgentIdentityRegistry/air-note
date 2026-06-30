use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, COOKIE, USER_AGENT};
use reqwest::redirect;
use reqwest::{Client, Url};
use serde::Serialize;
use serde_json::Value;
use std::fs;
use std::net::{IpAddr, Ipv4Addr, ToSocketAddrs};
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

/// Maximum redirects we will follow; each hop is re-validated against the SSRF guard.
const MAX_REDIRECTS: usize = 5;

/// SSRF guard: `true` when `ip` is NOT a safe public destination — loopback, private,
/// link-local (incl. the `169.254.169.254` cloud-metadata endpoint), CGNAT, benchmarking,
/// reserved, unspecified, broadcast, multicast, or documentation space. IPv4-in-IPv6
/// forms are unwrapped so `::ffff:127.0.0.1` (and `::1`) are caught too.
pub(crate) fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_multicast()
                || octets[0] == 0 // 0.0.0.0/8 "this network"
                || (octets[0] == 100 && (octets[1] & 0xc0) == 64) // 100.64.0.0/10 CGNAT
                || (octets[0] == 198 && (octets[1] & 0xfe) == 18) // 198.18.0.0/15 benchmarking
                || octets[0] >= 240 // 240.0.0.0/4 reserved (Ipv4Addr::is_reserved is unstable)
        }
        IpAddr::V6(v6) => {
            // Unwrap IPv4-mapped (::ffff:a.b.c.d) / IPv4-compatible (::a.b.c.d, incl. ::1).
            if let Some(v4) = v6.to_ipv4() {
                if is_blocked_ip(IpAddr::V4(v4)) {
                    return true;
                }
            }
            let segments = v6.segments();
            // NAT64 well-known prefix 64:ff9b::/96 embeds an IPv4 in the low 32 bits; a NAT64
            // gateway translates it to that v4, so unwrap and re-check the embedded address.
            if segments[..6] == [0x0064, 0xff9b, 0, 0, 0, 0] {
                let embedded = Ipv4Addr::new(
                    (segments[6] >> 8) as u8,
                    (segments[6] & 0xff) as u8,
                    (segments[7] >> 8) as u8,
                    (segments[7] & 0xff) as u8,
                );
                if is_blocked_ip(IpAddr::V4(embedded)) {
                    return true;
                }
            }
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (segments[0] & 0xfe00) == 0xfc00 // fc00::/7 unique local
                || (segments[0] & 0xffc0) == 0xfe80 // fe80::/10 link-local
        }
    }
}

/// Reject a URL whose host is — or resolves to — a private/internal address (SSRF guard).
/// Literal IPs are checked directly; hostnames are resolved and rejected if ANY resolved
/// address is internal. Blocking DNS — call [`validate_host_async`] from async code.
///
/// RESIDUAL (DNS rebinding): for a *hostname* this is a pre-flight check — the OS resolves
/// the name again at connect time, so an attacker who controls the record and flips it
/// between the two lookups could still reach an internal address. Closing that fully needs
/// connect-time IP pinning (a custom `reqwest::dns::Resolve` that runs `is_blocked_ip` on
/// the addresses it returns); tracked as a follow-up. Literal-IP and redirect-target checks
/// — the common SSRF vectors — are not affected by this race.
fn validate_host(url: &Url) -> Result<(), String> {
    let host_raw = url
        .host_str()
        .ok_or_else(|| "URL host is invalid.".to_string())?;
    // `host_str` brackets IPv6 literals (e.g. "[::1]"); strip before parsing.
    let host = host_raw.trim_start_matches('[').trim_end_matches(']');

    if let Ok(ip) = host.parse::<IpAddr>() {
        return if is_blocked_ip(ip) {
            Err(format!(
                "Refusing to fetch a private or internal address ({host})."
            ))
        } else {
            Ok(())
        };
    }

    let port = url.port_or_known_default().unwrap_or(443);
    let mut resolved_any = false;
    for addr in (host, port)
        .to_socket_addrs()
        .map_err(|_| format!("Unable to resolve host ({host})."))?
    {
        resolved_any = true;
        if is_blocked_ip(addr.ip()) {
            return Err(format!(
                "Refusing to fetch a host that resolves to a private or internal address ({host})."
            ));
        }
    }
    if !resolved_any {
        return Err(format!("Unable to resolve host ({host})."));
    }
    Ok(())
}

/// Async wrapper around [`validate_host`] that runs the blocking DNS lookup off the runtime.
async fn validate_host_async(url: &Url) -> Result<(), String> {
    let url = url.clone();
    tokio::task::spawn_blocking(move || validate_host(&url))
        .await
        .map_err(|_| "URL validation failed.".to_string())?
}

fn redirect_blocked_error(message: &str) -> std::io::Error {
    std::io::Error::other(message.to_string())
}

fn build_http_client() -> Result<Client, String> {
    // Re-validate EVERY redirect hop: a permitted public host can 30x to an internal
    // address (e.g. 169.254.169.254), so the initial-URL check alone is not enough. The
    // initial URL is validated by the caller before the request is sent; this policy
    // guards each subsequent hop.
    Client::builder()
        .redirect(redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= MAX_REDIRECTS {
                return attempt.error(redirect_blocked_error("Too many redirects."));
            }
            match validate_host(attempt.url()) {
                Ok(()) => attempt.follow(),
                Err(message) => attempt.error(redirect_blocked_error(&format!(
                    "Blocked redirect to a private or internal address: {message}"
                ))),
            }
        }))
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
    validate_host_async(&parsed_url).await?;
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
    validate_host_async(&parsed_url).await?;
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
    // SSRF guard for the headless-browser path. NOTE: this validates the INITIAL URL only;
    // Chromium follows its own redirects, so a 30x to an internal address is a residual risk
    // for Browser Mode (tracked for a follow-up: a Playwright route guard in the helper).
    validate_host(&parsed_url)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn v4(s: &str) -> IpAddr {
        IpAddr::V4(s.parse::<Ipv4Addr>().unwrap())
    }
    fn v6(s: &str) -> IpAddr {
        IpAddr::V6(s.parse::<Ipv6Addr>().unwrap())
    }

    #[test]
    fn blocks_internal_and_reserved_ipv4() {
        for ip in [
            "127.0.0.1",        // loopback
            "127.10.20.30",     // loopback /8
            "169.254.169.254",  // link-local cloud metadata
            "10.0.0.1",         // 10.0.0.0/8
            "10.255.255.255",
            "172.16.0.1",       // 172.16.0.0/12
            "172.31.255.255",
            "192.168.0.1",      // 192.168.0.0/16
            "192.168.255.255",
            "0.0.0.0",          // this-network
            "100.64.0.1",       // CGNAT
            "198.18.0.1",       // benchmarking
            "240.0.0.1",        // reserved
            "255.255.255.255",  // broadcast
            "224.0.0.1",        // multicast
        ] {
            assert!(is_blocked_ip(v4(ip)), "{ip} must be blocked");
        }
    }

    #[test]
    fn allows_public_ipv4_including_range_boundaries() {
        for ip in [
            "8.8.8.8", "1.1.1.1", "93.184.216.34", "140.82.112.3",
            "172.15.255.255", // just below 172.16/12
            "172.32.0.1",     // just above 172.16/12
            "198.20.0.1",     // just above 198.18/15
            "100.63.255.255", // just below CGNAT
            "100.128.0.1",    // just above CGNAT
            "223.255.255.255", // just below the multicast/reserved 224.0.0.0/3 block
        ] {
            assert!(!is_blocked_ip(v4(ip)), "{ip} must be allowed");
        }
    }

    #[test]
    fn blocks_internal_ipv6_and_mapped_v4() {
        for ip in [
            "::1",                    // loopback
            "::",                     // unspecified
            "fc00::1",                // unique-local
            "fd12:3456:789a::1",      // unique-local
            "fe80::1",                // link-local
            "ff02::1",                // multicast
            "::ffff:127.0.0.1",       // v4-mapped loopback
            "::ffff:169.254.169.254", // v4-mapped metadata
            "::ffff:10.0.0.1",        // v4-mapped private
            "64:ff9b::7f00:1",        // NAT64 of 127.0.0.1
            "64:ff9b::a00:1",         // NAT64 of 10.0.0.1
        ] {
            assert!(is_blocked_ip(v6(ip)), "{ip} must be blocked");
        }
    }

    #[test]
    fn allows_public_ipv6() {
        for ip in [
            "2606:4700:4700::1111",
            "2001:4860:4860::8888",
            "64:ff9b::808:808", // NAT64 of the PUBLIC 8.8.8.8 — must remain allowed
        ] {
            assert!(!is_blocked_ip(v6(ip)), "{ip} must be allowed");
        }
    }

    #[test]
    fn validate_host_rejects_literal_internal_urls() {
        for url in [
            "http://127.0.0.1/",
            "http://169.254.169.254/latest/meta-data/",
            "http://10.0.0.1:8080/x",
            "http://192.168.1.1/",
            "http://[::1]/",
            "http://[fd00::1]/",
            "http://0.0.0.0/",
        ] {
            let parsed = Url::parse(url).unwrap();
            assert!(validate_host(&parsed).is_err(), "{url} must be rejected");
        }
    }

    // Alternate IPv4 encodings (decimal / hex / octal) for 127.0.0.1 — the WHATWG URL
    // parser normalizes these to dotted-decimal, so the literal-IP guard catches them.
    // If a form is NOT normalized to a literal IP, it falls to the resolver path, which
    // still rejects an internal resolution — so either way it must be blocked.
    #[test]
    fn validate_host_rejects_alternate_ip_encodings() {
        for url in [
            "http://2130706433/",   // decimal 127.0.0.1
            "http://0x7f000001/",   // hex 127.0.0.1
            "http://0177.0.0.1/",   // octal-leading 127.0.0.1
        ] {
            let parsed = Url::parse(url).unwrap();
            assert!(validate_host(&parsed).is_err(), "{url} (alt-encoded loopback) must be rejected");
        }
    }

    #[test]
    fn validate_host_allows_public_literal_urls() {
        for url in ["http://8.8.8.8/", "https://1.1.1.1/"] {
            let parsed = Url::parse(url).unwrap();
            assert!(validate_host(&parsed).is_ok(), "{url} must be allowed");
        }
    }

    // The redirect policy in `build_http_client` applies `validate_host` to EVERY hop, so a
    // public host that 30x-redirects to one of these targets is rejected exactly like a
    // direct request to it. This asserts that per-hop decision at the validation level.
    #[test]
    fn redirect_targets_to_internal_addresses_are_rejected() {
        for target in [
            "http://169.254.169.254/", // cloud metadata via redirect
            "http://127.0.0.1/admin",
            "http://10.0.0.1/",
            "http://192.168.0.1/",
        ] {
            let parsed = Url::parse(target).unwrap();
            assert!(
                validate_host(&parsed).is_err(),
                "redirect target {target} must be rejected"
            );
        }
    }

    #[tokio::test]
    async fn web_fetch_public_blocks_internal_targets_before_connecting() {
        for url in [
            "http://127.0.0.1/",
            "http://169.254.169.254/latest/meta-data/",
            "http://10.0.0.1/",
            "http://192.168.1.1/",
            "http://[::1]/",
        ] {
            let result = web_fetch_public(url.to_string()).await;
            assert!(
                result.is_err(),
                "web_fetch_public({url}) must be blocked by the SSRF guard"
            );
        }
    }
}
