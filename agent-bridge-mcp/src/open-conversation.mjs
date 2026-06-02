// open-conversation.mjs — resolve a notification click into an argv that opens
// the conversation. Mechanism, not policy: the daemon fires an event; THIS decides
// what opens (a terminal showing history, an AI session, or a custom command/URL),
// driven by AIRMSG_OPEN. Built-in modes return a shell-free argv; only command:<tmpl>
// opts into `sh -c`, and even then placeholders are sanitized first.

import { spawn } from "node:child_process";
import { existsSync } from "node:fs";

/** POSIX single-quote a string for use inside a shell command line. */
export function shq(s) {
  return `'${String(s).replace(/'/g, `'\\''`)}'`;
}

/** Reduce an identifier (DID / AIR-id / alias) to shell-inert characters.
 *  DIDs use [A-Za-z0-9:._-]; everything else (quotes, spaces, ;, |, $, …) is dropped. */
export function sanitizeId(s) {
  return String(s ?? "").replace(/[^A-Za-z0-9:._-]/g, "");
}

/** AppleScript double-quoted string literal (escape backslash + double-quote). */
function appleStr(s) {
  return `"${String(s).replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
}

/** Wrap a shell command line so it runs in a NEW terminal window. Returns argv. */
export function wrapInTerminal(innerCmd, platform = process.platform) {
  if (platform === "darwin") {
    return [
      "osascript",
      "-e", `tell application "Terminal" to do script ${appleStr(innerCmd)}`,
      "-e", `tell application "Terminal" to activate`,
    ];
  }
  if (platform === "win32") {
    return ["cmd", "/c", "start", "cmd", "/k", innerCmd];
  }
  return ["x-terminal-emulator", "-e", "bash", "-lc", `${innerCmd}; exec bash`];
}

// Per-AI-CLI invocation. Each takes the prompt and returns the inner shell command.
const AI_PRESETS = {
  claude: (prompt) => `claude ${shq(prompt)}`,
  codex: (prompt) => `codex ${shq(prompt)}`,
  gemini: (prompt) => `gemini -i ${shq(prompt)}`,
};

/**
 * Resolve a click action into argv (or null).
 * Pure: all environment is via opts. For `ai` mode, the caller is responsible for
 * auto-detecting the AI CLI (pass `aiCmd: detectAiCmd()`); when omitted, defaults to "claude".
 * @returns {string[]|null}
 */
export function resolveOpenCommand(peer, {
  alias, thread,
  mode = process.env.AIRMSG_OPEN || "terminal-history",
  aiCmd,
  airMsgBin = process.env.AIR_MSG_BIN || "air-msg",
  platform = process.platform,
} = {}) {
  const p = sanitizeId(peer);
  if (mode === "none") return null;

  // airMsgBin is intentionally unquoted: it may be a multi-token command
  // (e.g. AIR_MSG_BIN="node /abs/cli.mjs"). The peer (p) is already sanitized.
  if (mode === "terminal-history") {
    return wrapInTerminal(`${airMsgBin} history --with ${p}`, platform);
  }

  if (mode === "ai") {
    const tool = aiCmd || "claude";
    const build = AI_PRESETS[tool] || ((prompt) => `${tool} ${shq(prompt)}`);
    const prompt =
      `Open my air-msg chat with ${p}. Run "${airMsgBin} history --with ${p}", ` +
      `show me the latest message, and help me reply.`;
    return wrapInTerminal(build(prompt), platform);
  }

  if (mode.startsWith("command:")) {
    const tmpl = mode.slice("command:".length);
    const expanded = tmpl
      .replaceAll("{peer}", p)
      .replaceAll("{alias}", sanitizeId(alias || ""))
      .replaceAll("{thread}", sanitizeId(thread || ""));
    return ["sh", "-c", expanded];
  }

  return null; // unknown mode → notify-only
}

/** Auto-detect the first AI CLI on PATH (claude → codex → gemini). Best-effort. */
export function detectAiCmd(env = process.env, candidates = ["claude", "codex", "gemini"]) {
  const dirs = (env.PATH || "").split(":").filter(Boolean);
  for (const c of candidates) {
    for (const d of dirs) {
      if (existsSync(`${d}/${c}`)) return c;
    }
  }
  return undefined;
}

/** Spawn a resolved click argv, detached, best-effort (never throws). */
export function runOpenCommand(argv) {
  if (!argv || !argv.length) return;
  try {
    spawn(argv[0], argv.slice(1), { detached: true, stdio: "ignore" }).unref();
  } catch (err) {
    process.stderr.write(`[open] ${err.message ?? err}\n`);
  }
}
