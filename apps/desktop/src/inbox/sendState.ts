export type SendEntry =
  | { status: "pending" }
  | { status: "ok"; envelope_id: string }
  | { status: "err"; retryable: boolean; reason: string };
export type SendState = Record<string, SendEntry>;

export function onSendStart(s: SendState, id: string): SendState { return { ...s, [id]: { status: "pending" } }; }
export function onSendOk(s: SendState, a: { id: string; envelope_id: string; encrypted?: boolean }): SendState {
  if (!(a.id in s)) return s;
  return { ...s, [a.id]: { status: "ok", envelope_id: a.envelope_id } };
}
export function onSendErr(s: SendState, a: { id: string; retryable: boolean; reason: string }): SendState {
  if (!(a.id in s)) return s;
  return { ...s, [a.id]: { status: "err", retryable: a.retryable, reason: a.reason } };
}
