# Desktop UI Shell Redesign — Design

**Date:** 2026-06-24
**Status:** Design (approved in brainstorming; pending spec review → plan)
**Branch:** `desktop-ui-shell-redesign`

## Goal

Replace the AIR Agent desktop app's current shell — a horizontal top row of nav buttons in a
centered 760px column (`apps/desktop/src/App.tsx` `Shell()`) — with a modern, Claude/GPT-style
shell:

1. A **left vertical sidebar** holding the existing navigation (Identity, Inbox, Memory, Review,
   Mandates, Settings) with the current count badges, plus a search trigger on top and a
   light/dark theme toggle + Settings pinned at the bottom.
2. A **global ⌘K command-palette** that searches across **memory + conversations + files** and
   shows grouped results in a keyboard-navigable overlay.
3. A **full visual restyle** adopting the existing-but-largely-unused design system in
   `apps/desktop/src/styles.css` (CSS-variable tokens + a complete dark theme) across the shell
   **and every panel**, replacing today's ad-hoc inline styles.

This is a cohesive single design, delivered as a **sequenced multi-task plan** (see Sequencing).

## Background / current state (grounded)

- `App.tsx` → `Shell()` renders a horizontal `<nav>` of `<Button>`s and a body ternary that
  swaps panels by a `View` union (`"identity" | "inbox" | "memory" | "review" | "mandates" |
  "settings"`). `InboxNavButton` (unread count via `useInbox`) and `ReviewNavButton` (pending
  count via polled `listProposals`) are bespoke badge buttons. Providers in `App()`:
  `IdentityProvider`, `OnboardingProvider`, `InboxProvider`, `AiLoopProvider`.
- A real **design system already exists** in `styles.css` (imported by `main.tsx`): tokens
  (`--bg`, `--surface`, `--surface-soft`, `--primary`, `--text-*`, `--elev-*`, `--radius-*`,
  motion vars, a font scale), a full `:root[data-theme="dark"]` theme, skins, and chat-layout
  tokens. There is a `components/ui/*` library that uses it (`Surface`, `ToggleSwitch`,
  `SettingsSectionCard`, `SlidePanel`, `StatusBadge`, …). **But the shell + most panels ignore
  it:** `Button`/`Card` and ~188 inline `style={{…}}` sites hardcode colors (`#1a1a1a`, `#eee`).
  Legacy `.sidebar` CSS exists in `styles.css` but is rendered by nothing.
- `api/engine.ts` already exposes `recall(query, k)` (memory search, used by `MemoryPanel`),
  `listProposals`, the desktop file list, the mandate ops, etc. There is **no** conversation- or
  file-search op yet; conversation summaries and the file list are already loaded client-side
  (inbox state; the Sources/files projection).

## Decisions (from brainstorming)

| Decision | Choice |
|---|---|
| Sidebar structure | **Nav-only** — search on top, the 6 menus vertical (with badges), Settings pinned bottom |
| Search scope | **Global** — memory + conversations + files, grouped results |
| Search UX | **⌘K command-palette overlay** — floats over the current view, keyboard-driven |
| Visual scope | **Full redesign** — adopt the design-system tokens + dark mode across shell **and all panels** |
| Delivery | One cohesive design, **sequenced plan** (not phased shipping) |

## Architecture

### A. Shell + Sidebar
- New `src/shell/Sidebar.tsx`: fixed-width (~220px) vertical column —
  - top: a **search trigger** (button styled as a search field, shows "⌘K") that opens the
    command palette;
  - middle: the nav items via a generalized `NavItem` (`{ view, label, count? }`) that subsumes
    the bespoke `InboxNavButton`/`ReviewNavButton` badge logic (badge source stays the same:
    inbox unread, polled proposal count);
  - bottom: a **theme toggle** (light/dark) + **Settings**.
- `App.tsx` `Shell()` becomes a 2-column flex: `<Sidebar view={view} onNavigate={setView} />` +
  a scrollable `<main>` rendering the selected panel. `View` union, onboarding flow, and the
  panels themselves are unchanged in behavior. The full-bleed layout replaces the 760px column.
- `Shell()` mounts `<CommandPalette />` once (shell-level) so ⌘K works from any view.

### B. Global search façade
- New `src/search/globalSearch.ts`: `globalSearch(query): Promise<GroupedResults>` fans out
  **concurrently** to three sources and returns
  `{ memory: Result[], conversations: Result[], files: Result[] }`, each `Result` =
  `{ id, kind, title, snippet, navigate: () => void }`.
  - **Memory** — the existing `recall(query, k)` op (server, already built).
  - **Conversations** — start by **filtering already-loaded conversation summaries** (title +
    preview) client-side via a pure helper; a server op for full message-content search is a
    later add (see Sequencing / Out of scope).
  - **Files** — start by **filtering the already-loaded file list** (name/path) client-side; a
    server op for file-content search is a later add.
