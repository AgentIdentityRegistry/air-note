# Desktop UI Shell Redesign — Plan 3 of 3: Panel Restyle + Polish

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate every panel's inner content off hardcoded inline colors onto the `styles.css` design-system tokens, adopt the existing `components/ui/*` primitives where they fit, and verify the whole app in both light and dark themes with an accessibility pass.

**Architecture:** This is a mechanical, behavior-preserving restyle. The rule: **every hardcoded color literal becomes a `var(--token)`** so dark mode (added in plan 1) works everywhere; layout inline styles (`display`, `gap`, `padding`) may stay inline or move to existing classes. Where a `components/ui/*` component already does the job (`ToggleSwitch`, `StatusBadge`, `SettingsSectionCard`, `Surface`), adopt it instead of re-styling by hand. One panel per task keeps each PR small and reviewable.

**Tech Stack:** React 18 + TypeScript, Tauri v2, CSS-variable tokens in `src/styles.css`, Vitest 2 (existing pure tests must stay green; no new unit tests — see "Testing approach").

**Depends on Plan 1** (Card/Button already token-based; `ThemeProvider`/dark mode live) **and Plan 2** (search shipped). **Do not start until plans 1 and 2 have landed.**

**Testing approach:** These tasks change appearance only — there is no new behavior to drive with TDD, and the codebase has zero panel component tests by design (logic already lives in tested pure helpers like `recallView.ts`, `proposalView.ts`, `mandateView.ts`). So each restyle task is verified by: (a) `npm run typecheck` passes, (b) `npm test` stays green (no logic touched), (c) a **manual light-AND-dark visual check** of that panel. The dark-mode/a11y sweep (Task 10) is the consolidated verification gate.

**Working directory for all commands:** `/Users/ahnkwangwook/air-note/apps/desktop`. Run `cd /Users/ahnkwangwook/air-note/apps/desktop` first.

---

## The literal → token mapping (apply consistently in every task)

| Hardcoded literal(s) | Replace with |
|---|---|
| `#1a1a1a`, `#0B0F17`, `#000`, `black` (text) | `var(--text-primary)` |
| `#666`, `#555`, `#444` (body/secondary text) | `var(--text-secondary)` |
| `#888`, `#999`, `#aaa` (muted/hint text) | `var(--text-tertiary)` |
| `#eee`, `#ccc`, `#E8EAED`, `#E0E3E8` (borders) | `var(--border-soft)` |
| `white`, `#fff`, `#ffffff` (surfaces) | `var(--surface)` |
| `#f0f0f0`, `#f6f6f6`, `#F8F9FB`, `#F3F4F6` (soft fills) | `var(--surface-soft)` |
| `#2F6BFF`, `#06c` (accent/links) | `var(--primary)` |
| `#b00`, `#A75D61` (errors) | `var(--error)` |
| `#070` (success) | `var(--success)` |
| `#A57C42` (warning) | `var(--warning)` |
| Selected-row tint `#EEF3FF` | `color-mix(in srgb, var(--primary) 10%, var(--surface))` |
| Banner bg `#FFF3F3` (error) | `color-mix(in srgb, var(--error) 10%, var(--surface))` |
| Banner bg `#FFF8EC` (warning) | `color-mix(in srgb, var(--warning) 12%, var(--surface))` |
| Diff add bg `#eafaef` / del bg `#fdecea` | `var(--diff-add-bg)` / `var(--diff-del-bg)` (added in Task 6) |

Spacing/radius numbers (`6/8/10/12/16/24`) are theme-independent and may stay as-is. After each task, grep the file to confirm no color literals remain:
```bash
grep -nE "#[0-9a-fA-F]{3,6}|: *['\"]white|: *['\"]black" src/<path>   # expect: no color matches
```

---

## File Structure

**Modified (one per task, in order):**
- Task 1: `src/components/Input.tsx`, `src/components/Loading.tsx`
- Task 2: `src/identity/IdentityPanel.tsx`
- Task 3: `src/inbox/InboxPanel.tsx`, `ConversationList.tsx`, `DialControl.tsx`, `MessageThread.tsx`, `Composer.tsx`, `NeedsDaemon.tsx`
- Task 4: `src/inbox/AIPanel.tsx`
- Task 5: `src/memory/MemoryPanel.tsx`
- Task 6: `src/styles.css` (diff tokens), `src/review/ReviewPanel.tsx`
- Task 7: `src/mandates/MandatesPanel.tsx`
- Task 8: `src/settings/AirSettings.tsx`, `src/sources/SourcesPanel.tsx`
- Task 9: `src/onboarding/{Welcome,NameAgent,GenerateAndRegister,Done}.tsx`
- Task 10: verification only (+ any small fixes the sweep surfaces)

