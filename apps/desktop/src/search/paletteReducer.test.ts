import { describe, it, expect } from "vitest";
import { paletteReducer, initialPaletteState, selectedResult, type PaletteState } from "./paletteReducer";
import type { GroupedResults } from "./types";

const results: GroupedResults = {
  memory: [{ id: "mem:1", kind: "memory", title: "M", snippet: "", target: { view: "memory" } }],
  conversations: [{ id: "conv:1", kind: "conversation", title: "C", snippet: "", target: { view: "inbox", convKey: "c" } }],
  files: [{ id: "file:1", kind: "file", title: "F", snippet: "", target: { view: "settings" } }],
  errors: { memory: false, conversations: false, files: false },
};
const ready = (): PaletteState => paletteReducer(initialPaletteState, { type: "setResults", results });

describe("paletteReducer", () => {
  it("setQuery updates the query text", () => {
    expect(paletteReducer(initialPaletteState, { type: "setQuery", query: "hi" }).query).toBe("hi");
  });

  it("setResults stores results, resets selection to 0, marks ready", () => {
    const s = ready();
    expect(s.selectedIndex).toBe(0);
    expect(s.status).toBe("ready");
  });

  it("move wraps down and up across the flattened 3-result list", () => {
    let s = ready();
    s = paletteReducer(s, { type: "move", delta: 1 });
    expect(s.selectedIndex).toBe(1);
    s = paletteReducer(s, { type: "move", delta: 1 });
    s = paletteReducer(s, { type: "move", delta: 1 });
    expect(s.selectedIndex).toBe(0); // wrapped past the end
    s = paletteReducer(s, { type: "move", delta: -1 });
    expect(s.selectedIndex).toBe(2); // wrapped before the start
  });

  it("move is a no-op when there are no results", () => {
    const s = paletteReducer(initialPaletteState, { type: "move", delta: 1 });
    expect(s.selectedIndex).toBe(0);
  });

  it("reset returns the initial state", () => {
    expect(paletteReducer(ready(), { type: "reset" })).toEqual(initialPaletteState);
  });

  it("selectedResult returns the flattened item at the selected index, or null", () => {
    expect(selectedResult(ready())?.id).toBe("mem:1");
    expect(selectedResult(paletteReducer(ready(), { type: "move", delta: 1 }))?.id).toBe("conv:1");
    expect(selectedResult(initialPaletteState)).toBeNull();
  });
});
