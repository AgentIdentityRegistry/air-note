// bridge-config.mjs — chat-app bridge config (bot token + chat id) at <home>/bridge.json,
// mode 0600 (same secret discipline as identity.json/contacts.json). The token is a secret:
// it lives ONLY in this file — never in sqlite, never echoed back.

import { readFileSync, writeFileSync, existsSync, chmodSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import { bridgeHome } from "./identity.mjs";

const configPath = (home) => join(home, "bridge.json");

export function loadBridgeConfig({ home = bridgeHome() } = {}) {
  const path = configPath(home);
  if (!existsSync(path)) return null;
  // A malformed bridge.json throws (same as loadIdentity/loadContacts) so a corrupted
  // config surfaces as an error rather than silently looking "not configured".
  return JSON.parse(readFileSync(path, "utf8"));
}

export function saveBridgeConfig(cfg, { home = bridgeHome() } = {}) {
  mkdirSync(home, { recursive: true, mode: 0o700 });
  const path = configPath(home);
  writeFileSync(path, JSON.stringify(cfg, null, 2), { mode: 0o600 });
  try { chmodSync(path, 0o600); } catch { /* best effort on non-POSIX */ }
  return path;
}
