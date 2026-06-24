# AIR Agent — Review-Fixes Design (2026-06-25)

**Status:** DRAFT — awaiting Peter's sign-off.
**Source:** Live review session 2026-06-25 — 19 change requests on the desktop UI (post PR #51 redesign).
**Method:** UI items scoped from the code directly; the 5 backend/cross-repo unknowns scoped by 4 parallel Opus research passes (findings folded in below).

---

## 1. Summary

19 requested changes, grouped into **6 milestones**. Roughly half are frontend-only (fast, low-risk). The rest are backend/architecture, **3 of which cross a security boundary** and want a review pass: #12 (first deliberate network egress from the brain), #15 (broad filesystem sweep near secrets), #16 (first time the encrypted brain is exposed to another app).

| Milestone | Items | Surface | Effort | Risk |
|---|---|---|---|---|
| **A — Shell, nav & copy** | 1, 2, 3, 5, 9, 10, 11, 13, 14, 17, 18, 19 | Frontend only | M (many small) | Low |
| **B — Rename local display name** | 4a | Tauri + UI | S | Low |
| **C — Contact names in AIR Note** | 6, 7, 8 | air-rs + Tauri + UI | M | Low–Med |
| **D — Brain model: Local or Cloud** | 12 | Desktop backend + UI | M–L | **Med (egress)** |
| **E — Ingest whole drive** | 15 | Engine + Tauri + UI | M | **Med (secrets/scale)** |
| **F — Second brain for Claude Desktop** | 16 | New MCP server + Tauri + UI | M–L | **Med–High (exposure)** |
| **G — Unique AIR username (@handle)** | 4b | Registry (air-site) + Tauri + UI | M | **Med (squatting)** |

**Sequencing (decided):** merge PR #51 → A → B → G → C → D → E → **F (capstone, last)**. A clears most of the visible review feedback fast; F is the strategic headline but heaviest + a new attack surface, so it lands last. G's cross-repo registry work ships server-side before the desktop calls it.

---

## 2. Decisions needed from Peter

Recommendations baked in; override any.

- **D1 — #16 priority. ✅ DECIDED: last (capstone).**
- **D2 — #16 security posture.** Build as **A2 (opt-in, read-only MCP shim that forwards to the running app)** so the encrypted DB + DEK stay in one process and Claude Desktop only gets read-only `recall`. _Rec: yes._
- **D3 — branch strategy. ✅ DECIDED: merge PR #51 first**, then branch each milestone off `main`.
- **D4 — #4 identity model. ✅ DECIDED: two distinct concepts** (→ milestones B + G):
  - **Local display name** (B) — freely editable anytime; shown in the UI and chat. Unsigned metadata, not part of the DID — trivial.
  - **Unique AIR username / @handle** (G) — globally unique, reserved, first-come-first-served, published on AIR, others can't take it. New registry feature (§3-G). **Open sub-decision (decide at G's build):** handles permanent vs. changeable-with-cooldown (squatting vs. typo-trapping).
- **D5 — #12 cloud reasoner.** Off-by-default, opt-in; reuse the chat side's providers (Anthropic / OpenAI-compat / Gemini) + keychain keys; **requires closing the `vault_set` gap** (no wired path to save an API key today) and a security review of the egress. _Rec: yes._
- **D6 — #15 ingest.** Unix-only (Windows has no walk yet); replace the 300 s wall-clock with **progress + cancel**; expand excludes and **security-review the combined filter** before shipping. _Rec: yes._
- **D7 — packaging.** Ship as staged PRs (one per milestone) rather than one mega-PR. _Rec: yes._

---

## 3. Per-item detail

### Milestone A — Shell, nav & copy (frontend only)

