import { describe, it, expect } from "vitest";
import { onSendStart, onSendOk, onSendErr, type SendState } from "./sendState";

describe("send reducer", () => {
  it("start → pending", () => { expect(onSendStart({}, "c1")).toEqual({ c1: { status: "pending" } }); });
  it("ok → records envelope_id", () => {
    const s: SendState = onSendStart({}, "c1");
    expect(onSendOk(s, { id: "c1", envelope_id: "e1", encrypted: true })).toEqual({ c1: { status: "ok", envelope_id: "e1" } });
  });
  it("err → carries retryable + reason", () => {
    const s: SendState = onSendStart({}, "c1");
    expect(onSendErr(s, { id: "c1", retryable: false, reason: "unresolvable" }))
      .toEqual({ c1: { status: "err", retryable: false, reason: "unresolvable" } });
  });
  it("ignores acks for unknown ids", () => { expect(onSendOk({}, { id: "ghost", envelope_id: "e", encrypted: true })).toEqual({}); });
});
