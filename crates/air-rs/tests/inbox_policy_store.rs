use air_rs::inbox::policy_store::{autonomy_for, load, set_autonomy, Autonomy};
use std::fs;
use tempfile::TempDir;

#[test]
fn missing_file_is_all_draft() {
    let h = TempDir::new().unwrap();
    assert_eq!(autonomy_for(h.path(), "did:x"), Autonomy::Draft);
}

#[test]
fn corrupt_file_is_all_draft() {
    let h = TempDir::new().unwrap();
    fs::write(h.path().join("agent-policy.json"), "{ not json").unwrap();
    assert_eq!(autonomy_for(h.path(), "did:x"), Autonomy::Draft);
}

#[test]
fn set_then_read_round_trips_and_persists_0600() {
    let h = TempDir::new().unwrap();
    set_autonomy(h.path(), "did:x", Autonomy::Auto).unwrap();
    assert_eq!(autonomy_for(h.path(), "did:x"), Autonomy::Auto);
    assert_eq!(autonomy_for(h.path(), "did:other"), Autonomy::Draft);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(h.path().join("agent-policy.json")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let raw = fs::read_to_string(h.path().join("agent-policy.json")).unwrap();
    assert!(raw.contains("\"auto\""));
    let _ = load(h.path());
}
