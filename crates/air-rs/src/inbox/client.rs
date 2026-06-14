//! Reconnecting daemon-socket client (ports daemon-ipc.mjs connectDaemon[Persistent]).
use crate::inbox::frames::{ClientFrame, Message};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Notify};

// The live socket client is Unix-only (the daemon is a POSIX Unix-domain-socket service). On
// Windows the inbox still COMPILES — the rusqlite reader, frames, stores, gate, replay, policy, and
// identity modules are all portable — but `connect_persistent` is a build-stub: Windows is a build
// target, not a run target, in v1 (design §11).
#[cfg(unix)]
use crate::inbox::frames::ServerFrame;
#[cfg(unix)]
use crate::inbox::line_parser::{FrameEvent, LineParser};
#[cfg(unix)]
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
#[cfg(unix)]
use tokio::net::UnixStream;

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

#[cfg(unix)]
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
    #[cfg(unix)]
    tokio::spawn(reconnect_loop(cfg, events, stop, wake, rx_out));
    #[cfg(not(unix))]
    {
        // Windows build-stub (design §11): no Unix-domain sockets here. Signal Offline once and
        // park on `wake` until `stop()` notifies it, so callers get a well-behaved (never-attaching)
        // handle instead of a compile error.
        let _ = (cfg, rx_out, stop);
        tokio::spawn(async move {
            let _ = events.send(InboxEvent::Offline);
            wake.notified().await;
        });
    }
    handle
}

#[cfg(unix)]
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

#[cfg(unix)]
enum ConnOutcome {
    Stopped,
    Attached,
    FailedToConnect,
}

#[cfg(unix)]
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
        // Cancellation safety (review I1): do NOT clear `buf` at the top of the loop. `read_until`
        // is a `select!` arm — if `wake`/`rx_out` wins while it has partially read a multi-segment
        // frame, those bytes are already appended to `buf` and the next iteration's `read_until`
        // MUST resume from them. Clearing here would discard a half-read frame (silent corruption).
        // We clear ONLY after a complete, newline-terminated read has been consumed (below).
        tokio::select! {
            biased;
            _ = wake.notified() => {
                if stop.load(Ordering::SeqCst) {
                    return ConnOutcome::Stopped;
                }
            }
            out = rx_out.recv() => {
                if let Some(frame) = out {
                    // A write failure after a successful attach ends the session like an EOF; the
                    // reconnect loop treats `Attached` as "a real session ended" and the backoff
                    // reset is intentional (we DID attach). A still-down daemon then escalates via
                    // FailedToConnect on the next attempt (review I2).
                    if write_frame(&mut wr, &frame).await.is_err() {
                        return ConnOutcome::Attached;
                    }
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
                        // D1: if the event receiver has been dropped (a caller that didn't `stop()`),
                        // `dispatch` reports it on the Message/Gap paths and we tear the loop down
                        // rather than reconnect-and-emit into the void forever. The channel client
                        // depends on this — its pump owns the only receiver.
                        if !dispatch(v, events, max_seen) {
                            return ConnOutcome::Stopped;
                        }
                    }
                }
                buf.clear(); // safe: read_until returned at a newline, so `buf` held one full line
                if stop.load(Ordering::SeqCst) {
                    return ConnOutcome::Stopped;
                }
            }
        }
    }
}

/// Decode + forward one server frame. Returns `false` ONLY when the event receiver has been dropped
/// on a Message/Gap emit (D1: the caller is gone, so the reconnect loop should terminate instead of
/// emitting into the void). All other outcomes — decode failure, send-ack frames, ignored frames —
/// return `true` (keep the loop alive).
#[cfg(unix)]
fn dispatch(
    v: Value,
    events: &mpsc::UnboundedSender<InboxEvent>,
    max_seen: &mut Option<i64>,
) -> bool {
    let frame: ServerFrame = match serde_json::from_value(v) {
        Ok(f) => f,
        Err(_) => return true,
    };
    match frame {
        ServerFrame::Message { message } => {
            if max_seen.is_none_or(|m| message.relay_seq > m) {
                *max_seen = Some(message.relay_seq);
            }
            // A send error here means the receiver was dropped — signal terminate (D1).
            events.send(InboxEvent::Message(message)).is_ok()
        }
        ServerFrame::Gap { after_seq } => {
            events.send(InboxEvent::Gap { after_seq }).is_ok()
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
            true
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
            true
        }
        ServerFrame::Status { .. } => true,
        ServerFrame::Pong
        | ServerFrame::HelloOk { .. }
        | ServerFrame::Error { .. }
        | ServerFrame::Unknown => true,
    }
}

#[cfg(unix)]
async fn write_frame<W: AsyncWriteExt + Unpin>(
    w: &mut W,
    f: &ClientFrame,
) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(f).expect("frame serializes");
    line.push(b'\n');
    w.write_all(&line).await?;
    w.flush().await
}
