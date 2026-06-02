// notifier.mjs — the "how to ring" seam. One notify() over a chosen backend:
//   macOS    → osascript  (built-in, reliably displays; click opens Script Editor, not the chat)
//   win/linux → node-notifier (clickable banners) → terminal bell
// Auto-detected; override with AIRMSG_NOTIFY (node-notifier | osascript | bell | none).
// macOS prefers osascript because the bundled terminal-notifier (node-notifier's macOS
// engine) silently fails to display on modern macOS (Darwin 25+ tightened notification
// signing) — proven on a live machine where only osascript banners appeared.
// Always best-effort: a broken notifier must never crash the watcher.

import { spawn } from "node:child_process";

/** Pure: argv for `osascript` to show a banner. */
export function buildOsascriptArgs({ title, message }) {
  // (same AppleScript escape as appleStr() in open-conversation.mjs — keep in sync)
  const esc = (s) =>
    String(s)
      .replace(/\\/g, "\\\\")
      .replace(/"/g, '\\"')
      .replace(/[\r\n\t]/g, " "); // AppleScript string literals can't contain raw newlines
  return ["-e", `display notification "${esc(message)}" with title "${esc(title)}"`];
}

const tryLoadNotifier = async () => (await import("node-notifier")).default;

/** Resolve the backend name. node-notifier load is injectable for tests. */
export async function resolveBackend({
  env = process.env,
  platform = process.platform,
  loadNotifier = tryLoadNotifier,
} = {}) {
  const override = env.AIRMSG_NOTIFY;
  if (override) return override; // node-notifier | osascript | bell | none
  // macOS: prefer the built-in osascript banner — it reliably displays, whereas the
  // bundled terminal-notifier (node-notifier's macOS engine) silently fails on modern
  // macOS. Opt into clickable banners with AIRMSG_NOTIFY=node-notifier where it works.
  if (platform === "darwin") return "osascript";
  try { await loadNotifier(); return "node-notifier"; }
  catch { return "bell"; }
}

/**
 * Build a notifier bound to a backend.
 * @param {object} opts
 * @param {(cmd:string[])=>void} [opts.onClick]  called with the resolved openCommand on click
 * @param {Function} [opts.loadNotifier]         inject node-notifier (tests)
 * @returns {Promise<{backend:string, notify:Function}>}
 */
export async function createNotifier({ onClick, loadNotifier = tryLoadNotifier, spawnFn = spawn } = {}) {
  const backend = await resolveBackend({ loadNotifier });
  let nn = null;
  if (backend === "node-notifier") {
    try { nn = await loadNotifier(); }
    catch { /* will fall back to terminal bell in notify() */ }
  }
  return {
    backend,
    async notify({ title, message, openCommand } = {}) {
      try {
        if (backend === "none") return;
        if (backend === "node-notifier" && nn) {
          nn.notify({ title, message, wait: true, sound: true }, (err, response, metadata) => {
            try {
              const clicked = !err && (response === "activate"
                || metadata?.activationType === "contentsClicked");
              if (clicked && openCommand && onClick) onClick(openCommand);
            } catch (cbErr) {
              process.stderr.write(`[notifier] click handler error: ${cbErr.message ?? cbErr}\n`);
            }
          });
          return;
        }
        if (backend === "osascript") {
          spawnFn("osascript", buildOsascriptArgs({ title, message }), { stdio: "ignore" }).unref();
          return;
        }
        // bell (and any unknown backend): ring the terminal.
        process.stdout.write("\x07");
      } catch (err) {
        process.stderr.write(`[notifier] ${err.message ?? err}\n`);
      }
    },
  };
}
