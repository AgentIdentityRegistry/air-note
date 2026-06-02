import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { loadBridgeConfig, saveBridgeConfig } from "../src/bridge-config.mjs";

let dir;
beforeEach(() => { dir = mkdtempSync(join(tmpdir(), "air-msg-cfg-")); });
afterEach(() => { rmSync(dir, { recursive: true, force: true }); });

test("load on a fresh dir returns null", () => {
  assert.equal(loadBridgeConfig({ home: dir }), null);
});

test("save then load round-trips", () => {
  saveBridgeConfig({ telegram: { bot_token: "T", chat_id: 555 } }, { home: dir });
  const cfg = loadBridgeConfig({ home: dir });
  assert.equal(cfg.telegram.bot_token, "T");
  assert.equal(cfg.telegram.chat_id, 555);
});

test("the config file is created mode 0600", () => {
  const path = saveBridgeConfig({ telegram: { bot_token: "T", chat_id: 1 } }, { home: dir });
  assert.equal(statSync(path).mode & 0o777, 0o600);
});