---

## Task 1: Shared primitives — Input + Loading

**Files:** `src/components/Input.tsx`, `src/components/Loading.tsx`

- [ ] **Step 1: Rewrite Input to lean on the themed base `input` element**

`styles.css` already styles bare `<input>` with tokens (border, radius, focus ring). The component only needs to spread props + allow caller overrides. Replace the entire contents of `src/components/Input.tsx`:
```tsx
import { InputHTMLAttributes } from "react";

export function Input({ style, ...rest }: InputHTMLAttributes<HTMLInputElement>) {
  // The base `input` rule in styles.css supplies border/radius/focus tokens; only width is enforced here.
  return <input {...rest} style={{ width: "100%", ...style }} />;
}
```

- [ ] **Step 2: Rewrite Loading to use a token color**

Replace the entire contents of `src/components/Loading.tsx`:
```tsx
export function Loading({ label = "Working..." }: { label?: string }) {
  return <div style={{ color: "var(--text-tertiary)", fontStyle: "italic" }}>{label}</div>;
}
```

- [ ] **Step 3: Verify**

Run: `npm run typecheck && npm test`
Expected: PASS. Then:
```bash
grep -nE "#[0-9a-fA-F]{3,6}" src/components/Input.tsx src/components/Loading.tsx
```
Expected: no matches.

- [ ] **Step 4: Commit**

```bash
git add src/components/Input.tsx src/components/Loading.tsx
git commit -m "refactor(desktop): Input/Loading use design tokens"
```

---

## Task 2: IdentityPanel (simplest — warm-up)

**File:** `src/identity/IdentityPanel.tsx` (~45 lines, ~11 inline sites; the only colors are `#666` ×3 on the field labels).

- [ ] **Step 1: Apply the mapping**

In `src/identity/IdentityPanel.tsx`, replace every `color: "#666"` with `color: "var(--text-secondary)"` (the field labels around lines 16, 21, 34). Leave layout (`marginTop`, `fontSize`, `fontFamily: "monospace"`) as-is.

- [ ] **Step 2: Verify**

Run: `npm run typecheck && npm test`
Expected: PASS.
```bash
grep -nE "#[0-9a-fA-F]{3,6}" src/identity/IdentityPanel.tsx
```
Expected: no matches.

- [ ] **Step 3: Manual check (light + dark)**

Run `npm run dev:web`, open Identity, toggle the theme. Labels readable in both; the card uses the themed `.card`. Stop the server.

- [ ] **Step 4: Commit**

```bash
git add src/identity/IdentityPanel.tsx
git commit -m "style(desktop): IdentityPanel uses tokens (dark-mode ready)"
```

---

## Task 3: InboxPanel shell + small children

**Files:** `src/inbox/InboxPanel.tsx`, `ConversationList.tsx`, `DialControl.tsx`, `MessageThread.tsx`, `Composer.tsx`, `NeedsDaemon.tsx`.

- [ ] **Step 1: InboxPanel banners + text**

In `src/inbox/InboxPanel.tsx` apply the mapping. The two non-obvious sites are the banners:
- Offline banner (≈ line 53): `background: "#FFF3F3", color: "#A75D61"` → `background: "color-mix(in srgb, var(--error) 10%, var(--surface))", color: "var(--error)"`.
- Archive-error banner (≈ line 58): `background: "#FFF8EC", color: "#A57C42"` → `background: "color-mix(in srgb, var(--warning) 12%, var(--surface))", color: "var(--warning)"`.
- The `color: "#666"` notes (≈ lines 47, 79) → `var(--text-secondary)`.
(`InboxPanel` already uses `ToggleSwitch` — leave it.)

- [ ] **Step 2: ConversationList selected-row tint**

In `src/inbox/ConversationList.tsx`, the selected-state literals (≈ lines 18–19) `#2F6BFF` / `#eee` / `#EEF3FF`:
- border `#2F6BFF` → `color-mix(in srgb, var(--primary) 26%, transparent)`
- border `#eee` → `var(--border-soft)`
- background `#EEF3FF` → `color-mix(in srgb, var(--primary) 10%, var(--surface))`
- any `#666` → `var(--text-secondary)`.

- [ ] **Step 3: DialControl + MessageThread + Composer + NeedsDaemon**

