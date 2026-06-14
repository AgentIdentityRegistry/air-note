//! Every fixture frame round-trips through the Rust types byte-for-byte (value-equal).
use air_rs::inbox::frames::{ClientFrame, Message, ServerFrame};
use serde_json::{json, Value};

const FIXTURES: &str =
    include_str!("../../../agent-bridge-mcp/test/fixtures/socket-frames.json");

fn fixtures() -> Value {
    serde_json::from_str(FIXTURES).expect("fixtures parse")
}

#[test]
fn fixture_version_is_1() {
    assert_eq!(fixtures()["version"], json!(1));
}

#[test]
fn client_frames_encode_to_fixtures() {
    let f = fixtures();
    let c = &f["client_to_daemon"];

    let hello = ClientFrame::Hello { role: "viewer".into(), since_seq: None };
    assert_eq!(serde_json::to_value(&hello).unwrap(), c["hello"]);

    let resume = ClientFrame::Hello { role: "channel".into(), since_seq: Some(41) };
    assert_eq!(serde_json::to_value(&resume).unwrap(), c["hello_channel_resume"]);

    assert_eq!(serde_json::to_value(ClientFrame::Ping).unwrap(), c["ping"]);
    assert_eq!(serde_json::to_value(ClientFrame::Status).unwrap(), c["status_request"]);

    let send: ClientFrame = serde_json::from_value(c["send"].clone()).unwrap();
    assert_eq!(serde_json::to_value(&send).unwrap(), c["send"]);
    assert!(matches!(send, ClientFrame::Send { ref plaintext, .. } if plaintext.is_none()));

    let send_pt: ClientFrame = serde_json::from_value(c["send_plaintext"].clone()).unwrap();
    assert_eq!(serde_json::to_value(&send_pt).unwrap(), c["send_plaintext"]);

    let send_reply: ClientFrame = serde_json::from_value(c["send_reply"].clone()).unwrap();
    assert_eq!(serde_json::to_value(&send_reply).unwrap(), c["send_reply"]);
}

#[test]
fn server_frames_decode_and_reencode_to_fixtures() {
    let f = fixtures();
    let d = &f["daemon_to_client"];
    for key in ["hello_ok", "message", "gap", "pong", "status", "send_ok", "send_err", "error"] {
        let frame: ServerFrame = serde_json::from_value(d[key].clone())
            .unwrap_or_else(|e| panic!("decode {key}: {e}"));
        assert!(!matches!(frame, ServerFrame::Unknown), "{key} must be a known frame");
        assert_eq!(serde_json::to_value(&frame).unwrap(), d[key], "{key} re-encode");
    }
}

#[test]
fn message_omits_key_changed_when_unchanged() {
    let d = &fixtures()["daemon_to_client"];
    let ServerFrame::Message { message } =
        serde_json::from_value(d["message"].clone()).unwrap()
    else { panic!("not a message") };
    assert_eq!(message.key_changed, None);
    assert!(!message.key_changed());
    assert_eq!(message.contact.as_deref(), Some("pat"));
    let v = serde_json::to_value(&message).unwrap();
    assert!(v.get("key_changed").is_none());
}

#[test]
fn unknown_frame_type_decodes_to_unknown_not_error() {
    let v = json!({ "type": "future-thing", "whatever": 1 });
    let frame: ServerFrame = serde_json::from_value(v).expect("must not error");
    assert_eq!(frame, ServerFrame::Unknown);
}

#[test]
fn unknown_fields_within_known_frame_are_ignored() {
    let v = json!({ "type": "gap", "after_seq": 9, "added_later": true });
    let frame: ServerFrame = serde_json::from_value(v).expect("must ignore extra field");
    assert_eq!(frame, ServerFrame::Gap { after_seq: 9 });
}

#[test]
fn status_clients_use_camelcase_lastseq() {
    let d = &fixtures()["daemon_to_client"];
    let ServerFrame::Status { clients, sinks, last_seq, .. } =
        serde_json::from_value(d["status"].clone()).unwrap()
    else { panic!("not status") };
    assert_eq!(last_seq, Some(7));
    assert_eq!(sinks, Some(vec!["banner".into(), "socket".into()]));
    assert_eq!(clients[0].role, "viewer");
    assert_eq!(clients[0].last_seq, Some(7));
    assert_eq!(clients[1].last_seq, None);
}

#[allow(unused)]
fn _types_ref(_: Message) {}