- **#2 / #3 — "Identity" → "AIR"; panel heading "Your agent" → "Agent Identity Registry".** Sidebar label + `IdentityPanel` heading. Files: `shell/nav.ts`, `shell/Sidebar.tsx`, `identity/IdentityPanel.tsx`.
- **#5 — "Inbox" → "AIR Note"; panel heading stays "Inbox".** Sidebar label only; `InboxPanel` heading unchanged. Files: `shell/nav.ts`, `inbox/InboxPanel.tsx`.
- **#11 — "Memory" → "Brain"** (sidebar + panel heading both become "Brain"). Files: `shell/nav.ts`, `memory/MemoryPanel.tsx`.
- **#13 — Move Review + Mandates *inside* Brain.** Sidebar collapses to **AIR · AIR Note · Brain · Settings**. Brain becomes a hub with sub-tabs: Search/Evolve · Review · Mandates. The Review "needs-attention" count moves onto the Brain tab (or its Review sub-tab). Files: `App.tsx` (view model), `shell/nav.ts`, `shell/Sidebar.tsx`, new `memory/BrainPanel.tsx` wrapper hosting `MemoryPanel` + `ReviewPanel` + `MandatesPanel`, `shell/useReviewCount.ts` (badge relocates).
- **#1 — Main-screen global search bar (replaces the sidebar search).** Remove the sidebar "Search… ⌘K" trigger; add a prominent search input in the main content area (ChatGPT/Claude style) that drives the **existing** `globalSearch` façade. ⌘K focuses it. Decide: inline results panel vs. opening the same palette overlay positioned under the bar. Files: `shell/Sidebar.tsx` (remove trigger), `App.tsx` / a new `shell/MainSearch.tsx`, reuse `search/globalSearch.ts` + `search/CommandPalette.tsx`, `shell/useCommandPaletteHotkey.ts`.
- **#10 — Chat layout fix (AIR Note thread).** Proper fixed 3-pane: fixed sidebar + fixed conversation list + scrolling message area (auto-scroll to newest) + composer pinned to the bottom; whole app constrained to viewport height (no page-level scroll). Pure CSS/layout. Files: `styles.css` (`.app-shell`, inbox panel/thread/composer rules), `inbox/InboxPanel.tsx` / `MessageThread.tsx` / `Composer.tsx` structure.
- **#9 — "New message" → "New Message"; Title-Case UI labels/buttons.** Descriptive sentences stay sentence-case. Copy-only sweep across components.
- **#17 — Settings + theme toggle → icon-only at the bottom** (gear + sun/moon, with `aria-label` + tooltip). Files: `shell/Sidebar.tsx`, `styles.css`.
- **#18 — Remove the dev line** "AIR endpoint is configured via `AIR_AGENT_USE_REAL_AIR`… (Settings UI in v1.1)". Delete; no user-facing replacement. File: `settings/AirSettings.tsx`.
- **#14 — Mandates: plain language + better UX.** Reframe around: **target file → source folders → you approve each change.** Working copy: _"A mandate is a standing rule: 'keep this file of mine up to date from these folders.' Your agent watches the folders and proposes an edit for you to approve — it never rewrites the file on its own."_ Files: `mandates/MandatesPanel.tsx` (copy + flow).
- **#19 — Plain-language pass (umbrella).** Simplify all copy app-wide; less jargon, more direct. Subsumes #9/#14/#18.

### Milestone B — Rename local display name (#4a)

**Research verdict:** trivial. The display name is **unsigned metadata** in `<app_data_dir>/identity.json` (`air/identity.rs`), is **not** part of the DID (derived from pubkey only) and **not** signed. Renaming touches no keys, no signature, no event log. This is the *local* name — freely changeable, shown in the UI and in chat; distinct from the published unique handle (Milestone G).
**Build:** new Tauri `rename_identity(new_name)` (load metadata → validate non-empty/max-len → swap `name` → save → return) registered in `main.rs`; inline edit/"Rename" affordance in `IdentityPanel`. Keep DID + keypair fixed.
**Files:** `apps/desktop/src-tauri/src/commands/identity.rs`, `main.rs`, `apps/desktop/src/identity/IdentityPanel.tsx`, `api/tauri.ts`. **Effort: S.**

### Milestone C — Contact names in AIR Note (#6/7/8)

**Research verdict:** the names **already exist in the data** — `~/.air-msg/contacts.json` stores per-contact `alias` (user-assigned) + `name` (registry); `agent-bridge-mcp` exposes `agent_list_contacts`; `air-rs` already reads `contacts.json` (single-DID lookup). The UI just never reads them — it prints `short(did)` via a regex. **agent-bridge-mcp needs zero changes; `contacts.json` stays read-only from the desktop.**
**Build:**
- **(6) Show names:** add `air_rs::inbox::stores::list_contacts(home)` (struct already exists; add `name`/`did` fields); new Tauri `inbox_contacts()` (mirror `inbox_conversations`); React contacts `Map<did, displayName>` in `state/inbox.tsx` with precedence **alias → registry name → short(did)**; replace `short()` call sites in `ConversationList.tsx` + `InboxPanel.tsx`. Pair registry-claimed names with the existing verified/key-changed badge (registry `name` is spoofable; the pin is the real guarantee). Rooms keep their id (not a person). Client-side join — no SQL change.
- **(7) Search by name:** extend `search/filterConversations.ts` to also match the resolved name (near-zero once conversations carry a title).
- **(8) Recipient dropdown:** `Composer.tsx` — when `to == null`, swap the free-text input for a contacts combobox (label = alias, value = DID) with a free-text fallback for unknown DIDs.
**Files:** `crates/air-rs/src/inbox/stores.rs`, `apps/desktop/src-tauri/src/commands/inbox.rs` + `main.rs`, `apps/desktop/src/{api/inbox.ts, state/inbox.tsx, inbox/ConversationList.tsx, inbox/InboxPanel.tsx, inbox/Composer.tsx, search/filterConversations.ts}`. **Effort: M.**

