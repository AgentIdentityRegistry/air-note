use air_rs::inbox::client::{connect_persistent, ClientConfig, InboxEvent, Role};
use air_rs::inbox::frames::ClientFrame;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::mpsc;

async fn read_line(reader: &mut (impl AsyncBufReadExt + Unpin)) -> Value {
    let mut buf = Vec::new();
    reader.read_until(b'\n', &mut buf).await.unwrap();
    serde_json::from_slice(&buf).unwrap()
}

#[tokio::test]
async fn attaches_sends_and_receives() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("daemon.sock");
    let listener = UnixListener::bind(&sock).unwrap();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (rd, mut wr) = stream.into_split();
        let mut reader = BufReader::new(rd);
        let hello = read_line(&mut reader).await;
        assert_eq!(hello["type"], "hello");
        assert_eq!(hello["role"], "viewer");
        assert!(
            hello.get("since_seq").is_none(),
            "first attach sends no since_seq"
        );
        wr.write_all(b"{\"type\":\"hello-ok\",\"pid\":4242,\"start_time\":\"t\",\"did\":\"did:me\"}\n")
            .await
            .unwrap();
        wr.write_all(b"{\"type\":\"message\",\"message\":{\"seq\":7,\"relay_seq\":7,\"envelope_id\":\"e7\",\"from\":\"did:peer\",\"verified\":true,\"encrypted\":true,\"received_at\":\"t\"}}\n")
            .await
            .unwrap();
        let send = read_line(&mut reader).await;
        assert_eq!(send["type"], "send");
        let id = send["id"].as_str().unwrap();
        wr.write_all(
            format!(
                "{{\"type\":\"send-ok\",\"id\":\"{id}\",\"envelope_id\":\"relay-1\",\"encrypted\":true}}\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
    });

    let (tx, mut rx) = mpsc::unbounded_channel::<InboxEvent>();
    let handle = connect_persistent(
        ClientConfig {
            socket_path: sock,
            role: Role::Viewer,
            baseline: None,
        },
        tx,
    );

    assert!(matches!(
        rx.recv().await.unwrap(),
        InboxEvent::Attached { pid: 4242, .. }
    ));
    match rx.recv().await.unwrap() {
        InboxEvent::Message(m) => assert_eq!(m.envelope_id, "e7"),
        e => panic!("{e:?}"),
    }
    handle.send_frame(ClientFrame::Send {
        id: "corr-1".into(),
        to: "did:peer".into(),
        body: json!({"type":"text","text":"hi"}),
        plaintext: None,
        thread_id: None,
        in_reply_to: None,
    });
    loop {
        match rx.recv().await.unwrap() {
            InboxEvent::SendOk { id, envelope_id, .. } => {
                assert_eq!(id, "corr-1");
                assert_eq!(envelope_id, "relay-1");
                break;
            }
            _ => continue,
        }
    }
    handle.stop();
    let _ = server.await;
}

#[tokio::test]
async fn resumes_with_since_seq_after_a_reconnect() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("daemon.sock");
    let listener = UnixListener::bind(&sock).unwrap();

    let server = tokio::spawn(async move {
        let (s1, _) = listener.accept().await.unwrap();
        let (rd, mut wr) = s1.into_split();
        let mut reader = BufReader::new(rd);
        let hello1 = read_line(&mut reader).await;
        assert!(hello1.get("since_seq").is_none());
        wr.write_all(b"{\"type\":\"hello-ok\",\"pid\":1,\"start_time\":\"t\",\"did\":\"did:me\"}\n")
            .await
            .unwrap();
        wr.write_all(b"{\"type\":\"message\",\"message\":{\"seq\":7,\"relay_seq\":7,\"envelope_id\":\"e7\",\"from\":\"did:peer\",\"verified\":true,\"encrypted\":true,\"received_at\":\"t\"}}\n")
            .await
            .unwrap();
        drop(wr);
        drop(reader);

        let (s2, _) = listener.accept().await.unwrap();
        let (rd2, mut wr2) = s2.into_split();
        let mut reader2 = BufReader::new(rd2);
        let hello2 = read_line(&mut reader2).await;
        assert_eq!(
            hello2["since_seq"],
            json!(7),
            "resume must send max-seen since_seq"
        );
        wr2.write_all(b"{\"type\":\"hello-ok\",\"pid\":1,\"start_time\":\"t\",\"did\":\"did:me\"}\n")
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
    });

    let (tx, mut rx) = mpsc::unbounded_channel::<InboxEvent>();
    let _h = connect_persistent(
        ClientConfig {
            socket_path: sock,
            role: Role::Channel,
            baseline: Some(3),
        },
        tx,
    );
    let mut attaches = 0;
    while let Some(ev) = rx.recv().await {
        if let InboxEvent::Attached { .. } = ev {
            attaches += 1;
            if attaches == 2 {
                break;
            }
        }
    }
    assert_eq!(attaches, 2);
    let _ = server.await;
}

#[tokio::test]
async fn signals_offline_when_no_daemon_is_listening() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("daemon.sock");
    let (tx, mut rx) = mpsc::unbounded_channel::<InboxEvent>();
    let handle = connect_persistent(
        ClientConfig {
            socket_path: sock,
            role: Role::Viewer,
            baseline: None,
        },
        tx,
    );
    let ev = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("offline within 2s")
        .unwrap();
    assert!(
        matches!(ev, InboxEvent::Offline),
        "expected Offline, got {ev:?}"
    );
    handle.stop();
}
