# Desktop UX Redesign — tray-first presence + the reading room — Design

**Status:** Rev 1 — owner-approved conversationally 2026-07-22 (every decision below carries Peter's
explicit pick from the visual brainstorm; mockups preserved in `2026-07-22-ux-mockups/`). Awaiting
independent review (architect + critic) and file-level owner review before per-sub-project planning.
**Build queue (owner decision 2026-07-22): design now, BUILD AFTER Rung 5 SP-V1** — nothing here
starts before the Rung-5 export side lands (which itself gates on the R4-A dogfood verdict after Sun
2026-07-27). Sub-project plans are written when their build slot arrives, never earlier (plans staled
against a moving codebase are a known failure class).
**North Star anchor:** `air/vision-background-first-claude-code-native-2026-07-14` (Peter, verbatim
posture): AIR Agent should RUN quietly and be reachable FROM Claude Code — a background presence, not
a destination app; desktop = settings/consent surface; de-prioritize destination UI. This redesign
makes that posture literal: the primary presence becomes a menu-bar item, the window becomes an
optional reading room, and seven legacy destination rooms retire.
**Trigger (owner, 2026-07-22):** "Make the app a whole lot simpler… a menu-bar icon with important
stuff… when open, more like Obsidian — click something and see what it saved. Right now it's a hot
mess of a page that continuously lists things… we need to work on UI and UX." Plus two hard critiques
folded as laws: no jargon ("Dossiers? Rung5? … should be clean") and modern materials ("make it
liquid glass — your design is so 90s Windows").

---

## §0 Goal + posture

Replace the accumulated multi-room desktop app with **three surfaces** — a menu-bar presence, a
two-pane reading room, and one Settings page — governed by two laws (§1). The daemon, the memory
model, consents, and every background loop are UNTOUCHED: this is a frontend transformation with a
thin set of new read-only wire ops. A wrong UI decision here costs pixels, never memories.

## §1 The two laws (apply to every screen, enforced in CI)

- **L1 — Jargon-free copy.** No internal vocabulary ever reaches a user-facing string. Banned (and
  their replacements): dossier → *topic page/summary*; reflect/reflection → *night tidy-up*; capture
  → *remembering*; evolve → (invisible — it's just "your summaries stay fresh"); library → *your
  memory*; retire/supersede → *forget / correct*; rung/SP-* → never shown; conflict → *"you saved two
  versions of this"*. Enforcement: a **jargon linter** — a CI test that scans every user-facing
  string (the string table, §5) against the banned list; a build with a banned word fails. The
  redesign ships a copy audit of ALL existing user-facing strings (Settings consent copy included —
  e.g. today's "Reflect on recently-missed topics" checkbox becomes plain).
- **L2 — Liquid Glass materials.** macOS vibrancy (translucent, blurred, layered materials; rounded
  radii; SF type) via Tauri's window-effects API over the existing CSS token system. A new
  `materials` token set joins the tokens file; the house rule stays absolute: **zero hardcoded colors
  in any component** (existing repo-wide grep gate continues). Light/dark follow the system; the
  constellation (§4) is dark-native by design. Approved visual reference: the committed mockups
  (`2026-07-22-ux-mockups/` — tray dropdown, window layouts, topic-page views).

## §2 Surface 1 — the menu-bar presence (owner pick: "B with recent-saves")

A template (monochrome) cut of the gold-planet icon lives in the macOS menu bar whenever the APP
process runs (the app owns the tray; the window is optional). Clicking opens one glass panel:

1. Heartbeat line, three states: `🟢 On — remembering as you work` (daemon reachable + remembering
   on); `⏸ Paused — not remembering right now` (daemon reachable, remembering off/paused);
   `🌙 Asleep — open AIR Agent to wake it` (daemon unreachable). Honest, never alarming.
2. Vitals line: memory count · last night's tidy report in plain words ("Tidied 2 topics overnight").
   Sourced from the same digest the engine already renders — re-worded per L1, integers only.
3. Search field → results open the reading room pre-filtered.
4. Switch chips: *Remember* ✓ · *Night tidy* ✓ · *⏸ Pause 1h*. The chips flip ONLY already-consented
   flags (the same App-only ops the Settings toggles call today); a first-ever enable still routes
   through the full blunt consent modal — the dropdown never becomes a consent bypass. *Pause 1h* =
   a UI-side timed disable+re-enable of remembering (no new engine semantics; plan verifies the
   cleanest mapping).
5. Recent-saves line (plain-English titles of the last few remembered items).
6. One button: **Browse your memory** → opens/focuses the window.

Window-optional lifecycle: closing the window keeps the tray and daemon (already true operationally —
becomes designed and stated); *Quit AIR Agent* from the tray menu is the real quit. Whether the Dock
icon hides while the window is closed (accessory activation policy) is a plan-stage decision with a
noted trade-off (accessory mode hides from ⌘Tab).

## §3 Surface 2 — the reading room (owner picks: "A two-pane" + "reading first")

**Two panes, classic-Obsidian shape.** Left sidebar (glass): search pinned on top; then *Topics*
(machine-grown), *Notes* (explicit "remember this" items), *Conversations* (captured sessions grouped
by project), *Files* (ingested). Right: the page for whatever is selected.

**The topic page — reading view (default).** The rule that kills list-itis: **the summary IS the
page; memories are footnotes.** Layout top-to-bottom: title + plain meta line ("34 memories · last
updated last night") → the living summary (the engine's existing topic page, which the night tidy-up
already keeps fresh) → *This week* group (individual memory cards, capped, "▾ N more" expander) →
older time groups (*July*, *Earlier*) COLLAPSED to a header+count until clicked. Invariant: the
default render is one calm screen regardless of memory count — no unbounded scroll, ever. Memory
cards: plain origin tag (*conversation / note / file*), date, snippet; opening a card shows full
content + the two honest actions (*correct this* → supersede; *forget this* → the existing tombstone
flow with its confirmation). A topic with an open disagreement shows ONE gentle card at the top
("You saved two versions of this — review") that opens the existing resolution choices re-worded per
L1; never a queue surface.

**Notes / Conversations / Files pages** reuse the same shape (grouped, collapsed, capped) — they are
list-like by nature but obey the same never-unbounded rule.

**Empty states are designed:** a fresh brain shows "Your first topics will grow here as you work —
nothing to do"; an empty search says "Nothing yet — I'll try to fill this gap overnight" (the
recall-miss → night-repair loop, speaking plainly; shown only when night tidy-up is on).

## §4 The constellation (owner pick: reading-first, ✦ one click away)

A `☰ Read / ✦ Constellation` toggle sits in the topic-page toolbar. The constellation renders the
topic as a gold sun with its memories orbiting — node size = recency+connectedness, links drawn from
REAL engine data (topic-page citations; shared entities between memories), neighbor topics faded at
the edges (click to travel). Zoomed fully out (or from the sidebar's *Galaxy* entry): all topics as
constellations — the "giant globe." Canvas-rendered force layout with a hard performance budget
(node/edge caps with clustering beyond the cap — exact numbers are a plan-stage measurement, and the
budget is a TESTED bound, not an aspiration). Honesty note carried from the brainstorm: unlike
Obsidian's graph, every line here corresponds to a real stored relationship; no decorative edges.

## §5 Surface 3 — Settings, and the retirement

**Settings (one page):** Identity (registry card), Connections (Claude Code connect/disconnect —
the existing SP2 machinery unchanged), the consent switches with plain-English copy (remembering /
night tidy-up / cloud assist with its existing blunt modal + egress banner), language pack, and the
reset/danger zone. All existing consent SEMANTICS byte-unchanged — only words and placement move.

**Retirement (owner decision: retire them all):** chat, inbox, mandates, missions, skills, sources,
review — removed from the app shell entirely (git history keeps the code). Fold-ins: identity/
onboarding → Settings; conflicts → the §3 topic-page card; egress banner + consent modals →
unchanged, re-homed. The **string table** becomes the single source of user-facing copy (one module
the jargon linter reads; components import from it — no inline user-facing literals).

## §6 Technical approach (chosen from three, owner-approved)

**Fresh shell, reused organs.** New shell components (tray panel, two-pane window, topic pages,
Settings) built clean; REUSED unchanged: the engine client + tauri command layer, consent modals,
vault/key seams, search machinery (the ⌘K engine re-skinned into the sidebar search), and the token
system. The old shell and retired-room components are deleted in the same sub-project that replaces
them (SP-U1) — no long-lived dual shell. Rejected alternatives, recorded: refactor-in-place (drags
the old IA's skeleton), parallel second app (double maintenance, YAGNI).

**Engine surface (small, read-only, App-only):** the reading room needs roughly: list topics (+counts
and updated-at), one topic's page + its sources grouped by time, and the constellation's neighbor
graph for a topic. Some of this exists via today's app-side list ops; what's missing becomes 2-3 new
App-only read ops following the exact house pattern (positive-allowlist refusal for guests, socket
tests). NO new write ops; NO daemon behavior changes. Exact op shapes are verified against real
source at SP-U1 plan time (never invented from memory — house lesson).

**Tray mechanics:** Tauri v2 tray-icon feature (currently `features = []` in the desktop Cargo.toml —
an additive flag), template icon derived from `app-icon.svg`, panel as a small always-on-top
vibrancy window. Platform scope: macOS first (Peter's daily platform); the tray abstraction keeps a
labeled seam for Windows/Linux later.

## §7 Sub-projects + order (each: own reviewed plan → subagent build → PR)

- **SP-U1 — the reading room + retirement** (the hot-mess killer): shell, sidebar, topic pages
  (reading view), Settings consolidation, string table + jargon linter + copy audit, retire the seven
  rooms, new read ops. Ships alone as a complete, simpler app.
- **SP-U2 — the menu-bar presence**: tray icon + glass dropdown + window-optional lifecycle.
- **SP-U3 — the constellation**: per-topic orbit + galaxy, canvas layout, perf budget, neighbor-graph
  read op if not landed in U1.

U1 before U2 (the window pain is the acute complaint; the tray depends on strings/tokens U1
establishes). U3 last (delight after utility). All after Rung 5 SP-V1 per the queue decision.

## §8 Error handling + testing

- Daemon-unreachable: tray shows the asleep line; window shows a calm reconnect state; no stack
  traces in either. All engine-call failures render plain-English inline states, never modals.
- Testing per house rules: vitest per component TDD (empty states, collapse/expand grouping, capped
  rendering with a large fixture, consent-copy rendering, tray panel states); the **jargon linter**
  as a CI test over the string table + a repo grep that no component carries user-facing literals;
  0-hardcoded-colors grep stays green; existing suites (engine, daemon, memharness) untouched and
  green by construction; constellation gets a perf test against its budget on a synthetic
  10k-memory fixture.
- The trial week (R4-A) is untouched — this spec merges nothing until after Rung 5 SP-V1 anyway.

## §9 Non-goals / deferred

- **AIR Note inbox slot** in the tray — deferred until the messaging product is user-real (owner
  left it out of the dropdown pick).
- Windows/Linux tray + vibrancy parity (labeled seam only).
- Graph EDITING (dragging nodes to merge topics etc.) — the constellation is read-only in U3;
  owner-approved merge flows stay where they are (R4-B territory, gated on its own evidence).
- Mobile/iPad, publishing/sharing surfaces, and any new write semantics.
- In-window chat of any kind (Claude Code IS the chat surface — background-first).

## §10 Open questions → plan stage

- Exact new read-op shapes + names, verified against `log.rs`/engine source (topics list, topic page
  + grouped sources, neighbor graph) — SP-U1 task 1 territory.
- Dock-icon policy while window closed (accessory vs regular) — trade-off note in §2.
- *Pause 1h* mapping (timed disable/enable vs a first-class engine pause) — pick the no-new-engine-
  semantics option unless the plan finds it ugly.
- Tauri vibrancy API surface + fallback when transparency is disabled by the OS (accessibility).
- Constellation node/edge caps + clustering thresholds — measured, not guessed.
- String-table module location + how tauri-side strings (dialogs) join the linter's scope.
