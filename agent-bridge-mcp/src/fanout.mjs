// src/fanout.mjs — deliver one received message to every daemon sink, in isolation.
// A sink is { name: string, deliver: (message) => void | Promise<void> }. One sink that throws
// or rejects must never block the others or bubble into the daemon's single receive loop, so each
// deliver() is wrapped and the fan-out runs them concurrently.

/** Deliver `message` to every sink, isolating per-sink failures (logged, never thrown). */
export async function fanOut(message, sinks, log = (s) => process.stderr.write(s + "\n")) {
  await Promise.all((sinks ?? []).map(async (sink) => {
    try {
      await sink.deliver(message);
    } catch (err) {
      log(`[daemon] sink "${sink.name}" failed: ${err?.message ?? err}`);
    }
  }));
}
