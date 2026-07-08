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
use bossclawd_proto::{
    read_frame, write_frame, Hello, HelloOk, Request, Response, Role, PROTO_VERSION,
};
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
        let hello = Hello { proto_version: PROTO_VERSION, role: Role::App };
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
///
/// Seeds the process-global provider-key cache to EMPTY first, so ops that read a provider-key
/// fingerprint (`GetReasonerReady` → `current_key_fingerprint` → `vault::secret_get_cached`) never
/// hit the real OS keychain — an unseeded first read blocks on a keychain-ACL prompt and hangs the
/// suite forever (the documented CI-hang hazard). Empty = "no provider key stored", the fixture
/// every test here wants (readiness stays fail-closed false).
async fn spawn_daemon() -> (tempfile::TempDir, PathBuf) {
    bossclawd::vault::seed_secret_cache_for_test(Default::default());
    let dir = tempfile::tempdir().unwrap();
    let sock_path = dir.path().join("bossclawd.sock");
    let home = dir.path().to_path_buf();
    server::spawn_for_test(sock_path.clone(), home).await;
    (dir, sock_path)
}

/// Like [`spawn_daemon`], but RETURNS the shared `Arc<EngineHandle>` so a test can seed engine
/// state (e.g. a write_proposal) directly BEFORE driving ops over the wire. Uses the same public
/// `test_engine` + production `run_accept_loop` as `spawn_for_test`, over a real 0600 temp socket.
async fn spawn_daemon_with_engine(
) -> (tempfile::TempDir, PathBuf, std::sync::Arc<bossclawd::engine::EngineHandle>) {
    use std::os::unix::fs::PermissionsExt;
    use tokio::net::UnixListener;

    bossclawd::vault::seed_secret_cache_for_test(Default::default());
    let dir = tempfile::tempdir().unwrap();
    let sock_path = dir.path().join("bossclawd.sock");
    let engine = std::sync::Arc::new(bossclawd::server::test_engine(dir.path().to_path_buf()));
    // Bind BEFORE returning so a client connect can't race the bind (same contract as spawn_for_test).
    let listener = UnixListener::bind(&sock_path).expect("bind test socket");
    std::fs::set_permissions(&sock_path, std::fs::Permissions::from_mode(0o600))
        .expect("chmod test socket 0600");
    // The PRODUCTION accept loop, sharing the SAME engine Arc we return — so the proposal a test
    // seeds through `engine` is the very one the wire `ApplyProposal` resolves.
    tokio::spawn(bossclawd::server::run_accept_loop(engine.clone(), listener));
    (dir, sock_path, engine)
}