- `DialControl.tsx` (≈ lines 24, 28): `#ccc` → `var(--border-soft)`; `#2F6BFF` → `var(--primary)`; `#0B0F17` → `var(--text-primary)`.
- `MessageThread.tsx` (≈ line 17): bubble `#2F6BFF` → `var(--primary)`; `#F3F4F6` → `var(--surface-soft)`; `#0B0F17` → `var(--text-primary)`.
- `Composer.tsx`: no color literals (layout only) — confirm with grep, no change expected.
- `NeedsDaemon.tsx` (≈ line 10): `<pre>` `background: "#F3F4F6"` → `background: "var(--surface-soft)"`.

- [ ] **Step 4: Verify**

Run: `npm run typecheck && npm test`
Expected: PASS.
```bash
grep -rnE "#[0-9a-fA-F]{3,6}|: *['\"]white" src/inbox/InboxPanel.tsx src/inbox/ConversationList.tsx src/inbox/DialControl.tsx src/inbox/MessageThread.tsx src/inbox/Composer.tsx src/inbox/NeedsDaemon.tsx
```
Expected: no matches.

- [ ] **Step 5: Manual check (light + dark)** — open Inbox, view a conversation, toggle theme: bubbles, selected row, banners all readable.

- [ ] **Step 6: Commit**

```bash
git add src/inbox/InboxPanel.tsx src/inbox/ConversationList.tsx src/inbox/DialControl.tsx src/inbox/MessageThread.tsx src/inbox/Composer.tsx src/inbox/NeedsDaemon.tsx
git commit -m "style(desktop): Inbox panel + children use tokens (dark-mode ready)"
```

---

## Task 4: AIPanel (module-level style consts)

**File:** `src/inbox/AIPanel.tsx` (~233 lines; defines `wrapStyle/rowStyle/draftTextStyle/sentTextStyle/textareaStyle` as `CSSProperties` consts at the bottom, lines ≈193–233, with hex `#E8EAED #F8F9FB #E0E3E8 #1a1a1a #888 #ccc #aaa #A75D61`).

- [ ] **Step 1: Convert the module-level style consts to tokens**

In the `CSSProperties` consts block (≈ lines 193–233) apply the mapping:
- `#E8EAED`, `#E0E3E8`, `#ccc` (borders) → `var(--border-soft)`
- `#F8F9FB` (soft fill) → `var(--surface-soft)`
- `#1a1a1a` (text) → `var(--text-primary)`
- `#888`, `#aaa` (muted) → `var(--text-tertiary)`
- `#A75D61` (error) → `var(--error)`

- [ ] **Step 2: Convert any inline color sites in the JSX** (the other ~12 `style={{}}` sites) using the same mapping.

- [ ] **Step 3: Verify**

Run: `npm run typecheck && npm test`
Expected: PASS.
```bash
grep -nE "#[0-9a-fA-F]{3,6}|: *['\"]white" src/inbox/AIPanel.tsx
```
Expected: no matches.

- [ ] **Step 4: Manual check (light + dark)** — open Inbox → AI draft panel; toggle theme; draft vs sent text and the textarea are readable.

- [ ] **Step 5: Commit**

```bash
git add src/inbox/AIPanel.tsx
git commit -m "style(desktop): AIPanel uses tokens (dark-mode ready)"
```

---

## Task 5: MemoryPanel

**File:** `src/memory/MemoryPanel.tsx` (~200 lines, ~22 inline sites). Logic stays in the already-tested `recallView.ts`/`evolveStatus.ts`.

- [ ] **Step 1: Apply the mapping**

Key sites:
- Raw search `<input>` border `#ccc` (≈ line 120) → `var(--border-soft)` (or drop the inline border to inherit the themed base `input` — preferred).
- Hit-kind badge (≈ lines 141–142): `color: "#555", background: "#f0f0f0"` → `color: "var(--text-secondary)", background: "var(--surface-soft)"`. (Optional: swap this hand-rolled badge for `<StatusBadge tone="neutral">` from `components/ui/StatusBadge`.)
- List item divider `borderBottom: "1px solid #eee"` (≈ line 138) and the Evolve section divider `borderTop: "1px solid #eee"` (≈ line 156) → `1px solid var(--border-soft)`.
- `color: "#999"` (≈ line 146) → `var(--text-tertiary)`.
- Error text `#b00` (≈ lines 128, 196) → `var(--error)`.
- Remaining `#666` → `var(--text-secondary)`.

