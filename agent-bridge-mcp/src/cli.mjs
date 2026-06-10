#!/usr/bin/env node
// air-msg — command-line front-end for the agent-messaging protocol.
//
// Direct human messaging: no AI in the loop. Reuses the SAME ~/.air-msg
// store, identity, signing, and contacts as the MCP server (core.mjs).
// Send a message from your terminal; an AI tool reading the same inbox sees
// it, and vice-versa.
//
//   air-msg register [--name "Peter"]
//   air-msg whoami
//   air-msg send <did|air-id|alias> <message...>
//   air-msg inbox [--limit N]
//   air-msg history [--with <contact|did>] [--thread <id>] [--limit N]
//   air-msg add <did|air-id> [alias]
//   air-msg contacts
//   air-msg search <query...> [--verified]
//   air-msg invite
//   air-msg attest <air-id> <type> [statement...]
//   air-msg health

import { fileURLToPath } from "node:url";
import { realpathSync } from "node:fs";
import * as core from "./core.mjs";
import { ensureIdentity } from "./identity.mjs";
import { createNotifier } from "./notifier.mjs";
import { resolveOpenCommand, runOpenCommand, detectAiCmd } from "./open-conversation.mjs";
import { watch } from "./watch.mjs";
import { acquireOrExit, releaseConsumerLock } from "./consumer-lock.mjs";
import { loadBridgeConfig, saveBridgeConfig } from "./bridge-config.mjs";
import { createTelegramAdapter, captureFirstChat } from "./adapters/telegram.mjs";
import { makeBridgeOutbound, makeReplyHandler, makeConfirmStore } from "./bridge.mjs";
import { getUpdateOffset, setUpdateOffset, pruneRoutes } from "./bridge-routes.mjs";
import { createInterface } from "node:readline/promises";
import { connectDaemonPersistent, cleanStaleSocket, probeDaemon } from "./daemon-ipc.mjs";

const tty = process.stdout.isTTY;
const c = {
  dim: (s) => (tty ? `\x1b[2m${s}\x1b[0m` : s),
  bold: (s) => (tty ? `\x1b[1m${s}\x1b[0m` : s),
  green: (s) => (tty ? `\x1b[32m${s}\x1b[0m` : s),
  red: (s) => (tty ? `\x1b[31m${s}\x1b[0m` : s),
  yellow: (s) => (tty ? `\x1b[33m${s}\x1b[0m` : s),
  cyan: (s) => (tty ? `\x1b[36m${s}\x1b[0m` : s),
};

/** Split argv into positionals + flags (--flag or --flag value). */
export function parseArgs(argv) {
  const positionals = [];
  const flags = {};
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a.startsWith("--")) {
      const key = a.slice(2);
      const next = argv[i + 1];
      if (next && !next.startsWith("--")) {
        flags[key] = next;
        i++;
      } else {
        flags[key] = true;
      }
    } else {
      positionals.push(a);
    }
  }
  return { positionals, flags };
}

/** Parse a `room` subcommand's OWN argv (the tokens after "room"): the leading token is the
 *  sub-command name, the remainder is parsed for positionals + flags. Parsing the sub-argv
 *  DIRECTLY is what keeps flags like --name/--kind/--mandate/--limit from being swallowed by
 *  the top-level parse (which strips them before the room handler ever sees them). */
export function parseRoomArgs(roomArgv) {
  const [sub, ...subArgv] = roomArgv;
  const { positionals, flags } = parseArgs(subArgv);
  return { sub, positionals, flags };
}

export function bodyText(body) {
  if (!body) return "";
  if (body.type === "text") return body.text;
  if (body.type === "room/msg") return body.text ?? "";
  if (body.type === "room/joined") return `📥 You were added to room "${body.room_name}"`;
  return JSON.stringify(body);
}

// Absolute path to the channel-push MCP server, resolved at runtime so the .mcp.json
// snippet in `air-msg help` is copy-paste-ready on any machine (no manual editing).
const CHANNEL_SERVER_PATH = fileURLToPath(new URL("./channel-server.mjs", import.meta.url));

