//! `air-memory-mcp` — an MCP stdio server exposing `recall` + `remember` over the `bossclawd`
//! socket as a scoped `MemoryClient` (SP1). Reads newline-delimited JSON-RPC messages from stdin,
//! answers each via `mcp::handle_message`, and writes response lines to stdout. Notifications get
//! no reply. Runs on a single-thread runtime — MCP requests are serial.

use air_memory_mcp::{daemon, mcp};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    // SP2: the `nudge` subcommand prints the static SessionStart reminder and exits — no socket,
    // no server. Claude Code's SessionStart hook runs `air-memory-mcp nudge`.
    if std::env::args().nth(1).as_deref() == Some("nudge") {
        print!("{}", air_memory_mcp::NUDGE_TEXT);
        return Ok(());
    }

    let sock = daemon::resolve_socket_path();
    eprintln!("air-memory-mcp: using daemon socket {}", sock.display());

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = mcp::handle_message(&sock, &line).await {
            stdout.write_all(response.as_bytes()).await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
        }
    }
    Ok(())
}
