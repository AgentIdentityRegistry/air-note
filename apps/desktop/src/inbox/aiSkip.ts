/**
 * aiSkip — pure pre-reservation gating for the AI-reply loop (Phase B task 6, Step 2.2).
 *
 * The controller MUST decide "do I even look at this message?" BEFORE it touches the Rust
 * atomic reserve. Three independent reasons to skip, all derived from the RAW message shape
 * (never from a rendered display string — D12/M-B: a benign body that *renders* to the
 * "🔒 (encrypted)" glyph must NOT be confused with an actually-unreadable body):
 *
 *  - rooms are excluded from AI auto-reply entirely (D6),
 *  - an unreadable body (absent on the wire, or an explicit `{type:"encrypted"}` sentinel)
 *    can't be replied to,
 *  - a DID already being processed in this handler invocation (the per-DID in-flight lock)
 *    must not be double-entered.
 *
 * Pure + deterministic: no I/O, no imports. The live lock/reserve/stream wiring lives in
 * `state/aiLoop.tsx`; this predicate is the part worth unit-testing in isolation.
 */

import type { InboxMessage } from "../api/inbox";

/** Why a channel message was skipped before reservation (for logs/tests); `null` ⇒ do not skip. */
export type SkipReason = "room" | "unreadable" | "in_flight";

/** A body is unreadable when it's absent on the wire OR carries the explicit encrypted sentinel.
 *  Gated on the RAW body object, NOT on `bodyText(body)` (D12/M-B). */
export function isUnreadableBody(body: unknown): boolean {
  if (body == null) return true;
  return (body as { type?: unknown }).type === "encrypted";
}

/**
 * Should this channel message be skipped before we reserve an AI slot?
 *
 * @param m         the raw channel message
 * @param inFlight  DIDs currently mid-processing (the per-DID lock set)
 * @returns the skip reason, or `null` to proceed to reservation
 */
export function shouldSkip(m: InboxMessage, inFlight: ReadonlySet<string>): SkipReason | null {
  if (m.room_id) return "room"; // D6 — rooms excluded
  if (isUnreadableBody(m.body)) return "unreadable"; // D12/M-B — gate on raw shape
  if (inFlight.has(m.from)) return "in_flight"; // per-DID lock
  return null;
}
