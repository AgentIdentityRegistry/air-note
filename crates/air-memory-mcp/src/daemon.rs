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

/// Per-call connect + round-trip bound. Generous, but guarantees a wedged/absent daemon can never
/// hang a tool call.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// The SHORT bound for the fire-and-forget capture poke ([`tool_capture_notify`]). Deliberately a
/// couple of seconds — NOT [`CALL_TIMEOUT`] — so a wedged daemon can never hang the millisecond
/// SessionEnd hook. On timeout the poke simply fails (discarded → exit 0 at the caller); the sweeper
/// backfills the missed session, so the immediacy is an optimization, never the durability path.
const CAPTURE_NOTIFY_TIMEOUT: Duration = Duration::from_secs(2);

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

/// Resolve the daemon socket path exactly as the daemon does — `BOSSCLAWD_SOCKET` if set, else
/// `<data_dir>/bossclawd.sock`, where `data_dir` is `BOSSCLAWD_DATA_DIR` if set, else the platform
/// app-data dir for the AIR Agent bundle id. Delegates to the shared `bossclawd-paths` crate so the
/// adapter and the daemon can never resolve to different paths. A hand-wired `.mcp.json` typically
/// sets `BOSSCLAWD_SOCKET` explicitly (see the crate README).
pub fn resolve_socket_path() -> PathBuf {
    bossclawd_paths::resolve_socket_path(&bossclawd_paths::resolve_data_dir())
}

/// Open a fresh connection, handshake as [`Role::MemoryClient`], send `req`, read the `Response`.
/// Bounded by [`CALL_TIMEOUT`]; any failure is a [`DaemonError`] (never a panic).
pub async fn call_daemon(sock: &Path, req: Request) -> Result<Response, DaemonError> {
    call_daemon_bounded(sock, req, CALL_TIMEOUT).await
}

/// The shared connect + handshake + one round-trip, bounded by an explicit `timeout`. Factored out
/// so the 30s tool calls ([`call_daemon`]) and the SHORT-timeout capture poke
/// ([`tool_capture_notify`]) run the exact same exchange with different bounds — the poke must never
/// inherit the 30s bound and hang the SessionEnd hook. Any failure is a [`DaemonError`] (never a
/// panic).
async fn call_daemon_bounded(
    sock: &Path,
    req: Request,
    timeout: Duration,
) -> Result<Response, DaemonError> {
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
    tokio::time::timeout(timeout, exchange)
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

/// Fire-and-forget capture poke (B2): a SHORT-timeout single round-trip asking the daemon to render
/// the just-ended Claude Code session now. All failures map to Ok(()) at the CALLER (the
/// `capture-notify` subcommand exits 0 regardless — the sweeper is the durability guarantee, I1/§6).
/// Uses [`CAPTURE_NOTIFY_TIMEOUT`], NOT the 30s [`CALL_TIMEOUT`], so a wedged daemon can never hang
/// the SessionEnd hook. The daemon validates `session_id` + confines `transcript_path` and may
/// cleanly reject (capture disabled, rate-limited, bad id/path); any non-`Ok` becomes an error the
/// caller discards.
pub async fn tool_capture_notify(
    sock: &Path,
    session_id: &str,
    transcript_path: &str,
) -> Result<(), DaemonError> {
    let req = Request::CaptureNotify {
        onboarded: true,
        session_id: session_id.to_string(),
        transcript_path: transcript_path.to_string(),
    };
    match call_daemon_bounded(sock, req, CAPTURE_NOTIFY_TIMEOUT).await? {
        Response::Ok => Ok(()),
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
