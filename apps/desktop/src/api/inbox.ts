import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

// ── AI-guard reservation result (mirrors Rust AiReservation) ─────────────────

export type AiReservation = {
  /** `"off"` | `"draft"` | `"auto"` — only `"auto"` means "send now". */
  decision: "off" | "draft" | "auto";
  /** Human-readable reason, surfaced to the UI/log. */
  reason: string;
  /** Reservation key; present (non-null) iff `decision === "auto"`. */
  token: string | null;
};

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

// ── Phase B: channel + AI-guard wrappers ────────────────────────────────────

/** Start the live channel (AI-feed) connection. Idempotent. */
export const inboxChannelStart = () => invoke<void>("inbox_channel_start");
/** Stop the live channel connection. */
export const inboxChannelStop = () => invoke<void>("inbox_channel_stop");

/**
 * Reserve an AI auto-send slot (D2/D11 decision). `receivedAt` is the message's ingest
 * timestamp (RFC-3339); `now` is stamped server-side. Returns the decision + optional token.
 */
export const inboxAiReserve = (did: string, threadId: string | undefined, receivedAt: string) =>
  invoke<AiReservation>("inbox_ai_reserve", { did, threadId, receivedAt });

/**
 * Finalize a reservation with the real `envelopeId` (call on `inbox_send_ok`).
 * `threadId` must match the one used at reserve time.
 */
export const inboxAiConfirm = (did: string, token: string, envelopeId: string, threadId: string) =>
  invoke<void>("inbox_ai_confirm", { did, token, envelopeId, threadId });

/**
 * Drop a reservation (call on send error, LLM error, or discard) so it never counts against budget.
 * A late cancel on an already-confirmed reservation is a safe no-op in air-rs.
 */
export const inboxAiCancel = (did: string, token: string) =>
  invoke<void>("inbox_ai_cancel", { did, token });

/**
 * The first non-archived agent id from `agents.json`, in array order.
 * Returns `null` (not an error) when there is no usable agent — the UI should prompt
 * "configure a reply model" rather than treating this as a failure.
 */
export const inboxDefaultAgent = () => invoke<string | null>("inbox_default_agent");

// ── LLM stream wrapper ───────────────────────────────────────────────────────

/**
 * Start a streaming LLM completion. Events arrive as `llm_stream_chunk` / `llm_stream_done` /
 * `llm_stream_error` / `llm_stream_notice`. Cancel via `llm_stream_cancel`.
 *
 * `systemOverride` (optional): when supplied, replaces the agent's derived system prompt for this
 * run — used by the AI-reply loop to inject a fully-fenced reply prompt (Phase B task 4).
 */
export const llmStreamStart = (
  runId: string,
  agentId: string,
  prompt: string,
  systemOverride?: string,
) => invoke<void>("llm_stream_start", { runId, agentId, prompt, systemOverride });

/**
 * Cancel an in-flight stream: sets the run's cancel flag; the run then emits `llm_stream_done`
 * with `cancelled: true`. Rejects when no stream is active for `runId` (e.g. it already finished) —
 * callers should treat that rejection as a safe no-op.
 */
export const llmStreamCancel = (runId: string) =>
  invoke<void>("llm_stream_cancel", { runId });

// ── Event payloads + a typed subscribe helper ───────────────────────────────────

export type InboxEvents = {
  inbox_attached: { pid: number; did: string };
  inbox_detached: Record<string, never>;
  inbox_offline: Record<string, never>;
  inbox_message: InboxMessage;
  inbox_send_ok: { id: string; envelope_id: string; encrypted: boolean };
  inbox_send_err: { id: string; retryable: boolean; reason: string };
  /** Live channel (AI-feed) message, after dedup + mute gating in the Rust pump. */
  inbox_channel_message: InboxMessage;
};

export function onInboxEvent<K extends keyof InboxEvents>(
  name: K, handler: (payload: InboxEvents[K]) => void,
): Promise<UnlistenFn> {
  return listen<InboxEvents[K]>(name, (e) => handler(e.payload));
}
