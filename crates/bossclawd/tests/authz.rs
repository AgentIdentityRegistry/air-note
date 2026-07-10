//! U2 — per-op authorization at the daemon boundary. Drives a REAL `bossclawd` accept loop over a
//! temp Unix socket (`server::spawn_for_test`) with a hermetic engine (in-memory vault + mock
//! providers, NEVER the OS keychain). A `MemoryClient`-role connection must be REFUSED every op
//! outside `{Recall, Remember}` (fail-closed) and ALLOWED those two; an `App`-role connection is
//! allowed everything. Unix-only (the daemon + socket are Unix-only).
#![cfg(unix)]

use std::path::PathBuf;

use bossclawd::server;
use bossclawd_proto::{
    read_frame, write_frame, Hello, HelloOk, OpErrorKindWire, Request, Response, Role, PROTO_VERSION,
};
use tokio::net::UnixStream;

/// A connected test client that handshakes with a chosen `Role`, then speaks framed `Request`s.
struct RoleClient {
    stream: UnixStream,
}

impl RoleClient {
    async fn connect(sock_path: &std::path::Path, role: Role) -> Self {
        let mut stream = UnixStream::connect(sock_path).await.expect("connect to daemon socket");
        let hello = Hello { proto_version: PROTO_VERSION, role };
        write_frame(&mut stream, &serde_json::to_vec(&hello).unwrap()).await.expect("send Hello");
        let reply = read_frame(&mut stream).await.expect("read HelloOk");
        let hello_ok: HelloOk = serde_json::from_slice(&reply).expect("parse HelloOk");
        assert_eq!(hello_ok.proto_version, PROTO_VERSION, "daemon speaks our version");
        Self { stream }
    }

    async fn call(&mut self, req: Request) -> Response {
        write_frame(&mut self.stream, &serde_json::to_vec(&req).unwrap()).await.expect("send req");
        let frame = read_frame(&mut self.stream).await.expect("read resp");
        serde_json::from_slice(&frame).expect("parse resp")
    }
}

/// Spawn a daemon on a fresh temp socket + temp home. Writes a valid `identity.json` into the home
/// so the daemon's `MemoryClient` onboarding recompute reports ONBOARDED (the happy fixture).
async fn spawn_onboarded_daemon() -> (tempfile::TempDir, PathBuf) {
    bossclawd::vault::seed_secret_cache_for_test(Default::default());
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().to_path_buf();
    std::fs::write(
        home.join("identity.json"),
        serde_json::json!({
            "did": "did:wba:example.com:tester",
            "name": "Tester",
            "created_at": "2026-07-09T00:00:00+00:00"
        })
        .to_string(),
    )
    .unwrap();
    let sock_path = home.join("bossclawd.sock");
    server::spawn_for_test(sock_path.clone(), home).await;
    (dir, sock_path)
}

fn is_not_permitted(resp: &Response) -> bool {
    matches!(resp, Response::Err { kind: OpErrorKindWire::NotPermitted, .. })
}

/// Every destructive / egress / mutation op is REFUSED for `MemoryClient` (fail-closed).
#[tokio::test]
async fn memory_client_is_refused_destructive_ops() {
    let (_dir, sock) = spawn_onboarded_daemon().await;
    let mut c = RoleClient::connect(&sock, Role::MemoryClient).await;

    // Teardown (identity reset — the most destructive op).
    assert!(is_not_permitted(&c.call(Request::Teardown).await), "Teardown must be refused");
    // EnableCloudReasoner (network egress).
    assert!(
        is_not_permitted(
            &c.call(Request::EnableCloudReasoner {
                onboarded: true,
                config: serde_json::json!({"mode": "cloud"}),
            })
            .await
        ),
        "EnableCloudReasoner must be refused"
    );
    // A grant op (opens a folder to ingestion).
    assert!(
        is_not_permitted(
            &c.call(Request::AddGrant { onboarded: true, path: PathBuf::from("/etc") }).await
        ),
        "AddGrant must be refused"
    );
    // A model op (triggers a 500MB+ download + re-embed migration).
    assert!(
        is_not_permitted(
            &c.call(Request::SetActiveModel {
                onboarded: true,
                model_id: "x".to_string(),
                safetensors_sha: "y".to_string(),
            })
            .await
        ),
        "SetActiveModel must be refused"
    );
    // FAIL-CLOSED: a plain read op that is NOT on the allowlist is still refused.
    assert!(
        is_not_permitted(&c.call(Request::Status { onboarded: true }).await),
        "Status is not on the MemoryClient allowlist → refused (fail-closed default)"
    );
}

/// `MemoryClient` is ALLOWED exactly `Recall` + `Remember`.
#[tokio::test]
async fn memory_client_is_allowed_recall_and_remember() {
    let (_dir, sock) = spawn_onboarded_daemon().await;
    let mut c = RoleClient::connect(&sock, Role::MemoryClient).await;

    let id = match c.call(Request::Remember { onboarded: true, text: "guest note".into() }).await {
        Response::Remember(id) => id,
        other => panic!("Remember must be allowed, got {other:?}"),
    };
    assert!(!id.is_empty());

    let hits = match c.call(Request::Recall { onboarded: true, query: "guest".into(), k: 5 }).await {
        Response::Recall(hits) => hits,
        other => panic!("Recall must be allowed, got {other:?}"),
    };
    assert!(hits.iter().any(|h| h.hit.event_id == id), "recall surfaces the just-remembered note");
}

/// `App` (the default role) is allowed every op — the existing app is unchanged.
#[tokio::test]
async fn app_role_is_allowed_all_ops() {
    let (_dir, sock) = spawn_onboarded_daemon().await;
    let mut c = RoleClient::connect(&sock, Role::App).await;
    // A representative destructive op the MemoryClient is refused: for App it dispatches normally
    // (a fresh brain teardown succeeds — the point is it is NOT NotPermitted).
    let resp = c.call(Request::Status { onboarded: true }).await;
    assert!(matches!(resp, Response::Status(_)), "App may call Status, got {resp:?}");
    let resp = c.call(Request::AddGrant { onboarded: true, path: std::env::temp_dir() }).await;
    assert!(!is_not_permitted(&resp), "App is never NotPermitted, got {resp:?}");
}

/// The mint footgun (security): a `MemoryClient` CANNOT assert onboarding to force a keystore mint.
/// Against a daemon whose home has NO identity.json, recall/remember return NotOnboarded even though
/// the client sends `onboarded: true` — the daemon recomputes onboarding itself for the guest role.
#[tokio::test]
async fn memory_client_cannot_force_onboarding() {
    bossclawd::vault::seed_secret_cache_for_test(Default::default());
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().to_path_buf(); // NO identity.json written.
    let sock = home.join("bossclawd.sock");
    server::spawn_for_test(sock.clone(), home).await;

    let mut c = RoleClient::connect(&sock, Role::MemoryClient).await;
    // Client lies `onboarded: true`; the daemon overrides it with its own (false) check.
    assert!(
        matches!(c.call(Request::Recall { onboarded: true, query: "x".into(), k: 1 }).await, Response::NotOnboarded),
        "guest recall on a not-onboarded brain → NotOnboarded (no mint)"
    );
    assert!(
        matches!(c.call(Request::Remember { onboarded: true, text: "x".into() }).await, Response::NotOnboarded),
        "guest remember on a not-onboarded brain → NotOnboarded (no mint)"
    );
}
