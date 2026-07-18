# Rung 4 — R4-A: the sleep loop (miss-driven reflection) — Implementation Plan

> For agentic workers: REQUIRED SUB-SKILL: superpowers:subagent-driven-development

**Goal.** Give the daemon a fourth background loop — a **night cleaner** — that, when the room is quiet,
(1) repairs the specific recall gaps the owner actually hit (consuming the SP3 recall-miss telemetry that today
has no *acting* consumer) and (2) refreshes dossiers whose cited sources went stale (the aftermath of a Rung-3
retire). It **adds freely** (dossier revisions are append-only supersedes, exactly the power evolve already
holds) and mints nothing. A wrong reflection costs a *superseded* dossier revision (recoverable from the log)
— never a lost memory (invariant I1). Implements the CONVERGED design
`docs/superpowers/specs/2026-07-19-rung4-reflection-design.md` (Rev 3, folded through two independent review
rounds); **R4-A scope only** — §1/§2/§4/§5/§6/§7. R4-B (entity merge) is OUT of this plan. Reflection is
`ConfigFlag::Reflect`-gated, default-CLOSED, `prime_switches`-forced-off — R4-A ships **dormant** with exactly
one deliberate, named, Reflect-independent behavior change (I3, below).

**Architecture.** Four thin layers, mirroring the shipped evolve / capture / conflict siblings.
- **`bossclaw-core` (the brain).** A new portable `reflect` module (the single documented consts block + the
  pure report/outcome data types + `normalized_query_key`); a shared **gather-path lineage exclusion** inside
  `gather_fact_set` (the load-bearing §2.3 prerequisite, Reflect-INDEPENDENT); a behavior-preserving
  extraction of the per-topic compose body into `refresh_topic_page`; the `reflect_miss_backlog` +
  `reflect_counters` + `reflect_cursor` re-derivable progress tables and accessors; the read-only query→topic
  bridge `reflect_topics_for_query` (over the real `entity_search`); the `attempt_miss` pipeline; the
  `refresh_stale_pages` tidy job; the `reflect_once` orchestrator; `ConfigFlag::Reflect` + its sticky
  setter/getter; and a cheap `newest_memory_activity_at` reader for the quiet predicate.
- **`bossclawd-proto` (the wire).** Two additive App-only `Request` variants (`SetReflectEnabled` +
  `ReflectEnabled`), one additive `Response::ReflectEnabled(bool)` (the write reuses `Response::Ok`), and
  `Role::allows` **UNCHANGED** — reflect is App-only by construction (the six-ops guest allowlist gains two
  entries in its `no` set). `PROTO_VERSION` stays `1` (additive variants only).
- **`bossclawd` (the daemon).** The `EngineHandle::reflect_once` wrapper (dedicated `reflect_lock`, the
  `cloud_consent_ok` chokepoint, the telemetry miss-ring drain, the post-tick rebuild set, `ReflectTelemetry`);
  the pure `decide_reflect` gate + the `reflect` sweeper (a hybrid of the conflict-sweeper spawn shape and a
  capture-style pure decide fn); `reflect_enabled_or_false`; the `prime_switches` force-off; the
  `SetReflectEnabled`/`ReflectEnabled` dispatch arms; and the never-truncated snapshot digest line.
- **`air_agent_desktop` (the settings surface).** One toggle in a dedicated Reflect settings block (command +
  registration + `Engine`/`EngineClient` methods + TS invoke + React panel + tests), and the desktop half of
  the `5 → 6` fresh-brain trip-wire.
- **`memharness` (dev-only, never ships).** The reflection non-regression **gate** (page-hit resolution so a
  reflected brain's dossier hits do not abort the run; a run-to-quiescence driver; `recall_regressed` as the
  SHIP gate; union-coverage as a separate REPORTED metric) plus the two REPORTED evidence probes (held-out
  generalization; dossier-vs-source blind A/B under the existing judge-trust ladder).

**R4-A boundary (do not cross).** NO recall re-ranking in favor of pages (dossier-primacy is future, decided on
§5.3(e) evidence). NO entity merge / duplicate detection / `merge_proposal` (that is R4-B). NO reflection-activity
review surface (R4-B ships the read-only listing; R4-A's visibility is the scoreboard + the digest counts + the
signed log, matching evolve's existing dossier posture — I-vis). NO semantic miss dedup (v1 key = trimmed
casefold hash). NO parsing of rotated `recall.jsonl` history (the backlog seeds from the live ring only). NO
`Wide` dossier reach; NO decay/archive tiers. NO Codex parity; NO cross-machine sync.

**Tech Stack.** Rust (workspace crates `bossclaw-core`, `bossclawd-proto`, `bossclawd`, `memharness`,
`air_agent_desktop`) + TypeScript/React (`apps/desktop/src`); `rusqlite`/SQLCipher; `serde_json`; `chrono`
(RFC3339 event `ts`); tokio (`spawn_blocking` at the engine boundary, `tokio::time::interval` +
`MissedTickBehavior::Skip` in the sweeper). Tests are Rust `#[test]`/`#[tokio::test]` matching each crate's
existing style (`tempfile::tempdir` + `open_log` in core; `MockEmbedder`/`ScriptedReasoner` fixtures; serde
round-trips in proto) and vitest for the desktop toggle. **All cargo commands are SYNCHRONOUS / foreground**
(never backgrounded) so each red→green transition is observed before the next step. The controller does NOT
run cargo during plan review; the commands below are for the executor.

**`#[cfg(unix)]` discipline.** Reflection does NOT touch the `#[cfg(unix)]` conflict-proposal subsystem — it is
built on portable seams (`gather_fact_set`, `summarize_topics`, `entity_search`, `recall`, `emit_page`,
`fold_sessions`, `all_entities`, `current_pages`, the `ConfigFlag` family — all ungated). Therefore **the core
`reflect` module and every new `bossclaw-core` reflect method + table + test is PORTABLE (ungated)** — same
posture as `evolve.rs`/`summarize.rs`. On the daemon side, `engine`/`server`/`telemetry`/`conflict` are already
`#[cfg(unix)]` modules (`bossclawd/src/lib.rs:36-56`), so the new daemon `reflect` sweeper module is declared
`#[cfg(unix)] pub mod reflect;` beside `conflict` and its wrappers/dispatch/snapshot code inherit the gate — no
per-fn gates. The desktop toggle commands are `#[cfg(unix)]` like `integrations_set_capture_enabled`
(`apps/desktop/src-tauri/src/main.rs:246-249`). `memharness` is cross-platform (dev-only).

### As-built anchors — VERIFIED against `feat-rung4-reflection` `1b80874` (2026-07-19)

Every function a test calls below already exists at these lines (or is created by an earlier task). Each line
was READ during planning (line numbers drift from the spec's §8, which was pinned to `main 7fb1e8a`;
these are the CURRENT branch values). Re-grep before editing if the file has drifted.

| Symbol | File | Line (verified) |
| --- | --- | --- |
| `ConfigFlag` enum / `key()` match / `explicitly_set` | `bossclaw-core/log.rs` | `:279` / `:302` / `:7472` |
| `CONFLICT_DETECT_ENABLED_KEY` / `conflict_detect_enabled` / `set_conflict_detect_enabled` (T1 template) | `bossclaw-core/log.rs` | `:272` / `:7639` / `:7650` |
| `latest_config_value` (fail-closed getter helper) | `bossclaw-core/log.rs` | `:7391` |
| `gather_fact_set` (builds `lineage` at `:8087-8099`) | `bossclaw-core/log.rs` | `:8076` |
| `fact_texts_for_ids` (ONLY caller = `gather_fact_set:8098`) / `source_ids_of_entity` / `source_ids_of_event` | `bossclaw-core/log.rs` | `:7945` / `:7988` / `:7998` |
| `summarize_topics` (per-topic body `:8120-8207`; idempotency `:8161-8166`; thin-skip `:8131`; D8 `:8194`) | `bossclaw-core/log.rs` | `:8111` |
| `current_page_for_topic` (idempotency key: cited set) | `bossclaw-core/log.rs` | `:8021` |
| `emit_page` (no embedder; non-empty `source_event_ids` required `:2783`) | `bossclaw-core/log.rs` | `:2772` |
| `PAGE_MIN_FACTS`=2 / `MAX_CLAIMS_PER_PAGE`=32 / `SUMMARIZE_SYSTEM` / `FactSet`(`.fact_count`) / `build_compose_prompt` / `compose_schema` / `citation_floor` / `assemble` / `parse_draft` | `bossclaw-core/summarize.rs` | `:26` / `:29` / `:37` / `:53`(`:76`) / `:222` / `:111` / `:253` / `:270` / `:292` |
| `evolve_once` (off-switch `:8277`) / `EvolveReport` (`skipped_disabled:61`) | `log.rs` / `evolve.rs` | `:8270` / `:32` |
| `entity_search` → `Vec<(entity_id, cosine_dist∈[0,2])>` (index keyed by `entity_id` at `:6429`) | `bossclaw-core/log.rs` | `:6576` |
| `resolve_mention` (`1.0 - dist`; `resolve_decision`) / `RESOLVE_HIGH`=0.92 / `RESOLVE_LOW`=0.75 / `GRAPH_CONTEXT_K`=8 | `log.rs` / `extract.rs` | `:6979` / `:19` / `:24` / `:57` |
| `all_entities` → `Vec<Entity>` / `Entity{entity_id,label,aliases,entity_type}` / `rebuild_entity_index` | `log.rs` / `graph.rs` / `log.rs` | `:2669` / `:277` / `:6425` |
| `recall` / `RecallOptions` / `Hit{event_id,score,sources,kind}` | `log.rs` / `recall.rs` / `recall.rs` | `:1790` / `:76` / `:35` |
| `SessionFold{superseded:9531, retired_notes:9539}` / `fold_sessions` / `session_events_ordered` | `bossclaw-core/log.rs` | `:9519` / `:9607` / `:5725` |
| `MEMORY_EVENT_TYPE`"memory" / `PAGE_EVENT_TYPE`"page" / `SESSION_CAPTURED_EVENT_TYPE`"session_captured" | `bossclaw-core/graph.rs` | `:23` / `:30` / `:35` |
| events `ts TEXT`(RFC3339, `Utc::now().to_rfc3339()`); parse idiom `DateTime::parse_from_rfc3339(&e.ts).ok().map(|d| d.timestamp())` | `bossclaw-core/log.rs` | `:749` / `:1171` / `:6816` |
| `ScriptedReasoner::new`/`with_response` (keys SHA-256 of `system \u{1f} prompt`) / `Reasoner::complete_json` | `bossclaw-core/reason.rs` | `:64`/`:70` / `:35` |
| test helpers `seed_topic_directly` / `scripted_both_passes` / `empty_pass_a` (reuse verbatim) | `bossclaw-core/tests/evolve.rs` | `:806` / (near `:833`) / `:798` |
| `scheduler`: `EVOLVE_INTERVAL`=300s / `TickGate` / `decide_tick` / `select_ready` / `spawn` | `bossclawd/engine/scheduler.rs` | `:23` / `:44` / `:53` / `:70` / `:90` |
| `capture::sweeper`: `SWEEP_INTERVAL`=300s / `QUIET_SECS`=600 / `decide_sweep`(pure) / `spawn` / `system_time_to_epoch` | `bossclawd/capture/sweeper.rs` | `:48` / `:54` / `:137` / `:284` / `:414` |
| `conflict::sweeper`: `ConflictSweepReport` / `run_conflict_sweep_once` / `spawn` (primary template) | `bossclawd/conflict/sweeper.rs` | `:12` / `:40` / `:68` |
| daemon `main.rs` sibling spawns (add reflect after `:162`) / `scheduler` import `:40` | `bossclawd/main.rs` | `:158-162` |
| engine lock fields (`evolve_lock:281`,`conflict_lock:284`,`resolve_lock:294`) + `new()` init | `bossclawd/engine/mod.rs` | `:279-303` / `:336-342` |
| `EvolveTelemetry` / `ConflictTelemetry`(template) / `record_tick` / `record_tick_into` | `bossclawd/engine/mod.rs` | `:244` / `:258` / `:1010` / `:1849` |
| `evolve_once` wrapper (lock→consent→ensure_indexed→spawn_blocking{rebuild_entity_index; core; rebuild_indexes; rebuild_graph}→record) | `bossclawd/engine/mod.rs` | `:956` |
| `cloud_consent_ok` / `evolve_enabled_or_false` / `conflict_detect_enabled_or_false`(T2 template) / `capture_enabled` / `set_evolve_enabled` / `is_onboarded_local` | `bossclawd/engine/mod.rs` | `:1692` / `:1040` / `:1052` / `:512` / `:1274` / `:407` |
| `serve_conflict_digest_lines` / `build_digest_lines`(pure) / byte-exact test | `bossclawd/engine/mod.rs` | `:1222` / `:1249` / `:2129` |
| `prime_switches` (5 force-offs) / fresh-brain trip-wire `assert_eq!(st.event_count, 5)` | `bossclawd/engine/mod.rs` | `:564` / `:2237` |
| `dispatch` / `SetCaptureEnabled` arm(`unit_result`→`Response::Ok`) / `CaptureEnabled` arm / `override_onboarding_for_guest`(`_ => None :233`) / `now_unix_secs` | `bossclawd/server.rs` | `:251` / `:492` / `:497` / `:211` / `:568` |
| `snapshot::build` (preamble = `serve_conflict_digest_lines(source)`, `:228`) / `assemble_fence` | `bossclawd/capture/snapshot.rs` | `:207` |
| `telemetry`: `RECENT_MISSES_CAP`=20 / `record`(`is_miss:98`) / `push_recent_miss`(`atomic_write_0600`) / `read_recent_misses`(private) / `stats` / `WRITE_LOCK` / `recent_misses_file` / doc header | `bossclawd/telemetry.rs` | `:40` / `:85` / `:180` / `:171` / `:121` / `:54` / `:216` / `:1,5-7,21-25` |
| proto `PROTO_VERSION`=1 / `Role::allows`(App=true) / `SetCaptureEnabled`(App-only) / `CaptureEnabled` / `Response::Ok` / `Response::CaptureEnabled(bool)` / six-ops test(`no` set) / `proto_version_still_one` | `bossclawd-proto/lib.rs` | `:44` / `:74` / `:250` / `:253` / `:298` / `:361` / `:865` / `:944` |
| `HitWire{hit,text}` / `HitMirror.kind` / `RecallMissWire{query,at}` / `RecallStatsWire` | `proto/lib.rs`,`proto/types.rs` | `:423` / `:337` / `:804` / `:812` |
| daemon-side trip-wire `assert_eq!(s.event_count, 5, ...)` | `bossclawd/tests/roundtrip.rs` | `:173` |
| desktop trip-wire `assert_eq!(st.event_count, 5, ...)` / `set_capture_enabled` / `capture_enabled` / round-trip test | `apps/desktop/src-tauri/src/engine/client.rs` | `:973` / `:436` / `:447` / `:1272` |
| desktop `Engine::set_capture_enabled`/`capture_enabled` / command `integrations_set_capture_enabled`(`:103`)+`integrations_capture_enabled`(`:121`) / registration | `desktop engine/mod.rs`,`commands/integrations.rs`,`main.rs` | `:502` / `:103` / `:246-249` |
| desktop TS `setCaptureEnabled`/`captureEnabled` + test / React `IntegrationsPanel`(state`:34`,handler`:87`,JSX`:135`) + test / mount `AirSettings.tsx:22` | `apps/desktop/src/...` | `api/integrations.ts:47,54` / `api/integrations.test.ts:37` / `settings/IntegrationsPanel.tsx` / `IntegrationsPanel.test.tsx:81` |
| memharness `RetrievedHit{page_id,snippet}` / `gold_rank` / `dedup_by_page` / `map_hits`(loud call `:106`) | `memharness/arms.rs` | `:12` / `:18` / `:24` / `:98` |
| memharness `PageResolver`(`by_event`) / `from_file_records` / `page_id_of`(fail-loud `anyhow::Error`) | `memharness/resolve.rs` | `:20` / `:28` / `:53` |
| memharness `run_queries` / `QueryCase{text,lang,source,gold_page_id}` / `RunConfig` / `CaseResult` | `memharness/run.rs` | `:139` / `:31` / `:44` / `:53` |
| memharness `recall_regressed` / `SegmentComparison` / `REGRESSION_ALPHA`=0.05 / `compare_runs` | `memharness/compare.rs` | `:136` / `:13` / `:123` / `:59` |
| memharness `prepare_corpus` / `manifest_sha` / `CorpusManifest` / `STRIP_FRONTMATTER` / `save_cases`/`load_cases` | `memharness/corpus.rs`,`cases.rs` | `:87` / `:42` / `:31` / `:19` / `:35`/`:53` |
| memharness judge ladder: `TRUST_AGREEMENT_MIN`=0.85 / `TRUST_KAPPA_MIN`=0.6 / `trust_verdict` / `judge_pair_blind` / `PairJudge` / `assign_blind` / `AUDIT_FLOOR`=30 | `memharness/judge.rs` | `:67` / `:68` / `:89` / `:241` / `:135` / `:213` / `:263` |
| memharness `HarnessDaemon::spawn_real`/`spawn_with_provider` / `WireClient`(`connect`,`add_grant`,`run_ingest`,`list_files`,`recall`) / `Command` enum | `memharness/daemon.rs`,`client.rs`,`main.rs` | `:61`/`:74` / `:29,63,72,82,91` / `:25` |

---

## File Structure

| File | Create/Modify | Responsibility |
| --- | --- | --- |
| `crates/bossclaw-core/src/reflect.rs` | **Create** | The ONE documented consts block; `ReflectReport`, `MissOutcome`, `MissState`, `TopicRefreshOutcome`, `StaleRefreshReport` data types; `normalized_query_key` (T5, T9). |
| `crates/bossclaw-core/src/lib.rs` | Modify | `pub mod reflect;` + re-export `ReflectReport` (T5). |
| `crates/bossclaw-core/src/log.rs` | Modify | `ConfigFlag::Reflect` + `REFLECT_ENABLED_KEY` + `reflect_enabled`/`set_reflect_enabled` + `key()` arm (T1); gather-path exclusion in `gather_fact_set` (T3); `refresh_topic_page` extraction (T4); `reflect_miss_backlog`/`reflect_counters`/`reflect_cursor` DDL + accessors (T5); `reflect_topics_for_query` (T6); `attempt_miss` (T7); `refresh_stale_pages` (T8); `reflect_once` (T9); `newest_memory_activity_at` (T11). |
| `crates/bossclawd-proto/src/lib.rs` | Modify | `Request::SetReflectEnabled`/`ReflectEnabled`, `Response::ReflectEnabled(bool)`, six-ops `no`-set entries; `Role::allows` UNCHANGED (T12). |
| `crates/bossclawd/src/lib.rs` | Modify | `#[cfg(unix)] pub mod reflect;` (T11). |
| `crates/bossclawd/src/engine/mod.rs` | Modify | `reflect_lock`+`reflect_tel` fields+init (T10); `ReflectTelemetry`+`record_reflect_tick`(+`_into`) (T10); `reflect_once` wrapper (T10); `reflect_enabled_or_false` (T2); `set_reflect_enabled`/`reflect_enabled` (T12); `prime_switches` force-off + trip-wire bump (T2); `serve_reflect_digest_line`+`build_reflect_digest_line` (T13). |
| `crates/bossclawd/src/reflect/mod.rs`,`sweeper.rs` | **Create** | `decide_reflect` pure fn + `ReflectSweepReport` + `run_reflect_sweep_once` + `spawn` (T11). |
| `crates/bossclawd/src/main.rs` | Modify | Spawn the reflect sweeper beside the three siblings (T11). |
| `crates/bossclawd/src/server.rs` | Modify | `SetReflectEnabled`/`ReflectEnabled` dispatch arms (T12). |
| `crates/bossclawd/src/telemetry.rs` | Modify | `take_recent_misses` drain accessor; disclosure doc-header update (T10). |
| `crates/bossclawd/src/capture/snapshot.rs` | Modify | `build` prepends `serve_reflect_digest_line(source)` into the preamble (T13). |
| `crates/bossclawd/tests/roundtrip.rs` | Modify | Trip-wire `5 → 6` (T2). |
| `crates/bossclawd/tests/reflect_e2e.rs` | **Create** | End-to-end enable→seed→tick→digest daemon test + guest-refused test (T16). |
| `apps/desktop/src-tauri/src/engine/client.rs` | Modify | `set_reflect_enabled`/`reflect_enabled` client methods; trip-wire `5 → 6` (T12b). |
| `apps/desktop/src-tauri/src/engine/mod.rs` | Modify | `Engine::set_reflect_enabled`/`reflect_enabled` (T12b). |
| `apps/desktop/src-tauri/src/commands/integrations.rs` | Modify | `integrations_set_reflect_enabled`/`integrations_reflect_enabled` commands + tests (T12b). |
| `apps/desktop/src-tauri/src/main.rs` | Modify | Register the two commands (T12b). |
| `apps/desktop/src/api/integrations.ts`(+`.test.ts`) | Modify | `setReflectEnabled`/`reflectEnabled` invoke wrappers + tests (T12b). |
| `apps/desktop/src/settings/ReflectPanel.tsx`(+`.test.tsx`) | **Create** | The Reflect toggle panel + component test (T12b). |
| `apps/desktop/src/settings/AirSettings.tsx` | Modify | Mount `<ReflectPanel/>` (T12b). |
| `crates/memharness/src/resolve.rs`,`arms.rs` | Modify | `PageResolver` resolves page-kind hits as synthetic non-gold occupants (T14). |
| `crates/memharness/src/reflect_pass.rs` | **Create** | Run-to-quiescence reflected-pass driver + union-coverage metric (T14). |
| `crates/memharness/src/probes.rs` | **Create** | Held-out generalization (d) + dossier-vs-source A/B (e) runners (T15). |
| `crates/memharness/src/lib.rs`,`main.rs` | Modify | Declare the new modules; document the Peter-gated runbook (T14, T15). |

