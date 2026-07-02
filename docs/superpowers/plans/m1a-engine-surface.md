# M1a Engine-Surface Inventory + Task 0.5 Keychain Gate Record

Task 0 + Task 0.5 output of `2026-07-02-air-agent-memory-hub-m1a-daemon.md` (Rev 2).
Captured 2026-07-02 against branch `feat-memory-hub-m1a-daemon` (base main `dd43d88`).

---

## Task 0.5 — Keychain-ACL GO/NO-GO spike: ✅ **GO** (with one signing condition)

**Question:** can a separately-built `bossclawd` read the app's DEK
(`air-agent.engine.dek`, service `ai.air-agent.desktop`, login keychain) with **no
interactive prompt**?

**Method:** a throwaway get-only Rust binary using the identical read path
(`keyring = "2.3"` → `Entry::new("ai.air-agent.desktop", "air-agent.engine.dek").get_password()`),
run under `timeout 15` on the signed-build Mac where the app has a live brain. The probe
never calls `set`/`delete` (cannot mint or orphan) and never prints secret material.

**The app's designated requirement** (what the keychain ACL trusts after the one-time
"Always Allow"): `identifier "ai.air-agent.desktop" and certificate leaf = H"15c03e5837ae7d6a776fd14816791f39975b07af"`
(the self-signed "AIR Agent Dev" cert from `scripts/dev-build-signed.sh` / PR #44).

| Config | Signature | Result |
|---|---|---|
| A | same cert, **own** identifier (`probe_a`) | ❌ BLOCKED — interactive keychain prompt; probe killed at 15s timeout |
| B | same cert **+ identifier `ai.air-agent.desktop`** | ✅ silent read, correct 32-byte DEK; first read 4682 ms (one-time ACL evaluation), subsequent reads 11–13 ms (3/3 stable) |

**GATE VERDICT: PASS.** Approach B stands. **Condition for Task 8:** `bossclawd` must be
signed with `--identifier ai.air-agent.desktop` **and** the same signing identity as the
app — Tauri `externalBin` default signing (per-binary identifier) reproduces Config A and
fails. The bundle/sign step needs an explicit identifier override for the daemon binary.

Notes for later tasks:
- **Same-identifier co-signing makes ownership symmetric:** any item minted by either
  binary carries the same designated requirement, so app and daemon can both read it
  silently. Teardown/re-mint from the daemon (post-M1a single key-holder) needs no ACL
  migration.
- The one-time ~4.7 s first-read latency happens at daemon boot only; harmless.
- `security find-identity -v` reports the self-signed cert as `CSSMERR_TP_NOT_TRUSTED`
  (0 "valid" identities) — cosmetic; `codesign` signs with it fine and the ACL matches.
- A future switch to a real Developer ID changes the certificate leaf → the existing
  item's ACL no longer matches → one interactive re-grant (or a migration step). Flag in
  the release plan when that happens.

---

## Task 0.1 — Engine method surface

Callers: `commands/engine.rs` (webview ops), `commands/identity.rs` (teardown),
`engine/scheduler.rs` (background ticks). No other engine callers outside `engine/`.

### Called from Tauri commands (need wire ops)

| Engine method (`engine/mod.rs`) | Mutates | Wrapping command (`commands/engine.rs`) |
|---|---|---|
| `status` :324 | no | `engine_status` :106 |
| `add_grant` :343 | yes | `engine_add_grant` :112 |
| `revoke_grant` :353 | yes | `engine_revoke_grant` :118 |
| `set_folder_writable` :365 | yes | `engine_set_folder_writable` :124 |
| `list_writable` :377 | no | `engine_list_writable` :130 (+ inside `engine_list_files` :154) |
| `list_grants` :388 | no | `engine_list_grants` :136 |
| `run_ingest` :400 | yes | `engine_run_ingest` :143 |
| `recall` :474 | no | `engine_recall` :187 |
| `evolve_once` :509 | yes | `engine_evolve_now` :448 (also scheduler tick) |
| `evolve_status` :584 | no | `engine_evolve_status` :341 |
| `set_evolve_enabled` :619 | yes | `engine_set_evolve_enabled` :358 |
| `set_proposals_enabled` :630 | yes | `engine_set_proposals_enabled` :368 |
| `set_mandates_enabled` :642 | yes | `engine_set_mandates_enabled` :375 |
| `mandates_enabled` :655 | no | `engine_mandates_enabled` :382 |
| `add_mandate` :667 | yes | `engine_add_mandate` :406 |
| `revoke_mandate` :689 | yes | `engine_revoke_mandate` :414 |
| `list_mandates` :699 | no | `engine_list_mandates` :420 |
| `mandate_writes` :710 | no | `engine_mandate_writes` :440 |
| `list_files` :721 | no | `engine_list_files` :150 |
| `list_proposals` :731 | no | `engine_list_proposals` :274 |
| `proposal_preview` :744 | no | `engine_proposal_preview` :302 |
| `teardown` :779 | yes | identity-reset flow, `commands/identity.rs:223` |
| `apply_proposal` :809 | yes | `engine_apply_proposal` :319 |
| `undo_apply` :898 | yes | `engine_undo_apply` :332 |
| `decline_proposal` :908 | yes | `engine_decline_proposal` :326 |
| `reasoner_config_or_default` :982 | no | `engine_get_reasoner_config` :543 |
| `reasoner_ready_or_false` :1015 | no | `engine_get_reasoner_config` :543 |
| `set_reasoner_config` :1036 | yes | `engine_set_reasoner_config` :567 |
| `enable_cloud_reasoner` :1050 | yes | `engine_enable_cloud_reasoner` :585 |

Mutating ops keep the engine's `try_lock` → `Busy` semantics behind the daemon dispatch
(plan Task 4).

### Scheduler-only (become daemon-internal — no wire op needed)

`evolve_enabled_or_false` :601 · `queue_depth_or_zero` :611 ·
`mandate_autoapply_sweep` :930 · `mandates_enabled_or_false` :965.
(`evolve_once` is both a scheduler tick and a command — keeps a wire op.)

### Engine-internal

`get_or_open` :290 (DEK unlock + `EventLog::open`) — moves wholesale into the daemon.

### Commands that never touch the engine (stay app-side, no wire op)

`engine_pick_folder` :159 / `engine_pick_file` :174 (native dialogs) ·
`engine_ollama_status` :473 (direct localhost HTTP probe; the *scheduler's* Ollama gate
probe moves into the daemon, but this UI status probe can stay in the app).

## Task 0.2 — Boundary types crossing the wire

**Confirmed: no `bossclaw-core` boundary type derives `Serialize`** → every one needs a
proto mirror + `From`/`Into` conversions (plan Task 2). Existing DTOs in
`commands/engine.rs` are the field source of truth.

| Core type | Location | Derives | Mirror field source |
|---|---|---|---|
| `Grant` | `graph.rs:417` | Debug, Clone, PartialEq, Eq | `GrantDto` :7 |
| `Mandate` | `graph.rs:497` | Debug, Clone, PartialEq, Eq | `MandateDto` :388 |
| `FileRecord` | `graph.rs:563` | Debug, Clone, PartialEq, Eq | `FileRecordDto` :19 |
| `IngestReport` | `ingest.rs:226` | Debug, Default, PartialEq, Eq — **no Clone** (conversions must consume) | `IngestReportDto` :49 + `SkipDto` :43 |
| `EvolveReport` | `evolve.rs:32` | Debug, Clone, PartialEq, Eq, Default | `EvolveReportDto` :234 |
| `EvolveStatus` | `evolve.rs:70` | Debug, Clone, PartialEq, Eq | `EvolveStatusDto` :206 |
| `Hit` | `recall.rs:35` | Debug, Clone — **no PartialEq** (mirror round-trip test asserts field-wise) | `HitDto` :76 |
| `PendingProposal` | `log.rs:394` | Debug, Clone, PartialEq, Eq | `ProposalDto` :254 |
| `MandateWriteRecord` | `log.rs:441` | Debug, Clone, PartialEq, Eq | `MandateWriteDto` :427 |
| `WriteOp` (enum) | `actuator.rs:21` | Debug, Clone, Copy, PartialEq, Eq | (variant names) |

Desktop-crate types the `Engine` surface returns (also not `Serialize` unless noted) —
these travel as proto types too, since their producers move daemon-side:

| Type | Location | Derives |
|---|---|---|
| `ProposalSummary` | `engine/mod.rs:121` | Debug, Clone |
| `MandateSummary` | `engine/mod.rs:154` | Debug, Clone |
| `MandateWriteSummary` | `engine/mod.rs:165` | Debug, Clone |
| `PreviewData` | `engine/mod.rs:195` | Debug, Clone |
| `EngineStatus` | `engine/mod.rs:218` | Debug, Clone, **Serialize** (already wire-ready) |
| `EvolveTelemetry` | `engine/mod.rs:236` | Default, Clone — written by `record_tick` on evolve ticks → **moves daemon-side**, surfaces via `evolve_status` |
| `ApplyResult` | `engine/mod.rs:790` | Debug, Clone |
| `ReasonerConfig` | `engine/reason.rs:109` | Debug, Clone (source: `ReasonerConfigDto` :532; `set_reasoner_config`/`enable_cloud_reasoner` inputs are already `serde_json::Value`) |

## Task 0.3 — What moves from the app into `bossclawd`

| Concern | Today (app) | Daemon note |
|---|---|---|
| DEK/keystore open | `EngineKeystore` (`engine/keystore.rs`), `get_or_open` (`engine/mod.rs:290`), `MacosVault` over `keyring 2.3` (`secrets/macos.rs`) | daemon builds its own vault; keychain read proven silent under the Task 0.5 signing condition |
| Embedder | `ResourceModel2Vec` + model dir `<resource_dir>/resources/models/potion-base-8M` (`main.rs:78-82`) | model path from `BOSSCLAWD_MODEL_DIR` env / install path — **no Tauri `resource_dir`** |
| Reasoner cell | `reasoner_cfg` cell (`main.rs:69-72`) + `ConfigReasonerProvider` (`main.rs:83-90`) + boot reseed `reseed_reasoner_cell` (`main.rs:100-106`) | whole cluster moves; reseed happens at daemon boot |
| Evolve scheduler | `scheduler::spawn` (`main.rs:111`) + its Ollama gate probe | daemon spawns it; app stops spawning |
