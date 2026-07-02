//! Integration tests: real round-trips over a REAL temp Unix socket against a live `bossclawd`
//! accept loop (`server::spawn_for_test`) backed by a temp engine home + in-memory vault +
//! `bossclaw_core` mock embedder/reasoner. NEVER touches the OS keychain (a keychain-ACL prompt
//! hangs CI forever — a hard project lesson).
//!
//! One helper (`Client`) does the `Hello` handshake once and then sends `Request`s / reads
//! `Response`s frame-by-frame, exactly as the desktop `SocketTransport` will (Task 5). The
//! `onboarded` gate is per-request, so each test drives onboarded/not-onboarded via the flag.
//!
//! These tests are Unix-only (the daemon + socket are Unix-only).
#![cfg(unix)]

use std::path::PathBuf;

use bossclawd::server;
use bossclawd_proto::{read_frame, write_frame, Hello, HelloOk, Request, Response, PROTO_VERSION};
use tokio::net::UnixStream;

/// A connected test client: holds one `UnixStream` and speaks the framed protocol over it.
struct Client {
    stream: UnixStream,
}

impl Client {
    /// Connect to `sock_path` and perform the `Hello`/`HelloOk` handshake, asserting the daemon
    /// answers with a matching-version `HelloOk`. Returns the ready client.
    async fn connect(sock_path: &std::path::Path) -> Self {
        let mut stream = UnixStream::connect(sock_path).await.expect("connect to daemon socket");
        // Handshake: send Hello, expect HelloOk.
        let hello = Hello { proto_version: PROTO_VERSION };
        write_frame(&mut stream, &serde_json::to_vec(&hello).unwrap()).await.expect("send Hello");
        let reply = read_frame(&mut stream).await.expect("read HelloOk");
        let hello_ok: HelloOk = serde_json::from_slice(&reply).expect("parse HelloOk");
        assert_eq!(hello_ok.proto_version, PROTO_VERSION, "daemon speaks our protocol version");
        assert!(hello_ok.pid > 0, "HelloOk carries the daemon pid");
        Self { stream }
    }

    /// Send one `Request` and read its `Response`.
    async fn call(&mut self, req: Request) -> Response {
        write_frame(&mut self.stream, &serde_json::to_vec(&req).unwrap()).await.expect("send request");
        let frame = read_frame(&mut self.stream).await.expect("read response");
        serde_json::from_slice(&frame).expect("parse response")
    }
}

/// Spin a daemon on a fresh temp socket + temp engine home, returning the tempdir guard (keeps the
/// socket + home alive) and the socket path. The daemon's socket is UNDER the tempdir so it is
/// cleaned up with it.
async fn spawn_daemon() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let sock_path = dir.path().join("bossclawd.sock");
    let home = dir.path().to_path_buf();
    server::spawn_for_test(sock_path.clone(), home).await;
    (dir, sock_path)
}

// ── The specced RED (Step 1): status round-trips over a real socket. ──

#[tokio::test]
async fn status_roundtrip_over_socket() {
    let (_dir, sock) = spawn_daemon().await;
    // Socket confidentiality: the daemon binds the socket 0600 (owner-only). TESTED over the real
    // socket file (the `spawn_for_test` helper chmods it identically to production `bind_socket_0600`).
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&sock).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "socket must be 0600, got {mode:o}");
    }
    let mut client = Client::connect(&sock).await;
    // Onboarded=true → a fresh brain opens Ready. (The handshake already proved HelloOk.)
    let resp = client.call(Request::Status { onboarded: true }).await;
    match resp {
        Response::Status(s) => {
            // A fresh brain primes 3 autonomy switches off → exactly 3 config events, chain intact.
            assert!(s.chain_ok, "fresh brain chain verifies");
            assert_eq!(s.event_count, 3, "prime_switches wrote the 3 sticky config events");
        }
        other => panic!("expected Status, got {other:?}"),
    }
}

// ── NotOnboarded round-trip: the signal crosses the wire (not a fault). ──

#[tokio::test]
async fn not_onboarded_roundtrip() {
    let (_dir, sock) = spawn_daemon().await;
    let mut client = Client::connect(&sock).await;
    // A gated op with onboarded=false must return the NotOnboarded SIGNAL, not Err.
    let resp = client.call(Request::ListGrants { onboarded: false }).await;
    assert!(matches!(resp, Response::NotOnboarded), "gated op not-onboarded → NotOnboarded, got {resp:?}");
    // Status with onboarded=false is never-erroring: a Status with the NotOnboarded state.
    let resp = client.call(Request::Status { onboarded: false }).await;
    match resp {
        Response::Status(s) => assert_eq!(
            s.state,
            bossclawd_proto::types::EngineStateWire::NotOnboarded,
            "status reflects not-onboarded state"
        ),
        other => panic!("expected Status, got {other:?}"),
    }
}

// ── List ops family: grants + writable + files round-trip. ──

