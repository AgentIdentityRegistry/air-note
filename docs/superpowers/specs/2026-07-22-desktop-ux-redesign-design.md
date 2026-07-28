# Desktop UX Redesign — tray-first presence + the reading room — Design

**Status:** Rev 2 — Rev 1 owner-approved conversationally 2026-07-22 (visual brainstorm; mockups in
`2026-07-22-ux-mockups/`); independently reviewed 2026-07-22: architect **SOUND-WITH-CHANGES**
(3 Blocker / 5 Major / 4 Minor — every finding source-verified with file:line evidence) + critic
**REWORK** (4 Critical / 6 Major / 4 Minor + gap list). ALL findings folded into this revision
(changelog §11). Convergent findings (both reviewers independently): the empty-Topics default trap,
the orphaned write-powers retirement, the op undercount, the phantom CI, the wrong room inventory —
per the house convergence rule, all treated as certain. Owner rulings 2026-07-22: **powers → compact
Settings section** (everything stays revocable); **CSS-only glass** (no private API — Mac App Store
stays possible); **Quit = app only, the brain keeps running**. Awaiting reviewer re-verification +
file-level owner review before per-sub-project planning.
**Build queue (owner, 2026-07-22): design now, BUILD AFTER Rung 5 SP-V1** (which itself gates on the
R4-A dogfood verdict after Sun 2026-07-27). Sub-project plans are written when their build slot
arrives, never earlier.
**North Star anchor:** `air/vision-background-first-claude-code-native-2026-07-14`: AIR Agent runs
quietly and is reachable FROM Claude Code — a background presence, not a destination app. This
redesign makes that literal.
**Trigger (owner, 2026-07-22):** "Make the app a whole lot simpler… a menu-bar icon with important
stuff… when open, more like Obsidian — click something and see what it saved. Right now it's a hot
mess of a page that continuously lists things." Plus two laws from the same conversation: no jargon,
and modern glass materials.

---

## §0 Goal + posture (honesty corrected per critic)

Replace the accumulated multi-room desktop app with **three surfaces** — a menu-bar presence, a
two-pane reading room, one Settings page — governed by two laws (§1). Rev 1 claimed "a wrong UI
decision here costs pixels, never memories"; review proved that sentence false and it is struck:
**several decisions in this spec govern memories, consents, and file-writing powers** — each such
decision is called out inline with its safety treatment. Daemon posture, stated honestly: **no
existing daemon behavior changes**, plus (a) six new read-only App-only wire ops (§6), (b) ONE new
engine behavior — the durable Pause (§2a), designed precisely so the UI can never touch the consent
machinery — and (c) a derived read-shape addition to the pages projection (updated-at + count).

## §1 The two laws (enforced by CI that SP-U0 builds first)

