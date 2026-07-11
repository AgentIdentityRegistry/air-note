# SP3 Plan C — Library Frontend Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the Memory browser — the "Library" Brain sub-tab (search, session list, reader, Delete, note Supersede, RecallStats strip), the onboarding-gated landing flip, the Connect consent checkbox, and the re-connect prompt — per spec §9/§6a of `docs/superpowers/specs/2026-07-11-memory-hub-sp3-never-forgets-design.md` (Rev 2). **Prerequisites: Plans A and B green on this branch.**

**Architecture:** New `View` variant `"library"` wired through nav (critic M5: this is nav wiring, not one line). Eight new Tauri commands bridge to Plan-A daemon ops; TS wrappers in `api/engine.ts`; components follow the existing panel idioms (tokens only, DI-over-mockIPC per the SP2/D2b lesson).

**Tech Stack:** React + TypeScript (`apps/desktop/src`), Tauri commands (`apps/desktop/src-tauri/src/commands/`), vitest (225 existing tests must stay green), eslint 0-warn, tsc, 0 hardcoded colors.

---

### Task C1: Tauri commands — the eight bridges

**Files:**
- Modify: `apps/desktop/src-tauri/src/commands/engine.rs` (follow the existing `engine_recall` pattern: `#[tauri::command]` → `Result<Dto, String>` → tested pure core)
- Modify: `apps/desktop/src-tauri/src/main.rs` (`generate_handler!` list, ~125-246)
- Test: the commands module's existing test pattern

- [ ] **Step 1: Read** `commands/engine.rs` — copy the exact DTO/delegation idiom of `engine_recall`/`engine_list_files`.
- [ ] **Step 2: Write failing tests** for the DTO mapping layer (pure fns, per the existing pattern): `SessionSummaryWire → SessionSummaryDto` (snake_case fields matching TS), `NoteWire → NoteDto`, `RecallStatsWire → RecallStatsDto`; `GetSession`'s `Err{Rejected}` maps to the string the UI matches on (`"session not found or deleted"`).
- [ ] **Step 3: Run** → FAIL. **Step 4: Implement** commands: `engine_list_sessions`, `engine_get_session(session_id)`, `engine_delete_session(session_id)`, `engine_list_notes`, `engine_supersede_note(event_id, text)`, `engine_recall_stats`, `engine_set_capture_enabled(enabled)` (forward-only: `backfill=false`), `engine_capture_enabled`. Register all eight in `generate_handler!`.
- [ ] **Step 5: Run** `cargo test -p bossclaw_desktop && cargo clippy --workspace --all-targets -- -D warnings` → PASS/clean. **Step 6: Commit** `git commit -m "feat(desktop): eight Library Tauri commands (sessions/notes/forget/stats/capture-flags)"`

---

### Task C2: TS API wrappers + DTOs

**Files:**
- Modify: `apps/desktop/src/api/engine.ts`
- Test: `apps/desktop/src/api/engine.test.ts` (or the existing api test file — match siblings)

- [ ] **Step 1: Write failing tests** (mock `invoke` the way existing api tests do — DI, not mockIPC):

```ts
it("listSessions invokes engine_list_sessions and returns typed rows", async () => { /* … */ });
it("deleteSession surfaces the already-deleted rejection as a typed result", async () => { /* … */ });
it("supersedeNote returns the new event id", async () => { /* … */ });
```

- [ ] **Step 2: Run** `npm run test --workspace @bossclaw/desktop` → FAIL.
- [ ] **Step 3: Implement:**

```ts
export type SessionSummaryDto = { session_id: string; title: string; project: string;
  tool: string; started_at: number; ended_at: number; approx_bytes: number };
export type SessionDetailDto = { summary: SessionSummaryDto; markdown: string };
export type NoteDto = { event_id: string; text: string; created_at: number;
  superseded_by: string | null };
export type RecallStatsDto = { total: number; misses: number;
  recent_misses: { query: string; at: number }[] };

export const listSessions = (): Promise<SessionSummaryDto[]> => invoke("engine_list_sessions");
export const getSession = (sessionId: string): Promise<SessionDetailDto> =>
  invoke("engine_get_session", { sessionId });
export const deleteSession = (sessionId: string): Promise<void> =>
  invoke("engine_delete_session", { sessionId });
export const listNotes = (): Promise<NoteDto[]> => invoke("engine_list_notes");
export const supersedeNote = (eventId: string, text: string): Promise<string> =>
  invoke("engine_supersede_note", { eventId, text });
export const recallStats = (): Promise<RecallStatsDto> => invoke("engine_recall_stats");
export const setCaptureEnabled = (enabled: boolean): Promise<void> =>
  invoke("engine_set_capture_enabled", { enabled });
export const captureEnabled = (): Promise<boolean> => invoke("engine_capture_enabled");
```

- [ ] **Step 4: Run** → PASS + `npm run typecheck --workspace @bossclaw/desktop`. **Step 5: Commit** `git commit -m "feat(desktop): Library api wrappers + DTOs"`

---

