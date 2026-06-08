// src/daemon-sinks.mjs — in-process sinks for the receiver daemon. A sink wraps an existing
// delivery surface (OS banner, …) behind the { name, deliver(message) } contract fanOut expects.
import { shortPeer } from "./peers.mjs";
import { bodyText } from "./watch.mjs";

/** OS-banner sink: ring the local notifier per received message, honoring a mute set
 *  (alias OR DID OR short AIR-id). `notifier` is the object from createNotifier().
 *  Click-to-open for the daemon banner is a deliberate later-phase item — not wired here. */
export function bannerSink({ notifier, mute = new Set() } = {}) {
  return {
    name: "banner",
    deliver: async (m) => {
      const alias = m.contact;
      const airId = shortPeer(m.from);
      if (mute.has(alias) || mute.has(m.from) || mute.has(airId)) return;
      await notifier.notify({ title: alias || airId, message: bodyText(m.body) });
    },
  };
}
