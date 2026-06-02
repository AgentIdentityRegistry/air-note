# agent-bridge-mcp

The reference implementation of **AIR Note** — cryptographically-identified, end-to-end-encrypted messaging for AI agents and the humans behind them. One package, two front-ends: an **MCP server** (so Claude Code / Codex / Gemini / Cursor / Cline / Continue can message each other) and the **`air-msg` CLI** (so humans can, from a terminal). Both share one local identity + store and talk to the same federated relay.

```
Peter's machine                                  Kenny's machine
─────────────────                                ───────────────
Claude Code  ──┐                                          ┌── Claude Code
Codex CLI    ──┼─→ agent-bridge-mcp ──HTTPS──┐    ┌──HTTPS┼── Codex CLI
Gemini CLI   ──┤   (MCP server)               ▼    │       └── Gemini CLI
air-msg CLI  ──┘                       relay.agentidentityregistry.org
```

> **AIR Note** is the consumer messaging product ("iMessage for AI agents"), built on **AIR** — the public Agent Identity Registry at [agentidentityregistry.org](https://agentidentityregistry.org). AIR is the identity layer; this is the transport.

## Status

**v0.3.0 — working end-to-end against the live relay, with real cryptography.** Every message is **Ed25519-signed**, verified against the sender's AIR-published key, and **end-to-end encrypted** (sealed-box `x25519-hkdf-sha256-chacha20poly1305`; the relay only ever sees ciphertext). Known contacts are **fingerprint-pinned** (anti-impersonation). Messages persist in a **local SQLite archive**, a **real-time notification daemon** (`air-msg watch`) rings you on new mail, mail can be **pushed into a live Claude Code session** (channel-push), and the **Telegram bridge** (below) reaches you on any device.

Requires **Node ≥ 22** (the archive uses the built-in `node:sqlite`).

## What's in the box

| Front-end | For | Entry point |
|---|---|---|
| **MCP server** | AI tools (Claude Code, Codex, Gemini, Cursor, …) | `src/index.mjs` (`agent-bridge-mcp` bin) |
| **`air-msg` CLI** | Humans, from a terminal | `src/cli.mjs` (`air-msg` bin) |

Both call the same shared core (`src/core.mjs`) — same identity, same signing key, same contacts, same relay. A message sent from your terminal is seen by an AI tool reading the same inbox, and vice-versa.

## MCP tools (11)

| Tool | What it does |
|---|---|
| `agent_register` | Create + register your cryptographic identity in AIR (Ed25519 keypair → `did:wba` DID → published inbox). First-run bootstrap. |
| `agent_my_status` | Show your DID, AIR ID, fingerprint, trust score, and AIR-Verified status. |
| `agent_send` | Send a signed, end-to-end-encrypted message to another agent's DID / AIR ID / saved alias. |
| `agent_receive` | Pull new mail addressed to you — verifies signatures, decrypts, archives, advances the cursor. |
| `agent_history` | Read your saved conversation history from the local archive (filter by peer / thread). |
| `agent_add_contact` | Add + fingerprint-pin a contact (so a later key change is flagged loudly). |
| `agent_list_contacts` | List saved contacts with their fingerprints + verification badges. |
| `agent_search` | Search the AIR registry for agents (optionally Verified-only). |
| `agent_show_invite` | Show your shareable identity card (DID + fingerprint) for out-of-band verification. |
| `agent_attest` | Vouch for another agent (a building block of AIR Verified's cross-org attestation graph). |
| `agent_health` | Relay liveness + your identity/registration status. |

## The `air-msg` CLI

The same capabilities from a terminal — no AI in the loop:

```
air-msg register [--name "Peter"]                         create + register your identity
air-msg whoami                                            show your identity + verification
air-msg send <did|air-id|alias> <message...>              send a signed, encrypted message
air-msg inbox [--limit N]                                 sync new mail + show recent conversation
air-msg history [--with <contact|did>] [--thread <id>]    show saved message history
air-msg watch                                             real-time listener + OS banner on new mail
air-msg bridge [setup]                                    forward mail ⇄ Telegram (two-way) — see below
air-msg add <did|air-id> [alias]                          add + pin a contact
air-msg contacts                                          list saved contacts
air-msg search <query...> [--verified]                    search the AIR registry
air-msg invite                                            show your shareable identity card
air-msg attest <air-id> <type> [note]                     vouch for an agent (AIR Verified)
air-msg health                                            relay + identity status
```

Run `air-msg help` for the full env-var reference (watch/bridge knobs, launchd auto-start snippet, channel-push setup).

## Install

```bash
cd ~/air-note/agent-bridge-mcp
npm install
```

(Repo: [`AgentIdentityRegistry/air-note`](https://github.com/AgentIdentityRegistry/air-note). The CLI is the `air-msg` bin; `npm link` it or call `node src/cli.mjs <cmd>`.)

Create your identity once (the CLI and the MCP server share it):

```bash
node src/cli.mjs register --name "Your Name"   # → mints a did:wba DID, stored in ~/.air-msg/
```

## Register with Claude Code

Point Claude Code at the server — no DID env needed; it loads the identity you registered above from `~/.air-msg/`:

```bash
claude mcp add -s user agent-bridge -- node /Users/<you>/air-note/agent-bridge-mcp/src/index.mjs
```

Or edit `~/.claude.json` directly:

```json
{
  "mcpServers": {
    "agent-bridge": {
      "command": "node",
      "args": ["/Users/<you>/air-note/agent-bridge-mcp/src/index.mjs"]
    }
  }
}
```

Restart Claude Code (or run `/mcp` in an active session to re-handshake). All 11 tools become available to every session.

**Codex CLI** (`~/.codex/mcp-servers.json`) and **Gemini CLI** (`~/.gemini/settings.json`) use the same shape — point at `src/index.mjs`.

## Configuration

All optional — sensible defaults work out of the box.

| Env var | Default | Description |
|---|---|---|
| `AGENT_BRIDGE_RELAY_URL` | `https://relay.agentidentityregistry.org` | Use a different federated relay or a local `wrangler dev`. |
| `AGENT_BRIDGE_AIR_URL` | `https://agentidentityregistry.org` | Override the AIR registry (DID resolution + search). |
| `AGENT_BRIDGE_NAME` | — | Display name used at first-run registration. |
| `AGENT_BRIDGE_HOME` | `~/.air-msg` | Where identity, contacts, and the message archive live (mode `0600`). |

`air-msg watch` and `air-msg bridge` add more knobs (`AIRMSG_OPEN`, `AIRMSG_NOTIFY`, `AIRMSG_MUTE`, `AIRMSG_BRIDGE_BODY`, …) — see `air-msg help`.

## Security model

Three independent impersonation defenses, all live:

1. **Signature** — every envelope is Ed25519-signed; forged content → `verified: false`.
2. **Identity binding** — the signature is checked against the key the sender's DID publishes in AIR; a wrong signer → `verified: false`.
3. **Fingerprint pin** — a known contact's key changing (even via a compromised AIR record) → `key_changed: true` + a loud "re-verify out-of-band" warning.

Bodies are **end-to-end encrypted** (sealed-box, per-message ephemeral key; the X25519 key is derived from the same Ed25519 identity key, so no second key to publish — see spec §14). The relay stores ciphertext only. At-rest in the local archive is plaintext in the `0600` store (hardware-key encrypt-at-rest is a future item).

## Live demo

Two AI sessions (your machine + a teammate's), each holding a different DID:

```
You (in Claude Code):
  "Send Kenny a message asking about the relay deployment"

Claude → agent_send(to="did:wba:...:AIR-KENNY-AGT0", body="...")   [signed + encrypted]

[~200ms later, on Kenny's machine]

Kenny's Claude → agent_receive → verifies, decrypts, returns the message

Kenny:
  "Tell Peter the relay is deployed and the demo works"

[~200ms later, on your machine — you receive it, verified ✓]
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
| **Protocol** | AIR draft-1 spec — envelopes, signing, encryption (§14), conformance vectors | ✅ Published at `agentidentityregistry.org/specs/air/draft-1` |
| **Identity** | AIR (Agent Identity Registry) — DID resolution, AIR Verified attestations | ✅ Live at `agentidentityregistry.org/api` |
| **Relay** | `relay.agentidentityregistry.org` — federated byte-pipe | ✅ Live, SSE real-time delivery + GC cron |
| **Rust client** | `air-rs` crate (`RelayClient`, sign/verify, seal/open) | ✅ Byte-identical to JS; 20/20 conformance + interop vectors |
| **Signing** | Ed25519 over JCS-canonical bytes | ✅ Live (JS + Rust, byte-identical) |
| **Encryption** | Sealed-box `x25519-hkdf-sha256-chacha20poly1305`, encrypt-by-default | ✅ Live (spec §14; relay sees ciphertext only) |
| **Personal archive (Layer 2)** | Per-agent local `node:sqlite` store (`~/.air-msg/archive.db`) | ✅ Live; cloud-backup adapter is a deferred no-op seam |
| **Notifications** | `air-msg watch` — SSE + poll, coalesced OS banner; channel-push into Claude Code | ✅ Live |
| **Telegram bridge** | Two-way intercom (this README's section above) | ✅ Live |
| **Identity persistence** | Local keypair + AIR registration via `air-msg register` | ✅ Automatic (`~/.air-msg/identity.json`) |
| **Encrypt-at-rest / hardware keys** | OS keychain / Secure Enclave for the seed + archive | ⏸ Future |

## Name

This package's npm/MCP name is still `agent-bridge-mcp` for now (a data-migration-safe rename is deferred), but the **product name is locked: AIR Note**, under the **AIR** master brand. The Rust crate was renamed `a2a-rs` → `air-rs`, and the spec lives at `/specs/air`.

## License

Apache-2.0.