/// Seed ONE open, loud-requiring `write_proposal` into `engine`'s brain and return
/// `(proposal_id, target_path, original_bytes, folder_guard)`. Mirrors the engine's own
/// `apply_proposal_loud_needs_ack_then_applies` recipe: grant+write-grant a temp folder, ingest a
/// target file, then append a Tier-B Edit proposal whose new content is a >=32-char unbroken
/// alphanumeric run (the secret-shaped diff that forces `requires_loud_modal`). The proposed bytes
/// go in the side table so apply can read them back. The returned `TempDir` keeps the folder alive —
/// the caller MUST hold it until after the apply, or the target dir is removed before the write.
async fn seed_loud_proposal(
    engine: &bossclawd::engine::EngineHandle,
) -> (String, std::path::PathBuf, Vec<u8>, tempfile::TempDir) {
    use bossclaw_core::actuator::{WriteOp, WriteProposal};
    use sha2::{Digest, Sha256};

    let log = engine.get_or_open(true).await.expect("open brain");
    let folder = tempfile::tempdir().unwrap();
    log.add_grant(folder.path()).expect("read grant");
    log.add_write_grant(folder.path()).expect("write grant");
    let path = folder.path().join("secrets.md");
    let original = b"placeholder\n".to_vec();
    std::fs::write(&path, &original).unwrap();

    // Ingest the target so the proposal cites a real lineage event (the file_ingested id).
    let embedder = bossclaw_core::embed::MockEmbedder::new(8);
    log.ingest_all(&bossclaw_core::ingest::ParserRouter::native_only(), &embedder).expect("ingest");
    let canonical = std::fs::canonicalize(&path).unwrap().to_string_lossy().to_string();
    let file_id = log
        .current_files()
        .expect("current_files")
        .into_iter()
        .find(|r| r.canonical_path == canonical)
        .map(|r| r.file_event_id)
        .expect("ingested file id");

    // A >=32-char unbroken alphanumeric run trips the secret-shaped diff flag → loud.
    let new_bytes = b"token=ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcd\n".to_vec();
    let hash = hex::encode(Sha256::digest(&new_bytes));
    let gated = log
        .propose_write(WriteProposal {
            target: path.clone(),
            new_content: new_bytes.clone(),
            op: WriteOp::Edit,
            source_event_ids: vec![file_id.clone()],
            rationale: "fix".to_string(),
        })
        .expect("gate the proposal");
    assert!(gated.verdict.requires_loud_modal, "secret-shaped content forces the loud modal");
    let vs = serde_json::json!({
        "requires_loud_modal": gated.verdict.requires_loud_modal,
        "taint": format!("{:?}", gated.verdict.taint),
        "allowed": gated.verdict.allowed,
        "base_content_hash": gated.verdict.base_content_hash,
    });
    let pid = log
        .append_write_proposal(
            &canonical, "edit", &hash, new_bytes.len() as u64, "fix",
            &serde_json::json!({"src":"a","relation":"r","dst":"b"}), &vs,
            std::slice::from_ref(&file_id),
        )
        .expect("append write_proposal");
    log.put_proposal_bytes(&pid, &new_bytes, &hash).expect("store proposal bytes");
    (pid, path, original, folder)
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

// ── Apply-proposal family: the review-apply path (loud-confirm gate + happy path) over the wire. ──
//
// Everything else here proves per-family REQUEST plumbing, but `apply_proposal` — the one op with a
// safety gate (`acknowledged_loud`) — had no wire coverage: the string-parity of `NeedsLoudConfirm`
// is unit-tested app-side, but no test drove the ACTUAL apply over the daemon. This seeds a REAL
// loud-requiring proposal into the daemon's engine, then drives apply over a real socket:
//   • acknowledged_loud=false → the daemon re-gates, sees a loud verdict, and returns the typed
//     `NeedsLoudConfirm` Err across the wire — and the file is UNCHANGED (no silent write);
//   • acknowledged_loud=true  → the same proposal applies and returns an `ApplyResult` carrying the
//     file_written id — and the file now holds the proposed bytes.
// The ack is the ONLY thing that differs between the two calls, so this is load-bearing on
// `acknowledged_loud` crossing the wire and reaching the engine's authoritative loud gate.
#[tokio::test]
async fn apply_proposal_loud_confirm_roundtrip() {
    let (_dir, sock, engine) = spawn_daemon_with_engine().await;
    let (pid, target, original, _folder) = seed_loud_proposal(&engine).await;
    let mut client = Client::connect(&sock).await;

    // 1) No ack → the daemon refuses with the typed NeedsLoudConfirm, and NOTHING is written.
    let resp = client
        .call(Request::ApplyProposal { onboarded: true, id: pid.clone(), acknowledged_loud: false })
        .await;
    match resp {
        Response::Err { kind, .. } => assert_eq!(
            kind,
            bossclawd_proto::OpErrorKindWire::NeedsLoudConfirm,
            "a loud write without ack → NeedsLoudConfirm over the wire",
        ),
        other => panic!("expected Err(NeedsLoudConfirm), got {other:?}"),
    }
    assert_eq!(std::fs::read(&target).unwrap(), original, "no write happened without the ack");

    // 2) With ack → the SAME proposal applies (a loud refuse fires pre-execute, so the proposal was
    //    not consumed by call 1 and is still resolvable) and returns an ApplyResult with a file id.
    let resp = client
        .call(Request::ApplyProposal { onboarded: true, id: pid, acknowledged_loud: true })
        .await;
    match resp {
        Response::ApplyProposal(r) => {
            assert!(!r.file_written_id.is_empty(), "apply returns a file_written id");
        }
        other => panic!("expected ApplyProposal, got {other:?}"),
    }
    assert_eq!(
        std::fs::read(&target).unwrap(),
        b"token=ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcd\n",
        "the ack let the loud write through",
    );
}

// ── test-helpers seam: a caller-supplied embedder provider reaches the engine (memharness
//    Phase 0 needs to inject the PRODUCTION ResourceModel2Vec; this proves the seam with a
//    call-count spy provider the engine is observed consulting). ──

/// A custom provider with a call-count spy: `embedder()` bumps the shared counter, so the test
/// can PROVE the engine consulted the injected provider — a seam that silently dropped the
/// parameter and fell back to the default provider would leave the counter at 0 and fail.
struct WideMockEmbedderProvider {
    calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}
impl bossclawd::engine::embed::EmbedderProvider for WideMockEmbedderProvider {
    fn embedder(
        &self,
    ) -> Result<std::sync::Arc<dyn bossclaw_core::Embedder>, bossclawd::engine::EngineOpError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(std::sync::Arc::new(bossclaw_core::MockEmbedder::new(16)))
    }
}

