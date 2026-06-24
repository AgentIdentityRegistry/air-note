# Desktop UI Shell Redesign — Plan 1 of 3: Theme Foundation + Shell/Sidebar

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the AIR Agent desktop app's horizontal top-nav shell with a two-column layout — a left vertical sidebar (nav with count badges, light/dark theme toggle, Settings pinned bottom) — and migrate the `Button`/`Card` primitives to the existing design-system tokens with a persisted light/dark theme.

**Architecture:** A new `ThemeProvider` (`src/state/theme.tsx`) sets `:root[data-theme]` and persists to `localStorage`; tokens already in `styles.css` cascade from there. The shell becomes `App.tsx` `Shell()` → `.app-shell` 2-column grid (`<Sidebar/>` + `<main className="main-area">`). The bespoke `InboxNavButton`/`ReviewNavButton` collapse into one generalized `NavItem`, fed by `useInbox().totalUnread` and a new `useReviewCount` hook. We **reuse the orphaned shell CSS already in `styles.css`** (`.app-shell`, `.sidebar`, `.tab-list`, `.tab-btn`, `.sidebar-footer`, `.main-area`) rather than writing new layout CSS.

**Tech Stack:** React 18 + TypeScript, Tauri v2, Vite, Vitest 2 (pure `.test.ts` today; this plan adds jsdom + `@testing-library/react` for component tests), CSS-variable design tokens in `src/styles.css`.

**Scope note:** This is plan 1 of 3 for the cohesive redesign in `docs/superpowers/specs/2026-06-24-desktop-ui-shell-redesign-design.md`. Plan 2 adds the ⌘K global-search command palette (it will add a search trigger to the top of the sidebar built here). Plan 3 restyles every panel's inner content off inline styles. After this plan the app has a working themed sidebar shell; panels still carry their own inline styles until plan 3 (a harmless transient state — there is no release pipeline yet).

**Working directory for all commands:** `/Users/ahnkwangwook/air-note/apps/desktop` (the `@air-agent/desktop` workspace). Run `cd /Users/ahnkwangwook/air-note/apps/desktop` first.

---

## File Structure

**Created:**
- `src/test/setup.ts` — Vitest setup: registers `@testing-library/jest-dom` matchers.
- `src/state/themePref.ts` — pure theme helpers (parse stored value, resolve initial theme).
- `src/state/themePref.test.ts` — pure tests for the above.
- `src/state/theme.tsx` — `ThemeProvider` + `useTheme()`.
- `src/state/theme.test.tsx` — component test (jsdom): toggle flips `data-theme` + persists.
- `src/shell/nav.ts` — `View` union (moved out of `App.tsx`), `MAIN_NAV`, `navBadge()`.
- `src/shell/nav.test.ts` — pure tests for `navBadge`.
- `src/shell/NavItem.tsx` — one nav button (label + optional count badge + active state).
- `src/shell/NavItem.test.tsx` — component test: renders badge, calls `onNavigate`.
- `src/shell/useReviewCount.ts` — hook polling `listProposals().length` for the Review badge.
- `src/shell/Sidebar.tsx` — the left column (brand, nav, footer with theme toggle + Settings).
- `src/shell/Sidebar.test.tsx` — component test: renders nav + badges, calls `onNavigate`.
- `src/components/Button.test.tsx` — component test: variant → class.
- `src/components/Card.test.tsx` — component test: renders `.card`.

**Modified:**
- `package.json` — add dev deps: `@testing-library/react`, `@testing-library/dom`, `@testing-library/jest-dom`, `jsdom`.
- `vitest.config.ts` — include `.test.tsx`, add `setupFiles`.
- `src/components/Button.tsx` — inline styles → token classes (`floating-primary-btn` / `secondary-btn`).
- `src/components/Card.tsx` — inline styles → `.card` token class.
- `src/styles.css` — add `.card`, `.theme-toggle-btn`, `.app-loading`, `.onboarding-wrap`, and a flex tweak to `.tab-btn`.
- `src/App.tsx` — add `ThemeProvider`; rewrite `Shell()` to the 2-column layout using `Sidebar`; delete `InboxNavButton`/`ReviewNavButton`; import `View` from `shell/nav`.

---

## Task 1: Component-test infrastructure

The repo has only pure `.test.ts` in a `node` env. The spec calls for component tests; add jsdom + Testing Library, opted into per-file so existing pure tests keep running in `node`.