## The ONE documented consts block (T5 creates it in `bossclaw-core/src/reflect.rs`)

All reflection tuning lives in a single block, PROVISIONAL / harness-tunable, mirroring the
`CONFLICT_PAIR_ERROR_BUDGET` doc pattern. Single-sourced in core; the daemon sweeper imports the two timing
consts (exactly as `conflict/sweeper.rs:9` imports `SWEEP_INTERVAL` from the capture sweeper).

```rust
//! Rung-4 R4-A reflection: the ONE documented tuning block + pure report/outcome data types.
//! PROVISIONAL / harness-tunable (spec §7; the `CONFLICT_PAIR_ERROR_BUDGET` precedent). PORTABLE —
//! reflection is built on portable seams (gather/summarize/entity_search/recall), never the
//! `#[cfg(unix)]` conflict-proposal subsystem, so nothing here is gated.

/// Idle window: reflection runs only when no memory-class append (memory + session-capture) landed
/// within this many seconds (spec §2.1). Provisional 600 — the capture sweeper's `QUIET_SECS`
/// precedent. Read fresh each tick against the newest relevant event's ts (no latch).
pub const REFLECT_QUIET_SECS: i64 = 600;

/// Starvation floor: if unrepaired/unparked misses exist AND this many seconds passed since the last
/// COMPLETED reflect run, run ONE budgeted tick even when not quiet and even when evolve is backlogged
/// (spec §2.1 precedence). Provisional 6h. Also the floor re-fire cadence (fires ≤ once per this window).
pub const REFLECT_STALENESS_FLOOR_SECS: i64 = 21_600;

/// Per-miss attempt budget → `parked` at/above this (spec §2.2; the Rung-3 poison-budget lesson).
pub const REFLECT_MISS_ATTEMPT_BUDGET: u32 = 3;

/// Open misses attempted per tick (spec §2.1 budget). Small + fixed; misses first, refresh with the rest.
pub const REFLECT_MISSES_PER_TICK: usize = 4;

/// Stale dossiers refreshed per tick (spec §2.1 budget).
pub const REFLECT_REFRESH_PER_TICK: usize = 4;

/// Top-N known topics a missed query may resolve to (spec §2.2 step 2). Conservative-first (spec §7.2).
pub const REFLECT_TOPIC_N: usize = 2;

/// `k` for the repair-check recalls in `attempt_miss` (spec §2.2 steps 1 & 4). The SP3 miss ring stores
/// only the query, not its original `k`, so reflection re-checks "does this query find anything now?" at a
/// stable modest `k`; any `k >= 1` answers hit-vs-miss, and 5 keeps the check cheap.
pub const REFLECT_RECALL_K: usize = 5;

/// Minimum cosine SIMILARITY (= `1.0 - entity_search` cosine distance, which lies in `[0,2]`; the
/// conversion mirrors `resolve_mention`, log.rs:6987) for a missed query to resolve to a KNOWN topic
/// (spec §2.2 step 2 / §7.2). Anchored to `extract::RESOLVE_LOW` (0.75) — the similarity BELOW which
/// evolve's own `resolve_decision` treats a mention as a brand-new entity (Mint) — so reflection never
/// claims to "know" a topic evolve itself would have minted fresh. Conservative per §7.2 (a too-high
/// floor merely yields more honest `no_material`); §5.3(d) tunes it upward toward `RESOLVE_HIGH` (0.92,
/// evolve's auto-merge bar) if precision demands. Compared as `1.0 - dist >= REFLECT_TOPIC_FLOOR`.
pub const REFLECT_TOPIC_FLOOR: f32 = 0.75;
```

---

## Task 1 — `ConfigFlag::Reflect` + sticky fail-closed setter/getter (core)

Spec §2.1 / §4 I3. Adds the fourth default-CLOSED sibling flag, mirroring the `ConflictDetect` family EXACTLY
(`log.rs:272`/`:7639`/`:7650`). Portable (config flags are cross-platform — the `ConfigFlag` doc at `:277`
says `#[cfg(unix)]` is unnecessary).

**Files**
- Modify: `crates/bossclaw-core/src/log.rs` — `REFLECT_ENABLED_KEY` const (after `CONFLICT_DETECT_ENABLED_KEY`
  `:272`); `ConfigFlag::Reflect` variant (after `ConflictDetect` `:297`); `key()` arm (after `:312`);
  `reflect_enabled`/`set_reflect_enabled` (after `set_conflict_detect_enabled` `:7670`).
- Test: `crates/bossclaw-core/src/log.rs` `mod tests`.

**Steps**

1. Write the failing test (append into `log.rs mod tests`):

```rust
#[test]
fn reflect_flag_is_default_closed_sticky_and_explicit_tracked() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    // Default CLOSED, exactly like conflict-detect: a never-set flag reads false (I3).
    assert!(!log.reflect_enabled().unwrap(), "reflect defaults CLOSED (never runs without consent)");
    assert!(!log.explicitly_set(ConfigFlag::Reflect).unwrap(), "never set yet");
    // Set true → sticky true; explicitly_set flips.
    log.set_reflect_enabled(true).unwrap();
    assert!(log.reflect_enabled().unwrap(), "explicit true wins");
    assert!(log.explicitly_set(ConfigFlag::Reflect).unwrap(), "now explicit");
    // A flagLESS later config event (e.g. a capture flip) must NOT re-close reflect (sticky newest-explicit).
    log.set_capture_enabled(true, false, 0).unwrap();
    assert!(log.reflect_enabled().unwrap(), "an unrelated config event does not disturb the reflect flag");
    // Explicit false wins over the earlier true.
    log.set_reflect_enabled(false).unwrap();
    assert!(!log.reflect_enabled().unwrap(), "newest explicit false is sticky");
    log.verify_chain().unwrap();
}
```

2. Run → FAIL: `cargo test -p bossclaw-core reflect_flag_is_default_closed_sticky_and_explicit_tracked`
   Expected: `no variant or associated item named Reflect found for enum ConfigFlag` (and `no method named
   reflect_enabled`).

3. Implement.
   (a) The key const (after `CONFLICT_DETECT_ENABLED_KEY` `:272`):

```rust
/// The `content` key carrying the Rung-4 R4-A reflection on/off switch (spec §2.1). Single-sourced (one
/// writer [`EventLog::set_reflect_enabled`], one reader [`EventLog::reflect_enabled`]). DEFAULT CLOSED —
/// the reflect loop never runs for a user who never consented (invariant I3), exactly like
/// [`CONFLICT_DETECT_ENABLED_KEY`].
const REFLECT_ENABLED_KEY: &str = "reflect_enabled";
```

   (b) The enum variant (after `ConflictDetect` `:297`):

```rust
    /// The Rung-4 R4-A reflection on/off switch ([`REFLECT_ENABLED_KEY`]). Default CLOSED.
    Reflect,
```

   (c) The `key()` arm (after the `ConflictDetect` arm `:312`):

```rust
            ConfigFlag::Reflect => REFLECT_ENABLED_KEY,
```

   (d) The setter/getter (after `set_conflict_detect_enabled` `:7670`) — byte-for-byte the ConflictDetect shape:

```rust
    /// Whether Rung-4 reflection is enabled (spec §2.1). STICKY / fail-closed via
    /// [`EventLog::latest_config_value`]'s newest-first scan; DEFAULT CLOSED (a never-set flag reads
    /// `false`), so the reflect loop never runs for a user who never consented (I3). Mirrors
    /// [`EventLog::conflict_detect_enabled`].
    pub fn reflect_enabled(&self) -> Result<bool, BossclawError> {
        Ok(self
            .latest_config_value(ConfigFlag::Reflect.key())?
            .and_then(|v| v.as_bool())
            .unwrap_or(false))
    }

    /// Flip the reflection switch by appending ONE signed + hash-chained control `config` event
    /// `{ "reflect_enabled": <enabled> }`. The ONLY writer of the key (so the reader can never drift the
    /// shape). Carries no model fields → never disturbs `active_model`. Mirrors
    /// [`EventLog::set_conflict_detect_enabled`].
    pub fn set_reflect_enabled(&self, enabled: bool) -> Result<(), BossclawError> {
        self.append(Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: CONFIG_EVENT_TYPE.to_string(),
            content: serde_json::Value::Object({
                let mut m = serde_json::Map::new();
                m.insert(REFLECT_ENABLED_KEY.to_string(), serde_json::Value::Bool(enabled));
                m
            }),
            model_meta: None,
            prev_hash: String::new(),
            hash: None,
            signed_by_did: self.signer_did(),
            signature: None,
        })?;
        Ok(())
    }
```

4. Run → PASS: `cargo test -p bossclaw-core reflect_flag_is_default_closed_sticky_and_explicit_tracked`
   Regression (the ConfigFlag family is unchanged for the others):
   `cargo test -p bossclaw-core config_flag explicitly_set conflict_detect`

5. Commit: `feat(rung4-a): ConfigFlag::Reflect + sticky fail-closed reflect_enabled/set_reflect_enabled`

---

## Task 2 — `prime_switches` force-off + `reflect_enabled_or_false` + the TWO daemon trip-wires 5→6 (daemon)