#[tokio::test]
async fn custom_embedder_provider_reaches_recall_over_the_wire() {
    use std::os::unix::fs::PermissionsExt;
    use tokio::net::UnixListener;

    bossclawd::vault::seed_secret_cache_for_test(Default::default());
    let dir = tempfile::tempdir().unwrap();
    let sock_path = dir.path().join("bossclawd.sock");
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let engine = std::sync::Arc::new(bossclawd::server::test_engine_with_embedder(
        dir.path().to_path_buf(),
        std::sync::Arc::new(WideMockEmbedderProvider { calls: calls.clone() }),
    ));
    let listener = UnixListener::bind(&sock_path).expect("bind test socket");
    std::fs::set_permissions(&sock_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    tokio::spawn(bossclawd::server::run_accept_loop(engine, listener));

    let src = tempfile::tempdir().unwrap();
    std::fs::write(src.path().join("a.txt"), "ferris the crab loves rust").unwrap();
    let mut client = Client::connect(&sock_path).await;
    assert!(matches!(
        client.call(Request::AddGrant { onboarded: true, path: src.path().to_path_buf() }).await,
        Response::Ok
    ));
    assert!(matches!(
        client.call(Request::RunIngest { onboarded: true }).await,
        Response::RunIngest(_)
    ));
    assert!(
        calls.load(std::sync::atomic::Ordering::SeqCst) > 0,
        "the INJECTED provider must be the one the engine consulted"
    );
    match client.call(Request::Recall { onboarded: true, query: "ferris crab".into(), k: 5 }).await {
        Response::Recall(hits) => {
            assert!(hits.iter().any(|h| h.text.contains("ferris")),
                "recall works with the injected provider");
        }
        other => panic!("expected Recall, got {other:?}"),
    }
}

// ── Rung 2: the language-pack ops reach the engine over the socket (A7). ──

/// `ModelStatus` round-trips over the wire: a fresh onboarded daemon with no language pack reports
/// `ModelStateWire::Ok` and no re-index progress (the mock provider's default state).
#[tokio::test]
async fn model_status_roundtrips_over_the_socket() {
    use bossclawd_proto::types::ModelStateWire;

    let (_dir, sock_path) = spawn_daemon().await;
    let mut client = Client::connect(&sock_path).await;
    match client.call(Request::ModelStatus { onboarded: true }).await {
        Response::ModelStatus(w) => {
            assert!(matches!(w.state, ModelStateWire::Ok), "no language pack → Ok, got {:?}", w.state);
            assert!(w.reindex.is_none(), "no migration running → no progress");
        }
        other => panic!("expected ModelStatus, got {other:?}"),
    }
}

/// `SetActiveModel` reaches `EngineHandle::set_active_model` over the wire: against the mock embedder
/// provider (whose `build_candidate` is unsupported) the synchronous validation fails, so the arm
/// surfaces the typed `Embedder` error — proving the dispatch arm is wired through (not that a real
/// migration runs, which needs real weights the hermetic daemon has none of).
#[tokio::test]
async fn set_active_model_reaches_the_engine_over_the_socket() {
    let (_dir, sock_path) = spawn_daemon().await;
    let mut client = Client::connect(&sock_path).await;
    let resp = client
        .call(Request::SetActiveModel {
            onboarded: true,
            model_id: "minishlab/potion-multilingual-128M".into(),
            safetensors_sha: "deadbeef".into(),
        })
        .await;
    match resp {
        Response::Err { kind, .. } => assert_eq!(
            kind,
            bossclawd_proto::OpErrorKindWire::Embedder,
            "the mock provider cannot build a candidate → typed Embedder error",
        ),
        other => panic!("expected Err(Embedder), got {other:?}"),
    }
}

// ── SP1 / M1b: the coding-agent write op reaches the engine over the socket (A2). ──

/// U1 over the wire: a `Remember` writes a memory the very next `Recall` surfaces
/// (the memory is embeddable + the engine invalidates the recall index on write).
///
/// PRE-ARM the index first so this test is LOAD-BEARING on `remember`'s invalidation line.
/// `remember` persists the event + its vector to the DB but touches NEITHER live index — both
/// recall arms (in-memory HNSW vector + FTS keyword) are refreshed only by `rebuild_indexes`,
/// which `ensure_indexed` runs solely while `indexed == false`. On a fresh brain `indexed` starts
/// `false`, so a follow-up `recall` would rebuild regardless and the note would surface even if
/// the invalidation line were deleted (an incidental pass). The warmup `recall` below drives
/// `ensure_indexed` and flips `indexed == true`; from there ONLY `remember`'s
/// `*self.indexed.lock().await = false` forces the rebuild that surfaces the new note (verified by
/// mutation-testing: deleting that line turns this test red).
#[tokio::test]
async fn remember_then_recall_roundtrips_over_socket() {
    let (_dir, sock) = spawn_daemon().await;
    let mut client = Client::connect(&sock).await;

    // Warm the recall index BEFORE writing, so `remember`'s invalidation is the thing under test.
    match client.call(Request::Recall { onboarded: true, query: "warmup".to_string(), k: 5 }).await {
        Response::Recall(_) => {} // empty is fine — the point is `ensure_indexed` ran (indexed=true).
        other => panic!("expected Response::Recall (warmup), got {other:?}"),
    }

    let resp = client
        .call(Request::Remember { onboarded: true, text: "aria novak ships rust".to_string() })
        .await;
    let event_id = match resp {
        Response::Remember(id) => id,
        other => panic!("expected Response::Remember, got {other:?}"),
    };
    assert!(!event_id.is_empty(), "Remember returns the new event id");

    let hits = match client
        .call(Request::Recall { onboarded: true, query: "aria rust".to_string(), k: 5 })
        .await
    {
        Response::Recall(hits) => hits,
        other => panic!("expected Response::Recall, got {other:?}"),
    };
    assert!(
        hits.iter().any(|h| h.hit.event_id == event_id && h.text.contains("aria novak")),
        "the remembered note is recalled with its hydrated snippet"
    );

    // Empty text is rejected (typed Rejected).
    let err = client
        .call(Request::Remember { onboarded: true, text: "   ".to_string() })
        .await;
    assert!(
        matches!(err, Response::Err { kind: bossclawd_proto::OpErrorKindWire::Rejected, .. }),
        "blank remember → Rejected, got {err:?}"
    );
}
