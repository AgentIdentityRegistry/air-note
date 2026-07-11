//! `air-memory-mcp` — an MCP stdio server exposing `recall` + `remember` over the `bossclawd`
//! socket as a scoped `MemoryClient` (SP1). Reads newline-delimited JSON-RPC messages from stdin,
//! answers each via `mcp::handle_message`, and writes response lines to stdout. Notifications get
//! no reply. Runs on a single-thread runtime — MCP requests are serial.

use air_memory_mcp::{daemon, hook, mcp};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    // SP3/B3: the `nudge` subcommand is Claude Code's SessionStart hook. It tries a LIVE, project-aware
    // orientation snapshot from the daemon and falls back to the static reminder on ANY failure. It is
    // fail-quiet by contract (spec §5 / I1): the hook must never break or block session start, so the
    // snapshot fetch is bounded (SNAPSHOT_TIMEOUT ≪ Claude Code's 5s hook kill), stdin is byte-bounded,
    // and it ALWAYS exits 0. An SP2-era hook that pipes no stdin still prints the static text (default
    // HookInput → no transcript_path → no matchable project key → static nudge, socket untouched).
    if std::env::args().nth(1).as_deref() == Some("nudge") {
        let h = hook::HookInput::read_from_stdin();
        print!("{}", air_memory_mcp::run_nudge(&daemon::resolve_socket_path(), h).await);
        return Ok(());
    }

    // SP3/B2: `capture-notify` is Claude Code's SessionEnd hook — a fire-and-forget poke asking the
    // daemon to render the just-ended session now. It reads the hook JSON from stdin, pokes the daemon
    // iff BOTH fields are present + non-empty (the B1 contract), and ALWAYS exits 0 (I1): any failure —
    // daemon down, capture disabled, a cleanly-rejected poke — is discarded because the sweeper (A9)
    // is the durability guarantee. The poke is an optimization for immediacy, never the data path.
    if std::env::args().nth(1).as_deref() == Some("capture-notify") {
        let h = hook::HookInput::read_from_stdin();
        if let Some((session_id, transcript_path)) = hook::capture_target(&h) {
            let _ = daemon::tool_capture_notify(
                &daemon::resolve_socket_path(),
                &session_id,
                &transcript_path,
            )
            .await;
        }
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
