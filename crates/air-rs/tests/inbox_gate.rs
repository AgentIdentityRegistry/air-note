use air_rs::inbox::frames::Message;
use air_rs::inbox::gate::channel_gate;
use std::collections::HashSet;

fn base() -> Message {
    Message {
        seq: 1,
        relay_seq: 1,
        envelope_id: "e1".into(),
        from: "did:wba:x:agents:AIR-PEER-PEER-PEER".into(),
        verified: true,
        encrypted: true,
        received_at: "t".into(),
        contact: Some("pat".into()),
        key_changed: None,
        thread_id: None,
        room_id: None,
        body: None,
    }
}

#[test]
fn admits_verified_pinned_unchanged_unmuted() {
    assert!(channel_gate(&base(), &HashSet::new()));
}

#[test]
fn rejects_unverified() {
    let mut m = base();
    m.verified = false;
    assert!(!channel_gate(&m, &HashSet::new()));
}

#[test]
fn rejects_unpinned_or_empty_contact() {
    let mut m = base();
    m.contact = None;
    assert!(!channel_gate(&m, &HashSet::new()));
    m.contact = Some(String::new());
    assert!(!channel_gate(&m, &HashSet::new()));
}

#[test]
fn rejects_key_changed() {
    let mut m = base();
    m.key_changed = Some(true);
    assert!(!channel_gate(&m, &HashSet::new()));
}

#[test]
fn rejects_muted_by_alias_did_or_airid() {
    let m = base();
    assert!(!channel_gate(&m, &HashSet::from(["pat".to_string()])));
    assert!(!channel_gate(&m, &HashSet::from([m.from.clone()])));
    assert!(!channel_gate(
        &m,
        &HashSet::from(["AIR-PEER-PEER-PEER".to_string()])
    ));
}
