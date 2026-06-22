import { describe, it, expect } from "vitest";
import { ingestSummary } from "./ingestSummary";
import type { IngestReportDto } from "../api/engine";

describe("ingestSummary", () => {
  it("renders counts in a compact line", () => {
    const r: IngestReportDto = { ingested: 3, superseded: 1, deduped: 12, skipped: [{ path: "/x/a.bin", reason: "not valid UTF-8" }], failed: [] };
    expect(ingestSummary(r)).toBe("3 added · 1 updated · 12 unchanged · 1 skipped · 0 failed");
  });
});
