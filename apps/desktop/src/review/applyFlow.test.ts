import { describe, it, expect, vi } from "vitest";
import { runApprove, classifyApplyError } from "./applyFlow";

describe("classifyApplyError", () => {
  it("detects NeedsLoudConfirm and Stale from the engine message text", () => {
    expect(classifyApplyError("this change needs an explicit review confirmation: ...")).toBe("loud");
    expect(classifyApplyError("the file changed since this was suggested: ...")).toBe("stale");
    expect(classifyApplyError("some other error")).toBe("other");
  });
});

describe("runApprove", () => {
  it("a loud proposal cannot apply without the ack: first call bounces, second (acked) succeeds", async () => {
    // Mock op: rejects NeedsLoudConfirm when acknowledged=false, succeeds when true.
    const op = vi.fn(async (_id: string, acknowledged: boolean) => {
      if (!acknowledged) throw new Error("this change needs an explicit review confirmation: x");
      return { file_written_id: "fw1" };
    });
    // First attempt without ack → returns { needsLoud: true }, op called once with false, no write.
    const first = await runApprove(op, "p1", false);
    expect(first).toEqual({ needsLoud: true });
    expect(op).toHaveBeenCalledTimes(1);
    expect(op).toHaveBeenLastCalledWith("p1", false);
    // Second attempt WITH ack → applied.
    const second = await runApprove(op, "p1", true);
    expect(second).toEqual({ applied: "fw1" });
    expect(op).toHaveBeenLastCalledWith("p1", true);
  });

  it("a non-loud proposal applies on the first call", async () => {
    const op = vi.fn(async () => ({ file_written_id: "fw2" }));
    expect(await runApprove(op, "p2", false)).toEqual({ applied: "fw2" });
  });
});
