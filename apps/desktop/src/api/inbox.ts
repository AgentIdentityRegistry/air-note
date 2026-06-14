import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

// ── Contract types (mirror crates/air-rs/src/inbox/*) ───────────────────────────

export type InboxMessage = {
  seq: number; relay_seq: number; envelope_id: string; from: string;
  verified: boolean; encrypted: boolean; received_at: string;
  contact?: string; key_changed?: boolean; thread_id?: string; room_id?: string; body?: unknown;
};

export type ArchiveRow = {
  envelope_id: string; direction: "received" | "sent"; thread_id: string; peer_did: string;
  from: string; to: string; timestamp: string; body: unknown;
  encrypted: boolean; verified: boolean; key_changed: boolean; spam: boolean;
  relay_seq: number | null; room_id: string | null; archived_at: string;
};

export type ConversationSummary = {
  conv_key: string; kind: "room" | "peer"; last_timestamp: string; count: number;
};

export type Adoption =
  | { state: "adopted"; did: string; air_id: string; name: string | null; dormant_did: string | null }
  | { state: "needs_daemon" };

export type InboxStatus = {
  home: string; socket_exists: boolean; identity_exists: boolean; archive_exists: boolean;
};

export type Autonomy = "off" | "draft" | "auto";

// ── Command wrappers (Tauri v2: JS camelCase → Rust snake_case, D5) ──────────────

export const inboxStatus = () => invoke<InboxStatus>("inbox_status");
export const inboxIdentity = (desktopPriorDid?: string) =>
  invoke<Adoption>("inbox_identity", { desktopPriorDid });
export const inboxStart = () => invoke<void>("inbox_start");
export const inboxStop = () => invoke<void>("inbox_stop");
/** Returns the correlation id; the ack arrives as an `inbox_send_ok`/`inbox_send_err` event. */
export const inboxSend = (to: string, body: unknown, threadId?: string, inReplyTo?: string) =>
  invoke<string>("inbox_send", { to, body, threadId, inReplyTo });
export const inboxConversations = () => invoke<ConversationSummary[]>("inbox_conversations");
/** peer XOR room XOR neither (recent across peers). */
export const inboxHistory = (peer?: string, room?: string, limit?: number, includeSpam?: boolean) =>
  invoke<ArchiveRow[]>("inbox_history", { peer, room, limit, includeSpam });
export const inboxPolicyGet = (did: string) => invoke<Autonomy>("inbox_policy_get", { did });
export const inboxPolicySet = (did: string, value: Autonomy) =>
  invoke<void>("inbox_policy_set", { did, value });

// ── Event payloads + a typed subscribe helper ───────────────────────────────────

export type InboxEvents = {
  inbox_attached: { pid: number; did: string };
  inbox_detached: Record<string, never>;
  inbox_offline: Record<string, never>;
  inbox_message: InboxMessage;
  inbox_send_ok: { id: string; envelope_id: string; encrypted: boolean };
  inbox_send_err: { id: string; retryable: boolean; reason: string };
};

export function onInboxEvent<K extends keyof InboxEvents>(
  name: K, handler: (payload: InboxEvents[K]) => void,
): Promise<UnlistenFn> {
  return listen<InboxEvents[K]>(name, (e) => handler(e.payload));
}
