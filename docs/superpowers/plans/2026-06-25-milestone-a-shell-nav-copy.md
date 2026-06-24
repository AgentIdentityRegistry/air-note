# Milestone A — Shell, Nav & Copy — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Apply the frontend-only review fixes (#1,2,3,5,9,10,11,13,14,17,18,19) from the 2026-06-25 review to the AIR Agent desktop shell.

**Architecture:** Keep the internal `View` union keys stable (`identity|inbox|memory|review|mandates|settings`) to avoid blast radius on search NavTargets; change **labels**, **nav structure** (Review+Mandates fold into a Brain hub), the **main-screen search placement**, the **chat layout**, **footer icons**, and **copy** only. Reuse the existing `globalSearch`/`CommandPalette` for the new main-screen search bar.

**Tech Stack:** React 18 + TypeScript, Vite, Vitest + @testing-library/react (jsdom), design-token CSS in `apps/desktop/src/styles.css`.

**Gates after every task:** `npm --prefix apps/desktop run test`, `npm --prefix apps/desktop run typecheck`, `npm --prefix apps/desktop run lint` — all green. Commit per task.

---

### Task A1: Nav relabel + Brain hub (Review & Mandates fold in) — #2, #5, #11, #13

**Files:**
- Modify: `apps/desktop/src/shell/nav.ts`
- Modify: `apps/desktop/src/shell/nav.test.ts`
- Create: `apps/desktop/src/memory/BrainPanel.tsx`
- Modify: `apps/desktop/src/App.tsx` (routing + Brain-active logic)
- Modify: `apps/desktop/src/shell/Sidebar.tsx` (badge → Brain; active logic)
- Modify: `apps/desktop/src/shell/Sidebar.test.tsx`

- [ ] **Step 1 — relabel + shrink MAIN_NAV.** In `nav.ts`, MAIN_NAV becomes exactly the three top-level tabs (Review/Mandates leave the sidebar):
```ts
export const MAIN_NAV: readonly NavItemDef[] = [
  { view: "identity", label: "AIR" },
  { view: "inbox", label: "AIR Note" },
  { view: "memory", label: "Brain" },
] as const;
```
Add a helper for the Brain hub's member views:
```ts
/** Views hosted inside the Brain hub (Brain search/evolve + Review + Mandates). */
export const BRAIN_VIEWS = ["memory", "review", "mandates"] as const;
export const isBrainView = (v: View): boolean => (BRAIN_VIEWS as readonly View[]).includes(v);
```
- [ ] **Step 2 — update `nav.test.ts`** to assert MAIN_NAV is `["AIR","AIR Note","Brain"]` (length 3) and that `isBrainView("review") === true`, `isBrainView("identity") === false`. Run `npm --prefix apps/desktop run test -- nav` → PASS.
- [ ] **Step 3 — BrainPanel** (`memory/BrainPanel.tsx`): a hub with three sub-tabs (Search & Evolve / Review / Mandates), driven by the active `view`. It renders `MemoryPanel` for `memory`, `ReviewPanel` for `review`, `MandatesPanel` for `mandates`, with a sub-tab row (`.chat-subtabs` exists) and a `reviewCount` badge on the Review sub-tab.
```tsx
import { MemoryPanel } from "./MemoryPanel";
import { ReviewPanel } from "../review/ReviewPanel";
import { MandatesPanel } from "../mandates/MandatesPanel";
import { StatusBadge } from "../components/ui/StatusBadge";
import type { View } from "../shell/nav";

const SUBTABS: { view: View; label: string }[] = [
  { view: "memory", label: "Search & Evolve" },
  { view: "review", label: "Review" },
  { view: "mandates", label: "Mandates" },
];

export function BrainPanel({ view, onSubNav, reviewCount }: {
  view: View; onSubNav: (v: View) => void; reviewCount: number;
}) {
  const active = view === "memory" || view === "review" || view === "mandates" ? view : "memory";
  return (
    <div>
      <nav className="chat-subtabs" aria-label="Brain sections">
        {SUBTABS.map((t) => (
          <button key={t.view} type="button"
            className={active === t.view ? "tab-inline active" : "tab-inline"}
            aria-current={active === t.view ? "page" : undefined}
            onClick={() => onSubNav(t.view)}>
            <span>{t.label}</span>
            {t.view === "review" && reviewCount > 0 ? <StatusBadge tone="primary">{String(reviewCount)}</StatusBadge> : null}
          </button>
        ))}
      </nav>
      <div style={{ marginTop: 12 }}>
        {active === "memory" ? <MemoryPanel /> : active === "review" ? <ReviewPanel /> : <MandatesPanel />}
      </div>
    </div>
  );
}
```
- [ ] **Step 4 — route through Brain in `App.tsx`.** Replace the `memory`/`review`/`mandates` branches in the `<main>` switch with a single `BrainPanel`; keep `identity`/`inbox`/`settings`. The Sidebar's `onNavigate` still sets `view` to one of the union members; clicking the "Brain" tab navigates to `memory` (the hub default). Pass `reviewCount` + an `onSubNav={setView}` to `BrainPanel`.
```tsx
// imports: add  import { BrainPanel } from "./memory/BrainPanel";  (drop direct MemoryPanel/ReviewPanel/MandatesPanel imports)
// in <main>:
{view === "identity" ? <IdentityPanel />
 : view === "inbox" ? <InboxPanel />
 : view === "settings" ? <AirSettings />
 : <BrainPanel view={view} onSubNav={setView} reviewCount={reviewCount} />}
// The Sidebar "Brain" tab must navigate to "memory": see Step 5.
```
- [ ] **Step 5 — Sidebar: Brain active + badge.** In `Sidebar.tsx`, the `memory` NavItem is "Brain"; it must render `active` when `view` is any Brain view and show the `reviewCount` badge. Update `countFor` so Brain carries the review count, and pass an `active` override:
```tsx
import { type View, MAIN_NAV, isBrainView } from "./nav";
// inboxUnread on inbox; reviewCount surfaces on the Brain (memory) tab:
const countFor = (v: View): number | undefined =>
  v === "inbox" ? inboxUnread : v === "memory" ? reviewCount : undefined;
// in the MAIN_NAV map, compute active per item:
active={item.view === "memory" ? isBrainView(view) : view === item.view}
```
- [ ] **Step 6 — fix `Sidebar.test.tsx`** for the new labels (`AIR`, `AIR Note`, `Brain`), the 3 primary tabs, and the badge now appearing on Brain. Run the full suite + typecheck + lint → green.
- [ ] **Step 7 — Commit:** `git add -A && git commit -m "feat(desktop): Brain hub — AIR/AIR Note/Brain nav; Review+Mandates fold into Brain"`

