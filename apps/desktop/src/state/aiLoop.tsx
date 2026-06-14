/**
 * aiLoop — the AI-reply loop controller (AI Inbox — Phase B, task 6).
 *
 * This is the integration crux: it wires the live channel feed → pre-reservation guards →
 * fenced reply prompt → streaming LLM completion → draft / auto-send → confirm / cancel.
 * It is SEPARATE from the A3 `InboxProvider` (which owns the user-facing inbox) and is mounted
 * INSIDE its subtree so both can listen to the shared `inbox_send_ok` / `inbox_send_err` Tauri
 * broadcasts; each matches only its own correlation ids.
 *
 * ── SECURITY INVARIANTS (do not weaken) ─────────────────────────────────────────────────────
 *  1. NEVER auto-send on ANY error/cancel path. A send is dispatched ONLY from `llm_stream_done`
 *     with `cancelled === false` AND `decision === "auto"`. `llm_stream_error`, `inbox_send_err`,
 *     a cancelled `done` (key-change/teardown), a failed reserve, or a missing reply agent all
 *     resolve to draft / failed / disabled — never to a send.
 *  2. NEVER `confirm` a send that didn't happen. `inboxAiConfirm` is called ONLY from a matched
 *     `inbox_send_ok`. On `inbox_send_err` / `llm_stream_error` we `inboxAiCancel` (or nothing).
 *  3. Only `decision === "auto"` ever sends; `draft` / `off` never do.
 *  4. The per-DID in-flight lock is released at send-dispatch (end of the `done` handler), NEVER
 *     held across the ack wait (I-B) — the Rust atomic reserve is the real cross-message guard.
 *     Every terminal path (off / draft / auto-dispatch / error / cancel) unlocks the DID, so a
 *     DID can never be permanently stranded.
 *  5. Rooms (D6) and unreadable bodies (D12) are skipped BEFORE reserving (see `shouldSkip`).
 *
 * Stale-closure safety: the channel/stream/send handlers are registered ONCE (mounted as stable
 * callbacks over refs), so all cross-event state — the dedupe set, the in-flight lock, the
 * per-run bookkeeping, the in-flight-send bookkeeping, the resolved reply agent — lives in refs,
 * never in closed-over state. React state mirrors the panel view only.
 */

import {
  createContext, useContext, useEffect, useMemo, useRef, useState, type ReactNode,
} from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  inboxChannelStart, inboxChannelStop,
  inboxAiReserve, inboxAiConfirm, inboxAiCancel,
  inboxDefaultAgent, inboxSend, inboxHistory, inboxPolicySet,
  llmStreamStart, llmStreamCancel, onInboxEvent,
  type InboxMessage, type ArchiveRow,
} from "../api/inbox";
import { buildReplyPrompt, HARDENED_SYSTEM, type ReplyContext } from "../inbox/aiPrompt";
import { newDedupe, markProcessed } from "../inbox/aiDedupe";
import { bodyText } from "../inbox/bodyText";
import { shouldSkip } from "../inbox/aiSkip";

/** Bounded history depth fed into the reply prompt. The prompt builder ALSO hard-clamps the total
 *  to D13's `MAX_TOTAL` (12000 bytes) — so 20 rows is a count ceiling, not the real limiter: it
 *  keeps the request small/cheap while letting the byte cap do the final trimming. A future tuner
 *  must NOT raise this expecting longer context — the byte cap silently wins past ~12 KB. */
const HISTORY_LIMIT = 20;

/** Panel status for a single drafted/sent reply, keyed by the *incoming* message's envelope id.
 *  `auto-sent` = sent by the auto path; `sent` = a human-approved manual send (Task 7 renders
 *  these distinctly — a human-approved send is NOT an auto-send). */
export type ReplyStatus = "drafting" | "drafted" | "auto-sent" | "sent" | "failed" | "disabled";

/** What the panel renders per incoming message. `token` is non-null ONLY while a still-reserved,
 *  not-yet-dispatched auto reply is outstanding — that's the only state in which `discard` should
 *  cancel the reservation; once dispatched it's cleared (the ack path owns it). */
export type ReplyView = {
  status: ReplyStatus;
  text: string;
  reason: string;
  peer: string;
  threadId: string | undefined; // thread to reply on (manual approve threads like the auto path)
  token: string | null;
  retryable?: boolean;          // on a `failed` row: whether the send may be manually retried (I4)
};