Spec §2.1 / §4 I3 / §5.4. Adds the sixth `prime_switches` write (unconditional-persist-OFF like
`ConflictDetect`), the fail-closed engine gate the sweeper reads each tick (mirroring
`conflict_detect_enabled_or_false` `:1052`), and moves the fresh-brain config-event-count trip-wire from `5` to
`6` at the TWO **daemon-side** sites. (The desktop third site `client.rs:973` is T12b's.) `engine/mod.rs` is a
`#[cfg(unix)]` module → no per-fn gates.

**Files**
- Modify: `crates/bossclawd/src/engine/mod.rs` — `prime_switches` (`:564`, after the ConflictDetect force-off
  `:591-593`); `reflect_enabled_or_false` (beside `conflict_detect_enabled_or_false` `:1052`); the fresh-brain
  test assertion `:2237`.
- Modify: `crates/bossclawd/tests/roundtrip.rs` — the trip-wire assertion `:173`.
- Test: `crates/bossclawd/src/engine/mod.rs` `mod tests` (the existing fresh-brain test, updated).

**Steps**

1. Update the failing assertion FIRST (this is the trip-wire; per I3 it moves in the SAME task that primes the
   flag). In `engine/mod.rs:2237` change `assert_eq!(st.event_count, 5);` to:

```rust
        // Rung-4 R4-A (design §4 I3): prime_switches now writes 6 sticky config events — the fifth was
        // Rung-3 conflict-detect force-off, the sixth is the Reflect force-off (both default-CLOSED).
        assert_eq!(st.event_count, 6, "prime_switches wrote the 6 sticky config events");
```

   And add a fail-closed gate assertion to the same fresh-brain test (or a sibling) proving reflect boots OFF:

```rust
        // I3 dormancy: a fresh brain has reflect forced explicitly OFF, so the sweeper gate is closed.
        assert!(!engine.reflect_enabled_or_false(true).await, "fresh brain: reflect is forced off");
```

   In `crates/bossclawd/tests/roundtrip.rs:173` change to:

```rust
        assert_eq!(s.event_count, 6, "prime_switches wrote the 6 sticky config events");
```

2. Run → FAIL: `cargo test -p bossclawd --lib engine::` and `cargo test -p bossclawd --test roundtrip`
   Expected: the count assertions fail (`5 != 6`) and `no method named reflect_enabled_or_false`.

3. Implement.
   (a) `prime_switches` (after the `ConflictDetect` block ending `:593`):

```rust
        // Rung-4 R4-A (§2.1, I3): reflection is default-CLOSED — its getter already returns false when
        // unset — so, like capture/conflict above, persist an EXPLICIT OFF the first time it was never set
        // (a tamper-evident "this brain has reflection off" record). Idempotent: `explicitly_set` is true
        // afterward, so a re-open writes nothing. This is the SIXTH sticky config event on a fresh brain.
        if !log.explicitly_set(ConfigFlag::Reflect)? {
            log.set_reflect_enabled(false)?;
        }
```

   (b) `reflect_enabled_or_false` (after `conflict_detect_enabled_or_false` `:1059`) — the direct-getter template:

```rust
    /// The reflection off-switch verdict, defaulting to `false` (OFF) on ANY error (not onboarded, open
    /// failure, …). The gate the reflect sweep reads each cycle — it must never propagate an error (a
    /// transient read failure must not trip reflection ON). Mirrors [`Self::conflict_detect_enabled_or_false`].
    pub async fn reflect_enabled_or_false(&self, onboarded: bool) -> bool {
        let Ok(log) = self.get_or_open(onboarded).await else {
            return false;
        };
        spawn_blocking(move || log.reflect_enabled().unwrap_or(false))
            .await
            .unwrap_or(false)
    }
```

4. Run → PASS: `cargo test -p bossclawd --lib engine::` and `cargo test -p bossclawd --test roundtrip`
   Regression: `cargo test -p bossclawd --lib prime_switches`

5. Commit: `feat(rung4-a): prime_switches force-off Reflect + reflect_enabled_or_false + daemon trip-wire 5→6`

---

## Task 3 — Shared gather-path lineage exclusion (core, Reflect-INDEPENDENT healing)

Spec §2.3 (arch M2, the Rev-1 silent defect) + §4 I3 (the ONE deliberate Reflect-independent change). The gather
path does NOT currently exclude retired/superseded lineage, so a stale-page refresh would re-gather the identical
set and the cited-set idempotency guard (`log.rs:8161-8166`) would skip the emit — detecting rot nightly while
never healing it. This task adds the exclusion INSIDE `gather_fact_set` (the single shared source consumed by
BOTH evolve's summarize and reflection's refresh — the I9 single-source lesson).

**Verified single-seam placement.** `fact_texts_for_ids` has EXACTLY ONE caller — `gather_fact_set:8098` (grep
confirmed). `gather_fact_set` builds `lineage` (`:8087-8097`) then derives BOTH `memories =
fact_texts_for_ids(&lineage)` AND `source_ids: lineage` (`:8098-8099`). So filtering `lineage` in
`gather_fact_set` shrinks BOTH the gathered memory texts AND the cited `source_event_ids` — precisely the
"texts-AND-cited-ids" correctness note (§2.3). The exclusion set is `fold.superseded ∪ fold.retired_notes`,
the SAME "gone" set recall's memory arm already uses (`log.rs:1853-1865`). This is the ONLY behavior change:
for a fresh corpus with no retirements the fold sets are empty → gather is byte-identical (why the existing
evolve/summarize goldens stay green). Portable (ungated).

**Files**
- Modify: `crates/bossclaw-core/src/log.rs` — `gather_fact_set` (`:8076`), filtering `lineage` before
  `:8098`. Uses `fold_sessions(&self.session_events_ordered()?)` (`:9607`/`:5725`).
- Test: `crates/bossclaw-core/src/log.rs` `mod tests` (the focused gather assertion) + append an end-to-end
  summarize re-emit test to `crates/bossclaw-core/tests/evolve.rs` (reuses `seed_topic_directly` `:806`).

**Steps**

1. Write the failing tests.
   (a) Focused gather-exclusion unit test (in `log.rs mod tests`) — the load-bearing correctness:

```rust
#[test]
fn gather_fact_set_excludes_retired_and_superseded_lineage_from_texts_and_cited_ids() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let emb = MockEmbedder::new(8);
    // Two source notes fold into one topic's lineage; a link ties the entity to a second endpoint.
    let m1 = log.remember(&emb, "Kenny joined Acme in 2019.").unwrap();
    let m2 = log.remember(&emb, "Kenny leads the platform team.").unwrap();
    let lineage = vec![m1.clone(), m2.clone()];
    let topic = log.entity("Kenny", &[], "person", "test-v1", &lineage).unwrap();
    let acme = log.entity("Acme", &[], "org", "test-v1", &lineage).unwrap();
    log.link_machine(&topic, "works_at", &acme, 0.9, "test-v1", &lineage).unwrap();
    log.rebuild_graph().unwrap();
    let entity = log.all_entities().unwrap().into_iter().find(|e| e.entity_id == topic).unwrap();

    // Before any retire: both source ids are gathered (texts + cited set).
    let before = log.gather_fact_set(&entity).unwrap();
    assert!(before.source_ids.contains(&m1) && before.source_ids.contains(&m2), "both cited");
    assert!(before.memories.iter().any(|(id, _)| id == &m1), "m1 text gathered");

    // Retire m1 (App path). The gather path must now drop it from BOTH the memory texts AND source_ids.
    log.retire_memory(&m1, None).unwrap();
    let after = log.gather_fact_set(&entity).unwrap();
    assert!(!after.source_ids.contains(&m1), "retired m1 dropped from the cited source_event_ids (D8 anchor)");
    assert!(!after.memories.iter().any(|(id, _)| id == &m1), "retired m1 dropped from the gathered texts");
    assert!(after.source_ids.contains(&m2), "the surviving source is untouched");
    assert!(after.memories.iter().any(|(id, _)| id == &m2), "m2 text still gathered");

    // Control: a topic with NOTHING retired gathers identically before/after (no behavior change on a
    // clean corpus — why the evolve/summarize goldens stay green).
    let c = log.remember(&emb, "Beta is a database.").unwrap();
    let ct = log.entity("Beta", &[], "product", "test-v1", std::slice::from_ref(&c)).unwrap();
    log.rebuild_graph().unwrap();
    let ce = log.all_entities().unwrap().into_iter().find(|e| e.entity_id == ct).unwrap();
    let g1 = log.gather_fact_set(&ce).unwrap();
    let g2 = log.gather_fact_set(&ce).unwrap();
    assert_eq!(g1.source_ids, g2.source_ids, "no retirement → gather is stable (control)");
}
```

   (b) End-to-end summarize re-emit (append to `crates/bossclaw-core/tests/evolve.rs`) — the healing is real:

```rust
#[test]
fn summarize_re_emits_a_healed_page_after_a_cited_source_is_retired() {
    // §2.3: retiring a cited source shrinks the gathered cited set, so the F6 idempotency guard now
    // DIFFERS → the dossier re-emits WITHOUT the retired citation (dossiers stop citing retired memories).
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let embedder = MockEmbedder::new(MID_DIM);
    // Seed a topic whose lineage has TWO source memories so a retire still leaves it summary-worthy.
    let m1 = seed_memory(&log, &embedder, "Kenny works at Acme.");
    let m2 = seed_memory(&log, &embedder, "Kenny lives in Denver.");
    let lineage = vec![m1.clone(), m2.clone()];
    let topic = log.entity("Kenny", &[], "person", "scripted-evolve-v1", &lineage).unwrap();
    let acme = log.entity("Acme", &[], "org", "scripted-evolve-v1", &lineage).unwrap();
    log.link_machine(&topic, "works_at", &acme, 0.9, "scripted-evolve-v1", &lineage).unwrap();
    log.rebuild_graph().unwrap();
    let entity = log.all_entities().unwrap().into_iter().find(|e| e.entity_id == topic).unwrap();

    // Tick 1: emit the first page, cites BOTH memories.
    let facts1 = log.gather_fact_set(&entity).unwrap();
    let r1 = scripted_both_passes("scripted-evolve-v1", "Kenny works at Acme.", &[], &[], empty_pass_a())
        .with_response(SUMMARIZE_SYSTEM, &build_compose_prompt(&facts1), json!({
            "title": "Kenny",
            "claims": [
                { "text": "Kenny works at Acme.", "cites": [m1.clone()] },
                { "text": "Kenny lives in Denver.", "cites": [m2.clone()] }
            ]}));
    assert_eq!(log.evolve_once(&embedder, &r1).unwrap().pages_emitted, 1);

    // Retire m1, then re-dirty the topic (a fresh link past the summarize cursor).
    log.retire_memory(&m1, None).unwrap();
    let beta = log.entity("Beta", &[], "org", "scripted-evolve-v1", std::slice::from_ref(&m2)).unwrap();
    log.link_machine(&topic, "advises", &beta, 0.9, "scripted-evolve-v1", std::slice::from_ref(&m2)).unwrap();
    log.rebuild_graph().unwrap();

    // Tick 2: gather the POST-exclusion facts (m1 gone) and script a compose citing only m2. The cited
    // set shrank vs the current page → the F6 guard fires an emit + supersede (the heal).
    let entity2 = log.all_entities().unwrap().into_iter().find(|e| e.entity_id == topic).unwrap();
    let facts2 = log.gather_fact_set(&entity2).unwrap();
    assert!(!facts2.source_ids.contains(&m1), "the retired source is excluded from the refreshed gather");
    let r2 = scripted_both_passes("scripted-evolve-v1", "Kenny works at Acme.", &[], &[], empty_pass_a())
        .with_response(SUMMARIZE_SYSTEM, &build_compose_prompt(&facts2), json!({
            "title": "Kenny", "claims": [{ "text": "Kenny lives in Denver.", "cites": [m2.clone()] }]}));
    let report2 = log.evolve_once(&embedder, &r2).unwrap();
    assert_eq!(report2.pages_superseded, 1, "the stale page is superseded by a healed revision");

    // The current page no longer cites the retired memory.
    let page = log.current_pages().unwrap().into_iter().find(|p| p.topic_id == topic).unwrap();
    let page_ev = log.stream_all().unwrap().into_iter().find(|e| e.id == page.page_event_id).unwrap();
    let cited = page_ev.model_meta.unwrap().source_event_ids;
    assert!(!cited.contains(&m1), "healed page no longer cites the retired memory");
    assert!(cited.contains(&m2), "healed page still cites the surviving memory");
    log.verify_chain().unwrap();
}
```

2. Run → FAIL:
   `cargo test -p bossclaw-core gather_fact_set_excludes_retired_and_superseded_lineage_from_texts_and_cited_ids`
   `cargo test -p bossclaw-core --test evolve summarize_re_emits_a_healed_page_after_a_cited_source_is_retired`
   Expected: the `after`/heal assertions fail — today the retired id is STILL gathered (the defect).

3. Implement. In `gather_fact_set` (`:8076`), after the `lineage.sort(); lineage.dedup();` at `:8096-8097` and
   BEFORE `let memories = self.fact_texts_for_ids(&lineage)?;` at `:8098`, insert the exclusion:

```rust
        lineage.sort();
        lineage.dedup();
        // §2.3 (I9 single-source): drop retired/superseded source ids from the gathered lineage so BOTH the
        // memory texts (fact_texts_for_ids below) AND the cited `source_event_ids` (the D8 taint anchor +
        // the F6 idempotency key) shrink together. The "gone" set is `superseded ∪ retired_notes` — the
        // SAME exclusion recall's memory arm applies (log.rs:1853-1865). Consumed by BOTH evolve's summarize
        // and reflection's refresh, so the two writers can never fight over a stale citation. On a corpus
        // with no retirements both sets are empty → this is a no-op (evolve goldens unchanged).
        let fold = fold_sessions(&self.session_events_ordered()?);
        if !fold.superseded.is_empty() || !fold.retired_notes.is_empty() {
            lineage.retain(|id| !fold.superseded.contains(id) && !fold.retired_notes.contains(id));
        }
        let memories = self.fact_texts_for_ids(&lineage)?;
        Ok(crate::summarize::FactSet { entity: entity.clone(), edges, memories, source_ids: lineage })
```

4. Run → PASS (both new tests), then the regression that proves behavior-preservation on a clean corpus:
   `cargo test -p bossclaw-core gather_fact_set_excludes_retired_and_superseded_lineage_from_texts_and_cited_ids`
   `cargo test -p bossclaw-core --test evolve` (the full evolve/summarize golden suite — MUST stay green:
   fresh corpora retire nothing, so gather is byte-identical).

5. Commit: `feat(rung4-a): gather-path retired/superseded lineage exclusion (Reflect-INDEPENDENT dossier heal)`

---

## Task 4 — Per-topic compose extraction: `refresh_topic_page` (core, behavior-preserving refactor)

Spec §2.2 step 3 / §2.3. Extract the per-topic body of `summarize_topics` (`log.rs:8120-8207`) into a private
reusable `refresh_topic_page` that both the batch loop and reflection call. The existing summarize goldens must
stay green UNCHANGED — that is the refactor's proof.

**Source-grounded signature deviation (see "Plan-time deviations").** The per-topic compose body takes only a
`&dyn Reasoner` — it NEVER touches an embedder (`summarize_topics:8111` has no embedder param; `emit_page:2772`
takes none; pages embed lazily via `append` + become recall-visible at the wrapper's post-tick
`rebuild_indexes`). So the honest signature is `refresh_topic_page(&self, reasoner, entity) -> Result<
TopicRefreshOutcome>`, NOT the brief's `(embedder, reasoner, entity)`. The outcome type lets both the batch
loop and reflection map their own counters (the batch keeps its `EvolveReport`; reflection has a different
report).

**Files**
- Modify: `crates/bossclaw-core/src/reflect.rs` — `TopicRefreshOutcome` (T5 also adds types here; if T4 lands
  first, create the file with just this enum and T5 fills the rest).
- Modify: `crates/bossclaw-core/src/log.rs` — `refresh_topic_page` (new private method beside
  `summarize_topics` `:8111`); rewrite the `summarize_topics` per-topic loop body to call it.
- Test: `crates/bossclaw-core/src/log.rs` `mod tests` (a direct `refresh_topic_page` outcome test) + the
  existing `tests/evolve.rs` summarize goldens (unchanged, as the proof).

**Steps**

1. Write the failing test (in `log.rs mod tests`) — the three outcomes, driven directly:

```rust
#[test]
fn refresh_topic_page_returns_emitted_unchanged_or_thin() {
    use crate::reflect::TopicRefreshOutcome;
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let emb = MockEmbedder::new(8);
    // Summary-worthy topic (>= PAGE_MIN_FACTS): one memory + one edge.
    let m1 = log.remember(&emb, "Kenny works at Acme.").unwrap();
    let topic = log.entity("Kenny", &[], "person", "test-v1", std::slice::from_ref(&m1)).unwrap();
    let acme = log.entity("Acme", &[], "org", "test-v1", std::slice::from_ref(&m1)).unwrap();
    log.link_machine(&topic, "works_at", &acme, 0.9, "test-v1", std::slice::from_ref(&m1)).unwrap();
    log.rebuild_graph().unwrap();
    let entity = log.all_entities().unwrap().into_iter().find(|e| e.entity_id == topic).unwrap();
    let facts = log.gather_fact_set(&entity).unwrap();
    let reasoner = crate::reason::ScriptedReasoner::new("test-v1").with_response(
        crate::summarize::SUMMARIZE_SYSTEM, &crate::summarize::build_compose_prompt(&facts),
        serde_json::json!({ "title": "Kenny", "claims": [{ "text": "Kenny works at Acme.", "cites": [m1] }]}),
    );
    // First refresh → Emitted (no prior page → not superseded).
    assert_eq!(log.refresh_topic_page(&reasoner, &entity).unwrap(),
        TopicRefreshOutcome::Emitted { superseded: false });
    log.rebuild_graph().unwrap();
    // Second refresh, identical grounding → SkippedUnchanged (F6 cited-set idempotency).
    let entity2 = log.all_entities().unwrap().into_iter().find(|e| e.entity_id == topic).unwrap();
    let facts2 = log.gather_fact_set(&entity2).unwrap();
    let reasoner2 = crate::reason::ScriptedReasoner::new("test-v1").with_response(
        crate::summarize::SUMMARIZE_SYSTEM, &crate::summarize::build_compose_prompt(&facts2),
        serde_json::json!({ "title": "Kenny", "claims": [{ "text": "Kenny works at Acme.", "cites": [
            facts2.memories[0].0.clone()] }]}),
    );
    assert_eq!(log.refresh_topic_page(&reasoner2, &entity2).unwrap(), TopicRefreshOutcome::SkippedUnchanged);
    // A topic below PAGE_MIN_FACTS → SkippedThin (no reasoner call needed).
    let thin = log.remember(&emb, "lonely note").unwrap();
    let te = log.entity("Lonely", &[], "misc", "test-v1", std::slice::from_ref(&thin)).unwrap();
    log.rebuild_graph().unwrap();
    let thin_entity = log.all_entities().unwrap().into_iter().find(|e| e.entity_id == te).unwrap();
    // fact_count = 0 edges + 1 memory = 1 < PAGE_MIN_FACTS(2) → thin.
    assert_eq!(log.refresh_topic_page(&reasoner, &thin_entity).unwrap(), TopicRefreshOutcome::SkippedThin);
}
```

2. Run → FAIL: `cargo test -p bossclaw-core refresh_topic_page_returns_emitted_unchanged_or_thin`
   Expected: `no method named refresh_topic_page` (+ `no ... TopicRefreshOutcome` until step 3a).

3. Implement.
   (a) `TopicRefreshOutcome` in `crates/bossclaw-core/src/reflect.rs` (create the module if T5 has not; add
   `pub mod reflect;` to `lib.rs` — T5 formalizes the re-exports):

```rust
/// The result of refreshing ONE topic's dossier (spec §2.2 step 3). Reused by evolve's summarize batch
/// AND reflection's refresh, so each caller maps it to its own report. PORTABLE.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopicRefreshOutcome {
    /// A page was emitted; `superseded` iff a prior page for this topic was replaced (F5).
    Emitted { superseded: bool },
    /// The gathered cited set matched the current page's → no emit (F6 idempotency).
    SkippedUnchanged,
    /// The fact-set is below `PAGE_MIN_FACTS`, or a gather/parse/assemble error occurred → no emit (F4).
    /// Distinct from `SkippedUnchanged` so `refresh_stale_pages` can count `unhealable_thin` (§2.3).
    SkippedThin,
    /// The compose reasoner call failed (transport/decoding). Distinct from `SkippedThin` so reflection can
    /// count `reasoner_errors` per-item (§2.4, the Rung-3 poison lesson); evolve's summarize treats it as a
    /// no-op `continue` exactly like the two Skipped variants (behavior-preserving).
    ReasonerError,
}
```

   (b) `refresh_topic_page` (private, beside `summarize_topics` `:8111`) — the extracted body, verbatim logic
   from `:8130-8207` but returning the outcome instead of mutating an `EvolveReport`:

```rust
    /// Refresh ONE topic's dossier through the citation-floored machinery (spec §2.2 step 3): gather →
    /// (thin? → SkippedThin) → compose → citation floor → assemble → F6 cited-set idempotency → emit_page
    /// (atomic supersede). PORTABLE, reasoner-only (no embedder: pages embed lazily via `append` and become
    /// recall-visible at the caller's post-tick `rebuild_indexes`). Per-topic errors fold to `SkippedThin`
    /// (F4: a topic failure must never propagate). Extracted from `summarize_topics` so evolve's batch AND
    /// reflection share ONE composer (I9 single-source) — the §2.3 gather exclusion applies to both by
    /// construction (it lives in `gather_fact_set`).
    fn refresh_topic_page(
        &self,
        reasoner: &dyn crate::reason::Reasoner,
        entity: &crate::graph::Entity,
    ) -> Result<crate::reflect::TopicRefreshOutcome, BossclawError> {
        use crate::reflect::TopicRefreshOutcome;
        let facts = match self.gather_fact_set(entity) {
            Ok(f) if f.fact_count() >= crate::summarize::PAGE_MIN_FACTS => f,
            _ => return Ok(TopicRefreshOutcome::SkippedThin), // too thin, or a gather error (F4)
        };
        let raw = match reasoner.complete_json(
            crate::summarize::SUMMARIZE_SYSTEM,
            &crate::summarize::build_compose_prompt(&facts),
            &crate::summarize::compose_schema(),
        ) {
            Ok(v) => v,
            Err(e) => {
                log::warn!("refresh: compose failed for {}, skipping: {e}", entity.entity_id);
                return Ok(TopicRefreshOutcome::ReasonerError);
            }
        };
        let draft = match crate::summarize::parse_draft(&raw) {
            Ok(d) => d,
            Err(e) => {
                log::warn!("refresh: malformed draft for {}, skipping: {e}", entity.entity_id);
                return Ok(TopicRefreshOutcome::SkippedThin);
            }
        };
        let floored = crate::summarize::citation_floor(&draft, &facts);
        let rendered = match crate::summarize::assemble(&floored) {
            Some(r) => r,
            None => return Ok(TopicRefreshOutcome::SkippedThin), // empty floor (F4)
        };
        // F6: an unchanged cited-source SET emits nothing (no supersede churn).
        let prior = self.current_page_for_topic(&entity.entity_id)?;
        if let Some((_pid, prior_cites)) = &prior {
            if prior_cites == &rendered.cites {
                return Ok(TopicRefreshOutcome::SkippedUnchanged);
            }
        }
        // Canonicalize each claim's own cites for F7 signed content (identical to summarize_topics:8174).
        let claims_json: Vec<serde_json::Value> = floored
            .claims
            .iter()
            .map(|c| {
                let mut cites = c.cites.clone();
                cites.sort();
                cites.dedup();
                serde_json::json!({ "text": c.text, "cites": cites })
            })
            .collect();
        let claims_capped = &claims_json[..claims_json.len().min(crate::summarize::MAX_CLAIMS_PER_PAGE)];
        let prior_id = prior.as_ref().map(|(id, _)| id.as_str());
        match self.emit_page(
            &entity.entity_id,
            &rendered.title,
            &rendered.text,
            claims_capped,
            &[],
            reasoner.model_id(),
            &facts.source_ids, // D8 taint anchor (engine gather lineage, post-exclusion)
            prior_id,
        ) {
            Ok((_pid, superseded)) => Ok(TopicRefreshOutcome::Emitted { superseded }),
            Err(e) => {
                log::warn!("refresh: emit_page failed for {}, skipping: {e}", entity.entity_id);
                Ok(TopicRefreshOutcome::SkippedThin)
            }
        }
    }
```

   (c) Rewrite the `summarize_topics` per-topic loop body (`:8120-8208`) to call the extraction. Replace the
   inner body (from `let entity = match ... :8121` through the `match self.emit_page(...) { Ok ... Err ... }`
   block at `:8187-8207`) with:

```rust
        for topic_id in dirty.iter().take(crate::extract::SUMMARY_BATCH) {
            let entity = match entities.iter().find(|e| &e.entity_id == topic_id) {
                Some(e) => e.clone(),
                None => continue, // a dirty endpoint with no folded entity → skip (F1-safe, comment unchanged)
            };
            match self.refresh_topic_page(reasoner, &entity)? {
                crate::reflect::TopicRefreshOutcome::Emitted { superseded } => {
                    report.pages_emitted += 1;
                    if superseded {
                        report.pages_superseded += 1;
                    }
                }
                crate::reflect::TopicRefreshOutcome::SkippedUnchanged
                | crate::reflect::TopicRefreshOutcome::SkippedThin
                | crate::reflect::TopicRefreshOutcome::ReasonerError => {}
            }
        }
```

   The surrounding `summarize_topics` frame (`cursor`/`dirty`/`drained`/`entities` setup `:8116-8119`, the
   post-loop `rebuild_graph` `:8210-8212`, and the cursor advance `:8214-8216`) is UNCHANGED.

4. Run → PASS: `cargo test -p bossclaw-core refresh_topic_page_returns_emitted_unchanged_or_thin`
   The refactor's proof — the full summarize golden suite MUST stay green UNCHANGED:
   `cargo test -p bossclaw-core --test evolve` and `cargo test -p bossclaw-core summarize`

5. Commit: `refactor(rung4-a): extract refresh_topic_page from summarize_topics (behavior-preserving, shared)`

---

## Task 5 — Reflect data types + re-derivable progress tables (core)

Spec §2.2 (durable backlog) / §2.4 (counters) / §7.2-7.3. Fills in `reflect.rs` (the consts block + report/outcome
types + `normalized_query_key`) and adds three re-derivable single-purpose tables + accessors in `log.rs`
(beside the conflict-cursor DDL family). All PORTABLE, ungated. These are re-derivable progress state (I5) —
losing them only re-learns misses from the ring; NOT Tier-A fold inputs.

**Files**
- Modify: `crates/bossclaw-core/src/reflect.rs` — `ReflectReport`, `MissOutcome`, `MissState`,
  `StaleRefreshReport`, `ReflectCursor`; `normalized_query_key`. (`TopicRefreshOutcome` already added by T4.)
- Modify: `crates/bossclaw-core/src/lib.rs` — `pub mod reflect;` + `pub use reflect::ReflectReport;`.
- Modify: `crates/bossclaw-core/src/log.rs` — three `CREATE TABLE` statements in the schema-init DDL block
  (beside the single-row cursor tables, near `:966-1037`); the accessors (a new `// ── Reflection state
  ──` section, e.g. after the conflict-cursor accessors).
- Test: `crates/bossclaw-core/src/log.rs` `mod tests` + `crates/bossclaw-core/src/reflect.rs` `mod tests`.

**Steps**

1. Write the failing tests.
   (a) In `reflect.rs mod tests` (the pure normalizer):

```rust
#[test]
fn normalized_query_key_is_trim_casefold_stable() {
    // Trimmed + casefolded → the same key; distinct queries → distinct keys; fixed-length hex.
    assert_eq!(normalized_query_key("  Where is Kenny?  "), normalized_query_key("where is kenny?"));
    assert_ne!(normalized_query_key("where is kenny?"), normalized_query_key("where is acme?"));
    assert_eq!(normalized_query_key("x").len(), 64, "sha256 hex is fixed-length (bounded PK)");
}
```

   (b) In `log.rs mod tests` (the backlog + counters + cursor round-trip):

```rust
#[test]
fn reflect_backlog_seeds_upsert_only_transitions_and_counters_roundtrip() {
    use crate::reflect::{normalized_query_key, MissState};
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let k1 = normalized_query_key("where is kenny?");
    let k2 = normalized_query_key("what is beta?");

    // Seed twice: upsert-if-new keeps the FIRST first_seen/state (no reset of progress).
    log.seed_miss(&k1, "where is kenny?", 100).unwrap();
    log.seed_miss(&k1, "where is kenny?", 200).unwrap(); // ignored (already present)
    log.seed_miss(&k2, "what is beta?", 150).unwrap();
    let open = log.open_misses(10).unwrap();
    assert_eq!(open.len(), 2, "two open misses");
    assert_eq!(open[0].0, k1, "oldest first_seen first"); // k1 seeded at 100 < k2 at 150

    // A terminal transition removes it from `open`; parked/no_material persist (spec §7.3).
    log.set_miss_state(&k1, MissState::NoMaterial, 300).unwrap();
    assert_eq!(log.open_misses(10).unwrap().len(), 1, "no_material is no longer open");

    // Attempt bump returns the new count; at budget the caller parks (see T7).
    let a = log.bump_miss_attempt(&k2, 400).unwrap();
    assert_eq!(a, 1, "first attempt");

    // Counters accumulate; cursor holds the last-served totals + the daemon-supplied timing markers.
    log.add_reflect_counters(3, 2).unwrap();
    log.add_reflect_counters(1, 0).unwrap();
    assert_eq!(log.reflect_counters().unwrap(), (4, 2), "refreshed_total, no_material_total");
    let c0 = log.reflect_cursor().unwrap();
    assert_eq!((c0.last_served_refreshed, c0.last_served_no_material), (0, 0), "nothing served yet");
    log.set_reflect_last_served(4, 2).unwrap();
    log.set_reflect_last_completed_run(999).unwrap();
    log.set_reflect_last_floor_fire(888).unwrap();
    let c = log.reflect_cursor().unwrap();
    assert_eq!((c.last_served_refreshed, c.last_served_no_material), (4, 2));
    assert_eq!((c.last_completed_run_at, c.last_floor_fire_at), (999, 888));
}
```

2. Run → FAIL: `cargo test -p bossclaw-core normalized_query_key_is_trim_casefold_stable reflect_backlog_seeds_upsert_only_transitions_and_counters_roundtrip`
   Expected: `cannot find function normalized_query_key` / `no method named seed_miss`.

3. Implement.
   (a) `reflect.rs` data types + normalizer (append below the consts block; `TopicRefreshOutcome` is already
   present from T4):

```rust
use sha2::{Digest, Sha256};

/// Normalized backlog key (spec §2.2 / §7.2): SHA-256 hex of the TRIMMED, casefolded query. v1 casefold =
/// `to_lowercase()` (semantic dedup stays OUT, §6). Fixed-length hex keeps the PK bounded; the raw
/// `query_text` is stored in its own column for the digest/UI. Bloat is bounded because pages are
/// ENTITY-keyed — near-duplicate queries about one topic converge on the same dossier. PURE.
pub fn normalized_query_key(query: &str) -> String {
    let mut h = Sha256::new();
    h.update(query.trim().to_lowercase().as_bytes());
    hex::encode(h.finalize())
}

/// Backlog state for one missed query (spec §2.2). `Open` is re-attempted; `RepairedByTime`/
/// `CandidateRepaired` are terminal-success (age out later, §7.3); `NoMaterial`/`Parked` are terminal and
/// PERSIST (they carry information — "we never knew this" / "we tried and could not"). PORTABLE.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissState { Open, RepairedByTime, CandidateRepaired, NoMaterial, Parked }

impl MissState {
    pub fn as_str(self) -> &'static str {
        match self {
            MissState::Open => "open",
            MissState::RepairedByTime => "repaired_by_time",
            MissState::CandidateRepaired => "candidate_repaired",
            MissState::NoMaterial => "no_material",
            MissState::Parked => "parked",
        }
    }
}

/// The result of ONE `attempt_miss` (spec §2.2). Consumed by `reflect_once` to tally the report + persist
/// the state. PORTABLE.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissOutcome {
    /// Recall now hits without any refresh (step 1).
    RepairedByTime,
    /// The query resolves to no known topic above the floor (step 2) — an honest "we never knew this".
    NoMaterial,
    /// A refresh made the replay recall hit (step 4). OPERATIONAL, not evidence (§5.1).
    CandidateRepaired,
    /// A refresh did not help; the attempt was bumped but the budget is not yet reached.
    StillMissing,
    /// The attempt reached `REFLECT_MISS_ATTEMPT_BUDGET` → parked (bounded loss).
    Parked,
    /// A per-item compose reasoner error occurred during the refresh (isolated + counted, §2.4).
    ReasonerError,
}

/// The report of one `refresh_stale_pages` pass (spec §2.3). PORTABLE.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct StaleRefreshReport {
    /// Stale pages whose refresh emitted a healed revision.
    pub healed: usize,
    /// Stale pages whose refresh changed nothing (F6) this pass.
    pub unchanged: usize,
    /// Stale pages that cannot legally re-emit — their lineage is ENTIRELY retired/superseded so the
    /// fact-set fell below `PAGE_MIN_FACTS` (§2.3 thin-set residual). Counted, not retried forever.
    pub unhealable_thin: usize,
    /// Per-item compose reasoner errors this pass (isolated + counted).
    pub reasoner_errors: usize,
}

/// The tick report `reflect_once` returns (spec §2.4). Mirrors `EvolveReport`'s derive/style. `merge_proposed`
/// is always 0 in R4-A (R4-B fills it). PORTABLE.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReflectReport {
    pub attempted: usize,
    pub candidate_repaired: usize,
    pub repaired_by_time: usize,
    pub no_material: usize,
    pub parked: usize,
    pub dossiers_refreshed: usize,
    pub unhealable_thin: usize,
    pub merge_proposed: usize,
    pub skipped_disabled: bool,
    pub reasoner_errors: usize,
}

/// The reflect progress cursor (re-derivable single row). `last_served_*` back the digest delta (T13);
/// `last_completed_run_at`/`last_floor_fire_at` are daemon-supplied epoch markers (clock-free core, the
/// `capture_enabled_at` precedent) backing the §2.1 starvation floor. PORTABLE.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ReflectCursor {
    pub last_served_refreshed: i64,
    pub last_served_no_material: i64,
    pub last_completed_run_at: i64,
    pub last_floor_fire_at: i64,
}
```

   (Add `hex` + `sha2` — both are already `bossclaw-core` deps used by `reason.rs`/`sign.rs`; no new deps.)

   (b) `lib.rs`: `pub mod reflect;` (beside `pub mod recall;` `:40`) and `pub use reflect::ReflectReport;`
   (beside `pub use evolve::{EvolveReport, EvolveStatus};` `:59`).

   (c) `log.rs` DDL (three tables, in the schema-init block beside the single-row cursor tables):

```rust
        // Rung-4 R4-A (§2.2): the durable miss backlog, re-derivable from the SP3 miss ring (I5). PK =
        // trimmed-casefold sha256 so near-duplicate queries converge; `query_text` kept for the digest/UI.
        store.exec(
            "CREATE TABLE IF NOT EXISTS reflect_miss_backlog (
                normalized_query_key TEXT PRIMARY KEY,
                query_text           TEXT NOT NULL,
                first_seen           INTEGER NOT NULL,
                attempts             INTEGER NOT NULL DEFAULT 0,
                state                TEXT NOT NULL DEFAULT 'open',
                updated_at           INTEGER NOT NULL
            )",
        )?;
        // Rung-4 R4-A (§2.4): cumulative operational counters (single row id=0).
        store.exec(
            "CREATE TABLE IF NOT EXISTS reflect_counters (
                id                INTEGER PRIMARY KEY CHECK (id = 0),
                refreshed_total   INTEGER NOT NULL DEFAULT 0,
                no_material_total INTEGER NOT NULL DEFAULT 0
            )",
        )?;
        // Rung-4 R4-A (§2.1/§2.4): progress cursor (single row id=0) — digest last-served totals + the
        // daemon-supplied floor timing markers.
        store.exec(
            "CREATE TABLE IF NOT EXISTS reflect_cursor (
                id                    INTEGER PRIMARY KEY CHECK (id = 0),
                last_served_refreshed INTEGER NOT NULL DEFAULT 0,
                last_served_no_material INTEGER NOT NULL DEFAULT 0,
                last_completed_run_at INTEGER NOT NULL DEFAULT 0,
                last_floor_fire_at    INTEGER NOT NULL DEFAULT 0
            )",
        )?;
```

   (d) `log.rs` accessors (new section):

```rust
    // ── Rung-4 R4-A reflection progress state (re-derivable, spec §2.2/§2.4). PORTABLE. ──

    /// Seed a miss into the backlog, upsert-IF-NEW (spec §2.2): a new key lands `open`; an existing key
    /// (any state, incl. parked/no_material) is left UNTOUCHED so ring churn can never reset progress.
    pub fn seed_miss(&self, key: &str, query: &str, now: i64) -> Result<(), BossclawError> {
        let store = self.inner.lock().expect(POISON);
        store.conn().execute(
            "INSERT INTO reflect_miss_backlog (normalized_query_key, query_text, first_seen, attempts, state, updated_at)
             VALUES (?1, ?2, ?3, 0, 'open', ?3)
             ON CONFLICT(normalized_query_key) DO NOTHING",
            rusqlite::params![key, query, now],
        )?;
        Ok(())
    }

    /// The `(key, query_text, attempts)` of open misses, oldest `first_seen` first, capped at `limit`.
    pub fn open_misses(&self, limit: usize) -> Result<Vec<(String, String, u32)>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT normalized_query_key, query_text, attempts FROM reflect_miss_backlog
             WHERE state = 'open' ORDER BY first_seen ASC, normalized_query_key ASC LIMIT ?1",
        )?;
        let rows = stmt.query_map(rusqlite::params![limit as i64], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)? as u32))
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Count OPEN (unparked) misses (spec §2.1 floor input). `parked`/`no_material`/repaired states are
    /// excluded by construction (only `state = 'open'` counts).
    pub fn open_miss_count(&self) -> Result<usize, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let n: i64 = store.conn().query_row(
            "SELECT COUNT(*) FROM reflect_miss_backlog WHERE state = 'open'", [], |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    /// Move a miss to a terminal or intermediate state (spec §2.2).
    pub fn set_miss_state(&self, key: &str, state: crate::reflect::MissState, now: i64) -> Result<(), BossclawError> {
        let store = self.inner.lock().expect(POISON);
        store.conn().execute(
            "UPDATE reflect_miss_backlog SET state = ?2, updated_at = ?3 WHERE normalized_query_key = ?1",
            rusqlite::params![key, state.as_str(), now],
        )?;
        Ok(())
    }

    /// Increment a miss's attempt count, returning the NEW count (spec §2.2).
    pub fn bump_miss_attempt(&self, key: &str, now: i64) -> Result<u32, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        conn.execute(
            "UPDATE reflect_miss_backlog SET attempts = attempts + 1, updated_at = ?2 WHERE normalized_query_key = ?1",
            rusqlite::params![key, now],
        )?;
        Ok(conn.query_row(
            "SELECT attempts FROM reflect_miss_backlog WHERE normalized_query_key = ?1",
            rusqlite::params![key], |r| r.get::<_, i64>(0),
        )? as u32)
    }

    /// Add to the cumulative operational counters (spec §2.4). Upsert of the single row.
    pub fn add_reflect_counters(&self, refreshed: i64, no_material: i64) -> Result<(), BossclawError> {
        let store = self.inner.lock().expect(POISON);
        store.conn().execute(
            "INSERT INTO reflect_counters (id, refreshed_total, no_material_total) VALUES (0, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET refreshed_total = refreshed_total + ?1,
                                           no_material_total = no_material_total + ?2",
            rusqlite::params![refreshed, no_material],
        )?;
        Ok(())
    }

    /// Read the cumulative `(refreshed_total, no_material_total)` (spec §2.4). `(0,0)` if never written.
    pub fn reflect_counters(&self) -> Result<(u64, u64), BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let row: Option<(i64, i64)> = store.conn().query_row(
            "SELECT refreshed_total, no_material_total FROM reflect_counters WHERE id = 0",
            [], |r| Ok((r.get(0)?, r.get(1)?)),
        ).optional()?;
        let (a, b) = row.unwrap_or((0, 0));
        Ok((a as u64, b as u64))
    }

    /// Read the reflect progress cursor (spec §2.1/§2.4). Default row if never written.
    pub fn reflect_cursor(&self) -> Result<crate::reflect::ReflectCursor, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let row: Option<(i64, i64, i64, i64)> = store.conn().query_row(
            "SELECT last_served_refreshed, last_served_no_material, last_completed_run_at, last_floor_fire_at
             FROM reflect_cursor WHERE id = 0",
            [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        ).optional()?;
        let (a, b, c, d) = row.unwrap_or((0, 0, 0, 0));
        Ok(crate::reflect::ReflectCursor {
            last_served_refreshed: a, last_served_no_material: b,
            last_completed_run_at: c, last_floor_fire_at: d,
        })
    }

    /// Advance the digest last-served totals (spec §2.4; T13 advances only on `source == "startup"`).
    pub fn set_reflect_last_served(&self, refreshed: i64, no_material: i64) -> Result<(), BossclawError> {
        self.upsert_reflect_cursor("last_served_refreshed = ?1, last_served_no_material = ?2",
            rusqlite::params![refreshed, no_material])
    }

    /// Record the wall-clock of the last COMPLETED reflect run (daemon-supplied; §2.1 floor).
    pub fn set_reflect_last_completed_run(&self, at: i64) -> Result<(), BossclawError> {
        self.upsert_reflect_cursor("last_completed_run_at = ?1", rusqlite::params![at])
    }

    /// Record the wall-clock of the last floor fire (daemon-supplied; §2.1 floor re-fire guard).
    pub fn set_reflect_last_floor_fire(&self, at: i64) -> Result<(), BossclawError> {
        self.upsert_reflect_cursor("last_floor_fire_at = ?1", rusqlite::params![at])
    }

    /// Shared single-row upsert for `reflect_cursor` (the row is created with defaults if absent).
    fn upsert_reflect_cursor(&self, set_clause: &str, params: &[&dyn rusqlite::ToSql]) -> Result<(), BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        conn.execute("INSERT OR IGNORE INTO reflect_cursor (id) VALUES (0)", [])?;
        conn.execute(&format!("UPDATE reflect_cursor SET {set_clause} WHERE id = 0"), params)?;
        Ok(())
    }
```

4. Run → PASS: `cargo test -p bossclaw-core normalized_query_key_is_trim_casefold_stable reflect_backlog_seeds_upsert_only_transitions_and_counters_roundtrip`
   Regression: `cargo test -p bossclaw-core --lib` (schema init still opens clean).

5. Commit: `feat(rung4-a): reflect module types + backlog/counters/cursor tables + normalized_query_key`

---

## Task 6 — Query→topic bridge: `reflect_topics_for_query` (core, read-only)

Spec §2.2 step 2 (arch B1). Resolves a missed query to ≤`REFLECT_TOPIC_N` KNOWN topics using the REAL
`entity_search` (`log.rs:6576`), read-only, above `REFLECT_TOPIC_FLOOR`. Reflection NEVER mints (minting is
evolve's job + a write) — a query resolving to no known topic is the `no_material` path. PORTABLE.

**Files**
- Modify: `crates/bossclaw-core/src/log.rs` — `reflect_topics_for_query` (beside `entity_search` `:6576`).
- Test: `crates/bossclaw-core/src/log.rs` `mod tests`.

**Steps**

1. Write the failing test (MockEmbedder-seeded entities: an above-floor match resolves; garbage → empty):

```rust
#[test]
fn reflect_topics_for_query_resolves_known_topics_and_empties_on_garbage() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let emb = MockEmbedder::new(64); // higher dim → distinct vectors for distinct labels
    let m = log.remember(&emb, "seed").unwrap();
    let kenny = log.entity("Kenny Ortega", &[], "person", "test-v1", std::slice::from_ref(&m)).unwrap();
    let _acme = log.entity("Acme Corporation", &[], "org", "test-v1", std::slice::from_ref(&m)).unwrap();
    log.rebuild_entity_index(&emb).unwrap(); // entity_search precondition

    // A query naming a known topic resolves to it (MockEmbedder is deterministic; the exact label
    // embeds to distance 0 → similarity 1.0, well above the floor).
    let topics = log.reflect_topics_for_query(&emb, "Kenny Ortega").unwrap();
    assert!(topics.contains(&kenny), "an above-floor query resolves to the known topic");
    assert!(topics.len() <= crate::reflect::REFLECT_TOPIC_N, "capped at N");

    // A garbage query far from every label → empty → the no_material path.
    let none = log.reflect_topics_for_query(&emb, "zzzz qqqq unrelated gibberish 9182").unwrap();
    assert!(none.is_empty(), "no known topic above the floor → empty (no_material)");
}
```

2. Run → FAIL: `cargo test -p bossclaw-core reflect_topics_for_query_resolves_known_topics_and_empties_on_garbage`
   Expected: `no method named reflect_topics_for_query`.

3. Implement (beside `entity_search` `:6576`):

```rust
    /// Resolve a missed query to ≤ [`crate::reflect::REFLECT_TOPIC_N`] KNOWN topic entity ids (spec §2.2
    /// step 2, the read-only bridge). Uses [`EventLog::entity_search`] over the entity-resolution index and
    /// keeps only candidates at/above [`crate::reflect::REFLECT_TOPIC_FLOOR`] cosine SIMILARITY
    /// (`1.0 - dist`, mirroring `resolve_mention`). NEVER mints (minting is a write, evolve's job): an empty
    /// result IS the `no_material` signal. Requires the entity index built (the reflect wrapper rebuilds it
    /// pre-tick, like evolve); a fresh brain with zero entities returns an empty index → empty result.
    pub fn reflect_topics_for_query(
        &self,
        embedder: &dyn Embedder,
        query: &str,
    ) -> Result<Vec<String>, BossclawError> {
        Ok(self
            .entity_search(embedder, query, crate::reflect::REFLECT_TOPIC_N)?
            .into_iter()
            .filter(|(_, dist)| 1.0 - dist >= crate::reflect::REFLECT_TOPIC_FLOOR)
            .map(|(id, _)| id)
            .collect())
    }
```

4. Run → PASS: `cargo test -p bossclaw-core reflect_topics_for_query_resolves_known_topics_and_empties_on_garbage`

5. Commit: `feat(rung4-a): reflect_topics_for_query bridge (read-only entity_search, floor + N)`

---

## Task 7 — Miss-attempt pipeline: `attempt_miss` (core)

Spec §2.2 (the four-step per-miss pipeline). One open miss → recall (hit → `RepairedByTime`) → bridge (empty →
`NoMaterial`) → refresh each resolved topic → replay recall (hit → `CandidateRepaired`; miss → bump attempt, at
budget → `Parked`). State transitions persisted via T5. PORTABLE. Deterministic under
`ScriptedReasoner`/`MockEmbedder`.

**Within-tick visibility note (documented, accepted).** The step-4 replay recall runs BEFORE the wrapper's
post-tick `rebuild_indexes`, so a just-emitted dossier surfaces via the KEYWORD arm only (the vector arm folds
it at the wrapper post-tick — `recall` lifecycle note, `log.rs:1783-1789`). This is sufficient for
`CandidateRepaired`, which is explicitly OPERATIONAL ("the mechanism fired"), not evidence (§5.1); the harness
gate (T14) — not this counter — carries the SHIP burden.

**Files**
- Modify: `crates/bossclaw-core/src/log.rs` — `attempt_miss` (in the reflection section from T5). Uses
  `recall` (`:1790`), `reflect_topics_for_query` (T6), `all_entities` (`:2669`), `refresh_topic_page` (T4),
  `bump_miss_attempt`/`set_miss_state` (T5).
- Test: `crates/bossclaw-core/src/log.rs` `mod tests`.

**Steps**

1. Write the failing test (each outcome path incl. 3-attempt parking; assert states + outcomes, not internals):

```rust
#[test]
fn attempt_miss_covers_repaired_by_time_no_material_candidate_and_parking() {
    use crate::reflect::{normalized_query_key, MissOutcome, MissState};
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let emb = MockEmbedder::new(64);
    log.rebuild_entity_index(&emb).unwrap(); // empty index is valid

    // ── no_material: a query resolving to no known topic (fresh brain, no entities). ──
    let qk = normalized_query_key("who is nobody?");
    log.seed_miss(&qk, "who is nobody?", 10).unwrap();
    let empty_reasoner = crate::reason::ScriptedReasoner::new("test-v1");
    let out = log.attempt_miss(&emb, &empty_reasoner, &qk, "who is nobody?", 20).unwrap();
    assert_eq!(out, MissOutcome::NoMaterial);

    // ── repaired_by_time: a query that recall now answers (a matching memory exists). ──
    let mem = log.remember(&emb, "The capital of France is Paris.").unwrap();
    log.rebuild_indexes(&emb).unwrap(); // make the memory recall-visible
    let rk = normalized_query_key("The capital of France is Paris.");
    log.seed_miss(&rk, "The capital of France is Paris.", 10).unwrap();
    let out = log.attempt_miss(&emb, &empty_reasoner, &rk, "The capital of France is Paris.", 20).unwrap();
    assert_eq!(out, MissOutcome::RepairedByTime);
    let _ = mem;

    // ── parking: a query that resolves to a known topic but the refresh never makes recall hit. After
    //    REFLECT_MISS_ATTEMPT_BUDGET(3) failed attempts the caller-side transition parks it. ──
    // (Build a topic whose dossier text does NOT contain the miss query's keywords, so the replay stays a
    //  miss; script the compose so refresh_topic_page emits.) Assert the third attempt returns Parked.
    // [full seeding mirrors T4's seed_topic_directly-style setup; the load-bearing assertion:]
    // let out3 = log.attempt_miss(&emb, &reasoner, &pk, "unrelated phrasing", 40).unwrap();
    // assert_eq!(out3, MissOutcome::Parked);
    // assert_eq!(open_state(&log, &pk), MissState::Parked.as_str());
}
```

   (The parking sub-case's full topic seeding follows the T4 idiom; the executor completes it so the third
   `attempt_miss` returns `Parked` and the backlog row reads `parked`.)

2. Run → FAIL: `cargo test -p bossclaw-core attempt_miss_covers_repaired_by_time_no_material_candidate_and_parking`
   Expected: `no method named attempt_miss`.

3. Implement (in the reflection section):

```rust
    /// Attempt to repair ONE open miss (spec §2.2 steps 1-4). Persists the resulting state via the backlog
    /// accessors and returns the [`crate::reflect::MissOutcome`] for the tick tally. PORTABLE. The refresh
    /// (step 3) recomposes the resolved entities' OWN lineage through the shared, §2.3-excluded gather — it
    /// injects NO new material and mints nothing (I1), so a true repair occurs only where a known topic's
    /// dossier was under-composed or stale (reach is LOW BY DESIGN; §5.3(d) measures it).
    pub fn attempt_miss(
        &self,
        embedder: &dyn Embedder,
        reasoner: &dyn crate::reason::Reasoner,
        key: &str,
        query: &str,
        now: i64,
    ) -> Result<crate::reflect::MissOutcome, BossclawError> {
        use crate::reflect::{MissOutcome, MissState, TopicRefreshOutcome, REFLECT_MISS_ATTEMPT_BUDGET, REFLECT_RECALL_K};
        let opts = crate::recall::RecallOptions::default();
        // 1. Re-run recall. Hit → repaired_by_time (no reasoner call).
        if !self.recall(embedder, query, REFLECT_RECALL_K, &opts)?.is_empty() {
            self.set_miss_state(key, MissState::RepairedByTime, now)?;
            return Ok(MissOutcome::RepairedByTime);
        }
        // 2. Resolve query → known topics. Empty → no_material.
        let topics = self.reflect_topics_for_query(embedder, query)?;
        if topics.is_empty() {
            self.set_miss_state(key, MissState::NoMaterial, now)?;
            return Ok(MissOutcome::NoMaterial);
        }
        // 3. Refresh each resolved topic's dossier (per-item error isolation).
        let entities = self.all_entities()?;
        let mut reasoner_errored = false;
        for tid in &topics {
            if let Some(entity) = entities.iter().find(|e| &e.entity_id == tid) {
                if self.refresh_topic_page(reasoner, entity)? == TopicRefreshOutcome::ReasonerError {
                    reasoner_errored = true;
                }
            }
        }
        // 4. Replay recall. Hit → candidate_repaired.
        if !self.recall(embedder, query, REFLECT_RECALL_K, &opts)?.is_empty() {
            self.set_miss_state(key, MissState::CandidateRepaired, now)?;
            return Ok(MissOutcome::CandidateRepaired);
        }
        // Still missing: bump the attempt; at budget → parked (bounded loss, I6).
        let attempts = self.bump_miss_attempt(key, now)?;
        if attempts >= REFLECT_MISS_ATTEMPT_BUDGET {
            self.set_miss_state(key, MissState::Parked, now)?;
            return Ok(MissOutcome::Parked);
        }
        Ok(if reasoner_errored { MissOutcome::ReasonerError } else { MissOutcome::StillMissing })
    }
```

4. Run → PASS: `cargo test -p bossclaw-core attempt_miss_covers_repaired_by_time_no_material_candidate_and_parking`

5. Commit: `feat(rung4-a): attempt_miss pipeline (recall → bridge → refresh → replay; budget → parked)`

---

## Task 8 — Stale-dossier refresh: `refresh_stale_pages` (core)

Spec §2.3 (the one autonomous tidy job) + the thin-set residual. Scans current pages for cited ids ∈
retired∪superseded; for each stale page (≤`cap`), refresh; count `healed`/`unchanged`/`unhealable_thin`.
PORTABLE.

**Convergence + no-forever-retry.** A page below `PAGE_MIN_FACTS` after exclusion cannot legally re-emit
(`refresh_topic_page` returns `SkippedThin`) and reflection never retires pages (I1), so it stays
stale-but-provenance-true. This pass counts it `unhealable_thin` (a distinct outcome, not an error) and does not
re-attempt it WITHIN the pass (a per-call `attempted` set); across nights the cited-set idempotency + the
per-tick budget keep it from wasting nights (§2.3).

**Files**
- Modify: `crates/bossclaw-core/src/log.rs` — `refresh_stale_pages` (reflection section). Uses `current_pages`
  (`:3706`), `fold_sessions`, `all_entities`, `refresh_topic_page` (T4), `current_page_for_topic` (`:8021`).
- Test: `crates/bossclaw-core/src/log.rs` `mod tests` (healed case from T3's seam; thin case).

**Steps**

1. Write the failing test:

```rust
#[test]
fn refresh_stale_pages_heals_and_counts_unhealable_thin() {
    use crate::reflect::TopicRefreshOutcome;
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let emb = MockEmbedder::new(64);
    // A topic with TWO cited sources + an edge → a page. Retire ONE source → the page is stale but still
    // summary-worthy → healed. A SECOND topic with ONE cited source → retire it → below PAGE_MIN_FACTS →
    // unhealable_thin.
    let m1 = log.remember(&emb, "Kenny works at Acme.").unwrap();
    let m2 = log.remember(&emb, "Kenny lives in Denver.").unwrap();
    let heal_lineage = vec![m1.clone(), m2.clone()];
    let kenny = log.entity("Kenny", &[], "person", "test-v1", &heal_lineage).unwrap();
    let acme = log.entity("Acme", &[], "org", "test-v1", &heal_lineage).unwrap();
    log.link_machine(&kenny, "works_at", &acme, 0.9, "test-v1", &heal_lineage).unwrap();
    log.rebuild_graph().unwrap();
    let ke = log.all_entities().unwrap().into_iter().find(|e| e.entity_id == kenny).unwrap();
    let f = log.gather_fact_set(&ke).unwrap();
    let r_heal = crate::reason::ScriptedReasoner::new("test-v1").with_response(
        crate::summarize::SUMMARIZE_SYSTEM, &crate::summarize::build_compose_prompt(&f),
        serde_json::json!({ "title": "Kenny", "claims": [{ "text": "Kenny works at Acme.", "cites": [m1.clone(), m2.clone()] }]}));
    assert_eq!(log.refresh_topic_page(&r_heal, &ke).unwrap(), TopicRefreshOutcome::Emitted { superseded: false });
    log.rebuild_graph().unwrap();

    // Retire m1 → the Kenny page's cited set now intersects retired → stale → healed on refresh (its
    // post-exclusion facts still clear PAGE_MIN_FACTS: one memory m2 + one edge). Script the healed compose.
    log.retire_memory(&m1, None).unwrap();
    let ke2 = log.all_entities().unwrap().into_iter().find(|e| e.entity_id == kenny).unwrap();
    let f2 = log.gather_fact_set(&ke2).unwrap();
    let r_healed = crate::reason::ScriptedReasoner::new("test-v1").with_response(
        crate::summarize::SUMMARIZE_SYSTEM, &crate::summarize::build_compose_prompt(&f2),
        serde_json::json!({ "title": "Kenny", "claims": [{ "text": "Kenny lives in Denver.", "cites": [m2.clone()] }]}));
    let report = log.refresh_stale_pages(&r_healed, 8).unwrap();
    assert_eq!(report.healed, 1, "the stale Kenny page healed");
    assert_eq!(report.unhealable_thin, 0);

    // Thin case: a topic with ONE cited source; retire it → its whole lineage is retired → below
    // PAGE_MIN_FACTS → unhealable_thin (page unchanged, not an error).
    // [seed a single-source topic + page, retire its only source, then:]
    // let thin_report = log.refresh_stale_pages(&any_reasoner, 8).unwrap();
    // assert_eq!(thin_report.unhealable_thin, 1, "an all-retired-lineage page is unhealable_thin");
    // assert_eq!(thin_report.healed, 0);
}
```

   (The executor completes the thin sub-case with a single-source topic whose only source is retired.)

2. Run → FAIL: `cargo test -p bossclaw-core refresh_stale_pages_heals_and_counts_unhealable_thin`
   Expected: `no method named refresh_stale_pages`.

3. Implement (reflection section):

```rust
    /// Refresh dossiers whose cited sources went stale (spec §2.3). A page is STALE when its cited
    /// `source_event_ids` intersect `superseded ∪ retired_notes` (the aftermath of a Rung-3 retire).
    /// For each stale topic (≤ `cap`), call [`EventLog::refresh_topic_page`] and tally the outcome:
    /// `Emitted` → healed, `SkippedUnchanged` → unchanged, `SkippedThin` → `unhealable_thin` (the §2.3
    /// residual — an all-retired lineage cannot re-emit; counted, never retried within this pass),
    /// `ReasonerError` → reasoner_errors (isolated). Reflection NEVER retires a page (I1). PORTABLE.
    pub fn refresh_stale_pages(
        &self,
        reasoner: &dyn crate::reason::Reasoner,
        cap: usize,
    ) -> Result<crate::reflect::StaleRefreshReport, BossclawError> {
        use crate::reflect::{StaleRefreshReport, TopicRefreshOutcome};
        let fold = fold_sessions(&self.session_events_ordered()?);
        // Nothing gone → nothing stale (the fresh-corpus fast path).
        if fold.superseded.is_empty() && fold.retired_notes.is_empty() {
            return Ok(StaleRefreshReport::default());
        }
        let entities = self.all_entities()?;
        let mut report = StaleRefreshReport::default();
        let mut attempted: std::collections::HashSet<String> = std::collections::HashSet::new();
        for page in self.current_pages()? {
            if report.healed + report.unchanged + report.unhealable_thin >= cap {
                break; // per-tick budget
            }
            if !attempted.insert(page.topic_id.clone()) {
                continue; // one refresh per topic per pass
            }
            // Read the page's cited set; stale iff it intersects the gone set.
            let Some((_pid, cites)) = self.current_page_for_topic(&page.topic_id)? else { continue };
            let stale = cites.iter().any(|id| fold.superseded.contains(id) || fold.retired_notes.contains(id));
            if !stale {
                continue;
            }
            let Some(entity) = entities.iter().find(|e| e.entity_id == page.topic_id) else { continue };
            match self.refresh_topic_page(reasoner, entity)? {
                TopicRefreshOutcome::Emitted { .. } => report.healed += 1,
                TopicRefreshOutcome::SkippedUnchanged => report.unchanged += 1,
                TopicRefreshOutcome::SkippedThin => report.unhealable_thin += 1,
                TopicRefreshOutcome::ReasonerError => report.reasoner_errors += 1,
            }
        }
        Ok(report)
    }
```

4. Run → PASS: `cargo test -p bossclaw-core refresh_stale_pages_heals_and_counts_unhealable_thin`

5. Commit: `feat(rung4-a): refresh_stale_pages tidy job (heal / unchanged / unhealable_thin residual)`

---

## Task 9 — Orchestrator: `reflect_once` + `ReflectReport` (core)

Spec §2.1 (budget/order) / §2.4 (report). Off-switch first (like `evolve_once:8277`), seed the backlog from
the tick's new misses, attempt ≤`REFLECT_MISSES_PER_TICK` open misses, then `refresh_stale_pages`
(≤`REFLECT_REFRESH_PER_TICK`), update cumulative counters, return the `ReflectReport`. Per-item error isolation.
PORTABLE.

**Files**
- Modify: `crates/bossclaw-core/src/log.rs` — `reflect_once` (reflection section). Uses `reflect_enabled`
  (T1), `seed_miss`/`open_misses`/`add_reflect_counters` (T5), `attempt_miss` (T7), `refresh_stale_pages` (T8),
  `normalized_query_key` (T5).
- Test: `crates/bossclaw-core/src/log.rs` `mod tests`.

**Steps**

1. Write the failing test (off-switch no-op; a tick that seeds + attempts + refreshes yields a populated report):

```rust
#[test]
fn reflect_once_is_off_by_default_then_reports_when_enabled() {
    use crate::reflect::normalized_query_key;
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let emb = MockEmbedder::new(64);
    log.rebuild_entity_index(&emb).unwrap();
    let reasoner = crate::reason::ScriptedReasoner::new("test-v1");

    // Off by default → skipped_disabled, nothing else (I3).
    let off = log.reflect_once(&emb, &reasoner, &["who is nobody?".to_string()], 100).unwrap();
    assert!(off.skipped_disabled, "reflect is off by default");
    assert_eq!(off.attempted, 0, "no work when disabled");
    assert!(log.open_misses(10).unwrap().is_empty(), "no backlog seeded when disabled");

    // Enable → the new miss is seeded and attempted; with no known topic it resolves no_material.
    log.set_reflect_enabled(true).unwrap();
    let r = log.reflect_once(&emb, &reasoner, &["who is nobody?".to_string()], 200).unwrap();
    assert!(!r.skipped_disabled);
    assert_eq!(r.attempted, 1, "one open miss attempted");
    assert_eq!(r.no_material, 1, "resolves to no known topic → no_material");
    let (_refreshed, no_material_total) = log.reflect_counters().unwrap();
    assert_eq!(no_material_total, 1, "cumulative no_material_total advanced");
    // The miss is now terminal (no_material) → not re-attempted next tick.
    let qk = normalized_query_key("who is nobody?");
    assert!(log.open_misses(10).unwrap().iter().all(|(k, _, _)| k != &qk), "no_material miss left open set");
}
```

2. Run → FAIL: `cargo test -p bossclaw-core reflect_once_is_off_by_default_then_reports_when_enabled`
   Expected: `no method named reflect_once`.

3. Implement (reflection section):

```rust
    /// Run ONE reflect tick (spec §2.1/§2.4). Off-switch FIRST (no work when disabled, I3). Seeds the tick's
    /// `new_misses` into the backlog (upsert-if-new), attempts ≤ `REFLECT_MISSES_PER_TICK` open misses
    /// (misses first), then refreshes ≤ `REFLECT_REFRESH_PER_TICK` stale pages, updates the cumulative
    /// counters, and returns the `ReflectReport`. Per-item errors are isolated (a poisoned miss/page counts
    /// `reasoner_errors` and the tick continues — the Rung-3 poison lesson at item granularity, I6).
    /// `now` is daemon-supplied (clock-free core). PORTABLE.
    pub fn reflect_once(
        &self,
        embedder: &dyn Embedder,
        reasoner: &dyn crate::reason::Reasoner,
        new_misses: &[String],
        now: i64,
    ) -> Result<crate::reflect::ReflectReport, BossclawError> {
        use crate::reflect::{normalized_query_key, MissOutcome, ReflectReport,
            REFLECT_MISSES_PER_TICK, REFLECT_REFRESH_PER_TICK};
        let mut report = ReflectReport::default();
        if !self.reflect_enabled()? {
            report.skipped_disabled = true;
            return Ok(report);
        }
        // Seed the ring's queries into the durable backlog (upsert-if-new; churn cannot drop a seen miss).
        for q in new_misses {
            self.seed_miss(&normalized_query_key(q), q, now)?;
        }
        // Attempt the oldest open misses (misses first, spec §2.1 priority).
        for (key, query, _attempts) in self.open_misses(REFLECT_MISSES_PER_TICK)? {
            report.attempted += 1;
            match self.attempt_miss(embedder, reasoner, &key, &query, now)? {
                MissOutcome::RepairedByTime => report.repaired_by_time += 1,
                MissOutcome::NoMaterial => report.no_material += 1,
                MissOutcome::CandidateRepaired => report.candidate_repaired += 1,
                MissOutcome::Parked => report.parked += 1,
                MissOutcome::ReasonerError => report.reasoner_errors += 1,
                MissOutcome::StillMissing => {}
            }
        }
        // Then the stale-dossier tidy with the remaining budget.
        let stale = self.refresh_stale_pages(reasoner, REFLECT_REFRESH_PER_TICK)?;
        report.dossiers_refreshed = stale.healed;
        report.unhealable_thin = stale.unhealable_thin;
        report.reasoner_errors += stale.reasoner_errors;
        // Cumulative counters (spec §2.4): dossiers refreshed = miss-driven candidate_repaired's refreshes
        // + tidy heals; no_material is the owner's most actionable signal.
        self.add_reflect_counters((report.dossiers_refreshed + report.candidate_repaired) as i64,
                                  report.no_material as i64)?;
        Ok(report)
    }
```

4. Run → PASS: `cargo test -p bossclaw-core reflect_once_is_off_by_default_then_reports_when_enabled`
   Regression (core reflect suite): `cargo test -p bossclaw-core reflect attempt_miss refresh_stale_pages refresh_topic_page`

5. Commit: `feat(rung4-a): reflect_once orchestrator + ReflectReport (off-switch, budgets, counters)`

---

## Task 10 — Engine wrapper `reflect_once` + `ReflectTelemetry` + disclosure header (daemon)

Spec §2.1 (serialization/consent/writes) / §2.4 (telemetry + disclosure). Adds `EngineHandle::reflect_once`
mirroring the `evolve_once` wrapper (`:956`): dedicated `reflect_lock.try_lock()` → `Busy`, `cloud_consent_ok`
pre-gate, the SP3 miss-ring read, `ensure_indexed`, `spawn_blocking { rebuild_entity_index (pre, the bridge
precondition); core reflect_once; rebuild_indexes + rebuild_graph (post, dossiers become recall-visible) }`,
`record_reflect_tick`, and the floor's last-completed-run stamp. `engine`/`telemetry` are `#[cfg(unix)]` modules
→ no per-fn gates. `now` is supplied by the sweeper boundary (mirrors `detect_conflicts_once(onboarded, now)`).

**Miss-ring read (deviation, documented).** Reflection reads the ring NON-destructively via the existing public
`Telemetry::stats()` (extracting the query strings), NOT a draining `take_recent_misses`. The durable backlog's
`seed_miss` is upsert-if-new, so re-reading the same ≤20 queries every tick is idempotent (a terminal miss is
never reset); a non-destructive read also PRESERVES the App's `RecallStats` "recent misses" view. The disclosure
doc-header still updates (the store now actively drives reflection).

**Files**
- Modify: `crates/bossclawd/src/engine/mod.rs` — `reflect_lock`/`reflect_tel` fields (`:279-303`) + `new()`
  init (`:336-342`); `ReflectTelemetry` (beside `ConflictTelemetry` `:258`); `record_reflect_tick` +
  `record_reflect_tick_into` (beside `record_tick`/`record_tick_into`); `reflect_once` wrapper (beside
  `evolve_once` `:956`).
- Modify: `crates/bossclawd/src/telemetry.rs` — the module doc-header (`:1`,`:5-7`,`:21-25`).
- Test: `crates/bossclawd/src/engine/mod.rs` `mod tests` (a Local-mode enabled tick + a `Busy` overlap).

**Steps**

1. Write the failing tests (in `engine/mod.rs mod tests`, mirroring the conflict-sweep engine e2e):

```rust
    #[tokio::test]
    async fn engine_reflect_once_runs_when_enabled_and_records_telemetry() {
        crate::vault::seed_secret_cache_for_test(Default::default());
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("identity.json"), serde_json::json!({
            "did": "did:wba:example.com:tester", "name": "Tester",
            "created_at": "2026-07-11T00:00:00+00:00" }).to_string()).unwrap();
        let engine = EngineHandle::new(
            crate::server::shared_test_vault(), dir.path().to_path_buf(),
            Arc::new(embed::MockEmbedderProvider::new(64)),
            Arc::new(reason::MockReasonerProvider::from_reasoner(
                Arc::new(bossclaw_core::ScriptedReasoner::new("test")))));
        let onboarded = true;
        engine.set_reflect_enabled(onboarded, true).await.unwrap();
        // A fresh brain with no misses/pages → an empty-but-successful tick (not skipped_disabled).
        let report = engine.reflect_once(onboarded, 1000).await.expect("reflect tick runs");
        assert!(!report.skipped_disabled, "enabled → runs");
        assert_eq!(report.attempted, 0, "no seeded misses yet");
    }
```

2. Run → FAIL: `cargo test -p bossclawd --lib engine_reflect_once_runs_when_enabled_and_records_telemetry`
   Expected: `no method named reflect_once` / `no method named set_reflect_enabled` (set_reflect_enabled lands
   in T12; land T10's wrapper first and this test after T12, or add a temporary in-test enable via the log).

3. Implement.
   (a) `ReflectTelemetry` (beside `ConflictTelemetry` `:258`):

```rust
/// Session-scoped reflection telemetry (mirrors [`ConflictTelemetry`]; in-memory, cleared on restart — the
/// durable operational totals live in the core `reflect_counters` table). Written by `record_reflect_tick`.
#[derive(Debug, Default, Clone)]
pub struct ReflectTelemetry {
    /// Wall-clock duration of the most recent tick, ms.
    pub last_tick_ms: Option<u128>,
    /// Ticks that returned an engine error this session.
    pub error_count: usize,
    /// The most recent tick error (truncated ~512 bytes — it can embed paths/reasoner output).
    pub last_error: Option<String>,
    /// Cumulative session tallies from the per-tick `ReflectReport` (the scoreboard, §2.4).
    pub dossiers_refreshed_total: usize,
    pub no_material_total: usize,
    pub parked_total: usize,
    pub unhealable_thin_total: usize,
    pub reasoner_errors_total: usize,
}
```

   (b) Fields (`:279-303`, after `conflict_tel`) + init (`:336-342`):

```rust
    /// Serializes manual + scheduled reflect ticks (`try_lock` → `Busy("reflect")`). DEDICATED, NOT reused
    /// `evolve_lock`: a long evolve tick must never block a reflect tick and vice-versa (the Rung-3
    /// dedicated-lock lesson).
    reflect_lock: Mutex<()>,
    /// Session reflection telemetry (poison-recovered on read). Mirrors `conflict_tel`.
    reflect_tel: std::sync::Mutex<ReflectTelemetry>,
```
```rust
            reflect_lock: Mutex::new(()),
            reflect_tel: std::sync::Mutex::new(ReflectTelemetry::default()),
```

   (c) `record_reflect_tick` + pure `record_reflect_tick_into` (beside `record_tick`/`record_tick_into`):

```rust
    /// Record one reflect tick's telemetry (thin `&self` wrapper over the pure recorder).
    fn record_reflect_tick(&self, ms: u128, result: &Result<bossclaw_core::ReflectReport, EngineOpError>) {
        record_reflect_tick_into(&self.reflect_tel, ms, result);
    }
```
```rust
/// Pure reflect-tick recorder. The lock is poison-RECOVERED (a panicked tick must not wedge the status
/// read); `last_tick_ms` is always set; on `Err` `error_count` bumps and `last_error` is stored TRUNCATED to
/// ~512 bytes; on `Ok` the report's counters fold into the session totals (the §2.4 scoreboard).
fn record_reflect_tick_into(
    tel: &std::sync::Mutex<ReflectTelemetry>,
    ms: u128,
    result: &Result<bossclaw_core::ReflectReport, EngineOpError>,
) {
    let mut tel = tel.lock().unwrap_or_else(|p| p.into_inner());
    tel.last_tick_ms = Some(ms);
    match result {
        Err(e) => {
            tel.error_count += 1;
            let mut s = e.to_string();
            truncate_on_char_boundary(&mut s, 512);
            tel.last_error = Some(s);
        }
        Ok(r) => {
            tel.dossiers_refreshed_total += r.dossiers_refreshed;
            tel.no_material_total += r.no_material;
            tel.parked_total += r.parked;
            tel.unhealable_thin_total += r.unhealable_thin;
            tel.reasoner_errors_total += r.reasoner_errors;
        }
    }
}
```

   (d) The `reflect_once` wrapper (beside `evolve_once` `:956`):

```rust
    /// Run ONE reflect tick (gated, serialized). `reflect_lock.try_lock()` (`Busy("reflect")` on overlap) →
    /// `cloud_consent_ok` (BEFORE the reasoner is built, so a cloud-not-ready tick egresses nothing, I2) →
    /// read the SP3 miss-ring queries (non-destructive) → `ensure_indexed` → `spawn_blocking`:
    /// `rebuild_entity_index` (the query→topic bridge precondition) THEN core `reflect_once` THEN
    /// `rebuild_indexes` + `rebuild_graph` (so a follow-up recall sees the refreshed dossiers) → record
    /// telemetry → stamp the floor's last-completed marker. `now` is the sweeper-boundary clock.
    pub async fn reflect_once(&self, onboarded: bool, now: i64) -> Result<bossclaw_core::ReflectReport, EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        let _guard = self.reflect_lock.try_lock().map_err(|_| EngineOpError::Busy("reflect"))?;
        if !self.cloud_consent_ok(onboarded).await {
            return Err(EngineOpError::Reasoner(
                "cloud reasoner not ready — signed consent or provider key missing".to_string(),
            ));
        }
        // Non-destructive read of the SP3 miss ring (the durable backlog dedups re-seeds; preserves RecallStats).
        let new_misses: Vec<String> = self
            .data_dir()
            .and_then(|d| crate::telemetry::Telemetry::open(d).ok())
            .and_then(|t| t.stats().ok())
            .map(|s| s.recent_misses.into_iter().map(|m| m.query).collect())
            .unwrap_or_default();
        let embedder = self.ensure_indexed(&log).await?;
        let reasoner = self.reasoner_provider.reasoner()?;
        let t0 = std::time::Instant::now();
        let result = spawn_blocking({
            let log = log.clone();
            let emb = embedder.clone();
            move || -> Result<bossclaw_core::ReflectReport, EngineOpError> {
                log.rebuild_entity_index(&*emb).map_err(|e| EngineOpError::Core(e.to_string()))?;
                let report = log
                    .reflect_once(&*emb, &*reasoner, &new_misses, now)
                    .map_err(|e| EngineOpError::Core(e.to_string()))?;
                log.rebuild_indexes(&*emb).map_err(|e| EngineOpError::Core(e.to_string()))?;
                log.rebuild_graph().map_err(|e| EngineOpError::Core(e.to_string()))?;
                Ok(report)
            }
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?;
        self.record_reflect_tick(t0.elapsed().as_millis(), &result);
        // On a completed tick, stamp the last-completed-run marker (best-effort; backs the §2.1 floor).
        if result.is_ok() {
            let log2 = log.clone();
            let _ = spawn_blocking(move || log2.set_reflect_last_completed_run(now)).await;
        }
        result
    }
```

   (e) `telemetry.rs` doc-header — revise the passive-signal framing (`:1`,`:5-7`,`:21-25`) to name the active
   machine consumer. Change line 1 to:

```rust
//! Recall-miss telemetry (SP3 A12) — the retrieval-floor tuning signal (spec §8) AND, since Rung-4 R4-A,
//! the ACTIVE input to the reflect loop, which reads these miss queries each quiet tick to repair coverage
//! gaps (design §2.4). With cloud consent ON, gathered material may egress under the existing consent.
```
   And in the "Privacy surface" block (`:21-25`) append one sentence:

```rust
//! Since Rung-4 R4-A the reflect loop READS these miss queries (never the results) to drive dossier
//! refreshes; queries stay local unless the owner has enabled the cloud reasoner (I2). Still QUERIES ONLY.
```

4. Run → PASS: `cargo test -p bossclawd --lib engine_reflect_once_runs_when_enabled_and_records_telemetry`
   Regression: `cargo test -p bossclawd --lib engine::` (the evolve/conflict wrappers untouched).

5. Commit: `feat(rung4-a): EngineHandle::reflect_once wrapper + ReflectTelemetry + active-consumer disclosure`

---

## Task 11 — `decide_reflect` pure gate + the reflect sweeper (daemon)

Spec §2.1 (the tick gate, quiet, starvation floor, evolve-backlog defer, precedence). A new `#[cfg(unix)] pub mod
reflect` daemon module — the conflict-sweeper spawn shape PLUS a capture-style PURE `decide_reflect` (so the
gate truth table, incl. floor-overrides-both, is unit-tested without a brain). Also adds the two cheap core
readers the gate needs.

**Files**
- Modify: `crates/bossclaw-core/src/log.rs` — `newest_memory_activity_at` (the quiet reader; `open_miss_count`
  landed in T5).
- Create: `crates/bossclawd/src/reflect/mod.rs` (`pub mod sweeper; pub use sweeper::*;`) and
  `crates/bossclawd/src/reflect/sweeper.rs`.
- Modify: `crates/bossclawd/src/lib.rs` — `#[cfg(unix)] pub mod reflect;` (beside `conflict` `:50`).
- Modify: `crates/bossclawd/src/engine/mod.rs` — `reflect_gate_inputs` (one `spawn_blocking` returning the
  gate's core reads); `reflect_reasoner_ready` (the `select_ready` dance, factored from the scheduler).
- Modify: `crates/bossclawd/src/main.rs` — spawn the reflect sweeper (after `:162`).
- Test: `crates/bossclaw-core/src/log.rs mod tests` (the quiet reader) + `crates/bossclawd/src/reflect/sweeper.rs
  mod tests` (the `decide_reflect` truth table).

**Steps**

1. Write the failing tests.
   (a) Core quiet reader (in `log.rs mod tests`):

```rust
#[test]
fn newest_memory_activity_at_tracks_memory_and_session_appends() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let emb = MockEmbedder::new(8);
    assert_eq!(log.newest_memory_activity_at().unwrap(), None, "no activity yet → None (treated as quiet)");
    log.remember(&emb, "a note").unwrap();
    let t = log.newest_memory_activity_at().unwrap().expect("a memory append registers activity");
    assert!(t > 0, "epoch seconds of the newest memory-class event");
}
```

   (b) `decide_reflect` truth table (in `reflect/sweeper.rs mod tests`):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> ReflectDecisionInput {
        ReflectDecisionInput {
            onboarded: true, reflect_enabled: true, reasoner_ready: true, now: 1_000_000,
            newest_activity_at: Some(1_000_000 - REFLECT_QUIET_SECS), // exactly quiet
            evolve_enabled: false, evolve_queue_depth: 0, open_unparked_misses: 0,
            last_completed_run_at: 1_000_000, last_floor_fire_at: 1_000_000,
        }
    }

    #[test]
    fn gate_off_when_not_onboarded_or_disabled_or_reasoner_down() {
        for i in [
            ReflectDecisionInput { onboarded: false, ..base() },
            ReflectDecisionInput { reflect_enabled: false, ..base() },
        ] { assert_eq!(decide_reflect(&i), ReflectDecision::GatedOff); }
        assert_eq!(decide_reflect(&ReflectDecisionInput { reasoner_ready: false, ..base() }),
            ReflectDecision::ReasonerUnavailable);
    }

    #[test]
    fn runs_only_when_quiet() {
        assert_eq!(decide_reflect(&base()), ReflectDecision::Run { floor_fired: false });
        let noisy = ReflectDecisionInput { newest_activity_at: Some(1_000_000 - 1), ..base() };
        assert_eq!(decide_reflect(&noisy), ReflectDecision::NotQuiet);
        let never = ReflectDecisionInput { newest_activity_at: None, ..base() };
        assert_eq!(decide_reflect(&never), ReflectDecision::Run { floor_fired: false }, "no activity ever = quiet");
    }

    #[test]
    fn defers_to_evolve_backlog_unless_floor_fires() {
        let backlogged = ReflectDecisionInput { evolve_enabled: true, evolve_queue_depth: 3, ..base() };
        assert_eq!(decide_reflect(&backlogged), ReflectDecision::DeferredEvolveBacklog);
    }

    #[test]
    fn floor_overrides_both_quiet_and_evolve_backlog() {
        // Not quiet AND evolve-backlogged, but a long-stale unparked miss → the floor fires anyway (§2.1).
        let wedged = ReflectDecisionInput {
            newest_activity_at: Some(1_000_000 - 1), // NOT quiet
            evolve_enabled: true, evolve_queue_depth: 5, // backlogged
            open_unparked_misses: 2,
            last_completed_run_at: 1_000_000 - REFLECT_STALENESS_FLOOR_SECS - 1,
            last_floor_fire_at: 1_000_000 - REFLECT_STALENESS_FLOOR_SECS - 1,
            ..base()
        };
        assert_eq!(decide_reflect(&wedged), ReflectDecision::Run { floor_fired: true });
        // But the floor fires at most once per interval: a recent floor fire → no re-fire (falls through
        // to the ordinary gate, which here is NotQuiet).
        let recent = ReflectDecisionInput { last_floor_fire_at: 1_000_000 - 10, ..wedged };
        assert_eq!(decide_reflect(&recent), ReflectDecision::NotQuiet);
    }
}
```

2. Run → FAIL: `cargo test -p bossclaw-core newest_memory_activity_at_tracks_memory_and_session_appends` and
   `cargo test -p bossclawd --lib reflect::sweeper` — both fail to compile (missing symbols).

3. Implement.
   (a) Core `newest_memory_activity_at` (in `log.rs`, near the other `events` readers):

```rust
    /// The epoch-seconds ts of the NEWEST memory-class event (`memory` ∪ `session_captured`), or `None`
    /// if none exist (spec §2.1 quiet predicate; the daemon computes `now - this >= REFLECT_QUIET_SECS`).
    /// Reads the newest by `seq DESC` and parses its RFC3339 `ts` to epoch (clock-free: a stored ts, not
    /// the wall clock). A table scan is acceptable at the 300s cadence (spec §2.1 — same cost class as
    /// `dirty_entities_since`; add a partial `event_type` index only if measured to matter).
    pub fn newest_memory_activity_at(&self) -> Result<Option<i64>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let ts: Option<String> = store.conn().query_row(
            "SELECT ts FROM events WHERE event_type IN (?1, ?2) ORDER BY seq DESC LIMIT 1",
            rusqlite::params![crate::graph::MEMORY_EVENT_TYPE, crate::graph::SESSION_CAPTURED_EVENT_TYPE],
            |r| r.get(0),
        ).optional()?;
        Ok(ts.and_then(|t| DateTime::parse_from_rfc3339(&t).ok().map(|d| d.timestamp())))
    }
```

   (b) `bossclawd/src/reflect/mod.rs`:

```rust
//! The Rung-4 R4-A reflection sweep loop. A PURE `decide_reflect` gate (capture-style, unit-tested truth
//! table) + a thin tokio loop reading the wall clock at the boundary (conflict-sweeper style). All heavy
//! work is one gated + serialized + spawn_blocking `EngineHandle::reflect_once` call.
pub mod sweeper;
pub use sweeper::*;
```

   (c) `bossclawd/src/reflect/sweeper.rs` (the pure gate + the loop):

```rust
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use bossclaw_core::reflect::{REFLECT_QUIET_SECS, REFLECT_STALENESS_FLOOR_SECS};

use crate::capture::sweeper::SWEEP_INTERVAL; // piggyback the 300s cadence (I2, conflict-sweeper precedent)
use crate::engine::EngineHandle;

/// The plain-data inputs to the PURE reflect gate (spec §2.1). Clock-free: `now` + the newest activity ts
/// are passed in. No fs / engine / lock.
#[derive(Debug, Clone)]
pub struct ReflectDecisionInput {
    pub onboarded: bool,
    pub reflect_enabled: bool,
    pub reasoner_ready: bool,
    pub now: i64,
    /// Newest memory-class event epoch, or `None` (no activity ever → quiet).
    pub newest_activity_at: Option<i64>,
    pub evolve_enabled: bool,
    pub evolve_queue_depth: usize,
    pub open_unparked_misses: usize,
    pub last_completed_run_at: i64,
    pub last_floor_fire_at: i64,
}

/// The gate verdict (spec §2.1). `Run.floor_fired` distinguishes a starvation-floor tick (bounded wake-time
/// work) from an ordinary quiet tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReflectDecision {
    Run { floor_fired: bool },
    GatedOff,
    ReasonerUnavailable,
    NotQuiet,
    DeferredEvolveBacklog,
}

/// The PURE tick decision (spec §2.1). Gate order: hard gates (onboarded ∧ enabled ∧ reasoner-ready) →
/// the starvation FLOOR (which overrides BOTH the quiet gate AND the evolve-backlog defer, §2.1 precedence:
/// a wedged evolve queue can never starve reflection) → quiet → evolve-backlog defer → run. The floor fires
/// at most once per `REFLECT_STALENESS_FLOOR_SECS` (the last-floor-fire guard).
pub fn decide_reflect(i: &ReflectDecisionInput) -> ReflectDecision {
    if !i.onboarded || !i.reflect_enabled {
        return ReflectDecision::GatedOff;
    }
    if !i.reasoner_ready {
        return ReflectDecision::ReasonerUnavailable; // cloud never silently falls back to local (§2.1)
    }
    // Starvation floor: unparked misses exist AND long since the last COMPLETED run AND not fired recently.
    let floor = i.open_unparked_misses > 0
        && i.now - i.last_completed_run_at > REFLECT_STALENESS_FLOOR_SECS
        && i.now - i.last_floor_fire_at > REFLECT_STALENESS_FLOOR_SECS;
    if floor {
        return ReflectDecision::Run { floor_fired: true }; // overrides quiet AND evolve-backlog defer
    }
    // Quiet: newest memory-class append older than the window (no activity ever = quiet).
    let quiet = i.newest_activity_at.map_or(true, |t| i.now - t >= REFLECT_QUIET_SECS);
    if !quiet {
        return ReflectDecision::NotQuiet;
    }
    // Evolve-backlog defer: the daytime helper goes first (its extraction feeds the entity graph).
    if i.evolve_enabled && i.evolve_queue_depth > 0 {
        return ReflectDecision::DeferredEvolveBacklog;
    }
    ReflectDecision::Run { floor_fired: false }
}

/// What one [`run_reflect_sweep_once`] did (mirrors `ConflictSweepReport`). All-zero + `gated_off` on a
/// disabled/non-onboarded brain (I3). No silent caps — `unhealable_thin` et al. surface in `report`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReflectSweepReport {
    pub gated_off: bool,
    pub not_quiet: bool,
    pub deferred_evolve_backlog: bool,
    pub floor_fired: bool,
    pub reasoner_unavailable: bool,
    pub report: bossclaw_core::ReflectReport,
}

/// Run ONE reflect sweep: gather the pure gate's inputs → `decide_reflect` → on `Run`, delegate to
/// `EngineHandle::reflect_once` and (on a floor tick) stamp the last-floor-fire marker. `now` is the
/// wall-clock epoch second (read by [`spawn`] at the boundary). Never panics; a reasoner/engine error
/// becomes a quiet `reasoner_unavailable` no-op (I6 — retry next cycle).
pub async fn run_reflect_sweep_once(engine: &EngineHandle, data_dir: &Path, now: i64) -> ReflectSweepReport {
    let onboarded = crate::identity::is_onboarded(data_dir);
    let reflect_enabled = onboarded && engine.reflect_enabled_or_false(onboarded).await;
    if !reflect_enabled {
        return ReflectSweepReport { gated_off: true, ..Default::default() };
    }
    let reasoner_ready = engine.reflect_reasoner_ready(onboarded).await;
    let evolve_enabled = engine.evolve_enabled_or_false(onboarded).await;
    let evolve_queue_depth = engine.queue_depth_or_zero(onboarded).await;
    let Some(g) = engine.reflect_gate_inputs(onboarded).await else {
        return ReflectSweepReport { reasoner_unavailable: true, ..Default::default() }; // open failure → no-op
    };
    let decision = decide_reflect(&ReflectDecisionInput {
        onboarded, reflect_enabled, reasoner_ready, now,
        newest_activity_at: g.newest_activity_at,
        evolve_enabled, evolve_queue_depth,
        open_unparked_misses: g.open_unparked_misses,
        last_completed_run_at: g.last_completed_run_at,
        last_floor_fire_at: g.last_floor_fire_at,
    });
    match decision {
        ReflectDecision::GatedOff => ReflectSweepReport { gated_off: true, ..Default::default() },
        ReflectDecision::ReasonerUnavailable => ReflectSweepReport { reasoner_unavailable: true, ..Default::default() },
        ReflectDecision::NotQuiet => ReflectSweepReport { not_quiet: true, ..Default::default() },
        ReflectDecision::DeferredEvolveBacklog => ReflectSweepReport { deferred_evolve_backlog: true, ..Default::default() },
        ReflectDecision::Run { floor_fired } => {
            if floor_fired {
                engine.stamp_reflect_floor_fire(onboarded, now).await; // best-effort; guards re-fires
            }
            match engine.reflect_once(onboarded, now).await {
                Ok(report) => ReflectSweepReport { floor_fired, report, ..Default::default() },
                Err(_) => ReflectSweepReport { reasoner_unavailable: true, floor_fired, ..Default::default() },
            }
        }
    }
}

/// Spawn the background reflect-sweep loop (mirrors `conflict::sweeper::spawn`). First tick fires
/// immediately; `MissedTickBehavior::Skip` prevents catch-up bursts. Reflection stays OFF until the owner
/// enables it (the gate lives inside `run_reflect_sweep_once`). A panic here cannot take down the daemon.
pub fn spawn(engine: Arc<EngineHandle>, data_dir: PathBuf) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(SWEEP_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let now = crate::capture::sweeper::system_time_to_epoch(Some(SystemTime::now()));
            let r = run_reflect_sweep_once(&engine, &data_dir, now).await;
            // Surface only real work (mirrors the conflict sweeper's quiet-on-noop discipline). No silent
            // caps: unhealable_thin is in the line.
            if r.report.dossiers_refreshed > 0 || r.report.candidate_repaired > 0 || r.report.no_material > 0
                || r.report.parked > 0 || r.report.unhealable_thin > 0 || r.report.reasoner_errors > 0
            {
                eprintln!(
                    "reflect-sweep: refreshed {} / candidate {} / repaired-by-time {} / no-material {} / \
                     parked {} / unhealable-thin {} (attempted {}, floor {}, reasoner-err {})",
                    r.report.dossiers_refreshed, r.report.candidate_repaired, r.report.repaired_by_time,
                    r.report.no_material, r.report.parked, r.report.unhealable_thin,
                    r.report.attempted, r.floor_fired, r.report.reasoner_errors,
                );
            }
        }
    });
}
```

   (d) `engine/mod.rs` helpers — the one-shot gate reads + the readiness dance + the floor-fire stamp:

```rust
    /// The core reads `decide_reflect` needs, in ONE spawn_blocking. `None` on an open failure (→ the
    /// sweeper no-ops that cycle).
    pub async fn reflect_gate_inputs(&self, onboarded: bool) -> Option<ReflectGateInputs> {
        let log = self.get_or_open(onboarded).await.ok()?;
        spawn_blocking(move || {
            let cur = log.reflect_cursor().unwrap_or_default();
            ReflectGateInputs {
                newest_activity_at: log.newest_memory_activity_at().unwrap_or(None),
                open_unparked_misses: log.open_miss_count().unwrap_or(0),
                last_completed_run_at: cur.last_completed_run_at,
                last_floor_fire_at: cur.last_floor_fire_at,
            }
        }).await.ok()
    }

    /// Reasoner readiness for reflection, mirroring the evolve scheduler's `select_ready` block
    /// (scheduler.rs:97-114): cloud mode trusts signed-consent readiness, local mode the Ollama probe;
    /// cloud NEVER silently falls back to local (spec §2.1 / §3.4).
    pub async fn reflect_reasoner_ready(&self, onboarded: bool) -> bool {
        let cfg = self.reasoner_config_or_default(onboarded).await;
        let cloud_mode = matches!(cfg.mode, crate::engine::reason::ReasonerMode::Cloud);
        let ollama_ready = if cloud_mode { false } else {
            let oll = crate::engine::ollama_probe::probe(crate::engine::reason::REASONER_MODEL_ID).await;
            oll.reachable && oll.model_present
        };
        let cloud_ready = if cloud_mode { self.reasoner_ready_or_false(onboarded).await } else { false };
        crate::engine::scheduler::select_ready(cloud_mode, ollama_ready, cloud_ready)
    }

    /// Stamp the last-floor-fire marker (best-effort; the §2.1 re-fire guard).
    pub async fn stamp_reflect_floor_fire(&self, onboarded: bool, now: i64) {
        if let Ok(log) = self.get_or_open(onboarded).await {
            let _ = spawn_blocking(move || log.set_reflect_last_floor_fire(now)).await;
        }
    }
