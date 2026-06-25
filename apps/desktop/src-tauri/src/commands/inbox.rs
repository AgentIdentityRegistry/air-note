//! Tauri command surface for the inbox (design §3/§6/§8). The viewer connection feeds live events;
//! send goes over the socket and its ack returns as an event; history/identity/policy are reads.
use crate::commands::identity::AppState;
use air_rs::inbox::archive_reader::ArchiveReader;
use air_rs::inbox::client::{connect_persistent, ClientConfig, ClientHandle, InboxEvent, Role};
use air_rs::inbox::frames::ClientFrame;
use air_rs::inbox::identity_adopter::{adopt, Adoption};
use air_rs::inbox::policy_store::{
    autonomy_for, cancel as policy_cancel, confirm as policy_confirm, reserve as policy_reserve,
    set_autonomy, Autonomy,
};
use air_rs::inbox::bridge_home;
use crate::llm_stream::first_active_agent_id;
use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::mpsc;

/// RFC-3339 UTC `now`, the clock the policy store's recency/budget/structural guards parse with
/// `chrono::DateTime::parse_from_rfc3339` (see `policy_store::within`). Stamped server-side so the
/// AI loop can never backdate the budget clock.
fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Probe whether a daemon socket + identity exist (drives the "install the CLI" screen).
#[tauri::command]
pub async fn inbox_status() -> Result<Value, String> {
    let home = bridge_home();
    Ok(json!({
        "home": home.to_string_lossy(),
        "socket_exists": home.join("daemon.sock").exists(),
        "identity_exists": home.join("identity.json").exists(),
        "archive_exists": home.join("archive.db").exists(),
    }))
}

/// The adopted identity (collision-as-norm). `desktop_prior_did` is the desktop's own legacy id, if any.
#[tauri::command]
pub async fn inbox_identity(desktop_prior_did: Option<String>) -> Result<Adoption, String> {
    Ok(adopt(&bridge_home(), desktop_prior_did.as_deref()))
}

/// Start the live viewer connection. Idempotent: a second call is a no-op while connected.
#[tauri::command]
pub async fn inbox_start(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    // M-1: assumes single-threaded front-end invocation (one webview, no concurrent double-start).
    // The check-then-store window below (lock released between the is_some() check and the store) is
    // acceptable under that assumption and is intentionally not guarded against a concurrent racer.
    {
        let guard = state.inbox.viewer.lock().map_err(|_| "viewer lock poisoned".to_string())?;
        if guard.is_some() {
            return Ok(());
        }
    }
    let home = bridge_home();
    let (tx, mut rx) = mpsc::unbounded_channel::<InboxEvent>();
    let handle: ClientHandle = connect_persistent(
        ClientConfig { socket_path: home.join("daemon.sock"), role: Role::Viewer, baseline: None },
        tx,
    );
    {
        let mut guard = state.inbox.viewer.lock().map_err(|_| "viewer lock poisoned".to_string())?;
        *guard = Some(handle);
    }
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(ev) = rx.recv().await {
            match ev {
                InboxEvent::Attached { pid, did } => { let _ = app2.emit("inbox_attached", json!({"pid": pid, "did": did})); }
                InboxEvent::Detached => { let _ = app2.emit("inbox_detached", json!({})); }
                InboxEvent::Offline => { let _ = app2.emit("inbox_offline", json!({})); }
                InboxEvent::Message(m) => { let _ = app2.emit("inbox_message", &m); }
                InboxEvent::Gap { .. } => {} // gap is channel-only (PROTOCOL §5); viewer never receives it
                InboxEvent::SendOk { id, envelope_id, encrypted } => { let _ = app2.emit("inbox_send_ok", json!({"id": id, "envelope_id": envelope_id, "encrypted": encrypted})); }
                InboxEvent::SendErr { id, retryable, reason } => { let _ = app2.emit("inbox_send_err", json!({"id": id, "retryable": retryable, "reason": reason})); }
                InboxEvent::Status(_) => {}
            }
        }
    });
    Ok(())
}

