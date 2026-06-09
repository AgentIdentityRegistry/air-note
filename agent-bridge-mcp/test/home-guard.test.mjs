import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { bridgeHome } from "../src/identity.mjs";

// Under the node test runner (NODE_TEST_CONTEXT set), bridgeHome() must refuse
// to fall through to the real ~/.air-msg — a test that forgets AGENT_BRIDGE_HOME
// would otherwise write fixture data into the user's live store.
test("bridgeHome: refuses the real home under the test runner, honors AGENT_BRIDGE_HOME", () => {
  const saved = process.env.AGENT_BRIDGE_HOME;
  try {
    delete process.env.AGENT_BRIDGE_HOME;
    assert.throws(() => bridgeHome(), /AGENT_BRIDGE_HOME/);

    const dir = mkdtempSync(join(tmpdir(), "air-msg-guard-"));
    try {
      process.env.AGENT_BRIDGE_HOME = dir;
      assert.equal(bridgeHome(), dir);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  } finally {
    if (saved === undefined) delete process.env.AGENT_BRIDGE_HOME;
    else process.env.AGENT_BRIDGE_HOME = saved;
  }
});
