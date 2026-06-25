# AIR Agent — Desktop Design System

> **Purpose.** This is the single, authoritative reference for the AIR Agent desktop app's *current* design system, derived entirely from the source code (no invention — every value is copied verbatim from the code). Feed it to Claude Design (or any design tool) so generated wireframes and mockups land perfectly on-brand with the real app.
>
> **Source of truth.** All values come from `apps/desktop/src/styles.css` (≈2032 lines) plus the React components in `apps/desktop/src/components/`, `components/ui/`, `shell/`, `search/`, and the feature panels. Where the code is ambiguous, this doc describes exactly what the code does rather than guessing (see [§10 Known quirks](#10-known-quirks--ambiguities)).

---

## Table of contents

1. [Overview & brand voice](#1-overview--brand-voice)
2. [Design tokens — full reference](#2-design-tokens--full-reference)
3. [Typography scale](#3-typography-scale)
4. [Spacing, radius, elevation, motion](#4-spacing-radius-elevation-motion)
5. [Component catalog](#5-component-catalog)
6. [Layout patterns](#6-layout-patterns)
7. [Interaction & copy conventions](#7-interaction--copy-conventions)
8. [Theming & accessibility](#8-theming--accessibility)
9. [Quick recipes for new screens](#9-quick-recipes-for-new-screens)
10. [Known quirks & ambiguities](#10-known-quirks--ambiguities)
11. [Appendix — raw token source](#11-appendix--raw-token-source)

---

## 1. Overview & brand voice

**What the app is.** AIR Agent is a **local-first, privacy-first** AI agent + personal-memory hub + secure messaging desktop app, built with **Tauri 2 + React + TypeScript**. Everything ("everything stays on your machine") is processed locally — memory search, the local "Evolve" model, file ingestion — and that privacy promise is reflected in calm, quiet, content-first visuals.

**The three top-level products (left-nav tabs):**

| Nav label | Internal `view` | What it is |
|-----------|-----------------|------------|
| **AIR** | `identity` | The Agent Identity Registry panel — your agent's name, DID, trust score. |
| **AIR Note** | `inbox` | Secure agent-to-agent messaging ("Inbox"), with an AI reply dial. |
| **Brain** | `memory` | A hub with sub-tabs: **Search & Evolve**, **Review**, **Mandates**. |
| **Settings** | `settings` | Pinned to the sidebar footer as an icon, not a primary tab. |

**Aesthetic.** Calm, modern, minimal, content-first. Near-flat surfaces, hairline borders (`rgba` at 6–7% opacity), tiny soft shadows, generous-but-tight spacing, a single restrained blue accent, and lots of neutral grays. No gradients (except a barely-there optional skin), no heavy chrome, no loud color. It reads like a focused productivity tool (think Linear / ChatGPT / Claude desktop), not a flashy consumer app.

**Design principles (as encoded in the code):**

- **Left-nav shell.** A fixed `228px` vertical sidebar (brand → primary nav → footer icons) + a scrolling main area. Collapsible to a `76px` rail.
- **⌘K search-first.** A prominent main-screen search bar that opens a full command-palette overlay; `⌘K` (and `Ctrl+K`) opens the same overlay. One search surface, two entry points. Searches across **memory, conversations, files**.
- **Token-driven.** Every color, radius, shadow, duration, and font size is a CSS custom property on `:root`. Components reference tokens, never hard-coded values (repo invariant: zero hardcoded hex in `.tsx`). Restyling = swap tokens.
- **Light/dark via `data-theme`.** A single `data-theme="dark"` attribute on `<html>` flips the whole palette. Choice persists in `localStorage` under `air.theme`; first paint falls back to OS `prefers-color-scheme`.
- **Plain language + Title-Case labels.** Copy explains things like you'd explain to a friend ("Everything stays on your machine", "keep this file of mine up to date from these folders"). Action buttons use **Title Case** ("New Message", "Ingest Now", "Create Mandate", "Allow All Edits", "Turn Evolve On").
- **Icon affordances.** Footer controls (Settings gear, light/dark toggle) are **icon-only buttons** with `aria-label` + `title`. 16px inline SVG icons, `stroke="currentColor"`.

---

## 2. Design tokens — full reference

All tokens are CSS custom properties. **Light** values live on `:root`; **dark** overrides live on `:root[data-theme="dark"]`. Two optional *skins* (`[data-skin="minimal_dark"]`, `[data-skin="jrpg"]`) override a subset; see [§2.6](#26-skins).

> Notation: a value of *"(inherits light)"* means the dark block does **not** redefine that token, so the light value is used in both themes.

### 2.1 Color — backgrounds & surfaces

| Token | Light | Dark | Use for… |
|-------|-------|------|----------|
| `--bg` | `#f3f4f6` | `#0d1016` | App background (`body`, behind everything). |
| `--surface` | `#ffffff` | `#131824` | Cards, panels, inputs, buttons, popovers — the default raised surface. |
| `--surface-soft` | `#eef0f3` | `#171e2c` | Subtle fills: toggle track bg, stat cards, code boxes, sidebar tint, "soft" zones. |
| `--border-soft` | `rgba(17, 24, 39, 0.06)` | `rgba(255, 255, 255, 0.07)` | The default hairline border on nearly every element. |

### 2.2 Color — brand / primary / accent

| Token | Light | Dark | Use for… |
|-------|-------|------|----------|
| `--primary` | `#2f6bff` | `#4b82ff` | Primary actions, selected states, focus rings, active-nav accent, user message bubble tint, links. |
| `--accent` | `#2f6bff` | `#4b82ff` | Secondary emphasis (currently identical to `--primary`); drives the `glow-executing` ring. |
| `--on-primary` | `#ffffff` | `(inherits light)` → `#ffffff` | Text/icon color placed **on** a `--primary` fill (readable-on-primary). Used by message bubbles & the AI dial's active segment. |

> Note: floating primary buttons hard-code `#fff` for their label rather than `--on-primary` (see [§10](#10-known-quirks--ambiguities)).

### 2.3 Color — semantic status

| Token | Light | Dark | Use for… |
|-------|-------|------|----------|
| `--success` | `#4d8f6f` | `#6e9f86` | Success pills/badges, "ready" status dots, verified `✓`, completed glow. |
| `--warning` | `#a57c42` | `#aa8a55` | Warnings, "attention" dots, "unverified"/"spam" badges, approval glow, palette error text. |
| `--error` | `#a75d61` | `#b57a7d` | Errors, error banners, danger buttons, "key changed", error glow, validation text. |

These three are intentionally **muted/desaturated** (sage green, ochre, dusty rose) rather than pure traffic-light colors — part of the calm aesthetic.

### 2.4 Color — text

| Token | Light | Dark | Use for… |
|-------|-------|------|----------|
| `--text-primary` | `#0b0f17` | `rgba(255, 255, 255, 0.92)` | Body copy, headings, primary labels. Also set as the root `color`. |
| `--text-secondary` | `rgba(11, 15, 23, 0.62)` | `rgba(255, 255, 255, 0.62)` | Secondary/muted copy (`.muted`), captions, helper text, sub-labels. |
| `--text-tertiary` | `rgba(11, 15, 23, 0.42)` | `rgba(255, 255, 255, 0.42)` | Faintest text: placeholders, the `⌕`/`⌘K` hints, empty-chat text, `Loading` label. |

### 2.5 Color — diff (Review preview / change cards)

| Token | Light | Dark | Use for… |
|-------|-------|------|----------|
| `--diff-add-bg` | `#eafaef` | `color-mix(in srgb, var(--success) 22%, var(--surface))` | Background of added lines in a diff. |
| `--diff-del-bg` | `#fdecea` | `color-mix(in srgb, var(--error) 22%, var(--surface))` | Background of removed lines in a diff. |
| `--diff-add-fg` | `#0a7d3c` | `color-mix(in srgb, var(--success) 90%, white)` | Text color of added lines. |
| `--diff-del-fg` | `#a3242b` | `color-mix(in srgb, var(--error) 90%, white)` | Text color of removed lines. |

### 2.6 Elevation (shadows)

| Token | Light | Dark | Use for… |
|-------|-------|------|----------|
| `--elev-1` | `0 1px 2px rgba(11, 15, 23, 0.05)` | `0 1px 2px rgba(0, 0, 0, 0.28)` | Resting cards & panels — barely-there lift. |
| `--elev-2` | `0 8px 20px rgba(11, 15, 23, 0.12)` | `0 8px 20px rgba(0, 0, 0, 0.32)` | Hovered cards, modals, login card, toast, slide-panel. |
| `--elev-3` | `0 10px 24px rgba(11, 15, 23, 0.14)` | `0 10px 24px rgba(0, 0, 0, 0.36)` | The command palette (highest standard surface). |

### 2.7 Radius

| Token | Value (both themes) | Use for… |
|-------|---------------------|----------|
| `--radius-sm` | `8px` | Buttons, inputs, pills/badges, icon buttons, tabs, palette items. |
| `--radius-md` | `10px` | Cards, panels, surfaces, the main-search bar, stat cards. |
| `--radius-lg` | `12px` | Login card, modal card, command palette, large containers. |

Radius is **not** themed (same in light & dark) but **is** overridden by the `jrpg` skin for chat/avatar shapes only.

### 2.8 Motion

| Token | Value | Use for… |
|-------|-------|----------|
| `--motion-fast` | `150ms` | Hover/focus transitions on buttons, inputs, message bubbles. |
| `--motion-normal` | `220ms` | Card hover lift, toggle thumb/track, slide-panel entrance, message fade-in. |
| `--motion-slow` | `320ms` | Reserved for slower transitions (defined; sparsely used). |
| `--motion-easing` | `cubic-bezier(0.4, 0, 0.2, 1)` | The single easing curve for *all* transitions & keyframes (standard "ease-in-out"). |

### 2.9 Typography tokens

| Token | Value | Use for… |
|-------|-------|----------|
| `--font-title` | `0.98rem` | Panel `h2` titles. |
| `--font-section` | `0.82rem` | Panel `h3` section headers. |
| `--font-body` | `0.74rem` | Base body font size (set on `body`). |
| `--font-label` | `0.68rem` | Form labels, nav-list headers, group labels, compact captions. |
| `--line-height-title` | `1.18` | Titles/headings. |
| `--line-height-body` | `1.26` | Body text (set on `body`). |

**Font stack** (set on `:root`, inherited everywhere):

```
-apple-system, BlinkMacSystemFont, "SF Pro Display", "SF Pro Text", "Segoe UI", Inter, system-ui, sans-serif
```

i.e. **system-native first** (San Francisco on macOS, Segoe UI on Windows), Inter as a web fallback. No custom/loaded web fonts. Monospace is used ad-hoc (`font-family: monospace` / `"inherit"`) for DIDs, file paths, and code.

### 2.10 Chat & avatar tokens

| Token | Light/base | `jrpg` skin | Use for… |
|-------|------------|-------------|----------|
| `--avatar-radius` | `999px` (circle) | `6px` | Avatar/glow-ring inner shape. |
| `--chat-user-radius` | `12px` | `8px` | User (sent) message bubble corner radius. |
| `--chat-assistant-radius` | `12px` | `4px` | Assistant/mission/received bubble corner radius. |
| `--chat-column-max` | `840px` | — | Max width of the centered chat column **and** the main-search bar. |

### 2.11 Skins

Skins are an *additional*, optional layer applied via a `data-skin` attribute on `<html>` (independent of `data-theme`). Only two exist in code:

- **`[data-skin="minimal_dark"]`** — re-declares the dark palette tokens (`--bg`, `--surface`, `--surface-soft`, `--border-soft`, `--text-*`, `--primary`, `--accent`, and the four `--diff-*`) to the same values as `data-theme="dark"`. Effectively "force dark regardless of `data-theme`." Sets `body { background: var(--bg) }`.
- **`[data-skin="jrpg"]`** — a playful "game UI" skin. Only overrides **shape** tokens (`--avatar-radius: 6px`, `--chat-user-radius: 8px`, `--chat-assistant-radius: 4px`), gives `body` a faint diagonal blue hatch pattern, thickens entity-card / settings-card / chat-message borders to `2px`, and adds a 1px blue ring to cards.

> These skins are defined in CSS but there is **no UI control** in the reviewed code that sets `data-skin` (theme.tsx only sets `data-theme`). Treat skins as an available theming hook, not a shipped user setting.

---

## 3. Typography scale

Type is **small and dense** by design (base body is `0.74rem` ≈ 11.8px at a 16px root). Hierarchy comes from size + weight + color, not large type.

| Role | Selector / usage | Size | Weight | Line-height | Notes |
|------|------------------|------|--------|-------------|-------|
| App brand | `.brand h1` | `1.14rem` | `620` | — | `letter-spacing: -0.02em`. "AIR Agent" in the sidebar. |
| Login title | `.login-card h1` | `1.5rem` | `700` | `1.18` (title) | The largest type in the app. |
| Big number | `.big-number` | `1.24rem` | `560` | — | `letter-spacing: -0.03em`. Stat values. |
| Panel title | `.panel h2` | `--font-title` `0.98rem` | `560` | `1.18` | `letter-spacing: -0.01em`. |
| Panel section | `.panel h3` | `--font-section` `0.82rem` | `540` | `1.18` | `letter-spacing: -0.01em`. |
| Settings title | `.settings-panel h2` | `0.94rem` | `600` | — | |
| Body (base) | `body` | `--font-body` `0.74rem` | `400` (inherit) | `1.26` | The global default. |
| Tab / nav label | `.tab-btn`, `.tab-inline` | `0.78rem` | `520` | — | |
| Form label | `label` | `--font-label` `0.68rem` | `500` | — | Grid layout, `gap: 8px`. |
| Pill / badge | `.status-badge`, `.pill` | `0.66rem` | `500` | — | `text-transform: capitalize`. |
| Palette group label | `.command-palette-group-label` | `--font-label` `0.68rem` | `600` | — | `text-transform: uppercase`, `letter-spacing: 0.04em`. |

**Heading conventions.** Inside a `.panel` / `.card`, headings are `h2` (title) → `h3` (section) → `h4` (sub-section, e.g. "Added"/"Removed"/"Changed" in a change card). Headings carry `margin: 0` and negative letter-spacing. Many panels set `<h2 style={{ margin: 0 }}>` inline. Section sub-headers are often just a `<div style={{ fontWeight: 600 }}>` label (e.g. "New Mandate", "Active Mandates", "Danger Zone", "Sources") rather than a semantic heading.

**Weight vocabulary.** The app uses a fine-grained, mostly *sub-bold* weight scale: `500` (labels, pills, check rows), `520` (tabs, nav), `540`/`560` (headings, palette titles), `600` (section labels, badges, table headers), `610`/`620` (brand, mission-card title), `700` (login title, avatar label). True `700` bold is rare and reserved.

---

## 4. Spacing, radius, elevation, motion

**Spacing.** There is **no named spacing scale token**; spacing is expressed directly in `px` (and occasionally `rem`). The de-facto scale observed across the codebase:

```
2 · 4 · 6 · 8 · 10 · 12 · 14 · 16 · 18 · 20 · 24
```

Conventions:
- **Card / panel padding:** `14px` (`.panel`), `16px` (`.card`), `12px` (compact cards: `.entity-card`, `.stat-card`, `.session-box`, `.run-item`-ish lists), `18px` (`.modal-card`).
- **Vertical rhythm inside panels:** `gap: 12px` (`.panel`, `.main-area`), `gap: 8–10px` for tight stacks, `20px` between settings sections (`.settings-stack`).
- **Main area padding:** `12px 14px`.
- **Sidebar padding:** `10px 10px 12px`; nav rows `7px 10px`.
- **Button padding:** `8px 12px` (default), `9px 14px` (floating primary), `5px 10px` (inline tab), `0` (icon button, fixed 34×34).
- **Inputs:** `8px 10px`.

**Radius / elevation / motion** — see tokens in [§2.7–2.8](#27-radius). Rules of thumb:
- Pills/badges use a one-off `7px` radius (not a token).
- Fully-round (`999px`) is used for: avatars, toggle track/thumb, status dots, message badges, mission-indicator pill, glow ring.
- Resting cards get `--elev-1`; **hover** raises `.entity-card` to `--elev-2` **and** `translateY(-1px)` over `--motion-normal`. This subtle lift-on-hover is the app's signature card interaction.

---

## 5. Component catalog

Each entry lists **purpose · variants · states · the exact classes/tokens**. React component files are under `apps/desktop/src/components/`, `components/ui/`, `shell/`, `search/`.

### 5.1 Button (`components/Button.tsx`)

A thin wrapper: `<Button variant="primary" | "secondary">`. Maps to a CSS class:

- **primary** → `.floating-primary-btn`
- **secondary** → `.secondary-btn`

There is also the **base `button` element style** (used by raw `<button>`s without a variant class, e.g. mandate "Remove", "Undo"), the **`.danger-btn`** modifier, **`.link-btn`** (text link), and **`.icon-btn`** (icon-only). All button variants share `border-radius: var(--radius-sm)`, `font-weight: 500`, and the fast tri-property transition (border/background/opacity over `--motion-fast`).

| Variant | Class | Default | Hover | Disabled |
|---------|-------|---------|-------|----------|
| **Base** | `button` | `bg: --surface`, `border: --border-soft`, `color: --text-primary`, `padding: 8px 12px` | `bg` lightens toward `--surface-soft` (72% mix), border darkens (text-secondary 24% mix) | `opacity: 0.6`, `cursor: not-allowed`, no shadow/transform |
| **Primary** | `.floating-primary-btn` | `bg: --primary`, `color: #fff`, `border: transparent`, `padding: 9px 14px`, `letter-spacing: 0.01em` | `bg: mix(--primary 86%, #000)` (darken) | inherits base disabled |
| **Secondary** | `.secondary-btn` | `bg: --surface`, `color: --text-primary`, `border: --border-soft` (all `!important`) | border (text-secondary 20% mix), `bg` (surface-soft 78% mix) | inherits |
| **Danger** | `.danger-btn` | `color: --error`, `border: mix(--error 34%, --border-soft)`, `bg: --surface` | `color: --error`, border (error 46% mix), `bg` (surface-soft 66% mix) | inherits |
| **Link** | `.link-btn` | `color: --primary`, no border/bg/shadow/padding, `text-decoration: underline`, `width: fit-content` | — | — |

`.danger-btn` is commonly composed onto a secondary button: `<Button variant="secondary" className="danger-btn">Reset agent</Button>`.

### 5.2 FloatingPrimaryButton (`components/ui/FloatingPrimaryButton.tsx`)

Same visual as `Button variant="primary"` — it simply renders `<button className="floating-primary-btn">`. Despite the name, the class has **no fixed/absolute positioning and no box-shadow** (`box-shadow: none !important`); "floating" is legacy naming. Use it for the dominant call-to-action on a screen.

### 5.3 Card (`components/Card.tsx`) & Surface (`components/ui/Surface.tsx`)

- **`Card`** → `<div className="card">`. Resting container: `bg: --surface`, `border: --border-soft`, `border-radius: --radius-md` (10px), `box-shadow: --elev-1`, `padding: 16px`. The workhorse content container — almost every panel wraps its body in one or more `Card`s.
- **`Surface`** → `<section class="surface surface-{level}">`, polymorphic via `as` prop, `elevation` ∈ `level1|level2|level3` (default `level2`). `.surface` = `bg --surface` + `border --border-soft` + `radius --radius-md`; `.surface-level{1,2,3}` add `--elev-{1,2,3}`. Use when you need an elevation other than the card default, or a non-`div` element.
- Related card flavors: **`.entity-card`** (list item card, hover-lifts to `--elev-2` + `translateY(-1px)`), **`.stat-card`** (3-up stats, `bg` = surface↔surface-soft mix), **`.session-box`**, **`.diagnostics-box`**, **`.modal-card`**, **`.login-card`**.

### 5.4 Input (`components/Input.tsx`)

Thin wrapper that just forces `width: 100%`; all visuals come from the base `input, select, textarea` rule:
- **Default:** `bg: --surface`, `border: 1px --border-soft`, `border-radius: --radius-sm`, `padding: 8px 10px`, `color: --text-primary`, `outline: none`.
- **Focus:** `border-color: mix(--primary 45%, transparent)` **+ a focus ring** `box-shadow: 0 0 0 3px mix(--primary 16%, transparent)`. This blue 3px halo is the app's standard focus affordance for text fields.
- Transitions border/box-shadow/background over `--motion-fast`.
- `label` is a CSS grid (`gap: 8px`, `font-size: --font-label`, `font-weight: 500`) so a `<label>` wrapping `<span>` + `<input>` stacks them automatically.

### 5.5 Loading (`components/Loading.tsx`)

Minimal text indicator: `<div style={{ color: var(--text-tertiary), fontStyle: italic }}>{label}</div>`, default label `"Working..."`. There is **no spinner component**; loading is communicated by faint italic text ("Loading…", "Searching…", "Ingesting…", "Loading preview…"). Button-level loading swaps the label (e.g. `Search` → `Searching…`) and disables the button.

### 5.6 StatusBadge & `.pill` (`components/ui/StatusBadge.tsx`)

`<StatusBadge tone="…">` → `<span class="status-badge status-badge-compact status-{tone}">`. Tones: **`neutral | primary | accent | success | warning | error`** (default `neutral`).

Shared shape (`.status-badge, .pill`): `inline-flex` centered, `border-radius: 7px`, `border: mix(--border-soft 92%)`, `padding: 2px 6px`, `font-size: 0.66rem`, `font-weight: 500`, `text-transform: capitalize`, `letter-spacing: 0.01em`.

| Tone | Class | Color | Background |
|------|-------|-------|------------|
| neutral | `.status-neutral` | `--text-secondary` | `mix(--surface-soft 92%)` |
| primary | `.status-primary` (also `.pill.active`) | `--text-primary` | `mix(--surface-soft 82%, --surface)` |
| accent | `.status-accent` | `--text-primary` | `mix(--surface-soft 72%, --surface)` |
| success | `.status-success` | `mix(--success 76%, #143d27)` | `mix(--success 22%, --surface)` |
| warning | `.status-warning` (also `.pill.medium`) | `mix(--warning 80%, #5f3b00)` | `mix(--warning 20%, --surface)` |
| error | `.status-error` (also `.pill.inactive`, `.pill.high`) | `mix(--error 80%, #4a1020)` | `mix(--error 18%, --surface)` |

`.pill` is the same base shape with severity aliases (`.active`, `.medium`, `.high`, `.inactive`) for priority/severity displays. Used for nav count badges (`tone="primary"`), message verification badges, and inline status.

> `status-badge-compact` is applied by the component but has **no CSS rule** — it's an inert hook (see [§10](#10-known-quirks--ambiguities)).

### 5.7 ToggleSwitch (`components/ui/ToggleSwitch.tsx`)

An accessible switch: `<button role="switch" aria-checked>` wrapped in a `<label class="toggle-wrap">` with optional leading `<span class="toggle-label">`. Renders a track, a thumb, and a text label that reads `On`/`Off` (overridable via `onLabel`/`offLabel`).

- **Container `.toggle-switch`:** pill (`border-radius: 999px`), `min-width: 144px`, `padding: 6px 10px`, `bg: --surface-soft`, `border: --border-soft`.
- **Track `.toggle-track`:** `38×22px`, `999px`, off = `mix(--text-secondary 30%, transparent)`.
- **Thumb `.toggle-thumb`:** `16×16px` white circle, `box-shadow: 0 4px 10px rgba(0,0,0,0.14)`, starts at `left: 12px`.
- **On state (`.toggle-switch.on`):** track → `mix(--primary 70%, transparent)`, thumb → `translateX(16px)`. Animated over `--motion-normal`.
- **Text `.toggle-text`:** `0.78rem`, `font-weight: 600`.
- `disabled` supported. Label text is often a full sentence ("Mandates are on — when on, your agent keeps each file in sync…").

### 5.8 GlowRing (`components/ui/GlowRing.tsx`)

An avatar/status halo: `<span class="glow-ring glow-{state}"><span class="glow-ring-inner">{children}</span></span>`. States: **`idle | planning | executing | approval | error | completed`** (default `idle`).

- **Inner `.glow-ring-inner`:** `30×30px`, `border-radius: var(--avatar-radius)` (circle by default), `bg: --surface`, `border: --border-soft`, centered content.
- Each non-idle state adds a colored `box-shadow: 0 0 0 4px mix(<color> X%, transparent)` **plus a pulsing keyframe animation**:
  - `planning` → `--primary`, `pulse-primary 2.6s infinite`
  - `executing` → `--accent`, `pulse-accent 2s infinite`
  - `approval` → `--warning`, `pulse-warning 2.2s infinite`
  - `error` → `--error`, `pulse-error 2.2s infinite`
  - `completed` → `--success`, `pulse-completed 1.2s ease-out 2` (plays twice, expands & fades)
  - `idle` → no shadow, no animation.
- Pulse keyframes oscillate the ring spread between `4px@X%` and `8px@(X/2)%`.

### 5.9 SettingsSectionCard (`components/ui/SettingsSectionCard.tsx`)

A **borderless** section block for settings (not a raised card): `title` (`h3`) + optional `description` (`.muted`) + optional `actions` slot, with a divider under the head and a body grid.

- `.settings-section-card`: `border: none`, `bg: transparent`, `box-shadow: none`, `padding: 4px 0`, grid `gap: 8px`.
- `.settings-section-card-head`: flex space-between, `border-bottom: 1px mix(--border-soft 76%)`, `padding-bottom: 8px`.
- `.settings-section-card-body`: grid `gap: 10px`, `padding-top: 6px`.
- Stack multiple inside `.settings-stack` (grid `gap: 20px`).

### 5.10 SlidePanel (`components/ui/SlidePanel.tsx`)

A right-side drawer/dialog. `<div class="slide-panel-backdrop"><aside class="slide-panel" role="dialog" aria-modal aria-label>`. Closes on backdrop `mousedown` and on **Escape** (keydown listener). Stops propagation inside.

- **Backdrop `.slide-panel-backdrop`:** `position: fixed; inset: 0`, `bg: rgba(10,14,22,0.14)`, `backdrop-filter: blur(2px)`, `justify-content: flex-end`, `padding: 16px`, `z-index: 70`.
- **Panel `.slide-panel`:** `width: min(760px, 100%)`, `height: calc(100vh - 32px)`, slides in from the right via `@keyframes slide-in` (opacity 0→1, `translateX(24px)→0`) over `--motion-normal`.
- At `≤1080px` the panel widens to `min(900px, 100%)`.

### 5.11 ChangeCard (`components/ChangeCard.tsx`)

Renders a config-change proposal as an `.entity-card`: `h3` summary + `.muted` ("This change will be logged and can be undone.") + up to three `h4` sections ("Added" / "Removed" / "Changed") each as a `.simple-list`, then a `.row-actions` footer with **Edit** / **Cancel** (secondary) + **Apply** (base button; label becomes "Apply (FSD)" for `applyMode === "fsd"`).

### 5.12 NavItem + count badge (`shell/NavItem.tsx`)

A primary-nav row: `<button class="tab-btn [active]">` with a `<span>` label and an optional trailing `<StatusBadge tone="primary">` count.

- **`.tab-btn`:** left-aligned flex space-between, `border-radius: 10px`, transparent border/bg, `font-weight: 520`, `font-size: 0.78rem`, `padding: 7px 10px`, `color: --text-primary`.
- **Hover:** `bg: mix(--surface 62%, transparent)`.
- **Active (`.tab-btn.active`):** `bg: mix(--primary 8%, --surface)` + a left accent bar `box-shadow: inset 2px 0 0 --primary`; carries `aria-current="page"`.
- **Count badge** only shows when count > 0 (`navBadge()`), as a primary-tone StatusBadge. Counts surface on **AIR Note** (unread) and **Brain** (review needs-attention).

### 5.13 Sub-tab `.tab-inline` (Brain hub) (`memory/BrainPanel.tsx`)

Pill-less inline tabs used inside the Brain hub ("Search & Evolve" / "Review" / "Mandates") and chat sub-tabs (`.chat-subtabs`, flex-wrap, `gap: 6px`).

- **`.tab-inline`:** `inline-flex` (`gap: 6px`), transparent border, `border-radius: --radius-sm`, `color: --text-secondary`, `font-size: 0.78rem`, `font-weight: 520`, `padding: 5px 10px`.
- **Hover:** `color: --text-primary`, faint bg.
- **Active (`.tab-inline.active`):** subtle border (`mix(--border-soft 90%)`), `bg: mix(--surface-soft 70%, --surface)`, `color: --text-primary`.
- The Review sub-tab appends a primary StatusBadge with the needs-attention count.

> A separate **settings tab** style (`.settings-tab`, used in `.settings-tabs`) renders as an **underline tab**: no border/bg, `border-bottom` goes from transparent → `mix(--primary 72%)` when `.active`, `color` secondary → primary.

### 5.14 Icon button `.icon-btn` (`shell/Sidebar.tsx`)

Square icon-only control (footer Settings gear + theme toggle): `inline-grid` centered, fixed **34×34px**, `padding: 0`, transparent, `color: --text-secondary`.
- **Hover:** `color: --text-primary`, faint bg, faint border.
- **Active (`.icon-btn.active`):** `bg: mix(--primary 8%, --surface)`, left accent bar `inset 2px 0 0 --primary` (matches `.tab-btn.active`).
- Always paired with `aria-label` + `title`; icons are 16px inline `<svg stroke="currentColor" stroke-width="2">` (Feather-style: gear, sun, moon).

### 5.15 MainSearch bar (`shell/MainSearch.tsx`)

The prominent, ChatGPT/Claude-style search trigger pinned at the top of the main area. It's a **button**, not a real input — clicking (or `⌘K`) opens the CommandPalette.

- **`.main-search`:** centered, `width: min(var(--chat-column-max), 100%)` (max 840px), flex row (`gap: 10px`), `border: --border-soft`, `border-radius: --radius-md`, `bg: --surface`, `box-shadow: --elev-1`, `color: --text-secondary`, `padding: 12px 14px`, left-aligned.
- **Hover:** border (text-secondary 22% mix) + faint surface-soft bg.
- Contents: a leading `⌕` glyph (`.main-search-icon`, `--text-tertiary`), placeholder text "Search memory, conversations, files…" (`.main-search-placeholder`, `0.82rem`, flex-1), and a trailing `⌘K` key hint (`.main-search-kbd`: `0.7rem`, `--text-tertiary`, bordered `6px` chip). `aria-label="Search memory, conversations, and files"`.

### 5.16 Command palette (`search/CommandPalette.tsx`)

A centered modal overlay (rendered via `createPortal` to `document.body`) for global search. See [§6.4](#64-command-palette-overlay) for the full layout. Key classes: `.command-palette-backdrop`, `.command-palette`, `.command-palette-input`, `.command-palette-results`, `.command-palette-group-label`, `.command-palette-item[.selected]`, `.command-palette-item-title`, `.command-palette-item-snippet`, `.command-palette-empty`, `.command-palette-error`.

### 5.17 AI dial (`inbox/DialControl.tsx`)

A 3-segment inline toggle for AI-reply autonomy: **`off` · `draft` · `auto`**. Rendered inline (not a token-class component): an `inline-flex` row with `border: 1px --border-soft`, `border-radius: 6px`, `overflow: hidden`; each segment is a borderless button. The **active** segment uses `bg: --primary` + `color: --on-primary`; inactive segments use `bg: --surface` + `color: --text-primary`. Prefixed by an `AI:` label in `--text-secondary`.

### 5.18 Other notable patterns

- **Stat grid:** `.stats-grid` (3 equal cols, collapses to 1 at ≤1080px) of `.stat-card`s, each with an `h3` and a `.big-number`.
- **Diff block (Review):** a `<pre>` with `bg: --surface-soft`, per-line color/background from the `--diff-*` tokens (`- `/`+ ` prefixes).
- **Toast:** `.app-toast` — fixed bottom-right, primary-tinted surface, `--elev-2`, `z-index: 90`.
- **Modal:** `.modal-backdrop` (`rgba(10,14,22,0.24)`, centered, `z-index: 80`) + `.modal-card` (`min(580px,100%)`, `--radius-lg`, `--elev-2`).
- **Usage table:** `.usage-table` — bordered, rounded, header row `bg: mix(--surface-soft 82%)`.

---

## 6. Layout patterns

### 6.1 App shell — the 2-column grid

`.app-shell` is the root: `height: 100vh`, `overflow: hidden`, `display: grid`, **`grid-template-columns: 228px 1fr`** (sidebar + main), `gap: 0`.

- **Rail-collapsed variant:** `.app-shell.rail-collapsed` → `grid-template-columns: 76px 1fr`.
- **Responsive (`≤1080px`):** collapses to a single column (`1fr`), `height: auto`, `min-height: 100vh`, `overflow: visible` — the sidebar stacks on top and the page scrolls normally.

### 6.2 Sidebar (`.sidebar`)

`display: grid; grid-template-rows: auto 1fr auto` (brand → nav → footer), `gap: 14px`, `padding: 10px 10px 12px`, `bg: mix(--surface-soft 44%, --surface)`, `border-right: 1px --border-soft`.

Three zones (top→bottom):
1. **Brand** (`.brand`): `<h1>AIR Agent</h1>` (`1.14rem`, weight `620`, tight tracking).
2. **Primary nav** (`.tab-list`, grid `gap: 4px`): the three NavItems (AIR / AIR Note / Brain). At `≤1080px` it becomes a 3-column grid.
3. **Footer icons** (`.sidebar-footer-icons`, flex `gap: 8px`): Settings gear (`.icon-btn`, active when on Settings) + light/dark toggle (`.icon-btn`, swaps Sun/Moon icon).

> There is also an **`.agent-rail`** family of styles (collapsible agent list, `.rail-agent-btn`, `.mission-state-dot`, `.status-dot.*`) present in CSS for a richer multi-agent rail; the currently-wired `Sidebar.tsx` uses the simpler `.sidebar` + `.tab-list` structure.

### 6.3 Main area (`.main-area`) & panels

- **`.main-area`:** `position: relative`, `padding: 12px 14px`, `display: grid; gap: 12px; align-content: start`, `overflow-y: auto` (this column scrolls). The `MainSearch` bar is the first child, then the active panel.
- **Inbox variant (`.main-area-fill`):** switches to `display: flex; flex-direction: column; overflow: hidden` so AIR Note can own its own internal scrolling instead of scrolling the page.
- **`.panel`:** `border: --border-soft`, `border-radius: --radius-md`, `bg: --surface`, `padding: 14px`, grid `gap: 12px`, `box-shadow: --elev-1`. Panel headings: `h2` (title) / `h3` (section). `.section-header` is a flex space-between row for a title + actions.
- **`.card`** is the simpler content container (16px padding) used by most feature panels.

### 6.4 AIR Note chat layout (viewport-fit, 3 zones)

AIR Note (`inbox/InboxPanel.tsx`) is a **fixed-height, internally-scrolling** layout (`.card.inbox-root`) so it never scrolls the page:

1. **Header (`.inbox-header`, fixed):** `<h2>Inbox</h2>` + a "Show spam" ToggleSwitch + a "New Message" secondary button.
2. **Banners (`.inbox-banner`, fixed):** optional adoption notice, an **offline banner** (error-tinted: `bg: mix(--error 10%, --surface)`, `color: --error`), and an **archive-error banner** (warning-tinted).
3. **Grid (`.inbox-grid`, flex-1):** **`grid-template-columns: 220px 1fr`** — a fixed-width **conversation list** (`.inbox-list-col`, its own `overflow-y: auto`) + a **thread column** (`.inbox-thread-col`, flex column):
   - **Thread head (`.inbox-thread-head`, fixed):** peer name + the AI dial.
   - **Scrolling messages (`.inbox-thread-scroll`, flex-1, `overflow-y: auto`):** the message thread + AI drafts panel + a bottom anchor that's auto-scrolled into view on new messages.
   - **Composer (pinned at the bottom):** recipient field (new convos only) + message input + Send.
- At `≤1080px` `.inbox-grid` collapses to a single column.

> A second, richer chat layout exists in CSS (`.chat-panel` / `.chat-shell` / `.chat-scroll-area` / `.chat-column` / `.chat-input-bar`) built around a centered `--chat-column-max` (840px) column with a pinned composer (`.chat-input-bar`: `border-top`, `padding: 12px 0 16px`) and a "jump to latest" row (`.chat-jump-row`). Message bubbles there use `.chat-message.user` vs `.chat-message.assistant`/`.mission-update`. See [§7.3](#73-message-bubbles).

### 6.5 Command palette overlay

- **Backdrop `.command-palette-backdrop`:** `fixed inset:0`, `bg: rgba(10,14,22,0.32)`, `backdrop-filter: blur(2px)`, content top-aligned with `padding-top: 12vh`, **`z-index: 100`** (highest in the app).
- **Panel `.command-palette`:** `width: min(640px, 92vw)`, `border: --border-soft`, `border-radius: --radius-lg`, `bg: --surface`, `box-shadow: --elev-3`, `overflow: hidden`, grid `rows: auto 1fr`.
- **Input `.command-palette-input`:** borderless except a bottom divider, `padding: 14px 16px`, `font-size: 0.95rem`, transparent bg, no focus ring.
- **Results `.command-palette-results`:** `max-height: 50vh`, scrolls, grid `gap: 2px`, `padding: 6px`.
- **Grouped results:** three fixed groups — **Memory**, **Conversations**, **Files** — each preceded by a `.command-palette-group-label` (uppercase, tracked, tertiary). A failed group shows `.command-palette-error` ("Couldn't search …", warning color).
- **Item `.command-palette-item`:** left-aligned, transparent, `border-radius: --radius-sm`, `padding: 8px 10px`, two stacked lines — `.command-palette-item-title` (`0.82rem`, weight `540`) + `.command-palette-item-snippet` (single-line ellipsis, secondary).
- **Selected `.command-palette-item.selected`:** `bg: mix(--primary 10%, --surface)`, `border: mix(--primary 22%)`. Keyboard: ↑/↓ move selection, Enter navigates, Esc closes.
- **Empty states:** `.command-palette-empty` — "Type to search across memory, conversations, and files." or "No results for "…"."

### 6.6 Responsive behavior (`@media (max-width: 1080px)`)

The single breakpoint. At ≤1080px: the shell stacks to one column and scrolls the page; the sidebar moves to a `border-bottom` and its nav becomes a 3-col grid; `.inbox-grid`, `.stats-grid`, `.runs-layout`, `.skills-layout`, `.policy-grid`, `.tool-grid`, and `.appearance-grid` all collapse to a single column; the slide-panel widens.

---

## 7. Interaction & copy conventions

### 7.1 Voice & tone

- **Plain language, privacy-forward.** Copy reassures and explains in everyday terms: "Everything stays on your machine", "Search everything the agent has read and learned", "A mandate is a standing rule: 'keep this file of mine up to date from these folders.'", "it never rewrites the file on its own."
- **Title Case for actions.** Buttons: "New Message", "Add Folder", "Ingest Now", "Allow All Edits", "Create Mandate", "Turn Evolve On/Off", "Evolve Now", "Turn Learning On", "Apply Anyway", "Reset agent". (Note: a few are sentence case, e.g. "Reset agent", "Pick File…", "Pick Folder…".)
- **Ellipsis for in-progress / pickers.** "Searching…", "Ingesting…", "Evolving…", "Loading preview…", "Pick File…", "checking…", "ready (…)".
- **Curly typography.** The UI uses real curly quotes/apostrophes and em/en dashes ("can't", "—", "·") throughout.

### 7.2 Empty & error states

- **Empty states** are quiet single lines in `--text-secondary`/`--text-tertiary`: "No messages.", "Nothing found yet — try a different search.", "No mandates yet. Create one above to keep a file in sync.", "No changes to review.", "Select a conversation, or start a new message."
- **Error banners** use semantic tints, not solid fills:
  - `.error-banner`: `border: mix(--error 30%)`, `bg: mix(--error 10%, --surface)`, `color: mix(--error 78%, #4a1020)`, `radius: 12px`, weight `600`.
  - `.error-text`: `color: --error`, `0.9rem`, weight `600`.
  - Inline errors are typically `<p style={{ color: var(--error), fontSize: 13 }}>`.
  - The AIR Note offline/archive banners build their tints inline from `--error`/`--warning` via `color-mix`.

### 7.3 Message bubbles (user vs assistant)

Two parallel implementations exist; both follow the same convention:

- **AIR Note thread (`inbox/MessageThread.tsx`):** each message is `max-width: 80%`, `border-radius: 10px`, `padding: 8px 12px`, `font-size: 14px`.
  - **Mine / sent (user):** `align-self: flex-end`, `bg: --primary`, `color: --on-primary`.
  - **Theirs / received:** `align-self: flex-start`, `bg: --surface-soft`, `color: --text-primary`.
  - **Pending:** `opacity: 0.6`.
- **Chat-message CSS (`.chat-message`):** `border: --border-soft`, `padding: 9px 11px`, `max-width: 82%`.
  - **`.user`:** `justify-self: end`, `border-radius: --chat-user-radius`, `bg: mix(--primary 10%, --surface)`, `border: mix(--primary 18%)` (a *tinted* bubble, lighter than the solid-primary thread bubble).
  - **`.assistant`:** `justify-self: start`, `border-radius: --chat-assistant-radius`, `bg: --surface`; its `<p>` fades in via `@keyframes message-fade-in`.
  - **`.mission-update`:** a system/agent note — soft surface bg, muted border, tighter padding.
  - **`.chat-message-badge`:** a pill (`999px`) header label on a message (e.g. message type), secondary color.

### 7.4 Verification, lock & status badges

Message badges (`inbox/badges.ts` → rendered as StatusBadges) mirror the CLI vocabulary:
- **`🔒`** (encrypted) → `neutral`.
- **`✓`** (verified) → `success`; otherwise **`unverified`** → `warning`.
- **`⚠ key changed`** → `error`.
- **`spam`** → `warning`.
- Transient: **`sending…`** → `neutral`, failure reason → `error` (with a "Retry" button when retryable).

In Review, risk is flagged inline: "· from a mandate" (`--primary`), "⚠ needs careful review" (`--error`), "enabled ✓".

### 7.5 The AI dial (off / draft / auto)

Autonomy for AI replies is a 3-state dial (see [§5.17](#517-ai-dial-inboxdialcontroltsx)): **off** (no AI), **draft** (AI drafts, you approve), **auto** (AI sends). Drafts surface in the AI panel as rows with **Approve & Send / Edit / Discard** actions. When no reply model is configured the panel shows a neutral "AI replies — configure a reply model to enable AI drafts".

### 7.6 The "always-loud" confirm (destructive edits)

Editing a file the agent learned from requires an explicit acknowledgement. The Review confirm card (`.modal`-like Card) shows a red "Confirm This Edit" heading, an explanation, a required **"I've reviewed this"** checkbox, then **Apply Anyway** (disabled until checked) / **Cancel**. Identity reset uses a native `confirm()` ("Delete this agent identity? This cannot be undone.") plus a `.danger-btn` in a "Danger Zone" block.

---

## 8. Theming & accessibility

### 8.1 Light / dark mechanism

- Theme is the string `"light" | "dark"` (`state/themePref.ts`). `ThemeProvider` (`state/theme.tsx`) writes it to **`document.documentElement.dataset.theme`** (i.e. `<html data-theme="dark">`) and persists it to `localStorage["air.theme"]`.
- **First paint:** a stored choice always wins; otherwise it follows the OS via `matchMedia("(prefers-color-scheme: dark)")` (`resolveInitialTheme`).
- **Toggle:** the sidebar moon/sun `.icon-btn` calls `toggleTheme()` (flips light↔dark). No "system/auto" tri-state in the UI — just light↔dark, seeded from OS once.
- **Skins** (`data-skin`, [§2.11](#211-skins)) are a separate, additive layer (no shipped control).

### 8.2 Accessibility patterns in code

- **Icon-only controls carry text alternatives:** every `.icon-btn` and icon-only action has both `aria-label` and `title` (Settings, theme toggle, Rename ✏️). Inline SVGs are `aria-hidden focusable="false"`.
- **Nav uses landmarks + current:** `<nav aria-label="Primary">`, sub-nav `aria-label="Brain sections"`; active items set `aria-current="page"`.
- **Switches are real switches:** ToggleSwitch and the AI panel use `role="switch"` + `aria-checked`; the toggle button is keyboard-operable.
- **Dialogs:** CommandPalette and SlidePanel set `role="dialog"` + `aria-modal="true"` + `aria-label`, close on Escape and on backdrop click. (Focus is moved into the palette input on open; note focus-trap is a documented follow-up, not yet implemented.)
- **Focus rings:** text inputs get the `0 0 0 3px mix(--primary 16%)` halo on `:focus`. Buttons rely on the browser default focus outline (no custom `:focus-visible` ring defined in CSS).
- **Readable-on-primary:** `--on-primary` exists specifically so text/icons on a `--primary` fill stay legible (used by message bubbles and the AI dial's active segment). Dark mode lightens `--primary` (#2f6bff → #4b82ff) so it stays vivid on dark surfaces.

### 8.3 Contrast intent

- Text uses a 3-step opacity ramp (`92%/62%/42%` of the base ink) so secondary/tertiary text is dimmer but still token-controlled — keep important copy at `--text-primary`.
- Semantic colors are *muted*; for badges/banners the system pairs a low-alpha colored **background** (`~10–22%` mix into the surface) with a **darkened/strengthened** foreground (e.g. `mix(--success 76%, #143d27)`) to preserve contrast on both themes. Mirror this pattern (tinted bg + strengthened fg) for any new status surface rather than using the raw token as a fill.

---

## 9. Quick recipes for new screens

For a generated mockup to look native, follow these defaults:

- **Page chrome:** put it inside `.app-shell` (228px sidebar + scrolling `.main-area`). Top of the main area = the `.main-search` bar (840px max, centered) unless it's AIR Note.
- **Containers:** wrap content in `.card` (16px pad, `--elev-1`, 10px radius). Group settings with borderless `SettingsSectionCard`s in a `.settings-stack` (20px gaps).
- **Headings:** `h2` for the panel title (`0.98rem`/`560`), `h3` for sections (`0.82rem`/`540`), `<div style="font-weight:600">` for minor labels. Negative letter-spacing on headings.
- **Body text:** small (`0.74rem` base / `13px` inline is common), `--text-secondary` for helper copy.
- **Primary action:** one `floating-primary-btn` (blue) per view; everything else `secondary-btn`. Destructive → `danger-btn`. Title Case labels.
- **Inputs:** standard token input with the blue focus halo; labels above via the `label` grid.
- **Status:** use `StatusBadge`/`.pill` tones, not custom colors. Toggles via `ToggleSwitch`. Empty/error/loading = quiet text in secondary/tertiary/error tokens (no spinners).
- **Spacing:** 8/12/16px are the safe defaults; 12px gaps inside panels.
- **Color discipline:** only ever reference tokens. One blue accent. Muted semantics. Hairline borders. Tiny shadows. Card-hover = `translateY(-1px)` + `--elev-2`.
- **Don't:** introduce gradients, drop shadows heavier than `--elev-3`, pure-saturated reds/greens, large display type, or new accent hues.

---

## 10. Known quirks & ambiguities

Flagged for review (the doc describes what the code actually does; it does not "fix" these):

1. **`--danger` is undefined.** `.chat-delete-btn` (styles.css:1396) uses `color: color-mix(in srgb, var(--text-primary) 80%, var(--danger))`, but **no `:root` block defines `--danger`** anywhere in `src/`. The whole `color-mix` resolves to an invalid value, so the rule is effectively dropped (the element keeps its inherited color). Almost certainly a typo for `--error`. Low impact (`.chat-delete-btn` is for an unwired chat header), but worth a one-line fix.
2. **`status-badge-compact` has no CSS.** `StatusBadge.tsx` always adds the class `status-badge-compact`, but there is no `.status-badge-compact` rule in styles.css — it's an inert hook (no visual effect). Either dead code or a placeholder for a compact variant.
3. **Primary button label color is hard-coded.** `.floating-primary-btn` sets `color: #fff !important` rather than `var(--on-primary)`. Functionally identical today (both are white) but it bypasses the token; if `--on-primary` ever changes, primary buttons won't follow.
4. **Two chat systems coexist.** The shipped AIR Note thread is the inline-styled `MessageThread`/`InboxPanel` (solid-primary bubbles, 220px list). A second, more polished token-based chat layout (`.chat-*` classes, tinted `.chat-message.user`, 840px centered column, pinned `.chat-input-bar`, "jump to latest") is fully styled in CSS but not the one InboxPanel renders. If you're designing the chat screen, decide which is canonical — they differ in bubble treatment and column width.
5. **`.agent-rail` vs `.sidebar`.** A richer collapsible agent rail (with per-agent status dots, mission state, overflow menus) is fully styled but the live `Sidebar.tsx` uses the simpler brand→nav→footer `.sidebar`. The `rail-collapsed` (76px) shell variant pairs with the rail, not the current sidebar.
6. **Skins have no UI.** `[data-skin="minimal_dark"]` and `[data-skin="jrpg"]` are defined but nothing in the reviewed code sets `data-skin`. They're a latent theming capability, not a user-facing setting.
7. **Inbox/AI-loop styling is largely inline.** Several feature areas (InboxPanel banners, MessageThread bubbles, DialControl, AIPanel, MemoryPanel search row, MandatesPanel forms) style with inline `style={{…}}` referencing tokens rather than CSS classes. Values there (e.g. `borderRadius: 6/8/10`, `fontSize: 12/13/14`) are consistent with, but occasionally finer-grained than, the token scale.

---

## 11. Appendix — raw token source

Verbatim from `apps/desktop/src/styles.css` for exactness.

### `:root` (light, base)

```css
:root {
  --bg: #f3f4f6;
  --surface: #ffffff;
  --surface-soft: #eef0f3;
  --border-soft: rgba(17, 24, 39, 0.06);
  --primary: #2f6bff;
  --accent: #2f6bff;
  --on-primary: #ffffff;
  --success: #4d8f6f;
  --warning: #a57c42;
  --error: #a75d61;
  --text-primary: #0b0f17;
  --text-secondary: rgba(11, 15, 23, 0.62);
  --text-tertiary: rgba(11, 15, 23, 0.42);
  --diff-add-bg: #eafaef;
  --diff-del-bg: #fdecea;
  --diff-add-fg: #0a7d3c;
  --diff-del-fg: #a3242b;
  --elev-1: 0 1px 2px rgba(11, 15, 23, 0.05);
  --elev-2: 0 8px 20px rgba(11, 15, 23, 0.12);
  --elev-3: 0 10px 24px rgba(11, 15, 23, 0.14);
  --radius-sm: 8px;
  --radius-md: 10px;
  --radius-lg: 12px;
  --motion-fast: 150ms;
  --motion-normal: 220ms;
  --motion-slow: 320ms;
  --motion-easing: cubic-bezier(0.4, 0, 0.2, 1);
  --font-title: 0.98rem;
  --font-section: 0.82rem;
  --font-body: 0.74rem;
  --font-label: 0.68rem;
  --line-height-title: 1.18;
  --line-height-body: 1.26;
  --avatar-radius: 999px;
  --chat-user-radius: 12px;
  --chat-assistant-radius: 12px;
  --chat-column-max: 840px;
  color: var(--text-primary);
  font-family: -apple-system, BlinkMacSystemFont, "SF Pro Display", "SF Pro Text", "Segoe UI", Inter, system-ui, sans-serif;
}
```

### `:root[data-theme="dark"]` (dark overrides)

```css
:root[data-theme="dark"] {
  --bg: #0d1016;
  --surface: #131824;
  --surface-soft: #171e2c;
  --border-soft: rgba(255, 255, 255, 0.07);
  --primary: #4b82ff;
  --accent: #4b82ff;
  --success: #6e9f86;
  --warning: #aa8a55;
  --error: #b57a7d;
  --text-primary: rgba(255, 255, 255, 0.92);
  --text-secondary: rgba(255, 255, 255, 0.62);
  --text-tertiary: rgba(255, 255, 255, 0.42);
  --elev-1: 0 1px 2px rgba(0, 0, 0, 0.28);
  --elev-2: 0 8px 20px rgba(0, 0, 0, 0.32);
  --elev-3: 0 10px 24px rgba(0, 0, 0, 0.36);
  --diff-add-bg: color-mix(in srgb, var(--success) 22%, var(--surface));
  --diff-del-bg: color-mix(in srgb, var(--error) 22%, var(--surface));
  --diff-add-fg: color-mix(in srgb, var(--success) 90%, white);
  --diff-del-fg: color-mix(in srgb, var(--error) 90%, white);
}
```

### Skins (verbatim)

```css
:root[data-skin="minimal_dark"] {
  --bg: #0d1016;
  --surface: #131824;
  --surface-soft: #171e2c;
  --border-soft: rgba(255, 255, 255, 0.07);
  --text-primary: rgba(255, 255, 255, 0.92);
  --text-secondary: rgba(255, 255, 255, 0.62);
  --text-tertiary: rgba(255, 255, 255, 0.42);
  --primary: #4b82ff;
  --accent: #4b82ff;
  --diff-add-bg: color-mix(in srgb, var(--success) 22%, var(--surface));
  --diff-del-bg: color-mix(in srgb, var(--error) 22%, var(--surface));
  --diff-add-fg: color-mix(in srgb, var(--success) 90%, white);
  --diff-del-fg: color-mix(in srgb, var(--error) 90%, white);
}

:root[data-skin="jrpg"] {
  --avatar-radius: 6px;
  --chat-user-radius: 8px;
  --chat-assistant-radius: 4px;
}
```

---

*Generated from the AIR Agent desktop source (`apps/desktop/`). Every value above is copied from the code; nothing is invented. When the code and this doc disagree, the code wins — regenerate this file.*