type AiLoopCtx = {
  /** `disabled` when there is no usable reply agent (panel shows "configure a reply model"). */
  mode: "loading" | "active" | "disabled";
  /** Drafted/sent replies keyed by incoming `envelope_id`. */
  replies: Map<string, ReplyView>;
  /** Approve a human-reviewed draft → send it (manual send; no auto-send semantics). */
  approveSend: (envelopeId: string) => Promise<void>;
  /** Edit the draft text in place (before approve). */
  edit: (envelopeId: string, text: string) => void;
  /** Drop a draft; if it was an auto reservation not yet sent, release the reservation (no leak). */
  discard: (envelopeId: string) => void;
};

const Ctx = createContext<AiLoopCtx | null>(null);

// ── Internal bookkeeping (refs only — never closed over by the once-registered handlers) ──────

/** Per-run state, keyed by the LLM `runId`. The `done`/`error`/`chunk` events carry only `runId`,
 *  so everything needed to finish a run (which message, which reservation, the accumulating text)
 *  is resolved through this map. */
type RunRecord = {
  envelopeId: string;   // the INCOMING message's envelope id (panel + dedupe key)
  peer: string;         // m.from
  threadId: string | undefined;
  decision: "draft" | "auto"; // "off" never starts a run
  token: string | null; // reservation token (non-null iff decision === "auto")
  reason: string;
  text: string;         // accumulating draft text (authoritative source for the send)
};

/** In-flight send bookkeeping, keyed by the `inboxSend` correlation id. Lets the shared
 *  `inbox_send_ok` / `inbox_send_err` broadcasts be matched to OUR sends only (and resolve the
 *  reservation to confirm/cancel). */
type SendRecord = {
  peer: string;
  token: string;        // always set — only auto sends are stashed, and auto always has a token
  threadId: string | undefined;
  envelopeId: string;   // incoming envelope id, to flip the panel row's status
};