#[tokio::test]
async fn list_ops_roundtrip() {
    let (_dir, sock) = spawn_daemon().await;
    let src = tempfile::tempdir().unwrap();
    let mut client = Client::connect(&sock).await;

    // Empty at first.
    assert!(matches!(client.call(Request::ListGrants { onboarded: true }).await, Response::ListGrants(g) if g.is_empty()));
    assert!(matches!(client.call(Request::ListWritable { onboarded: true }).await, Response::ListWritable(w) if w.is_empty()));
    assert!(matches!(client.call(Request::ListFiles { onboarded: true }).await, Response::ListFiles(f) if f.is_empty()));

    // Add a read grant → it appears (active, un-revoked).
    let resp = client.call(Request::AddGrant { onboarded: true, path: src.path().to_path_buf() }).await;
    assert!(matches!(resp, Response::Ok), "AddGrant → Ok, got {resp:?}");
    match client.call(Request::ListGrants { onboarded: true }).await {
        Response::ListGrants(g) => {
            assert_eq!(g.len(), 1);
            assert!(!g[0].revoked);
        }
        other => panic!("expected ListGrants, got {other:?}"),
    }

    // Set the folder writable → ListWritable reflects the canonical root.
    let resp = client
        .call(Request::SetFolderWritable { onboarded: true, path: src.path().to_path_buf(), on: true })
        .await;
    assert!(matches!(resp, Response::Ok));
    let canonical = std::fs::canonicalize(src.path()).unwrap().to_string_lossy().to_string();
    match client.call(Request::ListWritable { onboarded: true }).await {
        Response::ListWritable(w) => assert!(w.contains(&canonical), "writable root listed"),
        other => panic!("expected ListWritable, got {other:?}"),
    }

    // Revoke the grant → it stays listed but revoked.
    let resp = client.call(Request::RevokeGrant { onboarded: true, path: src.path().to_path_buf() }).await;
    assert!(matches!(resp, Response::Ok));
    match client.call(Request::ListGrants { onboarded: true }).await {
        Response::ListGrants(g) => assert!(g[0].revoked, "revoked grant is flagged"),
        other => panic!("expected ListGrants, got {other:?}"),
    }
}

// ── Run-ingest family: grant a folder with files, ingest, list files. ──

#[tokio::test]
async fn run_ingest_roundtrip() {
    let (_dir, sock) = spawn_daemon().await;
    let src = tempfile::tempdir().unwrap();
    std::fs::write(src.path().join("a.txt"), "the quick brown fox").unwrap();
    std::fs::write(src.path().join("b.md"), "# notes\nhello world").unwrap();
    let mut client = Client::connect(&sock).await;

    assert!(matches!(
        client.call(Request::AddGrant { onboarded: true, path: src.path().to_path_buf() }).await,
        Response::Ok
    ));
    match client.call(Request::RunIngest { onboarded: true }).await {
        Response::RunIngest(report) => {
            assert_eq!(report.ingested, 2, "both files ingested");
            assert!(report.failed.is_empty());
        }
        other => panic!("expected RunIngest, got {other:?}"),
    }
    match client.call(Request::ListFiles { onboarded: true }).await {
        Response::ListFiles(files) => assert_eq!(files.len(), 2, "two files tracked"),
        other => panic!("expected ListFiles, got {other:?}"),
    }
}

// ── Recall family: ingest text, recall it, get the hydrated snippet. ──

#[tokio::test]
async fn recall_roundtrip() {
    let (_dir, sock) = spawn_daemon().await;
    let src = tempfile::tempdir().unwrap();
    std::fs::write(src.path().join("a.txt"), "ferris the crab loves rust").unwrap();
    let mut client = Client::connect(&sock).await;

    assert!(matches!(
        client.call(Request::AddGrant { onboarded: true, path: src.path().to_path_buf() }).await,
        Response::Ok
    ));
    assert!(matches!(client.call(Request::RunIngest { onboarded: true }).await, Response::RunIngest(_)));
    match client.call(Request::Recall { onboarded: true, query: "ferris crab".into(), k: 5 }).await {
        Response::Recall(hits) => {
            assert!(hits.iter().any(|h| h.text.contains("ferris")), "recall hydrates the ingested snippet");
        }
        other => panic!("expected Recall, got {other:?}"),
    }
}

// ── Evolve family: status + toggles + a manual tick. ──

#[tokio::test]
async fn evolve_roundtrip() {
    let (_dir, sock) = spawn_daemon().await;
    let mut client = Client::connect(&sock).await;

    // EvolveStatus: fresh brain, evolve primed OFF.
    match client.call(Request::EvolveStatus { onboarded: true }).await {
        Response::EvolveStatus { status, telemetry } => {
            assert!(!status.enabled, "evolve starts disabled (prime_switches)");
            assert_eq!(telemetry.error_count, 0, "no ticks yet");
        }
        other => panic!("expected EvolveStatus, got {other:?}"),
    }

    // Toggle evolve/proposals/mandates on; read the mandates flag back.
    assert!(matches!(
        client.call(Request::SetEvolveEnabled { onboarded: true, enabled: true }).await,
        Response::Ok
    ));
    assert!(matches!(
        client.call(Request::SetProposalsEnabled { onboarded: true, enabled: true }).await,
        Response::Ok
    ));
    assert!(matches!(
        client.call(Request::SetMandatesEnabled { onboarded: true, enabled: true }).await,
        Response::Ok
    ));
    assert!(matches!(client.call(Request::MandatesEnabled { onboarded: true }).await, Response::MandatesEnabled(true)));
    match client.call(Request::EvolveStatus { onboarded: true }).await {
        Response::EvolveStatus { status, .. } => assert!(status.enabled, "the toggle took effect"),
        other => panic!("expected EvolveStatus, got {other:?}"),
    }

    // EvolveOnce over an EMPTY queue is a valid no-op tick (0 processed) → an EvolveOnce report.
    match client.call(Request::EvolveOnce { onboarded: true }).await {
        Response::EvolveOnce(report) => assert_eq!(report.memories_processed, 0, "empty-queue tick processes nothing"),
        other => panic!("expected EvolveOnce, got {other:?}"),
    }
}

