import type { ArchiveRow, InboxMessage } from "../api/inbox";
import { bodyText } from "./bodyText";

/** The one shape the inbox UI renders. `status`/`retryable`/`reason`/`correlationId` set only for optimistic sends. */
export type ThreadItem = {
  envelope_id: string; direction: "received" | "sent"; peer_did: string; room_id: string | null;
  from: string; to: string | null; timestamp: string; body: unknown;
  encrypted: boolean; verified: boolean; key_changed: boolean; spam: boolean;
  status?: "pending" | "ok" | "err"; retryable?: boolean; reason?: string; correlationId?: string;
};

export type Conversation = {
  convKey: string; kind: "room" | "peer"; lastTimestamp: string; lastText: string; unread: number;
};

export const convKey = (x: { room_id: string | null; peer_did: string }): string => x.room_id ?? x.peer_did;

export function fromArchiveRow(r: ArchiveRow): ThreadItem {
  return {
    envelope_id: r.envelope_id, direction: r.direction, peer_did: r.peer_did, room_id: r.room_id,
    from: r.from, to: r.to, timestamp: r.timestamp, body: r.body, encrypted: r.encrypted,
    verified: r.verified, key_changed: r.key_changed, spam: r.spam, status: "ok",
  };
}

/** Live viewer messages are always received; the peer is the sender. */
export function fromLiveMessage(m: InboxMessage): ThreadItem {
  return {
    envelope_id: m.envelope_id, direction: "received", peer_did: m.from, room_id: m.room_id ?? null,
    from: m.from, to: null, timestamp: m.received_at, body: m.body, encrypted: m.encrypted,
    verified: m.verified, key_changed: m.key_changed === true, spam: false, status: "ok",
  };
}

/** An optimistic sent row, shown immediately; resolved by the send-ok/err ack (Task 6). 1:1 only in v1. */
export function makeOptimistic(correlationId: string, to: string, body: unknown, timestamp: string): ThreadItem {
  return {
    envelope_id: `pending:${correlationId}`, direction: "sent", peer_did: to, room_id: null,
    from: "", to, timestamp, body, encrypted: true, verified: true, key_changed: false, spam: false,
    status: "pending", correlationId,
  };
}

/** First-occurrence wins. Callers MUST pass confirmed rows before optimistic ones so a
 *  pending/confirmed clash on the same envelope_id keeps the confirmed row (design §3 cross-stream dedupe). */
export function dedupeById<T extends { envelope_id: string }>(items: T[]): T[] {
  const seen = new Map<string, T>();
  for (const it of items) if (!seen.has(it.envelope_id)) seen.set(it.envelope_id, it);
  return [...seen.values()];
}

/** Group items into conversations, newest-first. `unreadIds` = envelope_ids counted as unread. */
export function groupConversations(items: ThreadItem[], unreadIds: Set<string>): Conversation[] {
  const map = new Map<string, Conversation>();
  for (const it of items) {
    const key = convKey(it);
    const prev = map.get(key);
    const isNewer = !prev || it.timestamp > prev.lastTimestamp;
    map.set(key, {
      convKey: key, kind: it.room_id ? "room" : "peer",
      lastTimestamp: isNewer ? it.timestamp : prev!.lastTimestamp,
      lastText: isNewer ? bodyText(it.body) : prev!.lastText,
      unread: (prev?.unread ?? 0) + (unreadIds.has(it.envelope_id) ? 1 : 0),
    });
  }
  return [...map.values()].sort((a, b) => (a.lastTimestamp < b.lastTimestamp ? 1 : -1));
}
