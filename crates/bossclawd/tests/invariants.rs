//! M1a Task 7 — invariant + failure-path suite at the DAEMON boundary.
//!
//! This file gathers the daemon-side invariants that Tasks 3–6 factored into composable pieces but
//! did not yet assert end-to-end through the daemon's real startup arbitration + handshake. Each test
//! spins the PRODUCTION accept path (`server::spawn_for_test`, which runs the same `run_accept_loop`
//! `main.rs` runs) over a real temp Unix socket, hermetically (in-memory vault + mock providers via
//! `test-helpers`, NEVER the OS keychain — a keychain-ACL prompt hangs CI forever).
//!
//! Every await is bounded by a TEST-side timeout so a wedged daemon fails the test rather than
//! hanging CI.
//!
//! # Coverage map (net-new here vs. pointers to existing coverage — no duplication)
//! - **Single-owner** (net-new here): the daemon boundary refuses a SECOND owner. `lock.rs`'s unit
//!   tests already cover `acquire_or_refuse` in isolation (reclaim-stale / refuse-live / 0600 / guard
//!   drop); THIS file proves the two arbitration gates end-to-end through the running daemon — the
//!   authoritative socket-bind gate (`AddrInUse` on a second bind of a live socket) AND the advisory
//!   fast path (`acquire_or_refuse` reclaims a stale lock + stale socket, then a fresh daemon binds
//!   and serves on the reclaimed path).
//! - **Version skew** (net-new here at the daemon boundary): a client whose `Hello` carries the wrong
//!   `proto_version` is closed WITHOUT a `HelloOk` and without any op dispatched. The app-layer half
//!   of this invariant (a `SocketTransport` against a version-skewed daemon → `Unavailable`) lives in
//!   the desktop `transport.rs` test module (`socket_transport_version_skew_is_unavailable`).
//! - **Fail-closed at connect / mid-request** (pointers): both are app-layer guarantees and live in
//!   the desktop `client.rs`/`transport.rs` test modules, which own the reusable `TestDaemon` harness
//!   (kill/restart a real daemon on a socket). See `fail_closed_at_connect_no_daemon`,
//!   `socket_transport_no_daemon_never_opens_local_store`, and `unavailable_mid_request_when_killed`.
//! - **Onboarding** (pointer + net-new socket variant): `roundtrip.rs::not_onboarded_roundtrip`
//!   already round-trips `NotOnboarded` over a real socket (raw frames); the desktop
//!   `client.rs::not_onboarded_maps_to_open_not_onboarded` covers the `SocketTransport` typed-error
//!   variant. This file adds a raw-frame reassertion alongside the single-owner tests for locality.

#![cfg(unix)]

use std::path::PathBuf;
use std::time::Duration;

use bossclawd::{lock, server};
use bossclawd_proto::{
    read_frame, write_frame, Hello, HelloOk, Request, Response, Role, PROTO_VERSION,
};
use tokio::net::{UnixListener, UnixStream};

/// A generous per-await test bound: every op answers near-instantly against the mock providers, so
/// any await that reaches this is a hang (a test failure), not slowness. Keeps a wedged daemon or a
/// never-answered handshake from stalling CI forever.
const TEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Await `fut`, failing the test (rather than hanging CI) if it exceeds [`TEST_TIMEOUT`].
async fn bounded<F: std::future::Future>(fut: F) -> F::Output {
    tokio::time::timeout(TEST_TIMEOUT, fut).await.expect("operation must not hang")
}

/// Spin a real daemon on a fresh temp socket under a temp home, returning the tempdir guard (keeps
/// the socket + home alive) and the socket path. Mirrors `roundtrip.rs::spawn_daemon`: seeds the
/// process-global provider-key cache to EMPTY first so any op that reads a provider-key fingerprint
/// never hits the real OS keychain (the documented CI-hang hazard).
async fn spawn_daemon() -> (tempfile::TempDir, PathBuf) {
    bossclawd::vault::seed_secret_cache_for_test(Default::default());
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("bossclawd.sock");
    server::spawn_for_test(sock.clone(), dir.path().to_path_buf()).await;
    (dir, sock)
}

/// A PID guaranteed not to be alive: spawn `true`, wait for it to exit, then reuse its (now-freed)
/// PID promptly. No sleeps, no hardcoded PIDs (mirrors `lock.rs`'s unit-test helper).
fn dead_pid() -> i32 {
    let mut child = std::process::Command::new("true").spawn().expect("spawn `true`");
    let pid = child.id() as i32;
    child.wait().expect("wait for `true` to exit");
    pid
}

// ── (a) Single-owner: a SECOND bind on a LIVE socket refuses (the authoritative gate). ──

