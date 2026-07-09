# Memory Hub Phase 2 (M1b Code Loop) — SP2: One-Click Claude Code Integration (Design)

**Status:** Draft 1 — awaiting Peter's spec review → then `writing-plans`. (Peter approved the design in brainstorming, 2026-07-10, with a standing ask: run the implementation plan through an architect + critic + security second opinion *before* any code.)

**Program:** Phase 2 of the memory-hub program (★ North Star `air/memory-strategy-2026-07-03-beat-the-stack`: "install AIR Agent → your Claude Code / Codex just never forgets"). Phase 1 retrieval floor is complete (rungs 0–1 + multilingual shipped, main `54dfefa`). Phase 2 = the Code loop, decomposed into three shippable sub-projects:

- **SP1** (shipped → [PR #76](https://github.com/AgentIdentityRegistry/air-note/pull/76), green, mergeable) — the safe read+write loop *backend*: a `remember` write op, per-op `MemoryClient` authz, and the Rust **MCP adapter** `crates/air-memory-mcp` exposing `recall` + `remember`, wired by hand (a documented `.mcp.json` snippet).
- **SP2 (this doc)** — replace the hand-wiring with **one click**: a desktop **Integrations** section in Settings that safely writes (and cleanly removes) the Claude Code MCP-server + SessionStart-hook config, backed by SP1's adapter.
- **SP3** — automatic behaviors (session-start snapshot, auto-capture, recall-miss instrumentation).

Each sub-project is independently shippable + testable and gets its own spec → plan → build cycle. SP2 depends on SP1 (the adapter binary + its socket protocol); SP3 depends on SP1/SP2.

**Goal:** From the AIR Agent desktop app, let a user connect AIR memory into Claude Code **globally, in one click** — the app writes the MCP-server entry and a minimal SessionStart nudge into the user's real Claude Code config, *merging* (never clobbering) and *reversibly*, so every Claude Code session everywhere can `recall`/`remember`. Ship it for **Claude Code only**, behind an agent-adapter seam that Codex slots into as a fast follow-up.

---

## 1. Why (the North Star, this half)

SP1 made the loop *possible* but *manual*: the user must locate the built `air-memory-mcp` binary, hand-edit a `.mcp.json`, and paste the socket path. That is a dead end for "install → it just works" — most users will never do it, and a hand-wired per-project config isn't "never forgets everywhere." SP2 turns the manual snippet into a single **Connect** button that writes the *global* Claude Code config so **every** session, in **every** folder, has AIR memory — and a **Disconnect** that removes exactly what it added. This is the bridge between a working backend (SP1) and the automatic memory behaviors (SP3): the tools have to be *reachable and known* before they can be made *smart*.

## 2. Product decisions (locked with Peter, 2026-07-10 brainstorming)

1. **Claude Code first** (not both agents at once). SP2 ships Claude Code wiring only, behind a clean **agent-adapter seam** so Codex is a fast follow-up (a second adapter writing TOML), not a rewrite. YAGNI: prove one target live before doubling the config-format surface.
2. **Global / user scope** (not per-project). Write the MCP server to `~/.claude.json`'s top-level `mcpServers` and the hook to `~/.claude/settings.json`'s `hooks.SessionStart`. One click ⇒ every Claude Code session everywhere. (Per-project `.mcp.json` is explicitly out — it would need the app to know which repo and would require re-clicking per project.)
3. **SessionStart hook = a minimal static nudge**, not auto-recall. The hook injects a short fixed reminder that AIR memory exists and how to use `recall`/`remember`. It makes **no** network/daemon call at session start. The *smart* auto-recall + recall-miss instrumentation stays in **SP3** — this keeps the SP2/SP3 boundary clean.
4. **UI lives as an "Integrations" section inside the existing Settings card** (next to Sources / Danger Zone). It is a set-once configuration action — exactly what Settings is for. No new top-level nav item; no Brain sub-view.
5. **Direct read-merge-write for both files** (not shelling out to `claude mcp add`). No dependency on the `claude` CLI being on `PATH`, fully unit-testable on temp fixtures, and the hook file has no CLI equivalent anyway — one mechanism stays consistent across both files.

## 3. Verified current-state reality (anchors — re-verify at plan time on this branch's base)

**The adapter binary (SP1, to be bundled + wired):**
- `crates/air-memory-mcp/` — the MCP stdio server (SP1 U3). `README.md` documents the *manual* wiring SP2 automates: `mcpServers."air-memory" = { command: <abs path to target/release/air-memory-mcp>, env: { BOSSCLAWD_SOCKET: <abs socket> } }`. If `BOSSCLAWD_SOCKET` is omitted the adapter self-resolves the same default the daemon uses; if the daemon is down the tools return a clean "memory service unavailable" (never crash the session).

**How a sibling binary reaches a stable path (mirror this for `air-memory-mcp`):**
- `apps/desktop/src-tauri/tauri.bundle.conf.json:4` → `"externalBin": ["binaries/bossclawd"]` (bundle-only config). `apps/desktop/src-tauri/binaries/.gitkeep` documents the contract: the actual `bossclawd-<triple>` is a build artifact copied into `binaries/`; Tauri places it in the bundle next to the main exe (macOS `Contents/MacOS/bossclawd`).
- `apps/desktop/src-tauri/src/engine/daemon.rs` — `ENV_BIN = "BOSSCLAWD_BIN"` (L25), `BIN_NAME = "bossclawd"` (L27), `resolve_bin_path(current_exe)` (L52): env override → sibling of the exe → parent-sibling dev fallback. `resolve_socket_path(data_dir)` (L42) delegates to `bossclawd_paths::resolve_socket_path`.
- `apps/desktop/src-tauri/src/main.rs:75–101` — at boot the app resolves `sock_path` + `bin_path` from `current_exe`, stages the model, `ensure_started`, and builds a `SocketTransport`. So the running app already knows, in Rust, both the exe dir (→ our binary) and the data dir (→ the socket): everything the config-writer needs to emit is resolvable at click-time.

**Socket-path single source (what we write into the config):**
- `crates/bossclawd-paths/src/lib.rs` — `ENV_SOCKET = "BOSSCLAWD_SOCKET"`, `SOCKET_FILE = "bossclawd.sock"`, `APP_DIR_NAME = "ai.air-agent.desktop"`, `resolve_data_dir()`, `resolve_socket_path(data_dir)`. macOS default socket: `~/Library/Application Support/ai.air-agent.desktop/bossclawd.sock`. Pure `*_from` helpers take env values as args (hermetic, race-free tests) — the pattern SP2's config-writer copies.

**The write targets (real Claude Code config schemas, verified on this machine 2026-07-10):**
- `~/.claude.json` — perms `0600`; has a top-level `mcpServers` object. A stdio entry's shape: `{ "type": "stdio", "command": <str>, "args": [<str>…], "timeout": <int> }` (e.g. the existing `chrome` entry). Plain JSON (no comments).
- `~/.claude/settings.json` — perms `0600`; keys include `hooks`, `permissions`, `env`, `statusLine`, …. `hooks.SessionStart` is an **array of matcher-groups**, each `{ "hooks": [ { "type": "command", "command": <str>, "timeout": <int> } ] }`. **On this machine the array is already non-empty** (Peter's forever-memory loop). ⇒ the smoking gun for read-merge-write: a naïve write would delete a foreign hook. A group with no `matcher` applies to all SessionStart sources (matches the existing entry).
- `~/.codex/` exists (Codex config is `~/.codex/config.toml`, `[mcp_servers.<name>]`) — **anchor for the seam only; not written in SP2**.

**Where the UI + commands attach:**
- `apps/desktop/src/settings/AirSettings.tsx` — a `Card` hosting `<SourcesPanel />` + a Danger Zone; the Integrations section is a new sibling here.
- Tauri commands are registered in `main.rs`'s `invoke_handler(generate_handler![…])`; existing simple command modules (`vault.rs` → `vault_set/has/delete`) are the pattern for a new `integrations` module. Frontend invoke wrappers live in `apps/desktop/src/api/tauri.ts` (e.g. `resetIdentity`).
- **Nothing references `.mcp.json`, `settings.json`, Codex, `SessionStart`, or "integrations" anywhere in `apps/desktop/**` today** — SP2 is greenfield.

## 4. Design principles / invariants

- **I1 — Merge, never replace (and keep key order).** Every foreign key in `~/.claude.json` and every foreign hook in `~/.claude/settings.json` is preserved — including existing **key order** (serde `preserve_order`, so merging the user's large real config only appends our key rather than alphabetizing the whole file). We only add/remove our own `air-memory` mcpServers key and our own SessionStart group.
- **I2 — Atomic + `0600`, no world-readable window.** Write a temp file *in the same directory* as the target, born `0600` (created with mode `0o600`, never chmod-after), `fsync`, then `rename` over the original (atomic within one filesystem). A crash leaves either the old file or the new file, never a truncated one, and never a readable-by-others temp.
- **I3 — Idempotent.** Connecting twice yields exactly one mcpServers entry and one SessionStart group. De-dup: mcpServers keyed by the name `air-memory`; the hook de-duped by matching a command that runs our binary **with the `nudge` subcommand**.
- **I4 — Reversible + surgical.** Disconnect removes *only* the `air-memory` mcpServers key and *only* the SessionStart group whose inner command runs our binary **with `nudge`** (both required — a foreign hook that merely mentions the string survives) — verified by a test that seeds foreign entries and asserts they survive.
- **I5 — Fail loud, fail closed, no partial write.** If a target file exists but is malformed JSON, **refuse** and report ("couldn't parse `<path>`; nothing changed") — never clobber. **Both config files are parsed + shape-validated before EITHER is written**, so a malformed/oddly-shaped second file leaves the first byte-unchanged (no partial write, no lying "nothing changed"). If Claude Code isn't detected, say so and don't scatter stray files.
- **I8 — Injection-safe hook.** The SessionStart hook `command` is shell-executed by Claude Code, so the binary path is POSIX **single-quote-escaped** (single quotes neutralize `"`/`$`/backtick/`\`) — a path metacharacter (or a hostile `AIR_MEMORY_MCP_BIN`) cannot inject a command. The mcpServers `command` is spawned as argv, so it needs no escaping.
- **I6 — Pure logic over injected paths.** The merge/detect/atomic-write logic is plain functions taking file paths (and the binary + socket paths) as arguments — hermetic, temp-dir tested, no process-global env or real `$HOME`. The Tauri command is a thin wrapper that resolves the real paths and calls them (mirrors `bossclawd-paths`'s `*_from` split).
- **I7 — Agent-adapter seam.** Claude Code is one adapter today; Codex drops in as a second adapter (TOML) with **no** change to the Tauri command layer or the shared atomic-write/status types, and **no** dead code shipped now (just a labeled seam).

## 5. Architecture

The running desktop app already knows, in Rust, its own exe dir (→ our bundled `air-memory-mcp`) and its data dir (→ the daemon socket). A **Connect** click resolves those plus the Claude Code config paths from `$HOME`, then merges our two entries in and atomically writes each file. **Disconnect** reverses it. The React Settings panel is a thin status + button surface over three Tauri commands.

```
Settings ▸ Integrations (React)
   │  invoke: integrations_status / _connect_claude_code / _disconnect_claude_code
   ▼
Tauri command layer (thin) ── resolves real paths ──▶ integrations::claude_code (pure)
   │  exe dir → resolve_memory_bin_path                     merge / detect / atomic_write(0600)
   │  data dir → bossclawd_paths::resolve_socket_path       │
   │  $HOME   → ~/.claude.json, ~/.claude/settings.json     ▼
   ▼                                          ~/.claude.json   (mcpServers += air-memory)
air-memory-mcp bundled sibling               ~/.claude/settings.json (SessionStart += nudge group)
(externalBin, next to the app exe)                  │
                                                    ▼  (next Claude Code session)
                          Claude Code ──stdio──▶ air-memory-mcp ──socket(MemoryClient)──▶ bossclawd
```

## 6. Units (each: purpose · interface · dependencies)

- **U1 — Bundle + locate the adapter binary** (`tauri.bundle.conf.json`, the build/copy step, a new resolver in `daemon.rs` or a sibling `integrations` helper). *Purpose:* ship `air-memory-mcp` next to the app exe and resolve its absolute path at runtime. *Interface:* `externalBin += "binaries/air-memory-mcp"`; the artifact-copy step gains a twin line for `air-memory-mcp-<triple>`; `resolve_memory_bin_path(current_exe) -> PathBuf` (env `AIR_MEMORY_MCP_BIN` override → sibling → dev fallback; a direct copy of `resolve_bin_path` with `BIN_NAME = "air-memory-mcp"`). *Depends on:* existing bundling.
- **U2 — The Claude Code config-writer (pure core)** (`src-tauri/src/integrations/claude_code.rs` + shared helpers). *Purpose:* detect / connect / disconnect against injected paths, honoring I1–I5. *Interface:* `detect(claude_json, settings_json) -> Status`; `connect(paths, binary: &Path, socket: &Path) -> Result<ConnectReport>`; `disconnect(paths) -> Result<()>`; plus `atomic_write_0600(path, bytes)` and JSON merge/prune helpers. All hermetic (temp fixtures). *Depends on:* `serde_json` (already in-tree), std `fs` + `PermissionsExt`. No new deps.
- **U3 — The SessionStart nudge** (a `nudge` subcommand on `crates/air-memory-mcp`). *Purpose:* give the hook a cross-platform, quoting-free command to run. *Interface:* `air-memory-mcp nudge` prints a `NUDGE_TEXT` const to stdout and exits 0 (no server, no socket, no network); all other invocations run the stdio server as today. *Depends on:* the adapter's `main`.
- **U4 — Tauri command surface** (`src-tauri/src/commands/integrations.rs` or a top-level module; registered in `generate_handler!`). *Purpose:* resolve real paths and call U2. *Interface:* `integrations_status() -> IntegrationsStatusDto`, `integrations_connect_claude_code() -> ConnectReportDto`, `integrations_disconnect_claude_code() -> ()`. Resolves the binary (U1, from `current_exe`), the socket (`bossclawd_paths`), and `~/.claude.json` / `~/.claude/settings.json` (from `$HOME`). *Depends on:* U1, U2.
- **U5 — The Settings Integrations panel** (`apps/desktop/src/settings/IntegrationsPanel.tsx` + `api/tauri.ts` wrappers, rendered by `AirSettings.tsx`). *Purpose:* show per-agent status + a Connect/Disconnect button. *Interface:* reads `integrations_status` on mount, buttons call connect/disconnect then refresh; states = Not found (disabled + "install Claude Code") / Not connected (Connect) / Connected (Disconnect). Reuses `Card`/`Button`; no hardcoded colors (repo rule). *Depends on:* U4.
- **U6 — Docs + end-to-end proof** (`crates/air-memory-mcp/README.md` update; an integration test). *Purpose:* replace the manual snippet with the one-click story and prove the round-trip. *Interface:* a test that seeds a `~/.claude.json` with a foreign mcpServers key **and** a `settings.json` with a foreign SessionStart hook (the exact machine state), then asserts connect → detect(Connected) → disconnect → detect(NotConnected) leaves the foreign entries intact and perms `0600` throughout. *Depends on:* U1–U5.

## 7. Data flow (the happy path — Connect)

1. User opens **Settings ▸ Integrations**, sees Claude Code = *Not connected*, clicks **Connect**.
2. Frontend calls `integrations_connect_claude_code()`.
3. Backend resolves: our binary (`resolve_memory_bin_path(current_exe)`), the socket (`bossclawd_paths::resolve_socket_path(resolve_data_dir())`), and the two config paths from `$HOME`.
4. Parse + validate BOTH files first (refuse on malformed, I5 — no partial write), merge in memory (I1/I3), then `atomic_write_0600` both (I2).
   - `~/.claude.json`: `mcpServers["air-memory"] = { type:"stdio", command:<binary>, args:[], env:{ BOSSCLAWD_SOCKET:<socket> } }` (the `command` is spawned as argv, not shell).
   - `~/.claude/settings.json`: append to `hooks.SessionStart` the group `{ hooks:[{ type:"command", command:"<single-quote-escaped binary> nudge", timeout:5 }] }` — the hook string IS shell-run, so the path is single-quote-escaped (I8, injection-safe).
5. Returns a `ConnectReport` (per-file: written / already-present / created-fresh).
6. Panel re-reads status → **Connected**. (Takes effect on Claude Code's *next* session — see the known limitation.)

## 8. Error handling

| Case | Behavior |
|---|---|
| `~/.claude.json` or `settings.json` is malformed JSON | Refuse; report "couldn't parse `<path>`; nothing changed". No clobber (I5). |
| Claude Code not detected (neither `~/.claude.json` nor `~/.claude/` exists) | Status `ClaudeCodeNotFound`; Connect disabled with "Install Claude Code first" — don't create stray files. |
| Config file absent but Claude Code detected (e.g. only `~/.claude/` exists) | Create the missing file fresh, `0600`, containing only our entry. |
| Malformed / odd-shaped 2nd file (valid 1st) | Both files are parsed + validated BEFORE either is written, so the 1st is left byte-unchanged and the error is honest (no partial write, I5). Only a mid-write I/O failure on the 2nd file is a residual partial case (rare; two files can't be truly atomic) — a re-click self-heals (idempotent). |
| Disconnect when not connected | No-op success (report "nothing to remove"). |
| Claude Code is *running* at click time | New server/hook takes effect on its **next** session. Real race: Claude Code writes `~/.claude.json` frequently, so a live session can overwrite our merge, or our whole-file rewrite can roll back state it wrote between our read and our rename. `preserve_order` + a minimal footprint shrink the diff but not the window. Mitigation: the panel + README tell the user to **quit Claude Code before connecting**; effect is next-session regardless. (A file lock is out of scope.) |
| AIR Agent moved / uninstalled after Connect | The written config points at the app's bundled binary; if the app is gone, Claude Code's `air-memory` server + the SessionStart hook fail to spawn each session. Mitigation: panel + README say **Disconnect before moving/uninstalling**; auto-heal is out of scope (SP3+). |
| Daemon down when a future Claude Code session calls a tool | Not SP2's concern — SP1's adapter already returns a clean "memory service unavailable". |

## 9. Testing strategy

- **Unit (pure, hermetic temp dirs — the bulk):** merge preserves a foreign `mcpServers` key **and** a foreign `SessionStart` hook; double-connect is idempotent (one key, one group); disconnect removes only ours (foreign entries survive); perms are `0600` after connect *and* after a fresh create; malformed-existing refuses without clobber; absent-file fresh-create; `detect` returns the right state across NotFound / NotConnected / Connected. Assert the temp file never briefly exists world-readable (create-with-mode, not chmod-after).
- **Nudge (U3):** `air-memory-mcp nudge` prints exactly `NUDGE_TEXT` and exits 0; a normal (no-arg) invocation still enters the server path (guarded so the nudge branch can't shadow the server).
- **Frontend (U5, vitest):** panel renders each of the three states; Connect/Disconnect call the correct command and refresh; button disabled in NotFound.
- **End-to-end (U6):** the seeded-foreign-entries round-trip above.
- **Gates:** `cargo build --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` + `cargo test` for touched crates (daemon-crates CI job already covers `air-memory-mcp`); `npm run typecheck` + `eslint` (0 warnings) + `vitest` for the desktop workspace; repo-wide **0 hardcoded colors** in `.tsx`. No keychain access anywhere in SP2 (avoids the macOS-CI keychain flake).

## 10. Out of scope (SP2) / deferred

- **Auto-recall, auto-capture, recall-miss instrumentation** (SP3). The SP2 hook is a static nudge only.
- **Codex build** — a follow-up second adapter (`~/.codex/config.toml`, `[mcp_servers.air-memory]` + its AGENTS.md/nudge equivalent, needs the `toml` crate). SP2 ships only the labeled seam.
- **Per-project `.mcp.json`** and any project-targeting UI.
- **Cryptographic role-proof / capability tokens** (SP1.x "Strict").
- **Hot-applying to a running Claude Code** (documented limitation; effect is next-session; the panel + README say to quit Claude Code before connecting).
- **Auto-cleanup on app move/uninstall** — the written config points at the app's bundled binary; the panel + README say to Disconnect before moving/uninstalling. A version-independent shim / auto-heal is out of scope (SP3+).
- Any change to the app's own daemon connection or to SP1's adapter server behavior (only the additive `nudge` subcommand).

## 11. Open questions to resolve during planning

**Resolved (planning + pre-code architect/critic/security review, 2026-07-10):** config-writer = a `src-tauri/src/integrations/` module (Q1); 3-state status, "Connected" = mcpServers present (Q2); file-presence detection + block-with-hint (Q3); `nudge` subcommand not a script file (Q4); atomic write = **std-only** born-0600 (`tempfile` is dev-only, Q5). Review also locked: `serde_json/preserve_order` (no whole-file reorder), validate-both-before-write (no partial write, I5), single-quote-escaped hook (I8), `0700` `~/.claude`, over-removal-safe removal marker (I4), adapter-binary existence check, honest running-Claude/uninstall copy. The original questions are recorded below.


1. **Config-writer home:** a `src-tauri/src/integrations/` module (simplest; Codex is also desktop-driven) vs a small shared crate. Decide by real reuse — default to the module.
2. **`Status` granularity:** three states (`NotFound`/`NotConnected`/`Connected`) vs adding `PartiallyConnected` for a half-written prior run. How "Connected" is defined — mcpServers entry present (essential) vs both entries present. Default: 3 states, "Connected" = mcpServers entry present, connect always ensures both; the panel can sub-note a missing hook.
3. **Claude Code detection heuristic:** presence of `~/.claude.json` / `~/.claude/` vs `claude` on `PATH`; and whether Connect is *blocked* when not detected vs *create-anyway*. Default: file-presence detection + block-with-hint.
4. **Nudge home:** the `air-memory-mcp nudge` subcommand (proposed — one artifact, no quoting, cross-platform) vs a written script file (another file to manage/remove). Confirm the subcommand.
5. **Atomic-write helper:** std-only (`OpenOptions.mode(0o600)` + `rename`) vs a vetted crate (`tempfile`/`atomicwrites`). Default: std, zero new deps.
6. **Explicitly rejected (listed to prevent scope creep):** writing into the user's `CLAUDE.md`, or shelling out to `claude mcp add`. The SessionStart hook + direct write are the chosen mechanisms.

## 12. Sequencing / branch

- **Branch off SP1.** SP2 needs SP1's adapter binary + protocol, and SP1 is still an open PR (#76, not merged). Branch `feat-memory-hub-sp2-claude-code-integration` off `feat-memory-hub-sp1-code-loop`; open the SP2 PR **stacked** (base = #76) — mirrors the prior stacked-PR pattern (SP3 #41 on #43). Once Peter merges #76 to `main`, rebase the SP2 PR onto `main`. This also satisfies the domain rule "do NOT merge SP1 without SP2 planned + PR open."
- **TDD per unit, subagent-driven execution** (fresh Opus per task; spec→quality review each), matching SP1.
- **Second opinion before code (Peter's standing ask):** the implementation plan goes through an **architect + critic + a dedicated security review** *before* any build — U2 (the config-writer that mutates the user's real `$HOME` files) is the security-sensitive unit; a bug there could clobber a foreign hook or leak a world-readable file. Whole-branch review before the PR flips to ready.
