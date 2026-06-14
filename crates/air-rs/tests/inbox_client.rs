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
async fn terminates_when_event_receiver_is_dropped_without_stop() {
    // D1: a caller that drops the event receiver without calling `stop()` must NOT leave the
    // reconnect loop spinning forever. After the receiver is gone, the next Message emit fails and
    // the loop terminates — so when the (still-open) first session ends, there is NO reconnect.
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("daemon.sock");
    let listener = UnixListener::bind(&sock).unwrap();

    let (accepted_tx, accepted_rx) = std::sync::mpsc::channel::<()>();
    let server = tokio::spawn(async move {
        let (s1, _) = listener.accept().await.unwrap();
        let (rd, mut wr) = s1.into_split();
        let mut reader = BufReader::new(rd);
        read_line(&mut reader).await; // hello
        wr.write_all(b"{\"type\":\"hello-ok\",\"pid\":1,\"start_time\":\"t\",\"did\":\"did:me\"}\n")
            .await
            .unwrap();
        wr.write_all(b"{\"type\":\"message\",\"message\":{\"seq\":7,\"relay_seq\":7,\"envelope_id\":\"e7\",\"from\":\"did:peer\",\"verified\":true,\"encrypted\":true,\"received_at\":\"t\"}}\n")
            .await
            .unwrap();
        // Give the client time to deliver e7 (which the test consumes, then drops rx). A second
        // message then hits a dropped receiver → the loop should terminate (D1).
        tokio::time::sleep(Duration::from_millis(200)).await;
        wr.write_all(b"{\"type\":\"message\",\"message\":{\"seq\":8,\"relay_seq\":8,\"envelope_id\":\"e8\",\"from\":\"did:peer\",\"verified\":true,\"encrypted\":true,\"received_at\":\"t\"}}\n")
            .await
            .unwrap();
        // End session 1. A still-spinning loop (the bug) would reconnect here; the fixed loop won't.
        drop(wr);
        drop(reader);
        // If a reconnect happens, this accept resolves and we signal it. With the fix it never does.
        if listener.accept().await.is_ok() {
            let _ = accepted_tx.send(());
        }
    });

    let (tx, mut rx) = mpsc::unbounded_channel::<InboxEvent>();
    let _handle = connect_persistent(
        ClientConfig {
            socket_path: sock,
            role: Role::Channel,
            baseline: None,
        },
        tx,
    );
    assert!(matches!(rx.recv().await.unwrap(), InboxEvent::Attached { .. }));
    match rx.recv().await.unwrap() {
        InboxEvent::Message(m) => assert_eq!(m.envelope_id, "e7"),
        e => panic!("{e:?}"),
    }
    // Drop the receiver WITHOUT stopping the handle — the loop must self-terminate (D1).
    drop(rx);

    // No reconnection must occur within a generous window (the loop is gone, not respawning).
    std::thread::sleep(Duration::from_millis(900));
    assert!(
        accepted_rx.try_recv().is_err(),
        "loop must NOT reconnect after the event receiver was dropped"
    );
    server.abort();
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

#[tokio::test]
async fn a_send_concurrent_with_a_split_inbound_frame_does_not_corrupt_it() {
    // Regression for review I1: the post-handshake select! loop must NOT clear `buf` at the top of
    // each iteration. A multi-segment inbound frame interrupted by a concurrent `send` would lose
    // its first segment (the cancelled read_until's partial bytes), corrupting/dropping the message.
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("daemon.sock");
    let listener = UnixListener::bind(&sock).unwrap();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (rd, mut wr) = stream.into_split();
        let mut reader = BufReader::new(rd);
        read_line(&mut reader).await; // hello
        wr.write_all(b"{\"type\":\"hello-ok\",\"pid\":1,\"start_time\":\"t\",\"did\":\"did:me\"}\n").await.unwrap();
        // First HALF of a message frame (no newline), flushed — the client reads it into `buf` and
        // a concurrent send will interrupt the in-flight read_until.
        wr.write_all(b"{\"type\":\"message\",\"message\":{\"seq\":9,\"relay_seq\":9,\"envelope_id\":\"split-9\",\"from\":\"did:peer\",").await.unwrap();
        wr.flush().await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await; // window for the send to interleave
        let send = read_line(&mut reader).await; // the client's queued send
        assert_eq!(send["type"], "send");
        // Second half + newline — completes the frame the client must reassemble.
        wr.write_all(b"\"verified\":true,\"encrypted\":true,\"received_at\":\"t\"}}\n").await.unwrap();
        wr.flush().await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
    });

    let (tx, mut rx) = mpsc::unbounded_channel::<InboxEvent>();
    let handle = connect_persistent(
        ClientConfig { socket_path: sock, role: Role::Viewer, baseline: None },
        tx,
    );
    assert!(matches!(rx.recv().await.unwrap(), InboxEvent::Attached { .. }));
    // Let the client read the partial first half into `buf`, THEN send — interrupting read_until.
    tokio::time::sleep(Duration::from_millis(100)).await;
    handle.send_frame(ClientFrame::Send {
        id: "s1".into(), to: "did:peer".into(), body: json!({"type":"text","text":"hi"}),
        plaintext: None, thread_id: None, in_reply_to: None,
    });
    // The split frame must arrive INTACT (reassembled across the interruption). Without the fix the
    // first half is dropped, the frame is unparseable, and no Message event ever arrives → timeout.
    let got = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let InboxEvent::Message(m) = rx.recv().await.unwrap() {
                return m;
            }
        }
    })
    .await
    .expect("split message must arrive intact within 3s");
    assert_eq!(got.envelope_id, "split-9");
    handle.stop();
    let _ = server.await;
}
