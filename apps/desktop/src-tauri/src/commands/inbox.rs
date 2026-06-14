//! Tauri command surface for the inbox (design §3/§6/§8). The viewer connection feeds live events;
//! send goes over the socket and its ack returns as an event; history/identity/policy are reads.
use crate::commands::identity::AppState;
use air_rs::inbox::archive_reader::ArchiveReader;
use air_rs::inbox::client::{connect_persistent, ClientConfig, ClientHandle, InboxEvent, Role};
use air_rs::inbox::frames::ClientFrame;
use air_rs::inbox::identity_adopter::{adopt, Adoption};
use air_rs::inbox::policy_store::{autonomy_for, set_autonomy, Autonomy};
use air_rs::inbox::bridge_home;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, State};
use tokio::sync::mpsc;

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
    {
        let guard = state.inbox.viewer.lock().map_err(|_| "inbox lock".to_string())?;
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
        let mut guard = state.inbox.viewer.lock().map_err(|_| "inbox lock".to_string())?;
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
    let mut guard = state.inbox.viewer.lock().map_err(|_| "inbox lock".to_string())?;
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
    let guard = state.inbox.viewer.lock().map_err(|_| "inbox lock".to_string())?;
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
    set_autonomy(&bridge_home(), &did, v).map_err(|e| e.to_string())?;
    Ok(())
}
