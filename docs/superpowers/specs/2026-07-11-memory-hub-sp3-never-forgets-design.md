# Memory Hub SP3 — "Never Forgets": Session Capture, Snapshot, Telemetry, Forget

**Status: DESIGN — approved by Peter 2026-07-11 (pending spec review)**
**Track:** Phase 2 (M1b Code loop), rung-2 completion per the North Star
(`air/memory-strategy-2026-07-03-beat-the-stack`). SP1 = the safe read+write loop
(#76). SP2 = one-click Claude Code integration (#77). SP3 = this document.
**Evidence base:** 106-agent competitive audit + doc-verified Claude Code mechanics,
2026-07-10 (GBrain `air/sp3-competitive-audit-2026-07-10`). Every architectural
choice below traces to a verified finding or an explicit Peter decision.

---

## 1. Product goal

Finish the North Star sentence for Claude Code: **install AIR Agent, connect once,
and your coding agent just never forgets** — through session ends, crashes, and
context compaction — with memory that is durable-until-commanded, readable,
searchable, on-device, and honest about deletion.

Five user-visible capabilities:

1. **Session archive (the shoebox).** Every Claude Code session is preserved as
   readable Markdown, permanently (beyond Claude Code's ~30-day transcript
   retention), locally, signed.
2. **Backfill.** At Connect time, the last ~30 days of existing sessions are
   imported (consent-gated, default ON): install → it *already* remembers.
3. **Session-start snapshot + compaction insurance.** Each new session opens with
   a lean, project-aware orientation; after every context compaction the agent is
   re-oriented with what *this session* had established.
4. **Memory browser.** A Brain-tab library of sessions and notes with search,
   view, and **Delete** (honest tombstone). Brain becomes the app's landing view.
5. **Recall-miss telemetry.** Every recall records whether it found anything;
   misses become the tuning signal for the retrieval floor.

### Non-goals (explicit, evidence-backed)

- **No LLM transcript extraction/compression.** Rejected twice on 2026 evidence
  (LongMemEval-V2: agent-over-raw-files 72.5% vs extraction/RAG pipelines ≤48.5%;
  Mem0's extraction router collapsed 68.3%→43.6% on adversarial forgetting).
  Rendering is deterministic.
- **No archive full-text indexing by default.** Deferred behind the measurement
  harness (LME-V2 frames indexing as a latency optimization, not a recall
  necessity). Session *titles/metadata* are recallable; bodies are not (yet).
- **No PreCompact or per-tool-call hooks.** The on-disk transcript is append-only
  and survives compaction (doc-verified); reading the source of truth beats
  re-recording it with 12 heavy hooks (AgentMemory's documented warts: token burn,
  OOM, silent failures).
- **No auto-recall injection.** Orientation-only snapshot; recall stays a tool the
  agent calls (evidence-locked in the Phase 2 strategy).
- **Not in SP3:** the Obsidian-style memory graph (SP4), Codex CLI integration
  (next SP), consolidation/decay tiers, cross-machine sync, `.mcpb` packaging,
  rung-3 conflict resolution.

---

## 2. Architecture overview

```
Claude Code                         bossclawd (daemon)                 disk
───────────                         ──────────────────                 ────
SessionEnd hook ──poke(ms)──►  CaptureNotify ─┐
                                              ├─► capture::render ──► <data>/sessions/<sid>.md
sweeper (interval, boot) ──scan──► unarchived ┘        │
                                                       └─► engine: session_captured event
SessionStart hook ──(stdin: source,sid)──► Snapshot ──► lean orientation text
recall (MCP/app) ──────────────────────► Recall ──► telemetry line (hits, top score)
Memory browser (app) ───────────────────► ListSessions / GetSession / DeleteSession /
                                          ListNotes / SupersedeNote / RecallStats
```

Principles carried from SP1/SP2: hooks are millisecond pokes (Claude Code kills
hooks doing real work on exit — issue #41577); **only the daemon writes** capture
artifacts (parallel-session-safe by construction — each session has a unique
`session_id` and transcript path, doc-verified); every new op passes the
fail-closed `Role::allows` allowlist; destructive ops are App-only.

---

## 3. Wire protocol (`bossclawd-proto`)

New `Request` variants (all with `onboarded: bool`, matching existing style):

| Request | Response | Roles |
|---|---|---|
| `CaptureNotify { session_id, transcript_path }` | `Ok` | App, **MemoryClient** |
| `Snapshot { project, source, session_id: Option<String> }` | `Snapshot(String)` | App, **MemoryClient** |
| `ListSessions {}` | `ListSessions(Vec<SessionSummaryWire>)` | App only |
| `GetSession { session_id }` | `Session(SessionDetailWire)` | App only |
| `DeleteSession { session_id }` | `Ok` | App only |
| `ListNotes {}` | `ListNotes(Vec<NoteWire>)` | App only |
| `SupersedeNote { event_id, text }` | `Remember(String)` (new event id) | App only |
| `RecallStats {}` | `RecallStats(RecallStatsWire)` | App only |
| `SetCaptureEnabled { enabled }` | `Ok` | App only |
| `CaptureEnabled {}` | `CaptureEnabled(bool)` | App only |

`SetCaptureEnabled`/`CaptureEnabled` mirror the `SetMandatesEnabled`/
`MandatesEnabled` precedent exactly (config event in the engine log; the sweeper
re-reads the gate each wake, like the evolve scheduler).

- `Role::allows` (proto `lib.rs`) extends the `MemoryClient` arm to exactly
  `Recall | Remember | CaptureNotify | Snapshot`. **Delete, listing, and
  supersede are NOT reachable from MCP/hooks** — a hostile prompt cannot command
  the eraser or enumerate the library. `override_onboarding_for_guest`
  (`server.rs`) gains the two new guest ops.
- `SessionSummaryWire { session_id, title, project, tool, started_at, ended_at,
  approx_bytes }`. `SessionDetailWire { summary: SessionSummaryWire, markdown:
  String }`. `NoteWire { event_id, text, created_at, superseded_by:
  Option<String> }`. `RecallStatsWire { total: u64, misses: u64, recent_misses:
  Vec<RecallMissWire> }`, `RecallMissWire { query, at }`.
- `PROTO_VERSION` bumps 1→2 (new variants; adapter and daemon ship together, and
  the handshake already fails loudly on mismatch).

## 4. Daemon: capture module (`crates/bossclawd/src/capture/`)

New module, three parts, all pure-testable cores:

**4a. Renderer (`render.rs`).** Deterministic JSONL→Markdown. Parses Claude Code
transcript lines **defensively** (the per-line schema is officially unpublished):
user prompts, assistant text, tool calls as one-liners (`▸ Bash: <description>`),
skip unknown/queue/hook noise, spilled tool-results referenced not inlined.
Output: front-matter (session_id, project/cwd, tool=`claude-code`, started/ended,
source transcript sha256) + readable body. Title = first user prompt, truncated.
No LLM anywhere. Fixture-driven tests (memharness already ships
`tests/fixtures/transcript_synthetic.jsonl`; add real-shape fixtures).

**4b. Store.** Rendered Markdown at `<data_dir>/sessions/<session_id>.md`
(**`sessions/`, not `archive/`** — `archive.db` is taken by the inbox WAL).
Alongside, one signed engine event per capture, event type
**`session_captured`** (new constant in `bossclaw-core/src/graph.rs`), content =
`{ text: "<title> — <project> (<date>)", origin: EXTERNAL_ORIGIN, session_id,
path, sha256, project, tool, started_at, ended_at }`. The small `text` makes the
session *discoverable* via recall (title-level pointer) without indexing the
body — the deliberate shoebox/indexing split. Re-capture of a grown transcript
follows the ingest idempotency pattern (`ingest.rs`): same sha → dedup/no-op;
changed → `SUPERSEDE_EVENT_TYPE` pair + rewrite the `.md`. External-tainted by
construction: recallable, never feeds evolve (existing `exclude_files`-class
gates extend to `session_captured`), never auto-applied.

**4c. Sweeper (`sweeper.rs`) + notify.** `CaptureNotify` validates then enqueues:
canonicalized `transcript_path` must live under the Claude projects root
(`~/.claude/projects`, env-overridable for tests) and end in `.jsonl`; anything
else → `Rejected`. The sweeper mirrors the evolve scheduler pattern
(`scheduler.rs`: `tokio::spawn` + interval + `MissedTickBehavior::Skip` +
re-read gates each wake; spawned as a sibling in `main.rs` step 6). Each wake:
scan projects root for `.jsonl` with quiet mtime (≥ `QUIET_SECS`, const 600) whose
(session_id, sha) isn't already captured → render + store. Gates: onboarded AND
`capture_enabled` config (default ON, set by Connect flow consent, toggle in
Integrations panel). The sweeper IS the durability story: crash, SIGKILL, power
loss, missed hook — captured within one sweep, up to Claude Code's retention
window. It is also the **backfill** engine: first run after Connect (with backfill
consent) simply finds ~30 days of quiet transcripts and archives them. Zero extra
machinery.

## 5. Snapshot + compaction insurance

New engine/daemon read op `Snapshot { project, source, session_id }`:

- `source ∈ {startup, resume, clear, compact}` (Claude Code's own hook vocabulary,
  passed through from hook stdin).
- **startup/clear/resume:** project-scoped orientation — most recent
  `session_captured` titles for this repo (N=5), most recent external notes
  (N=5), one affordance line ("Full history is searchable via `recall`").
- **compact (the insurance):** session-scoped re-orientation — the daemon renders
  the *current* session's transcript-so-far (it's on disk mid-session) and
  returns a tight **deterministic** digest (I5 applies — no LLM): session title,
  the last N user prompts, file paths seen in tool one-liners, and the tail of
  the last assistant message, plus the recall affordance. Injected into the
  freshly-compacted context via the hook's stdout. We cannot re-stuff a full
  window (physics); we hand back the luggage tag for the storage locker.
- **Hard size budget: `SNAPSHOT_MAX_BYTES = 4096`** (well under Claude Code's own
  25 KB native-memory injection), truncation priority: notes → sessions →
  affordance. Deterministic assembly, no LLM.

Adapter change (`air-memory-mcp`): the existing `nudge` subcommand now reads the
SessionStart hook JSON from stdin (`source`, `session_id`, `cwd`), calls
`Snapshot` with a short timeout, prints the result; **on any failure it prints
the current static `NUDGE_TEXT` and exits 0** (session start can never break;
fail-quiet). Because SP2 wrote the hook with **no matcher**, it already fires on
all four sources — existing connections upgrade behavior with zero config
migration.

## 6. Integrations (SP2 config-writer extension)

`connect()` additionally writes a **SessionEnd** hook group into
`~/.claude/settings.json`: `{"hooks":[{"type":"command","command":"'<binary>'
capture-notify","timeout":5}]}` — same `sh_single_quote` discipline, same
validate-both-before-write, same atomic-0600. Removal marker = command contains
BOTH `air-memory-mcp` AND `capture-notify` (mirrors the nudge marker rule).
`disconnect()` removes both hook kinds + the mcpServers entry. Connect is already
idempotent → existing SP2 users heal by clicking Connect once.

The `capture-notify` subcommand: read hook JSON from stdin (`session_id`,
`transcript_path`), one `CaptureNotify` round-trip with a short timeout, exit 0
regardless (the sweeper is the guarantee; the poke is an optimization).

**Connect flow consent (frontend):** the Connect dialog gains one disclosed
default-ON checkbox: *"Keep a local, private copy of your Claude Code sessions in
AIR memory — including your recent sessions (~30 days, N found)."* The N count is
computed app-side by the integrations command (a pure filesystem count of `.jsonl`
files under the Claude projects root — no daemon op needed). Unchecking sets
`capture_enabled=false` via `SetCaptureEnabled` (connect still wires
recall/remember/snapshot).
Disclosure copy must say **"processed on this Mac"** (audit finding: the field's
"local" products default to cloud processing; ours genuinely doesn't).

## 7. Forget (minimal, honest)

- **Delete session (App-only op):** removes `<data_dir>/sessions/<sid>.md`
  (content destroyed) and appends a signed **`session_deleted`** event
  `{ session_id, deleted_at }`. Listing, recall, and snapshot exclude deleted
  sessions (tombstone filter + `VectorIndex::remove` for the title vector). The
  chain honestly shows "captured, then deleted by owner" — a visible stub, never
  a silent hole.
- **Honesty note (disclosed in UI):** the append-only encrypted log retains the
  session *title* inside the prior `session_captured` event; the body is
  destroyed. Cryptographic erasure of chain history is rung-5 territory.
- **Supersede note (App-only op):** appends a `SUPERSEDE_EVENT_TYPE` pair —
  new corrected note event + supersede link (extends the existing file-ingest
  supersede pattern to `memory` events; the engine's "no note supersede yet" gap
  the recon flagged). Recall filters superseded notes.
- **Tests as the gate:** commanded-forgetting integration tests on the real
  hermetic daemon (extend `bossclawd/tests/memory_client_loop.rs` pattern):
  remember→supersede→assert old not recalled; capture→delete→assert not listed,
  not recalled, file gone, tombstone present; MemoryClient attempting
  DeleteSession/SupersedeNote → `NotPermitted`. A `memharness forget-check`
  scripted scenario runs the same cases against a hermetic engine for the
  measurement record.

## 8. Recall-miss telemetry

In the daemon's `Recall` arm: after each recall, append one JSON line to
`<data_dir>/telemetry/recall.jsonl` — `{ at, query, hits, top_score }` (local
only, size-capped with simple rotation at 5 MB; disclosed in settings copy).
`RecallStats` returns totals + last 20 misses. Surfaced read-only in the Memory
browser ("N recalls, M found nothing — recent misses: …"). This is the rung-1/2
tuning signal the North Star ordered (baseline now, re-measure after multilingual
and any future chunking work).

## 9. Frontend (Brain tab becomes home)

- **Landing:** `App.tsx` default view `identity` → `memory` (one line; Brain is
  the product).
- **BrainPanel:** new *first* sub-tab **"Library"** (existing "Search & Evolve",
  "Review", "Mandates" remain). Library = search bar (client-side filter over
  `ListSessions`+`ListNotes`, plus a "search memory" action running `recall`),
  session list (title, project, date) → View (rendered Markdown in a reader
  pane) / Delete (confirm dialog quoting the honesty note), notes list →
  Supersede (edit-in-place writes the corrected note), and the RecallStats strip.
- New Tauri commands (`commands/engine.rs` pattern, thin wrappers over tested
  cores): `engine_list_sessions`, `engine_get_session`, `engine_delete_session`,
  `engine_list_notes`, `engine_supersede_note`, `engine_recall_stats`; TS
  wrappers in `api/engine.ts`; ⌘K palette gains the Library as a search group
  (reuses `ListSessions`).
- Styling: tokens only (0 hardcoded colors), existing panel idioms.

## 10. Invariants

- **I1 — Hooks never block, never break.** Both subcommands exit ≤ timeout with
  exit 0 on every path; snapshot failure degrades to the static nudge.
- **I2 — Only the daemon writes** sessions/, telemetry/, and capture events.
  Hook/MCP surfaces are notify+read only.
- **I3 — Fail-closed roles.** MemoryClient = exactly
  {Recall, Remember, CaptureNotify, Snapshot}. Destructive ops are App-only;
  `Role::allows` stays a positive allowlist.
- **I4 — CaptureNotify is untrusted input.** Paths canonicalized and confined to
  the Claude projects root, `.jsonl` only; everything else `Rejected`.
- **I5 — Deterministic pipeline.** No LLM in render, snapshot, title, or
  telemetry paths.
- **I6 — Captured content is external-tainted** (`origin: EXTERNAL_ORIGIN`):
  recallable, excluded from evolve reasoner context, never auto-applied.
- **I7 — Deletion is honest.** Content destroyed + signed tombstone; deleted
  sessions invisible to list/recall/snapshot; retention of the title in the
  chain is disclosed.
- **I8 — Snapshot ≤ 4096 bytes**, always, by construction.
- **I9 — Idempotent capture.** Same transcript sha → no-op; grown transcript →
  supersede pair; re-running backfill/sweeps never duplicates.
- **I10 — Non-connected users unaffected.** No Claude Code connection → no
  hooks written, no sweeper activity beyond a gated no-op tick, no new files or
  directories created.

## 11. Testing

TDD per task (subagent-driven, per SP1/SP2 process). Highlights: renderer
fixture suite (real-shape + synthetic + hostile/unknown-line JSONL); sweeper
unit tests over a fake projects root (quiet-mtime, dedup, supersede, gate-off);
real-daemon loop tests for capture→recall-title, snapshot budget, forget cases,
role denials (extend `memory_client_loop.rs`); config-writer tests for the
SessionEnd hook add/remove + idempotent heal (extend SP2's suites); frontend
vitest for Library (list/view/delete/supersede/stats + landing default); e2e:
fixture transcript → capture-notify → rendered file + event → recall finds
title → snapshot mentions it → delete → all traces gone. Security review
(adversarial) required for: CaptureNotify path validation, the forget/tombstone
semantics, and the role-allowlist extension.

## 12. Deferred (tracked, not in SP3)

Memory graph view (SP4 — link derivation + force canvas). Codex CLI adapter.
Archive body indexing behind the harness A/B flag. `.mcpb` Claude Desktop
packaging. Signed-export bundle format (rung 5 — align with or consciously
diverge from Portable Agent Memory, arXiv 2605.11032; SP3's provenance fields in
`session_captured` keep the schema migration-free). Consolidation/decay.
Cross-machine sync. Telemetry-driven auto-tuning.
