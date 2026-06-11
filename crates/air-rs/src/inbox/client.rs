//! Reconnecting daemon-socket client (ports daemon-ipc.mjs connectDaemon[Persistent]).
use crate::inbox::frames::{ClientFrame, Message, ServerFrame};
use crate::inbox::line_parser::{FrameEvent, LineParser};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, Notify};

/// Initial reconnect backoff.
pub const INITIAL_BACKOFF: Duration = Duration::from_millis(500);
/// Maximum reconnect backoff.
pub const BACKOFF_CAP: Duration = Duration::from_millis(5000);
/// Handshake (hello → hello-ok) timeout.
pub const HANDSHAKE: Duration = Duration::from_millis(3000);

/// The role this client connects as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Sees all (mute-filtered) messages — the inbox feed.
    Viewer,
    /// Sees only channel-gated messages — the AI feed.
    Channel,
}

impl Role {
    fn wire(self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Channel => "channel",
        }
    }
}

/// What the client surfaces to the caller. The caller (Tauri layer) forwards these as events and,
/// for the Channel role, drives the replayer on `Gap`.
#[derive(Debug, Clone)]
pub enum InboxEvent {
    /// Attached to the daemon (handshake complete).
    Attached {
        /// Daemon process id.
        pid: i64,
        /// Daemon identity DID.
        did: String,
    },
    /// The connection closed; the client is reconnecting.
    Detached,
    /// The daemon is unreachable (attach failed); emitted once per outage streak.
    Offline,
    /// A delivered message.
    Message(Message),
    /// A sequence gap: the channel client should replay from the archive after `after_seq`.
    Gap {
        /// Replay messages with relay_seq strictly greater than this.
        after_seq: i64,
    },
    /// A send succeeded.
    SendOk {
        /// Correlation id from the send.
        id: String,
        /// Relay-assigned envelope id.
        envelope_id: String,
        /// Whether the envelope was encrypted.
        encrypted: bool,
    },
    /// A send failed.
    SendErr {
        /// Correlation id from the send.
        id: String,
        /// Whether a retry can plausibly succeed.
        retryable: bool,
        /// Human-readable reason.
        reason: String,
    },
    /// A raw status frame (as JSON).
    Status(Value),
}

/// Control handle: send frames to the daemon, or stop the client.
pub struct ClientHandle {
    stop: Arc<AtomicBool>,
    wake: Arc<Notify>,
    tx_out: mpsc::UnboundedSender<ClientFrame>,
}

impl ClientHandle {
    /// Queue a client frame (e.g. a `send`) for delivery on the active connection.
    pub fn send_frame(&self, f: ClientFrame) {
        let _ = self.tx_out.send(f);
    }
    /// Stop the client (closes the connection and ends the reconnect loop).
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.wake.notify_waiters();
    }
}

/// Configuration for a persistent client.
pub struct ClientConfig {
    /// Path to the daemon's Unix socket.
    pub socket_path: PathBuf,
    /// The role to connect as.
    pub role: Role,
    /// Baseline resume cursor captured BEFORE the first connect (archive cursor, or None).
    pub baseline: Option<i64>,
}

/// Spawn the persistent client on the caller's tokio runtime. Returns immediately with a handle.
pub fn connect_persistent(
    cfg: ClientConfig,
    events: mpsc::UnboundedSender<InboxEvent>,
) -> ClientHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let wake = Arc::new(Notify::new());
    let (tx_out, rx_out) = mpsc::unbounded_channel::<ClientFrame>();
    let handle = ClientHandle {
        stop: stop.clone(),
        wake: wake.clone(),
        tx_out,
    };
    tokio::spawn(reconnect_loop(cfg, events, stop, wake, rx_out));
    handle
}

async fn reconnect_loop(
    cfg: ClientConfig,
    events: mpsc::UnboundedSender<InboxEvent>,
    stop: Arc<AtomicBool>,
    wake: Arc<Notify>,
    mut rx_out: mpsc::UnboundedReceiver<ClientFrame>,
) {
    let mut max_seen: Option<i64> = None;
    let mut backoff = INITIAL_BACKOFF;
    let mut first = true;
    let mut offline_signaled = false;

    while !stop.load(Ordering::SeqCst) {
        if !first {
            tokio::select! {
                _ = tokio::time::sleep(backoff) => {}
                _ = wake.notified() => {}
            }
            if stop.load(Ordering::SeqCst) {
                return;
            }
        }
        let since = if first { None } else { max_seen.or(cfg.baseline) };
        first = false;

        match connect_once(
            &cfg,
            since,
            &events,
            &stop,
            &wake,
            &mut rx_out,
            &mut max_seen,
        )
        .await
        {
            ConnOutcome::Stopped => return,
            ConnOutcome::Attached => {
                backoff = INITIAL_BACKOFF;
                offline_signaled = false;
                let _ = events.send(InboxEvent::Detached);
            }
            ConnOutcome::FailedToConnect => {
                if !offline_signaled {
                    let _ = events.send(InboxEvent::Offline);
                    offline_signaled = true;
                }
                backoff = (backoff * 2).min(BACKOFF_CAP);
            }
        }
    }
}

