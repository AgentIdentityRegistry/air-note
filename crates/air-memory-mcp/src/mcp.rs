//! The hand-rolled MCP-over-stdio JSON-RPC 2.0 message handler. The MCP surface is exactly two
//! tools (`recall`, `remember`) plus the lifecycle methods `initialize` / `notifications/*` /
//! `tools/list`, so a full SDK is unnecessary (see the plan's "Open questions resolved"). Every
//! message is one JSON object on one line; [`handle_message`] returns `Some(response_line)` for a
//! request, or `None` for a notification (no `id`) — the loop writes `Some` to stdout and drops
//! `None`. Tool failures are `isError: true` tool results (never a JSON-RPC error), so the agent
//! sees a clean message instead of a broken session (I4).

use std::path::Path;

use serde_json::{json, Value};

use crate::daemon::{self, DaemonError};

/// Server identity reported in `initialize`.
pub const SERVER_NAME: &str = "air-memory-mcp";
/// Server version reported in `initialize`.
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
/// The MCP protocol revision we default to when the client omits one (we echo the client's if given).
pub const DEFAULT_PROTOCOL_VERSION: &str = "2025-06-18";
/// The two tool names.
pub const TOOL_RECALL: &str = "recall";
pub const TOOL_REMEMBER: &str = "remember";
/// Default `k` for `recall` when the caller omits it.
pub const DEFAULT_RECALL_K: usize = 8;

/// Handle one JSON-RPC line. `Some(line)` is the response to write to stdout; `None` means the
/// message was a notification (no `id`) that takes no reply. Never panics.
pub async fn handle_message(sock: &Path, line: &str) -> Option<String> {
    let msg: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return Some(error_line(Value::Null, -32700, "Parse error")),
    };
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or_default();

    // A JSON-RPC notification (no `id`) gets no reply — and never triggers a tool side effect. This
    // short-circuit runs BEFORE the method dispatch so a `tools/call` notification can never execute
    // a write (e.g. `remember`) as a side effect. The `-32700` parse-error guard above still fires
    // first, so malformed input keeps returning an error line with `id:null` per JSON-RPC.
    id.as_ref()?;
    // `id` is `Some` past the short-circuit; collapse it to an owned `Value` once so each arm below
    // reuses it (only one arm ever runs, so moving it per-arm is fine). `unwrap_or` is panic-free
    // here — the `None` case already returned above.
    let id = id.unwrap_or(Value::Null);

    match method {
        "initialize" => {
            let version = msg
                .pointer("/params/protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(DEFAULT_PROTOCOL_VERSION)
                .to_string();
            Some(result_line(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION }
                }),
            ))
        }
        "tools/list" => Some(result_line(id, tools_list_result())),
        "tools/call" => {
            let name = msg.pointer("/params/name").and_then(Value::as_str).unwrap_or_default();
            let args = msg.pointer("/params/arguments").cloned().unwrap_or(Value::Null);
            match name {
                TOOL_RECALL => Some(tool_result_line(id, run_recall(sock, &args).await)),
                TOOL_REMEMBER => Some(tool_result_line(id, run_remember(sock, &args).await)),
                // An unknown TOOL NAME is a routing failure → JSON-RPC `-32602` (invalid params),
                // whereas bad ARGUMENTS to a known tool are model-correctable → an `isError` tool
                // result (I4). The asymmetry is deliberate — do not unify these two error paths.
                other => Some(error_line(id, -32602, &format!("unknown tool: {other}"))),
            }
        }
        // A request with an unrecognized method (notifications already short-circuited above).
        _ => Some(error_line(id, -32601, &format!("method not found: {method}"))),
    }
}

/// The `tools/list` result: exactly `recall` + `remember`, each with a JSON-Schema `inputSchema`.
fn tools_list_result() -> Value {
    json!({
        "tools": [
            {
                "name": TOOL_RECALL,
                "description": "Search the user's AIR memory for notes relevant to a query. Returns \
                                ranked snippets. Use this before answering to recall prior context.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "What to search memory for." },
                        "k": {
                            "type": "integer",
                            "description": "Max results (default 8).",
                            "minimum": 1
                        }
                    },
                    "required": ["query"]
                }
            },
            {
                "name": TOOL_REMEMBER,
                "description": "Save a new note to the user's AIR memory so it can be recalled later. \
                                Stored as external (untrusted) content: recallable, never auto-applied.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "text": { "type": "string", "description": "The note to remember." }
                    },
                    "required": ["text"]
                }
            }
        ]
    })
}

/// Parse+run the `recall` tool.
async fn run_recall(sock: &Path, args: &Value) -> Result<String, DaemonError> {
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .ok_or_else(|| DaemonError::InvalidArgs("`query` (string) is required".to_string()))?;
    let k = match args.get("k") {
        None | Some(Value::Null) => DEFAULT_RECALL_K,
        Some(v) => v
            .as_u64()
            .filter(|n| *n >= 1)
            .map(|n| n as usize)
            .ok_or_else(|| DaemonError::InvalidArgs("`k` must be a positive integer".to_string()))?,
    };
    daemon::tool_recall(sock, query, k).await
}

/// Parse+run the `remember` tool.
async fn run_remember(sock: &Path, args: &Value) -> Result<String, DaemonError> {
    let text = args
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(|| DaemonError::InvalidArgs("`text` (string) is required".to_string()))?;
    daemon::tool_remember(sock, text).await
}

/// A JSON-RPC success response line.
fn result_line(id: Value, result: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

/// A JSON-RPC error response line.
fn error_line(id: Value, code: i64, message: &str) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }).to_string()
}

/// A `tools/call` result line: success text or an `isError: true` tool result (never a JSON-RPC
/// error — a failed tool must not break the session, I4).
fn tool_result_line(id: Value, outcome: Result<String, DaemonError>) -> String {
    let (text, is_error) = match outcome {
        Ok(text) => (text, false),
        Err(e) => (e.user_message(), true),
    };
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": { "content": [{ "type": "text", "text": text }], "isError": is_error }
    })
    .to_string()
}