---

### Task A2: Panel headings + remove dev line — #3, #11, #18

**Files:** `apps/desktop/src/identity/IdentityPanel.tsx`, `apps/desktop/src/memory/MemoryPanel.tsx`, `apps/desktop/src/settings/AirSettings.tsx`

- [ ] **Step 1 — IdentityPanel heading.** Change `<h2 style={{ margin: 0 }}>Your agent</h2>` → `Agent Identity Registry`.
- [ ] **Step 2 — MemoryPanel heading.** Change both `<h2 style={{ margin: 0 }}>Memory</h2>` occurrences (the `unavailable` card at ~line 55 and the main render at ~line 107) → `Brain`. Also soften the unavailable copy to plain language (e.g. "Couldn't reach your agent's memory.").
- [ ] **Step 3 — remove the dev line.** In `AirSettings.tsx`, delete the sentence about `AIR_AGENT_USE_REAL_AIR` / "Settings UI for this comes in v1.1." Read the file first; remove only that `<p>`/text node, nothing else.
- [ ] **Step 4 — gates green, commit:** `git commit -m "style(desktop): rename headings (Agent Identity Registry, Brain); drop dev env note"`

---

### Task A3: Main-screen search bar replaces the sidebar trigger — #1

**Files:** Create `apps/desktop/src/shell/MainSearch.tsx` (+ test); Modify `apps/desktop/src/shell/Sidebar.tsx`, `apps/desktop/src/App.tsx`, `apps/desktop/src/styles.css`

- [ ] **Step 1 — remove the sidebar trigger.** Delete the `.sidebar-search-trigger` button (and the `.sidebar-top` wrapper if it only held the brand + trigger; keep the `<h1>AIR Agent</h1>` brand). Remove the now-unused `onOpenSearch` plumbing from the sidebar OR repurpose (the palette is now opened from MainSearch + ⌘K). Update `Sidebar.test.tsx` accordingly.
- [ ] **Step 2 — MainSearch component.** A prominent button styled as a search field, rendered at the top of `.main-area`, that opens the existing CommandPalette overlay. ⌘K continues to open it (the `useCommandPaletteHotkey` hook already exists).
```tsx
export function MainSearch({ onOpen }: { onOpen: () => void }) {
  return (
    <button type="button" className="main-search" onClick={onOpen} aria-label="Search memory, conversations, and files">
      <span className="main-search-icon" aria-hidden>⌕</span>
      <span className="main-search-placeholder">Search memory, conversations, files…</span>
      <span className="main-search-kbd">⌘K</span>
    </button>
  );
}
```
- [ ] **Step 3 — mount in App.tsx** above the `<main>` content (inside `.main-area`, before the panel switch): `<MainSearch onOpen={openSearch} />`. The CommandPalette stays as-is.
- [ ] **Step 4 — styles.css.** Remove `.sidebar-search-trigger`/`.sidebar-search-kbd`/`.sidebar-top` rules if orphaned; add `.main-search` (full-width, prominent, token-based: surface bg, border, rounded, padding ~12px, hover state), `.main-search-icon`, `.main-search-placeholder` (text-secondary), `.main-search-kbd` (small kbd chip). Centered/max-width to feel ChatGPT-like.
- [ ] **Step 5 — gates green, commit:** `git commit -m "feat(desktop): prominent main-screen search bar (replaces sidebar search)"`

---

### Task A4: Chat layout — pin composer, fixed panes, fit viewport — #10

