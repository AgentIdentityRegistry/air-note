//! The `nudge` subcommand prints the static SessionStart reminder and exits 0 without touching
//! the daemon socket. SP2 wires this as the Claude Code SessionStart hook command.
use std::process::Command;

#[test]
fn nudge_subcommand_prints_nudge_text_and_exits_zero() {
    let out = Command::new(env!("CARGO_BIN_EXE_air-memory-mcp"))
        .arg("nudge")
        .output()
        .expect("run air-memory-mcp nudge");

    assert!(out.status.success(), "nudge must exit 0; got {:?}", out.status);
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        air_memory_mcp::NUDGE_TEXT,
        "stdout must be exactly NUDGE_TEXT (no trailing newline added)"
    );
    // It must NOT emit the server's socket banner (that only prints on the server path).
    let err = String::from_utf8(out.stderr).unwrap();
    assert!(!err.contains("using daemon socket"), "nudge must not start the server: {err}");
}
