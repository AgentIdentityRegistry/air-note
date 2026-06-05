// src/daemon.mjs — the always-on receiver daemon. Owns the single consumer lock, runs ONE
// watch() loop, and fans every received message out to its in-process sinks. The Phase 2 socket
// layer attaches additional dynamic sinks to the same fanOut; this phase wires in-process only.
import { watch } from "./watch.mjs";
import { fanOut } from "./fanout.mjs";

/** Run the daemon: drive watch() with an onMessage that fans out to `sinks`. Injectable for tests. */
export async function runDaemon({ identity, sinks, signal, watchFn = watch, log = (s) => process.stderr.write(s + "\n") }) {
  log(`[daemon] up: ${identity.did} · sinks: ${sinks.map((s) => s.name).join(", ") || "(none)"}`);
  await watchFn({
    signal,
    identity,
    notifier: { notify: async () => {} }, // the banner is a SINK now, not watch's own notifier
    openResolver: () => null,
    onMessage: (m) => fanOut(m, sinks, log),
  });
}