- [ ] **Step 2: Verify**

Run: `npm run typecheck && npm test`
Expected: PASS.
```bash
grep -nE "#[0-9a-fA-F]{3,6}|: *['\"]white" src/memory/MemoryPanel.tsx
```
Expected: no matches.

- [ ] **Step 3: Manual check (light + dark)** — run a recall search, view hits + the Evolve section; toggle theme.

- [ ] **Step 4: Commit**

```bash
git add src/memory/MemoryPanel.tsx
git commit -m "style(desktop): MemoryPanel uses tokens (dark-mode ready)"
```

---

## Task 6: ReviewPanel (+ diff tokens)

**Files:** `src/styles.css` (new diff tokens), `src/review/ReviewPanel.tsx` (~274 lines — richest color set: an inline diff renderer using `#b00 #070 #444 #fdecea #eafaef #f6f6f6`). Logic stays in the tested `proposalView.ts`/`diffView.ts`/`applyFlow.ts`.

- [ ] **Step 1: Add diff tokens to `styles.css` (light + dark)**

Add to the `:root { … }` block (alongside the other color tokens, before line 34):
```css
  --diff-add-bg: #eafaef;
  --diff-del-bg: #fdecea;
  --diff-add-fg: #0a7d3c;
  --diff-del-fg: #a3242b;
```
Add to the `:root[data-theme="dark"] { … }` block (before its closing brace, ≈ line 54):
```css
  --diff-add-bg: color-mix(in srgb, var(--success) 22%, var(--surface));
  --diff-del-bg: color-mix(in srgb, var(--error) 22%, var(--surface));
  --diff-add-fg: color-mix(in srgb, var(--success) 90%, white);
  --diff-del-fg: color-mix(in srgb, var(--error) 90%, white);
```

- [ ] **Step 2: Apply the mapping in ReviewPanel**

In `src/review/ReviewPanel.tsx`:
- Diff add lines: bg `#eafaef` → `var(--diff-add-bg)`, fg `#070` → `var(--diff-add-fg)`.
- Diff del lines: bg `#fdecea` → `var(--diff-del-bg)`, fg `#b00` → `var(--diff-del-fg)`.
- Diff context fg `#444` → `var(--text-secondary)`; `<pre>` bg `#f6f6f6` → `var(--surface-soft)`.
- Mandate tag `#06c` (≈ line 176) → `var(--primary)`.
- Risky/error `#b00` (≈ lines 153, 177, 226) → `var(--error)`.
- Remaining `#666` / `#888` → `var(--text-secondary)`.

- [ ] **Step 3: Verify**

Run: `npm run typecheck && npm test`
Expected: PASS.
```bash
grep -nE "#[0-9a-fA-F]{3,6}|: *['\"]white" src/review/ReviewPanel.tsx
```
Expected: no matches (the only hex now lives in the `:root` token definitions in `styles.css`).

- [ ] **Step 4: Manual check (light + dark)** — open Review with a pending proposal, expand the diff preview; added/removed lines must be clearly distinguishable in BOTH themes; open the loud-confirm modal.

- [ ] **Step 5: Commit**

```bash
git add src/styles.css src/review/ReviewPanel.tsx
git commit -m "style(desktop): ReviewPanel diff uses themed diff tokens (dark-mode ready)"
```

---

## Task 7: MandatesPanel (+ unify toggle on ToggleSwitch)

**File:** `src/mandates/MandatesPanel.tsx` (~245 lines, ~27 inline sites). It currently uses a native `<input type="checkbox">` for the enable toggle — unify it with the `ToggleSwitch` used by InboxPanel. Logic stays in the tested `mandateForm.ts`/`mandateView.ts`.

- [ ] **Step 1: Replace the enable checkbox with ToggleSwitch**

Add the import:
```tsx
import { ToggleSwitch } from "../components/ui/ToggleSwitch";
```
Find the enable-mandates `<input type="checkbox">`+label block (card "a", ≈ lines 150–160) and replace it with:
```tsx
<ToggleSwitch
  checked={enabled}
  onChange={(next) => setEnabledAndPersist(next)}
  label="Auto-apply clean mandate writes"
/>
```
Use the panel's existing enable state + setter (named `enabled`/its setter in the current code — keep whatever the current handler is; only the rendering changes). If the current handler is inline (e.g. `onChange={(e)=>...e.target.checked...}`), adapt it to receive the boolean `next` directly.

- [ ] **Step 2: Apply the color mapping to the rest**