```
   Add the small `ReflectGateInputs` struct (module level in `engine/mod.rs`):

```rust
/// The core reads the pure `decide_reflect` gate consumes (spec §2.1).
pub struct ReflectGateInputs {
    pub newest_activity_at: Option<i64>,
    pub open_unparked_misses: usize,
    pub last_completed_run_at: i64,
    pub last_floor_fire_at: i64,
}
```

   (e) `main.rs` (after the conflict sweeper spawn `:162`) — the fourth sibling:

```rust
        // Rung-4 R4-A: the reflection sweep. OFF by default (gated inside the loop on the owner's
        // `reflect_enabled` flag), so merging ships reflection dormant.
        bossclawd::reflect::sweeper::spawn(engine.clone(), data_dir.clone());
```
   (Update the stale `// (6) ... two background loops` comment `:154` to "four background loops".)

4. Run → PASS:
   `cargo test -p bossclaw-core newest_memory_activity_at_tracks_memory_and_session_appends`
   `cargo test -p bossclawd --lib reflect::sweeper`

5. Commit: `feat(rung4-a): decide_reflect gate + reflect sweeper (quiet/floor/defer) + newest_memory_activity_at`

---

## Task 12 — Enable path: proto `SetReflectEnabled`/`ReflectEnabled` ops + daemon dispatch (proto + daemon)

