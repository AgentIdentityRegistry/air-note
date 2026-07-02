//! The transport seam between the desktop app and the `bossclawd` daemon (M1a Task 5).
//!
//! [`Transport`] is the one-method async trait the [`super::client::EngineClient`] talks to:
//! `Request` in, `Response` out (or an [`EngineOpError`] for a transport-level failure). Two
//! implementations exist — the real [`SocketTransport`] over a persistent Unix stream, and a test
//! double the client's unit tests inject.
//!
//! # Concurrency model (M1a simplicity — DOCUMENTED CHOICE)
//! Requests are serialized through a single async `Mutex` over the connection state: at most one
//! request is in flight at a time. There are NO correlation ids — a `Request` frame is followed by
//! exactly its `Response` frame on the same stream, so the mutex makes the request/response pairing
//! unambiguous. This is the pre-resolved M1a choice (spec/plan Task 5); a correlation-id multiplexer
//! is a later optimization if concurrency ever needs it.
//!
//! # Connect + reconnect (fail-fast, never hang)
//! - Lazy connect: the stream is opened on the FIRST request (documented — no eager connect at
//!   construction), doing the [`Hello`]/[`HelloOk`] handshake once. A version mismatch or garbage
//!   handshake reply is treated as "unavailable".
//! - Reconnect-once-then-fail PER REQUEST: if the write OR read of a request round-trip errors, the
//!   stream is DROPPED (never reused after a mid-frame error — the frame codec is not
//!   cancellation-safe), a fresh connection + handshake is made, and the request is retried EXACTLY
//!   once. A second failure maps to [`EngineOpError::Unavailable`]. There is no background reconnect
//!   loop (that is a later milestone); this is per-request only.
//! - Never hang: every round-trip is wrapped in a [`tokio::time::timeout`]. On timeout the stream is
//!   dropped (safe precisely because we do NOT reuse it — we reconnect fresh), so a wedged daemon
//!   surfaces as `Unavailable` rather than an unbounded await.
//!
//! # Wired in (M1a Task 6)
//! This module + `client` are the daemon-transport seam, and the running app now consumes them:
//! `AppState.engine` is the [`super::Engine`] facade over an [`EngineClient`](super::client::EngineClient)
//! with a [`SocketTransport`] (the command tests inject `DuplexTransport` behind the same facade).
//! The Task-5-era `#![allow(dead_code)]` staging allowance is therefore gone.

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use bossclawd_proto::{read_frame, write_frame, Hello, HelloOk, Request, Response, PROTO_VERSION};
use tokio::net::UnixStream;
use tokio::sync::Mutex;

use super::EngineOpError;

/// The per-request round-trip timeout. Generous on purpose: engine ops like `run_ingest`/`evolve`
/// can take MINUTES on a large corpus, so a single large constant is used for every op rather than
/// a per-op table (M1a simplicity — documented). Its only job is to guarantee a wedged/half-dead
/// daemon can never make a call hang forever; a healthy long op finishes well within it. On a
/// timeout the stream is dropped (we never reuse a stream after a bounded-wait cancellation), so the
/// non-cancellation-safe frame codec is not left in a reused mid-frame state.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(600);

/// The transport seam: send one [`Request`], get one [`Response`] (or a transport-level
/// [`EngineOpError`]). Implementors own connection lifecycle; the client is transport-agnostic.
///
/// An `Ok(Response)` includes the daemon's own signals (`Response::Err`/`NotOnboarded`/`Busy`) —
/// those are NOT transport failures, they are engine outcomes the client maps to typed errors. An
/// `Err(EngineOpError)` from `request` means the TRANSPORT failed (daemon unreachable), always
/// [`EngineOpError::Unavailable`].
#[async_trait]
pub trait Transport: Send + Sync {
    /// Send `req` and await its `Response`. Never hangs (bounded by the implementor's timeout). A
    /// transport failure is reported as [`EngineOpError::Unavailable`].
    async fn request(&self, req: Request) -> Result<Response, EngineOpError>;
}