const HELP = `air-msg — cryptographically-signed agent messaging from your terminal

  air-msg register [--name "Peter"]      Create + register your identity
  air-msg whoami                         Show your identity + verification
  air-msg send <to> <message...>         Send a signed message
  air-msg inbox [--limit N]              sync new mail + show recent conversation
  air-msg history [--with <contact|did>] [--thread <id>] [--limit N]   show saved message history
  air-msg watch                          Listen for new mail + notify (Ctrl-C to stop)
  air-msg bridge [setup]                 Forward mail ⇄ Telegram (two-way; setup configures the bot)
  air-msg add <to> [alias]               Add + pin a contact
  air-msg contacts                       List saved contacts
  air-msg search <query...> [--verified] Search the AIR registry
  air-msg invite                         Show your shareable identity card
  air-msg attest <air-id> <type> [note]  Vouch for an agent (AIR Verified)
  air-msg room create --name <name>      Create a group chat room (you become founder)
  air-msg room invite <room> <to>        Invite an agent to a room (founder or admin)
  air-msg room kick <room> <member>      Remove a member from a room (founder only)
  air-msg room grant-admin <room> <to>   Grant admin rights (founder only)
  air-msg room revoke-admin <room> <id>  Revoke an admin mandate (founder only)
  air-msg room send <room> <text...>     Send a message to all room members
  air-msg room list                      List all rooms you are a member of
  air-msg room history <room> [--limit N] Show archived messages for a room
  air-msg room halt <room>               Halt a room (no messages while halted; founder only)
  air-msg room resume <room>             Resume a halted room (founder only)
  air-msg room sync <room>               Request op-set catch-up from the founder (best-effort)
  air-msg block <to>                     Drop all mail from a sender (convenience filter)
  air-msg unblock <to>                   Remove a sender from the blocklist
  air-msg blocked                        List blocked senders + drop tallies
  air-msg spam <envelope-id>             Hide a junk message + report it to AIR (private)
  air-msg delete --message <id> --yes    Delete one message from your local diary
  air-msg delete --with <to> --yes       Delete a whole conversation from your local diary
  inbox/history also accept --include-spam to reveal hidden spam.
  air-msg daemon status                  Show whether the receiver daemon is running
  air-msg daemon start                   Start the receiver daemon (banner sink, lock-guarded)
  air-msg daemon stop                    Stop the running daemon
  air-msg health                         Relay + identity status

  <to> may be a DID, an AIR ID, or a saved contact alias.
  attest <type>: identity_verification | operator_confirmation | dependency | safety_review

  watch config (env):
    AIRMSG_OPEN     terminal-history (default) | ai | command:<tmpl> | none
    AIRMSG_AI_CMD   claude | codex | gemini | <binary>   (for AIRMSG_OPEN=ai)
    AIRMSG_NOTIFY   node-notifier | osascript | bell | none   (auto if unset)
    AIRMSG_MUTE     comma-separated peers (alias/DID/AIR-id) to silence

  auto-start on login (macOS launchd): save this to
  ~/Library/LaunchAgents/org.air-msg.watch.plist then run: launchctl load <that path>
    <?xml version="1.0"?>
    <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
    <plist version="1.0"><dict>
      <key>Label</key><string>org.air-msg.watch</string>
      <key>ProgramArguments</key><array>
        <string>/usr/bin/env</string><string>air-msg</string><string>watch</string></array>
      <key>RunAtLoad</key><true/><key>KeepAlive</key><true/>
    </dict></plist>

  channel push (experimental — incoming mail into a live Claude Code session):
    1. add to your project .mcp.json (alongside the agent-bridge-mcp tools server, so the
       AI can reply with agent_send):
       { "mcpServers": { "air-msg-channel": {
           "command": "node",
           "args": ["${CHANNEL_SERVER_PATH}"] } } }
    2. launch from that dir, pointing Claude at the config (the --mcp-config flag is
       required — Claude does not auto-load .mcp.json here):
       claude --mcp-config ./.mcp.json --dangerously-load-development-channels server:air-msg-channel
    Only verified + pinned senders push into the session; everyone else still rings via
    air-msg watch. Requires a claude.ai / Console API key. Run only ONE live consumer per
    identity (the channel session OR air-msg watch, not both — they share the pull cursor).

  telegram bridge (two-way — mail → Telegram, reply in Telegram → AIR Note):
    air-msg bridge setup    one-time: paste a @BotFather token, /start the bot
    air-msg bridge          start the daemon (also fires the local banner like watch)
    env: AIRMSG_BRIDGE_BODY=meta  send metadata-only (no message text leaves your machine)
    Verified+pinned senders get one-tap reply; unverified replies need a /yes confirm.
    Run only ONE live consumer per identity (bridge OR watch OR the channel session).`;