Spec §2.5 (arch m8). Two additive App-only wire ops (the `SetCaptureEnabled` pattern, minus the capture-only
`backfill`): `SetReflectEnabled { onboarded, enabled }` (write, acked with `Response::Ok`) and
`ReflectEnabled { onboarded }` (read the toggle position, `Response::ReflectEnabled(bool)` — needed by the T12b
toggle, mirroring `CaptureEnabled`). `Role::allows` is UNTOUCHED (reflect is App-only by construction — the
guest six-ops `no` set gains two entries). `PROTO_VERSION` stays `1` (additive variants only). App-only ops need
NO `override_onboarding_for_guest` arm (the `_ => None` at `server.rs:233` refuses them for guests).

**Files**
- Modify: `crates/bossclawd-proto/src/lib.rs` — `Request::SetReflectEnabled`/`ReflectEnabled` (after
  `CaptureEnabled` `:253`); `Response::ReflectEnabled(bool)` (after `CaptureEnabled(bool)` `:361`); the
  six-ops `no` set (`:865`).
- Modify: `crates/bossclawd/src/server.rs` — two dispatch arms (after the `CaptureEnabled` arm `:498`);
  `crates/bossclawd/src/engine/mod.rs` — `set_reflect_enabled`/`reflect_enabled` engine methods (beside
  `set_evolve_enabled` `:1274` / `capture_enabled` `:512`).
