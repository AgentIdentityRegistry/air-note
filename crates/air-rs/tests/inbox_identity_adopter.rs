use air_rs::inbox::identity_adopter::{adopt, creation_allowed, Adoption};
use std::fs;
use tempfile::TempDir;

fn seed_identity(home: &TempDir, did: &str, name: &str) {
    fs::write(home.path().join("identity.json"),
        format!(r#"{{"version":1,"name":"{name}","air_id":"ignored","did":"{did}","seed_hex":"SECRET","agent_secret":"SECRET"}}"#)).unwrap();
}

#[test]
fn adopts_daemon_identity_and_derives_air_id() {
    let h = TempDir::new().unwrap();
    seed_identity(&h, "did:wba:x:agents:AIR-2JE0-EM7W-JNBK", "peters-agent");
    match adopt(h.path(), None) {
        Adoption::Adopted { did, air_id, name, dormant_did } => {
            assert_eq!(did, "did:wba:x:agents:AIR-2JE0-EM7W-JNBK");
            assert_eq!(air_id, "AIR-2JE0-EM7W-JNBK");
            assert_eq!(name.as_deref(), Some("peters-agent"));
            assert!(dormant_did.is_none());
        }
        _ => panic!("expected adoption"),
    }
}

#[test]
fn reports_prior_desktop_identity_as_dormant() {
    let h = TempDir::new().unwrap();
    seed_identity(&h, "did:daemon:AIR-NEW", "agent");
    match adopt(h.path(), Some("did:desktop:AIR-OLD")) {
        Adoption::Adopted { dormant_did, .. } => assert_eq!(dormant_did.as_deref(), Some("did:desktop:AIR-OLD")),
        _ => panic!(),
    }
    match adopt(h.path(), Some("did:daemon:AIR-NEW")) {
        Adoption::Adopted { dormant_did, .. } => assert!(dormant_did.is_none()),
        _ => panic!(),
    }
}

#[test]
fn no_daemon_identity_needs_daemon_and_allows_creation() {
    let h = TempDir::new().unwrap();
    assert_eq!(adopt(h.path(), None), Adoption::NeedsDaemon);
    assert!(creation_allowed(h.path()));
}

#[test]
fn creation_forbidden_when_daemon_exists() {
    let h = TempDir::new().unwrap();
    seed_identity(&h, "did:daemon:AIR-X", "agent");
    assert!(!creation_allowed(h.path()));
}