/// Blanket impl so an `Arc<dyn Transport>` is itself a `Transport`. This is what lets
/// [`super::Engine`] be a SINGLE non-generic type (`EngineClient<Arc<dyn Transport>>`) while still
/// choosing its concrete transport at construction — [`SocketTransport`] in production, the test
/// duplex in the command tests. `AppState` therefore needs no generic parameter (documented choice
/// in `engine/mod.rs`). Dynamic dispatch over the (already async, already `Mutex`-serialized) round
/// trip is negligible next to a socket write/read.
#[async_trait]
impl<T: Transport + ?Sized> Transport for std::sync::Arc<T> {
    async fn request(&self, req: Request) -> Result<Response, EngineOpError> {
        (**self).request(req).await
    }
}

/// A [`Transport`] over a persistent Unix-domain socket to `bossclawd`. Holds the connection behind
/// an async `Mutex` (single in-flight request; see the module docs) and lazily connects on first
/// use, performing the `Hello` handshake. See the module docs for the reconnect/timeout contract.
pub struct SocketTransport {
    socket_path: PathBuf,
    /// `Some` once connected + handshaken; `None` before the first request and after any drop. Held
    /// behind an async mutex so requests serialize (one in-flight) and the reconnect is race-free.
    conn: Mutex<Option<UnixStream>>,
}

impl SocketTransport {
    /// Build a transport that will connect to `socket_path` lazily on the first request. Does NOT
    /// connect here (documented lazy-connect) — construction is infallible and cheap.
    pub fn new(socket_path: PathBuf) -> Self {
        Self { socket_path, conn: Mutex::new(None) }
    }

    /// Open a fresh connection and complete the `Hello`/`HelloOk` handshake, or an
    /// [`EngineOpError::Unavailable`] on any failure (connect refused, I/O error, version mismatch,
    /// garbage reply). Never reuses an existing stream — always a brand-new one.
    async fn connect(&self) -> Result<UnixStream, EngineOpError> {
        let mut stream = UnixStream::connect(&self.socket_path)
            .await
            .map_err(|e| EngineOpError::Unavailable(format!("connect failed: {e}")))?;
        // Handshake: send Hello, expect a matching-version HelloOk. Bounded by the same timeout so a
        // daemon that accepts the socket but never answers the handshake can't hang us.
        let hello = Hello { proto_version: PROTO_VERSION };
        let hello_bytes = serde_json::to_vec(&hello)
            .map_err(|e| EngineOpError::Unavailable(format!("encode Hello: {e}")))?;
        let handshake = async {
            write_frame(&mut stream, &hello_bytes).await?;
            read_frame(&mut stream).await
        };
        let reply = tokio::time::timeout(REQUEST_TIMEOUT, handshake)
            .await
            .map_err(|_| EngineOpError::Unavailable("handshake timed out".to_string()))?
            .map_err(|e| EngineOpError::Unavailable(format!("handshake I/O error: {e}")))?;
        let hello_ok: HelloOk = serde_json::from_slice(&reply)
            .map_err(|_| EngineOpError::Unavailable("bad handshake reply from daemon".to_string()))?;
        if hello_ok.proto_version != PROTO_VERSION {
            return Err(EngineOpError::Unavailable(format!(
                "protocol version mismatch: client {PROTO_VERSION}, daemon {}",
                hello_ok.proto_version
            )));
        }
        // Surface the daemon pid once per (re)connect — `HelloOk` carries it for exactly this
        // single-owner diagnostic. eprintln! matches the crate's convention (no log facade).
        eprintln!("engine client: connected to bossclawd (pid {})", hello_ok.pid);
        Ok(stream)
    }

    /// One bounded request round-trip over `stream`: write the request frame, read the response
    /// frame, decode. On ANY error the CALLER must drop `stream` (it may be mid-frame — the codec is
    /// not cancellation-safe), so this takes `&mut UnixStream` and never retries internally.
    async fn round_trip(stream: &mut UnixStream, req: &Request) -> Result<Response, RoundTripError> {
        let bytes = serde_json::to_vec(req).map_err(|e| RoundTripError(format!("encode request: {e}")))?;
        let exchange = async {
            write_frame(stream, &bytes).await?;
            read_frame(stream).await
        };
        // Bounded: a timeout returns Err → the caller drops the (possibly mid-frame) stream. Safe
        // ONLY because we then reconnect fresh rather than reuse this stream.
        let frame = tokio::time::timeout(REQUEST_TIMEOUT, exchange)
            .await
            .map_err(|_| RoundTripError("request timed out".to_string()))?
            .map_err(|e| RoundTripError(format!("I/O error: {e}")))?;
        serde_json::from_slice(&frame).map_err(|e| RoundTripError(format!("decode response: {e}")))
    }
}

