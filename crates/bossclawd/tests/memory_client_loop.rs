//! U4 — the safe read+write loop over the REAL daemon socket, driven as a `MemoryClient` (the same
//! role the `air-memory-mcp` adapter uses). Proves: (1) recall works, (2) remember→recall
//! round-trips, (3) a destructive op is refused. Hermetic engine, onboarded fixture. Unix-only.
#![cfg(unix)]

use std::path::PathBuf;

use bossclawd::server;
use bossclawd_proto::{
    read_frame, write_frame, Hello, HelloOk, OpErrorKindWire, Request, Response, Role, PROTO_VERSION,
};
use tokio::net::UnixStream;

struct Guest {
    stream: UnixStream,
}

impl Guest {
    async fn connect(sock: &std::path::Path) -> Self {
        let mut stream = UnixStream::connect(sock).await.expect("connect");
        let hello = Hello { proto_version: PROTO_VERSION, role: Role::MemoryClient };
        write_frame(&mut stream, &serde_json::to_vec(&hello).unwrap()).await.unwrap();
        let hello_ok: HelloOk =
            serde_json::from_slice(&read_frame(&mut stream).await.unwrap()).unwrap();
        assert_eq!(hello_ok.proto_version, PROTO_VERSION);
        Self { stream }
    }
    async fn call(&mut self, req: Request) -> Response {
        write_frame(&mut self.stream, &serde_json::to_vec(&req).unwrap()).await.unwrap();
        serde_json::from_slice(&read_frame(&mut self.stream).await.unwrap()).unwrap()
    }
}

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
    let sock = home.join("bossclawd.sock");
    server::spawn_for_test(sock.clone(), home).await;
    (dir, sock)
}

#[tokio::test]
async fn memory_client_full_loop() {
    let (_dir, sock) = spawn_onboarded_daemon().await;
    let mut guest = Guest::connect(&sock).await;

    // (1) Recall on an empty brain is a clean empty result (not an error).
    match guest.call(Request::Recall { onboarded: true, query: "anything".into(), k: 5 }).await {
        Response::Recall(hits) => assert!(hits.is_empty(), "empty brain recalls nothing"),
        other => panic!("expected Recall, got {other:?}"),
    }

    // (2) Remember → the next recall surfaces it.
    let id = match guest.call(Request::Remember { onboarded: true, text: "kwang ships air".into() }).await {
        Response::Remember(id) => id,
        other => panic!("expected Remember, got {other:?}"),
    };
    match guest.call(Request::Recall { onboarded: true, query: "kwang air".into(), k: 5 }).await {
        Response::Recall(hits) => {
            assert!(hits.iter().any(|h| h.hit.event_id == id && h.text.contains("kwang ships air")));
        }
        other => panic!("expected Recall, got {other:?}"),
    }

    // (3) A destructive op is refused for the guest role.
    assert!(
        matches!(
            guest.call(Request::Teardown).await,
            Response::Err { kind: OpErrorKindWire::NotPermitted, .. }
        ),
        "Teardown is refused for MemoryClient"
    );
}