export function AiLoopProvider({ children }: { children: ReactNode }) {
  const [mode, setMode] = useState<"loading" | "active" | "disabled">("loading");
  const [replies, setReplies] = useState<Map<string, ReplyView>>(() => new Map());
  // Latest `replies` mirrored into a ref so the stable action callbacks (registered once) never read
  // a stale closure copy when they look up a row by envelope id.
  const repliesRef = useRef(replies);
  repliesRef.current = replies;

  // Cross-event state (refs — stable across renders, read/written by once-registered handlers).
  const dedupe = useRef<Set<string>>(newDedupe());
  const inFlight = useRef<Set<string>>(new Set());        // per-DID processing lock (Step 2)
  const runs = useRef<Map<string, RunRecord>>(new Map());  // runId → run state
  const sends = useRef<Map<string, SendRecord>>(new Map()); // correlationId → send state
  const replyAgentId = useRef<string | null>(null);
  const keyChangePending = useRef<Set<string>>(new Set()); // DIDs with a key-change policy write in flight — one write per DID (Step 3)

  // ── tiny panel-state mutators (functional, immutable Map copies) ────────────────────────────
  const patchReply = useRef((envelopeId: string, patch: Partial<ReplyView>) => {
    setReplies((prev) => {
      const next = new Map(prev);
      const cur = next.get(envelopeId);
      // A patch with no prior row only makes sense when it carries every required field; callers
      // that create a row always pass a full ReplyView, so treat the patch as the full row then.
      next.set(envelopeId, cur ? { ...cur, ...patch } : (patch as ReplyView));
      return next;
    });
  });

  // ── per-DID lock helpers ────────────────────────────────────────────────────────────────────
  const unlock = useRef((did: string) => { inFlight.current.delete(did); });

  // ── handle one channel message (Step 2) ─────────────────────────────────────────────────────
  // Registered once; reads everything from refs. `async` is fine — Tauri's listen() ignores the
  // returned promise, and we never depend on its completion elsewhere.
  const handle = useRef(async (m: InboxMessage) => {
    // The loop only ever does work when a reply agent is configured; this listener is only
    // registered in that case, and `replyAgentId` is the authoritative gate (invariant 1: a
    // missing agent must never reach a send).
    const agentId = replyAgentId.current;
    if (!agentId) return;

    // 1. dedupe — proceed only the first time we see this envelope id.
    const { set, fresh } = markProcessed(dedupe.current, m.envelope_id);
    dedupe.current = set;
    if (!fresh) return;

    // 2. pre-reservation skip: rooms (D6), unreadable bodies (D12/M-B), per-DID lock. Surface the
    //    typed reason so D6/D12/lock skips are observable in QA (I1).
    const skip = shouldSkip(m, inFlight.current);
    if (skip) {
      console.debug(`[aiLoop] skip ${m.envelope_id} from ${m.from}: ${skip}`);
      return;
    }

    // 3. lock the DID, then reserve an AI slot (the atomic cross-message guard lives in Rust).
    inFlight.current.add(m.from);
    let reserved: { decision: "off" | "draft" | "auto"; reason: string; token: string | null };
    try {
      reserved = await inboxAiReserve(m.from, m.thread_id, m.received_at);
    } catch {
      // 10 (reserve-failure variant): a failed reserve NEVER sends. Unlock and bail.
      unlock.current(m.from);
      return;
    }

    // 4. "off" → nothing to draft. Unlock and return (invariant 3: draft/off never send).
    if (reserved.decision === "off") {
      unlock.current(m.from);
      return;
    }

    // 5. build the fenced reply prompt from bounded history + this incoming message.
    let rows: ArchiveRow[];
    try {
      rows = await inboxHistory(m.from, undefined, HISTORY_LIMIT, false);
    } catch {
      rows = [];
    }
    const ctx: ReplyContext = {
      senderAlias: m.contact ?? null,
      senderDid: m.from,
      verified: m.verified,
      history: rows.map((r) => ({ direction: r.direction, text: bodyText(r.body) })),
      incomingText: bodyText(m.body),
    };
    const prompt = buildReplyPrompt(ctx);
    const runId = crypto.randomUUID();

    // 6. record the run + show the panel row as "drafting", THEN start the stream. Recording first
    //    guarantees the run is resolvable even if the first chunk/done races in immediately.
    const decision = reserved.decision; // "draft" | "auto"
    runs.current.set(runId, {
      envelopeId: m.envelope_id,
      peer: m.from,
      threadId: m.thread_id,
      decision,
      token: reserved.token,
      reason: reserved.reason,
      text: "",
    });
    patchReply.current(m.envelope_id, {
      status: "drafting", text: "", reason: reserved.reason, peer: m.from,
      threadId: m.thread_id, token: reserved.token,
    });

    try {
      await llmStreamStart(runId, agentId, prompt, HARDENED_SYSTEM);
    } catch (e) {
      // Stream failed to even start → treat exactly like llm_stream_error: cancel reservation,
      // clear the token (no longer live), mark failed, unlock. NEVER auto-send (invariant 1).
      runs.current.delete(runId);
      if (reserved.token) inboxAiCancel(m.from, reserved.token).catch(() => {});
      patchReply.current(m.envelope_id, { status: "failed", reason: errText(e), token: null });
      unlock.current(m.from);
    }
  });

  // ── LLM stream: chunk accumulation (refs) + done/error terminal handling ─────────────────────
  const onChunk = useRef((p: { runId: string; delta: string }) => {
    const rec = runs.current.get(p.runId);
    if (!rec) return; // not ours, or already finalized
    rec.text += p.delta;
    patchReply.current(rec.envelopeId, { text: rec.text });
  });

  const onDone = useRef(async (p: { runId: string; cancelled: boolean }) => {
    const rec = runs.current.get(p.runId);
    if (!rec) return;
    runs.current.delete(p.runId);

    // A cancelled completion (key-change / teardown cancel) is NOT a successful draft: never send,
    // never confirm. The reservation is cancelled by whoever cancelled the stream (Step 3 / discard).
    if (p.cancelled) { unlock.current(rec.peer); return; }

    const draft = rec.text; // authoritative accumulated text (read from the ref, not React state)

    // The per-DID lock MUST be released for every non-cancelled outcome (auto-dispatch, dispatch
    // failure, or draft). A `finally` makes that unconditional (invariant 4 / C1) — no terminal
    // branch can strand the DID even if it grows an early `return` later.
    try {
      if (rec.decision === "auto" && rec.token) {
        // Auto path: dispatch the send, stash it for the ack. The lock is released by the `finally`
        // below — we must NOT hold it across the ack wait (I-B); the Rust atomic reserve is the
        // real cross-message guard.
        try {
          const sentId = await inboxSend(rec.peer, { type: "text", text: draft }, rec.threadId, rec.envelopeId);
          sends.current.set(sentId, {
            peer: rec.peer, token: rec.token, threadId: rec.threadId, envelopeId: rec.envelopeId,
          });
          // Once dispatched, the reservation is owned by the send_ok/err ack path. Clear the row's
          // token so `discard` can no longer cancel it (#3): a discard now is a pure panel-clear,
          // never a cancel that races the ack into a "sent but UI says discarded" state.
          patchReply.current(rec.envelopeId, { text: draft, token: null });
        } catch (e) {
          // Dispatch itself failed (no ack will ever come) → cancel the reservation inline, mark
          // failed, and clear the token (it's no longer live). NEVER auto-resend (invariant 1).
          inboxAiCancel(rec.peer, rec.token).catch(() => {});
          patchReply.current(rec.envelopeId, { status: "failed", reason: errText(e), token: null });
        }
      } else {
        // Draft path (incl. a "degraded auto" the guard downgraded to draft): show for review.
        patchReply.current(rec.envelopeId, { status: "drafted", text: draft, reason: rec.reason });
      }
    } finally {
      unlock.current(rec.peer);
    }
  });

  const onError = useRef((p: { runId: string; message: string }) => {
    const rec = runs.current.get(p.runId);
    if (!rec) return;
    runs.current.delete(p.runId);
    // Reservation (if any) is cancelled inline and the token cleared (no longer live); the panel
    // shows failed; NEVER auto-send (invariant 1).
    if (rec.token) inboxAiCancel(rec.peer, rec.token).catch(() => {});
    patchReply.current(rec.envelopeId, { status: "failed", reason: p.message, token: null });
    unlock.current(rec.peer);
  });

  // ── send ack (shared broadcasts — match ONLY our stashed correlation ids) ────────────────────
  const onSendOk = useRef(async (p: { id: string; envelope_id: string }) => {
    const rec = sends.current.get(p.id);
    if (!rec) return; // not one of our sends (likely the A3 provider's) — ignore
    sends.current.delete(p.id);
    // The send really happened → confirm the reservation with the REAL envelope id (invariant 2).
    try {
      await inboxAiConfirm(rec.peer, rec.token, p.envelope_id, rec.threadId ?? "");
    } catch {
      // A lost confirm silently reopens the structural window (acceptable per spec). Retry once.
      try {
        await inboxAiConfirm(rec.peer, rec.token, p.envelope_id, rec.threadId ?? "");
      } catch { /* give up after one retry; the reservation TTL/recency backstop covers it */ }
    }
    patchReply.current(rec.envelopeId, { status: "auto-sent" });
  });

  const onSendErr = useRef((p: { id: string; retryable: boolean; reason: string }) => {
    const rec = sends.current.get(p.id);
    if (!rec) return; // not ours — ignore
    sends.current.delete(p.id);
    // The send failed → cancel the reservation (NEVER confirm — invariant 2). The token was already
    // cleared at dispatch, so this is the ack-path's own cancel of the now-stashed reservation.
    // We carry `retryable` to the panel (Task 7 shows retry vs permanent-failure) but NEVER
    // auto-resend ourselves (invariant 1 / §8).
    inboxAiCancel(rec.peer, rec.token).catch(() => {});
    patchReply.current(rec.envelopeId, { status: "failed", reason: p.reason, retryable: p.retryable });
  });

  // ── key-change (Step 3 / D7): force the DID to draft + cancel any in-flight draft/auto ────────
  const onKeyChange = useRef((m: InboxMessage) => {
    if (!m.key_changed) return;
    const did = m.from;

    // One policy write per DID: a burst of key-change frames triggers exactly one `inboxPolicySet`
    // (the `keyChangePending` gate clears once the write settles).
    if (!keyChangePending.current.has(did)) {
      keyChangePending.current.add(did);
      inboxPolicySet(did, "draft")
        .catch(() => {})
        .finally(() => { keyChangePending.current.delete(did); });
    }

    // Cancel any in-flight run(s) for this DID: stop the stream (M-D) and drop the reservation.
    // A cancelled stream emits `done {cancelled:true}`, which never sends and never confirms.
    for (const [runId, rec] of runs.current) {
      if (rec.peer !== did) continue;
      runs.current.delete(runId);
      cancelStream(runId);
      if (rec.token) inboxAiCancel(did, rec.token).catch(() => {});
      patchReply.current(rec.envelopeId, {
        status: "failed", reason: "Contact's key changed — held for review.", token: null,
      });
      unlock.current(did);
    }
  });

  // ── lifecycle: resolve the reply agent, then register all listeners + start the channel ───────
  // Step 1. Race-safe (mirrors the A3 provider's `reg()` pattern): if torn down mid-await we
  // unlisten the just-registered handler instead of leaking it, and stop the channel.
  useEffect(() => {
    let cancelled = false;
    const unlisten: Array<UnlistenFn> = [];
    const reg = async (p: Promise<UnlistenFn>) => {
      const off = await p;
      if (cancelled) { off(); return; }
      unlisten.push(off);
    };
    (async () => {
      let agent: string | null = null;
      try {
        agent = await inboxDefaultAgent();
      } catch {
        agent = null; // treat a failed lookup as "no agent" → disabled (never silently send)
      }
      if (cancelled) return;
      replyAgentId.current = agent;
      setMode(agent ? "active" : "disabled");
      if (!agent) return; // disabled: do not start the channel or wire the loop

      await reg(onInboxEvent("inbox_channel_message", (m) => { handle.current(m); }));
      await reg(onInboxEvent("inbox_send_ok", (p) => { onSendOk.current(p); }));
      await reg(onInboxEvent("inbox_send_err", (p) => { onSendErr.current(p); }));
      await reg(onInboxEvent("inbox_message", (m) => { onKeyChange.current(m); }));
      await reg(listen<{ runId: string; delta: string }>("llm_stream_chunk", (e) => onChunk.current(e.payload)));
      await reg(listen<{ runId: string; cancelled: boolean }>("llm_stream_done", (e) => { onDone.current(e.payload); }));
      await reg(listen<{ runId: string; message: string }>("llm_stream_error", (e) => onError.current(e.payload)));
      if (!cancelled) await inboxChannelStart();
    })().catch(() => { /* lifecycle errors are non-fatal; the panel just stays loading/disabled */ });

    return () => {
      cancelled = true;
      unlisten.forEach((fn) => fn());
      inboxChannelStop().catch(() => {});
    };
  }, []);

  // ── exposed actions ──────────────────────────────────────────────────────────────────────────
  const approveSend = useRef(async (envelopeId: string) => {
    // Manual approve of a HUMAN-REVIEWED draft. This is a plain user send — NOT an auto
    // reservation — so it never touches inboxAiReserve/Confirm.
    const view = repliesRef.current.get(envelopeId);
    if (!view || !view.text) return;
    // #8a: only a reviewable `drafted` row may be approved. Any other status (drafting / auto-sent /
    // sent / failed) OR a non-null token (still owned by the auto/ack path) → refuse. This blocks a
    // second inboxSend for an already-dispatched auto row, and a double-approve of the same draft.
    if (view.status !== "drafted" || view.token !== null) return;
    try {
      // #8b: thread the manual reply on the same thread the auto path would have used (D2 structural
      // + conversation threading), reusing the incoming envelope id as the in-reply-to.
      await inboxSend(view.peer, { type: "text", text: view.text }, view.threadId, envelopeId);
      patchReply.current(envelopeId, { status: "sent" }); // I2: human-approved, not an auto-send
    } catch (e) {
      patchReply.current(envelopeId, { status: "failed", reason: errText(e) });
    }
  });

  const edit = useRef((envelopeId: string, text: string) => {
    patchReply.current(envelopeId, { text });
  });

  const discard = useRef((envelopeId: string) => {
    const view = repliesRef.current.get(envelopeId);
    // A reserved-but-not-yet-sent auto MUST release its reservation so it never leaks against budget.
    if (view?.token) inboxAiCancel(view.peer, view.token).catch(() => {});
    setReplies((prev) => {
      const next = new Map(prev);
      next.delete(envelopeId);
      return next;
    });
  });

  // The action *functions* are stable refs that read live state via `repliesRef`, so the value only
  // needs rebuilding when the exposed `replies`/`mode` change (identity stability for consumers).
  const value = useMemo<AiLoopCtx>(() => ({
    mode,
    replies,
    approveSend: approveSend.current,
    edit: edit.current,
    discard: discard.current,
  }), [mode, replies]);

  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useAiLoop() {
  const c = useContext(Ctx);
  if (!c) throw new Error("useAiLoop must be inside AiLoopProvider");
  return c;
}

// ── helpers ────────────────────────────────────────────────────────────────────────────────────

/** Best-effort cancel of a stream (M-D). The command rejects when no stream is active (already
 *  finished), which is a safe no-op for us — swallow it. */
function cancelStream(runId: string): void {
  llmStreamCancel(runId).catch(() => {});
}

/** Normalize a thrown value (Tauri rejects with a string; guard against non-strings too). */
function errText(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : "Unknown error";
}