enum ConnOutcome {
    Stopped,
    Attached,
    FailedToConnect,
}

async fn connect_once(
    cfg: &ClientConfig,
    since: Option<i64>,
    events: &mpsc::UnboundedSender<InboxEvent>,
    stop: &Arc<AtomicBool>,
    wake: &Arc<Notify>,
    rx_out: &mut mpsc::UnboundedReceiver<ClientFrame>,
    max_seen: &mut Option<i64>,
) -> ConnOutcome {
    let stream = match UnixStream::connect(&cfg.socket_path).await {
        Ok(s) => s,
        Err(_) => return ConnOutcome::FailedToConnect,
    };
    let (rd, mut wr) = stream.into_split();
    let mut reader = BufReader::new(rd);

    let hello = ClientFrame::Hello {
        role: cfg.role.wire().to_string(),
        since_seq: since,
    };
    if write_frame(&mut wr, &hello).await.is_err() {
        return ConnOutcome::FailedToConnect;
    }

    // Handshake phase: read lines until hello-ok or timeout.
    // We use a separate buf and parser here; after the handshake we reuse reader but
    // allocate fresh buf/parser for the main loop (avoids lifetime issues with the
    // async block that cannot borrow outer locals across await points in select!).
    let attached = {
        let mut hs_parser = LineParser::new();
        let mut hs_buf = Vec::new();
        tokio::time::timeout(HANDSHAKE, async {
            loop {
                hs_buf.clear();
                let n = reader.read_until(b'\n', &mut hs_buf).await.ok()?;
                if n == 0 {
                    return None;
                }
                for ev in hs_parser.feed(&hs_buf) {
                    if let FrameEvent::Frame(v) = ev {
                        match serde_json::from_value::<ServerFrame>(v) {
                            Ok(ServerFrame::HelloOk { pid, did, .. }) => {
                                return Some((pid, did))
                            }
                            Ok(_) | Err(_) => return None,
                        }
                    }
                }
            }
        })
        .await
        .ok()
        .flatten()
    };

    let (pid, did) = match attached {
        Some(v) => v,
        None => return ConnOutcome::FailedToConnect,
    };
    if stop.load(Ordering::SeqCst) {
        return ConnOutcome::Stopped;
    }
    let _ = events.send(InboxEvent::Attached { pid, did });

    // Main event loop: fan-in on stop-wake, outbound frames, and inbound reads.
    let mut parser = LineParser::new();
    let mut buf = Vec::new();

    loop {
        buf.clear();
        tokio::select! {
            biased;
            _ = wake.notified() => {
                if stop.load(Ordering::SeqCst) {
                    return ConnOutcome::Stopped;
                }
            }
            out = rx_out.recv() => {
                match out {
                    Some(frame) => {
                        if write_frame(&mut wr, &frame).await.is_err() {
                            return ConnOutcome::Attached;
                        }
                    }
                    None => {}
                }
            }
            read = reader.read_until(b'\n', &mut buf) => {
                let n = match read {
                    Ok(n) => n,
                    Err(_) => return ConnOutcome::Attached,
                };
                if n == 0 {
                    return ConnOutcome::Attached;
                }
                for ev in parser.feed(&buf) {
                    if let FrameEvent::Frame(v) = ev {
                        dispatch(v, events, max_seen);
                    }
                }
                if stop.load(Ordering::SeqCst) {
                    return ConnOutcome::Stopped;
                }
            }
        }
    }
}

fn dispatch(
    v: Value,
    events: &mpsc::UnboundedSender<InboxEvent>,
    max_seen: &mut Option<i64>,
) {
    let frame: ServerFrame = match serde_json::from_value(v) {
        Ok(f) => f,
        Err(_) => return,
    };
    match frame {
        ServerFrame::Message { message } => {
            if max_seen.map_or(true, |m| message.relay_seq > m) {
                *max_seen = Some(message.relay_seq);
            }
            let _ = events.send(InboxEvent::Message(message));
        }
        ServerFrame::Gap { after_seq } => {
            let _ = events.send(InboxEvent::Gap { after_seq });
        }
        ServerFrame::SendOk {
            id,
            envelope_id,
            encrypted,
        } => {
            let _ = events.send(InboxEvent::SendOk {
                id,
                envelope_id,
                encrypted,
            });
        }
        ServerFrame::SendErr {
            id,
            retryable,
            reason,
        } => {
            let _ = events.send(InboxEvent::SendErr {
                id,
                retryable,
                reason,
            });
        }
        ServerFrame::Status { .. } => {}
        ServerFrame::Pong
        | ServerFrame::HelloOk { .. }
        | ServerFrame::Error { .. }
        | ServerFrame::Unknown => {}
    }
}

async fn write_frame<W: AsyncWriteExt + Unpin>(
    w: &mut W,
    f: &ClientFrame,
) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(f).expect("frame serializes");
    line.push(b'\n');
    w.write_all(&line).await?;
    w.flush().await
}