- Test: `crates/bossclawd-proto/src/lib.rs mod tests` + `crates/bossclawd/src/server.rs mod tests`.

**Steps**

1. Write the failing tests.
   (a) In `proto/lib.rs mod tests` (App allows, guest refuses, round-trips, version pinned):

```rust
    #[test]
    fn reflect_ops_are_app_only_and_round_trip() {
        use Request::*;
        // App allows both; MemoryClient (guest) refuses both — Role::allows is UNCHANGED.
        assert!(Role::App.allows(&SetReflectEnabled { onboarded: true, enabled: true }));
        assert!(Role::App.allows(&ReflectEnabled { onboarded: true }));
        assert!(!Role::MemoryClient.allows(&SetReflectEnabled { onboarded: true, enabled: true }),
            "reflect enable is App-only (guest-refused by construction)");
        assert!(!Role::MemoryClient.allows(&ReflectEnabled { onboarded: true }));
        // Additive → version stays 1.
        assert_eq!(PROTO_VERSION, 1);
        for req in [SetReflectEnabled { onboarded: true, enabled: false }, ReflectEnabled { onboarded: true }] {
            let back: Request = serde_json::from_slice(&serde_json::to_vec(&req).unwrap()).unwrap();
            assert_eq!(back, req);
        }
    }
```
   Extend the existing `memory_client_allows_exactly_six_ops` (`:865`) `no` array with:
```rust
            SetReflectEnabled { onboarded: true, enabled: true },
            ReflectEnabled { onboarded: true },
```

2. Run → FAIL: `cargo test -p bossclawd-proto reflect_ops_are_app_only_and_round_trip`
   Expected: `no variant named SetReflectEnabled`.

3. Implement.
   (a) `proto/lib.rs` — the two Request variants (after `CaptureEnabled` `:253`):

```rust
    /// Rung-4 R4-A: enable/disable the reflection loop. App-only (guest-refused). Follows the
    /// `SetCaptureEnabled` shape minus the capture-only `backfill` (reflection is a plain bool switch,
    /// like `SetMandatesEnabled`/conflict-detect). Acked with `Response::Ok`.
    SetReflectEnabled { onboarded: bool, enabled: bool },
    /// Rung-4 R4-A: read the sticky reflection flag (the toggle POSITION). App-only. Mirrors `CaptureEnabled`.
    ReflectEnabled { onboarded: bool },
```
   The Response arm (after `CaptureEnabled(bool)` `:361`):

```rust
    /// `ReflectEnabled` result — the sticky reflect-enabled flag.
    ReflectEnabled(bool),
```

   (b) `engine/mod.rs` engine methods (beside `set_evolve_enabled`/`capture_enabled`):

```rust
    /// Flip the sticky reflection off-switch (the toggle behind the settings panel). Gated.
    pub async fn set_reflect_enabled(&self, onboarded: bool, enabled: bool) -> Result<(), EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        spawn_blocking(move || log.set_reflect_enabled(enabled).map_err(|e| EngineOpError::Core(e.to_string())))
            .await
            .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// Read the sticky reflection flag (default CLOSED). Mirrors `capture_enabled`.
    pub async fn reflect_enabled(&self, onboarded: bool) -> Result<bool, EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        spawn_blocking(move || log.reflect_enabled().map_err(|e| EngineOpError::Core(e.to_string())))
            .await
            .map_err(|e| EngineOpError::Join(e.to_string()))?
    }
```

   (c) `server.rs` dispatch arms (after the `CaptureEnabled` arm `:498`, still inside `dispatch`):

```rust
        Request::SetReflectEnabled { onboarded, enabled } => {
            unit_result(engine.set_reflect_enabled(onboarded, enabled).await)
        }
        Request::ReflectEnabled { onboarded } => {
            op_result(engine.reflect_enabled(onboarded).await, Response::ReflectEnabled)
        }
```

4. Run → PASS:
   `cargo test -p bossclawd-proto reflect_ops_are_app_only_and_round_trip memory_client_allows_exactly_six_ops proto_version_still_one`
   `cargo test -p bossclawd --lib server::` (dispatch compiles; App-only guest refusal is by `Role::allows`).

5. Commit: `feat(rung4-a): proto SetReflectEnabled/ReflectEnabled App-only ops + daemon dispatch (PROTO_VERSION=1)`

---

## Task 13 — Snapshot digest line (daemon, never-truncated, neutral copy)

Spec §2.4 (the neutral digest line). A daemon-authored integer-only line rendered in the never-truncated
`render_fence` preamble beside the Rung-3 conflict lines. Reads the cumulative counters (T5) vs the last-served
totals; renders only when `n + m > 0`; advances the last-served totals ONLY on `source == "startup"` (the
Rung-3 startup-only rule); extract a pure `build_reflect_digest_line(n, m)` with a byte-exact test (the Rung-3
T14 lesson — test the real format fn).

**Files**
- Modify: `crates/bossclawd/src/engine/mod.rs` — `serve_reflect_digest_line` + pure `build_reflect_digest_line`
  (beside `serve_conflict_digest_lines` `:1222` / `build_digest_lines` `:1249`).
- Modify: `crates/bossclawd/src/capture/snapshot.rs` — `build` (`:228`) concatenates the reflect line into the
  preamble.
- Test: `crates/bossclawd/src/engine/mod.rs mod tests` (the byte-exact builder, beside `:2129`).

**Steps**

1. Write the failing test (byte-exact, in `engine/mod.rs mod tests` beside `build_digest_lines_...`):

```rust
    #[test]
    fn build_reflect_digest_line_is_byte_exact_and_gated_on_nonzero() {
        // Nothing new since last session → no line (an all-quiet reflect brain adds nothing).
        assert_eq!(EngineHandle::build_reflect_digest_line(0, 0), None);
        // Neutral copy, integer-only, pluralized with a bare `(s)` (matches the conflict-line style).
        assert_eq!(
            EngineHandle::build_reflect_digest_line(2, 3),
            Some("2 dossier(s) refreshed for recently-missed topics, 3 unknown-topic gap(s) since last session.".to_string()),
        );
        // Either non-zero alone still renders (both counts always shown for honesty).
        assert_eq!(
            EngineHandle::build_reflect_digest_line(0, 1),
            Some("0 dossier(s) refreshed for recently-missed topics, 1 unknown-topic gap(s) since last session.".to_string()),
        );
    }
```

2. Run → FAIL: `cargo test -p bossclawd --lib build_reflect_digest_line_is_byte_exact_and_gated_on_nonzero`
   Expected: `no function build_reflect_digest_line`.

3. Implement.
   (a) The pure builder + the serve wrapper (beside `serve_conflict_digest_lines`/`build_digest_lines`):

```rust
    /// The PURE reflect digest line (spec §2.4). Renders ONLY when `n + m > 0` (an all-quiet reflect brain
    /// adds nothing). Deliberately NEUTRAL copy — the digest must not present an operational counter as
    /// proven benefit (critic New-Minor-1): "refreshed for recently-missed topics" / "unknown-topic gaps".
    /// `m` (no_material) is the owner's most actionable signal ("your memory never knew this"). Integer-only.
    fn build_reflect_digest_line(n: u64, m: u64) -> Option<String> {
        if n + m == 0 {
            return None;
        }
        Some(format!(
            "{n} dossier(s) refreshed for recently-missed topics, {m} unknown-topic gap(s) since last session."
        ))
    }

    /// SERVE the reflect digest line for the SessionStart snapshot preamble (§2.4). INFALLIBLE — empty Vec
    /// on any error / not onboarded / no new activity (I1). Integer counts only (no memory content → no
    /// sanitize). Reads cumulative counters vs the last-served totals; advances the last-served totals ONLY
    /// on `source == "startup"` (mirrors `serve_conflict_digest_lines` — a mid-session compact must not
    /// consume the "since last session" window). Non-startup serves render the (unconsumed) line honestly.
    pub async fn serve_reflect_digest_line(&self, source: &str) -> Vec<String> {
        let onboarded = self.is_onboarded_local();
        let Ok(log) = self.get_or_open(onboarded).await else {
            return Vec::new();
        };
        let advance = source == "startup";
        spawn_blocking(move || {
            let (refreshed_total, no_material_total) = log.reflect_counters().unwrap_or((0, 0));
            let cur = log.reflect_cursor().unwrap_or_default();
            let n = refreshed_total.saturating_sub(cur.last_served_refreshed.max(0) as u64);
            let m = no_material_total.saturating_sub(cur.last_served_no_material.max(0) as u64);
            if advance {
                let _ = log.set_reflect_last_served(refreshed_total as i64, no_material_total as i64);
            }
            Self::build_reflect_digest_line(n, m).into_iter().collect()
        })
        .await
        .unwrap_or_default()
    }
```

   (b) `snapshot.rs` `build` (`:228`) — concatenate the reflect line AFTER the conflict lines (both
   never-truncated, integer-only):

```rust
    let mut preamble = engine.serve_conflict_digest_lines(source).await;
    // Rung-4 R4-A (§2.4): the reflection digest line, integer-only + neutral copy, joins the never-dropped
    // preamble. Its "since last session" window advances only on a fresh `startup` (decided inside serve_*).
    preamble.extend(engine.serve_reflect_digest_line(source).await);
    assemble_fence(&preamble, &entries)
```

4. Run → PASS: `cargo test -p bossclawd --lib build_reflect_digest_line_is_byte_exact_and_gated_on_nonzero`
   Regression: `cargo test -p bossclawd --lib capture::snapshot` (the conflict preamble + fence still green).

5. Commit: `feat(rung4-a): reflect snapshot digest line (neutral, integer-only, startup-only window)`

---

## Task 12b — Desktop Reflect toggle (desktop)

