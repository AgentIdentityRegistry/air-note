// src/service.mjs — auto-start unit generators for `air-msg daemon install` (spec §8).
// PURE string generators + a platform plan: the CLI does the file/exec I/O, tests assert content
// (spec §9: the load step itself is verified manually on a real box). Absolute paths everywhere:
// launchd/systemd provide no user PATH, so the HELP-text `/usr/bin/env air-msg` idiom cannot work.
import { join } from "node:path";

export const SERVICE_LABEL = "org.air-msg.daemon";
export const SYSTEMD_UNIT_NAME = "air-msg-daemon.service";

const xmlEscape = (s) => String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

/** macOS LaunchAgent: run the daemon at login and keep it alive. `home` (optional) pins
 *  AGENT_BRIDGE_HOME for non-default homes; `logPath` is computed by servicePlan so the log
 *  always sits beside the REAL store (~/.air-msg by default — never /tmp, which is world-readable
 *  and cleared on reboot; critic v1 note).
 *  PRECONDITION: launchd opens StandardOutPath/StandardErrorPath BEFORE spawning the process and
 *  does NOT create missing parent directories — the installer must mkdir the log directory first
 *  (Task 9 does this via `mkdirSync(dirname(plan.logPath), { recursive: true })`). */
export function launchdPlist({ nodePath, cliPath, home, logPath }) {
  return `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>${SERVICE_LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>${xmlEscape(nodePath)}</string>
    <string>${xmlEscape(cliPath)}</string>
    <string>daemon</string>
    <string>start</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>${home ? `
  <key>EnvironmentVariables</key>
  <dict><key>AGENT_BRIDGE_HOME</key><string>${xmlEscape(home)}</string></dict>` : ""}
  <key>StandardOutPath</key><string>${xmlEscape(logPath)}</string>
  <key>StandardErrorPath</key><string>${xmlEscape(logPath)}</string>
</dict>
</plist>
`;
}

/** Linux systemd-user unit: same contract as the LaunchAgent.
 *  QUOTING BOUNDARY (critic v1 H1): systemd does its own word-splitting — double quotes group
 *  tokens per systemd.service(5) — but this generator is content-tested only. The enable/--now
 *  load is covered by the repeatable `systemd-smoke` workflow (real ubuntu systemd; spaced-home
 *  Environment= quoting + Restart=always + uninstall — first green 2026-06-11). Re-run it after
 *  any change to this generator. Pathological-input caveats remain OUTSIDE that smoke's coverage: `%` is a systemd specifier prefix in unit values (a literal
 *  % needs %% doubling); a literal `"` or `\` inside quoted ExecStart tokens would also break
 *  C-style quoting — not fixed here, noted as a known boundary. */
export function systemdUnit({ nodePath, cliPath, home }) {
  return `[Unit]
Description=AIR Note receiver daemon (air-msg daemon start)

[Service]
ExecStart="${nodePath}" "${cliPath}" daemon start${home ? `
Environment="AGENT_BRIDGE_HOME=${home}"` : ""}
Restart=always
RestartSec=2

[Install]
WantedBy=default.target
`;
}

/** Decide file path + content + load/unload commands for this platform; null = unsupported
 *  (spec §2: Windows auto-start is v2 — no named-pipe ACL guarantee, no test box). */
export function servicePlan({ platform, homedir, nodePath, cliPath, home }) {
  // Log beside the resolved store: an explicit home, or bridgeHome()'s default ~/.air-msg.
  const logPath = join(home ?? join(homedir, ".air-msg"), "daemon.log");
  if (platform === "darwin") {
    const file = join(homedir, "Library", "LaunchAgents", `${SERVICE_LABEL}.plist`);
    return {
      kind: "launchd",
      file,
      logPath,
      content: launchdPlist({ nodePath, cliPath, home, logPath }),
      loadCmd: ["launchctl", "load", "-w", file],
      unloadCmd: ["launchctl", "unload", "-w", file],
    };
  }
  if (platform === "linux") {
    const file = join(homedir, ".config", "systemd", "user", SYSTEMD_UNIT_NAME);
    // stdout goes to journald by default on systemd; logPath is returned for API symmetry
    // and any future use — no daemon.log is written by systemd itself.
    return {
      kind: "systemd",
      file,
      logPath,
      content: systemdUnit({ nodePath, cliPath, home }),
      loadCmd: ["systemctl", "--user", "enable", "--now", SYSTEMD_UNIT_NAME],
      unloadCmd: ["systemctl", "--user", "disable", "--now", SYSTEMD_UNIT_NAME],
    };
  }
  return null;
}