**Files:** `apps/desktop/src/styles.css`, `apps/desktop/src/inbox/InboxPanel.tsx` (read first), possibly `MessageThread.tsx`/`Composer.tsx`

- [ ] **Step 1 — read** `InboxPanel.tsx` + its child layout to learn the current container structure (conversation list + thread + composer).
- [ ] **Step 2 — constrain the shell to the viewport.** `.app-shell` → `height: 100vh; min-height: 0; overflow: hidden;`. `.main-area` → allow internal scroll (`min-height: 0; overflow: hidden;` when hosting the inbox; the inbox panel manages its own scroll). Verify the non-inbox panels still scroll their own content (give `.main-area` `overflow-y: auto` by default, and let the inbox panel be a fixed-height flex column).
- [ ] **Step 3 — inbox 3-pane.** The inbox becomes: fixed conversation list (own column, `overflow-y:auto`), a thread column that is a flex column with a **scrolling message area** (`flex:1; min-height:0; overflow-y:auto`) and a **composer pinned at the bottom** (`flex:0 0 auto`), all inside a `height:100%` container. Auto-scroll the message area to newest on new messages (a `ref` + `scrollIntoView`/`scrollTop = scrollHeight` effect — reuse the existing `.chat-bottom-anchor` pattern if present).
- [ ] **Step 4 — acceptance:** open the Kenny conversation → the composer + Send are visible without page scroll; only the message list scrolls; the left nav + conversation list don't move; window resize keeps everything fit. Verify in the browser preview (light + dark).
- [ ] **Step 5 — gates green, commit:** `git commit -m "fix(desktop): AIR Note chat layout — pinned composer, fixed panes, fit to viewport"`

---

### Task A5: Sidebar footer → icon-only Settings + theme toggle — #17

**Files:** `apps/desktop/src/shell/Sidebar.tsx`, `apps/desktop/src/styles.css`, `apps/desktop/src/shell/Sidebar.test.tsx`

- [ ] **Step 1 — icons.** Replace the footer's text theme-toggle and the Settings `NavItem` with two compact **icon buttons** (inline SVG: a gear for Settings, a sun/moon for theme). Each keeps an `aria-label` ("Settings", "Toggle light or dark theme") and a `title` tooltip. Settings stays `active` when `view === "settings"`.
- [ ] **Step 2 — styles.css.** Add `.sidebar-footer-icons` (a flex row, gap, right/space-between), `.icon-btn` (square, token bg/border, hover), and an `.icon-btn.active` state for Settings. Keep `.theme-toggle-btn` removal/cleanup.
- [ ] **Step 3 — update `Sidebar.test.tsx`** to find Settings + theme by `aria-label` (icon buttons, no visible text). Gates green.
- [ ] **Step 4 — Commit:** `git commit -m "style(desktop): sidebar footer → icon-only Settings + theme toggle"`

---

### Task A6: Copy pass — Title-Case + plain language + Mandates UX — #9, #14, #19

**Files:** sweep across `apps/desktop/src/**` components; focus `apps/desktop/src/mandates/MandatesPanel.tsx` (read first), buttons/labels everywhere

- [ ] **Step 1 — Title-Case UI labels/buttons.** "New message" → "New Message"; audit visible button/label/heading text and Title-Case the short action labels (leave descriptive sentences in sentence case). Grep for button text in `src/**/*.tsx`.
- [ ] **Step 2 — Mandates plain language + UX.** Rewrite `MandatesPanel` copy around: **target file → source folders → you approve each change.** Lead text: "A mandate is a standing rule: 'keep this file of mine up to date from these folders.' Your agent watches the folders and proposes an edit for you to approve — it never rewrites the file on its own." Relabel inputs to plain language ("File to keep updated", "Folders to watch", "How to keep it in sync"); make the empty/active/recent states read clearly.
- [ ] **Step 3 — plain-language sweep.** Simplify jargon-y helper text in the panels touched this milestone (Brain/AIR/AIR Note/Settings). Keep it direct.
- [ ] **Step 4 — gates green, commit:** `git commit -m "style(desktop): plain-language + Title-Case copy pass; Mandates UX rewrite"`

---

## Self-Review checklist (run before execution)

- **Spec coverage:** A1→#2/5/11/13; A2→#3/11/18; A3→#1; A4→#10; A5→#17; A6→#9/14/19. All 12 Milestone-A items covered.
- **View-key stability:** the `View` union is unchanged, so search NavTargets (`view: "memory"|"inbox"|"settings"`) keep working; only labels/structure/CSS change.
- **Test debt:** `nav.test.ts`, `Sidebar.test.tsx`, `NavItem.test.tsx` are updated where their assertions change (A1, A3, A5).
- **No new engine ops / no backend** — Milestone A is frontend-only.

## Execution

Sequential (the tasks share `App.tsx`, `Sidebar.tsx`, `styles.css` — no safe parallelism). After A6: full gates, a separate code-review pass (code-reviewer subagent), manual light/dark browser-preview QA, then open the Milestone-A PR off `main`.