// ── Grant/mandate mutation family: add → list → activity → revoke. ──

#[tokio::test]
async fn mandate_mutations_roundtrip() {
    let (_dir, sock) = spawn_daemon().await;
    // A mandate target must be WRITE-granted AND outside every read root; the read-granted `scope`
    // holds the sources. (Mirrors the engine unit test `mandate_crud_round_trip_*`.)
    let dest = tempfile::tempdir().unwrap();
    let scope = tempfile::tempdir().unwrap();
    let target = dest.path().join("synced.md");
    std::fs::write(&target, b"x\n").unwrap();
    let mut client = Client::connect(&sock).await;

    // Set up: write-grant the dest, read-grant the scope.
    assert!(matches!(
        client.call(Request::SetFolderWritable { onboarded: true, path: dest.path().to_path_buf(), on: true }).await,
        Response::Ok
    ));
    assert!(matches!(
        client.call(Request::AddGrant { onboarded: true, path: scope.path().to_path_buf() }).await,
        Response::Ok
    ));

    // Add a mandate → AddMandate summary.
    let mandate_id = match client
        .call(Request::AddMandate {
            onboarded: true,
            target: target.clone(),
            source_scope: scope.path().to_path_buf(),
            recipe: "keep it synced".into(),
        })
        .await
    {
        Response::AddMandate(m) => {
            assert_eq!(m.recipe, "keep it synced");
            assert!(!m.revoked);
            m.mandate_grant_id
        }
        other => panic!("expected AddMandate, got {other:?}"),
    };

    // List mandates → one.
    match client.call(Request::ListMandates { onboarded: true }).await {
        Response::ListMandates(ms) => assert_eq!(ms.len(), 1),
        other => panic!("expected ListMandates, got {other:?}"),
    }
    // Mandate writes → none yet.
    match client.call(Request::MandateWrites { onboarded: true }).await {
        Response::MandateWrites(ws) => assert!(ws.is_empty()),
        other => panic!("expected MandateWrites, got {other:?}"),
    }

    // Revoke → list empty.
    assert!(matches!(
        client.call(Request::RevokeMandate { onboarded: true, mandate_grant_id: mandate_id }).await,
        Response::Ok
    ));
    match client.call(Request::ListMandates { onboarded: true }).await {
        Response::ListMandates(ms) => assert!(ms.is_empty(), "revoked → no active mandates"),
        other => panic!("expected ListMandates, got {other:?}"),
    }
}

// ── Reasoner config family: get default → set → read back; ready is fail-closed. ──

#[tokio::test]
async fn reasoner_config_roundtrip() {
    let (_dir, sock) = spawn_daemon().await;
    let mut client = Client::connect(&sock).await;

    // Default is Local (fail-safe).
    match client.call(Request::GetReasonerConfig { onboarded: true }).await {
        Response::ReasonerConfig(c) => {
            assert_eq!(c.mode, bossclawd_proto::types::ReasonerModeWire::Local, "default is Local");
        }
        other => panic!("expected ReasonerConfig, got {other:?}"),
    }
    // A Local config is not "cloud-ready" (this op answers cloud readiness; Local → false).
    assert!(matches!(
        client.call(Request::GetReasonerReady { onboarded: true }).await,
        Response::ReasonerReady(false)
    ));

    // Set a Cloud config (config only — NO consent granted, so it can't egress) and read it back.
    let resp = client
        .call(Request::SetReasonerConfig {
            onboarded: true,
            config: serde_json::json!({"mode":"cloud","provider":"anthropic","model":"claude-sonnet-4-6","base_url":null}),
        })
        .await;
    assert!(matches!(resp, Response::Ok), "SetReasonerConfig → Ok, got {resp:?}");
    match client.call(Request::GetReasonerConfig { onboarded: true }).await {
        Response::ReasonerConfig(c) => {
            assert_eq!(c.mode, bossclawd_proto::types::ReasonerModeWire::Cloud, "cloud config persisted");
            assert_eq!(c.model, "claude-sonnet-4-6");
        }
        other => panic!("expected ReasonerConfig, got {other:?}"),
    }
    // Cloud WITHOUT signed consent → still not ready (fail-closed R1). No egress possible.
    assert!(matches!(
        client.call(Request::GetReasonerReady { onboarded: true }).await,
        Response::ReasonerReady(false)
    ));
}
