# Desktop UI Shell Redesign — Plan 2 of 3: Global Search + ⌘K Command Palette

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a global ⌘K command-palette overlay that searches across **memory + conversations + files** and shows grouped, keyboard-navigable results; Enter on a result navigates to the right panel.

**Architecture:** A pure search layer in `src/search/` keeps all I/O at the edge. `globalSearch(query, deps)` fans out concurrently (via `Promise.allSettled`) to the existing `recall` engine op + the existing `listFiles` engine op, and filters the already-in-memory conversation list with a pure helper — **no new backend ops**. Results are plain data carrying a `target: NavTarget` descriptor (not a closure), so ranking/grouping/keyboard logic is unit-tested without React or the engine. `CommandPalette.tsx` is a portal overlay driven by a pure `paletteReducer`; the Shell owns open/close state, installs the global ⌘K hotkey, and resolves a chosen result's `target` into actual navigation (`setView` + `inbox.select`).

**Tech Stack:** React 18 + TypeScript, Tauri v2, Vitest 2 + jsdom + `@testing-library/react` (added in plan 1), CSS tokens in `src/styles.css`.

**Depends on Plan 1** (`docs/superpowers/plans/2026-06-24-desktop-ui-shell-redesign-1-theme-shell.md`): the shared `View` type in `src/shell/nav.ts`, the `Sidebar` component (this plan adds a search trigger to it), and the component-test infra. **Do not start until Plan 1 has landed.**

**Working directory for all commands:** `/Users/ahnkwangwook/air-note/apps/desktop`. Run `cd /Users/ahnkwangwook/air-note/apps/desktop` first.

---

## Key data contracts (defined in Task 1, used everywhere)

```ts
// src/search/types.ts
export type NavTarget = { view: View; convKey?: string };          // where Enter takes you
export type SearchResultKind = "memory" | "conversation" | "file";
export type SearchResult = { id: string; kind: SearchResultKind; title: string; snippet: string; target: NavTarget };
export type GroupedResults = {
  memory: SearchResult[];
  conversations: SearchResult[];
  files: SearchResult[];
  errors: { memory: boolean; conversations: boolean; files: boolean };
};
```

Source → result mapping (locked so every task agrees):
- **memory** ← `recall(query, 5)` → `HitDto[]`; `id: "mem:"+event_id`, `title` = kind label (Memory/Dossier/File), `snippet` = `text`, `target: { view: "memory" }`.
- **conversation** ← in-memory `Conversation[]` from `useInbox().conversations`; `id: "conv:"+convKey`, `title` = `convKey`, `snippet` = `lastText`, `target: { view: "inbox", convKey }`.
- **file** ← `listFiles()` → `FileRecordDto[]`; `id: "file:"+file_event_id`, `title` = basename, `snippet` = `canonical_path`, `target: { view: "settings" }` (files live under the Settings → Sources panel).

Navigation depth: conversations get true item selection (`inbox.select(convKey)` then show Inbox); memory/file results switch to the owning panel only (per-item deep-focus is out of scope — spec "where applicable").

---

## File Structure

**Created:**
- `src/search/types.ts` — the contracts above.
- `src/search/filterConversations.ts` + `.test.ts` — pure conversation filter.
- `src/search/filterFiles.ts` + `.test.ts` — pure file filter + `basename`.
- `src/search/rankResults.ts` + `.test.ts` — `memoryResults`, `flattenResults`.
- `src/search/globalSearch.ts` + `.test.ts` — the façade (`Promise.allSettled` fan-out, injected deps).
- `src/search/paletteReducer.ts` + `.test.ts` — pure keyboard/query/results reducer + `selectedResult`.
- `src/search/CommandPalette.tsx` + `.test.tsx` — the overlay (portal, debounce, focus, render).
- `src/shell/useCommandPaletteHotkey.ts` — global ⌘K / Ctrl-K listener hook.

**Modified:**
- `src/shell/Sidebar.tsx` — add the top search-trigger button (`onOpenSearch` prop).
- `src/shell/Sidebar.test.tsx` — assert the trigger calls `onOpenSearch`.
- `src/App.tsx` — `Shell()` owns `searchOpen` state, installs the hotkey, renders `<CommandPalette/>`, passes `onOpenSearch` to `Sidebar`, resolves `target` → navigation.
- `src/styles.css` — add `.command-palette*` classes (the only net-new CSS family; the rest of the design system already exists).

---

## Task 1: Search types + filterConversations

**Files:**
- Create: `src/search/types.ts`
- Create: `src/search/filterConversations.ts`
- Test: `src/search/filterConversations.test.ts`

- [ ] **Step 1: Write the types module**

