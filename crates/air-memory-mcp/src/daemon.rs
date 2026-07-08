//! The adapter's thin socket client over `bossclawd-proto`. Reuses proto's frame codec, handshake,
//! and `Request`/`Response` types verbatim (I5) — it never reimplements the wire protocol. Connects
//! fresh per tool call (MCP calls are infrequent; a fresh connect sidesteps the
//! non-cancellation-safe codec's mid-frame-reuse hazard entirely), handshakes as
//! [`Role::MemoryClient`], sends one request, reads one response. Every failure maps to a
//! [`DaemonError`] the MCP layer renders as a clean tool error (I4) — never a panic.

use std::path::{Path, PathBuf};
use std::time::Duration;

use bossclawd_proto::{
    read_frame, write_frame, Hello, HelloOk, OpErrorKindWire, Request, Response, Role, PROTO_VERSION,
};
use tokio::net::UnixStream;

// These four consts + `data_dir()`/`resolve_socket_path()` MUST stay in sync with the daemon's own
// socket/data-dir resolution in `crates/bossclawd/src/main.rs`. If the daemon changes how it
// resolves either, update BOTH. SP1 mirrors the logic deliberately rather than extracting a shared
// crate — OS path logic does not belong in the wire-protocol crate, and this is not a live bug (a
// follow-up may hoist a shared resolver).
/// Env override for the socket path (mirrors the daemon's `BOSSCLAWD_SOCKET`).
const ENV_SOCKET: &str = "BOSSCLAWD_SOCKET";
/// Env override for the data dir (mirrors the daemon's `BOSSCLAWD_DATA_DIR`).
const ENV_DATA_DIR: &str = "BOSSCLAWD_DATA_DIR";
/// The socket file under the data dir (mirrors the daemon's `SOCKET_FILE`).
const SOCKET_FILE: &str = "bossclawd.sock";
/// The AIR Agent bundle dir name (mirrors the daemon's `APP_DIR_NAME`).
const APP_DIR_NAME: &str = "ai.air-agent.desktop";
/// Per-call connect + round-trip bound. Generous, but guarantees a wedged/absent daemon can never
/// hang a tool call.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// A daemon-call failure, rendered by the MCP layer as a clean tool error (never a panic).
#[derive(Debug)]
pub enum DaemonError {
    /// The daemon socket is down / unreachable (connect refused, I/O error, timeout, bad handshake).
    Unavailable(String),
    /// The brain is not set up yet (the daemon recomputed onboarding for our guest role).
    NotOnboarded,
    /// Blank `remember` text, rejected before the daemon round-trip (defense in depth).
    EmptyText,
    /// A tool argument was missing or the wrong type.
    InvalidArgs(String),
    /// A typed engine error crossed the wire (`Response::Err`).
    Wire(String),
    /// An unexpected response variant (protocol drift) or a `Busy` signal.
    Protocol(String),
}

impl DaemonError {
    /// A single, user-facing sentence for the coding agent (surfaced as an `isError` tool result).
    pub fn user_message(&self) -> String {
        match self {
            DaemonError::Unavailable(_) => {
                "AIR memory service is unavailable — is AIR Agent (the bossclawd daemon) running?"
                    .to_string()
            }
            DaemonError::NotOnboarded => {
                "AIR Agent isn't set up yet — open the AIR Agent app and complete onboarding first."
                    .to_string()
            }
            DaemonError::EmptyText => "Cannot remember empty or blank text.".to_string(),
            DaemonError::InvalidArgs(m) => format!("Invalid tool arguments: {m}"),
            DaemonError::Wire(m) => format!("AIR memory error: {m}"),
            DaemonError::Protocol(m) => format!("Unexpected AIR memory response: {m}"),
        }
    }
}

/// Resolve the daemon socket path exactly as the daemon does: `BOSSCLAWD_SOCKET` if set, else
/// `<data_dir>/bossclawd.sock`, where `data_dir` is `BOSSCLAWD_DATA_DIR` if set, else the platform
/// app-data dir for the AIR Agent bundle id. A hand-wired `.mcp.json` typically sets
/// `BOSSCLAWD_SOCKET` explicitly (see the crate README).
pub fn resolve_socket_path() -> PathBuf {
    if let Some(p) = std::env::var_os(ENV_SOCKET) {
        return PathBuf::from(p);
    }
    data_dir().join(SOCKET_FILE)
}

/// The platform data dir (mirrors the daemon's `resolve_data_dir`/`app_data_dir`). Falls back to
/// the current dir only if `HOME` is unset (a degraded environment).
fn data_dir() -> PathBuf {
    if let Some(d) = std::env::var_os(ENV_DATA_DIR) {
        return PathBuf::from(d);
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join("Library/Application Support").join(APP_DIR_NAME);
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
            return PathBuf::from(xdg).join(APP_DIR_NAME);
        }
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(".local/share").join(APP_DIR_NAME);
        }
    }
    PathBuf::from(".")
}