### Milestone D — Brain model: Local or Cloud (#12)

**Research verdict:** the `Reasoner`/`ReasonerProvider` seam was **pre-designed for cloud drop-in** (the error variant is already stubbed). Cleanest path is a **desktop-side `CloudReasoner`** using the desktop's `reqwest` + vault — this avoids the bossclaw-core "network-posture" CI guard (which only permits `ureq` in the engine) and sits next to the existing `claude_generate`/keychain code.
**Build:** desktop `CloudReasoner` impl of `Reasoner::complete_json` (Anthropic/OpenAI/Gemini, **schema-constrained JSON output is net-new** — chat only streams free text); `CloudReasonerProvider` reading the key from the vault; make the reasoner config-driven (not the hardcoded `REASONER_MODEL_ID`); `engine_get/set_reasoner_config` commands; branch the scheduler's `ollama_ready` gate so "cloud" bypasses the Ollama probe; **close the `vault_set` gap** so a key can be saved; Local/Cloud selector UI in the Brain tab. **Off by default, opt-in** (mirrors evolve/mandates); the egress wants a security review.
**Files:** `apps/desktop/src-tauri/src/engine/reason.rs`, `engine/mod.rs`, `engine/scheduler.rs`, `commands/engine.rs` + `main.rs`, `vault.rs` (+ `vault_set` command), Brain-tab UI. **Effort: M–L.**

### Milestone E — Ingest whole drive (#15)

**Research verdict:** ingest exists with real DoS guards (300 s wall-clock, depth 64, 100 k entries/dir, 10 MiB file cap, symlink-skip, `(dev,ino)` dedup) and a **security never-touch list** (~30 secret patterns). A bare `$HOME` grant **runs but degrades**: no bulk-junk excludes → the 300 s cap trips → nondeterministic partial ingest; per-file synchronous embed is the real cost; no progress/cancel; broad sweep widens the secret-leak surface.
**Build:** keep the simple model — one-click = `add_grant($HOME)` + ingest — but add, **in the engine** (next to the security list, kept as a *separate* const): a `SKIP_DIRS_SCALE` set (`node_modules .git target .cache .venv __pycache__ .next dist build .Trash Library/Caches "Photos Library" .gradle .cargo/registry` …) + `*.app`/`*.framework` bundle skip + optional "skip hidden dotfiles" toggle; thread a **cancel `AtomicBool` + progress callback** through `walk_grant`/`ingest_all` (the periodic-check spots already exist), surfaced via a Tauri `Channel`/emit; **replace the 300 s wall-clock with the cancel flag** for explicit bulk runs; batch embeds (perf, optional). UI: "Ingest my whole drive" button + progress bar + Cancel + a skip summary. **Security-review the combined exclude/never-touch filter.** Unix-only.
**Files:** `crates/bossclaw-core/src/ingest.rs` (excludes + progress/cancel), `log.rs` (embed batching, optional), `engine/mod.rs` (`run_ingest` signature), `commands/engine.rs` + `main.rs` (`engine_ingest_home`, `engine_cancel_ingest`, progress), `apps/desktop/src/{api/engine.ts, sources/SourcesPanel.tsx, sources/ingestSummary.ts}`. **Effort: M (+S/M for batching).**

### Milestone F — Second brain for Claude Desktop (#16)