async function bridgeSetup() {
  const rl = createInterface({ input: process.stdin, output: process.stdout });
  try {
    console.log(c.bold("AIR Note → Telegram bridge setup"));
    console.log(c.yellow(
      "⚠ PRIVACY: by default the FULL message text is sent to Telegram's servers, " +
      "outside AIR Note's end-to-end encryption. Run the bridge with AIRMSG_BRIDGE_BODY=meta " +
      "for metadata-only pings."));
    console.log(c.dim("1) In Telegram, message @BotFather → /newbot → copy the token it gives you."));
    const token = (await rl.question("Paste your bot token: ")).trim();
    if (!token) { console.error("No token — aborting."); process.exit(1); }

    console.log(c.dim("2) Now open your new bot in Telegram and send it /start (a bot can't message you first)."));
    console.log(c.dim("   Waiting for your message…"));
    const chatId = await captureFirstChat({ token });
    if (chatId == null) { console.error("Timed out waiting for /start — run setup again."); process.exit(1); }

    const path = saveBridgeConfig({ telegram: { bot_token: token, chat_id: chatId } });
    console.log(`${c.green("✓ saved")} ${c.dim(path)} (chat ${chatId})`);
    console.log(`${c.dim("Run")} ${c.bold("air-msg bridge")} ${c.dim("to start the doorbell.")}`);
  } finally {
    rl.close();
  }
}