/// Open a fresh connection, handshake as [`Role::MemoryClient`], send `req`, read the `Response`.
/// Bounded by [`CALL_TIMEOUT`]; any failure is a [`DaemonError`] (never a panic).
pub async fn call_daemon(sock: &Path, req: Request) -> Result<Response, DaemonError> {
    let exchange = async {
        let mut stream = UnixStream::connect(sock)
            .await
            .map_err(|e| DaemonError::Unavailable(format!("connect failed: {e}")))?;
        let hello = Hello { proto_version: PROTO_VERSION, role: Role::MemoryClient };
        let hello_bytes = serde_json::to_vec(&hello)
            .map_err(|e| DaemonError::Protocol(format!("encode Hello: {e}")))?;
        write_frame(&mut stream, &hello_bytes)
            .await
            .map_err(|e| DaemonError::Unavailable(format!("handshake write: {e}")))?;
        let reply = read_frame(&mut stream)
            .await
            .map_err(|e| DaemonError::Unavailable(format!("handshake read: {e}")))?;
        let hello_ok: HelloOk = serde_json::from_slice(&reply)
            .map_err(|_| DaemonError::Unavailable("bad handshake reply".to_string()))?;
        if hello_ok.proto_version != PROTO_VERSION {
            // Not `Unavailable`: the daemon IS running and reachable, it just speaks a different
            // protocol. `Protocol` keeps the actionable "adapter X, daemon Y" detail in
            // `user_message` instead of collapsing it to a generic "is AIR Agent running?".
            return Err(DaemonError::Protocol(format!(
                "protocol version mismatch: adapter {PROTO_VERSION}, daemon {}",
                hello_ok.proto_version
            )));
        }
        let req_bytes = serde_json::to_vec(&req)
            .map_err(|e| DaemonError::Protocol(format!("encode request: {e}")))?;
        write_frame(&mut stream, &req_bytes)
            .await
            .map_err(|e| DaemonError::Unavailable(format!("request write: {e}")))?;
        let frame = read_frame(&mut stream)
            .await
            .map_err(|e| DaemonError::Unavailable(format!("response read: {e}")))?;
        serde_json::from_slice::<Response>(&frame)
            .map_err(|e| DaemonError::Protocol(format!("decode response: {e}")))
    };
    tokio::time::timeout(CALL_TIMEOUT, exchange)
        .await
        .map_err(|_| DaemonError::Unavailable("daemon call timed out".to_string()))?
}

/// Map a non-success `Response` (shared by both tools) to a `DaemonError`.
fn map_error_response(resp: Response) -> DaemonError {
    match resp {
        Response::NotOnboarded => DaemonError::NotOnboarded,
        Response::Busy(op) => DaemonError::Protocol(format!("memory service busy: {op}")),
        Response::Err { kind, message } => match kind {
            OpErrorKindWire::NotPermitted => {
                // Unreachable via the 2-tool surface (defense in depth); surface it plainly.
                DaemonError::Wire("operation not permitted".to_string())
            }
            _ => DaemonError::Wire(message),
        },
        other => DaemonError::Protocol(format!("unexpected response: {other:?}")),
    }
}

/// The `recall` tool: send `Request::Recall`, render the hits as a readable text block.
pub async fn tool_recall(sock: &Path, query: &str, k: usize) -> Result<String, DaemonError> {
    match call_daemon(sock, Request::Recall { onboarded: true, query: query.to_string(), k }).await? {
        Response::Recall(hits) => Ok(render_hits(query, &hits)),
        other => Err(map_error_response(other)),
    }
}

/// The `remember` tool: reject blank text, else send `Request::Remember` and confirm with the id.
pub async fn tool_remember(sock: &Path, text: &str) -> Result<String, DaemonError> {
    if text.trim().is_empty() {
        return Err(DaemonError::EmptyText);
    }
    match call_daemon(sock, Request::Remember { onboarded: true, text: text.to_string() }).await? {
        Response::Remember(id) => Ok(format!("Remembered. (event {id})")),
        other => Err(map_error_response(other)),
    }
}

/// Render recall hits as a compact, agent-readable text block.
fn render_hits(query: &str, hits: &[bossclawd_proto::HitWire]) -> String {
    if hits.is_empty() {
        return format!("No memories found for \"{query}\".");
    }
    let mut out = format!("{} memory result(s) for \"{query}\":\n", hits.len());
    for (i, h) in hits.iter().enumerate() {
        // Collapse any interior newline/whitespace run to a single space so a multi-line snippet
        // stays on one numbered row (a bare `trim()` would only strip the ends and let an interior
        // newline spill across lines, misaligning the list an agent reads).
        let snippet = h.text.split_whitespace().collect::<Vec<_>>().join(" ");
        out.push_str(&format!(
            "{}. [{}] (score {:.3}) {}\n",
            i + 1,
            h.hit.kind,
            h.hit.score,
            snippet
        ));
    }
    out
}