Mostly `#666` → `var(--text-secondary)` (secondary text, ≈ lines 149, 205, 212, 213, 233) and `#b00` → `var(--error)` (errors, ≈ lines 157, 195). Raw `<input>`/`<textarea>` in the New-mandate form: drop inline `border`/`padding` overrides to inherit the themed base styles, or map `#ccc` → `var(--border-soft)`.

- [ ] **Step 3: Verify**

Run: `npm run typecheck && npm test`
Expected: PASS (note: `mandateForm.test.ts` / `mandateView.test.ts` exercise the logic and must stay green).
```bash
grep -nE "#[0-9a-fA-F]{3,6}|: *['\"]white" src/mandates/MandatesPanel.tsx
```
Expected: no matches.

- [ ] **Step 4: Manual check (light + dark)** — open Mandates; the enable toggle now matches Inbox's switch; create-form inputs and activity list readable in both themes.

- [ ] **Step 5: Commit**

```bash
git add src/mandates/MandatesPanel.tsx
git commit -m "style(desktop): MandatesPanel uses tokens + unified ToggleSwitch"
```

---

## Task 8: Settings + Sources (+ adopt SettingsSectionCard)

**Files:** `src/settings/AirSettings.tsx` (~38 lines), `src/sources/SourcesPanel.tsx` (~186 lines, rendered inside AirSettings). The orphaned `SettingsSectionCard` (`components/ui/`) is the natural container here.

- [ ] **Step 1: AirSettings**

In `src/settings/AirSettings.tsx`: `#666` (≈ lines 20, 29) → `var(--text-secondary)`; the Danger-zone divider `borderTop: "1px solid #eee"` (≈ line 27) → `1px solid var(--border-soft)`; the Reset button inline `color: "#b00"` (≈ line 32) → replace with the existing `danger-btn` class (`<Button className="danger-btn">` — Button now forwards `className` from plan 1) and drop the inline color.

- [ ] **Step 2: SourcesPanel**

In `src/sources/SourcesPanel.tsx`: divider `#eee` (≈ line 126) → `var(--border-soft)`; `#666`/`#888` (≈ lines 41, 128, 159, 170) → `var(--text-secondary)`/`var(--text-tertiary)`; `#b00` errors (≈ lines 143, 144, 163) → `var(--error)`. Replace the raw per-row `<button>` elements (Allow/Disallow/Revoke) with the token `secondary-btn`/`danger-btn` classes (add `className="secondary-btn"` to the bare buttons) so they theme and match the rest. Optionally wrap the section in `<SettingsSectionCard title="Sources" description="…">` (import from `components/ui/SettingsSectionCard`) instead of the hand-rolled `borderTop` div.

- [ ] **Step 3: Verify**

Run: `npm run typecheck && npm test`
Expected: PASS (`sources/*.test.ts` logic tests stay green).
```bash
grep -rnE "#[0-9a-fA-F]{3,6}|: *['\"]white" src/settings/AirSettings.tsx src/sources/SourcesPanel.tsx
```
Expected: no matches.

- [ ] **Step 4: Manual check (light + dark)** — open Settings; Sources list, grant buttons, Danger-zone reset, and the file `<details>` all readable + themed.

- [ ] **Step 5: Commit**

```bash
git add src/settings/AirSettings.tsx src/sources/SourcesPanel.tsx
git commit -m "style(desktop): Settings + Sources use tokens + ui components"
```

---

## Task 9: Onboarding screens

**Files:** `src/onboarding/Welcome.tsx`, `NameAgent.tsx`, `GenerateAndRegister.tsx`, `Done.tsx`. These render before identity exists (the `.onboarding-wrap` from plan 1). They use `Card`/`Button`/`Input` (already token-based) plus a few inline colors.

- [ ] **Step 1: Apply the mapping**

Grep each file and replace any color literals per the mapping table:
```bash
grep -rnE "#[0-9a-fA-F]{3,6}|: *['\"]white|: *['\"]black" src/onboarding/
```
Replace each hit (typically `#666` → `var(--text-secondary)`, any error `#b00` → `var(--error)`). Layout inline styles stay.

- [ ] **Step 2: Verify**

Run: `npm run typecheck && npm test`
Expected: PASS.
```bash
grep -rnE "#[0-9a-fA-F]{3,6}|: *['\"]white|: *['\"]black" src/onboarding/
```
Expected: no matches.

- [ ] **Step 3: Manual check (light + dark)**