Create `src/search/types.ts`:
```ts
import type { View } from "../shell/nav";

/** Where pressing Enter on a result takes the user. */
export type NavTarget = { view: View; convKey?: string };

export type SearchResultKind = "memory" | "conversation" | "file";

export type SearchResult = {
  id: string;
  kind: SearchResultKind;
  title: string;
  snippet: string;
  target: NavTarget;
};

export type GroupedResults = {
  memory: SearchResult[];
  conversations: SearchResult[];
  files: SearchResult[];
  errors: { memory: boolean; conversations: boolean; files: boolean };
};

/** The all-empty result set (empty query, or before the first search). */
export const EMPTY_RESULTS: GroupedResults = {
  memory: [],
  conversations: [],
  files: [],
  errors: { memory: false, conversations: false, files: false },
};
```

- [ ] **Step 2: Write the failing test**

Create `src/search/filterConversations.test.ts`:
```ts
import { describe, it, expect } from "vitest";
import { filterConversations } from "./filterConversations";
import type { Conversation } from "../inbox/model";

const conv = (convKey: string, lastText: string): Conversation => ({
  convKey, kind: "peer", lastTimestamp: "2026-06-24T00:00:00Z", lastText, unread: 0,
});

describe("filterConversations", () => {
  const convs = [
    conv("did:key:alice", "lunch tomorrow?"),
    conv("did:key:bob", "shipping the redesign"),
  ];

  it("returns [] for an empty query", () => {
    expect(filterConversations(convs, "  ")).toEqual([]);
  });

  it("matches case-insensitively on convKey and lastText", () => {
    expect(filterConversations(convs, "ALICE").map((r) => r.id)).toEqual(["conv:did:key:alice"]);
    expect(filterConversations(convs, "redesign").map((r) => r.id)).toEqual(["conv:did:key:bob"]);
  });

  it("maps to a SearchResult targeting the inbox conversation", () => {
    const [r] = filterConversations(convs, "alice");
    expect(r).toMatchObject({
      kind: "conversation",
      title: "did:key:alice",
      snippet: "lunch tomorrow?",
      target: { view: "inbox", convKey: "did:key:alice" },
    });
  });

  it("caps the number of results", () => {
    const many = Array.from({ length: 10 }, (_, i) => conv(`did:key:p${i}`, "hello"));
    expect(filterConversations(many, "hello", 3)).toHaveLength(3);
  });
});
```

- [ ] **Step 3: Run test to verify it fails**

Run: `npm test -- filterConversations`
Expected: FAIL — "Cannot find module './filterConversations'".

- [ ] **Step 4: Write the implementation**

Create `src/search/filterConversations.ts`:
```ts
import type { Conversation } from "../inbox/model";
import type { SearchResult } from "./types";

/** Pure client-side filter over already-loaded conversation summaries (title + preview). */
export function filterConversations(convs: Conversation[], query: string, cap = 5): SearchResult[] {
  const q = query.trim().toLowerCase();
  if (!q) return [];
  return convs
    .filter((c) => c.convKey.toLowerCase().includes(q) || c.lastText.toLowerCase().includes(q))
    .slice(0, cap)
    .map((c) => ({
      id: `conv:${c.convKey}`,
      kind: "conversation" as const,
      title: c.convKey,
      snippet: c.lastText,
      target: { view: "inbox" as const, convKey: c.convKey },
    }));
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `npm test -- filterConversations`
Expected: PASS (4 tests).

- [ ] **Step 6: Commit**

```bash
git add src/search/types.ts src/search/filterConversations.ts src/search/filterConversations.test.ts
git commit -m "feat(desktop): search types + pure conversation filter"
```

---

## Task 2: filterFiles

**Files:**
- Create: `src/search/filterFiles.ts`
- Test: `src/search/filterFiles.test.ts`

- [ ] **Step 1: Write the failing test**

Create `src/search/filterFiles.test.ts`:
```ts
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm test -- filterFiles`
Expected: FAIL — "Cannot find module './filterFiles'".

- [ ] **Step 3: Write the implementation**

Create `src/search/filterFiles.ts`:
```ts
import type { FileRecordDto } from "../api/engine";
import type { SearchResult } from "./types";

/** Last path segment, handling both posix (/) and windows (\) separators. */
export function basename(path: string): string {
  const parts = path.split(/[/\\]/);
  return parts[parts.length - 1] || path;
}