- **L1 — Jargon-free copy.** No internal vocabulary reaches a user-facing string. Banned → replaced:
  dossier → *topic page/summary*; reflect → *night tidy-up*; capture → *remembering*; evolve → the
  WORD is invisible but its SWITCH is not (§5 "Keep summaries fresh" — review Blocker: hiding the
  switch that produces the reading room's content would blank Topics for every new install); library
  → *your memory*; retire/supersede → *forget / correct*; conflict → *"you saved two versions"*.
  Enforcement, three layers (critic M6): (1) a **string table** module = the single source of
  user-facing copy, jargon-linted in CI; (2) a **daemon-error translation boundary** — one
  `engineErrorToPlainEnglish(kind)` mapper keyed on the wire's typed `OpErrorKindWire`, plus a lint
  rule banning `String(e)` in render paths (34 such sites exist today and speak raw engine jargon);
  (3) the linter + colors grep + vitest + eslint run in a **new desktop CI lane** — none of this
  exists in CI today (verified), so building the lane is SP-U0 work, and this spec says "is
  introduced," never "stays green." Strings are keyed (Korean-ready); full i18n is a §9 non-goal.
- **L2 — Glass, CSS-only (owner ruling).** The glass look — layered translucency, depth, rounded
  materials, SF type — built entirely with CSS inside an **opaque window**: no `transparent` flag,
  no `macos-private-api`, no true behind-window blur. Trade recorded: real vibrancy requires a
  private API that bars the Mac App Store; owner chose the App-Store-viable path. Tokens live in
  `apps/desktop/src/styles.css` (named explicitly — the dead `design/tokens.ts`/`themes.ts` are
  swept in §5); the colors grep extends to `.ts` as well as `.tsx`. Light/dark follow the system;
  the constellation is dark-native. Visual reference: the committed mockups, EXCEPT where this spec
  supersedes them (the spec wins over the mockups on tray contents — §2 is authoritative).

## §2 Surface 1 — the menu-bar presence

The app process owns a template (monochrome) menu-bar icon derived from the gold planet
(prerequisite: the `feat-app-icon` branch lands first; tray needs the `tray-icon` + `image-png`
cargo features — two flags, verified). Clicking opens one glass panel:

1. Heartbeat, three states: `🟢 On — remembering as you work` · `⏸ Paused — not remembering right
   now` (daemon reachable, remembering off or paused) · `🌙 Asleep — open AIR Agent to wake it`
   (daemon unreachable).
2. Vitals: memory count + last night's tidy report in plain words — served by the new `Vitals` read
   op (§6; the digest is engine-internal today and reachable only inside Snapshot — verified).
3. Search → opens the reading room pre-filtered.
4. Switch chips: *Remember* ✓ · *Night tidy* ✓ · *⏸ Pause 1h* (§2a). Chips flip only
   already-consented flags via the existing App-only ops; a first-ever enable still routes through
   the full consent modal. The dropdown is never a consent surface.
5. Recent saves — **privacy-first (critic M7):** by default a COUNT + relative time ("3 saves in the
   last hour"), never content; click-to-reveal shows titles; a persisted "Show recent saves"
   preference (default OFF) can keep titles visible. Rationale: titles are verbatim first-prompt
   prefixes (up to 120 chars) — menu bars appear in screen-shares.
6. One button: **Browse your memory**.

**Lifecycle (corrected per architect):** today, closing the last window EXITS the app process
(verified — no tray, no ExitRequested handler exists; only the daemon survives, by its own design).
SP-U2 adds the `ExitRequested → prevent_exit()` handler so closing the window keeps the app + tray
alive. **Quit AIR Agent** (tray menu) quits the app; **the brain keeps running** (owner ruling) — a
clearly-worded *Stop the brain* control lives in Settings' danger zone for a full stop. Accepted +
stated: with the app quit, the daemon runs with NO indicator (that is the North-Star state); the
presence returns on next launch. Login-item autostart and Dock-icon policy are §10 plan questions.

### §2a Pause — a first-class engine feature (forced by critic Critical 1+2; Rev 1's mapping is DISQUALIFIED)

Review traced Rev 1's "UI-side timed disable+re-enable" to permanent, silent memory loss: re-enabling
capture REWRITES the forward-only window (`capture_enabled_at`) and SPENDS the one-time backfill
consent (`backfill_consented: false` on disable — engine's own test: "forward-only window moves to
re-enable"), destroying the paused hour AND the entire in-flight 30-day backfill; and a UI-side timer
dies with the app, leaving remembering off indefinitely. Therefore:

- Pause is a **new engine op** (`SetPause{until}` — App-only): a durable, signed `pause_until`
  deadline the **daemon itself enforces and auto-expires** — it gates the capture sweeper while
  active and **never touches** `capture_enabled` / `capture_enabled_at` / `backfill_consented`.
  Survives app quit and daemon restart; hard maximum 8 hours; the tray offers 1h.
- Scope, stated in the panel's own copy: Pause stops **remembering new activity** only ("new
  activity won't be remembered until 15:00 — summaries and file automations are unaffected; their
  own switches are in Settings"). No hidden couplings.
- Tests (§8): pause → sweep skips; expiry → resumes with the ORIGINAL window and backfill state
  byte-untouched; app-death during pause → daemon still expires it.

## §3 Surface 2 — the reading room

Two panes, classic-Obsidian shape. Left sidebar (glass): search pinned; then *Topics*, *Notes*,
*Conversations* (by project), *Files*. Right: the page.

**The topic page — reading view (default).** The rule that kills list-itis: **the summary IS the
page; memories are footnotes** — now with testable numbers (critic M9): at 1280×800 the default
render of ANY topic page is **≤ 1.5 viewport heights**; the summary clamps to **12 lines** with a
"read all" expander (the summary text itself is engine-rendered and unbounded — the clamp is the
UI's job); *This week* shows **≤ 5 cards**; older time groups render collapsed; every expander
reveals in **pages of 20** (expanders are themselves capped — "no unbounded scroll, ever" applies
to expanded states too). Memory cards: plain origin tag, date, snippet; opened cards offer *correct
this* and *forget this* (forget = the existing tombstone flow + confirmation; the wire op exists
today but has no tauri/TS binding — §6 adds it). A topic with an open disagreement shows ONE gentle
card — **view-only in this redesign**: it explains and points to Claude Code to resolve ("resolve it
where you work"). Rationale: resolution over the wire is a WRITE op; keeping U1 read-only preserves
the §0 posture. In-app resolution is deferred (§9).

**Three empty states, all designed, all honest (review Blocker — they are different situations):**
- *Fresh brain*: "Your first topics will grow here as you work — nothing to do." (Shown only when
  the summary loop is actually ON.)
- *Summaries off*: "AIR isn't building topic summaries yet — turn on 'Keep summaries fresh' in
  Settings." One click routes there. (Without this, a default install — summary loop force-off,
  verified — shows a permanently blank Topics list and Rev 1's empty state would LIE.)
- *Topic without a summary yet* (memories exist, no page row): the page leads with the grouped
  memories under a quiet notice: "No summary yet — it will appear after the next tidy-up."

**Cloud disclosure (critic M8):** the existing egress banner (unchanged semantics: visible whenever
cloud assist is enabled) renders persistently across ALL reading-room surfaces and search — not
only in Settings. Disclosure lives where cloud-derived content is seen, or it isn't disclosure.

## §4 The constellation (✦ one click away; reading-first)

As Rev 1 — topic as gold sun, memories orbiting, real links only (citations + shared entities),
neighbor topics at the edges, galaxy at full zoom-out — now with **floor requirements in-spec**
(critic M10; a deferred budget is a test that cannot fail): on the 10k-memory synthetic fixture on
the reference machine, **first paint ≤ 500 ms** and **≥ 30 fps** sustained pan/zoom; if no cap/
clustering configuration meets the floor, U3 ships a static layout or does not ship.
`prefers-reduced-motion` gets a static layout. Canvas-rendered; caps + clustering thresholds are
plan-stage measurements against these floors.

## §5 Surface 3 — Settings, the corrected retirement, and the powers section

**The room inventory, corrected against `shell/nav.ts` (both reviewers; Rev 1's list was wrong in
both directions).** Actual views today: identity · inbox · library · memory · review · mandates ·
settings (library/memory/review/mandates live inside the Brain hub).

- **Retire (rooms):** *inbox* (including the chat surface `AIPanel`, the inbox state provider, and
  the AI-reply loop), *review*, *mandates* — as ROOMS; their live powers re-home below. *identity*
  as a room — onboarding becomes a **first-run route** shown before the shell until onboarded
  (today's gate, restated; Settings hosts the identity card afterward).
- **Re-home:** *library* + *memory* → the reading room (NOTE: `MemoryPanel` hosts the cloud-consent
  panel, egress banner, and language-pack card — these move to Settings/§3, listed explicitly so
  nothing consent-bearing is dropped); *sources* is ALREADY a Settings section (not a room) and
  stays, absorbed into the powers section.
- **Sweep (dead code, verified zero importers):** `missions.ts`, `skills/`, `toolRegistry.ts`,
  `design/tokens.ts`, `design/themes.ts`.

**Settings, one calm page:** Identity card · Connections (Claude Code connect/disconnect) · the
consent switches in plain English — *Remembering*, *Night tidy-up*, **"Keep summaries fresh"** (the
summary loop's plain-English switch — review Blocker: it must exist and must explain when a
summarizer isn't set up yet: "needs a local model or cloud assist") — · Cloud assist (existing blunt
modal + banner) · Language pack (the embedding model — not UI localization; copy will say so) ·
**"What AIR can touch"** (owner ruling — the powers section): folder grants (list/add/revoke),
per-folder write permission (with its existing proposals coupling), the automation kill-switch +
standing-automation list + revoke, write history + undo, and the pending-review queue with the
existing loud-confirmation apply/decline. Nothing live is ever headless: every consent granted
anywhere remains revocable here. · *Stop the brain* + reset (danger zone).

**Data, stated (critic gap):** retiring the inbox room removes the UI only — archived conversations
remain on disk untouched and unsurfaced (recorded here so it is a decision, not an accident).
Configured automations and grants remain fully controllable via the powers section.

## §6 Technical approach (claims corrected)

**Fresh shell, reused organs — with the reuse claims audited:** genuinely reusable unchanged: the
tauri command layer, consent modals, vault seams, `styles.css` tokens. The ⌘K search is a
**rework, not a reuse** (architect M8 — it imports inbox types in six files): the façade shape
stays; the conversations source is removed, a topics source added; budgeted as its own task.

**Engine surface, enumerated honestly (was "2-3", verified ≈ six + one behavior):** new App-only
READ ops — `ListTopics` (counts + updated-at; the pages projection gains derived updated-at/count —
a read-shape change, named here), `TopicPage` (page + sources grouped by time), `TopicNeighbors`
(constellation), `Vitals` (memory count + tidy digest line), `RecentSaves`; plus surfacing the
EXISTING `RetireMemory` wire op through tauri + TS (it has no binding today). Each new read op ships
with a daemon-side sanitizer (the `sanitize_conflict_row` precedent) so retired/tombstoned content
cannot leak, an allowlist entry (App-only), and socket tests. The ONE write/behavior addition is
`SetPause` (§2a). Conflict resolution stays out (view-only card, §3).

**Tray mechanics:** Tauri v2 `tray-icon` + `image-png` features (additive); template icon from
`app-icon.svg` (prerequisite: that branch merges first); the panel is a small always-on-top window
styled by L2 (CSS glass — no transparency flags needed, which also sidesteps the §10 accessory-mode
interaction question).

## §7 Sub-projects + order (restructured per review)

- **SP-U0 — plumbing + gates first** (new; executor note from review): the desktop **CI lane**
  (vitest + eslint --max-warnings 0 + colors grep incl. `.ts` + jargon linter) and ALL §6 engine
  work (six read ops + sanitizers + `SetPause` + socket tests + the pages read-shape change). Lands
  before any UI change, so every later commit is provably green and the UI is never built against
  invented op shapes.
- **SP-U1 — the reading room + Settings + retirement**: new shell, sidebar, topic pages (reading
  view, numeric bounds), the three empty states, Settings incl. the powers section, string table +
  error-translation boundary + copy audit, retirement/re-home/sweep per §5. Commit rule (architect
  M10): every commit changes component + test + importers together; deletions remove all three in
  one commit. Ships as one PR; **rollback = revert of that squash** (stated as the mechanism), with
  a pre-merge owner GUI walkthrough on a signed build as the ship gate.
- **SP-U2 — the menu-bar presence**: tray + dropdown + §2 lifecycle (prevent_exit, Quit semantics).
- **SP-U3 — the constellation**: against the §4 floors.

U0 → U1 → U2 → U3, all after Rung 5 SP-V1. **Success, observable (critic gap):** two metrics
reported (not gated) from existing local data after U1+U2 have been live a while: window-opens per
week (expected DOWN — the North Star is fewer visits) and tray-opens per week (expected UP).

## §8 Error handling + testing

- All engine-call failures render through the §1 translation boundary — plain-English inline states,
  never raw `String(e)`, never modals. Daemon-unreachable states per §2.
- Tests: the §2a pause suite (window + backfill byte-untouched; daemon-side expiry; app-death case);
  numeric calm-screen assertions (§3 bounds as rendered-node/height checks on a large fixture);
  privacy default (recent saves show counts, not titles, until revealed); banner-presence test on
  every §3 surface with cloud on; the three empty states; jargon linter + colors grep in the new CI
  lane; socket tests for every new op incl. guest-refusal; constellation floor benchmark (§4);
  accessibility: keyboard navigation across both panes, focus trap in the tray panel (a known
  deferred item from the last shell redesign — now in scope), contrast checks on glass tokens,
  reduced-motion. Existing engine/daemon/memharness suites untouched and green.
- The R4-A trial is untouched — nothing here merges before Rung 5 SP-V1 anyway.

## §9 Non-goals / deferred

- In-app conflict RESOLUTION (view-only card ships; resolving stays in Claude Code — revisit later).
- AIR Note inbox slot in the tray; Windows/Linux tray + glass parity (labeled seams).
- Full i18n/localization (strings are keyed and Korean-ready; per-locale banned lists later; the
  "language pack" naming collision with the embedding model gets fixed copy now, §5).
- Graph editing; owner-approved merges stay R4-B territory. Mobile; publishing; in-window chat
  (Claude Code IS the chat). Mac App Store submission (not pursued now — but the CSS-glass ruling
  deliberately keeps it possible).
- True vibrancy (revisit only if the App Store stance changes).

## §10 Open questions → plan stage

- Exact op shapes/names for the six read ops + `SetPause`, verified against source at SP-U0 plan
  time (never invented — house rule).
- Dock-icon policy while the window is closed; login-item autostart for the tray presence.
- Final pause copy (the §2a scope sentence), and whether 1h/8h are the right presets.
- String-table module location; how tauri-side dialog strings join the linter corpus (the daemon
  side is covered by the §1 mapper).
- Reference machine definition for the §4 floors.

## §11 Changelog

- **Rev 2 (2026-07-22):** folded independent review round 1 — architect SOUND-WITH-CHANGES (A1
  sources-orphan → powers section; A2 mandates/review orphan → owner ruling: compact Settings
  powers section, everything revocable; A3 no app-side topic path + evolve force-off → §6 op
  enumeration + §5 "Keep summaries fresh" + §3 three empty states; A4 op undercount → six ops + one
  write, conflict card view-only; A5 lifecycle half-false → §2 corrected + prevent_exit in U2; A6
  Liquid-Glass private-API trade → owner ruling: CSS-only glass, App Store stays possible; A7 no CI
  → SP-U0; A8 search entanglement → rework-not-reuse; A9 room inventory corrected vs nav.ts; A10
  commit rule; A11 icon-branch prerequisite + two cargo features; A12 tokens = styles.css, sweep
  dead token modules) and critic REWORK (C1+C2 **Critical**: pause consent-destruction + undying
  pause → §2a first-class durable engine pause, Rev 1 mapping disqualified, §10 steer inverted; C3
  empty-Topics trap [convergent w/ A3]; C4 orphaned powers [convergent w/ A1/A2]; C5 honest daemon
  posture → §0 rewritten, "costs pixels" struck; C6 linter blindness → translation boundary +
  String(e) lint + CI lane; C7 tray privacy → counts-by-default + click-to-reveal; C8 egress banner
  on all surfaces; C9 numeric calm-screen bounds; C10 constellation floors; C11 inventory
  [convergent w/ A9]; C12 spec-vs-mockup authority stated; C13 i18n non-goal + keyed strings +
  language-pack naming fix; C14 quit semantics → owner ruling + Stop-the-brain in Settings; gaps:
  data-migration statement, onboarding first-run route, pause scope copy, rollback mechanism +
  ship gate, a11y test scope, success metrics). Sub-projects restructured U0→U1→U2→U3.
- **Rev 1 (2026-07-22):** initial design from the owner visual brainstorm (tray B+recent-saves;
  two-pane reading-first; constellation toggle; retire-all simplification; two laws).
