use air_rs::inbox::stores::*;
use std::fs;
use tempfile::TempDir;

fn home() -> TempDir {
    TempDir::new().unwrap()
}

#[test]
fn short_peer_extracts_air_id_or_passes_through() {
    assert_eq!(
        short_peer("did:wba:agentidentityregistry.org:agents:AIR-2JE0-EM7W-JNBK"),
        "AIR-2JE0-EM7W-JNBK"
    );
    assert_eq!(short_peer("plain-alias"), "plain-alias");
}

#[test]
fn mute_set_parses_and_trims() {
    std::env::set_var("AIRMSG_MUTE", " pat, AIR-XXXX ,,bob ");
    let m = parse_mute_set();
    assert!(m.contains("pat") && m.contains("AIR-XXXX") && m.contains("bob"));
    assert!(!m.contains(""));
    std::env::remove_var("AIRMSG_MUTE");
}

#[test]
fn contact_lookup_returns_alias_when_pinned() {
    let h = home();
    fs::write(
        h.path().join("contacts.json"),
        r#"{"version":1,"contacts":{"did:x":{"alias":"pat","air_id":"AIR-1","public_key_multibase":"zABC"}}}"#,
    )
    .unwrap();
    let c = get_contact_by_did(h.path(), "did:x").unwrap();
    assert_eq!(c.alias.as_deref(), Some("pat"));
    assert!(get_contact_by_did(h.path(), "did:unknown").is_none());
}

#[test]
fn blocked_check_fails_open_on_missing_or_corrupt() {
    let h = home();
    assert!(!is_blocked(h.path(), "did:x"));
    fs::write(h.path().join("blocklist.json"), "{ this is not json").unwrap();
    assert!(!is_blocked(h.path(), "did:x"));
    fs::write(
        h.path().join("blocklist.json"),
        r#"{"version":1,"blocked":{"did:x":{"air_id":"AIR-1"}}}"#,
    )
    .unwrap();
    assert!(is_blocked(h.path(), "did:x"));
    assert!(!is_blocked(h.path(), "did:y"));
}

#[test]
fn identity_meta_reads_only_public_fields() {
    let h = home();
    fs::write(
        h.path().join("identity.json"),
        r#"{"version":1,"name":"peters-agent","air_id":"AIR-2JE0","did":"did:wba:x:agents:AIR-2JE0","seed_hex":"DEADBEEF","agent_secret":"TOPSECRET"}"#,
    )
    .unwrap();
    let meta = read_daemon_identity_meta(h.path()).unwrap();
    assert_eq!(meta.did, "did:wba:x:agents:AIR-2JE0");
    assert_eq!(meta.name.as_deref(), Some("peters-agent"));
}

#[test]
fn list_contacts_returns_all_with_did_keys_and_registry_fields() {
    let h = home();
    fs::write(
        h.path().join("contacts.json"),
        r#"{"version":1,"contacts":{
            "did:wba:x:agents:AIR-1":{"alias":"kenny","air_id":"AIR-1","name":"Kenny","username":"kenny","public_key_multibase":"zA","verified_at_pin":true},
            "did:wba:x:agents:AIR-2":{"air_id":"AIR-2","public_key_multibase":"zB"}
        }}"#,
    )
    .unwrap();
    let got = list_contacts(h.path()); // sorted by did for determinism
    assert_eq!(got.len(), 2);

    assert_eq!(got[0].did, "did:wba:x:agents:AIR-1");
    assert_eq!(got[0].alias.as_deref(), Some("kenny"));
    assert_eq!(got[0].name.as_deref(), Some("Kenny"));
    assert_eq!(got[0].username.as_deref(), Some("kenny"));
    assert!(got[0].verified_at_pin);

    // minimal record: registry fields absent → None; verified defaults to false
    assert_eq!(got[1].did, "did:wba:x:agents:AIR-2");
    assert_eq!(got[1].alias, None);
    assert_eq!(got[1].name, None);
    assert_eq!(got[1].username, None);
    assert!(!got[1].verified_at_pin);
}

#[test]
fn list_contacts_is_empty_on_missing_or_corrupt_file() {
    let h = home();
    assert!(list_contacts(h.path()).is_empty()); // missing
    fs::write(h.path().join("contacts.json"), "{ not json").unwrap();
    assert!(list_contacts(h.path()).is_empty()); // corrupt → fail-soft
}