To see onboarding without wiping identity, temporarily render the flow, or verify on a fresh profile. At minimum confirm typecheck + grep clean and the screens use `Card`/`Button` (themed).

- [ ] **Step 4: Commit**

```bash
git add src/onboarding/
git commit -m "style(desktop): onboarding screens use tokens (dark-mode ready)"
```

---

## Task 10: Dark-mode + accessibility sweep + final gates

The consolidated verification gate for the whole redesign.

- [ ] **Step 1: Repo-wide check for stragglers**

```bash
cd /Users/ahnkwangwook/air-note/apps/desktop
grep -rnE "#[0-9a-fA-F]{3,6}|: *['\"]white|: *['\"]black" src --include=*.tsx | grep -v styles.css
```
Expected: **no matches** outside `styles.css` (which legitimately defines the raw token values + light-diff hexes). Fix any straggler with the mapping, committing per the panel it belongs to.

- [ ] **Step 2: Full light + dark walkthrough**

Run `npm run dev` (Tauri shell, or `npm run dev:web`). For EACH view — Identity, Inbox (+ a conversation + AI draft), Memory (run a search), Review (expand a diff + loud modal), Mandates, Settings (+ Sources), and the ⌘K palette — toggle light↔dark and confirm: no hardcoded white/black boxes, readable text contrast, visible borders, sensible selected/hover states.

- [ ] **Step 3: Accessibility pass**

- Sidebar nav items: active item has `aria-current="page"` (plan 1) ✓ — confirm focus rings are visible in both themes.
- Command palette: `role="dialog"` + `aria-modal` (plan 2) ✓ — confirm input autofocus on open, Esc closes, and focus returns to a sensible place after close.
- Theme toggle + search trigger have `aria-label`s (plans 1–2) ✓.
- Tab through each panel: every interactive control is reachable and shows a focus indicator (the base `:focus` token ring in `styles.css`).
- Fix any gaps found, committing to the relevant file.

- [ ] **Step 4: Full gate sweep**

```bash
cd /Users/ahnkwangwook/air-note/apps/desktop
npm test
npm run typecheck
npm run lint
cd /Users/ahnkwangwook/air-note
cargo build -p air_agent_desktop
cargo clippy -p air_agent_desktop -- -D warnings
cargo audit --deny warnings
```
Expected: all PASS.

- [ ] **Step 5: Commit any sweep fixes + push**

```bash
git add -A
git commit -m "style(desktop): dark-mode + a11y polish across all panels"   # only if the sweep changed files
git status -sb
git push
```

- [ ] **Step 6: Open the PR for the whole redesign**

With all three plans landed on `desktop-ui-shell-redesign`, open one PR to `main`:
```bash
gh pr create --base main --head desktop-ui-shell-redesign \
  --title "Desktop UI shell redesign: sidebar + ⌘K global search + dark mode" \
  --body "Implements docs/superpowers/specs/2026-06-24-desktop-ui-shell-redesign-design.md across plans 1–3 (theme+shell, global search, panel restyle). All gates green."
```

---

## Self-Review (completed during authoring)

- **Spec coverage (Sequencing items 5 + 6, Architecture D):** every panel migrated off inline styles to tokens (Tasks 2–9, one panel per task per the spec's "one panel per task keeps PRs reviewable"); `Button`/`Card` already token-driven from plan 1; `components/ui/*` adopted where it fits (`ToggleSwitch` in Mandates, `SettingsSectionCard`/`secondary-btn`/`danger-btn` in Settings/Sources, optional `StatusBadge` in Memory); dark-mode QA + a11y sweep + full gate sweep (Task 10). ✓
- **Dark mode:** the migration rule (every color literal → `var(--token)`) is exactly what makes the existing `:root[data-theme="dark"]` theme take effect everywhere; new diff tokens cover the one place (`ReviewPanel`) where `--success`/`--error` foregrounds weren't enough. ✓
- **Type consistency:** no new types introduced; uses primitives/components from plans 1–2 (`Button` `className`, `ToggleSwitch`, `SettingsSectionCard`, `StatusBadge`) with their existing signatures. ✓
- **No placeholders:** each task gives the concrete literal→token edits (with line refs) for the non-obvious sites, a grep gate to prove completeness, and exact verify/commit commands. The mechanical sites are covered by the shared mapping table + the per-file grep gate rather than re-pasting 1500+ lines of near-identical panel code. ✓
- **Out of scope (deferred, per spec YAGNI):** conversation-history list in the sidebar; icon-rail/3-column; skins beyond light/dark; any engine/behavior change.