async function main() {
  const [cmd, ...rest] = process.argv.slice(2);
  const { positionals, flags } = parseArgs(rest);

  switch (cmd) {
    case "register": {
      const r = await core.register({ name: flags.name });
      if (r.status === "already_registered") {
        console.log(`${c.dim("Already registered as")} ${c.bold(r.air_id)}  ${c.dim(r.did)}`);
      } else {
        console.log(`${c.green("✓ Registered")} as ${c.bold(r.air_id)}`);
        console.log(`  DID:         ${r.did}`);
        console.log(`  inbox live:  ${r.service_endpoint_published ? c.green("yes") : c.yellow("no")}`);
      }
      break;
    }
    case "whoami": {
      const s = await core.myStatus();
      if (!s.registered) { console.log(c.yellow("Not registered. Run: air-msg register")); break; }
      const badge = s.verified ? c.green("✓ AIR Verified") : c.dim("self-verified");
      console.log(`${c.bold(s.name || s.air_id)}  ${badge}`);
      console.log(`  AIR ID:      ${s.air_id}`);
      console.log(`  DID:         ${s.did}`);
      console.log(`  fingerprint: ${c.cyan(s.fingerprint)}`);
      console.log(`  trust score: ${s.trust_score ?? "?"}`);
      if (s.verification_status) {
        const v = s.verification_status;
        console.log(`  verified:    ${v.score ?? 0}/${v.score_required ?? 300} weight · ${v.distinct_whois_roots ?? 0}/${v.distinct_whois_roots_required ?? 3} org roots`);
      }
      break;
    }
    case "send": {
      const [to, ...words] = positionals;
      if (!to || words.length === 0) { console.error("usage: air-msg send <to> <message...>"); process.exit(1); }
      const r = await core.send({ to, body: words.join(" "), plaintext: !!flags.plaintext });
      const encBadge = r.encrypted ? "🔒 sent (encrypted)" : "✉️  sent (NOT encrypted)";
      console.log(`${c.green("✓")} ${encBadge} → ${r.to}`);
      console.log(`  ${c.dim("envelope " + r.envelope_id + " · thread " + r.thread_id)}`);
      break;
    }
    case "inbox": {
      const synced = await core.receiveAll();
      const { messages } = core.recentInbox({
        limit: flags.limit ? Number(flags.limit) : 20,
        includeSpam: !!flags["include-spam"],
      });
      console.log(`${messages.length} in archive${synced.count ? `  (${synced.count} new)` : ""}`);
      console.log("");
      for (const m of messages) {
        // Room-join is a system event, not a 1:1 letter — render it as a one-line marker.
        if (m.body?.type === "room/joined") {
          console.log(`  ★ ${c.cyan(bodyText(m.body))}  ${c.dim(m.timestamp)}`);
          continue;
        }
        const arrow = m.direction === "sent" ? "↑" : "↓";
        const encBadge = m.encrypted ? "🔒" : "✉️ ";
        const vrf = m.verified ? c.green("✓") : c.red("✗");
        const who = m.direction === "sent" ? `to ${m.to}` : `from ${m.from}`;
        const roomTag = m.room_id ? c.cyan("[room] ") : "";
        console.log(`  ${arrow} ${encBadge} ${vrf} ${who}  ${c.dim(m.timestamp)}`);
        console.log(`    ${roomTag}${bodyText(m.body)}`);
        console.log(`    ${c.dim("id " + m.envelope_id)}`);
      }
      break;
    }
    case "history": {
      const result = core.historyOp({
        peer: flags.with, thread: flags.thread,
        limit: flags.limit ? Number(flags.limit) : 50,
        includeSpam: !!flags["include-spam"],
      });
      const scope = flags.with ? ` with ${result.resolvedPeer}` : "";
      console.log(`${result.count} message(s)${scope}`);
      console.log("");
      for (const m of [...result.messages].reverse()) {
        const arrow = m.direction === "sent" ? "↑" : "↓";
        const encBadge = m.encrypted ? "🔒" : "✉️ ";
        const who = m.direction === "sent" ? `to ${m.to}` : `from ${m.from}`;
        console.log(`  ${arrow} ${encBadge} ${who}  ${c.dim(m.timestamp)}`);
        console.log(`    ${bodyText(m.body)}`);
        console.log(`    ${c.dim("id " + m.envelope_id)}`);
      }
      break;
    }
    case "watch": {
      const identity = await ensureIdentity();
      const printFeedLine = (m) => {
        const who = m.contact ? m.contact : m.from;
        const enc = m.encrypted ? "🔒" : "✉️ ";
        const vrf = m.verified ? c.green("✓") : c.red("✗");
        console.log(`  ↓ ${enc} ${vrf} ${who}  ${c.dim(new Date().toISOString())}`);
        console.log(`    ${bodyText(m.body)}`);
      };
      // §7 row 1: socket live → attach as viewer (no lock, no second banner — the daemon's
      // bannerSink already rings; this terminal is a passive feed).
      try {
        const handle = await connectDaemonPersistent({
          role: "viewer",
          onMessage: printFeedLine,
          onAttach: () => console.log(`${c.green("● watching")} ${c.dim("(attached to air-msgd — daemon owns the pull; Ctrl-C detaches only this feed)")}`),
          onDetach: () => console.log(c.dim("  …daemon connection lost — reconnecting")),
          log: () => {},   // M1: suppress raw [client] transport lines from the curated stdout feed
        });
        // C1: connectDaemonPersistent holds only an unref'd backoff timer during outages — it
        // cannot pin the event loop. Signal listeners do not pin it either. This ref'd interval
        // keeps the process alive across daemon restarts until the user explicitly Ctrl-Cs.
        const keepAlive = setInterval(() => {}, 60_000);
        await new Promise((resolve) => {
          const stop = () => { console.log(c.dim("\n…detaching from daemon")); clearInterval(keepAlive); handle.close(); resolve(); };
          process.once("SIGINT", stop);
          process.once("SIGTERM", stop);
        });
        break;
      } catch (e) {
        if (e.code !== "DAEMON_DOWN") throw e;         // §7 rows 2-3: no daemon → legacy standalone below
      }
      if (!acquireOrExit("watch")) break;
      cleanStaleSocket();                              // §7 row 2: lock acquired proves socket is stale
      try {
        const openMode = process.env.AIRMSG_OPEN || "terminal-history";
        const aiCmd = process.env.AIRMSG_AI_CMD || (openMode === "ai" ? detectAiCmd() : undefined);
        const openResolver = (peer, info) =>
          resolveOpenCommand(peer, { mode: openMode, aiCmd, ...info });

        const notifier = await createNotifier({ onClick: (argv) => runOpenCommand(argv) });

        const ac = new AbortController();
        const stop = () => { console.log(c.dim("\n…stopping watch")); ac.abort(); };
        process.once("SIGINT", stop);
        process.once("SIGTERM", stop);

        console.log(`${c.green("● watching")} ${c.bold(identity.did)}`);
        console.log(`  ${c.dim(`notify: ${notifier.backend} · open: ${openMode}${aiCmd ? " (" + aiCmd + ")" : ""} · Ctrl-C to stop`)}`);

        await watch({
          signal: ac.signal, identity, notifier, openResolver,
          onMessage: printFeedLine,
        }).catch((e) => { if (e?.name !== "AbortError") throw e; });
      } finally {
        releaseConsumerLock();
      }
      break;
    }
    case "bridge": {
      if (positionals[0] === "setup") { await bridgeSetup(); break; }

      const cfg = loadBridgeConfig();
      if (!cfg?.telegram?.bot_token || cfg?.telegram?.chat_id == null) {
        console.error("Bridge not configured. Run: air-msg bridge setup");
        process.exit(1);
      }
      // §7 bridge row: the daemon owns the pull; a standalone bridge beside it would fight for
      // the consumer lock and lose with a generic message — refuse with the real reason instead.
      // (In-daemon Telegram is the "bridge to doorbell-grade" roadmap item, not Phase 4.)
      if (await probeDaemon()) {
        console.error("bridge: the daemon owns the message pull on this identity.");
        console.error("Stop it first (air-msg daemon stop) to run the standalone bridge,");
        console.error("or keep the daemon — in-daemon Telegram is on the roadmap.");
        process.exit(1);
      }
      if (!acquireOrExit("bridge")) break;
      cleanStaleSocket();                              // §7 row 2: lock acquired proves socket is stale
      try {
        const identity = await ensureIdentity();
        const bodyMode = process.env.AIRMSG_BRIDGE_BODY === "meta" ? "meta" : "full";

        const ac = new AbortController();
        const stop = () => { console.log(c.dim("\n…stopping bridge")); ac.abort(); };
        process.once("SIGINT", stop);
        process.once("SIGTERM", stop);

        const adapter = createTelegramAdapter({
          token: cfg.telegram.bot_token,
          chatId: Number(cfg.telegram.chat_id),
          getOffset: () => getUpdateOffset({ platform: "telegram" }),
          setOffset: (o) => setUpdateOffset({ platform: "telegram", offset: o }),
          signal: ac.signal,
        });

        // D6: the bridge is a superset of `watch` — fire the local OS banner too.
        const openMode = process.env.AIRMSG_OPEN || "terminal-history";
        const aiCmd = process.env.AIRMSG_AI_CMD || (openMode === "ai" ? detectAiCmd() : undefined);
        const openResolver = (peer, info) => resolveOpenCommand(peer, { mode: openMode, aiCmd, ...info });
        const notifier = await createNotifier({ onClick: (argv) => runOpenCommand(argv) });

        const confirm = makeConfirmStore();
        const outbound = makeBridgeOutbound({ adapter, bodyMode });

        // v1: prune on start only. The 30-day age bound + maxRows cap keep bridge_routes
        // bounded, and a long-lived daemon re-prunes on each restart, so the spec's
        // "+ periodically" (§12) is intentionally deferred — not an oversight.
        pruneRoutes({ platform: "telegram", now: Date.now() });
        console.log(`${c.green("● bridging")} ${c.bold(identity.did)} ${c.dim("→ Telegram")}`);
        console.log(`  ${c.dim(`body: ${bodyMode} · notify: ${notifier.backend} · Ctrl-C to stop`)}`);
        if (bodyMode === "full") {
          console.log(c.yellow("  ⚠ full message text is sent to Telegram (outside E2E). Set AIRMSG_BRIDGE_BODY=meta for metadata-only."));
        }

        // INBOUND loop (replies → AIR Notes) runs alongside the OUTBOUND watch loop.
        const inbound = adapter
          .listen({ signal: ac.signal, onReply: makeReplyHandler({ sendFn: core.send, confirm }) })
          .catch((e) => { if (e?.name !== "AbortError") console.error("bridge inbound:", e.message ?? e); });

        await watch({
          signal: ac.signal, identity, notifier, openResolver,
          onMessage: (m) => {
            outbound(m); // push to Telegram + store the route
            const vrf = (m.verified && !m.key_changed) ? c.green("✓") : c.red("⚠");
            console.log(`  ↓→tg ${vrf} ${m.contact || m.from}`);
          },
        }).catch((e) => { if (e?.name !== "AbortError") throw e; });

        await inbound;
      } finally {
        releaseConsumerLock();
      }
      break;
    }
    case "add": {
      const [to, alias] = positionals;
      if (!to) { console.error("usage: air-msg add <did|air-id> [alias]"); process.exit(1); }
      const r = await core.addContactOp({ to, alias });
      const ct = r.contact;
      console.log(`${r.key_changed ? c.red("⚠ RE-PINNED (key changed)") : c.green("✓ added")} ${c.bold(ct.alias)}`);
      console.log(`  ${ct.name || ct.air_id}  ${ct.air_verified ? c.green("✓ Verified") : c.dim("self-verified")}`);
      console.log(`  fingerprint: ${c.cyan(ct.fingerprint)}`);
      console.log(c.dim("  → verify this fingerprint with them out-of-band before trusting."));
      break;
    }
    case "contacts": {
      const r = core.listContactsOp();
      if (r.count === 0) { console.log(c.dim("(no contacts)")); break; }
      for (const ct of r.contacts) {
        const badge = ct.air_verified ? c.green("✓") : c.dim("·");
        const warn = ct.key_changed_since_first_pin ? c.red("  ⚠ key changed since first pin") : "";
        console.log(`${badge} ${c.bold(ct.alias)}  ${c.cyan(ct.fingerprint)}  ${c.dim(ct.air_id)}${warn}`);
      }
      break;
    }
    case "search": {
      const r = await core.search({ query: positionals.join(" "), verified_only: !!flags.verified });
      if (r.count === 0) { console.log(c.dim("(no matches)")); break; }
      for (const a of r.results) {
        const badge = a.air_verified ? c.green("✓ Verified") : c.dim("self-verified");
        console.log(`${c.bold(a.name)}  ${badge}  ${c.dim("trust " + a.trust_score)}`);
        console.log(`  ${a.air_id}  ${c.dim(a.did)}`);
      }
      break;
    }
    case "invite": {
      const r = await core.showInvite();
      console.log(c.bold(r.name));
      console.log(`  DID:         ${r.did}`);
      console.log(`  fingerprint: ${c.cyan(r.fingerprint)}`);
      console.log(`\n  ${c.dim(r.share_line)}`);
      break;
    }
    case "attest": {
      const [subject, type, ...note] = positionals;
      if (!subject || !type) { console.error("usage: air-msg attest <air-id> <type> [statement...]"); process.exit(1); }
      const r = await core.attest({ subject, attestation_type: type, statement: note.join(" ") });
      console.log(`${c.green("✓ attested")} ${subject} ${c.dim("(" + type + ")")}`);
      break;
    }
    case "block": {
      const [peer] = positionals;
      if (!peer) { console.error("usage: air-msg block <did|air-id|alias>"); process.exit(1); }
      const r = await core.blockOp({ peer });
      console.log(`${c.green("✓ " + r.status)} ${c.bold(r.alias || r.air_id || r.did)}`);
      console.log(c.dim("  their mail is now dropped on arrival. NOTE: unblocking later cannot recover messages dropped while blocked, and a sender who forges a different identity can still get through (block is a convenience filter, not a security wall)."));
      break;
    }
    case "unblock": {
      const [peer] = positionals;
      if (!peer) { console.error("usage: air-msg unblock <did|air-id|alias>"); process.exit(1); }
      const r = await core.unblockOp({ peer });
      console.log(`${c.green("✓ " + r.status)} ${c.dim(r.did)}`);
      break;
    }
    case "blocked": {
      const r = core.listBlockedOp();
      if (r.count === 0) { console.log(c.dim("(no blocked senders)")); break; }
      for (const b of r.blocked) {
        const tally = b.drop_count
          ? c.dim(`  ${b.drop_count} dropped${b.last_drop_at ? ", last " + b.last_drop_at : ""}`)
          : "";
        console.log(`${c.red("⊘")} ${c.bold(b.alias || b.air_id)}  ${c.dim(b.did)}${tally}`);
      }
      console.log(c.dim("  (tallies are advisory — a forged sender can evade or inflate them)"));
      break;
    }
    case "spam": {
      const [envelope_id] = positionals;
      if (!envelope_id) { console.error("usage: air-msg spam <envelope-id>   (copy the id from inbox/history)"); process.exit(1); }
      const r = await core.reportSpamOp({ envelope_id });
      const report = r.reported
        ? c.green("reported")
        : c.yellow("local-only" + (r.reason ? ` (${r.reason})` : ""));
      console.log(`${c.green("✓ hidden")} ${c.dim("from " + r.subject)} · ${report}`);
      break;
    }
    case "delete": {
      if (!flags.yes) { console.error("refusing to delete without --yes (this permanently erases local diary rows)"); process.exit(1); }
      let r;
      if (flags.message) r = await core.deleteOp({ envelope_id: flags.message, confirm: true });
      else if (flags.with) r = await core.deleteOp({ peer: flags.with, confirm: true });
      else { console.error("usage: air-msg delete --message <envelope-id> | --with <peer>  --yes"); process.exit(1); }
      const tgt = r.scope === "message" ? r.envelope_id : "conversation with " + r.peer;
      console.log(`${c.green("✓ deleted")} ${r.deleted} row(s)  ${c.dim(tgt)}`);
      break;
    }
    case "room": {
      // Parse the room sub-argv straight from `rest` (the tokens after "room"). The
      // top-level parse already stripped --name/--kind/--mandate/--limit into the
      // top-level `flags`, so re-parsing those leftovers would lose every room flag.
      const { sub, positionals: rPos, flags: rFlags } = parseRoomArgs(rest);
      switch (sub) {
        case "create": {
          const name = rFlags.name || rPos.join(" ");
          if (!name) { console.error("usage: air-msg room create --name <name>"); process.exit(1); }
          const r = await core.roomCreateOp({ name });
          console.log(`${c.green("✓ created")} ${c.bold(name)}`);
          console.log(`  room_id:   ${r.room_id}`);
          console.log(`  thread_id: ${r.thread_id}`);
          break;
        }
        case "invite": {
          const [room_id, to] = rPos;
          if (!room_id || !to) { console.error("usage: air-msg room invite <room_id> <did|air-id> [--mandate <id>] [--kind human|agent]"); process.exit(1); }
          const r = await core.roomInviteOp({ room_id, to, mandate_id: rFlags.mandate, kind: rFlags.kind });
          const ok = r.fanout.filter((x) => x.ok).length;
          const boot = r.bootstrap_ok ? "" : c.red(` (bootstrap failed: ${r.bootstrap_error})`);
          console.log(`${c.green("✓ invited")} ${r.member_did}  ${c.dim(`fanout: ${ok}/${r.fanout.length} ok`)}${boot}`);
          break;
        }
        case "kick": {
          const [room_id, member] = rPos;
          if (!room_id || !member) { console.error("usage: air-msg room kick <room_id> <did|air-id>"); process.exit(1); }
          const r = await core.roomKickOp({ room_id, member });
          const ok = r.fanout.filter((x) => x.ok).length;
          console.log(`${c.green("✓ kicked")} ${r.member_did}  ${c.dim(`fanout: ${ok}/${r.fanout.length} ok`)}`);
          break;
        }
        case "grant-admin": {
          const [room_id, to] = rPos;
          if (!room_id || !to) { console.error("usage: air-msg room grant-admin <room_id> <did|air-id>"); process.exit(1); }
          const r = await core.roomGrantAdminOp({ room_id, to });
          console.log(`${c.green("✓ admin granted")}  mandate_id: ${r.mandate_id}`);
          break;
        }
        case "revoke-admin": {
          const [room_id, mandate_id] = rPos;
          if (!room_id || !mandate_id) { console.error("usage: air-msg room revoke-admin <room_id> <mandate_id>"); process.exit(1); }
          const r = await core.roomRevokeAdminOp({ room_id, mandate_id });
          console.log(`${c.green("✓ admin revoked")}  mandate_id: ${r.mandate_id}`);
          break;
        }
        case "send": {
          const [room_id, ...words] = rPos;
          if (!room_id || words.length === 0) { console.error("usage: air-msg room send <room_id> <text...>"); process.exit(1); }
          const r = await core.sendRoom({ room_id, text: words.join(" ") });
          const ok = r.report.filter((x) => x.ok).length;
          console.log(`${c.green("✓ sent")} seq ${r.sender_seq}  ${c.dim(`${ok}/${r.report.length} delivered`)}`);
          break;
        }
        case "list": {
          const r = core.roomListOp();
          if (r.count === 0) { console.log(c.dim("(no rooms)")); break; }
          for (const rm of r.rooms) {
            const role = rm.am_founder ? c.cyan("founder") : c.dim("member");
            const halted = rm.halted ? c.red(" [halted]") : "";
            console.log(`${c.bold(rm.name)}  ${role}${halted}  ${c.dim(rm.room_id)}`);
            console.log(`  ${rm.member_count} member(s)  thread: ${c.dim(rm.thread_id)}`);
          }
          break;
        }
        case "history": {
          const [room_id] = rPos;
          if (!room_id) { console.error("usage: air-msg room history <room_id> [--limit N]"); process.exit(1); }
          const r = core.roomHistoryOp({ room_id, limit: rFlags.limit ? Number(rFlags.limit) : 50 });
          console.log(`${r.count} message(s) in room ${room_id}`);
          for (const m of [...r.messages].reverse()) {
            const arrow = m.direction === "sent" ? "↑" : "↓";
            console.log(`  ${arrow} ${m.from}  ${c.dim(m.timestamp)}`);
            console.log(`    ${bodyText(m.body)}`);
          }
          break;
        }
        case "halt": {
          const [room_id] = rPos;
          if (!room_id) { console.error("usage: air-msg room halt <room_id>"); process.exit(1); }
          const r = await core.roomHaltOp({ room_id });
          console.log(`${c.green("✓ halted")} ${room_id}`);
          break;
        }
        case "resume": {
          const [room_id] = rPos;
          if (!room_id) { console.error("usage: air-msg room resume <room_id>"); process.exit(1); }
          const r = await core.roomResumeOp({ room_id });
          console.log(`${c.green("✓ resumed")} ${room_id}`);
          break;
        }
        case "sync": {
          const [room_id] = rPos;
          if (!room_id) { console.error("usage: air-msg room sync <room_id>"); process.exit(1); }
          const r = await core.roomRequestOp({ room_id });
          console.log(`${r.status === "sent" ? c.green("✓ sync request sent") : c.yellow("⚠ sync request failed")} → ${r.to}`);
          if (r.error) console.log(`  ${c.dim(r.error)}`);
          break;
        }
        default: {
          console.error(`unknown room subcommand: ${sub || "(none)"}`);
          console.log(`  room subcommands: create | invite | kick | grant-admin | revoke-admin | send | list | history | halt | resume | sync`);
          process.exit(1);
        }
      }
      break;
    }
    case "daemon": {
      const { sub } = parseRoomArgs(rest); // reuse the sub-arg splitter
      switch (sub) {
        case "start": {
          const { startDaemon } = await import("./daemon.mjs");
          await startDaemon();
          break;
        }
        case "stop": {
          const { readDaemonPid, clearDaemonPid } = await import("./daemon.mjs");
          const rec = readDaemonPid();
          if (!rec) { console.log(c.dim("daemon not running")); break; }
          try { process.kill(rec.pid, "SIGTERM"); console.log(`${c.green("✓ stopped")} ${c.dim("pid " + rec.pid)}`); }
          catch (e) { console.error(`could not signal pid ${rec.pid}: ${e.message}`); clearDaemonPid(); }
          break;
        }
        case "status": {
          const { daemonStatus } = await import("./daemon.mjs");
          const s = daemonStatus();
          console.log(`daemon: ${s.running ? c.green("running") : c.dim("stopped")}` +
            (s.pid ? `  ${c.dim("pid " + s.pid + (s.start_time ? " since " + s.start_time : ""))}` : ""));
          console.log(`cursor: ${s.cursor ?? "?"}`);
          if (s.running) {
            const { queryDaemonStatus, socketPath } = await import("./daemon-ipc.mjs");
            const live = await queryDaemonStatus();
            if (live) {
              const roles = live.clients.map((cl) => cl.role).join(", ") || "none";
              console.log(`socket: ${socketPath()}`);
              console.log(`clients: ${live.clients.length} (${roles})  ·  last relay_seq: ${live.last_seq ?? "—"}`);
              console.log(`sinks: ${(live.sinks ?? []).join(", ") || "?"}`);
            } else {
              console.log(c.yellow("socket: unreachable (PID alive but not answering — split-brain? check daemon logs)"));
            }
          }
          break;
        }
        default:
          console.error(`unknown daemon subcommand: ${sub || "(none)"}`);
          console.log(`  daemon subcommands: start | stop | status`);
          process.exit(1);
      }
      break;
    }
    case "health": {
      const r = await core.health();
      const ok = r.relay?.status === "ok";
      console.log(`relay: ${ok ? c.green("ok") : c.red(JSON.stringify(r.relay))}  ${c.dim(r.relay_url)}`);
      console.log(`you:   ${r.registered ? c.green(r.my_did) : c.yellow("not registered")}`);
      break;
    }
    case "help":
    case "--help":
    case "-h":
    case undefined:
      console.log(HELP);
      break;
    default:
      console.error(`unknown command: ${cmd}\n`);
      console.log(HELP);
      process.exit(1);
  }
}

// Run the CLI only when invoked directly (`node cli.mjs …` or the `air-msg` bin) — NOT when a
// test imports this module for its exported helpers (parseArgs/parseRoomArgs/bodyText).
// realpathSync resolves the `air-msg` bin symlink to this file so both entry points match;
// an unresolvable argv[1] (pathological) is treated as "not a direct invocation" rather than
// crashing the process at startup.
let invokedDirectly = false;
try {
  invokedDirectly = !!process.argv[1] && realpathSync(process.argv[1]) === fileURLToPath(import.meta.url);
} catch { /* argv[1] unresolvable — do not run main() */ }
if (invokedDirectly) {
  main().catch((e) => {
    console.error(c.red("error: ") + String(e.message ?? e));
    process.exit(1);
  });
}