**Files:**
- Modify: `package.json`
- Modify: `vitest.config.ts`
- Create: `src/test/setup.ts`
- Create (temporary): `src/test/smoke.test.tsx`

- [ ] **Step 1: Add dev dependencies**

Run:
```bash
cd /Users/ahnkwangwook/air-note/apps/desktop
npm install -D @testing-library/react@^16 @testing-library/dom@^10 @testing-library/jest-dom@^6 jsdom@^25
```
Expected: installs succeed; `package.json` devDependencies now list all four; `package-lock.json` updated.

- [ ] **Step 2: Create the Vitest setup file**

Create `src/test/setup.ts`:
```ts
// Registers @testing-library/jest-dom matchers (toBeInTheDocument, toHaveClass, …) with Vitest's expect.
import "@testing-library/jest-dom/vitest";
```

- [ ] **Step 3: Update vitest config to include `.test.tsx` + the setup file**

Replace the entire contents of `vitest.config.ts` with:
```ts
import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    // Default env stays "node" so the 19 existing pure tests are unchanged.
    // Component tests opt into jsdom with a `// @vitest-environment jsdom` pragma on line 1.
    environment: "node",
    include: ["src/**/*.test.ts", "src/**/*.test.tsx"],
    setupFiles: ["src/test/setup.ts"],
  },
});
```

- [ ] **Step 4: Write a temporary smoke test to prove the jsdom + jest-dom wiring**

Create `src/test/smoke.test.tsx`:
```tsx
// @vitest-environment jsdom
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";

describe("test infra", () => {
  it("renders into jsdom and jest-dom matchers work", () => {
    render(<button>hello</button>);
    expect(screen.getByRole("button", { name: "hello" })).toBeInTheDocument();
  });
});
```

- [ ] **Step 5: Run the whole suite — smoke test plus all existing pure tests pass**

Run: `npm test`
Expected: PASS — the new smoke test passes in jsdom AND all 19 existing pure `.test.ts` files still pass in node.

- [ ] **Step 6: Delete the temporary smoke test**

Run: `rm src/test/smoke.test.tsx`

- [ ] **Step 7: Commit**

```bash
git add package.json package-lock.json vitest.config.ts src/test/setup.ts
git commit -m "test(desktop): add jsdom + testing-library component-test infra"
```

---

## Task 2: Pure theme-preference helpers

Keep the decision logic (what theme to start with) pure and unit-tested; the provider in Task 3 just wires it to React + the DOM.

**Files:**
- Create: `src/state/themePref.ts`
- Test: `src/state/themePref.test.ts`

- [ ] **Step 1: Write the failing test**

Create `src/state/themePref.test.ts`:
```ts
import { describe, it, expect } from "vitest";
import { parseStoredTheme, resolveInitialTheme, THEME_STORAGE_KEY } from "./themePref";

describe("parseStoredTheme", () => {
  it("accepts the two valid values", () => {
    expect(parseStoredTheme("light")).toBe("light");
    expect(parseStoredTheme("dark")).toBe("dark");
  });
  it("returns null for missing or garbage values", () => {
    expect(parseStoredTheme(null)).toBeNull();
    expect(parseStoredTheme("")).toBeNull();
    expect(parseStoredTheme("blue")).toBeNull();
  });
});

describe("resolveInitialTheme", () => {
  it("prefers a stored value over the system preference", () => {
    expect(resolveInitialTheme("light", true)).toBe("light");
    expect(resolveInitialTheme("dark", false)).toBe("dark");
  });
  it("falls back to the system preference when nothing is stored", () => {
    expect(resolveInitialTheme(null, true)).toBe("dark");
    expect(resolveInitialTheme(null, false)).toBe("light");
  });
});