/// A round-trip-level failure (I/O, timeout, or a decode error). Internal — the transport maps it to
/// a reconnect-then-retry, and finally to [`EngineOpError::Unavailable`] if the retry also fails.
struct RoundTripError(String);

#[async_trait]
impl Transport for SocketTransport {
    async fn request(&self, req: Request) -> Result<Response, EngineOpError> {
        let mut guard = self.conn.lock().await;

        // ── Attempt 1: reuse an existing connection if we have one; else connect fresh. ──
        // If a first-attempt round-trip fails we DROP the stream (mid-frame-unsafe codec) and fall
        // through to the single reconnect+retry below.
        if guard.is_some() {
            let stream = guard.as_mut().expect("checked is_some");
            match Self::round_trip(stream, &req).await {
                Ok(resp) => return Ok(resp),
                Err(_) => {
                    // Drop the (possibly mid-frame) stream — never reuse it.
                    *guard = None;
                }
            }
        }

        // ── Attempt 2 (the single reconnect+retry): fresh connect + handshake, then one round-trip.
        // Any failure here is terminal → Unavailable. On success the fresh stream is memoized.
        let mut stream = self.connect().await?;
        match Self::round_trip(&mut stream, &req).await {
            Ok(resp) => {
                *guard = Some(stream);
                Ok(resp)
            }
            Err(e) => {
                // Drop the stream (do not memoize a broken connection) and report unavailable.
                Err(EngineOpError::Unavailable(e.0))
            }
        }
    }
}

// ── DuplexTransport (Task 6): the command suite's in-process daemon over `tokio::io::duplex` ──
//
// A `#[cfg(test)]` [`Transport`] that talks to the REAL daemon dispatch (`bossclawd`'s
// `serve_connection`) over an in-process byte pipe instead of a Unix socket. It exists so the
// EXISTING command tests can run their UNCHANGED assertions against `EngineClient` while the only
// thing that changed is the transport — the behavior-preservation proof for Task 6. The server half
// runs `serve_connection` (the exact code path the socket accept loop uses, Hello handshake and
// all), backed by a hermetic `test_engine` (in-memory vault + mock providers, NEVER the OS
// keychain), so the previously keychain-hanging command tests become hermetic too.
#[cfg(test)]
pub use duplex::DuplexTransport;

#[cfg(test)]
mod duplex {
    use super::*;
    use bossclawd_proto::{read_frame, write_frame, Hello, HelloOk, Request, Response, PROTO_VERSION};
    use tokio::io::DuplexStream;

    /// The in-process transport double. Holds the CLIENT half of one `tokio::io::duplex` pipe; the
    /// SERVER half is driven by `serve_connection` (the daemon's real dispatch) running on a
    /// DEDICATED thread + its own `current_thread` runtime. One persistent connection, one in-flight
    /// request at a time (the same `Mutex` model as [`SocketTransport`]) — no reconnect, because a
    /// duplex pipe cannot be re-dialed. If it ever breaks the transport reports
    /// [`EngineOpError::Unavailable`]; the command tests never break it.
    ///
    /// # Why a dedicated thread (not `tokio::spawn`)
    /// The Tauri IPC command tests are plain `#[test]` (Tauri drives its own runtime for the invoke),
    /// so `DuplexTransport::new` is called from a NON-async context where `tokio::spawn` would panic
    /// with "no reactor running". Owning a thread + runtime makes construction context-independent —
    /// the two `DuplexStream` halves are just in-memory buffers with wakers, so it's fine for the
    /// server half to be driven by this thread's runtime while the client half is polled by whatever
    /// runtime the caller uses. This mirrors the client tests' `TestDaemon` thread-per-daemon model.
    pub struct DuplexTransport {
        /// The client end + a "handshake done" flag, behind an async mutex so requests serialize and
        /// the one-time handshake is race-free (mirrors `SocketTransport`'s single-in-flight model).
        conn: Mutex<DuplexConn>,
    }

    struct DuplexConn {
        stream: DuplexStream,
        handshaken: bool,
    }

