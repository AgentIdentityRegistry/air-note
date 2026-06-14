//! Proves the inbox feature compiles and home-resolution honours AGENT_BRIDGE_HOME.
use air_rs::inbox::bridge_home;

#[test]
fn bridge_home_honours_env() {
    // Hermetic: this process-global env is fine because the test asserts the override path.
    std::env::set_var("AGENT_BRIDGE_HOME", "/tmp/air-a2-scaffold");
    assert_eq!(bridge_home(), std::path::PathBuf::from("/tmp/air-a2-scaffold"));
    std::env::remove_var("AGENT_BRIDGE_HOME");
}