**Research verdict:** **not built.** Nothing exposes bossclaw-core recall over MCP today (agent-bridge-mcp = messaging only; air-site/mcp-server = remote registry only). Claude Desktop reads `~/Library/Application Support/Claude/claude_desktop_config.json` (already holds `gbrain` + user `preferences` — a connect flow **must read-merge-write `mcpServers` only**, never clobber). Recommended **A2** architecture: a thin **stdio MCP shim** (cloned from the `agent-bridge-mcp` skeleton) that forwards a read-only `recall` tool to the **already-running** AIR Agent via a small loopback command — so the encrypted DB stays single-owner and the DEK never leaves the one Tauri process (A1, a second Rust binary opening `brain.db`, risks `SQLITE_BUSY` + a second DEK/keychain path + more password prompts).
**Build:** new `air-memory-mcp/` stdio server (recall tool, forwards to the app); a loopback recall endpoint/command on the app (reuse `EngineHandle::recall`); Tauri `claude_desktop_connect/status/disconnect` (atomic read-merge-write of the config, mode 0600 preserved); a "Connect to Claude Desktop" button in Settings; UX copy telling the user to restart Claude Desktop. **Opt-in, read-only**; security note: this is the first time the brain is queryable by another app's prompts (injection surface) — scope tools read-only, consider result limits.
**Files:** new `air-memory-mcp/` (server), `apps/desktop/src-tauri/src/commands/claude_bridge.rs` + loopback recall + `main.rs`, `apps/desktop/src/settings/…` (UI). All in `air-note` (no air-site changes). **Effort: M–L.**

### Milestone G — Unique AIR username / @handle (#4b)

**Research verdict:** doesn't exist today. The registry `name` is free-form and **not unique** (duplicates allowed with a warning, `api/src/index.js:1135`); `air_id` (`AIR-XXXX-…`) is the only unique handle but it's machine-shaped. `/check-name` (`checkName()`, `index.js:1472`) is **advisory only** (case-insensitive display-name probe; reserves nothing). No username/handle/slug concept anywhere. **Big reuse win:** the existing owner-auth (`X-Agent-Secret` → `agent_secret_hash`, used by the agent-update PUT, `index.js:1252`) is exactly the claim auth needed.
**Build (registry — air-site):** migration `0007_add_username.sql` (next in sequence) adding `username` + a **case-folded UNIQUE index** (store a normalized `LOWER(username)` and unique-index *that* — a bare SQLite UNIQUE is case-sensitive and would let `Alice`/`alice` both be claimed); a `GET …/check-username` availability probe (mirror `checkName()`); a claim path on the authenticated update (allow-list `username`; enforce first-come via the DB UNIQUE constraint → 409 on collision, **not** a read-then-write TOCTOU); validation (charset `^[a-z0-9_]{3,30}$`, reserved-words denylist incl. the `AIR-` shape, homoglyph/confusable normalization to stop look-alike squatting); optional `GET /agents/by-username/{u}` resolver.
**Build (desktop):** `check_username` + `claim_username` on the `AirClient` trait + `HttpAirClient` + `MockAirClient` (reuse the `X-Agent-Secret` path already in `update()`); Tauri commands in `commands/identity.rs` + `main.rs` (owner secret already loaded from keychain); a claim/check UI in the AIR (Identity) panel, **visibly distinct** from the local-display-name rename (B).
**Cross-repo:** the registry side (air-site: `api/migrations/0007…`, `api/src/index.js`, `openapi.yaml`, tests; + `mcp-server/` tools + `sdk/python` parity) must ship **before** the desktop can call it. Pairs naturally with #6/#7 (the @handle is the proper *global*, human-addressable lookup key; the local contact `alias` is the *local* name).
**Open decision:** handles permanent vs. changeable-with-cooldown. **Effort: M.** **Risk: Med** — case/homoglyph squatting, FCFS race (must be an atomic DB constraint), legacy agents with `NULL` secret-hash can't self-claim.

---

## 4. Cross-cutting

- **`vault_set` gap (blocks D).** There's no wired Tauri command today to *save* a provider API key (only `web_auth_set` writes the keychain blob). Closing this once unblocks both the cloud reasoner (#12) and the chat side. Sequence it ahead of D.
- **Security reviews.** #12 (egress), #15 (broad sweep near secrets), #16 (brain exposure), and G (username squatting/validation) each warrant a focused pass before merge.
- **Branch strategy.** ✅ Merge PR #51 first, then branch each milestone off `main`.
- **Platform.** #15 is Unix-only (Windows walk not built). Everything else is cross-platform.
- **Out of scope (noted, not built):** per-message sender names in 1:1 threads (#6 mainly matters in the list/header + rooms); incremental/resumable ingest; registry re-publish on rename (unless D4 says otherwise).

## 5. Execution (after sign-off)

Decompose each approved milestone into a TDD task plan (the PR #51 pattern: fresh subagent per task, two-stage spec→quality review, whole-impl review), Opus throughout. Order: **merge PR #51 → A → B → G → C → D → E → F**. Ship as staged PRs; G's registry (air-site) side ships before its desktop side. Manual GUI QA per milestone; security review on D, E, F, and G.
