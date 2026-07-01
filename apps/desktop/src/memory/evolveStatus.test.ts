import { describe, it, expect } from "vitest";
import { formatEvolve } from "./evolveStatus";
import type { EvolveStatusDto } from "../api/engine";

describe("formatEvolve", () => {
  it("summarizes an enabled loop", () => {
    const s: EvolveStatusDto = {
      enabled: true,
      queue_depth: 3,
      last_tick_ms: 120,
      error_count: 0,
      last_error: null,
      last_tainted_snippets: null,
    };
    expect(formatEvolve(s)).toBe("On · 3 queued · last tick 120ms · 0 errors");
  });

  it("shows Off and an em dash when the loop has never ticked", () => {
    const s: EvolveStatusDto = {
      enabled: false,
      queue_depth: 0,
      last_tick_ms: null,
      error_count: 0,
      last_error: null,
      last_tainted_snippets: null,
    };
    expect(formatEvolve(s)).toBe("Off · 0 queued · last tick — · 0 errors");
  });

  it("reports the error count", () => {
    const s: EvolveStatusDto = {
      enabled: true,
      queue_depth: 1,
      last_tick_ms: 9,
      error_count: 2,
      last_error: "boom",
      last_tainted_snippets: null,
    };
    expect(formatEvolve(s)).toBe("On · 1 queued · last tick 9ms · 2 errors");
  });
});
