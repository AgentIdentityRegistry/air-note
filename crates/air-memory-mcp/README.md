# air-memory-mcp

An MCP (Model Context Protocol) stdio server that gives a coding agent two tools backed by your
AIR Agent memory:

- **`recall(query, k?)`** — search your AIR memory for relevant notes.
- **`remember(text)`** — save a new note (stored as external/untrusted: recallable, never
  auto-applied).

It talks to the local `bossclawd` daemon (the AIR Agent memory engine) over its Unix socket as a
scoped **MemoryClient** — the daemon refuses it every other operation (no teardown, no cloud
enable, no folder grants), enforced daemon-side.

## Build

```bash
cargo build --release -p air-memory-mcp
# binary: target/release/air-memory-mcp
```

## Wire it into Claude Code

**One click (recommended):** in the AIR Agent app, open **Settings ▸ Integrations** and click
**Connect Claude Code**. This writes the `air-memory` MCP server to `~/.claude.json` and a
SessionStart nudge to `~/.claude/settings.json` (merging with your existing config, never
replacing it), so every Claude Code session everywhere can `recall`/`remember`. **Disconnect**
removes exactly those entries. Takes effect on your next Claude Code session. Disconnect before
moving or uninstalling AIR Agent (the config points at the app's bundled binary); if `~/.claude.json`
is a symlink, connecting replaces it with a regular file. For best results, quit Claude Code first.

**Manual (advanced / headless):** add to your `.mcp.json` — see the entry shape below.

```json
{
  "mcpServers": {
    "air-memory": {
      "command": "/absolute/path/to/target/release/air-memory-mcp",
      "env": {
        "BOSSCLAWD_SOCKET": "/Users/you/Library/Application Support/ai.air-agent.desktop/bossclawd.sock"
      }
    }
  }
}
```

- If `BOSSCLAWD_SOCKET` is omitted, the adapter resolves the same default path the daemon uses:
  macOS `~/Library/Application Support/ai.air-agent.desktop/bossclawd.sock`, Linux
  `$XDG_DATA_HOME`/`~/.local/share`/`ai.air-agent.desktop/bossclawd.sock`.
- If AIR Agent isn't running, the tools return a clean "memory service unavailable" message (they
  never crash the session).

## Security

The adapter connects as `MemoryClient`; the daemon enforces a fail-closed allowlist (`recall`,
`remember` only) — a compromised or buggy adapter still cannot reach any destructive/egress op.
This is the "Simple" bar (a cooperative client is scoped); it does not defend against a *malicious*
same-uid process (which could already connect today). Cryptographic capability tokens are a future
hardening.

## License

Apache-2.0.