/// The daemon's real single-owner gate is the socket bind (`main.rs` treats an `AddrInUse` bind as
/// the final arbiter — the loser of the race steps aside). With a live daemon already serving on a
/// socket, a second `UnixListener::bind` on the SAME path MUST fail with `AddrInUse` — proving the OS
/// itself refuses a second owner, so the daemon can never double-serve one memory cursor.
#[tokio::test]
async fn second_bind_on_live_socket_is_addr_in_use() {
    let (_dir, sock) = spawn_daemon().await;

    // Sanity: the live daemon answers a real handshake (it truly owns the socket).
    let mut stream = bounded(UnixStream::connect(&sock)).await.expect("connect to live daemon");
    let hello = Hello { proto_version: PROTO_VERSION, role: Role::App };
    bounded(write_frame(&mut stream, &serde_json::to_vec(&hello).unwrap()))
        .await
        .expect("send Hello");
    let reply = bounded(read_frame(&mut stream)).await.expect("read HelloOk");
    let hello_ok: HelloOk = serde_json::from_slice(&reply).expect("parse HelloOk");
    assert_eq!(hello_ok.proto_version, PROTO_VERSION, "live owner speaks our version");

    // The authoritative gate: a second bind on the SAME live socket path is refused by the OS.
    let err = UnixListener::bind(&sock).expect_err("second bind on a live socket must fail");
    assert_eq!(
        err.kind(),
        std::io::ErrorKind::AddrInUse,
        "second owner must lose the bind race with AddrInUse, got {err:?}"
    );
}

// ── (a) Single-owner: a STALE lock + STALE socket are reclaimed, then a fresh daemon serves. ──

/// A crashed daemon can leave a stale lock file (dead PID) AND a stale socket file behind. A fresh
/// daemon must reclaim BOTH (the advisory `acquire_or_refuse` fast path) and then bind + serve on the
/// reclaimed path — proving the leftovers of a dead owner don't wedge a restart. `lock.rs` unit-tests
/// `acquire_or_refuse` in isolation; this drives it end-to-end into a live, serving daemon.
#[tokio::test]
async fn stale_lock_and_socket_are_reclaimed_then_daemon_serves() {
    bossclawd::vault::seed_secret_cache_for_test(Default::default());
    let dir = tempfile::tempdir().unwrap();
    let lock_path = dir.path().join("bossclawd.lock");
    let sock = dir.path().join("bossclawd.sock");

    // Leave behind a dead owner's litter: a lock file with a dead PID + a stale socket *file* with
    // nothing listening (a plain file stands in for a leftover socket inode — `connect()` fails on
    // it, so it is reclaimable, not a live owner). This is exactly the on-disk state after a crash.
    std::fs::write(&sock, b"").expect("write stale socket file");
    // A dead-PID lock file, written via the same JSON shape the daemon writes (pid + since).
    std::fs::write(
        &lock_path,
        serde_json::to_vec(&serde_json::json!({ "pid": dead_pid(), "since": "0" })).unwrap(),
    )
    .expect("write stale lock file");

    // The advisory fast path reclaims: it unlinks the stale socket + stale lock and takes ownership,
    // returning a guard whose PID is ours. (This is the exact call `main.rs` step (2) makes.)
    let guard = lock::acquire_or_refuse(&lock_path, &sock).expect("reclaim stale leftovers");
    assert_eq!(guard.pid(), std::process::id() as i32, "we own the reclaimed lock");
    assert!(!sock.exists(), "the stale socket file was unlinked on reclaim");

    // End-to-end: a fresh daemon now binds + serves on the reclaimed socket path and answers ops.
    server::spawn_for_test(sock.clone(), dir.path().to_path_buf()).await;
    let mut client = RawClient::connect(&sock).await;
    match bounded(client.call(Request::Status { onboarded: true })).await {
        Response::Status(s) => assert!(s.chain_ok, "the reclaimed-path daemon serves a live engine"),
        other => panic!("expected Status from the reclaimed-path daemon, got {other:?}"),
    }

    // Keep the guard alive until here so its Drop (which unlinks the lock iff we own it) runs last.
    drop(guard);
}

// ── (b) Version skew: a wrong-`proto_version` Hello is closed WITHOUT a HelloOk. ──

