// adapters/telegram.mjs — the ONLY Telegram-specific module. Implements the bridge
// adapter seam: send(ping) and listen({signal,onReply}), plus captureFirstChat for setup.
// Outbound sends are serialized + retried once on 429 (rate-limit safety). Inbound uses
// getUpdates long-polling (no public server). All HTTP injected via fetchImpl for tests;
// the persisted update offset is read/advanced through getOffset/setOffset (bridge-routes)
// so a restart resumes and a redelivered update is never double-processed.

const API = (token, method) => `https://api.telegram.org/bot${token}/${method}`;

const sleep = (ms, signal) => new Promise((res) => {
  const t = setTimeout(res, ms);
  signal?.addEventListener("abort", () => { clearTimeout(t); res(); }, { once: true });
});

export function createTelegramAdapter({
  token, chatId, fetchImpl = fetch, getOffset = () => 0, setOffset = () => {},
  longPollSecs = 25, signal, log = (s) => process.stderr.write(s + "\n"),
}) {
  const chat = Number(chatId);
  let chain = Promise.resolve(); // serialize outbound sends (Telegram ~1 msg/s per chat)

  async function rawSend(text, replyToId) {
    const params = { chat_id: chat, text }; // NO parse_mode — text is untrusted
    if (replyToId) params.reply_to_message_id = Number(replyToId);
    for (let attempt = 0; ; attempt++) {
      const resp = await fetchImpl(API(token, "sendMessage"), {
        method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(params),
      });
      if (resp.status === 429 && attempt < 1) {
        let retry = 1;
        try { retry = (await resp.json())?.parameters?.retry_after ?? 1; } catch { /* ignore */ }
        await sleep(retry * 1000, signal);
        if (signal?.aborted) return null; // shutting down → stop retrying, degrade
        continue;
      }
      const data = await resp.json();
      if (!data.ok) throw new Error(`telegram sendMessage failed: ${data.description ?? resp.status}`);
      return String(data.result.message_id);
    }
  }

  return {
    name: "telegram",

    /** Send one ping. Serialized; returns the message id, or null if it failed (degrade). */
    async send({ title, body }) {
      const text = `${title}\n${body}`;
      const p = chain.then(() => rawSend(text))
        .catch((e) => { log(`[telegram] send: ${e.message ?? e}`); return null; });
      chain = p.then(() => {}, () => {});
      return p;
    },

    /** Long-poll getUpdates until aborted. Filters to the saved chat; per reply, awaits
     *  onReply, then advances the offset only past SUCCESSFUL updates (at-least-once). */
    async listen({ signal, onReply }) {
      let backoff = 1000;
      while (!signal?.aborted) {
        const offset = (() => { try { return getOffset(); } catch { return 0; } })();
        let updates;
        try {
          const url = API(token, "getUpdates") + `?timeout=${longPollSecs}&offset=${offset}`;
          const resp = await fetchImpl(url, { signal });
          const data = await resp.json();
          if (!data.ok) throw new Error(data.description ?? "getUpdates not ok");
          updates = data.result ?? [];
          backoff = 1000;
        } catch (e) {
          if (signal?.aborted) break;
          log(`[telegram] getUpdates: ${e.message ?? e}`);
          await sleep(backoff, signal);
          backoff = Math.min(backoff * 2, 5000);
          continue;
        }

        let lastOk = offset - 1;
        for (const u of updates) {
          const msg = u.message;
          if (!msg || Number(msg.chat?.id) !== chat || typeof msg.text !== "string") {
            lastOk = u.update_id; continue;
          }
          const replyToExternalId = msg.reply_to_message ? String(msg.reply_to_message.message_id) : null;
          try {
            await onReply({ replyToExternalId, text: msg.text, reply: (t) => rawSend(t, msg.message_id) });
            lastOk = u.update_id;
          } catch (e) {
            log(`[telegram] onReply failed (update ${u.update_id}): ${e.message ?? e}`);
            break;
          }
        }
        if (lastOk >= offset) { try { setOffset(lastOk + 1); } catch (e) { log(`[telegram] setOffset: ${e.message ?? e}`); } }
      }
    },
  };
}

/** Setup helper: poll getUpdates until the first message arrives; return its chat id.
 *  Used by `air-msg bridge setup` to capture the user's chat id after they /start the bot. */
export async function captureFirstChat({ token, fetchImpl = fetch, signal, pollDelayMs = 2000, maxPolls = 150, log = (s) => process.stderr.write(s + "\n") }) {
  for (let i = 0; i < maxPolls && !signal?.aborted; i++) {
    try {
      const resp = await fetchImpl(API(token, "getUpdates") + `?timeout=0&offset=0`, { signal });
      const data = await resp.json();
      if (!data.ok) throw new Error(data.description ?? "getUpdates not ok");
      const withChat = (data.result ?? []).find((u) => u.message?.chat?.id != null);
      if (withChat) return Number(withChat.message.chat.id);
    } catch (e) {
      log(`[telegram] captureFirstChat poll: ${e.message ?? e}`); // transient — keep polling
    }
    await sleep(pollDelayMs, signal);
  }
  return null;
}
