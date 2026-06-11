//! Phase A2 — the desktop inbox client stack (Tauri-agnostic).
//!
//! A reconnecting two-role daemon-socket client, a read-only WAL archive reader, an
//! identity-adopter, and a policy store. Built against the normative contract in
//! `agent-bridge-mcp/docs/PROTOCOL.md` + `test/fixtures/socket-frames.json`.

pub mod frames;
pub mod line_parser;
pub mod stores;
pub mod gate;
pub mod archive_reader;
pub mod replay;
pub mod identity_adopter;
pub mod policy_store;
pub mod client;

/// Resolve the daemon home: `AGENT_BRIDGE_HOME` or `~/.air-msg` (POSIX v1).
/// Mirrors `agent-bridge-mcp/src/identity.mjs` `bridgeHome()`.
pub fn bridge_home() -> std::path::PathBuf {
    if let Ok(h) = std::env::var("AGENT_BRIDGE_HOME") {
        return std::path::PathBuf::from(h);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    std::path::Path::new(&home).join(".air-msg")
}
