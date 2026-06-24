import { describe, it, expect } from "vitest";
import { toMandateRow, toActivityRow } from "./mandateView";
import type { MandateDto, MandateWriteDto } from "../api/engine";

const mandate: MandateDto = {
  mandate_grant_id: "m1",
  target: "/home/me/dest/synced.md",
  source_scope: "/home/me/scope",
  recipe: "keep it synced",
  granted_at: "2026-06-24T10:00:00Z",
  revoked: false,
};

describe("toMandateRow", () => {
  it("derives the target basename + folder and passes through scope/recipe", () => {
    const r = toMandateRow(mandate);
    expect(r.id).toBe("m1");
    expect(r.targetName).toBe("synced.md");
    expect(r.targetFolder).toBe("/home/me/dest");
    expect(r.sourceScope).toBe("/home/me/scope");
    expect(r.recipe).toBe("keep it synced");
    expect(r.grantedAt).toBe("2026-06-24T10:00:00Z");
  });

  it("falls back to the full path when there is no separator", () => {
    const r = toMandateRow({ ...mandate, target: "synced.md" });
    expect(r.targetName).toBe("synced.md");
    expect(r.targetFolder).toBe("");
  });

  it("treats a trailing slash as an empty basename (splitPath splits at the last separator)", () => {
    const r = toMandateRow({ ...mandate, target: "/home/me/dest/" });
    expect(r.targetName).toBe("");
    expect(r.targetFolder).toBe("/home/me/dest");
  });
});

const write: MandateWriteDto = {
  file_written_id: "fw1",
  target: "/home/me/dest/synced.md",
  written_at: "2026-06-24T11:00:00Z",
  undone: false,
};

describe("toActivityRow", () => {
  it("maps an applied write to a row with Undo enabled", () => {
    const r = toActivityRow(write);
    expect(r.fileWrittenId).toBe("fw1");
    expect(r.fileName).toBe("synced.md");
    expect(r.writtenAt).toBe("2026-06-24T11:00:00Z");
    expect(r.canUndo).toBe(true);
    expect(r.label).toBe("Synced");
  });

  it("disables Undo and relabels when undone", () => {
    const r = toActivityRow({ ...write, undone: true });
    expect(r.canUndo).toBe(false);
    expect(r.label).toBe("Undone");
  });
});
