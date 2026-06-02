#!/usr/bin/env node
// agent-bridge-mcp — MCP server exposing the agent-messaging protocol to
// Claude Code / Codex / Gemini / Cursor / Cline. Thin wrapper over core.mjs
// (the shared brain that the CLI in cli.mjs also uses).
//
// Real cryptographic identity: first use auto-generates an Ed25519 key,
// registers with AIR, and publishes a relay inbox. Outgoing messages are
// signed; incoming messages are verified against the sender's AIR-resolved
// key and checked against pinned contact fingerprints (anti-phishing).
//
// Spec: https://agentidentityregistry.org/specs/air/draft-1
// Working name "air-msg" — final name TBD (~/SuperClaw/.omc/naming-research.md).

import { Server } from "@modelcontextprotocol/sdk/server/index.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import {
  CallToolRequestSchema,
  ListToolsRequestSchema,
} from "@modelcontextprotocol/sdk/types.js";
import { loadIdentity } from "./identity.mjs";
import * as core from "./core.mjs";

const SERVER_VERSION = core.CORE_VERSION;

const TOOLS = [
  {
    name: "agent_register",
    description:
      "Generate this agent's cryptographic identity and register it with AIR. " +
      "Idempotent. Run once; then you can send/receive signed messages. " +
      "Registration is self-verified tier; AIR Verified is earned via attestations.",
    inputSchema: {
      type: "object",
      properties: { name: { type: "string", description: "Optional display name in AIR." } },
    },
  },
  {
    name: "agent_my_status",
    description:
      "Show your AIR identity: AIR ID, DID, key fingerprint, trust score, and " +
      "AIR Verified status (attestation count + distinct org roots).",
    inputSchema: { type: "object", properties: {} },
  },
  {
    name: "agent_send",
    description:
      "Send a cryptographically signed message to another agent by DID, AIR ID, " +
      "or saved contact alias. The recipient can verify it really came from you.",
    inputSchema: {
      type: "object",
      properties: {
        to: { type: "string", description: "Recipient DID, AIR ID, or contact alias." },
        body: { oneOf: [{ type: "string" }, { type: "object", additionalProperties: true }], description: "Text or structured payload." },
        thread_id: { type: "string", description: "Optional thread UUID to continue a conversation." },
        in_reply_to: { type: "string", description: "Optional envelope id this replies to." },
        plaintext: { type: "boolean", description: "Send unencrypted on purpose (default false = end-to-end encrypted). Use only for agents that cannot decrypt." },
      },
      required: ["to", "body"],
    },
  },
  {
    name: "agent_receive",
    description:
      "Fetch messages addressed to you and VERIFY each signature against the " +
      "sender's AIR-resolved key. verified:false = untrusted. key_changed:true = " +
      "a known contact's key changed since you pinned it (possible compromise).",
    inputSchema: {
      type: "object",
      properties: {
        since: { type: "number", description: "Cursor from a prior call (0/omit for first)." },
        limit: { type: "number", description: "Max per batch (default 50, max 200)." },
      },
    },
  },
  {
    name: "agent_attest",
    description:
      "Cryptographically vouch for another agent — the building block of AIR " +
      "Verified. Signs a typed attestation and publishes it. You cannot attest yourself.",
    inputSchema: {
      type: "object",
      properties: {
        subject: { type: "string", description: "AIR ID of the agent you're vouching for." },
        attestation_type: { type: "string", enum: core.VALID_ATTESTATION_TYPES, description: "identity_verification | operator_confirmation | dependency | safety_review" },
        statement: { type: "string", description: "Optional human-readable note." },
      },
      required: ["subject", "attestation_type"],
    },
  },
  {
    name: "agent_add_contact",
    description:
      "Add a contact and PIN their current public key. Later messages whose key " +
      "differs from the pin are flagged (the 'security code changed' guarantee). " +
      "Verify the shown fingerprint out-of-band before trusting.",
    inputSchema: {
      type: "object",
      properties: {
        to: { type: "string", description: "Contact's DID or AIR ID." },
        alias: { type: "string", description: "Optional short name." },
      },
      required: ["to"],
    },
  },
  {
    name: "agent_search",
    description:
      "Search the public AIR registry for agents by name. Returns candidates with " +
      "AIR Verified status + trust score. Set verified_only to surface only Verified agents.",
    inputSchema: {
      type: "object",
      properties: {
        query: { type: "string", description: "Name or substring to search for." },
        verified_only: { type: "boolean", description: "Only AIR Verified agents (default false)." },
        limit: { type: "number", description: "Max results (default 20)." },
      },
      required: ["query"],
    },
  },
  {
    name: "agent_list_contacts",
    description: "List saved contacts: alias, AIR ID, pinned fingerprint, verified status, last seen.",
    inputSchema: { type: "object", properties: {} },
  },
  {
    name: "agent_show_invite",
    description:
      "Show your shareable identity card (DID + AIR ID + fingerprint) so someone " +
      "can add YOU. Share the fingerprint over a trusted channel for OOB verification.",
    inputSchema: { type: "object", properties: {} },
  },
  {
    name: "agent_health",
    description: "Relay liveness + queue stats + your registration status. Good first troubleshooting call.",
    inputSchema: { type: "object", properties: {} },
  },
  {
    name: "agent_history",
    description: "Read your saved message history from the local archive (sent + received).",
    inputSchema: {
      type: "object",
      properties: {
        peer: { type: "string", description: "Filter to one conversation: a DID, AIR id, or contact alias." },
        thread: { type: "string", description: "Filter to one thread id." },
        limit: { type: "number", description: "Max messages to return (default 50)." },
      },
    },
  },
];

// Map each tool to a core operation.
const HANDLERS = {
  agent_register: (a) => core.register(a),
  agent_my_status: () => core.myStatus(),
  agent_send: (a) => core.send(a),
  agent_receive: (a) => core.receive(a),
  agent_attest: (a) => core.attest(a),
  agent_add_contact: (a) => core.addContactOp(a),
  agent_search: (a) => core.search(a),
  agent_list_contacts: () => core.listContactsOp(),
  agent_show_invite: () => core.showInvite(),
  agent_health: () => core.health(),
  agent_history: (a) => core.historyOp(a),
};

const server = new Server(
  { name: "agent-bridge-mcp", version: SERVER_VERSION },
  { capabilities: { tools: {} } }
);

server.setRequestHandler(ListToolsRequestSchema, async () => ({ tools: TOOLS }));

server.setRequestHandler(CallToolRequestSchema, async (request) => {
  const { name, arguments: args } = request.params;
  const handler = HANDLERS[name];
  if (!handler) {
    return { isError: true, content: [{ type: "text", text: `unknown tool: ${name}` }] };
  }
  try {
    const result = await handler(args ?? {});
    return { content: [{ type: "text", text: JSON.stringify(result, null, 2) }] };
  } catch (e) {
    return {
      isError: true,
      content: [{ type: "text", text: JSON.stringify({ error: String(e.message ?? e), tool: name }, null, 2) }],
    };
  }
});

const transport = new StdioServerTransport();
await server.connect(transport);
const boot = loadIdentity();
process.stderr.write(
  `agent-bridge-mcp v${SERVER_VERSION} ready ` +
    `(${boot ? `registered as ${boot.air_id}` : "no identity yet — bootstraps on first send"})\n`
);