### Task C3: Nav wiring — `"library"` View + onboarding-gated landing

**Files:**
- Modify: `apps/desktop/src/shell/nav.ts` (View union at :2, MAIN_NAV at ~:10, BRAIN_VIEWS at :14)
- Modify: `apps/desktop/src/memory/BrainPanel.tsx` (SUBTABS at :8, isBrainView fallback at ~:31)
- Modify: `apps/desktop/src/App.tsx` (default view at :43, onboarding gate at :69)
- Test: existing nav/App tests (extend)

- [ ] **Step 1: Write failing tests:**

```ts
it("library is a Brain view and the first sub-tab", () => {
  expect(BRAIN_VIEWS).toContain("library");
  expect(SUBTABS[0].view).toBe("library");
});
it("MAIN_NAV Brain item routes to library", () => { /* nav item view === "library" */ });
it("onboarded users land on library; fresh users land on identity", () => {
  // render App with onboarded=true → Library visible; onboarded=false → IdentityPanel
  // (architect Minor: never bypass the onboarding gate)
});
```

- [ ] **Step 2: Run** → FAIL. **Step 3: Implement:** add `"library"` to the `View` union + `BRAIN_VIEWS`; `MAIN_NAV` Brain item → `"library"`; `SUBTABS` gains `{ view: "library", label: "Library" }` FIRST; `isBrainView` fallback → `"library"`; `App.tsx` initial view = `onboarded ? "library" : "identity"` (read the same onboarding signal the `:69` gate uses — follow how App already learns onboarding state; if it's async, initialize `"identity"` and flip once onboarding resolves true, which also preserves the gate).
- [ ] **Step 4: Run** vitest + typecheck → PASS. **Step 5: Commit** `git commit -m "feat(desktop): library View wired through nav; Brain/Library is the onboarded landing"`

---

### Task C4: Library — list view with search

**Files:**
- Create: `apps/desktop/src/memory/LibraryPanel.tsx`
- Modify: `apps/desktop/src/memory/BrainPanel.tsx` (render LibraryPanel for `"library"`)
- Test: `apps/desktop/src/memory/LibraryPanel.test.tsx`

- [ ] **Step 1: Write failing tests** (DI props for api fns, per house style):

```ts
it("renders sessions (title/project/date) and notes, newest first", async () => { /* … */ });
it("search box filters client-side across titles, projects, and note text", async () => { /* … */ });
it("a 'search memory' action runs recall and shows hits in a Memory group", async () => { /* … */ });
it("empty archive shows the capture-off hint when captureEnabled=false", async () => { /* … */ });
```

- [ ] **Step 2: Run** → FAIL. **Step 3: Implement** `LibraryPanel` — props `{ api }` bundle (listSessions/listNotes/recall/recallStats/captureEnabled + the C5/C6 mutation fns threaded later); layout: search input on top (existing input classes/tokens), sessions section, notes section; client-side filter (`title|project|note.text` case-insensitive contains); a "Search memory" button running `recall(query, 10)` into a results group (reuse `recallView.toRow` from MemoryPanel if exported — else copy its row rendering; do NOT re-implement scoring display differently).
- [ ] **Step 4: Run** vitest + `grep -rn "#[0-9a-fA-F]\{3,8\}" apps/desktop/src/memory/LibraryPanel.tsx` → PASS / no matches (0 hardcoded colors).
- [ ] **Step 5: Commit** `git commit -m "feat(desktop): Library panel — sessions+notes list with client search and recall action"`

---

### Task C5: Library — session reader + Delete flow

**Files:**
- Modify: `apps/desktop/src/memory/LibraryPanel.tsx` (+ a small `SessionReader` component in the same file or sibling — split if >~150 lines)
- Test: extend `LibraryPanel.test.tsx`

- [ ] **Step 1: Write failing tests:**

```ts
it("clicking a session opens the reader with rendered markdown", async () => { /* getSession */ });
it("Delete asks for confirmation quoting the honesty note, then removes the row", async () => {
  // confirm dialog text includes "title remains in the encrypted log" (spec §7b disclosure)
});
it("deleting an already-deleted session shows 'already deleted', not a generic error", async () => {
  // api.deleteSession rejects with the Rejected message → friendly state (spec §3 race)
});
```

- [ ] **Step 2: Run** → FAIL. **Step 3: Implement:** reader pane renders `markdown` as preformatted text (no HTML injection — render as text, NOT dangerouslySetInnerHTML); Delete button → in-app confirm (existing modal/danger-btn idioms; the `.danger-btn` class exists since the shell redesign) with the honesty-note copy; on success remove from list; on `"session not found or deleted"` show the already-deleted notice and refresh the list.
- [ ] **Step 4: Run** → PASS. **Step 5: Commit** `git commit -m "feat(desktop): session reader + honest Delete flow (confirm, tombstone copy, race-safe)"`

---

### Task C6: Library — note Supersede + RecallStats strip

**Files:**
- Modify: `apps/desktop/src/memory/LibraryPanel.tsx`
- Test: extend `LibraryPanel.test.tsx`

- [ ] **Step 1: Write failing tests:**

```ts
it("Supersede opens edit-in-place prefilled with the note, saves the corrected text", async () => {
  // supersedeNote(eventId, newText) called; list shows replacement, old marked superseded
});
it("stats strip renders totals and recent misses (queries only)", async () => {
  // "N recalls · M found nothing" + up to 5 recent miss queries
});
```

- [ ] **Step 2: Run** → FAIL. **Step 3: Implement:** per-note Supersede button → inline textarea (prefilled) + Save/Cancel; superseded notes render dimmed with a "superseded" chip (existing muted-token classes); stats strip at the panel top-right fed by `recallStats()`.
- [ ] **Step 4: Run** → PASS. **Step 5: Commit** `git commit -m "feat(desktop): note supersede edit-in-place + recall-miss stats strip"`

---

### Task C7: Integrations panel — consent checkbox + re-connect prompt + capture toggle

**Files:**
- Modify: `apps/desktop/src/settings/IntegrationsPanel.tsx`
- Modify: `apps/desktop/src/api/integrations.ts` (status DTO gains `capture`; `connectClaudeCode(captureConsent)`; `backfillCount()`; `setCaptureEnabled` re-export)
- Test: `apps/desktop/src/settings/IntegrationsPanel.test.tsx` (extend SP2's suite)

- [ ] **Step 1: Write failing tests:**

```ts
it("connect flow shows the pre-checked consent checkbox with the disclosure copy and N count", async () => {
  // copy MUST contain "processed on this Mac" AND "stored unencrypted on this Mac" (spec §6a)
  // and the live count from backfillCount()
});
it("unchecking consent still connects but passes captureConsent=false", async () => { /* … */ });
it("Connected-without-capture shows 'Re-connect to enable session capture'", async () => {
  // status Connected { capture: false } → the SP2-install-base prompt (critic M3)
});
it("capture toggle calls setCaptureEnabled and is labeled forward-only", async () => { /* … */ });
```

- [ ] **Step 2: Run** → FAIL. **Step 3: Implement:** checkbox (default checked, visually separate from the Connect button per security L11) with the two-clause disclosure + `~30 days, N found`; wire `connectClaudeCode(captureConsent)`; render the third detect state with the re-connect prompt; a capture on/off toggle (calls `engine_set_capture_enabled`; helper text: "applies going forward — history import is only offered at Connect").
- [ ] **Step 4: Run** vitest + 0-hardcoded-colors grep on the panel → PASS. **Step 5: Commit** `git commit -m "feat(desktop): consent checkbox + disclosure, re-connect prompt, forward-only capture toggle"`

---

### Task C8: ⌘K palette — Library group

**Files:**
- Modify: `apps/desktop/src/search/globalSearch.ts` (+ deps type), `apps/desktop/src/search/CommandPalette.tsx` (group rendering + navigation target)
- Test: extend the existing globalSearch/CommandPalette tests

- [ ] **Step 1: Write failing tests:**

```ts
it("globalSearch includes a Library group from listSessions, filtered by query", async () => { /* … */ });
it("selecting a Library hit navigates to the library view", async () => { /* setView("library") */ });
```

- [ ] **Step 2: Run** → FAIL. **Step 3: Implement:** add `listSessions` to the `globalSearch` deps fan-out (client-side title/project match, cap at the existing `RESULTS_PER_GROUP`); palette renders the group and routes selection to `"library"` (pass the selected session id via the existing navigation state mechanism if one exists — otherwise just land on Library with the search box prefilled).
- [ ] **Step 4: Run** → PASS. **Step 5: Commit** `git commit -m "feat(desktop): command palette gains a Library group"`

---

### Task C9: Plan-C gates + whole-branch wrap

- [ ] **Step 1:** Full frontend gates: `npm run typecheck --workspace @bossclaw/desktop && npm run lint --workspace @bossclaw/desktop && npm run test --workspace @bossclaw/desktop` → clean, 0-warn, all green (225 pre-existing + new).
- [ ] **Step 2:** Repo-wide `grep -rn "#[0-9a-fA-F]\{3,8\}" apps/desktop/src --include="*.tsx"` → 0 hardcoded colors.
- [ ] **Step 3:** Full workspace gates: `cargo build --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` (expect the known keychain-test exclusion if it flakes — run desktop tests keychain-free per the D-milestone lesson).
- [ ] **Step 4:** Placeholder sweep of the whole branch diff: no TODO/stub/`.only`/`.skip`/`#[allow(dead_code)]` leftovers.
- [ ] **Step 5:** Dispatch the whole-branch final review (code + the remaining spec §11 security gate #2, snapshot fencing — if not already run in Plan A11's review) → SHIP required before PR.
- [ ] **Step 6:** Push; open the PR (`feat-memory-hub-sp3-never-forgets` → main) with the spec+plans linked, deferred items listed (§12), and the CI-dormant note if stacking applies. Update GBrain handoff per the AIR session protocol.
