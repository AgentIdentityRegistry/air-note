# agent-bridge-mcp

MCP server bridging Claude Code / Codex / Gemini / Cursor / Cline / Continue to the AIR A2A messaging relay. **Real-time cross-machine, cross-AI-tool messaging between agents that hold cryptographic identity.**

```
Peter's machine                                  Kenny's machine
─────────────────                                ───────────────
Claude Code  ──┐                                          ┌── Claude Code
Codex CLI    ──┼─→ agent-bridge-mcp ──HTTPS──┐    ┌──HTTPS┼── Codex CLI
Gemini CLI   ──┘   (stdio MCP)                ▼    │       └── Gemini CLI
                                       relay.agentidentityregistry.org
```

## Status

**v0.0.1 — MVP.** Working end-to-end against the live relay. **No envelope signing yet** — sends a placeholder signature. Real EdDSA signing lands when we wire AIM/AIR identity in the next iteration. Use only for trusted testing.

## Three tools

| Tool | What it does |
|---|---|
| `agent_send` | Send a real-time message to another AI agent's DID. The recipient's agent receives it via their own `agent_receive` call within ~1 second. |
| `agent_receive` | Pull pending messages addressed to your DID. Returns decoded envelopes plus a cursor for incremental polling. |
| `agent_health` | Check the relay's liveness, bindings, and current queue stats. Useful for troubleshooting. |

## Install (from this repo)

```bash
cd ~/SuperClaw/agent-bridge-mcp
npm install
```

## Register with Claude Code

Add to your global Claude Code MCP config (`~/.claude.json` or via the `claude mcp add` CLI):

```bash
claude mcp add -s user agent-bridge \
  --env AGENT_BRIDGE_MY_DID="did:wba:agentidentityregistry.org:agents:AIR-YOUR-AGT0" \
  -- node /Users/<you>/SuperClaw/agent-bridge-mcp/src/index.mjs
```

Or edit `~/.claude.json` directly:

```json
{
  "mcpServers": {
    "agent-bridge": {
      "command": "node",
      "args": ["/Users/<you>/SuperClaw/agent-bridge-mcp/src/index.mjs"],
      "env": {
        "AGENT_BRIDGE_MY_DID": "did:wba:agentidentityregistry.org:agents:AIR-YOUR-AGT0"
      }
    }
  }
}
```

After adding, restart Claude Code (or run `/mcp` inside an active session to re-handshake). The three tools become available to every session.

### For Codex CLI

Codex CLI's MCP config lives at `~/.codex/mcp-servers.json` (or per the Codex docs). Same shape — point at this server's `src/index.mjs` with `AGENT_BRIDGE_MY_DID` set.

### For Gemini CLI

Gemini CLI's MCP integration lives at `~/.gemini/settings.json`. Same pattern.

## Configuration

| Env var | Default | Description |
|---|---|---|
| `AGENT_BRIDGE_MY_DID` | — (required for send/receive) | Your agent's DID, as registered in AIR |
| `AGENT_BRIDGE_RELAY_URL` | `https://relay.agentidentityregistry.org` | Override to use a different federated relay or a local `wrangler dev` |

## Live demo

Once configured in two Claude Code sessions (your machine + a teammate's), each holding a different DID:

```
You (in Claude Code):
  "Send Kenny a message asking about the relay deployment"

Claude → calls agent_send(to="did:wba:...:AIR-KENNY-AGT0", body="...")

[~200ms later, on Kenny's machine]

Kenny's Claude → polls agent_receive, returns the message

Kenny (in Claude Code):
  "Tell Peter the relay is deployed and the demo works"

[~200ms later, on your machine]

You receive: "Tell Peter the relay is deployed and the demo works"
```

## Telegram bridge (two-way)

Forward incoming AIR Note mail out to **Telegram**, and reply from inside Telegram to send a real (signed + encrypted) AIR Note back — a universal "doorbell" that reaches you on any device, even away from your AI tools.

```bash
air-msg bridge setup    # one-time: paste a @BotFather bot token, then /start the bot
air-msg bridge          # run the daemon (also raises the local OS banner, like `watch`)
```

**How replies work:** each incoming message arrives in Telegram as its own ping. To reply, use Telegram's *Reply* (swipe / long-press → Reply) on the specific ping — the bridge routes your text back to that exact sender, continuing the same thread. A bare message that isn't a reply is never sent (you'll be asked to reply to a specific ping, so you can't mis-send). Verified **and** pinned contacts get one-tap reply; an unverified sender requires a `/yes` confirmation first.

> ⚠️ **Privacy:** by default the **full message text** is sent to Telegram's servers, which is **outside AIR Note's end-to-end encryption**. Set `AIRMSG_BRIDGE_BODY=meta` to send metadata-only pings (e.g. "📬 mail from Alice") and keep message bodies on your machine.

**One live consumer per identity:** `air-msg bridge`, `air-msg watch`, and the channel-push server all share one relay read-cursor, so run only **one** at a time — the others refuse to start with a clear message. `bridge` is a superset of `watch`: it raises the local banner *and* forwards to Telegram.

| Env var | Default | Description |
|---|---|---|
| `AIRMSG_BRIDGE_BODY` | `full` | `meta` = send metadata-only pings (no decrypted message text leaves your machine) |

The bot token + your chat id live only in `~/.air-msg/bridge.json` (mode `0600`), never in the message database.

## Status of the stack underneath

| Layer | What | Status |
|---|---|---|
| **Protocol** | A2A draft-1 spec — envelopes, signing, conformance vectors | ✅ Published at `agentidentityregistry.org/specs/air/draft-1` |
| **Identity** | AIR (Agent Identity Registry) — DID resolution, service_endpoints | ✅ Live at `agentidentityregistry.org/api` |
| **Relay** | `relay.agentidentityregistry.org` — federated byte-pipe | ✅ Live, with SSE real-time delivery + GC cron |
| **Rust client** | `a2a-rs` crate with `RelayClient` | ✅ Live integration tests pass |
| **MCP bridge** | This package | ✅ MVP (this is what you're reading) |
| **Real signing** | EdDSA over JCS-canonical bytes | ⏸ Next iteration |
| **Personal archive (Layer 2)** | Per-agent local SQLite + cloud backup | ⏸ Design pending |
| **Persistent identity** | AIM-style local keypair file + AIR registration | ⏸ Manual today |

## License

Apache-2.0.

## Working name

This server is currently called `agent-bridge-mcp` as a placeholder. The protocol it speaks doesn't have its final name yet (the BossClaw team decided to sleep on it after discovering naming collisions with both Google's A2A and the OpenA2A security project). When the real name lands, this package + its MCP server name will be renamed in lockstep.
