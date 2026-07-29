//! U2 — per-op authorization at the daemon boundary. Drives a REAL `bossclawd` accept loop over a
//! temp Unix socket (`server::spawn_for_test`) with a hermetic engine (in-memory vault + mock
//! providers, NEVER the OS keychain). A `MemoryClient`-role connection must be REFUSED every op
//! outside `{Recall, Remember}` (fail-closed) and ALLOWED those two; an `App`-role connection is
//! allowed everything. Unix-only (the daemon + socket are Unix-only).
#![cfg(unix)]

use std::path::PathBuf;

use bossclawd::server;
use bossclawd_proto::types::ResolveActionWire;
use bossclawd_proto::{
    read_frame, write_frame, ExportSelectionWire, Hello, HelloOk, OpErrorKindWire, Request, Response,
    RetireTarget, Role, MAX_FRAME, PROTO_VERSION,
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
    // Rung 3 retire ops (§7.3) are App-only — a guest is refused over the real socket, not just at
    // the two unit-level gates.
    assert!(
        is_not_permitted(
            &c.call(Request::RetireMemory {
                onboarded: true,
                target: RetireTarget::Note { event_id: "x".into() },
            })
            .await
        ),
        "RetireMemory must be refused"
    );
    assert!(
        is_not_permitted(
            &c.call(Request::Unretire { onboarded: true, retired_event_id: "x".into() }).await
        ),
        "Unretire must be refused"
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

/// Rung-3 Phase-3 (I8 relaxation): the SAME `MemoryClient` guest connection MAY drive conflict
/// *resolution* (`ListConflicts` + `ResolveConflict` — confirm-on-a-trusted-surface) yet is STILL
/// refused raw memory *retirement* (`RetireMemory`, App-only). End-to-end allowlist proof over the
/// real socket: the resolve ops reach the engine (permitted), the raw retire is refused at the
/// boundary (NotPermitted, never reaching the engine). Resolution is not a backdoor to direct retire.
#[tokio::test]
async fn memory_client_can_resolve_conflicts_but_still_cannot_retire_directly() {
    let (_dir, sock) = spawn_onboarded_daemon().await;
    let mut c = RoleClient::connect(&sock, Role::MemoryClient).await;

    // ListConflicts is PERMITTED for the guest — an empty list on a fresh brain, crucially NOT a
    // NotPermitted refusal (the op dispatched into the engine).
    let resp = c.call(Request::ListConflicts { onboarded: true }).await;
    assert!(!is_not_permitted(&resp), "guest may ListConflicts, got {resp:?}");
    assert!(matches!(resp, Response::ListConflicts(_)), "guest ListConflicts dispatches, got {resp:?}");

    // ResolveConflict on an unknown id is a clean, TYPED Rejected (permitted op, bad arg): it reached
    // the engine — proving the op is allowed — and the core folded the unknown id to InvalidInput →
    // Rejected. Distinctly NOT NotPermitted and NOT a transport error.
    let resp = c
        .call(Request::ResolveConflict {
            onboarded: true,
            proposal_id: "NOPE".into(),
            action: ResolveActionWire::Dismiss,
        })
        .await;
    assert!(
        matches!(resp, Response::Err { kind: OpErrorKindWire::Rejected, .. }),
        "guest may call resolve; an unknown id folds to the typed Rejected, got {resp:?}"
    );

    // But raw retirement stays App-only: the SAME guest is refused at the boundary (never reaches the
    // engine). This is the negative half of the allowlist.
    let resp = c
        .call(Request::RetireMemory {
            onboarded: true,
            target: RetireTarget::Note { event_id: "e".into() },
        })
        .await;
    assert!(is_not_permitted(&resp), "guest still cannot RetireMemory directly, got {resp:?}");
}

/// R4-A (Task 16): reflection enable/read is App-only OVER THE SOCKET. A `MemoryClient` guest is
/// refused BOTH ops at the role boundary (never reaches the engine — `Role::allows` is unchanged,
/// I8); the `App` gets the full round-trip: default CLOSED, enable acks `Ok`, reads back `true`.
#[tokio::test]
async fn reflect_enable_is_app_only_over_the_socket() {
    let (_tmp, sock) = spawn_onboarded_daemon().await;
    // Guest: refused at the role boundary (never reaches the engine) — Role::allows is UNCHANGED (I8).
    let mut guest = RoleClient::connect(&sock, Role::MemoryClient).await;
    let resp = guest.call(Request::SetReflectEnabled { onboarded: true, enabled: true }).await;
    assert!(is_not_permitted(&resp), "guest cannot enable reflection: {resp:?}");
    let resp = guest.call(Request::ReflectEnabled { onboarded: true }).await;
    assert!(is_not_permitted(&resp), "guest cannot read the reflect flag: {resp:?}");
    // App: full round-trip — default CLOSED, enable acks Ok, reads back true.
    let mut app = RoleClient::connect(&sock, Role::App).await;
    let resp = app.call(Request::ReflectEnabled { onboarded: true }).await;
    assert!(matches!(resp, Response::ReflectEnabled(false)), "default CLOSED, got {resp:?}");
    let resp = app.call(Request::SetReflectEnabled { onboarded: true, enabled: true }).await;
    assert!(matches!(resp, Response::Ok), "App enable acks Ok, got {resp:?}");
    let resp = app.call(Request::ReflectEnabled { onboarded: true }).await;
    assert!(matches!(resp, Response::ReflectEnabled(true)), "reads back enabled, got {resp:?}");
}

/// An empty export selection — the shared fixture for the guest-refusal proof (a guest must be
/// refused at the ROLE boundary, before any argument is even looked at).
fn empty_selection() -> ExportSelectionWire {
    ExportSelectionWire {
        note_event_ids: vec![],
        session_event_ids: vec![],
        ingest_event_ids: vec![],
        description: String::new(),
    }
}

/// Rung-5 §2.4: all THREE export-side ops are App-only OVER THE SOCKET. A `MemoryClient` guest may
/// read memories, but is refused at the role boundary (never reaches the engine) for each of: reading
/// the brain verifying key, writing the identity binding, and sealing a signed export. The `App` side
/// proves the ops are genuinely dispatched (not refused for everyone): `BrainVerifyingKey` returns a
/// real multibase base58btc key.
#[tokio::test]
async fn rung5_export_ops_are_app_only() {
    let (_dir, sock) = spawn_onboarded_daemon().await;
    let mut guest = RoleClient::connect(&sock, Role::MemoryClient).await;
    for req in [
        Request::BrainVerifyingKey { onboarded: true },
        Request::SetBinding { onboarded: true, attestation: serde_json::Value::Null },
        Request::ExportBundle { onboarded: true, selection: empty_selection() },
    ] {
        let label = format!("{req:?}");
        let resp = guest.call(req).await;
        assert!(is_not_permitted(&resp), "guest must be refused {label}, got {resp:?}");
    }
    // Same guest connection still works for its own ops — the refusals above are per-op, not a
    // broken/closed connection.
    let resp = guest.call(Request::Remember { onboarded: true, text: "guest note".into() }).await;
    assert!(matches!(resp, Response::Remember(_)), "guest keeps its own ops, got {resp:?}");

    let mut app = RoleClient::connect(&sock, Role::App).await;
    match app.call(Request::BrainVerifyingKey { onboarded: true }).await {
        Response::BrainVerifyingKey(key) => {
            assert!(key.starts_with('z'), "multibase base58btc key, got {key}")
        }
        other => panic!("App BrainVerifyingKey should succeed, got {other:?}"),
    }
}

/// Characterization backing the daemon's authoritative pre-frame guard: the wire frame is
/// `serde_json::to_vec(&Response::Bundle(text))`, which JSON-ESCAPES the already-JSON `.airmem`.
/// That escaping overhead is far larger than the 2 MiB gap between core's `MAX_EXPORT_BYTES`
/// (30 MiB) and `MAX_FRAME` (32 MiB), so a bundle that passed BOTH the pre-build estimate AND core's
/// own byte belt can still overflow the frame — which is exactly why `export_dispatch` bounds the
/// SERIALIZED response length and answers with the typed `BundleTooLarge` rather than letting a
/// generic frame error replace it.
#[test]
fn wire_frame_escaping_can_overflow_a_sub_cap_bundle() {
    const CORE_BELT: usize = 30 * 1024 * 1024; // bossclaw_core::log::MAX_EXPORT_BYTES
    // Quote/backslash-heavy JSON text: every byte doubles under JSON escaping (worst case).
    let text = "\\\"".repeat(CORE_BELT / 2);
    assert!(text.len() <= CORE_BELT, "raw text is within core's byte belt");
    let wire = serde_json::to_vec(&Response::Bundle(text)).unwrap();
    assert!(
        wire.len() > MAX_FRAME,
        "escaped wire frame ({} bytes) must overflow MAX_FRAME ({MAX_FRAME}) — the dispatch has to \
         refuse with the typed BundleTooLarge, not a generic frame error",
        wire.len()
    );
}