/// Stop the live viewer connection.
#[tauri::command]
pub async fn inbox_stop(state: State<'_, AppState>) -> Result<(), String> {
    let mut guard = state.inbox.viewer.lock().map_err(|_| "viewer lock poisoned".to_string())?;
    if let Some(h) = guard.take() {
        h.stop();
    }
    Ok(())
}

/// Send a message over the socket. Returns the correlation id; the ack arrives as an event.
#[tauri::command]
pub async fn inbox_send(
    state: State<'_, AppState>,
    to: String,
    body: Value,
    thread_id: Option<String>,
    in_reply_to: Option<String>,
) -> Result<String, String> {
    let id = uuid::Uuid::new_v4().to_string();
    let guard = state.inbox.viewer.lock().map_err(|_| "viewer lock poisoned".to_string())?;
    let handle = guard.as_ref().ok_or("inbox not connected — call inbox_start first")?;
    handle.send_frame(ClientFrame::Send {
        id: id.clone(), to, body, plaintext: None, thread_id, in_reply_to,
    });
    Ok(id)
}

/// Conversation list (newest-first) for the §6 sidebar — 1:1 keyed by peer_did, rooms by room_id.
#[tauri::command]
pub async fn inbox_conversations() -> Result<Value, String> {
    let home = bridge_home();
    if !home.join("archive.db").exists() {
        return Ok(json!([]));
    }
    tauri::async_runtime::spawn_blocking(move || -> Result<Value, String> {
        let reader = ArchiveReader::open(&home).map_err(|e| e.to_string())?;
        let convs = reader.conversations().map_err(|e| e.to_string())?;
        serde_json::to_value(convs).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// All pinned contacts with their DID keys, for the §6 did→display-name join (Milestone C).
/// Mirrors `inbox_conversations`: no args, guards on file existence, reads on a blocking task.
#[tauri::command]
pub async fn inbox_contacts() -> Result<Value, String> {
    let home = bridge_home();
    if !home.join("contacts.json").exists() {
        return Ok(json!([]));
    }
    tauri::async_runtime::spawn_blocking(move || -> Result<Value, String> {
        let contacts = air_rs::inbox::stores::list_contacts(&home);
        serde_json::to_value(contacts).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// History for one peer, one room, or recent across peers when both are None.
#[tauri::command]
pub async fn inbox_history(
    peer: Option<String>,
    room: Option<String>,
    limit: Option<i64>,
    include_spam: Option<bool>,
) -> Result<Value, String> {
    let home = bridge_home();
    if !home.join("archive.db").exists() {
        return Ok(json!([]));
    }
    tauri::async_runtime::spawn_blocking(move || -> Result<Value, String> {
        let reader = ArchiveReader::open(&home).map_err(|e| e.to_string())?;
        let rows = reader
            .history(peer.as_deref(), None, room.as_deref(), None, limit.unwrap_or(50), include_spam.unwrap_or(false))
            .map_err(|e| e.to_string())?;
        serde_json::to_value(rows).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Read a contact's autonomy dial.
#[tauri::command]
pub async fn inbox_policy_get(did: String) -> Result<String, String> {
    Ok(match autonomy_for(&bridge_home(), &did) {
        Autonomy::Off => "off", Autonomy::Draft => "draft", Autonomy::Auto => "auto",
    }.to_string())
}

/// Set a contact's autonomy dial.
#[tauri::command]
pub async fn inbox_policy_set(did: String, value: String) -> Result<(), String> {
    let v = match value.as_str() {
        "off" => Autonomy::Off, "draft" => Autonomy::Draft, "auto" => Autonomy::Auto,
        other => return Err(format!("unknown autonomy '{other}'")),
    };
    // CF1: set_autonomy does synchronous file I/O behind the process-global POLICY_LOCK; running it
    // directly in this async body would block a Tokio worker. Push it onto a blocking worker.
    tauri::async_runtime::spawn_blocking(move || set_autonomy(&bridge_home(), &did, v).map(|_| ()))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// The outcome of an AI auto-send reservation, surfaced to the controller (D2/D11 guards live in
/// air-rs `policy_store::reserve`). `decision` is the lowercase autonomy verb (`off`/`draft`/`auto`);
/// `token` is `Some` iff `decision == "auto"` and is the key for [`inbox_ai_confirm`]/[`inbox_ai_cancel`].
#[derive(Debug, Serialize)]
pub struct AiReservation {
    /// `"off"` | `"draft"` | `"auto"` — only `"auto"` means "send now".
    pub decision: String,
    /// Human-readable reason, surfaced to the UI/log.
    pub reason: String,
    /// The reservation key; present iff `decision == "auto"`.
    pub token: Option<String>,
}

/// Reserve an AI auto-send slot for `did` (the atomic D2/D11 decision). `received_at` is the message's
/// ingest time (the recency clock); `now` is stamped server-side. If the answer is `auto`, a pending
/// reservation is persisted and its `token` returned — the controller must later [`inbox_ai_confirm`]
/// it (on send_ok) or [`inbox_ai_cancel`] it (on failure/discard) so it doesn't wedge the budget.
#[tauri::command]
pub async fn inbox_ai_reserve(
    did: String,
    thread_id: Option<String>,
    received_at: String,
) -> Result<AiReservation, String> {
    let now = now_rfc3339();
    // CF1: reserve does synchronous file I/O behind the process-global POLICY_LOCK — never run it on
    // a Tokio worker thread.
    tauri::async_runtime::spawn_blocking(move || {
        let r = policy_reserve(&bridge_home(), &did, thread_id.as_deref(), &received_at, &now)
            .map_err(|e| e.to_string())?;
        let decision = match r.decision {
            Autonomy::Off => "off",
            Autonomy::Draft => "draft",
            Autonomy::Auto => "auto",
        }
        .to_string();
        Ok(AiReservation { decision, reason: r.reason, token: r.token })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Finalize a reservation with the real `envelope_id` (on `inbox_send_ok`). `now` is stamped
/// server-side; air-rs `confirm` re-inserts a confirmed record if the pending one was already pruned
/// (I-A), so a real auto-send is never lost from the ledger or the structural guard.
#[tauri::command]
pub async fn inbox_ai_confirm(
    did: String,
    token: String,
    envelope_id: String,
    thread_id: String,
) -> Result<(), String> {
    let now = now_rfc3339();
    // CF1: confirm does synchronous file I/O behind the process-global POLICY_LOCK.
    tauri::async_runtime::spawn_blocking(move || {
        policy_confirm(&bridge_home(), &did, &token, &envelope_id, &thread_id, &now)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Drop a reservation (on send_err / llm_error / discard) so it never counts against budget. Only
/// removes a still-pending reservation, so a late cancel can't delete an already-confirmed send.
#[tauri::command]
pub async fn inbox_ai_cancel(did: String, token: String) -> Result<(), String> {
    // CF1: cancel does synchronous file I/O behind the process-global POLICY_LOCK.
    tauri::async_runtime::spawn_blocking(move || {
        policy_cancel(&bridge_home(), &did, &token).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// D4: the default reply agent for the AI loop — the first non-archived agent id from `agents.json`,
/// deterministic in array order. Returns `None` (not an error) when there's no usable agent, so the
/// UI can prompt "configure a reply model" without the command failing. File read/parse errors also
/// collapse to `None` for the same reason. The `agents.json` shape logic is reused from `llm_stream`.
#[tauri::command]
pub async fn inbox_default_agent(app: AppHandle) -> Result<Option<String>, String> {
    // app_data_dir() is a cheap path lookup (no I/O); the file READ goes on a blocking worker (M-5,
    // CF1 discipline). The pure parse runs afterward off the returned string.
    let Ok(dir) = app.path().app_data_dir() else {
        return Ok(None);
    };
    let raw = tauri::async_runtime::spawn_blocking(move || {
        std::fs::read_to_string(dir.join("agents.json")).ok()
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(raw.as_deref().and_then(first_active_agent_id))
}