describe("THEME_STORAGE_KEY", () => {
  it("is the namespaced key", () => {
    expect(THEME_STORAGE_KEY).toBe("air.theme");
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm test -- themePref`
Expected: FAIL — "Cannot find module './themePref'".

- [ ] **Step 3: Write the implementation**

Create `src/state/themePref.ts`:
```ts
export type Theme = "light" | "dark";

/** localStorage key holding the user's explicit theme choice. */
export const THEME_STORAGE_KEY = "air.theme";

/** Parse a stored value into a Theme, or null if absent/invalid. */
export function parseStoredTheme(raw: string | null): Theme | null {
  return raw === "light" || raw === "dark" ? raw : null;
}

/** A stored choice always wins; otherwise follow the OS dark-mode preference. */
export function resolveInitialTheme(stored: Theme | null, prefersDark: boolean): Theme {
  return stored ?? (prefersDark ? "dark" : "light");
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npm test -- themePref`
Expected: PASS (3 describe blocks).

- [ ] **Step 5: Commit**

```bash
git add src/state/themePref.ts src/state/themePref.test.ts
git commit -m "feat(desktop): pure theme-preference helpers"
```

---

## Task 3: ThemeProvider + useTheme

**Files:**
- Create: `src/state/theme.tsx`
- Test: `src/state/theme.test.tsx`

- [ ] **Step 1: Write the failing component test**

Create `src/state/theme.test.tsx`:
```tsx
// @vitest-environment jsdom
import { describe, it, expect, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { ThemeProvider, useTheme } from "./theme";
import { THEME_STORAGE_KEY } from "./themePref";

function Probe() {
  const { theme, toggleTheme } = useTheme();
  return <button onClick={toggleTheme}>theme:{theme}</button>;
}

describe("ThemeProvider", () => {
  beforeEach(() => {
    localStorage.clear();
    delete document.documentElement.dataset.theme;
  });

  it("sets data-theme on the root and persists when toggled", () => {
    render(<ThemeProvider><Probe /></ThemeProvider>);
    // matchMedia is undefined in jsdom → guard yields light as the default.
    expect(document.documentElement.dataset.theme).toBe("light");

    fireEvent.click(screen.getByRole("button"));

    expect(screen.getByRole("button").textContent).toBe("theme:dark");
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(localStorage.getItem(THEME_STORAGE_KEY)).toBe("dark");
  });

  it("reads the persisted theme on mount", () => {
    localStorage.setItem(THEME_STORAGE_KEY, "dark");
    render(<ThemeProvider><Probe /></ThemeProvider>);
    expect(document.documentElement.dataset.theme).toBe("dark");
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm test -- theme.test`
Expected: FAIL — "Cannot find module './theme'".

- [ ] **Step 3: Write the implementation**

Create `src/state/theme.tsx`:
```tsx
import { createContext, useContext, useEffect, useState, type ReactNode } from "react";
import { type Theme, THEME_STORAGE_KEY, parseStoredTheme, resolveInitialTheme } from "./themePref";

type ThemeCtx = { theme: Theme; toggleTheme: () => void; setTheme: (t: Theme) => void };

const Ctx = createContext<ThemeCtx | null>(null);

/** Compute the first-paint theme from storage, falling back to the OS preference. */
function initialTheme(): Theme {
  const stored = parseStoredTheme(
    typeof localStorage !== "undefined" ? localStorage.getItem(THEME_STORAGE_KEY) : null,
  );
  const prefersDark =
    typeof matchMedia !== "undefined" && matchMedia("(prefers-color-scheme: dark)").matches;
  return resolveInitialTheme(stored, prefersDark);
}

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [theme, setThemeState] = useState<Theme>(initialTheme);

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    try {
      localStorage.setItem(THEME_STORAGE_KEY, theme);
    } catch {
      /* private mode / storage disabled — theme still applies for this session. */
    }
  }, [theme]);

  const setTheme = (t: Theme) => setThemeState(t);
  const toggleTheme = () => setThemeState((t) => (t === "dark" ? "light" : "dark"));

  return <Ctx.Provider value={{ theme, toggleTheme, setTheme }}>{children}</Ctx.Provider>;
}

export function useTheme(): ThemeCtx {
  const c = useContext(Ctx);
  if (!c) throw new Error("useTheme must be inside ThemeProvider");
  return c;
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npm test -- theme.test`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add src/state/theme.tsx src/state/theme.test.tsx
git commit -m "feat(desktop): ThemeProvider with persisted light/dark theme"
```

> Note: `ThemeProvider` is wired into `App()` in Task 9 (alongside the shell rewrite) so the providers change once.

---

## Task 4: Migrate Card to a token class

**Files:**
- Modify: `src/styles.css` (add `.card`)
- Modify: `src/components/Card.tsx`
- Test: `src/components/Card.test.tsx`

- [ ] **Step 1: Add the `.card` token class to `styles.css`**

Append to `src/styles.css` (after the existing `.surface` block, around line 183):
```css
.card {
  border: 1px solid var(--border-soft);
  border-radius: var(--radius-md);
  background: var(--surface);
  box-shadow: var(--elev-1);
  padding: 16px;
}
```

- [ ] **Step 2: Write the failing test**

Create `src/components/Card.test.tsx`:
```tsx
// @vitest-environment jsdom
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { Card } from "./Card";

describe("Card", () => {
  it("renders children inside a .card container with no inline colors", () => {
    render(<Card><span>body</span></Card>);
    const card = screen.getByText("body").parentElement!;
    expect(card).toHaveClass("card");
    expect(card.getAttribute("style")).toBeNull();
  });
});
```

- [ ] **Step 3: Run test to verify it fails**

Run: `npm test -- Card.test`
Expected: FAIL — the current `Card` renders inline `style` and no `.card` class.

- [ ] **Step 4: Rewrite Card**

Replace the entire contents of `src/components/Card.tsx`:
```tsx
import { ReactNode } from "react";

export function Card({ children }: { children: ReactNode }) {
  return <div className="card">{children}</div>;
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `npm test -- Card.test`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/styles.css src/components/Card.tsx src/components/Card.test.tsx
git commit -m "refactor(desktop): Card uses .card token class"
```

---

## Task 5: Migrate Button to token classes

Reuse the existing `.floating-primary-btn` (primary) and `.secondary-btn` (secondary) classes; the base `button` element already themes via `styles.css`. Keep spreading `...rest` so callers' `onClick`/`disabled`/`style` still work, and merge an optional `className`.

**Files:**
- Modify: `src/components/Button.tsx`
- Test: `src/components/Button.test.tsx`

- [ ] **Step 1: Write the failing test**

Create `src/components/Button.test.tsx`:
```tsx
// @vitest-environment jsdom
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { Button } from "./Button";

describe("Button", () => {
  it("maps the primary variant to the primary token class", () => {
    render(<Button>Save</Button>);
    expect(screen.getByRole("button", { name: "Save" })).toHaveClass("floating-primary-btn");
  });

  it("maps the secondary variant to the secondary token class", () => {
    render(<Button variant="secondary">Cancel</Button>);
    expect(screen.getByRole("button", { name: "Cancel" })).toHaveClass("secondary-btn");
  });

  it("merges an extra className and forwards onClick", () => {
    const onClick = vi.fn();
    render(<Button className="danger-btn" onClick={onClick}>Reset</Button>);
    const btn = screen.getByRole("button", { name: "Reset" });
    expect(btn).toHaveClass("floating-primary-btn");
    expect(btn).toHaveClass("danger-btn");
    fireEvent.click(btn);
    expect(onClick).toHaveBeenCalledOnce();
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm test -- Button.test`
Expected: FAIL — current `Button` has no `className` prop and uses inline styles.

- [ ] **Step 3: Rewrite Button**

Replace the entire contents of `src/components/Button.tsx`:
```tsx
import { ButtonHTMLAttributes } from "react";

export function Button({
  children,
  variant = "primary",
  className,
  ...rest
}: { variant?: "primary" | "secondary" } & ButtonHTMLAttributes<HTMLButtonElement>) {
  const variantClass = variant === "primary" ? "floating-primary-btn" : "secondary-btn";
  return (
    <button {...rest} className={[variantClass, className].filter(Boolean).join(" ")}>
      {children}
    </button>
  );
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npm test -- Button.test`
Expected: PASS (3 tests).

- [ ] **Step 5: Typecheck — confirm no caller relied on the removed prop shape**

Run: `npm run typecheck`
Expected: PASS. (Callers pass `variant`, `onClick`, `disabled`, `children`, and sometimes `style` — all still accepted via `...rest`.)

- [ ] **Step 6: Commit**

```bash
git add src/components/Button.tsx src/components/Button.test.tsx
git commit -m "refactor(desktop): Button uses token classes, accepts className"
```

---

## Task 6: Nav model + NavItem component

Move the `View` union out of `App.tsx` into a shared module (plans 2 + 3 import it), define the main nav list, and build one `NavItem` that subsumes the old `InboxNavButton`/`ReviewNavButton` badge rendering.

**Files:**
- Create: `src/shell/nav.ts`
- Test: `src/shell/nav.test.ts`
- Create: `src/shell/NavItem.tsx`
- Test: `src/shell/NavItem.test.tsx`
- Modify: `src/styles.css` (flex tweak to `.tab-btn`)

- [ ] **Step 1: Write the failing pure test for the nav model**

Create `src/shell/nav.test.ts`:
```ts
import { describe, it, expect } from "vitest";
import { MAIN_NAV, navBadge } from "./nav";

describe("MAIN_NAV", () => {
  it("lists the five primary views in order (Settings is pinned separately)", () => {
    expect(MAIN_NAV.map((n) => n.view)).toEqual([
      "identity", "inbox", "memory", "review", "mandates",
    ]);
  });
});

describe("navBadge", () => {
  it("returns the count as a string when positive", () => {
    expect(navBadge(3)).toBe("3");
  });
  it("returns null for zero or undefined (no badge)", () => {
    expect(navBadge(0)).toBeNull();
    expect(navBadge(undefined)).toBeNull();
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm test -- shell/nav.test`
Expected: FAIL — "Cannot find module './nav'".

- [ ] **Step 3: Write the nav model**

Create `src/shell/nav.ts`:
```ts
/** The set of top-level panels the shell can show. Single source of truth (App, Sidebar, search). */
export type View = "identity" | "inbox" | "memory" | "review" | "mandates" | "settings";

export type NavItemDef = { view: View; label: string };

/** Primary nav, in display order. `settings` is rendered separately (pinned to the footer). */
export const MAIN_NAV: readonly NavItemDef[] = [
  { view: "identity", label: "Identity" },
  { view: "inbox", label: "Inbox" },
  { view: "memory", label: "Memory" },
  { view: "review", label: "Review" },
  { view: "mandates", label: "Mandates" },
] as const;

/** Display text for a nav count badge, or null when there is nothing to show. */
export function navBadge(count: number | undefined): string | null {
  return count && count > 0 ? String(count) : null;
}
```

- [ ] **Step 4: Run the pure test — passes**

Run: `npm test -- shell/nav.test`
Expected: PASS.

- [ ] **Step 5: Add the flex tweak to `.tab-btn` so a label + badge sit on one row**

In `src/styles.css`, find the `.tab-btn` rule (around line 528) and add three properties to it so it reads:
```css
.tab-btn {
  text-align: left;
  border: 1px solid transparent;
  border-radius: 10px;
  color: var(--text-primary);
  background: transparent;
  box-shadow: none;
  font-weight: 520;
  font-size: 0.78rem;
  padding: 7px 10px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
}
```

- [ ] **Step 6: Write the failing NavItem component test**

Create `src/shell/NavItem.test.tsx`:
```tsx
// @vitest-environment jsdom
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { NavItem } from "./NavItem";

describe("NavItem", () => {
  it("renders the label and a badge when count > 0", () => {
    render(<NavItem view="inbox" label="Inbox" count={4} active={false} onNavigate={() => {}} />);
    expect(screen.getByText("Inbox")).toBeInTheDocument();
    expect(screen.getByText("4")).toBeInTheDocument();
  });

  it("renders no badge when count is 0 or undefined", () => {
    render(<NavItem view="memory" label="Memory" active={false} onNavigate={() => {}} />);
    expect(screen.queryByText("0")).not.toBeInTheDocument();
  });

  it("marks the active item and calls onNavigate with its view", () => {
    const onNavigate = vi.fn();
    render(<NavItem view="review" label="Review" active onNavigate={onNavigate} />);
    const btn = screen.getByRole("button", { name: /Review/ });
    expect(btn).toHaveClass("active");
    expect(btn).toHaveAttribute("aria-current", "page");
    fireEvent.click(btn);
    expect(onNavigate).toHaveBeenCalledWith("review");
  });
});
```

- [ ] **Step 7: Run test to verify it fails**

Run: `npm test -- NavItem`
Expected: FAIL — "Cannot find module './NavItem'".

- [ ] **Step 8: Write NavItem**

Create `src/shell/NavItem.tsx`:
```tsx
import { type View, navBadge } from "./nav";
import { StatusBadge } from "../components/ui/StatusBadge";

export function NavItem({
  view,
  label,
  count,
  active,
  onNavigate,
}: {
  view: View;
  label: string;
  count?: number;
  active: boolean;
  onNavigate: (v: View) => void;
}) {
  const badge = navBadge(count);
  return (
    <button
      type="button"
      className={active ? "tab-btn active" : "tab-btn"}
      aria-current={active ? "page" : undefined}
      onClick={() => onNavigate(view)}
    >
      <span>{label}</span>
      {badge ? <StatusBadge tone="primary">{badge}</StatusBadge> : null}
    </button>
  );
}
```

- [ ] **Step 9: Run test to verify it passes**

Run: `npm test -- NavItem`
Expected: PASS (3 tests).

- [ ] **Step 10: Commit**

```bash
git add src/shell/nav.ts src/shell/nav.test.ts src/shell/NavItem.tsx src/shell/NavItem.test.tsx src/styles.css
git commit -m "feat(desktop): shared View/nav model + NavItem with count badge"
```

---

## Task 7: useReviewCount hook

Extract the polled pending-proposal count out of the old `ReviewNavButton` into a reusable hook. The sidebar badge is always visible now, so it polls while identity is present (the old code polled only while the Review tab was active — the always-fresh badge is the intended improvement).

**Files:**
- Create: `src/shell/useReviewCount.ts`

- [ ] **Step 1: Write the hook**

Create `src/shell/useReviewCount.ts`:
```ts
import { useEffect, useState } from "react";
import { listProposals } from "../api/engine";

/** How often the Review nav badge refreshes its pending count. */
const REVIEW_POLL_MS = 5000;

/**
 * Pending-proposal count for the Review nav badge.
 * Polls every REVIEW_POLL_MS while `identityPresent` (no engine exists before onboarding).
 */
export function useReviewCount(identityPresent: boolean): number {
  const [count, setCount] = useState(0);
  useEffect(() => {
    if (!identityPresent) {
      setCount(0);
      return;
    }
    let alive = true;
    const refresh = () => {
      listProposals()
        .then((ps) => { if (alive) setCount(ps.length); })
        .catch(() => { if (alive) setCount(0); });
    };
    refresh();
    const id = setInterval(refresh, REVIEW_POLL_MS);
    return () => { alive = false; clearInterval(id); };
  }, [identityPresent]);
  return count;
}
```

- [ ] **Step 2: Typecheck**

Run: `npm run typecheck`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add src/shell/useReviewCount.ts
git commit -m "feat(desktop): useReviewCount hook for the Review nav badge"
```

---

## Task 8: Sidebar component

**Files:**
- Create: `src/shell/Sidebar.tsx`
- Test: `src/shell/Sidebar.test.tsx`

The sidebar reads the theme from context, so the test wraps it in `ThemeProvider`. (Plan 2 adds a search trigger into the top region; the top is brand-only for now.)

- [ ] **Step 1: Write the failing component test**

Create `src/shell/Sidebar.test.tsx`:
```tsx
// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach } from "vitest";
import { type ComponentProps } from "react";
import { render, screen, fireEvent } from "@testing-library/react";
import { Sidebar } from "./Sidebar";
import { ThemeProvider } from "../state/theme";

function renderSidebar(props: Partial<ComponentProps<typeof Sidebar>> = {}) {
  const onNavigate = vi.fn();
  render(
    <ThemeProvider>
      <Sidebar view="identity" onNavigate={onNavigate} inboxUnread={0} reviewCount={0} {...props} />
    </ThemeProvider>,
  );
  return { onNavigate };
}

describe("Sidebar", () => {
  beforeEach(() => {
    localStorage.clear();
    delete document.documentElement.dataset.theme;
  });

  it("renders the five primary nav items plus Settings", () => {
    renderSidebar();
    for (const label of ["Identity", "Inbox", "Memory", "Review", "Mandates", "Settings"]) {
      expect(screen.getByRole("button", { name: new RegExp(label) })).toBeInTheDocument();
    }
  });

  it("shows the inbox unread + review badges", () => {
    renderSidebar({ inboxUnread: 7, reviewCount: 2 });
    expect(screen.getByText("7")).toBeInTheDocument();
    expect(screen.getByText("2")).toBeInTheDocument();
  });

  it("calls onNavigate when a nav item is clicked", () => {
    const { onNavigate } = renderSidebar();
    fireEvent.click(screen.getByRole("button", { name: /Memory/ }));
    expect(onNavigate).toHaveBeenCalledWith("memory");
  });

  it("toggles the theme from the footer button", () => {
    renderSidebar();
    expect(document.documentElement.dataset.theme).toBe("light");
    fireEvent.click(screen.getByRole("button", { name: /theme/i }));
    expect(document.documentElement.dataset.theme).toBe("dark");
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm test -- Sidebar`
Expected: FAIL — "Cannot find module './Sidebar'".

- [ ] **Step 3: Write Sidebar**

Create `src/shell/Sidebar.tsx`:
```tsx
import { type View, MAIN_NAV } from "./nav";
import { NavItem } from "./NavItem";
import { useTheme } from "../state/theme";

export function Sidebar({
  view,
  onNavigate,
  inboxUnread,
  reviewCount,
}: {
  view: View;
  onNavigate: (v: View) => void;
  inboxUnread: number;
  reviewCount: number;
}) {
  const { theme, toggleTheme } = useTheme();
  const countFor = (v: View): number | undefined =>
    v === "inbox" ? inboxUnread : v === "review" ? reviewCount : undefined;

  return (
    <aside className="sidebar">
      <div className="brand">
        <h1>AIR Agent</h1>
      </div>

      <nav className="tab-list" aria-label="Primary">
        {MAIN_NAV.map((item) => (
          <NavItem
            key={item.view}
            view={item.view}
            label={item.label}
            count={countFor(item.view)}
            active={view === item.view}
            onNavigate={onNavigate}
          />
        ))}
      </nav>

      <div className="sidebar-footer">
        <button
          type="button"
          className="secondary-btn theme-toggle-btn"
          aria-label="Toggle light or dark theme"
          onClick={toggleTheme}
        >
          {theme === "dark" ? "☀ Light" : "☾ Dark"}
        </button>
        <NavItem view="settings" label="Settings" active={view === "settings"} onNavigate={onNavigate} />
      </div>
    </aside>
  );
}
```

- [ ] **Step 4: Add the `.theme-toggle-btn` rule to `styles.css`**

Append to `src/styles.css`:
```css
.theme-toggle-btn {
  width: 100%;
  text-align: left;
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `npm test -- Sidebar`
Expected: PASS (4 tests).

- [ ] **Step 6: Commit**

```bash
git add src/shell/Sidebar.tsx src/shell/Sidebar.test.tsx src/styles.css
git commit -m "feat(desktop): left Sidebar with nav badges + theme toggle"
```

---

## Task 9: Rewrite App Shell to the two-column layout

Wire `ThemeProvider`, swap the horizontal nav for `<Sidebar/>`, delete the bespoke badge buttons, and import `View` from `shell/nav`. The onboarding flow and the panel ternary are behavior-unchanged.

**Files:**
- Modify: `src/App.tsx`
- Modify: `src/styles.css` (add `.app-loading`, `.onboarding-wrap`)

- [ ] **Step 1: Add the two small layout classes to `styles.css`**

Append to `src/styles.css`:
```css
.app-loading {
  padding: 2rem;
  color: var(--text-secondary);
}

.onboarding-wrap {
  max-width: 600px;
  margin: 0 auto;
  padding: 2rem;
}
```

- [ ] **Step 2: Replace the entire contents of `src/App.tsx`**

```tsx
import { useState } from "react";
import { IdentityProvider, useIdentity } from "./state/identity";
import { OnboardingProvider, useOnboarding } from "./state/onboarding";
import { InboxProvider, useInbox } from "./state/inbox";
import { AiLoopProvider } from "./state/aiLoop";
import { ThemeProvider } from "./state/theme";
import { Welcome } from "./onboarding/Welcome";
import { NameAgent } from "./onboarding/NameAgent";
import { GenerateAndRegister } from "./onboarding/GenerateAndRegister";
import { Done } from "./onboarding/Done";
import { IdentityPanel } from "./identity/IdentityPanel";
import { InboxPanel } from "./inbox/InboxPanel";
import { MemoryPanel } from "./memory/MemoryPanel";
import { ReviewPanel } from "./review/ReviewPanel";
import { MandatesPanel } from "./mandates/MandatesPanel";
import { AirSettings } from "./settings/AirSettings";
import { Sidebar } from "./shell/Sidebar";
import { useReviewCount } from "./shell/useReviewCount";
import type { View } from "./shell/nav";

export default function App() {
  return (
    <ThemeProvider>
      <IdentityProvider>
        <OnboardingProvider>
          <InboxProvider>
            <AiLoopProvider>
              <Shell />
            </AiLoopProvider>
          </InboxProvider>
        </OnboardingProvider>
      </IdentityProvider>
    </ThemeProvider>
  );
}

function Shell() {
  const { identity, loading } = useIdentity();
  const { totalUnread } = useInbox();
  const reviewCount = useReviewCount(!!identity);
  const [view, setView] = useState<View>("identity");
  const [onboardingDone, setOnboardingDone] = useState(false);

  if (loading) return <div className="app-loading">Loading…</div>;
  if (!identity && !onboardingDone) {
    return (
      <div className="onboarding-wrap">
        <OnboardingFlow onDone={() => setOnboardingDone(true)} />
      </div>
    );
  }

  return (
    <div className="app-shell">
      <Sidebar view={view} onNavigate={setView} inboxUnread={totalUnread} reviewCount={reviewCount} />
      <main className="main-area">
        {view === "identity" ? (
          <IdentityPanel />
        ) : view === "inbox" ? (
          <InboxPanel />
        ) : view === "memory" ? (
          <MemoryPanel />
        ) : view === "review" ? (
          <ReviewPanel />
        ) : view === "mandates" ? (
          <MandatesPanel />
        ) : (
          <AirSettings />
        )}
      </main>
    </div>
  );
}

function OnboardingFlow({ onDone }: { onDone: () => void }) {
  const { state } = useOnboarding();
  switch (state.step) {
    case "welcome":
      return <Welcome />;
    case "name":
      return <NameAgent />;
    case "generating":
    case "registering":
      return <GenerateAndRegister />;
    case "done":
      return <Done onFinish={onDone} />;
  }
}
```

- [ ] **Step 3: Typecheck**

Run: `npm run typecheck`
Expected: PASS — `View` now comes from `shell/nav`; `InboxNavButton`/`ReviewNavButton` are gone; `listProposals` is no longer imported here (it moved into `useReviewCount`).

- [ ] **Step 4: Run the full suite**

Run: `npm test`
Expected: PASS — all pure + component tests.

- [ ] **Step 5: Manual smoke check**

Run: `npm run dev:web` and open the printed URL (or `npm run dev` for the Tauri shell).
Verify by observation:
- Left sidebar shows Identity / Inbox / Memory / Review / Mandates, with Settings + the theme toggle pinned at the bottom.
- Clicking items swaps the main panel; the active item shows the left-accent bar.
- The theme toggle flips the whole app light↔dark and survives a reload.
- Inbox/Review badges appear when there are unread messages / pending proposals.

Stop the dev server when done.

- [ ] **Step 6: Commit**

```bash
git add src/App.tsx src/styles.css
git commit -m "feat(desktop): two-column shell with left Sidebar + ThemeProvider"
```

---

## Task 10: Plan-1 gate sweep

Run every gate this repo enforces, so plan 1 lands green.

- [ ] **Step 1: Frontend gates**

```bash
cd /Users/ahnkwangwook/air-note/apps/desktop
npm test
npm run typecheck
npm run lint
```
Expected: all PASS. Fix any eslint findings (e.g. unused imports) before continuing.

- [ ] **Step 2: Rust gates (unchanged by this plan, but must stay green)**

```bash
cd /Users/ahnkwangwook/air-note
cargo build -p air_agent_desktop
cargo clippy -p air_agent_desktop -- -D warnings
cargo audit --deny warnings
```
Expected: all PASS. (No Rust files changed in this plan; this confirms the workspace is still green.)

- [ ] **Step 3: Confirm the branch is clean and pushed**

```bash
git status -sb
git push
```
Expected: working tree clean; branch `desktop-ui-shell-redesign` pushed to origin.

---

## Self-Review (completed during authoring)

- **Spec coverage (Sequencing items 1 + 2):** ThemeProvider + Button/Card→tokens (Task 2–5); 2-column shell + Sidebar + `NavItem` subsuming the badge buttons + theme toggle (Task 6–9). ✓
- **Reuse-first:** Sidebar/shell reuse the orphaned `.app-shell`/`.sidebar`/`.tab-list`/`.tab-btn`/`.sidebar-footer`/`.main-area` classes already in `styles.css`; only `.card`, `.theme-toggle-btn`, `.app-loading`, `.onboarding-wrap`, and a flex tweak to `.tab-btn` are new CSS. ✓
- **Type consistency:** `View` is defined once in `shell/nav.ts` and imported by `App.tsx` (and by plans 2 + 3). `Theme` is defined once in `themePref.ts` and reused by `theme.tsx`. `navBadge`/`MAIN_NAV` names are used identically across Task 6, 8. ✓
- **No placeholders:** every code + test step contains complete code and exact run/expected lines. ✓
- **Out of scope (correctly deferred):** the search trigger button + command palette (plan 2); panel inner-content restyle (plan 3).