/** Pure client-side filter over the already-loaded file list (name/path). */
export function filterFiles(files: FileRecordDto[], query: string, cap = 5): SearchResult[] {
  const q = query.trim().toLowerCase();
  if (!q) return [];
  return files
    .filter((f) => f.canonical_path.toLowerCase().includes(q))
    .slice(0, cap)
    .map((f) => ({
      id: `file:${f.file_event_id}`,
      kind: "file" as const,
      title: basename(f.canonical_path),
      snippet: f.canonical_path,
      target: { view: "settings" as const },
    }));
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npm test -- filterFiles`
Expected: PASS (2 describe blocks).

- [ ] **Step 5: Commit**

```bash
git add src/search/filterFiles.ts src/search/filterFiles.test.ts
git commit -m "feat(desktop): pure file filter for global search"
```

---

## Task 3: rankResults (memory mapping + flatten)

**Files:**
- Create: `src/search/rankResults.ts`
- Test: `src/search/rankResults.test.ts`

- [ ] **Step 1: Write the failing test**

Create `src/search/rankResults.test.ts`:
```ts
import { describe, it, expect } from "vitest";
import { memoryResults, flattenResults } from "./rankResults";
import type { HitDto } from "../api/engine";
import type { GroupedResults } from "./types";

const hit = (event_id: string, kind: string, text: string): HitDto => ({
  event_id, kind, text, score: 1, sources: ["vector"],
});

describe("memoryResults", () => {
  it("labels kinds (memory/page/file_ingested) and targets the memory panel", () => {
    const out = memoryResults([hit("e1", "page", "dossier text"), hit("e2", "memory", "a memory")]);
    expect(out[0]).toMatchObject({ id: "mem:e1", kind: "memory", title: "Dossier", snippet: "dossier text", target: { view: "memory" } });
    expect(out[1].title).toBe("Memory");
  });
  it("falls back to the raw kind and caps", () => {
    expect(memoryResults([hit("e", "weird", "t")])[0].title).toBe("weird");
    expect(memoryResults(Array.from({ length: 9 }, (_, i) => hit(`e${i}`, "memory", "t")), 5)).toHaveLength(5);
  });
});

describe("flattenResults", () => {
  it("concatenates memory, then conversations, then files in order", () => {
    const g: GroupedResults = {
      memory: [{ id: "mem:1", kind: "memory", title: "M", snippet: "", target: { view: "memory" } }],
      conversations: [{ id: "conv:1", kind: "conversation", title: "C", snippet: "", target: { view: "inbox", convKey: "c" } }],
      files: [{ id: "file:1", kind: "file", title: "F", snippet: "", target: { view: "settings" } }],
      errors: { memory: false, conversations: false, files: false },
    };
    expect(flattenResults(g).map((r) => r.id)).toEqual(["mem:1", "conv:1", "file:1"]);
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm test -- rankResults`
Expected: FAIL — "Cannot find module './rankResults'".

- [ ] **Step 3: Write the implementation**

Create `src/search/rankResults.ts`:
```ts
import type { HitDto } from "../api/engine";
import type { GroupedResults, SearchResult } from "./types";

/** Engine event kinds → human labels (mirrors memory/recallView.ts). */
const MEMORY_KIND_LABEL: Record<string, string> = {
  memory: "Memory",
  page: "Dossier",
  file_ingested: "File",
};

/** Map recall hits to memory SearchResults (capped, ordered as recall returned them). */
export function memoryResults(hits: HitDto[], cap = 5): SearchResult[] {
  return hits.slice(0, cap).map((h) => ({
    id: `mem:${h.event_id}`,
    kind: "memory" as const,
    title: MEMORY_KIND_LABEL[h.kind] ?? h.kind,
    snippet: h.text,
    target: { view: "memory" as const },
  }));
}

/** Flatten grouped results into the keyboard-navigation order: memory → conversations → files. */
export function flattenResults(g: GroupedResults): SearchResult[] {
  return [...g.memory, ...g.conversations, ...g.files];
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npm test -- rankResults`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/search/rankResults.ts src/search/rankResults.test.ts
git commit -m "feat(desktop): memory result mapping + flatten ordering"
```

---

## Task 4: globalSearch façade

The only I/O point. Injects `recall`/`listFiles`/`conversations` as `deps` so it is fully testable without the engine. A failing source yields an empty group + an error flag (`Promise.allSettled` semantics).

**Files:**
- Create: `src/search/globalSearch.ts`
- Test: `src/search/globalSearch.test.ts`

- [ ] **Step 1: Write the failing test**

Create `src/search/globalSearch.test.ts`:
```ts
import { describe, it, expect, vi } from "vitest";
import { globalSearch, type GlobalSearchDeps } from "./globalSearch";
import type { Conversation } from "../inbox/model";
import type { HitDto, FileRecordDto } from "../api/engine";

const conv = (k: string, t: string): Conversation => ({
  convKey: k, kind: "peer", lastTimestamp: "2026-06-24T00:00:00Z", lastText: t, unread: 0,
});
const hit = (id: string, t: string): HitDto => ({ event_id: id, kind: "memory", text: t, score: 1, sources: ["vector"] });
const file = (p: string, id: string): FileRecordDto => ({
  canonical_path: p, file_event_id: id, content_hash: "h", grant_root: "/r", writable: false,
});

const deps = (over: Partial<GlobalSearchDeps> = {}): GlobalSearchDeps => ({
  recall: vi.fn(async () => [hit("e1", "alpha memory")]),
  listFiles: vi.fn(async () => [file("/d/alpha.md", "f1")]),
  conversations: [conv("did:key:alpha", "alpha chat")],
  ...over,
});

describe("globalSearch", () => {
  it("returns all-empty groups for an empty query and never calls the engine", async () => {
    const d = deps();
    const out = await globalSearch("   ", d);
    expect(out.memory).toEqual([]);
    expect(out.conversations).toEqual([]);
    expect(out.files).toEqual([]);
    expect(d.recall).not.toHaveBeenCalled();
    expect(d.listFiles).not.toHaveBeenCalled();
  });

  it("fans out to all three sources and groups the hits", async () => {
    const out = await globalSearch("alpha", deps());
    expect(out.memory.map((r) => r.id)).toEqual(["mem:e1"]);
    expect(out.conversations.map((r) => r.id)).toEqual(["conv:did:key:alpha"]);
    expect(out.files.map((r) => r.id)).toEqual(["file:f1"]);
    expect(out.errors).toEqual({ memory: false, conversations: false, files: false });
  });

  it("isolates a failing source: empty group + error flag, others still return", async () => {
    const out = await globalSearch("alpha", deps({ recall: vi.fn(async () => { throw new Error("engine down"); }) }));
    expect(out.memory).toEqual([]);
    expect(out.errors.memory).toBe(true);
    expect(out.conversations.map((r) => r.id)).toEqual(["conv:did:key:alpha"]);
    expect(out.files.map((r) => r.id)).toEqual(["file:f1"]);
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm test -- globalSearch`
Expected: FAIL — "Cannot find module './globalSearch'".

- [ ] **Step 3: Write the implementation**

Create `src/search/globalSearch.ts`:
```ts
import { recall as recallOp, listFiles as listFilesOp, type HitDto, type FileRecordDto } from "../api/engine";
import type { Conversation } from "../inbox/model";
import { filterConversations } from "./filterConversations";
import { filterFiles } from "./filterFiles";
import { memoryResults } from "./rankResults";
import { type GroupedResults, EMPTY_RESULTS } from "./types";

/** Injected I/O + data, so the façade is testable without the engine. */
export type GlobalSearchDeps = {
  recall: (q: string, k: number) => Promise<HitDto[]>;
  listFiles: () => Promise<FileRecordDto[]>;
  conversations: Conversation[];
};

const MEMORY_K = 5;

/**
 * Fan out to memory (recall), files (listFiles), and the in-memory conversations concurrently.
 * Each source is isolated: a rejection yields an empty group + errors.<source> = true.
 */
export async function globalSearch(query: string, deps: GlobalSearchDeps): Promise<GroupedResults> {
  const q = query.trim();
  if (!q) return EMPTY_RESULTS;

  const [mem, files] = await Promise.allSettled([deps.recall(q, MEMORY_K), deps.listFiles()]);

  return {
    memory: mem.status === "fulfilled" ? memoryResults(mem.value) : [],
    conversations: filterConversations(deps.conversations, q),
    files: files.status === "fulfilled" ? filterFiles(files.value, q) : [],
    errors: {
      memory: mem.status === "rejected",
      conversations: false, // pure client-side filter cannot fail
      files: files.status === "rejected",
    },
  };
}

/** Wire the real engine ops + the live conversation list at the call site. */
export const defaultSearchDeps = (conversations: Conversation[]): GlobalSearchDeps => ({
  recall: recallOp,
  listFiles: listFilesOp,
  conversations,
});
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npm test -- globalSearch`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add src/search/globalSearch.ts src/search/globalSearch.test.ts
git commit -m "feat(desktop): globalSearch façade over recall + files + conversations"
```

---

## Task 5: paletteReducer (keyboard / query / results state)

**Files:**
- Create: `src/search/paletteReducer.ts`
- Test: `src/search/paletteReducer.test.ts`

- [ ] **Step 1: Write the failing test**

Create `src/search/paletteReducer.test.ts`:
```ts
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm test -- paletteReducer`
Expected: FAIL — "Cannot find module './paletteReducer'".

- [ ] **Step 3: Write the implementation**

Create `src/search/paletteReducer.ts`:
```ts
import { type GroupedResults, EMPTY_RESULTS, type SearchResult } from "./types";
import { flattenResults } from "./rankResults";

export type PaletteState = {
  query: string;
  results: GroupedResults;
  selectedIndex: number;
  status: "idle" | "loading" | "ready";
};

export const initialPaletteState: PaletteState = {
  query: "",
  results: EMPTY_RESULTS,
  selectedIndex: 0,
  status: "idle",
};

export type PaletteAction =
  | { type: "reset" }
  | { type: "setQuery"; query: string }
  | { type: "loading" }
  | { type: "setResults"; results: GroupedResults }
  | { type: "move"; delta: 1 | -1 };

export function paletteReducer(state: PaletteState, action: PaletteAction): PaletteState {
  switch (action.type) {
    case "reset":
      return initialPaletteState;
    case "setQuery":
      return { ...state, query: action.query };
    case "loading":
      return { ...state, status: "loading" };
    case "setResults":
      return { ...state, results: action.results, selectedIndex: 0, status: "ready" };
    case "move": {
      const n = flattenResults(state.results).length;
      if (n === 0) return state;
      const next = (state.selectedIndex + action.delta + n) % n;
      return { ...state, selectedIndex: next };
    }
  }
}

/** The currently-highlighted result, or null when the list is empty. */
export function selectedResult(state: PaletteState): SearchResult | null {
  return flattenResults(state.results)[state.selectedIndex] ?? null;
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npm test -- paletteReducer`
Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add src/search/paletteReducer.ts src/search/paletteReducer.test.ts
git commit -m "feat(desktop): pure command-palette reducer + selectedResult"
```

---

## Task 6: ⌘K hotkey hook

**Files:**
- Create: `src/shell/useCommandPaletteHotkey.ts`

- [ ] **Step 1: Write the hook**

Create `src/shell/useCommandPaletteHotkey.ts`:
```ts
import { useEffect } from "react";

/**
 * Install a global ⌘K (macOS) / Ctrl-K (others) listener that calls `onOpen`.
 * `onOpen` is read through a ref-free dependency so callers pass a stable handler
 * (e.g. a useState setter) without re-binding every render.
 */
export function useCommandPaletteHotkey(onOpen: () => void): void {
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        onOpen();
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [onOpen]);
}
```

- [ ] **Step 2: Typecheck**

Run: `npm run typecheck`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add src/shell/useCommandPaletteHotkey.ts
git commit -m "feat(desktop): global ⌘K/Ctrl-K hotkey hook"
```

---

## Task 7: CommandPalette overlay

A controlled overlay: the Shell owns `open`/`onClose`. It debounces the query into `globalSearch` (deps from the real engine ops + `useInbox().conversations`), renders grouped results via the reducer, traps focus, closes on Esc/backdrop, and on Enter calls `onNavigate(target)`.

**Files:**
- Create: `src/search/CommandPalette.tsx`
- Test: `src/search/CommandPalette.test.tsx`
- Modify: `src/styles.css` (add `.command-palette*`)

- [ ] **Step 1: Add the command-palette CSS (the only net-new CSS family)**

Append to `src/styles.css`:
```css
.command-palette-backdrop {
  position: fixed;
  inset: 0;
  background: rgba(10, 14, 22, 0.32);
  backdrop-filter: blur(2px);
  display: flex;
  justify-content: center;
  align-items: flex-start;
  padding-top: 12vh;
  z-index: 100;
}

.command-palette {
  width: min(640px, 92vw);
  border: 1px solid var(--border-soft);
  border-radius: var(--radius-lg);
  background: var(--surface);
  box-shadow: var(--elev-3);
  overflow: hidden;
  display: grid;
  grid-template-rows: auto 1fr;
}

.command-palette-input {
  border: none;
  border-bottom: 1px solid var(--border-soft);
  border-radius: 0;
  padding: 14px 16px;
  font-size: 0.95rem;
  background: transparent;
}

.command-palette-input:focus {
  box-shadow: none;
}

.command-palette-results {
  max-height: 50vh;
  overflow-y: auto;
  padding: 6px;
  display: grid;
  gap: 2px;
  align-content: start;
}

.command-palette-group-label {
  padding: 8px 10px 4px;
  color: var(--text-tertiary);
  font-size: var(--font-label);
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.04em;
}

.command-palette-item {
  text-align: left;
  border: 1px solid transparent;
  border-radius: var(--radius-sm);
  background: transparent;
  box-shadow: none;
  padding: 8px 10px;
  display: grid;
  gap: 2px;
}

.command-palette-item.selected {
  background: color-mix(in srgb, var(--primary) 10%, var(--surface));
  border-color: color-mix(in srgb, var(--primary) 22%, transparent);
}

.command-palette-item-title {
  font-weight: 540;
  font-size: 0.82rem;
}

.command-palette-item-snippet {
  color: var(--text-secondary);
  font-size: 0.74rem;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.command-palette-empty,
.command-palette-error {
  padding: 14px 16px;
  color: var(--text-secondary);
  font-size: 0.8rem;
}

.command-palette-error {
  color: var(--warning);
}
```

- [ ] **Step 2: Write the failing component test**

Create `src/search/CommandPalette.test.tsx`:
```tsx
// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { CommandPalette } from "./CommandPalette";
import type { GroupedResults } from "./types";

// Stub the inbox hook (palette reads conversations from it) and the search façade.
vi.mock("../state/inbox", () => ({ useInbox: () => ({ conversations: [] }) }));

const grouped: GroupedResults = {
  memory: [{ id: "mem:1", kind: "memory", title: "Memory", snippet: "alpha memory", target: { view: "memory" } }],
  conversations: [{ id: "conv:1", kind: "conversation", title: "did:key:bob", snippet: "alpha chat", target: { view: "inbox", convKey: "did:key:bob" } }],
  files: [],
  errors: { memory: false, conversations: false, files: false },
};
const search = vi.fn(async () => grouped);

beforeEach(() => search.mockClear());

function setup(open = true) {
  const onClose = vi.fn();
  const onNavigate = vi.fn();
  render(<CommandPalette open={open} onClose={onClose} onNavigate={onNavigate} search={search} />);
  return { onClose, onNavigate };
}

describe("CommandPalette", () => {
  it("renders nothing when closed", () => {
    setup(false);
    expect(screen.queryByPlaceholderText(/search/i)).not.toBeInTheDocument();
  });

  it("focuses the input, debounces a query, and renders grouped results", async () => {
    setup();
    const input = screen.getByPlaceholderText(/search/i);
    expect(input).toHaveFocus();
    fireEvent.change(input, { target: { value: "alpha" } });
    // Real timers: findByText polls up to 1000ms, easily covering the 180ms debounce.
    expect(await screen.findByText("alpha memory")).toBeInTheDocument();
    expect(screen.getByText("did:key:bob")).toBeInTheDocument();
    expect(search).toHaveBeenCalledWith("alpha", expect.anything());
  });

  it("Enter navigates to the selected result's target and closes", async () => {
    const { onNavigate, onClose } = setup();
    fireEvent.change(screen.getByPlaceholderText(/search/i), { target: { value: "alpha" } });
    await screen.findByText("alpha memory");
    fireEvent.keyDown(window, { key: "ArrowDown" }); // move from memory[0] to conversation[1]
    fireEvent.keyDown(window, { key: "Enter" });
    expect(onNavigate).toHaveBeenCalledWith({ view: "inbox", convKey: "did:key:bob" });
    expect(onClose).toHaveBeenCalled();
  });

  it("Esc closes", () => {
    const { onClose } = setup();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).toHaveBeenCalled();
  });
});
```

- [ ] **Step 3: Run test to verify it fails**

Run: `npm test -- CommandPalette`
Expected: FAIL — "Cannot find module './CommandPalette'".

- [ ] **Step 4: Write CommandPalette**

Create `src/search/CommandPalette.tsx`:
```tsx
import { useEffect, useReducer, useRef } from "react";
import { createPortal } from "react-dom";
import { useInbox } from "../state/inbox";
import { globalSearch, defaultSearchDeps } from "./globalSearch";
import { paletteReducer, initialPaletteState, selectedResult } from "./paletteReducer";
import { type GroupedResults, type SearchResult, type NavTarget } from "./types";

const DEBOUNCE_MS = 180;

type SearchFn = (query: string, deps: ReturnType<typeof defaultSearchDeps>) => Promise<GroupedResults>;

export function CommandPalette({
  open,
  onClose,
  onNavigate,
  search = globalSearch as SearchFn,
}: {
  open: boolean;
  onClose: () => void;
  onNavigate: (target: NavTarget) => void;
  /** Injectable for tests; defaults to the real façade. */
  search?: SearchFn;
}) {
  const { conversations } = useInbox();
  const [state, dispatch] = useReducer(paletteReducer, initialPaletteState);
  const inputRef = useRef<HTMLInputElement>(null);

  // Reset + focus on open.
  useEffect(() => {
    if (open) {
      dispatch({ type: "reset" });
      inputRef.current?.focus();
    }
  }, [open]);

  // Debounced search whenever the query changes while open.
  useEffect(() => {
    if (!open) return;
    const q = state.query;
    if (!q.trim()) {
      dispatch({ type: "reset" });
      return;
    }
    dispatch({ type: "loading" });
    const id = setTimeout(() => {
      search(q, defaultSearchDeps(conversations)).then((results) => dispatch({ type: "setResults", results }));
    }, DEBOUNCE_MS);
    return () => clearTimeout(id);
  }, [open, state.query, conversations, search]);

  // Keyboard: arrows move, Enter navigates, Esc closes.
  useEffect(() => {
    if (!open) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === "ArrowDown") { e.preventDefault(); dispatch({ type: "move", delta: 1 }); }
      else if (e.key === "ArrowUp") { e.preventDefault(); dispatch({ type: "move", delta: -1 }); }
      else if (e.key === "Escape") { e.preventDefault(); onClose(); }
      else if (e.key === "Enter") {
        const sel = selectedResult(state);
        if (sel) { onNavigate(sel.target); onClose(); }
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [open, state, onClose, onNavigate]);

  if (!open) return null;

  const groups: Array<{ label: string; items: SearchResult[]; error: boolean; source: string }> = [
    { label: "Memory", items: state.results.memory, error: state.results.errors.memory, source: "memory" },
    { label: "Conversations", items: state.results.conversations, error: state.results.errors.conversations, source: "conversations" },
    { label: "Files", items: state.results.files, error: state.results.errors.files, source: "files" },
  ];
  const flat = [...state.results.memory, ...state.results.conversations, ...state.results.files];
  const hasAny = flat.length > 0;

  return createPortal(
    <div
      className="command-palette-backdrop"
      onMouseDown={(e) => { if (e.target === e.currentTarget) onClose(); }}
    >
      <div className="command-palette" role="dialog" aria-modal="true" aria-label="Global search">
        <input
          ref={inputRef}
          className="command-palette-input"
          placeholder="Search memory, conversations, files…"
          value={state.query}
          onChange={(e) => dispatch({ type: "setQuery", query: e.target.value })}
        />
        <div className="command-palette-results">
          {!state.query.trim() ? (
            <p className="command-palette-empty">Type to search across memory, conversations, and files.</p>
          ) : !hasAny && state.status === "ready" ? (
            <p className="command-palette-empty">No results for “{state.query}”.</p>
          ) : (
            groups.map((g) =>
              g.error ? (
                <p key={g.source} className="command-palette-error">Couldn’t search {g.label.toLowerCase()}.</p>
              ) : g.items.length > 0 ? (
                <div key={g.source}>
                  <div className="command-palette-group-label">{g.label}</div>
                  {g.items.map((item) => {
                    const isSelected = flat[state.selectedIndex]?.id === item.id;
                    return (
                      <button
                        key={item.id}
                        type="button"
                        className={isSelected ? "command-palette-item selected" : "command-palette-item"}
                        onMouseDown={(e) => { e.preventDefault(); onNavigate(item.target); onClose(); }}
                      >
                        <span className="command-palette-item-title">{item.title}</span>
                        <span className="command-palette-item-snippet">{item.snippet}</span>
                      </button>
                    );
                  })}
                </div>
              ) : null,
            )
          )}
        </div>
      </div>
    </div>,
    document.body,
  );
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `npm test -- CommandPalette`
Expected: PASS (4 tests).

- [ ] **Step 6: Commit**

```bash
git add src/search/CommandPalette.tsx src/search/CommandPalette.test.tsx src/styles.css
git commit -m "feat(desktop): ⌘K command-palette overlay (portal, debounce, keyboard)"
```

---

## Task 8: Wire the palette into the shell + add the sidebar search trigger

**Files:**
- Modify: `src/shell/Sidebar.tsx`
- Modify: `src/shell/Sidebar.test.tsx`
- Modify: `src/App.tsx`
- Modify: `src/styles.css` (add `.sidebar-search-trigger`)

- [ ] **Step 1: Add the search trigger to the Sidebar**

In `src/shell/Sidebar.tsx`, add `onOpenSearch: () => void` to the props type, and render a trigger inside the top region. Replace the props destructuring + the `<div className="brand">` block:

Change the signature to:
```tsx
export function Sidebar({
  view,
  onNavigate,
  inboxUnread,
  reviewCount,
  onOpenSearch,
}: {
  view: View;
  onNavigate: (v: View) => void;
  inboxUnread: number;
  reviewCount: number;
  onOpenSearch: () => void;
}) {
```

Replace the brand block with:
```tsx
      <div className="sidebar-top">
        <div className="brand">
          <h1>AIR Agent</h1>
        </div>
        <button
          type="button"
          className="secondary-btn sidebar-search-trigger"
          onClick={onOpenSearch}
          aria-label="Open global search"
        >
          <span>Search…</span>
          <span className="sidebar-search-kbd">⌘K</span>
        </button>
      </div>
```

- [ ] **Step 2: Add the trigger styles to `styles.css`**

Append to `src/styles.css`:
```css
.sidebar-top {
  display: grid;
  gap: 10px;
}

.sidebar-search-trigger {
  width: 100%;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  color: var(--text-secondary);
}

.sidebar-search-kbd {
  font-size: 0.7rem;
  color: var(--text-tertiary);
  border: 1px solid var(--border-soft);
  border-radius: 6px;
  padding: 1px 5px;
}
```

- [ ] **Step 3: Update the Sidebar test for the new required prop + trigger**

In `src/shell/Sidebar.test.tsx`, update the `renderSidebar` helper to pass `onOpenSearch`, and add a test. Replace the helper:
```tsx
function renderSidebar(props: Partial<ComponentProps<typeof Sidebar>> = {}) {
  const onNavigate = vi.fn();
  const onOpenSearch = vi.fn();
  render(
    <ThemeProvider>
      <Sidebar view="identity" onNavigate={onNavigate} inboxUnread={0} reviewCount={0} onOpenSearch={onOpenSearch} {...props} />
    </ThemeProvider>,
  );
  return { onNavigate, onOpenSearch };
}
```
Add this test inside the `describe("Sidebar", …)` block:
```tsx
  it("calls onOpenSearch when the search trigger is clicked", () => {
    const { onOpenSearch } = renderSidebar();
    fireEvent.click(screen.getByRole("button", { name: /open global search/i }));
    expect(onOpenSearch).toHaveBeenCalledOnce();
  });
```

- [ ] **Step 4: Wire the Shell — open state, hotkey, palette, navigation resolution**

In `src/App.tsx`, add imports near the other shell imports:
```tsx
import { CommandPalette } from "./search/CommandPalette";
import { useCommandPaletteHotkey } from "./shell/useCommandPaletteHotkey";
import { useCallback } from "react";
import type { NavTarget } from "./search/types";
```
(Adjust the existing `import { useState } from "react";` to `import { useCallback, useState } from "react";` rather than adding a second react import.)

In `Shell()`, after the existing state declarations add:
```tsx
  const { select } = useInbox();
  const [searchOpen, setSearchOpen] = useState(false);
  const openSearch = useCallback(() => setSearchOpen(true), []);
  useCommandPaletteHotkey(openSearch);

  const navigateTo = useCallback((target: NavTarget) => {
    setView(target.view);
    if (target.view === "inbox" && target.convKey) select(target.convKey);
  }, [select]);
```
Update the `<Sidebar … />` usage to pass `onOpenSearch={openSearch}`, and render the palette inside the `.app-shell` div (after `<main>`):
```tsx
      <Sidebar view={view} onNavigate={setView} inboxUnread={totalUnread} reviewCount={reviewCount} onOpenSearch={openSearch} />
      <main className="main-area">
        {/* …unchanged panel ternary… */}
      </main>
      <CommandPalette open={searchOpen} onClose={() => setSearchOpen(false)} onNavigate={navigateTo} />
```
Note: `useInbox()` is already called in `Shell` for `totalUnread`; merge `select` into that single destructure (`const { totalUnread, select } = useInbox();`) rather than calling the hook twice.

- [ ] **Step 5: Typecheck + full suite**

Run: `npm run typecheck && npm test`
Expected: PASS. (`Sidebar` now requires `onOpenSearch`; the only caller is `App.tsx`, updated above; the test helper is updated.)

- [ ] **Step 6: Manual smoke check**

Run: `npm run dev:web` (or `npm run dev`). Verify:
- Press ⌘K (or Ctrl-K) anywhere → palette opens with the input focused.
- The sidebar "Search… ⌘K" trigger also opens it.
- Typing shows grouped Memory / Conversations / Files results; ↑/↓ moves the highlight; Enter jumps to the right panel (and selects the conversation for a conversation hit); Esc and backdrop-click close it.
- With the engine unreachable, the Memory group shows "Couldn't search memory." but conversations/files still work.

Stop the dev server when done.

- [ ] **Step 7: Commit**

```bash
git add src/shell/Sidebar.tsx src/shell/Sidebar.test.tsx src/App.tsx src/styles.css
git commit -m "feat(desktop): wire ⌘K palette into shell + sidebar search trigger"
```

---

## Task 9: Plan-2 gate sweep

- [ ] **Step 1: Frontend gates**

```bash
cd /Users/ahnkwangwook/air-note/apps/desktop
npm test
npm run typecheck
npm run lint
```
Expected: all PASS.

- [ ] **Step 2: Rust gates (must stay green)**

```bash
cd /Users/ahnkwangwook/air-note
cargo build -p air_agent_desktop
cargo clippy -p air_agent_desktop -- -D warnings
cargo audit --deny warnings
```
Expected: all PASS.

- [ ] **Step 3: Confirm clean + pushed**

```bash
git status -sb
git push
```
Expected: clean tree; branch pushed.

---

## Self-Review (completed during authoring)

- **Spec coverage (Sequencing items 3 + 4, Architecture B + C, Error handling):** pure helpers `filterConversations`/`filterFiles`/`rankResults` (Task 1–3); `globalSearch` façade with `Promise.allSettled` per-source isolation (Task 4); pure `paletteReducer` for ↑/↓ wrap, Enter, empty no-op (Task 5); ⌘K hotkey (Task 6); `CommandPalette` overlay with debounce, focus, grouped results, per-source "couldn't search" notes (Task 7); sidebar trigger + shell wiring + `target`→navigate (Task 8). ✓
- **Reuse-first / no new backend ops:** memory uses the existing `recall`; files use the existing `listFiles`; conversations come from the in-memory `useInbox().conversations`. No engine/Rust changes. ✓
- **Type consistency:** `View` imported from `shell/nav` (plan 1); `SearchResult`/`GroupedResults`/`NavTarget`/`EMPTY_RESULTS` defined once in `search/types.ts`; `flattenResults` ordering (memory→conversations→files) is identical in `rankResults.ts`, the reducer's `move`, and the palette's render/selection. `globalSearch` signature `(query, deps)` matches the `search` prop type injected in the CommandPalette test. ✓
- **No placeholders:** every step has complete code + exact run/expected lines. ✓
- **Out of scope (deferred):** full-text content search of message/file bodies (façade shape already supports adding those ops later without changing `SearchResult`).
