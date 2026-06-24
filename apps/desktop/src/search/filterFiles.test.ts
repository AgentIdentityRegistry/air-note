import { describe, it, expect } from "vitest";
import { filterFiles, basename } from "./filterFiles";
import type { FileRecordDto } from "../api/engine";

const file = (path: string, id: string): FileRecordDto => ({
  canonical_path: path, file_event_id: id, content_hash: "h", grant_root: "/root", writable: false,
});

describe("basename", () => {
  it("returns the last path segment for posix and windows separators", () => {
    expect(basename("/a/b/notes.md")).toBe("notes.md");
    expect(basename("C:\\docs\\plan.txt")).toBe("plan.txt");
    expect(basename("solo.md")).toBe("solo.md");
  });
});

describe("filterFiles", () => {
  const files = [file("/notes/lunch.md", "f1"), file("/work/redesign-spec.md", "f2")];

  it("returns [] for an empty query", () => {
    expect(filterFiles(files, "")).toEqual([]);
  });

  it("matches case-insensitively on the path", () => {
    expect(filterFiles(files, "REDESIGN").map((r) => r.id)).toEqual(["file:f2"]);
  });

  it("maps to a SearchResult targeting the settings panel", () => {
    const [r] = filterFiles(files, "lunch");
    expect(r).toMatchObject({
      kind: "file",
      title: "lunch.md",
      snippet: "/notes/lunch.md",
      target: { view: "settings" },
    });
  });

  it("caps results", () => {
    const many = Array.from({ length: 9 }, (_, i) => file(`/d/f${i}.md`, `f${i}`));
    expect(filterFiles(many, ".md", 4)).toHaveLength(4);
  });
});