- Pure, unit-tested helpers in `src/search/`: `filterConversations`, `filterFiles`, and
  `mergeAndGroup`/`rankResults` (ordering + capping per group). Keeping I/O (recall) at the edge
  and the filtering/grouping pure makes the façade testable without the engine.
- Rationale for "reuse first": memory recall is the only source needing the engine; conversations
  and files are already in memory client-side, so the global-search **value lands without new
  backend ops**, and deeper content search can be added op-by-op later without changing the
  façade's shape.

### C. ⌘K command palette
- New `src/search/CommandPalette.tsx`: a portal overlay (focus-trapped, click-outside + Esc to
  close) with a global `keydown` listener for ⌘K / Ctrl-K. A debounced query drives
  `globalSearch`; results render grouped (Memory · Conversations · Files); ↑/↓ move a selection
  index across the flattened result list; Enter calls the selected `Result.navigate()` (switches
  `View` and, where applicable, focuses the item) and closes.
- The selection/keyboard logic is a **pure reducer** (`paletteReducer(state, action)`) so the
  arrow/enter/escape behavior is unit-tested independently of the DOM.

### D. Design-system adoption + dark mode
- Replace hardcoded inline styles in `components/Button.tsx`, `components/Card.tsx`, and **every
  panel** (`identity`, `inbox`, `memory`, `review`, `mandates`, `settings`) with the `styles.css`
  tokens / token-based classes, reusing `components/ui/*` where it fits. `Button`/`Card` become
  token-driven (so the whole tree themes from one place).
- New `src/state/theme.tsx` `ThemeProvider`: holds `"light" | "dark"`, sets `:root[data-theme]`,
  persists to `localStorage` (key `air.theme`), defaults to system preference
  (`prefers-color-scheme`) on first run. The sidebar theme toggle flips it. Tokens for dark
  already exist — no new color work, just wiring + migrating call sites off inline styles.

## Data flow
- `View` state stays in `Shell()`; `Sidebar` is presentational (`view` + `onNavigate`).
- Theme: `ThemeProvider` (added in `App()` alongside the existing providers) → `data-theme` on
  `:root` → tokens cascade. Persisted; no engine involvement.
- Search: palette-local state (`query`, `results`, `selectedIndex`, `open`), opened by the global
  hotkey or the sidebar trigger. `globalSearch` reads `recall` (engine) + in-memory conversation
  summaries + file list. No new global store.
- Existing providers (`Identity`/`Onboarding`/`Inbox`/`AiLoop`) and all engine ops are untouched.

## Error handling
- `globalSearch` runs the three sources independently; a failing source yields an empty group +
  a small inline "couldn't search <source>" note rather than failing the whole palette
  (`Promise.allSettled` semantics). An empty query shows nothing (or a hint), never errors.
- Theme read from `localStorage` falls back to system preference, then light, on any parse error.

## Testing
- **Pure (vitest):** `filterConversations`, `filterFiles`, `mergeAndGroup`/`rankResults`,
  `paletteReducer` (↑/↓ wrap, Enter on empty is a no-op, Esc closes), theme persistence helper.
- **Component (testing-library):** CommandPalette opens on ⌘K, renders grouped results for a
  stubbed `globalSearch`, Enter calls the selected result's `navigate`, Esc closes; Sidebar
  renders the nav + badges and calls `onNavigate`.
- **Backend:** none required for the reuse-first façade. If/when the conversation/file
  content-search ops are added, they get engine/desktop tests in the SP-op style.
- All existing vitest + the Rust gates (incl. the new `cargo audit` job) stay green.

## Sequencing (one design, ordered plan)
1. **Theme foundation** — `ThemeProvider` + migrate `Button`/`Card` to tokens (smallest, unblocks the rest).
2. **Shell + Sidebar** — 2-column layout, `NavItem` (subsuming the badge buttons), theme toggle.
3. **Search façade + pure helpers** — `globalSearch` over recall + client-side conversation/file filters, fully unit-tested.
4. **⌘K command palette** — overlay + `paletteReducer` + global hotkey + navigation.
5. **Panel restyle pass** — migrate each panel off inline styles to tokens (the bulk; one panel per task keeps PRs reviewable).
6. **Polish + gates** — dark-mode QA across every panel, a11y (focus trap, labels), full gate sweep.

If the plan proves too large for one implementation cycle, decompose at the writing-plans step
into sub-projects along these seams (theme+shell · search · restyle), each its own plan.

## Out of scope (YAGNI)
- Full-text **content** search of conversation messages / file bodies (start with title/path/
  preview filtering; add server ops later if wanted).
- Conversation-history list in the sidebar (we chose nav-only; not GPT-style recents).
- Icon-rail / 3-column layout (rejected in brainstorming).
- Multiple skins beyond light/dark (the `data-skin` machinery exists but we ship just light+dark).
- Any change to engine behavior or the SP1–SP5 feature set.
