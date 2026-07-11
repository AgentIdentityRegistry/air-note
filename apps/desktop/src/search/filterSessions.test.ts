import { describe, it, expect } from "vitest";
import { filterSessions } from "./filterSessions";
import type { SessionSummaryDto } from "../api/engine";

const session = (id: string, title: string, project: string): SessionSummaryDto => ({
  session_id: id, title, project, tool: "claude-code", started_at: 1, ended_at: 2, approx_bytes: 100,
});

describe("filterSessions", () => {
  const sessions = [
    session("s1", "Lunch planning", "notes"),
    session("s2", "Redesign spec", "work"),
  ];

  it("returns [] for an empty query", () => {
    expect(filterSessions(sessions, "")).toEqual([]);
  });

  it("matches case-insensitively on the title", () => {
    expect(filterSessions(sessions, "REDESIGN").map((r) => r.id)).toEqual(["session:s2"]);
  });

  it("matches case-insensitively on the project", () => {
    expect(filterSessions(sessions, "WORK").map((r) => r.id)).toEqual(["session:s2"]);
  });

  it("maps to a SearchResult targeting the Library view", () => {
    const [r] = filterSessions(sessions, "lunch");
    expect(r).toMatchObject({
      kind: "session",
      title: "Lunch planning",
      snippet: "notes",
      target: { view: "library" },
    });
  });

  it("produces unique prefixed ids", () => {
    expect(filterSessions(sessions, "e").map((r) => r.id)).toEqual(["session:s1", "session:s2"]);
  });

  it("caps results", () => {
    const many = Array.from({ length: 9 }, (_, i) => session(`s${i}`, `Session ${i}`, "proj"));
    expect(filterSessions(many, "session", 4)).toHaveLength(4);
  });
});
