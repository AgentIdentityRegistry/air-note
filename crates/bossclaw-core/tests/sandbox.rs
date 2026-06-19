//! Real-subprocess jail proofs. Gated: require --features markitdown AND a built
//! venv at $BOSSCLAW_MARKITDOWN_VENV AND the OS jail tool present.
//! Run: BOSSCLAW_MARKITDOWN_VENV=/tmp/m5b-venv cargo test -p bossclaw-core --features markitdown --test sandbox -- --ignored
#![cfg(all(any(target_os = "macos", target_os = "linux"), feature = "markitdown"))]

#[test]
#[ignore]
fn egress_probe_proves_network_denied_with_teeth() {
    assert!(bossclaw_core::sandbox_test_hooks::probe_egress_blocks(), "the jail MUST deny network (jailed connect refused)");
    assert!(!bossclaw_core::sandbox_test_hooks::unjailed_probe_blocks(), "un-jailed MUST connect — proves the probe has teeth");
}
