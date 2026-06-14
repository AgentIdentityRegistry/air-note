import { describe, it, expect } from "vitest";
import { mergeSidebar } from "./sidebar";
import type { Conversation } from "./model";
import type { ConversationSummary } from "../api/inbox";

const summary = (key: string, ts: string): ConversationSummary =>
  ({ conv_key: key, kind: "peer", last_timestamp: ts, count: 1 });
const conv = (key: string, ts: string, text: string, unread = 0): Conversation =>
  ({ convKey: key, kind: "peer", lastTimestamp: ts, lastText: text, unread });

describe("mergeSidebar", () => {
  it("shows every summary; enriches preview/unread from loaded rows", () => {
    const out = mergeSidebar([summary("p1", "2"), summary("p2", "1")], [conv("p1", "2", "hi", 3)]);
    expect(out.map((c) => c.convKey)).toEqual(["p1", "p2"]);
    expect(out[0]).toMatchObject({ lastText: "hi", unread: 3 });
    expect(out[1]).toMatchObject({ lastText: "", unread: 0 }); // p2 not loaded yet → no preview
  });
  it("appends a brand-new live conversation absent from summaries", () => {
    const out = mergeSidebar([summary("p1", "1")], [conv("p1", "1", "a"), conv("pNEW", "9", "new!", 1)]);
    expect(out.map((c) => c.convKey)).toEqual(["pNEW", "p1"]); // newest-first
  });
});