Spec §2.5. The single toggle in a DEDICATED Reflect settings block (the capture toggle is nested behind the
Claude-Code-connected gate with history-import copy; reflection is unrelated to Claude Code connection state, so
it gets its own panel — the desktop investigation's recommendation). Pure mirroring of the `SetCaptureEnabled`
wiring (command + registration + `Engine`/`EngineClient` + TS invoke + React + tests), minus `backfill`, plus
the desktop half of the `5 → 6` trip-wire. Split from T12 because it is ~120-190 lines across 6-8 files with a
hard dependency on T12's wire ops. Commands are `#[cfg(unix)]` like the capture commands.

**Files**
- Modify: `apps/desktop/src-tauri/src/engine/client.rs` — `set_reflect_enabled`/`reflect_enabled` (after
  `capture_enabled` `:452`); trip-wire `:973` (`5 → 6`); a round-trip test (beside `:1272`).
- Modify: `apps/desktop/src-tauri/src/engine/mod.rs` — `Engine::set_reflect_enabled`/`reflect_enabled` (after
  `:514`).
- Modify: `apps/desktop/src-tauri/src/commands/integrations.rs` — `integrations_set_reflect_enabled` +
  `integrations_reflect_enabled` (after `:125`) + fake-transport tests (`mod tests` `:157`).
- Modify: `apps/desktop/src-tauri/src/main.rs` — register both (`generate_handler!` `:249`).
- Modify: `apps/desktop/src/api/integrations.ts`(+`.test.ts`) — `setReflectEnabled`/`reflectEnabled` (after
  `:55`) + invoke-contract tests (after `:47`).
- Create: `apps/desktop/src/settings/ReflectPanel.tsx`(+`.test.tsx`); Modify `AirSettings.tsx` (mount at `:22`).

**Steps**

1. Write the failing tests.
   (a) Desktop socket round-trip (`client.rs`, mirroring `set_and_get_capture_enabled_over_the_socket:1272`):

```rust
    #[tokio::test]
    async fn set_and_get_reflect_enabled_over_the_socket() {
        let daemon = TestDaemon::spawn().await;
        let client = daemon.client();
        assert!(!bounded(client.reflect_enabled(true)).await.unwrap(), "default CLOSED");
        bounded(client.set_reflect_enabled(true, true)).await.unwrap();
        assert!(bounded(client.reflect_enabled(true)).await.unwrap(), "reads back enabled");
    }
```
   And bump the trip-wire (`client.rs:973`):
```rust
        assert_eq!(st.event_count, 6, "prime_switches wrote the 6 sticky config events");
```
   (b) TS invoke contract (`api/integrations.test.ts`, mirroring `:37`):
```ts
  it("setReflectEnabled forwards the enabled flag as a camelCase arg", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);
    await setReflectEnabled(true);
    expect(invoke).toHaveBeenCalledWith("integrations_set_reflect_enabled", { enabled: true });
  });
  it("reflectEnabled invokes its command and returns the bool", async () => {
    vi.mocked(invoke).mockResolvedValue(false);
    expect(await reflectEnabled()).toBe(false);
    expect(invoke).toHaveBeenCalledWith("integrations_reflect_enabled");
  });
```
   (c) React component (`ReflectPanel.test.tsx`, mirroring `IntegrationsPanel.test.tsx:81`):
```tsx
  it("the reflect toggle reflects the engine flag and toggling calls setReflectEnabled", async () => {
    vi.mocked(api.reflectEnabled).mockResolvedValue(true);
    vi.mocked(api.setReflectEnabled).mockResolvedValue(undefined);
    render(<ReflectPanel />);
    const toggle = await screen.findByRole("checkbox", { name: /reflect on recently-missed topics/i });
    expect(toggle).toBeChecked();
    fireEvent.click(toggle);
    await waitFor(() => expect(api.setReflectEnabled).toHaveBeenCalledWith(false));
  });
```

2. Run → FAIL: `cargo test -p air_agent_desktop set_and_get_reflect_enabled_over_the_socket` and
   `cd apps/desktop && npx vitest run src/api/integrations.test.ts src/settings/ReflectPanel.test.tsx`
   Expected: missing methods / missing exports / missing component.

3. Implement.
   (a) `client.rs` methods (after `capture_enabled` `:452`) — mirror `:436-452` minus `backfill`:
```rust
    /// Enable/disable reflection. App-only; the daemon supplies nothing extra (a plain bool switch).
    pub async fn set_reflect_enabled(&self, onboarded: bool, enabled: bool) -> Result<(), EngineOpError> {
        self.unit(Request::SetReflectEnabled { onboarded, enabled }).await
    }
    /// Read the sticky reflect-enabled flag (default CLOSED). Mirrors `capture_enabled`.
    pub async fn reflect_enabled(&self, onboarded: bool) -> Result<bool, EngineOpError> {
        match self.request(Request::ReflectEnabled { onboarded }).await? {
            Response::ReflectEnabled(b) => Ok(b),
            other => Err(unexpected(other)),
        }
    }
```
   (b) `engine/mod.rs` (after `:514`) — thin passthroughs mirroring `set_capture_enabled`/`capture_enabled`
   (drop `backfill`); `commands/integrations.rs` (after `:125`) — the two `#[tauri::command]`s mirroring
   `integrations_set_capture_enabled`/`integrations_capture_enabled` (no `toggle_capture_wiring`; call
   `engine.set_reflect_enabled(onboarded, enabled)` directly; the read fails closed to `false`); `main.rs`
   (`:249`) — two `#[cfg(unix)]` registrations.
   (c) `api/integrations.ts` (after `:55`):
```ts
export const setReflectEnabled = (enabled: boolean): Promise<void> =>
  invoke<void>("integrations_set_reflect_enabled", { enabled });
export const reflectEnabled = (): Promise<boolean> =>
  invoke<boolean>("integrations_reflect_enabled");
```
   (d) `ReflectPanel.tsx` — a standalone panel (state + mount read + toggle handler, mirroring the capture
   toggle's `useState`/`refreshCapture`/`onToggleCapture` idiom at `IntegrationsPanel.tsx:34,49,87`) with the
   label "Reflect on recently-missed topics" + neutral sub-copy ("When your machine is idle, AIR quietly
   refreshes dossiers for topics you recently searched and couldn't find."); mount `<ReflectPanel/>` in
   `AirSettings.tsx:22`.

4. Run → PASS: the three test commands above.
   Full desktop gate: `cargo test -p air_agent_desktop` and `cd apps/desktop && npx vitest run && npx tsc --noEmit && npx eslint src --max-warnings 0`.

5. Commit: `feat(rung4-a): desktop Reflect settings toggle + client methods + trip-wire 5→6`

---

## Task 14 — Reflection harness pass 1: the runnable non-regression gate (memharness)

Spec §5.2 / §5.3(a)(b)(c). Two deliverables: (1) the PAGE ARM — `map_hits` resolves dossier `page` hits as
SYNTHETIC non-gold occupants so a reflected-brain run does not abort on the `PageResolver` fail-loud (`page`
events are not file events); (2) the reflected-pass DRIVER — enable evolve+reflect, seed a scripted retire +
a frozen synthetic miss set, run BOTH loops to quiescence, `run_queries` both arms, and gate with
`recall_regressed`. Union-coverage is a SEPARATE, REPORTED metric — never gated (critic New-Blocker-1: the gold
page scores ONLY as itself; a dossier NEVER substitutes for the gold it cites; crowding-out = the regression the
gate exists to catch). `memharness` is dev-only, never ships.

**Two corrections to the brief's memharness assumptions (verified).** `PageResolver` lives in `resolve.rs:20-59`
(NOT `arms.rs`); its fail-loud is an `anyhow::Error` in `page_id_of` (`resolve.rs:53`), not a panic. The
kind-branch belongs in `map_hits` (`arms.rs:98`), which already holds each `HitWire` (its `hit.kind` is the
event type — `proto/types.rs:337`). The single-page-id hit/dedup/rank model (`arms.rs:12,18,24`) stays intact.

**Files**
- Modify: `crates/memharness/src/arms.rs` — `map_hits` (`:98`) branches on `hit.kind == "page"`.
- Create: `crates/memharness/src/reflect_pass.rs` — the quiescence driver + `union_coverage` metric.
- Modify: `crates/memharness/src/lib.rs` — `pub mod reflect_pass;`; `main.rs` — a `reflect-gate` subcommand +
  the Peter-gated runbook doc.
- Test: `crates/memharness/src/arms.rs mod tests` (hand-built HitWires) + `reflect_pass.rs mod tests` (doubles).

**Steps**

1. Write the failing test (in `arms.rs mod tests`) — page hits resolve as non-gold, file hits stay loud:

```rust
    #[test]
    fn page_hits_resolve_as_synthetic_non_gold_occupants_and_file_hits_stay_loud() {
        use bossclawd_proto::{HitWire, HitMirror};
        // A resolver mapping ONE file event → the gold page id.
        let resolver = PageResolver::from_pairs_for_test(&[("file-ev-1", "air/kenny")]);
        let hits = vec![
            // A reflected-brain dossier hit (kind="page") — must NOT abort, and must NOT equal any gold id.
            HitWire { hit: HitMirror { event_id: "page-ev-9".into(), score: 0.9, sources: vec![], kind: "page".into() }, text: "dossier body".into() },
            // A file hit — resolves to the gold page id (the un-rigged path).
            HitWire { hit: HitMirror { event_id: "file-ev-1".into(), score: 0.8, sources: vec![], kind: "file_ingested".into() }, text: "source".into() },
        ];
        let mapped = map_hits(&resolver, hits).expect("a page hit must not abort the run");
        assert_eq!(mapped[0].page_id, "__dossier__:page-ev-9", "dossier → synthetic non-gold id");
        assert_ne!(mapped[0].page_id, "air/kenny", "a dossier NEVER equals the gold it cites (gate: gold-as-itself)");
        assert_eq!(mapped[1].page_id, "air/kenny", "the file hit still resolves to gold");
        // An unmapped FILE hit still fails loud (the no-evolve invariant for file hits is preserved).
        let bad = vec![HitWire { hit: HitMirror { event_id: "ghost".into(), score: 0.1, sources: vec![], kind: "file_ingested".into() }, text: "x".into() }];
        assert!(map_hits(&resolver, bad).is_err(), "an unmapped file hit still stops the run (no silent fallback)");
    }
```

   (Add a tiny `PageResolver::from_pairs_for_test(&[(&str,&str)])` test ctor in `resolve.rs` if none exists —
   it just fills `by_event`; the load-bearing production ctor `from_file_records` is untouched.)

2. Run → FAIL: `cargo test -p memharness page_hits_resolve_as_synthetic_non_gold_occupants_and_file_hits_stay_loud`
   Expected: the page hit currently routes through `page_id_of("page-ev-9")` → `Err` (unmapped) → the run aborts.

3. Implement.
   (a) `map_hits` (`arms.rs:98`) — the kind branch:

```rust
pub fn map_hits(
    resolver: &PageResolver,
    wire_hits: Vec<bossclawd_proto::HitWire>,
) -> anyhow::Result<Vec<RetrievedHit>> {
    wire_hits
        .into_iter()
        .map(|h| {
            // §5.3(b): a reflected-brain dossier (`page` event) is NOT a corpus file, so it does not resolve
            // through the file bridge. Give it a SYNTHETIC page id that occupies a rank slot (so crowding the
            // gold FILE page out of top-k registers as a regression) but can NEVER equal a corpus gold id
            // (the gate credits gold ONLY as itself). File hits keep the loud no-evolve invariant.
            let page_id = if h.hit.kind == bossclaw_core::graph::PAGE_EVENT_TYPE {
                format!("__dossier__:{}", h.hit.event_id)
            } else {
                resolver.page_id_of(&h.hit.event_id)?
            };
            Ok(RetrievedHit { page_id, snippet: h.text })
        })
        .collect()
}
```

   (b) `reflect_pass.rs` — the run-to-quiescence driver (structure; the gate reuses `compare_runs` +
   `recall_regressed`):

```rust
//! The Rung-4 R4-A reflection non-regression gate (spec §5.2/§5.3). Drives BOTH loops to quiescence over the
//! frozen corpus + a seeded retire + a frozen synthetic miss set, scores both arms, and gates with
//! `recall_regressed`. Union-coverage is REPORTED only, never gated (critic New-Blocker-1). Dev-only.

/// A hard cap on evolve+reflect drive cycles; non-convergence is a FAIL-LOUD error, never a silent cap.
pub const MAX_QUIESCENCE_CYCLES: usize = 64;

/// Drive evolve + reflect to quiescence (both queues drained) on an ALREADY-INGESTED, evolve+reflect-ENABLED
/// brain. Returns the cycle count, or an error if it did not converge within `MAX_QUIESCENCE_CYCLES`.
/// `tick_evolve` / `tick_reflect` are injected so hermetic tests can drive doubles and the live path drives
/// the real `EngineHandle::{evolve_once, reflect_once}`.
pub fn drive_to_quiescence(
    mut tick_evolve: impl FnMut() -> anyhow::Result<usize>, // returns remaining evolve queue depth
    mut tick_reflect: impl FnMut() -> anyhow::Result<usize>, // returns open-miss count after the tick
) -> anyhow::Result<usize> {
    for cycle in 1..=MAX_QUIESCENCE_CYCLES {
        let evolve_left = tick_evolve()?;
        let misses_left = tick_reflect()?;
        if evolve_left == 0 && misses_left == 0 {
            return Ok(cycle);
        }
    }
    anyhow::bail!(
        "reflect gate: evolve+reflect did not reach quiescence in {MAX_QUIESCENCE_CYCLES} cycles — refusing \
         to score a non-converged pass (a bounded loop must terminate, not spin nights)"
    )
}

/// Union-coverage (REPORTED, never gated): for each known-item case whose gold FILE page is NOT in the top-k,
/// does a dossier hit whose CITED sources include the gold file appear, and at what rank? Reported alongside
/// s@k so the future dossier-primacy decision has honest data, but it NEVER feeds `recall_regressed`.
/// BLOCKED ON OPEN QUESTION #2 (below): computing this needs each dossier hit's cited `source_event_ids`,
/// which the recall wire `Hit` does NOT carry — settle the read mechanism (a `GetPage`-style op vs an
/// in-process page-event read) at plan review, THEN implement this signature over `(cases, per-case dossier
/// hits with their resolved cited file page_ids, gold)`. Not part of T14's runnable-gate red→green.
```

   The `reflect-gate` subcommand orchestration (main.rs): ingest the frozen corpus (the existing
   `prepare_corpus` → wire `run_ingest` path), enable evolve + reflect, SEED a scripted retire (§5.3(c);
   simplest per the brief = a direct `retire_memory` on a seeded `remember` note folded into an entity's
   lineage — see the deviation note), load a FROZEN synthetic miss-set file, `drive_to_quiescence`, then
   `run_queries` for the reflected arm and a baseline arm, write both reports, and `compare_runs` +
   `recall_regressed` as the SHIP gate. Hermetic tests use `AirDouble`/`GbrainDouble`/doubles (the
   `tests/hermetic_run_e2e.rs` fixtures) to exercise `drive_to_quiescence` + `map_hits` + the gate WITHOUT a
   live model; the live frozen-corpus run is Peter-gated (document the runbook: `cargo run -p memharness --
   reflect-gate --corpus ~/brain --miss-set <frozen.jsonl> --reports-dir <outside-repo>`).

4. Run → PASS: `cargo test -p memharness page_hits_resolve_as_synthetic_non_gold_occupants_and_file_hits_stay_loud`
   + the hermetic driver tests. The live gate run is Peter-gated (NOT in CI).

5. Commit: `feat(rung4-a): memharness reflection non-regression gate (page arm + quiescence driver + recall_regressed)`

---

## Task 15 — Reflection evidence probes (d) + (e) (memharness, REPORTED not gated)

Spec §5.3(d)(e). Two REPORTED probes (informing the future dossier-primacy decision; NOT SHIP-gated in R4-A):
(d) **held-out generalization** — reflect on miss set A, score success@k on a DISJOINT paraphrase set B over the
same topics; the pre-registered threshold is a REPORTED kill-criterion constant, not a gate. (e)
**dossier-vs-source A/B** — blind position-swapped judging of answers composed from the dossier page vs its raw
cited memories, on the open-case set, reusing the EXISTING judge machinery; the judge is admitted ONLY if it
clears the Phase-0 trust ladder (`judge.rs`: `TRUST_AGREEMENT_MIN = 0.85` / `TRUST_KAPPA_MIN = 0.6`), else the
lift is printed as UNINTERPRETABLE (the real token is `audit_incomplete`/`trusted:false`).

**Verified seam.** The A/B machinery is `judge.rs` (`PairJudge`, `judge_pair_blind:241`, `assign_blind:213`,
`trust_verdict:89`) — NOT `conflict_grade.rs` (that is a separate conflict-judge precision-CI grader). `(e)`
reuses `judge_pair_blind` + `trust_verdict` exactly as `run_queries` does for open cases (`run.rs:181-193`).

**Files**
- Create: `crates/memharness/src/probes.rs` — the (d) held-out runner (a disjoint paraphrase set file format +
  scorer) and the (e) dossier-vs-source runner (blind judge over dossier-context vs source-context answers,
  gated on `trust_verdict`).
- Modify: `crates/memharness/src/lib.rs` (`pub mod probes;`); `main.rs` (surface both as REPORTED lines).
- Test: `crates/memharness/src/probes.rs mod tests` (doubles for both runners).

**Steps**

1. Write the failing tests (hermetic doubles): (d) an A→B held-out run reports a success@k on B with a REPORTED
   kill-criterion constant `HELD_OUT_LIFT_KILL: f64` (documented, not asserted as a gate); (e) a dossier-vs-source
   run over `GoodJudge`/`ContrarianAuditor` doubles produces a `TrustVerdict` and, when `trusted == false`,
   prints the lift as UNINTERPRETABLE rather than as evidence. (Assert the machinery: the (e) runner returns a
   `TrustVerdict` and an `Option<f64>` lift that is `None`/flagged when untrusted.)

2. Run → FAIL: `cargo test -p memharness probes` (missing module).

3. Implement `probes.rs`:
   - `pub const HELD_OUT_LIFT_KILL: f64 = <pre-registered at plan review>;` — a REPORTED kill-criterion, with
     a doc comment stating it is agreed BEFORE the live dogfood (spec §5.5) and is NOT a CI gate.
   - `pub fn held_out_probe(reflect_set: &[QueryCase], eval_set: &[QueryCase], air: &mut dyn AirRetriever,
     cfg: &RunConfig) -> HeldOutReport` — reflect (drive) on `reflect_set`'s topics, then score success@k on the
     DISJOINT `eval_set` (paraphrases), reusing `dedup_by_page` + `gold_rank` + `success_at_k`.
   - `pub fn dossier_vs_source_ab(open_cases: &[QueryCase], /* dossier ctx + source ctx per case */,
     answerer: &dyn Answerer, judge: &dyn PairJudge, auditor: Option<&dyn PairJudge>, seed: u64)
     -> (TrustVerdict, Option<f64>)` — for each open case, compose an answer from the DOSSIER context and one
     from its RAW CITED memories, `judge_pair_blind` (dossier-vs-source, blind + position-swapped), audit via
     `select_audit_indices` + `trust_verdict`; return the trust verdict + the lift, where the lift is
     `Some(_)` only when `trusted`, else `None` (printed UNINTERPRETABLE).

4. Run → PASS: `cargo test -p memharness probes`.

5. Commit: `feat(rung4-a): memharness evidence probes (held-out generalization + dossier-vs-source A/B, reported)`

---

## Task 16 — Final wire-in verification + FULL-WORKSPACE exit gate

Spec §5 (exit gate) / §5.4 (dormancy) / §4 I3. An end-to-end daemon test drives a real reflect tick over the
in-process engine and asserts the counters + the digest line + guest refusal; then the full-workspace gate runs
with the `== 6` trip-wire green at ALL THREE sites.

**Files**
- Create: `crates/bossclawd/tests/reflect_e2e.rs` — the end-to-end enable→seed→tick→digest test + guest-refused.
- No change: the three trip-wire sites (`roundtrip.rs:173`, `engine/mod.rs:2237` — T2; `client.rs:973` — T12b)
  now read `6`; this task only verifies them green.

**Steps**

1. Write the failing integration test (`crates/bossclawd/tests/reflect_e2e.rs`, mirroring the conflict-sweep
   engine e2e `conflict/sweeper.rs:112`):

```rust
#[tokio::test]
async fn reflect_tick_heals_a_stale_dossier_and_the_digest_line_appears() {
    // Real in-process engine (dim-64 mock embedder + a scripted reasoner), identity on disk so `is_onboarded`
    // passes. Enable reflect + evolve, seed a topic+page whose source is then retired (a stale dossier), and a
    // synthetic miss; drive a reflect tick and assert the report counters > 0 + the startup digest line.
    crate::common::... // reuse the crate's engine test harness
    engine.set_reflect_enabled(true, true).await.unwrap();
    // ... seed: remember two sources → evolve to a page → retire one source (stale lineage). ...
    let report = engine.reflect_once(true, 1000).await.unwrap();
    assert!(report.dossiers_refreshed >= 1 || report.no_material >= 1 || report.repaired_by_time >= 1,
        "the tick did real work (healed a stale dossier and/or classified the seeded miss)");
    // The digest line appears in a fresh startup snapshot preamble.
    let lines = engine.serve_reflect_digest_line("startup").await;
    assert!(lines.iter().any(|l| l.contains("since last session")), "the neutral digest line renders");
}

#[tokio::test]
async fn guest_cannot_enable_reflection_over_the_socket() {
    // I8 unchanged: SetReflectEnabled is App-only. A MemoryClient connection is refused (NotPermitted).
    let daemon = TestDaemon::spawn().await;
    let resp = daemon.roundtrip(Role::MemoryClient, Request::SetReflectEnabled { onboarded: true, enabled: true }).await;
    assert!(matches!(resp, Response::Err { kind: OpErrorKindWire::NotPermitted, .. }),
        "guest cannot enable reflection (App-only)");
}
```

2. Run → FAIL (until the harness seams compile): `cargo test -p bossclawd --test reflect_e2e`

3. Implement: nothing new in product code — this exercises the T1-T13 surface end-to-end. Adapt to the crate's
   real engine test harness (mirror `engine/mod.rs mod tests` seeding + the `authz`/`roundtrip` socket helper).

4. Run → PASS, then the FULL-WORKSPACE EXIT GATE (all foreground, all must be green):

```
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p bossclaw-core
cargo test -p bossclawd-proto
cargo test -p bossclawd
cargo test -p memharness
cargo test -p air_agent_desktop
cd apps/desktop && npx vitest run && npx tsc --noEmit && npx eslint src --max-warnings 0
```

   Exit-gate cross-checks (design §5):
   - **Dormancy (I3), the `== 6` trip-wire at ALL THREE sites:** `cargo test -p bossclawd --test roundtrip`
     (`roundtrip.rs:173`), `cargo test -p bossclawd --lib engine::` (`engine/mod.rs:2237`), `cargo test -p
     air_agent_desktop` (`client.rs:973`). A fresh brain writes exactly 6 sticky config events and reflection
     does NOTHING until the explicit App-only enable.
   - **Non-regression gate (§5.2):** T14 (`recall_regressed`), Peter-gated live run documented.
   - **Reflect-independent heal + fresh-corpus exclusion path (§5.3(c), New-Major-1):** T3 + T14's seeded retire.
   - **Scoreboard vs evidence separation (§5):** T13 (operational digest) vs T15 (reported probes).

5. Commit: `test(rung4-a): reflect end-to-end (heal + digest + guest-refused) + full-workspace exit gate green`

---

## Plan-time deviations from the spec brief (with evidence)

1. **`refresh_topic_page` is reasoner-only (no `embedder` param).** The brief specifies
   `refresh_topic_page(embedder, reasoner, entity)`. The extracted per-topic body of `summarize_topics`
   (`log.rs:8120-8207`) NEVER touches an embedder: `summarize_topics` takes only `reasoner` (`:8111`), and
   `emit_page` takes no embedder (`:2772`) — pages embed lazily via `append` and become recall-visible at the
   caller's post-tick `rebuild_indexes`. So the honest signature is `(reasoner, entity)`; the embedder lives in
   the callers that need `recall`/`entity_search` (T7's `attempt_miss`, the T10 wrapper), not in the composer.
2. **T3 exclusion applied in `gather_fact_set` only (single point), not also in `fact_texts_for_ids`.** The brief
   names both. `fact_texts_for_ids` has EXACTLY ONE caller (`gather_fact_set:8098`, grep-confirmed), and both
   `memories` and `source_ids` derive from the single `lineage` vec — filtering `lineage` in `gather_fact_set`
   shrinks both (the "texts-AND-cited-ids" correctness note), at one clean seam.
3. **The miss ring is READ non-destructively (via `Telemetry::stats()`), not drained via a new
   `take_recent_misses`.** The brief anticipated a drain accessor. The durable backlog's `seed_miss` is
   upsert-if-new, so re-reading the same ≤20 queries every tick is idempotent (a terminal miss is never reset);
   a non-destructive read ALSO preserves the App's `RecallStats` "recent misses" view (which the brief asks to
   preserve). No new telemetry method; the disclosure doc-header still updates (T10e).
4. **T12 split into T12 (proto+daemon wire) + T12b (desktop toggle), and a `ReflectEnabled` READ op was added.**
   The brief sanctions the split "if the desktop toggle turns out to be large" — the desktop investigation found
   ~120-190 lines across 6-8 files with a hard dependency on the wire ops, warranting its own task. The read op
   `ReflectEnabled { onboarded }` (beyond the brief's `SetReflectEnabled`) is required for the toggle to render
   its position, mirroring `CaptureEnabled`; both are App-only. The write reuses `Response::Ok` (no new Response
   variant), exactly as `SetCaptureEnabled` does (`server.rs:495` → `unit_result` → `Response::Ok`).
5. **The `reflect_once` engine wrapper takes `(onboarded, now)`, not `(onboarded)`.** Core `reflect_once` needs
   `now` for the backlog timestamps + the floor's last-completed stamp, and core stays clock-free (the
   `capture_enabled_at`/`detect_conflicts_once(onboarded, now)` precedent). The sweeper supplies the boundary
   clock, exactly like the conflict sweeper.
6. **`PageResolver` is in `resolve.rs` (not `arms.rs`); its fail-loud is an `anyhow::Error` (`page_id_of:53`),
   not a panic; the judge trust ladder for §5.3(e) is in `judge.rs` (not `conflict_grade.rs`).** T14/T15 target
   the real seams; the page-hit branch lives in `map_hits` (`arms.rs:98`), keyed on `HitWire.hit.kind == "page"`.
7. **`last_completed_run_at` / `last_floor_fire_at` live in the core `reflect_cursor` table (daemon-supplied
   epoch i64s), the simplest honest home.** The brief left the home to me ("daemon or core cursor family — pick
   the simplest honest home and justify"). Core is the single home for the reflect progress family (backlog +
   counters + cursor), and the `capture_enabled_at` precedent already stores a daemon-supplied wall-clock i64 in
   core, keeping core clock-free. The daemon reads them via `reflect_gate_inputs` and writes them via
   `set_reflect_last_completed_run` / `set_reflect_last_floor_fire`.

## Open questions for plan review (do NOT let the executor guess these silently)

1. **`REFLECT_TOPIC_FLOOR = 0.75` (RESOLVE_LOW) vs a tighter 0.92 (RESOLVE_HIGH).** I anchored the floor to
   `extract::RESOLVE_LOW` (the bar below which evolve mints a fresh entity) with the harness (§5.3(d)) tuning it
   upward. §7.2 says "start conservative (high floor)". A reviewer may prefer starting at `RESOLVE_HIGH` (0.92,
   evolve's auto-merge bar) — accepting near-total `no_material` initially — and letting §5.3(d) lower it. Both
   are defensible; the const is the single-line dial. **Decide the starting value at plan review.**
2. **The memharness reflected-pass CONTROL SURFACE (T14).** Driving evolve+reflect to quiescence requires the
   harness to (a) ENABLE evolve+reflect on `HarnessDaemon`, (b) TICK `evolve_once`/`reflect_once` repeatedly, and
   (c) for union-coverage, READ a dossier's cited `source_event_ids`. The verified `HarnessDaemon` API
   (`spawn_real`/`spawn_with_provider`/`home`/`socket_path`) + `WireClient`
   (`connect`/`add_grant`/`run_ingest`/`list_files`/`recall`) exposes NONE of these. **Which control surface?**
   Options: (i) expose the in-process `EngineHandle` on `HarnessDaemon` (simplest for an in-process dev harness);
   (ii) add wire ops (`EvolveNow`/`ReflectNow`/`GetPage`) — heavier, App-only. I recommend (i) for evolve/reflect
   ticks + enables, and flag that union-coverage's "read a dossier's cites" needs either a `GetPage`-style read
   or an in-process page-event read. This is the single biggest unresolved seam and must be settled before T14
   is built.
3. **The §5.3(c) retirement seed mechanism.** I chose the simplest that exercises the T3 gather-exclusion: a
   direct `retire_memory(note, None)` on a seeded `remember` note folded into an entity's lineage (deterministic
   under a scripted reasoner). The brief allows "resolve_conflict on a seeded proposal OR direct retire_memory".
   `resolve_conflict` would additionally require seeding a `conflict_proposal` (the `#[cfg(unix)]` Phase-2/3
   machinery) — heavier and orthogonal to what the gate tests. **Confirm `retire_memory` is an acceptable seed**
   (it is App-reachable and portable; the exclusion set is `retired_notes ∪ superseded`, so an App retire
   populates `retired_notes` identically to a conflict retire).
4. **`reflect_once` cumulative "dossiers refreshed" counter definition.** I count
   `dossiers_refreshed (tidy heals) + candidate_repaired (miss-driven refreshes that made recall hit)` into the
   `refreshed_total` counter feeding the digest's `n`. A reviewer may prefer `refreshed_total` to count ONLY
   tidy heals (excluding miss-driven candidate_repaired) so the digest's "dossiers refreshed" is unambiguous.
   **Confirm the digest `n` composition.**
5. **Held-out probe (d) pre-registered threshold + `MAX_QUIESCENCE_CYCLES`.** `HELD_OUT_LIFT_KILL` (§5.3(d)) is a
   REPORTED kill-criterion "agreed at plan review before the live dogfood" (§5.5) — its VALUE is deliberately a
   plan-review decision, not a source-derived constant. `MAX_QUIESCENCE_CYCLES = 64` is a provisional bound;
   confirm it is comfortably above the realistic evolve+reflect convergence depth for the frozen corpus.

## Appendix — invariant → task cross-reference (design §4)

| Invariant | Where upheld |
| --- | --- |
| **I1** (append-only dossier REVISIONS; supersede-in-recall, prior recoverable; NEVER mints entities or retires anything) | T3/T4 (`refresh_topic_page` → `emit_page` supersede), T8 (`refresh_stale_pages` never retires; `unhealable_thin` residual), T9 (`reflect_once` mints nothing) |
| **I2** (no silent egress; reasoner behind `cloud_consent_ok`; miss QUERIES drive local search; MATERIAL reaches a reasoner only inside consent) | T10 (`cloud_consent_ok` before the reasoner is built; local default), T11 (`reflect_reasoner_ready` — cloud never falls back local) |
| **I3** (dormant; `ConfigFlag::Reflect` default-closed + `prime_switches` force-off; ONE named Reflect-independent change = the §2.3 gather heal; trip-wire `5 → 6` at all THREE sites) | T1 (default-closed flag), T2 (force-off + 2 daemon trip-wires), T3 (the named Reflect-independent heal), T12b (desktop trip-wire), T16 (dormancy assertion) |
| **I5** (append-only durable state; `reflect_miss_backlog`/counters/cursor are re-derivable progress state, not history) | T5 (re-derivable tables; upsert-if-new; losing them re-learns from the ring) |
| **I6** (fail-safe; per-miss attempt budget → parked; per-tick caps; floor ≤ once/interval; `Busy` on overlap; torn ticks idempotent) | T7 (attempt budget → parked), T9 (per-tick caps), T10 (`reflect_lock` → `Busy`; set-diff emit is idempotent), T11 (floor re-fire guard) |
| **I7** (hostile-output; dossiers citation-floored subtract-only; no raw model text logged; digest lines integer-only) | T4 (`citation_floor` via `refresh_topic_page`), T13 (integer-only neutral digest line) |
| **I8** (relaxed for R4-B only; R4-A adds NO guest-reachable ops; `SetReflectEnabled`/`ReflectEnabled` are App-only) | T12 (`Role::allows` UNCHANGED; six-ops `no` set gains both), T16 (guest-refused test) |
| **I9** (stop-nagging; parked misses stop being attempted; backlog dedup by normalized key; ONE shared gather exclusion so the two writers never fight) | T3 (single-source gather exclusion), T5 (`open_misses` excludes parked; normalized-key dedup), T7 (parked → not re-attempted) |
| **I-vis** (staged visibility; R4-A = scoreboard + digest counts + the signed log; no per-item review surface, matching evolve's dossier posture) | T10 (`ReflectTelemetry` scoreboard), T13 (digest counts), T5 (`reflect_counters` durable totals) |

