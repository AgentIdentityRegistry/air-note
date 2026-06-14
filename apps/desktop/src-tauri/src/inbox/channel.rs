//! The desktop's live CHANNEL (AI-feed) connection. One per app, mirroring the viewer in
//! `commands/inbox.rs`, but with the channel-specific replay drive (PROTOCOL §5): the channel is the
//! only role the daemon sends `Gap` to, and on a gap we replay the hole from the archive through the
//! `Replayer` (which re-applies invariants #4 + #5 and dedupes across the live/replay overlap).
//!
//! This is a SECOND socket SUBSCRIBER, not a second puller — the daemon supports N subscribers, and
//! the viewer + channel coexisting is the design's intent. There is exactly one pull loop (the
//! daemon's); this client only subscribes.
use crate::commands::identity::AppState;
use air_rs::inbox::archive_reader::ArchiveReader;
use air_rs::inbox::bridge_home;
use air_rs::inbox::client::{connect_persistent, ClientConfig, ClientHandle, InboxEvent, Role};
use air_rs::inbox::replay::Replayer;
use air_rs::inbox::stores::parse_mute_set;
use serde_json::json;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::mpsc;

/// Start the live channel (AI-feed) connection. Idempotent: a second call is a no-op while connected.
///
/// Baseline resume cursor = the archive's pull cursor (`ArchiveReader::get_cursor`), captured BEFORE
/// the first connect so a reconnect after the very first attach can resume from the daemon's frontier
/// rather than replaying the whole archive. The mute set comes from `AIRMSG_MUTE` (env), parsed once
/// for the lifetime of this connection and held in the spawned task's `Replayer`.
#[tauri::command]
pub async fn inbox_channel_start(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    // M-1: assumes single-threaded front-end invocation (one webview, no concurrent double-start).
    // The lock is released between this idempotency check and the store below; that check-then-store
    // window is acceptable under that assumption and is not guarded against a concurrent racer.
    {
        let guard = state.inbox.channel.lock().map_err(|_| "channel lock poisoned".to_string())?;
        if guard.is_some() {
            return Ok(());
        }
    }
    let home = bridge_home();

    // Baseline cursor is blocking file/DB I/O — read it off the Tokio workers (read discipline).
    // A missing/unreadable archive yields `None` (full first-attach resume), never a hard error.
    // M-2: the existence probe lives INSIDE the closure too — both to keep all I/O off the async
    // thread and to short-circuit BEFORE `ArchiveReader::open`'s retry loop (12×50ms) on a missing DB.
    let home_for_cursor = home.clone();
    let baseline: Option<i64> = tauri::async_runtime::spawn_blocking(move || {
        if !home_for_cursor.join("archive.db").exists() {
            return None;
        }
        ArchiveReader::open(&home_for_cursor)
            .and_then(|r| r.get_cursor())
            .ok()
    })
    .await
    .map_err(|e| e.to_string())?;

    let (tx, mut rx) = mpsc::unbounded_channel::<InboxEvent>();
    let handle: ClientHandle = connect_persistent(
        ClientConfig { socket_path: home.join("daemon.sock"), role: Role::Channel, baseline },
        tx,
    );
    {
        let mut guard = state.inbox.channel.lock().map_err(|_| "channel lock poisoned".to_string())?;
        *guard = Some(handle);
    }

    let app2 = app.clone();
    let home2 = home.clone();
    tauri::async_runtime::spawn(async move {
        // The replayer lives for the whole connection: its dedupe set spans the live/replay overlap
        // (a gap replay can re-deliver messages the live stream already showed). The mute set is
        // parsed once here — same lifetime as the JS pipeline's per-connection mute.
        let mut replayer = Replayer::new(parse_mute_set());
        while let Some(ev) = rx.recv().await {
            match ev {
                InboxEvent::Attached { pid, did } => {
                    let _ = app2.emit("inbox_channel_attached", json!({"pid": pid, "did": did}));
                }
                InboxEvent::Detached => {
                    let _ = app2.emit("inbox_channel_detached", json!({}));
                }
                InboxEvent::Offline => {
                    let _ = app2.emit("inbox_channel_offline", json!({}));
                }
                InboxEvent::Message(m) => {
                    // Live path: dedupe + re-gate (#5) via the replayer; emit only admitted messages.
                    if let Some(admitted) = replayer.live(m) {
                        let _ = app2.emit("inbox_channel_message", &admitted);
                    }
                }
                InboxEvent::Gap { after_seq } => {
                    // Replay the hole from the archive. The reader open + paginated read is blocking
                    // I/O, so run it on a blocking worker; the replayer must move in and back out so
                    // its dedupe state persists across gaps (hence the move-in / return tuple).
                    let home_for_gap = home2.clone();
                    let taken = std::mem::replace(&mut replayer, Replayer::new(Default::default()));
                    let result = tauri::async_runtime::spawn_blocking(move || {
                        match ArchiveReader::open(&home_for_gap) {
                            Ok(reader) => {
                                let mut r = taken;
                                let msgs = r.gap(&reader, &home_for_gap, after_seq);
                                (r, msgs.ok())
                            }
                            // Archive unavailable: keep the replayer (with its dedupe set) intact and
                            // emit nothing — the next live frame or gap will retry.
                            Err(_) => (taken, None),
                        }
                    })
                    .await;
                    match result {
                        Ok((r, msgs)) => {
                            replayer = r;
                            if let Some(msgs) = msgs {
                                for m in &msgs {
                                    let _ = app2.emit("inbox_channel_message", m);
                                }
                            }
                        }
                        // The blocking task panicked or was cancelled: the moved-out replayer is gone,
                        // so rebuild a fresh one (re-reads mute) rather than wedge the pump. Dedupe
                        // history is lost, but re-gating still holds and the daemon won't re-send gaps.
                        Err(_) => {
                            replayer = Replayer::new(parse_mute_set());
                        }
                    }
                }
                // The channel never sends; ignore any stray send acks defensively.
                InboxEvent::SendOk { .. } | InboxEvent::SendErr { .. } | InboxEvent::Status(_) => {}
            }
        }
    });
    Ok(())
}

/// Stop the live channel connection.
#[tauri::command]
pub async fn inbox_channel_stop(state: State<'_, AppState>) -> Result<(), String> {
    let mut guard = state.inbox.channel.lock().map_err(|_| "channel lock poisoned".to_string())?;
    if let Some(h) = guard.take() {
        h.stop();
    }
    Ok(())
}
