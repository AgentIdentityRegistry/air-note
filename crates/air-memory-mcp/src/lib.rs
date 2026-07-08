//! `air-memory-mcp` library surface: the daemon client (`daemon`) and the MCP JSON-RPC handler
//! (`mcp`). The bin (`main.rs`) is a thin stdio loop over these. Split into a lib so integration
//! tests can drive `daemon`/`mcp` directly.

pub mod daemon;
pub mod mcp;
