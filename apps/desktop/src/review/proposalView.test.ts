import { describe, it, expect } from "vitest";
import { toProposalRow } from "./proposalView";
import type { ProposalDto } from "../api/engine";

const base: ProposalDto = {
  id: "p1",
  target: "/home/me/notes/alice.md",
  op: "edit",
  new_content_hash: "abc",
  rationale: "Alice now works at Globex",
  requires_loud_modal: false,
  producer: "",
};

describe("toProposalRow", () => {
  it("derives the basename + folder and passes through the Why", () => {
    const r = toProposalRow(base);
    expect(r.id).toBe("p1");
    expect(r.fileName).toBe("alice.md");
    expect(r.folder).toBe("/home/me/notes");
    expect(r.why).toBe("Alice now works at Globex");
    expect(r.risky).toBe(false);
    expect(r.opLabel).toBe("Edit");
  });

  it("flags a loud-modal proposal as risky and labels delete", () => {
    const r = toProposalRow({ ...base, requires_loud_modal: true, op: "delete" });
    expect(r.risky).toBe(true);
    expect(r.opLabel).toBe("Delete");
  });

  it("falls back to the full path when there is no separator", () => {
    const r = toProposalRow({ ...base, target: "alice.md" });
    expect(r.fileName).toBe("alice.md");
    expect(r.folder).toBe("");
  });
});