/// The handshake is the version gate: a client built against an incompatible protocol sends a `Hello`
/// with a mismatched `proto_version`, and the daemon MUST close the connection without replying
/// `HelloOk` and without dispatching any op. The daemon sends a best-effort error frame first (so a
/// client sees a reason) then EOF; the crucial invariant is that whatever arrives is NOT a valid
/// same-version `HelloOk`, and the stream then ends — so a skewed client can never mis-deserialize a
/// later frame against the wrong schema. (The app-layer half — `SocketTransport` → `Unavailable` — is
/// in the desktop `transport.rs` test `socket_transport_version_skew_is_unavailable`.)
#[tokio::test]
async fn version_skew_hello_is_closed_without_hello_ok() {
    let (_dir, sock) = spawn_daemon().await;
    let mut stream = bounded(UnixStream::connect(&sock)).await.expect("connect");

    // A client from the FUTURE: one version ahead. (u32 = 1 today, so +1 = 2 — no overflow.)
    let skewed = Hello { proto_version: PROTO_VERSION + 1, role: Role::App };
    bounded(write_frame(&mut stream, &serde_json::to_vec(&skewed).unwrap()))
        .await
        .expect("send skewed Hello");

    // Read whatever the daemon sends. It replies a best-effort protocol-error frame then closes, so
    // either a frame arrives (which must NOT parse as a same-version HelloOk) or the stream is
    // already at EOF. Bounded so a (buggy) daemon that neither answers nor closes fails, not hangs.
    // An immediate EOF (the `Err` arm falls through) is also a correct refusal.
    if let Ok(frame) = bounded(read_frame(&mut stream)).await {
        // If anything came back it must NOT be a valid same-version HelloOk (the skew was refused).
        let as_hello_ok: Result<HelloOk, _> = serde_json::from_slice(&frame);
        assert!(
            !as_hello_ok.map(|h| h.proto_version == PROTO_VERSION).unwrap_or(false),
            "a version-skewed Hello must NOT get a matching HelloOk"
        );
        // Positively pin the typed-refusal contract production actually sends: the frame IS the
        // daemon's protocol-error `Response::Err` (not just some non-HelloOk garbage). This is what
        // `serve_connection`'s version-mismatch branch writes before closing.
        let refusal: Response = serde_json::from_slice(&frame)
            .expect("version-skew reply must be a Response");
        assert!(
            matches!(refusal, Response::Err { .. }),
            "version-skew refusal must be a typed Response::Err, got {refusal:?}"
        );
        // The next read must be EOF: the daemon closes after the single error frame (no dispatch).
        let eof = bounded(read_frame(&mut stream)).await;
        assert!(eof.is_err(), "daemon must close the skewed connection (EOF), got {eof:?}");
    }
}

// ── Onboarding: the `NotOnboarded` SIGNAL crosses a real socket (raw frames). ──

/// A raw-frame reassertion (co-located with the single-owner tests for locality) that a gated op with
/// `onboarded: false` returns the `NotOnboarded` SIGNAL, not a fault, over a real socket. This mirrors
/// `roundtrip.rs::not_onboarded_roundtrip`; the `SocketTransport` typed-error variant
/// (`Open(NotOnboarded)`) is covered by the desktop `client.rs::not_onboarded_maps_to_open_not_onboarded`.
#[tokio::test]
async fn not_onboarded_signal_over_socket() {
    let (_dir, sock) = spawn_daemon().await;
    let mut client = RawClient::connect(&sock).await;
    let resp = bounded(client.call(Request::ListGrants { onboarded: false })).await;
    assert!(
        matches!(resp, Response::NotOnboarded),
        "gated op with onboarded=false → NotOnboarded signal, got {resp:?}"
    );
}

// ── A minimal raw framed client (handshake once, then Request→Response frames). ──
//
// Deliberately a tiny local copy of `roundtrip.rs::Client`: an integration test crate can't share a
// `#[cfg(test)]`/private helper across test files, and the raw framed client is ~15 lines. It does
// NOT reimplement daemon dispatch (the hard rule) — it only drives the wire, exactly as the desktop
// `SocketTransport` does.

/// A connected raw client: one `UnixStream` speaking the framed protocol after a `Hello`/`HelloOk`.
struct RawClient {
    stream: UnixStream,
}

impl RawClient {
    /// Connect + do the `Hello`/`HelloOk` handshake, asserting a matching-version `HelloOk`.
    async fn connect(sock: &std::path::Path) -> Self {
        let mut stream = bounded(UnixStream::connect(sock)).await.expect("connect to daemon socket");
        let hello = Hello { proto_version: PROTO_VERSION, role: Role::App };
        bounded(write_frame(&mut stream, &serde_json::to_vec(&hello).unwrap()))
            .await
            .expect("send Hello");
        let reply = bounded(read_frame(&mut stream)).await.expect("read HelloOk");
        let hello_ok: HelloOk = serde_json::from_slice(&reply).expect("parse HelloOk");
        assert_eq!(hello_ok.proto_version, PROTO_VERSION, "daemon speaks our version");
        Self { stream }
    }

    /// Send one `Request`, read its `Response`.
    async fn call(&mut self, req: Request) -> Response {
        bounded(write_frame(&mut self.stream, &serde_json::to_vec(&req).unwrap()))
            .await
            .expect("send request");
        let frame = bounded(read_frame(&mut self.stream)).await.expect("read response");
        serde_json::from_slice(&frame).expect("parse response")
    }
}