    impl DuplexTransport {
        /// Wire up an in-process daemon backed by a temp-home hermetic engine at `home` and return a
        /// transport over the client half. Runs the daemon's real `serve_connection` on a dedicated
        /// thread; that thread exits once this transport (and thus the client half) is dropped.
        pub fn new(home: std::path::PathBuf) -> Self {
            // HERMETIC: pre-seed the daemon's process-global provider-key cache to EMPTY so any op
            // that reads a provider-key fingerprint (`GetReasonerReady` → `current_key_fingerprint`)
            // short-circuits and NEVER hits the OS keychain (a keychain-ACL prompt hangs CI forever —
            // a hard project lesson). This is what makes the previously keychain-hanging command test
            // (`engine_get_reasoner_config_defaults_local_not_ready`) hermetic. The cache is a static
            // shared by every daemon instance, so one empty seed covers all.
            bossclawd::vault::seed_secret_cache_for_test(std::collections::HashMap::new());
            // A generous buffer: engine responses (recall hits, previews) are small and framing is
            // length-prefixed, so a modest pipe never starves a single-in-flight round trip.
            let (client, server) = tokio::io::duplex(64 * 1024);
            // The daemon's REAL per-connection dispatch (handshake → Request → engine → Response),
            // the same fn the socket accept loop spawns — so the command suite exercises the true
            // daemon code path, just without a socket. On its own thread + current-thread runtime so
            // construction works from a sync (`#[test]`) caller. The thread is detached: its serve
            // loop ends on its own when this transport (and thus the client half) is dropped and the
            // pipe closes (`read_frame` → EOF), so the thread finishes shortly after — no join needed.
            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("build duplex daemon runtime");
                rt.block_on(async move {
                    let engine = std::sync::Arc::new(bossclawd::server::test_engine(home));
                    let (read, write) = tokio::io::split(server);
                    let _ = bossclawd::server::serve_connection(engine, read, write).await;
                });
            });
            Self { conn: Mutex::new(DuplexConn { stream: client, handshaken: false }) }
        }

        /// The Hello/HelloOk handshake, done once on the first request (same contract as
        /// `SocketTransport::connect`): a version mismatch or garbage reply is treated as unavailable.
        async fn handshake(stream: &mut DuplexStream) -> Result<(), EngineOpError> {
            let hello = Hello { proto_version: PROTO_VERSION };
            let hello_bytes = serde_json::to_vec(&hello)
                .map_err(|e| EngineOpError::Unavailable(format!("encode Hello: {e}")))?;
            let exchange = async {
                write_frame(stream, &hello_bytes).await?;
                read_frame(stream).await
            };
            let reply = tokio::time::timeout(REQUEST_TIMEOUT, exchange)
                .await
                .map_err(|_| EngineOpError::Unavailable("handshake timed out".to_string()))?
                .map_err(|e| EngineOpError::Unavailable(format!("handshake I/O error: {e}")))?;
            let hello_ok: HelloOk = serde_json::from_slice(&reply)
                .map_err(|_| EngineOpError::Unavailable("bad handshake reply".to_string()))?;
            if hello_ok.proto_version != PROTO_VERSION {
                return Err(EngineOpError::Unavailable(format!(
                    "protocol version mismatch: client {PROTO_VERSION}, daemon {}",
                    hello_ok.proto_version
                )));
            }
            Ok(())
        }
    }

    #[async_trait]
    impl Transport for DuplexTransport {
        async fn request(&self, req: Request) -> Result<Response, EngineOpError> {
            let mut guard = self.conn.lock().await;
            if !guard.handshaken {
                Self::handshake(&mut guard.stream).await?;
                guard.handshaken = true;
            }
            let bytes = serde_json::to_vec(&req)
                .map_err(|e| EngineOpError::Unavailable(format!("encode request: {e}")))?;
            let stream = &mut guard.stream;
            let exchange = async {
                write_frame(stream, &bytes).await?;
                read_frame(stream).await
            };
            let frame = tokio::time::timeout(REQUEST_TIMEOUT, exchange)
                .await
                .map_err(|_| EngineOpError::Unavailable("request timed out".to_string()))?
                .map_err(|e| EngineOpError::Unavailable(format!("I/O error: {e}")))?;
            serde_json::from_slice(&frame)
                .map_err(|e| EngineOpError::Unavailable(format!("decode response: {e}")))
        }
    }
}
