import { test } from "node:test";
import assert from "node:assert/strict";
import { launchdPlist, systemdUnit, servicePlan, SERVICE_LABEL } from "../src/service.mjs";

const ARGS = { nodePath: "/opt/node 22/bin/node", cliPath: "/Users/me/air-note/agent-bridge-mcp/src/cli.mjs", home: "/Users/me/.air-msg", logPath: "/Users/me/.air-msg/daemon.log" };

test("launchdPlist: absolute paths, keepalive, run-at-load, log path, XML-escaped", () => {
  const xml = launchdPlist(ARGS);
  assert.match(xml, new RegExp(`<string>${SERVICE_LABEL}</string>`));
  assert.match(xml, /<string>\/opt\/node 22\/bin\/node<\/string>/);   // process.execPath verbatim (spaces fine in plist strings)
  assert.match(xml, /<string>daemon<\/string>\s*<string>start<\/string>/);
  assert.match(xml, /<key>RunAtLoad<\/key>\s*<true\/>/);
  assert.match(xml, /<key>KeepAlive<\/key>\s*<true\/>/);
  assert.match(xml, /daemon\.log/);
  assert.match(xml, /AGENT_BRIDGE_HOME/);                              // home was given → env var set
  const noHome = launchdPlist({ ...ARGS, home: undefined });
  assert.doesNotMatch(noHome, /AGENT_BRIDGE_HOME/);                    // default home → no env override
  const esc = launchdPlist({ ...ARGS, cliPath: "/tmp/a&b<c>/cli.mjs" });
  assert.match(esc, /a&amp;b&lt;c&gt;/);                               // XML entities escaped
});

test("systemdUnit: quoted ExecStart, Restart=always, default.target", () => {
  // CONTENT assertion ONLY (critic v1 H1): this proves what we EMIT, not that systemd parses it.
  // systemd does its own word-splitting (double quotes group tokens per systemd.service(5)), but
  // no systemd exists in this environment — the `systemctl --user enable --now` load is a
  // MANUAL smoke on a real Linux box before the systemd path is trusted (stated in the PR body).
  const unit = systemdUnit(ARGS);
  assert.match(unit, /ExecStart="\/opt\/node 22\/bin\/node" "\/Users\/me\/air-note\/agent-bridge-mcp\/src\/cli\.mjs" daemon start/);
  assert.match(unit, /Restart=always/);
  assert.match(unit, /Environment="AGENT_BRIDGE_HOME=/);
  assert.match(unit, /WantedBy=default\.target/);
  assert.doesNotMatch(systemdUnit({ ...ARGS, home: undefined }), /AGENT_BRIDGE_HOME/);
});

test("servicePlan: darwin → launchd plist under LaunchAgents; linux → systemd-user unit; else null", () => {
  const mac = servicePlan({ platform: "darwin", homedir: "/Users/me", nodePath: ARGS.nodePath, cliPath: ARGS.cliPath });
  assert.equal(mac.kind, "launchd");
  assert.equal(mac.file, `/Users/me/Library/LaunchAgents/${SERVICE_LABEL}.plist`);
  assert.deepEqual(mac.loadCmd, ["launchctl", "load", "-w", mac.file]);
  assert.deepEqual(mac.unloadCmd, ["launchctl", "unload", "-w", mac.file]);
  assert.match(mac.content, /<plist/);
  assert.match(mac.content, /\/Users\/me\/\.air-msg\/daemon\.log/);   // default home → log beside the real store, never /tmp
  assert.equal(mac.logPath, "/Users/me/.air-msg/daemon.log");          // installer uses this to mkdir the log directory
  const lin = servicePlan({ platform: "linux", homedir: "/home/me", nodePath: ARGS.nodePath, cliPath: ARGS.cliPath });
  assert.equal(lin.kind, "systemd");
  assert.equal(lin.file, "/home/me/.config/systemd/user/air-msg-daemon.service");
  assert.deepEqual(lin.loadCmd, ["systemctl", "--user", "enable", "--now", "air-msg-daemon.service"]);
  assert.deepEqual(lin.unloadCmd, ["systemctl", "--user", "disable", "--now", "air-msg-daemon.service"]);
  assert.equal(lin.logPath, "/home/me/.air-msg/daemon.log");           // returned for API symmetry; journald owns stdout on linux
  assert.equal(servicePlan({ platform: "win32", homedir: "C:\\u", nodePath: "n", cliPath: "c" }), null);   // spec §2: Windows is v2
});
