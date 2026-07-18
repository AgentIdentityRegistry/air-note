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

/// Per-call bound for the snapshot fetch ([`tool_snapshot`]). MUST be well under Claude Code's 5s
/// SessionStart hook timeout so the static-nudge fallback still prints before the hook is killed (a
/// cold daemon can take ~1s just to open the encrypted DB). NOT the 30s tool [`CALL_TIMEOUT`] —
/// reusing that would defeat the fallback in exactly the cold-daemon case it exists for.
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(2);

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

/// The `list_conflicts` tool: send `Request::ListConflicts`, render the pending conflicts as text.
pub async fn tool_list_conflicts(sock: &Path) -> Result<String, DaemonError> {
    match call_daemon(sock, Request::ListConflicts { onboarded: true }).await? {
        Response::ListConflicts(rows) => Ok(render_conflicts(&rows)),
        other => Err(map_error_response(other)),
    }
}

/// The `resolve_conflict` tool: send `Request::ResolveConflict`, confirm the outcome.
pub async fn tool_resolve_conflict(
    sock: &Path,
    proposal_id: &str,
    action: bossclawd_proto::types::ResolveActionWire,
) -> Result<String, DaemonError> {
    let req = Request::ResolveConflict { onboarded: true, proposal_id: proposal_id.to_string(), action };
    match call_daemon(sock, req).await? {
        Response::ResolveConflict { applied, marker_event_id } => Ok(if applied {
            format!("Resolved conflict {proposal_id}. (marker {})", marker_event_id.as_deref().unwrap_or("-"))
        } else {
            format!("Conflict {proposal_id} was already resolved (no change).")
        }),
        other => Err(map_error_response(other)),
    }
}

/// Render pending conflicts as a compact, agent-readable block.
fn render_conflicts(rows: &[bossclawd_proto::types::ConflictProposalWire]) -> String {
    if rows.is_empty() {
        return "No pending memory conflicts.".to_string();
    }
    let mut out = format!("{} pending memory conflict(s):\n", rows.len());
    for (i, r) in rows.iter().enumerate() {
        out.push_str(&format!(
            "{}. id={} [{}] {} vs {}\n",
            i + 1, r.id, r.confidence_band, describe_ref(&r.a_ref), describe_ref(&r.b_ref)
        ));
    }
    out.push_str("Use resolve_conflict with the id and an action (retire_older/retire_newer/keep_both/dismiss).");
    out
}

/// One-line, id-only description of a wire ref (no memory content — the daemon carries only ids + band).
fn describe_ref(r: &bossclawd_proto::types::ConflictRefWire) -> String {
    match r {
        bossclawd_proto::types::ConflictRefWire::Note { event_id } => format!("note:{event_id}"),
        bossclawd_proto::types::ConflictRefWire::Passage { session_id, passage_id } => {
            format!("passage:{session_id}#{passage_id}")
        }
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

/// Fetch a live orientation snapshot for the SessionStart nudge (B3). MemoryClient handshake;
/// `Request::Snapshot`; on `Response::Snapshot(s)` → `Ok(s)`; any other response or error → `Err`
/// (the caller falls back to the static [`crate::NUDGE_TEXT`]). Bounded by [`SNAPSHOT_TIMEOUT`] via
/// [`call_daemon_bounded`] (B2's shared helper), NOT the 30s [`CALL_TIMEOUT`], so a cold/wedged daemon
/// can never hold the SessionStart hook past its short budget — the fallback still prints inside 5s.
///
/// `project` MUST be the transcript's parent-dir slug ([`crate::hook::snapshot_project`]) so it matches
/// what capture stored (`server::transcript_project_slug` / the sweeper); `session_id`/`transcript_path`
/// are passed through only for the `source=compact` flavor's live-transcript digest (absent for a
/// fresh start).
pub async fn tool_snapshot(
    sock: &Path,
    project: &str,
    source: &str,
    session_id: Option<String>,
    transcript_path: Option<String>,
) -> Result<String, DaemonError> {
    let req = Request::Snapshot {
        onboarded: true,
        project: project.to_string(),
        source: source.to_string(),
        session_id,
        transcript_path,
    };
    match call_daemon_bounded(sock, req, SNAPSHOT_TIMEOUT).await? {
        Response::Snapshot(text) => Ok(text),
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
